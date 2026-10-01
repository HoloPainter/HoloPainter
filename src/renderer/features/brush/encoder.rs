use anyhow::{Result, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        brush_engine::{
            BrushResourceDefinition, BuiltinPassInput, ParamValue, PassClearValue, PassOutputLoad,
            RasterMode, ResourceRefValue, SamplerKind, SurfaceTextureSource, SurfaceTextureSync,
            TextureAssetSource, TextureResourceExtent, TextureResourceFormat,
            TextureResourceLifetime, UniformKind,
        },
        damage::{DamageMap, rects_touch, union_rect},
        document::MeshData,
        geometry::RectU32,
        math::gl_to_wgpu_depth,
        selection::{ActiveSelection, SelectionMaskId},
        stroke::{StrokeContext, StrokeDab, SurfaceDab, SurfaceProjectionId},
        stroke_preset::StrokeOp,
        surface::PaintSurfaceId,
        viewport_visibility::ViewportSceneVisibility,
    },
    renderer::{
        command::{RendererStrokeOperation, RendererStrokeStyle},
        document::scene::SceneDepthSlot,
        features::brush::{
            engine_pipelines::{
                BrushPassRuntime, BrushResourceParamField, BrushSpaceResourceUsage,
                CompiledBrushResource, CompiledPassBinding, CompiledResourceRef, PassInputRuntime,
                resource_definition, resource_name,
            },
            pass_deps::BrushPassDeps,
            scratch::{
                material_scratch_key, material_scratch_view, release_material_scratch_after_submit,
                release_viewport_scratch_after_submit, viewport_scratch_key,
                viewport_scratch_texture, viewport_scratch_view,
            },
            transient as stroke_transient,
            types::{BrushDabInstance, CurrentDabBatchHeader, SurfaceDabGpu},
            viewport_cache::{
                SurfaceViewportMaterialSourceKey, SurfaceViewportRuntimeCache,
                SurfaceViewportSourceSnapshotKey, viewport_source_color_cache_name,
            },
        },
        gpu::{clear_rgba_target, copy_a_to_b, copy_rect_a_to_b, frame::GpuFrame},
    },
};

use super::engine_types::{BakeUniform, BrushEngineUniform, UvDabGpu};
use super::params::{resolve_per_dab_f32_slots, resolve_per_dab_f32_slots_for_dab};

#[derive(Default)]
pub(crate) struct BrushPassEncoder;

#[derive(Default)]
struct SourceOnlySnapshotMaterials {
    allocated: Vec<usize>,
    valid: Vec<usize>,
}

struct SurfaceViewportMaterialSource<'a> {
    material_index: usize,
    view: &'a wgpu::TextureView,
    cache_key: Option<SurfaceViewportMaterialSourceKey>,
}

const SURFACE_TARGET_MESH_UV_SCISSOR_GUARD_PX: f32 = 32.0;
const SURFACE_TARGET_MESH_UV_SCISSOR_MAX_AREA_RATIO: f32 = 0.45;
const SURFACE_TARGET_MESH_UV_SCISSOR_MAX_RECTS: usize = 16;
const SURFACE_VIEWPORT_SOURCE_SCISSOR_GUARD_PX: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceScissorFallbackReason {
    MissingData,
    InvalidInput,
    BoundaryLookup,
    NoContribution,
    FullRect,
    RectLimit,
    AreaLimit,
}

#[derive(Debug, PartialEq, Eq)]
enum SurfaceScissorResult {
    Scissored(Vec<RectU32>),
    Full(SurfaceScissorFallbackReason),
}

