use crate::{
    application::{
        command::{ToolCancelReason, ToolInputEvent},
        fill_controller,
        selection_controller::{deselect_selection_output, rectangle_selection_output},
        shape_controller::{rectangle_erase_plan_for_target, rectangle_paint_plan_for_target},
        tool_controller::handle_tool_input_into_with_scratch,
    },
    core::{
        composite::SelectionCompositeMode,
        decal::{DecalImageAsset, DecalToolOptions},
        document::MeshId,
        stroke_preset::StrokeToolPreset,
        tool::{
            ColorPickerToolOptions, FillToolOptions, SelectionToolOptions, ShapeToolOptions, ToolId,
        },
    },
};
use anyhow::Result;
use glam::Vec2;
use std::sync::Arc;

use super::{
    ApplyOneShotRenderPlan, InputHoldToken, ReducerOutput, decal_controller,
    state::{AppState, PaintTargetScope, ToolState, ViewState},
    transform_controller,
};

pub fn set_active_tool(
    state: &mut AppState,
    tool_id: ToolId,
    _scratch: &mut crate::application::stroke_controller::StrokeSampleScratch,
) -> Result<ReducerOutput> {
    if state.active_layer_target() == crate::core::document::ActiveLayerTarget::EmbeddedImage {
        return Ok(ReducerOutput::default());
    }
    if state.tool.active_tool_id() == tool_id
        || state.tool.has_active_modal_tool()
        || !state.tool.momentary_tool_overrides.is_empty()
    {
        return Ok(ReducerOutput::default());
    }
    let has_decal_session = state.has_active_decal_session();
    if state.tool.is_interacting() && !has_decal_session {
        return Ok(ReducerOutput::default());
    }
    let output = if has_decal_session {
        decal_controller::cancel_active_decal(state, ToolCancelReason::ToolChanged)
    } else {
        ReducerOutput::default()
    };
    state.tool.set_active_tool(tool_id);
    state.tool.view_projection_decal_start_requested =
        tool_id == ToolId::ViewProjectionDecal && state.decal_image().is_some();
    if tool_id == ToolId::SurfaceDecal {
        state.set_status_key(if state.decal_image().is_some() {
            "status-decal-click-surface"
        } else {
            "status-decal-select-image"
        });
    } else if tool_id == ToolId::ViewProjectionDecal {
        state.set_status_key(if state.decal_image().is_some() {
            "status-view-decal-preparing"
        } else {
            "status-view-decal-select-image"
        });
    }
    Ok(output)
}

pub fn begin_momentary_tool(
    state: &mut AppState,
    token: InputHoldToken,
    tool_id: ToolId,
) -> ReducerOutput {
    if state.active_layer_target() != crate::core::document::ActiveLayerTarget::EmbeddedImage {
        state.tool.begin_momentary_tool(token, tool_id);
    }
    ReducerOutput::default()
}

pub fn complete_momentary_tool(
    state: &mut AppState,
    token: InputHoldToken,
    tool_id: ToolId,
    select_tool: bool,
) -> ReducerOutput {
    let select_tool = select_tool
        && state.active_layer_target() != crate::core::document::ActiveLayerTarget::EmbeddedImage;
    state
        .tool
        .complete_momentary_tool(token, tool_id, select_tool);
    ReducerOutput::default()
}

pub fn begin_transform_mode(state: &mut AppState) -> ReducerOutput {
    if state.active_layer_target() == crate::core::document::ActiveLayerTarget::EmbeddedImage {
        return ReducerOutput::default();
    }
    if state.document().is_some() {
        state.tool.begin_transform_mode();
        if state.tool.has_active_modal_tool() {
            state.set_status_key("status-transform-mode-ready");
        }
    }
    ReducerOutput::default()
}

pub fn apply_active_transform(state: &mut AppState) -> Result<ReducerOutput> {
    let output = transform_controller::apply_active_transform(state)?;
    state.tool.clear_modal_tool();
    Ok(output)
}

pub fn apply_active_decal(state: &mut AppState) -> Result<ReducerOutput> {
    decal_controller::apply_active_decal(state)
}

pub fn cancel_active_transform(state: &mut AppState) -> ReducerOutput {
    let output = transform_controller::cancel_active_transform(state);
    let was_modal = state.tool.has_active_modal_tool();
    state.tool.clear_modal_tool();
    if was_modal && !output.has_renderer_work() {
        state.set_status_key("status-transform-cancelled");
    }
    output
}

pub fn begin_transient_tool_override(
    tool: &mut ToolState,
    token: InputHoldToken,
    id: String,
) -> ReducerOutput {
    tool.begin_transient_tool_override(token, id);
    ReducerOutput::default()
}

pub fn end_transient_tool_override(tool: &mut ToolState, token: InputHoldToken) -> ReducerOutput {
    tool.end_transient_tool_override(token);
    ReducerOutput::default()
}

pub fn set_active_tool_preset_index(tool: &mut ToolState, index: usize) -> ReducerOutput {
    if !tool.is_interacting() && tool.active_stroke_preset_index() != Some(index) {
        tool.set_active_tool_preset_index(index);
    }
    ReducerOutput::default()
}

pub fn update_active_tool_preset(
    tool: &mut ToolState,
    tool_preset: StrokeToolPreset,
) -> Result<ReducerOutput> {
    if let Some(index) = tool.active_stroke_preset_index() {
        tool.update_runtime_stroke_preset(index, tool_preset)?;
    }
    Ok(ReducerOutput::default())
}

pub fn update_runtime_stroke_preset(
    tool: &mut ToolState,
    preset_index: usize,
    tool_preset: StrokeToolPreset,
) -> Result<ReducerOutput> {
    tool.update_runtime_stroke_preset(preset_index, tool_preset)?;
    Ok(ReducerOutput::default())
}

pub fn set_runtime_brush_engine(
    tool: &mut ToolState,
    preset_index: usize,
    preset_id: &str,
    engine_id: &str,
) -> Result<ReducerOutput> {
    tool.set_runtime_brush_engine(preset_index, preset_id, engine_id)?;
    Ok(ReducerOutput::default())
}

