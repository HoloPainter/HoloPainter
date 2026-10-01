use eframe::egui::{self, PointerButton, Sense};
use glam::Vec2;

use crate::{
    application::{
        AppState, Command, UvViewInputContext, ViewPointerEvent, ViewPointerMeta, ViewPointerPhase,
        plan_uv_brush_overlay_request,
    },
    core::{
        adjustment::{Adjustment, UvMirrorAdjustment, UvMirrorAxis},
        document::ActiveLayerTarget,
        stroke::StrokeSpace,
        tool::{ColorSampleSource, ToolBehavior},
        transform::{TransformHandle, UvTransformPreview, hit_test_transform_handle},
        uv_view::{UV_VIEW_FIT_MARGIN_PX, UV_VIEW_MIN_VISIBLE_PX, UvViewTransform},
    },
    localization::Localization,
    renderer::{
        ColorSampleAnchor, ColorSampleIntent, ColorSampleTarget, ColorSampleUiRequest,
        ColorSampleView, SelectionOverlayRequest, UvViewRequest,
    },
    ui::{
        icons::UiIconRegistry,
        input::{
            modifiers::input_modifiers_from_egui,
            pointer_pressure::current_pointer_pressure,
            shortcut_profile::ShortcutProfile,
            uv_mapping::{uv_rect_to_screen_quad, uv_to_screen},
            uv_navigation::UvViewNavigationState,
            view_navigation::{
                ViewNavigationAction, ViewNavigationKind, dominant_drag_delta,
                update_view_navigation,
            },
            view_pointer::{
                ViewPointerInputRouter, ViewPointerInputSample, ViewPointerTarget,
                pointer_down_position,
            },
        },
        render_resources::UiRenderResources,
        view_output::{UiRequest, ViewOutput},
        widgets::{
            operation_commit_controls::{
                OperationCommitAction, draw_operation_commit_controls,
                operation_commit_controls_layout,
            },
            viewport_toolbar::{ViewportToolbarKind, draw_viewport_toolbar},
        },
    },
};

use super::{
    brush_overlay_screen_position,
    color_picker::{current_layer_surface, unit_uv},
    tool_cursor::{apply_tool_cursor, color_sample_position, draw_color_sample_preview},
    update_brush_size_drag,
};

const TRANSFORM_HANDLE_RADIUS_PX: f32 = 5.0;
const TRANSFORM_HANDLE_HIT_RADIUS_PX: f32 = 9.0;
const UV_VIEW_ZOOM_SPEED: f32 = 0.002;
const UV_VIEW_OVERLAY_MARGIN: f32 = 6.0;
const UV_MATERIAL_SELECTOR_WIDTH: f32 = 180.0;
const UV_MATERIAL_SELECTOR_HEIGHT: f32 = 26.0;

#[derive(Debug, Clone, Copy)]
struct UvMaterialSelectorInteraction {
    rect: egui::Rect,
    popup_open: bool,
}

impl UvMaterialSelectorInteraction {
    fn pointer_over(self, ctx: &egui::Context) -> bool {
        ctx.pointer_hover_pos()
            .is_some_and(|pointer| self.rect.contains(pointer))
    }
}

