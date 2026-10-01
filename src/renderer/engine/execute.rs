use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{render_report::RenderMetrics, stroke::PaintSurfaceSet, surface::PaintSurfaceId},
    renderer::{
        ColorSampleTarget, EditCommand, GpuDocumentCommand, RendererFramePlan,
        SelectionEditCommand, ViewCommand,
        command::StrokeCommand,
        features::{
            apply::feature::ApplyFeatureDeps, brush::deps::BrushFeatureDeps,
            filter::feature::FilterFeatureDeps, selection::feature::SelectionFeatureDeps,
            transform::TransformFeatureDeps,
        },
        gpu::frame::GpuFrame,
        mutation::{MutationLog, SurfaceDamage, presentation::PresentDirty},
        presentation::{OutputRequests, UvViewOutputRequest, ViewportOutputRequest},
        report::{
            CommandResult, RenderChanges, RenderExecutionReport, record_surface_prepare_metrics,
        },
    },
};

use super::core::RenderEngine;

impl RenderEngine {
    pub(super) fn submit_frame(&mut self, frame: GpuFrame) -> wgpu::SubmissionIndex {
        self.state
            .submissions
            .submit_frame(self.state.gpu.ctx.queue(), frame)
    }

    pub fn execute(&mut self, plan: RendererFramePlan) -> RenderExecutionReport {
        let has_commands = !plan.is_empty();
        let color_sample_request = plan.color_sample_request;
        let mut metrics = RenderMetrics {
            effect_batches: 1,
            executed_effect_count: plan.command_count(),
            ..RenderMetrics::default()
        };
        let mut changes = RenderChanges::default();
        let commit_requested = plan
            .commit_request
            .as_ref()
            .map_or(false, |request| !request.is_empty());
        if commit_requested {
            if let Err(err) = self.ensure_no_outstanding_commit() {
                metrics.merge(self.take_metrics());
                return RenderExecutionReport::error(
                    format!("Renderer commit gate failed: {err:#}"),
                    metrics,
                    changes,
                );
            }
        }
        let device = self.state.gpu.ctx.device().clone();
        let mut batch_frame = has_commands.then(|| {
            self.state
                .submissions
                .create_frame(&device, "renderer_execute")
        });
        let mut mutations = MutationLog::default();
        let mut diagnostic_readbacks = Vec::new();
        let mut output_requests = OutputRequests::default();
        if has_commands {
            self.state.document.surfaces.begin_residency_epoch();
        }

        for command in plan.document_commands {
            let frame = batch_frame
                .as_mut()
                .expect("non-empty render batch must have an active frame");
            let command_result = self.execute_document_plan_command(frame, command);
            match command_result {
                Ok(command_result) => {
                    let (command_mutations, command_metrics, command_diagnostics) =
                        command_result.into_parts();
                    metrics.merge(command_metrics);
                    mutations.merge(command_mutations);
                    diagnostic_readbacks.extend(command_diagnostics);
                }
                Err(err) => {
                    // A failed command may already have recorded partial GPU work
                    // into the batch frame. Do not submit that frame: dropping it
                    // keeps the GPU from observing a partially recorded command
                    // buffer. Renderer mutations are also left unapplied because
                    // the batch did not reach the normal mutation/presentation
                    // boundary.
                    drop(batch_frame.take());
                    self.abort_active_edits();
                    metrics.merge(self.take_metrics());
                    return RenderExecutionReport::error(
                        format!("Renderer command failed: {err:#}"),
                        metrics,
                        changes,
                    );
                }
            }
        }

        for command in plan.edit_commands {
            let frame = batch_frame
                .as_mut()
                .expect("non-empty render batch must have an active frame");
            let command_result = self.execute_edit_command(frame, command);
            match command_result {
                Ok(command_result) => {
                    let (command_mutations, command_metrics, command_diagnostics) =
                        command_result.into_parts();
                    metrics.merge(command_metrics);
                    mutations.merge(command_mutations);
                    diagnostic_readbacks.extend(command_diagnostics);
                }
                Err(err) => {
                    drop(batch_frame.take());
                    self.abort_active_edits();
                    metrics.merge(self.take_metrics());
                    return RenderExecutionReport::error(
                        format!("Renderer command failed: {err:#}"),
                        metrics,
                        changes,
                    );
                }
            }
        }

        for request in plan.view_requests {
            push_view_request_outputs(&mut output_requests, request);
        }

        // Frame execution is phase-oriented: explicit view requests and
        // mutation-derived composite requests are prepared after the aggregate
        // mutation phase.
        let (mut present_dirty, mutation_metrics) = self.apply_mutation_log(&mutations);
        metrics.merge(mutation_metrics);
        self.features.composite.request_outputs_for_mutations(
            &self.state.document,
            &mutations,
            &mut output_requests,
        );
        if let Some(ColorSampleTarget::CompositeTexture { material_index, .. }) =
            color_sample_request.map(|request| request.target)
        {
            output_requests.push_material_composite(material_index);
        }
        present_dirty.merge(output_requests.present_dirty());
        if let Some(frame) = batch_frame.as_mut() {
            match self.prepare_required_outputs(frame, present_dirty, output_requests) {
                Ok(output_metrics) => metrics.merge(output_metrics),
                Err(err) => {
                    drop(batch_frame.take());
                    self.abort_active_edits();
                    metrics.merge(self.take_metrics());
                    return RenderExecutionReport::error(
                        format!("Renderer output preparation failed: {err:#}"),
                        metrics,
                        changes,
                    );
                }
            }
        }
        if let (Some(frame), Some(request)) = (batch_frame.as_mut(), color_sample_request) {
            self.enqueue_color_sample(frame, request);
        }
        let started_commit = if let Some(frame) = batch_frame.as_mut() {
            match self
                .enqueue_surface_commit_readbacks(
                    frame,
                    &mutations,
                    plan.commit_request.as_ref(),
                    diagnostic_readbacks,
                )
                .map_err(|error| format!("{error:#}"))
            {
                Ok(started_commit) => Ok(started_commit),
                Err(error) => {
                    drop(batch_frame.take());
                    self.abort_active_edits();
                    metrics.merge(self.take_metrics());
                    return RenderExecutionReport::error(
                        format!("Renderer commit readback enqueue failed: {error}"),
                        metrics,
                        changes,
                    );
                }
            }
        } else {
            Ok(None)
        };
        if let Some(frame) = batch_frame.take() {
            self.submit_frame(frame);
        }
        metrics.layer_gpu_eviction_count = metrics.layer_gpu_eviction_count.saturating_add(
            self.state
                .document
                .surfaces
                .evict_unused_resident_surfaces(),
        );

        changes.observe_mutations(&mutations);
        changes.request_present(present_dirty);
        metrics.merge(self.take_metrics());
        log_surface_paint_metrics(&metrics);
        RenderExecutionReport::ok_with_started_commit(metrics, changes, started_commit)
    }

