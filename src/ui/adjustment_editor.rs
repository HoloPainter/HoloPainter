use crate::ui::widgets::interaction_gate::InteractionGate;
use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::{
        adjustment::{
            Adjustment, GRADIENT_LOCATION_MAX, GradientMapAdjustment, GradientStop,
            UvMirrorAdjustment, UvMirrorAxis, UvMirrorDirection,
        },
        surface::{LayerId, LayerMaterialMask},
    },
    localization::Localization,
    ui::{
        adjustment_controls::{
            AdjustmentControlsOutput, AdjustmentControlsState, draw_adjustment_controls,
        },
        adjustment_gradient_editor::draw_gradient_editor,
        view_output::ViewOutput,
    },
};

const ADJUSTMENT_EDITOR_WINDOW_ID: &str = "adjustment_editor_window";
const ADJUSTMENT_EDITOR_DEFAULT_SIZE: [f32; 2] = [460.0, 540.0];
const ADJUSTMENT_EDITOR_MIN_WIDTH: f32 = 360.0;
const ADJUSTMENT_EDITOR_MIN_HEIGHT: f32 = 240.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdjustmentEditorTarget {
    layer_id: LayerId,
    document_generation: u64,
}

#[derive(Default)]
pub struct AdjustmentEditorState {
    open: bool,
    target: Option<AdjustmentEditorTarget>,
    edit_session: Option<AdjustmentEditSession>,
    next_edit_session: u64,
    controls: AdjustmentControlsState,
    gradient_selected_stop: Option<usize>,
}

impl AdjustmentEditorState {
    pub fn open_for(&mut self, layer_id: LayerId, document_generation: u64) {
        let target = AdjustmentEditorTarget {
            layer_id,
            document_generation,
        };
        if self.target != Some(target) {
            self.reset_edit_state();
        }
        self.target = Some(target);
        self.open = true;
    }

    fn close(&mut self) {
        self.open = false;
        self.target = None;
        self.reset_edit_state();
    }

    fn reset_edit_state(&mut self) {
        self.edit_session = None;
        self.controls.reset();
        self.gradient_selected_stop = None;
    }

    fn valid_target(&mut self, state: &AppState) -> Option<(LayerId, Adjustment, String)> {
        if !self.open {
            return None;
        }
        let Some(target) = self.target else {
            self.close();
            return None;
        };
        if target.document_generation != state.document_generation() {
            self.close();
            return None;
        }
        let Some(document) = state.document() else {
            self.close();
            return None;
        };
        let Some(node) = document.layer_tree.get(target.layer_id) else {
            self.close();
            return None;
        };
        let Some(adjustment) = document.layer_tree.adjustment(target.layer_id) else {
            self.close();
            return None;
        };
        Some((target.layer_id, adjustment, node.props.name.clone()))
    }
}

#[derive(Debug, Clone, Copy)]
struct AdjustmentEditSession {
    layer_id: LayerId,
    edit_session: u64,
}

