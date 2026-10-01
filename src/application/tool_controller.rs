use crate::core::{
    stroke::StrokeSpace,
    tool::{SelectionShapeKind, ShapePaintMode, ShapeToolKind, ToolBehavior, ToolId},
};
use anyhow::Result;

use super::{
    ReducerOutput, RendererPlanBuilder, ToolInputEvent,
    command::{PointerSample, PointerSampleKind},
    decal_controller, fill_controller,
    selection_controller::{lasso_selection_output, rectangle_selection_output},
    shape_controller::{
        lasso_erase_plan_for_target, lasso_paint_plan_for_target, rectangle_erase_plan_for_target,
        rectangle_paint_plan_for_target, viewport_lasso_erase_plan_for_target,
        viewport_lasso_paint_plan_for_target, viewport_rectangle_erase_plan_for_target,
        viewport_rectangle_paint_plan_for_target,
    },
    state::{
        AppState, CancelledToolSession, FinishedLassoDrag, FinishedRectDrag, PaintTargetScope,
    },
    stroke_controller, transform_controller,
};

const MIN_RECT_DRAG_DISTANCE_UV_SQUARED: f32 = 0.000001;
const MIN_RECT_DRAG_DISTANCE_SURFACE_SQUARED: f32 = 9.0;

pub(crate) fn handle_tool_input_into_with_scratch(
    state: &mut AppState,
    event: ToolInputEvent,
    scratch: &mut stroke_controller::StrokeSampleScratch,
) -> Result<ReducerOutput> {
    if let ToolInputEvent::Cancel { reason } = &event {
        if state.tool.transform_session().is_some()
            || state.tool.embedded_image_transform_session().is_some()
        {
            scratch.clear();
            return Ok(transform_controller::handle_transform_cancel(
                state, *reason,
            ));
        }
        if state.tool.decal_session().is_some()
            || state.tool.view_projection_decal_session().is_some()
        {
            scratch.clear();
            return Ok(decal_controller::cancel_active_decal(state, *reason));
        }
        let output = match state.tool.cancel_session() {
            CancelledToolSession::Stroke => {
                let mut output = ReducerOutput::default();
                output.push_edit_command(crate::renderer::EditCommand::Stroke(
                    crate::renderer::StrokeCommand::Cancel,
                ));
                output
            }
            CancelledToolSession::Fill => {
                let mut output = ReducerOutput::default();
                output.push_edit_command(crate::renderer::EditCommand::Apply(
                    crate::renderer::ApplyCommand::FillStroke(
                        crate::renderer::FillStrokeCommand::Cancel,
                    ),
                ));
                output
            }
            CancelledToolSession::None | CancelledToolSession::Other => ReducerOutput::default(),
        };
        scratch.clear();
        return Ok(output);
    }
    let Some(tool) = state.effective_tool_definition().cloned() else {
        return Ok(ReducerOutput::default());
    };
    let sample = event_sample(&event);
    if !tool.supports_space(sample.space()) {
        return Ok(ReducerOutput::default());
    }

    match tool.behavior {
        ToolBehavior::NoOp => Ok(ReducerOutput::default()),
        ToolBehavior::Stroke { .. } => handle_stroke_input(state, event, scratch),
        ToolBehavior::Shape {
            shape: ShapeToolKind::Rectangle,
            paint_mode,
        } => handle_rectangle_shape_input(state, tool.id, paint_mode, event),
        ToolBehavior::Shape {
            shape: ShapeToolKind::Lasso,
            paint_mode,
        } => handle_lasso_shape_input(state, tool.id, paint_mode, event),
        ToolBehavior::Selection {
            shape: SelectionShapeKind::Rectangle,
        } => Ok(handle_rectangle_selection_input(state, event)),
        ToolBehavior::Selection {
            shape: SelectionShapeKind::Lasso,
        } => Ok(handle_lasso_selection_input(state, event)),
        ToolBehavior::Fill { scope } => {
            fill_controller::handle_fill_input(state, scope, event, scratch)
        }
        ToolBehavior::Decal { kind } => Ok(match kind {
            crate::core::tool::DecalKind::Surface => {
                decal_controller::handle_decal_input(state, event)
            }
            crate::core::tool::DecalKind::ViewProjection => {
                decal_controller::handle_view_projection_decal_input(state, event)
            }
        }),
        ToolBehavior::Transform => transform_controller::handle_transform_input(state, event),
        ToolBehavior::ColorPicker => Ok(ReducerOutput::default()),
    }
}

