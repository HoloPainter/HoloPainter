use eframe::egui;

use crate::{
    application::{AppState, Command, EditorAction, EditorActionContext},
    core::{
        composite::SelectionCompositeMode,
        tool::{SelectionToolOptions, ToolId},
    },
    localization::Localization,
    ui::view_output::ViewOutput,
};

pub fn draw_selection_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let mut options = state.selection_options().clone();
    egui::ComboBox::from_id_salt("selection_tool_operation")
        .selected_text(selection_mode_label(l10n, options.operation))
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut options.operation,
                SelectionCompositeMode::Replace,
                selection_mode_label(l10n, SelectionCompositeMode::Replace),
            );
            ui.selectable_value(
                &mut options.operation,
                SelectionCompositeMode::Add,
                selection_mode_label(l10n, SelectionCompositeMode::Add),
            );
            ui.selectable_value(
                &mut options.operation,
                SelectionCompositeMode::Subtract,
                selection_mode_label(l10n, SelectionCompositeMode::Subtract),
            );
            ui.selectable_value(
                &mut options.operation,
                SelectionCompositeMode::Intersect,
                selection_mode_label(l10n, SelectionCompositeMode::Intersect),
            );
            ui.selectable_value(
                &mut options.operation,
                SelectionCompositeMode::Difference,
                selection_mode_label(l10n, SelectionCompositeMode::Difference),
            );
        });
    if &options != state.selection_options() {
        output.push(Command::UpdateSelectionToolOptions(SelectionToolOptions {
            operation: options.operation,
        }));
    }
    if ui
        .add_enabled(
            actions.is_enabled(EditorAction::Deselect),
            egui::Button::new(l10n.text("tool-selection-deselect")),
        )
        .clicked()
    {
        output.push_action(EditorAction::Deselect);
        output.request_repaint();
    }
    ui.label(l10n.text(match state.panel_tool_id() {
        ToolId::LassoSelection => "tool-selection-lasso-help",
        _ => "tool-selection-rectangle-help",
    }));
    output
}

pub(crate) fn selection_mode_label(l10n: &Localization, mode: SelectionCompositeMode) -> String {
    l10n.text(match mode {
        SelectionCompositeMode::Replace => "selection-mode-replace",
        SelectionCompositeMode::Add => "selection-mode-add",
        SelectionCompositeMode::Subtract => "selection-mode-subtract",
        SelectionCompositeMode::Intersect => "selection-mode-intersect",
        SelectionCompositeMode::Difference => "selection-mode-difference",
        SelectionCompositeMode::Clear => "selection-mode-clear",
        SelectionCompositeMode::Invert => "selection-mode-invert",
    })
}
