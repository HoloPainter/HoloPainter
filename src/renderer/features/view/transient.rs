use eframe::egui_wgpu::wgpu;

use crate::renderer::transient::{
    TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures,
};

const VIEW_TRANSIENT_NAMESPACE: &str = "view_transient";
const VIEWPORT_BRUSH_OVERLAY_NAME: &str = "viewport_brush_overlay";

pub(crate) fn ensure_viewport_brush_overlay(
    scratch: &mut TransientTextures,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    desired_size: [u32; 2],
) {
    scratch.ensure_transient_texture_with_usage(
        device,
        encoder,
        viewport_brush_overlay_key(),
        desired_size,
        wgpu::TextureFormat::R8Unorm,
        viewport_stroke_usage(),
        "viewport_brush_overlay_tex",
        "clear_new_viewport_brush_overlay",
    );
}

pub(crate) fn viewport_brush_overlay_view(
    scratch: &TransientTextures,
) -> Option<&wgpu::TextureView> {
    scratch.transient_texture_view(&viewport_brush_overlay_key())
}

fn viewport_brush_overlay_key() -> TransientTextureKey {
    TransientTextureKey::new(
        VIEW_TRANSIENT_NAMESPACE,
        TransientTextureScope::Viewport,
        VIEWPORT_BRUSH_OVERLAY_NAME,
        TransientLifetime::Manual,
    )
}

fn viewport_stroke_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
}
