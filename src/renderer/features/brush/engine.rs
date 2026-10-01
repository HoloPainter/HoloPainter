use std::sync::Arc;

use anyhow::{Result, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        brush_engine::{BrushEngineRegistry, TextureResourceLifetime},
        damage::DamageMap,
        render_report::{GpuTextureMetrics, RenderMetrics},
        selection::ActiveSelection,
        stroke::{PaintSurfaceSet, StrokeContext, StrokeDab},
        stroke_preset::StrokeOp,
        surface::PaintSurfaceId,
        texture::TextureCatalog,
    },
    renderer::{
        command::{RendererStrokeStyle, StrokeDabPayload, StrokeSpace, StrokeTarget},
        document::{
            materials::MaterialRegistry,
            scene::SceneStore,
            selection::SelectionMasks,
            surfaces::{SurfaceEditContext, SurfacePrepareStats, SurfaceRepository},
        },
        engine::gpu_state::RendererGpuState,
        features::brush::{
            engine_pipelines::BrushEnginePipelines,
            engine_resources::BrushResources,
            executor::BrushPassExecutor,
            pass_deps::BrushPassDeps,
            resources::{BrushGpuResources, BrushResourceContext},
            scratch::{
                material_scratch_view, release_material_scratch_after_submit,
                release_viewport_scratch_after_submit,
            },
            session::BrushStrokeSession,
            transient as stroke_transient,
        },
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
        scene_capture::{SceneCapture, SceneCapturePipelines},
        surface_edit::{
            EditMask, SurfaceEditPipelines, SurfaceEditResources, SurfaceEditTarget, UvIslandBleed,
        },
        transient::TransientTextures,
    },
};

pub(crate) struct BrushEngineRunner {
    pipelines: BrushEnginePipelines,
    resources: BrushResources,
    executor: BrushPassExecutor,
    surface_edit_pipelines: SurfaceEditPipelines,
    surface_edit_resources: SurfaceEditResources,
    uv_island_bleed: UvIslandBleed,
}

pub(crate) struct BrushEngineDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) stroke_resources: &'a mut BrushGpuResources,
    pub(crate) materials: &'a MaterialRegistry,
    pub(crate) surfaces: &'a mut SurfaceRepository,
    pub(crate) scene: &'a mut SceneStore,
    pub(crate) selections: &'a mut SelectionMasks,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
}

struct BrushExecutionDeps<'a, 'surface> {
    gpu: &'a mut RendererGpuState,
    stroke_resources: &'a mut BrushGpuResources,
    materials: &'a MaterialRegistry,
    surface_edit: &'a mut SurfaceEditContext<'surface>,
    scene: &'a mut SceneStore,
    selections: &'a mut SelectionMasks,
    scratch: &'a mut TransientTextures,
    scene_capture: &'a mut SceneCapture,
    scene_capture_pipelines: &'a SceneCapturePipelines,
}

