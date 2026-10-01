use crate::ui::widgets::interaction_gate::InteractionGate;
use eframe::egui;
use eframe::egui::{
    color_picker::{Alpha, color_picker_color32},
    containers::menu::SubMenuButton,
};

use crate::{
    application::{AppState, Command},
    core::camera::{
        CameraProjection, MAX_FOV_Y_DEGREES, MAX_ORTHOGRAPHIC_HEIGHT, MIN_FOV_Y_DEGREES,
        MIN_ORTHOGRAPHIC_HEIGHT,
    },
    core::viewport_shading::ViewportShading,
    localization::Localization,
    ui::{
        icons::UiIconRegistry,
        input::shortcut_profile::{
            PointerContext, PointerGestureAction, PointerGestureTrigger, ShortcutPointerButton,
            ShortcutProfile,
        },
        view_output::ViewOutput,
    },
};

const TOOLBAR_MARGIN: f32 = 6.0;
const TOOLBAR_ITEM_SPACING: f32 = 2.0;
const BUTTON_SIZE: f32 = 26.0;
const ICON_SIZE: f32 = 18.0;
const MENU_BUTTON_WIDTH: f32 = 14.0;
const POPUP_WIDTH: f32 = 210.0;
const CAMERA_POPUP_WIDTH: f32 = 240.0;
const HELP_POPUP_WIDTH: f32 = 340.0;
const HELP_INLINE_ICON_HEIGHT: f32 = 17.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewportToolbarKind {
    View3d,
    Uv,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewportToolbarInteraction {
    pub rect: egui::Rect,
    pub popup_open: bool,
}

impl ViewportToolbarInteraction {
    pub fn pointer_over_toolbar(self, ctx: &egui::Context) -> bool {
        ctx.pointer_hover_pos()
            .is_some_and(|pointer| self.rect.contains(pointer))
    }
}

pub fn draw_viewport_toolbar(
    ui: &mut egui::Ui,
    viewport_rect: egui::Rect,
    kind: ViewportToolbarKind,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    shortcut_profile: &ShortcutProfile,
    output: &mut ViewOutput,
) -> ViewportToolbarInteraction {
    let width = toolbar_width(kind);
    let position = egui::pos2(
        (viewport_rect.right() - TOOLBAR_MARGIN - width).max(viewport_rect.left() + TOOLBAR_MARGIN),
        viewport_rect.top() + TOOLBAR_MARGIN,
    );

    let area = egui::Area::new(egui::Id::new(("viewport_toolbar", kind)))
        .order(egui::Order::Middle)
        .fixed_pos(position)
        .movable(false)
        .show(ui.ctx(), |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ui.set_min_height(BUTTON_SIZE);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let mut popup_open = false;
                popup_open |= match kind {
                    ViewportToolbarKind::View3d => {
                        draw_3d_camera_controls(ui, state, icons, l10n, output)
                    }
                    ViewportToolbarKind::Uv => {
                        draw_uv_camera_controls(ui, state, icons, l10n, output)
                    }
                };
                ui.add_space(TOOLBAR_ITEM_SPACING);
                if kind == ViewportToolbarKind::View3d {
                    popup_open |= draw_mirror_controls(ui, state, icons, l10n, output);
                    ui.add_space(TOOLBAR_ITEM_SPACING);
                }
                popup_open |= draw_wireframe_controls(ui, kind, state, icons, l10n, output);
                ui.add_space(TOOLBAR_ITEM_SPACING);
                popup_open |= draw_viewport_settings(ui, kind, state, icons, l10n, output);
                ui.add_space(TOOLBAR_ITEM_SPACING);
                popup_open |= draw_view_help(ui, kind, icons, l10n, shortcut_profile);
                popup_open
            })
            .inner
        });

    ViewportToolbarInteraction {
        rect: area.response.rect,
        popup_open: area.inner,
    }
}

fn toolbar_width(kind: ViewportToolbarKind) -> f32 {
    let content_width = match kind {
        ViewportToolbarKind::View3d => {
            (BUTTON_SIZE + MENU_BUTTON_WIDTH) * 2.0 + BUTTON_SIZE * 3.0 + TOOLBAR_ITEM_SPACING * 4.0
        }
        ViewportToolbarKind::Uv => {
            BUTTON_SIZE + MENU_BUTTON_WIDTH + BUTTON_SIZE * 3.0 + TOOLBAR_ITEM_SPACING * 3.0
        }
    };
    content_width
}

