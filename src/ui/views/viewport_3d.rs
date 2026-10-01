use eframe::egui::{self, PointerButton, Sense};
use glam::Vec2;

use crate::{
    application::{
        AppState, Command, ViewPointerEvent, ViewPointerMeta, ViewPointerPhase,
        ViewportInputContext, analyze_viewport_hover, plan_decal_overlay_request_for_viewport,
        plan_surface_mirror_plane_overlay_request,
    },
    core::{
        decal::{
            DECAL_HANDLE_HIT_RADIUS_PX, DECAL_HANDLE_RADIUS_PX, DecalHandleGeometry,
            hit_test_decal_handle,
        },
        document::RaycastScratch,
        stroke::StrokeSpace,
        tool::{ColorSampleSource, ToolBehavior},
        transform::TransformHandle,
    },
    localization::Localization,
    renderer::{
        BrushOverlayRequest, ColorSampleAnchor, ColorSampleIntent, ColorSampleTarget,
        ColorSampleUiRequest, ColorSampleView, MirrorPlaneOverlayRequest, SelectionOverlayRequest,
        ViewportViewRequest,
    },
    ui::{
        icons::UiIconRegistry,
        input::{
            modifiers::input_modifiers_from_egui,
            pointer_pressure::current_pointer_pressure,
            shortcut_profile::ShortcutProfile,
            view_navigation::{
                ViewNavigationAction, ViewNavigationKind, dominant_drag_delta,
                update_view_navigation,
            },
            view_pointer::{
                ViewPointerInputRouter, ViewPointerInputSample, ViewPointerTarget,
                pointer_down_position,
            },
        },
        render_resources::{UiRenderResources, VIEWPORT_BACKGROUND_RGB},
        view_output::{UiRequest, ViewOutput},
        widgets::{
            operation_commit_controls::{
                OperationCommitAction, draw_operation_commit_controls,
                operation_commit_controls_layout,
            },
            orientation_gizmo::draw_orientation_gizmo,
            viewport_toolbar::{ViewportToolbarKind, draw_viewport_toolbar},
        },
    },
};

use super::{
    brush_overlay_screen_position,
    color_picker::current_layer_surface,
    surface_brush_overlay_size_points,
    tool_cursor::{apply_tool_cursor, color_sample_position, draw_color_sample_preview},
    update_brush_size_drag,
};

const VIEW_DRAG_ZOOM_EXPONENT_PER_POINT: f32 = 0.01;

