use anyhow::Result;
use glam::{Quat, Vec2, Vec3};

use crate::{
    application::{
        PaintEditBlockReason, PointerSample, PointerSampleKind, ReducerOutput, RendererCommitSpec,
        ToolCancelReason, ToolInputEvent, history_capture::PixelHistoryCapture,
    },
    core::{
        decal::{
            DECAL_HANDLE_HIT_RADIUS_PX, DecalApplyPlan, DecalNormalMode, DecalProjection,
            DecalTransform, ViewProjectionDecalTransform, hit_test_decal_handle,
        },
        math::ray_from_viewport_px,
        stroke::StrokeSpace,
        transform::TransformHandle,
    },
    renderer::{ApplyCommand, EditCommand},
};

use super::state::{
    AppState, DecalGesture, DecalSession, PaintTargetScope, ToolSession,
    ViewProjectionDecalGesture, ViewProjectionDecalSession,
};

const ROTATION_SNAP_RADIANS: f32 = std::f32::consts::PI / 12.0;

fn set_decal_paint_unavailable_status(
    state: &mut AppState,
    reason: Option<PaintEditBlockReason>,
    applying: bool,
) {
    let key = match (reason, applying) {
        (Some(PaintEditBlockReason::MaterialExcluded), false) => {
            "status-decal-place-material-excluded"
        }
        (Some(PaintEditBlockReason::MaterialExcluded), true) => {
            "status-decal-apply-material-excluded"
        }
        (Some(PaintEditBlockReason::ActiveLayerLocked), false) => "status-decal-place-layer-locked",
        (Some(PaintEditBlockReason::ActiveLayerLocked), true) => "status-decal-apply-layer-locked",
        (Some(PaintEditBlockReason::ActiveLayerHidden), false) => "status-decal-place-layer-hidden",
        (Some(PaintEditBlockReason::ActiveLayerHidden), true) => "status-decal-apply-layer-hidden",
        (Some(PaintEditBlockReason::ActiveLayerIsGroup), false) => "status-decal-place-layer-group",
        (Some(PaintEditBlockReason::ActiveLayerIsGroup), true) => "status-decal-apply-layer-group",
        (Some(PaintEditBlockReason::ActiveTargetNotEditable), false) => {
            "status-decal-place-target-not-editable"
        }
        (Some(PaintEditBlockReason::ActiveTargetNotEditable), true) => {
            "status-decal-apply-target-not-editable"
        }
        (None, false) => "status-decal-place-no-document",
        (None, true) => "status-decal-apply-no-document",
    };
    state.set_status_key(key);
}

pub(crate) fn handle_decal_input(state: &mut AppState, event: ToolInputEvent) -> ReducerOutput {
    match event {
        ToolInputEvent::PointerDown(sample) => {
            if state.tool.decal_session().is_some() {
                begin_decal_gesture(state, sample)
            } else {
                begin_decal_session(state, sample)
            }
        }
        ToolInputEvent::PointerMove(sample) => update_decal_gesture(state, sample),
        ToolInputEvent::PointerUp(_) => finish_decal_gesture(state),
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
}

pub(crate) fn begin_view_projection_decal(
    state: &mut AppState,
    viewport_size: [u32; 2],
) -> ReducerOutput {
    if state.effective_tool_id() != crate::core::tool::ToolId::ViewProjectionDecal
        || state.has_active_decal_session()
    {
        return ReducerOutput::default();
    }
    let Some(image_size) = state.decal_image().map(|image| image.size) else {
        state.set_status_key("status-view-decal-select-image");
        return ReducerOutput::default();
    };
    let paint_edit = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials);
    if !paint_edit.is_allowed() {
        set_decal_paint_unavailable_status(state, paint_edit.blocked_reason(), false);
        return ReducerOutput::default();
    }
    let Some(transform) = ViewProjectionDecalTransform::initial(image_size, viewport_size) else {
        state.set_status_key("status-view-decal-create-rectangle-failed");
        return ReducerOutput::default();
    };
    let scene_visibility = state.viewport_scene_visibility().clone();
    state.tool.set_session(ToolSession::ViewProjectionDecal(
        ViewProjectionDecalSession {
            transform,
            gesture: None,
            scene_visibility,
        },
    ));
    state.tool.view_projection_decal_start_requested = false;
    state.set_status_key("status-view-decal-ready");
    ReducerOutput::default()
}

pub(crate) fn request_view_projection_decal(state: &mut AppState) -> ReducerOutput {
    if state.effective_tool_id() == crate::core::tool::ToolId::ViewProjectionDecal
        && state.decal_image().is_some()
        && !state.has_active_decal_session()
    {
        state.tool.view_projection_decal_start_requested = true;
        state.set_status_key("status-view-decal-preparing");
    }
    ReducerOutput::default()
}

pub(crate) fn sync_view_projection_decal_viewport(
    state: &mut AppState,
    viewport_size: [u32; 2],
) -> ReducerOutput {
    let Some(session) = state.tool.view_projection_decal_session_mut() else {
        return ReducerOutput::default();
    };
    if session.transform.viewport_size != viewport_size
        && let Some(transform) = session.transform.resized(viewport_size)
    {
        session.transform = transform;
        session.gesture = None;
    }
    ReducerOutput::default()
}