fn draw_3d_camera_controls(
    ui: &mut egui::Ui,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    output: &mut ViewOutput,
) -> bool {
    let popup_id = egui::Id::new((
        "viewport_toolbar_popup",
        ViewportToolbarKind::View3d,
        "camera",
    ));
    let popup_was_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = icon_button(ui, icons, "builtin.icon.photo_camera", popup_was_open)
        .on_hover_text(l10n.text("viewport-camera"));

    let popup_shown = egui::Popup::menu(&response)
        .id(popup_id)
        .width(CAMERA_POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.availability_ui(true, state.is_stroking(), |ui| {
                ui.set_min_width(CAMERA_POPUP_WIDTH - 20.0);
                ui.label(l10n.text("viewport-camera"));
                ui.separator();

                let enabled = state.can_edit_viewport_camera_settings() || state.is_stroking();
                if ui
                    .add_available(
                        enabled,
                        egui::Button::new(l10n.text("viewport-reset-camera")),
                    )
                    .on_hover_text(l10n.text("viewport-reset-camera-3d-help"))
                    .clicked()
                {
                    output.push(Command::ResetViewportCamera);
                    output.request_repaint();
                    ui.close();
                }
                let can_frame_all = enabled
                    && state.document().is_some_and(|document| {
                        document
                            .mesh
                            .positions
                            .iter()
                            .any(|position| position.is_finite())
                    });
                if ui
                    .add_available(
                        can_frame_all,
                        egui::Button::new(l10n.text("viewport-frame-all")),
                    )
                    .on_hover_text(l10n.text("viewport-frame-all-help"))
                    .clicked()
                {
                    output.push(Command::FrameAllViewport);
                    output.request_repaint();
                    ui.close();
                }
                ui.separator();

                let mut camera = state.camera().clone();
                ui.label(l10n.text("viewport-projection"));
                let mut projection = camera.projection;
                let perspective_changed = ui
                    .add_available(
                        enabled,
                        egui::RadioButton::new(
                            projection == CameraProjection::Perspective,
                            l10n.text("viewport-projection-perspective"),
                        ),
                    )
                    .clicked();
                let orthographic_changed = ui
                    .add_available(
                        enabled,
                        egui::RadioButton::new(
                            projection == CameraProjection::Orthographic,
                            l10n.text("viewport-projection-orthographic"),
                        ),
                    )
                    .clicked();
                if perspective_changed {
                    projection = CameraProjection::Perspective;
                } else if orthographic_changed {
                    projection = CameraProjection::Orthographic;
                }
                if projection != camera.projection {
                    camera.set_projection(projection);
                    output.push(Command::SetCamera(camera.clone()));
                    output.request_repaint();
                }

                match camera.projection {
                    CameraProjection::Perspective => {
                        let mut degrees = camera.fov_y_radians.to_degrees();
                        if ui
                            .add_available(
                                enabled,
                                egui::DragValue::new(&mut degrees)
                                    .prefix(format!(
                                        "{} ",
                                        l10n.text("viewport-field-of-view-prefix")
                                    ))
                                    .suffix("°")
                                    .range(MIN_FOV_Y_DEGREES..=MAX_FOV_Y_DEGREES)
                                    .speed(0.25)
                                    .max_decimals(1),
                            )
                            .on_hover_text(l10n.text("viewport-field-of-view-help"))
                            .changed()
                        {
                            camera.set_fov_y_degrees(degrees);
                            output.push(Command::SetCamera(camera));
                            output.request_repaint();
                        }
                    }
                    CameraProjection::Orthographic => {
                        let mut height = camera.orthographic_height;
                        if ui
                            .add_available(
                                enabled,
                                egui::DragValue::new(&mut height)
                                    .prefix(format!(
                                        "{} ",
                                        l10n.text("viewport-orthographic-height-prefix")
                                    ))
                                    .range(MIN_ORTHOGRAPHIC_HEIGHT..=MAX_ORTHOGRAPHIC_HEIGHT)
                                    .speed(0.01)
                                    .max_decimals(3),
                            )
                            .on_hover_text(l10n.text("viewport-orthographic-height-help"))
                            .changed()
                        {
                            camera.set_orthographic_height(height);
                            output.push(Command::SetCamera(camera));
                            output.request_repaint();
                        }
                    }
                }
            });
        })
        .is_some();

    popup_was_open || popup_shown || response.clicked()
}