pub fn draw_viewport_3d(
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
    let available = ui.available_size();
    let desired = egui::vec2(available.x.max(100.0), available.y.max(100.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        4.0,
        egui::Color32::from_rgb(
            VIEWPORT_BACKGROUND_RGB[0],
            VIEWPORT_BACKGROUND_RGB[1],
            VIEWPORT_BACKGROUND_RGB[2],
        ),
    );
    let toolbar = draw_viewport_toolbar(
        ui,
        rect,
        ViewportToolbarKind::View3d,
        state,
        icons,
        l10n,
        shortcut_profile,
        &mut output,
    );
    let pointer_over_toolbar = toolbar.pointer_over_toolbar(ui.ctx());
    let mut block_new_view_input =
        !interaction_enabled || toolbar.popup_open || pointer_over_toolbar;

    let color_picker_active = state
        .effective_tool_definition()
        .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::ColorPicker));

    let viewport_size = [rect.width().round() as u32, rect.height().round() as u32];
    if state.view_projection_decal_start_requested() {
        output.push(Command::BeginViewProjectionDecal { viewport_size });
    } else if state
        .active_view_projection_decal_transform()
        .is_some_and(|transform| transform.viewport_size != viewport_size)
    {
        output.push(Command::SyncViewProjectionDecalViewport { viewport_size });
    }
    let view_proj = state
        .camera()
        .view_projection_matrix(rect.width().max(1.0) / rect.height().max(1.0));
    let inv_view_proj = view_proj.inverse();
    let cam_pos = state.camera().position();
    let viewport_context = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj,
        camera_world: [cam_pos.x, cam_pos.y, cam_pos.z],
    };
    let surface_decal_geometry = state
        .active_decal_handle_display_transform()
        .and_then(|transform| transform.handle_geometry(view_proj, viewport_size));
    let view_decal_geometry = state.active_view_projection_decal_geometry();
    let decal_geometry = surface_decal_geometry.or(view_decal_geometry);
    let operation_controls_layout = if state.has_active_decal_session() {
        decal_geometry.and_then(|geometry| {
            let corners = geometry
                .corners
                .map(|point| rect.min + egui::vec2(point.x, point.y));
            operation_commit_controls_layout(rect, corners)
        })
    } else {
        None
    };
    let operation_control_rect = operation_controls_layout.map(|layout| layout.rect);
    let gizmo = state.viewport_gizmo_visible().then(|| {
        draw_orientation_gizmo(
            ui.ctx(),
            ui.id().with("orientation_gizmo"),
            rect,
            state.camera(),
            l10n,
            interaction_enabled && state.can_edit_viewport_camera_settings(),
            operation_control_rect.as_slice(),
        )
    });
    if let Some(interaction) = gizmo {
        block_new_view_input |= interaction.pointer_over;
        if let Some(delta_radians) = interaction.orbit_radians {
            let mut camera = state.camera().clone();
            camera.orbit_radians(Vec2::new(delta_radians.x, delta_radians.y));
            output.push(Command::SetCamera(camera));
            output.request_repaint();
        } else if let Some(axis_view) = interaction.selected_view {
            let mut camera = state.camera().clone();
            let (forward, up_reference) = axis_view.camera_basis();
            camera.set_view_direction(forward, up_reference);
            output.push(Command::SetCamera(camera));
            output.request_repaint();
        }
    }

    let operation_controls = operation_controls_layout.map(|layout| {
        draw_operation_commit_controls(
            ui.ctx(),
            ui.id().with("decal_operation_commit_controls"),
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
                output.push(Command::ApplyActiveDecal);
            }
            Some(OperationCommitAction::Cancel) => {
                output.push(Command::CancelActiveDecal);
            }
            None => {}
        }
    }

    let navigation = update_view_navigation(
        ui.ctx(),
        ui.id().with("view_navigation"),
        ViewNavigationKind::Viewport3d,
        rect,
        interaction_enabled,
        !block_new_view_input
            && !state.is_tool_pointer_gesture_active()
            && !pointer_input.is_active(),
        (shortcut_profile, shortcut_profile_revision),
    );
    let mut hover_raycast_scratch = RaycastScratch::default();
    let brush_size_start_size_screen_points = (navigation.started()
        && navigation.action() == Some(ViewNavigationAction::BrushSize))
    .then(|| {
        analyze_viewport_hover(
            state,
            local_px(navigation.start_position(), rect),
            1.0,
            viewport_context,
            &mut hover_raycast_scratch,
        )
        .brush_overlay_request
        .as_ref()
        .and_then(|request| match request {
            BrushOverlayRequest::Surface(request) => surface_brush_overlay_size_points(request),
        })
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
    if let Some(action) = navigation.action() {
        let pointer_delta = navigation.pointer_delta();
        if pointer_delta != egui::Vec2::ZERO {
            let mut camera = state.camera().clone();
            let changed = match action {
                ViewNavigationAction::Orbit3d => {
                    camera.orbit(Vec2::new(pointer_delta.x, pointer_delta.y));
                    true
                }
                ViewNavigationAction::Pan3d => {
                    camera.pan(Vec2::new(pointer_delta.x, pointer_delta.y));
                    true
                }
                ViewNavigationAction::Zoom3d => {
                    camera.zoom_by_factor(
                        (dominant_drag_delta(pointer_delta) * VIEW_DRAG_ZOOM_EXPONENT_PER_POINT)
                            .exp(),
                    );
                    true
                }
                ViewNavigationAction::RotateUv
                | ViewNavigationAction::PanUv
                | ViewNavigationAction::ZoomUv
                | ViewNavigationAction::BrushSize => false,
            };
            if changed {
                output.request_repaint();
                output.push(Command::SetCamera(camera));
            }
        }
    }

    if !block_new_view_input
        && !navigation.owns_input()
        && !state.is_tool_pointer_gesture_active()
        && response.hovered()
    {
        let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            output.request_repaint();
            let mut camera = state.camera().clone();
            camera.zoom(scroll);
            output.push(Command::SetCamera(camera));
        }
    }

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
    let hover_analysis = brush_overlay_pointer.map(|pointer| {
        let viewport_pos = local_px(pointer, rect);
        let pressure_pointer = live_pointer.unwrap_or(pointer);
        let pressure = current_pointer_pressure(ui.ctx(), pressure_pointer);
        analyze_viewport_hover(
            state,
            viewport_pos,
            pressure,
            viewport_context,
            &mut hover_raycast_scratch,
        )
    });
    let brush_overlay_request = hover_analysis
        .as_ref()
        .and_then(|hover| hover.brush_overlay_request.clone());

    let mirror_plane_overlay_request = plan_surface_mirror_plane_overlay_request(state, view_proj);
    let decal_overlay_request =
        plan_decal_overlay_request_for_viewport(state, view_proj, viewport_size);
    let renderer_is_available = render_resources.renderer_available;
    let brush_overlay_visible = renderer_is_available && brush_overlay_request.is_some();
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
        && renderer_is_available
        && let Some(pointer) = color_sample_pointer
        && let Some(position) = color_sample_position(pointer, rect, viewport_size)
    {
        let surface_hit = hover_analysis.as_ref().and_then(|hover| hover.surface_hit);
        let resolved_target = match color_sample_source {
            ColorSampleSource::View => Some((
                ColorSampleTarget::ViewOutput {
                    view: ColorSampleView::Viewport3d,
                    position,
                },
                None,
            )),
            ColorSampleSource::CompositeTexture => surface_hit.map(|hit| {
                (
                    ColorSampleTarget::CompositeTexture {
                        material_index: hit.material_index.as_usize(),
                        uv: [hit.uv.x, hit.uv.y],
                    },
                    None,
                )
            }),
            ColorSampleSource::CurrentLayer => surface_hit.and_then(|hit| {
                current_layer_surface(state, hit.material_index.as_usize()).map(
                    |(surface, layer_id)| {
                        (
                            ColorSampleTarget::SurfaceTexture {
                                surface,
                                uv: [hit.uv.x, hit.uv.y],
                            },
                            Some(layer_id),
                        )
                    },
                )
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
                    view: ColorSampleView::Viewport3d,
                    position,
                },
                target,
                active_layer_id,
            }));
            output.request_repaint();
        }
    }
    if renderer_is_available {
        let selection_overlay_request = selection_overlay_request(ui.ctx(), state);
        if selection_overlay_request.is_some() {
            output.request_repaint();
        }
        output.request_viewport_render(ViewportViewRequest {
            camera: state.camera().clone(),
            view_proj,
            camera_world: viewport_context.camera_world,
            viewport_size,
            brush_overlay_request,
            selection_overlay_request,
            mirror_plane_overlay_request,
            decal_overlay_request,
            scene_visibility: state.viewport_scene_visibility().clone(),
            show_wireframe: state.viewport_wireframe_visible(),
            wireframe_style: state.viewport_wireframe_style(),
            background_color: state.viewport_background_color(),
            shading: state.viewport_shading(),
        });
    }
    if let Some(texture_id) = render_resources.viewport_texture_id {
        painter.image(
            texture_id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    if renderer_is_available && let Some(request) = mirror_plane_overlay_request {
        draw_mirror_plane_label(&painter, l10n, rect, view_proj, request);
    }

    if let (Some(transform), Some(texture_id)) = (
        state.active_view_projection_decal_transform(),
        render_resources.decal_thumbnail_texture_id,
    ) {
        draw_view_projection_image(&painter, rect, transform, texture_id);
    }
    if let Some(geometry) = decal_geometry {
        draw_decal_overlay(&painter, rect, geometry, state.active_decal_handle());
    }

    if let Some(preview) = state.surface_rect_drag_preview()
        && preview.viewport_size == viewport_size
    {
        let min = rect.min
            + egui::vec2(
                preview.start_px.x.min(preview.current_px.x),
                preview.start_px.y.min(preview.current_px.y),
            );
        let max = rect.min
            + egui::vec2(
                preview.start_px.x.max(preview.current_px.x),
                preview.start_px.y.max(preview.current_px.y),
            );
        painter.rect_stroke(
            egui::Rect::from_min_max(min, max),
            0.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 180, 255)),
            egui::StrokeKind::Inside,
        );
    }

    if let Some(preview) = state.surface_lasso_drag_preview()
        && preview.viewport_size == viewport_size
        && preview.points_px.len() >= 2
    {
        let mut screen_points: Vec<egui::Pos2> = preview
            .points_px
            .into_iter()
            .map(|point| rect.min + egui::vec2(point.x, point.y))
            .collect();
        screen_points.push(screen_points[0]);
        painter.add(egui::Shape::line(
            screen_points,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 180, 255)),
        ));
    }

    if let Some(action) = navigation.action() {
        ui.ctx().set_cursor_image(None);
        let cursor = match action {
            ViewNavigationAction::Orbit3d | ViewNavigationAction::Pan3d => {
                egui::CursorIcon::Grabbing
            }
            ViewNavigationAction::Zoom3d => egui::CursorIcon::ResizeNwSe,
            ViewNavigationAction::BrushSize => egui::CursorIcon::ResizeNwSe,
            ViewNavigationAction::RotateUv
            | ViewNavigationAction::PanUv
            | ViewNavigationAction::ZoomUv => unreachable!("UV navigation in 3D view"),
        };
        ui.ctx().set_cursor_icon(cursor);
    } else if !block_new_view_input
        && (response.hovered()
            || (color_picker_active && response.dragged_by(PointerButton::Primary)))
        && let Some(_pointer) = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|pointer| rect.contains(*pointer))
    {
        if let Some(geometry) = decal_geometry {
            if let Some(hover) = hover_analysis
                .as_ref()
                .filter(|hover| hover.paint_edit.blocked_reason().is_some())
            {
                apply_tool_cursor(
                    ui,
                    l10n,
                    state,
                    rect,
                    StrokeSpace::Surface,
                    brush_overlay_visible,
                    hover.paint_edit,
                );
            } else {
                apply_decal_cursor(ui, state, icons, rect, geometry);
            }
        } else if (!response.dragged_by(PointerButton::Primary) || color_picker_active)
            && let Some(hover) = hover_analysis.as_ref()
        {
            apply_tool_cursor(
                ui,
                l10n,
                state,
                rect,
                StrokeSpace::Surface,
                brush_overlay_visible,
                hover.paint_edit,
            );
        }
    }

    if color_picker_active && let Some(pointer) = color_sample_pointer {
        draw_color_sample_preview(
            ui,
            rect,
            pointer,
            ColorSampleView::Viewport3d,
            color_sample_source,
            state.document_generation(),
            render_resources.color_sample_preview,
        );
    }

    let active_tool_is_stroke = state
        .effective_tool_definition()
        .is_some_and(|tool| tool.is_stroke());
    let mut blocked_rects = vec![toolbar.rect];
    if let Some(interaction) = gizmo {
        blocked_rects.push(interaction.rect);
    }
    if let Some(interaction) = operation_controls {
        blocked_rects.push(interaction.rect);
    }
    let pointer_frame = pointer_input.collect_view_samples(
        ui.ctx(),
        ViewPointerTarget::Viewport3d,
        rect,
        &blocked_rects,
        interaction_enabled,
        active_tool_is_stroke
            && renderer_is_available
            && !toolbar.popup_open
            && !navigation.owns_input(),
    );
    if renderer_is_available {
        for reason in pointer_frame.cancellations.iter().copied() {
            output.push(Command::ViewPointer(ViewPointerEvent::Cancel { reason }));
        }
        for sample in pointer_frame.samples {
            push_raw_viewport_pointer_command(&mut output, sample, viewport_context);
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
        && renderer_is_available
    {
        push_viewport_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Click,
            pointer,
            rect,
            viewport_context,
        );
    }

    if response_primary_input_enabled
        && !block_new_view_input
        && response.drag_started_by(PointerButton::Primary)
        && let Some(current_pointer) = response.interact_pointer_pos()
        && renderer_is_available
    {
        let press_origin = ui.ctx().input(|input| input.pointer.press_origin());
        let pointer = pointer_down_position(current_pointer, press_origin, rect);
        push_viewport_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Down,
            pointer,
            rect,
            viewport_context,
        );
    }

    if response_primary_input_enabled
        && response.dragged_by(PointerButton::Primary)
        && let Some(pointer) = response.interact_pointer_pos()
        && renderer_is_available
    {
        push_viewport_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Move,
            pointer,
            rect,
            viewport_context,
        );
    }

    if response_primary_input_enabled
        && response.drag_stopped_by(PointerButton::Primary)
        && renderer_is_available
    {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        push_viewport_pointer_command(
            ui,
            &mut output,
            ViewPointerPhase::Up,
            pointer,
            rect,
            viewport_context,
        );
    }

    output
}