fn handle_stroke_input(
    state: &mut AppState,
    event: ToolInputEvent,
    scratch: &mut stroke_controller::StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let mut renderer_plan = RendererPlanBuilder::default();
    let stroke_output = stroke_controller::handle_tool_input_into_with_scratch(
        state,
        event,
        &mut renderer_plan,
        scratch,
    )?;
    let history_transaction = match stroke_output.history_capture {
        Some(capture) => capture.capture(state)?,
        None => None,
    };
    Ok(ReducerOutput::with_renderer_plan(renderer_plan)
        .with_optional_renderer_history_transaction("Brush Stroke", history_transaction)
        .with_renderer_commit(stroke_output.renderer_commit))
}

fn handle_rectangle_shape_input(
    state: &mut AppState,
    tool_id: ToolId,
    paint_mode: ShapePaintMode,
    event: ToolInputEvent,
) -> Result<ReducerOutput> {
    match event {
        ToolInputEvent::PointerDown(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, view } => {
                if state
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                    .into_allowed()
                    .is_none()
                {
                    return Ok(ReducerOutput::default());
                }
                state
                    .tool
                    .begin_shape_drag(tool_id, uv, view.gesture_view_size());
            }
            PointerSampleKind::Surface { view, .. } => {
                if state
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                    .into_allowed()
                    .is_none()
                {
                    return Ok(ReducerOutput::default());
                }
                state
                    .tool
                    .begin_surface_shape_drag(tool_id, sample.screen_px(), view);
            }
        },
        ToolInputEvent::PointerMove(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, .. } => {
                state.tool.update_shape_drag(tool_id, StrokeSpace::Uv, uv);
            }
            PointerSampleKind::Surface { .. } => {
                state
                    .tool
                    .update_shape_drag(tool_id, StrokeSpace::Surface, sample.screen_px());
            }
        },
        ToolInputEvent::PointerUp(sample) => {
            let drag = match sample.kind {
                PointerSampleKind::Uv { uv, .. } => {
                    state.tool.finish_shape_drag(tool_id, StrokeSpace::Uv, uv)
                }
                PointerSampleKind::Surface { .. } => {
                    state
                        .tool
                        .finish_shape_drag(tool_id, StrokeSpace::Surface, sample.screen_px())
                }
            };
            let Some(drag) = drag else {
                return Ok(ReducerOutput::default());
            };
            if drag.distance_squared() <= rect_drag_threshold_squared(drag) {
                return Ok(ReducerOutput::default());
            }
            let plan = rectangle_shape_plan(state, paint_mode, drag);
            if let Some(plan) = plan {
                let history_label = plan.history_label();
                let renderer_commit = plan.renderer_commit();
                let (command, history_transaction) = plan.into_edit_command_and_history(state)?;
                let mut output = ReducerOutput::default()
                    .with_optional_renderer_history_transaction(history_label, history_transaction)
                    .with_renderer_commit(renderer_commit);
                output.push_edit_command(command);
                return Ok(output);
            }
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
    Ok(ReducerOutput::default())
}

fn handle_lasso_shape_input(
    state: &mut AppState,
    tool_id: ToolId,
    paint_mode: ShapePaintMode,
    event: ToolInputEvent,
) -> Result<ReducerOutput> {
    match event {
        ToolInputEvent::PointerDown(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, view } => {
                if state
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                    .into_allowed()
                    .is_none()
                {
                    return Ok(ReducerOutput::default());
                }
                state
                    .tool
                    .begin_lasso_shape_drag(tool_id, uv, view.gesture_view_size());
            }
            PointerSampleKind::Surface { view, .. } => {
                if state
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                    .into_allowed()
                    .is_none()
                {
                    return Ok(ReducerOutput::default());
                }
                state
                    .tool
                    .begin_surface_lasso_shape_drag(tool_id, sample.screen_px(), view);
            }
        },
        ToolInputEvent::PointerMove(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, .. } => {
                state.tool.update_shape_drag(tool_id, StrokeSpace::Uv, uv);
            }
            PointerSampleKind::Surface { .. } => {
                state
                    .tool
                    .update_shape_drag(tool_id, StrokeSpace::Surface, sample.screen_px());
            }
        },
        ToolInputEvent::PointerUp(sample) => {
            let drag = match sample.kind {
                PointerSampleKind::Uv { uv, .. } => {
                    state
                        .tool
                        .finish_lasso_shape_drag(tool_id, StrokeSpace::Uv, uv)
                }
                PointerSampleKind::Surface { .. } => state.tool.finish_lasso_shape_drag(
                    tool_id,
                    StrokeSpace::Surface,
                    sample.screen_px(),
                ),
            };
            let Some(drag) = drag.filter(valid_lasso_drag) else {
                return Ok(ReducerOutput::default());
            };
            let Some(plan) = lasso_shape_plan(state, paint_mode, drag) else {
                return Ok(ReducerOutput::default());
            };
            let history_label = plan.history_label();
            let renderer_commit = plan.renderer_commit();
            let (command, history_transaction) = plan.into_edit_command_and_history(state)?;
            let mut output = ReducerOutput::default()
                .with_optional_renderer_history_transaction(history_label, history_transaction)
                .with_renderer_commit(renderer_commit);
            output.push_edit_command(command);
            return Ok(output);
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
    Ok(ReducerOutput::default())
}

