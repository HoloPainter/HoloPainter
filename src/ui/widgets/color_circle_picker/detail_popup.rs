use eframe::egui::{self, Color32, Id, Pos2, Rect, Response, Sense, Stroke, Vec2};

use crate::{
    localization::Localization,
    ui::{icons::UiIconRegistry, input::screen_eyedropper},
};

use super::{
    ColorCircleState, Hsv, color32_from_rgb_f32, contrast_color, hsv_to_color32,
    set_state_from_rgb, wrap01, write_rgb_if_changed,
};

const POPUP_WIDTH: f32 = 340.0;
const PREVIEW_SIZE: f32 = 48.0;
const SLIDER_WIDTH: f32 = 190.0;
const SLIDER_HEIGHT: f32 = 18.0;
const NUMBER_WIDTH: f32 = 58.0;
const GRADIENT_STEPS: usize = 48;
const EYEDROPPER_BUTTON_SIZE: f32 = 24.0;
const EYEDROPPER_ICON_SIZE: f32 = 22.0;
const EYEDROPPER_ICON_ID: &str = "builtin.icon.colorize";

#[derive(Clone, Debug)]
pub(super) struct ColorDetailState {
    hex_text: String,
    hex_error: bool,
    hex_editing: bool,
}

impl ColorDetailState {
    pub(super) fn new(color: Color32) -> Self {
        Self {
            hex_text: format_hex_color(color),
            hex_error: false,
            hex_editing: false,
        }
    }

    pub(super) fn prepare(&mut self, color: Color32, popup_open: bool) {
        if !popup_open {
            self.set_color(color);
            self.hex_editing = false;
        } else if !self.hex_editing && !self.hex_error {
            self.hex_text = format_hex_color(color);
        }
    }

    pub(super) fn set_color(&mut self, color: Color32) {
        self.hex_text = format_hex_color(color);
        self.hex_error = false;
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct DetailPopupOutput {
    pub(super) color_changed: bool,
    pub(super) start_screen_eyedropper: bool,
}

pub(super) fn show_detail_color_popup(
    popup_id: Id,
    anchor: &Response,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
    icons: &UiIconRegistry,
    l10n: &Localization,
) -> DetailPopupOutput {
    egui::Popup::menu(anchor)
        .id(popup_id)
        .width(POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(POPUP_WIDTH - 20.0);
            detail_color_edit_ui(ui, popup_id, color, state, icons, l10n)
        })
        .map(|response| response.inner)
        .unwrap_or_default()
}

fn detail_color_edit_ui(
    ui: &mut egui::Ui,
    id: Id,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
    icons: &UiIconRegistry,
    l10n: &Localization,
) -> DetailPopupOutput {
    let mut output = DetailPopupOutput::default();

    ui.horizontal(|ui| {
        paint_color_preview(ui, color32_from_rgb_f32(*color));
        ui.vertical(|ui| {
            output.color_changed |= hex_color_edit_ui(ui, color, state);
            ui.add_space(2.0);
            if screen_eyedropper_button(ui, icons, l10n).clicked() {
                egui::Popup::close_id(ui.ctx(), id);
                output.start_screen_eyedropper = true;
            }
            show_hex_error(ui, l10n, state);
        });
    });

    ui.add_space(6.0);
    output.color_changed |= rgb_slider_section(ui, id.with("rgb"), color, state);

    ui.add_space(6.0);
    output.color_changed |= hsv_slider_section(ui, id.with("hsv"), color, state);

    output
}

fn paint_color_preview(ui: &mut egui::Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(PREVIEW_SIZE), Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(3), color);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(3),
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    response
}