fn draw_view_projection_image(
    painter: &egui::Painter,
    viewport_rect: egui::Rect,
    transform: crate::core::decal::ViewProjectionDecalTransform,
    texture_id: egui::TextureId,
) {
    let corners = transform.corners_px();
    let mut mesh = egui::Mesh::with_texture(texture_id);
    let color = egui::Color32::from_white_alpha(90);
    for (point, uv) in corners.into_iter().zip([
        egui::pos2(0.0, 0.0),
        egui::pos2(1.0, 0.0),
        egui::pos2(1.0, 1.0),
        egui::pos2(0.0, 1.0),
    ]) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: viewport_rect.min + egui::vec2(point.x, point.y),
            uv,
            color,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_decal_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    geometry: DecalHandleGeometry,
    active_handle: Option<TransformHandle>,
) {
    let to_screen = |point: Vec2| rect.min + egui::vec2(point.x, point.y);
    let corners = geometry.corners.map(to_screen);
    let edge_centers = geometry.edge_centers.map(to_screen);
    let center = to_screen(geometry.center);
    let rotate = to_screen(geometry.rotate);
    let outline_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 180, 255));
    let mut outline = corners.to_vec();
    outline.push(corners[0]);
    painter.add(egui::Shape::line(outline, outline_stroke));
    painter.line_segment([edge_centers[0], rotate], outline_stroke);

    let handles = [
        (TransformHandle::ScaleTopLeft, corners[0]),
        (TransformHandle::ScaleTopRight, corners[1]),
        (TransformHandle::ScaleBottomRight, corners[2]),
        (TransformHandle::ScaleBottomLeft, corners[3]),
        (TransformHandle::ScaleTop, edge_centers[0]),
        (TransformHandle::ScaleRight, edge_centers[1]),
        (TransformHandle::ScaleBottom, edge_centers[2]),
        (TransformHandle::ScaleLeft, edge_centers[3]),
        (TransformHandle::Rotate, rotate),
    ];
    for (handle, point) in handles {
        let fill = if active_handle == Some(handle) {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            egui::Color32::WHITE
        };
        painter.circle(
            point,
            DECAL_HANDLE_RADIUS_PX,
            fill,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(40, 110, 180)),
        );
    }

    let center_color = if active_handle == Some(TransformHandle::Move) {
        egui::Color32::from_rgb(255, 210, 80)
    } else {
        egui::Color32::WHITE
    };
    painter.circle(
        center,
        4.0,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(1.5, center_color),
    );
    painter.line_segment(
        [center - egui::vec2(7.0, 0.0), center + egui::vec2(7.0, 0.0)],
        egui::Stroke::new(1.0, center_color),
    );
    painter.line_segment(
        [center - egui::vec2(0.0, 7.0), center + egui::vec2(0.0, 7.0)],
        egui::Stroke::new(1.0, center_color),
    );
}

