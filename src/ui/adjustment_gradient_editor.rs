use eframe::egui;

use crate::core::adjustment::{GRADIENT_LOCATION_MAX, GradientMapAdjustment, GradientStop};

const GRADIENT_EDITOR_HEIGHT: f32 = 74.0;
const GRADIENT_BAR_HEIGHT: f32 = 34.0;
const GRADIENT_MARGIN: f32 = 10.0;
const GRADIENT_MARKER_HEIGHT: f32 = 18.0;
const GRADIENT_MARKER_HALF_WIDTH: f32 = 7.0;
const GRADIENT_MARKER_HIT_HALF_WIDTH: f32 = 12.0;
const GRADIENT_MARKER_HIT_VERTICAL_PADDING: f32 = 4.0;
const GRADIENT_SAMPLE_COUNT: usize = 64;

pub(crate) struct GradientEditorOutput {
    pub(crate) response: egui::Response,
    pub(crate) changed: bool,
    pub(crate) discrete_change: bool,
}

pub(crate) fn draw_gradient_editor(
    ui: &mut egui::Ui,
    gradient: &mut GradientMapAdjustment,
    selected_stop: &mut Option<usize>,
) -> GradientEditorOutput {
    let desired_size = egui::vec2(ui.available_width().max(220.0), GRADIENT_EDITOR_HEIGHT);
    let (outer_rect, mut response) =
        ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let bar_rect = egui::Rect::from_min_size(
        egui::pos2(
            outer_rect.left() + GRADIENT_MARGIN,
            outer_rect.top() + GRADIENT_MARGIN,
        ),
        egui::vec2(
            (outer_rect.width() - GRADIENT_MARGIN * 2.0).max(1.0),
            GRADIENT_BAR_HEIGHT,
        ),
    );
    let mut changed = false;
    let mut discrete_change = false;

    if response.drag_started() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            *selected_stop = hit_test_gradient_stop(gradient, bar_rect, pointer);
        }
    }

    if response.double_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
        && bar_rect.expand(4.0).contains(pointer)
        && hit_test_gradient_stop(gradient, bar_rect, pointer).is_none()
    {
        let position = position_from_screen(bar_rect, pointer.x);
        let color = gradient
            .evaluate(position)
            .map(|channel| (channel * 255.0).round() as u8);
        if let Some(index) = gradient.insert_stop(GradientStop {
            location: (position * GRADIENT_LOCATION_MAX as f32).round() as u16,
            midpoint: 50,
            color,
        }) {
            *selected_stop = Some(index);
            changed = true;
            discrete_change = true;
        }
    } else if response.dragged()
        && let Some(index) = *selected_stop
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(mut stop) = gradient.stop(index)
    {
        stop.location = (position_from_screen(bar_rect, pointer.x) * GRADIENT_LOCATION_MAX as f32)
            .round() as u16;
        changed |= gradient.set_stop(index, stop);
    }

    if selected_stop.is_some_and(|index| index >= gradient.stops().len()) {
        *selected_stop = None;
    }

    paint_gradient(ui, bar_rect, gradient, *selected_stop);
    if changed {
        response.mark_changed();
        ui.ctx().request_repaint();
    }
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    GradientEditorOutput {
        response,
        changed,
        discrete_change,
    }
}