fn draw_uv_camera_controls(
    ui: &mut egui::Ui,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    output: &mut ViewOutput,
) -> bool {
    let popup_id = egui::Id::new(("viewport_toolbar_popup", ViewportToolbarKind::Uv, "camera"));
    let popup_was_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = icon_button(ui, icons, "builtin.icon.photo_camera", popup_was_open)
        .on_hover_text(l10n.text("viewport-camera"));

    let popup_shown = egui::Popup::menu(&response)
        .id(popup_id)
        .width(CAMERA_POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.availability_ui(true, state.is_stroking(), |ui| {
                ui.set_min_width(CAMERA_POPUP_WIDTH - 20.0);
                ui.label(l10n.text("viewport-camera"));
                ui.separator();
                if ui
                    .add_available(
                        state.document().is_some() && (state.is_tool_idle() || state.is_stroking()),
                        egui::Button::new(l10n.text("viewport-reset-camera")),
                    )
                    .on_hover_text(l10n.text("viewport-reset-camera-uv-help"))
                    .clicked()
                {
                    output.push(Command::ResetUvViewTransform);
                    output.request_repaint();
                    ui.close();
                }
            });
        })
        .is_some();

    popup_was_open || popup_shown || response.clicked()
}

fn draw_mirror_controls(
    ui: &mut egui::Ui,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    output: &mut ViewOutput,
) -> bool {
    let popup_id = egui::Id::new((
        "viewport_toolbar_popup",
        ViewportToolbarKind::View3d,
        "mirror",
    ));
    let popup_was_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let mirror_options = state.surface_mirror_options();

    let (main, menu) = split_icon_button(
        ui,
        icons,
        "builtin.icon.flip",
        mirror_options.x_enabled,
        popup_was_open,
        state.is_tool_idle() || state.is_stroking(),
        state.is_stroking(),
        &l10n.text("viewport-toggle-x-mirror"),
        &l10n.text("viewport-x-mirror-settings"),
    );

    if main.clicked() {
        output.push(Command::SetSurfaceMirrorXEnabled(!mirror_options.x_enabled));
        output.request_repaint();
    }

    let popup = egui::Popup::menu(&menu)
        .id(popup_id)
        .width(POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let popup_shown = popup
        .show(|ui| {
            ui.set_min_width(POPUP_WIDTH - 20.0);
            ui.label(l10n.text("viewport-x-mirror"));
            ui.separator();

            let mut show_plane = mirror_options.show_x_plane;
            if ui
                .add_available(
                    mirror_options.x_enabled,
                    egui::Checkbox::new(&mut show_plane, l10n.text("viewport-show-mirror-plane")),
                )
                .on_hover_text(l10n.text("viewport-show-mirror-plane-help"))
                .changed()
            {
                output.push(Command::SetSurfaceMirrorXPlaneVisible(show_plane));
                output.request_repaint();
            }

            let mut plane_x = mirror_options.x_plane;
            if ui
                .availability_ui(
                    state.is_tool_idle() || state.is_stroking(),
                    state.is_stroking(),
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut plane_x)
                                .prefix(format!(
                                    "{} ",
                                    l10n.text("viewport-mirror-center-x-prefix")
                                ))
                                .speed(0.01),
                        )
                    },
                )
                .inner
                .on_hover_text(l10n.text("viewport-mirror-center-x-help"))
                .changed()
            {
                output.push(Command::SetSurfaceMirrorXPlane(plane_x));
                output.request_repaint();
            }
        })
        .is_some();

    popup_was_open || popup_shown || menu.clicked()
}

