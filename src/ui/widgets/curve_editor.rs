use eframe::egui;

use crate::core::{
    adjustment::{CurveChannel as AdjustmentCurve, CurvePoint as AdjustmentCurvePoint},
    curve::{Curve, CurvePoint},
};

#[derive(Clone, Copy)]
pub(crate) struct EditorPoint {
    x: f32,
    y: f32,
}

pub(crate) trait EditableCurve {
    fn editor_points(&self) -> Vec<EditorPoint>;
    fn editor_evaluate(&self, input: f32) -> f32;
    fn editor_set_point(&mut self, index: usize, point: EditorPoint) -> bool;
    fn editor_insert_point(&mut self, point: EditorPoint) -> Option<usize>;
    fn editor_remove_point(&mut self, index: usize) -> bool;
}

impl EditableCurve for Curve {
    fn editor_points(&self) -> Vec<EditorPoint> {
        self.points()
            .iter()
            .map(|p| EditorPoint { x: p.x, y: p.y })
            .collect()
    }
    fn editor_evaluate(&self, input: f32) -> f32 {
        self.evaluate(input)
    }
    fn editor_set_point(&mut self, index: usize, p: EditorPoint) -> bool {
        self.set_point(index, CurvePoint { x: p.x, y: p.y })
    }
    fn editor_insert_point(&mut self, p: EditorPoint) -> Option<usize> {
        self.insert_point(CurvePoint { x: p.x, y: p.y })
    }
    fn editor_remove_point(&mut self, index: usize) -> bool {
        self.remove_point(index)
    }
}

impl EditableCurve for AdjustmentCurve {
    fn editor_points(&self) -> Vec<EditorPoint> {
        self.points()
            .iter()
            .map(|p| EditorPoint {
                x: p.input as f32 / 255.0,
                y: p.output as f32 / 255.0,
            })
            .collect()
    }
    fn editor_evaluate(&self, input: f32) -> f32 {
        self.evaluate(input)
    }
    fn editor_set_point(&mut self, index: usize, p: EditorPoint) -> bool {
        self.set_point(
            index,
            AdjustmentCurvePoint {
                input: (p.x * 255.0).round() as u8,
                output: (p.y * 255.0).round() as u8,
            },
        )
    }
    fn editor_insert_point(&mut self, p: EditorPoint) -> Option<usize> {
        self.insert_point(AdjustmentCurvePoint {
            input: (p.x * 255.0).round() as u8,
            output: (p.y * 255.0).round() as u8,
        })
    }
    fn editor_remove_point(&mut self, index: usize) -> bool {
        self.remove_point(index)
    }
}

const CURVE_GRAPH_HEIGHT: f32 = 240.0;
const CURVE_GRAPH_MARGIN: f32 = 10.0;
const CURVE_POINT_RADIUS: f32 = 5.0;
const CURVE_POINT_HIT_RADIUS: f32 = 14.0;
const CURVE_LINE_HIT_RADIUS: f32 = 6.0;
const CURVE_SAMPLE_COUNT: usize = 256;

pub(crate) struct CurveEditorOutput {
    pub(crate) response: egui::Response,
    pub(crate) changed: bool,
    pub(crate) discrete_change: bool,
}

