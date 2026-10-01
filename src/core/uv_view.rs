use glam::Vec2;

pub const UV_VIEW_MIN_ZOOM: f32 = 0.01;
pub const UV_VIEW_MAX_ZOOM: f32 = 32.0;
pub const UV_VIEW_MIN_VISIBLE_PX: f32 = 32.0;
pub const UV_VIEW_FIT_MARGIN_PX: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvViewTransform {
    pub center_uv: Vec2,
    /// Display scale relative to source texture pixels. `1.0` is 100%.
    pub zoom: f32,
    pub rotation_radians: f32,
}

impl Default for UvViewTransform {
    fn default() -> Self {
        Self {
            center_uv: Vec2::splat(0.5),
            zoom: 1.0,
            rotation_radians: 0.0,
        }
    }
}

impl UvViewTransform {
    pub fn normalized(self) -> Self {
        if !self.is_finite() {
            return Self::default();
        }
        Self {
            center_uv: self.center_uv,
            zoom: self.zoom.clamp(UV_VIEW_MIN_ZOOM, UV_VIEW_MAX_ZOOM),
            rotation_radians: normalize_angle(self.rotation_radians),
        }
    }

    pub fn is_finite(self) -> bool {
        self.center_uv.is_finite()
            && self.zoom.is_finite()
            && self.zoom > 0.0
            && self.rotation_radians.is_finite()
    }

    pub fn fit_to_view(view_size: [u32; 2], canvas_size: [u32; 2], margin_px: f32) -> Self {
        let view = view_size_vec(view_size);
        let canvas = canvas_size_vec(canvas_size);
        let margin = margin_px.max(0.0);
        let available = Vec2::new(
            (view.x - margin * 2.0).max(1.0),
            (view.y - margin * 2.0).max(1.0),
        );
        Self {
            center_uv: Vec2::splat(0.5),
            zoom: (available.x / canvas.x)
                .min(available.y / canvas.y)
                .clamp(UV_VIEW_MIN_ZOOM, UV_VIEW_MAX_ZOOM),
            rotation_radians: 0.0,
        }
    }

    pub fn uv_to_view_px(self, uv: Vec2, view_size: [u32; 2], canvas_size: [u32; 2]) -> Vec2 {
        let view = view_size_vec(view_size);
        let canvas = canvas_size_vec(canvas_size);
        view * 0.5
            + rotate_clockwise(
                (uv - self.center_uv) * canvas * self.zoom,
                self.rotation_radians,
            )
    }

    pub fn view_px_to_uv(
        self,
        position_px: Vec2,
        view_size: [u32; 2],
        canvas_size: [u32; 2],
    ) -> Vec2 {
        let view = view_size_vec(view_size);
        let canvas = canvas_size_vec(canvas_size);
        let canvas_offset = rotate_clockwise(position_px - view * 0.5, -self.rotation_radians);
        self.center_uv + canvas_offset / (canvas * self.zoom)
    }

    pub fn screen_delta_to_uv_delta(self, delta_px: Vec2, canvas_size: [u32; 2]) -> Vec2 {
        let canvas = canvas_size_vec(canvas_size);
        rotate_clockwise(delta_px, -self.rotation_radians) / (canvas * self.zoom)
    }

    pub fn panned_by_screen_delta(self, delta_px: Vec2, canvas_size: [u32; 2]) -> Self {
        Self {
            center_uv: self.center_uv - self.screen_delta_to_uv_delta(delta_px, canvas_size),
            ..self
        }
    }

    pub fn zoom_about(
        self,
        pointer_px: Vec2,
        view_size: [u32; 2],
        canvas_size: [u32; 2],
        zoom_factor: f32,
    ) -> Self {
        if !zoom_factor.is_finite() || zoom_factor <= 0.0 {
            return self.normalized();
        }
        let normalized = self.normalized();
        let anchor_uv = normalized.view_px_to_uv(pointer_px, view_size, canvas_size);
        let new_zoom = (normalized.zoom * zoom_factor).clamp(UV_VIEW_MIN_ZOOM, UV_VIEW_MAX_ZOOM);
        let view = view_size_vec(view_size);
        let canvas = canvas_size_vec(canvas_size);
        let pointer_canvas_offset =
            rotate_clockwise(pointer_px - view * 0.5, -normalized.rotation_radians);
        let center_uv = anchor_uv - pointer_canvas_offset / (canvas * new_zoom);
        Self {
            center_uv,
            zoom: new_zoom,
            rotation_radians: normalized.rotation_radians,
        }
    }

