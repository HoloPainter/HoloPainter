pub(crate) mod adjustment_gpu;
pub mod api;
pub mod command;
pub(crate) mod decal_image;
pub(crate) mod document;
pub mod engine;
pub mod features;
pub mod frame;
pub(crate) mod gpu;
pub(crate) mod mask;
pub(crate) mod mutation;
pub(crate) mod pixel;
pub mod presentation;
pub mod readback;
pub mod report;
pub mod result;
pub(crate) mod scene_capture;
pub(crate) mod surface_edit;
pub(crate) mod transient;
pub mod view;

pub use crate::core::image::Rgba8Snapshot;
pub use api::RenderEngine as Renderer;
pub use api::{
    PartialCompositeDecision, RenderEngine, normalize_composite_clip_rects,
    partial_composite_decision, tree_supports_partial_composite,
    visible_surfaces_from_composite_tree,
};
pub use command::{
    RendererStrokeOperation, RendererStrokeStyle, StrokeCommand, StrokeDabPayload, StrokeTarget,
    SurfaceProjectionDabBatch, TransformCommand,
};
pub use features::{
    apply::{ApplyCommand, ApplyOperation, FillCoverage, FillCoverageDelta, FillStrokeCommand},
    composite::{CompositeBakeTarget, CompositeCommand},
    filter::{FilterCommand, SpatialBlurParams, SurfaceBlurParams},
    selection::SelectionEditCommand,
    view::ViewCommand,
};
pub use frame::{
    CommitRequest, EditCommand, GpuDocumentCommand, MaterialRegistration, MaterialUpload,
    RendererFramePlan, SelectionCommitRequest, ViewRequest,
};
pub use presentation::{MaterialTextureOutput, TextureOutputs};
pub use readback::{ReadbackRequest, ReadbackResult};
pub use report::RenderExecutionReport;
pub use result::{
    CompletedRenderCommit, RenderCommitArtifacts, RenderCommitId, RendererDiagnostic,
    SelectionCommit, StartedRenderCommit, SurfaceCommit,
};
pub use view::{
    BrushOverlayRequest, ColorSampleAnchor, ColorSampleIntent, ColorSamplePreview,
    ColorSampleRequest, ColorSampleTarget, ColorSampleUiRequest, ColorSampleView,
    CompletedColorSample, DecalOverlayRequest, MirrorPlaneOverlayRequest, SelectionOverlayRequest,
    SurfaceBrushOverlayRequest, ToolBrushPreview, ToolPreviewItem, ToolPreviewKind,
    ToolPreviewRequest, UvBrushOverlayRequest, UvViewRequest, ViewportViewRequest,
};