pub(crate) fn handle_view_projection_decal_input(
    state: &mut AppState,
    event: ToolInputEvent,
) -> ReducerOutput {
    match event {
        ToolInputEvent::PointerDown(sample) => begin_view_projection_gesture(state, sample),
        ToolInputEvent::PointerMove(sample) => update_view_projection_gesture(state, sample),
        ToolInputEvent::PointerUp(_) => finish_view_projection_gesture(state),
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
}

fn begin_view_projection_gesture(state: &mut AppState, sample: PointerSample) -> ReducerOutput {
    let Some(transform) = state.active_view_projection_decal_transform() else {
        return ReducerOutput::default();
    };
    let Some(geometry) = transform.handle_geometry() else {
        return ReducerOutput::default();
    };
    let Some(handle) =
        hit_test_decal_handle(sample.screen_px(), geometry, DECAL_HANDLE_HIT_RADIUS_PX)
    else {
        return ReducerOutput::default();
    };
    let Some(session) = state.tool.view_projection_decal_session_mut() else {
        return ReducerOutput::default();
    };
    session.gesture = Some(ViewProjectionDecalGesture {
        handle,
        pointer_start_px: sample.screen_px(),
        base_transform: transform,
    });
    state.set_status_key("status-view-decal-transforming");
    ReducerOutput::default()
}

fn update_view_projection_gesture(state: &mut AppState, sample: PointerSample) -> ReducerOutput {
    let Some(gesture) = state
        .tool
        .view_projection_decal_session()
        .and_then(|session| session.gesture)
    else {
        return ReducerOutput::default();
    };
    let updated = transform_view_projection_decal(gesture, sample);
    let Some(session) = state.tool.view_projection_decal_session_mut() else {
        return ReducerOutput::default();
    };
    session.transform = updated;
    ReducerOutput::default()
}

fn finish_view_projection_gesture(state: &mut AppState) -> ReducerOutput {
    if state
        .tool
        .view_projection_decal_session_mut()
        .is_some_and(|session| session.gesture.take().is_some())
    {
        state.set_status_key("status-view-decal-ready");
    }
    ReducerOutput::default()
}

fn transform_view_projection_decal(
    gesture: ViewProjectionDecalGesture,
    sample: PointerSample,
) -> ViewProjectionDecalTransform {
    let base = gesture.base_transform;
    let pointer = sample.screen_px();
    let delta = pointer - gesture.pointer_start_px;
    let viewport = Vec2::new(base.viewport_size[0] as f32, base.viewport_size[1] as f32);
    if gesture.handle == TransformHandle::Move {
        let center = (base.center_px() + delta).clamp(Vec2::ZERO, viewport);
        return ViewProjectionDecalTransform {
            center_uv: center / viewport,
            ..base
        };
    }
    if gesture.handle == TransformHandle::Rotate {
        let center = base.center_px();
        let start = gesture.pointer_start_px - center;
        let current = pointer - center;
        if start.length_squared() <= 1.0e-6 || current.length_squared() <= 1.0e-6 {
            return base;
        }
        let mut angle = base.rotation_radians + current.y.atan2(current.x) - start.y.atan2(start.x);
        if sample.modifiers().shift {
            angle = (angle / ROTATION_SNAP_RADIANS).round() * ROTATION_SNAP_RADIANS;
        }
        return ViewProjectionDecalTransform {
            rotation_radians: wrap_angle(angle),
            ..base
        };
    }

    let (sign_x, sign_y) = scale_handle_signs(gesture.handle);
    let center = base.center_px();
    let (sin, cos) = base.rotation_radians.sin_cos();
    let to_local = |point: Vec2| {
        let p = point - center;
        Vec2::new(cos * p.x + sin * p.y, -sin * p.x + cos * p.y)
    };
    let target = to_local(pointer);
    let from_center = sample.modifiers().alt;
    let mut size = base.size_px;
    let mut local_center = Vec2::ZERO;
    let minimum = 16.0;

    if sign_x.is_some() && sign_y.is_some() && !sample.modifiers().shift {
        let sx = sign_x.unwrap();
        let sy = sign_y.unwrap();
        let requested_x = if from_center {
            2.0 * sx * target.x
        } else {
            sx * (target.x + sx * base.size_px.x * 0.5)
        };
        let requested_y = if from_center {
            2.0 * sy * target.y
        } else {
            sy * (target.y + sy * base.size_px.y * 0.5)
        };
        let factor = (requested_x / base.size_px.x)
            .max(requested_y / base.size_px.y)
            .max(minimum / base.size_px.min_element());
        size = base.size_px * factor;
        if !from_center {
            local_center = Vec2::new(
                -sx * base.size_px.x * 0.5 + sx * size.x * 0.5,
                -sy * base.size_px.y * 0.5 + sy * size.y * 0.5,
            );
        }
    } else {
        if let Some(sign) = sign_x {
            let (new_size, offset) =
                scaled_axis(base.size_px.x, target.x, sign, from_center, minimum);
            size.x = new_size;
            local_center.x = offset;
        }
        if let Some(sign) = sign_y {
            let (new_size, offset) =
                scaled_axis(base.size_px.y, target.y, sign, from_center, minimum);
            size.y = new_size;
            local_center.y = offset;
        }
    }
    let rotated_offset = Vec2::new(
        cos * local_center.x - sin * local_center.y,
        sin * local_center.x + cos * local_center.y,
    );
    let new_center = (center + rotated_offset).clamp(Vec2::ZERO, viewport);
    ViewProjectionDecalTransform {
        center_uv: new_center / viewport,
        size_px: size,
        ..base
    }
}

pub(crate) fn cancel_active_decal(state: &mut AppState, reason: ToolCancelReason) -> ReducerOutput {
    if matches!(
        reason,
        ToolCancelReason::FocusLost
            | ToolCancelReason::PointerCaptureLost
            | ToolCancelReason::TouchCancelled
    ) {
        let surface_gesture_ended = state
            .tool
            .decal_session_mut()
            .is_some_and(|session| session.gesture.take().is_some());
        let view_gesture_ended = state
            .tool
            .view_projection_decal_session_mut()
            .is_some_and(|session| session.gesture.take().is_some());
        let gesture_ended = surface_gesture_ended || view_gesture_ended;
        if gesture_ended {
            state.set_status_key(if view_gesture_ended {
                "status-view-decal-ready"
            } else {
                "status-decal-ready"
            });
        }
        return ReducerOutput::default();
    }

    let cancelled = state.tool.take_decal_session().is_some()
        || state.tool.take_view_projection_decal_session().is_some();
    state.tool.view_projection_decal_start_requested = false;
    if cancelled {
        state.set_status_key("status-decal-cancelled");
    }
    ReducerOutput::default()
}

pub(crate) fn apply_active_decal(state: &mut AppState) -> Result<ReducerOutput> {
    if state.tool.view_projection_decal_session().is_some() {
        return apply_active_view_projection_decal(state);
    }
    let Some(session) = state.tool.decal_session().cloned() else {
        return Ok(ReducerOutput::default());
    };
    if session.gesture.is_some() {
        state.set_status_key("status-decal-finish-gesture");
        return Ok(ReducerOutput::default());
    }
    let Some(image) = state.decal_image().cloned() else {
        state.set_status_key("status-decal-select-image-before-apply");
        return Ok(ReducerOutput::default());
    };
    let opacity = state.decal_options().opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        state.set_status_key("status-decal-opacity-zero");
        return Ok(ReducerOutput::default());
    }
    let paint_edit = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials);
    let blocked_reason = paint_edit.blocked_reason();
    let Some(paint_target) = paint_edit.into_allowed() else {
        set_decal_paint_unavailable_status(state, blocked_reason, true);
        return Ok(ReducerOutput::default());
    };
    let surfaces = paint_target.surfaces_vec();
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let active_selection = document.active_selection.clone();
    let eligible_surfaces = surfaces
        .into_iter()
        .filter(|surface| {
            active_selection.is_effectively_full()
                || active_selection
                    .material_mask(surface.material_index())
                    .is_some_and(|mask| mask.mask_id.is_some())
        })
        .collect::<Vec<_>>();
    let Some(plan) = DecalApplyPlan::build_with_visibility(
        document,
        &eligible_surfaces,
        session.transform,
        &session.scene_visibility,
    ) else {
        state.set_status_key("status-decal-footprint-failed");
        return Ok(ReducerOutput::default());
    };
    if plan.is_empty() {
        state.set_status_key("status-decal-no-editable-overlap");
        return Ok(ReducerOutput::default());
    }
    let surfaces = plan.surfaces();
    let damage = plan.damage();
    let history_transaction = PixelHistoryCapture::from_damage(damage)
        .map(|capture| capture.capture(state))
        .transpose()?
        .flatten();
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction("Surface Decal", history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(surfaces));
    output.push_edit_command(EditCommand::Apply(ApplyCommand::Decal {
        plan,
        image,
        projection: DecalProjection::Surface(session.transform),
        opacity,
        active_selection,
    }));
    let _ = state.tool.take_decal_session();
    state.set_status_key("status-surface-decal-applied");
    Ok(output)
}

