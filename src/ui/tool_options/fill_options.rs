use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::tool::{FillScope, FillToolOptions},
    localization::Localization,
    ui::{tool_options::widgets::labeled_slider, view_output::ViewOutput},
};

pub fn draw_fill_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    scope: FillScope,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let mut options = state.fill_options().clone();
    if labeled_slider(
        ui,
        l10n.text("field-opacity"),
        egui::Slider::new(&mut options.opacity, 0.0..=1.0),
    )
    .changed()
    {
        output.push(Command::UpdateFillToolOptions(FillToolOptions {
            opacity: options.opacity,
        }));
    }
    ui.label(l10n.text(match scope {
        FillScope::Material => "tool-fill-material-help",
        FillScope::Mesh => "tool-fill-mesh-help",
        FillScope::Polygon => "tool-fill-polygon-help",
    }));
    output
}
