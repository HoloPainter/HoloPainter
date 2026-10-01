use glam::{Mat4, Vec3};

use crate::core::{
    document::{Document, RaycastScratch},
    math::ray_from_viewport_px,
    stroke::SurfaceDab,
    viewport_visibility::ViewportSceneVisibility,
};

use super::super::command::ViewportInputContext;

pub(crate) fn mirror_x_matrix(plane_x: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(plane_x, 0.0, 0.0))
        * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))
        * Mat4::from_translation(Vec3::new(-plane_x, 0.0, 0.0))
}

pub(crate) fn mirror_x_view(
    primary: ViewportInputContext,
    plane_x: f32,
) -> Option<ViewportInputContext> {
    if !plane_x.is_finite() {
        return None;
    }
    let mirror = mirror_x_matrix(plane_x);
    let view_proj = primary.view_proj * mirror;
    let inv_view_proj = mirror * primary.inv_view_proj;
    let camera_world = mirror.transform_point3(Vec3::from(primary.camera_world));
    if !view_proj.to_cols_array().into_iter().all(f32::is_finite)
        || !inv_view_proj
            .to_cols_array()
            .into_iter()
            .all(f32::is_finite)
        || !camera_world.is_finite()
    {
        return None;
    }
    Some(ViewportInputContext {
        size: primary.size,
        view_proj,
        inv_view_proj,
        camera_world: camera_world.into(),
    })
}

pub(crate) fn mirror_x_dab(
    document: &Document,
    primary: SurfaceDab,
    mirror_view: ViewportInputContext,
    plane_x: f32,
    radius_world: f32,
    preferred_triangle: Option<usize>,
    visibility: &ViewportSceneVisibility,
    scratch: &mut RaycastScratch,
) -> Option<SurfaceDab> {
    let scene_diagonal = document.mesh.scene_diagonal();
    let plane_epsilon = (scene_diagonal * 1e-6).max(1e-6);
    if (primary.world_pos.x - plane_x).abs() <= plane_epsilon {
        return None;
    }

    let (ray_origin, ray_direction) = ray_from_viewport_px(
        primary.screen_px,
        mirror_view.size,
        mirror_view.inv_view_proj,
    )?;
    let hit = document.raycast_visible_with_preferred_triangle(
        ray_origin,
        ray_direction,
        preferred_triangle,
        visibility,
        scratch,
    )?;

    let mirrored_x = 2.0 * plane_x - primary.world_pos.x;
    if !mirrored_x.is_finite() {
        return None;
    }
    let expected_position = Vec3::new(mirrored_x, primary.world_pos.y, primary.world_pos.z);
    let expected_normal = Vec3::new(
        -primary.world_normal.x,
        primary.world_normal.y,
        primary.world_normal.z,
    )
    .normalize_or_zero();
    let position_tolerance = (radius_world * primary.radius_scale * 0.25)
        .max(scene_diagonal * 1e-5)
        .max(1e-6);
    if hit.world_pos.distance(expected_position) > position_tolerance
        || hit.world_normal.dot(expected_normal) < 0.5
    {
        return None;
    }

    let mut mirrored = SurfaceDab::from_hit_with_scales(
        primary.screen_px,
        hit,
        primary.pressure,
        primary.radius_scale,
    );
    let reflected_x = Vec3::new(
        -primary.tangent_x.x,
        primary.tangent_x.y,
        primary.tangent_x.z,
    );
    let reflected_y = Vec3::new(
        -primary.tangent_y.x,
        primary.tangent_y.y,
        primary.tangent_y.z,
    );
    let normal = mirrored.world_normal.normalize_or_zero();
    let tangent_x = (reflected_x - normal * reflected_x.dot(normal)).normalize_or_zero();
    if normal.length_squared() <= f32::EPSILON || tangent_x.length_squared() <= f32::EPSILON {
        return None;
    }
    let mut tangent_y = normal.cross(tangent_x).normalize_or_zero();
    if tangent_y.dot(reflected_y) < 0.0 {
        tangent_y = -tangent_y;
    }
    if tangent_y.length_squared() <= f32::EPSILON {
        return None;
    }
    mirrored.tangent_x = tangent_x;
    mirrored.tangent_y = tangent_y;
    Some(mirrored)
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec3};

    use super::*;

    #[test]
    fn mirror_x_is_its_own_inverse_around_arbitrary_plane() {
        let plane_x = 1.25;
        let mirror = mirror_x_matrix(plane_x);
        assert!(mirror.abs_diff_eq(mirror.inverse(), 1e-6));
        assert!(
            mirror
                .transform_point3(Vec3::new(2.0, 3.0, 4.0))
                .abs_diff_eq(Vec3::new(0.5, 3.0, 4.0), 1e-6)
        );
    }

    #[test]
    fn mirror_view_projects_reflected_point_to_same_clip_position() {
        let primary = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [2.0, 0.0, 0.0],
        };
        let plane_x = 0.75;
        let mirror_view = mirror_x_view(primary, plane_x).expect("mirror view should be finite");
        let point = Vec3::new(0.25, 0.5, 0.0);
        let reflected = mirror_x_matrix(plane_x).transform_point3(point);
        assert!(
            (primary.view_proj * point.extend(1.0))
                .abs_diff_eq(mirror_view.view_proj * reflected.extend(1.0), 1e-6)
        );
    }

    #[test]
    fn mirror_view_rejects_non_finite_results() {
        let primary = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [2.0, 0.0, 0.0],
        };
        assert!(mirror_x_view(primary, f32::NAN).is_none());
        assert!(mirror_x_view(primary, f32::MAX).is_none());
    }
}
