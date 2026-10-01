use crate::{
    core::{
        adjustment::Adjustment,
        composite::{GroupCompositeMode, LayerBlendMode},
        geometry::RectU32,
        render_report::RenderMetrics,
        surface::{CompositeGroup, CompositeNode, CompositeProps, CompositeTree, PaintSurfaceId},
    },
    renderer::{
        adjustment_gpu::{AdjustmentLutUniform, adjustment_gpu_params},
        document::{GpuDocument, surfaces::SurfacePixelFormat},
        engine::gpu_state::RendererGpuState,
        features::composite::{
            cache::{
                ActiveCompositeHint, ActiveRunBoundaryCheckpointViews, ActiveRunCheckpointViews,
                CompositeCache, CompositeNeed, GeneralCompositeMaterialCache,
                IsolatedGroupCacheEntry,
            },
            graph::{
                ActiveAboveStep, ActiveBoundaryComposite, ActiveCompositePlanMiss,
                ActiveRunBoundaryPlan, ActiveRunLeaf, ActiveRunPlan, ActiveSetRunPlan,
                ActiveSetStep, build_active_set_run_plan, build_flat_normal_active_run_plan,
            },
            pipelines::CompositePipelines,
            planner::{
                GroupDamageDependency, PartialCompositeDecision, group_damage_dependency,
                partial_composite_decision, partial_composite_working_clips,
                root_uv_mirror_adjustment, total_rect_area, visible_surfaces_from_composite_tree,
            },
            source_provider::{
                CompositeSourceProvider, CompositeSourceResolveMode, CompositeSourceTexture,
            },
            types::LayerCompositeUniform,
        },
        gpu::{clear_rgba_target, create_render_scratch_texture, frame::GpuFrame},
        pixel::rgba8_alpha_to_r8,
        transient::{
            TransientLifetime, TransientTextureKey, TransientTextureScope, TransientTextures,
        },
    },
};
use anyhow::{Result, bail};
use eframe::egui_wgpu::wgpu;

/// Coordinates material-level compositing and keeps composite invalidation,
/// residency preparation, and partial/full composite planning out of the
/// renderer facade.
#[derive(Debug, Default)]
pub(crate) struct MaterialCompositor;

impl MaterialCompositor {
    pub(crate) fn prepare_material_composite(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        tree: &CompositeTree,
    ) -> Result<RenderMetrics> {
        let metrics = RenderMetrics::default();
        if material_index >= document.materials.material_count() {
            return Ok(metrics);
        }
        let Some(size) = document.materials.texture_size(material_index) else {
            return Ok(metrics);
        };
        let active_hint = composites.active_hint(material_index);
        composites.ensure_material(gpu_state.device(), material_index, size);
        let need = composites.composite_need(material_index, tree);
        if matches!(need, CompositeNeed::Clean) {
            return Ok(metrics);
        }
        if matches!(need, CompositeNeed::SignatureChanged) {
            composites.record_signature_changed_invalidation(material_index);
        }
        let uses_shader_blend_modes = tree_uses_shader_blend_modes(tree);
        let clips = if matches!(need, CompositeNeed::PartialDirty) {
            partial_composite_working_clips(
                size,
                tree,
                &composites.dirty_state(material_index).rects,
            )
        } else {
            Vec::new()
        };
        if let Some(active_hint) = active_hint
            && matches!(
                need,
                CompositeNeed::PartialDirty
                    | CompositeNeed::FullDirty
                    | CompositeNeed::SignatureChanged
            )
        {
            return self.prepare_active_preview_composite(
                frame,
                gpu_state,
                composite_pipelines,
                document,
                scratch,
                composites,
                material_index,
                tree,
                active_hint,
                need,
                size,
                &clips,
            );
        }
        self.prepare_general_material_composite(
            frame,
            gpu_state,
            composite_pipelines,
            document,
            scratch,
            composites,
            material_index,
            tree,
            need,
            size,
            &clips,
            uses_shader_blend_modes,
        )
    }

    fn prepare_active_preview_composite(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        tree: &CompositeTree,
        active_hint: ActiveCompositeHint,
        need: CompositeNeed,
        size: [u32; 2],
        clips: &[RectU32],
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        if tree_contains_gated_pass_through(tree) || tree_contains_adjustment(tree) {
            return self.prepare_gated_pass_through_active_preview(
                frame,
                gpu_state,
                composite_pipelines,
                document,
                scratch,
                composites,
                material_index,
                tree,
                active_hint,
                need,
                size,
                clips,
            );
        }
        if !active_hint.is_single()
            && let Ok(plan) = build_active_set_run_plan(tree, active_hint.surfaces())
        {
            let checkpoint_count = plan.checkpoint_count();
            if checkpoint_count <= composites.active_run_above_step_checkpoint_budget() {
                let boundary_checkpoint_shape = plan.boundary_checkpoint_shape();
                validate_active_run_checkpoints_ready(
                    composites,
                    material_index,
                    size,
                    active_hint.surfaces(),
                    tree,
                    &boundary_checkpoint_shape,
                    &mut metrics,
                )?;
                validate_active_set_partner_source_caches_ready(
                    &*document,
                    composites,
                    &mut metrics,
                    material_index,
                    size,
                    &active_hint,
                    &plan,
                )?;
                let rendered = {
                    let source_provider = CompositeSourceProvider::new(
                        &*document,
                        Some(active_hint.clone()),
                        Some(composites),
                        CompositeSourceResolveMode::StrokePreviewNoUpload,
                    );
                    let Some(checkpoints) = composites.active_run_checkpoint_views(material_index)
                    else {
                        return self.active_preview_plan_miss(composites, metrics);
                    };
                    let Some(output_view) = composites.output_view(material_index) else {
                        return self.active_preview_plan_miss(composites, metrics);
                    };
                    if matches!(need, CompositeNeed::PartialDirty) {
                        if clips.is_empty() {
                            true
                        } else {
                            let render_result = clips.iter().try_for_each(|clip| {
                                composite_active_set_run_plan_to_view(
                                    &source_provider,
                                    gpu_state,
                                    composite_pipelines,
                                    frame,
                                    material_index,
                                    size,
                                    &plan,
                                    &checkpoints,
                                    output_view,
                                    Some(*clip),
                                )
                            });
                            metrics.merge(source_provider.take_metrics());
                            render_result?;
                            metrics.partial_composite_executions =
                                metrics.partial_composite_executions.saturating_add(1);
                            metrics.partial_composite_rect_count = metrics
                                .partial_composite_rect_count
                                .saturating_add(clips.len());
                            metrics.partial_composite_pixel_area = metrics
                                .partial_composite_pixel_area
                                .saturating_add(total_rect_area(clips));
                            true
                        }
                    } else {
                        let render_result = composite_active_set_run_plan_to_view(
                            &source_provider,
                            gpu_state,
                            composite_pipelines,
                            frame,
                            material_index,
                            size,
                            &plan,
                            &checkpoints,
                            output_view,
                            None,
                        );
                        metrics.merge(source_provider.take_metrics());
                        render_result?;
                        metrics.full_composite_executions =
                            metrics.full_composite_executions.saturating_add(1);
                        true
                    }
                };
                if rendered {
                    composites.mark_clean(material_index, tree);
                }
                return Ok(metrics);
            }
            composites
                .record_stroke_preview_checkpoint_budget_fallback(material_index, checkpoint_count);
            return self.active_preview_plan_miss(composites, metrics);
        }
        if active_hint.is_single()
            && let Some(plan) =
                build_flat_normal_active_run_plan(tree, active_hint.primary_surface())
        {
            let checkpoint_count = plan.checkpoint_count();
            if checkpoint_count <= composites.active_run_above_step_checkpoint_budget() {
                let boundary_checkpoint_shape = plan.boundary_checkpoint_shape();
                validate_active_run_checkpoints_ready(
                    composites,
                    material_index,
                    size,
                    active_hint.surfaces(),
                    tree,
                    &boundary_checkpoint_shape,
                    &mut metrics,
                )?;
                validate_active_run_partner_source_cache_ready(
                    &*document,
                    composites,
                    &mut metrics,
                    material_index,
                    size,
                    &active_hint,
                    &plan,
                )?;
                let rendered = {
                    let source_provider = CompositeSourceProvider::new(
                        &*document,
                        Some(active_hint.clone()),
                        Some(composites),
                        CompositeSourceResolveMode::StrokePreviewNoUpload,
                    );
                    let Some(checkpoints) = composites.active_run_checkpoint_views(material_index)
                    else {
                        return self.active_preview_plan_miss(composites, metrics);
                    };
                    let Some(output_view) = composites.output_view(material_index) else {
                        return self.active_preview_plan_miss(composites, metrics);
                    };
                    if matches!(need, CompositeNeed::PartialDirty) {
                        if clips.is_empty() {
                            true
                        } else {
                            let render_result = clips.iter().try_for_each(|clip| {
                                composite_active_run_plan_to_view(
                                    &source_provider,
                                    gpu_state,
                                    composite_pipelines,
                                    frame,
                                    material_index,
                                    size,
                                    &plan,
                                    &checkpoints,
                                    output_view,
                                    Some(*clip),
                                )
                            });
                            metrics.merge(source_provider.take_metrics());
                            render_result?;
                            metrics.partial_composite_executions =
                                metrics.partial_composite_executions.saturating_add(1);
                            metrics.partial_composite_rect_count = metrics
                                .partial_composite_rect_count
                                .saturating_add(clips.len());
                            metrics.partial_composite_pixel_area = metrics
                                .partial_composite_pixel_area
                                .saturating_add(total_rect_area(clips));
                            true
                        }
                    } else {
                        let render_result = composite_active_run_plan_to_view(
                            &source_provider,
                            gpu_state,
                            composite_pipelines,
                            frame,
                            material_index,
                            size,
                            &plan,
                            &checkpoints,
                            output_view,
                            None,
                        );
                        metrics.merge(source_provider.take_metrics());
                        render_result?;
                        metrics.full_composite_executions =
                            metrics.full_composite_executions.saturating_add(1);
                        true
                    }
                };
                if rendered {
                    composites.mark_clean(material_index, tree);
                }
                return Ok(metrics);
            }
            composites
                .record_stroke_preview_checkpoint_budget_fallback(material_index, checkpoint_count);
            return self.active_preview_plan_miss(composites, metrics);
        }
        self.active_preview_plan_miss(composites, metrics)
    }

    fn prepare_gated_pass_through_active_preview(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        tree: &CompositeTree,
        active_hint: ActiveCompositeHint,
        need: CompositeNeed,
        size: [u32; 2],
        clips: &[RectU32],
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        validate_full_tree_source_caches_ready(
            &*document,
            composites,
            &mut metrics,
            material_index,
            size,
            &active_hint,
            tree,
        )?;
        let partial_decision = if matches!(need, CompositeNeed::PartialDirty) {
            partial_composite_decision(size, tree, clips)
        } else {
            PartialCompositeDecision::FullFallback
        };
        if !matches!(partial_decision, PartialCompositeDecision::Skip) {
            ensure_general_composite_scratch_textures(
                scratch,
                gpu_state,
                frame,
                material_index,
                tree,
                size,
                true,
            );
        }
        let render_result = (|| -> Result<bool> {
            let source_provider = CompositeSourceProvider::new(
                &*document,
                Some(active_hint),
                Some(composites),
                CompositeSourceResolveMode::StrokePreviewNoUpload,
            );
            let Some(output_view) = composites.output_view(material_index) else {
                return Ok(false);
            };
            // Active preview sources can change every frame without general-cache invalidation.
            // Keep this path uncached while still sharing the general composite implementation.
            let mut general_cache = GeneralCompositeMaterialCache::disabled();
            match partial_decision {
                PartialCompositeDecision::Partial => {
                    let result = composite_material_tree_clips_to_view_with_shader_blending(
                        &source_provider,
                        gpu_state,
                        composite_pipelines,
                        frame,
                        &*scratch,
                        material_index,
                        tree,
                        size,
                        output_view,
                        clips,
                        &mut general_cache,
                        &mut metrics,
                    );
                    metrics.merge(source_provider.take_metrics());
                    result?;
                    metrics.partial_composite_executions =
                        metrics.partial_composite_executions.saturating_add(1);
                    metrics.partial_composite_rect_count = metrics
                        .partial_composite_rect_count
                        .saturating_add(clips.len());
                    metrics.partial_composite_pixel_area = metrics
                        .partial_composite_pixel_area
                        .saturating_add(total_rect_area(clips));
                    Ok(true)
                }
                PartialCompositeDecision::FullFallback => {
                    let result = composite_material_tree_to_view_with_shader_blending(
                        &source_provider,
                        gpu_state,
                        composite_pipelines,
                        frame,
                        &*scratch,
                        material_index,
                        tree,
                        size,
                        output_view,
                        None,
                        &mut general_cache,
                        &mut metrics,
                    );
                    metrics.merge(source_provider.take_metrics());
                    result?;
                    metrics.full_composite_executions =
                        metrics.full_composite_executions.saturating_add(1);
                    if matches!(need, CompositeNeed::PartialDirty) {
                        metrics.partial_composite_full_fallbacks =
                            metrics.partial_composite_full_fallbacks.saturating_add(1);
                    }
                    Ok(true)
                }
                PartialCompositeDecision::Skip => Ok(true),
            }
        })();
        if !matches!(partial_decision, PartialCompositeDecision::Skip) {
            release_composite_operation_scratch_textures(scratch, frame, material_index);
        }
        let rendered = render_result?;
        if rendered {
            composites.mark_clean(material_index, tree);
        }
        Ok(metrics)
    }