    pub fn rotated_by(self, delta_radians: f32) -> Self {
        if !delta_radians.is_finite() {
            return self.normalized();
        }
        Self {
            rotation_radians: normalize_angle(self.rotation_radians + delta_radians),
            ..self.normalized()
        }
    }

    pub fn with_rotation(self, rotation_radians: f32) -> Self {
        if !rotation_radians.is_finite() {
            return self.normalized();
        }
        Self {
            rotation_radians: normalize_angle(rotation_radians),
            ..self.normalized()
        }
    }

    pub fn canvas_corners_px(self, view_size: [u32; 2], canvas_size: [u32; 2]) -> [Vec2; 4] {
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ]
        .map(|uv| self.uv_to_view_px(uv, view_size, canvas_size))
    }

    pub fn contains_view_px(
        self,
        position_px: Vec2,
        view_size: [u32; 2],
        canvas_size: [u32; 2],
    ) -> bool {
        let uv = self.view_px_to_uv(position_px, view_size, canvas_size);
        (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)
    }

    pub fn clamped_for_view(
        self,
        view_size: [u32; 2],
        canvas_size: [u32; 2],
        minimum_visible_px: f32,
    ) -> Self {
        let mut transform = self.normalized();
        let view = view_size_vec(view_size);
        let minimum_visible = Vec2::new(
            minimum_visible_px.max(0.0).min(view.x * 0.5),
            minimum_visible_px.max(0.0).min(view.y * 0.5),
        );
        let corners = transform.canvas_corners_px(view_size, canvas_size);
        let mut min = corners[0];
        let mut max = corners[0];
        for corner in corners.into_iter().skip(1) {
            min = min.min(corner);
            max = max.max(corner);
        }

        let mut canvas_shift = Vec2::ZERO;
        if max.x < minimum_visible.x {
            canvas_shift.x = minimum_visible.x - max.x;
        } else if min.x > view.x - minimum_visible.x {
            canvas_shift.x = view.x - minimum_visible.x - min.x;
        }
        if max.y < minimum_visible.y {
            canvas_shift.y = minimum_visible.y - max.y;
        } else if min.y > view.y - minimum_visible.y {
            canvas_shift.y = view.y - minimum_visible.y - min.y;
        }

        if canvas_shift != Vec2::ZERO {
            transform = transform.panned_by_screen_delta(canvas_shift, canvas_size);
        }
        transform.normalized()
    }
}

pub fn rotate_clockwise(value: Vec2, radians: f32) -> Vec2 {
    let (sin, cos) = radians.sin_cos();
    Vec2::new(cos * value.x - sin * value.y, sin * value.x + cos * value.y)
}

fn view_size_vec(view_size: [u32; 2]) -> Vec2 {
    Vec2::new(view_size[0].max(1) as f32, view_size[1].max(1) as f32)
}

fn canvas_size_vec(canvas_size: [u32; 2]) -> Vec2 {
    Vec2::new(canvas_size[0].max(1) as f32, canvas_size[1].max(1) as f32)
}

