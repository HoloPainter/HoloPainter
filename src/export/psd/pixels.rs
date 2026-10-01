use anyhow::{Result, ensure};

use crate::core::{
    adjustment::{UvMirrorAdjustment, UvMirrorAxis, UvMirrorDirection},
    embedded_image::{EmbeddedImageAsset, EmbeddedImageTransform},
};

pub(super) fn mirrored_straight_rgba8(
    premultiplied_rgba8: &[u8],
    size: [u32; 2],
    mirrors: &[UvMirrorAdjustment],
) -> Result<Vec<u8>> {
    validate_rgba_len(premultiplied_rgba8, size)?;
    let mut output = remap_rgba8(premultiplied_rgba8, size, mirrors);
    unpremultiply_rgba8(&mut output);
    Ok(output)
}

pub(super) fn mirrored_mask_r8(
    mask_rgba8: &[u8],
    size: [u32; 2],
    mirrors: &[UvMirrorAdjustment],
) -> Result<Vec<u8>> {
    validate_rgba_len(mask_rgba8, size)?;
    let pixel_count = pixel_count(size)?;
    let mut output = Vec::with_capacity(pixel_count);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let [source_x, source_y] = mapped_pixel([x, y], size, mirrors);
            let source = ((source_y as usize * size[0] as usize) + source_x as usize) * 4;
            output.push(mask_rgba8[source]);
        }
    }
    Ok(output)
}

pub(super) fn rasterize_embedded_image(
    asset: &EmbeddedImageAsset,
    transform: EmbeddedImageTransform,
    output_size: [u32; 2],
    mirrors: &[UvMirrorAdjustment],
) -> Result<Vec<u8>> {
    ensure!(transform.is_valid(), "embedded image transform is invalid");
    validate_rgba_len(&asset.rgba8, asset.size)?;
    let output_len = pixel_count(output_size)?
        .checked_mul(4)
        .ok_or_else(|| anyhow::anyhow!("embedded image output byte length overflows"))?;
    let mut premultiplied = Vec::with_capacity(output_len);
    let rows = output_uv_to_source_uv_rows(transform);
    for y in 0..output_size[1] {
        for x in 0..output_size[0] {
            let uv = mapped_uv([x, y], output_size, mirrors);
            let source_uv = [
                rows[0][0] * uv[0] + rows[0][1] * uv[1] + rows[0][2],
                rows[1][0] * uv[0] + rows[1][1] * uv[1] + rows[1][2],
            ];
            let sample = sample_premultiplied_bilinear(asset, source_uv);
            premultiplied.extend(sample.map(float_to_u8));
        }
    }
    unpremultiply_rgba8(&mut premultiplied);
    Ok(premultiplied)
}

pub fn unpremultiply_rgba8(rgba8: &mut [u8]) {
    for pixel in rgba8.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[..3].fill(0);
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

fn remap_rgba8(source: &[u8], size: [u32; 2], mirrors: &[UvMirrorAdjustment]) -> Vec<u8> {
    let mut output = Vec::with_capacity(source.len());
    for y in 0..size[1] {
        for x in 0..size[0] {
            let [source_x, source_y] = mapped_pixel([x, y], size, mirrors);
            let offset = ((source_y as usize * size[0] as usize) + source_x as usize) * 4;
            output.extend_from_slice(&source[offset..offset + 4]);
        }
    }
    output
}

fn mapped_pixel(pixel: [u32; 2], size: [u32; 2], mirrors: &[UvMirrorAdjustment]) -> [u32; 2] {
    let uv = mapped_uv(pixel, size, mirrors);
    [
        (uv[0] * size[0] as f32)
            .floor()
            .clamp(0.0, size[0] as f32 - 1.0) as u32,
        (uv[1] * size[1] as f32)
            .floor()
            .clamp(0.0, size[1] as f32 - 1.0) as u32,
    ]
}

fn mapped_uv(pixel: [u32; 2], size: [u32; 2], mirrors: &[UvMirrorAdjustment]) -> [f32; 2] {
    let mut uv = [
        (pixel[0] as f32 + 0.5) / size[0] as f32,
        (pixel[1] as f32 + 0.5) / size[1] as f32,
    ];
    for mirror in mirrors {
        uv = apply_uv_mirror_mapping(uv, size, *mirror);
    }
    uv
}

fn apply_uv_mirror_mapping(
    mut uv: [f32; 2],
    size: [u32; 2],
    mirror: UvMirrorAdjustment,
) -> [f32; 2] {
    let axis = match mirror.axis {
        UvMirrorAxis::X => 0,
        UvMirrorAxis::Y => 1,
    };
    let extent = size[axis].max(1) as f32;
    let position = (mirror.position.clamp(0.0, 1.0) * extent * 2.0).round() / (extent * 2.0);
    let coordinate = uv[axis];
    let source_side = match mirror.direction {
        UvMirrorDirection::PositiveToNegative => coordinate >= position,
        UvMirrorDirection::NegativeToPositive => coordinate <= position,
    };
    if source_side {
        return uv;
    }
    let reflected = 2.0 * position - coordinate;
    if (0.0..=1.0).contains(&reflected) {
        uv[axis] = reflected;
    }
    uv
}

fn output_uv_to_source_uv_rows(transform: EmbeddedImageTransform) -> [[f32; 3]; 2] {
    let sin = transform.rotation_radians.sin();
    let cos = transform.rotation_radians.cos();
    let row0_x = cos / transform.size_uv.x;
    let row0_y = sin / transform.size_uv.x;
    let row1_x = -sin / transform.size_uv.y;
    let row1_y = cos / transform.size_uv.y;
    [
        [
            row0_x,
            row0_y,
            0.5 - row0_x * transform.center_uv.x - row0_y * transform.center_uv.y,
        ],
        [
            row1_x,
            row1_y,
            0.5 - row1_x * transform.center_uv.x - row1_y * transform.center_uv.y,
        ],
    ]
}

fn sample_premultiplied_bilinear(asset: &EmbeddedImageAsset, uv: [f32; 2]) -> [f32; 4] {
    let coordinate = [
        uv[0] * asset.size[0] as f32 - 0.5,
        uv[1] * asset.size[1] as f32 - 0.5,
    ];
    let origin = [coordinate[0].floor() as i32, coordinate[1].floor() as i32];
    let fraction = [
        coordinate[0] - coordinate[0].floor(),
        coordinate[1] - coordinate[1].floor(),
    ];
    let top = mix(
        load_premultiplied(asset, origin),
        load_premultiplied(asset, [origin[0] + 1, origin[1]]),
        fraction[0],
    );
    let bottom = mix(
        load_premultiplied(asset, [origin[0], origin[1] + 1]),
        load_premultiplied(asset, [origin[0] + 1, origin[1] + 1]),
        fraction[0],
    );
    mix(top, bottom, fraction[1])
}

fn load_premultiplied(asset: &EmbeddedImageAsset, pixel: [i32; 2]) -> [f32; 4] {
    if pixel[0] < 0
        || pixel[1] < 0
        || pixel[0] >= asset.size[0] as i32
        || pixel[1] >= asset.size[1] as i32
    {
        return [0.0; 4];
    }
    let offset = ((pixel[1] as usize * asset.size[0] as usize) + pixel[0] as usize) * 4;
    let alpha = asset.rgba8[offset + 3] as f32 / 255.0;
    [
        asset.rgba8[offset] as f32 / 255.0 * alpha,
        asset.rgba8[offset + 1] as f32 / 255.0 * alpha,
        asset.rgba8[offset + 2] as f32 / 255.0 * alpha,
        alpha,
    ]
}

fn mix(a: [f32; 4], b: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|index| a[index] + (b[index] - a[index]) * amount)
}

