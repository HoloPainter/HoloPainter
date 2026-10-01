use eframe::egui;
use glam::Vec3;

use crate::{core::camera::OrbitCamera, localization::Localization};

const GIZMO_SIZE: f32 = 72.0;
const GIZMO_MARGIN: f32 = 8.0;
const BACKGROUND_RADIUS: f32 = 32.0;
const AXIS_LENGTH: f32 = 24.0;
const POSITIVE_ENDPOINT_RADIUS: f32 = 9.0;
const NEGATIVE_ENDPOINT_RADIUS: f32 = 6.0;
const ENDPOINT_HIT_RADIUS: f32 = 10.0;
const COLLAPSED_RING_RADIUS: f32 = 14.0;
const COLLAPSED_RING_HIT_RADIUS: f32 = 16.0;
const COLLAPSED_THRESHOLD: f32 = 3.0;
const DRAG_HALF_TURN_DISTANCE: f32 = BACKGROUND_RADIUS * 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisView {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}

impl AxisView {
    pub fn camera_basis(self) -> (Vec3, Vec3) {
        match self {
            Self::PositiveX => (Vec3::NEG_X, Vec3::Y),
            Self::NegativeX => (Vec3::X, Vec3::Y),
            Self::PositiveY => (Vec3::NEG_Y, Vec3::NEG_Z),
            Self::NegativeY => (Vec3::Y, Vec3::Z),
            Self::PositiveZ => (Vec3::NEG_Z, Vec3::Y),
            Self::NegativeZ => (Vec3::Z, Vec3::Y),
        }
    }

    fn world_axis(self) -> Vec3 {
        match self {
            Self::PositiveX => Vec3::X,
            Self::NegativeX => Vec3::NEG_X,
            Self::PositiveY => Vec3::Y,
            Self::NegativeY => Vec3::NEG_Y,
            Self::PositiveZ => Vec3::Z,
            Self::NegativeZ => Vec3::NEG_Z,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::PositiveX => "X",
            Self::NegativeX => "-",
            Self::PositiveY => "Y",
            Self::NegativeY => "-",
            Self::PositiveZ => "Z",
            Self::NegativeZ => "-",
        }
    }

    fn tooltip_key(self) -> &'static str {
        match self {
            Self::PositiveX => "orientation-view-positive-x",
            Self::NegativeX => "orientation-view-negative-x",
            Self::PositiveY => "orientation-view-positive-y",
            Self::NegativeY => "orientation-view-negative-y",
            Self::PositiveZ => "orientation-view-positive-z",
            Self::NegativeZ => "orientation-view-negative-z",
        }
    }

    fn is_positive(self) -> bool {
        matches!(self, Self::PositiveX | Self::PositiveY | Self::PositiveZ)
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::PositiveX | Self::NegativeX => egui::Color32::from_rgb(224, 90, 90),
            Self::PositiveY | Self::NegativeY => egui::Color32::from_rgb(101, 185, 94),
            Self::PositiveZ | Self::NegativeZ => egui::Color32::from_rgb(84, 133, 216),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OrientationGizmoInteraction {
    pub rect: egui::Rect,
    pub selected_view: Option<AxisView>,
    pub orbit_radians: Option<egui::Vec2>,
    pub pointer_over: bool,
}

#[derive(Debug, Clone, Copy)]
struct AxisEndpoint {
    view: AxisView,
    position: egui::Pos2,
    depth: f32,
    collapsed: bool,
}

pub fn draw_orientation_gizmo(
    ctx: &egui::Context,
    id: egui::Id,
    viewport_rect: egui::Rect,
    camera: &OrbitCamera,
    l10n: &Localization,
    enabled: bool,
    occlusion_rects: &[egui::Rect],
) -> OrientationGizmoInteraction {
    let gizmo_rect = orientation_gizmo_rect(viewport_rect);
    let mut selected_view = None;
    let mut orbit_radians = None;
    let mut pointer_over = false;

    egui::Area::new(id)
        .order(egui::Order::Middle)
        .fixed_pos(gizmo_rect.min)
        .movable(false)
        .show(ctx, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(gizmo_rect.size(), egui::Sense::click_and_drag());
            let center = rect.center();
            let mut endpoints = axis_endpoints(center, camera);
            let pointer = ctx.pointer_hover_pos();
            let pointer_available =
                pointer.is_some_and(|position| point_is_available(position, rect, occlusion_rects));
            let drag_origin_available = ctx.input(|input| {
                input
                    .pointer
                    .press_origin()
                    .is_some_and(|position| point_is_available(position, rect, occlusion_rects))
            });
            let dragging = enabled
                && drag_origin_available
                && response.dragged_by(egui::PointerButton::Primary);
            pointer_over = pointer_available || dragging;

            if dragging {
                let delta = ctx.input(|input| input.pointer.delta());
                if delta != egui::Vec2::ZERO {
                    orbit_radians = Some(gizmo_drag_orbit_radians(delta));
                }
            }

            let hovered_view = if enabled && pointer_available && !dragging {
                pointer.and_then(|position| hit_test_axis(position, &mut endpoints))
            } else {
                None
            };
            let opacity = if enabled { 1.0 } else { 0.4 };
            draw_gizmo(ui.painter(), center, &mut endpoints, hovered_view, opacity);

            if dragging {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if let Some(view) = hovered_view {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                response
                    .clone()
                    .on_hover_text(l10n.text(view.tooltip_key()));
                if response.clicked_by(egui::PointerButton::Primary) {
                    selected_view = Some(view);
                }
            } else if enabled && pointer_available {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
        });

    OrientationGizmoInteraction {
        rect: gizmo_rect,
        selected_view,
        orbit_radians,
        pointer_over,
    }
}

fn gizmo_drag_orbit_radians(delta: egui::Vec2) -> egui::Vec2 {
    delta * (std::f32::consts::PI / DRAG_HALF_TURN_DISTANCE)
}

fn orientation_gizmo_rect(viewport_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            viewport_rect.right() - GIZMO_MARGIN - GIZMO_SIZE,
            viewport_rect.bottom() - GIZMO_MARGIN - GIZMO_SIZE,
        ),
        egui::Vec2::splat(GIZMO_SIZE),
    )
}

