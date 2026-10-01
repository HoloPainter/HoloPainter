use glam::{Mat4, Vec2, Vec3, Vec4};

pub fn gl_to_wgpu_depth() -> Mat4 {
    Mat4::from_cols_array(&[
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0,
    ])
}

pub fn ndc_to_depth01(ndc_z: f32) -> f32 {
    ndc_z * 0.5 + 0.5
}

pub fn ray_from_viewport_px(
    px: Vec2,
    viewport_size: [u32; 2],
    inv_view_proj: Mat4,
) -> Option<(Vec3, Vec3)> {
    let width = viewport_size[0].max(1) as f32;
    let height = viewport_size[1].max(1) as f32;
    let ndc_x = (px.x / width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (px.y / height) * 2.0;

    let near = inv_view_proj * Vec4::new(ndc_x, ndc_y, -1.0, 1.0);
    let far = inv_view_proj * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if near.w.abs() <= f32::EPSILON || far.w.abs() <= f32::EPSILON {
        return None;
    }

    let near_world = near.truncate() / near.w;
    let far_world = far.truncate() / far.w;
    let dir = (far_world - near_world).normalize_or_zero();
    (dir.length_squared() > f32::EPSILON).then_some((near_world, dir))
}

pub fn uv_scissor_rect(
    center_uv: Vec2,
    radius_px: f32,
    tex_size: [u32; 2],
) -> Option<(u32, u32, u32, u32)> {
    let tex_w = tex_size[0].max(1);
    let tex_h = tex_size[1].max(1);
    let center_x = center_uv.x * tex_w as f32;
    let center_y = center_uv.y * tex_h as f32;

    let min_x = (center_x - radius_px).floor().max(0.0) as u32;
    let min_y = (center_y - radius_px).floor().max(0.0) as u32;
    let max_x = (center_x + radius_px).ceil().min(tex_w as f32) as u32;
    let max_y = (center_y + radius_px).ceil().min(tex_h as f32) as u32;

    if max_x <= min_x || max_y <= min_y {
        None
    } else {
        Some((min_x, min_y, max_x - min_x, max_y - min_y))
    }
}

#[cfg(test)]
mod camera_ray_tests {
    use super::*;
    use crate::core::camera::{CameraProjection, OrbitCamera};

    #[test]
    fn orthographic_rays_have_distinct_origins_and_parallel_unit_directions() {
        let mut camera = OrbitCamera::default();
        camera.set_projection(CameraProjection::Orthographic);
        let inv_view_proj = camera.view_projection_matrix(4.0 / 3.0).inverse();
        let center = ray_from_viewport_px(Vec2::new(400.0, 300.0), [800, 600], inv_view_proj)
            .expect("center ray");
        let edge = ray_from_viewport_px(Vec2::new(750.0, 300.0), [800, 600], inv_view_proj)
            .expect("edge ray");

        assert!((center.0 - edge.0).length() > 0.1);
        assert!((center.1 - edge.1).length() < 1e-5);
        assert!(center.1.is_finite() && edge.1.is_finite());
        assert!((center.1.length() - 1.0).abs() < 1e-5);
        assert!((edge.1.length() - 1.0).abs() < 1e-5);
    }
}
