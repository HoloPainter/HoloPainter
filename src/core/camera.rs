use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

const CAMERA_ZOOM_SPEED: f32 = 0.002;
pub const MIN_CAMERA_DISTANCE: f32 = 0.1;
pub const MAX_CAMERA_DISTANCE: f32 = 100.0;
pub const MIN_FOV_Y_DEGREES: f32 = 10.0;
pub const MAX_FOV_Y_DEGREES: f32 = 120.0;
pub const MIN_ORTHOGRAPHIC_HEIGHT: f32 = 0.01;
pub const MAX_ORTHOGRAPHIC_HEIGHT: f32 = 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraProjection {
    Perspective,
    Orthographic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub orientation: Quat,
    pub distance: f32,
    pub projection: CameraProjection,
    pub fov_y_radians: f32,
    pub orthographic_height: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let distance = 3.0;
        let fov_y_radians = 50.0_f32.to_radians();
        Self {
            target: Vec3::ZERO,
            orientation: orientation_from_forward_up(
                Vec3::new(
                    0.3_f32.cos() * 0.8_f32.cos(),
                    0.3_f32.sin(),
                    0.3_f32.cos() * 0.8_f32.sin(),
                ),
                Vec3::Y,
            ),
            distance,
            projection: CameraProjection::Perspective,
            fov_y_radians,
            orthographic_height: 2.0 * distance * (fov_y_radians * 0.5).tan(),
            near: 0.1,
            far: 200.0,
        }
    }
}

impl OrbitCamera {
    pub fn position(&self) -> Vec3 {
        self.target - self.forward() * self.distance
    }

    pub fn forward(&self) -> Vec3 {
        (self.orientation.normalize() * Vec3::NEG_Z).normalize_or_zero()
    }

    pub fn right(&self) -> Vec3 {
        (self.orientation.normalize() * Vec3::X).normalize_or_zero()
    }

    pub fn up(&self) -> Vec3 {
        (self.orientation.normalize() * Vec3::Y).normalize_or_zero()
    }