    fn active_preview_plan_miss(
        &self,
        composites: &mut CompositeCache,
        _metrics: RenderMetrics,
    ) -> Result<RenderMetrics> {
        composites.record_stroke_composite_plan_miss();
        bail!(
            "active stroke preview composite has no no-upload plan; GeneralComposite fallback is disabled"
        )
    }

    fn prepare_general_material_composite(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        tree: &CompositeTree,
        need: CompositeNeed,
        size: [u32; 2],
        clips: &[RectU32],
        uses_shader_blend_modes: bool,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        let partial_decision = if matches!(need, CompositeNeed::PartialDirty) {
            partial_composite_decision(size, tree, &clips)
        } else {
            PartialCompositeDecision::FullFallback
        };
        if !matches!(partial_decision, PartialCompositeDecision::Skip) {
            ensure_general_composite_scratch_textures(
                scratch,
                gpu_state,
                frame,
                material_index,
                tree,
                size,
                uses_shader_blend_modes,
            );
        }
        let render_result = (|| -> Result<bool> {
            let source_provider = CompositeSourceProvider::new(
                &*document,
                None,
                None,
                CompositeSourceResolveMode::GeneralComposite,
            );
            let Some((output_view, general_cache, cache_metrics)) =
                composites.general_composite_parts(material_index)
            else {
                return Ok(false);
            };
            match partial_decision {
                PartialCompositeDecision::Partial => {
                    let render_result = if uses_shader_blend_modes {
                        composite_material_tree_clips_to_view_with_shader_blending(
                            &source_provider,
                            gpu_state,
                            composite_pipelines,
                            frame,
                            &*scratch,
                            material_index,
                            tree,
                            size,
                            output_view,
                            &clips,
                            general_cache,
                            cache_metrics,
                        )
                    } else {
                        clips.iter().try_for_each(|clip| {
                            composite_material_tree_to_view(
                                &source_provider,
                                gpu_state,
                                composite_pipelines,
                                frame,
                                &*scratch,
                                material_index,
                                tree,
                                size,
                                output_view,
                                Some(*clip),
                                general_cache,
                                cache_metrics,
                            )
                        })
                    };
                    metrics.merge(source_provider.take_metrics());
                    render_result?;
                    metrics.partial_composite_executions =
                        metrics.partial_composite_executions.saturating_add(1);
                    metrics.partial_composite_rect_count = metrics
                        .partial_composite_rect_count
                        .saturating_add(clips.len());
                    metrics.partial_composite_pixel_area = metrics
                        .partial_composite_pixel_area
                        .saturating_add(total_rect_area(&clips));
                    Ok(true)
                }
                PartialCompositeDecision::FullFallback => {
                    let render_result = if uses_shader_blend_modes {
                        composite_material_tree_to_view_with_shader_blending(
                            &source_provider,
                            gpu_state,
                            composite_pipelines,
                            frame,
                            &*scratch,
                            material_index,
                            tree,
                            size,
                            output_view,
                            None,
                            general_cache,
                            cache_metrics,
                        )
                    } else {
                        composite_material_tree_to_view(
                            &source_provider,
                            gpu_state,
                            composite_pipelines,
                            frame,
                            &*scratch,
                            material_index,
                            tree,
                            size,
                            output_view,
                            None,
                            general_cache,
                            cache_metrics,
                        )
                    };
                    metrics.merge(source_provider.take_metrics());
                    render_result?;
                    metrics.full_composite_executions =
                        metrics.full_composite_executions.saturating_add(1);
                    if matches!(need, CompositeNeed::PartialDirty) {
                        metrics.partial_composite_full_fallbacks =
                            metrics.partial_composite_full_fallbacks.saturating_add(1);
                    }
                    Ok(true)
                }
                // Empty normalized clips mean every tracked dirty rect was invalid or
                // clipped away, so no GPU work is needed before clearing the dirty state.
                PartialCompositeDecision::Skip => Ok(true),
            }
        })();
        if !matches!(partial_decision, PartialCompositeDecision::Skip) {
            release_composite_operation_scratch_textures(scratch, frame, material_index);
        }
        let rendered = render_result?;
        if rendered {
            composites.mark_clean(material_index, tree);
        }
        Ok(metrics)
    }

    pub(crate) fn prime_active_run_working_set(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        active_hint: ActiveCompositeHint,
        tree: &CompositeTree,
    ) -> Result<RenderMetrics> {
        let result = self.prime_active_run_working_set_inner(
            frame,
            gpu_state,
            composite_pipelines,
            document,
            scratch,
            composites,
            material_index,
            active_hint,
            tree,
        );
        if result.is_err() {
            composites.clear_active_preview_state(material_index);
        }
        result
    }

    fn prime_active_run_working_set_inner(
        &self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        composite_pipelines: &CompositePipelines,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        composites: &mut CompositeCache,
        material_index: usize,
        active_hint: ActiveCompositeHint,
        tree: &CompositeTree,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        if material_index >= document.materials.material_count() {
            return Ok(metrics);
        }
        let Some(size) = document.materials.texture_size(material_index) else {
            return Ok(metrics);
        };
        composites.ensure_material(gpu_state.device(), material_index, size);
        composites.set_active_hint_for_surfaces(active_hint.surfaces().iter().copied());
        // Stroke begin is the explicit priming boundary for active preview.
        // Uploads here prepare checkpoints and source caches so later stroke
        // preview composites can use StrokePreviewNoUpload source resolution.
        if tree_contains_gated_pass_through(tree) || tree_contains_adjustment(tree) {
            ensure_full_tree_source_caches(
                &*document,
                composites,
                &mut metrics,
                gpu_state,
                frame,
                material_index,
                size,
                &active_hint,
                tree,
                true,
            )?;
            return Ok(metrics);
        }
        if !active_hint.is_single() {
            let plan = match build_active_set_run_plan(tree, active_hint.surfaces()) {
                Ok(plan) => plan,
                Err(reason) => {
                    return fail_active_prime(composites, material_index, reason, "plan miss");
                }
            };
            let checkpoint_count = plan.checkpoint_count();
            if checkpoint_count > composites.active_run_above_step_checkpoint_budget() {
                return fail_active_prime_checkpoint_budget(
                    composites,
                    material_index,
                    checkpoint_count,
                );
            }
            let boundary_checkpoint_shape = plan.boundary_checkpoint_shape();
            let rebuild_checkpoints = composites.ensure_active_run_checkpoints(
                gpu_state.device(),
                material_index,
                size,
                active_hint.surfaces(),
                tree,
                &boundary_checkpoint_shape,
            );
            if rebuild_checkpoints {
                {
                    let source_provider = CompositeSourceProvider::for_active_prime(
                        &*document,
                        Some(active_hint.clone()),
                        None,
                        CompositeSourceResolveMode::CheckpointRebuild,
                    );
                    let Some(checkpoints) = composites.active_run_checkpoint_views(material_index)
                    else {
                        return fail_active_prime(
                            composites,
                            material_index,
                            ActiveCompositePlanMiss::NoActiveSurface,
                            "checkpoint views missing",
                        );
                    };
                    let rebuild_result = rebuild_active_set_run_checkpoints(
                        &source_provider,
                        gpu_state,
                        composite_pipelines,
                        frame,
                        scratch,
                        material_index,
                        size,
                        &plan,
                        checkpoints,
                    );
                    metrics.merge(source_provider.take_metrics());
                    rebuild_result?;
                }
                composites.mark_active_run_checkpoints_valid(material_index);
            }
            ensure_active_set_partner_source_caches(
                &*document,
                composites,
                &mut metrics,
                gpu_state,
                frame,
                material_index,
                size,
                &active_hint,
                &plan,
                true,
            )?;
            return Ok(metrics);
        }
        let active_surface = active_hint.primary_surface();
        let Some(plan) = build_flat_normal_active_run_plan(tree, active_surface) else {
            return fail_active_prime(
                composites,
                material_index,
                ActiveCompositePlanMiss::UnsupportedRootProps,
                "single-active plan miss",
            );
        };
        let checkpoint_count = plan.checkpoint_count();
        if checkpoint_count > composites.active_run_above_step_checkpoint_budget() {
            return fail_active_prime_checkpoint_budget(
                composites,
                material_index,
                checkpoint_count,
            );
        }
        let boundary_checkpoint_shape = plan.boundary_checkpoint_shape();
        let rebuild_checkpoints = composites.ensure_active_run_checkpoints(
            gpu_state.device(),
            material_index,
            size,
            active_hint.surfaces(),
            tree,
            &boundary_checkpoint_shape,
        );
        if rebuild_checkpoints {
            {
                let source_provider = CompositeSourceProvider::for_active_prime(
                    &*document,
                    Some(active_hint.clone()),
                    None,
                    CompositeSourceResolveMode::CheckpointRebuild,
                );
                let Some(checkpoints) = composites.active_run_checkpoint_views(material_index)
                else {
                    return fail_active_prime(
                        composites,
                        material_index,
                        ActiveCompositePlanMiss::NoActiveSurface,
                        "checkpoint views missing",
                    );
                };
                let rebuild_result = rebuild_active_run_checkpoints(
                    &source_provider,
                    gpu_state,
                    composite_pipelines,
                    frame,
                    scratch,
                    material_index,
                    size,
                    &plan,
                    &checkpoints,
                );
                metrics.merge(source_provider.take_metrics());
                rebuild_result?;
            }
            composites.mark_active_run_checkpoints_valid(material_index);
        }
        ensure_active_run_partner_source_cache(
            &*document,
            composites,
            &mut metrics,
            gpu_state,
            frame,
            material_index,
            size,
            &active_hint,
            &plan,
            true,
        )?;
        Ok(metrics)
    }
}

fn fail_active_prime(
    composites: &mut CompositeCache,
    material_index: usize,
    reason: ActiveCompositePlanMiss,
    context: &'static str,
) -> Result<RenderMetrics> {
    composites.clear_active_preview_state(material_index);
    composites.record_active_prime_plan_miss();
    bail!("active composite priming failed ({context}): {reason:?}")
}

fn fail_active_prime_checkpoint_budget(
    composites: &mut CompositeCache,
    material_index: usize,
    checkpoint_count: usize,
) -> Result<RenderMetrics> {
    composites.record_active_prime_checkpoint_budget_fallback(material_index, checkpoint_count);
    composites.clear_active_preview_state(material_index);
    bail!(
        "active composite priming failed: checkpoint budget exceeded ({checkpoint_count} checkpoints requested)"
    )
}

fn ensure_active_set_partner_source_caches(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    plan: &ActiveSetRunPlan<'_>,
    record_prime_uploads: bool,
) -> Result<()> {
    let mut partners = Vec::new();
    collect_active_set_partner_sources(plan, active_hint.surfaces(), &mut partners);
    composites.retain_active_run_source_cache_sources(
        material_index,
        active_hint.surfaces(),
        &partners,
    );
    if partners.is_empty() {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    }
    for source in partners {
        ensure_active_run_source_cache(
            document,
            composites,
            metrics,
            gpu,
            frame,
            material_index,
            output_size,
            active_hint,
            source,
            record_prime_uploads,
        )?;
    }
    Ok(())
}

fn ensure_full_tree_source_caches(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    tree: &CompositeTree,
    record_prime_uploads: bool,
) -> Result<()> {
    let sources = full_tree_inactive_sources(tree, active_hint.surfaces());
    composites.retain_active_run_source_cache_sources(
        material_index,
        active_hint.surfaces(),
        &sources,
    );
    if sources.is_empty() {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    }
    for source in sources {
        ensure_active_run_source_cache(
            document,
            composites,
            metrics,
            gpu,
            frame,
            material_index,
            output_size,
            active_hint,
            source,
            record_prime_uploads,
        )?;
    }
    Ok(())
}

fn validate_full_tree_source_caches_ready(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    tree: &CompositeTree,
) -> Result<()> {
    for source in full_tree_inactive_sources(tree, active_hint.surfaces()) {
        validate_active_run_source_cache_ready(
            document,
            composites,
            metrics,
            material_index,
            output_size,
            active_hint,
            source,
        )?;
    }
    Ok(())
}

