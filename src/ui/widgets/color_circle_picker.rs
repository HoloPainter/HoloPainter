use std::f32::consts::{PI, TAU};

use eframe::egui::{self, Color32, Id, Pos2, Rect, Response, Sense, Stroke, Vec2};

use crate::{localization::Localization, ui::icons::UiIconRegistry};

mod detail_popup;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumericMode {
    Hsv,
    Rgb,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivePart {
    HueRing,
    SvSquare,
}

#[derive(Clone, Debug)]
struct ColorCircleState {
    hsv: Hsv,
    last_valid_hue: f32,
    active_part: Option<ActivePart>,
    numeric_mode: NumericMode,
    detail: detail_popup::ColorDetailState,
}

pub struct ColorCirclePickerOutput {
    pub response: Response,
    pub start_screen_eyedropper: bool,
}

pub fn color_circle_picker_rgb(
    ui: &mut egui::Ui,
    id: Id,
    color: &mut [f32; 3],
    icons: &UiIconRegistry,
    l10n: &Localization,
) -> ColorCirclePickerOutput {
    let current_rgb = color32_from_rgb_f32(*color);
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_persisted::<ColorCircleState>(id))
        .unwrap_or_else(|| {
            let hsv = rgb_to_hsv_preserve_hue(current_rgb, 0.0);
            ColorCircleState {
                hsv,
                last_valid_hue: hsv.h,
                active_part: None,
                numeric_mode: NumericMode::Hsv,
                detail: detail_popup::ColorDetailState::new(current_rgb),
            }
        });
    state = normalize_state(state);

    if rgb_tuple(hsv_to_color32(state.hsv)) != rgb_tuple(current_rgb) {
        set_state_from_rgb(&mut state, current_rgb);
    }

    let mut changed = false;
    let mut start_screen_eyedropper = false;
    let size = ui.available_width().min(260.0).max(72.0);
    let geometry = ColorCircleGeometry::new(size);
    let (circle_rect, mut response) =
        ui.allocate_exact_size(geometry.size, Sense::click_and_drag());
    let geometry = geometry.with_rect(circle_rect);

    if response.drag_started() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            state.active_part = hit_test(pointer, &geometry, ui.spacing().interact_size.y);
        }
    }

    if (response.drag_started() || response.dragged() || response.clicked())
        && state.active_part.is_some()
    {
        if let Some(pointer) = response.interact_pointer_pos() {
            match state.active_part {
                Some(ActivePart::HueRing) => {
                    if let Some(hue) = hue_from_pos(pointer, geometry.center) {
                        state.hsv.h = hue;
                        state.last_valid_hue = hue;
                        changed |= write_rgb_if_changed(color, hsv_to_color32(state.hsv));
                    }
                }
                Some(ActivePart::SvSquare) => {
                    state.hsv.s =
                        remap_clamp(pointer.x, geometry.sv_rect.left(), geometry.sv_rect.right());
                    state.hsv.v =
                        remap_clamp(pointer.y, geometry.sv_rect.bottom(), geometry.sv_rect.top());
                    changed |= write_rgb_if_changed(color, hsv_to_color32(state.hsv));
                }
                None => {}
            }
        }
    }

    if response.drag_stopped() {
        state.active_part = None;
    }

    paint_hue_ring(ui, &geometry);
    paint_sv_square(ui, &geometry, state.hsv.h);
    paint_markers(ui, &geometry, state.hsv);

    let editor_response = ui
        .horizontal(|ui| {
            let popup_id = id.with("detail_color_popup");
            let popup_was_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
            state
                .detail
                .prepare(color32_from_rgb_f32(*color), popup_was_open);

            let mut row_response = show_current_color(ui, l10n, *color, popup_was_open);
            if row_response.clicked() && !popup_was_open {
                state.detail.set_color(color32_from_rgb_f32(*color));
            }
            let detail_output = detail_popup::show_detail_color_popup(
                popup_id,
                &row_response,
                color,
                &mut state,
                icons,
                l10n,
            );
            changed |= detail_output.color_changed;
            start_screen_eyedropper |= detail_output.start_screen_eyedropper;

            let numeric_response = numeric_color_edit_ui(ui, l10n, color, &mut state, &mut changed);
            row_response = row_response.union(numeric_response);
            row_response
        })
        .inner;
    response = response.union(editor_response);

    if changed {
        response.mark_changed();
        ui.ctx().request_repaint();
    }

    ui.ctx().data_mut(|data| data.insert_persisted(id, state));
    ColorCirclePickerOutput {
        response,
        start_screen_eyedropper,
    }
}

