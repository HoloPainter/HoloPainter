use eframe::egui;

pub fn labeled_slider(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    slider: egui::Slider<'_>,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.label(label);
        let slider_size = [ui.available_width(), ui.spacing().interact_size.y];
        ui.add_sized(slider_size, slider)
    })
    .inner
}

const GRID_CONTROL_WIDTH: f32 = 190.0;
const GRID_LABEL_WIDTH: f32 = 132.0;

pub fn grid_labeled_control<R>(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    add_control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_sized(
        [GRID_LABEL_WIDTH, ui.spacing().interact_size.y],
        egui::Label::new(label).truncate(),
    );
    let response = add_control(ui);
    ui.end_row();
    response
}

pub fn grid_labeled_slider(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    slider: egui::Slider<'_>,
) -> egui::Response {
    grid_labeled_control(ui, label, |ui| {
        let slider_size = [GRID_CONTROL_WIDTH, ui.spacing().interact_size.y];
        ui.add_sized(slider_size, slider)
    })
}