impl BrushEngineRunner {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &BrushEngineRegistry,
        textures: &TextureCatalog,
    ) -> Result<Self> {
        Ok(Self {
            pipelines: BrushEnginePipelines::new(device, registry)?,
            resources: BrushResources::new(device, queue, textures)?,
            executor: BrushPassExecutor::new(),
            surface_edit_pipelines: SurfaceEditPipelines::new(device),
            surface_edit_resources: SurfaceEditResources::new(device),
            uv_island_bleed: UvIslandBleed::default(),
        })
    }

    pub(crate) fn register_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        self.resources.insert_texture(device, queue, definition)
    }

    pub(crate) fn unregister_texture(&mut self, id: &str) {
        self.resources.remove_texture(id);
    }

    pub(crate) fn register_engine(
        &mut self,
        device: &wgpu::Device,
        engine: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        self.pipelines.register_engine(device, engine)
    }

    pub(crate) fn unregister_engine(&mut self, id: &str) {
        self.pipelines.unregister_engine(id);
    }

    pub(crate) fn clear_scene_caches(&mut self) {
        self.uv_island_bleed.clear();
    }

    pub(crate) fn retain_material_count(&mut self, material_count: usize) {
        self.uv_island_bleed.retain_material_count(material_count);
    }

    pub(crate) fn prewarm_uv_island_masks(
        &mut self,
        gpu: &RendererGpuState,
        scene: &SceneStore,
        frame: &mut GpuFrame,
        material_sizes: impl IntoIterator<Item = (usize, [u32; 2])>,
    ) {
        self.uv_island_bleed.prewarm_materials(
            gpu,
            scene,
            &self.surface_edit_pipelines,
            frame.encoder(),
            material_sizes,
        );
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        self.uv_island_bleed.take_metrics()
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        self.uv_island_bleed.texture_metrics()
    }

    pub(crate) fn begin_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushEngineDeps<'_>,
        target: StrokeTarget,
        style: &RendererStrokeStyle,
        active_selection: Arc<ActiveSelection>,
    ) -> Result<(BrushStrokeSession, CommandResult)> {
        let mut mutations = MutationLog::default();
        let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
        let mut edit_deps = BrushExecutionDeps {
            gpu: &mut *deps.gpu,
            stroke_resources: &mut *deps.stroke_resources,
            materials: deps.materials,
            surface_edit: &mut surface_edit,
            scene: &mut *deps.scene,
            selections: &mut *deps.selections,
            scratch: &mut *deps.scratch,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
        };
        let (session, metrics) =
            self.begin_stroke_target(frame, &mut edit_deps, target, style, active_selection)?;
        drop(edit_deps);
        drop(surface_edit);
        Ok((session, CommandResult::new(mutations, metrics)))
    }

    pub(crate) fn stamp_stroke_payload(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushEngineDeps<'_>,
        session: &mut BrushStrokeSession,
        dabs: StrokeDabPayload,
        preview_surfaces: PaintSurfaceSet,
        preview_damage: Option<DamageMap>,
    ) -> Result<CommandResult> {
        let mut mutations = MutationLog::default();
        let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
        let mut edit_deps = BrushExecutionDeps {
            gpu: &mut *deps.gpu,
            stroke_resources: &mut *deps.stroke_resources,
            materials: deps.materials,
            surface_edit: &mut surface_edit,
            scene: &mut *deps.scene,
            selections: &mut *deps.selections,
            scratch: &mut *deps.scratch,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
        };
        let (metrics, newly_active_surfaces) =
            self.stamp_stroke_payload_for_session(frame, &mut edit_deps, session, dabs)?;
        let mut metrics = metrics;
        if matches!(session.space(), StrokeSpace::Surface { .. }) {
            record_surface_damage_metrics(
                &mut metrics,
                &*edit_deps.surface_edit,
                &preview_surfaces,
                preview_damage.as_ref(),
            );
        }
        edit_deps
            .surface_edit
            .record_preview_damage(&preview_surfaces, preview_damage);
        drop(edit_deps);
        drop(surface_edit);
        let result = CommandResult::new(mutations, metrics);
        if newly_active_surfaces.is_empty() {
            Ok(result)
        } else {
            Ok(result.with_active_paint_surfaces(
                newly_active_surfaces.iter(),
                session.space().surfaces().iter(),
            ))
        }
    }

    pub(crate) fn finalize_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushEngineDeps<'_>,
        session: &BrushStrokeSession,
        damage: Option<&DamageMap>,
        committed_surfaces: PaintSurfaceSet,
        mutation_damage: Option<DamageMap>,
    ) -> Result<CommandResult> {
        let mut mutations = MutationLog::default();
        let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
        let mut edit_deps = BrushExecutionDeps {
            gpu: &mut *deps.gpu,
            stroke_resources: &mut *deps.stroke_resources,
            materials: deps.materials,
            surface_edit: &mut surface_edit,
            scene: &mut *deps.scene,
            selections: &mut *deps.selections,
            scratch: &mut *deps.scratch,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
        };
        let metrics = self.finalize_active_stroke(frame, &mut edit_deps, session, damage)?;
        edit_deps
            .surface_edit
            .record_committed_surface_damage(&committed_surfaces, mutation_damage);
        drop(edit_deps);
        drop(surface_edit);
        Ok(CommandResult::new(mutations, metrics))
    }

    pub(crate) fn cancel_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushEngineDeps<'_>,
        session: &BrushStrokeSession,
    ) -> CommandResult {
        let mut mutations = MutationLog::default();
        let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
        let mut edit_deps = BrushExecutionDeps {
            gpu: &mut *deps.gpu,
            stroke_resources: &mut *deps.stroke_resources,
            materials: deps.materials,
            surface_edit: &mut surface_edit,
            scene: &mut *deps.scene,
            selections: &mut *deps.selections,
            scratch: &mut *deps.scratch,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
        };
        let mut metrics = RenderMetrics::default();
        let mut stroke_deps = self.stroke_deps(&mut edit_deps, &mut metrics);
        session.cancel(&self.executor, frame, &mut stroke_deps);
        drop(stroke_deps);

        let mut material_indices = session
            .space()
            .surfaces()
            .iter()
            .map(|surface| surface.material_index().as_usize())
            .collect::<Vec<_>>();
        material_indices.sort_unstable();
        material_indices.dedup();
        for material_index in material_indices {
            stroke_transient::release_material_transient_uv_after_submit(
                edit_deps.scratch,
                frame,
                material_index,
            );
            release_material_scratch_after_submit(
                edit_deps.scratch,
                frame,
                material_index,
                TextureResourceLifetime::Stroke,
            );
        }
        release_viewport_scratch_after_submit(
            edit_deps.scratch,
            frame,
            TextureResourceLifetime::Stroke,
        );

        for surface in session.space().surfaces().iter() {
            mutations.surfaces.cancelled(surface);
        }
        CommandResult::new(mutations, metrics)
    }

    fn stroke_deps<'a, 'deps, 'surface>(
        &'a self,
        deps: &'a mut BrushExecutionDeps<'deps, 'surface>,
        metrics: &'a mut RenderMetrics,
    ) -> BrushPassDeps<'a, 'surface> {
        BrushPassDeps {
            gpu: &*deps.gpu,
            stroke_resources: BrushResourceContext::new(
                &mut *deps.stroke_resources,
                deps.gpu.device(),
            ),
            material_count: deps.materials.material_count(),
            textures: self.resources.textures(),
            pipelines: &self.pipelines,
            surfaces: &mut *deps.surface_edit,
            scene: &mut *deps.scene,
            scratch: &mut *deps.scratch,
            selections: &mut *deps.selections,
            scene_capture: &mut *deps.scene_capture,
            scene_capture_pipelines: deps.scene_capture_pipelines,
            metrics,
        }
    }

    fn begin_stroke_target(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        target: StrokeTarget,
        style: &RendererStrokeStyle,
        active_selection: Arc<ActiveSelection>,
    ) -> Result<(BrushStrokeSession, RenderMetrics)> {
        match target {
            StrokeTarget::Uv { surface } => {
                self.begin_uv_stroke(frame, deps, surface, style, active_selection)
            }
            StrokeTarget::Surface { surfaces, context } => {
                self.begin_surface_stroke(frame, deps, surfaces, context, style, active_selection)
            }
        }
    }

    fn begin_uv_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        target: PaintSurfaceId,
        style: &RendererStrokeStyle,
        active_selection: Arc<ActiveSelection>,
    ) -> Result<(BrushStrokeSession, RenderMetrics)> {
        let mut metrics = RenderMetrics::default();
        prepare_surface_for_operation(frame, &*deps.gpu, deps.surface_edit, &mut metrics, target)?;
        let mut stroke_deps = self.stroke_deps(deps, &mut metrics);
        let brush_uniform = self.executor.resolve_brush_uniform(&stroke_deps, style)?;
        self.executor
            .begin_uv_stroke(frame, &mut stroke_deps, target, style);
        let session = BrushStrokeSession::uv(target, style, brush_uniform, active_selection);
        Ok((session, metrics))
    }

    fn begin_surface_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        surfaces: PaintSurfaceSet,
        context: Arc<StrokeContext>,
        style: &RendererStrokeStyle,
        active_selection: Arc<ActiveSelection>,
    ) -> Result<(BrushStrokeSession, RenderMetrics)> {
        let mut metrics = RenderMetrics::default();
        record_surface_target_counts(&mut metrics, surfaces.as_slice());
        record_new_surface_counts(&mut metrics, surfaces.as_slice());
        prepare_surfaces_for_operation(
            frame,
            &*deps.gpu,
            deps.surface_edit,
            &mut metrics,
            surfaces.as_slice(),
        )?;
        let mut stroke_deps = self.stroke_deps(deps, &mut metrics);
        let brush_uniform = self.executor.resolve_brush_uniform(&stroke_deps, style)?;
        self.executor
            .begin_surface_stroke(frame, &mut stroke_deps, surfaces.as_slice(), style);
        let session =
            BrushStrokeSession::surface(surfaces, context, style, brush_uniform, active_selection);
        Ok((session, metrics))
    }

    fn stamp_stroke_payload_for_session(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &mut BrushStrokeSession,
        dabs: StrokeDabPayload,
    ) -> Result<(RenderMetrics, PaintSurfaceSet)> {
        let space = session.space().clone();
        match (space, dabs) {
            (StrokeSpace::Uv { target }, StrokeDabPayload::Stroke(dabs)) => {
                let metrics = self.stamp_uv_batch(frame, deps, session, target, &dabs)?;
                Ok((metrics, PaintSurfaceSet::from_vec(Vec::new())))
            }
            (
                StrokeSpace::Surface { .. },
                StrokeDabPayload::Surface {
                    source_surfaces,
                    target_surfaces,
                    projection_batches,
                },
            ) => self.stamp_surface_batch(
                frame,
                deps,
                session,
                &source_surfaces,
                &target_surfaces,
                &projection_batches,
            ),
            _ => bail!("brush received dabs for the wrong stroke space"),
        }
    }

    fn stamp_uv_batch(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &mut BrushStrokeSession,
        target: PaintSurfaceId,
        dabs: &[StrokeDab],
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        prepare_surface_for_operation(frame, &*deps.gpu, deps.surface_edit, &mut metrics, target)?;
        if !session.space().matches_uv(target) {
            bail!("missing active brush UV stroke");
        }
        {
            let mut stroke_deps = self.stroke_deps(deps, &mut metrics);
            session.stamp_uv_batch(&self.executor, frame, &mut stroke_deps, target, dabs)?;
        }
        resolve_mask_edits_for_surfaces(
            frame,
            deps.gpu,
            deps.surface_edit,
            std::iter::once(target),
        )?;
        Ok(metrics)
    }

    fn surface_stamp_needs_viewport_source_color(
        &self,
        session: &BrushStrokeSession,
    ) -> Result<bool> {
        let runtime = runtime_for_style(&self.pipelines, session.style())?;
        Ok(runtime.surface_viewport_usage().needs_viewport_source_color
            || runtime.surface_usage().needs_viewport_source_color)
    }

    fn stamp_surface_batch(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &mut BrushStrokeSession,
        source_surfaces: &PaintSurfaceSet,
        target_surfaces: &PaintSurfaceSet,
        projection_batches: &[crate::renderer::command::SurfaceProjectionDabBatch],
    ) -> Result<(RenderMetrics, PaintSurfaceSet)> {
        let mut metrics = RenderMetrics::default();
        metrics.surface_projection_batch_count = projection_batches.len();
        metrics.surface_dab_count = projection_batches
            .iter()
            .map(|batch| batch.dabs.len())
            .sum();
        metrics.surface_target_material_dab_reference_count = projection_batches
            .iter()
            .flat_map(|batch| &batch.target_materials_by_dab)
            .map(Vec::len)
            .sum();
        metrics.surface_multi_material_dab_count = projection_batches
            .iter()
            .flat_map(|batch| &batch.target_materials_by_dab)
            .filter(|materials| materials.len() > 1)
            .count();
        record_surface_target_counts(&mut metrics, target_surfaces.as_slice());
        let source_resources_changed = if self.surface_stamp_needs_viewport_source_color(session)? {
            prepare_source_surfaces_for_operation(
                frame,
                &*deps.gpu,
                deps.surface_edit,
                &mut metrics,
                source_surfaces.as_slice(),
            )?
        } else {
            false
        };
        let target_resources_changed = prepare_surfaces_for_operation(
            frame,
            &*deps.gpu,
            deps.surface_edit,
            &mut metrics,
            target_surfaces.as_slice(),
        )?;
        if source_resources_changed || target_resources_changed {
            session.clear_surface_viewport_material_bind_groups();
        }
        let added_surfaces = session.add_surface_targets(target_surfaces);
        record_new_surface_counts(&mut metrics, added_surfaces.as_slice());
        {
            let mut stroke_deps = self.stroke_deps(deps, &mut metrics);
            if !added_surfaces.is_empty() {
                session.begin_added_surface_targets(
                    &self.executor,
                    frame,
                    &mut stroke_deps,
                    &added_surfaces,
                );
            }
            self.executor.prepare_surface_logical_batch_sources(
                frame,
                &mut stroke_deps,
                target_surfaces.as_slice(),
                session.style(),
            )?;
            for batch in projection_batches {
                session.stamp_surface_batch(
                    &self.executor,
                    frame,
                    &mut stroke_deps,
                    source_surfaces,
                    target_surfaces,
                    batch,
                )?;
            }
        }
        resolve_mask_edits_for_surfaces(
            frame,
            deps.gpu,
            deps.surface_edit,
            target_surfaces.iter(),
        )?;
        Ok((metrics, added_surfaces))
    }

    fn finalize_active_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &BrushStrokeSession,
        damage: Option<&DamageMap>,
    ) -> Result<RenderMetrics> {
        match session.space().clone() {
            StrokeSpace::Uv { target } => {
                self.finalize_uv_stroke(frame, deps, session, target, damage)
            }
            StrokeSpace::Surface { surfaces } => {
                self.finalize_surface_stroke(frame, deps, session, &surfaces, damage)
            }
        }
    }

    fn finalize_uv_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &BrushStrokeSession,
        target: PaintSurfaceId,
        _damage: Option<&DamageMap>,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        prepare_surface_for_operation(frame, &*deps.gpu, deps.surface_edit, &mut metrics, target)?;
        if !session.space().matches_uv(target) {
            bail!("missing active brush UV stroke");
        }
        let mut stroke_deps = self.stroke_deps(deps, &mut metrics);
        session.finalize_uv_stroke(&self.executor, frame, &mut stroke_deps, target);
        resolve_mask_edits_for_surfaces(
            frame,
            deps.gpu,
            deps.surface_edit,
            std::iter::once(target),
        )?;
        stroke_transient::release_material_transient_uv_after_submit(
            deps.scratch,
            frame,
            target.material_index().as_usize(),
        );
        Ok(metrics)
    }

    fn finalize_surface_stroke(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut BrushExecutionDeps<'_, '_>,
        session: &BrushStrokeSession,
        surfaces: &PaintSurfaceSet,
        damage: Option<&DamageMap>,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        prepare_surfaces_for_operation(
            frame,
            &*deps.gpu,
            deps.surface_edit,
            &mut metrics,
            surfaces.as_slice(),
        )?;
        if !session.space().matches_surface(surfaces) {
            bail!("missing active brush surface stroke");
        }
        {
            let dilation_mask =
                runtime_for_style(&self.pipelines, session.style())?.surface_dilation_mask();
            let mut stroke_deps = BrushPassDeps {
                gpu: &*deps.gpu,
                stroke_resources: BrushResourceContext::new(
                    &mut *deps.stroke_resources,
                    deps.gpu.device(),
                ),
                material_count: deps.materials.material_count(),
                textures: self.resources.textures(),
                pipelines: &self.pipelines,
                surfaces: &mut *deps.surface_edit,
                scene: &mut *deps.scene,
                scratch: &mut *deps.scratch,
                selections: &mut *deps.selections,
                scene_capture: &mut *deps.scene_capture,
                scene_capture_pipelines: deps.scene_capture_pipelines,
                metrics: &mut metrics,
            };
            session.finalize_surface_stroke(
                &self.executor,
                frame,
                &mut stroke_deps,
                surfaces,
                damage,
            );
            drop(stroke_deps);
            if let Some(dilation_mask) = dilation_mask.as_ref() {
                for surface in surfaces.iter() {
                    record_surface_stroke_dilation(
                        frame,
                        &*deps.gpu,
                        deps.surface_edit,
                        deps.scene,
                        deps.scratch,
                        &self.surface_edit_resources,
                        &self.surface_edit_pipelines,
                        &mut self.uv_island_bleed,
                        surface,
                        dilation_mask.name.as_str(),
                        dilation_mask.lifetime,
                    );
                }
            }
        }
        resolve_mask_edits_for_surfaces(frame, deps.gpu, deps.surface_edit, surfaces.iter())?;
        for surface in surfaces.iter() {
            let material_index = surface.material_index().as_usize();
            stroke_transient::release_material_transient_uv_after_submit(
                deps.scratch,
                frame,
                material_index,
            );
            release_material_scratch_after_submit(
                deps.scratch,
                frame,
                material_index,
                TextureResourceLifetime::Stroke,
            );
        }
        release_viewport_scratch_after_submit(deps.scratch, frame, TextureResourceLifetime::Stroke);
        Ok(metrics)
    }
}