fn color_submenu_row(ui: &mut egui::Ui, label: &str, color: &mut [f32; 3], tooltip: &str) -> bool {
    let rgba = egui::Rgba::from_rgb(color[0], color[1], color[2]);
    let mut picker_color = egui::Color32::from(rgba);
    let button = egui::Button::new((
        label,
        egui::Atom::grow(),
        egui::RichText::new("■").size(18.0).color(picker_color),
        egui::RichText::new(SubMenuButton::RIGHT_ARROW),
    ));
    let (response, picker) = SubMenuButton::from_button(button).ui(ui, |ui| {
        ui.spacing_mut().slider_width = 200.0;
        color_picker_color32(ui, &mut picker_color, Alpha::Opaque)
    });
    response.on_hover_text(tooltip);

    let changed = picker.is_some_and(|picker| picker.inner);
    if changed {
        let rgba = egui::Rgba::from(picker_color);
        color[0] = rgba.r();
        color[1] = rgba.g();
        color[2] = rgba.b();
    }
    changed
}

fn draw_wireframe_controls(
    ui: &mut egui::Ui,
    kind: ViewportToolbarKind,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    output: &mut ViewOutput,
) -> bool {
    let popup_id = egui::Id::new(("viewport_toolbar_popup", kind, "wireframe"));
    let open_state_id = popup_id.with("open");
    let mut popup_open = ui
        .ctx()
        .data_mut(|data| data.get_temp::<bool>(open_state_id))
        .unwrap_or(false);
    let visible = match kind {
        ViewportToolbarKind::View3d => state.viewport_wireframe_visible(),
        ViewportToolbarKind::Uv => state.uv_wireframe_visible(),
    };

    let (main, menu) = split_icon_button(
        ui,
        icons,
        "builtin.icon.mesh",
        visible,
        popup_open,
        true,
        false,
        &l10n.text("viewport-toggle-wireframe"),
        &l10n.text("viewport-wireframe-settings"),
    );

    if main.clicked() {
        output.push(match kind {
            ViewportToolbarKind::View3d => Command::SetViewportWireframeVisible(!visible),
            ViewportToolbarKind::Uv => Command::SetUvWireframeVisible(!visible),
        });
        output.request_repaint();
    }

    if menu.clicked() {
        popup_open = !popup_open;
    }
    let popup = egui::Popup::menu(&menu)
        .id(popup_id)
        .open_bool(&mut popup_open)
        .width(POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let popup_shown = popup
        .show(|ui| {
            ui.set_min_width(POPUP_WIDTH - 20.0);
            ui.label(l10n.text("viewport-wireframe"));
            ui.separator();
            let mut style = match kind {
                ViewportToolbarKind::View3d => state.viewport_wireframe_style(),
                ViewportToolbarKind::Uv => state.uv_wireframe_style(),
            };
            if color_submenu_row(
                ui,
                &l10n.text("field-color"),
                &mut style.color,
                &l10n.text("viewport-wireframe-color-help"),
            ) {
                output.push(match kind {
                    ViewportToolbarKind::View3d => Command::SetViewportWireframeStyle(style),
                    ViewportToolbarKind::Uv => Command::SetUvWireframeStyle(style),
                });
                output.request_repaint();
            }
            let mut opacity_percent = style.opacity * 100.0;
            if ui
                .add(
                    egui::Slider::new(&mut opacity_percent, 0.0..=100.0)
                        .text(l10n.text("field-opacity"))
                        .suffix("%"),
                )
                .changed()
            {
                style.opacity = opacity_percent / 100.0;
                output.push(match kind {
                    ViewportToolbarKind::View3d => Command::SetViewportWireframeStyle(style),
                    ViewportToolbarKind::Uv => Command::SetUvWireframeStyle(style),
                });
                output.request_repaint();
            }
        })
        .is_some();

    ui.ctx()
        .data_mut(|data| data.insert_temp(open_state_id, popup_open));

    popup_open || popup_shown || menu.clicked()
}

fn draw_viewport_settings(
    ui: &mut egui::Ui,
    kind: ViewportToolbarKind,
    state: &AppState,
    icons: &UiIconRegistry,
    l10n: &Localization,
    output: &mut ViewOutput,
) -> bool {
    let popup_id = egui::Id::new(("viewport_toolbar_popup", kind, "settings"));
    // Keep the parent outside egui's single memory-managed popup slot so the nested color
    // picker can open.
    let open_state_id = popup_id.with("open");
    let mut popup_open = ui
        .ctx()
        .data_mut(|data| data.get_temp::<bool>(open_state_id))
        .unwrap_or(false);
    let response = icon_button(ui, icons, "builtin.icon.masked_transitions", popup_open)
        .on_hover_text(l10n.text("viewport-settings"));
    if response.clicked() {
        popup_open = !popup_open;
    }

    let popup = egui::Popup::menu(&response)
        .id(popup_id)
        .open_bool(&mut popup_open)
        .width(POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let popup_shown = popup
        .show(|ui| {
            ui.set_min_width(POPUP_WIDTH - 20.0);
            ui.label(l10n.text("viewport-settings"));
            ui.separator();
            if kind == ViewportToolbarKind::View3d {
                ui.label(l10n.text("viewport-shading"));
                let shading = state.viewport_shading();
                if ui
                    .radio(
                        shading == ViewportShading::Unlit,
                        l10n.text("viewport-shading-unlit"),
                    )
                    .clicked()
                {
                    output.push(Command::SetViewportShading(ViewportShading::Unlit));
                    output.request_repaint();
                }
                if ui
                    .radio(
                        shading == ViewportShading::Shade,
                        l10n.text("viewport-shading-shade"),
                    )
                    .clicked()
                {
                    output.push(Command::SetViewportShading(ViewportShading::Shade));
                    output.request_repaint();
                }
                ui.separator();
                let mut gizmo_visible = state.viewport_gizmo_visible();
                if ui
                    .checkbox(&mut gizmo_visible, l10n.text("viewport-show-gizmo"))
                    .on_hover_text(l10n.text("viewport-show-gizmo-help"))
                    .changed()
                {
                    output.push(Command::SetViewportGizmoVisible(gizmo_visible));
                    output.request_repaint();
                }
                ui.separator();
            }
            if kind == ViewportToolbarKind::Uv {
                let transform = state.uv_view_transform();
                let mut args = fluent::FluentArgs::new();
                args.set("degrees", transform.rotation_radians.to_degrees());
                ui.label(l10n.format("viewport-rotation", Some(&args)));
                if ui
                    .add_available(
                        transform.rotation_radians.abs() > f32::EPSILON,
                        egui::Button::new(l10n.text("viewport-reset-rotation")),
                    )
                    .clicked()
                {
                    output.push(Command::SetUvViewTransform(transform.with_rotation(0.0)));
                    output.request_repaint();
                }
                ui.separator();
            }
            let mut background_color = match kind {
                ViewportToolbarKind::View3d => state.viewport_background_color(),
                ViewportToolbarKind::Uv => state.uv_view_background_color(),
            };
            let tooltip_key = match kind {
                ViewportToolbarKind::View3d => "viewport-background-color-3d-help",
                ViewportToolbarKind::Uv => "viewport-background-color-uv-help",
            };
            if color_submenu_row(
                ui,
                &l10n.text("viewport-background-color"),
                &mut background_color,
                &l10n.text(tooltip_key),
            ) {
                output.push(match kind {
                    ViewportToolbarKind::View3d => {
                        Command::SetViewportBackgroundColor(background_color)
                    }
                    ViewportToolbarKind::Uv => Command::SetUvViewBackgroundColor(background_color),
                });
                output.request_repaint();
            }
        })
        .is_some();

    ui.ctx()
        .data_mut(|data| data.insert_temp(open_state_id, popup_open));

    popup_open || popup_shown || response.clicked()
}

const VIEW3D_HELP_ACTIONS: &[PointerGestureAction] = &[
    PointerGestureAction::Orbit3d,
    PointerGestureAction::Pan3d,
    PointerGestureAction::Zoom3d,
    PointerGestureAction::BrushSize,
];

const UV_HELP_ACTIONS: &[PointerGestureAction] = &[
    PointerGestureAction::RotateUv,
    PointerGestureAction::PanUv,
    PointerGestureAction::ZoomUv,
    PointerGestureAction::BrushSize,
];

fn view_help_content(
    kind: ViewportToolbarKind,
) -> (
    &'static str,
    PointerContext,
    &'static [PointerGestureAction],
) {
    match kind {
        ViewportToolbarKind::View3d => (
            "viewport-controls-3d",
            PointerContext::Viewport3d,
            VIEW3D_HELP_ACTIONS,
        ),
        ViewportToolbarKind::Uv => ("viewport-controls-uv", PointerContext::Uv, UV_HELP_ACTIONS),
    }
}

fn draw_view_help(
    ui: &mut egui::Ui,
    kind: ViewportToolbarKind,
    icons: &UiIconRegistry,
    l10n: &Localization,
    shortcut_profile: &ShortcutProfile,
) -> bool {
    let popup_id = egui::Id::new(("viewport_toolbar_popup", kind, "help"));
    let popup_was_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = icon_button(ui, icons, "builtin.icon.help", popup_was_open)
        .on_hover_text(l10n.text("viewport-controls"));

    let popup_shown = egui::Popup::menu(&response)
        .id(popup_id)
        .width(HELP_POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(HELP_POPUP_WIDTH - 20.0);
            let (title, context, actions) = view_help_content(kind);
            ui.label(egui::RichText::new(l10n.text(title)).strong());
            ui.separator();
            for action in actions {
                draw_view_help_row(ui, icons, l10n, shortcut_profile, context, *action);
            }
            if kind == ViewportToolbarKind::Uv {
                ui.add_space(6.0);
                ui.label(l10n.text("viewport-rotate-snap-help"));
            }
        })
        .is_some();

    popup_was_open || popup_shown || response.clicked()
}

fn draw_view_help_row(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    l10n: &Localization,
    shortcut_profile: &ShortcutProfile,
    context: PointerContext,
    action: PointerGestureAction,
) {
    let triggers = shortcut_profile.pointer_triggers_for_action(context, action);
    let includes_scroll = matches!(
        action,
        PointerGestureAction::Zoom3d | PointerGestureAction::ZoomUv
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(format!("{}:", pointer_action_name(l10n, action))).strong());

        if triggers.is_empty() {
            ui.label(l10n.text("shortcut-unassigned"));
        }
        for (index, trigger) in triggers.iter().enumerate() {
            if index > 0 {
                ui.label(",");
            }
            draw_pointer_gesture_trigger(ui, icons, l10n, trigger);
        }
        if includes_scroll {
            ui.label(",");
            draw_inline_help_icon(ui, icons, ViewHelpInputIcon::Scroll);
        }
    });
}