fn apply_active_view_projection_decal(state: &mut AppState) -> Result<ReducerOutput> {
    let Some(session) = state.tool.view_projection_decal_session().cloned() else {
        return Ok(ReducerOutput::default());
    };
    if session.gesture.is_some() {
        state.set_status_key("status-decal-finish-gesture");
        return Ok(ReducerOutput::default());
    }
    let Some(image) = state.decal_image().cloned() else {
        state.set_status_key("status-decal-select-image-before-apply");
        return Ok(ReducerOutput::default());
    };
    let opacity = state.decal_options().opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        state.set_status_key("status-decal-opacity-zero");
        return Ok(ReducerOutput::default());
    }
    let paint_edit = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials);
    let blocked_reason = paint_edit.blocked_reason();
    let Some(paint_target) = paint_edit.into_allowed() else {
        set_decal_paint_unavailable_status(state, blocked_reason, true);
        return Ok(ReducerOutput::default());
    };
    let viewport_size = session.transform.viewport_size;
    let viewport_view_proj = state
        .camera()
        .view_projection_matrix(viewport_size[0].max(1) as f32 / viewport_size[1].max(1) as f32);
    let Some(projector_view_proj) = session
        .transform
        .projector_view_projection(viewport_view_proj)
    else {
        state.set_status_key("status-view-decal-projection-failed");
        return Ok(ReducerOutput::default());
    };
    let projection = DecalProjection::ViewProjection {
        projector_view_proj,
        viewport_view_proj,
        depth_size: [
            session.transform.size_px.x.round().max(1.0) as u32,
            session.transform.size_px.y.round().max(1.0) as u32,
        ],
    };
    let surfaces = paint_target.surfaces_vec();
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let active_selection = document.active_selection.clone();
    let eligible_surfaces = surfaces
        .into_iter()
        .filter(|surface| {
            active_selection.is_effectively_full()
                || active_selection
                    .material_mask(surface.material_index())
                    .is_some_and(|mask| mask.mask_id.is_some())
        })
        .collect::<Vec<_>>();
    let Some(plan) = DecalApplyPlan::build_for_projection_with_visibility(
        document,
        &eligible_surfaces,
        projection,
        &session.scene_visibility,
    ) else {
        state.set_status_key("status-decal-footprint-failed");
        return Ok(ReducerOutput::default());
    };
    if plan.is_empty() {
        state.set_status_key("status-decal-no-editable-overlap");
        return Ok(ReducerOutput::default());
    }
    let surfaces = plan.surfaces();
    let history_transaction = PixelHistoryCapture::from_damage(plan.damage())
        .map(|capture| capture.capture(state))
        .transpose()?
        .flatten();
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction("View Projection Decal", history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(surfaces));
    output.push_edit_command(EditCommand::Apply(ApplyCommand::Decal {
        plan,
        image,
        projection,
        opacity,
        active_selection,
    }));
    let _ = state.tool.take_view_projection_decal_session();
    state.tool.view_projection_decal_start_requested = false;
    state.set_status_key("status-view-decal-applied");
    Ok(output)
}