pub fn draw_uv_view(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    render_resources: &UiRenderResources,
    pointer_input: &mut ViewPointerInputRouter,
    icons: &UiIconRegistry,
    interaction_enabled: bool,
    shortcut_profile: &ShortcutProfile,
    shortcut_profile_revision: u64,
) -> ViewOutput {
    let mut output = ViewOutput::default();

    if !render_resources.renderer_available {
        ui.label(l10n.text("renderer-unavailable"));
        return output;
    }

    let material_index = focused_material_index(state);
    let canvas_size = focused_material_size(state, material_index).unwrap_or([1, 1]);
    let available = ui.available_size();
    let desired = egui::vec2(available.x.max(1.0), available.y.max(1.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let material_selector = draw_uv_material_selector(ui, l10n, rect, state, &mut output);
    let toolbar = draw_viewport_toolbar(
        ui,
        rect,
        ViewportToolbarKind::Uv,
        state,
        icons,
        l10n,
        shortcut_profile,
        &mut output,
    );
    let pointer_over_material_selector =
        material_selector.is_some_and(|interaction| interaction.pointer_over(ui.ctx()));
    let pointer_over_toolbar = toolbar.pointer_over_toolbar(ui.ctx());

    let uv_view_size = [rect.width().round() as u32, rect.height().round() as u32];
    let stored_transform = state.uv_view_transform();
    let transform_initialized = state.uv_view_transform_initialized();
    let mut transform = if transform_initialized {
        stored_transform.clamped_for_view(uv_view_size, canvas_size, UV_VIEW_MIN_VISIBLE_PX)
    } else {
        UvViewTransform::fit_to_view(uv_view_size, canvas_size, UV_VIEW_FIT_MARGIN_PX)
    };
    let transform_preview = state.uv_transform_preview();
    let initial_transform_geometry = transform_preview
        .map(|preview| transform_overlay_geometry(rect, canvas_size, preview, transform));
    let initial_operation_controls_layout = if state.has_active_raster_transform_session() {
        initial_transform_geometry
            .and_then(|geometry| operation_commit_controls_layout(rect, geometry.corners))
    } else {
        None
    };
    let pointer_over_operation_controls =
        initial_operation_controls_layout.is_some_and(|layout| layout.pointer_over(ui.ctx()));
    let material_selector_popup_open =
        material_selector.is_some_and(|interaction| interaction.popup_open);
    let mut block_new_view_input = !interaction_enabled
        || material_selector_popup_open
        || toolbar.popup_open
        || pointer_over_material_selector
        || pointer_over_toolbar
        || pointer_over_operation_controls;
    let color_picker_active = state
        .effective_tool_definition()
        .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::ColorPicker));
    let navigation = update_view_navigation(
        ui.ctx(),
        ui.id().with("view_navigation"),
        ViewNavigationKind::Uv,
        rect,
        interaction_enabled,
        !block_new_view_input
            && !state.is_tool_pointer_gesture_active()
            && !pointer_input.is_active(),
        (shortcut_profile, shortcut_profile_revision),
    );
    let brush_size_start_size_screen_points = (navigation.started()
        && navigation.action() == Some(ViewNavigationAction::BrushSize))
    .then(|| {
        let start_context = UvViewInputContext {
            size: uv_view_size,
            canvas_size,
            transform,
        };
        plan_uv_brush_overlay_request(
            state,
            local_px(navigation.start_position(), rect),
            1.0,
            start_context,
        )
        .map(|request| request.radius_px * 2.0)
    })
    .flatten();
    let brush_size_drag_anchor = update_brush_size_drag(
        ui.ctx(),
        ui.id().with("brush_size_drag"),
        navigation,
        brush_size_start_size_screen_points,
        state,
        &mut output,
    );
    let navigation_allows_brush_hover =
        !navigation.owns_input() || brush_size_drag_anchor.is_some();
    let uv_navigation_id = ui.id().with("uv_transform_navigation");
    let mut uv_navigation = ui
        .ctx()
        .data_mut(|data| data.get_persisted::<UvViewNavigationState>(uv_navigation_id))
        .unwrap_or_default();
    if !interaction_enabled {
        uv_navigation.end();
    }
    if navigation.started()
        && let Some(action) = navigation.action()
    {
        uv_navigation.begin(
            action,
            transform,
            local_px(navigation.start_position(), rect),
        );
    }
    if navigation.owns_input() && uv_navigation.is_active() {
        let total_delta = navigation.total_delta();
        let snap_rotation = ui.ctx().input(|input| input.modifiers.shift);
        let updated_transform = uv_navigation.update(
            transform,
            Vec2::new(total_delta.x, total_delta.y),
            dominant_drag_delta(total_delta),
            uv_view_size,
            canvas_size,
            snap_rotation,
        );
        if updated_transform != transform {
            transform = updated_transform;
            output.request_repaint();
        }
    }
    if navigation.ended() {
        uv_navigation.end();
    }

    if !block_new_view_input
        && !navigation.owns_input()
        && !state.is_tool_pointer_gesture_active()
        && response.hovered()
        && let Some(pointer) = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|pos| rect.contains(*pos))
    {
        let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let zoom_factor = (scroll * UV_VIEW_ZOOM_SPEED).exp();
            transform = transform
                .zoom_about(
                    local_px(pointer, rect),
                    uv_view_size,
                    canvas_size,
                    zoom_factor,
                )
                .clamped_for_view(uv_view_size, canvas_size, UV_VIEW_MIN_VISIBLE_PX);
            output.request_repaint();
        }
    }

    if !transform_initialized || transform != stored_transform {
        output.push(Command::SetUvViewTransform(transform));
    }
    ui.ctx()
        .data_mut(|data| data.insert_persisted(uv_navigation_id, uv_navigation));

    let transform_geometry = transform_preview
        .map(|preview| transform_overlay_geometry(rect, canvas_size, preview, transform));
    let operation_controls_layout = if state.has_active_raster_transform_session() {
        transform_geometry
            .and_then(|geometry| operation_commit_controls_layout(rect, geometry.corners))
    } else {
        None
    };
    let operation_controls = operation_controls_layout.map(|layout| {
        draw_operation_commit_controls(
            ui.ctx(),
            ui.id().with("transform_operation_commit_controls"),
            layout,
            icons,
            l10n,
            interaction_enabled && !state.is_tool_pointer_gesture_active(),
        )
    });
    if let Some(interaction) = operation_controls {
        block_new_view_input |= interaction.pointer_over(ui.ctx());
        match interaction.action {
            Some(OperationCommitAction::Apply) => {
                output.push(Command::ApplyActiveTransform);
            }
            Some(OperationCommitAction::Cancel) => {
                output.push(Command::CancelActiveTransform);
            }
            None => {}
        }
    }

    let uv_view_context = UvViewInputContext {
        size: uv_view_size,
        canvas_size,
        transform,
    };
    let live_pointer = ui.ctx().pointer_hover_pos();
    let normal_brush_hover_allowed = !block_new_view_input
        && navigation_allows_brush_hover
        && (response.hovered()
            || (color_picker_active && response.dragged_by(PointerButton::Primary)));
    let brush_overlay_pointer = brush_overlay_screen_position(
        live_pointer,
        rect,
        normal_brush_hover_allowed,
        brush_size_drag_anchor,
    );
    let brush_overlay_request = brush_overlay_pointer.and_then(|pointer| {
        let pressure_pointer = live_pointer.unwrap_or(pointer);
        let pressure = current_pointer_pressure(ui.ctx(), pressure_pointer);
        plan_uv_brush_overlay_request(state, local_px(pointer, rect), pressure, uv_view_context)
    });

    let brush_overlay_visible = brush_overlay_request.is_some();
    let color_sample_source = state.color_picker_options().source;
    let color_sample_pointer = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|pointer| rect.contains(*pointer))
        .filter(|_| {
            !block_new_view_input
                && navigation_allows_brush_hover
                && (response.hovered()
                    || (color_picker_active && response.dragged_by(PointerButton::Primary)))
        });
    if color_picker_active
        && let Some(pointer) = color_sample_pointer
        && let Some(position) = color_sample_position(pointer, rect, uv_view_size)
    {
        let uv =
            unit_uv(transform.view_px_to_uv(local_px(pointer, rect), uv_view_size, canvas_size));
        let resolved_target = match color_sample_source {
            ColorSampleSource::View => Some((
                ColorSampleTarget::ViewOutput {
                    view: ColorSampleView::Uv,
                    position,
                },
                None,
            )),
            ColorSampleSource::CompositeTexture => uv.map(|uv| {
                (
                    ColorSampleTarget::CompositeTexture { material_index, uv },
                    None,
                )
            }),
            ColorSampleSource::CurrentLayer => uv.and_then(|uv| {
                current_layer_surface(state, material_index).map(|(surface, layer_id)| {
                    (
                        ColorSampleTarget::SurfaceTexture { surface, uv },
                        Some(layer_id),
                    )
                })
            }),
        };
        if let Some((target, active_layer_id)) = resolved_target {
            let intent = if response.clicked_by(PointerButton::Primary)
                || ui.ctx().input(|input| input.pointer.primary_down())
            {
                ColorSampleIntent::Apply
            } else {
                ColorSampleIntent::Preview
            };
            output.request_ui(UiRequest::SampleColor(ColorSampleUiRequest {
                intent,
                source: color_sample_source,
                anchor: ColorSampleAnchor {
                    view: ColorSampleView::Uv,
                    position,
                },
                target,
                active_layer_id,
            }));
            output.request_repaint();
        }
    }
    let selection_overlay_request = selection_overlay_request(ui.ctx(), state);
    if selection_overlay_request.is_some() {
        output.request_repaint();
    }
    output.request_uv_view_render(UvViewRequest {
        material_index,
        uv_view_size,
        transform,
        brush_overlay_request,
        selection_overlay_request,
        show_wireframe: state.uv_wireframe_visible(),
        wireframe_style: state.uv_wireframe_style(),
        background_color: state.uv_view_background_color(),
    });

    let painter = ui.painter_at(rect);
    if let Some(texture_id) = render_resources.uv_view_texture_id {
        painter.image(
            texture_id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    draw_canvas_border(&painter, rect, canvas_size, transform);
    if let Some(mirror) = active_uv_mirror_overlay(state, material_index) {
        draw_uv_mirror_line(&painter, rect, canvas_size, transform, mirror);
    }

    if let Some((start, current)) = state.uv_drag_preview() {
        let mut preview =
            uv_rect_to_screen_quad(start, current, rect, canvas_size, transform).to_vec();
        preview.push(preview[0]);
        painter.add(egui::Shape::line(
            preview,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 180, 255)),
        ));
    }

    if let Some(points) = state.uv_lasso_drag_preview()
        && points.len() >= 2
    {
        let mut screen_points: Vec<egui::Pos2> = points
            .into_iter()
            .map(|uv| uv_to_screen(uv, rect, canvas_size, transform))
            .collect();
        screen_points.push(screen_points[0]);
        painter.add(egui::Shape::line(
            screen_points,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 180, 255)),
        ));
    }

    if let (Some(preview), Some(geometry)) = (transform_preview, transform_geometry) {
        draw_transform_overlay(&painter, preview, geometry);
    }

    if let Some(action) = navigation.action() {
        match action {
            ViewNavigationAction::RotateUv => apply_rotate_cursor(ui, icons),
            ViewNavigationAction::PanUv => {
                ui.ctx().set_cursor_image(None);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            ViewNavigationAction::ZoomUv => {
                ui.ctx().set_cursor_image(None);
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            ViewNavigationAction::BrushSize => {
                ui.ctx().set_cursor_image(None);
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            ViewNavigationAction::Orbit3d
            | ViewNavigationAction::Pan3d
            | ViewNavigationAction::Zoom3d => unreachable!("3D navigation in UV view"),
        }
    } else if !block_new_view_input
        && (response.hovered()
            || (color_picker_active && response.dragged_by(PointerButton::Primary)))
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pointer| response.rect.contains(pointer))
    {
        let transform_tool_active = state
            .effective_tool_definition()
            .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::Transform));
        if transform_tool_active {
            apply_transform_cursor(
                ui,
                icons,
                response.rect,
                canvas_size,
                transform_preview,
                transform,
            );
        } else if !response.dragged_by(PointerButton::Primary) || color_picker_active {
            apply_tool_cursor(
                ui,
                l10n,
                state,
                rect,
                StrokeSpace::Uv,
                brush_overlay_visible,
                state.paint_edit_permission(Some(state.focused_material_index())),
            );
        }
    }

    if color_picker_active && let Some(pointer) = color_sample_pointer {
        draw_color_sample_preview(
            ui,
            rect,
            pointer,
            ColorSampleView::Uv,
            color_sample_source,
            state.document_generation(),
            render_resources.color_sample_preview,
        );
    }

    let active_tool_is_stroke = state
        .effective_tool_definition()
        .is_some_and(|tool| tool.is_stroke());
    let mut blocked_rects = vec![toolbar.rect];
    if let Some(interaction) = material_selector {
        blocked_rects.push(interaction.rect);
    }
    if let Some(interaction) = operation_controls {
        blocked_rects.push(interaction.rect);
    }
    let pointer_frame = pointer_input.collect_view_samples(
        ui.ctx(),
        ViewPointerTarget::Uv,
        response.rect,
        &blocked_rects,
        interaction_enabled,
        active_tool_is_stroke
            && !material_selector_popup_open
            && !toolbar.popup_open
            && !navigation.owns_input(),
    );
    for reason in pointer_frame.cancellations.iter().copied() {
        output.push(Command::ViewPointer(ViewPointerEvent::Cancel { reason }));
    }
    if !navigation.owns_input() {
        for sample in pointer_frame.samples {
            push_raw_uv_pointer_command(&mut output, sample, uv_view_context);
        }
    }
    let response_primary_input_enabled = !navigation.consumes_primary()
        && (!active_tool_is_stroke
            || (!pointer_frame.saw_touch_event
                && !pointer_frame.saw_tablet_event
                && !pointer_frame.suppress_egui_pointer
                && !pointer_input.is_active()));

    if response_primary_input_enabled
        && !block_new_view_input
        && response.clicked_by(PointerButton::Primary)
        && let Some(pointer) = response.interact_pointer_pos()
    {
        push_uv_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Click,
            pointer,
            response.rect,
            uv_view_context,
        );
    }

    if response_primary_input_enabled
        && !block_new_view_input
        && response.drag_started_by(PointerButton::Primary)
        && let Some(current_pointer) = response.interact_pointer_pos()
    {
        let press_origin = ui.ctx().input(|input| input.pointer.press_origin());
        let pointer = pointer_down_position(current_pointer, press_origin, response.rect);
        push_uv_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Down,
            pointer,
            response.rect,
            uv_view_context,
        );
    }

    if response_primary_input_enabled
        && response.dragged_by(PointerButton::Primary)
        && let Some(pointer) = response.interact_pointer_pos()
    {
        push_uv_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Move,
            pointer,
            response.rect,
            uv_view_context,
        );
    }

    if response_primary_input_enabled && response.drag_stopped_by(PointerButton::Primary) {
        let pointer = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.center());
        push_uv_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Up,
            pointer,
            response.rect,
            uv_view_context,
        );
    }

    output
}