impl BrushPassEncoder {
    pub(crate) fn begin_uv_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        style: &RendererStrokeStyle,
    ) {
        let Ok((usage, resources, stroke_begin_clears)) = uv_stroke_begin_runtime_data(txn, style)
        else {
            return;
        };
        if usage.requires_scene_mesh() && !txn.scene.has_mesh() {
            return;
        }
        self.ensure_resources_for_usage(frame, txn, target, &usage);
        let material_index = target.material_index().as_usize();
        if let Some(texture_size) = txn.surfaces.surface_texture_size(target) {
            stroke_transient::ensure_material_source_uv(
                txn.scratch,
                txn.gpu.device(),
                frame.encoder(),
                material_index,
                texture_size,
            );
        }
        let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) else {
            return;
        };
        if usage.needs_current_source {
            copy_a_to_b(
                frame.encoder(),
                layer.write_texture,
                layer.read_texture,
                layer.texture_size,
            );
        }
        if let Some(source_texture) =
            stroke_transient::material_source_uv_texture(txn.scratch, material_index)
        {
            copy_a_to_b(
                frame.encoder(),
                layer.write_texture,
                source_texture,
                layer.texture_size,
            );
        }
        let _ = clear_stroke_begin_outputs(
            frame,
            txn,
            target,
            &resources,
            &stroke_begin_clears,
            stroke_transient::material_stroke_uv_view(txn.scratch, material_index),
            false,
        );
    }

    pub(crate) fn stamp_uv_batch(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        dabs: &[StrokeDab],
        instances: &[BrushDabInstance],
        style: &RendererStrokeStyle,
        brush_uniform: &BrushEngineUniform,
        active_selection: &ActiveSelection,
    ) -> Result<()> {
        if dabs.is_empty() {
            return Ok(());
        }
        let usage = runtime_usage(txn, style, false)?;
        if usage.requires_scene_mesh() && !txn.scene.has_mesh() {
            return Ok(());
        }
        self.ensure_resources_for_usage(frame, txn, target, &usage);
        self.prepare_logical_batch_sources(
            frame,
            txn,
            &[target],
            usage.needs_batch_source,
            usage.needs_current_source,
            false,
        );
        self.sync_current_source_for_usage(frame, txn, target, &usage, None, false);
        if usage.reads_current_dab_batch {
            self.write_uv_current_dab_batch(frame, txn, target, dabs, style);
        }
        if usage.uses_uv_dab_quad_instances {
            self.write_uv_dab_quad_instances(frame, txn, instances, style);
        }
        self.record_pass_set(
            frame,
            txn,
            target,
            style,
            brush_uniform,
            active_selection,
            false,
            None,
            dabs.len() as u32,
            None,
            SceneDepthSlot::Main,
        )
    }

    pub(crate) fn finalize_uv_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
    ) {
        if let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) {
            copy_a_to_b(
                frame.encoder(),
                layer.write_texture,
                layer.read_texture,
                layer.texture_size,
            );
        }
        if let Some(stroke_view) = stroke_transient::material_stroke_uv_view(
            txn.scratch,
            target.material_index().as_usize(),
        ) {
            clear_rgba_target(
                frame.encoder(),
                stroke_view,
                [0.0, 0.0, 0.0, 0.0],
                "clear_brush_uv_finalize",
            );
        }
    }

    pub(crate) fn begin_surface_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
    ) {
        let Ok((usage, resources, stroke_begin_clears)) =
            surface_stroke_begin_runtime_data(txn, style)
        else {
            return;
        };
        if usage.requires_scene_mesh() && !txn.scene.has_mesh() {
            return;
        }
        for target in surfaces {
            self.ensure_resources_for_usage(frame, txn, *target, &usage);
            if let Some(texture_size) = txn.surfaces.surface_texture_size(*target) {
                stroke_transient::ensure_material_source_uv(
                    txn.scratch,
                    txn.gpu.device(),
                    frame.encoder(),
                    target.material_index().as_usize(),
                    texture_size,
                );
            }
            if let Some(layer) = txn.surfaces.stroke_surface_target(*target, txn.scratch) {
                if let Some(source_texture) = stroke_transient::material_source_uv_texture(
                    txn.scratch,
                    target.material_index().as_usize(),
                ) {
                    copy_a_to_b(
                        frame.encoder(),
                        layer.write_texture,
                        source_texture,
                        layer.texture_size,
                    );
                    txn.metrics.surface_snapshot_copy_bytes = txn
                        .metrics
                        .surface_snapshot_copy_bytes
                        .saturating_add(rgba8_size_bytes(layer.texture_size));
                }
                let cleared_bytes = clear_stroke_begin_outputs(
                    frame,
                    txn,
                    *target,
                    &resources,
                    &stroke_begin_clears,
                    stroke_transient::material_stroke_uv_view(
                        txn.scratch,
                        target.material_index().as_usize(),
                    ),
                    true,
                );
                txn.metrics.surface_stroke_begin_clear_bytes = txn
                    .metrics
                    .surface_stroke_begin_clear_bytes
                    .saturating_add(cleared_bytes);
            }
        }
    }

    pub(crate) fn cancel_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
    ) {
        for target in surfaces {
            let material_index = target.material_index().as_usize();
            let Some(source_texture) =
                stroke_transient::material_source_uv_texture(txn.scratch, material_index)
            else {
                continue;
            };
            let Some(layer) = txn.surfaces.stroke_surface_target(*target, txn.scratch) else {
                continue;
            };
            // `read_texture` is the source snapshot itself, so restoring it would
            // encode an invalid same-texture copy. Only the persistent/proxy write
            // target was mutated by the stroke and needs to be restored.
            copy_a_to_b(
                frame.encoder(),
                source_texture,
                layer.write_texture,
                layer.texture_size,
            );
        }
    }

    pub(crate) fn stamp_surface_batch(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        source_surfaces: &[PaintSurfaceId],
        target_surfaces: &[PaintSurfaceId],
        dabs: &[SurfaceDab],
        surface_directions: &[glam::Vec2],
        target_materials_by_dab: &[Vec<usize>],
        viewport_instances: &[BrushDabInstance],
        projection_id: crate::core::stroke::SurfaceProjectionId,
        style: &RendererStrokeStyle,
        brush_uniform: &BrushEngineUniform,
        ctx: &StrokeContext,
        active_selection: &ActiveSelection,
        viewport_runtime_cache: &mut SurfaceViewportRuntimeCache,
    ) -> Result<()> {
        let StrokeContext::Surface(surface_ctx) = ctx else {
            return Ok(());
        };
        if dabs.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(dabs.len(), surface_directions.len());
        let (
            surface_viewport_usage,
            usage,
            has_surface_viewport_passes,
            surface_current_source_sync_can_clip,
        ) = surface_stamp_runtime_data(txn, style)?;
        let source_input_usage = viewport_source_input_usage(&surface_viewport_usage, &usage);
        if (source_input_usage.requires_scene_mesh() || usage.requires_scene_mesh())
            && !txn.scene.has_mesh()
        {
            return Ok(());
        }
        let viewport_size = surface_ctx.viewport_size;
        let projection = surface_ctx
            .projection(projection_id)
            .ok_or_else(|| anyhow::anyhow!("missing surface projection {projection_id:?}"))?;
        let depth_slot = SceneDepthSlot::for_surface_projection(projection_id);
        txn.scene.write_view_proj(
            txn.gpu.device(),
            frame,
            gl_to_wgpu_depth() * projection.view_proj_matrix_gl,
        );
        let has_viewport = viewport_size[0] != 0 && viewport_size[1] != 0;
        let mut source_only_snapshots = SourceOnlySnapshotMaterials::default();
        if has_viewport
            && (source_input_usage.needs_surface_viewport_inputs() || has_surface_viewport_passes)
        {
            source_only_snapshots = self.prepare_surface_viewport_inputs(
                frame,
                txn,
                source_surfaces,
                target_surfaces,
                dabs,
                style,
                surface_ctx,
                projection,
                depth_slot,
                &source_input_usage,
                viewport_instances,
                viewport_runtime_cache,
            );
        }
        for material_index in source_only_snapshots.allocated {
            stroke_transient::release_material_source_uv_after_submit(
                txn.scratch,
                frame,
                material_index,
            );
        }
        if has_viewport && surface_viewport_usage.uses_viewport_dab_quad_instances {
            self.write_uv_dab_quad_instances(frame, txn, viewport_instances, style);
        }
        if has_viewport && has_surface_viewport_passes {
            self.ensure_viewport_resources_for_usage(
                frame,
                txn,
                viewport_size,
                &surface_viewport_usage,
            );
            self.record_surface_viewport_pass_set(
                frame,
                txn,
                target_surfaces,
                style,
                brush_uniform,
                active_selection,
                viewport_instances.len() as u32,
                depth_slot,
                &surface_ctx.scene_visibility,
            )?;
        }
        if usage.needs_surface_dabs() {
            self.write_surface_dabs(frame, txn, dabs, surface_directions, style);
        }
        for target in target_surfaces {
            self.ensure_resources_for_usage(frame, txn, *target, &usage);
            let target_mesh_uv_clips = match surface_target_mesh_uv_scissor_result(
                txn,
                *target,
                dabs,
                target_materials_by_dab,
                style,
            ) {
                SurfaceScissorResult::Scissored(rects) => Some(rects),
                SurfaceScissorResult::Full(reason) => {
                    record_surface_scissor_fallback(txn.metrics, reason);
                    None
                }
            };
            let current_source_copy_clip = surface_current_source_sync_clip(
                surface_current_source_sync_can_clip,
                target_mesh_uv_clips.as_deref(),
            );
            self.sync_current_source_for_usage(
                frame,
                txn,
                *target,
                &usage,
                current_source_copy_clip,
                true,
            );
            self.record_pass_set(
                frame,
                txn,
                *target,
                style,
                brush_uniform,
                active_selection,
                true,
                Some(&surface_ctx.scene_visibility),
                dabs.len() as u32,
                target_mesh_uv_clips.as_deref(),
                depth_slot,
            )?;
        }
        if has_viewport {
            release_viewport_scratch_after_submit(
                txn.scratch,
                frame,
                TextureResourceLifetime::Pass,
            );
        }
        Ok(())
    }

    pub(crate) fn prepare_surface_logical_batch_sources(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target_surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
    ) -> Result<()> {
        let (viewport_usage, surface_usage, _, _) = surface_stamp_runtime_data(txn, style)?;
        self.prepare_logical_batch_sources(
            frame,
            txn,
            target_surfaces,
            viewport_usage.needs_batch_source || surface_usage.needs_batch_source,
            viewport_usage.needs_current_source || surface_usage.needs_current_source,
            true,
        );
        Ok(())
    }

    fn prepare_logical_batch_sources(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target_surfaces: &[PaintSurfaceId],
        needs_batch_source: bool,
        needs_current_source: bool,
        record_surface_metrics: bool,
    ) {
        if !needs_batch_source {
            return;
        }
        let needs_dedicated_batch_source = needs_current_source;
        for target in target_surfaces {
            let Some(texture_size) = txn.surfaces.surface_texture_size(*target) else {
                continue;
            };
            if !needs_dedicated_batch_source {
                let Some(layer) = txn.surfaces.stroke_surface_target(*target, txn.scratch) else {
                    continue;
                };
                copy_a_to_b(
                    frame.encoder(),
                    layer.write_texture,
                    layer.read_texture,
                    texture_size,
                );
                if record_surface_metrics {
                    txn.metrics.surface_full_sync_copy_bytes = txn
                        .metrics
                        .surface_full_sync_copy_bytes
                        .saturating_add(rgba8_size_bytes(texture_size));
                }
                continue;
            }
            stroke_transient::ensure_material_batch_source_uv(
                txn.scratch,
                txn.gpu.device(),
                frame.encoder(),
                target.material_index().as_usize(),
                texture_size,
            );
            let Some(layer) = txn.surfaces.stroke_surface_target(*target, txn.scratch) else {
                continue;
            };
            let Some(batch_texture) = stroke_transient::material_batch_source_uv_texture(
                txn.scratch,
                target.material_index().as_usize(),
            ) else {
                continue;
            };
            copy_a_to_b(
                frame.encoder(),
                layer.write_texture,
                batch_texture,
                texture_size,
            );
            if record_surface_metrics {
                txn.metrics.surface_full_sync_copy_bytes = txn
                    .metrics
                    .surface_full_sync_copy_bytes
                    .saturating_add(rgba8_size_bytes(texture_size));
            }
        }
    }

    pub(crate) fn finalize_surface_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
        damage: Option<&DamageMap>,
    ) {
        let _ = style;

        for target in surfaces {
            let Some(layer) = txn.surfaces.stroke_surface_target(*target, txn.scratch) else {
                continue;
            };
            copy_surface_damage_a_to_b(
                frame.encoder(),
                layer.write_texture,
                layer.read_texture,
                layer.texture_size,
                *target,
                damage,
            );
        }
    }

    fn ensure_resources_for_usage(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        usage: &BrushSpaceResourceUsage,
    ) {
        let Some(tex_size) = txn.surfaces.surface_texture_size(target) else {
            return;
        };
        let material_index = target.material_index().as_usize();
        if usage.needs_material_source_texture() {
            stroke_transient::ensure_material_source_uv(
                txn.scratch,
                txn.gpu.device(),
                frame.encoder(),
                material_index,
                tex_size,
            );
        }
        for scratch_resource in &usage.scratch_resources {
            if scratch_resource.extent != TextureResourceExtent::PaintSurface {
                continue;
            }
            txn.scratch.ensure_transient_texture(
                txn.gpu.device(),
                frame.encoder(),
                material_scratch_key(target, &scratch_resource.name, scratch_resource.lifetime),
                tex_size,
                texture_format(scratch_resource.format),
                "brush_scratch_uv_tex",
                "clear_brush_scratch_uv",
            );
        }
    }

    fn ensure_viewport_resources_for_usage(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        viewport_size: [u32; 2],
        usage: &BrushSpaceResourceUsage,
    ) {
        for scratch_resource in &usage.scratch_resources {
            if scratch_resource.extent != TextureResourceExtent::Viewport {
                continue;
            }
            txn.scratch.ensure_transient_texture(
                txn.gpu.device(),
                frame.encoder(),
                viewport_scratch_key(&scratch_resource.name, scratch_resource.lifetime),
                viewport_size,
                texture_format(scratch_resource.format),
                "brush_scratch_viewport_tex",
                "clear_brush_scratch_viewport",
            );
        }
    }

    fn sync_current_source_for_usage(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        usage: &BrushSpaceResourceUsage,
        copy_clips: Option<&[RectU32]>,
        record_surface_metrics: bool,
    ) {
        if !usage.needs_current_source {
            return;
        }
        let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) else {
            return;
        };
        if record_surface_metrics {
            record_surface_sync_copy_metrics(txn.metrics, layer.texture_size, copy_clips);
        }
        copy_surface_rects_a_to_b(
            frame.encoder(),
            layer.write_texture,
            layer.read_texture,
            layer.texture_size,
            copy_clips,
        );
    }

    fn write_uv_current_dab_batch(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        dabs: &[StrokeDab],
        style: &RendererStrokeStyle,
    ) {
        let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) else {
            return;
        };
        let Some(radius_world) = brush_radius_world(style) else {
            return;
        };
        let uv_linear_scale = txn
            .scene
            .mesh()
            .map(|mesh| mesh.uv_linear_scale)
            .unwrap_or(1.0)
            .max(0.0);
        let tex_scale =
            ((layer.texture_size[0].max(1) as f32) * (layer.texture_size[1].max(1) as f32)).sqrt();
        let radius_px = (radius_world.max(1e-4) * uv_linear_scale * tex_scale).max(1e-6);
        txn.stroke_resources.ensure_current_dab_batch_capacity(
            std::mem::size_of::<UvDabGpu>() as u64,
            dabs.len() as u32,
        );
        let header = CurrentDabBatchHeader {
            count: dabs.len() as u32,
            _pad0: [0, 0, 0],
        };
        let tex_size = [
            layer.texture_size[0].max(1) as f32,
            layer.texture_size[1].max(1) as f32,
        ];
        let gpu_dabs: Vec<UvDabGpu> = dabs
            .iter()
            .map(|dab| {
                let dynamic_values = runtime_for_style(txn, style)
                    .map(|runtime| {
                        resolve_per_dab_f32_slots_for_dab(style, runtime.param_layout(), dab)
                    })
                    .unwrap_or([[0.0; 4]; 4]);
                UvDabGpu::from_dab(dab, radius_px, tex_size, dynamic_values)
            })
            .collect();
        frame.write_buffer_pod(
            txn.gpu.device(),
            txn.stroke_resources.current_dab_batch(),
            0,
            &header,
        );
        if !gpu_dabs.is_empty() {
            frame.write_buffer_slice(
                txn.gpu.device(),
                txn.stroke_resources.current_dab_batch(),
                std::mem::size_of::<CurrentDabBatchHeader>() as u64,
                &gpu_dabs,
            );
        }
    }

    fn write_uv_dab_quad_instances(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        instances: &[BrushDabInstance],
        style: &RendererStrokeStyle,
    ) {
        let runtime = runtime_for_style(txn, style).ok();
        let instances = instances
            .iter()
            .map(|instance| {
                let mut instance = *instance;
                if let Some(runtime) = runtime {
                    let dynamic_values =
                        resolve_per_dab_f32_slots(style, runtime.param_layout(), instance.pressure);
                    instance.dynamic0 = dynamic_values[0];
                    instance.dynamic1 = dynamic_values[1];
                    instance.dynamic2 = dynamic_values[2];
                    instance.dynamic3 = dynamic_values[3];
                }
                instance
            })
            .collect::<Vec<_>>();
        txn.stroke_resources
            .ensure_instance_capacity(instances.len() as u32);
        frame.write_buffer_slice(
            txn.gpu.device(),
            txn.stroke_resources.instances(),
            0,
            &instances,
        );
    }

    fn write_surface_current_dab_batch(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        dabs: &[SurfaceDabGpu],
    ) {
        txn.stroke_resources.ensure_current_dab_batch_capacity(
            std::mem::size_of::<SurfaceDabGpu>() as u64,
            dabs.len() as u32,
        );
        let header = CurrentDabBatchHeader {
            count: dabs.len() as u32,
            _pad0: [0, 0, 0],
        };
        frame.write_buffer_pod(
            txn.gpu.device(),
            txn.stroke_resources.current_dab_batch(),
            0,
            &header,
        );
        if !dabs.is_empty() {
            frame.write_buffer_slice(
                txn.gpu.device(),
                txn.stroke_resources.current_dab_batch(),
                std::mem::size_of::<CurrentDabBatchHeader>() as u64,
                dabs,
            );
        }
    }

    fn write_surface_dabs(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        dabs: &[SurfaceDab],
        surface_directions: &[glam::Vec2],
        style: &RendererStrokeStyle,
    ) {
        let Some(radius_world) = brush_radius_world(style) else {
            return;
        };
        txn.stroke_resources
            .ensure_surface_dab_capacity(dabs.len() as u32);
        let gpu_dabs: Vec<SurfaceDabGpu> = dabs
            .iter()
            .zip(surface_directions)
            .map(|(dab, direction)| {
                let radius = (radius_world * dab.radius_scale).max(1e-6);
                let dynamic_values = runtime_for_style(txn, style)
                    .map(|runtime| {
                        resolve_per_dab_f32_slots(style, runtime.param_layout(), dab.pressure)
                    })
                    .unwrap_or([[0.0; 4]; 4]);
                SurfaceDabGpu {
                    center_radius: [dab.world_pos.x, dab.world_pos.y, dab.world_pos.z, radius],
                    normal_pressure: [
                        dab.world_normal.x,
                        dab.world_normal.y,
                        dab.world_normal.z,
                        dab.pressure,
                    ],
                    tangent_x_pad: [
                        dab.tangent_x.x,
                        dab.tangent_x.y,
                        dab.tangent_x.z,
                        direction.x,
                    ],
                    tangent_y_pad: [
                        dab.tangent_y.x,
                        dab.tangent_y.y,
                        dab.tangent_y.z,
                        direction.y,
                    ],
                    dynamic0: dynamic_values[0],
                    dynamic1: dynamic_values[1],
                    dynamic2: dynamic_values[2],
                    dynamic3: dynamic_values[3],
                }
            })
            .collect();
        frame.write_buffer_slice(
            txn.gpu.device(),
            txn.stroke_resources.surface_dabs(),
            0,
            &gpu_dabs,
        );
        self.write_surface_current_dab_batch(frame, txn, &gpu_dabs);
    }

    fn prepare_surface_viewport_inputs(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        source_surfaces: &[PaintSurfaceId],
        target_surfaces: &[PaintSurfaceId],
        dabs: &[SurfaceDab],
        style: &RendererStrokeStyle,
        surface_ctx: &crate::core::stroke::SurfaceStrokeContext,
        projection: &crate::core::stroke::SurfaceProjectionContext,
        depth_slot: SceneDepthSlot,
        usage: &BrushSpaceResourceUsage,
        _viewport_instances: &[BrushDabInstance],
        viewport_runtime_cache: &mut SurfaceViewportRuntimeCache,
    ) -> SourceOnlySnapshotMaterials {
        let viewport_size = surface_ctx.viewport_size;
        if viewport_size[0] == 0 || viewport_size[1] == 0 {
            return SourceOnlySnapshotMaterials::default();
        }
        let camera = surface_ctx.camera;
        let viewport_view_proj_gl = projection.view_proj_matrix_gl;
        if usage.needs_viewport_depth {
            let visible_index_ranges = txn
                .scene
                .mesh()
                .map(|mesh| mesh.visible_index_ranges(&surface_ctx.scene_visibility))
                .unwrap_or_default();
            txn.scene.ensure_scene_depth_for_index_ranges(
                txn.gpu,
                txn.scene_capture,
                &txn.scene_capture_pipelines.viewport_prepass_bgl,
                &txn.scene_capture_pipelines.viewport_prepass_pipeline,
                frame,
                depth_slot,
                viewport_size,
                gl_to_wgpu_depth() * viewport_view_proj_gl,
                &visible_index_ranges,
            );
        }

        let mut source_only_snapshots = SourceOnlySnapshotMaterials::default();

        if usage.needs_viewport_source_color {
            let use_stroke_snapshot = usage.needs_stroke_snapshot
                && !usage.needs_batch_source
                && !usage.needs_current_source;
            let capture_rect = (!use_stroke_snapshot).then(|| {
                surface_viewport_source_capture_rect(
                    dabs,
                    style,
                    projection.view_proj_matrix_gl,
                    viewport_size,
                )
            });
            let capture_rect = capture_rect.flatten();
            let cache_key = use_stroke_snapshot.then(|| {
                surface_viewport_source_snapshot_key(
                    surface_ctx,
                    projection,
                    source_surfaces,
                    txn.material_count,
                )
            });
            let cache_name = viewport_source_color_cache_name(projection.id);
            let cache_hit = cache_key.as_ref().is_some_and(|key| {
                viewport_runtime_cache.source_snapshot_key(projection.id) == Some(key)
                    && viewport_scratch_texture(
                        txn.scratch,
                        cache_name,
                        TextureResourceLifetime::Stroke,
                    )
                    .is_some()
            });

            if cache_hit {
                restore_cached_viewport_source_color(frame, txn, projection.id, viewport_size);
            } else {
                if use_stroke_snapshot {
                    for target in source_surfaces {
                        if target.material_index().as_usize() >= txn.material_count {
                            continue;
                        }
                        if contains_surface(target_surfaces, *target) {
                            if !self.ensure_material_source_for_surface(frame, txn, *target) {
                                continue;
                            }
                            if let Some(layer) =
                                txn.surfaces.stroke_surface_target(*target, txn.scratch)
                            {
                                let Some(source_texture) =
                                    stroke_transient::material_source_uv_texture(
                                        txn.scratch,
                                        target.material_index().as_usize(),
                                    )
                                else {
                                    continue;
                                };
                                copy_a_to_b(
                                    frame.encoder(),
                                    layer.write_texture,
                                    source_texture,
                                    layer.texture_size,
                                );
                            }
                        } else {
                            if !self.ensure_material_source_for_surface(frame, txn, *target) {
                                continue;
                            }
                            if !source_only_snapshots
                                .allocated
                                .contains(&target.material_index().as_usize())
                            {
                                source_only_snapshots
                                    .allocated
                                    .push(target.material_index().as_usize());
                            }
                            let source = (
                                txn.surfaces.surface_texture_size(*target),
                                txn.surfaces.surface_texture(*target),
                            );
                            if let (Some(texture_size), Some(surface_texture)) = source {
                                let Some(source_texture) =
                                    stroke_transient::material_source_uv_texture(
                                        txn.scratch,
                                        target.material_index().as_usize(),
                                    )
                                else {
                                    continue;
                                };
                                copy_a_to_b(
                                    frame.encoder(),
                                    surface_texture,
                                    source_texture,
                                    texture_size,
                                );
                                if !source_only_snapshots
                                    .valid
                                    .contains(&target.material_index().as_usize())
                                {
                                    source_only_snapshots
                                        .valid
                                        .push(target.material_index().as_usize());
                                }
                            }
                        }
                    }
                } else if usage.needs_current_source {
                    for target in target_surfaces {
                        if target.material_index().as_usize() >= txn.material_count {
                            continue;
                        }
                        self.ensure_material_source_for_surface(frame, txn, *target);
                        if let Some(layer) =
                            txn.surfaces.stroke_surface_target(*target, txn.scratch)
                        {
                            // The capture is scissored in viewport space, but any visible mesh fragment
                            // inside that rect may sample outside the target UV damage bounds. Keep
                            // the material read side complete before rendering the scene source.
                            copy_a_to_b(
                                frame.encoder(),
                                layer.write_texture,
                                layer.read_texture,
                                layer.texture_size,
                            );
                        }
                    }
                }

                let mut material_sources = Vec::with_capacity(source_surfaces.len());
                for target in source_surfaces {
                    let material_index = target.material_index().as_usize();
                    if material_index >= txn.material_count {
                        continue;
                    }
                    if use_stroke_snapshot {
                        let is_target = contains_surface(target_surfaces, *target);
                        let has_valid_snapshot =
                            is_target || source_only_snapshots.valid.contains(&material_index);
                        if has_valid_snapshot {
                            if let Some(source_view) = stroke_transient::material_source_uv_view(
                                txn.scratch,
                                material_index,
                            ) {
                                material_sources.push(SurfaceViewportMaterialSource {
                                    material_index,
                                    view: source_view,
                                    // Source-only snapshot textures are released after this
                                    // submission, so snapshot bind groups stay local to the capture.
                                    cache_key: None,
                                });
                            }
                        }
                    } else if contains_surface(target_surfaces, *target) && usage.needs_batch_source
                    {
                        if let Some(layer) =
                            txn.surfaces.stroke_surface_target(*target, txn.scratch)
                        {
                            let (view, cache_key) = if let Some(batch_view) =
                                stroke_transient::material_batch_source_uv_view(
                                    txn.scratch,
                                    material_index,
                                ) {
                                (
                                    batch_view,
                                    SurfaceViewportMaterialSourceKey::BatchSource(material_index),
                                )
                            } else {
                                (
                                    layer.read_view,
                                    SurfaceViewportMaterialSourceKey::StrokeSource(material_index),
                                )
                            };
                            material_sources.push(SurfaceViewportMaterialSource {
                                material_index,
                                view,
                                cache_key: Some(cache_key),
                            });
                        }
                    } else if contains_surface(target_surfaces, *target)
                        && usage.needs_current_source
                    {
                        if let Some(layer) =
                            txn.surfaces.stroke_surface_target(*target, txn.scratch)
                        {
                            material_sources.push(SurfaceViewportMaterialSource {
                                material_index,
                                view: layer.read_view,
                                cache_key: Some(SurfaceViewportMaterialSourceKey::StrokeSource(
                                    material_index,
                                )),
                            });
                        }
                    } else if let Some(surface_view) = txn.surfaces.surface_texture_view(*target) {
                        material_sources.push(SurfaceViewportMaterialSource {
                            material_index,
                            view: surface_view,
                            cache_key: Some(SurfaceViewportMaterialSourceKey::PersistentSurface(
                                *target,
                            )),
                        });
                    }
                }

                let mut uncached_bind_groups = Vec::new();
                for source in &material_sources {
                    if let Some(cache_key) = source.cache_key {
                        if viewport_runtime_cache
                            .material_bind_group(cache_key)
                            .is_none()
                        {
                            let bind_group = txn.scene_capture.create_viewport_material_bind_group(
                                txn.gpu,
                                txn.scene_capture_pipelines,
                                txn.scene,
                                source.view,
                            );
                            viewport_runtime_cache
                                .insert_material_bind_group(cache_key, bind_group);
                        }
                    } else {
                        uncached_bind_groups.push(
                            txn.scene_capture.create_viewport_material_bind_group(
                                txn.gpu,
                                txn.scene_capture_pipelines,
                                txn.scene,
                                source.view,
                            ),
                        );
                    }
                }

                let mut uncached_index = 0usize;
                let mut material_bind_groups = Vec::with_capacity(material_sources.len());
                for source in &material_sources {
                    let bind_group = if let Some(cache_key) = source.cache_key {
                        viewport_runtime_cache
                            .material_bind_group(cache_key)
                            .expect("cached viewport material bind group must exist")
                    } else {
                        let bind_group = &uncached_bind_groups[uncached_index];
                        uncached_index += 1;
                        bind_group
                    };
                    material_bind_groups.push((source.material_index, bind_group));
                }

                txn.scene_capture
                    .record_filter_backup_scene_color_from_material_bind_groups(
                        txn.gpu,
                        txn.scene_capture_pipelines,
                        txn.scene,
                        frame,
                        viewport_view_proj_gl,
                        projection.camera_world,
                        viewport_size,
                        &material_bind_groups,
                        capture_rect,
                        &surface_ctx.scene_visibility,
                    );

                if let Some(cache_key) = cache_key {
                    cache_viewport_source_color(frame, txn, projection.id, viewport_size);
                    viewport_runtime_cache.set_source_snapshot_key(projection.id, cache_key);
                }
            }
        }
        let inv_tan_half_fov = 1.0 / (0.5 * camera.fov_y_radians).tan();
        let pixels_per_world_unit = viewport_size[1] as f32 / camera.orthographic_height.max(0.01);
        let projection_type = match camera.projection {
            crate::core::camera::CameraProjection::Perspective => 0.0,
            crate::core::camera::CameraProjection::Orthographic => 1.0,
        };
        let bake = BakeUniform {
            camera_world: [
                projection.camera_world[0],
                projection.camera_world[1],
                projection.camera_world[2],
                0.0,
            ],
            paint_color: [1.0, 1.0, 1.0, 1.0],
            viewport_depth: [
                viewport_size[0] as f32,
                viewport_size[1] as f32,
                0.0001,
                0.0,
            ],
            viewport_metrics: [
                viewport_size[0] as f32,
                viewport_size[1] as f32,
                inv_tan_half_fov,
                pixels_per_world_unit,
            ],
            depth_params: [camera.near, camera.far, projection_type, 0.0],
        };
        frame.write_buffer_pod(txn.gpu.device(), &txn.pipelines.bake_uniform, 0, &bake);
        source_only_snapshots
    }

    fn ensure_material_source_for_surface(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
    ) -> bool {
        let Some(tex_size) = txn.surfaces.surface_texture_size(target) else {
            return false;
        };
        stroke_transient::ensure_material_source_uv(
            txn.scratch,
            txn.gpu.device(),
            frame.encoder(),
            target.material_index().as_usize(),
            tex_size,
        );
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn record_surface_viewport_pass_set(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
        brush_uniform: &BrushEngineUniform,
        active_selection: &ActiveSelection,
        dab_instance_count: u32,
        depth_slot: SceneDepthSlot,
        scene_visibility: &ViewportSceneVisibility,
    ) -> Result<()> {
        let Some(target) = surfaces
            .iter()
            .copied()
            .find(|target| target.material_index().as_usize() < txn.material_count)
        else {
            return Ok(());
        };
        let engine_id = brush_engine_id(style)?;
        let runtime = txn
            .pipelines
            .runtime(engine_id)
            .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))?;
        let brush_uniform =
            brush_uniform_for_pass_set(txn, target, style, runtime, brush_uniform, true);
        frame.write_buffer_pod(
            txn.gpu.device(),
            &txn.pipelines.brush_uniform,
            0,
            &brush_uniform,
        );
        txn.metrics.surface_brush_pass_count = txn
            .metrics
            .surface_brush_pass_count
            .saturating_add(runtime.surface_viewport_passes().len());
        for pass_runtime in runtime.surface_viewport_passes() {
            self.record_single_pass(
                frame,
                txn,
                target,
                runtime.resources(),
                pass_runtime,
                style,
                active_selection,
                dab_instance_count,
                None,
                depth_slot,
                Some(scene_visibility),
            )?;
        }
        Ok(())
    }

    fn record_pass_set(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        style: &RendererStrokeStyle,
        brush_uniform: &BrushEngineUniform,
        active_selection: &ActiveSelection,
        surface: bool,
        scene_visibility: Option<&ViewportSceneVisibility>,
        dab_instance_count: u32,
        target_mesh_uv_clips: Option<&[RectU32]>,
        depth_slot: SceneDepthSlot,
    ) -> Result<()> {
        let engine_id = brush_engine_id(style)?;
        let runtime = txn
            .pipelines
            .runtime(engine_id)
            .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))?;
        let passes = if surface {
            runtime.surface_passes()
        } else {
            runtime.uv_passes()
        };
        let material_index = target.material_index().as_usize();
        if material_index >= txn.material_count {
            return Ok(());
        }
        let brush_uniform =
            brush_uniform_for_pass_set(txn, target, style, runtime, brush_uniform, surface);
        frame.write_buffer_pod(
            txn.gpu.device(),
            &txn.pipelines.brush_uniform,
            0,
            &brush_uniform,
        );
        let usage = if surface {
            runtime.surface_usage()
        } else {
            runtime.uv_usage()
        };
        let update_current_source = passes.iter().any(|pass| pass.reads_current_source);
        // Surface engines that read StrokeSnapshot must keep that snapshot fixed
        // across all dab batches so opacity remains a stroke-level cap. Only
        // refresh the read side after direct canvas output when later passes need
        // CurrentSource, or for the incremental UV snapshot path below.
        // The incremental UV path covers engines that intentionally sample a
        // stroke snapshot while clearing their per-batch scratch outputs. Engines
        // that need live feedback, such as smudge, should declare CurrentSource
        // in RON instead.
        let sync_incremental_uv_canvas_output = !surface
            && usage.needs_stroke_snapshot
            && scratch_texture_clears_every_pass(passes, runtime.resources());

        if surface {
            txn.metrics.surface_brush_pass_count = txn
                .metrics
                .surface_brush_pass_count
                .saturating_add(passes.len());
        }

        for pass_runtime in passes {
            self.record_single_pass(
                frame,
                txn,
                target,
                runtime.resources(),
                pass_runtime,
                style,
                active_selection,
                dab_instance_count,
                target_mesh_uv_clips,
                depth_slot,
                scene_visibility,
            )?;
            if pass_runtime.writes_canvas()
                && (update_current_source || sync_incremental_uv_canvas_output)
            {
                if let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) {
                    if surface {
                        record_surface_sync_copy_metrics(
                            txn.metrics,
                            layer.texture_size,
                            surface_canvas_write_sync_clip(
                                surface,
                                pass_runtime,
                                target_mesh_uv_clips,
                            ),
                        );
                    }
                    copy_surface_rects_a_to_b(
                        frame.encoder(),
                        layer.write_texture,
                        layer.read_texture,
                        layer.texture_size,
                        surface_canvas_write_sync_clip(surface, pass_runtime, target_mesh_uv_clips),
                    );
                }
            }
        }
        release_material_scratch_after_submit(
            txn.scratch,
            frame,
            material_index,
            TextureResourceLifetime::Pass,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_single_pass(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        resources: &[CompiledBrushResource],
        pass_runtime: &BrushPassRuntime,
        style: &RendererStrokeStyle,
        active_selection: &ActiveSelection,
        dab_instance_count: u32,
        target_mesh_uv_clips: Option<&[RectU32]>,
        depth_slot: SceneDepthSlot,
        scene_visibility: Option<&ViewportSceneVisibility>,
    ) -> Result<()> {
        let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) else {
            return Ok(());
        };
        if pass_runtime.raster == RasterMode::TargetMeshUv {
            record_surface_target_mesh_uv_pass_metrics(
                txn.metrics,
                layer.texture_size,
                target_mesh_uv_clips,
            );
        }
        let material_index = target.material_index().as_usize();
        let stroke_view = stroke_transient::material_stroke_uv_view(txn.scratch, material_index);
        let output_views = pass_runtime
            .outputs
            .iter()
            .map(|output| {
                output_texture_view(
                    txn,
                    target,
                    resources,
                    output.resource,
                    layer.write_view,
                    stroke_view,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let mut entries = Vec::with_capacity(pass_runtime.inputs.len());
        for input in &pass_runtime.inputs {
            entries.push(bind_group_entry_for_input(
                txn,
                target,
                resources,
                input,
                layer.read_view,
                layer.write_view,
                stroke_view,
                style,
                active_selection,
                &pass_runtime.resource_params,
                depth_slot,
            )?);
        }
        let bind_group = txn
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush_pass_bg"),
                layout: &pass_runtime.bind_group_layout,
                entries: &entries,
            });
        let color_attachments = output_views
            .iter()
            .zip(pass_runtime.outputs.iter())
            .map(|(view, output)| {
                Some(wgpu::RenderPassColorAttachment {
                    view: *view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op(output.load, output.clear_value.as_ref()),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })
            })
            .collect::<Vec<_>>();
        let mut render_pass = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&format!("brush_{}_pass", pass_runtime.name)),
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        render_pass.set_pipeline(&pass_runtime.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        match pass_runtime.raster {
            RasterMode::FullscreenTriangle => render_pass.draw(0..3, 0..1),
            RasterMode::TargetMeshUv => {
                let Some(mesh) = txn.scene.mesh() else {
                    return Ok(());
                };
                render_pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                render_pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                if let Some(clips) = target_mesh_uv_clips {
                    for clip in clips {
                        render_pass.set_scissor_rect(
                            clip.origin[0],
                            clip.origin[1],
                            clip.size[0],
                            clip.size[1],
                        );
                        for sm in mesh
                            .sub_meshes_for_material(target.material_index().as_usize())
                            .filter(|sm| {
                                scene_visibility.is_none_or(|visibility| sm.is_visible(visibility))
                            })
                        {
                            render_pass.draw_indexed(
                                sm.index_start..(sm.index_start + sm.index_count),
                                0,
                                0..1,
                            );
                        }
                    }
                } else {
                    for sm in mesh
                        .sub_meshes_for_material(target.material_index().as_usize())
                        .filter(|sm| {
                            scene_visibility.is_none_or(|visibility| sm.is_visible(visibility))
                        })
                    {
                        render_pass.draw_indexed(
                            sm.index_start..(sm.index_start + sm.index_count),
                            0,
                            0..1,
                        );
                    }
                }
            }
            RasterMode::UVDabQuadInstances | RasterMode::ViewportDabQuadInstances => {
                render_pass.set_vertex_buffer(0, txn.gpu.viewport_quad().slice(..));
                render_pass.set_vertex_buffer(1, txn.stroke_resources.instances().slice(..));
                render_pass.draw(0..6, 0..dab_instance_count);
            }
        }
        Ok(())
    }
}