fn show_current_color(
    ui: &mut egui::Ui,
    l10n: &Localization,
    color: [f32; 3],
    popup_open: bool,
) -> Response {
    let height = ui.spacing().interact_size.y;
    let width = height * 2.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let visuals = ui.style().interact_selectable(&response, popup_open);
    let visual_rect = rect.expand(visuals.expansion);

    ui.painter().rect_filled(
        visual_rect,
        visuals.corner_radius,
        Color32::from_rgb(
            current_channel(color[0]),
            current_channel(color[1]),
            current_channel(color[2]),
        ),
    );
    ui.painter().rect_stroke(
        visual_rect,
        visuals.corner_radius,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    response.on_hover_text(l10n.text("color-open-detail-editor"))
}

fn numeric_mode_ui(ui: &mut egui::Ui, l10n: &Localization, mode: &mut NumericMode) -> Response {
    let label = match mode {
        NumericMode::Hsv => "HSV",
        NumericMode::Rgb => "RGB",
    };
    let response = ui
        .small_button(label)
        .on_hover_text(l10n.text("color-switch-numeric-mode"));
    if response.clicked() {
        *mode = match mode {
            NumericMode::Hsv => NumericMode::Rgb,
            NumericMode::Rgb => NumericMode::Hsv,
        };
    }
    response
}

fn numeric_color_edit_ui(
    ui: &mut egui::Ui,
    l10n: &Localization,
    color: &mut [f32; 3],
    state: &mut ColorCircleState,
    changed: &mut bool,
) -> Response {
    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            numeric_mode_ui(ui, l10n, &mut state.numeric_mode);
            match state.numeric_mode {
                NumericMode::Hsv => {
                    let mut hue = state.hsv.h * 360.0;
                    let mut saturation = state.hsv.s * 100.0;
                    let mut value = state.hsv.v * 100.0;

                    let hue_changed = ui
                        .add(
                            egui::DragValue::new(&mut hue)
                                .speed(0.5)
                                .range(0.0..=360.0)
                                .prefix("H ")
                                .suffix("°")
                                .max_decimals(0),
                        )
                        .changed();
                    let saturation_changed = ui
                        .add(
                            egui::DragValue::new(&mut saturation)
                                .speed(0.25)
                                .range(0.0..=100.0)
                                .prefix("S ")
                                .suffix("%")
                                .max_decimals(0),
                        )
                        .changed();
                    let value_changed = ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .speed(0.25)
                                .range(0.0..=100.0)
                                .prefix("V ")
                                .suffix("%")
                                .max_decimals(0),
                        )
                        .changed();

                    if hue_changed || saturation_changed || value_changed {
                        state.hsv.h = wrap01(hue / 360.0);
                        state.hsv.s = (saturation / 100.0).clamp(0.0, 1.0);
                        state.hsv.v = (value / 100.0).clamp(0.0, 1.0);
                        if hue_changed {
                            state.last_valid_hue = state.hsv.h;
                        }
                        *changed |= write_rgb_if_changed(color, hsv_to_color32(state.hsv));
                    }
                }
                NumericMode::Rgb => {
                    let mut red = current_channel(color[0]);
                    let mut green = current_channel(color[1]);
                    let mut blue = current_channel(color[2]);

                    let red_changed = ui
                        .add(
                            egui::DragValue::new(&mut red)
                                .speed(0.5)
                                .range(0..=255)
                                .prefix("R "),
                        )
                        .changed();
                    let green_changed = ui
                        .add(
                            egui::DragValue::new(&mut green)
                                .speed(0.5)
                                .range(0..=255)
                                .prefix("G "),
                        )
                        .changed();
                    let blue_changed = ui
                        .add(
                            egui::DragValue::new(&mut blue)
                                .speed(0.5)
                                .range(0..=255)
                                .prefix("B "),
                        )
                        .changed();

                    if red_changed || green_changed || blue_changed {
                        let new_color = Color32::from_rgb(red, green, blue);
                        set_state_from_rgb(state, new_color);
                        *changed |= write_rgb_if_changed(color, new_color);
                    }
                }
            }
        })
        .response
    })
    .inner
}

#[derive(Clone, Copy, Debug)]
struct ColorCircleGeometry {
    size: Vec2,
    center: Pos2,
    outer_radius: f32,
    inner_radius: f32,
    ring_width: f32,
    sv_rect: Rect,
}