pub fn reset_brush_preset(
    tool: &mut ToolState,
    preset_index: usize,
    preset_id: &str,
) -> Result<ReducerOutput> {
    tool.reset_brush_preset(preset_index, preset_id)?;
    Ok(ReducerOutput::default())
}

pub fn create_brush_preset(
    tool: &mut ToolState,
    group_id: &str,
    source_preset_id: &str,
    new_preset_id: &str,
    preset: StrokeToolPreset,
) -> Result<ReducerOutput> {
    tool.create_brush_preset(group_id, source_preset_id, new_preset_id, preset)?;
    Ok(ReducerOutput::default())
}

pub fn delete_brush_preset(tool: &mut ToolState, preset_id: &str) -> Result<ReducerOutput> {
    tool.delete_brush_preset(preset_id)?;
    Ok(ReducerOutput::default())
}

pub fn move_tool_group(
    tool: &mut ToolState,
    from_index: usize,
    to_index: usize,
) -> Result<ReducerOutput> {
    tool.move_tool_group(from_index, to_index)?;
    Ok(ReducerOutput::default())
}

pub fn move_tool_entry(
    tool: &mut ToolState,
    group_id: &str,
    from_index: usize,
    to_index: usize,
) -> Result<ReducerOutput> {
    tool.move_tool_entry(group_id, from_index, to_index)?;
    Ok(ReducerOutput::default())
}

pub fn set_decal_image(state: &mut AppState, image: Arc<DecalImageAsset>) -> ReducerOutput {
    let image_changed = state
        .decal_image()
        .is_none_or(|current| current.id != image.id);
    if image_changed && state.has_active_decal_session() {
        let _ = decal_controller::cancel_active_decal(state, ToolCancelReason::ToolChanged);
    }
    state.tool.decal_image = Some(image);
    if state.effective_tool_id() == ToolId::SurfaceDecal {
        state.set_status_key("status-decal-click-surface");
    } else if state.effective_tool_id() == ToolId::ViewProjectionDecal {
        state.tool.view_projection_decal_start_requested = true;
        state.set_status_key("status-view-decal-preparing");
    }
    ReducerOutput::default()
}

pub fn clear_decal_image(state: &mut AppState) -> ReducerOutput {
    if state.has_active_decal_session() {
        let _ = decal_controller::cancel_active_decal(state, ToolCancelReason::ToolChanged);
    }
    state.tool.decal_image = None;
    state.tool.view_projection_decal_start_requested = false;
    if state.effective_tool_id() == ToolId::SurfaceDecal {
        state.set_status_key("status-decal-select-image");
    }
    ReducerOutput::default()
}

pub fn cancel_active_decal(state: &mut AppState) -> ReducerOutput {
    decal_controller::cancel_active_decal(state, ToolCancelReason::Escape)
}

pub fn update_decal_tool_options(
    state: &mut AppState,
    mut options: DecalToolOptions,
) -> ReducerOutput {
    options.opacity = if options.opacity.is_finite() {
        options.opacity.clamp(0.0, 1.0)
    } else {
        state.tool.decal_options.opacity
    };
    options.smooth_angle_degrees = if options.smooth_angle_degrees.is_finite() {
        options.smooth_angle_degrees.clamp(0.0, 180.0)
    } else {
        state.tool.decal_options.smooth_angle_degrees
    };
    let refresh_normal = decal_options_require_normal_refresh(&state.tool.decal_options, &options);
    state.tool.decal_options = options;
    if refresh_normal {
        decal_controller::refresh_active_decal_normal(state);
    }
    ReducerOutput::default()
}

fn decal_options_require_normal_refresh(
    previous: &DecalToolOptions,
    next: &DecalToolOptions,
) -> bool {
    previous.normal_mode != next.normal_mode
        || previous.smooth_angle_degrees != next.smooth_angle_degrees
}

pub fn update_fill_tool_options(tool: &mut ToolState, options: FillToolOptions) -> ReducerOutput {
    tool.fill_options = options;
    ReducerOutput::default()
}

pub fn update_shape_tool_options(tool: &mut ToolState, options: ShapeToolOptions) -> ReducerOutput {
    tool.shape_options = options;
    ReducerOutput::default()
}

pub fn update_selection_tool_options(
    tool: &mut ToolState,
    options: SelectionToolOptions,
) -> ReducerOutput {
    tool.selection_options = options;
    ReducerOutput::default()
}

pub fn update_color_picker_tool_options(
    tool: &mut ToolState,
    options: ColorPickerToolOptions,
) -> ReducerOutput {
    tool.color_picker_options = options;
    ReducerOutput::default()
}

pub fn set_surface_mirror_x_enabled(state: &mut AppState, enabled: bool) -> ReducerOutput {
    if state.tool.is_idle() {
        state.view.surface_mirror_x_enabled = enabled;
    }
    ReducerOutput::default()
}

pub fn set_surface_mirror_x_plane(state: &mut AppState, plane_x: f32) -> ReducerOutput {
    if state.tool.is_idle() && plane_x.is_finite() {
        state.view.surface_mirror_x_plane = plane_x;
    }
    ReducerOutput::default()
}

pub fn set_surface_mirror_x_plane_visible(view: &mut ViewState, visible: bool) -> ReducerOutput {
    view.surface_mirror_x_plane_visible = visible;
    ReducerOutput::default()
}

pub(crate) fn tool_input_into_with_scratch(
    state: &mut AppState,
    event: ToolInputEvent,
    scratch: &mut crate::application::stroke_controller::StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let output = handle_tool_input_into_with_scratch(state, event, scratch)?;
    state.tool.finish_pending_momentary_tools();
    Ok(output)
}

pub(crate) fn advance_active_stroke_into_with_scratch(
    state: &mut AppState,
    time_s: f64,
    scratch: &mut crate::application::stroke_controller::StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let mut renderer_plan = super::RendererPlanBuilder::default();
    crate::application::stroke_controller::advance_active_stroke_into(
        state,
        time_s,
        &mut renderer_plan,
        scratch,
    );
    Ok(ReducerOutput::with_renderer_plan(renderer_plan))
}

