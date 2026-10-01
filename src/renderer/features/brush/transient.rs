use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    gpu::frame::GpuFrame,
    transient::{TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures},
};

const STROKE_TRANSIENT_NAMESPACE: &str = "stroke_transient";
const MATERIAL_SOURCE_UV_NAME: &str = "material_source_uv";
const MATERIAL_BATCH_SOURCE_UV_NAME: &str = "material_batch_source_uv";
const MATERIAL_STROKE_UV_NAME: &str = "material_stroke_uv";

pub(crate) fn ensure_material_stroke_uv(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    material_index: usize,
    tex_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        material_stroke_uv_key(material_index),
        tex_size,
        wgpu::TextureFormat::R8Unorm,
        material_stroke_usage(),
        "normal_stroke_uv_tex",
        "clear_new_normal_stroke_uv",
    );
}

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
        material_source_usage(),
        "stroke_source_uv_tex",
        "clear_new_stroke_source_uv",
    );
}

pub(crate) fn ensure_material_batch_source_uv(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    material_index: usize,
    tex_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        material_batch_source_uv_key(material_index),
        tex_size,
        wgpu::TextureFormat::Rgba8Unorm,
        material_source_usage(),
        "stroke_batch_source_uv_tex",
        "clear_new_stroke_batch_source_uv",
    );
}

pub(crate) fn release_material_source_uv_after_submit(
    scratch: &mut TransientTextures,
    _frame: &mut GpuFrame,
    material_index: usize,
) {
    scratch.release_transient_texture_to_idle(&material_source_uv_key(material_index));
}

pub(crate) fn release_material_transient_uv_after_submit(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    material_index: usize,
) {
    release_material_source_uv_after_submit(scratch, frame, material_index);
}

pub(crate) fn material_stroke_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_stroke_uv_key(material_index))
}

pub(crate) fn material_source_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_source_uv_key(material_index))
}

pub(crate) fn material_source_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_source_uv_key(material_index))
}

pub(crate) fn material_batch_source_uv_view(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&material_batch_source_uv_key(material_index))
}

pub(crate) fn material_batch_source_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_batch_source_uv_key(material_index))
}

pub(crate) fn material_stroke_uv_texture(
    scratch: &TransientTextures,
    material_index: usize,
) -> Option<&wgpu::Texture> {
    scratch.transient_texture(&material_stroke_uv_key(material_index))
}

fn material_source_uv_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        STROKE_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_SOURCE_UV_NAME,
        TransientLifetime::Submission,
    )
}

fn material_batch_source_uv_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        STROKE_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_BATCH_SOURCE_UV_NAME,
        TransientLifetime::Submission,
    )
}

fn material_stroke_uv_key(material_index: usize) -> TransientTextureKey {
    TransientTextureKey::new(
        STROKE_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        MATERIAL_STROKE_UV_NAME,
        TransientLifetime::Manual,
    )
}

fn default_scratch_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
}

fn material_source_usage() -> wgpu::TextureUsages {
    default_scratch_usage() | wgpu::TextureUsages::STORAGE_BINDING
}

fn material_stroke_usage() -> wgpu::TextureUsages {
    default_scratch_usage()
}
