use eframe::egui;

use crate::core::document::MaterialUiColor;

pub(super) fn paint_material_swatch(ui: &egui::Ui, rect: egui::Rect, color: MaterialUiColor) {
    ui.painter().rect_filled(
        rect,
        2.0,
        egui::Color32::from_rgb(color.rgb[0], color.rgb[1], color.rgb[2]),
    );
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
        egui::StrokeKind::Inside,
    );
}