fn apply_decal_cursor(
    ui: &egui::Ui,
    state: &AppState,
    icons: &UiIconRegistry,
    rect: egui::Rect,
    geometry: DecalHandleGeometry,
) {
    let handle = state.active_decal_handle().or_else(|| {
        let pointer = ui.ctx().pointer_hover_pos()?;
        let local = pointer - rect.min;
        hit_test_decal_handle(
            Vec2::new(local.x, local.y),
            geometry,
            DECAL_HANDLE_HIT_RADIUS_PX,
        )
    });
    if handle == Some(TransformHandle::Rotate) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        ui.ctx()
            .set_cursor_image(Some(icons.rotate_cursor().clone()));
        return;
    }

    ui.ctx().set_cursor_image(None);
    let cursor = match handle {
        Some(TransformHandle::Move) => egui::CursorIcon::Move,
        Some(TransformHandle::Rotate) => unreachable!("rotate cursor handled above"),
        Some(handle) => decal_resize_cursor(handle, geometry),
        None => egui::CursorIcon::Crosshair,
    };
    ui.ctx().set_cursor_icon(cursor);
}

fn decal_resize_cursor(handle: TransformHandle, geometry: DecalHandleGeometry) -> egui::CursorIcon {
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
    let octant = (direction.y.atan2(direction.x) / std::f32::consts::FRAC_PI_4).round() as i32;
    match octant.rem_euclid(4) {
        0 => egui::CursorIcon::ResizeHorizontal,
        1 => egui::CursorIcon::ResizeNwSe,
        2 => egui::CursorIcon::ResizeVertical,
        3 => egui::CursorIcon::ResizeNeSw,
        _ => unreachable!("rem_euclid(4) returns 0..=3"),
    }
}

