use anyhow::{Result, ensure};
use image::{GrayImage, ImageBuffer, Luma, RgbaImage, imageops::FilterType};

use crate::core::image::rgba8_len;

const RESIZE_FILTER: FilterType = FilterType::CatmullRom;

pub fn resize_rgba8(
    source_size: [u32; 2],
    destination_size: [u32; 2],
    rgba8: &[u8],
) -> Result<Vec<u8>> {
    validate_size(source_size, "source")?;
    validate_size(destination_size, "destination")?;
    ensure!(
        rgba8.len() == rgba8_len(source_size)?,
        "RGBA resize source payload size mismatch"
    );
    if source_size == destination_size {
        return Ok(rgba8.to_vec());
    }

    let mut premultiplied = rgba8.to_vec();
    for pixel in premultiplied.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let source = RgbaImage::from_raw(source_size[0], source_size[1], premultiplied)
        .expect("validated RGBA payload must match source dimensions");
    let resized = image::imageops::resize(
        &source,
        destination_size[0],
        destination_size[1],
        RESIZE_FILTER,
    );
    let mut result = resized.into_raw();
    for pixel in result.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 {
            pixel[..3].fill(0);
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = (((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255)) as u8;
        }
    }
    Ok(result)
}

pub fn resize_mask_rgba8(
    source_size: [u32; 2],
    destination_size: [u32; 2],
    rgba8: &[u8],
) -> Result<Vec<u8>> {
    ensure!(
        rgba8.len() == rgba8_len(source_size)?,
        "mask RGBA resize source payload size mismatch"
    );
    let source = rgba8
        .chunks_exact(4)
        .map(|pixel| pixel[3])
        .collect::<Vec<_>>();
    let resized = resize_r8(source_size, destination_size, &source)?;
    let mut result = Vec::with_capacity(resized.len().saturating_mul(4));
    for value in resized {
        result.extend_from_slice(&[value; 4]);
    }
    Ok(result)
}

pub fn resize_r8(source_size: [u32; 2], destination_size: [u32; 2], r8: &[u8]) -> Result<Vec<u8>> {
    validate_size(source_size, "source")?;
    validate_size(destination_size, "destination")?;
    let expected_len = (source_size[0] as usize)
        .checked_mul(source_size[1] as usize)
        .ok_or_else(|| anyhow::anyhow!("R8 resize source dimensions overflow"))?;
    ensure!(
        r8.len() == expected_len,
        "R8 resize source payload size mismatch"
    );
    if source_size == destination_size {
        return Ok(r8.to_vec());
    }
    let source: GrayImage =
        ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(source_size[0], source_size[1], r8.to_vec())
            .expect("validated R8 payload must match source dimensions");
    Ok(image::imageops::resize(
        &source,
        destination_size[0],
        destination_size[1],
        RESIZE_FILTER,
    )
    .into_raw())
}

fn validate_size(size: [u32; 2], label: &str) -> Result<()> {
    ensure!(
        size[0] > 0 && size[1] > 0,
        "{label} resize dimensions must be > 0"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_resize_preserves_dimensions_and_opaque_color() {
        let source = [200, 20, 10, 255].repeat(4);
        let resized = resize_rgba8([2, 2], [4, 4], &source).unwrap();
        assert_eq!(resized.len(), 4 * 4 * 4);
        assert!(
            resized
                .chunks_exact(4)
                .all(|pixel| pixel == [200, 20, 10, 255])
        );
    }

    #[test]
    fn transparent_rgba_does_not_introduce_rgb_fringe() {
        let source = vec![255, 0, 0, 255, 0, 0, 0, 0];
        let resized = resize_rgba8([2, 1], [8, 1], &source).unwrap();
        for pixel in resized.chunks_exact(4) {
            if pixel[3] == 0 {
                assert_eq!(&pixel[..3], &[0, 0, 0]);
            }
        }
    }

    #[test]
    fn mask_resize_replicates_scalar_to_rgba() {
        let source = vec![0, 0, 0, 0, 255, 255, 255, 255];
        let resized = resize_mask_rgba8([2, 1], [4, 1], &source).unwrap();
        assert_eq!(resized.len(), 16);
        assert!(
            resized
                .chunks_exact(4)
                .all(|pixel| pixel.iter().all(|value| *value == pixel[0]))
        );
    }

    #[test]
    fn r8_resize_preserves_same_size_exactly() {
        let source = vec![0, 64, 128, 255];
        assert_eq!(resize_r8([2, 2], [2, 2], &source).unwrap(), source);
    }
}
