use glam::Vec2;

use crate::core::uv_view::{UV_VIEW_MIN_VISIBLE_PX, UvViewTransform};

use super::view_navigation::ViewNavigationAction;

const ROTATION_RADIANS_PER_POINT: f32 = 0.3_f32.to_radians();
const ROTATION_SNAP_RADIANS: f32 = 15.0_f32.to_radians();
const DRAG_ZOOM_EXPONENT_PER_POINT: f32 = 0.01;

#[derive(Debug, Clone, Copy, Default)]
pub struct UvViewNavigationState {
    gesture: Option<UvNavigationGesture>,
}

#[derive(Debug, Clone, Copy)]
enum UvNavigationGesture {
    Pan {
        start_transform: UvViewTransform,
    },
    Rotate {
        start_transform: UvViewTransform,
    },
    Zoom {
        start_transform: UvViewTransform,
        anchor_px: Vec2,
    },
}

impl UvViewNavigationState {
    pub fn begin(
        &mut self,
        action: ViewNavigationAction,
        transform: UvViewTransform,
        start_position_px: Vec2,
    ) {
        self.gesture = match action {
            ViewNavigationAction::PanUv => Some(UvNavigationGesture::Pan {
                start_transform: transform,
            }),
            ViewNavigationAction::RotateUv => Some(UvNavigationGesture::Rotate {
                start_transform: transform,
            }),
            ViewNavigationAction::ZoomUv => Some(UvNavigationGesture::Zoom {
                start_transform: transform,
                anchor_px: start_position_px,
            }),
            ViewNavigationAction::Orbit3d
            | ViewNavigationAction::Pan3d
            | ViewNavigationAction::Zoom3d
            | ViewNavigationAction::BrushSize => None,
        };
    }

    pub fn update(
        self,
        current_transform: UvViewTransform,
        total_delta: Vec2,
        dominant_delta: f32,
        view_size: [u32; 2],
        canvas_size: [u32; 2],
        snap_rotation: bool,
    ) -> UvViewTransform {
        let candidate = match self.gesture {
            Some(UvNavigationGesture::Pan { start_transform }) => {
                start_transform.panned_by_screen_delta(total_delta, canvas_size)
            }
            Some(UvNavigationGesture::Rotate { start_transform }) => {
                let unsnapped_rotation =
                    start_transform.rotation_radians + dominant_delta * ROTATION_RADIANS_PER_POINT;
                let rotation = if snap_rotation {
                    (unsnapped_rotation / ROTATION_SNAP_RADIANS).round() * ROTATION_SNAP_RADIANS
                } else {
                    unsnapped_rotation
                };
                start_transform.with_rotation(rotation)
            }
            Some(UvNavigationGesture::Zoom {
                start_transform,
                anchor_px,
            }) => start_transform.zoom_about(
                anchor_px,
                view_size,
                canvas_size,
                (-dominant_delta * DRAG_ZOOM_EXPONENT_PER_POINT).exp(),
            ),
            None => current_transform,
        };
        candidate.clamped_for_view(view_size, canvas_size, UV_VIEW_MIN_VISIBLE_PX)
    }

    pub fn end(&mut self) {
        self.gesture = None;
    }

    pub fn is_active(self) -> bool {
        self.gesture.is_some()
    }

    pub fn is_rotating(self) -> bool {
        matches!(self.gesture, Some(UvNavigationGesture::Rotate { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_vec2_close(actual: Vec2, expected: Vec2) {
        let delta = actual - expected;
        assert!(
            delta.abs().max_element() <= 2.0e-4,
            "actual={actual:?} expected={expected:?} delta={delta:?}"
        );
    }

    #[test]
    fn pan_uses_total_delta_from_gesture_start() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        state.begin(ViewNavigationAction::PanUv, transform, Vec2::ZERO);
        let panned = state.update(
            transform,
            Vec2::new(40.0, 20.0),
            40.0,
            [400, 400],
            [400, 400],
            false,
        );
        assert_vec2_close(panned.center_uv, Vec2::new(0.4, 0.45));
    }

    #[test]
    fn rotation_snaps_to_fifteen_degrees() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        state.begin(ViewNavigationAction::RotateUv, transform, Vec2::ZERO);
        let rotated = state.update(
            transform,
            Vec2::new(47.0, 0.0),
            47.0,
            [400, 400],
            [400, 400],
            true,
        );
        assert!((rotated.rotation_radians - 15.0_f32.to_radians()).abs() <= 1.0e-5);
    }

    #[test]
    fn releasing_snap_restores_unsnapped_rotation() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        state.begin(ViewNavigationAction::RotateUv, transform, Vec2::ZERO);
        let snapped = state.update(
            transform,
            Vec2::new(30.0, 0.0),
            30.0,
            [400, 400],
            [400, 400],
            true,
        );
        let unsnapped = state.update(
            snapped,
            Vec2::new(30.0, 0.0),
            30.0,
            [400, 400],
            [400, 400],
            false,
        );
        assert!((unsnapped.rotation_radians - 9.0_f32.to_radians()).abs() <= 1.0e-5);
    }

    #[test]
    fn zoom_keeps_drag_start_anchor_fixed() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        let anchor = Vec2::new(120.0, 170.0);
        let anchor_uv = transform.view_px_to_uv(anchor, [400, 400], [400, 400]);
        state.begin(ViewNavigationAction::ZoomUv, transform, anchor);
        let zoomed = state.update(
            transform,
            Vec2::new(0.0, -50.0),
            -50.0,
            [400, 400],
            [400, 400],
            false,
        );
        assert_vec2_close(
            zoomed.view_px_to_uv(anchor, [400, 400], [400, 400]),
            anchor_uv,
        );
    }

    #[test]
    fn vertical_drag_rotates_canvas() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        state.begin(ViewNavigationAction::RotateUv, transform, Vec2::ZERO);
        let rotated = state.update(
            transform,
            Vec2::new(0.0, 30.0),
            30.0,
            [400, 400],
            [400, 400],
            false,
        );
        assert!((rotated.rotation_radians - 9.0_f32.to_radians()).abs() <= 1.0e-5);
    }

    #[test]
    fn horizontal_drag_zooms_while_keeping_anchor_fixed() {
        let mut state = UvViewNavigationState::default();
        let transform = UvViewTransform::default();
        let anchor = Vec2::new(120.0, 170.0);
        let anchor_uv = transform.view_px_to_uv(anchor, [400, 400], [400, 400]);
        state.begin(ViewNavigationAction::ZoomUv, transform, anchor);
        let zoomed = state.update(
            transform,
            Vec2::new(-50.0, 0.0),
            -50.0,
            [400, 400],
            [400, 400],
            false,
        );
        assert!(zoomed.zoom > transform.zoom);
        assert_vec2_close(
            zoomed.view_px_to_uv(anchor, [400, 400], [400, 400]),
            anchor_uv,
        );
    }

    #[test]
    fn non_uv_action_does_not_start_transform_navigation() {
        let mut state = UvViewNavigationState::default();
        state.begin(
            ViewNavigationAction::BrushSize,
            UvViewTransform::default(),
            Vec2::ZERO,
        );
        assert!(!state.is_active());
        assert!(!state.is_rotating());
    }
}
