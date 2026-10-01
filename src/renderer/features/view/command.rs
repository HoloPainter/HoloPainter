use glam::Mat4;

use crate::{
    core::{
        camera::OrbitCamera, uv_view::UvViewTransform, viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility, wireframe::WireframeStyle,
    },
    renderer::view::{
        BrushOverlayRequest, DecalOverlayRequest, MirrorPlaneOverlayRequest,
        SelectionOverlayRequest, ToolPreviewRequest, UvBrushOverlayRequest,
    },
};

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ViewCommand {
    RenderViewport {
        camera: OrbitCamera,
        view_proj: Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        brush_overlay_request: Option<BrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        mirror_plane_overlay_request: Option<MirrorPlaneOverlayRequest>,
        decal_overlay_request: Option<DecalOverlayRequest>,
        scene_visibility: ViewportSceneVisibility,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
        shading: ViewportShading,
    },
    RenderUvView {
        material_index: usize,
        uv_view_size: [u32; 2],
        transform: UvViewTransform,
        brush_overlay_request: Option<UvBrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
    },
    RenderToolPreviews(ToolPreviewRequest),
}
