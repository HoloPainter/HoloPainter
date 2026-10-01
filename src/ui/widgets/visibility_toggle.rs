use eframe::egui;

use crate::ui::icons::UiIconRegistry;

const ICON_VISIBILITY: &str = "builtin.icon.visibility";
const ICON_VISIBILITY_OFF: &str = "builtin.icon.visibility_off";
const BUTTON_SIZE: f32 = 24.0;
const ICON_SIZE: f32 = 18.0;

pub fn draw_visibility_toggle(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    visible: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::Vec2::splat(BUTTON_SIZE), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let rect = rect.expand(visuals.expansion);
        let show_frame = response.hovered()
            || response.highlighted()
            || response.has_focus()
            || response.is_pointer_button_down_on();

        if show_frame {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            ui.painter().rect_stroke(
                rect,
                visuals.corner_radius,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }

        let icon_id = visibility_icon_id(visible);
        if let Some(texture) = icons.texture(icon_id) {
            let tint = if ui.is_enabled() {
                if visible {
                    visuals.fg_stroke.color
                } else {
                    ui.visuals().weak_text_color()
                }
            } else {
                ui.visuals().widgets.noninteractive.fg_stroke.color
            };
            let icon_rect =
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(ICON_SIZE));
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                tint,
            );
        }
    }

    response
}

fn visibility_icon_id(visible: bool) -> &'static str {
    if visible {
        ICON_VISIBILITY
    } else {
        ICON_VISIBILITY_OFF
    }
}
