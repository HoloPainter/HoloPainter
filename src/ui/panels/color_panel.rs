use eframe::egui;

use crate::{
    application::{AppState, Command},
    localization::Localization,
    ui::{
        icons::UiIconRegistry,
        view_output::{ScreenEyedropperTarget, UiRequest, ViewOutput},
        widgets::color_circle_picker::color_circle_picker_rgb,
    },
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ColorPanelContentOutput {
    pub changed: bool,
    pub start_screen_eyedropper: bool,
}

pub(super) fn draw_color_panel_content(
    ui: &mut egui::Ui,
    id: egui::Id,
    color: &mut [f32; 3],
    icons: &UiIconRegistry,
    l10n: &Localization,
) -> ColorPanelContentOutput {
    let picker = color_circle_picker_rgb(ui, id.with("picker"), color, icons, l10n);
    ColorPanelContentOutput {
        changed: picker.response.changed(),
        start_screen_eyedropper: picker.start_screen_eyedropper,
    }
}

pub fn draw_color_panel(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    icons: &UiIconRegistry,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    egui::ScrollArea::vertical()
        .id_salt("color_panel_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut current_color = state.current_color();
            let content_id = ui.make_persistent_id("main_color_panel");
            let content = draw_color_panel_content(ui, content_id, &mut current_color, icons, l10n);
            if content.changed {
                output.push(Command::SetCurrentColor(current_color));
                output.request_repaint();
            }
            if content.start_screen_eyedropper {
                output.request_ui(UiRequest::StartScreenEyedropper(
                    ScreenEyedropperTarget::CurrentColor,
                ));
                output.request_repaint();
            }
        });
    output
}
