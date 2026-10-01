use crate::renderer::{
    document::{scene::SceneResources, selection::SelectionStore, surfaces::SurfaceEditContext},
    engine::gpu_state::RendererGpuState,
    features::apply::pipelines::PaintApplyPipelines,
    mask::projection_gpu::PreparedViewportPolygonCoverage,
    scene_capture::{SceneCapture, SceneCapturePipelines},
    transient::TransientTextures,
};

pub(crate) struct PaintApplyDeps<'a, 'surface> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) pipelines: &'a PaintApplyPipelines,
    pub(crate) surfaces: &'a mut SurfaceEditContext<'surface>,
    pub(crate) scene: &'a mut SceneResources,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) selections: &'a mut SelectionStore,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
    pub(crate) viewport_polygon_coverage: Option<PreparedViewportPolygonCoverage>,
}