    fn abort_active_edits(&mut self) {
        self.features.apply.abort();
        self.features.brush.abort();
        self.features.transform.abort();
    }

    fn execute_document_plan_command(
        &mut self,
        frame: &mut GpuFrame,
        command: GpuDocumentCommand,
    ) -> Result<CommandResult> {
        match command {
            GpuDocumentCommand::SyncEmbeddedImages { images } => {
                self.state.document.embedded_images.replace(images);
                Ok(CommandResult::default())
            }
            GpuDocumentCommand::UpsertEmbeddedImage { image } => {
                self.state.document.embedded_images.upsert(image);
                Ok(CommandResult::default())
            }
            GpuDocumentCommand::RemoveEmbeddedImage { image_id } => {
                self.state.document.embedded_images.remove(image_id);
                Ok(CommandResult::default())
            }
            GpuDocumentCommand::SetEmbeddedImagePreview {
                layer_id,
                material_index,
                transform,
                damage,
            } => {
                self.state
                    .document
                    .embedded_images
                    .set_preview(layer_id, transform);
                let mut mutations = MutationLog::default();
                if let Some(damage) = damage {
                    mutations
                        .composites
                        .layer_value_rect(material_index, damage);
                }
                Ok(CommandResult::from_mutations(mutations))
            }
            GpuDocumentCommand::UploadScene { .. }
            | GpuDocumentCommand::AppendMaterials { .. }
            | GpuDocumentCommand::ReplaceMesh { .. }
            | GpuDocumentCommand::SetMaterialRenderSettings { .. }
            | GpuDocumentCommand::ResizeMaterialTexture { .. }
            | GpuDocumentCommand::CreateSurface { .. }
            | GpuDocumentCommand::DeleteSurface { .. }
            | GpuDocumentCommand::DuplicateSurface { .. }
            | GpuDocumentCommand::UploadSurfaceRgba8 { .. }
            | GpuDocumentCommand::UploadSurfaceTiles { .. } => {
                self.execute_document_command(frame, command)
            }
            GpuDocumentCommand::UploadSelectionTiles {
                active_selection,
                tiles,
            } => self.upload_selection_tiles(frame, active_selection, tiles),
            GpuDocumentCommand::SyncMaterialTree {
                material_index,
                tree,
            } => Ok(CommandResult::from_mutations(
                self.state.document.sync_material_tree(material_index, tree),
            )),
            GpuDocumentCommand::UpdateLayerProps { layer_id, props } => {
                Ok(CommandResult::from_mutations(
                    self.state
                        .document
                        .update_composite_layer_props(layer_id, props),
                ))
            }
            GpuDocumentCommand::UpdateAdjustment {
                layer_id,
                adjustment,
            } => Ok(CommandResult::from_mutations(
                self.state
                    .document
                    .update_composite_adjustment(layer_id, adjustment),
            )),
        }
    }

