use eframe::egui_wgpu::wgpu;

use crate::{
    core::{brush_engine::TextureResourceLifetime, surface::PaintSurfaceId},
    renderer::{
        gpu::frame::GpuFrame,
        transient::{
            TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures,
        },
    },
};

pub(crate) const BRUSH_SCRATCH_NAMESPACE: &str = "brush_scratch";

pub(crate) fn material_scratch_key(
    target: PaintSurfaceId,
    name: &str,
    lifetime: TextureResourceLifetime,
) -> TransientTextureKey {
    TransientTextureKey::new(
        BRUSH_SCRATCH_NAMESPACE,
        TransientTextureScope::Material(target.material_index().as_usize()),
        name,
        transient_lifetime(lifetime),
    )
}

pub(crate) fn viewport_scratch_key(
    name: &str,
    lifetime: TextureResourceLifetime,
) -> TransientTextureKey {
    TransientTextureKey::new(
        BRUSH_SCRATCH_NAMESPACE,
        TransientTextureScope::Viewport,
        name,
        transient_lifetime(lifetime),
    )
}

pub(crate) fn material_scratch_view<'a>(
    scratch: &'a TransientTextures,
    target: PaintSurfaceId,
    name: &str,
    lifetime: TextureResourceLifetime,
) -> Option<&'a wgpu::TextureView> {
    scratch.transient_texture_view(&material_scratch_key(target, name, lifetime))
}

pub(crate) fn viewport_scratch_view<'a>(
    scratch: &'a TransientTextures,
    name: &str,
    lifetime: TextureResourceLifetime,
) -> Option<&'a wgpu::TextureView> {
    scratch.transient_texture_view(&viewport_scratch_key(name, lifetime))
}

pub(crate) fn viewport_scratch_texture<'a>(
    scratch: &'a TransientTextures,
    name: &str,
    lifetime: TextureResourceLifetime,
) -> Option<&'a wgpu::Texture> {
    scratch.transient_texture(&viewport_scratch_key(name, lifetime))
}

pub(crate) fn release_material_scratch_after_submit(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    material_index: usize,
    lifetime: TextureResourceLifetime,
) {
    scratch.release_transient_textures_after_submit(
        frame,
        TransientTextureScope::Material(material_index),
        BRUSH_SCRATCH_NAMESPACE,
        transient_lifetime(lifetime),
    );
}

pub(crate) fn release_viewport_scratch_after_submit(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    lifetime: TextureResourceLifetime,
) {
    scratch.release_transient_textures_after_submit(
        frame,
        TransientTextureScope::Viewport,
        BRUSH_SCRATCH_NAMESPACE,
        transient_lifetime(lifetime),
    );
}

pub(crate) fn transient_lifetime(lifetime: TextureResourceLifetime) -> TransientLifetime {
    match lifetime {
        TextureResourceLifetime::Pass | TextureResourceLifetime::DabBatch => {
            TransientLifetime::Submission
        }
        TextureResourceLifetime::Stroke => TransientLifetime::Operation,
        TextureResourceLifetime::Document | TextureResourceLifetime::Engine => {
            TransientLifetime::Operation
        }
    }
}