pub(crate) fn fill_material_into(
    state: &mut AppState,
    material_index: usize,
    opacity: f32,
) -> Result<ReducerOutput> {
    fill_controller::fill_once(
        state,
        super::state::FillTargetKey::Material {
            material_index: material_index.into(),
        },
        state.current_color(),
        opacity,
    )
}

pub(crate) fn fill_mesh_into(
    state: &mut AppState,
    mesh_id: MeshId,
    opacity: f32,
) -> Result<ReducerOutput> {
    fill_controller::fill_once(
        state,
        super::state::FillTargetKey::Mesh { mesh_id },
        state.current_color(),
        opacity,
    )
}

pub(crate) fn rectangle_paint_uv_into(
    state: &mut AppState,
    min_uv: Vec2,
    max_uv: Vec2,
    opacity: f32,
) -> Result<ReducerOutput> {
    let color = state.current_color();
    let focused_material_index = state.document.focused_material_index();
    let plan = state.document().and_then(|document| {
        state
            .document
            .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
            .into_allowed()
            .and_then(|paint_target| {
                rectangle_paint_plan_for_target(
                    document,
                    paint_target,
                    focused_material_index,
                    min_uv,
                    max_uv,
                    color,
                    opacity,
                )
            })
    });
    apply_one_shot_output(plan, state)
}

pub(crate) fn rectangle_erase_uv_into(
    state: &mut AppState,
    min_uv: Vec2,
    max_uv: Vec2,
    opacity: f32,
) -> Result<ReducerOutput> {
    let focused_material_index = state.document.focused_material_index();
    let plan = state.document().and_then(|document| {
        state
            .document
            .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
            .into_allowed()
            .and_then(|paint_target| {
                rectangle_erase_plan_for_target(
                    document,
                    paint_target,
                    focused_material_index,
                    min_uv,
                    max_uv,
                    opacity,
                )
            })
    });
    apply_one_shot_output(plan, state)
}

fn apply_one_shot_output(
    plan: Option<ApplyOneShotRenderPlan>,
    state: &AppState,
) -> Result<ReducerOutput> {
    let Some(plan) = plan else {
        return Ok(ReducerOutput::default());
    };
    let history_label = plan.history_label();
    let renderer_commit = plan.renderer_commit();
    let (command, history_transaction) = plan.into_edit_command_and_history(state)?;
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction(history_label, history_transaction)
        .with_renderer_commit(renderer_commit);
    output.push_edit_command(command);
    Ok(output)
}

pub(crate) fn rectangle_select_uv_into(
    state: &mut AppState,
    min_uv: Vec2,
    max_uv: Vec2,
    operation: SelectionCompositeMode,
) -> Result<ReducerOutput> {
    let output = rectangle_selection_output(state, min_uv, max_uv, operation);
    state.set_status_key(super::selection_status_key(operation));
    Ok(output)
}