    fn execute_edit_command(
        &mut self,
        frame: &mut GpuFrame,
        command: EditCommand,
    ) -> Result<CommandResult> {
        match command {
            EditCommand::Stroke(command) => self.execute_stroke_command(frame, command),
            EditCommand::Apply(command) => self.execute_apply_command(frame, command),
            EditCommand::Composite(command) => {
                let features = &mut self.features;
                features.composite.execute_command(
                    frame,
                    &self.state.gpu,
                    &mut self.state.document,
                    &mut self.state.transient,
                    command,
                )
            }
            EditCommand::Filter(command) => {
                let features = &mut self.features;
                let mut deps = FilterFeatureDeps {
                    gpu: &mut self.state.gpu,
                    surfaces: &mut self.state.document.surfaces,
                    materials: &self.state.document.materials,
                    selections: &self.state.document.selections,
                };
                features.filter.execute(frame, &mut deps, command)
            }
            EditCommand::Selection(command) => self.execute_selection_edit_command(frame, command),
            EditCommand::Transform(command) => {
                let features = &mut self.features;
                let mut deps = TransformFeatureDeps {
                    gpu: &mut self.state.gpu,
                    surfaces: &mut self.state.document.surfaces,
                    selections: &mut self.state.document.selections,
                };
                features.transform.execute(frame, &mut deps, command)
            }
        }
    }

    fn upload_selection_tiles(
        &mut self,
        frame: &mut GpuFrame,
        active_selection: crate::core::selection::ActiveSelection,
        tiles: Vec<crate::core::selection::SelectionTilePayload>,
    ) -> Result<CommandResult> {
        let device = self.state.gpu.ctx.device().clone();
        self.features.selection.upload_tiles(
            frame,
            &device,
            &mut self.state.document,
            active_selection,
            tiles,
        )
    }

