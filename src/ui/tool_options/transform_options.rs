use eframe::egui;

use crate::{
    application::{AppState, Command, TransformNumericEdit},
    localization::Localization,
    ui::{
        input::shortcut_profile::{ShortcutAction, ShortcutProfile},
        view_output::ViewOutput,
    },
};

pub fn draw_transform_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    shortcut_profile: &ShortcutProfile,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    ui.label(l10n.text("tool-transform-operate-uv"));
    ui.separator();
    if let Some(mut values) = state.transform_numeric_values() {
        ui.add_enabled_ui(!state.is_tool_pointer_gesture_active(), |ui| {
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::DragValue::new(&mut values.center_px.x)
                        .prefix("X: ")
                        .suffix(" px")
                        .speed(1.0),
                );
                record_numeric_edit(
                    &mut output,
                    response,
                    TransformNumericEdit::CenterX(values.center_px.x),
                );
                let response = ui.add(
                    egui::DragValue::new(&mut values.center_px.y)
                        .prefix("Y: ")
                        .suffix(" px")
                        .speed(1.0),
                );
                record_numeric_edit(
                    &mut output,
                    response,
                    TransformNumericEdit::CenterY(values.center_px.y),
                );
            });
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::DragValue::new(&mut values.size_px.x)
                        .prefix("W: ")
                        .suffix(" px")
                        .speed(1.0),
                );
                record_numeric_edit(
                    &mut output,
                    response,
                    TransformNumericEdit::Width(values.size_px.x),
                );
                let response = ui.add(
                    egui::DragValue::new(&mut values.size_px.y)
                        .prefix("H: ")
                        .suffix(" px")
                        .speed(1.0),
                );
                record_numeric_edit(
                    &mut output,
                    response,
                    TransformNumericEdit::Height(values.size_px.y),
                );
            });
            let response = ui.add(
                egui::DragValue::new(&mut values.rotation_degrees)
                    .prefix(format!("{} ", l10n.text("tool-transform-angle-prefix")))
                    .suffix("°")
                    .speed(0.5),
            );
            record_numeric_edit(
                &mut output,
                response,
                TransformNumericEdit::RotationDegrees(values.rotation_degrees),
            );
        });
    } else {
        ui.add_enabled(
            false,
            egui::Label::new(l10n.text("tool-transform-no-target")),
        );
    }
    ui.separator();
    ui.label(l10n.text("tool-transform-drag-inside"));
    ui.label(l10n.text("tool-transform-drag-handles"));
    ui.label(l10n.text("tool-transform-negative-size"));
    if state.active_transform_preserves_aspect() {
        ui.label(l10n.text("tool-transform-aspect-locked"));
    } else {
        ui.label(l10n.text("tool-transform-aspect-shift"));
    }
    ui.label(l10n.text("tool-transform-scale-center"));
    if state.has_active_raster_transform_session() {
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(l10n.text("action-apply")).clicked() {
                output.push(Command::ApplyActiveTransform);
            }
            if ui.button(l10n.text("action-cancel")).clicked() {
                output.push(Command::CancelActiveTransform);
            }
        });
        let apply = shortcut_profile
            .first_chord_for_action(ShortcutAction::ApplyActiveOperation)
            .map(|chord| chord.display())
            .unwrap_or_else(|| l10n.text("shortcut-unassigned"));
        let cancel = shortcut_profile
            .first_chord_for_action(ShortcutAction::CancelActiveOperation)
            .map(|chord| chord.display())
            .unwrap_or_else(|| l10n.text("shortcut-unassigned"));
        let mut args = fluent::FluentArgs::new();
        args.set("shortcut", apply);
        ui.label(l10n.format("tool-transform-apply-shortcut", Some(&args)));
        let mut args = fluent::FluentArgs::new();
        args.set("shortcut", cancel);
        ui.label(l10n.format("tool-transform-cancel-shortcut", Some(&args)));
    }
    output
}

fn record_numeric_edit(
    output: &mut ViewOutput,
    response: egui::Response,
    edit: TransformNumericEdit,
) {
    if response.changed() {
        output.push(Command::BeginTransformNumericEdit);
        output.push(Command::UpdateTransformNumeric(edit));
    }
    if response.drag_stopped() || response.lost_focus() {
        output.push(Command::CommitTransformNumericEdit);
    }
}