pub(crate) fn draw_curve_editor<C: EditableCurve>(
    ui: &mut egui::Ui,
    channel: &mut C,
    selected_point: &mut Option<usize>,
    curve_color: egui::Color32,
    minimum_width: Option<f32>,
    preview_input: Option<f32>,
) -> CurveEditorOutput {
    let available_width = ui.available_width();
    let width = minimum_width.map_or(available_width, |minimum| available_width.max(minimum));
    let desired_size = egui::vec2(width, CURVE_GRAPH_HEIGHT);
    let (outer_rect, mut response) =
        ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let graph_rect = outer_rect.shrink(CURVE_GRAPH_MARGIN);
    let mut changed = false;
    let mut discrete_change = false;

    if response.double_clicked() {
        if let Some(pointer) = response.interact_pointer_pos()
            && let Some(index) = hit_test_curve_point(channel, graph_rect, pointer)
        {
            *selected_point = Some(index);
            if channel.editor_remove_point(index) {
                *selected_point = None;
                changed = true;
                discrete_change = true;
            }
        }
    } else if response.drag_started() {
        if let Some(pointer) = response.interact_pointer_pos() {
            *selected_point = hit_test_curve_point(channel, graph_rect, pointer);
        }
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        if let Some(index) = hit_test_curve_point(channel, graph_rect, pointer) {
            *selected_point = Some(index);
        } else if hit_test_curve_line(channel, graph_rect, pointer) {
            let input = point_from_screen(graph_rect, pointer).x;
            let point = EditorPoint {
                x: input,
                y: channel.editor_evaluate(input),
            };
            if let Some(index) = channel.editor_insert_point(point) {
                *selected_point = Some(index);
                changed = true;
                discrete_change = true;
            }
        } else {
            *selected_point = None;
        }
    }

    if !discrete_change
        && response.dragged()
        && let Some(index) = *selected_point
        && let Some(pointer) = response.interact_pointer_pos()
        && channel.editor_set_point(index, point_from_screen(graph_rect, pointer))
    {
        changed = true;
    }

    if selected_point.is_some_and(|index| index >= channel.editor_points().len()) {
        *selected_point = None;
    }

    paint_curve_graph(
        ui,
        graph_rect,
        channel,
        *selected_point,
        curve_color,
        preview_input,
    );
    if changed {
        response.mark_changed();
        ui.ctx().request_repaint();
    }
    response = response.on_hover_cursor(egui::CursorIcon::Crosshair);

    CurveEditorOutput {
        response,
        changed,
        discrete_change,
    }
}

fn paint_curve_graph<C: EditableCurve>(
    ui: &egui::Ui,
    rect: egui::Rect,
    channel: &C,
    selected_point: Option<usize>,
    curve_color: egui::Color32,
    preview_input: Option<f32>,
) {
    let visuals = ui.visuals();
    let background = visuals.extreme_bg_color;
    let outline = visuals.widgets.noninteractive.bg_stroke;
    let grid_color = visuals
        .widgets
        .noninteractive
        .fg_stroke
        .color
        .gamma_multiply(0.25);
    ui.painter().rect_filled(rect, 2.0, background);
    ui.painter()
        .rect_stroke(rect, 2.0, outline, egui::StrokeKind::Inside);

    for division in 1..4 {
        let amount = division as f32 / 4.0;
        let x = egui::lerp(rect.left()..=rect.right(), amount);
        let y = egui::lerp(rect.bottom()..=rect.top(), amount);
        ui.painter().line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, grid_color),
        );
        ui.painter().line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, grid_color),
        );
    }

    let curve_points = (0..CURVE_SAMPLE_COUNT)
        .map(|index| {
            let input = index as f32 / (CURVE_SAMPLE_COUNT - 1) as f32;
            point_to_screen(
                rect,
                EditorPoint {
                    x: input,
                    y: channel.editor_evaluate(input),
                },
            )
        })
        .collect::<Vec<_>>();
    ui.painter().add(egui::Shape::line(
        curve_points,
        egui::Stroke::new(2.0, curve_color),
    ));

    for (index, point) in channel.editor_points().into_iter().enumerate() {
        let screen = point_to_screen(rect, point);
        let selected = selected_point == Some(index);
        let fill = if selected {
            visuals.selection.bg_fill
        } else {
            background
        };
        let stroke_color = if selected {
            visuals.selection.stroke.color
        } else {
            curve_color
        };
        ui.painter().circle_filled(
            screen,
            CURVE_POINT_RADIUS + (if selected { 1.0 } else { 0.0 }),
            fill,
        );
        ui.painter().circle_stroke(
            screen,
            CURVE_POINT_RADIUS + (if selected { 1.0 } else { 0.0 }),
            egui::Stroke::new(2.0, stroke_color),
        );
    }

    if let Some(input) = preview_input {
        let marker = point_to_screen(
            rect,
            EditorPoint {
                x: input,
                y: channel.editor_evaluate(input),
            },
        );
        ui.painter()
            .circle_filled(marker, 4.0, egui::Color32::YELLOW);
        ui.painter()
            .circle_stroke(marker, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
    }
}