    fn execute_apply_command(
        &mut self,
        frame: &mut GpuFrame,
        command: crate::renderer::ApplyCommand,
    ) -> Result<CommandResult> {
        let mut result = {
            let features = &mut self.features;
            let mut deps = ApplyFeatureDeps {
                gpu: &mut self.state.gpu,
                surfaces: &mut self.state.document.surfaces,
                scene: &mut self.state.document.scene,
                selections: &mut self.state.document.selections,
                scratch: &mut self.state.transient,
                scene_capture: &mut self.scene_capture,
                scene_capture_pipelines: &features.scene_capture_pipelines,
                decal_images: &mut features.decal_images,
            };
            features.apply.execute(frame, &mut deps, command)?
        };
        if result.has_newly_active_paint_surfaces() {
            let metrics = self.prime_active_working_sets_for_new_surfaces(
                frame,
                &result.newly_active_paint_surfaces,
                &result.active_paint_surfaces,
            )?;
            result.metrics.merge(metrics);
        }
        Ok(result)
    }

    fn execute_selection_edit_command(
        &mut self,
        frame: &mut GpuFrame,
        command: SelectionEditCommand,
    ) -> Result<CommandResult> {
        let features = &mut self.features;
        let mut deps = SelectionFeatureDeps {
            gpu: &mut self.state.gpu,
            selections: &mut self.state.document.selections,
            materials: &self.state.document.materials,
            scene: &mut self.state.document.scene,
            scratch: &mut self.state.transient,
            scene_capture: &mut self.scene_capture,
            scene_capture_pipelines: &features.scene_capture_pipelines,
            projection_pipelines: features.apply.pipelines(),
        };
        features
            .selection
            .execute_edit_command(frame, &mut deps, command)
    }

    fn execute_stroke_command(
        &mut self,
        frame: &mut GpuFrame,
        command: StrokeCommand,
    ) -> Result<CommandResult> {
        let prime_surfaces = match &command {
            StrokeCommand::Begin { target, .. } => Some(target.surfaces()),
            StrokeCommand::AddDabs { .. } | StrokeCommand::End { .. } | StrokeCommand::Cancel => {
                None
            }
        };
        let mut result = {
            let features = &mut self.features;
            let mut deps = BrushFeatureDeps {
                brush_engines: &features.brush_engines,
                gpu: &mut self.state.gpu,
                materials: &self.state.document.materials,
                surfaces: &mut self.state.document.surfaces,
                scene: &mut self.state.document.scene,
                selections: &mut self.state.document.selections,
                scratch: &mut self.state.transient,
                scene_capture: &mut self.scene_capture,
                scene_capture_pipelines: &features.scene_capture_pipelines,
            };
            features.brush.execute(frame, &mut deps, command)?
        };
        if let Some(prime_surfaces) = prime_surfaces {
            let metrics = self.prime_active_working_sets_for_surfaces(frame, &prime_surfaces)?;
            result.metrics.merge(metrics);
        }
        if result.has_newly_active_paint_surfaces() {
            let metrics = self.prime_active_working_sets_for_new_surfaces(
                frame,
                &result.newly_active_paint_surfaces,
                &result.active_paint_surfaces,
            )?;
            result.metrics.merge(metrics);
        }
        Ok(result)
    }

    fn prime_active_working_sets_for_surfaces(
        &mut self,
        frame: &mut GpuFrame,
        surfaces: &PaintSurfaceSet,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        for active_surfaces in active_surface_sets_by_material(surfaces) {
            let prime_metrics = self.features.composite.prime_active_run_working_set(
                frame,
                &self.state.gpu,
                &mut self.state.document,
                &mut self.state.transient,
                active_surfaces,
            )?;
            metrics.merge(prime_metrics);
        }
        Ok(metrics)
    }

    fn prime_active_working_sets_for_new_surfaces(
        &mut self,
        frame: &mut GpuFrame,
        newly_active_surfaces: &[PaintSurfaceId],
        active_surfaces: &[PaintSurfaceId],
    ) -> Result<RenderMetrics> {
        let mut material_indices = Vec::new();
        for surface in newly_active_surfaces {
            let material_index = surface.material_index().as_usize();
            if !material_indices.contains(&material_index) {
                material_indices.push(material_index);
            }
        }
        let mut metrics = RenderMetrics::default();
        metrics.active_composite_incremental_prime_calls = 1;
        metrics.active_composite_incremental_prime_surfaces = newly_active_surfaces.len();
        metrics.active_composite_incremental_prime_materials = material_indices.len();
        for material_index in material_indices {
            let surfaces_for_material: Vec<_> = active_surfaces
                .iter()
                .copied()
                .filter(|surface| surface.material_index().as_usize() == material_index)
                .collect();
            if surfaces_for_material.is_empty() {
                continue;
            }
            let prime_metrics = self.features.composite.prime_active_run_working_set(
                frame,
                &self.state.gpu,
                &mut self.state.document,
                &mut self.state.transient,
                PaintSurfaceSet::from_vec(surfaces_for_material),
            )?;
            metrics.merge(prime_metrics);
        }
        Ok(metrics)
    }