pub(crate) fn deselect_selection_into(state: &mut AppState) -> Result<ReducerOutput> {
    let output = deselect_selection_output(state);
    if output.has_renderer_work() {
        state.set_status_key("status-selection-deselected");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{Mat3, Vec2, Vec3};

    use crate::{
        application::{
            AppState, ApplicationRuntime, Command, InputHoldToken, InputModifiers, PointerSample,
            ToolCancelReason, ToolInputEvent, reduce,
            state::{
                DecalSession, EditorDocumentState, ToolSession, TransformGesture, TransformSession,
                TransformSessionSource,
            },
            stroke_controller::StrokeSampleScratch,
        },
        core::{
            decal::{
                DecalImageAsset, DecalImageId, DecalNormalMode, DecalToolOptions, DecalTransform,
            },
            document::{Document, MaterialSpec, MeshData, MeshId, SurfaceHit},
            geometry::RectU32,
            stroke_preset::{StrokePresetOp, StrokeStrategy},
            surface::PaintSurfaceId,
            tool::{ToolBehavior, ToolId},
            tool_catalog::DEFAULT_BRUSH_PRESET_ID,
            transform::{TransformHandle, TransformScaleConstraint, UvTransform},
        },
        renderer::{EditCommand, TransformCommand},
    };

    fn state_with_document() -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![MaterialSpec::new("A", [8, 8])],
        ));
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        state
    }

    fn empty_builtin_brush_group(state: &mut AppState) {
        for preset_id in [
            "builtin.brush.paint.directional_flat",
            "builtin.brush.paint.round_soft",
        ] {
            let _ = reduce(
                state,
                Command::DeleteBrushPreset {
                    preset_id: preset_id.to_owned(),
                },
            );
        }
    }

    fn install_decal_session(state: &mut AppState) {
        let hit = SurfaceHit {
            world_pos: Vec3::ZERO,
            world_normal: Vec3::Z,
            uv: Vec2::ZERO,
            uv_edge_distance: 1.0,
            uv_paint_boundary_distance: 1.0,
            triangle_index: 0,
            material_index: 0.into(),
            mesh_id: MeshId(0),
            t: 1.0,
        };
        state.tool.set_session(ToolSession::Decal(DecalSession {
            transform: DecalTransform {
                center_world: Vec3::ZERO,
                axis_x_world: Vec3::X,
                axis_y_world: Vec3::Y,
                normal_world: Vec3::Z,
                size_world: Vec2::ONE,
                projection_depth_world: 1.0,
            },
            source_hit: hit,
            gesture: None,
            scene_visibility: Default::default(),
        }));
    }

    fn install_transform_session(state: &mut AppState, gesture_active: bool) {
        let (target, texture_size, selection_before) = {
            let document = state.document().unwrap();
            let layer_id = document.layer_tree.default_raster_layer().unwrap();
            let target = PaintSurfaceId::raster(0.into(), layer_id);
            (
                target,
                document.texture_size_for_surface(target).unwrap(),
                document.active_selection.clone(),
            )
        };
        let base_transform = UvTransform::identity();
        let transform = UvTransform::from_matrix(Mat3::from_translation(Vec2::X)).unwrap();
        state
            .tool
            .set_session(ToolSession::Transform(TransformSession {
                target,
                material_index: 0.into(),
                texture_size,
                source_bounds: RectU32::full(texture_size),
                source: TransformSessionSource::ExistingSurface,
                selection_before,
                transform,
                scale_sign: Vec2::ONE,
                scale_constraint: TransformScaleConstraint::Free,
                gesture: gesture_active.then_some(TransformGesture {
                    handle: TransformHandle::Move,
                    start_texture_px: Vec2::ZERO,
                    base_transform,
                    base_scale_sign: Vec2::ONE,
                }),
                last_preview_time_s: None,
                last_submitted_transform: None,
            }));
    }

    #[test]
    fn decal_image_and_options_are_stored_in_tool_state() {
        let mut state = AppState::default();
        let image = Arc::new(DecalImageAsset {
            id: DecalImageId(7),
            file_name: "logo.png".to_owned(),
            size: [1, 1],
            rgba8: Arc::<[u8]>::from([10, 20, 30, 255]),
        });

        let _ = reduce(&mut state, Command::SetDecalImage(image.clone()));
        assert_eq!(state.decal_image(), Some(&image));

        let _ = reduce(
            &mut state,
            Command::UpdateDecalToolOptions(DecalToolOptions {
                opacity: 1.5,
                ..DecalToolOptions::default()
            }),
        );
        assert_eq!(state.decal_options().opacity, 1.0);

        let _ = reduce(
            &mut state,
            Command::UpdateDecalToolOptions(DecalToolOptions {
                opacity: f32::NAN,
                smooth_angle_degrees: f32::NAN,
                ..DecalToolOptions::default()
            }),
        );
        assert_eq!(state.decal_options().opacity, 1.0);
        assert_eq!(state.decal_options().smooth_angle_degrees, 60.0);

        let _ = reduce(&mut state, Command::ClearDecalImage);
        assert!(state.decal_image().is_none());
    }

    #[test]
    fn decal_normal_refresh_ignores_opacity_only_changes() {
        let previous = DecalToolOptions::default();
        let opacity_only = DecalToolOptions {
            opacity: 0.25,
            ..previous.clone()
        };
        let normal_mode = DecalToolOptions {
            normal_mode: DecalNormalMode::Smooth,
            ..previous.clone()
        };
        let smooth_angle = DecalToolOptions {
            smooth_angle_degrees: 30.0,
            ..previous.clone()
        };

        assert!(!super::decal_options_require_normal_refresh(
            &previous,
            &opacity_only
        ));
        assert!(super::decal_options_require_normal_refresh(
            &previous,
            &normal_mode
        ));
        assert!(super::decal_options_require_normal_refresh(
            &previous,
            &smooth_angle
        ));
    }

    #[test]
    fn selecting_another_tool_cancels_an_active_decal_session() {
        let mut state = state_with_document();
        state.set_active_tool(ToolId::SurfaceDecal);
        install_decal_session(&mut state);

        let plan = reduce(
            &mut state,
            Command::SetActiveTool(ToolId::RectangleSelection),
        );

        assert_eq!(state.active_tool_id(), ToolId::RectangleSelection);
        assert!(!state.has_active_decal_session());
        assert!(plan.edit_commands.is_empty());
    }

    #[test]
    fn replacing_decal_image_cancels_an_active_session() {
        let mut state = state_with_document();
        state.set_active_tool(ToolId::SurfaceDecal);
        install_decal_session(&mut state);
        let replacement = Arc::new(DecalImageAsset {
            id: DecalImageId(9),
            file_name: "replacement.png".to_owned(),
            size: [1, 1],
            rgba8: Arc::<[u8]>::from([255, 255, 255, 255]),
        });

        let _ = reduce(&mut state, Command::SetDecalImage(replacement.clone()));

        assert!(!state.has_active_decal_session());
        assert_eq!(state.decal_image(), Some(&replacement));
        assert_eq!(state.status(), "Click a surface to place the decal");
    }

    #[test]
    fn selecting_another_tool_is_ignored_during_an_active_stroke() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(PointerSample::uv(
                Vec2::new(0.5, 0.5),
                InputModifiers::default(),
            ))),
        );
        assert!(state.tool.is_stroking());

        let plan = reduce(
            &mut state,
            Command::SetActiveTool(ToolId::RectangleSelection),
        );

        assert_eq!(state.active_tool_id(), selected_tool);
        assert!(state.tool.is_stroking());
        assert!(plan.edit_commands.is_empty());
    }

    #[test]
    fn selecting_another_tool_is_ignored_during_an_active_transform() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();
        install_transform_session(&mut state, false);

        let plan = reduce(
            &mut state,
            Command::SetActiveTool(ToolId::RectangleSelection),
        );

        assert_eq!(state.active_tool_id(), selected_tool);
        assert!(state.tool.transform_session().is_some());
        assert!(plan.edit_commands.is_empty());
    }

    #[test]
    fn momentary_tool_does_not_change_the_selected_tool() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();
        let token = InputHoldToken(1);

        let _ = reduce(
            &mut state,
            Command::BeginMomentaryTool {
                token,
                tool_id: ToolId::RectangleSelection,
            },
        );
        assert_eq!(state.active_tool_id(), selected_tool);
        assert_eq!(state.effective_tool_id(), ToolId::RectangleSelection);
        assert_eq!(state.panel_tool_id(), ToolId::RectangleSelection);

        let _ = reduce(
            &mut state,
            Command::CompleteMomentaryTool {
                token,
                tool_id: ToolId::RectangleSelection,
                select_tool: false,
            },
        );
        assert_eq!(state.active_tool_id(), selected_tool);
        assert_eq!(state.effective_tool_id(), selected_tool);
        assert_eq!(state.panel_tool_id(), selected_tool);
    }

    #[test]
    fn transform_mode_restores_the_selected_tool_after_cancel() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();

        let _ = reduce(&mut state, Command::BeginTransformMode);
        assert_eq!(state.active_tool_id(), selected_tool);
        assert_eq!(state.effective_tool_id(), ToolId::Transform);
        assert_eq!(state.panel_tool_id(), ToolId::Transform);

        let _ = reduce(&mut state, Command::CancelActiveTransform);
        assert_eq!(state.active_tool_id(), selected_tool);
        assert_eq!(state.effective_tool_id(), selected_tool);
        assert_eq!(state.panel_tool_id(), selected_tool);
    }

    #[test]
    fn brush_stroke_waits_for_gpu_finalization_before_reporting_project_change() {
        let mut state = state_with_document();
        let mut scratch = StrokeSampleScratch::default();
        let sample = PointerSample::uv(Vec2::new(0.5, 0.5), InputModifiers::default());

        let down = super::tool_input_into_with_scratch(
            &mut state,
            ToolInputEvent::PointerDown(sample),
            &mut scratch,
        )
        .unwrap();
        assert!(!down.project_changed());

        let up = super::tool_input_into_with_scratch(
            &mut state,
            ToolInputEvent::PointerUp(sample),
            &mut scratch,
        )
        .unwrap();
        assert!(!up.project_changed());
    }

    #[test]
    fn transform_begin_and_cancel_do_not_change_project() {
        let mut state = state_with_document();

        assert!(!super::begin_transform_mode(&mut state).project_changed());
        assert!(!super::cancel_active_transform(&mut state).project_changed());
    }

    #[test]
    fn camera_is_passive_but_layer_creation_changes_project() {
        let mut runtime = ApplicationRuntime::new(state_with_document());

        assert!(
            !runtime
                .dispatch(Command::SetCamera(Default::default()))
                .unwrap()
        );
        assert!(runtime.dispatch(Command::AddLayer).unwrap());
    }

    #[test]
    fn transient_override_and_undo_do_not_change_the_panel_tool() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();

        let _ = reduce(
            &mut state,
            Command::BeginTransientToolOverride {
                token: InputHoldToken(2),
                id: "erase".to_owned(),
            },
        );
        assert_eq!(state.panel_tool_id(), selected_tool);

        let _ = reduce(&mut state, Command::Undo);
        assert_eq!(state.active_tool_id(), selected_tool);
        assert_eq!(state.panel_tool_id(), selected_tool);
    }

    #[test]
    fn stroke_preset_update_targets_the_panel_tool_preset() {
        let mut state = state_with_document();
        let selected_preset_index = state.active_stroke_preset_index().unwrap();
        let selected_preset_before = state
            .stroke_tool_preset(selected_preset_index)
            .unwrap()
            .clone();
        let eraser_tool = state
            .tool_id_by_config_id("builtin.brush.eraser.round")
            .unwrap();
        let eraser_preset_index = match &state
            .tool_catalog()
            .iter()
            .find(|tool| tool.id == eraser_tool)
            .unwrap()
            .behavior
        {
            ToolBehavior::Stroke { preset_index } => *preset_index,
            _ => panic!("eraser should be a stroke tool"),
        };
        let mut eraser_preset = state
            .stroke_tool_preset(eraser_preset_index)
            .unwrap()
            .clone();
        eraser_preset.input_filter.stabilization = 7;

        let _ = reduce(
            &mut state,
            Command::BeginMomentaryTool {
                token: InputHoldToken(3),
                tool_id: eraser_tool,
            },
        );
        assert_eq!(state.panel_tool_id(), eraser_tool);
        let _ = reduce(
            &mut state,
            Command::UpdateRuntimeStrokePreset {
                preset_index: eraser_preset_index,
                preset: eraser_preset.clone(),
            },
        );

        assert_eq!(
            state.stroke_tool_preset(eraser_preset_index),
            Some(&eraser_preset)
        );
        assert_eq!(
            state.stroke_tool_preset(selected_preset_index),
            Some(&selected_preset_before)
        );
        assert!(
            state
                .user_brush_preset_definitions()
                .iter()
                .any(|preset| preset.id() == "builtin.brush.eraser.round")
        );

        let _ = reduce(
            &mut state,
            Command::CompleteMomentaryTool {
                token: InputHoldToken(3),
                tool_id: eraser_tool,
                select_tool: false,
            },
        );
        let expected_defaults = state
            .brush_engine_defaults(eraser_preset_index, "builtin.brush.eraser.round")
            .unwrap();
        let _ = reduce(
            &mut state,
            Command::ResetBrushPreset {
                preset_index: eraser_preset_index,
                preset_id: "builtin.brush.eraser.round".to_owned(),
            },
        );
        assert_eq!(
            state.stroke_tool_preset(eraser_preset_index),
            Some(&expected_defaults)
        );
        assert_eq!(expected_defaults.input_filter.stabilization, 7);
    }

    #[test]
    fn cancelling_an_active_transform_keeps_the_selected_tool() {
        let mut state = state_with_document();
        let selected_tool = state.active_tool_id();
        install_transform_session(&mut state, false);

        let plan = reduce(&mut state, Command::CancelActiveTransform);

        assert_eq!(state.active_tool_id(), selected_tool);
        assert!(state.tool.is_idle());
        assert!(
            plan.edit_commands
                .iter()
                .any(|command| matches!(command, EditCommand::Transform(TransformCommand::Cancel)))
        );
    }

    #[test]
    fn focus_loss_cancels_only_the_active_transform_gesture() {
        let mut state = state_with_document();
        install_transform_session(&mut state, true);

        let plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::Cancel {
                reason: ToolCancelReason::FocusLost,
            }),
        );

        let session = state.tool.transform_session().unwrap();
        assert_eq!(session.transform, UvTransform::identity());
        assert!(session.gesture.is_none());
        assert!(plan.edit_commands.iter().any(|command| matches!(
            command,
            EditCommand::Transform(TransformCommand::Preview { .. })
        )));
    }
    #[test]
    fn create_and_delete_brush_preset_updates_runtime_catalog_and_layout() {
        let mut state = state_with_document();
        let source_id = "builtin.brush.paint.round_soft";
        let source_tool = state.tool_id_by_config_id(source_id).expect("source tool");
        state.set_active_tool(source_tool);
        let source_preset_index = state.active_stroke_preset_index().expect("stroke preset");
        let mut new_preset = state
            .stroke_tool_preset(source_preset_index)
            .expect("source preset")
            .clone();
        new_preset.input_filter.stabilization = 9;
        let _ = reduce(
            &mut state,
            Command::UpdateRuntimeStrokePreset {
                preset_index: source_preset_index,
                preset: new_preset.clone(),
            },
        );
        new_preset.name = "Test Created Brush".to_owned();
        let new_id = "user.brush.test_created";

        let _ = reduce(
            &mut state,
            Command::CreateBrushPreset {
                group_id: "tool.brush".to_owned(),
                source_preset_id: source_id.to_owned(),
                new_preset_id: new_id.to_owned(),
                preset: new_preset,
            },
        );

        let created_tool = state.tool_id_by_config_id(new_id).expect("created tool");
        assert_eq!(state.active_tool_id(), created_tool);
        assert_eq!(
            state.panel_tool_definition().unwrap().name,
            "Test Created Brush"
        );
        assert!(state
            .tool_layout_snapshot()
            .tools
            .iter()
            .find(|group| group.id == "tool.brush")
            .unwrap()
            .entries
            .iter()
            .any(|entry| matches!(entry, crate::core::tool_layout::ToolEntryDefinition::BrushPreset(id) if id == new_id)));
        assert!(
            state
                .user_brush_preset_definitions()
                .iter()
                .any(|preset| preset.id() == new_id)
        );
        let layout = state.tool_layout_snapshot();
        let entries = &layout
            .tools
            .iter()
            .find(|group| group.id == "tool.brush")
            .unwrap()
            .entries;
        let source_position = entries.iter().position(|entry| matches!(entry,
            crate::core::tool_layout::ToolEntryDefinition::BrushPreset(id) if id == source_id
        )).unwrap();
        assert!(matches!(&entries[source_position + 1],
            crate::core::tool_layout::ToolEntryDefinition::BrushPreset(id) if id == new_id
        ));
        let source_tool = state.tool_id_by_config_id(source_id).expect("source tool");
        let source_index = match state
            .tool_catalog()
            .iter()
            .find(|tool| tool.id == source_tool)
            .expect("source definition")
            .behavior
        {
            ToolBehavior::Stroke { preset_index } => preset_index,
            _ => panic!("source should remain a stroke tool"),
        };
        assert_eq!(
            state
                .stroke_tool_preset(source_index)
                .expect("source runtime")
                .input_filter
                .stabilization,
            9
        );

        let _ = reduce(
            &mut state,
            Command::DeleteBrushPreset {
                preset_id: new_id.to_owned(),
            },
        );

        assert!(state.tool_id_by_config_id(new_id).is_none());
        assert_eq!(
            state.panel_tool_definition().unwrap().config_id,
            "builtin.brush.paint.directional_flat"
        );
        assert!(
            !state
                .user_brush_preset_definitions()
                .iter()
                .any(|preset| preset.id() == new_id)
        );
        assert_eq!(
            state
                .stroke_tool_preset(source_index)
                .expect("source runtime")
                .input_filter
                .stabilization,
            9
        );
    }

    #[test]
    fn deleting_the_active_last_brush_activates_its_empty_group() {
        let mut state = state_with_document();
        let preset_id = "builtin.brush.paint.round_soft";
        let brush = state.tool_id_by_config_id(preset_id).expect("brush tool");
        state.set_active_tool(brush);

        empty_builtin_brush_group(&mut state);

        let ToolId::EmptyGroup(group_index) = state.active_tool_id() else {
            panic!("the deleted brush group should activate its runtime no-op");
        };
        assert_eq!(state.tool_shelf().groups[group_index].id, "tool.brush");
        assert!(state.tool_shelf().groups[group_index].entries.is_empty());
        assert_eq!(
            state.representative_tool_id(group_index),
            Some(state.active_tool_id())
        );
        assert!(state.active_tool_preset().is_none());
        assert!(state.effective_tool_preset().is_none());
        assert!(state.effective_brush_radius_world().is_none());
    }

    #[test]
    fn deleting_a_non_active_last_brush_preserves_the_active_tool() {
        let mut state = state_with_document();
        let active = state
            .tool_id_by_config_id("builtin.brush.airbrush.soft")
            .expect("airbrush");
        state.set_active_tool(active);

        empty_builtin_brush_group(&mut state);

        assert_eq!(state.active_tool_id(), active);
        let group_index = state
            .tool_shelf()
            .groups
            .iter()
            .position(|group| group.id == "tool.brush")
            .expect("brush group");
        assert!(state.tool_shelf().groups[group_index].entries.is_empty());
        assert_eq!(
            state.representative_tool_id(group_index),
            Some(ToolId::EmptyGroup(group_index))
        );
    }

    #[test]
    fn creating_in_an_empty_group_uses_the_default_template_and_activates_the_new_brush() {
        let mut state = state_with_document();
        let source_id = "builtin.brush.paint.round_soft";
        let source_tool = state.tool_id_by_config_id(source_id).expect("brush tool");
        state.set_active_tool(source_tool);
        empty_builtin_brush_group(&mut state);
        let mut preset = state.default_brush_preset().clone();
        preset.name = "Created From Default".to_owned();
        let new_id = "user.brush.created_from_default";

        let _ = reduce(
            &mut state,
            Command::CreateBrushPreset {
                group_id: "tool.brush".to_owned(),
                source_preset_id: DEFAULT_BRUSH_PRESET_ID.to_owned(),
                new_preset_id: new_id.to_owned(),
                preset: preset.clone(),
            },
        );

        let created = state.tool_id_by_config_id(new_id).expect("created brush");
        assert_eq!(state.active_tool_id(), created);
        assert_eq!(state.active_tool_preset(), Some(&preset));
        assert_eq!(
            state.representative_tool_id_for_group("tool.brush"),
            Some(created)
        );
        assert_eq!(
            state
                .tool_shelf()
                .groups
                .iter()
                .find(|group| group.id == "tool.brush")
                .expect("brush group")
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn reordering_an_active_empty_group_preserves_its_stable_identity() {
        let mut state = state_with_document();
        let preset_id = "builtin.brush.paint.round_soft";
        let brush = state.tool_id_by_config_id(preset_id).expect("brush tool");
        state.set_active_tool(brush);
        empty_builtin_brush_group(&mut state);
        let config_id = state
            .panel_tool_definition()
            .expect("empty group no-op")
            .config_id
            .clone();
        let from_index = state
            .tool_shelf()
            .groups
            .iter()
            .position(|group| group.id == "tool.brush")
            .expect("brush group");
        let to_index = state.tool_shelf().groups.len() - 1;

        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index,
                to_index,
            },
        );

        assert_eq!(state.panel_tool_definition().unwrap().config_id, config_id);
        assert_eq!(state.tool_shelf().groups[to_index].id, "tool.brush");
        assert_eq!(state.active_tool_id(), ToolId::EmptyGroup(to_index));
        assert!(state.active_tool_preset().is_none());
    }

    #[test]
    fn preset_apis_do_not_fall_back_for_non_stroke_or_invalid_tools() {
        let mut state = state_with_document();
        let fill = state
            .tool_id_by_config_id("builtin.tool.fill.material")
            .expect("fill tool");
        state.set_active_tool(fill);
        assert!(state.active_tool_preset().is_none());
        assert!(state.effective_tool_preset().is_none());

        let color_picker = state
            .tool_id_by_config_id("builtin.tool.sampler.color_picker")
            .expect("color picker");
        state.set_active_tool(color_picker);
        assert!(state.active_tool_preset().is_none());
        assert!(state.effective_tool_preset().is_none());

        let brush = state
            .tool_id_by_config_id("builtin.brush.airbrush.soft")
            .expect("airbrush");
        state.set_active_tool(brush);
        state
            .tool
            .tool_catalog
            .iter_mut()
            .find(|tool| tool.id == brush)
            .expect("brush definition")
            .behavior = ToolBehavior::Stroke {
            preset_index: usize::MAX,
        };
        assert!(state.active_tool_preset().is_none());
        assert!(state.effective_tool_preset().is_none());
    }

    #[test]
    fn removing_all_stroke_presets_is_a_valid_runtime_state() {
        let mut state = state_with_document();
        for preset_id in [
            "builtin.brush.paint.round_soft",
            "builtin.brush.paint.directional_flat",
            "builtin.brush.airbrush.soft",
            "builtin.brush.eraser.round",
            "builtin.brush.filter.blur_soft",
            "builtin.brush.smudge.soft",
        ] {
            let _ = reduce(
                &mut state,
                Command::DeleteBrushPreset {
                    preset_id: preset_id.to_owned(),
                },
            );
        }

        assert!(state.tool.tool_presets.is_empty());
        assert!(state.active_tool_preset().is_none());
        assert!(state.default_brush_preset().name.contains("Default"));
    }

    #[test]
    fn configuration_reload_refreshes_the_default_creation_template() {
        let mut state = state_with_document();
        state.tool.default_brush_preset.name = "Stale Template".to_owned();

        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index: 0,
                to_index: 1,
            },
        );

        assert_eq!(state.default_brush_preset().name, "Default Brush");
    }

    #[test]
    fn moving_tool_group_preserves_active_tool_and_group_representatives() {
        let mut state = state_with_document();
        let fill_polygon = state
            .tool_id_by_config_id("builtin.tool.fill.polygon")
            .expect("fill polygon");
        let smudge = state
            .tool_id_by_config_id("builtin.brush.smudge.soft")
            .expect("smudge");
        state.set_active_tool(fill_polygon);
        state.set_active_tool(smudge);
        let before = state.tool_layout_snapshot();
        let fill_index = before
            .tools
            .iter()
            .position(|group| group.id == "tool.fill")
            .expect("fill group");

        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index: fill_index,
                to_index: 1,
            },
        );

        let layout_ids = state
            .tool_layout_snapshot()
            .tools
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        let shelf_ids = state
            .tool_shelf()
            .groups
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(layout_ids, shelf_ids);
        assert_eq!(layout_ids[1], "tool.fill");
        assert_eq!(state.active_tool_id(), smudge);
        assert_eq!(state.panel_tool_id(), smudge);
        assert_eq!(
            state.representative_tool_id_for_group("tool.fill"),
            Some(fill_polygon)
        );
    }

    #[test]
    fn moving_tool_group_uses_final_destination_index() {
        let mut state = state_with_document();
        let original = state
            .tool_layout_snapshot()
            .tools
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        let last_index = original.len() - 1;

        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index: 1,
                to_index: last_index,
            },
        );
        let moved = state
            .tool_layout_snapshot()
            .tools
            .iter()
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(moved[last_index], original[1]);

        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index: last_index,
                to_index: 1,
            },
        );
        assert_eq!(
            state
                .tool_layout_snapshot()
                .tools
                .iter()
                .map(|group| group.id.clone())
                .collect::<Vec<_>>(),
            original
        );

        let before_no_op = state.tool_layout_snapshot();
        let _ = reduce(
            &mut state,
            Command::MoveToolGroup {
                from_index: 1,
                to_index: 1,
            },
        );
        assert_eq!(state.tool_layout_snapshot(), before_no_op);
    }

    #[test]
    fn moving_builtin_tool_entry_updates_layout_and_runtime_shelf() {
        let mut state = state_with_document();
        let fill_mesh = state
            .tool_id_by_config_id("builtin.tool.fill.mesh")
            .expect("fill mesh");
        state.set_active_tool(fill_mesh);

        let _ = reduce(
            &mut state,
            Command::MoveToolEntry {
                group_id: "tool.fill".to_owned(),
                from_index: 2,
                to_index: 0,
            },
        );

        let layout = state.tool_layout_snapshot();
        let group = layout
            .tools
            .iter()
            .find(|group| group.id == "tool.fill")
            .expect("fill layout group");
        assert!(matches!(
            &group.entries[0],
            crate::core::tool_layout::ToolEntryDefinition::BuiltinTool(id)
                if id == "builtin.tool.fill.polygon"
        ));
        let shelf = state
            .tool_shelf()
            .groups
            .iter()
            .find(|group| group.id == "tool.fill")
            .expect("fill shelf group");
        assert_eq!(
            shelf
                .entries
                .iter()
                .map(|entry| entry.tool_id)
                .collect::<Vec<_>>(),
            vec![ToolId::FillPolygon, ToolId::FillMaterial, ToolId::FillMesh]
        );
        assert_eq!(state.active_tool_id(), fill_mesh);
        assert_eq!(state.panel_tool_id(), fill_mesh);
    }

    #[test]
    fn moving_brush_entry_preserves_runtime_only_settings() {
        let mut state = state_with_document();
        let preset_id = "builtin.brush.smudge.soft";
        let smudge = state.tool_id_by_config_id(preset_id).expect("smudge");
        state.set_active_tool(smudge);
        let old_preset_index = state.active_stroke_preset_index().expect("smudge preset");
        let mut runtime_preset = state
            .stroke_tool_preset(old_preset_index)
            .expect("runtime preset")
            .clone();
        runtime_preset.input_filter.stabilization = 9;
        let _ = reduce(
            &mut state,
            Command::UpdateRuntimeStrokePreset {
                preset_index: old_preset_index,
                preset: runtime_preset,
            },
        );

        let _ = reduce(
            &mut state,
            Command::MoveToolEntry {
                group_id: "tool.filter_brush".to_owned(),
                from_index: 1,
                to_index: 0,
            },
        );

        let smudge = state.tool_id_by_config_id(preset_id).expect("moved smudge");
        let new_preset_index = match state
            .tool_catalog()
            .iter()
            .find(|tool| tool.id == smudge)
            .expect("smudge definition")
            .behavior
        {
            ToolBehavior::Stroke { preset_index } => preset_index,
            _ => panic!("smudge should be a stroke tool"),
        };
        assert_eq!(state.active_tool_id(), smudge);
        assert_eq!(state.panel_tool_id(), smudge);
        assert_eq!(
            state
                .stroke_tool_preset(new_preset_index)
                .expect("moved runtime preset")
                .input_filter
                .stabilization,
            9
        );
        let group = state
            .tool_shelf()
            .groups
            .iter()
            .find(|group| group.id == "tool.filter_brush")
            .expect("filter brush group");
        assert_eq!(group.entries[0].tool_id, smudge);
    }

    #[test]
    fn tool_reorder_rejects_invalid_indices_and_active_interaction() {
        let mut state = state_with_document();
        let original = state.tool_layout_snapshot();
        assert!(state.tool.move_tool_group(usize::MAX, 0).is_err());
        assert!(
            state
                .tool
                .move_tool_entry("tool.fill", 0, usize::MAX)
                .is_err()
        );
        assert_eq!(state.tool_layout_snapshot(), original);

        install_decal_session(&mut state);
        assert!(state.tool.move_tool_group(0, 1).is_err());
        assert!(state.tool.move_tool_entry("tool.fill", 0, 1).is_err());
        assert_eq!(state.tool_layout_snapshot(), original);
    }

    #[test]
    fn engine_switch_uses_new_engine_default_stroke_when_needed() {
        let mut state = state_with_document();
        let preset_id = "builtin.brush.airbrush.soft";
        let tool_id = state
            .tool_id_by_config_id(preset_id)
            .expect("airbrush tool");
        state.set_active_tool(tool_id);
        let preset_index = state.active_stroke_preset_index().expect("preset index");
        assert!(matches!(
            state
                .stroke_tool_preset(preset_index)
                .unwrap()
                .stroke_strategy,
            StrokeStrategy::ContinuousDab { .. }
        ));
        let expected_default = state
            .brush_engines()
            .get("blur")
            .expect("blur engine")
            .default_stroke
            .clone();

        let _ = reduce(
            &mut state,
            Command::SetRuntimeBrushEngine {
                preset_index,
                preset_id: preset_id.to_owned(),
                engine_id: "blur".to_owned(),
            },
        );

        let switched_tool = state
            .tool_id_by_config_id(preset_id)
            .expect("switched tool");
        let switched_preset_index = match state
            .tool_catalog()
            .iter()
            .find(|tool| tool.id == switched_tool)
            .unwrap()
            .behavior
        {
            ToolBehavior::Stroke { preset_index } => preset_index,
            _ => panic!("brush should remain a stroke tool"),
        };
        let switched = state.stroke_tool_preset(switched_preset_index).unwrap();
        assert_eq!(switched.stroke_strategy, expected_default);
        let StrokePresetOp::BrushEngine { engine_id, .. } = &switched.stroke_op;
        assert_eq!(engine_id, "blur");
        assert!(
            state
                .user_brush_preset_definitions()
                .iter()
                .any(|preset| preset.id() == preset_id)
        );
        assert!(
            state
                .user_brush_preset_definitions()
                .iter()
                .any(|preset| preset.id() == preset_id && preset.engine_id() == "blur")
        );

        let _ = reduce(
            &mut state,
            Command::SetRuntimeBrushEngine {
                preset_index: switched_preset_index,
                preset_id: preset_id.to_owned(),
                engine_id: "paint".to_owned(),
            },
        );
        let _ = reduce(
            &mut state,
            Command::ResetBrushPreset {
                preset_index: switched_preset_index,
                preset_id: preset_id.to_owned(),
            },
        );
        let reset = state.stroke_tool_preset(switched_preset_index).unwrap();
        let StrokePresetOp::BrushEngine { engine_id, .. } = &reset.stroke_op;
        assert_eq!(engine_id, "paint");
    }
}
