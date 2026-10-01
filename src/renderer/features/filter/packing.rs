use anyhow::{Result, anyhow, ensure};

use crate::core::surface::PaintSurfaceId;

pub(super) const MAX_FILTER_WORKING_SET_BYTES: u64 = 768 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AtlasPlacement {
    pub(super) material_index: usize,
    pub(super) origin: [u32; 2],
    pub(super) size: [u32; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilterAtlasLayout {
    pub(super) size: [u32; 2],
    pub(super) max_material_size: [u32; 2],
    pub(super) placements: Vec<AtlasPlacement>,
    pub(super) base_working_set_bytes: u64,
}

pub(super) fn pack_filter_materials(
    material_sizes: &[[u32; 2]],
    active_materials: &[bool],
    max_texture_dimension_2d: u32,
) -> Result<FilterAtlasLayout> {
    ensure!(
        material_sizes.len() == active_materials.len(),
        "filter material size and active-mask counts differ"
    );
    ensure!(
        max_texture_dimension_2d > 0,
        "filter device texture limit is zero"
    );

    let mut entries = material_sizes
        .iter()
        .copied()
        .zip(active_materials.iter().copied())
        .enumerate()
        .filter_map(|(material_index, (size, active))| {
            active.then_some(AtlasPlacement {
                material_index,
                origin: [0, 0],
                size,
            })
        })
        .collect::<Vec<_>>();
    ensure!(!entries.is_empty(), "filter has no active materials");

    for entry in &entries {
        ensure!(
            entry.size[0] > 0 && entry.size[1] > 0,
            "filter material {} has an empty texture",
            entry.material_index
        );
        ensure!(
            entry.size[0] <= max_texture_dimension_2d && entry.size[1] <= max_texture_dimension_2d,
            "filter material {} size {}x{} exceeds the device texture limit {}",
            entry.material_index,
            entry.size[0],
            entry.size[1],
            max_texture_dimension_2d
        );
    }

    entries.sort_by(|a, b| {
        b.size[1]
            .cmp(&a.size[1])
            .then_with(|| b.size[0].cmp(&a.size[0]))
            .then_with(|| a.material_index.cmp(&b.material_index))
    });

    let max_width = entries.iter().map(|entry| entry.size[0]).max().unwrap_or(1);
    let total_area = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(u64::from(entry.size[0]) * u64::from(entry.size[1]))
            .ok_or_else(|| anyhow!("filter atlas area overflow"))
    })?;
    let target_width = (total_area as f64).sqrt().ceil() as u32;
    let mut candidate_widths = vec![
        max_width,
        target_width.clamp(max_width, max_texture_dimension_2d),
    ];
    let mut power_of_two = max_width.next_power_of_two();
    while power_of_two < max_texture_dimension_2d {
        candidate_widths.push(power_of_two);
        let next = power_of_two.saturating_mul(2);
        if next <= power_of_two {
            break;
        }
        power_of_two = next;
    }
    candidate_widths.push(max_texture_dimension_2d);
    candidate_widths.sort_unstable();
    candidate_widths.dedup();

    let mut best: Option<([u32; 2], Vec<AtlasPlacement>)> = None;
    for width in candidate_widths {
        let Some((height, placements)) = shelf_pack(&entries, width, max_texture_dimension_2d)
        else {
            continue;
        };
        let area = u64::from(width) * u64::from(height);
        let replace = best.as_ref().is_none_or(|(best_size, _)| {
            area < u64::from(best_size[0]) * u64::from(best_size[1])
                || (area == u64::from(best_size[0]) * u64::from(best_size[1])
                    && height < best_size[1])
        });
        if replace {
            best = Some(([width, height], placements));
        }
    }

    let Some((size, mut placements)) = best else {
        return Err(anyhow!(
            "filter source atlas cannot fit within the device texture limit {}",
            max_texture_dimension_2d
        ));
    };
    placements.sort_by_key(|placement| placement.material_index);
    let max_material_size = placements.iter().fold([1, 1], |maximum, placement| {
        [
            maximum[0].max(placement.size[0]),
            maximum[1].max(placement.size[1]),
        ]
    });
    let atlas_pixels = u64::from(size[0]) * u64::from(size[1]);
    let scratch_pixels = u64::from(max_material_size[0]) * u64::from(max_material_size[1]);
    let base_working_set_bytes = atlas_pixels
        .checked_add(scratch_pixels.saturating_mul(2))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("filter working-set estimate overflow"))?;

    Ok(FilterAtlasLayout {
        size,
        max_material_size,
        placements,
        base_working_set_bytes,
    })
}