fn surface_current_source_readers_use_target_mesh_uv(passes: &[BrushPassRuntime]) -> bool {
    passes
        .iter()
        .filter(|pass| pass.reads_current_source())
        .all(|pass| pass.raster == RasterMode::TargetMeshUv)
}

fn surface_current_source_sync_clip(
    can_clip: bool,
    target_mesh_uv_clips: Option<&[RectU32]>,
) -> Option<&[RectU32]> {
    can_clip.then_some(target_mesh_uv_clips).flatten()
}

fn surface_canvas_write_sync_clip<'a>(
    surface: bool,
    pass_runtime: &BrushPassRuntime,
    target_mesh_uv_clips: Option<&'a [RectU32]>,
) -> Option<&'a [RectU32]> {
    (surface && pass_runtime.raster == RasterMode::TargetMeshUv)
        .then_some(target_mesh_uv_clips)
        .flatten()
}

fn copy_surface_rects_a_to_b(
    enc: &mut wgpu::CommandEncoder,
    paint_a: &wgpu::Texture,
    paint_b: &wgpu::Texture,
    texture_size: [u32; 2],
    rects: Option<&[RectU32]>,
) {
    let Some(rects) = rects else {
        copy_a_to_b(enc, paint_a, paint_b, texture_size);
        return;
    };
    if rects.is_empty() {
        copy_a_to_b(enc, paint_a, paint_b, texture_size);
        return;
    }
    for rect in rects {
        if rect.origin == [0, 0] && rect.size == texture_size {
            copy_a_to_b(enc, paint_a, paint_b, texture_size);
            return;
        }
    }
    for rect in rects {
        copy_rect_a_to_b(enc, paint_a, paint_b, *rect);
    }
}

