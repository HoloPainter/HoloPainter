use anyhow::{Result, bail, ensure};

use crate::core::{
    composite::{ApplyParams, TextureCompositeMode},
    document::ActiveLayerTarget,
    mask::{FullMaskSource, MaskSource},
    surface::{PaintSurfaceId, PaintSurfaceRole},
    tool_operation::{PaintOperation, ToolOperation},
};

use super::{
    ApplyOneShotRenderPlan, DamageMap, PaintEditDecision, ReducerOutput,
    history_capture::PixelHistoryCapture,
    paint_edit::{PaintTargetScope, ResolvedPaintTarget},
    state::AppState,
};

pub(crate) fn cut_surface_pixels(
    state: &mut AppState,
    target: PaintSurfaceId,
) -> Result<ReducerOutput> {
    ensure!(
        state.active_layer_id() == target.layer_id,
        "cut layer target is no longer active"
    );
    let expected_active_target = match target.role {
        PaintSurfaceRole::Raster => ActiveLayerTarget::Raster,
        PaintSurfaceRole::LayerMask => ActiveLayerTarget::LayerMask,
    };
    ensure!(
        state.active_layer_target() == expected_active_target,
        "cut target changed before the edit was applied"
    );
    ensure!(
        state.focused_material_index() == target.material_index.as_usize(),
        "cut material changed before the edit was applied"
    );

    let paint_target = match state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Material(target.material_index()))
    {
        PaintEditDecision::Allowed(target) => target,
        PaintEditDecision::Blocked(reason) => {
            bail!("cut target is no longer editable: {reason:?}");
        }
        PaintEditDecision::Unavailable => {
            bail!("cut target is unavailable");
        }
    };

    ensure!(
        paint_target.target() == target,
        "cut target changed before the edit was applied"
    );

    clear_pixels(
        state,
        "Cut",
        paint_target,
        FullMaskSource::Material {
            material_index: target.material_index().as_usize(),
        },
    )
}

pub(crate) fn delete_selected_pixels(state: &mut AppState) -> Result<ReducerOutput> {
    let has_selection = state
        .active_selection()
        .is_some_and(|selection| selection.is_active());
    if !has_selection {
        return Ok(ReducerOutput::default());
    }

    let paint_target = match state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
    {
        PaintEditDecision::Allowed(target) => target,
        PaintEditDecision::Blocked(_) | PaintEditDecision::Unavailable => {
            return Ok(ReducerOutput::default());
        }
    };

    clear_pixels(
        state,
        "Delete",
        paint_target,
        FullMaskSource::MeshAllMaterials,
    )
}

fn clear_pixels(
    state: &mut AppState,
    history_label: &'static str,
    paint_target: ResolvedPaintTarget,
    full_mask: FullMaskSource,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        bail!("{history_label} requires a loaded document");
    };
    let target = paint_target.target();
    let surfaces = paint_target.surfaces_vec();
    let mut damage = DamageMap::default();
    for surface in &surfaces {
        let Some(texture_size) = document.texture_size_for_surface(*surface) else {
            bail!("{history_label} surface is unavailable");
        };
        damage.add_full_surface(*surface, texture_size);
    }

    let plan = ApplyOneShotRenderPlan::new(
        history_label,
        target,
        surfaces,
        MaskSource::Full(full_mask),
        ToolOperation::Paint(PaintOperation::solid_color([0.0, 0.0, 0.0])),
        ApplyParams::new(1.0, TextureCompositeMode::Clear),
        document.active_selection.clone(),
        Some(damage.clone()),
        PixelHistoryCapture::from_damage(damage),
    );

    let history_label = plan.history_label();
    let renderer_commit = plan.renderer_commit();
    let (command, history_transaction) = plan.into_edit_command_and_history(state)?;
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction(history_label, history_transaction)
        .with_renderer_commit(renderer_commit);
    output.push_edit_command(command);
    Ok(output)
}