pub(crate) fn validate_adjustment_preview_working_set(
    material_sizes: &[[u32; 2]],
    surfaces: &[PaintSurfaceId],
) -> Result<u64> {
    ensure!(
        !surfaces.is_empty(),
        "adjustment filter preview has no target surfaces"
    );
    let mut base_bytes = 0u64;
    let mut max_pixels = 0u64;
    for surface in surfaces {
        let size = *material_sizes
            .get(surface.material_index().as_usize())
            .ok_or_else(|| {
                anyhow!(
                    "adjustment filter preview material index {} is out of range",
                    surface.material_index
                )
            })?;
        let pixels = u64::from(size[0])
            .checked_mul(u64::from(size[1]))
            .ok_or_else(|| anyhow!("adjustment filter preview size overflow"))?;
        base_bytes = base_bytes
            .checked_add(
                pixels
                    .checked_mul(4)
                    .ok_or_else(|| anyhow!("adjustment filter preview working-set overflow"))?,
            )
            .ok_or_else(|| anyhow!("adjustment filter preview working-set overflow"))?;
        max_pixels = max_pixels.max(pixels);
    }
    let scratch_bytes = max_pixels
        .checked_mul(8)
        .ok_or_else(|| anyhow!("adjustment filter preview working-set overflow"))?;
    validate_filter_working_set("Adjustment Filter Preview", base_bytes, scratch_bytes)
}

pub(super) fn validate_filter_working_set(
    filter_name: &str,
    base_working_set_bytes: u64,
    additional_bytes: u64,
) -> Result<u64> {
    let estimated = base_working_set_bytes
        .checked_add(additional_bytes)
        .ok_or_else(|| anyhow!("{filter_name} working-set estimate overflow"))?;
    ensure!(
        estimated <= MAX_FILTER_WORKING_SET_BYTES,
        "{filter_name} requires approximately {} MiB of temporary GPU memory; the limit is {} MiB",
        estimated.div_ceil(1024 * 1024),
        MAX_FILTER_WORKING_SET_BYTES / (1024 * 1024)
    );
    Ok(estimated)
}

fn shelf_pack(
    entries: &[AtlasPlacement],
    width: u32,
    max_height: u32,
) -> Option<(u32, Vec<AtlasPlacement>)> {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_height = 0u32;
    let mut placements = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.size[0] > width {
            return None;
        }
        if x > 0 && x.checked_add(entry.size[0])? > width {
            y = y.checked_add(row_height)?;
            x = 0;
            row_height = 0;
        }
        if y.checked_add(entry.size[1])? > max_height {
            return None;
        }
        placements.push(AtlasPlacement {
            origin: [x, y],
            ..*entry
        });
        x = x.checked_add(entry.size[0])?;
        row_height = row_height.max(entry.size[1]);
    }
    let height = y.checked_add(row_height)?.max(1);
    (height <= max_height).then_some((height, placements))
}

#[cfg(test)]
mod tests {
    use crate::core::surface::PaintSurfaceId;

    use super::{
        MAX_FILTER_WORKING_SET_BYTES, pack_filter_materials,
        validate_adjustment_preview_working_set, validate_filter_working_set,
    };

    #[test]
    fn atlas_packs_only_active_materials_without_max_size_array_waste() {
        let layout = pack_filter_materials(
            &[[8192, 8192], [512, 512], [512, 512]],
            &[false, true, true],
            8192,
        )
        .unwrap();

        assert_eq!(layout.placements.len(), 2);
        assert!(
            layout
                .placements
                .iter()
                .all(|entry| entry.material_index != 0)
        );
        assert!(u64::from(layout.size[0]) * u64::from(layout.size[1]) <= 512 * 1024);
    }

    #[test]
    fn adjustment_preview_working_set_counts_base_surfaces_and_one_shared_scratch() {
        let tree = crate::core::surface::LayerTree::new_default_raster();
        let layer_id = tree.default_raster_layer().expect("default raster");
        let surfaces = [
            PaintSurfaceId::raster(0.into(), layer_id),
            PaintSurfaceId::raster(1.into(), layer_id),
        ];
        let estimated =
            validate_adjustment_preview_working_set(&[[1024, 1024], [512, 512]], &surfaces)
                .unwrap();

        assert_eq!(estimated, 13 * 1024 * 1024);
    }

    #[test]
    fn working_set_rejects_bytes_above_budget() {
        let error = validate_filter_working_set("Spatial Blur", MAX_FILTER_WORKING_SET_BYTES, 1)
            .unwrap_err();

        assert!(error.to_string().contains("temporary GPU memory"));
        assert_eq!(MAX_FILTER_WORKING_SET_BYTES, 768 * 1024 * 1024);
    }
}