fn full_tree_inactive_sources(
    tree: &CompositeTree,
    active_surfaces: &[PaintSurfaceId],
) -> Vec<PaintSurfaceId> {
    visible_surfaces_from_composite_tree(tree)
        .into_iter()
        .filter(|surface| !active_surfaces.contains(surface))
        .collect()
}

fn validate_active_run_checkpoints_ready(
    composites: &mut CompositeCache,
    material_index: usize,
    size: [u32; 2],
    active_surfaces: &[PaintSurfaceId],
    tree: &CompositeTree,
    boundary_above_step_counts: &[usize],
    metrics: &mut RenderMetrics,
) -> Result<()> {
    if composites.active_run_checkpoints_ready(
        material_index,
        size,
        active_surfaces,
        tree,
        boundary_above_step_counts,
    ) {
        return Ok(());
    }
    composites.record_stroke_composite_plan_miss();
    metrics.stroke_composite_plan_misses = metrics.stroke_composite_plan_misses.saturating_add(1);
    bail!("active stroke preview checkpoint data is missing or stale")
}

fn validate_active_set_partner_source_caches_ready(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    plan: &ActiveSetRunPlan<'_>,
) -> Result<()> {
    let mut partners = Vec::new();
    collect_active_set_partner_sources(plan, active_hint.surfaces(), &mut partners);
    for source in partners {
        validate_active_run_source_cache_ready(
            document,
            composites,
            metrics,
            material_index,
            output_size,
            active_hint,
            source,
        )?;
    }
    Ok(())
}

fn validate_active_run_partner_source_cache_ready(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    plan: &ActiveRunPlan<'_>,
) -> Result<()> {
    let mut partners = Vec::new();
    collect_active_run_partner_sources(plan, active_hint.surfaces(), &mut partners);
    for source in partners {
        validate_active_run_source_cache_ready(
            document,
            composites,
            metrics,
            material_index,
            output_size,
            active_hint,
            source,
        )?;
    }
    Ok(())
}

fn validate_active_run_source_cache_ready(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    source: PaintSurfaceId,
) -> Result<()> {
    if source.material_index.as_usize() != material_index {
        composites.record_stroke_composite_plan_miss();
        metrics.stroke_composite_plan_misses =
            metrics.stroke_composite_plan_misses.saturating_add(1);
        bail!(
            "active stroke preview source cache material mismatch: {:?}",
            source
        );
    }
    let Some(size) = document.surfaces.surface_texture_size(source) else {
        composites.record_stroke_composite_plan_miss();
        metrics.stroke_composite_plan_misses =
            metrics.stroke_composite_plan_misses.saturating_add(1);
        bail!(
            "active stroke preview source cache target is missing: {:?}",
            source
        );
    };
    if size != output_size {
        composites.record_stroke_composite_plan_miss();
        metrics.stroke_composite_plan_misses =
            metrics.stroke_composite_plan_misses.saturating_add(1);
        bail!(
            "active stroke preview source cache size mismatch: {:?}",
            source
        );
    }
    let Some(format) = document.surfaces.surface_record_pixel_format(source) else {
        composites.record_stroke_composite_plan_miss();
        metrics.stroke_composite_plan_misses =
            metrics.stroke_composite_plan_misses.saturating_add(1);
        bail!(
            "active stroke preview source cache format is missing: {:?}",
            source
        );
    };
    if composites.active_run_source_cache_matches(
        material_index,
        active_hint.surfaces(),
        source,
        size,
        format,
    ) {
        return Ok(());
    }
    composites.record_stroke_composite_plan_miss();
    metrics.stroke_composite_plan_misses = metrics.stroke_composite_plan_misses.saturating_add(1);
    bail!(
        "active stroke preview source cache is missing or stale: {:?}",
        source
    )
}

fn collect_active_set_partner_sources(
    plan: &ActiveSetRunPlan<'_>,
    active_surfaces: &[PaintSurfaceId],
    partners: &mut Vec<PaintSurfaceId>,
) {
    for step in &plan.steps {
        match step {
            ActiveSetStep::StaticRun { .. } => {}
            ActiveSetStep::ActiveRaster(node) => {
                for source in active_node_partner_sources(node, active_surfaces) {
                    push_partner_source(partners, active_surfaces, source);
                }
            }
            ActiveSetStep::ActiveGroup { group, plan } => {
                if let Some(mask) = group.mask {
                    push_partner_source(partners, active_surfaces, mask);
                }
                collect_active_set_partner_sources(plan, active_surfaces, partners);
            }
        }
    }
}

fn collect_active_run_partner_sources(
    plan: &ActiveRunPlan<'_>,
    active_surfaces: &[PaintSurfaceId],
    partners: &mut Vec<PaintSurfaceId>,
) {
    for source in active_leaf_partner_sources(plan.active, active_surfaces) {
        push_partner_source(partners, active_surfaces, source);
    }
    for boundary in &plan.boundaries {
        if let Some(mask) = boundary.active_child.mask {
            push_partner_source(partners, active_surfaces, mask);
        }
    }
}

fn push_partner_source(
    partners: &mut Vec<PaintSurfaceId>,
    active_surfaces: &[PaintSurfaceId],
    source: PaintSurfaceId,
) {
    if !active_surfaces.contains(&source) && !partners.contains(&source) {
        partners.push(source);
    }
}

fn ensure_active_run_partner_source_cache(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    plan: &ActiveRunPlan<'_>,
    record_prime_uploads: bool,
) -> Result<()> {
    let mut partners = Vec::new();
    collect_active_run_partner_sources(plan, active_hint.surfaces(), &mut partners);
    composites.retain_active_run_source_cache_sources(
        material_index,
        active_hint.surfaces(),
        &partners,
    );
    if partners.is_empty() {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    }
    for source in partners {
        ensure_active_run_source_cache(
            document,
            composites,
            metrics,
            gpu,
            frame,
            material_index,
            output_size,
            active_hint,
            source,
            record_prime_uploads,
        )?;
    }
    Ok(())
}

fn ensure_active_run_source_cache(
    document: &GpuDocument,
    composites: &mut CompositeCache,
    metrics: &mut RenderMetrics,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    active_hint: &ActiveCompositeHint,
    source: PaintSurfaceId,
    record_prime_uploads: bool,
) -> Result<()> {
    if source.material_index.as_usize() != material_index {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    }
    let Some(size) = document.surfaces.surface_texture_size(source) else {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    };
    if size != output_size {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    }
    let Some(format) = document.surfaces.surface_record_pixel_format(source) else {
        bail!("active run source cache target is missing: {:?}", source);
    };
    if composites.active_run_source_cache_matches(
        material_index,
        active_hint.surfaces(),
        source,
        size,
        format,
    ) {
        return Ok(());
    }
    if let Some(source_texture) = document.surfaces.surface_texture(source) {
        let texture = composites.replace_active_run_source_cache(
            gpu.device(),
            material_index,
            active_hint.surfaces(),
            source,
            size,
            format,
        );
        frame.encoder().copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        // This metric counts source-cache population events. Keep it shared
        // with CPU uploads so existing reports keep the same meaning.
        record_active_run_source_cache_upload(metrics, size, format, record_prime_uploads);
        return Ok(());
    }
    let Some((snapshot, _stats)) =
        document
            .surfaces
            .read_fresh_surface_snapshot(source, [0, 0], size)?
    else {
        composites.clear_active_run_source_cache(material_index);
        return Ok(());
    };
    let texture = composites.replace_active_run_source_cache(
        gpu.device(),
        material_index,
        active_hint.surfaces(),
        source,
        size,
        format,
    );
    match format {
        SurfacePixelFormat::Rgba8 => {
            frame.write_texture_rgba8(gpu.device(), texture, [0, 0], size, &snapshot.rgba8);
        }
        SurfacePixelFormat::R8 => {
            let r8 = rgba8_alpha_to_r8(&snapshot.rgba8);
            frame.write_texture_r8(gpu.device(), texture, [0, 0], size, &r8);
        }
    }
    record_active_run_source_cache_upload(metrics, size, format, record_prime_uploads);
    Ok(())
}

fn record_active_run_source_cache_upload(
    metrics: &mut RenderMetrics,
    size: [u32; 2],
    format: SurfacePixelFormat,
    record_prime_uploads: bool,
) {
    let bytes = format.saturating_byte_len(size);
    metrics.active_composite_source_cache_uploads = metrics
        .active_composite_source_cache_uploads
        .saturating_add(1);
    if record_prime_uploads {
        metrics.active_composite_prime_upload_calls = metrics
            .active_composite_prime_upload_calls
            .saturating_add(1);
        metrics.active_composite_prime_upload_bytes = metrics
            .active_composite_prime_upload_bytes
            .saturating_add(bytes);
    }
}

fn active_leaf_partner_sources(
    active: ActiveRunLeaf<'_>,
    active_surfaces: &[PaintSurfaceId],
) -> Vec<PaintSurfaceId> {
    let ActiveRunLeaf::Raster(node) = active else {
        return Vec::new();
    };
    active_node_partner_sources(node, active_surfaces)
}

fn active_node_partner_sources(
    active_node: &CompositeNode,
    active_surfaces: &[PaintSurfaceId],
) -> Vec<PaintSurfaceId> {
    match active_node {
        CompositeNode::Raster { surface, mask, .. } => {
            let surface_is_active = active_surfaces.contains(surface);
            let mask_is_active = mask.is_some_and(|mask| active_surfaces.contains(&mask));
            let mut partners = Vec::new();
            if surface_is_active
                && let Some(mask) = mask
                && !mask_is_active
            {
                partners.push(*mask);
            }
            if mask_is_active && !surface_is_active {
                partners.push(*surface);
            }
            partners
        }
        CompositeNode::SolidFill { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::Adjustment { .. }
        | CompositeNode::Group(_) => Vec::new(),
    }
}

fn rebuild_active_run_checkpoints(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &mut TransientTextures,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
) -> Result<()> {
    if plan.boundaries.len() != checkpoints.boundaries.len() {
        bail!("active run checkpoint boundary shape does not match plan");
    }

    for (boundary, checkpoint) in plan.boundaries.iter().zip(checkpoints.boundaries.iter()) {
        rebuild_active_run_boundary_checkpoints(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            output_size,
            boundary,
            checkpoint,
        )?;
    }
    Ok(())
}

fn rebuild_active_set_run_checkpoints(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &mut TransientTextures,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveSetRunPlan<'_>,
    checkpoints: ActiveRunCheckpointViews<'_>,
) -> Result<()> {
    if plan.checkpoint_count() != checkpoints.boundaries.len() {
        bail!("active set checkpoint shape does not match plan");
    }
    rebuild_active_set_plan_checkpoints(
        sources,
        gpu,
        pipelines,
        frame,
        scratch,
        material_index,
        output_size,
        plan,
        &checkpoints,
    )
}

fn rebuild_active_set_plan_checkpoints(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &mut TransientTextures,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveSetRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
) -> Result<()> {
    for step in &plan.steps {
        match step {
            ActiveSetStep::StaticRun {
                nodes,
                checkpoint_index,
                uses_shader_blending,
            } => {
                let Some(checkpoint) = checkpoints.boundaries.get(*checkpoint_index) else {
                    bail!("active set checkpoint index is out of range");
                };
                if *uses_shader_blending {
                    composite_node_slice_to_view_with_shader_blending(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        scratch,
                        material_index,
                        nodes,
                        output_size,
                        checkpoint.below,
                    )?;
                } else {
                    composite_source_over_nodes_to_view(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        material_index,
                        nodes,
                        output_size,
                        checkpoint.below,
                        "clear_active_set_static_checkpoint",
                    )?;
                }
            }
            ActiveSetStep::ActiveRaster(_) => {}
            ActiveSetStep::ActiveGroup { plan, .. } => rebuild_active_set_plan_checkpoints(
                sources,
                gpu,
                pipelines,
                frame,
                scratch,
                material_index,
                output_size,
                plan,
                checkpoints,
            )?,
        }
    }
    Ok(())
}

fn rebuild_active_run_boundary_checkpoints(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &mut TransientTextures,
    material_index: usize,
    output_size: [u32; 2],
    boundary: &ActiveRunBoundaryPlan<'_>,
    checkpoint: &ActiveRunBoundaryCheckpointViews<'_>,
) -> Result<()> {
    if boundary.below_uses_shader_blending {
        composite_node_slice_to_view_with_shader_blending(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            boundary.below,
            output_size,
            checkpoint.below,
        )?;
    } else {
        composite_source_over_nodes_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            boundary.below,
            output_size,
            checkpoint.below,
            "clear_active_run_below_checkpoint",
        )?;
    }
    for (step, checkpoint_view) in boundary
        .above_steps
        .iter()
        .zip(checkpoint.above_steps.iter())
    {
        match step {
            ActiveAboveStep::SourceOverRun(nodes) => composite_source_over_nodes_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                material_index,
                nodes,
                output_size,
                checkpoint_view,
                "clear_active_run_above_source_over_checkpoint",
            )?,
            ActiveAboveStep::NonNormalSource(node) => composite_non_normal_source_cache_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                material_index,
                node,
                output_size,
                checkpoint_view,
            )?,
        }
    }
    Ok(())
}