fn draw_mirror_plane_label(
    painter: &egui::Painter,
    l10n: &Localization,
    rect: egui::Rect,
    view_proj: glam::Mat4,
    request: MirrorPlaneOverlayRequest,
) {
    let mut anchor = None;
    for y_sign in [-1.0, 1.0] {
        for z_sign in [-1.0, 1.0] {
            let world = glam::Vec3::new(
                request.plane_x,
                request.center_yz[0] + request.half_extent_yz[0] * y_sign,
                request.center_yz[1] + request.half_extent_yz[1] * z_sign,
            );
            let Some(local) = project_world_to_viewport(view_proj, world, rect.size()) else {
                continue;
            };
            if local.x < 0.0 || local.y < 0.0 || local.x > rect.width() || local.y > rect.height() {
                continue;
            }
            if anchor.is_none_or(|current: egui::Pos2| local.y < current.y) {
                anchor = Some(local);
            }
        }
    }
    let Some(anchor) = anchor else {
        return;
    };
    let mut args = fluent::FluentArgs::new();
    args.set("coordinate", format_plane_coordinate(request.plane_x));
    let text = l10n.format("viewport-mirror-plane-label", Some(&args));
    painter.text(
        rect.min + anchor.to_vec2() + egui::vec2(6.0, -6.0),
        egui::Align2::LEFT_BOTTOM,
        text,
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(80, 210, 255),
    );
}

