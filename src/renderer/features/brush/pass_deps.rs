use super::engine_resources::TextureGpuStore;

use crate::{
    core::render_report::RenderMetrics,
    renderer::{
        document::{scene::SceneStore, selection::SelectionMasks, surfaces::SurfaceEditContext},
        engine::gpu_state::RendererGpuState,
        features::brush::{
            engine_pipelines::BrushEnginePipelines, resources::BrushResourceContext,
        },
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
    },
};

pub(crate) struct BrushPassDeps<'a, 'surface> {
    pub(crate) gpu: &'a RendererGpuState,
    pub(crate) stroke_resources: BrushResourceContext<'a>,
    pub(crate) material_count: usize,
    pub(crate) textures: &'a TextureGpuStore,
    pub(crate) pipelines: &'a BrushEnginePipelines,
    pub(crate) surfaces: &'a mut SurfaceEditContext<'surface>,
    pub(crate) scene: &'a mut SceneStore,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) selections: &'a mut SelectionMasks,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
    pub(crate) metrics: &'a mut RenderMetrics,
}