fn hit_test_curve_point<C: EditableCurve>(
    channel: &C,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<usize> {
    channel
        .editor_points()
        .into_iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let distance = point_to_screen(rect, point).distance(pointer);
            (distance <= CURVE_POINT_HIT_RADIUS).then_some((index, distance))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn hit_test_curve_line<C: EditableCurve>(
    channel: &C,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> bool {
    if !rect.expand(CURVE_LINE_HIT_RADIUS).contains(pointer) {
        return false;
    }

    let mut previous = point_to_screen(
        rect,
        EditorPoint {
            x: 0.0,
            y: channel.editor_evaluate(0.0),
        },
    );
    for index in 1..CURVE_SAMPLE_COUNT {
        let input = index as f32 / (CURVE_SAMPLE_COUNT - 1) as f32;
        let current = point_to_screen(
            rect,
            EditorPoint {
                x: input,
                y: channel.editor_evaluate(input),
            },
        );
        if distance_to_segment(pointer, previous, current) <= CURVE_LINE_HIT_RADIUS {
            return true;
        }
        previous = current;
    }
    false
}

fn distance_to_segment(point: egui::Pos2, start: egui::Pos2, end: egui::Pos2) -> f32 {
    let segment_x = end.x - start.x;
    let segment_y = end.y - start.y;
    let length_squared = segment_x * segment_x + segment_y * segment_y;
    if length_squared <= f32::EPSILON {
        return point.distance(start);
    }

    let point_x = point.x - start.x;
    let point_y = point.y - start.y;
    let amount = ((point_x * segment_x + point_y * segment_y) / length_squared).clamp(0.0, 1.0);
    point.distance(egui::pos2(
        start.x + segment_x * amount,
        start.y + segment_y * amount,
    ))
}

fn point_to_screen(rect: egui::Rect, point: EditorPoint) -> egui::Pos2 {
    egui::pos2(
        egui::lerp(rect.left()..=rect.right(), point.x.clamp(0.0, 1.0)),
        egui::lerp(rect.bottom()..=rect.top(), point.y.clamp(0.0, 1.0)),
    )
}

fn point_from_screen(rect: egui::Rect, point: egui::Pos2) -> EditorPoint {
    EditorPoint {
        x: ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        y: ((rect.bottom() - point.y) / rect.height()).clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_screen_mapping_round_trips() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
        let point = EditorPoint { x: 0.25, y: 0.75 };
        let round_trip = point_from_screen(rect, point_to_screen(rect, point));
        assert!((round_trip.x - point.x).abs() < 1.0e-6);
        assert!((round_trip.y - point.y).abs() < 1.0e-6);
    }

    #[test]
    fn curve_hit_test_selects_the_nearest_control_point() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let mut curve = Curve::default();
        let middle = curve.insert_point(CurvePoint { x: 0.5, y: 0.5 }).unwrap();
        assert_eq!(
            hit_test_curve_point(&curve, rect, egui::pos2(52.0, 49.0)),
            Some(middle)
        );
        assert_eq!(
            hit_test_curve_point(&curve, rect, egui::pos2(63.0, 50.0)),
            Some(middle)
        );
    }

    #[test]
    fn curve_line_hit_test_accepts_clicks_near_the_visible_curve() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let curve = Curve::default();
        assert!(hit_test_curve_line(&curve, rect, egui::pos2(50.0, 53.0)));
        assert!(!hit_test_curve_line(&curve, rect, egui::pos2(50.0, 70.0)));
    }
}