fn hex_color_edit_ui(
    ui: &mut egui::Ui,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
) -> bool {
    let error_color = ui.visuals().error_fg_color;
    let response = ui
        .scope(|ui| {
            if state.detail.hex_error {
                ui.visuals_mut().override_text_color = Some(error_color);
            }
            ui.add_sized(
                Vec2::new(190.0, ui.spacing().interact_size.y),
                egui::TextEdit::singleline(&mut state.detail.hex_text)
                    .desired_width(190.0)
                    .char_limit(7),
            )
        })
        .inner;

    let mut changed = false;
    if response.changed() {
        state.detail.hex_error = false;
        if let Ok(parsed) = parse_hex_color(&state.detail.hex_text) {
            set_state_from_rgb(state, parsed);
            changed |= write_rgb_if_changed(color, parsed);
        }
    }

    let enter_pressed =
        response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    if response.lost_focus() || enter_pressed {
        match parse_hex_color(&state.detail.hex_text) {
            Ok(parsed) => {
                set_state_from_rgb(state, parsed);
                changed |= write_rgb_if_changed(color, parsed);
                state.detail.set_color(parsed);
            }
            Err(_) => {
                state.detail.hex_error = true;
            }
        }
    }
    state.detail.hex_editing = response.has_focus();

    changed
}

fn show_hex_error(ui: &mut egui::Ui, l10n: &Localization, state: &ColorCircleState) {
    if state.detail.hex_error {
        ui.label(
            egui::RichText::new(l10n.text("color-hex-error"))
                .small()
                .color(ui.visuals().error_fg_color),
        );
    }
}

fn screen_eyedropper_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    l10n: &Localization,
) -> Response {
    let available = screen_eyedropper::available();
    let response = ui
        .add_enabled_ui(available, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::splat(EYEDROPPER_BUTTON_SIZE), Sense::click());
            if ui.is_rect_visible(rect) {
                let visuals = ui.style().interact_selectable(&response, false);
                let visual_rect = rect.expand(visuals.expansion);
                ui.painter()
                    .rect_filled(visual_rect, visuals.corner_radius, visuals.weak_bg_fill);
                ui.painter().rect_stroke(
                    visual_rect,
                    visuals.corner_radius,
                    visuals.bg_stroke,
                    egui::StrokeKind::Inside,
                );
                if let Some(texture) = icons.texture(EYEDROPPER_ICON_ID) {
                    let tint = if ui.is_enabled() {
                        visuals.fg_stroke.color
                    } else {
                        ui.visuals().widgets.noninteractive.fg_stroke.color
                    };
                    let icon_rect =
                        Rect::from_center_size(rect.center(), Vec2::splat(EYEDROPPER_ICON_SIZE));
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                }
            }
            response
        })
        .inner;

    if available {
        response.on_hover_text(l10n.text("color-pick-from-screen"))
    } else {
        response.on_disabled_hover_text(l10n.text("color-screen-eyedropper-windows-only"))
    }
}

fn rgb_slider_section(
    ui: &mut egui::Ui,
    id: Id,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
) -> bool {
    let current = color32_from_rgb_f32(*color);
    let mut red = current.r();
    let mut green = current.g();
    let mut blue = current.b();
    let mut edited = false;

    egui::Grid::new(id)
        .num_columns(3)
        .spacing(Vec2::new(8.0, 4.0))
        .show(ui, |ui| {
            ui.label("R");
            let mut slider_value = red as f32 / 255.0;
            let response = color_gradient_slider(ui, id.with("r_slider"), &mut slider_value, |t| {
                Color32::from_rgb((t * 255.0).round() as u8, green, blue)
            });
            if response.changed() {
                red = (slider_value * 255.0).round() as u8;
                edited = true;
            }
            edited |= ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut red).speed(0.5).range(0..=255),
                )
                .changed();
            ui.end_row();

            ui.label("G");
            let mut slider_value = green as f32 / 255.0;
            let response = color_gradient_slider(ui, id.with("g_slider"), &mut slider_value, |t| {
                Color32::from_rgb(red, (t * 255.0).round() as u8, blue)
            });
            if response.changed() {
                green = (slider_value * 255.0).round() as u8;
                edited = true;
            }
            edited |= ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut green).speed(0.5).range(0..=255),
                )
                .changed();
            ui.end_row();

            ui.label("B");
            let mut slider_value = blue as f32 / 255.0;
            let response = color_gradient_slider(ui, id.with("b_slider"), &mut slider_value, |t| {
                Color32::from_rgb(red, green, (t * 255.0).round() as u8)
            });
            if response.changed() {
                blue = (slider_value * 255.0).round() as u8;
                edited = true;
            }
            edited |= ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut blue).speed(0.5).range(0..=255),
                )
                .changed();
            ui.end_row();
        });

    if !edited {
        return false;
    }

    let new_color = Color32::from_rgb(red, green, blue);
    set_state_from_rgb(state, new_color);
    state.detail.set_color(new_color);
    write_rgb_if_changed(color, new_color)
}

