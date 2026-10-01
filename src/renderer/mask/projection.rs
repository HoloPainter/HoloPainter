use glam::{Mat4, Vec2, Vec3};

use crate::core::{
    document::MeshData,
    geometry::RectU32,
    mask::{
        ViewportPolygonProjectionMaskSource, ViewportProjection, ViewportRectProjectionMaskSource,
    },
    math::gl_to_wgpu_depth,
    stroke::SurfaceProjectionId,
};

use super::shape::point_in_polygon_even_odd;

const BASE_DEPTH_EPSILON: f32 = 0.0005;
const MAX_DEPTH_EPSILON: f32 = 0.02;
const DEPTH_SLOPE_PIXEL_SCALE: f32 = 0.75;

pub(crate) struct ViewportProjectionDepth {
    viewport: [u32; 2],
    view_proj: Mat4,
    depth: Vec<f32>,
}

pub(crate) struct ProjectionMaskRect {
    pub(crate) rect: RectU32,
    pub(crate) r8: Vec<u8>,
}

pub(crate) fn build_viewport_projection_depth(
    mesh: &MeshData,
    rect: &ViewportRectProjectionMaskSource,
) -> Option<ViewportProjectionDepth> {
    let projection = primary_projection(&rect.projections)?;
    Some(build_projection_depth(
        mesh,
        rect.viewport_size,
        projection.view_proj,
    ))
}

pub(crate) fn build_viewport_polygon_projection_depth(
    mesh: &MeshData,
    polygon: &ViewportPolygonProjectionMaskSource,
) -> Option<ViewportProjectionDepth> {
    let projection = primary_projection(&polygon.projections)?;
    Some(build_projection_depth(
        mesh,
        polygon.viewport_size,
        projection.view_proj,
    ))
}

fn primary_projection(projections: &[ViewportProjection]) -> Option<&ViewportProjection> {
    projections
        .iter()
        .find(|projection| projection.id == SurfaceProjectionId::Primary)
        .or_else(|| projections.first())
}

fn build_projection_depth(
    mesh: &MeshData,
    viewport_size: [u32; 2],
    view_proj: Mat4,
) -> ViewportProjectionDepth {
    let viewport = [viewport_size[0].max(1), viewport_size[1].max(1)];
    let view_proj = gl_to_wgpu_depth() * view_proj;
    let depth = rasterize_viewport_depth(mesh, view_proj, viewport);
    ViewportProjectionDepth {
        viewport,
        view_proj,
        depth,
    }
}

pub(crate) fn viewport_rect_projection_mask_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    rect: &ViewportRectProjectionMaskSource,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Vec<u8> {
    projection_mask_r8_with_depth(
        mesh,
        material_index,
        ProjectionRegion::Rectangle(rect),
        texture_size,
        depth,
    )
}

pub(crate) fn viewport_polygon_projection_mask_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    polygon: &ViewportPolygonProjectionMaskSource,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Vec<u8> {
    projection_mask_r8_with_depth(
        mesh,
        material_index,
        ProjectionRegion::Polygon(polygon),
        texture_size,
        depth,
    )
}

pub(crate) fn viewport_rect_projection_mask_rect_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    rect: &ViewportRectProjectionMaskSource,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Option<ProjectionMaskRect> {
    projection_mask_rect_r8_with_depth(
        mesh,
        material_index,
        ProjectionRegion::Rectangle(rect),
        texture_size,
        depth,
    )
}

pub(crate) fn viewport_polygon_projection_mask_rect_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    polygon: &ViewportPolygonProjectionMaskSource,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Option<ProjectionMaskRect> {
    projection_mask_rect_r8_with_depth(
        mesh,
        material_index,
        ProjectionRegion::Polygon(polygon),
        texture_size,
        depth,
    )
}

fn projection_mask_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    region: ProjectionRegion<'_>,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Vec<u8> {
    let width = texture_size[0].max(1) as usize;
    let height = texture_size[1].max(1) as usize;
    let mut mask = vec![0; width * height];
    rasterize_viewport_projection_for_material(
        &mut mask,
        width,
        height,
        mesh,
        material_index,
        region,
        depth,
    );
    mask
}

fn projection_mask_rect_r8_with_depth(
    mesh: &MeshData,
    material_index: usize,
    region: ProjectionRegion<'_>,
    texture_size: [u32; 2],
    depth: &ViewportProjectionDepth,
) -> Option<ProjectionMaskRect> {
    let width = texture_size[0].max(1) as usize;
    let height = texture_size[1].max(1) as usize;
    let mut mask = vec![0; width * height];
    rasterize_viewport_projection_for_material(
        &mut mask,
        width,
        height,
        mesh,
        material_index,
        region,
        depth,
    );
    crop_nonzero_mask(mask, texture_size)
}