fn draw_pointer_gesture_trigger(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    l10n: &Localization,
    trigger: &PointerGestureTrigger,
) {
    for (enabled, label) in [
        (trigger.modifiers.ctrl, "Ctrl"),
        (trigger.modifiers.shift, "Shift"),
        (trigger.modifiers.alt, "Alt"),
    ] {
        if enabled {
            ui.label(label);
            ui.label("+");
        }
    }

    match trigger.button {
        ShortcutPointerButton::Primary => {
            draw_inline_help_icon(ui, icons, ViewHelpInputIcon::MouseLeft)
        }
        ShortcutPointerButton::Middle => {
            draw_inline_help_icon(ui, icons, ViewHelpInputIcon::MouseMiddle)
        }
        ShortcutPointerButton::Secondary => {
            draw_inline_help_icon(ui, icons, ViewHelpInputIcon::MouseRight)
        }
        ShortcutPointerButton::Extra1 | ShortcutPointerButton::Extra2 => {
            ui.label(
                egui::RichText::new(pointer_button_name(l10n, trigger.button))
                    .monospace()
                    .background_color(ui.visuals().faint_bg_color),
            );
        }
    }
}

fn pointer_action_name(l10n: &Localization, action: PointerGestureAction) -> String {
    let key = match action {
        PointerGestureAction::Orbit3d => "pointer-action-orbit",
        PointerGestureAction::Pan3d => "pointer-action-pan",
        PointerGestureAction::Zoom3d => "pointer-action-zoom",
        PointerGestureAction::RotateUv => "pointer-action-rotate",
        PointerGestureAction::PanUv => "pointer-action-pan",
        PointerGestureAction::ZoomUv => "pointer-action-zoom",
        PointerGestureAction::BrushSize => "pointer-action-brush-size",
    };
    l10n.text(key)
}