fn record_surface_target_counts(metrics: &mut RenderMetrics, surfaces: &[PaintSurfaceId]) {
    metrics.surface_target_surface_count_max =
        metrics.surface_target_surface_count_max.max(surfaces.len());
    metrics.surface_target_material_count_max = metrics
        .surface_target_material_count_max
        .max(unique_material_count(surfaces));
}

fn record_new_surface_counts(metrics: &mut RenderMetrics, surfaces: &[PaintSurfaceId]) {
    metrics.surface_new_stroke_surface_count = metrics
        .surface_new_stroke_surface_count
        .saturating_add(surfaces.len());
    metrics.surface_new_stroke_material_count = metrics
        .surface_new_stroke_material_count
        .saturating_add(unique_material_count(surfaces));
}

fn unique_material_count(surfaces: &[PaintSurfaceId]) -> usize {
    let mut materials = Vec::with_capacity(surfaces.len());
    for surface in surfaces {
        let material = surface.material_index().as_usize();
        if !materials.contains(&material) {
            materials.push(material);
        }
    }
    materials.len()
}

fn record_surface_damage_metrics(
    metrics: &mut RenderMetrics,
    surface_edit: &SurfaceEditContext<'_>,
    surfaces: &PaintSurfaceSet,
    damage: Option<&DamageMap>,
) {
    let Some(damage) = damage else {
        metrics.surface_full_damage_count = metrics
            .surface_full_damage_count
            .saturating_add(surfaces.len());
        return;
    };
    for pixel in &damage.pixels {
        record_surface_damage_rect_metric(
            metrics,
            surface_edit.surface_texture_size(pixel.surface),
            pixel.rect,
        );
    }
}

