use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::tool::{ColorPickerToolOptions, ColorSampleSource},
    localization::Localization,
    ui::view_output::ViewOutput,
};

pub fn draw_color_picker_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let mut options = state.color_picker_options();

    ui.horizontal(|ui| {
        ui.label(l10n.text("tool-color-picker-source"));
        egui::ComboBox::from_id_salt("color_picker_source")
            .selected_text(source_label(l10n, options.source))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut options.source,
                    ColorSampleSource::View,
                    source_label(l10n, ColorSampleSource::View),
                );
                ui.selectable_value(
                    &mut options.source,
                    ColorSampleSource::CompositeTexture,
                    source_label(l10n, ColorSampleSource::CompositeTexture),
                );
                ui.selectable_value(
                    &mut options.source,
                    ColorSampleSource::CurrentLayer,
                    source_label(l10n, ColorSampleSource::CurrentLayer),
                );
            });
    });

    if options != state.color_picker_options() {
        output.push(Command::UpdateColorPickerToolOptions(
            ColorPickerToolOptions {
                source: options.source,
            },
        ));
    }

    ui.label(source_description(l10n, options.source));
    ui.label(l10n.text("tool-color-picker-help"));
    output
}

fn source_label(l10n: &Localization, source: ColorSampleSource) -> String {
    l10n.text(match source {
        ColorSampleSource::View => "color-source-view",
        ColorSampleSource::CompositeTexture => "color-source-composite-texture",
        ColorSampleSource::CurrentLayer => "color-source-current-layer",
    })
}

fn source_description(l10n: &Localization, source: ColorSampleSource) -> String {
    l10n.text(match source {
        ColorSampleSource::View => "color-source-view-description",
        ColorSampleSource::CompositeTexture => "color-source-composite-description",
        ColorSampleSource::CurrentLayer => "color-source-current-layer-description",
    })
}