fn copy_surface_damage_a_to_b(
    enc: &mut wgpu::CommandEncoder,
    paint_a: &wgpu::Texture,
    paint_b: &wgpu::Texture,
    texture_size: [u32; 2],
    target: PaintSurfaceId,
    damage: Option<&DamageMap>,
) {
    let Some(damage) = damage else {
        copy_a_to_b(enc, paint_a, paint_b, texture_size);
        return;
    };

    for pixel in damage.pixels.iter().filter(|pixel| pixel.surface == target) {
        if pixel.rect.origin == [0, 0] && pixel.rect.size == texture_size {
            copy_a_to_b(enc, paint_a, paint_b, texture_size);
            return;
        }
        copy_rect_a_to_b(enc, paint_a, paint_b, pixel.rect);
    }
}

fn pixel_area(size: [u32; 2]) -> usize {
    (size[0] as usize).saturating_mul(size[1] as usize)
}

fn total_rect_area(rects: &[RectU32]) -> usize {
    rects
        .iter()
        .map(|rect| pixel_area(rect.size))
        .fold(0usize, usize::saturating_add)
}

fn rgba8_size_bytes(size: [u32; 2]) -> usize {
    pixel_area(size).saturating_mul(4)
}

fn record_surface_sync_copy_metrics(
    metrics: &mut crate::core::render_report::RenderMetrics,
    texture_size: [u32; 2],
    rects: Option<&[RectU32]>,
) {
    let full_bytes = rgba8_size_bytes(texture_size);
    let Some(rects) = rects else {
        metrics.surface_full_sync_copy_bytes = metrics
            .surface_full_sync_copy_bytes
            .saturating_add(full_bytes);
        return;
    };
    if rects.is_empty()
        || rects
            .iter()
            .any(|rect| rect.origin == [0, 0] && rect.size == texture_size)
    {
        metrics.surface_full_sync_copy_bytes = metrics
            .surface_full_sync_copy_bytes
            .saturating_add(full_bytes);
    } else {
        metrics.surface_clipped_sync_copy_bytes = metrics
            .surface_clipped_sync_copy_bytes
            .saturating_add(total_rect_area(rects).saturating_mul(4));
    }
}