fn active_uv_mirror_overlay(state: &AppState, material_index: usize) -> Option<UvMirrorAdjustment> {
    if state.active_layer_target() != ActiveLayerTarget::Adjustment {
        return None;
    }

    let document = state.document()?;
    let layer_id = state.active_layer_id();
    let material_id = document.material_id(material_index.into())?;
    if !document
        .layer_tree
        .effective_material_allowed(layer_id, material_id)
    {
        return None;
    }

    match document.layer_tree.adjustment(layer_id)? {
        Adjustment::UvMirror(mirror) => Some(mirror),
        _ => None,
    }
}

fn draw_uv_mirror_line(
    painter: &egui::Painter,
    view_rect: egui::Rect,
    canvas_size: [u32; 2],
    transform: UvViewTransform,
    mirror: UvMirrorAdjustment,
) {
    let [start_uv, end_uv] = uv_mirror_line_uv(mirror, canvas_size);
    painter.line_segment(
        [
            uv_to_screen(start_uv, view_rect, canvas_size, transform),
            uv_to_screen(end_uv, view_rect, canvas_size, transform),
        ],
        egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 210, 255)),
    );
}

fn uv_mirror_line_uv(mirror: UvMirrorAdjustment, canvas_size: [u32; 2]) -> [Vec2; 2] {
    let extent = match mirror.axis {
        UvMirrorAxis::X => canvas_size[0],
        UvMirrorAxis::Y => canvas_size[1],
    }
    .max(1) as f32;
    let position = (mirror.position.clamp(0.0, 1.0) * extent * 2.0).round() / (extent * 2.0);

    match mirror.axis {
        UvMirrorAxis::X => [Vec2::new(position, 0.0), Vec2::new(position, 1.0)],
        UvMirrorAxis::Y => [Vec2::new(0.0, position), Vec2::new(1.0, position)],
    }
}

