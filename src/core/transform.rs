use glam::{Mat3, Vec2, Vec3};

const MATRIX_EPSILON: f32 = 1.0e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvTransform {
    matrix: Mat3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformNumericValues {
    pub center_px: Vec2,
    pub size_px: Vec2,
    pub rotation_degrees: f32,
}

impl UvTransform {
    pub const fn identity() -> Self {
        Self {
            matrix: Mat3::IDENTITY,
        }
    }

    pub fn from_matrix(matrix: Mat3) -> Option<Self> {
        let elements = matrix.to_cols_array();
        if elements.iter().any(|value| !value.is_finite())
            || matrix.x_axis.z.abs() > MATRIX_EPSILON
            || matrix.y_axis.z.abs() > MATRIX_EPSILON
            || (matrix.z_axis.z - 1.0).abs() > MATRIX_EPSILON
        {
            return None;
        }
        let determinant = matrix.determinant();
        (determinant.is_finite() && determinant.abs() > MATRIX_EPSILON).then_some(Self { matrix })
    }

    pub fn from_trs(
        translation_px: Vec2,
        rotation_radians: f32,
        scale: Vec2,
        pivot_px: Vec2,
    ) -> Option<Self> {
        Self::from_matrix(
            Mat3::from_translation(translation_px)
                * Mat3::from_translation(pivot_px)
                * Mat3::from_angle(rotation_radians)
                * Mat3::from_scale(scale)
                * Mat3::from_translation(-pivot_px),
        )
    }

    pub const fn matrix(self) -> Mat3 {
        self.matrix
    }

    pub fn inverse_matrix(self) -> Option<Mat3> {
        let determinant = self.matrix.determinant();
        (determinant.is_finite() && determinant.abs() > MATRIX_EPSILON)
            .then(|| self.matrix.inverse())
    }

    pub fn transform_point(self, point_px: Vec2) -> Vec2 {
        let point = self.matrix * Vec3::new(point_px.x, point_px.y, 1.0);
        point.truncate()
    }

    pub fn is_effectively_identity(self) -> bool {
        self.matrix
            .to_cols_array()
            .into_iter()
            .zip(Mat3::IDENTITY.to_cols_array())
            .all(|(actual, expected)| (actual - expected).abs() <= MATRIX_EPSILON)
    }

    pub fn numeric_values(
        self,
        source_origin_px: Vec2,
        source_size_px: Vec2,
    ) -> TransformNumericValues {
        self.numeric_values_with_scale_sign(source_origin_px, source_size_px, Vec2::ONE)
    }

    pub fn numeric_values_with_scale_sign(
        self,
        source_origin_px: Vec2,
        source_size_px: Vec2,
        scale_sign: Vec2,
    ) -> TransformNumericValues {
        let pivot_px = source_origin_px + source_size_px * 0.5;
        let matrix = self.matrix();
        let scale_sign = Vec2::new(
            if scale_sign.x < 0.0 { -1.0 } else { 1.0 },
            if scale_sign.y < 0.0 { -1.0 } else { 1.0 },
        );
        let rotation_axis = matrix.x_axis.truncate() * scale_sign.x;
        TransformNumericValues {
            center_px: self.transform_point(pivot_px),
            size_px: Vec2::new(
                source_size_px.x * matrix.x_axis.truncate().length() * scale_sign.x,
                source_size_px.y * matrix.y_axis.truncate().length() * scale_sign.y,
            ),
            rotation_degrees: normalize_degrees(
                rotation_axis.y.atan2(rotation_axis.x).to_degrees(),
            ),
        }
    }

    pub fn from_numeric_values(
        source_origin_px: Vec2,
        source_size_px: Vec2,
        values: TransformNumericValues,
    ) -> Option<Self> {
        if !source_origin_px.is_finite()
            || !source_size_px.is_finite()
            || !values.center_px.is_finite()
            || !values.size_px.is_finite()
            || !values.rotation_degrees.is_finite()
            || !source_size_px.cmpgt(Vec2::ZERO).all()
            || values.size_px.x == 0.0
            || values.size_px.y == 0.0
        {
            return None;
        }
        let pivot_px = source_origin_px + source_size_px * 0.5;
        Self::from_trs(
            values.center_px - pivot_px,
            normalize_degrees(values.rotation_degrees).to_radians(),
            values.size_px / source_size_px,
            pivot_px,
        )
    }
}

fn normalize_degrees(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

impl Default for UvTransform {
    fn default() -> Self {
        Self::identity()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformScaleConstraint {
    Free,
    PreserveAspect,
}

impl Default for TransformScaleConstraint {
    fn default() -> Self {
        Self::Free
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformHandle {
    Move,
    ScaleLeft,
    ScaleRight,
    ScaleTop,
    ScaleBottom,
    ScaleTopLeft,
    ScaleTopRight,
    ScaleBottomLeft,
    ScaleBottomRight,
    Rotate,
}

impl TransformHandle {
    pub fn affects_x(self) -> bool {
        matches!(
            self,
            Self::ScaleLeft
                | Self::ScaleRight
                | Self::ScaleTopLeft
                | Self::ScaleTopRight
                | Self::ScaleBottomLeft
                | Self::ScaleBottomRight
        )
    }

    pub fn affects_y(self) -> bool {
        matches!(
            self,
            Self::ScaleTop
                | Self::ScaleBottom
                | Self::ScaleTopLeft
                | Self::ScaleTopRight
                | Self::ScaleBottomLeft
                | Self::ScaleBottomRight
        )
    }

    pub fn is_corner(self) -> bool {
        matches!(
            self,
            Self::ScaleTopLeft
                | Self::ScaleTopRight
                | Self::ScaleBottomLeft
                | Self::ScaleBottomRight
        )
    }
}

pub(crate) fn hit_test_transform_handle(
    pointer: Vec2,
    corners: [Vec2; 4],
    handle_hit_radius: f32,
) -> TransformHandle {
    let edge_centers = [
        (corners[0] + corners[1]) * 0.5,
        (corners[1] + corners[2]) * 0.5,
        (corners[2] + corners[3]) * 0.5,
        (corners[3] + corners[0]) * 0.5,
    ];
    let handles = [
        (TransformHandle::ScaleTopLeft, corners[0]),
        (TransformHandle::ScaleTopRight, corners[1]),
        (TransformHandle::ScaleBottomRight, corners[2]),
        (TransformHandle::ScaleBottomLeft, corners[3]),
        (TransformHandle::ScaleTop, edge_centers[0]),
        (TransformHandle::ScaleRight, edge_centers[1]),
        (TransformHandle::ScaleBottom, edge_centers[2]),
        (TransformHandle::ScaleLeft, edge_centers[3]),
    ];
    let hit_radius_squared = handle_hit_radius * handle_hit_radius;
    if let Some((handle, _)) = handles
        .into_iter()
        .find(|(_, point)| point.distance_squared(pointer) <= hit_radius_squared)
    {
        return handle;
    }

    if point_in_convex_quad(pointer, corners) {
        TransformHandle::Move
    } else {
        TransformHandle::Rotate
    }
}

fn point_in_convex_quad(point: Vec2, quad: [Vec2; 4]) -> bool {
    let mut sign = 0.0f32;
    for index in 0..4 {
        let a = quad[index];
        let b = quad[(index + 1) % 4];
        let cross = (b - a).perp_dot(point - a);
        if cross.abs() <= 1.0e-4 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvTransformPreview {
    pub corners_uv: [Vec2; 4],
    pub pivot_uv: Vec2,
    pub active_handle: Option<TransformHandle>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_rotates_and_translates_about_pivot() {
        let transform = UvTransform::from_trs(
            Vec2::new(3.0, -2.0),
            std::f32::consts::FRAC_PI_2,
            Vec2::ONE,
            Vec2::new(1.0, 1.0),
        )
        .unwrap();

        let transformed = transform.transform_point(Vec2::new(2.0, 1.0));

        assert!((transformed - Vec2::new(4.0, 0.0)).length() < 1.0e-5);
    }

    #[test]
    fn inverse_matrix_round_trips_points() {
        let transform = UvTransform::from_trs(
            Vec2::new(5.0, 7.0),
            0.4,
            Vec2::new(1.5, 0.75),
            Vec2::new(3.0, 2.0),
        )
        .unwrap();
        let point = Vec2::new(11.0, 13.0);
        let transformed = transform.transform_point(point);
        let inverse = transform.inverse_matrix().unwrap();
        let recovered = inverse * transformed.extend(1.0);

        assert!((recovered.truncate() - point).length() < 1.0e-4);
    }

    #[test]
    fn numeric_values_round_trip_center_size_and_normalized_rotation() {
        let values = TransformNumericValues {
            center_px: Vec2::new(-12.5, 48.0),
            size_px: Vec2::new(30.0, 7.5),
            rotation_degrees: 450.0,
        };
        let transform =
            UvTransform::from_numeric_values(Vec2::new(10.0, 20.0), Vec2::new(12.0, 6.0), values)
                .unwrap();

        let actual = transform.numeric_values(Vec2::new(10.0, 20.0), Vec2::new(12.0, 6.0));

        assert!(actual.center_px.abs_diff_eq(values.center_px, 1.0e-5));
        assert!(actual.size_px.abs_diff_eq(values.size_px, 1.0e-5));
        assert!((actual.rotation_degrees - 90.0).abs() < 1.0e-5);
    }

    #[test]
    fn numeric_values_round_trip_negative_scale_axes() {
        let source_origin = Vec2::new(10.0, 20.0);
        let source_size = Vec2::new(12.0, 6.0);
        for size_px in [
            Vec2::new(-30.0, 7.5),
            Vec2::new(30.0, -7.5),
            Vec2::new(-30.0, -7.5),
        ] {
            let values = TransformNumericValues {
                center_px: Vec2::new(-12.5, 48.0),
                size_px,
                rotation_degrees: 37.0,
            };
            let transform =
                UvTransform::from_numeric_values(source_origin, source_size, values).unwrap();
            let actual = transform.numeric_values_with_scale_sign(
                source_origin,
                source_size,
                Vec2::new(size_px.x.signum(), size_px.y.signum()),
            );

            assert!(actual.center_px.abs_diff_eq(values.center_px, 1.0e-5));
            assert!(actual.size_px.abs_diff_eq(values.size_px, 1.0e-5));
            assert!((actual.rotation_degrees - values.rotation_degrees).abs() < 1.0e-5);
        }
    }

    #[test]
    fn numeric_values_reject_zero_scale_axes() {
        let source_origin = Vec2::ZERO;
        let source_size = Vec2::splat(10.0);
        let values = TransformNumericValues {
            center_px: Vec2::splat(5.0),
            size_px: Vec2::new(0.0, 10.0),
            rotation_degrees: 0.0,
        };

        assert!(UvTransform::from_numeric_values(source_origin, source_size, values).is_none());
    }

    #[test]
    fn rejects_non_finite_and_non_invertible_matrices() {
        assert!(UvTransform::from_matrix(Mat3::from_scale(Vec2::ZERO)).is_none());
        let mut values = Mat3::IDENTITY.to_cols_array();
        values[0] = f32::NAN;
        assert!(UvTransform::from_matrix(Mat3::from_cols_array(&values)).is_none());
    }

    #[test]
    fn transform_hit_test_moves_inside_and_rotates_outside() {
        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
        ];

        assert_eq!(
            hit_test_transform_handle(Vec2::new(2.0, 2.0), corners, 0.25),
            TransformHandle::Move
        );
        assert_eq!(
            hit_test_transform_handle(Vec2::new(6.0, 2.0), corners, 0.25),
            TransformHandle::Rotate
        );
    }

    #[test]
    fn transform_hit_test_prioritizes_scale_handles() {
        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
        ];
        assert_eq!(
            hit_test_transform_handle(Vec2::new(4.0, 2.0), corners, 0.25),
            TransformHandle::ScaleRight
        );
    }

    #[test]
    fn transform_hit_test_uses_the_rotated_quad_boundary() {
        let corners = [
            Vec2::new(0.0, -2.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 2.0),
            Vec2::new(-2.0, 0.0),
        ];
        assert_eq!(
            hit_test_transform_handle(Vec2::ZERO, corners, 0.1),
            TransformHandle::Move
        );
        assert_eq!(
            hit_test_transform_handle(Vec2::new(0.5, -1.5), corners, 0.1),
            TransformHandle::Move
        );
        assert_eq!(
            hit_test_transform_handle(Vec2::new(1.5, -1.5), corners, 0.1),
            TransformHandle::Rotate
        );
    }
}