fn record_surface_scissor_fallback(
    metrics: &mut crate::core::render_report::RenderMetrics,
    reason: SurfaceScissorFallbackReason,
) {
    let counter = match reason {
        SurfaceScissorFallbackReason::MissingData => {
            &mut metrics.surface_scissor_fallback_missing_data
        }
        SurfaceScissorFallbackReason::InvalidInput => {
            &mut metrics.surface_scissor_fallback_invalid_input
        }
        SurfaceScissorFallbackReason::BoundaryLookup => {
            &mut metrics.surface_scissor_fallback_boundary_lookup
        }
        SurfaceScissorFallbackReason::NoContribution => {
            &mut metrics.surface_scissor_fallback_no_contribution
        }
        SurfaceScissorFallbackReason::FullRect => &mut metrics.surface_scissor_fallback_full_rect,
        SurfaceScissorFallbackReason::RectLimit => &mut metrics.surface_scissor_fallback_rect_limit,
        SurfaceScissorFallbackReason::AreaLimit => &mut metrics.surface_scissor_fallback_area_limit,
    };
    *counter = counter.saturating_add(1);
}

fn record_surface_target_mesh_uv_pass_metrics(
    metrics: &mut crate::core::render_report::RenderMetrics,
    texture_size: [u32; 2],
    clips: Option<&[RectU32]>,
) {
    metrics.surface_target_mesh_uv_pass_count =
        metrics.surface_target_mesh_uv_pass_count.saturating_add(1);
    match clips {
        Some(clips) => {
            metrics.surface_target_mesh_uv_scissored_pass_count = metrics
                .surface_target_mesh_uv_scissored_pass_count
                .saturating_add(1);
            metrics.surface_target_mesh_uv_scissor_rect_count = metrics
                .surface_target_mesh_uv_scissor_rect_count
                .saturating_add(clips.len());
            metrics.surface_target_mesh_uv_scissor_pixel_area = metrics
                .surface_target_mesh_uv_scissor_pixel_area
                .saturating_add(total_rect_area(clips));
        }
        None => {
            metrics.surface_target_mesh_uv_full_pass_count = metrics
                .surface_target_mesh_uv_full_pass_count
                .saturating_add(1);
            metrics.surface_target_mesh_uv_full_pixel_area = metrics
                .surface_target_mesh_uv_full_pixel_area
                .saturating_add(pixel_area(texture_size));
        }
    }
}

fn surface_target_mesh_uv_scissor_result(
    txn: &BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    dabs: &[SurfaceDab],
    target_materials_by_dab: &[Vec<usize>],
    style: &RendererStrokeStyle,
) -> SurfaceScissorResult {
    let Some(texture_size) = txn.surfaces.surface_texture_size(target) else {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
    };
    let Some(mesh) = txn.scene.mesh() else {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
    };
    let uv_linear_scale = mesh.uv_linear_scale.max(0.0);
    let Some(radius_world) = brush_radius_world(style) else {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::InvalidInput);
    };
    surface_target_mesh_uv_scissor_result_for_dabs(
        Some(&mesh.source_mesh),
        texture_size,
        target.material_index().as_usize(),
        dabs,
        target_materials_by_dab,
        radius_world,
        uv_linear_scale,
    )
}

#[cfg(test)]
fn surface_target_mesh_uv_scissor_rects_for_dabs(
    mesh: Option<&MeshData>,
    texture_size: [u32; 2],
    material_index: usize,
    dabs: &[SurfaceDab],
    target_materials_by_dab: &[Vec<usize>],
    radius_world: f32,
    uv_linear_scale: f32,
) -> Option<Vec<RectU32>> {
    match surface_target_mesh_uv_scissor_result_for_dabs(
        mesh,
        texture_size,
        material_index,
        dabs,
        target_materials_by_dab,
        radius_world,
        uv_linear_scale,
    ) {
        SurfaceScissorResult::Scissored(rects) => Some(rects),
        SurfaceScissorResult::Full(_) => None,
    }
}

fn surface_target_mesh_uv_scissor_result_for_dabs(
    mesh: Option<&MeshData>,
    texture_size: [u32; 2],
    material_index: usize,
    dabs: &[SurfaceDab],
    target_materials_by_dab: &[Vec<usize>],
    radius_world: f32,
    uv_linear_scale: f32,
) -> SurfaceScissorResult {
    let texture_width = texture_size[0].max(1) as f32;
    let texture_height = texture_size[1].max(1) as f32;
    let radius_uv_base = radius_world.max(0.0) * uv_linear_scale.max(0.0);
    if !(radius_uv_base.is_finite() && radius_uv_base > 0.0) {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::InvalidInput);
    }

    if target_materials_by_dab.len() != dabs.len() {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
    }

    let guard_px = SURFACE_TARGET_MESH_UV_SCISSOR_GUARD_PX;
    let guard_uv = (guard_px / texture_width).max(guard_px / texture_height);
    let mut rects = Vec::new();
    let mut saw_dab = false;

    for (dab, target_materials) in dabs.iter().zip(target_materials_by_dab) {
        if !target_materials.contains(&material_index) {
            continue;
        }
        let Some(uv) = dab.uv else {
            return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
        };
        let Some(uv_paint_boundary_distance) = dab.uv_paint_boundary_distance else {
            return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
        };
        let radius_uv = radius_uv_base * dab.radius_scale.max(0.0);
        if !(uv.x.is_finite()
            && uv.y.is_finite()
            && !uv_paint_boundary_distance.is_nan()
            && radius_uv.is_finite())
        {
            return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::InvalidInput);
        }

        let mut contributed = false;
        if dab.material_index == material_index {
            let center_x = uv.x * texture_width;
            let center_y = uv.y * texture_height;
            let radius_x = radius_uv * texture_width + guard_px;
            let radius_y = radius_uv * texture_height + guard_px;
            let Some(dab_rect) = pixel_bbox_to_rect(
                texture_size,
                center_x - radius_x,
                center_y - radius_y,
                center_x + radius_x,
                center_y + radius_y,
            ) else {
                return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::InvalidInput);
            };
            push_merged_surface_target_mesh_uv_scissor_rect(&mut rects, dab_rect);
            contributed = true;
        }

        if uv_paint_boundary_distance <= radius_uv + guard_uv {
            let Some(mesh) = mesh else {
                return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
            };
            let Some(triangle_index) = dab.triangle_index else {
                return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::MissingData);
            };
            let Some(boundary_rects) = mesh.uv_boundary_rects_for_target_material(
                triangle_index,
                uv,
                material_index,
                radius_uv,
                texture_size,
                guard_px,
            ) else {
                return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::BoundaryLookup);
            };
            for rect in boundary_rects {
                push_merged_surface_target_mesh_uv_scissor_rect(&mut rects, rect);
                contributed = true;
            }
        } else if dab.material_index != material_index {
            return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::NoContribution);
        }

        if contributed {
            saw_dab = true;
        } else {
            return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::NoContribution);
        }
    }

    if !saw_dab {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::NoContribution);
    }
    if rects.is_empty() {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::NoContribution);
    }
    if rects.iter().any(|rect| rect.size == texture_size) {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::FullRect);
    }
    if rects.len() > SURFACE_TARGET_MESH_UV_SCISSOR_MAX_RECTS {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::RectLimit);
    }
    let full_area = texture_size[0].max(1) as f32 * texture_size[1].max(1) as f32;
    let rect_area: f32 = rects
        .iter()
        .map(|rect| rect.size[0] as f32 * rect.size[1] as f32)
        .sum();
    if rect_area / full_area > SURFACE_TARGET_MESH_UV_SCISSOR_MAX_AREA_RATIO {
        return SurfaceScissorResult::Full(SurfaceScissorFallbackReason::AreaLimit);
    }
    SurfaceScissorResult::Scissored(rects)
}

fn push_merged_surface_target_mesh_uv_scissor_rect(rects: &mut Vec<RectU32>, rect: RectU32) {
    rects.push(rect);
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                if rects_touch(rects[i], rects[j]) {
                    let merged = union_rect(rects[i], rects[j]);
                    rects[i] = merged;
                    rects.remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }
}

fn pixel_bbox_to_rect(
    texture_size: [u32; 2],
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> Option<RectU32> {
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return None;
    }
    let x0 = min_x.min(max_x).floor().clamp(0.0, texture_size[0] as f32) as u32;
    let y0 = min_y.min(max_y).floor().clamp(0.0, texture_size[1] as f32) as u32;
    let x1 = min_x.max(max_x).ceil().clamp(0.0, texture_size[0] as f32) as u32;
    let y1 = min_y.max(max_y).ceil().clamp(0.0, texture_size[1] as f32) as u32;
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    Some(RectU32 {
        origin: [x0, y0],
        size: [x1 - x0, y1 - y0],
    })
}

fn viewport_source_input_usage(
    surface_viewport_usage: &BrushSpaceResourceUsage,
    surface_target_usage: &BrushSpaceResourceUsage,
) -> BrushSpaceResourceUsage {
    BrushSpaceResourceUsage {
        needs_stroke_snapshot: surface_viewport_usage.needs_stroke_snapshot
            || surface_target_usage.needs_stroke_snapshot,
        needs_batch_source: surface_viewport_usage.needs_batch_source
            || surface_target_usage.needs_batch_source,
        needs_current_source: surface_viewport_usage.needs_current_source
            || surface_target_usage.needs_current_source,
        scratch_resources: Vec::new(),
        needs_viewport_source_color: surface_viewport_usage.needs_viewport_source_color
            || surface_target_usage.needs_viewport_source_color,
        needs_viewport_depth: surface_viewport_usage.needs_viewport_depth
            || surface_target_usage.needs_viewport_depth,
        reads_current_dab_batch: false,
        uses_target_mesh_uv: surface_viewport_usage.uses_target_mesh_uv
            || surface_target_usage.uses_target_mesh_uv,
        uses_uv_dab_quad_instances: false,
        uses_viewport_dab_quad_instances: false,
        reads_selection_mask: false,
    }
}

fn restore_cached_viewport_source_color(
    frame: &mut GpuFrame,
    txn: &mut BrushPassDeps<'_, '_>,
    projection_id: SurfaceProjectionId,
    viewport_size: [u32; 2],
) {
    let Some(cache_texture) = viewport_scratch_texture(
        txn.scratch,
        viewport_source_color_cache_name(projection_id),
        TextureResourceLifetime::Stroke,
    ) else {
        return;
    };
    txn.scene_capture.copy_texture_to_viewport_source_color(
        frame.encoder(),
        cache_texture,
        viewport_size,
    );
}

fn cache_viewport_source_color(
    frame: &mut GpuFrame,
    txn: &mut BrushPassDeps<'_, '_>,
    projection_id: SurfaceProjectionId,
    viewport_size: [u32; 2],
) {
    let cache_name = viewport_source_color_cache_name(projection_id);
    txn.scratch.ensure_transient_texture(
        txn.gpu.device(),
        frame.encoder(),
        viewport_scratch_key(cache_name, TextureResourceLifetime::Stroke),
        viewport_size,
        texture_format(TextureResourceFormat::Rgba8Unorm),
        "surface_viewport_source_color_cache_tex",
        "clear_surface_viewport_source_color_cache",
    );
    let Some(cache_texture) =
        viewport_scratch_texture(txn.scratch, cache_name, TextureResourceLifetime::Stroke)
    else {
        return;
    };
    txn.scene_capture.copy_viewport_source_color_to_texture(
        frame.encoder(),
        cache_texture,
        viewport_size,
    );
}

fn surface_viewport_source_snapshot_key(
    surface_ctx: &crate::core::stroke::SurfaceStrokeContext,
    projection: &crate::core::stroke::SurfaceProjectionContext,
    source_surfaces: &[PaintSurfaceId],
    material_count: usize,
) -> SurfaceViewportSourceSnapshotKey {
    let mut source_material_indices = source_surfaces
        .iter()
        .map(|surface| surface.material_index().as_usize())
        .filter(|material_index| *material_index < material_count)
        .collect::<Vec<_>>();
    source_material_indices.sort_unstable();
    source_material_indices.dedup();

    SurfaceViewportSourceSnapshotKey {
        viewport_size: surface_ctx.viewport_size,
        view_proj_bits: mat4_bits(projection.view_proj_matrix_gl),
        camera_world_bits: vec3_bits(projection.camera_world),
        source_material_indices,
        scene_visibility_revision: surface_ctx.scene_visibility.revision(),
    }
}