pub fn draw_adjustment_editor_window(
    ctx: &egui::Context,
    l10n: &Localization,
    state: &AppState,
    ui_state: &mut AdjustmentEditorState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some((layer_id, adjustment, layer_name)) = ui_state.valid_target(state) else {
        return output;
    };

    let layer_locked = state
        .document()
        .is_some_and(|document| document.layer_tree.is_effectively_locked(layer_id));
    let mut open = ui_state.open;
    let was_open = open;
    let mut command = None;
    let mut title_args = fluent::FluentArgs::new();
    title_args.set("layer", layer_name.clone());
    let title = l10n.format("adjustment-editor-title", Some(&title_args));
    egui::Window::new(title)
        .id(egui::Id::new(ADJUSTMENT_EDITOR_WINDOW_ID))
        .open(&mut open)
        .default_size(ADJUSTMENT_EDITOR_DEFAULT_SIZE)
        .resizable(true)
        .show(ctx, |ui| {
            ui.set_min_width(ADJUSTMENT_EDITOR_MIN_WIDTH);
            ui.set_min_height(ADJUSTMENT_EDITOR_MIN_HEIGHT);
            ui.heading(adjustment_kind_label(l10n, adjustment.kind()));
            let mut args = fluent::FluentArgs::new();
            args.set("layer", layer_name.clone());
            ui.label(l10n.format("adjustment-editor-layer", Some(&args)));
            ui.separator();

            let ui_enabled =
                (!state.is_document_edit_interacting() || state.is_stroking()) && !layer_locked;
            if layer_locked {
                ui.label(l10n.text("adjustment-editor-locked"));
            }

            // Keep the non-shrinking scroll area last so the lock status row reduces its
            // available height instead of increasing the window's remembered desired size.
            ui.availability_ui(ui_enabled, state.is_stroking(), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("adjustment_editor_window_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        draw_adjustment_contents(
                            ui,
                            l10n,
                            state,
                            layer_id,
                            adjustment,
                            ui_state,
                            &mut command,
                        );
                    });
            });
        });

    ui_state.open = open;
    if was_open && !open {
        ui_state.reset_edit_state();
    }
    if let Some(command) = command {
        output.push(command);
        output.request_repaint();
    }
    output
}

fn draw_adjustment_contents(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    layer_id: LayerId,
    adjustment: Adjustment,
    ui_state: &mut AdjustmentEditorState,
    command: &mut Option<Command>,
) {
    match adjustment {
        Adjustment::BrightnessContrast(_)
        | Adjustment::Levels(_)
        | Adjustment::Curves(_)
        | Adjustment::HueSaturation(_) => {
            let mut edited = adjustment;
            let controls_output = draw_adjustment_controls(
                ui,
                l10n,
                &mut edited,
                &mut ui_state.controls,
                crate::core::surface_filter::SurfaceFilterDomain::Color,
            );
            queue_controls_change(ui_state, layer_id, edited, controls_output, command);
        }
        Adjustment::Invert => {
            let mut edited = adjustment;
            draw_adjustment_controls(
                ui,
                l10n,
                &mut edited,
                &mut ui_state.controls,
                crate::core::surface_filter::SurfaceFilterDomain::Color,
            );
            ui.label(l10n.text("adjustment-invert-opacity-help"));
        }
        Adjustment::GradientMap(mut gradient) => {
            draw_gradient_map_contents(ui, l10n, layer_id, &mut gradient, ui_state, command);
        }
        Adjustment::UvMirror(mut value) => {
            draw_uv_mirror_material_selector(ui, l10n, state, layer_id, command);
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.label(l10n.text("adjustment-axis"));
                let before = value.axis;
                ui.selectable_value(&mut value.axis, UvMirrorAxis::X, "X");
                ui.selectable_value(&mut value.axis, UvMirrorAxis::Y, "Y");
                if value.axis != before {
                    queue_adjustment_discrete_change(
                        ui_state,
                        layer_id,
                        Adjustment::UvMirror(value),
                        command,
                    );
                }
            });

            let direction_label = uv_mirror_direction_label(l10n, value.axis, value.direction);
            let before = value.direction;
            egui::ComboBox::from_id_salt(("uv_mirror_direction", layer_id))
                .selected_text(direction_label)
                .show_ui(ui, |ui| match value.axis {
                    UvMirrorAxis::X => {
                        ui.selectable_value(
                            &mut value.direction,
                            UvMirrorDirection::PositiveToNegative,
                            l10n.text("adjustment-direction-right-left"),
                        );
                        ui.selectable_value(
                            &mut value.direction,
                            UvMirrorDirection::NegativeToPositive,
                            l10n.text("adjustment-direction-left-right"),
                        );
                    }
                    UvMirrorAxis::Y => {
                        ui.selectable_value(
                            &mut value.direction,
                            UvMirrorDirection::PositiveToNegative,
                            l10n.text("adjustment-direction-bottom-top"),
                        );
                        ui.selectable_value(
                            &mut value.direction,
                            UvMirrorDirection::NegativeToPositive,
                            l10n.text("adjustment-direction-top-bottom"),
                        );
                    }
                });
            if value.direction != before {
                queue_adjustment_discrete_change(
                    ui_state,
                    layer_id,
                    Adjustment::UvMirror(value),
                    command,
                );
            }

            let mut position_percent = value.position * 100.0;
            let response = ui.add(
                egui::Slider::new(&mut position_percent, 0.0..=100.0)
                    .text(l10n.text("adjustment-position"))
                    .suffix("%"),
            );
            if response.changed() {
                value.position = position_percent / 100.0;
                queue_adjustment_change(
                    ui_state,
                    layer_id,
                    Adjustment::UvMirror(value),
                    &response,
                    command,
                );
            }

            if ui.button(l10n.text("action-reset")).clicked() {
                queue_adjustment_discrete_change(
                    ui_state,
                    layer_id,
                    Adjustment::UvMirror(UvMirrorAdjustment::default()),
                    command,
                );
            }
        }
    }
}

