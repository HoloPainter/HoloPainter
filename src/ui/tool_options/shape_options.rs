use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::tool::{ShapeToolOptions, ToolId},
    localization::Localization,
    ui::{tool_options::widgets::labeled_slider, view_output::ViewOutput},
};

pub fn draw_shape_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let mut options = state.shape_options().clone();
    if labeled_slider(
        ui,
        l10n.text("field-opacity"),
        egui::Slider::new(&mut options.opacity, 0.0..=1.0),
    )
    .changed()
    {
        output.push(Command::UpdateShapeToolOptions(ShapeToolOptions {
            opacity: options.opacity,
        }));
    }
    ui.label(l10n.text(match state.panel_tool_id() {
        ToolId::LassoPaint => "tool-shape-lasso-paint-help",
        ToolId::LassoErase => "tool-shape-lasso-erase-help",
        _ => "tool-shape-rectangle-help",
    }));
    output
}