fn mat4_bits(matrix: glam::Mat4) -> [u32; 16] {
    let mut bits = [0; 16];
    for (dst, src) in bits.iter_mut().zip(matrix.to_cols_array()) {
        *dst = src.to_bits();
    }
    bits
}

fn vec3_bits(values: [f32; 3]) -> [u32; 3] {
    values.map(f32::to_bits)
}

fn contains_surface(surfaces: &[PaintSurfaceId], target: PaintSurfaceId) -> bool {
    surfaces.contains(&target)
}

fn surface_viewport_source_capture_rect(
    dabs: &[SurfaceDab],
    style: &RendererStrokeStyle,
    view_proj: glam::Mat4,
    viewport_size: [u32; 2],
) -> Option<RectU32> {
    let StrokeOp::BrushEngine {
        radius_world,
        param_dynamics,
        surface_source_material_scope,
        ..
    } = &style.stroke_op;
    let RendererStrokeOperation::BrushEngine { params, .. } = &style.operation;
    if !matches!(
        surface_source_material_scope,
        crate::core::brush_engine::SurfaceSourceMaterialScope::BrushFootprint { .. }
    ) {
        return None;
    }

    let mut min = glam::Vec2::splat(f32::INFINITY);
    let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
    for dab in dabs {
        let (center, radius_px) =
            projected_surface_dab_center_radius_px(view_proj, *dab, *radius_world, viewport_size)?;
        let extra_radius_px = surface_source_material_scope
            .extra_radius_px(radius_px, dab.pressure, params, param_dynamics)
            .unwrap_or(0.0)
            .max(0.0);
        let guarded_radius = radius_px + extra_radius_px + SURFACE_VIEWPORT_SOURCE_SCISSOR_GUARD_PX;
        min = min.min(center - glam::Vec2::splat(guarded_radius));
        max = max.max(center + glam::Vec2::splat(guarded_radius));
    }

    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let width = viewport_size[0].max(1);
    let height = viewport_size[1].max(1);
    let x0 = min.x.floor().max(0.0).min(width as f32) as u32;
    let y0 = min.y.floor().max(0.0).min(height as f32) as u32;
    let x1 = max.x.ceil().max(0.0).min(width as f32) as u32;
    let y1 = max.y.ceil().max(0.0).min(height as f32) as u32;
    (x1 > x0 && y1 > y0).then_some(RectU32 {
        origin: [x0, y0],
        size: [x1 - x0, y1 - y0],
    })
}

fn projected_surface_dab_center_radius_px(
    view_proj: glam::Mat4,
    dab: SurfaceDab,
    radius_world: f32,
    viewport_size: [u32; 2],
) -> Option<(glam::Vec2, f32)> {
    let center = world_to_viewport_px(view_proj, dab.world_pos, viewport_size)?;
    let radius_world = radius_world.max(0.0) * dab.radius_scale.max(0.0);
    if radius_world <= f32::EPSILON {
        return Some((center, 0.0));
    }
    let offsets = [
        dab.tangent_x * radius_world,
        -dab.tangent_x * radius_world,
        dab.tangent_y * radius_world,
        -dab.tangent_y * radius_world,
    ];
    let mut radius_px = 0.0f32;
    for offset in offsets {
        let edge = world_to_viewport_px(view_proj, dab.world_pos + offset, viewport_size)?;
        radius_px = radius_px.max((edge - center).length());
    }
    Some((center, radius_px))
}