    pub fn view_matrix(&self) -> Mat4 {
        let position = self.position();
        glam::camera::rh::view::look_at_mat4(position, position + self.forward(), self.up())
    }

    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() {
            aspect.max(0.001)
        } else {
            1.0
        };
        match self.projection {
            CameraProjection::Perspective => glam::camera::rh::proj::opengl::perspective(
                self.fov_y_radians,
                aspect,
                self.near,
                self.far,
            ),
            CameraProjection::Orthographic => {
                let half_height = self.orthographic_height * 0.5;
                let half_width = half_height * aspect;
                glam::camera::rh::proj::opengl::orthographic(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    self.near,
                    self.far,
                )
            }
        }
    }

    pub fn view_projection_matrix(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.orbit_radians(delta * 0.01);
    }

    pub fn orbit_radians(&mut self, delta_radians: Vec2) {
        let yaw = Quat::from_axis_angle(self.up(), -delta_radians.x);
        let pitch = Quat::from_axis_angle(self.right(), -delta_radians.y);
        self.orientation = (yaw * pitch * self.orientation).normalize();
    }

    pub fn pan(&mut self, delta: Vec2) {
        let scale_basis = match self.projection {
            CameraProjection::Perspective => self.distance,
            CameraProjection::Orthographic => self.orthographic_height,
        };
        let scale = scale_basis * 0.0015;
        self.target -= self.right() * delta.x * scale;
        self.target += self.up() * delta.y * scale;
    }

    pub fn zoom(&mut self, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        let factor = 1.0 - delta * CAMERA_ZOOM_SPEED;
        match self.projection {
            CameraProjection::Perspective => {
                self.distance =
                    (self.distance * factor).clamp(MIN_CAMERA_DISTANCE, MAX_CAMERA_DISTANCE);
            }
            CameraProjection::Orthographic => {
                self.orthographic_height = (self.orthographic_height * factor)
                    .clamp(MIN_ORTHOGRAPHIC_HEIGHT, MAX_ORTHOGRAPHIC_HEIGHT);
            }
        }
    }

    pub fn zoom_by_factor(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        match self.projection {
            CameraProjection::Perspective => {
                self.distance =
                    (self.distance * factor).clamp(MIN_CAMERA_DISTANCE, MAX_CAMERA_DISTANCE);
            }
            CameraProjection::Orthographic => {
                self.orthographic_height = (self.orthographic_height * factor)
                    .clamp(MIN_ORTHOGRAPHIC_HEIGHT, MAX_ORTHOGRAPHIC_HEIGHT);
            }
        }
    }

    pub fn set_view_direction(&mut self, forward: Vec3, up_reference: Vec3) {
        self.orientation = orientation_from_forward_up(forward, up_reference);
    }

    pub fn set_projection(&mut self, projection: CameraProjection) {
        if self.projection == projection {
            return;
        }
        match projection {
            CameraProjection::Perspective => {
                let tan_half_fov = (self.fov_y_radians * 0.5).tan();
                self.distance = (self.orthographic_height / (2.0 * tan_half_fov))
                    .clamp(MIN_CAMERA_DISTANCE, MAX_CAMERA_DISTANCE);
            }
            CameraProjection::Orthographic => {
                self.orthographic_height = self
                    .visible_height_at_target()
                    .clamp(MIN_ORTHOGRAPHIC_HEIGHT, MAX_ORTHOGRAPHIC_HEIGHT);
            }
        }
        self.projection = projection;
    }

    pub fn set_fov_y_degrees(&mut self, degrees: f32) {
        if degrees.is_finite() {
            self.fov_y_radians = degrees
                .clamp(MIN_FOV_Y_DEGREES, MAX_FOV_Y_DEGREES)
                .to_radians();
        }
    }

    pub fn set_orthographic_height(&mut self, height: f32) {
        if height.is_finite() && height > 0.0 {
            self.orthographic_height =
                height.clamp(MIN_ORTHOGRAPHIC_HEIGHT, MAX_ORTHOGRAPHIC_HEIGHT);
        }
    }

    pub fn visible_height_at_target(&self) -> f32 {
        match self.projection {
            CameraProjection::Perspective => 2.0 * self.distance * (self.fov_y_radians * 0.5).tan(),
            CameraProjection::Orthographic => self.orthographic_height,
        }
    }
}