fn record_surface_damage_rect_metric(
    metrics: &mut RenderMetrics,
    texture_size: Option<[u32; 2]>,
    rect: crate::core::geometry::RectU32,
) {
    let is_full = texture_size.is_some_and(|size| rect.origin == [0, 0] && rect.size == size);
    if is_full {
        metrics.surface_full_damage_count = metrics.surface_full_damage_count.saturating_add(1);
    } else {
        metrics.surface_partial_damage_rect_count =
            metrics.surface_partial_damage_rect_count.saturating_add(1);
    }
}

fn prepare_surface_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surface: PaintSurfaceId,
) -> Result<bool> {
    let stats = surface_edit.prepare_edit_surface_for_frame(gpu, frame, surface)?;
    let resources_changed = stats.evicted || stats.rehydrated;
    record_surface_prepare_stats(metrics, stats);
    Ok(resources_changed)
}

#[allow(clippy::too_many_arguments)]
fn record_surface_stroke_dilation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &SurfaceEditContext<'_>,
    scene: &SceneStore,
    scratch: &TransientTextures,
    surface_edit_resources: &SurfaceEditResources,
    surface_edit_pipelines: &SurfaceEditPipelines,
    uv_island_bleed: &mut UvIslandBleed,
    target: PaintSurfaceId,
    mask_resource_name: &str,
    mask_lifetime: TextureResourceLifetime,
) {
    let Some(edit_mask_view) =
        material_scratch_view(scratch, target, mask_resource_name, mask_lifetime)
    else {
        return;
    };
    let Some(layer) = surface_edit.stroke_surface_target(target, scratch) else {
        return;
    };
    uv_island_bleed.record(
        gpu,
        surface_edit_resources,
        scene,
        surface_edit_pipelines,
        frame,
        SurfaceEditTarget {
            material_index: target.material_index().as_usize(),
            texture_size: layer.texture_size,
            write_texture: layer.write_texture,
            write_view: layer.write_view,
            read_texture: layer.read_texture,
            read_view: layer.read_view,
        },
        EditMask {
            view: edit_mask_view,
        },
    );
}