fn point_is_available(
    position: egui::Pos2,
    gizmo_rect: egui::Rect,
    occlusion_rects: &[egui::Rect],
) -> bool {
    gizmo_rect.contains(position)
        && !occlusion_rects
            .iter()
            .any(|occlusion| occlusion.contains(position))
}

fn axis_endpoints(center: egui::Pos2, camera: &OrbitCamera) -> [AxisEndpoint; 6] {
    const VIEWS: [AxisView; 6] = [
        AxisView::PositiveX,
        AxisView::NegativeX,
        AxisView::PositiveY,
        AxisView::NegativeY,
        AxisView::PositiveZ,
        AxisView::NegativeZ,
    ];

    VIEWS.map(|view| {
        let axis = view.world_axis();
        let offset = egui::vec2(axis.dot(camera.right()), -axis.dot(camera.up())) * AXIS_LENGTH;
        AxisEndpoint {
            view,
            position: center + offset,
            depth: axis.dot(camera.forward()),
            collapsed: offset.length() < COLLAPSED_THRESHOLD,
        }
    })
}

fn draw_gizmo(
    painter: &egui::Painter,
    center: egui::Pos2,
    endpoints: &mut [AxisEndpoint; 6],
    hovered_view: Option<AxisView>,
    opacity: f32,
) {
    painter.circle_filled(
        center,
        BACKGROUND_RADIUS,
        with_opacity(
            egui::Color32::from_rgba_unmultiplied(20, 22, 26, 148),
            opacity,
        ),
    );

    endpoints.sort_by(|left, right| right.depth.total_cmp(&left.depth));
    for endpoint in endpoints.iter() {
        painter.line_segment(
            [center, endpoint.position],
            egui::Stroke::new(1.5, with_opacity(endpoint.view.color(), opacity * 0.8)),
        );
    }

    painter.circle_filled(
        center,
        3.0,
        with_opacity(egui::Color32::from_rgb(220, 225, 230), opacity),
    );

    for endpoint in endpoints.iter() {
        let hovered = hovered_view == Some(endpoint.view);
        draw_endpoint(painter, *endpoint, hovered, opacity);
    }
}

fn draw_endpoint(painter: &egui::Painter, endpoint: AxisEndpoint, hovered: bool, opacity: f32) {
    let color = with_opacity(endpoint.view.color(), opacity);
    let stroke_width = if hovered { 2.0 } else { 1.0 };

    if endpoint.collapsed && endpoint.depth > 0.0 {
        let radius = COLLAPSED_RING_RADIUS + if hovered { 1.0 } else { 0.0 };
        painter.circle_stroke(
            endpoint.position,
            radius,
            egui::Stroke::new(stroke_width, color),
        );
        return;
    }

    let radius = (if endpoint.view.is_positive() {
        POSITIVE_ENDPOINT_RADIUS
    } else {
        NEGATIVE_ENDPOINT_RADIUS
    }) + if hovered { 1.0 } else { 0.0 };

    if endpoint.view.is_positive() {
        painter.circle_filled(endpoint.position, radius, color);
        painter.circle_stroke(
            endpoint.position,
            radius,
            egui::Stroke::new(
                stroke_width,
                with_opacity(egui::Color32::from_rgb(30, 32, 36), opacity),
            ),
        );
        painter.text(
            endpoint.position,
            egui::Align2::CENTER_CENTER,
            endpoint.view.symbol(),
            egui::FontId::proportional(10.0),
            with_opacity(egui::Color32::WHITE, opacity),
        );
    } else {
        painter.circle_filled(
            endpoint.position,
            radius,
            with_opacity(
                egui::Color32::from_rgba_unmultiplied(20, 22, 26, 210),
                opacity,
            ),
        );
        painter.circle_stroke(
            endpoint.position,
            radius,
            egui::Stroke::new(stroke_width, color),
        );
        painter.text(
            endpoint.position,
            egui::Align2::CENTER_CENTER,
            endpoint.view.symbol(),
            egui::FontId::proportional(9.0),
            color,
        );
    }
}