fn draw_uv_mirror_material_selector(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    layer_id: LayerId,
    command: &mut Option<Command>,
) {
    let Some(document) = state.document() else {
        return;
    };
    let selected_material = document
        .layer_tree
        .layer_material_mask(layer_id)
        .and_then(|mask| match mask {
            LayerMaterialMask::Specified(material_ids) if material_ids.len() == 1 => {
                material_ids.iter().next().copied()
            }
            _ => None,
        });
    let selected_text = selected_material
        .and_then(|material_id| {
            document
                .materials
                .iter()
                .enumerate()
                .find(|(_, material)| material.id == material_id)
                .map(|(index, material)| uv_mirror_material_name(l10n, index, &material.name))
        })
        .unwrap_or_else(|| l10n.text("adjustment-material-unassigned"));

    ui.horizontal(|ui| {
        ui.label(l10n.text("material-title"));
        egui::ComboBox::from_id_salt(("uv_mirror_material", layer_id))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for (index, material) in document.materials.iter().enumerate() {
                    let selected = selected_material == Some(material.id);
                    if ui
                        .selectable_label(
                            selected,
                            uv_mirror_material_name(l10n, index, &material.name),
                        )
                        .clicked()
                    {
                        *command = Some(Command::SetLayerMaterialMask {
                            layer_id,
                            material_mask: LayerMaterialMask::Specified(
                                [material.id].into_iter().collect(),
                            ),
                        });
                    }
                }
            });
    });
}

fn uv_mirror_material_name(l10n: &Localization, index: usize, name: &str) -> String {
    if name.is_empty() {
        let mut args = fluent::FluentArgs::new();
        args.set("index", index as i64);
        l10n.format("material-fallback-name", Some(&args))
    } else {
        name.to_owned()
    }
}