fn draw_canvas_border(
    painter: &egui::Painter,
    view_rect: egui::Rect,
    canvas_size: [u32; 2],
    transform: UvViewTransform,
) {
    let mut corners = [
        Vec2::ZERO,
        Vec2::new(1.0, 0.0),
        Vec2::ONE,
        Vec2::new(0.0, 1.0),
    ]
    .map(|uv| uv_to_screen(uv, view_rect, canvas_size, transform))
    .to_vec();
    corners.push(corners[0]);
    painter.add(egui::Shape::line(
        corners,
        egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
    ));
}

#[derive(Debug, Clone, Copy)]
struct TransformOverlayGeometry {
    corners: [egui::Pos2; 4],
    edge_centers: [egui::Pos2; 4],
    pivot: egui::Pos2,
}

fn draw_transform_overlay(
    painter: &egui::Painter,
    preview: UvTransformPreview,
    geometry: TransformOverlayGeometry,
) {
    let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 180, 255));
    let mut outline = geometry.corners.to_vec();
    outline.push(geometry.corners[0]);
    painter.add(egui::Shape::line(outline, stroke));
    let handles = [
        (TransformHandle::ScaleTopLeft, geometry.corners[0]),
        (TransformHandle::ScaleTopRight, geometry.corners[1]),
        (TransformHandle::ScaleBottomRight, geometry.corners[2]),
        (TransformHandle::ScaleBottomLeft, geometry.corners[3]),
        (TransformHandle::ScaleTop, geometry.edge_centers[0]),
        (TransformHandle::ScaleRight, geometry.edge_centers[1]),
        (TransformHandle::ScaleBottom, geometry.edge_centers[2]),
        (TransformHandle::ScaleLeft, geometry.edge_centers[3]),
    ];
    for (handle, point) in handles {
        let fill = if preview.active_handle == Some(handle) {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            egui::Color32::WHITE
        };
        painter.circle(
            point,
            TRANSFORM_HANDLE_RADIUS_PX,
            fill,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(40, 110, 180)),
        );
    }

    painter.circle(
        geometry.pivot,
        4.0,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 210, 80)),
    );
    painter.line_segment(
        [
            geometry.pivot - egui::vec2(7.0, 0.0),
            geometry.pivot + egui::vec2(7.0, 0.0),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 210, 80)),
    );
    painter.line_segment(
        [
            geometry.pivot - egui::vec2(0.0, 7.0),
            geometry.pivot + egui::vec2(0.0, 7.0),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 210, 80)),
    );
}

