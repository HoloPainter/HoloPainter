mod action;
#[cfg(test)]
mod app_renderer_e2e_tests;
mod camera_controller;
mod command;
mod commit_finalizer;
mod composite_impact;
mod damage;
mod decal_controller;
mod document_reducer;
mod fill_controller;
mod history;
mod history_capture;
mod history_finalizer;
mod history_reducer;
mod holopack_import;
mod image_copy_workflow;
mod image_cut_controller;
mod image_decode;
mod image_import_reducer;
mod image_import_workflow;
mod import_workflow;
mod layer_mask_paste_controller;
mod layer_reducer;
mod mesh_reload;
mod outcome;
mod paint_edit;
mod reducer;
mod render_plan;
mod runtime;
mod selection_controller;
mod shape_controller;
mod startup_project;
mod state;
mod status;
mod stroke;
mod stroke_controller;
mod surface_filter_controller;
mod tile_payload;
mod tool_controller;
mod tool_reducer;
mod transform_controller;
mod view_input_planner;
mod view_reducer;

pub(crate) use crate::core::damage::DamageMap;
pub(crate) use crate::core::document_tile_store::InitialPixels;
pub(crate) use crate::core::geometry::RectU32;
pub(crate) use crate::core::image::LayerImageSnapshot;
pub(crate) use crate::core::render_report::RenderMetrics;
pub(crate) use crate::core::selection::SelectionTilePayload;
pub(crate) use crate::core::stroke_style::ResolvedStrokeStyle;
pub(crate) use crate::core::tile::{TileCoord, TileGrid};
pub(crate) use crate::renderer::{RenderCommitArtifacts, SelectionCommit, SurfaceCommit};
pub use action::{EditorAction, EditorActionBlockReason, EditorActionContext};
pub use camera_controller::fit_camera_to_document;
pub use command::{
    Command, InputHoldToken, InputModifiers, PointerSample, PointerSampleKind, PointerSampleMeta,
    ToolCancelReason, ToolInputEvent, TransformNumericEdit, UvViewInputContext, ViewPointerEvent,
    ViewPointerMeta, ViewPointerPhase, ViewportInputContext,
};
pub(crate) use composite_impact::CompositeImpact;
pub(crate) use damage::{GEOMETRY_DAMAGE_GUARD_PX, uv_rect_to_pixel_rect};
#[cfg(test)]
pub(crate) use history::HistoryAtom as HistoryEntry;
pub(crate) use history::{
    HistoryAtom, HistoryTransaction, HistoryTransactionId, LayerCompositeSettings,
    LayerPropertyEdit, LayerTreeEdit, LayerTreeSnapshot, PendingCpuHistoryTransaction,
    PendingLayerPropertyEdit, PendingPixelEdit, PendingRendererHistoryTransaction, PixelEdit,
    SelectionTileEdit,
};
pub(crate) use holopack_import::{HoloPackConflictKind, HoloPackImportPlan};
pub use image_copy_workflow::{
    CopiedLayerContent, CopiedLayerImage, CopiedLayerMask, LayerMaskClipboardPayload,
    PreparedLayerCut, PreparedLayerCutAction, can_cut_layer_content, copy_layer_content,
    prepare_cut_layer_content,
};
pub use image_decode::{
    DecodedRgba8Image, RasterImageFormat, SUPPORTED_RASTER_IMAGE_EXTENSIONS, decode_raster_image,
    is_supported_raster_image_path, prepare_rgba8_image, prepare_straight_rgba8_image,
};
pub use image_import_workflow::{
    EmbeddedImageImportPayload, ImportedImagePayload, load_image_as_command, load_rgba8_as_command,
    load_rgba8_as_command_at, load_rgba8_as_paste_command_at,
};
pub(crate) use image_import_workflow::{imported_layer_name, load_decoded_image_as_command};
pub use import_workflow::create_new_project_command;
pub use mesh_reload::{
    MeshReloadMatchConfidence, MeshReloadMaterialBinding, MeshReloadMaterialMatch,
    MeshReloadMaterialTarget, MeshReloadRequest, build_mesh_reload_matches,
    create_mesh_reload_command,
};
pub(crate) use outcome::{
    CompositeSync, PendingDocumentCommit, PendingEditTransaction, PendingHistoryAtom,
    PreparedRendererSubmission, ReducerOutput, RendererCommitSpec, RendererFinalizationPayload,
    RendererSubmissionWork, SubmittedRendererTransaction,
};
pub use paint_edit::{PaintEditBlockReason, PaintEditDecision};
#[cfg(test)]
pub(crate) use reducer::reduce;
pub(crate) use render_plan::{ApplyOneShotRenderPlan, RendererPlanBuilder};
pub use runtime::ApplicationRuntime;
pub(crate) use startup_project::create_startup_project;
pub use state::AppState;
pub use status::StatusMessage;
pub(crate) use status::selection_status_key;
pub(crate) use surface_filter_controller::{
    SPATIAL_BLUR_MAX_ANGLE_DEGREES, SPATIAL_BLUR_MAX_RADIUS_PX, SPATIAL_BLUR_MIN_ANGLE_DEGREES,
    SPATIAL_BLUR_MIN_RADIUS_PX, SURFACE_BLUR_MAX_RADIUS_PX, SURFACE_BLUR_MIN_RADIUS_PX,
};
pub(crate) use tile_payload::{PIXEL_HISTORY_TILE_SIZE, PixelSnapshotData};
#[cfg(test)]
pub(crate) use tile_payload::{PixelSnapshotStorageKind, choose_pixel_snapshot_storage};
pub use view_input_planner::{
    ViewportHoverAnalysis, analyze_viewport_hover, plan_decal_overlay_request,
    plan_decal_overlay_request_for_viewport, plan_surface_mirror_plane_overlay_request,
    plan_uv_brush_overlay_request,
};