fn draw_gradient_map_contents(
    ui: &mut egui::Ui,
    l10n: &Localization,
    layer_id: LayerId,
    gradient: &mut GradientMapAdjustment,
    ui_state: &mut AdjustmentEditorState,
    command: &mut Option<Command>,
) {
    let editor_output = draw_gradient_editor(ui, gradient, &mut ui_state.gradient_selected_stop);
    if editor_output.changed {
        let adjustment = Adjustment::GradientMap(gradient.clone());
        if editor_output.discrete_change {
            queue_adjustment_discrete_change(ui_state, layer_id, adjustment, command);
        } else {
            queue_adjustment_change(
                ui_state,
                layer_id,
                adjustment,
                &editor_output.response,
                command,
            );
        }
    } else if editor_output.response.drag_stopped() {
        ui_state.edit_session = None;
    }

    if let Some(index) = ui_state.gradient_selected_stop
        && let Some(stop) = gradient.stop(index)
    {
        ui.separator();
        let mut args = fluent::FluentArgs::new();
        args.set("index", (index + 1) as i64);
        args.set("count", gradient.stops().len() as i64);
        ui.label(l10n.format("adjustment-stop-position", Some(&args)));
        let mut position = stop.location as f32 * 100.0 / GRADIENT_LOCATION_MAX as f32;
        let position_response = ui.add(
            egui::DragValue::new(&mut position)
                .prefix(l10n.text("adjustment-position-prefix"))
                .suffix("%")
                .range(0.0..=100.0)
                .speed(0.5),
        );
        if position_response.changed() {
            let mut edited = stop;
            edited.location = (position * GRADIENT_LOCATION_MAX as f32 / 100.0).round() as u16;
            if gradient.set_stop(index, edited) {
                queue_adjustment_change(
                    ui_state,
                    layer_id,
                    Adjustment::GradientMap(gradient.clone()),
                    &position_response,
                    command,
                );
            }
        }

        let mut midpoint = gradient.stop(index).unwrap_or(stop).midpoint;
        let midpoint_response = ui.add(
            egui::DragValue::new(&mut midpoint)
                .prefix(l10n.text("adjustment-midpoint-prefix"))
                .suffix("%")
                .range(5..=95)
                .speed(1.0),
        );
        if midpoint_response.changed() {
            let mut edited = gradient.stop(index).unwrap_or(stop);
            edited.midpoint = midpoint;
            if gradient.set_stop(index, edited) {
                queue_adjustment_change(
                    ui_state,
                    layer_id,
                    Adjustment::GradientMap(gradient.clone()),
                    &midpoint_response,
                    command,
                );
            }
        }

        let mut color = gradient
            .stop(index)
            .unwrap_or(stop)
            .color
            .map(|channel| channel as f32 / 255.0);
        let color_response = ui.color_edit_button_rgb(&mut color);
        if color_response.changed() {
            let mut edited = gradient.stop(index).unwrap_or(stop);
            edited.color = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            if gradient.set_stop(index, edited) {
                queue_adjustment_change(
                    ui_state,
                    layer_id,
                    Adjustment::GradientMap(gradient.clone()),
                    &color_response,
                    command,
                );
            }
        }
    }

    let mut settings_changed = false;
    ui.horizontal_wrapped(|ui| {
        settings_changed |= ui
            .checkbox(&mut gradient.reverse, l10n.text("adjustment-reverse"))
            .changed();
        settings_changed |= ui
            .checkbox(&mut gradient.dither, l10n.text("adjustment-dither"))
            .changed();
    });
    if settings_changed {
        queue_adjustment_discrete_change(
            ui_state,
            layer_id,
            Adjustment::GradientMap(gradient.clone()),
            command,
        );
    }

    ui.horizontal_wrapped(|ui| {
        if ui.button(l10n.text("adjustment-add-stop")).clicked()
            && let Some(index) = insert_gradient_stop_at_largest_gap(gradient)
        {
            ui_state.gradient_selected_stop = Some(index);
            queue_adjustment_discrete_change(
                ui_state,
                layer_id,
                Adjustment::GradientMap(gradient.clone()),
                command,
            );
        }
        let can_delete = gradient.stops().len() > 2 && ui_state.gradient_selected_stop.is_some();
        if ui
            .add_available(
                can_delete,
                egui::Button::new(l10n.text("adjustment-delete-stop")),
            )
            .clicked()
            && let Some(index) = ui_state.gradient_selected_stop
            && gradient.remove_stop(index)
        {
            ui_state.gradient_selected_stop = None;
            queue_adjustment_discrete_change(
                ui_state,
                layer_id,
                Adjustment::GradientMap(gradient.clone()),
                command,
            );
        }
        if ui.button(l10n.text("action-reset")).clicked() {
            ui_state.gradient_selected_stop = None;
            queue_adjustment_discrete_change(
                ui_state,
                layer_id,
                Adjustment::GradientMap(GradientMapAdjustment::default()),
                command,
            );
        }
        let mut args = fluent::FluentArgs::new();
        args.set("count", gradient.stops().len() as i64);
        ui.weak(l10n.format("adjustment-stops-count", Some(&args)));
    });
    ui.weak(l10n.text("adjustment-gradient-help"));
}