fn hit_test_axis(pointer: egui::Pos2, endpoints: &mut [AxisEndpoint; 6]) -> Option<AxisView> {
    endpoints.sort_by(|left, right| left.depth.total_cmp(&right.depth));
    endpoints.iter().find_map(|endpoint| {
        let distance = pointer.distance(endpoint.position);
        let hit = if endpoint.collapsed && endpoint.depth > 0.0 {
            distance > ENDPOINT_HIT_RADIUS && distance <= COLLAPSED_RING_HIT_RADIUS
        } else {
            distance <= ENDPOINT_HIT_RADIUS
        };
        hit.then_some(endpoint.view)
    })
}

fn with_opacity(color: egui::Color32, opacity: f32) -> egui::Color32 {
    let alpha = ((color.a() as f32) * opacity.clamp(0.0, 1.0)).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_fixed_to_viewport_bottom_right() {
        let viewport = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 170.0));
        let rect = orientation_gizmo_rect(viewport);

        assert_eq!(rect.size(), egui::Vec2::splat(GIZMO_SIZE));
        assert_eq!(rect.right(), viewport.right() - GIZMO_MARGIN);
        assert_eq!(rect.bottom(), viewport.bottom() - GIZMO_MARGIN);
    }

    #[test]
    fn positive_axis_views_use_expected_camera_bases() {
        assert_eq!(AxisView::PositiveX.camera_basis(), (Vec3::NEG_X, Vec3::Y));
        assert_eq!(
            AxisView::PositiveY.camera_basis(),
            (Vec3::NEG_Y, Vec3::NEG_Z)
        );
        assert_eq!(AxisView::PositiveZ.camera_basis(), (Vec3::NEG_Z, Vec3::Y));
    }

    #[test]
    fn collapsed_axis_hit_test_selects_near_endpoint_then_outer_ring() {
        let center = egui::pos2(40.0, 40.0);
        let camera = OrbitCamera {
            orientation: glam::Quat::IDENTITY,
            ..OrbitCamera::default()
        };
        let mut endpoints = axis_endpoints(center, &camera);

        assert_eq!(
            hit_test_axis(center, &mut endpoints),
            Some(AxisView::PositiveZ)
        );
        assert_eq!(
            hit_test_axis(center + egui::vec2(13.0, 0.0), &mut endpoints),
            Some(AxisView::NegativeZ)
        );
    }

    #[test]
    fn available_points_exclude_occluded_gizmo_regions() {
        let gizmo = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(72.0, 72.0));
        let occlusion = egui::Rect::from_min_max(egui::pos2(50.0, 60.0), egui::pos2(90.0, 100.0));

        assert!(point_is_available(
            egui::pos2(20.0, 30.0),
            gizmo,
            &[occlusion]
        ));
        assert!(!point_is_available(
            egui::pos2(60.0, 70.0),
            gizmo,
            &[occlusion]
        ));
        assert!(!point_is_available(
            egui::pos2(5.0, 15.0),
            gizmo,
            &[occlusion]
        ));
    }

    #[test]
    fn drag_across_gizmo_size_rotates_half_a_turn() {
        let horizontal = gizmo_drag_orbit_radians(egui::vec2(DRAG_HALF_TURN_DISTANCE, 0.0));
        let vertical = gizmo_drag_orbit_radians(egui::vec2(0.0, DRAG_HALF_TURN_DISTANCE));

        assert!((horizontal.x - std::f32::consts::PI).abs() < 1.0e-6);
        assert_eq!(horizontal.y, 0.0);
        assert_eq!(vertical.x, 0.0);
        assert!((vertical.y - std::f32::consts::PI).abs() < 1.0e-6);
    }

    #[test]
    fn projected_axes_follow_camera_screen_basis() {
        let center = egui::pos2(40.0, 40.0);
        let camera = OrbitCamera {
            orientation: glam::Quat::IDENTITY,
            ..OrbitCamera::default()
        };
        let endpoints = axis_endpoints(center, &camera);
        let positive_x = endpoints
            .iter()
            .find(|endpoint| endpoint.view == AxisView::PositiveX)
            .unwrap();
        let positive_y = endpoints
            .iter()
            .find(|endpoint| endpoint.view == AxisView::PositiveY)
            .unwrap();

        assert_eq!(positive_x.position, center + egui::vec2(AXIS_LENGTH, 0.0));
        assert_eq!(positive_y.position, center + egui::vec2(0.0, -AXIS_LENGTH));
    }
}