#[derive(Clone, Copy)]
enum ProjectionRegion<'a> {
    Rectangle(&'a ViewportRectProjectionMaskSource),
    Polygon(&'a ViewportPolygonProjectionMaskSource),
}

impl ProjectionRegion<'_> {
    fn contains(self, point: Vec2) -> bool {
        match self {
            Self::Rectangle(rect) => {
                let min = rect.min_px.min(rect.max_px);
                let max = rect.min_px.max(rect.max_px);
                point.x >= min.x && point.y >= min.y && point.x <= max.x && point.y <= max.y
            }
            Self::Polygon(polygon) => point_in_polygon_even_odd(point, &polygon.points_px),
        }
    }
}

fn rasterize_viewport_projection_for_material(
    mask: &mut [u8],
    width: usize,
    height: usize,
    mesh: &MeshData,
    material_index: usize,
    region: ProjectionRegion<'_>,
    depth: &ViewportProjectionDepth,
) {
    for sm in mesh
        .sub_meshes
        .iter()
        .filter(|sm| sm.material_index == material_index)
    {
        let start = sm.start_index as usize / 3;
        let count = sm.index_count as usize / 3;
        for tri in mesh.indices.iter().skip(start).take(count) {
            rasterize_uv_projected_triangle(mask, width, height, mesh, *tri, region, depth);
        }
    }
}

fn crop_nonzero_mask(mask: Vec<u8>, texture_size: [u32; 2]) -> Option<ProjectionMaskRect> {
    let width = texture_size[0].max(1) as usize;
    let height = texture_size[1].max(1) as usize;
    debug_assert_eq!(mask.len(), width * height);

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] == 0 {
                continue;
            }
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + 1);
            max_y = max_y.max(y + 1);
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return None;
    }

    let rect_width = max_x - min_x;
    let rect_height = max_y - min_y;
    let mut r8 = vec![0; rect_width * rect_height];
    for row in 0..rect_height {
        let src_start = (min_y + row) * width + min_x;
        let dst_start = row * rect_width;
        r8[dst_start..dst_start + rect_width]
            .copy_from_slice(&mask[src_start..src_start + rect_width]);
    }
    Some(ProjectionMaskRect {
        rect: RectU32 {
            origin: [min_x as u32, min_y as u32],
            size: [rect_width as u32, rect_height as u32],
        },
        r8,
    })
}