fn world_to_viewport_px(
    view_proj: glam::Mat4,
    world: glam::Vec3,
    viewport_size: [u32; 2],
) -> Option<glam::Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w.abs() <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    Some(glam::Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0].max(1) as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1].max(1) as f32,
    ))
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3};

    use crate::{
        core::{
            brush_engine::{
                ParamValue, SurfaceSourceMaterialScope, SurfaceSourceRadiusTerm,
                TextureResourceExtent, TextureResourceFormat, TextureResourceLifetime,
            },
            document::{MeshData, MeshId, MeshObject, SubMesh},
            geometry::RectU32,
            render_report::RenderMetrics,
            stroke::SurfaceDab,
            stroke_preset::{PressureDynamics, StrokeOp},
        },
        renderer::{
            command::{RendererStrokeOperation, RendererStrokeStyle},
            features::brush::engine_pipelines::BrushScratchResource,
        },
    };

    use super::{
        BrushSpaceResourceUsage, SurfaceScissorFallbackReason, record_surface_scissor_fallback,
        record_surface_target_mesh_uv_pass_metrics, rgba8_size_bytes,
        surface_current_source_sync_clip, surface_target_mesh_uv_scissor_rects_for_dabs,
        surface_target_mesh_uv_scissor_result_for_dabs, surface_viewport_source_capture_rect,
        viewport_source_input_usage,
    };

    fn viewport_capture_style(scope: SurfaceSourceMaterialScope) -> RendererStrokeStyle {
        let params = vec![("sample_ratio".to_owned(), ParamValue::F32(1.0))];
        RendererStrokeStyle {
            stroke_op: StrokeOp::BrushEngine {
                engine_id: "test".to_owned(),
                radius_world: 0.1,
                radius_pressure: PressureDynamics::default(),
                params: params.clone(),
                param_dynamics: Vec::new(),
                surface_source_material_scope: scope,
            },
            operation: RendererStrokeOperation::BrushEngine {
                color: [1.0; 3],
                params,
            },
        }
    }

    #[test]
    fn surface_viewport_source_capture_rect_includes_declared_extra_radius() {
        let style = viewport_capture_style(SurfaceSourceMaterialScope::BrushFootprint {
            extra_radius: vec![SurfaceSourceRadiusTerm::BrushRadiusRatio {
                param: "sample_ratio".to_owned(),
                scale: 1.0,
            }],
        });
        let dab = SurfaceDab::with_scales(Vec2::new(50.0, 50.0), Vec3::ZERO, Vec3::Z, 0, 1.0, 1.0);

        let rect = surface_viewport_source_capture_rect(&[dab], &style, Mat4::IDENTITY, [100, 100])
            .expect("brush footprint capture should be scissored");

        assert_eq!(rect.origin, [38, 38]);
        assert_eq!(rect.size, [24, 24]);
    }

    #[test]
    fn surface_viewport_source_capture_rect_falls_back_for_all_materials() {
        let style = viewport_capture_style(SurfaceSourceMaterialScope::AllMaterials);
        let dab = SurfaceDab::with_scales(Vec2::new(50.0, 50.0), Vec3::ZERO, Vec3::Z, 0, 1.0, 1.0);

        assert_eq!(
            surface_viewport_source_capture_rect(&[dab], &style, Mat4::IDENTITY, [100, 100]),
            None
        );
    }

    #[test]
    fn surface_viewport_source_capture_rect_clamps_to_viewport_edge() {
        let style = viewport_capture_style(SurfaceSourceMaterialScope::BrushFootprint {
            extra_radius: vec![SurfaceSourceRadiusTerm::BrushRadiusRatio {
                param: "sample_ratio".to_owned(),
                scale: 1.0,
            }],
        });
        let dab =
            SurfaceDab::with_scales(Vec2::ZERO, Vec3::new(-1.0, 1.0, 0.0), Vec3::Z, 0, 1.0, 1.0);

        let rect = surface_viewport_source_capture_rect(&[dab], &style, Mat4::IDENTITY, [100, 100])
            .expect("edge dab should keep its visible capture area");

        assert_eq!(rect.origin, [0, 0]);
        // The upper edge uses a conservative ceil; projected radius arithmetic may
        // land fractionally above 12 px and must not clip a source sample.
        assert_eq!(rect.size, [13, 13]);
    }

    #[test]
    fn viewport_source_input_usage_does_not_inherit_target_scratch_resources() {
        let surface_viewport_usage = BrushSpaceResourceUsage {
            needs_viewport_source_color: true,
            needs_viewport_depth: true,
            ..BrushSpaceResourceUsage::default()
        };
        let surface_target_usage = BrushSpaceResourceUsage {
            needs_current_source: true,
            scratch_resources: vec![BrushScratchResource {
                name: "target_only".to_owned(),
                format: TextureResourceFormat::Rgba8Unorm,
                extent: TextureResourceExtent::PaintSurface,
                lifetime: TextureResourceLifetime::Pass,
            }],
            reads_current_dab_batch: true,
            reads_selection_mask: true,
            ..BrushSpaceResourceUsage::default()
        };

        let usage = viewport_source_input_usage(&surface_viewport_usage, &surface_target_usage);

        assert!(usage.needs_viewport_source_color);
        assert!(usage.needs_viewport_depth);
        assert!(usage.needs_current_source);
        assert!(usage.scratch_resources.is_empty());
        assert!(!usage.reads_current_dab_batch);
        assert!(!usage.reads_selection_mask);
    }

    #[test]
    fn surface_current_source_sync_clip_respects_can_clip_flag() {
        let rect = RectU32 {
            origin: [128, 256],
            size: [64, 96],
        };

        let rects = [rect];
        assert_eq!(
            surface_current_source_sync_clip(true, Some(&rects[..])),
            Some(&rects[..])
        );
        assert_eq!(
            surface_current_source_sync_clip(false, Some(&rects[..])),
            None
        );
        assert_eq!(surface_current_source_sync_clip(true, None), None);
    }

    #[test]
    fn brush_usage_requires_scene_mesh_only_for_mesh_backed_inputs() {
        assert!(!BrushSpaceResourceUsage::default().requires_scene_mesh());

        assert!(
            BrushSpaceResourceUsage {
                uses_target_mesh_uv: true,
                ..BrushSpaceResourceUsage::default()
            }
            .requires_scene_mesh()
        );

        assert!(
            BrushSpaceResourceUsage {
                needs_viewport_depth: true,
                ..BrushSpaceResourceUsage::default()
            }
            .requires_scene_mesh()
        );

        assert!(
            BrushSpaceResourceUsage {
                needs_viewport_source_color: true,
                ..BrushSpaceResourceUsage::default()
            }
            .requires_scene_mesh()
        );

        assert!(
            !BrushSpaceResourceUsage {
                uses_uv_dab_quad_instances: true,
                reads_current_dab_batch: true,
                ..BrushSpaceResourceUsage::default()
            }
            .requires_scene_mesh()
        );
    }

    fn scissor_target_materials(dabs: &[SurfaceDab], material_index: usize) -> Vec<Vec<usize>> {
        vec![vec![material_index]; dabs.len()]
    }

    fn scissor_test_dab(
        material_index: usize,
        uv: Vec2,
        uv_paint_boundary_distance: f32,
        radius_scale: f32,
    ) -> SurfaceDab {
        let mut dab = SurfaceDab::with_scales(
            Vec2::ZERO,
            Vec3::ZERO,
            Vec3::Z,
            material_index,
            1.0,
            radius_scale,
        );
        dab.uv = Some(uv);
        dab.uv_edge_distance = Some(uv_paint_boundary_distance);
        dab.uv_paint_boundary_distance = Some(uv_paint_boundary_distance);
        dab
    }

    fn scissor_boundary_dab(material_index: usize, uv: Vec2, triangle_index: usize) -> SurfaceDab {
        let mut dab = scissor_test_dab(material_index, uv, 0.01, 1.0);
        dab.triangle_index = Some(triangle_index);
        dab.mesh_id = Some(MeshId(0));
        dab
    }

    fn scissor_material_boundary_mesh() -> MeshData {
        MeshData::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
            vec![Vec3::Z; 4],
            vec![[0, 1, 2], [0, 2, 3]],
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 3,
                    material_index: 0,
                    material_name: "M0".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 3,
                    index_count: 3,
                    material_index: 1,
                    material_name: "M1".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0), MeshId(0)],
        )
        .unwrap()
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_bounds_centered_dab() {
        let dabs = [scissor_test_dab(2, Vec2::new(0.5, 0.5), 1.0, 1.0)];
        let target_materials = scissor_target_materials(&dabs, 2);

        let rects = surface_target_mesh_uv_scissor_rects_for_dabs(
            None,
            [1024, 1024],
            2,
            &dabs,
            &target_materials,
            0.01,
            1.0,
        )
        .expect("centered dab should produce a bounded scissor rect");

        assert_eq!(rects.len(), 1);
        let rect = rects[0];
        assert!(rect.origin[0] > 0);
        assert!(rect.origin[1] > 0);
        assert!(rect.size[0] < 1024);
        assert!(rect.size[1] < 1024);
    }

    #[test]
    fn surface_target_mesh_uv_scissor_success_records_workload() {
        let rects = [RectU32 {
            origin: [10, 20],
            size: [30, 40],
        }];
        let mut metrics = RenderMetrics::default();

        record_surface_target_mesh_uv_pass_metrics(&mut metrics, [2048, 2048], Some(&rects));

        assert_eq!(metrics.surface_target_mesh_uv_pass_count, 1);
        assert_eq!(metrics.surface_target_mesh_uv_scissored_pass_count, 1);
        assert_eq!(metrics.surface_target_mesh_uv_full_pass_count, 0);
        assert_eq!(metrics.surface_target_mesh_uv_scissor_rect_count, 1);
        assert_eq!(metrics.surface_target_mesh_uv_scissor_pixel_area, 1_200);
    }

    #[test]
    fn surface_target_mesh_uv_fallback_records_exact_reason_and_full_workload() {
        let dabs = [scissor_test_dab(0, Vec2::new(0.5, 0.5), 1.0, 1.0)];
        let result = surface_target_mesh_uv_scissor_result_for_dabs(
            None,
            [2048, 2048],
            0,
            &dabs,
            &[],
            0.01,
            1.0,
        );
        let super::SurfaceScissorResult::Full(reason) = result else {
            panic!("mismatched dab targeting must use a full pass");
        };
        let mut metrics = RenderMetrics::default();
        record_surface_scissor_fallback(&mut metrics, reason);
        record_surface_target_mesh_uv_pass_metrics(&mut metrics, [2048, 2048], None);

        assert_eq!(reason, SurfaceScissorFallbackReason::MissingData);
        assert_eq!(metrics.surface_scissor_fallback_missing_data, 1);
        assert_eq!(metrics.surface_target_mesh_uv_full_pass_count, 1);
        assert_eq!(metrics.surface_target_mesh_uv_full_pixel_area, 2048 * 2048);
    }

    #[test]
    fn surface_snapshot_copy_bytes_scale_with_full_texture_area() {
        assert_eq!(rgba8_size_bytes([1024, 1024]), 4 * 1024 * 1024);
        assert_eq!(rgba8_size_bytes([2048, 2048]), 16 * 1024 * 1024);
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_allows_without_paint_boundary() {
        let dabs = [scissor_test_dab(2, Vec2::new(0.5, 0.5), f32::INFINITY, 1.0)];
        let target_materials = scissor_target_materials(&dabs, 2);

        let rect = surface_target_mesh_uv_scissor_rects_for_dabs(
            None,
            [1024, 1024],
            2,
            &dabs,
            &target_materials,
            0.01,
            1.0,
        );

        assert!(rect.is_some());
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_disables_for_cross_material_dabs() {
        let dabs = [
            scissor_test_dab(2, Vec2::new(0.5, 0.5), 1.0, 1.0),
            scissor_test_dab(3, Vec2::new(0.5, 0.5), 1.0, 1.0),
        ];
        let target_materials = scissor_target_materials(&dabs, 2);

        assert!(
            surface_target_mesh_uv_scissor_rects_for_dabs(
                None,
                [1024, 1024],
                2,
                &dabs,
                &target_materials,
                0.01,
                1.0,
            )
            .is_none()
        );
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_ignores_irrelevant_mixed_material_dabs() {
        let dabs = [
            scissor_test_dab(2, Vec2::new(0.5, 0.5), 1.0, 1.0),
            scissor_test_dab(3, Vec2::new(0.1, 0.1), 1.0, 1.0),
        ];
        let target_materials = vec![vec![2], vec![3]];

        let rect = surface_target_mesh_uv_scissor_rects_for_dabs(
            None,
            [1024, 1024],
            2,
            &dabs,
            &target_materials,
            0.01,
            1.0,
        );

        assert!(rect.is_some());
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_disables_near_paint_boundary() {
        let dabs = [scissor_test_dab(2, Vec2::new(0.5, 0.5), 0.02, 1.0)];
        let target_materials = scissor_target_materials(&dabs, 2);

        assert!(
            surface_target_mesh_uv_scissor_rects_for_dabs(
                None,
                [1024, 1024],
                2,
                &dabs,
                &target_materials,
                0.01,
                1.0,
            )
            .is_none()
        );
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_keeps_separated_dabs_as_multiple_rects() {
        let dabs = [
            scissor_test_dab(2, Vec2::new(0.1, 0.1), 1.0, 1.0),
            scissor_test_dab(2, Vec2::new(0.9, 0.9), 1.0, 1.0),
        ];
        let target_materials = scissor_target_materials(&dabs, 2);

        let rects = surface_target_mesh_uv_scissor_rects_for_dabs(
            None,
            [1024, 1024],
            2,
            &dabs,
            &target_materials,
            0.01,
            1.0,
        )
        .expect("separated dabs should stay clipped");

        assert_eq!(rects.len(), 2);
        assert!(
            rects
                .iter()
                .all(|rect| rect.size[0] < 256 && rect.size[1] < 256)
        );
    }

    #[test]
    fn surface_target_mesh_uv_scissor_rect_adds_neighbor_rect_at_material_boundary() {
        let mesh = scissor_material_boundary_mesh();
        let dabs = [scissor_boundary_dab(0, Vec2::new(0.50, 0.49), 0)];
        let target_materials = vec![vec![0, 1]];

        let rects = surface_target_mesh_uv_scissor_rects_for_dabs(
            Some(&mesh),
            [1024, 1024],
            1,
            &dabs,
            &target_materials,
            0.01,
            1.0,
        )
        .expect("boundary-crossing dab should keep target material clipped");

        assert!(!rects.is_empty());
        assert!(rects.iter().all(|rect| rect.size != [1024, 1024]));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveSelection {
    Full,
    Empty,
    GpuMask(SelectionMaskId),
}

impl EffectiveSelection {
    fn from_active_selection(active_selection: &ActiveSelection, material_index: usize) -> Self {
        if active_selection.is_effectively_full() {
            return Self::Full;
        }
        if let Some(mask_id) = active_selection
            .masks
            .iter()
            .find(|mask| mask.material_index.as_usize() == material_index)
            .and_then(|mask| mask.mask_id)
        {
            Self::GpuMask(mask_id)
        } else {
            Self::Empty
        }
    }
}

fn texture_entry<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn buffer_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn scratch_texture_clears_every_pass(
    passes: &[BrushPassRuntime],
    resources: &[CompiledBrushResource],
) -> bool {
    passes.iter().any(|pass| {
        pass.outputs.iter().any(|output| {
            output.load == PassOutputLoad::ClearEveryPass
                && matches!(
                    resource_definition(resources, output.resource),
                    BrushResourceDefinition::Texture { .. }
                )
        })
    })
}

fn load_op(
    load: PassOutputLoad,
    clear_value: Option<&PassClearValue>,
) -> wgpu::LoadOp<wgpu::Color> {
    match load {
        PassOutputLoad::ClearEveryPass => wgpu::LoadOp::Clear(clear_color(clear_value)),
        PassOutputLoad::Load | PassOutputLoad::ClearOnStrokeBegin | PassOutputLoad::DontCare => {
            wgpu::LoadOp::Load
        }
    }
}

fn clear_color(clear_value: Option<&PassClearValue>) -> wgpu::Color {
    match clear_value {
        Some(PassClearValue::F32(value)) => wgpu::Color {
            r: f64::from(*value),
            g: 0.0,
            b: 0.0,
            a: 0.0,
        },
        Some(PassClearValue::Rgba((r, g, b, a))) => wgpu::Color {
            r: f64::from(*r),
            g: f64::from(*g),
            b: f64::from(*b),
            a: f64::from(*a),
        },
        None => wgpu::Color::TRANSPARENT,
    }
}

fn texture_format(format: TextureResourceFormat) -> wgpu::TextureFormat {
    match format {
        TextureResourceFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureResourceFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        TextureResourceFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
    }
}

#[derive(Debug, Clone)]
struct StrokeBeginClear {
    resource: CompiledResourceRef,
    clear_value: Option<PassClearValue>,
}

fn stroke_begin_clears(passes: &[BrushPassRuntime]) -> Vec<StrokeBeginClear> {
    passes
        .iter()
        .flat_map(|pass| pass.outputs.iter())
        .filter(|output| output.load == PassOutputLoad::ClearOnStrokeBegin)
        .map(|output| StrokeBeginClear {
            resource: output.resource,
            clear_value: output.clear_value.clone(),
        })
        .collect()
}

fn clear_stroke_begin_outputs(
    frame: &mut GpuFrame,
    txn: &BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    resources: &[CompiledBrushResource],
    clears: &[StrokeBeginClear],
    stroke_view: Option<&wgpu::TextureView>,
    record_surface_metrics: bool,
) -> usize {
    let mut cleared_bytes = 0usize;
    for clear in clears {
        let Some(view) =
            stroke_begin_clear_view(txn, target, resources, clear.resource, stroke_view)
        else {
            continue;
        };
        if record_surface_metrics
            && let BrushResourceDefinition::Texture {
                format,
                extent: TextureResourceExtent::PaintSurface,
                ..
            } = resource_definition(resources, clear.resource)
            && let Some(size) = txn.surfaces.surface_texture_size(target)
        {
            let bytes_per_pixel = match format {
                TextureResourceFormat::R8Unorm => 1,
                TextureResourceFormat::Rgba8Unorm => 4,
                TextureResourceFormat::Rgba16Float => 8,
            };
            cleared_bytes =
                cleared_bytes.saturating_add(pixel_area(size).saturating_mul(bytes_per_pixel));
        }
        clear_rgba_target(
            frame.encoder(),
            view,
            clear_value_rgba(clear.clear_value.as_ref()),
            "clear_brush_stroke_begin_output",
        );
    }
    cleared_bytes
}

fn stroke_begin_clear_view<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    resources: &'a [CompiledBrushResource],
    resource: CompiledResourceRef,
    _stroke_view: Option<&'a wgpu::TextureView>,
) -> Option<&'a wgpu::TextureView> {
    let name = resource_name(resources, resource);
    match resource_definition(resources, resource) {
        BrushResourceDefinition::Texture {
            extent: TextureResourceExtent::PaintSurface,
            lifetime,
            ..
        } => material_scratch_view(txn.scratch, target, name, *lifetime),
        BrushResourceDefinition::Texture {
            extent: TextureResourceExtent::Viewport,
            lifetime,
            ..
        } => viewport_scratch_view(txn.scratch, name, *lifetime),
        _ => None,
    }
}

fn clear_value_rgba(clear_value: Option<&PassClearValue>) -> [f32; 4] {
    match clear_value {
        Some(PassClearValue::F32(value)) => [*value, 0.0, 0.0, 0.0],
        Some(PassClearValue::Rgba(value)) => [value.0, value.1, value.2, value.3],
        None => [0.0, 0.0, 0.0, 0.0],
    }
}

fn selection_mask_view<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    material_index: usize,
    active_selection: &ActiveSelection,
) -> &'a wgpu::TextureView {
    match EffectiveSelection::from_active_selection(active_selection, material_index) {
        EffectiveSelection::Full => &txn.textures.solid_white.view,
        EffectiveSelection::Empty => &txn.textures.solid_black.view,
        EffectiveSelection::GpuMask(mask_id) => txn
            .selections
            .view(mask_id)
            .unwrap_or(&txn.textures.solid_black.view),
    }
}

fn sampler_for_kind<'a>(txn: &'a BrushPassDeps<'_, '_>, sampler: SamplerKind) -> &'a wgpu::Sampler {
    match sampler {
        SamplerKind::LinearClamp => txn.gpu.paint_sampler(),
        SamplerKind::NearestClamp => &txn.textures.nearest_clamp_sampler,
        SamplerKind::LinearRepeat => &txn.textures.linear_repeat_sampler,
        SamplerKind::NearestRepeat => &txn.textures.nearest_repeat_sampler,
    }
}