enum ActiveBoundarySource<'a> {
    Noop,
    ActiveRaster(&'a CompositeNode),
    DynamicGroup(&'a wgpu::TextureView),
}

fn composite_active_set_run_plan_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveSetRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    if plan.checkpoint_count() != checkpoints.boundaries.len() {
        bail!("active set checkpoint shape does not match plan");
    }
    clear_composite_target(
        frame.encoder(),
        output_view,
        pipelines,
        clip,
        "clear_material_presentation_before_active_set_composite",
    );
    composite_active_set_plan_steps_to_view(
        sources,
        gpu,
        pipelines,
        frame,
        material_index,
        output_size,
        plan,
        &checkpoints,
        output_view,
        clip,
    )
}

fn composite_active_set_plan_steps_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveSetRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    for step in &plan.steps {
        match step {
            ActiveSetStep::StaticRun {
                checkpoint_index, ..
            } => {
                let Some(checkpoint) = checkpoints.boundaries.get(*checkpoint_index) else {
                    bail!("active set checkpoint index is out of range");
                };
                composite_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    checkpoint.below,
                    None,
                    identity_source_over_props(),
                    output_view,
                    clip,
                );
            }
            ActiveSetStep::ActiveRaster(node) => {
                composite_active_set_raster_to_view(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    output_size,
                    node,
                    output_view,
                    clip,
                )?;
            }
            ActiveSetStep::ActiveGroup { group, plan } => {
                composite_active_set_group_to_view(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    output_size,
                    group,
                    plan,
                    checkpoints,
                    output_view,
                    clip,
                )?;
            }
        }
    }
    Ok(())
}

fn composite_active_set_group_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    group: &CompositeGroup,
    plan: &ActiveSetRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return Ok(());
    }
    let (_scratch_texture, scratch_view) =
        create_render_scratch_texture(gpu.device(), output_size, "active_set_group_scratch");
    clear_rgba_target(
        frame.encoder(),
        &scratch_view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_active_set_group_scratch",
    );
    composite_active_set_plan_steps_to_view(
        sources,
        gpu,
        pipelines,
        frame,
        material_index,
        output_size,
        plan,
        checkpoints,
        &scratch_view,
        None,
    )?;
    let mask =
        composite_mask_texture(sources, gpu, frame, group.mask, material_index, output_size)?;
    composite_view_to_view(
        gpu,
        pipelines,
        frame,
        &scratch_view,
        mask.as_ref().map(CompositeSourceTexture::view),
        group.props,
        output_view,
        clip,
    );
    Ok(())
}

fn composite_active_set_raster_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    node: &CompositeNode,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    if node_props(node).blend_mode == LayerBlendMode::Normal {
        composite_flat_raster_node_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            node,
            output_size,
            output_view,
            clip,
        )
    } else {
        let (_blend_texture, blend_view) = create_render_scratch_texture(
            gpu.device(),
            output_size,
            "active_set_active_raster_blend_result",
        );
        composite_raster_node_over_backdrop_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            node,
            output_size,
            output_view,
            &blend_view,
            clip,
        )?;
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            &blend_view,
            output_view,
            "clear_active_set_active_raster_blend_copy",
            clip,
        );
        Ok(())
    }
}

fn composite_active_run_plan_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    plan: &ActiveRunPlan<'_>,
    checkpoints: &ActiveRunCheckpointViews<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    if plan.boundaries.len() != checkpoints.boundaries.len() {
        bail!("active run checkpoint boundary shape does not match plan");
    }

    let mut dynamic_textures = Vec::with_capacity(plan.boundaries.len());
    let mut dynamic_views = Vec::with_capacity(plan.boundaries.len());
    for (boundary_index, (boundary, checkpoint)) in plan
        .boundaries
        .iter()
        .zip(checkpoints.boundaries.iter())
        .enumerate()
    {
        let (dynamic_texture, dynamic_view) = create_render_scratch_texture(
            gpu.device(),
            output_size,
            &format!("active_run_boundary_{boundary_index}_dynamic"),
        );
        if boundary_index == 0 {
            composite_active_boundary_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                material_index,
                output_size,
                boundary,
                checkpoint,
                match plan.active {
                    ActiveRunLeaf::Raster(node) => ActiveBoundarySource::ActiveRaster(node),
                    ActiveRunLeaf::GroupMaskInner => ActiveBoundarySource::Noop,
                },
                &dynamic_view,
                clip,
            )?;
        } else {
            let previous_dynamic = dynamic_views
                .last()
                .expect("outer active boundary requires inner dynamic view");
            composite_active_boundary_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                material_index,
                output_size,
                boundary,
                checkpoint,
                ActiveBoundarySource::DynamicGroup(previous_dynamic),
                &dynamic_view,
                clip,
            )?;
        }
        dynamic_textures.push(dynamic_texture);
        dynamic_views.push(dynamic_view);
    }

    let Some(final_view) = dynamic_views.last() else {
        return Ok(());
    };
    clear_composite_target(
        frame.encoder(),
        output_view,
        pipelines,
        clip,
        "clear_material_presentation_before_active_run_composite",
    );
    composite_view_to_view(
        gpu,
        pipelines,
        frame,
        final_view,
        None,
        identity_source_over_props(),
        output_view,
        clip,
    );
    drop(dynamic_textures);
    Ok(())
}

fn composite_active_boundary_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    boundary: &ActiveRunBoundaryPlan<'_>,
    checkpoint: &ActiveRunBoundaryCheckpointViews<'_>,
    active_source: ActiveBoundarySource<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    clear_composite_target(
        frame.encoder(),
        output_view,
        pipelines,
        clip,
        "clear_active_run_boundary_dynamic",
    );
    composite_view_to_view(
        gpu,
        pipelines,
        frame,
        checkpoint.below,
        None,
        identity_source_over_props(),
        output_view,
        clip,
    );
    composite_active_boundary_source_to_view(
        sources,
        gpu,
        pipelines,
        frame,
        material_index,
        output_size,
        active_source,
        boundary.active_child,
        output_view,
        clip,
    )?;

    if boundary.has_non_normal_above {
        composite_active_boundary_above_with_shader_steps_to_view(
            gpu,
            pipelines,
            frame,
            output_size,
            boundary,
            checkpoint,
            output_view,
            clip,
        );
    } else {
        for checkpoint_view in &checkpoint.above_steps {
            composite_view_to_view(
                gpu,
                pipelines,
                frame,
                checkpoint_view,
                None,
                identity_source_over_props(),
                output_view,
                clip,
            );
        }
    }
    Ok(())
}

fn composite_active_boundary_source_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    output_size: [u32; 2],
    active_source: ActiveBoundarySource<'_>,
    active_child: ActiveBoundaryComposite,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    match active_source {
        ActiveBoundarySource::ActiveRaster(node) => {
            if node_props(node).blend_mode == LayerBlendMode::Normal {
                composite_flat_raster_node_to_view(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    node,
                    output_size,
                    output_view,
                    clip,
                )?;
            } else {
                let (_blend_texture, blend_view) = create_render_scratch_texture(
                    gpu.device(),
                    output_size,
                    "active_run_active_raster_blend_result",
                );
                composite_raster_node_over_backdrop_to_view(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    node,
                    output_size,
                    output_view,
                    &blend_view,
                    clip,
                )?;
                copy_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    &blend_view,
                    output_view,
                    "clear_active_run_active_raster_blend_copy",
                    clip,
                );
            }
        }
        ActiveBoundarySource::Noop => {}
        ActiveBoundarySource::DynamicGroup(dynamic_view) => {
            let mask = composite_mask_texture(
                sources,
                gpu,
                frame,
                active_child.mask,
                material_index,
                output_size,
            )?;
            if active_child.props.blend_mode == LayerBlendMode::Normal {
                composite_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    dynamic_view,
                    mask.as_ref().map(CompositeSourceTexture::view),
                    active_child.props,
                    output_view,
                    clip,
                );
            } else {
                let (_blend_texture, blend_view) = create_render_scratch_texture(
                    gpu.device(),
                    output_size,
                    "active_run_active_group_blend_result",
                );
                composite_view_over_backdrop_to_view(
                    gpu,
                    pipelines,
                    frame,
                    dynamic_view,
                    mask.as_ref().map(CompositeSourceTexture::view),
                    output_view,
                    active_child.props,
                    &blend_view,
                    clip,
                );
                copy_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    &blend_view,
                    output_view,
                    "clear_active_run_active_group_blend_copy",
                    clip,
                );
            }
        }
    }
    Ok(())
}

fn composite_active_boundary_above_with_shader_steps_to_view(
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    output_size: [u32; 2],
    boundary: &ActiveRunBoundaryPlan<'_>,
    checkpoint: &ActiveRunBoundaryCheckpointViews<'_>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) {
    let (_temp_texture, temp_view) = create_render_scratch_texture(
        gpu.device(),
        output_size,
        "active_run_boundary_shader_step_temp",
    );
    let mut current_is_output = true;
    for (step, checkpoint_view) in boundary
        .above_steps
        .iter()
        .zip(checkpoint.above_steps.iter())
    {
        match step {
            ActiveAboveStep::SourceOverRun(_) => {
                let current_view = if current_is_output {
                    output_view
                } else {
                    &temp_view
                };
                composite_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    checkpoint_view,
                    None,
                    identity_source_over_props(),
                    current_view,
                    clip,
                );
            }
            ActiveAboveStep::NonNormalSource(node) => {
                let (backdrop_view, target_view) = if current_is_output {
                    (output_view, &temp_view)
                } else {
                    (&temp_view, output_view)
                };
                composite_view_over_backdrop_to_view(
                    gpu,
                    pipelines,
                    frame,
                    checkpoint_view,
                    None,
                    backdrop_view,
                    cached_non_normal_blend_props(node_props(node)),
                    target_view,
                    clip,
                );
                current_is_output = !current_is_output;
            }
        }
    }

    if !current_is_output {
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            &temp_view,
            output_view,
            "clear_active_run_boundary_shader_copy",
            clip,
        );
    }
}

fn composite_source_over_nodes_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    nodes: &[CompositeNode],
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clear_label: &'static str,
) -> Result<()> {
    clear_rgba_target(
        frame.encoder(),
        output_view,
        [0.0, 0.0, 0.0, 0.0],
        clear_label,
    );
    for node in nodes {
        composite_source_over_node_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            node,
            output_size,
            output_view,
        )?;
    }
    Ok(())
}

fn composite_source_over_node_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    node: &CompositeNode,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    match node {
        CompositeNode::Raster { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::SolidFill { .. }
        | CompositeNode::Adjustment { .. } => composite_flat_raster_node_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            node,
            output_size,
            output_view,
            None,
        ),
        CompositeNode::Group(group) => composite_group_source_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            group,
            output_size,
            output_view,
        ),
    }
}

fn composite_flat_raster_node_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    node: &CompositeNode,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    with_composite_leaf_source(
        sources,
        gpu,
        frame,
        material_index,
        node,
        output_size,
        |frame, source, mask_view, props| {
            composite_draw_to_view(
                gpu.device(),
                gpu.paint_sampler(),
                pipelines,
                frame,
                CompositeDrawRequest {
                    source,
                    mask_view,
                    backdrop_view: None,
                    props,
                    output_view,
                    clip,
                },
            );
        },
    )
}

fn composite_raster_node_over_backdrop_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    node: &CompositeNode,
    output_size: [u32; 2],
    backdrop_view: &wgpu::TextureView,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) -> Result<()> {
    if !leaf_props_require_source(node.props()) {
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            backdrop_view,
            output_view,
            "clear_active_run_empty_raster_blend_copy",
            clip,
        );
        return Ok(());
    }
    with_composite_leaf_source(
        sources,
        gpu,
        frame,
        material_index,
        node,
        output_size,
        |frame, source, mask_view, props| {
            composite_draw_to_view(
                gpu.device(),
                gpu.paint_sampler(),
                pipelines,
                frame,
                CompositeDrawRequest {
                    source,
                    mask_view,
                    backdrop_view: Some(backdrop_view),
                    props,
                    output_view,
                    clip,
                },
            );
        },
    )
}

