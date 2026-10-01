use crate::core::{geometry::RectU32, mask::MeshGeometryMaskSource};

pub(crate) fn mesh_geometry_mask_rgba8(
    mesh: &MeshGeometryMaskSource,
    material_index: usize,
    texture_size: [u32; 2],
) -> Vec<u8> {
    let width = texture_size[0].max(1) as usize;
    let height = texture_size[1].max(1) as usize;
    let mut mask = vec![0_u8; width * height * 4];
    for tri in mesh
        .triangles
        .iter()
        .filter(|tri| tri.mesh_id == mesh.mesh_id && tri.material_index == material_index)
    {
        rasterize_uv_triangle_rgba8(&mut mask, width, height, tri.uv);
    }
    mask
}

fn rasterize_uv_triangle_rgba8(mask: &mut [u8], width: usize, height: usize, uv: [glam::Vec2; 3]) {
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
            let p = glam::Vec2::new(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            );
            if point_in_triangle(p, uv[0], uv[1], uv[2]) {
                let offset = (y * width + x) * 4;
                mask[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
}

pub(crate) fn triangle_geometry_mask_rect_r8(
    triangles: &[[glam::Vec2; 3]],
    texture_size: [u32; 2],
) -> Option<(RectU32, Vec<u8>)> {
    let width = texture_size[0].max(1) as usize;
    let height = texture_size[1].max(1) as usize;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut saw_point = false;
    for triangle in triangles {
        for point in triangle {
            if !(point.x.is_finite() && point.y.is_finite()) {
                continue;
            }
            saw_point = true;
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
    }
    if !saw_point {
        return None;
    }

    let min_x = min_x.clamp(0.0, 1.0);
    let min_y = min_y.clamp(0.0, 1.0);
    let max_x = max_x.clamp(0.0, 1.0);
    let max_y = max_y.clamp(0.0, 1.0);
    let x0 = (min_x * width as f32).floor().max(0.0) as usize;
    let y0 = (min_y * height as f32).floor().max(0.0) as usize;
    let x1 = (max_x * width as f32).ceil().min(width as f32) as usize;
    let y1 = (max_y * height as f32).ceil().min(height as f32) as usize;
    if x0 >= x1 || y0 >= y1 {
        return None;
    }

    let rect_width = x1 - x0;
    let rect_height = y1 - y0;
    let mut mask = vec![0_u8; rect_width * rect_height];
    for triangle in triangles {
        rasterize_uv_triangle_r8_rect(
            &mut mask,
            rect_width,
            rect_height,
            x0,
            y0,
            width,
            height,
            *triangle,
        );
    }
    Some((
        RectU32 {
            origin: [x0 as u32, y0 as u32],
            size: [rect_width as u32, rect_height as u32],
        },
        mask,
    ))
}

#[allow(clippy::too_many_arguments)]
fn rasterize_uv_triangle_r8_rect(
    mask: &mut [u8],
    rect_width: usize,
    rect_height: usize,
    origin_x: usize,
    origin_y: usize,
    texture_width: usize,
    texture_height: usize,
    uv: [glam::Vec2; 3],
) {
    for local_y in 0..rect_height {
        let y = origin_y + local_y;
        for local_x in 0..rect_width {
            let x = origin_x + local_x;
            let p = glam::Vec2::new(
                (x as f32 + 0.5) / texture_width as f32,
                (y as f32 + 0.5) / texture_height as f32,
            );
            if point_in_triangle(p, uv[0], uv[1], uv[2]) {
                mask[local_y * rect_width + local_x] = 255;
            }
        }
    }
}

fn point_in_triangle(p: glam::Vec2, a: glam::Vec2, b: glam::Vec2, c: glam::Vec2) -> bool {
    const EPSILON: f32 = 1e-6;
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let denom = v0.perp_dot(v1);
    if denom.abs() <= EPSILON {
        return false;
    }
    let u = v2.perp_dot(v1) / denom;
    let v = v0.perp_dot(v2) / denom;
    u >= -EPSILON && v >= -EPSILON && u + v <= 1.0 + EPSILON
}