fn rasterize_viewport_depth(mesh: &MeshData, view_proj: Mat4, viewport: [u32; 2]) -> Vec<f32> {
    let width = viewport[0] as usize;
    let height = viewport[1] as usize;
    let mut depth = vec![1.0; width * height];
    for tri in mesh.indices.iter() {
        let p = (*tri).map(|index| mesh.positions[index as usize]);
        let Some(sp) = project_triangle(view_proj, p, viewport) else {
            continue;
        };
        let min_x = sp
            .iter()
            .map(|p| p.xy.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = sp
            .iter()
            .map(|p| p.xy.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(width as f32) as usize;
        let min_y = sp
            .iter()
            .map(|p| p.xy.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = sp
            .iter()
            .map(|p| p.xy.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(height as f32) as usize;
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let Some(b) = barycentric(p, sp[0].xy, sp[1].xy, sp[2].xy) else {
                    continue;
                };
                let z = sp[0].z * b.x + sp[1].z * b.y + sp[2].z * b.z;
                let ix = y * width + x;
                if z >= 0.0 && z <= depth[ix] {
                    depth[ix] = z;
                }
            }
        }
    }
    depth
}

fn rasterize_uv_projected_triangle(
    mask: &mut [u8],
    width: usize,
    height: usize,
    mesh: &MeshData,
    tri: [u32; 3],
    region: ProjectionRegion<'_>,
    depth: &ViewportProjectionDepth,
) {
    let uv = tri.map(|index| mesh.uvs[index as usize]);
    let pos = tri.map(|index| mesh.positions[index as usize]);
    let depth_epsilon = project_triangle(depth.view_proj, pos, depth.viewport)
        .map(projected_triangle_depth_epsilon)
        .unwrap_or(BASE_DEPTH_EPSILON);
    let min_x = uv
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min)
        .clamp(0.0, 1.0);
    let max_x = uv
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .clamp(0.0, 1.0);
    let min_y = uv
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .clamp(0.0, 1.0);
    let max_y = uv
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .clamp(0.0, 1.0);
    let x0 = (min_x * width as f32).floor().max(0.0) as usize;
    let x1 = (max_x * width as f32).ceil().min(width as f32) as usize;
    let y0 = (min_y * height as f32).floor().max(0.0) as usize;
    let y1 = (max_y * height as f32).ceil().min(height as f32) as usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let sample_uv = Vec2::new(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            );
            let Some(b) = barycentric(sample_uv, uv[0], uv[1], uv[2]) else {
                continue;
            };
            let world = pos[0] * b.x + pos[1] * b.y + pos[2] * b.z;
            let Some(projected) = project_point(depth.view_proj, world, depth.viewport) else {
                continue;
            };
            if !region.contains(projected.xy) {
                continue;
            }
            let depth_x = projected
                .xy
                .x
                .floor()
                .clamp(0.0, depth.viewport[0] as f32 - 1.0) as usize;
            let depth_y = projected
                .xy
                .y
                .floor()
                .clamp(0.0, depth.viewport[1] as f32 - 1.0) as usize;
            let scene_depth = depth.depth[depth_y * depth.viewport[0] as usize + depth_x];
            if projected_depth_visible(scene_depth, projected.z, depth_epsilon) {
                mask[y * width + x] = 255;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ProjectedPoint {
    xy: Vec2,
    z: f32,
}

fn project_triangle(
    view_proj: Mat4,
    tri: [Vec3; 3],
    viewport: [u32; 2],
) -> Option<[ProjectedPoint; 3]> {
    Some([
        project_point(view_proj, tri[0], viewport)?,
        project_point(view_proj, tri[1], viewport)?,
        project_point(view_proj, tri[2], viewport)?,
    ])
}

fn project_point(view_proj: Mat4, world: Vec3, viewport: [u32; 2]) -> Option<ProjectedPoint> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.z < 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some(ProjectedPoint {
        xy: Vec2::new(
            (ndc.x * 0.5 + 0.5) * viewport[0] as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * viewport[1] as f32,
        ),
        z: ndc.z,
    })
}

fn projected_triangle_depth_epsilon(projected: [ProjectedPoint; 3]) -> f32 {
    let edge_1 = projected[1].xy - projected[0].xy;
    let edge_2 = projected[2].xy - projected[0].xy;
    let depth_1 = projected[1].z - projected[0].z;
    let depth_2 = projected[2].z - projected[0].z;
    let det = edge_1.x * edge_2.y - edge_1.y * edge_2.x;
    if det.abs() <= 1e-6 {
        return MAX_DEPTH_EPSILON;
    }

    let depth_gradient = Vec2::new(
        (depth_1 * edge_2.y - edge_1.y * depth_2) / det,
        (edge_1.x * depth_2 - depth_1 * edge_2.x) / det,
    );
    let epsilon = BASE_DEPTH_EPSILON
        + DEPTH_SLOPE_PIXEL_SCALE * (depth_gradient.x.abs() + depth_gradient.y.abs());
    epsilon.clamp(BASE_DEPTH_EPSILON, MAX_DEPTH_EPSILON)
}

fn projected_depth_visible(scene_depth: f32, projected_depth: f32, depth_epsilon: f32) -> bool {
    projected_depth <= scene_depth + depth_epsilon
}

fn barycentric(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> Option<Vec3> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() <= 1e-6 {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let u = 1.0 - v - w;
    (u >= -1e-6 && v >= -1e-6 && w >= -1e-6).then_some(Vec3::new(u, v, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_depth_visibility_uses_one_sided_occlusion() {
        assert!(projected_depth_visible(0.50, 0.45, BASE_DEPTH_EPSILON));
        assert!(projected_depth_visible(0.50, 0.5004, BASE_DEPTH_EPSILON));
        assert!(!projected_depth_visible(0.50, 0.51, BASE_DEPTH_EPSILON));
    }

    #[test]
    fn projected_triangle_depth_epsilon_grows_with_screen_depth_slope() {
        let steep = projected_triangle_depth_epsilon([
            ProjectedPoint {
                xy: Vec2::new(0.0, 0.0),
                z: 0.40,
            },
            ProjectedPoint {
                xy: Vec2::new(1.0, 0.0),
                z: 0.42,
            },
            ProjectedPoint {
                xy: Vec2::new(0.0, 10.0),
                z: 0.40,
            },
        ]);
        assert!(steep > BASE_DEPTH_EPSILON, "epsilon={steep}");
        assert!(steep <= MAX_DEPTH_EPSILON, "epsilon={steep}");

        let degenerate = projected_triangle_depth_epsilon([
            ProjectedPoint {
                xy: Vec2::new(1.0, 1.0),
                z: 0.40,
            },
            ProjectedPoint {
                xy: Vec2::new(1.0, 1.0),
                z: 0.41,
            },
            ProjectedPoint {
                xy: Vec2::new(1.0, 1.0),
                z: 0.42,
            },
        ]);
        assert_eq!(degenerate, MAX_DEPTH_EPSILON);
    }
}