fn paint_gradient(
    ui: &egui::Ui,
    bar_rect: egui::Rect,
    gradient: &GradientMapAdjustment,
    selected_stop: Option<usize>,
) {
    let mut mesh = egui::Mesh::default();
    for index in 0..GRADIENT_SAMPLE_COUNT - 1 {
        let left_amount = index as f32 / (GRADIENT_SAMPLE_COUNT - 1) as f32;
        let right_amount = (index + 1) as f32 / (GRADIENT_SAMPLE_COUNT - 1) as f32;
        let left_x = egui::lerp(bar_rect.left()..=bar_rect.right(), left_amount);
        let right_x = egui::lerp(bar_rect.left()..=bar_rect.right(), right_amount);
        let left_color = color32_float(gradient.evaluate(left_amount));
        let right_color = color32_float(gradient.evaluate(right_amount));
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(left_x, bar_rect.top()), left_color);
        mesh.colored_vertex(egui::pos2(left_x, bar_rect.bottom()), left_color);
        mesh.colored_vertex(egui::pos2(right_x, bar_rect.top()), right_color);
        mesh.colored_vertex(egui::pos2(right_x, bar_rect.bottom()), right_color);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 2, base + 1, base + 3);
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        bar_rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    for (index, stop) in gradient.stops().iter().copied().enumerate() {
        let center_x = screen_x_from_position(
            bar_rect,
            stop.location as f32 / GRADIENT_LOCATION_MAX as f32,
        );
        let tip = egui::pos2(center_x, bar_rect.bottom());
        let bottom = bar_rect.bottom() + GRADIENT_MARKER_HEIGHT;
        let marker = vec![
            tip,
            egui::pos2(center_x - GRADIENT_MARKER_HALF_WIDTH, bottom),
            egui::pos2(center_x + GRADIENT_MARKER_HALF_WIDTH, bottom),
        ];
        let selected = selected_stop == Some(index);
        let outline = if selected {
            ui.visuals().selection.stroke
        } else {
            ui.visuals().widgets.noninteractive.fg_stroke
        };
        ui.painter().add(egui::Shape::convex_polygon(
            marker,
            color32(stop.color),
            outline,
        ));
        if selected {
            ui.painter().circle_filled(
                egui::pos2(center_x, bottom + 3.0),
                2.0,
                ui.visuals().selection.bg_fill,
            );
        }
    }
}

fn hit_test_gradient_stop(
    gradient: &GradientMapAdjustment,
    bar_rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<usize> {
    gradient
        .stops()
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, stop)| {
            let center_x = screen_x_from_position(
                bar_rect,
                stop.location as f32 / GRADIENT_LOCATION_MAX as f32,
            );
            gradient_stop_hit_rect(bar_rect, center_x)
                .contains(pointer)
                .then_some((index, (center_x - pointer.x).abs()))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn gradient_stop_hit_rect(bar_rect: egui::Rect, center_x: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            center_x - GRADIENT_MARKER_HIT_HALF_WIDTH,
            bar_rect.top() - GRADIENT_MARKER_HIT_VERTICAL_PADDING,
        ),
        egui::pos2(
            center_x + GRADIENT_MARKER_HIT_HALF_WIDTH,
            bar_rect.bottom() + GRADIENT_MARKER_HEIGHT + GRADIENT_MARKER_HIT_VERTICAL_PADDING,
        ),
    )
}

fn screen_x_from_position(rect: egui::Rect, position: f32) -> f32 {
    egui::lerp(rect.left()..=rect.right(), position.clamp(0.0, 1.0))
}

fn position_from_screen(rect: egui::Rect, x: f32) -> f32 {
    ((x - rect.left()) / rect.width()).clamp(0.0, 1.0)
}

fn color32(color: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(color[0], color[1], color[2])
}

fn color32_float(color: [f32; 3]) -> egui::Color32 {
    color32(color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_position_mapping_round_trips() {
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(200.0, 30.0));
        let position = 0.37;
        let round_trip = position_from_screen(rect, screen_x_from_position(rect, position));
        assert!((round_trip - position).abs() < 1.0e-6);
    }

    #[test]
    fn gradient_hit_test_selects_nearest_marker_across_bar_and_marker() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 30.0));
        let gradient = GradientMapAdjustment::default();
        assert_eq!(
            hit_test_gradient_stop(&gradient, rect, egui::pos2(99.0, 10.0)),
            Some(1)
        );
        assert_eq!(
            hit_test_gradient_stop(&gradient, rect, egui::pos2(89.0, 45.0)),
            Some(1)
        );
        assert_eq!(
            hit_test_gradient_stop(&gradient, rect, egui::pos2(87.0, 45.0)),
            None
        );
    }
}
