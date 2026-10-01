use anyhow::Result;

use crate::{
    core::{
        damage::DamageMap,
        selection::ActiveSelection,
        stroke::{StrokeContext, StrokeDab, SurfaceDab},
        stroke_preset::StrokeOp,
        surface::PaintSurfaceId,
    },
    renderer::{
        command::RendererStrokeStyle, features::brush::types::BrushDabInstance,
        gpu::frame::GpuFrame,
    },
};

use super::{
    encoder::BrushPassEncoder, engine_types::BrushEngineUniform,
    params::resolve_brush_engine_uniform, pass_deps::BrushPassDeps,
    viewport_cache::SurfaceViewportRuntimeCache,
};

#[derive(Default)]
pub(crate) struct BrushPassExecutor {
    encoder: BrushPassEncoder,
}

impl BrushPassExecutor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn begin_uv_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        style: &RendererStrokeStyle,
    ) {
        self.encoder.begin_uv_stroke(frame, txn, target, style);
    }

    pub(crate) fn resolve_brush_uniform(
        &self,
        txn: &BrushPassDeps<'_, '_>,
        style: &RendererStrokeStyle,
    ) -> Result<BrushEngineUniform> {
        let StrokeOp::BrushEngine { engine_id, .. } = &style.stroke_op;
        let runtime = txn
            .pipelines
            .runtime(engine_id)
            .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))?;
        Ok(resolve_brush_engine_uniform(style, runtime.param_layout()))
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
        self.encoder.stamp_uv_batch(
            frame,
            txn,
            target,
            dabs,
            instances,
            style,
            brush_uniform,
            active_selection,
        )
    }

    pub(crate) fn finalize_uv_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
    ) {
        self.encoder.finalize_uv_stroke(frame, txn, target);
    }

    pub(crate) fn cancel_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
    ) {
        self.encoder.cancel_stroke(frame, txn, surfaces);
    }

    pub(crate) fn begin_surface_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
    ) {
        self.encoder
            .begin_surface_stroke(frame, txn, surfaces, style);
    }

    pub(crate) fn surface_stamp_needs_viewport_instances(
        &self,
        txn: &BrushPassDeps<'_, '_>,
        style: &RendererStrokeStyle,
    ) -> Result<bool> {
        let StrokeOp::BrushEngine { engine_id, .. } = &style.stroke_op;
        let runtime = txn
            .pipelines
            .runtime(engine_id)
            .ok_or_else(|| anyhow::anyhow!("brush engine {:?} is not loaded", engine_id))?;
        Ok(runtime
            .surface_viewport_usage()
            .uses_viewport_dab_quad_instances)
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
        self.encoder.stamp_surface_batch(
            frame,
            txn,
            source_surfaces,
            target_surfaces,
            dabs,
            surface_directions,
            target_materials_by_dab,
            viewport_instances,
            projection_id,
            style,
            brush_uniform,
            ctx,
            active_selection,
            viewport_runtime_cache,
        )
    }

    pub(crate) fn prepare_surface_logical_batch_sources(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target_surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
    ) -> Result<()> {
        self.encoder
            .prepare_surface_logical_batch_sources(frame, txn, target_surfaces, style)
    }

    pub(crate) fn finalize_surface_stroke(
        &self,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &[PaintSurfaceId],
        style: &RendererStrokeStyle,
        damage: Option<&DamageMap>,
    ) {
        self.encoder
            .finalize_surface_stroke(frame, txn, surfaces, style, damage);
    }
}