fn hsv_slider_section(
    ui: &mut egui::Ui,
    id: Id,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
) -> bool {
    let mut hue = wrap01(state.hsv.h);
    let mut saturation = state.hsv.s.clamp(0.0, 1.0);
    let mut value = state.hsv.v.clamp(0.0, 1.0);
    let mut hue_edited = false;
    let mut edited = false;

    egui::Grid::new(id)
        .num_columns(3)
        .spacing(Vec2::new(8.0, 4.0))
        .show(ui, |ui| {
            ui.label("H");
            let response = color_gradient_slider(ui, id.with("h_slider"), &mut hue, |t| {
                hsv_to_color32(Hsv {
                    h: t,
                    s: 1.0,
                    v: 1.0,
                })
            });
            if response.changed() {
                hue = hue.min(359.0 / 360.0);
                hue_edited = true;
                edited = true;
            }
            let mut degrees = (hue * 360.0).round().clamp(0.0, 359.0) as u16;
            if ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut degrees)
                        .speed(0.5)
                        .range(0..=359)
                        .suffix("°"),
                )
                .changed()
            {
                hue = degrees as f32 / 360.0;
                hue_edited = true;
                edited = true;
            }
            ui.end_row();

            ui.label("S");
            let response = color_gradient_slider(ui, id.with("s_slider"), &mut saturation, |t| {
                hsv_to_color32(Hsv {
                    h: hue,
                    s: t,
                    v: 1.0,
                })
            });
            edited |= response.changed();
            let mut percent = (saturation * 100.0).round() as u16;
            if ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut percent)
                        .speed(0.25)
                        .range(0..=100)
                        .suffix("%"),
                )
                .changed()
            {
                saturation = percent as f32 / 100.0;
                edited = true;
            }
            ui.end_row();

            ui.label("V");
            let response = color_gradient_slider(ui, id.with("v_slider"), &mut value, |t| {
                hsv_to_color32(Hsv {
                    h: hue,
                    s: saturation,
                    v: t,
                })
            });
            edited |= response.changed();
            let mut percent = (value * 100.0).round() as u16;
            if ui
                .add_sized(
                    Vec2::new(NUMBER_WIDTH, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut percent)
                        .speed(0.25)
                        .range(0..=100)
                        .suffix("%"),
                )
                .changed()
            {
                value = percent as f32 / 100.0;
                edited = true;
            }
            ui.end_row();
        });

    if !edited {
        return false;
    }

    state.hsv = Hsv {
        h: wrap01(hue),
        s: saturation.clamp(0.0, 1.0),
        v: value.clamp(0.0, 1.0),
    };
    if hue_edited {
        state.last_valid_hue = state.hsv.h;
    }
    let new_color = hsv_to_color32(state.hsv);
    state.detail.set_color(new_color);
    write_rgb_if_changed(color, new_color)
}