fn begin_decal_session(state: &mut AppState, sample: PointerSample) -> ReducerOutput {
    if sample.space() != StrokeSpace::Surface {
        return ReducerOutput::default();
    }
    let PointerSampleKind::Surface {
        view,
        hit: Some(hit),
    } = sample.kind
    else {
        return ReducerOutput::default();
    };
    let Some(image_size) = state.decal_image().map(|image| image.size) else {
        state.set_status_key("status-decal-select-image");
        return ReducerOutput::default();
    };
    let paint_edit = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials);
    if !paint_edit.is_allowed() {
        set_decal_paint_unavailable_status(state, paint_edit.blocked_reason(), false);
        return ReducerOutput::default();
    }
    let Some(scene_diagonal) = state
        .document()
        .map(|document| document.mesh.scene_diagonal())
    else {
        state.set_status_key("status-decal-load-mesh");
        return ReducerOutput::default();
    };
    let Some(surface_normal) = decal_surface_normal(state, hit) else {
        state.set_status_key("status-decal-normal-failed");
        return ReducerOutput::default();
    };
    let horizontal_axis =
        screen_horizontal_axis_on_plane(sample.screen_px(), view, hit.world_pos, surface_normal)
            .unwrap_or_else(|| state.camera().right());
    let Some(transform) = DecalTransform::from_surface_hit(
        hit,
        surface_normal,
        horizontal_axis,
        state.camera().up(),
        scene_diagonal,
        image_size,
    ) else {
        state.set_status_key("status-decal-transform-create-failed");
        return ReducerOutput::default();
    };

    let scene_visibility = state.viewport_scene_visibility().clone();
    state.tool.set_session(ToolSession::Decal(DecalSession {
        transform,
        source_hit: hit,
        gesture: None,
        scene_visibility,
    }));
    state.set_status_key("status-decal-ready");
    ReducerOutput::default()
}

fn begin_decal_gesture(state: &mut AppState, sample: PointerSample) -> ReducerOutput {
    let PointerSampleKind::Surface { view, .. } = sample.kind else {
        return ReducerOutput::default();
    };
    let Some(base_transform) = state.active_decal_transform() else {
        return ReducerOutput::default();
    };
    let Some(display_transform) = state.active_decal_handle_display_transform() else {
        return ReducerOutput::default();
    };
    let Some(geometry) = display_transform.handle_geometry(view.view_proj, view.size) else {
        return ReducerOutput::default();
    };
    let Some(handle) =
        hit_test_decal_handle(sample.screen_px(), geometry, DECAL_HANDLE_HIT_RADIUS_PX)
    else {
        return ReducerOutput::default();
    };
    let grab_local = pointer_on_decal_plane(sample, base_transform)
        .unwrap_or_else(|| decal_handle_local(base_transform, handle));
    let Some(session) = state.tool.decal_session_mut() else {
        return ReducerOutput::default();
    };
    session.gesture = Some(DecalGesture {
        handle,
        pointer_start_px: sample.screen_px(),
        base_transform,
        grab_local,
    });
    state.set_status_key("status-decal-transforming");
    ReducerOutput::default()
}

fn update_decal_gesture(state: &mut AppState, sample: PointerSample) -> ReducerOutput {
    let Some(gesture) = state
        .tool
        .decal_session()
        .and_then(|session| session.gesture)
    else {
        return ReducerOutput::default();
    };
    let minimum_size = state
        .document()
        .map(|document| document.mesh.scene_diagonal() * 0.0005)
        .unwrap_or(1.0e-6)
        .max(1.0e-6);

    let updated = match gesture.handle {
        TransformHandle::Move => move_decal_transform(state, sample, gesture),
        TransformHandle::Rotate => rotate_decal_transform(state, sample, gesture),
        handle => scale_decal_transform_from_sample(sample, gesture, handle, minimum_size),
    };
    let Some((transform, hit)) = updated else {
        return ReducerOutput::default();
    };
    let Some(session) = state.tool.decal_session_mut() else {
        return ReducerOutput::default();
    };
    session.transform = transform;
    if let Some(hit) = hit {
        session.source_hit = hit;
    }
    ReducerOutput::default()
}

fn finish_decal_gesture(state: &mut AppState) -> ReducerOutput {
    let gesture_ended = state
        .tool
        .decal_session_mut()
        .is_some_and(|session| session.gesture.take().is_some());
    if gesture_ended {
        state.set_status_key("status-decal-ready");
    }
    ReducerOutput::default()
}

pub(crate) fn refresh_active_decal_normal(state: &mut AppState) {
    let Some(session) = state.tool.decal_session().cloned() else {
        return;
    };
    let Some(normal_world) = decal_surface_normal(state, session.source_hit) else {
        return;
    };
    let Some(transform) = reorient_decal_transform(session.transform, normal_world) else {
        return;
    };
    let Some(session) = state.tool.decal_session_mut() else {
        return;
    };
    session.transform = transform;
    if let Some(gesture) = &mut session.gesture {
        if let Some(base_transform) = reorient_decal_transform(gesture.base_transform, normal_world)
        {
            gesture.base_transform = base_transform;
        }
    }
}

fn move_decal_transform(
    state: &AppState,
    sample: PointerSample,
    gesture: DecalGesture,
) -> Option<(DecalTransform, Option<crate::core::document::SurfaceHit>)> {
    let PointerSampleKind::Surface { hit: Some(hit), .. } = sample.kind else {
        return None;
    };
    let normal_world = decal_surface_normal(state, hit)?;
    let axis_x_world = tangent_axis(gesture.base_transform.axis_x_world, normal_world)
        .or_else(|| tangent_axis(gesture.base_transform.axis_y_world, normal_world))?;
    let axis_y_world = normal_world.cross(axis_x_world).normalize_or_zero();
    let transform = DecalTransform {
        center_world: hit.world_pos
            - axis_x_world * gesture.grab_local.x
            - axis_y_world * gesture.grab_local.y,
        axis_x_world,
        axis_y_world,
        normal_world,
        ..gesture.base_transform
    };
    transform.is_valid().then_some((transform, Some(hit)))
}

