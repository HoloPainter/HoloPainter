use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    gpu::frame::GpuFrame,
    transient::{TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures},
};

const FILL_TRANSIENT_NAMESPACE: &str = "fill_transient";
const MATERIAL_SOURCE_UV_NAME: &str = "material_source_uv";
const MATERIAL_COVERAGE_UV_NAME: &str = "material_coverage_uv";

pub(crate) fn ensure_material_source_uv(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    material_index: usize,
    tex_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        material_source_uv_key(material_index),
        tex_size,
        wgpu::TextureFormat::Rgba8Unorm,
        source_usage(),
        "fill_source_uv_tex",
        "clear_new_fill_source_uv",
    );
}

pub(crate) fn ensure_material_coverage_uv(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    material_index: usize,
    tex_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        material_coverage_uv_key(material_index),
        tex_size,
        wgpu::TextureFormat::R8Unorm,
        mask_usage(),
        "fill_coverage_uv_tex",
        "clear_new_fill_coverage_uv",
    );
}

pub(crate) fn material_source_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_source_uv_key(material_index))
}

pub(crate) fn material_source_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_source_uv_key(material_index))
}

pub(crate) fn material_coverage_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_coverage_uv_key(material_index))
}

pub(crate) fn material_coverage_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_coverage_uv_key(material_index))
}

pub(crate) fn retain_material_textures_until_submit(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    material_index: usize,
) {
    scratch.retain_transient_texture_until_submit(frame, &material_source_uv_key(material_index));
    scratch.retain_transient_texture_until_submit(frame, &material_coverage_uv_key(material_index));
}

fn material_source_uv_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        FILL_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_SOURCE_UV_NAME,
        TransientLifetime::Manual,
    )
}

fn material_coverage_uv_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        FILL_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_COVERAGE_UV_NAME,
        TransientLifetime::Manual,
    )
}

fn source_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::STORAGE_BINDING
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
}

fn mask_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_source_supports_uv_island_bleed_storage_output() {
        assert!(source_usage().contains(wgpu::TextureUsages::STORAGE_BINDING));
    }
}
