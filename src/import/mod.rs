use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result, bail, ensure};

use crate::{core::document::MeshData, core::material::MaterialRenderSettings};

mod fbx;
mod gltf;

pub(crate) fn load_builtin_cube() -> Result<ImportedAsset> {
    let bytes = crate::embedded_resources::bytes("models/cube.gltf")
        .context("embedded Cube model is missing")?;
    gltf::load_bytes(bytes).context("loading embedded Cube model")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFormat {
    Gltf,
    Fbx,
}

pub const SUPPORTED_MODEL_EXTENSIONS: &[&str] = &["gltf", "glb", "fbx"];

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMaterial {
    pub source_material_index: Option<usize>,
    pub name: String,
    pub render_settings: MaterialRenderSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedTexture {
    pub size: [u32; 2],
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ImportedAsset {
    pub mesh: MeshData,
    pub materials: Vec<ImportedMaterial>,
}

impl ModelFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .context("model file has no supported extension")?;
        match extension.as_str() {
            "glb" | "gltf" => Ok(Self::Gltf),
            "fbx" => Ok(Self::Fbx),
            _ => bail!("unsupported model format: .{extension}"),
        }
    }

    pub fn supports_base_color_texture_import(self) -> bool {
        matches!(self, Self::Gltf | Self::Fbx)
    }
}

pub fn load_model(path: &Path) -> Result<ImportedAsset> {
    match ModelFormat::from_path(path)? {
        ModelFormat::Gltf => gltf::load(path),
        ModelFormat::Fbx => fbx::load(path),
    }
}

fn build_wireframe_edges(triangles: &[[u32; 3]]) -> Vec<[u32; 2]> {
    let mut edges = HashSet::new();
    for &[i0, i1, i2] in triangles {
        for mut edge in [[i0, i1], [i1, i2], [i2, i0]] {
            edge.sort_unstable();
            edges.insert(edge);
        }
    }
    let mut edges = edges.into_iter().collect::<Vec<_>>();
    edges.sort_unstable();
    edges
}

fn decode_imported_texture(encoded: &[u8]) -> Result<ImportedTexture> {
    let format = image::guess_format(encoded).context("unsupported or invalid image encoding")?;
    ensure!(
        matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg),
        "unsupported image format: {format:?}; only PNG and JPEG are supported"
    );
    let decoded = image::load_from_memory_with_format(encoded, format)
        .context("failed to decode PNG/JPEG image")?;
    let rgba = decoded.into_rgba8();
    Ok(ImportedTexture {
        size: [rgba.width(), rgba.height()],
        rgba8: rgba.into_raw(),
    })
}

pub fn load_base_color_textures(
    path: &Path,
    materials: &[ImportedMaterial],
) -> Result<Vec<Option<ImportedTexture>>> {
    match ModelFormat::from_path(path)? {
        ModelFormat::Gltf => gltf::load_base_color_textures(path, materials),
        ModelFormat::Fbx => fbx::load_base_color_textures(path, materials),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ModelFormat, load_model};

    #[test]
    fn model_format_dispatches_supported_extensions_case_insensitively() {
        assert_eq!(
            ModelFormat::from_path(Path::new("mesh.gltf")).unwrap(),
            ModelFormat::Gltf
        );
        assert_eq!(
            ModelFormat::from_path(Path::new("mesh.GLB")).unwrap(),
            ModelFormat::Gltf
        );
        assert_eq!(
            ModelFormat::from_path(Path::new("mesh.FBX")).unwrap(),
            ModelFormat::Fbx
        );
    }

    #[test]
    fn model_format_rejects_unsupported_or_missing_extensions() {
        assert_eq!(
            load_model(Path::new("mesh.obj")).unwrap_err().to_string(),
            "unsupported model format: .obj"
        );
        assert_eq!(
            load_model(Path::new("mesh")).unwrap_err().to_string(),
            "model file has no supported extension"
        );
    }

    #[test]
    fn texture_import_capability_includes_all_supported_formats() {
        assert!(ModelFormat::Gltf.supports_base_color_texture_import());
        assert!(ModelFormat::Fbx.supports_base_color_texture_import());
    }
}