fn pointer_button_name(l10n: &Localization, button: ShortcutPointerButton) -> String {
    let key = match button {
        ShortcutPointerButton::Primary => "pointer-button-primary",
        ShortcutPointerButton::Middle => "pointer-button-middle",
        ShortcutPointerButton::Secondary => "pointer-button-secondary",
        ShortcutPointerButton::Extra1 => "pointer-button-extra-1",
        ShortcutPointerButton::Extra2 => "pointer-button-extra-2",
    };
    l10n.text(key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewHelpInputIcon {
    MouseLeft,
    MouseRight,
    MouseMiddle,
    Scroll,
}

impl ViewHelpInputIcon {
    fn icon_id(self) -> &'static str {
        match self {
            Self::MouseLeft => "builtin.icon.mouse_left_click",
            Self::MouseRight => "builtin.icon.mouse_right_click",
            Self::MouseMiddle => "builtin.icon.mouse_middle_click",
            Self::Scroll => "builtin.icon.scroll",
        }
    }
}

fn draw_inline_help_icon(ui: &mut egui::Ui, icons: &UiIconRegistry, icon: ViewHelpInputIcon) {
    let texture = icons
        .texture(icon.icon_id())
        .expect("viewport help icon must be registered");
    let texture_size = texture.size();
    let size = egui::vec2(
        HELP_INLINE_ICON_HEIGHT * texture_size[0] as f32 / texture_size[1] as f32,
        HELP_INLINE_ICON_HEIGHT,
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            ui.visuals().text_color(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn split_icon_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    active: bool,
    popup_open: bool,
    main_enabled: bool,
    main_input_blocked: bool,
    main_tooltip: &str,
    menu_tooltip: &str,
) -> (egui::Response, egui::Response) {
    let main = ui
        .availability_ui(true, main_input_blocked, |ui| {
            icon_button_with_rounding(
                ui,
                icons,
                icon_id,
                active,
                main_enabled,
                ButtonRounding::Left,
            )
        })
        .inner
        .on_hover_text(main_tooltip);
    let menu = menu_button(ui, popup_open, ButtonRounding::Right).on_hover_text(menu_tooltip);
    (main, menu)
}

fn icon_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    selected: bool,
) -> egui::Response {
    icon_button_with_rounding(ui, icons, icon_id, selected, true, ButtonRounding::All)
}

fn icon_button_with_rounding(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    selected: bool,
    enabled: bool,
    rounding: ButtonRounding,
) -> egui::Response {
    let button_size = egui::Vec2::splat(BUTTON_SIZE);
    let icon_size = egui::Vec2::splat(ICON_SIZE);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(button_size, sense);

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        paint_button_frame(ui, rect, &visuals, rounding);

        if let Some(texture) = icons.texture(icon_id) {
            let tint = if enabled {
                visuals.fg_stroke.color
            } else {
                ui.visuals().widgets.noninteractive.fg_stroke.color
            };
            let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
    }

    response
}

fn menu_button(ui: &mut egui::Ui, selected: bool, rounding: ButtonRounding) -> egui::Response {
    let size = egui::vec2(MENU_BUTTON_WIDTH, BUTTON_SIZE);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        paint_button_frame(ui, rect, &visuals, rounding);

        let center = rect.center() + egui::vec2(0.0, 0.5);
        let half_width = 2.5;
        let half_height = 1.5;
        let points = vec![
            center + egui::vec2(-half_width, -half_height),
            center + egui::vec2(half_width, -half_height),
            center + egui::vec2(0.0, half_height),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            visuals.fg_stroke.color,
            egui::Stroke::NONE,
        ));
    }

    response
}

fn paint_button_frame(
    ui: &egui::Ui,
    rect: egui::Rect,
    visuals: &egui::style::WidgetVisuals,
    rounding: ButtonRounding,
) {
    let visual_rect = rect.expand(visuals.expansion);
    let corner_radius = rounding.apply(visuals.corner_radius);
    ui.painter()
        .rect_filled(visual_rect, corner_radius, visuals.weak_bg_fill);
    ui.painter().rect_stroke(
        visual_rect,
        corner_radius,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

#[derive(Debug, Clone, Copy)]
enum ButtonRounding {
    All,
    Left,
    Right,
}

impl ButtonRounding {
    fn apply(self, radius: egui::CornerRadius) -> egui::CornerRadius {
        match self {
            Self::All => radius,
            Self::Left => egui::CornerRadius {
                nw: radius.nw,
                ne: 0,
                sw: radius.sw,
                se: 0,
            },
            Self::Right => egui::CornerRadius {
                nw: 0,
                ne: radius.ne,
                sw: 0,
                se: radius.se,
            },
        }
    }
}