fn composite_group_source_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    group: &CompositeGroup,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return Ok(());
    }
    if group.props.blend_mode != LayerBlendMode::Normal {
        bail!("group source cache only supports Normal group blend mode");
    }

    let (_inner_texture, inner_view) =
        create_render_scratch_texture(gpu.device(), output_size, "active_run_group_source_inner");
    if group_children_use_shader_blend_modes(group) {
        composite_group_children_to_view_with_shader_blending_unpooled(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            group,
            output_size,
            &inner_view,
        )?;
    } else {
        clear_rgba_target(
            frame.encoder(),
            &inner_view,
            [0.0, 0.0, 0.0, 0.0],
            "clear_active_run_group_source_inner",
        );
        composite_group_children_to_view_unpooled(
            sources,
            gpu,
            pipelines,
            frame,
            material_index,
            group,
            output_size,
            &inner_view,
        )?;
    }
    let mask =
        composite_mask_texture(sources, gpu, frame, group.mask, material_index, output_size)?;
    composite_view_to_view(
        gpu,
        pipelines,
        frame,
        &inner_view,
        mask.as_ref().map(CompositeSourceTexture::view),
        group.props,
        output_view,
        None,
    );
    Ok(())
}

fn composite_non_normal_source_cache_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    node: &CompositeNode,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    clear_rgba_target(
        frame.encoder(),
        output_view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_non_normal_source_cache",
    );
    match node {
        CompositeNode::Raster { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::SolidFill { .. }
        | CompositeNode::Adjustment { .. } => {
            with_composite_leaf_source(
                sources,
                gpu,
                frame,
                material_index,
                node,
                output_size,
                |frame, source, mask_view, props| {
                    composite_draw_to_view(
                        gpu.device(),
                        gpu.paint_sampler(),
                        pipelines,
                        frame,
                        CompositeDrawRequest {
                            source,
                            mask_view,
                            backdrop_view: None,
                            props: cached_source_props(props),
                            output_view,
                            clip: None,
                        },
                    );
                },
            )?;
        }
        CompositeNode::Group(group) => {
            if !group.props.visible || group.props.opacity <= 0.0 {
                return Ok(());
            }
            let (_inner_texture, inner_view) = create_render_scratch_texture(
                gpu.device(),
                output_size,
                "non_normal_group_source_cache_inner",
            );
            if group_children_use_shader_blend_modes(group) {
                composite_group_children_to_view_with_shader_blending_unpooled(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    group,
                    output_size,
                    &inner_view,
                )?;
            } else {
                clear_rgba_target(
                    frame.encoder(),
                    &inner_view,
                    [0.0, 0.0, 0.0, 0.0],
                    "clear_non_normal_group_source_cache_inner",
                );
                composite_group_children_to_view_unpooled(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    group,
                    output_size,
                    &inner_view,
                )?;
            }
            let mask = composite_mask_texture(
                sources,
                gpu,
                frame,
                group.mask,
                material_index,
                output_size,
            )?;
            composite_view_to_view(
                gpu,
                pipelines,
                frame,
                &inner_view,
                mask.as_ref().map(CompositeSourceTexture::view),
                cached_source_props(group.props),
                output_view,
                None,
            );
        }
    }
    Ok(())
}

fn node_props(node: &CompositeNode) -> CompositeProps {
    node.props()
}

fn cached_source_props(props: CompositeProps) -> CompositeProps {
    CompositeProps {
        visible: props.visible,
        opacity: props.opacity,
        blend_mode: LayerBlendMode::Normal,
    }
}

fn cached_non_normal_blend_props(props: CompositeProps) -> CompositeProps {
    CompositeProps {
        visible: props.visible,
        opacity: 1.0,
        blend_mode: props.blend_mode,
    }
}

fn identity_source_over_props() -> CompositeProps {
    CompositeProps {
        visible: true,
        opacity: 1.0,
        blend_mode: LayerBlendMode::Normal,
    }
}

fn composite_group_slice(group: &CompositeGroup, start: usize, end: usize) -> CompositeGroup {
    CompositeGroup {
        layer_id: None,
        mode: GroupCompositeMode::Isolated,
        props: identity_source_over_props(),
        mask: None,
        children: group.children[start..end].to_vec(),
    }
}

const COMPOSITE_TRANSIENT_NAMESPACE: &str = "composite_transient";

fn composite_operation_scratch_key(
    material_index: usize,
    name: impl Into<String>,
) -> TransientTextureKey {
    TransientTextureKey::new(
        COMPOSITE_TRANSIENT_NAMESPACE,
        TransientTextureScope::Material(material_index),
        name,
        TransientLifetime::Operation,
    )
}

fn ensure_composite_operation_scratch_texture(
    scratch: &mut TransientTextures,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    name: impl Into<String>,
    output_size: [u32; 2],
) -> TransientTextureKey {
    let key = composite_operation_scratch_key(material_index, name);
    scratch.ensure_transient_texture_without_clear(
        gpu.device(),
        frame.encoder(),
        key.clone(),
        output_size,
        wgpu::TextureFormat::Rgba8Unorm,
        "composite_transient_texture",
    );
    key
}

fn general_group_source_key(material_index: usize, depth: usize) -> TransientTextureKey {
    composite_operation_scratch_key(
        material_index,
        format!("general_group_source_depth_{depth}"),
    )
}

fn general_shader_accum_key(
    material_index: usize,
    depth: usize,
    slot: char,
) -> TransientTextureKey {
    composite_operation_scratch_key(
        material_index,
        format!("general_shader_accum_{slot}_depth_{depth}"),
    )
}

fn composite_group_max_depth(group: &CompositeGroup, depth: usize) -> usize {
    group.children.iter().fold(depth, |max_depth, child| {
        let CompositeNode::Group(child_group) = child else {
            return max_depth;
        };
        max_depth.max(composite_group_max_depth(
            child_group,
            depth.saturating_add(1),
        ))
    })
}

fn ensure_general_composite_scratch_textures(
    scratch: &mut TransientTextures,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    tree: &CompositeTree,
    output_size: [u32; 2],
    uses_shader_blend_modes: bool,
) {
    let max_depth = composite_group_max_depth(&tree.root, 0);
    for depth in 1..=max_depth {
        ensure_composite_operation_scratch_texture(
            scratch,
            gpu,
            frame,
            material_index,
            format!("general_group_source_depth_{depth}"),
            output_size,
        );
    }
    if uses_shader_blend_modes {
        for depth in 0..=max_depth {
            for slot in ['a', 'b'] {
                ensure_composite_operation_scratch_texture(
                    scratch,
                    gpu,
                    frame,
                    material_index,
                    format!("general_shader_accum_{slot}_depth_{depth}"),
                    output_size,
                );
            }
        }
    }
}

fn general_group_source_view<'a>(
    scratch: &'a TransientTextures,
    material_index: usize,
    depth: usize,
) -> &'a wgpu::TextureView {
    scratch
        .transient_texture_view(&general_group_source_key(material_index, depth))
        .expect("general composite group source texture was preallocated")
}

fn general_shader_accum_view<'a>(
    scratch: &'a TransientTextures,
    material_index: usize,
    depth: usize,
    slot: char,
) -> &'a wgpu::TextureView {
    scratch
        .transient_texture_view(&general_shader_accum_key(material_index, depth, slot))
        .expect("general composite shader accumulation texture was preallocated")
}

fn release_composite_operation_scratch_textures(
    scratch: &mut TransientTextures,
    frame: &mut GpuFrame,
    material_index: usize,
) {
    scratch.release_transient_textures_after_submit(
        frame,
        TransientTextureScope::Material(material_index),
        COMPOSITE_TRANSIENT_NAMESPACE,
        TransientLifetime::Operation,
    );
}

fn copy_view_to_view(
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    source_view: &wgpu::TextureView,
    output_view: &wgpu::TextureView,
    clear_label: &'static str,
    clip: Option<RectU32>,
) {
    clear_composite_target(frame.encoder(), output_view, pipelines, clip, clear_label);
    composite_view_to_view(
        gpu,
        pipelines,
        frame,
        source_view,
        None,
        identity_source_over_props(),
        output_view,
        clip,
    );
}

fn composite_material_tree_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    tree: &CompositeTree,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if clip.is_some() {
        clear_composite_target(
            frame.encoder(),
            output_view,
            pipelines,
            clip,
            "clear_material_presentation_before_tree_composite",
        );
        return composite_group_children_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &tree.root,
            0,
            output_size,
            output_view,
            clip,
            general_cache,
            cache_metrics,
        );
    }
    let Some(mut checkpoint) =
        general_cache.take_prefix_checkpoint(gpu.device(), tree, output_size, cache_metrics)
    else {
        clear_rgba_target(
            frame.encoder(),
            output_view,
            [0.0, 0.0, 0.0, 0.0],
            "clear_material_presentation_before_tree_composite",
        );
        return composite_group_children_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &tree.root,
            0,
            output_size,
            output_view,
            None,
            general_cache,
            cache_metrics,
        );
    };

    let result = (|| -> Result<()> {
        if !checkpoint.valid {
            clear_rgba_target(
                frame.encoder(),
                &checkpoint.view,
                [0.0, 0.0, 0.0, 0.0],
                "clear_general_prefix_checkpoint",
            );
            let prefix = composite_group_slice(&tree.root, 0, checkpoint.prefix_len);
            composite_group_children_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                scratch,
                material_index,
                &prefix,
                0,
                output_size,
                &checkpoint.view,
                None,
                general_cache,
                cache_metrics,
            )?;
            checkpoint.valid = true;
        }
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            &checkpoint.view,
            output_view,
            "clear_material_presentation_from_prefix_checkpoint",
            None,
        );
        let suffix =
            composite_group_slice(&tree.root, checkpoint.prefix_len, tree.root.children.len());
        composite_group_children_to_view(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &suffix,
            0,
            output_size,
            output_view,
            None,
            general_cache,
            cache_metrics,
        )
    })();
    general_cache.restore_prefix_checkpoint(checkpoint);
    result
}

fn composite_material_tree_to_view_with_shader_blending(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    tree: &CompositeTree,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if clip.is_some() {
        return composite_group_children_to_view_with_shader_blending(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &tree.root,
            0,
            output_size,
            output_view,
            clip,
            general_cache,
            cache_metrics,
        );
    }
    let Some(mut checkpoint) =
        general_cache.take_prefix_checkpoint(gpu.device(), tree, output_size, cache_metrics)
    else {
        return composite_group_children_to_view_with_shader_blending(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &tree.root,
            0,
            output_size,
            output_view,
            None,
            general_cache,
            cache_metrics,
        );
    };

    let result = (|| -> Result<()> {
        if !checkpoint.valid {
            let prefix = composite_group_slice(&tree.root, 0, checkpoint.prefix_len);
            composite_group_children_to_view_with_shader_blending(
                sources,
                gpu,
                pipelines,
                frame,
                scratch,
                material_index,
                &prefix,
                0,
                output_size,
                &checkpoint.view,
                None,
                general_cache,
                cache_metrics,
            )?;
            checkpoint.valid = true;
        }
        let suffix =
            composite_group_slice(&tree.root, checkpoint.prefix_len, tree.root.children.len());
        composite_group_children_over_backdrop_to_view_with_shader_blending(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &suffix,
            0,
            output_size,
            Some(&checkpoint.view),
            output_view,
            None,
            general_cache,
            cache_metrics,
        )
    })();
    general_cache.restore_prefix_checkpoint(checkpoint);
    result
}

fn composite_material_tree_clips_to_view_with_shader_blending(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    tree: &CompositeTree,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clips: &[RectU32],
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if root_uv_mirror_adjustment(tree).is_some() {
        return composite_group_children_to_view_with_shader_blending_clips(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            &tree.root,
            0,
            output_size,
            output_view,
            clips,
            general_cache,
            cache_metrics,
        );
    }

    for clip in clips {
        composite_material_tree_to_view_with_shader_blending(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            tree,
            output_size,
            output_view,
            Some(*clip),
            general_cache,
            cache_metrics,
        )?;
    }
    Ok(())
}

fn composite_group_children_to_view_unpooled(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    group: &CompositeGroup,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return Ok(());
    }
    for child in &group.children {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {
                with_composite_leaf_source(
                    sources,
                    gpu,
                    frame,
                    material_index,
                    child,
                    output_size,
                    |frame, source, mask_view, props| {
                        composite_draw_to_view(
                            gpu.device(),
                            gpu.paint_sampler(),
                            pipelines,
                            frame,
                            CompositeDrawRequest {
                                source,
                                mask_view,
                                backdrop_view: None,
                                props,
                                output_view,
                                clip: None,
                            },
                        );
                    },
                )?;
            }
            CompositeNode::Group(child_group) => {
                if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                    continue;
                }
                let (scratch_texture, scratch_view) = create_render_scratch_texture(
                    gpu.device(),
                    output_size,
                    "group_composite_scratch",
                );
                clear_rgba_target(
                    frame.encoder(),
                    &scratch_view,
                    [0.0, 0.0, 0.0, 0.0],
                    "clear_group_composite_scratch",
                );
                composite_group_children_to_view_unpooled(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    child_group,
                    output_size,
                    &scratch_view,
                )?;
                let mask = composite_mask_texture(
                    sources,
                    gpu,
                    frame,
                    child_group.mask,
                    material_index,
                    output_size,
                )?;
                composite_view_to_view(
                    gpu,
                    pipelines,
                    frame,
                    &scratch_view,
                    mask.as_ref().map(CompositeSourceTexture::view),
                    child_group.props,
                    output_view,
                    None,
                );
                drop(scratch_texture);
            }
        }
    }
    Ok(())
}

