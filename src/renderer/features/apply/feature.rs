use anyhow::{Result, bail};
use eframe::egui_wgpu::wgpu;

use super::command::{ApplyCommand, FillCoverage, FillStrokeCommand};
use super::fill_session::FillApplySession;

use crate::{
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        damage::DamageMap,
        decal::DecalApplyPlan,
        mask::{MaskSource, ProjectionMaskSource},
        render_report::{GpuTextureMetrics, RenderMetrics},
        selection::ActiveSelection,
        stroke::PaintSurfaceSet,
        surface::PaintSurfaceId,
    },
    renderer::{
        decal_image::DecalImageCache,
        document::{
            scene::SceneResources,
            selection::SelectionMasks,
            surfaces::{SurfaceEditContext, SurfacePrepareStats, SurfaceRepository},
        },
        engine::gpu_state::RendererGpuState,
        features::{
            apply::{
                decal::{DecalApplyRequest, DecalApplyResources, record_decal_apply},
                fill_transient,
                masked::{
                    plan_one_shot_masked_apply_requests, record_masked_paint_apply_with_resources,
                    record_one_shot_masked_apply,
                },
                operation::ApplyOperation,
                paint::composite_uniform_from_apply_operation,
            },
            brush::transient as stroke_transient,
        },
        gpu::{clear_rgba_target, copy_a_to_b, frame::GpuFrame},
        mask::{
            geometry::triangle_geometry_mask_rect_r8,
            projection_gpu::prepare_viewport_polygon_coverage,
        },
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
        scene_capture::{SceneCapture, SceneCapturePipelines},
        surface_edit::{
            EditMask, SurfaceEditPipelines, SurfaceEditResources, SurfaceEditTarget, UvIslandBleed,
        },
        transient::TransientTextures,
    },
};

use super::{deps::PaintApplyDeps, pipelines::PaintApplyPipelines};

/// Target vertical boundary for apply renderer work.
pub(crate) struct ApplyFeature {
    pipelines: PaintApplyPipelines,
    surface_edit_pipelines: SurfaceEditPipelines,
    surface_edit_resources: SurfaceEditResources,
    uv_island_bleed: UvIslandBleed,
    decal_resources: DecalApplyResources,
    active_fill: Option<FillApplySession>,
}

pub(crate) struct ApplyFeatureDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) surfaces: &'a mut SurfaceRepository,
    pub(crate) scene: &'a mut SceneResources,
    pub(crate) selections: &'a mut SelectionMasks,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
    pub(crate) decal_images: &'a mut DecalImageCache,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct ApplyExecution {
    metrics: RenderMetrics,
    applied: bool,
    newly_active_paint_surfaces: Vec<PaintSurfaceId>,
    active_paint_surfaces: Vec<PaintSurfaceId>,
}

impl ApplyExecution {
    fn new(metrics: RenderMetrics, applied: bool) -> Self {
        Self {
            metrics,
            applied,
            newly_active_paint_surfaces: Vec::new(),
            active_paint_surfaces: Vec::new(),
        }
    }

    fn with_active_paint_surfaces(
        mut self,
        newly_active_paint_surfaces: Vec<PaintSurfaceId>,
        active_paint_surfaces: Vec<PaintSurfaceId>,
    ) -> Self {
        self.newly_active_paint_surfaces = newly_active_paint_surfaces;
        self.active_paint_surfaces = active_paint_surfaces;
        self
    }
}