fn prepare_source_surface_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surface: PaintSurfaceId,
) -> Result<bool> {
    let stats = surface_edit.prepare_read_surface_for_frame(gpu, frame, surface)?;
    let resources_changed = stats.evicted || stats.rehydrated;
    record_surface_prepare_stats(metrics, stats);
    Ok(resources_changed)
}

fn prepare_surfaces_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surfaces: &[PaintSurfaceId],
) -> Result<bool> {
    let mut resources_changed = false;
    for surface in surfaces {
        resources_changed |=
            prepare_surface_for_operation(frame, gpu, surface_edit, metrics, *surface)?;
    }
    Ok(resources_changed)
}

fn prepare_source_surfaces_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surfaces: &[PaintSurfaceId],
) -> Result<bool> {
    let mut resources_changed = false;
    for surface in surfaces {
        resources_changed |=
            prepare_source_surface_for_operation(frame, gpu, surface_edit, metrics, *surface)?;
    }
    Ok(resources_changed)
}

fn runtime_for_style<'a>(
    pipelines: &'a BrushEnginePipelines,
    style: &RendererStrokeStyle,
) -> Result<&'a crate::renderer::features::brush::engine_pipelines::BrushEngineRuntime> {
    let StrokeOp::BrushEngine { engine_id, .. } = &style.stroke_op;
    pipelines
        .runtime(engine_id)
        .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))
}

fn record_surface_prepare_stats(metrics: &mut RenderMetrics, stats: SurfacePrepareStats) {
    record_surface_prepare_metrics(metrics, &stats);
}

fn resolve_mask_edits_for_surfaces(
    frame: &mut GpuFrame,
    gpu: &mut RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    surfaces: impl IntoIterator<Item = PaintSurfaceId>,
) -> Result<()> {
    surface_edit.resolve_mask_edit_proxies_into_frame(gpu, frame, surfaces)
}

#[cfg(test)]
mod tests {
    use crate::core::{geometry::RectU32, render_report::RenderMetrics};

    use super::record_surface_damage_rect_metric;

    #[test]
    fn full_surface_damage_increments_full_counter() {
        let mut metrics = RenderMetrics::default();

        record_surface_damage_rect_metric(
            &mut metrics,
            Some([2048, 2048]),
            RectU32 {
                origin: [0, 0],
                size: [2048, 2048],
            },
        );

        assert_eq!(metrics.surface_full_damage_count, 1);
        assert_eq!(metrics.surface_partial_damage_rect_count, 0);
    }
}
