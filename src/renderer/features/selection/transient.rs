use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    gpu::frame::GpuFrame,
    transient::{TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures},
};

const SELECTION_TRANSIENT_NAMESPACE: &str = "selection_transient";
const MATERIAL_SOURCE_NAME: &str = "selection_source_uv";

pub(crate) fn ensure_material_source_uv(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    material_index: usize,
    texture_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        material_source_key(material_index),
        texture_size,
        wgpu::TextureFormat::R8Unorm,
        selection_source_usage(),
        "selection_source_uv_tex",
        "clear_new_selection_source_uv",
    );
}

pub(crate) fn material_source_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_source_key(material_index))
}

pub(crate) fn material_source_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_source_key(material_index))
}

pub(crate) fn release_material_source_uv_after_submit(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    material_index: usize,
) {
    scratch.retain_transient_texture_until_submit(frame, &material_source_key(material_index));
}

fn material_source_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        SELECTION_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_SOURCE_NAME,
        TransientLifetime::Submission,
    )
}

fn selection_source_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
}