fn apply_transform_cursor(
    ui: &egui::Ui,
    icons: &UiIconRegistry,
    view_rect: egui::Rect,
    canvas_size: [u32; 2],
    preview: Option<UvTransformPreview>,
    transform: UvViewTransform,
) {
    let Some(preview) = preview else {
        ui.ctx().set_cursor_image(None);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        return;
    };
    let geometry = transform_overlay_geometry(view_rect, canvas_size, preview, transform);
    let handle = preview.active_handle.or_else(|| {
        let pointer = ui.ctx().pointer_hover_pos()?;
        Some(hit_test_transform_handle(
            pos2_to_vec2(pointer),
            geometry.corners.map(pos2_to_vec2),
            TRANSFORM_HANDLE_HIT_RADIUS_PX,
        ))
    });
    if handle == Some(TransformHandle::Rotate) {
        apply_rotate_cursor(ui, icons);
        return;
    }

    ui.ctx().set_cursor_image(None);
    let cursor = match handle {
        Some(TransformHandle::Move) => egui::CursorIcon::Move,
        Some(TransformHandle::Rotate) => unreachable!("rotate cursor handled above"),
        Some(handle) => transform_resize_cursor(handle, geometry),
        None => egui::CursorIcon::Crosshair,
    };
    ui.ctx().set_cursor_icon(cursor);
}

