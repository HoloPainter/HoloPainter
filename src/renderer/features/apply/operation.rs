use crate::{
    core::surface::PaintSurfaceId,
    renderer::{
        features::apply::{
            deps::PaintApplyDeps,
            masked::{OneShotMaskedApplyRequest, record_masked_paint_apply_with_uniform},
            materialize::PreparedMask,
            paint::composite_uniform_from_apply_operation,
        },
        gpu::{copy_a_to_b, frame::GpuFrame},
    },
};

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyOperation {
    SolidColorPaint { color: [f32; 3] },
}

pub(crate) fn apply_operation_with_prepared_mask(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    request: &OneShotMaskedApplyRequest,
    mask: &PreparedMask,
) -> bool {
    debug_assert_eq!(mask.target, request.target);
    let target = request.target;
    let comp_u = composite_uniform_from_apply_operation(&request.operation, request.params);
    prepare_layer_snapshot(frame, txn, target, mask).is_some() && {
        record_masked_paint_apply_with_uniform(
            frame,
            txn,
            target,
            &comp_u,
            Some(&request.active_selection),
        );
        true
    }
}

fn prepare_layer_snapshot(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    target: PaintSurfaceId,
    mask: &PreparedMask,
) -> Option<()> {
    debug_assert_eq!(mask.material_index, target.material_index().as_usize());
    let layer = txn.surfaces.stroke_surface_target(target, txn.scratch)?;
    debug_assert_eq!(mask.texture_size, layer.texture_size);
    copy_a_to_b(
        frame.encoder(),
        layer.write_texture,
        layer.read_texture,
        layer.texture_size,
    );
    Some(())
}