fn normalize_angle(radians: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (radians + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: [u32; 2] = [800, 600];
    const CANVAS: [u32; 2] = [400, 300];

    fn assert_vec2_close(actual: Vec2, expected: Vec2) {
        let delta = actual - expected;
        assert!(
            delta.abs().max_element() <= 2.0e-4,
            "actual={actual:?} expected={expected:?} delta={delta:?}"
        );
    }

    #[test]
    fn one_x_maps_source_pixels_without_stretching_to_view() {
        let transform = UvViewTransform::default();
        assert_vec2_close(
            transform.uv_to_view_px(Vec2::ZERO, VIEW, CANVAS),
            Vec2::new(200.0, 150.0),
        );
        assert_vec2_close(
            transform.uv_to_view_px(Vec2::ONE, VIEW, CANVAS),
            Vec2::new(600.0, 450.0),
        );
    }

    #[test]
    fn resizing_view_keeps_canvas_display_size() {
        let transform = UvViewTransform::default();
        let small = transform.canvas_corners_px([600, 500], CANVAS);
        let large = transform.canvas_corners_px([1000, 800], CANVAS);
        assert_vec2_close(small[2] - small[0], Vec2::new(400.0, 300.0));
        assert_vec2_close(large[2] - large[0], Vec2::new(400.0, 300.0));
    }

    #[test]
    fn fit_to_view_uses_canvas_aspect_ratio() {
        let transform = UvViewTransform::fit_to_view(VIEW, CANVAS, 20.0);
        assert!((transform.zoom - 1.8666667).abs() <= 1.0e-5);
        let corners = transform.canvas_corners_px(VIEW, CANVAS);
        assert!((corners[0].y - 20.0).abs() <= 1.0e-4);
        assert!((corners[2].y - 580.0).abs() <= 1.0e-4);
    }

    #[test]
    fn zoom_can_go_below_one_x() {
        let zoomed =
            UvViewTransform::default().zoom_about(Vec2::new(400.0, 300.0), VIEW, CANVAS, 0.25);
        assert!((zoomed.zoom - 0.25).abs() <= 1.0e-5);
    }

    #[test]
    fn uv_and_view_coordinates_round_trip() {
        let transform = UvViewTransform {
            center_uv: Vec2::new(0.37, 0.62),
            zoom: 3.5,
            rotation_radians: 0.71,
        };
        let uv = Vec2::new(0.23, 0.81);
        let screen = transform.uv_to_view_px(uv, VIEW, CANVAS);
        assert_vec2_close(transform.view_px_to_uv(screen, VIEW, CANVAS), uv);
    }

    #[test]
    fn positive_rotation_is_clockwise_on_screen() {
        let transform = UvViewTransform::default().with_rotation(std::f32::consts::FRAC_PI_2);
        let right = transform.uv_to_view_px(Vec2::new(1.0, 0.5), VIEW, CANVAS);
        assert_vec2_close(right, Vec2::new(400.0, 500.0));
    }

    #[test]
    fn pan_follows_screen_drag_when_rotated() {
        let transform = UvViewTransform::default().with_rotation(std::f32::consts::FRAC_PI_2);
        let before = transform.uv_to_view_px(Vec2::new(0.5, 0.5), VIEW, CANVAS);
        let after_transform = transform.panned_by_screen_delta(Vec2::new(25.0, -10.0), CANVAS);
        let after = after_transform.uv_to_view_px(Vec2::new(0.5, 0.5), VIEW, CANVAS);
        assert_vec2_close(after - before, Vec2::new(25.0, -10.0));
    }

    #[test]
    fn zoom_keeps_pointer_anchor_fixed() {
        let transform = UvViewTransform {
            center_uv: Vec2::new(0.4, 0.6),
            zoom: 2.0,
            rotation_radians: 0.45,
        };
        let pointer = Vec2::new(610.0, 175.0);
        let anchor = transform.view_px_to_uv(pointer, VIEW, CANVAS);
        let zoomed = transform.zoom_about(pointer, VIEW, CANVAS, 1.75);
        assert_vec2_close(zoomed.uv_to_view_px(anchor, VIEW, CANVAS), pointer);
    }

    #[test]
    fn one_x_canvas_can_pan_but_cannot_be_lost() {
        let panned = UvViewTransform::default()
            .panned_by_screen_delta(Vec2::new(1000.0, 1000.0), CANVAS)
            .clamped_for_view(VIEW, CANVAS, UV_VIEW_MIN_VISIBLE_PX);
        let corners = panned.canvas_corners_px(VIEW, CANVAS);
        let max = corners
            .into_iter()
            .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
        assert!(max.x >= UV_VIEW_MIN_VISIBLE_PX - 1.0e-4);
        assert!(max.y >= UV_VIEW_MIN_VISIBLE_PX - 1.0e-4);
        assert_ne!(panned.center_uv, Vec2::splat(0.5));
    }

    #[test]
    fn rotated_canvas_cannot_be_lost() {
        let panned = UvViewTransform::default()
            .with_rotation(0.73)
            .panned_by_screen_delta(Vec2::new(-2000.0, 1800.0), CANVAS)
            .clamped_for_view(VIEW, CANVAS, UV_VIEW_MIN_VISIBLE_PX);
        let corners = panned.canvas_corners_px(VIEW, CANVAS);
        let min = corners
            .into_iter()
            .fold(Vec2::splat(f32::INFINITY), Vec2::min);
        assert!(min.x <= VIEW[0] as f32 - UV_VIEW_MIN_VISIBLE_PX + 1.0e-4);
        assert!(min.y <= VIEW[1] as f32 - UV_VIEW_MIN_VISIBLE_PX + 1.0e-4);
    }

    #[test]
    fn invalid_transform_normalizes_to_default() {
        let invalid = UvViewTransform {
            center_uv: Vec2::new(f32::NAN, 0.5),
            zoom: f32::INFINITY,
            rotation_radians: 0.0,
        };
        assert_eq!(invalid.normalized(), UvViewTransform::default());
    }
}
