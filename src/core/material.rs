use std::collections::HashSet;

use anyhow::{Result, anyhow, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialIndex(pub usize);

impl MaterialIndex {
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

impl From<usize> for MaterialIndex {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<MaterialIndex> for usize {
    fn from(value: MaterialIndex) -> Self {
        value.0
    }
}

impl std::fmt::Display for MaterialIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl TryFrom<MaterialIndex> for u32 {
    type Error = <u32 as TryFrom<usize>>::Error;

    fn try_from(value: MaterialIndex) -> Result<Self, Self::Error> {
        Self::try_from(value.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaterialUiColor {
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialRenderMode {
    Opaque,
    Cutoff,
    Blend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaterialRenderSettings {
    pub double_sided: bool,
    pub render_mode: MaterialRenderMode,
    pub alpha_cutoff: u8,
}

impl Default for MaterialRenderSettings {
    fn default() -> Self {
        Self {
            double_sided: true,
            render_mode: MaterialRenderMode::Opaque,
            alpha_cutoff: 128,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialSpec {
    pub name: String,
    pub texture_size: [u32; 2],
    pub render_settings: MaterialRenderSettings,
    pub export_image_file_name: Option<String>,
    pub export_psd_file_name: Option<String>,
}

impl MaterialSpec {
    pub fn new(name: impl Into<String>, texture_size: [u32; 2]) -> Self {
        let texture_size = [texture_size[0].max(1), texture_size[1].max(1)];
        Self {
            name: name.into(),
            texture_size,
            render_settings: MaterialRenderSettings::default(),
            export_image_file_name: None,
            export_psd_file_name: None,
        }
    }

    pub fn with_render_settings(mut self, render_settings: MaterialRenderSettings) -> Self {
        self.render_settings = render_settings;
        self
    }

    pub fn with_export_image_file_name(mut self, file_name: Option<String>) -> Self {
        self.export_image_file_name = file_name;
        self
    }

    pub fn with_export_psd_file_name(mut self, file_name: Option<String>) -> Self {
        self.export_psd_file_name = file_name;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialData {
    pub id: MaterialId,
    pub name: String,
    pub texture_size: [u32; 2],
    pub ui_color: MaterialUiColor,
    pub render_settings: MaterialRenderSettings,
    pub export_image_file_name: Option<String>,
    pub export_psd_file_name: Option<String>,
}

pub(crate) fn materialize_materials(materials: Vec<MaterialSpec>) -> (Vec<MaterialData>, u64) {
    let mut materialized = Vec::with_capacity(materials.len());
    let mut used_colors = HashSet::with_capacity(materials.len());
    let mut material_id_high_water = 0_u64;

    for material in materials {
        material_id_high_water = material_id_high_water
            .checked_add(1)
            .expect("material id space exhausted");
        let id = MaterialId(material_id_high_water);
        let ui_color = allocate_material_ui_color(id, &used_colors);
        used_colors.insert(ui_color);
        materialized.push(MaterialData {
            id,
            name: material.name,
            texture_size: material.texture_size,
            ui_color,
            render_settings: material.render_settings,
            export_image_file_name: material.export_image_file_name,
            export_psd_file_name: material.export_psd_file_name,
        });
    }

    (materialized, material_id_high_water)
}

pub(crate) fn append_materials(
    existing: &[MaterialData],
    materials: Vec<MaterialSpec>,
    id_high_water: u64,
) -> Result<(Vec<MaterialData>, u64)> {
    let mut appended = Vec::with_capacity(materials.len());
    let mut used_colors = existing
        .iter()
        .map(|material| material.ui_color)
        .collect::<HashSet<_>>();
    let mut next_id = existing
        .iter()
        .map(|material| material.id.0)
        .max()
        .unwrap_or(0)
        .max(id_high_water);

    for material in materials {
        ensure!(
            material.texture_size[0] > 0 && material.texture_size[1] > 0,
            "material texture size must be > 0"
        );
        next_id = next_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("material id space exhausted"))?;
        let id = MaterialId(next_id);
        let ui_color = allocate_material_ui_color(id, &used_colors);
        used_colors.insert(ui_color);
        appended.push(MaterialData {
            id,
            name: material.name,
            texture_size: material.texture_size,
            ui_color,
            render_settings: material.render_settings,
            export_image_file_name: material.export_image_file_name,
            export_psd_file_name: material.export_psd_file_name,
        });
    }

    Ok((appended, next_id))
}

pub(crate) fn restore_materials(
    materials: Vec<(MaterialId, MaterialSpec)>,
    id_high_water: u64,
) -> (Vec<MaterialData>, u64) {
    let mut restored = Vec::with_capacity(materials.len());
    let mut used_colors = HashSet::with_capacity(materials.len());
    let mut maximum_id = 0;
    for (id, material) in materials {
        maximum_id = maximum_id.max(id.0);
        let ui_color = allocate_material_ui_color(id, &used_colors);
        used_colors.insert(ui_color);
        restored.push(MaterialData {
            id,
            name: material.name,
            texture_size: material.texture_size,
            ui_color,
            render_settings: material.render_settings,
            export_image_file_name: material.export_image_file_name,
            export_psd_file_name: material.export_psd_file_name,
        });
    }
    (restored, id_high_water.max(maximum_id))
}

fn allocate_material_ui_color(
    material_id: MaterialId,
    used_colors: &HashSet<MaterialUiColor>,
) -> MaterialUiColor {
    const PALETTE: [[u8; 3]; 20] = [
        [230, 79, 70],
        [67, 126, 219],
        [66, 171, 93],
        [217, 145, 43],
        [154, 96, 203],
        [37, 168, 166],
        [211, 91, 151],
        [140, 116, 78],
        [102, 144, 59],
        [82, 107, 179],
        [221, 111, 46],
        [74, 162, 208],
        [192, 75, 101],
        [123, 151, 212],
        [109, 169, 132],
        [183, 112, 191],
        [203, 164, 61],
        [64, 151, 138],
        [201, 103, 82],
        [118, 120, 193],
    ];

    let start = material_id.0.saturating_sub(1) as usize % PALETTE.len();
    for offset in 0..PALETTE.len() {
        let color = MaterialUiColor {
            rgb: PALETTE[(start + offset) % PALETTE.len()],
        };
        if !used_colors.contains(&color) {
            return color;
        }
    }

    let mut candidate = material_id.0 ^ 0x9e37_79b9_7f4a_7c15;
    loop {
        candidate ^= candidate >> 12;
        candidate ^= candidate << 25;
        candidate ^= candidate >> 27;
        let mixed = candidate.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let color = MaterialUiColor {
            rgb: [
                64 + (mixed & 0x97) as u8,
                64 + ((mixed >> 8) & 0x97) as u8,
                64 + ((mixed >> 16) & 0x97) as u8,
            ],
        };
        if !used_colors.contains(&color) {
            return color;
        }
        candidate = candidate
            .checked_add(1)
            .expect("material color space exhausted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_settings_default_to_double_sided_opaque_with_midpoint_cutoff() {
        assert_eq!(
            MaterialRenderSettings::default(),
            MaterialRenderSettings {
                double_sided: true,
                render_mode: MaterialRenderMode::Opaque,
                alpha_cutoff: 128,
            }
        );
    }

    #[test]
    fn new_material_has_no_export_file_names() {
        let material = MaterialSpec::new("Material", [8, 8]);
        assert_eq!(material.export_image_file_name, None);
        assert_eq!(material.export_psd_file_name, None);
    }

    #[test]
    fn materializes_unique_nonzero_ids_and_colors() {
        let specs = vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
            MaterialSpec::new("C", [8, 8]),
        ];

        let (materials, material_id_high_water) = materialize_materials(specs);

        assert_eq!(material_id_high_water, 3);
        assert_eq!(materials[0].id, MaterialId(1));
        assert_eq!(materials[1].id, MaterialId(2));
        assert_eq!(materials[2].id, MaterialId(3));
        assert_eq!(
            materials
                .iter()
                .map(|material| material.ui_color)
                .collect::<HashSet<_>>()
                .len(),
            materials.len()
        );
    }

    #[test]
    fn materialization_is_deterministic_for_different_counts() {
        for count in [0, 1, 3, 24] {
            let specs = (0..count)
                .map(|index| MaterialSpec::new(format!("M{index}"), [0, 4]))
                .collect::<Vec<_>>();
            let first = materialize_materials(specs.clone());
            let second = materialize_materials(specs);

            assert_eq!(first, second);
            assert_eq!(first.1, count as u64);
            assert!(
                first
                    .0
                    .iter()
                    .all(|material| material.texture_size == [1, 4])
            );
        }
    }

    #[test]
    fn appended_materials_keep_existing_ids_and_colors_and_allocate_new_ids() {
        let (existing, high_water) = materialize_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);

        let (appended, next_high_water) = append_materials(
            &existing,
            vec![MaterialSpec::new("C", [16, 16])],
            high_water,
        )
        .unwrap();

        assert_eq!(existing[0].id, MaterialId(1));
        assert_eq!(existing[1].id, MaterialId(2));
        assert_eq!(appended[0].id, MaterialId(3));
        assert_eq!(next_high_water, 3);
        assert!(
            existing
                .iter()
                .all(|material| material.ui_color != appended[0].ui_color)
        );
    }
}