fn lasso_shape_plan(
    state: &AppState,
    paint_mode: ShapePaintMode,
    drag: FinishedLassoDrag,
) -> Option<super::ApplyOneShotRenderPlan> {
    let document = state.document()?;
    match drag {
        FinishedLassoDrag::Uv { points_uv, .. } => {
            let focused_material_index = state.document.focused_material_index();
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .and_then(|paint_target| match paint_mode {
                    ShapePaintMode::Paint => lasso_paint_plan_for_target(
                        document,
                        paint_target,
                        focused_material_index,
                        points_uv,
                        state.current_color(),
                        state.shape_options().opacity,
                    ),
                    ShapePaintMode::Erase => lasso_erase_plan_for_target(
                        document,
                        paint_target,
                        focused_material_index,
                        points_uv,
                        state.shape_options().opacity,
                    ),
                })
        }
        FinishedLassoDrag::Surface { points_px, view } => state
            .document
            .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
            .into_allowed()
            .and_then(|paint_target| match paint_mode {
                ShapePaintMode::Paint => viewport_lasso_paint_plan_for_target(
                    document,
                    paint_target,
                    points_px,
                    view,
                    enabled_surface_mirror_x_plane(state),
                    state.viewport_scene_visibility(),
                    state.current_color(),
                    state.shape_options().opacity,
                ),
                ShapePaintMode::Erase => viewport_lasso_erase_plan_for_target(
                    document,
                    paint_target,
                    points_px,
                    view,
                    enabled_surface_mirror_x_plane(state),
                    state.viewport_scene_visibility(),
                    state.shape_options().opacity,
                ),
            }),
    }
}

fn rectangle_shape_plan(
    state: &AppState,
    paint_mode: ShapePaintMode,
    drag: FinishedRectDrag,
) -> Option<super::ApplyOneShotRenderPlan> {
    let document = state.document()?;
    match drag {
        FinishedRectDrag::Uv { start, end } => {
            let min_uv = start.min(end);
            let max_uv = start.max(end);
            let focused_material_index = state.document.focused_material_index();
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .and_then(|paint_target| match paint_mode {
                    ShapePaintMode::Paint => rectangle_paint_plan_for_target(
                        document,
                        paint_target,
                        focused_material_index,
                        min_uv,
                        max_uv,
                        state.current_color(),
                        state.shape_options().opacity,
                    ),
                    ShapePaintMode::Erase => rectangle_erase_plan_for_target(
                        document,
                        paint_target,
                        focused_material_index,
                        min_uv,
                        max_uv,
                        state.shape_options().opacity,
                    ),
                })
        }
        FinishedRectDrag::Surface {
            start_px,
            end_px,
            view,
        } => state
            .document
            .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
            .into_allowed()
            .and_then(|paint_target| match paint_mode {
                ShapePaintMode::Paint => viewport_rectangle_paint_plan_for_target(
                    document,
                    paint_target,
                    start_px,
                    end_px,
                    view,
                    enabled_surface_mirror_x_plane(state),
                    state.viewport_scene_visibility(),
                    state.current_color(),
                    state.shape_options().opacity,
                ),
                ShapePaintMode::Erase => viewport_rectangle_erase_plan_for_target(
                    document,
                    paint_target,
                    start_px,
                    end_px,
                    view,
                    enabled_surface_mirror_x_plane(state),
                    state.viewport_scene_visibility(),
                    state.shape_options().opacity,
                ),
            }),
    }
}

fn enabled_surface_mirror_x_plane(state: &AppState) -> Option<f32> {
    let options = state.surface_mirror_options();
    options.x_enabled.then_some(options.x_plane)
}