fn orientation_from_forward_up(forward: Vec3, up_reference: Vec3) -> Quat {
    let forward = forward.normalize_or_zero();
    let fallback_forward = if forward.length_squared() > f32::EPSILON {
        forward
    } else {
        Vec3::NEG_Z
    };
    let mut right = fallback_forward.cross(up_reference).normalize_or_zero();
    if right.length_squared() <= f32::EPSILON {
        right = fallback_forward.cross(Vec3::X).normalize_or_zero();
    }
    if right.length_squared() <= f32::EPSILON {
        right = Vec3::X;
    }
    let up = right.cross(fallback_forward).normalize_or_zero();
    Quat::from_mat3(&Mat3::from_cols(right, up, -fallback_forward)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approx_eq(left: f32, right: f32, tolerance: f32) {
        assert!(
            (left - right).abs() <= tolerance,
            "left={left}, right={right}, tolerance={tolerance}"
        );
    }

    fn assert_vec3_approx_eq(left: Vec3, right: Vec3, tolerance: f32) {
        assert_approx_eq(left.x, right.x, tolerance);
        assert_approx_eq(left.y, right.y, tolerance);
        assert_approx_eq(left.z, right.z, tolerance);
    }

    fn assert_finite_matrix(matrix: Mat4) {
        assert!(
            matrix.to_cols_array().into_iter().all(f32::is_finite),
            "{matrix:?}"
        );
    }

    #[test]
    fn set_view_direction_preserves_camera_framing_and_projection() {
        let mut camera = OrbitCamera::default();
        let target = camera.target;
        let distance = camera.distance;
        let projection = camera.projection;
        let fov_y_radians = camera.fov_y_radians;
        let orthographic_height = camera.orthographic_height;
        let near = camera.near;
        let far = camera.far;

        camera.set_view_direction(Vec3::NEG_X, Vec3::Y);

        assert_vec3_approx_eq(camera.forward(), Vec3::NEG_X, 1e-6);
        assert_eq!(camera.target, target);
        assert_eq!(camera.distance, distance);
        assert_eq!(camera.projection, projection);
        assert_eq!(camera.fov_y_radians, fov_y_radians);
        assert_eq!(camera.orthographic_height, orthographic_height);
        assert_eq!(camera.near, near);
        assert_eq!(camera.far, far);
    }

    #[test]
    fn set_view_direction_keeps_camera_basis_orthonormal() {
        let mut camera = OrbitCamera::default();
        camera.set_view_direction(Vec3::NEG_Y, Vec3::NEG_Z);

        let forward = camera.forward();
        let right = camera.right();
        let up = camera.up();
        assert_approx_eq(forward.length(), 1.0, 1e-5);
        assert_approx_eq(right.length(), 1.0, 1e-5);
        assert_approx_eq(up.length(), 1.0, 1e-5);
        assert_approx_eq(forward.dot(right), 0.0, 1e-5);
        assert_approx_eq(forward.dot(up), 0.0, 1e-5);
        assert_approx_eq(right.dot(up), 0.0, 1e-5);
        assert_vec3_approx_eq(right, Vec3::X, 1e-5);
    }

    #[test]
    fn vertical_orbit_can_pass_ninety_degrees() {
        let mut camera = OrbitCamera {
            orientation: Quat::IDENTITY,
            ..OrbitCamera::default()
        };

        camera.orbit(Vec2::new(0.0, -200.0));

        assert!(camera.forward().y > 0.9, "{:?}", camera.forward());
        assert_finite_matrix(camera.view_matrix());
        assert_finite_matrix(camera.view_projection_matrix(1.0));
    }

    #[test]
    fn repeated_free_orbit_keeps_camera_basis_orthonormal() {
        let mut camera = OrbitCamera::default();

        for _ in 0..200 {
            camera.orbit(Vec2::new(17.0, -23.0));
        }

        let forward = camera.forward();
        let right = camera.right();
        let up = camera.up();
        assert_approx_eq(forward.length(), 1.0, 1e-5);
        assert_approx_eq(right.length(), 1.0, 1e-5);
        assert_approx_eq(up.length(), 1.0, 1e-5);
        assert_approx_eq(forward.dot(right), 0.0, 1e-5);
        assert_approx_eq(forward.dot(up), 0.0, 1e-5);
        assert_approx_eq(right.dot(up), 0.0, 1e-5);
    }

    #[test]
    fn pan_uses_current_rolled_screen_axes() {
        let mut camera = OrbitCamera {
            target: Vec3::ZERO,
            orientation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            distance: 2.0,
            ..OrbitCamera::default()
        };
        let right = camera.right();
        let up = camera.up();

        camera.pan(Vec2::new(10.0, 20.0));

        let scale = 2.0 * 0.0015;
        assert_vec3_approx_eq(
            camera.target,
            -right * 10.0 * scale + up * 20.0 * scale,
            1e-6,
        );
    }

    #[test]
    fn zoom_preserves_existing_distance_clamp() {
        let mut camera = OrbitCamera {
            distance: 3.0,
            ..OrbitCamera::default()
        };

        camera.zoom(10_000.0);
        assert_approx_eq(camera.distance, 0.1, 1e-6);

        camera.zoom(-1_000_000.0);
        assert_approx_eq(camera.distance, 100.0, 1e-6);
    }

    #[test]
    fn zoom_by_factor_updates_perspective_distance() {
        let mut camera = OrbitCamera {
            distance: 4.0,
            ..OrbitCamera::default()
        };

        camera.zoom_by_factor(0.5);

        assert_approx_eq(camera.distance, 2.0, 1e-6);
    }

    #[test]
    fn zoom_by_factor_updates_orthographic_height() {
        let mut camera = OrbitCamera {
            projection: CameraProjection::Orthographic,
            orthographic_height: 8.0,
            ..OrbitCamera::default()
        };

        camera.zoom_by_factor(0.25);

        assert_approx_eq(camera.orthographic_height, 2.0, 1e-6);
    }

    #[test]
    fn zoom_by_factor_rejects_invalid_factors_and_clamps_valid_ones() {
        let mut camera = OrbitCamera {
            distance: 3.0,
            ..OrbitCamera::default()
        };

        camera.zoom_by_factor(f32::NAN);
        camera.zoom_by_factor(0.0);
        assert_approx_eq(camera.distance, 3.0, 1e-6);

        camera.zoom_by_factor(1.0e-6);
        assert_approx_eq(camera.distance, MIN_CAMERA_DISTANCE, 1e-6);

        camera.zoom_by_factor(1.0e9);
        assert_approx_eq(camera.distance, MAX_CAMERA_DISTANCE, 1e-6);
    }

    #[test]
    fn default_orthographic_height_matches_perspective_target_height() {
        let camera = OrbitCamera::default();
        assert_eq!(camera.projection, CameraProjection::Perspective);
        assert_approx_eq(camera.fov_y_radians.to_degrees(), 50.0, 1e-5);
        assert_approx_eq(camera.orthographic_height, 2.797_845, 1e-5);
    }

    #[test]
    fn projection_switch_preserves_visible_height_at_target() {
        let mut camera = OrbitCamera::default();
        let visible_height = camera.visible_height_at_target();
        camera.set_projection(CameraProjection::Orthographic);
        assert_approx_eq(camera.orthographic_height, visible_height, 1e-5);

        camera.set_orthographic_height(8.0);
        camera.set_projection(CameraProjection::Perspective);
        assert_approx_eq(camera.visible_height_at_target(), 8.0, 1e-5);
    }

    #[test]
    fn setters_reject_non_finite_values_and_clamp_finite_values() {
        let mut camera = OrbitCamera::default();
        let fov = camera.fov_y_radians;
        let height = camera.orthographic_height;
        camera.set_fov_y_degrees(f32::NAN);
        camera.set_orthographic_height(f32::INFINITY);
        camera.set_orthographic_height(0.0);
        assert_eq!(camera.fov_y_radians, fov);
        assert_eq!(camera.orthographic_height, height);

        camera.set_fov_y_degrees(1.0);
        camera.set_orthographic_height(10_000.0);
        assert_approx_eq(camera.fov_y_radians.to_degrees(), MIN_FOV_Y_DEGREES, 1e-5);
        assert_eq!(camera.orthographic_height, MAX_ORTHOGRAPHIC_HEIGHT);
    }

    #[test]
    fn orthographic_zoom_changes_height_without_moving_camera() {
        let mut camera = OrbitCamera::default();
        camera.set_projection(CameraProjection::Orthographic);
        let distance = camera.distance;
        let height = camera.orthographic_height;
        camera.zoom(100.0);
        assert_eq!(camera.distance, distance);
        assert!(camera.orthographic_height < height);
    }

    #[test]
    fn orthographic_projection_is_finite_for_extreme_aspects() {
        let mut camera = OrbitCamera::default();
        camera.set_projection(CameraProjection::Orthographic);
        assert_finite_matrix(camera.view_projection_matrix(0.0));
        assert_finite_matrix(camera.view_projection_matrix(100_000.0));
        assert_finite_matrix(camera.view_projection_matrix(f32::NAN));
    }
}