fn adjustment_kind_label(
    l10n: &Localization,
    kind: crate::core::adjustment::AdjustmentKind,
) -> String {
    l10n.text(match kind {
        crate::core::adjustment::AdjustmentKind::BrightnessContrast => {
            "adjustment-brightness-contrast"
        }
        crate::core::adjustment::AdjustmentKind::Levels => "adjustment-levels",
        crate::core::adjustment::AdjustmentKind::Curves => "adjustment-curves",
        crate::core::adjustment::AdjustmentKind::HueSaturation => "adjustment-hue-saturation",
        crate::core::adjustment::AdjustmentKind::Invert => "adjustment-invert",
        crate::core::adjustment::AdjustmentKind::GradientMap => "adjustment-gradient-map",
        crate::core::adjustment::AdjustmentKind::UvMirror => "adjustment-uv-mirror",
    })
}

fn uv_mirror_direction_label(
    l10n: &Localization,
    axis: UvMirrorAxis,
    direction: UvMirrorDirection,
) -> String {
    l10n.text(match (axis, direction) {
        (UvMirrorAxis::X, UvMirrorDirection::PositiveToNegative) => {
            "adjustment-direction-right-left"
        }
        (UvMirrorAxis::X, UvMirrorDirection::NegativeToPositive) => {
            "adjustment-direction-left-right"
        }
        (UvMirrorAxis::Y, UvMirrorDirection::PositiveToNegative) => {
            "adjustment-direction-bottom-top"
        }
        (UvMirrorAxis::Y, UvMirrorDirection::NegativeToPositive) => {
            "adjustment-direction-top-bottom"
        }
    })
}

fn insert_gradient_stop_at_largest_gap(gradient: &mut GradientMapAdjustment) -> Option<usize> {
    let stops = gradient.stops();
    let (left, right) = stops
        .windows(2)
        .max_by(|left, right| {
            let left_gap = left[1].location - left[0].location;
            let right_gap = right[1].location - right[0].location;
            left_gap.cmp(&right_gap)
        })
        .map(|pair| (pair[0], pair[1]))?;
    let location = left.location + (right.location - left.location) / 2;
    let color = gradient
        .evaluate(location as f32 / GRADIENT_LOCATION_MAX as f32)
        .map(|channel| (channel * 255.0).round() as u8);
    gradient.insert_stop(GradientStop {
        location,
        midpoint: 50,
        color,
    })
}

fn queue_controls_change(
    ui_state: &mut AdjustmentEditorState,
    layer_id: LayerId,
    adjustment: Adjustment,
    controls_output: AdjustmentControlsOutput,
    command: &mut Option<Command>,
) {
    if controls_output.changed {
        if controls_output.discrete_change {
            queue_adjustment_discrete_change(ui_state, layer_id, adjustment, command);
        } else {
            queue_adjustment_interaction_change(
                ui_state,
                layer_id,
                adjustment,
                controls_output,
                command,
            );
        }
    } else if controls_output.interaction_ended {
        ui_state.edit_session = None;
    }
}

fn queue_adjustment_interaction_change(
    ui_state: &mut AdjustmentEditorState,
    layer_id: LayerId,
    adjustment: Adjustment,
    controls_output: AdjustmentControlsOutput,
    command: &mut Option<Command>,
) {
    let edit_session = if controls_output.drag_started
        || !controls_output.dragged
        || ui_state
            .edit_session
            .is_none_or(|session| session.layer_id != layer_id)
    {
        next_adjustment_edit_session(ui_state, layer_id)
    } else {
        ui_state
            .edit_session
            .map(|session| session.edit_session)
            .unwrap_or_else(|| next_adjustment_edit_session(ui_state, layer_id))
    };
    *command = Some(Command::SetAdjustment {
        layer_id,
        adjustment,
        edit_session,
    });
    if controls_output.drag_stopped || !controls_output.dragged {
        ui_state.edit_session = None;
    }
}