fn composite_group_children_to_view_with_shader_blending_unpooled(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    material_index: usize,
    group: &CompositeGroup,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        clear_rgba_target(
            frame.encoder(),
            output_view,
            [0.0, 0.0, 0.0, 0.0],
            "clear_invisible_shader_blend_group",
        );
        return Ok(());
    }

    let (_accum_a_texture, accum_a_view) =
        create_render_scratch_texture(gpu.device(), output_size, "shader_blend_accum_a");
    let (_accum_b_texture, accum_b_view) =
        create_render_scratch_texture(gpu.device(), output_size, "shader_blend_accum_b");
    clear_rgba_target(
        frame.encoder(),
        &accum_a_view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_shader_blend_initial_accum",
    );

    let mut current_is_a = true;
    for child in &group.children {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {
                if !leaf_props_require_source(node_props(child)) {
                    continue;
                }
                let (backdrop_view, target_view) = if current_is_a {
                    (&accum_a_view, &accum_b_view)
                } else {
                    (&accum_b_view, &accum_a_view)
                };
                with_composite_leaf_source(
                    sources,
                    gpu,
                    frame,
                    material_index,
                    child,
                    output_size,
                    |frame, source, mask_view, props| {
                        composite_draw_to_view(
                            gpu.device(),
                            gpu.paint_sampler(),
                            pipelines,
                            frame,
                            CompositeDrawRequest {
                                source,
                                mask_view,
                                backdrop_view: Some(backdrop_view),
                                props,
                                output_view: target_view,
                                clip: None,
                            },
                        );
                    },
                )?;
                current_is_a = !current_is_a;
            }
            CompositeNode::Group(child_group) => {
                if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                    continue;
                }
                let (_scratch_texture, scratch_view) = create_render_scratch_texture(
                    gpu.device(),
                    output_size,
                    "shader_blend_group_scratch",
                );
                composite_group_children_to_view_with_shader_blending_unpooled(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    material_index,
                    child_group,
                    output_size,
                    &scratch_view,
                )?;
                let (backdrop_view, target_view) = if current_is_a {
                    (&accum_a_view, &accum_b_view)
                } else {
                    (&accum_b_view, &accum_a_view)
                };
                let mask = composite_mask_texture(
                    sources,
                    gpu,
                    frame,
                    child_group.mask,
                    material_index,
                    output_size,
                )?;
                composite_view_over_backdrop_to_view(
                    gpu,
                    pipelines,
                    frame,
                    &scratch_view,
                    mask.as_ref().map(CompositeSourceTexture::view),
                    backdrop_view,
                    child_group.props,
                    target_view,
                    None,
                );
                current_is_a = !current_is_a;
            }
        }
    }

    let (final_view, transparent_view) = if current_is_a {
        (&accum_a_view, &accum_b_view)
    } else {
        (&accum_b_view, &accum_a_view)
    };
    clear_rgba_target(
        frame.encoder(),
        transparent_view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_shader_blend_final_backdrop",
    );
    composite_view_over_backdrop_to_view(
        gpu,
        pipelines,
        frame,
        final_view,
        None,
        transparent_view,
        CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        },
        output_view,
        None,
    );
    Ok(())
}

fn prepare_isolated_group_cache(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    group: &CompositeGroup,
    depth: usize,
    output_size: [u32; 2],
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<Option<IsolatedGroupCacheEntry>> {
    let Some(mut entry) =
        general_cache.take_isolated_group_cache(gpu.device(), group, output_size, cache_metrics)
    else {
        return Ok(None);
    };
    let dirty = entry.dirty.clone();
    let update_result = if dirty.full {
        if group_children_use_shader_blend_modes(group) {
            composite_group_children_to_view_with_shader_blending(
                sources,
                gpu,
                pipelines,
                frame,
                scratch,
                material_index,
                group,
                depth,
                output_size,
                &entry.view,
                None,
                general_cache,
                cache_metrics,
            )
        } else {
            clear_rgba_target(
                frame.encoder(),
                &entry.view,
                [0.0, 0.0, 0.0, 0.0],
                "clear_general_isolated_group_cache",
            );
            composite_group_children_to_view(
                sources,
                gpu,
                pipelines,
                frame,
                scratch,
                material_index,
                group,
                depth,
                output_size,
                &entry.view,
                None,
                general_cache,
                cache_metrics,
            )
        }
    } else if matches!(
        group_damage_dependency(&group.children),
        GroupDamageDependency::SingleUvMirror(_)
    ) {
        // The destination clip samples the reflected source-side backdrop. Rebuild every
        // dependency clip in one accumulator walk so no clip observes stale scratch contents.
        composite_group_children_to_view_with_shader_blending_clips(
            sources,
            gpu,
            pipelines,
            frame,
            scratch,
            material_index,
            group,
            depth,
            output_size,
            &entry.view,
            &dirty.rects,
            general_cache,
            cache_metrics,
        )
    } else {
        let mut result = Ok(());
        for clip in dirty.rects {
            result = if group_children_use_shader_blend_modes(group) {
                composite_group_children_to_view_with_shader_blending(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    scratch,
                    material_index,
                    group,
                    depth,
                    output_size,
                    &entry.view,
                    Some(clip),
                    general_cache,
                    cache_metrics,
                )
            } else {
                clear_view_rect(
                    frame.encoder(),
                    &entry.view,
                    &pipelines.clear_rect_pipeline,
                    clip,
                );
                composite_group_children_to_view(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    scratch,
                    material_index,
                    group,
                    depth,
                    output_size,
                    &entry.view,
                    Some(clip),
                    general_cache,
                    cache_metrics,
                )
            };
            if result.is_err() {
                break;
            }
        }
        result
    };
    if let Err(error) = update_result {
        general_cache.restore_isolated_group_cache(entry);
        return Err(error);
    }
    entry.dirty = Default::default();
    Ok(Some(entry))
}

fn composite_group_children_to_view(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    group: &CompositeGroup,
    depth: usize,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return Ok(());
    }
    for child in &group.children {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {
                with_composite_leaf_source(
                    sources,
                    gpu,
                    frame,
                    material_index,
                    child,
                    output_size,
                    |frame, source, mask_view, props| {
                        composite_draw_to_view(
                            gpu.device(),
                            gpu.paint_sampler(),
                            pipelines,
                            frame,
                            CompositeDrawRequest {
                                source,
                                mask_view,
                                backdrop_view: None,
                                props,
                                output_view,
                                clip,
                            },
                        );
                    },
                )?;
            }
            CompositeNode::Group(child_group) => {
                if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                    continue;
                }
                let child_depth = depth.saturating_add(1);
                if let Some(entry) = prepare_isolated_group_cache(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    scratch,
                    material_index,
                    child_group,
                    child_depth,
                    output_size,
                    general_cache,
                    cache_metrics,
                )? {
                    let result = (|| -> Result<()> {
                        let mask = composite_mask_texture(
                            sources,
                            gpu,
                            frame,
                            child_group.mask,
                            material_index,
                            output_size,
                        )?;
                        composite_view_to_view(
                            gpu,
                            pipelines,
                            frame,
                            &entry.view,
                            mask.as_ref().map(CompositeSourceTexture::view),
                            child_group.props,
                            output_view,
                            clip,
                        );
                        Ok(())
                    })();
                    general_cache.restore_isolated_group_cache(entry);
                    result?;
                } else {
                    let scratch_view =
                        general_group_source_view(scratch, material_index, child_depth);
                    clear_composite_target(
                        frame.encoder(),
                        scratch_view,
                        pipelines,
                        clip,
                        "clear_group_composite_scratch",
                    );
                    composite_group_children_to_view(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        scratch,
                        material_index,
                        child_group,
                        child_depth,
                        output_size,
                        scratch_view,
                        clip,
                        general_cache,
                        cache_metrics,
                    )?;
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    composite_view_to_view(
                        gpu,
                        pipelines,
                        frame,
                        scratch_view,
                        mask.as_ref().map(CompositeSourceTexture::view),
                        child_group.props,
                        output_view,
                        clip,
                    );
                }
            }
        }
    }
    Ok(())
}

fn composite_node_slice_to_view_with_shader_blending(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &mut TransientTextures,
    material_index: usize,
    nodes: &[CompositeNode],
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
) -> Result<()> {
    let accum_a_key = ensure_composite_operation_scratch_texture(
        scratch,
        gpu,
        frame,
        material_index,
        "active_run_below_shader_accum_a",
        output_size,
    );
    let accum_b_key = ensure_composite_operation_scratch_texture(
        scratch,
        gpu,
        frame,
        material_index,
        "active_run_below_shader_accum_b",
        output_size,
    );
    let result = (|| -> Result<()> {
        let accum_a_view = scratch
            .transient_texture_view(&accum_a_key)
            .expect("active run shader accumulation A texture was just ensured");
        let accum_b_view = scratch
            .transient_texture_view(&accum_b_key)
            .expect("active run shader accumulation B texture was just ensured");
        clear_rgba_target(
            frame.encoder(),
            accum_a_view,
            [0.0, 0.0, 0.0, 0.0],
            "clear_active_run_below_shader_initial_accum",
        );

        let mut current_is_a = true;
        for child in nodes {
            match child {
                CompositeNode::Raster { .. }
                | CompositeNode::EmbeddedImage { .. }
                | CompositeNode::SolidFill { .. }
                | CompositeNode::Adjustment { .. } => {
                    if !leaf_props_require_source(node_props(child)) {
                        continue;
                    }
                    let (backdrop_view, target_view) = if current_is_a {
                        (accum_a_view, accum_b_view)
                    } else {
                        (accum_b_view, accum_a_view)
                    };
                    with_composite_leaf_source(
                        sources,
                        gpu,
                        frame,
                        material_index,
                        child,
                        output_size,
                        |frame, source, mask_view, props| {
                            composite_draw_to_view(
                                gpu.device(),
                                gpu.paint_sampler(),
                                pipelines,
                                frame,
                                CompositeDrawRequest {
                                    source: source.clone(),
                                    mask_view,
                                    backdrop_view: Some(backdrop_view),
                                    props,
                                    output_view: target_view,
                                    clip: None,
                                },
                            );
                        },
                    )?;
                    current_is_a = !current_is_a;
                }
                CompositeNode::Group(child_group) => {
                    if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                        continue;
                    }
                    let (_scratch_texture, scratch_view) = create_render_scratch_texture(
                        gpu.device(),
                        output_size,
                        "active_run_below_shader_group_scratch",
                    );
                    composite_group_children_to_view_with_shader_blending_unpooled(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        material_index,
                        child_group,
                        output_size,
                        &scratch_view,
                    )?;
                    let (backdrop_view, target_view) = if current_is_a {
                        (accum_a_view, accum_b_view)
                    } else {
                        (accum_b_view, accum_a_view)
                    };
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    composite_view_over_backdrop_to_view(
                        gpu,
                        pipelines,
                        frame,
                        &scratch_view,
                        mask.as_ref().map(CompositeSourceTexture::view),
                        backdrop_view,
                        child_group.props,
                        target_view,
                        None,
                    );
                    current_is_a = !current_is_a;
                }
            }
        }

        let final_view = if current_is_a {
            accum_a_view
        } else {
            accum_b_view
        };
        clear_rgba_target(
            frame.encoder(),
            output_view,
            [0.0, 0.0, 0.0, 0.0],
            "clear_active_run_below_shader_checkpoint",
        );
        composite_view_to_view(
            gpu,
            pipelines,
            frame,
            final_view,
            None,
            identity_source_over_props(),
            output_view,
            None,
        );
        Ok(())
    })();
    release_composite_operation_scratch_textures(scratch, frame, material_index);
    result
}

fn composite_group_children_to_view_with_shader_blending(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    group: &CompositeGroup,
    depth: usize,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    composite_group_children_over_backdrop_to_view_with_shader_blending(
        sources,
        gpu,
        pipelines,
        frame,
        scratch,
        material_index,
        group,
        depth,
        output_size,
        None,
        output_view,
        clip,
        general_cache,
        cache_metrics,
    )
}

