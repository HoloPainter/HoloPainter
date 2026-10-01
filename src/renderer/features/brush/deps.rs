use crate::{
    core::brush_engine::BrushEngineRegistry,
    renderer::{
        document::{
            materials::MaterialRegistry, scene::SceneStore, selection::SelectionMasks,
            surfaces::SurfaceRepository,
        },
        engine::gpu_state::RendererGpuState,
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
    },
};

pub(crate) struct BrushFeatureDeps<'a> {
    pub(crate) brush_engines: &'a BrushEngineRegistry,
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) materials: &'a MaterialRegistry,
    pub(crate) surfaces: &'a mut SurfaceRepository,
    pub(crate) scene: &'a mut SceneStore,
    pub(crate) selections: &'a mut SelectionMasks,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
}