fn color_gradient_slider(
    ui: &mut egui::Ui,
    id: Id,
    value: &mut f32,
    gradient: impl Fn(f32) -> Color32,
) -> Response {
    ui.push_id(id, |ui| {
        let (rect, mut response) = ui.allocate_exact_size(
            Vec2::new(SLIDER_WIDTH, SLIDER_HEIGHT),
            Sense::click_and_drag(),
        );

        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let next = remap_slider_position(pointer.x, rect.left(), rect.right());
            if (*value - next).abs() > f32::EPSILON {
                *value = next;
                response.mark_changed();
            }
        }

        let paint_rect = rect.shrink(1.0);
        let mut mesh = egui::Mesh::default();
        for index in 0..=GRADIENT_STEPS {
            let t = index as f32 / GRADIENT_STEPS as f32;
            let x = egui::lerp(paint_rect.left()..=paint_rect.right(), t);
            let color = gradient(t);
            mesh.colored_vertex(Pos2::new(x, paint_rect.top()), color);
            mesh.colored_vertex(Pos2::new(x, paint_rect.bottom()), color);
        }
        for index in 0..GRADIENT_STEPS {
            let base = (index * 2) as u32;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 2, base + 1, base + 3);
        }
        ui.painter().add(egui::Shape::mesh(mesh));

        let visuals = ui.style().interact_selectable(&response, false);
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(3),
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );

        let marker_x = egui::lerp(rect.left()..=rect.right(), value.clamp(0.0, 1.0));
        let marker_color = gradient(value.clamp(0.0, 1.0));
        ui.painter().line_segment(
            [
                Pos2::new(marker_x, rect.top() + 1.0),
                Pos2::new(marker_x, rect.bottom() - 1.0),
            ],
            Stroke::new(3.0, contrast_color(marker_color)),
        );
        ui.painter().line_segment(
            [
                Pos2::new(marker_x, rect.top() + 1.0),
                Pos2::new(marker_x, rect.bottom() - 1.0),
            ],
            Stroke::new(1.0, marker_color),
        );

        response
    })
    .inner
}

fn remap_slider_position(value: f32, start: f32, end: f32) -> f32 {
    let width = end - start;
    if width.abs() <= f32::EPSILON {
        return 0.0;
    }
    ((value - start) / width).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HexColorError {
    InvalidLength,
    InvalidDigit,
}

fn parse_hex_color(text: &str) -> Result<Color32, HexColorError> {
    let digits = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if digits.len() != 6 {
        return Err(HexColorError::InvalidLength);
    }

    let value = u32::from_str_radix(digits, 16).map_err(|_| HexColorError::InvalidDigit)?;
    Ok(Color32::from_rgb(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ))
}

fn format_hex_color(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rgb_as_uppercase_hex() {
        assert_eq!(format_hex_color(Color32::from_rgb(74, 144, 226)), "#4A90E2");
    }

    #[test]
    fn parses_hex_with_hash() {
        assert_eq!(
            parse_hex_color("#4A90E2"),
            Ok(Color32::from_rgb(74, 144, 226))
        );
    }

    #[test]
    fn parses_hex_without_hash() {
        assert_eq!(
            parse_hex_color("4A90E2"),
            Ok(Color32::from_rgb(74, 144, 226))
        );
    }

    #[test]
    fn parses_lowercase_hex() {
        assert_eq!(
            parse_hex_color("4a90e2"),
            Ok(Color32::from_rgb(74, 144, 226))
        );
    }

    #[test]
    fn rejects_short_hex() {
        assert_eq!(parse_hex_color("#FFF"), Err(HexColorError::InvalidLength));
    }

    #[test]
    fn rejects_long_hex() {
        assert_eq!(
            parse_hex_color("#FFFFFFFF"),
            Err(HexColorError::InvalidLength)
        );
    }

    #[test]
    fn rejects_non_hex_characters() {
        assert_eq!(parse_hex_color("#GG0000"), Err(HexColorError::InvalidDigit));
    }

    #[test]
    fn hex_round_trip_preserves_rgb() {
        let color = Color32::from_rgb(12, 34, 56);
        assert_eq!(parse_hex_color(&format_hex_color(color)), Ok(color));
    }
}
