use glam::Vec3;

use crate::{
    application::AppState,
    core::camera::{CameraProjection, MIN_CAMERA_DISTANCE, OrbitCamera},
};

pub fn fit_camera_to_document(state: &AppState) -> Option<OrbitCamera> {
    fit_camera_to_document_from(state, state.camera().clone())
}

pub fn reset_camera_for_document(state: &AppState) -> OrbitCamera {
    let default = OrbitCamera::default();
    fit_camera_to_document_from(state, default.clone()).unwrap_or(default)
}

fn fit_camera_to_document_from(state: &AppState, mut camera: OrbitCamera) -> Option<OrbitCamera> {
    let document = state.document()?;
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut has_position = false;
    for position in document
        .mesh
        .positions
        .iter()
        .copied()
        .filter(|position| position.is_finite())
    {
        min = min.min(position);
        max = max.max(position);
        has_position = true;
    }
    if !has_position {
        return None;
    }
    let center = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(0.2);
    fit_camera_to_sphere(&mut camera, center, radius);
    Some(camera)
}

fn fit_camera_to_sphere(camera: &mut OrbitCamera, center: Vec3, radius: f32) {
    camera.target = center;
    match camera.projection {
        CameraProjection::Perspective => {
            camera.distance = radius / (camera.fov_y_radians * 0.5).tan() + radius;
        }
        CameraProjection::Orthographic => {
            camera.set_orthographic_height(radius * 2.0 * 1.1);
            camera.distance = (radius * 2.0).max(MIN_CAMERA_DISTANCE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_fit_preserves_existing_distance_formula() {
        let mut camera = OrbitCamera::default();
        let radius = 2.0;
        fit_camera_to_sphere(&mut camera, Vec3::ONE, radius);
        let expected = radius / (camera.fov_y_radians * 0.5).tan() + radius;
        assert_eq!(camera.target, Vec3::ONE);
        assert!((camera.distance - expected).abs() < 1e-5);
    }

    #[test]
    fn orthographic_fit_sets_margin_and_keeps_camera_outside_radius() {
        let mut camera = OrbitCamera::default();
        camera.set_projection(CameraProjection::Orthographic);
        fit_camera_to_sphere(&mut camera, Vec3::ONE, 2.0);
        assert_eq!(camera.target, Vec3::ONE);
        assert!((camera.orthographic_height - 4.4).abs() < 1e-5);
        assert!(camera.distance > 2.0);
    }
}
