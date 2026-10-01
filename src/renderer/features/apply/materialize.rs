use crate::{
    core::{
        mask::{GeometryMaskSource, MaskSource, ProjectionMaskSource, ShapeMaskSource},
        surface::PaintSurfaceId,
    },
    renderer::{
        features::{
            apply::{
                deps::PaintApplyDeps,
                masked::{OneShotMaskedApplyRequest, operation_kind},
            },
            brush::transient as stroke_transient,
        },
        gpu::{clear_rgba_target, frame::GpuFrame},
        mask::support::{MaskConsumer, UnsupportedMaskError, ensure_supported},
        mask::{
            geometry::mesh_geometry_mask_rgba8,
            projection_gpu::{
                ViewportProjectionMaskGpuDeps, write_viewport_projection_mask_to_stroke_uv,
            },
            shape::{polygon_mask_rect_r8, rectangle_mask_r8},
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreparedMask {
    pub(crate) target: PaintSurfaceId,
    pub(crate) material_index: usize,
    pub(crate) texture_size: [u32; 2],
}

pub(crate) fn prepare_mask_texture_for_request(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    request: &OneShotMaskedApplyRequest,
) -> anyhow::Result<Option<PreparedMask>> {
    ensure_supported(
        MaskConsumer::Paint,
        &request.mask,
        Some(operation_kind(&request.operation)),
    )?;
    let target = request.target;
    let material_index = target.material_index().as_usize();
    let Some(texture_size) = txn.surfaces.surface_texture_size(target) else {
        return Ok(None);
    };
    ensure_apply_scratch_textures(frame, txn, material_index, texture_size);

    match &request.mask {
        MaskSource::Full(_) => {
            if write_full_mask(frame, txn, material_index).is_none() {
                return Ok(None);
            }
        }
        MaskSource::Shape(ShapeMaskSource::Rectangle(rect)) => {
            let r8 = rectangle_mask_r8(rect, texture_size);
            if write_r8_mask(frame, txn, material_index, texture_size, &r8).is_none() {
                return Ok(None);
            }
        }
        MaskSource::Shape(ShapeMaskSource::Polygon(polygon)) => {
            let Some((rect, r8)) = polygon_mask_rect_r8(polygon, texture_size) else {
                return Ok(None);
            };
            if write_r8_mask_rect(frame, txn, material_index, rect, &r8).is_none() {
                return Ok(None);
            }
        }
        MaskSource::Geometry(GeometryMaskSource::Mesh(mesh)) => {
            let rgba8 = mesh_geometry_mask_rgba8(mesh, material_index, texture_size);
            let r8 = rgba8_mask_to_r8(&rgba8);
            if write_r8_mask(frame, txn, material_index, texture_size, &r8).is_none() {
                return Ok(None);
            }
        }
        MaskSource::Projection(projection) => {
            if write_viewport_projection_mask(frame, txn, target, projection, texture_size)
                .is_none()
            {
                return Ok(None);
            }
        }
        MaskSource::Brush(_) | MaskSource::Flood(_) | MaskSource::ExistingSelection => {
            return Err(UnsupportedMaskError::new(
                MaskConsumer::Paint,
                &request.mask,
                Some(operation_kind(&request.operation)),
            )
            .into());
        }
    }

    Ok(Some(PreparedMask {
        target,
        material_index,
        texture_size,
    }))
}

fn ensure_apply_scratch_textures(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    material_index: usize,
    texture_size: [u32; 2],
) {
    stroke_transient::ensure_material_source_uv(
        txn.scratch,
        txn.gpu.device(),
        frame.encoder(),
        material_index,
        texture_size,
    );
    stroke_transient::ensure_material_stroke_uv(
        txn.scratch,
        txn.gpu.device(),
        frame.encoder(),
        material_index,
        texture_size,
    );
}

fn write_full_mask(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    material_index: usize,
) -> Option<()> {
    let stroke_view = stroke_transient::material_stroke_uv_view(txn.scratch, material_index)?;
    clear_rgba_target(
        frame.encoder(),
        stroke_view,
        [1.0, 1.0, 1.0, 1.0],
        "clear_full_mask_one_shot",
    );
    Some(())
}

fn write_r8_mask(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    material_index: usize,
    texture_size: [u32; 2],
    r8: &[u8],
) -> Option<()> {
    let stroke_texture = stroke_transient::material_stroke_uv_texture(txn.scratch, material_index)?;
    frame.write_texture_r8(txn.gpu.device(), stroke_texture, [0, 0], texture_size, r8);
    Some(())
}

fn write_r8_mask_rect(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    material_index: usize,
    rect: crate::core::geometry::RectU32,
    r8: &[u8],
) -> Option<()> {
    let stroke_texture = stroke_transient::material_stroke_uv_texture(txn.scratch, material_index)?;
    let stroke_view = stroke_transient::material_stroke_uv_view(txn.scratch, material_index)?;
    clear_rgba_target(
        frame.encoder(),
        stroke_view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_polygon_mask_one_shot",
    );
    frame.write_texture_r8(txn.gpu.device(), stroke_texture, rect.origin, rect.size, r8);
    Some(())
}

fn write_viewport_projection_mask(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    target: PaintSurfaceId,
    projection: &ProjectionMaskSource,
    texture_size: [u32; 2],
) -> Option<()> {
    let material_index = target.material_index().as_usize();
    let polygon_coverage = txn.viewport_polygon_coverage.as_ref();
    let mut projection_deps = ViewportProjectionMaskGpuDeps {
        gpu: txn.gpu,
        pipelines: txn.pipelines,
        scene: txn.scene,
        scratch: txn.scratch,
        scene_capture: txn.scene_capture,
        scene_capture_pipelines: txn.scene_capture_pipelines,
    };
    write_viewport_projection_mask_to_stroke_uv(
        frame,
        &mut projection_deps,
        material_index,
        projection,
        texture_size,
        polygon_coverage,
    )
}

fn rgba8_mask_to_r8(rgba8: &[u8]) -> Vec<u8> {
    rgba8.chunks_exact(4).map(|pixel| pixel[0]).collect()
}