    /// Applies renderer-owned state transitions derived from a mutation log.
    ///
    /// Normal execution applies command mutations once at the batch boundary so
    /// composite cache invalidation and presentation dirty derivation observe
    /// the same aggregate mutation log that is reported to the host.
    pub(super) fn apply_mutation_log(
        &mut self,
        mutations: &MutationLog,
    ) -> (PresentDirty, RenderMetrics) {
        let mut metrics = RenderMetrics::default();
        self.commit_persistent_surface_damage(mutations, &mut metrics);
        self.features.composite.invalidate_mutations(mutations);
        (mutations.present_dirty(), metrics)
    }

    fn commit_persistent_surface_damage(
        &mut self,
        mutations: &MutationLog,
        metrics: &mut RenderMetrics,
    ) {
        for damage in mutations.surface_commits.iter() {
            let commit = match damage {
                SurfaceDamage::Preview { .. }
                | SurfaceDamage::Transient { .. }
                | SurfaceDamage::Cancelled { .. } => continue,
                SurfaceDamage::Full { surface } => {
                    self.state.document.surfaces.commit_full_damage(*surface)
                }
                SurfaceDamage::Rects { surface, damage } => self
                    .state
                    .document
                    .surfaces
                    .commit_damage(*surface, Some(damage)),
            };
            record_surface_prepare_metrics(metrics, &commit.prepare);
        }
        for damage in mutations.surfaces.iter() {
            let (surface, damage) = match damage {
                SurfaceDamage::Preview { surface, damage } => (*surface, damage.as_ref()),
                SurfaceDamage::Transient { surface, damage } => (*surface, Some(damage)),
                SurfaceDamage::Cancelled { surface } => {
                    // A synchronous diagnostic readback can refresh the renderer tile shadow
                    // with transient preview pixels. Cancellation restores the GPU texture from
                    // the preview base snapshot, so invalidate that shadow again before it can
                    // be used as if it still matched the resident texture.
                    let commit = self.state.document.surfaces.commit_full_damage(*surface);
                    record_surface_prepare_metrics(metrics, &commit.prepare);
                    continue;
                }
                _ => continue,
            };
            // Preview and transient edits already mutate the resident GPU texture.
            // Mark only the changed footprint stale when it is known, so residency
            // cannot rehydrate old pixels before the edit is finalized or cancelled.
            let commit = match damage {
                Some(damage) if !damage.is_empty() => self
                    .state
                    .document
                    .surfaces
                    .commit_damage(surface, Some(damage)),
                Some(_) => continue,
                None => self.state.document.surfaces.commit_full_damage(surface),
            };
            record_surface_prepare_metrics(metrics, &commit.prepare);
        }
    }
}