fn composite_group_children_over_backdrop_to_view_with_shader_blending(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    group: &CompositeGroup,
    depth: usize,
    output_size: [u32; 2],
    initial_backdrop_view: Option<&wgpu::TextureView>,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        if let Some(backdrop_view) = initial_backdrop_view {
            copy_view_to_view(
                gpu,
                pipelines,
                frame,
                backdrop_view,
                output_view,
                "clear_invisible_pass_through_group_copy",
                clip,
            );
        } else {
            clear_composite_target(
                frame.encoder(),
                output_view,
                pipelines,
                clip,
                "clear_invisible_shader_blend_group",
            );
        }
        return Ok(());
    }

    let accum_a_view = general_shader_accum_view(scratch, material_index, depth, 'a');
    let accum_b_view = general_shader_accum_view(scratch, material_index, depth, 'b');
    if let Some(backdrop_view) = initial_backdrop_view {
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            backdrop_view,
            accum_a_view,
            "clear_shader_blend_initial_backdrop_copy",
            clip,
        );
    } else {
        clear_composite_target(
            frame.encoder(),
            accum_a_view,
            pipelines,
            clip,
            "clear_shader_blend_initial_accum",
        );
    }

    let mut current_is_a = true;
    for child in &group.children {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {
                if !leaf_props_require_source(node_props(child)) {
                    continue;
                }
                let (backdrop_view, target_view) = if current_is_a {
                    (accum_a_view, accum_b_view)
                } else {
                    (accum_b_view, accum_a_view)
                };
                with_composite_leaf_source(
                    sources,
                    gpu,
                    frame,
                    material_index,
                    child,
                    output_size,
                    |frame, source, mask_view, props| {
                        composite_draw_to_view(
                            gpu.device(),
                            gpu.paint_sampler(),
                            pipelines,
                            frame,
                            CompositeDrawRequest {
                                source,
                                mask_view,
                                backdrop_view: Some(backdrop_view),
                                props,
                                output_view: target_view,
                                clip,
                            },
                        );
                    },
                )?;
                current_is_a = !current_is_a;
            }
            CompositeNode::Group(child_group) => {
                if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                    continue;
                }
                let child_depth = depth.saturating_add(1);
                let (backdrop_view, target_view) = if current_is_a {
                    (accum_a_view, accum_b_view)
                } else {
                    (accum_b_view, accum_a_view)
                };
                if child_group.mode == GroupCompositeMode::PassThrough {
                    let scratch_view =
                        general_group_source_view(scratch, material_index, child_depth);
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    composite_group_children_over_backdrop_to_view_with_shader_blending(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        scratch,
                        material_index,
                        child_group,
                        child_depth,
                        output_size,
                        Some(backdrop_view),
                        scratch_view,
                        clip,
                        general_cache,
                        cache_metrics,
                    )?;
                    pass_through_gate_to_view(
                        gpu,
                        pipelines,
                        frame,
                        backdrop_view,
                        scratch_view,
                        mask.as_ref().map(CompositeSourceTexture::view),
                        child_group.props.opacity,
                        target_view,
                        clip,
                    );
                } else if let Some(entry) = prepare_isolated_group_cache(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    scratch,
                    material_index,
                    child_group,
                    child_depth,
                    output_size,
                    general_cache,
                    cache_metrics,
                )? {
                    let result = (|| -> Result<()> {
                        let mask = composite_mask_texture(
                            sources,
                            gpu,
                            frame,
                            child_group.mask,
                            material_index,
                            output_size,
                        )?;
                        composite_view_over_backdrop_to_view(
                            gpu,
                            pipelines,
                            frame,
                            &entry.view,
                            mask.as_ref().map(CompositeSourceTexture::view),
                            backdrop_view,
                            child_group.props,
                            target_view,
                            clip,
                        );
                        Ok(())
                    })();
                    general_cache.restore_isolated_group_cache(entry);
                    result?;
                } else {
                    let scratch_view =
                        general_group_source_view(scratch, material_index, child_depth);
                    composite_group_children_over_backdrop_to_view_with_shader_blending(
                        sources,
                        gpu,
                        pipelines,
                        frame,
                        scratch,
                        material_index,
                        child_group,
                        child_depth,
                        output_size,
                        None,
                        scratch_view,
                        clip,
                        general_cache,
                        cache_metrics,
                    )?;
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    composite_view_over_backdrop_to_view(
                        gpu,
                        pipelines,
                        frame,
                        scratch_view,
                        mask.as_ref().map(CompositeSourceTexture::view),
                        backdrop_view,
                        child_group.props,
                        target_view,
                        clip,
                    );
                }
                current_is_a = !current_is_a;
            }
        }
    }

    let final_view = if current_is_a {
        accum_a_view
    } else {
        accum_b_view
    };
    copy_view_to_view(
        gpu,
        pipelines,
        frame,
        final_view,
        output_view,
        "clear_shader_blend_final_copy",
        clip,
    );
    Ok(())
}

fn composite_group_children_to_view_with_shader_blending_clips(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    scratch: &TransientTextures,
    material_index: usize,
    group: &CompositeGroup,
    depth: usize,
    output_size: [u32; 2],
    output_view: &wgpu::TextureView,
    clips: &[RectU32],
    general_cache: &mut GeneralCompositeMaterialCache,
    cache_metrics: &mut RenderMetrics,
) -> Result<()> {
    if clips.is_empty() {
        return Ok(());
    }
    if !group.props.visible || group.props.opacity <= 0.0 {
        for clip in clips {
            clear_view_rect(
                frame.encoder(),
                output_view,
                &pipelines.clear_rect_pipeline,
                *clip,
            );
        }
        return Ok(());
    }

    let accum_a_view = general_shader_accum_view(scratch, material_index, depth, 'a');
    let accum_b_view = general_shader_accum_view(scratch, material_index, depth, 'b');
    for clip in clips {
        clear_view_rect(
            frame.encoder(),
            accum_a_view,
            &pipelines.clear_rect_pipeline,
            *clip,
        );
    }

    let mut current_is_a = true;
    for child in &group.children {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {
                if !leaf_props_require_source(node_props(child)) {
                    continue;
                }
                let (backdrop_view, target_view) = if current_is_a {
                    (accum_a_view, accum_b_view)
                } else {
                    (accum_b_view, accum_a_view)
                };
                with_composite_leaf_source(
                    sources,
                    gpu,
                    frame,
                    material_index,
                    child,
                    output_size,
                    |frame, source, mask_view, props| {
                        for clip in clips {
                            composite_draw_to_view(
                                gpu.device(),
                                gpu.paint_sampler(),
                                pipelines,
                                frame,
                                CompositeDrawRequest {
                                    source: source.clone(),
                                    mask_view,
                                    backdrop_view: Some(backdrop_view),
                                    props,
                                    output_view: target_view,
                                    clip: Some(*clip),
                                },
                            );
                        }
                    },
                )?;
                current_is_a = !current_is_a;
            }
            CompositeNode::Group(child_group) => {
                if !child_group.props.visible || child_group.props.opacity <= 0.0 {
                    continue;
                }
                let child_depth = depth.saturating_add(1);
                let (backdrop_view, target_view) = if current_is_a {
                    (accum_a_view, accum_b_view)
                } else {
                    (accum_b_view, accum_a_view)
                };
                if child_group.mode == GroupCompositeMode::PassThrough {
                    let scratch_view =
                        general_group_source_view(scratch, material_index, child_depth);
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    for clip in clips {
                        composite_group_children_over_backdrop_to_view_with_shader_blending(
                            sources,
                            gpu,
                            pipelines,
                            frame,
                            scratch,
                            material_index,
                            child_group,
                            child_depth,
                            output_size,
                            Some(backdrop_view),
                            scratch_view,
                            Some(*clip),
                            general_cache,
                            cache_metrics,
                        )?;
                        pass_through_gate_to_view(
                            gpu,
                            pipelines,
                            frame,
                            backdrop_view,
                            scratch_view,
                            mask.as_ref().map(CompositeSourceTexture::view),
                            child_group.props.opacity,
                            target_view,
                            Some(*clip),
                        );
                    }
                } else if let Some(entry) = prepare_isolated_group_cache(
                    sources,
                    gpu,
                    pipelines,
                    frame,
                    scratch,
                    material_index,
                    child_group,
                    child_depth,
                    output_size,
                    general_cache,
                    cache_metrics,
                )? {
                    let result = (|| -> Result<()> {
                        let mask = composite_mask_texture(
                            sources,
                            gpu,
                            frame,
                            child_group.mask,
                            material_index,
                            output_size,
                        )?;
                        for clip in clips {
                            composite_view_over_backdrop_to_view(
                                gpu,
                                pipelines,
                                frame,
                                &entry.view,
                                mask.as_ref().map(CompositeSourceTexture::view),
                                backdrop_view,
                                child_group.props,
                                target_view,
                                Some(*clip),
                            );
                        }
                        Ok(())
                    })();
                    general_cache.restore_isolated_group_cache(entry);
                    result?;
                } else {
                    let scratch_view =
                        general_group_source_view(scratch, material_index, child_depth);
                    let mask = composite_mask_texture(
                        sources,
                        gpu,
                        frame,
                        child_group.mask,
                        material_index,
                        output_size,
                    )?;
                    for clip in clips {
                        composite_group_children_over_backdrop_to_view_with_shader_blending(
                            sources,
                            gpu,
                            pipelines,
                            frame,
                            scratch,
                            material_index,
                            child_group,
                            child_depth,
                            output_size,
                            None,
                            scratch_view,
                            Some(*clip),
                            general_cache,
                            cache_metrics,
                        )?;
                        composite_view_over_backdrop_to_view(
                            gpu,
                            pipelines,
                            frame,
                            scratch_view,
                            mask.as_ref().map(CompositeSourceTexture::view),
                            backdrop_view,
                            child_group.props,
                            target_view,
                            Some(*clip),
                        );
                    }
                }
                current_is_a = !current_is_a;
            }
        }
    }

    let final_view = if current_is_a {
        accum_a_view
    } else {
        accum_b_view
    };
    for clip in clips {
        copy_view_to_view(
            gpu,
            pipelines,
            frame,
            final_view,
            output_view,
            "clear_clipped_shader_blend_final_copy",
            Some(*clip),
        );
    }
    Ok(())
}

fn leaf_props_require_source(props: CompositeProps) -> bool {
    props.visible && props.opacity > 0.0
}

fn with_composite_leaf_source(
    sources: &CompositeSourceProvider<'_>,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    material_index: usize,
    node: &CompositeNode,
    output_size: [u32; 2],
    draw: impl FnOnce(
        &mut GpuFrame,
        CompositeDrawSource<'_>,
        Option<&wgpu::TextureView>,
        CompositeProps,
    ),
) -> Result<()> {
    match node {
        CompositeNode::Raster {
            surface,
            mask,
            props,
        } => {
            if !leaf_props_require_source(*props) {
                return Ok(());
            }
            let Some(source) = composite_surface_texture(
                sources,
                gpu,
                frame,
                *surface,
                material_index,
                output_size,
            )?
            else {
                bail!("composite source is missing: {:?}", surface);
            };
            let mask =
                composite_mask_texture(sources, gpu, frame, *mask, material_index, output_size)?;
            draw(
                frame,
                CompositeDrawSource::Texture(source.view()),
                mask.as_ref().map(CompositeSourceTexture::view),
                *props,
            );
        }
        CompositeNode::EmbeddedImage {
            layer_id,
            image_id,
            transform,
            mask,
            props,
            ..
        } => {
            if !leaf_props_require_source(*props) {
                return Ok(());
            }
            let Some(source) = sources.resolve_embedded(
                gpu,
                frame,
                *layer_id,
                *image_id,
                *transform,
                output_size,
            )?
            else {
                bail!("embedded image composite source is missing: {:?}", image_id);
            };
            let mask =
                composite_mask_texture(sources, gpu, frame, *mask, material_index, output_size)?;
            draw(
                frame,
                source.transformed().map_or(
                    CompositeDrawSource::Texture(source.view()),
                    |transform| CompositeDrawSource::TransformedTexture {
                        view: source.view(),
                        transform,
                    },
                ),
                mask.as_ref().map(CompositeSourceTexture::view),
                *props,
            );
        }
        CompositeNode::SolidFill {
            color, mask, props, ..
        } => {
            if !leaf_props_require_source(*props) {
                return Ok(());
            }
            let mask =
                composite_mask_texture(sources, gpu, frame, *mask, material_index, output_size)?;
            draw(
                frame,
                CompositeDrawSource::SolidColor(*color),
                mask.as_ref().map(CompositeSourceTexture::view),
                *props,
            );
        }
        CompositeNode::Adjustment {
            adjustment,
            mask,
            props,
            ..
        } => {
            if !leaf_props_require_source(*props) {
                return Ok(());
            }
            let mask =
                composite_mask_texture(sources, gpu, frame, *mask, material_index, output_size)?;
            draw(
                frame,
                CompositeDrawSource::Adjustment(adjustment.clone()),
                mask.as_ref().map(CompositeSourceTexture::view),
                *props,
            );
        }
        CompositeNode::Group(_) => bail!("composite leaf source requested for a group"),
    }
    Ok(())
}

