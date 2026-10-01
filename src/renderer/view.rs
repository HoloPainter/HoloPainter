use std::sync::Arc;

use glam::Mat4;

use crate::{
    core::{
        camera::OrbitCamera,
        decal::{DecalImageAsset, DecalProjection},
        selection::ActiveSelection,
        stroke::{StrokeDab, SurfaceDab},
        stroke_preset::StrokeOp,
        surface::{LayerId, PaintSurfaceId},
        tool::{ColorSampleSource, ToolId},
        uv_view::UvViewTransform,
        viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility,
        wireframe::WireframeStyle,
    },
    renderer::command::RendererStrokeStyle,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSampleIntent {
    Preview,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSampleView {
    Viewport3d,
    Uv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorSampleAnchor {
    pub view: ColorSampleView,
    pub position: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorSampleTarget {
    ViewOutput {
        view: ColorSampleView,
        position: [u32; 2],
    },
    CompositeTexture {
        material_index: usize,
        uv: [f32; 2],
    },
    SurfaceTexture {
        surface: PaintSurfaceId,
        uv: [f32; 2],
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSampleUiRequest {
    pub intent: ColorSampleIntent,
    pub source: ColorSampleSource,
    pub anchor: ColorSampleAnchor,
    pub target: ColorSampleTarget,
    pub active_layer_id: Option<LayerId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSampleRequest {
    pub id: u64,
    pub intent: ColorSampleIntent,
    pub source: ColorSampleSource,
    pub anchor: ColorSampleAnchor,
    pub target: ColorSampleTarget,
    pub document_generation: u64,
    pub active_layer_id: Option<LayerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedColorSample {
    pub request: ColorSampleRequest,
    pub result: Result<[u8; 4], String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorSamplePreview {
    pub rgb: [u8; 3],
    pub source: ColorSampleSource,
    pub anchor: ColorSampleAnchor,
    pub document_generation: u64,
    pub active_layer_id: Option<LayerId>,
}

#[derive(Debug, Clone)]
pub enum BrushOverlayRequest {
    Surface(SurfaceBrushOverlayRequest),
}

#[derive(Debug, Clone)]
pub struct SurfaceBrushOverlayRequest {
    pub viewport_size: [u32; 2],
    pub viewport_view_proj_gl: Mat4,
    pub view_direction_world: [f32; 3],
    pub stroke_op: StrokeOp,
    pub dab: SurfaceDab,
}

#[derive(Debug, Clone)]
pub struct UvBrushOverlayRequest {
    pub center_px: [f32; 2],
    pub view_size: [u32; 2],
    pub radius_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MirrorPlaneOverlayRequest {
    pub plane_x: f32,
    pub center_yz: [f32; 2],
    pub half_extent_yz: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct SelectionOverlayRequest {
    pub active_selection: ActiveSelection,
    pub phase: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecalOverlayRequest {
    pub image: Arc<DecalImageAsset>,
    pub projection: DecalProjection,
    pub opacity: f32,
    pub scene_visibility: ViewportSceneVisibility,
}

#[derive(Debug, Clone)]
pub struct ViewportViewRequest {
    pub camera: OrbitCamera,
    pub view_proj: Mat4,
    pub camera_world: [f32; 3],
    pub viewport_size: [u32; 2],
    pub brush_overlay_request: Option<BrushOverlayRequest>,
    pub selection_overlay_request: Option<SelectionOverlayRequest>,
    pub mirror_plane_overlay_request: Option<MirrorPlaneOverlayRequest>,
    pub decal_overlay_request: Option<DecalOverlayRequest>,
    pub scene_visibility: ViewportSceneVisibility,
    pub show_wireframe: bool,
    pub wireframe_style: WireframeStyle,
    pub background_color: [f32; 3],
    pub shading: ViewportShading,
}

#[derive(Debug, Clone)]
pub struct UvViewRequest {
    pub material_index: usize,
    pub uv_view_size: [u32; 2],
    pub transform: UvViewTransform,
    pub brush_overlay_request: Option<UvBrushOverlayRequest>,
    pub selection_overlay_request: Option<SelectionOverlayRequest>,
    pub show_wireframe: bool,
    pub wireframe_style: WireframeStyle,
    pub background_color: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolPreviewRequest {
    pub item_size: [u32; 2],
    pub items: Vec<ToolPreviewItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolPreviewItem {
    pub tool_id: ToolId,
    pub kind: ToolPreviewKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolPreviewKind {
    Brush(ToolBrushPreview),
    RectangleErase,
    LassoErase,
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolBrushPreview {
    pub style: RendererStrokeStyle,
    pub dabs: Vec<StrokeDab>,
}