fn apply_rotate_cursor(ui: &egui::Ui, icons: &UiIconRegistry) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    ui.ctx()
        .set_cursor_image(Some(icons.rotate_cursor().clone()));
}

fn transform_resize_cursor(
    handle: TransformHandle,
    geometry: TransformOverlayGeometry,
) -> egui::CursorIcon {
    let direction = match handle {
        TransformHandle::ScaleLeft | TransformHandle::ScaleRight => {
            geometry.edge_centers[1] - geometry.edge_centers[3]
        }
        TransformHandle::ScaleTop | TransformHandle::ScaleBottom => {
            geometry.edge_centers[2] - geometry.edge_centers[0]
        }
        TransformHandle::ScaleTopLeft | TransformHandle::ScaleBottomRight => {
            geometry.corners[2] - geometry.corners[0]
        }
        TransformHandle::ScaleTopRight | TransformHandle::ScaleBottomLeft => {
            geometry.corners[3] - geometry.corners[1]
        }
        TransformHandle::Move | TransformHandle::Rotate => {
            unreachable!("resize cursor requires a scale handle")
        }
    };
    resize_cursor_for_screen_direction(direction)
}

fn resize_cursor_for_screen_direction(direction: egui::Vec2) -> egui::CursorIcon {
    let octant = (direction.y.atan2(direction.x) / std::f32::consts::FRAC_PI_4).round() as i32;
    match octant.rem_euclid(4) {
        0 => egui::CursorIcon::ResizeHorizontal,
        1 => egui::CursorIcon::ResizeNwSe,
        2 => egui::CursorIcon::ResizeVertical,
        3 => egui::CursorIcon::ResizeNeSw,
        _ => unreachable!("rem_euclid(4) returns 0..=3"),
    }
}

