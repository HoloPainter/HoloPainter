use std::collections::VecDeque;

use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        brush_engine::BrushEngineRegistry,
        image_asset::ImageAssetCatalog,
        render_report::{GpuTextureMetrics, RenderMetrics},
        texture::TextureCatalog,
    },
    renderer::{
        engine::{
            commit_readback::PendingCommitReadback, features::RenderFeatures,
            metrics::RenderMetricsState, state::RendererState,
        },
        result::RenderCommitId,
        scene_capture::SceneCapture,
    },
};

pub struct RenderEngine {
    pub(super) state: RendererState,
    pub(super) features: RenderFeatures,
    pub(super) metrics: RenderMetricsState,
    pub(super) scene_capture: SceneCapture,
    pub(super) next_commit_id: u64,
    pub(super) pending_commit_readbacks: VecDeque<PendingCommitReadback>,
    pub(super) awaiting_commit_acceptance: Option<RenderCommitId>,
}

impl RenderEngine {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, tex_size: [u32; 2]) -> Result<Self> {
        let image_assets = ImageAssetCatalog::load_effective(None)?;
        let brush_textures = TextureCatalog::from_image_assets(&image_assets)?;
        let brush_engines = BrushEngineRegistry::load_effective(Vec::new(), &brush_textures)?;
        Self::new_with_brush_resources(device, queue, tex_size, &brush_engines, &brush_textures)
    }

    pub(crate) fn new_with_brush_resources(
        device: wgpu::Device,
        queue: wgpu::Queue,
        tex_size: [u32; 2],
        brush_engines: &BrushEngineRegistry,
        brush_textures: &TextureCatalog,
    ) -> Result<Self> {
        let state = RendererState::new(device, queue, tex_size)?;
        let features = RenderFeatures::new(
            state.gpu.ctx.device(),
            state.gpu.ctx.queue(),
            tex_size,
            brush_engines,
            brush_textures,
        )?;
        let scene_capture = SceneCapture::new(state.gpu.ctx.device());
        Ok(Self {
            state,
            features,
            metrics: RenderMetricsState::default(),
            scene_capture,
            next_commit_id: 1,
            pending_commit_readbacks: VecDeque::new(),
            awaiting_commit_acceptance: None,
        })
    }

    /// Returns render metrics accumulated since the previous `take_metrics` call.
    ///
    /// These counters are renderer-wide and may include render-view operations,
    /// readbacks, uploads, or composite invalidations, not only effect execution.
    pub fn take_metrics(&mut self) -> RenderMetrics {
        let mut taken = self.metrics.take();
        taken.merge(self.features.composite.take_metrics());
        taken.merge(self.features.brush.take_metrics());
        taken.merge(self.features.apply.take_metrics());
        taken.merge(self.state.transient.take_metrics());
        taken
    }

    pub fn poll_completed_color_samples(&mut self) -> Vec<crate::renderer::CompletedColorSample> {
        let device = self.state.gpu.ctx.device().clone();
        self.features.color_sampler.poll(&device)
    }

    /// Blocks until submitted GPU work has been processed, then returns completed color samples.
    ///
    /// Interactive callers should normally use [`Self::poll_completed_color_samples`] so the UI
    /// thread is not stalled. This blocking variant is intended for synchronous/headless callers
    /// and integration tests.
    pub fn wait_completed_color_samples(&mut self) -> Vec<crate::renderer::CompletedColorSample> {
        let device = self.state.gpu.ctx.device().clone();
        self.features.color_sampler.wait(&device)
    }

    pub(crate) fn register_brush_texture(
        &mut self,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        let device = self.state.gpu.ctx.device().clone();
        let queue = self.state.gpu.ctx.queue().clone();
        self.features
            .register_brush_texture(&device, &queue, definition)
    }

    pub(crate) fn unregister_brush_texture(&mut self, id: &str) {
        self.features.unregister_brush_texture(id);
    }

    pub(crate) fn register_brush_engine(
        &mut self,
        definition: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        let device = self.state.gpu.ctx.device().clone();
        self.features.register_brush_engine(&device, definition)
    }

    pub(crate) fn unregister_brush_engine(&mut self, id: &str) {
        self.features.unregister_brush_engine(id);
    }

    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.state
            .gpu
            .ctx
            .device()
            .limits()
            .max_texture_dimension_2d
    }

    pub fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        metrics.merge(self.state.document.surfaces.texture_metrics());
        metrics.merge(self.state.document.selections.texture_metrics());
        metrics.merge(self.features.composite.texture_metrics());
        metrics.merge(self.features.brush.texture_metrics());
        metrics.merge(self.features.apply.texture_metrics());
        metrics.merge(self.features.view.texture_metrics());
        metrics.merge(self.features.decal_images.texture_metrics());
        metrics.merge(self.features.tool_preview.texture_metrics());
        metrics.merge(self.scene_capture.texture_metrics());
        metrics.merge(self.state.transient.texture_metrics());
        metrics
    }
}