impl ApplyFeature {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: PaintApplyPipelines::new(device),
            surface_edit_pipelines: SurfaceEditPipelines::new(device),
            surface_edit_resources: SurfaceEditResources::new(device),
            uv_island_bleed: UvIslandBleed::default(),
            decal_resources: DecalApplyResources::new(device),
            active_fill: None,
        }
    }

    pub(crate) fn clear_scene_caches(&mut self) {
        self.uv_island_bleed.clear();
    }

    pub(crate) fn abort(&mut self) {
        self.active_fill = None;
    }

    pub(crate) fn retain_material_count(&mut self, material_count: usize) {
        self.uv_island_bleed.retain_material_count(material_count);
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        self.uv_island_bleed.take_metrics()
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        self.uv_island_bleed.texture_metrics()
    }

    pub(crate) fn pipelines(&self) -> &PaintApplyPipelines {
        &self.pipelines
    }

    pub(crate) fn execute(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut ApplyFeatureDeps<'_>,
        command: ApplyCommand,
    ) -> Result<CommandResult> {
        let mut mutations = MutationLog::default();
        let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);

        let execution = match command {
            ApplyCommand::FillStroke(command) => self.apply_fill_stroke(
                frame,
                &mut *deps.gpu,
                &mut surface_edit,
                &mut *deps.scene,
                &mut *deps.selections,
                &mut *deps.scratch,
                command,
            )?,
            ApplyCommand::OneShot {
                target,
                surfaces,
                mask,
                operation,
                params,
                active_selection,
                damage,
            } => {
                let execution = self.apply_one_shot(
                    frame,
                    &mut *deps.gpu,
                    &mut surface_edit,
                    &mut *deps.scene,
                    &mut *deps.selections,
                    &mut *deps.scratch,
                    &mut *deps.scene_capture,
                    deps.scene_capture_pipelines,
                    target,
                    &surfaces,
                    &mask,
                    &operation,
                    params,
                    &active_selection,
                    damage.as_ref(),
                )?;
                if execution.applied {
                    surface_edit.record_committed_target_damage(target, damage);
                }
                execution
            }
            ApplyCommand::Decal {
                plan,
                image,
                projection,
                opacity,
                active_selection,
            } => {
                let execution = self.apply_decal(
                    frame,
                    &mut *deps.gpu,
                    &mut surface_edit,
                    &mut *deps.scene,
                    &mut *deps.selections,
                    &mut *deps.scratch,
                    &mut *deps.scene_capture,
                    deps.scene_capture_pipelines,
                    &mut *deps.decal_images,
                    &plan,
                    &image,
                    projection,
                    opacity,
                    &active_selection,
                )?;
                if execution.applied {
                    surface_edit.record_committed_surface_damage(
                        &PaintSurfaceSet::from_vec(plan.surfaces()),
                        Some(plan.damage()),
                    );
                }
                execution
            }
        };

        drop(surface_edit);
        let mut result = CommandResult::new(mutations, execution.metrics);
        if !execution.newly_active_paint_surfaces.is_empty() {
            result = result.with_active_paint_surfaces(
                execution.newly_active_paint_surfaces,
                execution.active_paint_surfaces,
            );
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_fill_stroke(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        scene: &mut SceneResources,
        selections: &mut SelectionMasks,
        scratch: &mut TransientTextures,
        command: FillStrokeCommand,
    ) -> Result<ApplyExecution> {
        match command {
            FillStrokeCommand::Begin {
                operation,
                params,
                active_selection,
            } => {
                if self.active_fill.is_some() {
                    bail!("fill stroke already active");
                }
                self.active_fill = Some(FillApplySession::new(operation, params, active_selection));
                Ok(ApplyExecution::default())
            }
            FillStrokeCommand::Extend {
                deltas,
                preview_damage,
            } => {
                let Some(mut session) = self.active_fill.take() else {
                    bail!("fill stroke extend without active session");
                };
                let result = self.extend_fill_stroke(
                    frame,
                    gpu,
                    surface_edit,
                    selections,
                    scratch,
                    &mut session,
                    deltas,
                    preview_damage,
                );
                self.active_fill = Some(session);
                result
            }
            FillStrokeCommand::End { damage } => {
                let Some(session) = self.active_fill.take() else {
                    bail!("fill stroke end without active session");
                };
                let result = self.end_fill_stroke(
                    frame,
                    gpu,
                    surface_edit,
                    scene,
                    scratch,
                    &session,
                    damage,
                );
                if result.is_err() {
                    self.active_fill = Some(session);
                }
                result
            }
            FillStrokeCommand::Cancel => {
                let Some(session) = self.active_fill.take() else {
                    return Ok(ApplyExecution::default());
                };
                let result = self.cancel_fill_stroke(frame, gpu, surface_edit, scratch, &session);
                if result.is_err() {
                    self.active_fill = Some(session);
                }
                result
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn extend_fill_stroke(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        selections: &mut SelectionMasks,
        scratch: &mut TransientTextures,
        session: &mut FillApplySession,
        deltas: Vec<crate::renderer::FillCoverageDelta>,
        preview_damage: Option<DamageMap>,
    ) -> Result<ApplyExecution> {
        let mut metrics = RenderMetrics::default();
        let mut touched = Vec::new();
        let mut newly_active = Vec::new();
        let comp_u = composite_uniform_from_apply_operation(&session.operation, session.params);
        for delta in deltas {
            let surface = delta.surface;
            prepare_surface_for_operation(frame, gpu, surface_edit, &mut metrics, surface)?;
            let Some(texture_size) = surface_edit.surface_texture_size(surface) else {
                continue;
            };
            let update = session.extend(delta, texture_size);
            let material_index = surface.material_index().as_usize();
            fill_transient::ensure_material_source_uv(
                scratch,
                gpu.device(),
                frame.encoder(),
                material_index,
                texture_size,
            );
            fill_transient::ensure_material_coverage_uv(
                scratch,
                gpu.device(),
                frame.encoder(),
                material_index,
                texture_size,
            );
            if update.first_touch {
                let Some(target) = surface_edit.edit_surface_target(surface) else {
                    continue;
                };
                let Some(source) =
                    fill_transient::material_source_uv_texture(scratch, material_index)
                else {
                    continue;
                };
                copy_a_to_b(frame.encoder(), target.texture, source, texture_size);
            }
            let Some(coverage_view) =
                fill_transient::material_coverage_uv_view(scratch, material_index)
            else {
                continue;
            };
            clear_rgba_target(
                frame.encoder(),
                coverage_view,
                [0.0, 0.0, 0.0, 0.0],
                "clear_fill_coverage",
            );
            match &update.coverage {
                FillCoverage::Full => clear_rgba_target(
                    frame.encoder(),
                    coverage_view,
                    [1.0, 1.0, 1.0, 1.0],
                    "fill_full_coverage",
                ),
                FillCoverage::Triangles(triangles) => {
                    if let Some((rect, pixels)) =
                        triangle_geometry_mask_rect_r8(triangles, texture_size)
                    {
                        if let Some(texture) =
                            fill_transient::material_coverage_uv_texture(scratch, material_index)
                        {
                            frame.write_texture_r8(
                                gpu.device(),
                                texture,
                                rect.origin,
                                rect.size,
                                &pixels,
                            );
                        }
                    }
                }
            }
            let Some(source_texture) =
                fill_transient::material_source_uv_texture(scratch, material_index)
            else {
                continue;
            };
            let Some(source_view) =
                fill_transient::material_source_uv_view(scratch, material_index)
            else {
                continue;
            };
            let Some(target) = surface_edit.edit_surface_target(surface) else {
                continue;
            };
            if record_masked_paint_apply_with_resources(
                frame,
                gpu,
                &self.pipelines,
                selections,
                material_index,
                coverage_view,
                source_texture,
                source_view,
                target.texture,
                target.view,
                texture_size,
                &comp_u,
                Some(&session.active_selection),
            ) {
                surface_edit.resolve_mask_edit_proxy_into_frame(gpu, frame, surface)?;
                if update.first_touch && !newly_active.contains(&surface) {
                    newly_active.push(surface);
                }
                if !touched.contains(&surface) {
                    touched.push(surface);
                }
            }
        }
        let touched = PaintSurfaceSet::from_vec(touched);
        if !touched.is_empty() {
            surface_edit.record_preview_damage(&touched, preview_damage);
        }
        Ok(ApplyExecution::new(metrics, !touched.is_empty())
            .with_active_paint_surfaces(newly_active, session.surfaces()))
    }

    #[allow(clippy::too_many_arguments)]
    fn end_fill_stroke(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        scene: &mut SceneResources,
        scratch: &mut TransientTextures,
        session: &FillApplySession,
        damage: Option<DamageMap>,
    ) -> Result<ApplyExecution> {
        let mut metrics = RenderMetrics::default();
        let surfaces = PaintSurfaceSet::from_vec(session.surfaces());
        for (surface, state) in session.surface_entries() {
            if state.needs_geometry_dilation() {
                prepare_surface_for_operation(frame, gpu, surface_edit, &mut metrics, surface)?;
                let material_index = surface.material_index().as_usize();
                let Some(mask_view) =
                    fill_transient::material_coverage_uv_view(scratch, material_index)
                else {
                    continue;
                };
                let Some(source_texture) =
                    fill_transient::material_source_uv_texture(scratch, material_index)
                else {
                    continue;
                };
                let Some(source_view) =
                    fill_transient::material_source_uv_view(scratch, material_index)
                else {
                    continue;
                };
                let Some(target) = surface_edit.edit_surface_target(surface) else {
                    continue;
                };
                self.uv_island_bleed.record(
                    gpu,
                    &self.surface_edit_resources,
                    scene,
                    &self.surface_edit_pipelines,
                    frame,
                    SurfaceEditTarget {
                        material_index,
                        texture_size: state.texture_size,
                        write_texture: target.texture,
                        write_view: target.view,
                        read_texture: source_texture,
                        read_view: source_view,
                    },
                    EditMask { view: mask_view },
                );
                surface_edit.resolve_mask_edit_proxy_into_frame(gpu, frame, surface)?;
            }
        }
        if !surfaces.is_empty() {
            surface_edit.record_committed_surface_damage(&surfaces, damage);
        }
        retain_fill_session_textures(scratch, frame, session);
        Ok(ApplyExecution::new(metrics, !surfaces.is_empty()))
    }

    fn cancel_fill_stroke(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        scratch: &mut TransientTextures,
        session: &FillApplySession,
    ) -> Result<ApplyExecution> {
        let mut metrics = RenderMetrics::default();
        let surfaces = PaintSurfaceSet::from_vec(session.surfaces());
        for surface in surfaces.iter() {
            prepare_surface_for_operation(frame, gpu, surface_edit, &mut metrics, surface)?;
            let material_index = surface.material_index().as_usize();
            let Some(source) = fill_transient::material_source_uv_texture(scratch, material_index)
            else {
                continue;
            };
            let Some(target) = surface_edit.edit_surface_target(surface) else {
                continue;
            };
            copy_a_to_b(frame.encoder(), source, target.texture, target.texture_size);
            surface_edit.resolve_mask_edit_proxy_into_frame(gpu, frame, surface)?;
        }
        if !surfaces.is_empty() {
            surface_edit.record_cancelled_surfaces(&surfaces);
        }
        retain_fill_session_textures(scratch, frame, session);
        Ok(ApplyExecution::new(metrics, !surfaces.is_empty()))
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_decal(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        scene: &mut SceneResources,
        selections: &mut SelectionMasks,
        scratch: &mut TransientTextures,
        scene_capture: &mut SceneCapture,
        scene_capture_pipelines: &SceneCapturePipelines,
        decal_images: &mut DecalImageCache,
        plan: &DecalApplyPlan,
        image: &crate::core::decal::DecalImageAsset,
        projection: crate::core::decal::DecalProjection,
        opacity: f32,
        active_selection: &ActiveSelection,
    ) -> Result<ApplyExecution> {
        let mut metrics = RenderMetrics::default();
        record_decal_plan_metrics(&mut metrics, plan);
        let surfaces = plan.surfaces();
        prepare_surfaces_for_operation(frame, gpu, surface_edit, &mut metrics, &surfaces)?;
        let mut apply_deps = PaintApplyDeps {
            gpu,
            pipelines: &self.pipelines,
            surfaces: surface_edit,
            scene,
            scratch,
            selections,
            scene_capture,
            scene_capture_pipelines,
            viewport_polygon_coverage: None,
        };
        let decal_depth_generation = apply_deps.scene_capture.decal_depth_generation();
        let applied = record_decal_apply(
            frame,
            &mut apply_deps,
            &mut self.decal_resources,
            decal_images,
            &self.surface_edit_resources,
            &self.surface_edit_pipelines,
            &mut self.uv_island_bleed,
            DecalApplyRequest {
                plan,
                image,
                projection,
                opacity,
                active_selection,
            },
        )?;
        if apply_deps.scene_capture.decal_depth_generation() != decal_depth_generation {
            metrics.decal_depth_target_recreate_count =
                metrics.decal_depth_target_recreate_count.saturating_add(1);
        }
        Ok(ApplyExecution::new(metrics, applied))
    }

    fn apply_one_shot(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        surface_edit: &mut SurfaceEditContext<'_>,
        scene: &mut SceneResources,
        selections: &mut SelectionMasks,
        scratch: &mut TransientTextures,
        scene_capture: &mut SceneCapture,
        scene_capture_pipelines: &SceneCapturePipelines,
        target: PaintSurfaceId,
        surfaces: &[PaintSurfaceId],
        mask: &MaskSource,
        operation: &ApplyOperation,
        params: ApplyParams<TextureCompositeMode>,
        active_selection: &ActiveSelection,
        _damage: Option<&DamageMap>,
    ) -> Result<ApplyExecution> {
        let surfaces = surfaces.to_vec();
        let requests = plan_one_shot_masked_apply_requests(
            target,
            &surfaces,
            mask,
            operation,
            params,
            active_selection,
        )?;
        crate::renderer::features::apply::masked::preflight_one_shot_requests(&requests)?;

        let mut metrics = RenderMetrics::default();
        prepare_surface_for_operation(frame, gpu, surface_edit, &mut metrics, target)?;
        prepare_surfaces_for_operation(frame, gpu, surface_edit, &mut metrics, &surfaces)?;

        let viewport_polygon_coverage = match mask {
            MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(polygon)) => {
                prepare_viewport_polygon_coverage(frame, gpu.device(), polygon)
            }
            _ => None,
        };
        let mut apply_deps = PaintApplyDeps {
            gpu,
            pipelines: &self.pipelines,
            surfaces: surface_edit,
            scene,
            scratch,
            selections,
            scene_capture,
            scene_capture_pipelines,
            viewport_polygon_coverage,
        };

        let mut any_applied = false;
        for request in requests {
            let applied = record_one_shot_masked_apply(frame, &mut apply_deps, &request)?;
            if let Some(applied) = applied {
                if applied.needs_geometry_dilation {
                    record_geometry_dilation(
                        frame,
                        &mut apply_deps,
                        &self.surface_edit_resources,
                        &self.surface_edit_pipelines,
                        &mut self.uv_island_bleed,
                        applied,
                    );
                }
                apply_deps.surfaces.resolve_mask_edit_proxy_into_frame(
                    &mut *apply_deps.gpu,
                    frame,
                    request.target,
                )?;
            }
            any_applied |= applied.is_some();
        }

        if let Some(coverage) = apply_deps.viewport_polygon_coverage.take() {
            coverage.retain_until_submit(frame);
        }

        Ok(ApplyExecution::new(metrics, any_applied))
    }
}

fn retain_fill_session_textures(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    session: &FillApplySession,
) {
    let mut materials = session
        .surfaces()
        .into_iter()
        .map(|surface| surface.material_index().as_usize())
        .collect::<Vec<_>>();
    materials.sort_unstable();
    materials.dedup();
    for material_index in materials {
        fill_transient::retain_material_textures_until_submit(scratch, frame, material_index);
    }
}

fn record_geometry_dilation(
    frame: &mut GpuFrame,
    deps: &mut PaintApplyDeps<'_, '_>,
    surface_edit_resources: &SurfaceEditResources,
    surface_edit_pipelines: &SurfaceEditPipelines,
    uv_island_bleed: &mut UvIslandBleed,
    applied: crate::renderer::features::apply::masked::OneShotMaskedApplyResult,
) {
    let Some(edit_mask_view) =
        stroke_transient::material_stroke_uv_view(deps.scratch, applied.material_index)
    else {
        return;
    };
    let Some(layer) = deps
        .surfaces
        .stroke_surface_target(applied.target, deps.scratch)
    else {
        return;
    };
    debug_assert_eq!(layer.texture_size, applied.texture_size);
    uv_island_bleed.record(
        &*deps.gpu,
        surface_edit_resources,
        deps.scene,
        surface_edit_pipelines,
        frame,
        SurfaceEditTarget {
            material_index: applied.material_index,
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

fn record_decal_plan_metrics(metrics: &mut RenderMetrics, plan: &DecalApplyPlan) {
    metrics.decal_plan_build_time_us = metrics
        .decal_plan_build_time_us
        .saturating_add(plan.metrics.build_time_us);
    metrics.decal_candidate_triangle_count = metrics
        .decal_candidate_triangle_count
        .saturating_add(plan.metrics.candidate_triangle_count);
    metrics.decal_intersection_count = metrics
        .decal_intersection_count
        .saturating_add(plan.metrics.intersection_count);
    metrics.decal_target_count = metrics
        .decal_target_count
        .saturating_add(plan.targets.len());
    metrics.decal_depth_index_range_count = metrics
        .decal_depth_index_range_count
        .saturating_add(plan.depth_index_ranges.len());

    for target in &plan.targets {
        metrics.decal_footprint_rect_count = metrics
            .decal_footprint_rect_count
            .saturating_add(target.footprint_rects.len());
        metrics.decal_damage_rect_count = metrics
            .decal_damage_rect_count
            .saturating_add(target.damage_rects.len());
        metrics.decal_draw_batch_count = metrics
            .decal_draw_batch_count
            .saturating_add(target.draw_batches.len());
        metrics.decal_draw_index_range_count = metrics
            .decal_draw_index_range_count
            .saturating_add(target.draw_index_range_count());
        metrics.decal_draw_call_count = metrics
            .decal_draw_call_count
            .saturating_add(target.draw_call_count());
    }
}

fn prepare_surface_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surface: PaintSurfaceId,
) -> Result<()> {
    let stats = surface_edit.prepare_edit_surface_for_frame(gpu, frame, surface)?;
    record_surface_prepare_stats(metrics, stats);
    Ok(())
}

fn prepare_surfaces_for_operation(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    surface_edit: &mut SurfaceEditContext<'_>,
    metrics: &mut RenderMetrics,
    surfaces: &[PaintSurfaceId],
) -> Result<()> {
    for surface in surfaces {
        prepare_surface_for_operation(frame, gpu, surface_edit, metrics, *surface)?;
    }
    Ok(())
}

fn record_surface_prepare_stats(metrics: &mut RenderMetrics, stats: SurfacePrepareStats) {
    record_surface_prepare_metrics(metrics, &stats);
}