fn rotate_decal_transform(
    state: &AppState,
    sample: PointerSample,
    gesture: DecalGesture,
) -> Option<(DecalTransform, Option<crate::core::document::SurfaceHit>)> {
    let PointerSampleKind::Surface { view, .. } = sample.kind else {
        return None;
    };
    let source_hit = state.tool.decal_session()?.source_hit;
    let display_transform =
        state.decal_handle_display_transform(gesture.base_transform, source_hit)?;
    let geometry = display_transform.handle_geometry(view.view_proj, view.size)?;
    let start = gesture.pointer_start_px - geometry.center;
    let current = sample.screen_px() - geometry.center;
    if start.length_squared() <= f32::EPSILON || current.length_squared() <= f32::EPSILON {
        return None;
    }
    let mut screen_delta = current.y.atan2(current.x) - start.y.atan2(start.x);
    screen_delta = wrap_angle(screen_delta);
    if sample.meta.modifiers.shift {
        screen_delta = (screen_delta / ROTATION_SNAP_RADIANS).round() * ROTATION_SNAP_RADIANS;
    }
    let rotation = Quat::from_axis_angle(gesture.base_transform.normal_world, -screen_delta);
    let transform = DecalTransform {
        axis_x_world: (rotation * gesture.base_transform.axis_x_world).normalize_or_zero(),
        axis_y_world: (rotation * gesture.base_transform.axis_y_world).normalize_or_zero(),
        ..gesture.base_transform
    };
    transform.is_valid().then_some((transform, None))
}

fn scale_decal_transform_from_sample(
    sample: PointerSample,
    gesture: DecalGesture,
    handle: TransformHandle,
    minimum_size: f32,
) -> Option<(DecalTransform, Option<crate::core::document::SurfaceHit>)> {
    let current_local = pointer_on_decal_plane(sample, gesture.base_transform)?;
    let handle_local = decal_handle_local(gesture.base_transform, handle);
    let target_local = current_local - (gesture.grab_local - handle_local);
    let preserve_aspect = if handle.is_corner() {
        !sample.meta.modifiers.shift
    } else {
        sample.meta.modifiers.shift
    };
    let transform = scale_decal_transform(
        gesture.base_transform,
        handle,
        target_local,
        sample.meta.modifiers.alt,
        preserve_aspect,
        minimum_size,
    )?;
    Some((transform, None))
}

fn scale_decal_transform(
    base: DecalTransform,
    handle: TransformHandle,
    target_local: Vec2,
    from_center: bool,
    preserve_aspect: bool,
    minimum_size: f32,
) -> Option<DecalTransform> {
    let (sign_x, sign_y) = scale_handle_signs(handle);
    if sign_x.is_none() && sign_y.is_none() {
        return None;
    }
    let minimum_size = minimum_size.max(1.0e-6);
    let mut size = base.size_world;
    let mut center_local = Vec2::ZERO;

    if preserve_aspect && handle.is_corner() {
        let sx = sign_x?;
        let sy = sign_y?;
        let (origin, base_vector) = if from_center {
            (
                Vec2::ZERO,
                Vec2::new(sx * base.size_world.x * 0.5, sy * base.size_world.y * 0.5),
            )
        } else {
            let anchor = Vec2::new(-sx * base.size_world.x * 0.5, -sy * base.size_world.y * 0.5);
            (
                anchor,
                Vec2::new(sx * base.size_world.x, sy * base.size_world.y),
            )
        };
        let minimum_factor =
            (minimum_size / base.size_world.x).max(minimum_size / base.size_world.y);
        let factor = ((target_local - origin).dot(base_vector) / base_vector.length_squared())
            .max(minimum_factor);
        size = base.size_world * factor;
        if !from_center {
            center_local = origin + Vec2::new(sx * size.x, sy * size.y) * 0.5;
        }
    } else if preserve_aspect {
        let minimum_factor =
            (minimum_size / base.size_world.x).max(minimum_size / base.size_world.y);
        if let Some(sx) = sign_x {
            let (requested_size, _) = scaled_axis(
                base.size_world.x,
                target_local.x,
                sx,
                from_center,
                minimum_size,
            );
            let factor = (requested_size / base.size_world.x).max(minimum_factor);
            size = base.size_world * factor;
            if !from_center {
                let anchor = -sx * base.size_world.x * 0.5;
                center_local.x = anchor + sx * size.x * 0.5;
            }
        } else {
            let sy = sign_y?;
            let (requested_size, _) = scaled_axis(
                base.size_world.y,
                target_local.y,
                sy,
                from_center,
                minimum_size,
            );
            let factor = (requested_size / base.size_world.y).max(minimum_factor);
            size = base.size_world * factor;
            if !from_center {
                let anchor = -sy * base.size_world.y * 0.5;
                center_local.y = anchor + sy * size.y * 0.5;
            }
        }
    } else {
        if let Some(sx) = sign_x {
            let (new_size, center) = scaled_axis(
                base.size_world.x,
                target_local.x,
                sx,
                from_center,
                minimum_size,
            );
            size.x = new_size;
            center_local.x = center;
        }
        if let Some(sy) = sign_y {
            let (new_size, center) = scaled_axis(
                base.size_world.y,
                target_local.y,
                sy,
                from_center,
                minimum_size,
            );
            size.y = new_size;
            center_local.y = center;
        }
    }

    let transform = DecalTransform {
        center_world: base.center_world
            + base.axis_x_world * center_local.x
            + base.axis_y_world * center_local.y,
        size_world: size,
        ..base
    };
    transform.is_valid().then_some(transform)
}