fn project_world_to_viewport(
    view_proj: glam::Mat4,
    world: glam::Vec3,
    viewport_size: egui::Vec2,
) -> Option<egui::Pos2> {
    let clip = view_proj * world.extend(1.0);
    if !clip.is_finite() || clip.w <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() || ndc.z < -1.0 || ndc.z > 1.0 {
        return None;
    }
    Some(egui::pos2(
        (ndc.x * 0.5 + 0.5) * viewport_size.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size.y,
    ))
}

fn format_plane_coordinate(value: f32) -> String {
    let abs = value.abs();
    if abs >= 1_000_000.0 || (abs > 0.0 && abs < 0.001) {
        format!("{value:.3e}")
    } else {
        format!("{value:.3}")
    }
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

fn push_viewport_pointer_command(
    ui: &egui::Ui,
    output: &mut ViewOutput,
    phase: ViewPointerPhase,
    pointer: egui::Pos2,
    rect: egui::Rect,
    view: ViewportInputContext,
) {
    let modifiers = input_modifiers_from_egui(ui.ctx().input(|i| i.modifiers));
    let pressure = current_pointer_pressure(ui.ctx(), pointer);
    let position_px = local_px(pointer, rect);
    let time_s = ui.ctx().input(|input| input.time);
    push_viewport_pointer_sample(
        output,
        phase,
        position_px,
        pressure,
        time_s,
        modifiers,
        view,
    );
}

fn push_raw_viewport_pointer_command(
    output: &mut ViewOutput,
    sample: ViewPointerInputSample,
    view: ViewportInputContext,
) {
    push_viewport_pointer_sample(
        output,
        sample.phase,
        sample.position_px,
        sample.pressure,
        sample.time_s,
        sample.modifiers,
        view,
    );
}

fn push_viewport_pointer_sample(
    output: &mut ViewOutput,
    phase: ViewPointerPhase,
    position_px: Vec2,
    pressure: f32,
    time_s: f64,
    modifiers: crate::application::InputModifiers,
    view: ViewportInputContext,
) {
    output.request_repaint();
    output.push(Command::ViewPointer(ViewPointerEvent::Viewport3d {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decal_drag_start_hit_test_uses_press_origin() {
        let view_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(200.0, 200.0));
        let geometry = DecalHandleGeometry {
            corners: [
                Vec2::new(20.0, 20.0),
                Vec2::new(80.0, 20.0),
                Vec2::new(80.0, 80.0),
                Vec2::new(20.0, 80.0),
            ],
            edge_centers: [
                Vec2::new(50.0, 20.0),
                Vec2::new(80.0, 50.0),
                Vec2::new(50.0, 80.0),
                Vec2::new(20.0, 50.0),
            ],
            center: Vec2::new(50.0, 50.0),
            rotate: Vec2::new(50.0, 0.0),
        };
        let press_origin = egui::pos2(111.0, 120.0);
        let current_pointer = egui::pos2(105.0, 120.0);

        assert_eq!(
            hit_test_decal_handle(
                local_px(current_pointer, view_rect),
                geometry,
                DECAL_HANDLE_HIT_RADIUS_PX,
            ),
            None
        );

        let pointer = pointer_down_position(current_pointer, Some(press_origin), view_rect);

        assert_eq!(
            hit_test_decal_handle(
                local_px(pointer, view_rect),
                geometry,
                DECAL_HANDLE_HIT_RADIUS_PX,
            ),
            Some(TransformHandle::ScaleTopLeft)
        );
    }
}