impl ColorCircleGeometry {
    fn new(size: f32) -> Self {
        let ring_width = (size * 0.075).clamp(6.0, size * 0.22);
        let gap = (size * 0.025).max(2.0);
        let outer_radius = size * 0.5;
        let inner_radius = (outer_radius - ring_width).max(outer_radius * 0.5);
        let square_radius = (inner_radius - gap).max(18.0);
        let sv_side = (std::f32::consts::SQRT_2 * square_radius)
            .min(size - 2.0 * (ring_width + gap))
            .max(24.0);
        Self {
            size: Vec2::splat(size),
            center: Pos2::ZERO,
            outer_radius,
            inner_radius,
            ring_width,
            sv_rect: Rect::from_center_size(Pos2::ZERO, Vec2::splat(sv_side)),
        }
    }

    fn with_rect(mut self, rect: Rect) -> Self {
        self.center = rect.center();
        self.sv_rect = Rect::from_center_size(self.center, self.sv_rect.size());
        self
    }
}

fn paint_hue_ring(ui: &egui::Ui, geometry: &ColorCircleGeometry) {
    const SEGMENTS: usize = 216;
    let mut mesh = egui::Mesh::default();

    for index in 0..SEGMENTS {
        let h0 = index as f32 / SEGMENTS as f32;
        let h1 = (index + 1) as f32 / SEGMENTS as f32;
        let theta0 = hue_to_theta(h0);
        let theta1 = hue_to_theta(h1);
        let outer0 = point_on_circle(geometry.center, geometry.outer_radius, theta0);
        let inner0 = point_on_circle(geometry.center, geometry.inner_radius, theta0);
        let outer1 = point_on_circle(geometry.center, geometry.outer_radius, theta1);
        let inner1 = point_on_circle(geometry.center, geometry.inner_radius, theta1);
        let color0 = hsv_to_color32(Hsv {
            h: h0,
            s: 1.0,
            v: 1.0,
        });
        let color1 = hsv_to_color32(Hsv {
            h: h1,
            s: 1.0,
            v: 1.0,
        });

        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(outer0, color0);
        mesh.colored_vertex(inner0, color0);
        mesh.colored_vertex(outer1, color1);
        mesh.colored_vertex(inner1, color1);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 2, base + 1, base + 3);
    }

    ui.painter().add(egui::Shape::mesh(mesh));
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .circle_stroke(geometry.center, geometry.outer_radius, stroke);
    ui.painter()
        .circle_stroke(geometry.center, geometry.inner_radius, stroke);
}

