use anyhow::{Result, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        render_report::{GpuTextureMetrics, RenderMetrics},
        stroke_preset::StrokeOp,
    },
    renderer::{
        command::{RendererStrokeStyle, StrokeCommand},
        document::scene::SceneStore,
        engine::gpu_state::RendererGpuState,
        features::brush::{
            BrushEngineRunner, deps::BrushFeatureDeps, engine::BrushEngineDeps,
            resources::BrushGpuResources, session::BrushStrokeSession,
        },
        gpu::frame::GpuFrame,
        report::CommandResult,
    },
};

pub(crate) struct BrushFeature {
    brush: BrushEngineRunner,
    resources: BrushGpuResources,
    active_stroke: Option<BrushStrokeSession>,
}

impl BrushFeature {
    pub(crate) fn new(device: &wgpu::Device, brush: BrushEngineRunner) -> Self {
        Self {
            brush,
            resources: BrushGpuResources::new(device),
            active_stroke: None,
        }
    }

    pub(crate) fn register_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        self.brush.register_texture(device, queue, definition)
    }

    pub(crate) fn unregister_texture(&mut self, id: &str) {
        self.brush.unregister_texture(id);
    }

    pub(crate) fn register_engine(
        &mut self,
        device: &wgpu::Device,
        definition: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        self.brush.register_engine(device, definition)
    }

    pub(crate) fn unregister_engine(&mut self, id: &str) {
        self.brush.unregister_engine(id);
    }

    pub(crate) fn execute(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushFeatureDeps<'_>,
        command: StrokeCommand,
    ) -> Result<CommandResult> {
        if let StrokeCommand::Begin { style, .. } = &command {
            validate_brush_style(deps, style.as_ref())?;
        }
        let mut brush_deps = BrushEngineDeps {
            gpu: &mut *deps.gpu,
            stroke_resources: &mut self.resources,
            materials: deps.materials,
            surfaces: &mut *deps.surfaces,
            scene: &mut *deps.scene,
            selections: &mut *deps.selections,
            scratch: &mut *deps.scratch,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
        };
        match command {
            StrokeCommand::Begin {
                target,
                style,
                active_selection,
            } => {
                let (session, result) = self.brush.begin_stroke(
                    frame,
                    &mut brush_deps,
                    target,
                    style.as_ref(),
                    active_selection,
                )?;
                self.active_stroke = Some(session);
                Ok(result)
            }
            StrokeCommand::AddDabs {
                dabs,
                preview_damage,
            } => {
                let Some(session) = self.active_stroke.as_mut() else {
                    bail!("stroke dabs submitted without active stroke");
                };
                let preview_surfaces = session.preview_surfaces_for_dabs(&dabs)?;
                self.brush.stamp_stroke_payload(
                    frame,
                    &mut brush_deps,
                    session,
                    dabs,
                    preview_surfaces,
                    preview_damage,
                )
            }
            StrokeCommand::End { damage } => {
                let Some(session) = self.active_stroke.take() else {
                    bail!("stroke end submitted without active stroke");
                };
                let committed_surfaces = session.space().surfaces();
                let mutation_damage = damage.clone();
                match self.brush.finalize_stroke(
                    frame,
                    &mut brush_deps,
                    &session,
                    damage.as_ref(),
                    committed_surfaces,
                    mutation_damage,
                ) {
                    Ok(result) => Ok(result),
                    Err(err) => {
                        self.active_stroke = Some(session);
                        Err(err)
                    }
                }
            }
            StrokeCommand::Cancel => {
                let Some(session) = self.active_stroke.take() else {
                    return Ok(CommandResult::default());
                };
                Ok(self.brush.cancel_stroke(frame, &mut brush_deps, &session))
            }
        }
    }

    pub(crate) fn abort(&mut self) {
        self.active_stroke = None;
    }

    pub(crate) fn clear_scene_caches(&mut self) {
        self.brush.clear_scene_caches();
    }

    pub(crate) fn retain_material_count(&mut self, material_count: usize) {
        self.brush.retain_material_count(material_count);
    }

    pub(crate) fn prewarm_uv_island_masks(
        &mut self,
        gpu: &RendererGpuState,
        scene: &SceneStore,
        frame: &mut GpuFrame,
        material_sizes: impl IntoIterator<Item = (usize, [u32; 2])>,
    ) {
        self.brush
            .prewarm_uv_island_masks(gpu, scene, frame, material_sizes);
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        let mut metrics = self.brush.take_metrics();
        metrics.brush_buffer_reallocation_count = self.resources.take_reallocation_count();
        metrics
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        self.brush.texture_metrics()
    }
}

fn validate_brush_style(deps: &BrushFeatureDeps<'_>, style: &RendererStrokeStyle) -> Result<()> {
    let StrokeOp::BrushEngine { engine_id, .. } = &style.stroke_op;
    if deps.brush_engines.get(engine_id).is_none() {
        bail!("unknown brush engine id {:?}", engine_id);
    }
    Ok(())
}