fn transform_overlay_geometry(
    view_rect: egui::Rect,
    canvas_size: [u32; 2],
    preview: UvTransformPreview,
    transform: UvViewTransform,
) -> TransformOverlayGeometry {
    let corners = preview
        .corners_uv
        .map(|uv| uv_to_screen(uv, view_rect, canvas_size, transform));
    let edge_centers = [
        corners[0].lerp(corners[1], 0.5),
        corners[1].lerp(corners[2], 0.5),
        corners[2].lerp(corners[3], 0.5),
        corners[3].lerp(corners[0], 0.5),
    ];
    let pivot = uv_to_screen(preview.pivot_uv, view_rect, canvas_size, transform);
    TransformOverlayGeometry {
        corners,
        edge_centers,
        pivot,
    }
}

fn pos2_to_vec2(point: egui::Pos2) -> Vec2 {
    Vec2::new(point.x, point.y)
}

fn selection_overlay_request(
    ctx: &egui::Context,
    state: &AppState,
) -> Option<SelectionOverlayRequest> {
    let active_selection = state.active_selection()?.clone();
    if !active_selection.is_visible_active() {
        return None;
    }
    Some(SelectionOverlayRequest {
        active_selection,
        phase: ctx.input(|input| input.time as f32),
    })
}

fn push_uv_pointer_command(
    ui: &egui::Ui,
    output: &mut ViewOutput,
    phase: ViewPointerPhase,
    pointer: egui::Pos2,
    rect: egui::Rect,
    view: UvViewInputContext,
) {
    let modifiers = input_modifiers_from_egui(ui.ctx().input(|i| i.modifiers));
    let pressure = current_pointer_pressure(ui.ctx(), pointer);
    let position_px = local_px(pointer, rect);
    let time_s = ui.ctx().input(|input| input.time);
    push_uv_pointer_sample(
        output,
        phase,
        position_px,
        pressure,
        time_s,
        modifiers,
        view,
    );
}

fn push_raw_uv_pointer_command(
    output: &mut ViewOutput,
    sample: ViewPointerInputSample,
    view: UvViewInputContext,
) {
    push_uv_pointer_sample(
        output,
        sample.phase,
        sample.position_px,
        sample.pressure,
        sample.time_s,
        sample.modifiers,
        view,
    );
}

fn push_uv_pointer_sample(
    output: &mut ViewOutput,
    phase: ViewPointerPhase,
    position_px: Vec2,
    pressure: f32,
    time_s: f64,
    modifiers: crate::application::InputModifiers,
    view: UvViewInputContext,
) {
    output.request_repaint();
    output.push(Command::ViewPointer(ViewPointerEvent::Uv {
        meta: ViewPointerMeta {
            phase,
            position_px,
            pressure,
            time_s,
            modifiers,
        },
        view,
    }));
}

fn local_px(pointer: egui::Pos2, rect: egui::Rect) -> Vec2 {
    let local = pointer - rect.min;
    Vec2::new(local.x, local.y)
}