fn paint_sv_square(ui: &egui::Ui, geometry: &ColorCircleGeometry, hue: f32) {
    const DIVISIONS: usize = 36;
    let mut mesh = egui::Mesh::default();

    for y in 0..=DIVISIONS {
        for x in 0..=DIVISIONS {
            let saturation = x as f32 / DIVISIONS as f32;
            let value = 1.0 - y as f32 / DIVISIONS as f32;
            let position = Pos2::new(
                egui::lerp(
                    geometry.sv_rect.left()..=geometry.sv_rect.right(),
                    saturation,
                ),
                egui::lerp(
                    geometry.sv_rect.top()..=geometry.sv_rect.bottom(),
                    y as f32 / DIVISIONS as f32,
                ),
            );
            mesh.colored_vertex(
                position,
                hsv_to_color32(Hsv {
                    h: hue,
                    s: saturation,
                    v: value,
                }),
            );
        }
    }

    for y in 0..DIVISIONS {
        for x in 0..DIVISIONS {
            let top_left = (y * (DIVISIONS + 1) + x) as u32;
            let top_right = top_left + 1;
            let bottom_left = top_left + (DIVISIONS + 1) as u32;
            let bottom_right = bottom_left + 1;
            mesh.add_triangle(top_left, bottom_left, top_right);
            mesh.add_triangle(top_right, bottom_left, bottom_right);
        }
    }

    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        geometry.sv_rect,
        egui::CornerRadius::same(2),
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

fn paint_markers(ui: &egui::Ui, geometry: &ColorCircleGeometry, hsv: Hsv) {
    let hue_position = point_on_circle(
        geometry.center,
        (geometry.outer_radius + geometry.inner_radius) * 0.5,
        hue_to_theta(hsv.h),
    );
    let hue_color = hsv_to_color32(Hsv {
        h: hsv.h,
        s: 1.0,
        v: 1.0,
    });
    let hue_radius = geometry.ring_width * 0.55;
    ui.painter()
        .circle_filled(hue_position, hue_radius, hue_color);
    ui.painter().circle_stroke(
        hue_position,
        hue_radius,
        Stroke::new(1.5, contrast_color(hue_color)),
    );

    let sv_position = Pos2::new(
        egui::lerp(geometry.sv_rect.left()..=geometry.sv_rect.right(), hsv.s),
        egui::lerp(geometry.sv_rect.bottom()..=geometry.sv_rect.top(), hsv.v),
    );
    let sv_color = hsv_to_color32(hsv);
    let sv_radius = (geometry.sv_rect.width() / 24.0).max(4.0);
    ui.painter().circle_filled(sv_position, sv_radius, sv_color);
    ui.painter().circle_stroke(
        sv_position,
        sv_radius,
        Stroke::new(1.5, contrast_color(sv_color)),
    );
}

fn hit_test(
    pointer: Pos2,
    geometry: &ColorCircleGeometry,
    interaction_height: f32,
) -> Option<ActivePart> {
    if geometry.sv_rect.contains(pointer) {
        return Some(ActivePart::SvSquare);
    }

    let distance = pointer.distance(geometry.center);
    let hit_slop = (interaction_height * 0.15).max(3.0);
    if (geometry.inner_radius - hit_slop..=geometry.outer_radius + hit_slop).contains(&distance) {
        Some(ActivePart::HueRing)
    } else {
        None
    }
}

fn hue_from_pos(pointer: Pos2, center: Pos2) -> Option<f32> {
    let delta = pointer - center;
    if delta.length_sq() <= 1.0e-4 {
        return None;
    }
    let theta = delta.y.atan2(delta.x);
    Some(wrap01((theta + PI / 2.0) / TAU))
}

fn hue_to_theta(hue: f32) -> f32 {
    -PI / 2.0 + wrap01(hue) * TAU
}

fn point_on_circle(center: Pos2, radius: f32, theta: f32) -> Pos2 {
    Pos2::new(
        center.x + theta.cos() * radius,
        center.y + theta.sin() * radius,
    )
}

fn remap_clamp(value: f32, from_start: f32, from_end: f32) -> f32 {
    let denominator = from_end - from_start;
    if denominator.abs() <= f32::EPSILON {
        return 0.0;
    }
    ((value - from_start) / denominator).clamp(0.0, 1.0)
}

fn set_state_from_rgb(state: &mut ColorCircleState, color: Color32) {
    state.hsv = rgb_to_hsv_preserve_hue(color, state.last_valid_hue);
    if hue_is_defined(state.hsv) {
        state.last_valid_hue = state.hsv.h;
    } else {
        state.hsv.h = state.last_valid_hue;
    }
}

fn normalize_state(mut state: ColorCircleState) -> ColorCircleState {
    if !state.last_valid_hue.is_finite() {
        state.last_valid_hue = 0.0;
    }
    state.last_valid_hue = wrap01(state.last_valid_hue);

    if !state.hsv.h.is_finite() {
        state.hsv.h = state.last_valid_hue;
    }
    if !state.hsv.s.is_finite() {
        state.hsv.s = 0.0;
    }
    if !state.hsv.v.is_finite() {
        state.hsv.v = 0.0;
    }

    state.hsv.h = wrap01(state.hsv.h);
    state.hsv.s = state.hsv.s.clamp(0.0, 1.0);
    state.hsv.v = state.hsv.v.clamp(0.0, 1.0);
    state
}

fn hue_is_defined(hsv: Hsv) -> bool {
    hsv.s > 0.0 && hsv.v > 0.0
}

fn rgb_to_hsv_preserve_hue(color: Color32, fallback_hue: f32) -> Hsv {
    let red = color.r() as f32 / 255.0;
    let green = color.g() as f32 / 255.0;
    let blue = color.b() as f32 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let value = max;
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };

    let hue = if delta <= f32::EPSILON {
        wrap01(fallback_hue)
    } else if (max - red).abs() <= f32::EPSILON {
        wrap01(((green - blue) / delta) / 6.0)
    } else if (max - green).abs() <= f32::EPSILON {
        wrap01(((blue - red) / delta + 2.0) / 6.0)
    } else {
        wrap01(((red - green) / delta + 4.0) / 6.0)
    };

    Hsv {
        h: hue,
        s: saturation,
        v: value,
    }
}