fn queue_adjustment_change(
    ui_state: &mut AdjustmentEditorState,
    layer_id: LayerId,
    adjustment: Adjustment,
    response: &egui::Response,
    command: &mut Option<Command>,
) {
    let edit_session = if response.drag_started()
        || !response.dragged()
        || ui_state
            .edit_session
            .is_none_or(|session| session.layer_id != layer_id)
    {
        next_adjustment_edit_session(ui_state, layer_id)
    } else {
        ui_state
            .edit_session
            .map(|session| session.edit_session)
            .unwrap_or_else(|| next_adjustment_edit_session(ui_state, layer_id))
    };
    *command = Some(Command::SetAdjustment {
        layer_id,
        adjustment,
        edit_session,
    });
    if response.drag_stopped() || !response.dragged() {
        ui_state.edit_session = None;
    }
}

fn queue_adjustment_discrete_change(
    ui_state: &mut AdjustmentEditorState,
    layer_id: LayerId,
    adjustment: Adjustment,
    command: &mut Option<Command>,
) {
    let edit_session = next_adjustment_edit_session(ui_state, layer_id);
    ui_state.edit_session = None;
    *command = Some(Command::SetAdjustment {
        layer_id,
        adjustment,
        edit_session,
    });
}

fn next_adjustment_edit_session(ui_state: &mut AdjustmentEditorState, layer_id: LayerId) -> u64 {
    ui_state.next_edit_session = ui_state.next_edit_session.wrapping_add(1).max(1);
    let edit_session = ui_state.next_edit_session;
    ui_state.edit_session = Some(AdjustmentEditSession {
        layer_id,
        edit_session,
    });
    edit_session
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_ids() -> (LayerId, LayerId) {
        let tree = crate::core::surface::LayerTree::new_default_raster();
        (
            tree.root(),
            tree.default_raster_layer().expect("default raster"),
        )
    }

    #[test]
    fn opening_another_adjustment_target_ends_the_previous_edit_session() {
        let (first, second) = layer_ids();
        let mut state = AdjustmentEditorState::default();
        state.open_for(first, 7);
        state.edit_session = Some(AdjustmentEditSession {
            layer_id: first,
            edit_session: 42,
        });
        state.gradient_selected_stop = Some(1);

        state.open_for(second, 7);

        assert!(state.open);
        assert_eq!(state.target.map(|target| target.layer_id), Some(second));
        assert!(state.edit_session.is_none());
        assert!(state.gradient_selected_stop.is_none());
    }

    #[test]
    fn reopening_the_same_target_preserves_the_active_edit_session() {
        let (_, target) = layer_ids();
        let mut state = AdjustmentEditorState::default();
        state.open_for(target, 7);
        state.edit_session = Some(AdjustmentEditSession {
            layer_id: target,
            edit_session: 42,
        });

        state.open_for(target, 7);

        assert_eq!(
            state.edit_session.map(|session| session.edit_session),
            Some(42)
        );
    }

    #[test]
    fn missing_document_closes_the_editor_and_clears_its_target() {
        let (_, target) = layer_ids();
        let app_state = AppState::default();
        let mut state = AdjustmentEditorState::default();
        state.open_for(target, app_state.document_generation());

        assert!(state.valid_target(&app_state).is_none());
        assert!(!state.open);
        assert!(state.target.is_none());
    }

    #[test]
    fn close_clears_target_and_edit_session() {
        let (_, target) = layer_ids();
        let mut state = AdjustmentEditorState::default();
        state.open_for(target, 7);
        state.edit_session = Some(AdjustmentEditSession {
            layer_id: target,
            edit_session: 42,
        });

        state.close();

        assert!(!state.open);
        assert!(state.target.is_none());
        assert!(state.edit_session.is_none());
    }

    #[test]
    fn add_gradient_stop_uses_the_largest_gap_and_interpolated_color() {
        let mut gradient = GradientMapAdjustment::default();
        let index = insert_gradient_stop_at_largest_gap(&mut gradient).unwrap();
        assert_eq!(index, 1);
        let stop = gradient.stop(index).unwrap();
        assert_eq!(stop.location, 2048);
        assert_eq!(stop.color, [128; 3]);
    }
}