fn bind_group_entry_for_input<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    resources: &'a [CompiledBrushResource],
    input: &'a PassInputRuntime,
    current_source_view: &'a wgpu::TextureView,
    canvas_view: &'a wgpu::TextureView,
    stroke_view: Option<&'a wgpu::TextureView>,
    style: &'a RendererStrokeStyle,
    active_selection: &'a ActiveSelection,
    resource_params: &'a [BrushResourceParamField],
    depth_slot: SceneDepthSlot,
) -> Result<wgpu::BindGroupEntry<'a>> {
    match &input.binding {
        CompiledPassBinding::SampledTexture { resource, .. } => {
            let view = input_texture_view(
                txn,
                target,
                resources,
                *resource,
                current_source_view,
                canvas_view,
                stroke_view,
                style,
                resource_params,
                depth_slot,
            )?;
            Ok(texture_entry(input.binding_index, view))
        }
        CompiledPassBinding::Builtin(BuiltinPassInput::CurrentDabBatch) => Ok(buffer_entry(
            input.binding_index,
            txn.stroke_resources.current_dab_batch(),
        )),

        CompiledPassBinding::Builtin(BuiltinPassInput::SelectionMask) => Ok(texture_entry(
            input.binding_index,
            selection_mask_view(txn, target.material_index().as_usize(), active_selection),
        )),
        CompiledPassBinding::Builtin(BuiltinPassInput::StrokeDabs) => {
            bail!("StrokeDabs is not supported")
        }
        CompiledPassBinding::Sampler(sampler) => Ok(wgpu::BindGroupEntry {
            binding: input.binding_index,
            resource: wgpu::BindingResource::Sampler(sampler_for_kind(txn, *sampler)),
        }),
        CompiledPassBinding::Uniform(UniformKind::BrushParams) => Ok(wgpu::BindGroupEntry {
            binding: input.binding_index,
            resource: txn.pipelines.brush_uniform.as_entire_binding(),
        }),
        CompiledPassBinding::Uniform(UniformKind::ViewProj) => Ok(wgpu::BindGroupEntry {
            binding: input.binding_index,
            resource: txn.scene.view_proj_uniform().as_entire_binding(),
        }),
        CompiledPassBinding::Uniform(UniformKind::BakeParams) => Ok(wgpu::BindGroupEntry {
            binding: input.binding_index,
            resource: txn.pipelines.bake_uniform.as_entire_binding(),
        }),
    }
}

fn input_texture_view<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    resources: &'a [CompiledBrushResource],
    resource: CompiledResourceRef,
    current_source_view: &'a wgpu::TextureView,
    canvas_view: &'a wgpu::TextureView,
    _stroke_view: Option<&'a wgpu::TextureView>,
    style: &'a RendererStrokeStyle,
    resource_params: &'a [BrushResourceParamField],
    depth_slot: SceneDepthSlot,
) -> Result<&'a wgpu::TextureView> {
    match resource_definition(resources, resource) {
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync: SurfaceTextureSync::StrokeBeginSnapshot,
            ..
        } => stroke_transient::material_source_uv_view(
            txn.scratch,
            target.material_index().as_usize(),
        )
        .ok_or_else(|| anyhow::anyhow!("StrokeBeginSnapshot requested but not allocated")),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync: SurfaceTextureSync::BeforeEachBatch,
            ..
        } => Ok(stroke_transient::material_batch_source_uv_view(
            txn.scratch,
            target.material_index().as_usize(),
        )
        .unwrap_or(current_source_view)),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync: SurfaceTextureSync::BeforeEachPass,
            ..
        } => Ok(current_source_view),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync: SurfaceTextureSync::Live,
            ..
        } => Ok(canvas_view),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync,
            ..
        } => bail!(
            "canvas surface texture input uses unsupported sync {:?}",
            sync
        ),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::ViewportColor,
            ..
        } => Ok(txn.scene_capture.viewport_source_color_view()),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::ViewportDepth,
            ..
        } => Ok(txn.scene_capture.depth_color_view(depth_slot)),
        BrushResourceDefinition::Texture {
            extent, lifetime, ..
        } => {
            let name = resource_name(resources, resource);
            match extent {
                TextureResourceExtent::PaintSurface => {
                    material_scratch_view(txn.scratch, target, name, *lifetime).ok_or_else(|| {
                        anyhow::anyhow!(
                            "brush paint-surface scratch resource {:?} requested but not allocated",
                            name
                        )
                    })
                }
                TextureResourceExtent::Viewport => {
                    viewport_scratch_view(txn.scratch, name, *lifetime).ok_or_else(|| {
                        anyhow::anyhow!(
                            "brush viewport scratch resource {:?} requested but not allocated",
                            name
                        )
                    })
                }
                _ => bail!("brush texture resource {:?} uses unsupported extent", name),
            }
        }
        BrushResourceDefinition::TextureAsset { source, .. } => {
            texture_asset_view(txn, style, resource_params, source)
        }
    }
}

fn output_texture_view<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    resources: &'a [CompiledBrushResource],
    resource: CompiledResourceRef,
    canvas_view: &'a wgpu::TextureView,
    _stroke_view: Option<&'a wgpu::TextureView>,
) -> Result<&'a wgpu::TextureView> {
    match resource_definition(resources, resource) {
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync: SurfaceTextureSync::Live,
            ..
        } => Ok(canvas_view),
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas,
            sync,
            ..
        } => bail!(
            "canvas surface texture output uses unsupported sync {:?}",
            sync
        ),
        BrushResourceDefinition::Texture {
            extent, lifetime, ..
        } => {
            let name = resource_name(resources, resource);
            match extent {
                TextureResourceExtent::PaintSurface => {
                    material_scratch_view(txn.scratch, target, name, *lifetime).ok_or_else(|| {
                        anyhow::anyhow!(
                            "brush paint-surface scratch resource {:?} requested but not allocated",
                            name
                        )
                    })
                }
                TextureResourceExtent::Viewport => {
                    viewport_scratch_view(txn.scratch, name, *lifetime).ok_or_else(|| {
                        anyhow::anyhow!(
                            "brush viewport scratch resource {:?} requested but not allocated",
                            name
                        )
                    })
                }
                _ => bail!("brush texture resource {:?} uses unsupported extent", name),
            }
        }
        _ => bail!(
            "resource {:?} cannot be used as a brush render attachment",
            resource_name(resources, resource)
        ),
    }
}

fn brush_engine_id(style: &RendererStrokeStyle) -> Result<&str> {
    let StrokeOp::BrushEngine { engine_id, .. } = &style.stroke_op;
    Ok(engine_id)
}

fn brush_radius_world(style: &RendererStrokeStyle) -> Option<f32> {
    match &style.stroke_op {
        StrokeOp::BrushEngine { radius_world, .. } => Some(*radius_world),
    }
}

fn resource_ref_param_value_by_name(
    style: &RendererStrokeStyle,
    resource_params: &[BrushResourceParamField],
    param_name: &str,
) -> Result<ResourceRefValue> {
    let RendererStrokeOperation::BrushEngine { params, .. } = &style.operation;
    let Some(field) = resource_params
        .iter()
        .find(|field| field.name == param_name)
    else {
        bail!("missing texture ResourceRef param {:?}", param_name);
    };
    let value = params
        .iter()
        .find_map(|(candidate, value)| (candidate == &field.name).then_some(value))
        .unwrap_or(&field.default);
    match value {
        ParamValue::ResourceRef(value) => Ok(value.clone()),
        _ => bail!("resource param {:?} is not a ResourceRef", field.name),
    }
}

fn texture_asset_view<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
    resource_params: &[BrushResourceParamField],
    source: &TextureAssetSource,
) -> Result<&'a wgpu::TextureView> {
    let id = match source {
        TextureAssetSource::Static { id } => id.as_str(),
        TextureAssetSource::RefParam { param } => {
            match resource_ref_param_value_by_name(style, resource_params, param)? {
                ResourceRefValue::Resource(id) => return Ok(&txn.textures.resolve(&id)?.view),
                ResourceRefValue::None => bail!("texture ResourceRef param {:?} is None", param),
            }
        }
    };
    Ok(&txn.textures.resolve(id)?.view)
}
fn uv_stroke_begin_runtime_data(
    txn: &BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
) -> Result<(
    BrushSpaceResourceUsage,
    Vec<CompiledBrushResource>,
    Vec<StrokeBeginClear>,
)> {
    let runtime = runtime_for_style(txn, style)?;
    Ok((
        runtime.uv_usage().clone(),
        runtime.resources().to_vec(),
        stroke_begin_clears(runtime.uv_passes()),
    ))
}

fn surface_stroke_begin_runtime_data(
    txn: &BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
) -> Result<(
    BrushSpaceResourceUsage,
    Vec<CompiledBrushResource>,
    Vec<StrokeBeginClear>,
)> {
    let runtime = runtime_for_style(txn, style)?;
    Ok((
        runtime.surface_usage().clone(),
        runtime.resources().to_vec(),
        stroke_begin_clears(runtime.surface_passes()),
    ))
}

fn surface_stamp_runtime_data(
    txn: &BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
) -> Result<(BrushSpaceResourceUsage, BrushSpaceResourceUsage, bool, bool)> {
    let runtime = runtime_for_style(txn, style)?;
    Ok((
        runtime.surface_viewport_usage().clone(),
        runtime.surface_usage().clone(),
        !runtime.surface_viewport_passes().is_empty(),
        surface_current_source_readers_use_target_mesh_uv(runtime.surface_passes()),
    ))
}

fn runtime_usage(
    txn: &BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
    surface: bool,
) -> Result<BrushSpaceResourceUsage> {
    let runtime = runtime_for_style(txn, style)?;
    Ok((if surface {
        runtime.surface_usage()
    } else {
        runtime.uv_usage()
    })
    .clone())
}

fn runtime_for_style<'a>(
    txn: &'a BrushPassDeps<'_, '_>,
    style: &RendererStrokeStyle,
) -> Result<&'a crate::renderer::features::brush::engine_pipelines::BrushEngineRuntime> {
    let engine_id = brush_engine_id(style)?;
    txn.pipelines
        .runtime(engine_id)
        .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))
}

fn brush_uniform_for_pass_set(
    txn: &BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    style: &RendererStrokeStyle,
    _runtime: &crate::renderer::features::brush::engine_pipelines::BrushEngineRuntime,
    base: &BrushEngineUniform,
    surface: bool,
) -> BrushEngineUniform {
    let mut uniform = *base;
    if let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) {
        uniform.base_radius[2] = layer.texture_size[0].max(1) as f32;
        uniform.base_radius[3] = layer.texture_size[1].max(1) as f32;
    }
    if !surface {
        if let Some(radius_px) = brush_uv_brush_base_radius_px(txn, target, style) {
            uniform.base_radius[1] = radius_px;
        }
    }
    uniform
}

fn brush_uv_brush_base_radius_px(
    txn: &BrushPassDeps<'_, '_>,
    target: PaintSurfaceId,
    style: &RendererStrokeStyle,
) -> Option<f32> {
    let radius_world = brush_radius_world(style)?;
    let layer = txn.surfaces.stroke_surface_target(target, txn.scratch)?;
    let uv_linear_scale = txn
        .scene
        .mesh()
        .map(|mesh| mesh.uv_linear_scale)
        .unwrap_or(1.0)
        .max(0.0);
    let tex_scale =
        ((layer.texture_size[0].max(1) as f32) * (layer.texture_size[1].max(1) as f32)).sqrt();
    Some(radius_world.max(0.0) * uv_linear_scale * tex_scale)
}
