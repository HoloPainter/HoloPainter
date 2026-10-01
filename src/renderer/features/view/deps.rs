use crate::renderer::{
    document::{materials::MaterialRegistry, scene::SceneResources},
    engine::gpu_state::RendererGpuState,
    features::{
        composite::CompositeOutputViews, view::pipelines::ViewPipelines, view::targets::ViewTargets,
    },
    scene_capture::{SceneCapture, SceneCapturePipelines},
    transient::TransientTextures,
};

pub(crate) struct ViewStrokeDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) view_pipelines: &'a ViewPipelines,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
    pub(crate) composite_outputs: CompositeOutputViews<'a>,
    pub(crate) materials: &'a MaterialRegistry,
    pub(crate) scene: &'a mut SceneResources,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) view_targets: &'a mut ViewTargets,
    pub(crate) scene_capture: &'a mut SceneCapture,
}