fn hsv_to_color32(hsv: Hsv) -> Color32 {
    let hue = wrap01(hsv.h);
    let saturation = hsv.s.clamp(0.0, 1.0);
    let value = hsv.v.clamp(0.0, 1.0);

    if saturation <= f32::EPSILON {
        let channel = float_channel_to_u8(value);
        return Color32::from_rgb(channel, channel, channel);
    }

    let sector = hue * 6.0;
    let index = sector.floor() as i32;
    let fraction = sector - index as f32;
    let low = value * (1.0 - saturation);
    let falling = value * (1.0 - saturation * fraction);
    let rising = value * (1.0 - saturation * (1.0 - fraction));
    let (red, green, blue) = match index.rem_euclid(6) {
        0 => (value, rising, low),
        1 => (falling, value, low),
        2 => (low, value, rising),
        3 => (low, falling, value),
        4 => (rising, low, value),
        _ => (value, low, falling),
    };

    Color32::from_rgb(
        float_channel_to_u8(red),
        float_channel_to_u8(green),
        float_channel_to_u8(blue),
    )
}

fn color32_from_rgb_f32(color: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        current_channel(color[0]),
        current_channel(color[1]),
        current_channel(color[2]),
    )
}

fn write_rgb_if_changed(color: &mut [f32; 3], new_rgb: Color32) -> bool {
    let old_rgb = color32_from_rgb_f32(*color);
    if rgb_tuple(old_rgb) == rgb_tuple(new_rgb) {
        return false;
    }

    color[0] = new_rgb.r() as f32 / 255.0;
    color[1] = new_rgb.g() as f32 / 255.0;
    color[2] = new_rgb.b() as f32 / 255.0;
    true
}

fn rgb_tuple(color: Color32) -> (u8, u8, u8) {
    (color.r(), color.g(), color.b())
}

fn current_channel(value: f32) -> u8 {
    float_channel_to_u8(sanitize_unit(value))
}

fn sanitize_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn float_channel_to_u8(value: f32) -> u8 {
    (sanitize_unit(value) * 255.0).round() as u8
}

fn wrap01(value: f32) -> f32 {
    if value.is_finite() {
        value.rem_euclid(1.0)
    } else {
        0.0
    }
}

fn contrast_color(color: Color32) -> Color32 {
    let intensity =
        (0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32) / 255.0;
    if intensity < 0.5 {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_colors_convert_to_expected_hues() {
        let red = rgb_to_hsv_preserve_hue(Color32::RED, 0.5);
        let green = rgb_to_hsv_preserve_hue(Color32::GREEN, 0.5);
        let blue = rgb_to_hsv_preserve_hue(Color32::BLUE, 0.5);

        assert!((red.h - 0.0).abs() <= f32::EPSILON);
        assert!((green.h - 1.0 / 3.0).abs() <= 1.0e-6);
        assert!((blue.h - 2.0 / 3.0).abs() <= 1.0e-6);
        assert_eq!(red.s, 1.0);
        assert_eq!(red.v, 1.0);
    }

    #[test]
    fn grayscale_conversion_preserves_previous_hue() {
        let hsv = rgb_to_hsv_preserve_hue(Color32::from_gray(128), 0.73);
        assert!((hsv.h - 0.73).abs() <= f32::EPSILON);
        assert_eq!(hsv.s, 0.0);
    }

    #[test]
    fn hsv_conversion_wraps_hue_at_one_turn() {
        let start = hsv_to_color32(Hsv {
            h: 0.0,
            s: 1.0,
            v: 1.0,
        });
        let wrapped = hsv_to_color32(Hsv {
            h: 1.0,
            s: 1.0,
            v: 1.0,
        });
        assert_eq!(start, wrapped);
    }

    #[test]
    fn writing_rgb_updates_all_channels() {
        let mut rgb = [0.1, 0.2, 0.3];
        assert!(write_rgb_if_changed(
            &mut rgb,
            Color32::from_rgb(255, 128, 0)
        ));
        assert_eq!(color32_from_rgb_f32(rgb), Color32::from_rgb(255, 128, 0));
    }

    #[test]
    fn non_finite_channels_are_sanitized() {
        assert_eq!(current_channel(f32::NAN), 0);
        assert_eq!(current_channel(f32::INFINITY), 0);
        assert_eq!(current_channel(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn square_hit_test_takes_priority_over_ring() {
        let geometry = ColorCircleGeometry::new(120.0)
            .with_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(120.0)));
        assert_eq!(
            hit_test(geometry.center, &geometry, 20.0),
            Some(ActivePart::SvSquare)
        );
    }
}
