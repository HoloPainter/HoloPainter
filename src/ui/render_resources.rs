use eframe::egui::TextureId;

use crate::renderer::ColorSamplePreview;

pub const VIEWPORT_BACKGROUND_RGB: [u8; 3] = [24, 24, 24];

#[derive(Debug, Default, Clone, Copy)]
pub struct UiRenderResources {
    pub renderer_available: bool,
    pub viewport_texture_id: Option<TextureId>,
    pub uv_view_texture_id: Option<TextureId>,
    pub tool_preview_texture_id: Option<TextureId>,
    pub decal_thumbnail_texture_id: Option<TextureId>,
    pub color_sample_preview: Option<ColorSamplePreview>,
}