fn handle_rectangle_selection_input(state: &mut AppState, event: ToolInputEvent) -> ReducerOutput {
    match event {
        ToolInputEvent::PointerDown(sample) => {
            let operation = state.selection_options().operation;
            match sample.kind {
                PointerSampleKind::Uv { uv, view } => state.tool.begin_selection_drag(
                    SelectionShapeKind::Rectangle,
                    operation,
                    uv,
                    view.gesture_view_size(),
                ),
                PointerSampleKind::Surface { view, .. } => state.tool.begin_surface_selection_drag(
                    SelectionShapeKind::Rectangle,
                    operation,
                    sample.screen_px(),
                    view,
                ),
            }
        }
        ToolInputEvent::PointerMove(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, .. } => {
                state.tool.update_selection_drag(StrokeSpace::Uv, uv);
            }
            PointerSampleKind::Surface { .. } => {
                state
                    .tool
                    .update_selection_drag(StrokeSpace::Surface, sample.screen_px());
            }
        },
        ToolInputEvent::PointerUp(sample) => {
            let finished = match sample.kind {
                PointerSampleKind::Uv { uv, .. } => {
                    state.tool.finish_selection_drag(StrokeSpace::Uv, uv)
                }
                PointerSampleKind::Surface { .. } => state
                    .tool
                    .finish_selection_drag(StrokeSpace::Surface, sample.screen_px()),
            };
            let Some((operation, drag)) = finished else {
                return ReducerOutput::default();
            };
            if drag.distance_squared() <= rect_drag_threshold_squared(drag) {
                return ReducerOutput::default();
            }
            let output = match drag {
                FinishedRectDrag::Uv { start, end } => {
                    rectangle_selection_output(state, start.min(end), start.max(end), operation)
                }
                FinishedRectDrag::Surface {
                    start_px,
                    end_px,
                    view,
                } => super::selection_controller::viewport_rectangle_selection_output(
                    state, start_px, end_px, view, operation,
                ),
            };
            state.set_status_key(super::selection_status_key(operation));
            return output;
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
    ReducerOutput::default()
}

fn handle_lasso_selection_input(state: &mut AppState, event: ToolInputEvent) -> ReducerOutput {
    match event {
        ToolInputEvent::PointerDown(sample) => {
            let operation = state.selection_options().operation;
            match sample.kind {
                PointerSampleKind::Uv { uv, view } => state.tool.begin_selection_drag(
                    SelectionShapeKind::Lasso,
                    operation,
                    uv,
                    view.gesture_view_size(),
                ),
                PointerSampleKind::Surface { view, .. } => state.tool.begin_surface_selection_drag(
                    SelectionShapeKind::Lasso,
                    operation,
                    sample.screen_px(),
                    view,
                ),
            }
        }
        ToolInputEvent::PointerMove(sample) => match sample.kind {
            PointerSampleKind::Uv { uv, .. } => {
                state.tool.update_selection_drag(StrokeSpace::Uv, uv);
            }
            PointerSampleKind::Surface { .. } => {
                state
                    .tool
                    .update_selection_drag(StrokeSpace::Surface, sample.screen_px());
            }
        },
        ToolInputEvent::PointerUp(sample) => {
            let finished = match sample.kind {
                PointerSampleKind::Uv { uv, .. } => {
                    state.tool.finish_lasso_selection_drag(StrokeSpace::Uv, uv)
                }
                PointerSampleKind::Surface { .. } => state
                    .tool
                    .finish_lasso_selection_drag(StrokeSpace::Surface, sample.screen_px()),
            };
            let Some((operation, drag)) = finished else {
                return ReducerOutput::default();
            };
            if !valid_lasso_drag(&drag) {
                return ReducerOutput::default();
            }
            let output = match drag {
                FinishedLassoDrag::Uv { points_uv, .. } => {
                    lasso_selection_output(state, points_uv, operation)
                }
                FinishedLassoDrag::Surface { points_px, view } => {
                    super::selection_controller::viewport_lasso_selection_output(
                        state, points_px, view, operation,
                    )
                }
            };
            state.set_status_key(super::selection_status_key(operation));
            return output;
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
    ReducerOutput::default()
}

fn valid_lasso_drag(drag: &FinishedLassoDrag) -> bool {
    let (points, scale) = match drag {
        FinishedLassoDrag::Uv {
            points_uv,
            view_size,
        } => (
            points_uv.as_slice(),
            glam::Vec2::new(view_size[0].max(1) as f32, view_size[1].max(1) as f32),
        ),
        FinishedLassoDrag::Surface { points_px, .. } => (points_px.as_slice(), glam::Vec2::ONE),
    };
    if points.len() < 3 {
        return false;
    }
    let min = points
        .iter()
        .copied()
        .fold(glam::Vec2::splat(f32::INFINITY), glam::Vec2::min);
    let max = points
        .iter()
        .copied()
        .fold(glam::Vec2::splat(f32::NEG_INFINITY), glam::Vec2::max);
    let size_px = (max - min) * scale;
    size_px.x >= 2.0 && size_px.y >= 2.0
}

fn rect_drag_threshold_squared(drag: FinishedRectDrag) -> f32 {
    match drag {
        FinishedRectDrag::Uv { .. } => MIN_RECT_DRAG_DISTANCE_UV_SQUARED,
        FinishedRectDrag::Surface { .. } => MIN_RECT_DRAG_DISTANCE_SURFACE_SQUARED,
    }
}

fn event_sample(event: &ToolInputEvent) -> &PointerSample {
    match event {
        ToolInputEvent::PointerDown(sample)
        | ToolInputEvent::PointerMove(sample)
        | ToolInputEvent::PointerUp(sample) => sample,
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before sample dispatch"),
    }
}