fn float_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn validate_rgba_len(rgba8: &[u8], size: [u32; 2]) -> Result<()> {
    let expected = pixel_count(size)?
        .checked_mul(4)
        .ok_or_else(|| anyhow::anyhow!("RGBA byte length overflows"))?;
    ensure!(
        rgba8.len() == expected,
        "RGBA byte length does not match dimensions"
    );
    Ok(())
}

fn pixel_count(size: [u32; 2]) -> Result<usize> {
    ensure!(!size.contains(&0), "pixel dimensions must be non-zero");
    usize::try_from(size[0])?
        .checked_mul(usize::try_from(size[1])?)
        .ok_or_else(|| anyhow::anyhow!("pixel count overflows"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn positive_to_negative_x_mirror_copies_right_half_to_left() {
        let rgba = [
            [10, 0, 0, 255],
            [20, 0, 0, 255],
            [30, 0, 0, 255],
            [40, 0, 0, 255],
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let mirrored =
            mirrored_straight_rgba8(&rgba, [4, 1], &[UvMirrorAdjustment::default()]).unwrap();
        assert_eq!(
            mirrored
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![40, 30, 30, 40]
        );
    }

    #[test]
    fn multiple_mirrors_are_composed_before_one_pixel_generation() {
        let rgba = (1_u8..=4)
            .flat_map(|value| [value, 0, 0, 255])
            .collect::<Vec<_>>();
        let mirrors = [
            UvMirrorAdjustment::default(),
            UvMirrorAdjustment {
                direction: UvMirrorDirection::NegativeToPositive,
                ..Default::default()
            },
        ];
        let mirrored = mirrored_straight_rgba8(&rgba, [4, 1], &mirrors).unwrap();
        assert_eq!(mirrored.len(), rgba.len());
        assert_eq!(
            mirrored
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![1, 2, 2, 1]
        );
    }

    #[test]
    fn unpremultiply_clears_rgb_for_zero_alpha_and_restores_translucent_color() {
        let mut rgba = vec![99, 88, 77, 0, 64, 32, 16, 128];
        unpremultiply_rgba8(&mut rgba);
        assert_eq!(rgba, vec![0, 0, 0, 0, 128, 64, 32, 128]);
    }

    #[test]
    fn embedded_image_transform_and_uv_mirror_are_baked_into_one_rasterization() {
        let rgba8 = (1_u8..=4)
            .flat_map(|value| [value * 10, 0, 0, 255])
            .collect::<Vec<_>>();
        let asset = EmbeddedImageAsset::new(
            crate::core::embedded_image::EmbeddedImageId(1),
            "embedded.png".to_owned(),
            [4, 1],
            Arc::from(rgba8),
        )
        .unwrap();
        let transform = EmbeddedImageTransform::initial([4, 1], [4, 1]).unwrap();

        let rasterized =
            rasterize_embedded_image(&asset, transform, [4, 1], &[UvMirrorAdjustment::default()])
                .unwrap();

        assert_eq!(
            rasterized
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![40, 30, 30, 40]
        );
    }
}