fn log_surface_paint_metrics(metrics: &RenderMetrics) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static FRAME: AtomicU64 = AtomicU64::new(0);
    let enabled = *ENABLED.get_or_init(|| {
        std::env::var("HOLOPAINTER_PAINT_PERF")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
    });
    if !enabled
        || (metrics.surface_projection_batch_count == 0
            && metrics.surface_new_stroke_surface_count == 0)
    {
        return;
    }
    let frame = FRAME.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "[paint-perf] frame={frame} targets={} materials={} new_surfaces={} new_materials={} batches={} dabs={} material_dab_refs={} multi_material_dabs={} brush_passes={} mesh_uv_scissored={} mesh_uv_full={} scissor_rects={} scissor_px={} full_px={} fallback_missing={} fallback_invalid={} fallback_boundary={} fallback_no_contribution={} fallback_full_rect={} fallback_rect_limit={} fallback_area_limit={} damage_partial_rects={} damage_full={} snapshot_copy_bytes={} stroke_clear_bytes={} sync_clipped_bytes={} sync_full_bytes={} composite_partial={} composite_full={} composite_full_fallbacks={}",
        metrics.surface_target_surface_count_max,
        metrics.surface_target_material_count_max,
        metrics.surface_new_stroke_surface_count,
        metrics.surface_new_stroke_material_count,
        metrics.surface_projection_batch_count,
        metrics.surface_dab_count,
        metrics.surface_target_material_dab_reference_count,
        metrics.surface_multi_material_dab_count,
        metrics.surface_brush_pass_count,
        metrics.surface_target_mesh_uv_scissored_pass_count,
        metrics.surface_target_mesh_uv_full_pass_count,
        metrics.surface_target_mesh_uv_scissor_rect_count,
        metrics.surface_target_mesh_uv_scissor_pixel_area,
        metrics.surface_target_mesh_uv_full_pixel_area,
        metrics.surface_scissor_fallback_missing_data,
        metrics.surface_scissor_fallback_invalid_input,
        metrics.surface_scissor_fallback_boundary_lookup,
        metrics.surface_scissor_fallback_no_contribution,
        metrics.surface_scissor_fallback_full_rect,
        metrics.surface_scissor_fallback_rect_limit,
        metrics.surface_scissor_fallback_area_limit,
        metrics.surface_partial_damage_rect_count,
        metrics.surface_full_damage_count,
        metrics.surface_snapshot_copy_bytes,
        metrics.surface_stroke_begin_clear_bytes,
        metrics.surface_clipped_sync_copy_bytes,
        metrics.surface_full_sync_copy_bytes,
        metrics.partial_composite_executions,
        metrics.full_composite_executions,
        metrics.partial_composite_full_fallbacks,
    );
}

fn active_surface_sets_by_material(surfaces: &PaintSurfaceSet) -> Vec<PaintSurfaceSet> {
    let mut grouped: Vec<(usize, Vec<_>)> = Vec::new();
    for surface in surfaces.iter() {
        if let Some((_, group)) = grouped
            .iter_mut()
            .find(|(material_index, _)| *material_index == surface.material_index().as_usize())
        {
            if !group.contains(&surface) {
                group.push(surface);
            }
            continue;
        }
        grouped.push((surface.material_index().as_usize(), vec![surface]));
    }
    grouped
        .into_iter()
        .map(|(_, surfaces)| PaintSurfaceSet::from_vec(surfaces))
        .collect()
}

fn push_view_request_outputs(output_requests: &mut OutputRequests, request: ViewCommand) {
    match request {
        ViewCommand::RenderViewport {
            camera,
            view_proj,
            camera_world,
            viewport_size,
            brush_overlay_request,
            selection_overlay_request,
            mirror_plane_overlay_request,
            decal_overlay_request,
            scene_visibility,
            show_wireframe,
            wireframe_style,
            background_color,
            shading,
        } => output_requests.push_viewport(ViewportOutputRequest {
            camera,
            view_proj,
            camera_world,
            viewport_size,
            brush_overlay_request,
            selection_overlay_request,
            mirror_plane_overlay_request,
            decal_overlay_request,
            scene_visibility,
            show_wireframe,
            wireframe_style,
            background_color,
            shading,
        }),
        ViewCommand::RenderUvView {
            material_index,
            uv_view_size,
            transform,
            brush_overlay_request,
            selection_overlay_request,
            show_wireframe,
            wireframe_style,
            background_color,
        } => output_requests.push_uv_view(UvViewOutputRequest {
            material_index,
            uv_view_size,
            transform,
            brush_overlay_request,
            selection_overlay_request,
            show_wireframe,
            wireframe_style,
            background_color,
        }),
        ViewCommand::RenderToolPreviews(request) => output_requests.push_tool_preview(request),
    }
}
