use eframe::egui;

use crate::{
    application::{AppState, EditorAction, EditorActionContext},
    localization::Localization,
    ui::{
        panels::runtime_metrics::{self, RuntimeMetricsUiData},
        view_output::ViewOutput,
    },
};

#[derive(Debug, Default)]
pub struct TopBarOutput {
    pub view: ViewOutput,
    pub new_project_requested: bool,
    pub metrics_toggled: bool,
}

pub fn draw_top_bar(
    root_ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    metrics: &RuntimeMetricsUiData,
    metrics_window_open: bool,
    actions: EditorActionContext,
) -> TopBarOutput {
    let mut output = TopBarOutput::default();
    egui::Panel::top("top_bar").show(root_ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button(l10n.text("menu-file-new-project")).clicked() {
                output.new_project_requested = true;
            }
            if ui
                .add_enabled(
                    actions.is_enabled(EditorAction::Undo),
                    egui::Button::new(l10n.text("menu-edit-undo")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::Undo);
            }
            if ui
                .add_enabled(
                    actions.is_enabled(EditorAction::Redo),
                    egui::Button::new(l10n.text("menu-edit-redo")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::Redo);
            }
            ui.label(state.status().format(l10n));
            runtime_metrics::draw_top_bar_metrics(ui, l10n, metrics);
            if ui
                .selectable_label(metrics_window_open, l10n.text("top-bar-metrics"))
                .on_hover_text(l10n.text("top-bar-metrics-help"))
                .clicked()
            {
                output.metrics_toggled = true;
            }
        });
    });
    output
}