fn scaled_axis(
    base_size: f32,
    target: f32,
    sign: f32,
    from_center: bool,
    minimum_size: f32,
) -> (f32, f32) {
    if from_center {
        let size = (2.0 * sign * target).max(minimum_size);
        (size, 0.0)
    } else {
        let anchor = -sign * base_size * 0.5;
        let size = (sign * (target - anchor)).max(minimum_size);
        (size, anchor + sign * size * 0.5)
    }
}

fn pointer_on_decal_plane(sample: PointerSample, transform: DecalTransform) -> Option<Vec2> {
    let PointerSampleKind::Surface { view, .. } = sample.kind else {
        return None;
    };
    let (ray_origin, ray_direction) =
        ray_from_viewport_px(sample.screen_px(), view.size, view.inv_view_proj)?;
    let denominator = ray_direction.dot(transform.normal_world);
    if denominator.abs() <= 1.0e-6 {
        return None;
    }
    let distance = (transform.center_world - ray_origin).dot(transform.normal_world) / denominator;
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    let world = ray_origin + ray_direction * distance;
    let relative = world - transform.center_world;
    Some(Vec2::new(
        relative.dot(transform.axis_x_world),
        relative.dot(transform.axis_y_world),
    ))
}

fn decal_surface_normal(state: &AppState, hit: crate::core::document::SurfaceHit) -> Option<Vec3> {
    let document = state.document()?;
    let mut normal = match state.decal_options().normal_mode {
        DecalNormalMode::Face => document
            .mesh
            .triangle_geometric_normal(hit.triangle_index)?,
        DecalNormalMode::Smooth => document.mesh.local_smooth_triangle_geometric_normal(
            hit.triangle_index,
            hit.world_pos,
            state.decal_options().smooth_angle_degrees,
        )?,
    };
    if state.decal_options().normal_mode == DecalNormalMode::Face {
        let shading_normal = hit.world_normal.normalize_or_zero();
        if shading_normal.length_squared() > f32::EPSILON && normal.dot(shading_normal) < 0.0 {
            normal = -normal;
        }
    }
    if state.decal_options().normal_mode == DecalNormalMode::Smooth {
        let face_normal = document
            .mesh
            .triangle_geometric_normal(hit.triangle_index)?;
        if normal.dot(face_normal) < 0.0 {
            normal = -normal;
        }
    }
    (normal.length_squared() > f32::EPSILON && normal.is_finite()).then_some(normal)
}

fn reorient_decal_transform(
    transform: DecalTransform,
    normal_world: Vec3,
) -> Option<DecalTransform> {
    let normal_world = normal_world.normalize_or_zero();
    let axis_x_world = tangent_axis(transform.axis_x_world, normal_world)
        .or_else(|| tangent_axis(transform.axis_y_world, normal_world))?;
    let axis_y_world = normal_world.cross(axis_x_world).normalize_or_zero();
    let transform = DecalTransform {
        axis_x_world,
        axis_y_world,
        normal_world,
        ..transform
    };
    transform.is_valid().then_some(transform)
}

fn screen_horizontal_axis_on_plane(
    pointer_px: Vec2,
    view: crate::application::ViewportInputContext,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let max_x = view.size[0].saturating_sub(1) as f32;
    let offset = (view.size[0].max(1) as f32 * 0.01).clamp(2.0, 8.0);
    let left_px = Vec2::new((pointer_px.x - offset).clamp(0.0, max_x), pointer_px.y);
    let right_px = Vec2::new((pointer_px.x + offset).clamp(0.0, max_x), pointer_px.y);
    if right_px.x - left_px.x <= 1.0e-3 {
        return None;
    }
    let left = ray_plane_intersection(left_px, view, plane_point, plane_normal)?;
    let right = ray_plane_intersection(right_px, view, plane_point, plane_normal)?;
    tangent_axis(right - left, plane_normal)
}

fn ray_plane_intersection(
    pointer_px: Vec2,
    view: crate::application::ViewportInputContext,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let (ray_origin, ray_direction) =
        ray_from_viewport_px(pointer_px, view.size, view.inv_view_proj)?;
    let denominator = ray_direction.dot(plane_normal);
    if denominator.abs() <= 1.0e-6 {
        return None;
    }
    let distance = (plane_point - ray_origin).dot(plane_normal) / denominator;
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    let intersection = ray_origin + ray_direction * distance;
    intersection.is_finite().then_some(intersection)
}

fn decal_handle_local(transform: DecalTransform, handle: TransformHandle) -> Vec2 {
    let half = transform.size_world * 0.5;
    match handle {
        TransformHandle::ScaleTopLeft => Vec2::new(-half.x, -half.y),
        TransformHandle::ScaleTopRight => Vec2::new(half.x, -half.y),
        TransformHandle::ScaleBottomRight => Vec2::new(half.x, half.y),
        TransformHandle::ScaleBottomLeft => Vec2::new(-half.x, half.y),
        TransformHandle::ScaleTop => Vec2::new(0.0, -half.y),
        TransformHandle::ScaleRight => Vec2::new(half.x, 0.0),
        TransformHandle::ScaleBottom => Vec2::new(0.0, half.y),
        TransformHandle::ScaleLeft => Vec2::new(-half.x, 0.0),
        TransformHandle::Move | TransformHandle::Rotate => Vec2::ZERO,
    }
}