fn draw_uv_material_selector(
    ui: &mut egui::Ui,
    l10n: &Localization,
    viewport_rect: egui::Rect,
    state: &AppState,
    output: &mut ViewOutput,
) -> Option<UvMaterialSelectorInteraction> {
    let document = state.document()?;
    if !document.has_materials() {
        return None;
    }

    let focused = state.focused_material_index();
    let selected_text = document.materials.get(focused)?.name.as_str();
    let interacting = state.is_document_edit_interacting();
    let position = egui::pos2(
        viewport_rect.left() + UV_VIEW_OVERLAY_MARGIN,
        viewport_rect.top() + UV_VIEW_OVERLAY_MARGIN,
    );

    let area = egui::Area::new(egui::Id::new("uv_view_material_selector"))
        .order(egui::Order::Middle)
        .fixed_pos(position)
        .movable(false)
        .show(ui.ctx(), |ui| {
            let combo = ui.add_enabled_ui(!interacting, |ui| {
                ui.spacing_mut().interact_size.y = UV_MATERIAL_SELECTOR_HEIGHT;

                egui::ComboBox::from_id_salt("uv_view_material")
                    .width(UV_MATERIAL_SELECTOR_WIDTH)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for (index, material) in document.materials.iter().enumerate() {
                            if ui
                                .selectable_label(index == focused, material.name.as_str())
                                .clicked()
                                && index != focused
                            {
                                output.push(Command::SetFocusedMaterial(index));
                                output.request_repaint();
                            }
                        }
                    })
            });
            let response = if interacting {
                combo
                    .inner
                    .response
                    .on_disabled_hover_text(l10n.text("uv-view-finish-tool-to-change-material"))
            } else {
                combo.inner.response
            };
            (response.rect, combo.inner.inner.is_some())
        });

    Some(UvMaterialSelectorInteraction {
        rect: area.inner.0,
        popup_open: area.inner.1,
    })
}

fn focused_material_size(state: &AppState, material_index: usize) -> Option<[u32; 2]> {
    state
        .document()?
        .materials
        .get(material_index)
        .map(|material| material.texture_size)
}

fn focused_material_index(state: &AppState) -> usize {
    state.focused_material_index()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_overlay_geometry(corners: [egui::Pos2; 4]) -> TransformOverlayGeometry {
        TransformOverlayGeometry {
            edge_centers: [
                corners[0].lerp(corners[1], 0.5),
                corners[1].lerp(corners[2], 0.5),
                corners[2].lerp(corners[3], 0.5),
                corners[3].lerp(corners[0], 0.5),
            ],
            corners,
            pivot: egui::Pos2::ZERO,
        }
    }

    #[test]
    fn uv_mirror_line_uses_render_axis_snap_for_x_and_y() {
        let x_line = uv_mirror_line_uv(
            UvMirrorAdjustment {
                axis: UvMirrorAxis::X,
                position: 0.53,
                ..UvMirrorAdjustment::default()
            },
            [128, 64],
        );
        assert_eq!(
            x_line,
            [Vec2::new(68.0 / 128.0, 0.0), Vec2::new(68.0 / 128.0, 1.0)]
        );

        let y_line = uv_mirror_line_uv(
            UvMirrorAdjustment {
                axis: UvMirrorAxis::Y,
                position: 0.53,
                ..UvMirrorAdjustment::default()
            },
            [128, 64],
        );
        assert_eq!(
            y_line,
            [Vec2::new(0.0, 34.0 / 64.0), Vec2::new(1.0, 34.0 / 64.0)]
        );
    }

    #[test]
    fn transform_resize_cursors_follow_the_rotated_screen_axes() {
        let unrotated = test_overlay_geometry([
            egui::pos2(-2.0, -1.0),
            egui::pos2(2.0, -1.0),
            egui::pos2(2.0, 1.0),
            egui::pos2(-2.0, 1.0),
        ]);
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleRight, unrotated),
            egui::CursorIcon::ResizeHorizontal
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTop, unrotated),
            egui::CursorIcon::ResizeVertical
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTopLeft, unrotated),
            egui::CursorIcon::ResizeNwSe
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTopRight, unrotated),
            egui::CursorIcon::ResizeNeSw
        );

        let rotated_clockwise = test_overlay_geometry([
            egui::pos2(1.0, -2.0),
            egui::pos2(1.0, 2.0),
            egui::pos2(-1.0, 2.0),
            egui::pos2(-1.0, -2.0),
        ]);
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleRight, rotated_clockwise),
            egui::CursorIcon::ResizeVertical
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTop, rotated_clockwise),
            egui::CursorIcon::ResizeHorizontal
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTopLeft, rotated_clockwise),
            egui::CursorIcon::ResizeNeSw
        );
        assert_eq!(
            transform_resize_cursor(TransformHandle::ScaleTopRight, rotated_clockwise),
            egui::CursorIcon::ResizeNwSe
        );
    }
}