fn composite_surface_texture<'a>(
    sources: &'a CompositeSourceProvider<'a>,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    surface: PaintSurfaceId,
    material_index: usize,
    output_size: [u32; 2],
) -> Result<Option<CompositeSourceTexture<'a>>> {
    sources.resolve(gpu, frame, surface, material_index, output_size)
}

fn composite_mask_texture<'a>(
    sources: &'a CompositeSourceProvider<'a>,
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    mask: Option<PaintSurfaceId>,
    material_index: usize,
    output_size: [u32; 2],
) -> Result<Option<CompositeSourceTexture<'a>>> {
    let Some(mask) = mask else {
        return Ok(None);
    };
    let Some(texture) =
        composite_surface_texture(sources, gpu, frame, mask, material_index, output_size)?
    else {
        bail!("composite mask source is missing: {:?}", mask);
    };
    Ok(Some(texture))
}

#[derive(Clone)]
enum CompositeDrawSource<'a> {
    Texture(&'a wgpu::TextureView),
    TransformedTexture {
        view: &'a wgpu::TextureView,
        transform: crate::core::embedded_image::EmbeddedImageTransform,
    },
    SolidColor([f32; 3]),
    Adjustment(Adjustment),
}

struct CompositeDrawRequest<'a> {
    source: CompositeDrawSource<'a>,
    mask_view: Option<&'a wgpu::TextureView>,
    backdrop_view: Option<&'a wgpu::TextureView>,
    props: CompositeProps,
    output_view: &'a wgpu::TextureView,
    clip: Option<RectU32>,
}

fn composite_draw_to_view(
    device: &wgpu::Device,
    paint_sampler: &wgpu::Sampler,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    request: CompositeDrawRequest<'_>,
) {
    if !request.props.visible || request.props.opacity <= 0.0 {
        return;
    }

    let uses_backdrop = request.backdrop_view.is_some();
    let (
        source_view,
        source_kind,
        solid_color,
        adjustment_kind,
        adjustment_params,
        adjustment_lut,
        transform_rows,
    ) = match request.source {
        CompositeDrawSource::Texture(view) => {
            (view, 0, [0.0; 4], 0, [[0.0; 4]; 8], None, [[0.0; 4]; 2])
        }
        CompositeDrawSource::TransformedTexture { view, transform } => (
            view,
            3,
            [0.0; 4],
            0,
            [[0.0; 4]; 8],
            None,
            crate::renderer::document::embedded_images::output_uv_to_source_uv_rows(transform),
        ),
        CompositeDrawSource::SolidColor(color) => (
            &pipelines.solid_fill_dummy_view,
            1,
            [color[0], color[1], color[2], 1.0],
            0,
            [[0.0; 4]; 8],
            None,
            [[0.0; 4]; 2],
        ),
        CompositeDrawSource::Adjustment(adjustment) => {
            let gpu_params = adjustment_gpu_params(&adjustment);
            (
                &pipelines.solid_fill_dummy_view,
                2,
                [0.0; 4],
                gpu_params.kind,
                gpu_params.params,
                AdjustmentLutUniform::for_adjustment(&adjustment),
                [[0.0; 4]; 2],
            )
        }
    };
    if source_kind == 2 && !uses_backdrop {
        return;
    }
    let uniform = LayerCompositeUniform {
        opacity: request.props.opacity.clamp(0.0, 1.0),
        blend_mode: request.props.blend_mode as u32,
        mask_enabled: u32::from(request.mask_view.is_some()),
        source_kind,
        solid_color,
        adjustment_kind,
        _adjustment_pad: [0; 3],
        adjustment_params,
        output_uv_to_source_uv_row0: transform_rows[0],
        output_uv_to_source_uv_row1: transform_rows[1],
    };
    frame.write_buffer_pod(device, &pipelines.layer_composite_uniform, 0, &uniform);
    if let Some(adjustment_lut) = adjustment_lut {
        frame.write_buffer_pod(
            device,
            &pipelines.adjustment_lut_uniform,
            0,
            &adjustment_lut,
        );
    }
    pipelines.record_composite_draw_call();

    let bind_group = if let Some(backdrop_view) = request.backdrop_view {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tree_blend_bg"),
            layout: &pipelines.layer_blend_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(backdrop_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(paint_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pipelines.layer_composite_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        request.mask_view.unwrap_or(source_view),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: pipelines.adjustment_lut_uniform.as_entire_binding(),
                },
            ],
        })
    } else {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tree_composite_bg"),
            layout: &pipelines.layer_composite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(paint_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pipelines.layer_composite_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        request.mask_view.unwrap_or(source_view),
                    ),
                },
            ],
        })
    };

    let pass_label = match (uses_backdrop, request.clip.is_some()) {
        (true, true) => "tree_blend_clipped_pass",
        (true, false) => "tree_blend_pass",
        (false, true) => "tree_composite_clipped_pass",
        (false, false) => "tree_composite_pass",
    };
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(pass_label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: request.output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    if let Some(clip) = request.clip {
        pass.set_scissor_rect(clip.origin[0], clip.origin[1], clip.size[0], clip.size[1]);
    }
    if uses_backdrop {
        pass.set_pipeline(&pipelines.layer_blend_pipeline);
    } else {
        pass.set_pipeline(&pipelines.layer_composite_pipeline);
    }
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}

fn composite_view_over_backdrop_to_view(
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    source_view: &wgpu::TextureView,
    mask_view: Option<&wgpu::TextureView>,
    backdrop_view: &wgpu::TextureView,
    props: CompositeProps,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) {
    composite_draw_to_view(
        gpu.device(),
        gpu.paint_sampler(),
        pipelines,
        frame,
        CompositeDrawRequest {
            source: CompositeDrawSource::Texture(source_view),
            mask_view,
            backdrop_view: Some(backdrop_view),
            props,
            output_view,
            clip,
        },
    );
}

fn pass_through_gate_to_view(
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    before_view: &wgpu::TextureView,
    after_view: &wgpu::TextureView,
    mask_view: Option<&wgpu::TextureView>,
    opacity: f32,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) {
    let uniform = LayerCompositeUniform {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: LayerBlendMode::Normal as u32,
        mask_enabled: u32::from(mask_view.is_some()),
        source_kind: 0,
        solid_color: [0.0; 4],
        adjustment_kind: 0,
        _adjustment_pad: [0; 3],
        adjustment_params: [[0.0; 4]; 8],
        output_uv_to_source_uv_row0: [0.0; 4],
        output_uv_to_source_uv_row1: [0.0; 4],
    };
    frame.write_buffer_pod(
        gpu.device(),
        &pipelines.layer_composite_uniform,
        0,
        &uniform,
    );
    pipelines.record_composite_draw_call();
    let bind_group = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pass_through_gate_bg"),
        layout: &pipelines.layer_blend_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(after_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(before_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(gpu.paint_sampler()),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipelines.layer_composite_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(mask_view.unwrap_or(after_view)),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: pipelines.adjustment_lut_uniform.as_entire_binding(),
            },
        ],
    });
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if clip.is_some() {
                "pass_through_gate_clipped_pass"
            } else {
                "pass_through_gate_pass"
            }),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    if let Some(clip) = clip {
        pass.set_scissor_rect(clip.origin[0], clip.origin[1], clip.size[0], clip.size[1]);
    }
    pass.set_pipeline(&pipelines.pass_through_gate_pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}

fn tree_uses_shader_blend_modes(tree: &CompositeTree) -> bool {
    tree.root.props.blend_mode != LayerBlendMode::Normal
        || group_children_use_shader_blend_modes(&tree.root)
}

fn group_uses_shader_blend_modes(group: &CompositeGroup) -> bool {
    group.mode == GroupCompositeMode::PassThrough
        || group.props.blend_mode != LayerBlendMode::Normal
        || group_children_use_shader_blend_modes(group)
}

fn group_children_use_shader_blend_modes(group: &CompositeGroup) -> bool {
    group.children.iter().any(|child| match child {
        CompositeNode::Raster { props, .. }
        | CompositeNode::EmbeddedImage { props, .. }
        | CompositeNode::SolidFill { props, .. } => props.blend_mode != LayerBlendMode::Normal,
        CompositeNode::Adjustment { .. } => true,
        CompositeNode::Group(group) => group_uses_shader_blend_modes(group),
    })
}

fn tree_contains_adjustment(tree: &CompositeTree) -> bool {
    group_contains_adjustment(&tree.root)
}

fn group_contains_adjustment(group: &CompositeGroup) -> bool {
    group.children.iter().any(|node| match node {
        CompositeNode::Adjustment { .. } => true,
        CompositeNode::Group(group) => group_contains_adjustment(group),
        CompositeNode::Raster { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::SolidFill { .. } => false,
    })
}

fn tree_contains_gated_pass_through(tree: &CompositeTree) -> bool {
    tree.root
        .children
        .iter()
        .any(node_contains_pass_through_group)
}

fn node_contains_pass_through_group(node: &CompositeNode) -> bool {
    match node {
        CompositeNode::Raster { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::SolidFill { .. }
        | CompositeNode::Adjustment { .. } => false,
        CompositeNode::Group(group) => {
            group.mode == GroupCompositeMode::PassThrough
                || group.children.iter().any(node_contains_pass_through_group)
        }
    }
}

fn composite_view_to_view(
    gpu: &RendererGpuState,
    pipelines: &CompositePipelines,
    frame: &mut GpuFrame,
    source_view: &wgpu::TextureView,
    mask_view: Option<&wgpu::TextureView>,
    props: CompositeProps,
    output_view: &wgpu::TextureView,
    clip: Option<RectU32>,
) {
    composite_draw_to_view(
        gpu.device(),
        gpu.paint_sampler(),
        pipelines,
        frame,
        CompositeDrawRequest {
            source: CompositeDrawSource::Texture(source_view),
            mask_view,
            backdrop_view: None,
            props,
            output_view,
            clip,
        },
    );
}

fn clear_composite_target(
    encoder: &mut wgpu::CommandEncoder,
    output_view: &wgpu::TextureView,
    pipelines: &CompositePipelines,
    clip: Option<RectU32>,
    full_clear_label: &'static str,
) {
    if let Some(clip) = clip {
        clear_view_rect(encoder, output_view, &pipelines.clear_rect_pipeline, clip);
    } else {
        clear_rgba_target(encoder, output_view, [0.0, 0.0, 0.0, 0.0], full_clear_label);
    }
}

fn clear_view_rect(
    encoder: &mut wgpu::CommandEncoder,
    output_view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    clip: RectU32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear_material_presentation_clip"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: output_view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_scissor_rect(clip.origin[0], clip.origin[1], clip.size[0], clip.size[1]);
    pass.set_pipeline(pipeline);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use crate::core::{
        composite::{GroupCompositeMode, LayerBlendMode},
        surface::{CompositeGroup, CompositeNode, CompositeProps, CompositeTree},
    };

    use super::{
        composite_group_max_depth, tree_contains_gated_pass_through, tree_uses_shader_blend_modes,
    };

    fn group(children: Vec<CompositeNode>) -> CompositeGroup {
        CompositeGroup {
            layer_id: None,
            mode: GroupCompositeMode::Isolated,
            props: CompositeProps {
                visible: true,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            mask: None,
            children,
        }
    }

    #[test]
    fn general_composite_scratch_depth_tracks_deepest_group_path() {
        let tree = group(vec![
            CompositeNode::Group(group(Vec::new())),
            CompositeNode::Group(group(vec![CompositeNode::Group(group(vec![
                CompositeNode::Group(group(Vec::new())),
            ]))])),
        ]);

        assert_eq!(composite_group_max_depth(&tree, 0), 3);
    }

    #[test]
    fn gated_pass_through_selects_shader_and_full_tree_active_paths() {
        let mut gated = group(Vec::new());
        gated.mode = GroupCompositeMode::PassThrough;
        gated.props.opacity = 0.5;
        let tree = CompositeTree {
            root: group(vec![CompositeNode::Group(gated)]),
        };

        assert!(tree_uses_shader_blend_modes(&tree));
        assert!(tree_contains_gated_pass_through(&tree));
    }

    #[test]
    fn root_pass_through_mode_does_not_require_a_gate() {
        let mut root = group(Vec::new());
        root.mode = GroupCompositeMode::PassThrough;
        let tree = CompositeTree { root };

        assert!(!tree_uses_shader_blend_modes(&tree));
        assert!(!tree_contains_gated_pass_through(&tree));
    }
}