fn scale_handle_signs(handle: TransformHandle) -> (Option<f32>, Option<f32>) {
    match handle {
        TransformHandle::ScaleLeft => (Some(-1.0), None),
        TransformHandle::ScaleRight => (Some(1.0), None),
        TransformHandle::ScaleTop => (None, Some(-1.0)),
        TransformHandle::ScaleBottom => (None, Some(1.0)),
        TransformHandle::ScaleTopLeft => (Some(-1.0), Some(-1.0)),
        TransformHandle::ScaleTopRight => (Some(1.0), Some(-1.0)),
        TransformHandle::ScaleBottomLeft => (Some(-1.0), Some(1.0)),
        TransformHandle::ScaleBottomRight => (Some(1.0), Some(1.0)),
        TransformHandle::Move | TransformHandle::Rotate => (None, None),
    }
}

fn tangent_axis(candidate: Vec3, normal: Vec3) -> Option<Vec3> {
    let tangent = (candidate - normal * candidate.dot(normal)).normalize_or_zero();
    (tangent.length_squared() > f32::EPSILON).then_some(tangent)
}

fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{Mat4, Vec2, Vec3};

    use crate::{
        application::{
            AppState, Command, InputModifiers, PointerSample, PointerSampleKind, ToolCancelReason,
            ToolInputEvent, ViewportInputContext, reduce, state::EditorDocumentState,
        },
        core::{
            decal::{
                DecalImageAsset, DecalImageId, DecalNormalMode, DecalToolOptions, DecalTransform,
            },
            document::{Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh, SurfaceHit},
            tool::ToolId,
        },
    };

    #[test]
    fn decal_status_reports_material_mask_exclusion_reason() {
        let mut state = AppState::default();

        super::set_decal_paint_unavailable_status(
            &mut state,
            Some(crate::application::PaintEditBlockReason::MaterialExcluded),
            false,
        );

        assert!(
            state
                .status()
                .contains("material mask excludes every surface")
        );
    }

    fn state_with_surface_document() -> AppState {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0)],
        )
        .unwrap();
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            mesh,
            vec![MaterialSpec::new("Material", [8, 8])],
        ));
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        state.set_active_tool(ToolId::SurfaceDecal);
        state.tool.decal_image = Some(Arc::new(DecalImageAsset {
            id: DecalImageId(1),
            file_name: "decal.png".to_owned(),
            size: [4, 2],
            rgba8: Arc::<[u8]>::from(vec![255; 4 * 2 * 4]),
        }));
        state
    }

    fn pointer_sample() -> PointerSample {
        PointerSample::surface_with_pressure(
            Vec2::new(10.0, 20.0),
            1.0,
            InputModifiers::default(),
            ViewportInputContext {
                size: [100, 100],
                view_proj: Mat4::IDENTITY,
                inv_view_proj: Mat4::IDENTITY,
                camera_world: [0.0, 0.0, 2.0],
            },
            SurfaceHit {
                world_pos: Vec3::new(0.25, 0.5, 0.0),
                world_normal: Vec3::Z,
                uv: Vec2::new(0.5, 0.5),
                uv_edge_distance: 1.0,
                uv_paint_boundary_distance: 1.0,
                triangle_index: 0,
                material_index: 0.into(),
                mesh_id: MeshId(0),
                t: 2.0,
            },
        )
    }

    fn state_with_bent_surface_document() -> AppState {
        let mut state = state_with_surface_document();
        let angle = 60.0_f32.to_radians();
        let positions = vec![
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::new(0.0, angle.cos(), angle.sin()),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::NEG_X; positions.len()],
            vec![[0, 1, 2], [0, 1, 3]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 6,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0), MeshId(0)],
        )
        .unwrap();
        state.document.document = Some(Document::new(
            mesh,
            vec![MaterialSpec::new("Material", [8, 8])],
        ));
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        state
    }

    fn edge_pointer_sample() -> PointerSample {
        let mut sample = pointer_sample();
        let PointerSampleKind::Surface {
            view,
            hit: Some(mut hit),
        } = sample.kind
        else {
            unreachable!();
        };
        hit.world_pos = Vec3::new(0.5, 0.0, 0.0);
        sample.kind = PointerSampleKind::Surface {
            view,
            hit: Some(hit),
        };
        sample
    }

    #[test]
    fn pointer_down_starts_decal_session_with_image_aspect() {
        let mut state = state_with_surface_document();

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(pointer_sample())),
        );

        let transform = state.active_decal_transform().unwrap();
        assert!(
            transform
                .center_world
                .abs_diff_eq(Vec3::new(0.25, 0.5, 0.0), 1e-6)
        );
        assert!((transform.size_world.x / transform.size_world.y - 2.0).abs() < 1e-6);
        assert_eq!(state.status(), "Decal ready — Esc to cancel");
    }

    #[test]
    fn pointer_down_uses_geometric_normal_and_screen_horizontal_axis() {
        let mut state = state_with_surface_document();
        let mut sample = pointer_sample();
        let PointerSampleKind::Surface {
            view,
            hit: Some(mut hit),
        } = sample.kind
        else {
            unreachable!();
        };
        hit.world_normal = Vec3::new(0.25, 0.1, 1.0).normalize();
        sample.kind = PointerSampleKind::Surface {
            view,
            hit: Some(hit),
        };

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample)),
        );

        let transform = state.active_decal_transform().unwrap();
        assert!(transform.normal_world.abs_diff_eq(Vec3::Z, 1.0e-6));
        assert!(transform.axis_x_world.abs_diff_eq(Vec3::X, 1.0e-6));
    }

    #[test]
    fn smooth_mode_places_decal_with_average_geometric_normal() {
        let mut state = state_with_bent_surface_document();
        state.tool.decal_options.normal_mode = DecalNormalMode::Smooth;
        let expected = (Vec3::Z
            + Vec3::new(
                0.0,
                -60.0_f32.to_radians().sin(),
                60.0_f32.to_radians().cos(),
            ))
        .normalize();

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(edge_pointer_sample())),
        );

        assert!(
            state
                .active_decal_transform()
                .unwrap()
                .normal_world
                .abs_diff_eq(expected, 1.0e-5)
        );
    }

    #[test]
    fn changing_normal_options_reorients_active_decal_without_resizing() {
        let mut state = state_with_bent_surface_document();
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(edge_pointer_sample())),
        );
        let before = state.active_decal_transform().unwrap();

        let _ = reduce(
            &mut state,
            Command::UpdateDecalToolOptions(DecalToolOptions {
                normal_mode: DecalNormalMode::Smooth,
                smooth_angle_degrees: 60.0,
                ..DecalToolOptions::default()
            }),
        );

        let after = state.active_decal_transform().unwrap();
        assert_eq!(after.center_world, before.center_world);
        assert_eq!(after.size_world, before.size_world);
        assert_eq!(after.projection_depth_world, before.projection_depth_world);
        assert!(!after.normal_world.abs_diff_eq(before.normal_world, 1.0e-5));
    }

    #[test]
    fn pointer_down_without_image_does_not_start_session() {
        let mut state = state_with_surface_document();
        state.tool.decal_image = None;

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(pointer_sample())),
        );

        assert!(!state.has_active_decal_session());
        assert_eq!(state.status(), "Select an image to place a decal");
    }

    #[test]
    fn apply_decal_emits_renderer_commit_and_ends_session() {
        let mut state = state_with_surface_document();
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(pointer_sample())),
        );

        let plan = reduce(&mut state, Command::ApplyActiveDecal);

        assert!(!state.has_active_decal_session());
        assert_eq!(state.status(), "Surface decal applied");
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [crate::renderer::EditCommand::Apply(
                crate::renderer::ApplyCommand::Decal { .. }
            )]
        ));
        assert_eq!(
            plan.commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.len()),
            Some(1)
        );
    }

    #[test]
    fn escape_cancels_decal_without_document_changes() {
        let mut state = state_with_surface_document();
        let layer_tree_before = state.document().unwrap().layer_tree.clone();
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(pointer_sample())),
        );

        let plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::Cancel {
                reason: ToolCancelReason::Escape,
            }),
        );

        assert!(!state.has_active_decal_session());
        assert_eq!(state.document().unwrap().layer_tree, layer_tree_before);
        assert!(plan.edit_commands.is_empty());
        assert_eq!(state.status(), "Decal cancelled");
    }

    #[test]
    fn corner_scale_preserves_aspect_without_shift() {
        let base = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world: Vec2::new(2.0, 1.0),
            projection_depth_world: 2.0,
        };

        let scaled = super::scale_decal_transform(
            base,
            crate::core::transform::TransformHandle::ScaleBottomRight,
            Vec2::new(2.0, 0.75),
            false,
            true,
            0.01,
        )
        .unwrap();

        assert!((scaled.size_world.x / scaled.size_world.y - 2.0).abs() < 1e-6);
        assert!(scaled.size_world.x > base.size_world.x);
    }

    #[test]
    fn shift_corner_scale_allows_independent_axes() {
        let base = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world: Vec2::new(2.0, 1.0),
            projection_depth_world: 2.0,
        };

        let scaled = super::scale_decal_transform(
            base,
            crate::core::transform::TransformHandle::ScaleBottomRight,
            Vec2::new(2.0, 0.75),
            false,
            false,
            0.01,
        )
        .unwrap();

        assert!((scaled.size_world.x - 3.0).abs() < 1e-6);
        assert!((scaled.size_world.y - 1.25).abs() < 1e-6);
    }

    #[test]
    fn pointer_capture_loss_ends_only_the_decal_gesture() {
        let mut state = state_with_surface_document();
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(pointer_sample())),
        );
        let session = state.tool.decal_session_mut().unwrap();
        let base_transform = session.transform;
        session.gesture = Some(crate::application::state::DecalGesture {
            handle: crate::core::transform::TransformHandle::Move,
            pointer_start_px: Vec2::ZERO,
            base_transform,
            grab_local: Vec2::ZERO,
        });
        assert!(state.is_tool_pointer_gesture_active());

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::Cancel {
                reason: ToolCancelReason::PointerCaptureLost,
            }),
        );

        assert!(state.has_active_decal_session());
        assert_eq!(state.active_decal_handle(), None);
        assert_eq!(state.status(), "Decal ready — Esc to cancel");
    }

    #[test]
    fn view_projection_decal_starts_centered_and_resizes_with_viewport() {
        let mut state = state_with_surface_document();
        state.set_active_tool(ToolId::ViewProjectionDecal);

        let _ = reduce(
            &mut state,
            Command::BeginViewProjectionDecal {
                viewport_size: [200, 100],
            },
        );
        let initial = state.active_view_projection_decal_transform().unwrap();
        assert_eq!(initial.center_uv, Vec2::splat(0.5));
        assert!(initial.size_px.abs_diff_eq(Vec2::new(100.0, 50.0), 1.0e-6));

        let _ = reduce(
            &mut state,
            Command::SyncViewProjectionDecalViewport {
                viewport_size: [400, 200],
            },
        );
        let resized = state.active_view_projection_decal_transform().unwrap();
        assert!(resized.size_px.abs_diff_eq(Vec2::new(200.0, 100.0), 1.0e-6));
    }

    #[test]
    fn applying_view_projection_decal_emits_projective_renderer_command() {
        let mut state = state_with_surface_document();
        state.set_active_tool(ToolId::ViewProjectionDecal);
        let _ = reduce(
            &mut state,
            Command::BeginViewProjectionDecal {
                viewport_size: [100, 100],
            },
        );

        let plan = reduce(&mut state, Command::ApplyActiveDecal);

        assert!(!state.has_active_decal_session());
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [crate::renderer::EditCommand::Apply(
                crate::renderer::ApplyCommand::Decal {
                    projection: crate::core::decal::DecalProjection::ViewProjection { .. },
                    ..
                }
            )]
        ));
        assert_eq!(state.status(), "View projection decal applied");
    }
}
