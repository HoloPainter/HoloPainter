use glam::Vec2;

use crate::core::{
    geometry::RectU32,
    mask::{PolygonMaskSource, RectangleMaskSource},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RasterRect {
    width: u32,
    height: u32,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl RasterRect {
    fn from_rectangle(rect: &RectangleMaskSource, texture_size: [u32; 2]) -> Self {
        let width = texture_size[0].max(1);
        let height = texture_size[1].max(1);
        let min_uv = rect.min_uv.min(rect.max_uv).clamp(Vec2::ZERO, Vec2::ONE);
        let max_uv = rect.min_uv.max(rect.max_uv).clamp(Vec2::ZERO, Vec2::ONE);
        let min_x = ((min_uv.x * width as f32).floor() as u32).min(width);
        let min_y = ((min_uv.y * height as f32).floor() as u32).min(height);
        let max_x = ((max_uv.x * width as f32).ceil() as u32).min(width);
        let max_y = ((max_uv.y * height as f32).ceil() as u32).min(height);

        Self {
            width,
            height,
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }
}

pub(crate) fn rectangle_mask_r8(rect: &RectangleMaskSource, texture_size: [u32; 2]) -> Vec<u8> {
    let bounds = RasterRect::from_rectangle(rect, texture_size);
    let mut r8 = vec![0; bounds.width as usize * bounds.height as usize];
    for y in bounds.min_y..bounds.max_y {
        for x in bounds.min_x..bounds.max_x {
            r8[(y * bounds.width + x) as usize] = 255;
        }
    }
    r8
}

pub(crate) fn rectangle_mask_rect_r8(
    rect: &RectangleMaskSource,
    texture_size: [u32; 2],
) -> Option<(RectU32, Vec<u8>)> {
    let bounds = RasterRect::from_rectangle(rect, texture_size);
    let width = bounds.max_x.saturating_sub(bounds.min_x);
    let height = bounds.max_y.saturating_sub(bounds.min_y);
    if width == 0 || height == 0 {
        return None;
    }
    let rect = RectU32 {
        origin: [bounds.min_x, bounds.min_y],
        size: [width, height],
    };
    Some((rect, vec![255; width as usize * height as usize]))
}

pub(crate) fn polygon_mask_r8(polygon: &PolygonMaskSource, texture_size: [u32; 2]) -> Vec<u8> {
    let texture_size = [texture_size[0].max(1), texture_size[1].max(1)];
    let points_px = uv_points_to_pixels(&polygon.points_uv, texture_size);
    let mut r8 = vec![0; texture_size[0] as usize * texture_size[1] as usize];
    if let Some((rect, cropped)) = rasterize_polygon_rect_r8(&points_px, texture_size) {
        copy_rect_into_full(&mut r8, texture_size, rect, &cropped);
    }
    r8
}

pub(crate) fn polygon_mask_rect_r8(
    polygon: &PolygonMaskSource,
    texture_size: [u32; 2],
) -> Option<(RectU32, Vec<u8>)> {
    let texture_size = [texture_size[0].max(1), texture_size[1].max(1)];
    let points_px = uv_points_to_pixels(&polygon.points_uv, texture_size);
    rasterize_polygon_rect_r8(&points_px, texture_size)
}

#[cfg(test)]
pub(crate) fn rasterize_polygon_r8(points_px: &[Vec2], canvas_size: [u32; 2]) -> Vec<u8> {
    let canvas_size = [canvas_size[0].max(1), canvas_size[1].max(1)];
    let mut r8 = vec![0; canvas_size[0] as usize * canvas_size[1] as usize];
    if let Some((rect, cropped)) = rasterize_polygon_rect_r8(points_px, canvas_size) {
        copy_rect_into_full(&mut r8, canvas_size, rect, &cropped);
    }
    r8
}

pub(crate) fn point_in_polygon_even_odd(point: Vec2, points: &[Vec2]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut boundary_hits = 0;
    let mut previous = points[points.len() - 1];
    for &current in points {
        if point_on_segment(point, previous, current) {
            boundary_hits += 1;
        }
        previous = current;
    }
    if boundary_hits == 1 {
        return true;
    }

    let mut inside = false;
    let mut previous = points[points.len() - 1];
    for &current in points {
        let crosses_y = (current.y > point.y) != (previous.y > point.y);
        if crosses_y {
            let denominator = previous.y - current.y;
            let intersection_x = if denominator.abs() <= f32::EPSILON {
                current.x
            } else {
                current.x + (point.y - current.y) * (previous.x - current.x) / denominator
            };
            if point.x < intersection_x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn point_on_segment(point: Vec2, start: Vec2, end: Vec2) -> bool {
    let edge = end - start;
    let to_point = point - start;
    let edge_len_squared = edge.length_squared();
    if edge_len_squared <= f32::EPSILON {
        return point.distance_squared(start) <= f32::EPSILON;
    }
    let cross = edge.perp_dot(to_point).abs();
    let tolerance = 1.0e-5 * edge.length().max(1.0);
    if cross > tolerance {
        return false;
    }
    let projection = to_point.dot(edge);
    projection >= -tolerance && projection <= edge_len_squared + tolerance
}

pub(crate) fn rasterize_polygon_rect_r8(
    points_px: &[Vec2],
    canvas_size: [u32; 2],
) -> Option<(RectU32, Vec<u8>)> {
    if points_px.len() < 3 {
        return None;
    }
    let mut points = Vec::with_capacity(points_px.len());
    for point in points_px.iter().copied() {
        if points
            .last()
            .is_none_or(|previous: &Vec2| previous.distance_squared(point) > f32::EPSILON)
        {
            points.push(point);
        }
    }
    if points.len() > 1 && points[0].distance_squared(points[points.len() - 1]) <= f32::EPSILON {
        points.pop();
    }
    if points.len() < 3 {
        return None;
    }
    let width = canvas_size[0].max(1);
    let height = canvas_size[1].max(1);
    let min = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    let min_x = min.x.floor().clamp(0.0, width as f32) as u32;
    let min_y = min.y.floor().clamp(0.0, height as f32) as u32;
    let max_x = max.x.ceil().clamp(0.0, width as f32) as u32;
    let max_y = max.y.ceil().clamp(0.0, height as f32) as u32;
    let rect_width = max_x.saturating_sub(min_x);
    let rect_height = max_y.saturating_sub(min_y);
    if rect_width == 0 || rect_height == 0 {
        return None;
    }

    let rect = RectU32 {
        origin: [min_x, min_y],
        size: [rect_width, rect_height],
    };
    let mut r8 = vec![0; rect_width as usize * rect_height as usize];
    let mut crossings = Vec::with_capacity(points.len());
    let mut boundary_hits = vec![0u16; rect_width as usize];
    let mut has_coverage = false;
    for local_y in 0..rect_height {
        let sample_y = min_y as f32 + local_y as f32 + 0.5;
        crossings.clear();
        boundary_hits.fill(0);

        let mut previous = points[points.len() - 1];
        for &current in &points {
            if (current.y > sample_y) != (previous.y > sample_y) {
                let intersection_x = current.x
                    + (sample_y - current.y) * (previous.x - current.x) / (previous.y - current.y);
                crossings.push(intersection_x);
            }

            record_scanline_boundary_hits(&mut boundary_hits, min_x, sample_y, previous, current);
            previous = current;
        }

        crossings.sort_unstable_by(f32::total_cmp);
        let row_start = (local_y * rect_width) as usize;
        for pair in crossings.chunks_exact(2) {
            let start = (pair[0] - 0.5).ceil().clamp(min_x as f32, max_x as f32) as u32;
            let end = (pair[1] - 0.5).ceil().clamp(min_x as f32, max_x as f32) as u32;
            if start >= end {
                continue;
            }
            let local_start = start.saturating_sub(min_x) as usize;
            let local_end = end.saturating_sub(min_x) as usize;
            r8[row_start + local_start..row_start + local_end].fill(255);
            has_coverage = true;
        }

        for (local_x, hits) in boundary_hits.iter().copied().enumerate() {
            if hits == 1 {
                r8[row_start + local_x] = 255;
                has_coverage = true;
            }
        }
    }
    has_coverage.then_some((rect, r8))
}

fn record_scanline_boundary_hits(
    boundary_hits: &mut [u16],
    min_x: u32,
    sample_y: f32,
    start: Vec2,
    end: Vec2,
) {
    let edge = end - start;
    let edge_len = edge.length();
    let tolerance = 1.0e-5 * edge_len.max(1.0);
    if edge_len <= f32::EPSILON
        || sample_y < start.y.min(end.y) - tolerance
        || sample_y > start.y.max(end.y) + tolerance
    {
        return;
    }

    let (candidate_min, candidate_max) = if edge.y.abs() <= f32::EPSILON {
        if (sample_y - start.y).abs() > tolerance {
            return;
        }
        (
            start.x.min(end.x) - tolerance,
            start.x.max(end.x) + tolerance,
        )
    } else {
        let intersection_x = start.x + (sample_y - start.y) * edge.x / edge.y;
        let x_tolerance = tolerance / edge.y.abs();
        (intersection_x - x_tolerance, intersection_x + x_tolerance)
    };

    let first = (candidate_min - 0.5).ceil() as i64;
    let last = (candidate_max - 0.5).floor() as i64;
    let row_min = min_x as i64;
    let row_max = row_min + boundary_hits.len() as i64 - 1;
    for x in first.max(row_min)..=last.min(row_max) {
        let sample = Vec2::new(x as f32 + 0.5, sample_y);
        if point_on_segment(sample, start, end) {
            let index = (x - row_min) as usize;
            boundary_hits[index] = boundary_hits[index].saturating_add(1);
        }
    }
}

fn uv_points_to_pixels(points_uv: &[Vec2], texture_size: [u32; 2]) -> Vec<Vec2> {
    let scale = Vec2::new(texture_size[0] as f32, texture_size[1] as f32);
    points_uv
        .iter()
        .copied()
        .map(|uv| uv.clamp(Vec2::ZERO, Vec2::ONE) * scale)
        .collect()
}

fn copy_rect_into_full(full: &mut [u8], texture_size: [u32; 2], rect: RectU32, cropped: &[u8]) {
    let full_width = texture_size[0] as usize;
    let rect_width = rect.size[0] as usize;
    for row in 0..rect.size[1] as usize {
        let src_start = row * rect_width;
        let dst_start = (rect.origin[1] as usize + row) * full_width + rect.origin[0] as usize;
        full[dst_start..dst_start + rect_width]
            .copy_from_slice(&cropped[src_start..src_start + rect_width]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_mask_fills_triangle_pixels() {
        let polygon = PolygonMaskSource {
            points_uv: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 1.0),
            ],
            material_index: Some(0),
        };
        let mask = polygon_mask_r8(&polygon, [4, 4]);
        assert_eq!(mask[0], 255);
        assert_eq!(mask[3], 255);
        assert_eq!(mask[15], 0);
    }

    #[test]
    fn polygon_winding_does_not_change_mask() {
        let points = vec![
            Vec2::new(0.1, 0.1),
            Vec2::new(0.9, 0.1),
            Vec2::new(0.5, 0.9),
        ];
        let forward = polygon_mask_r8(
            &PolygonMaskSource {
                points_uv: points.clone(),
                material_index: None,
            },
            [16, 16],
        );
        let reverse = polygon_mask_r8(
            &PolygonMaskSource {
                points_uv: points.into_iter().rev().collect(),
                material_index: None,
            },
            [16, 16],
        );
        assert_eq!(forward, reverse);
    }

    #[test]
    fn self_intersection_uses_even_odd_rule() {
        let bow_tie = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
            Vec2::new(4.0, 0.0),
        ];
        assert!(point_in_polygon_even_odd(Vec2::new(1.0, 1.0), &bow_tie));
        assert!(!point_in_polygon_even_odd(Vec2::new(2.0, 2.0), &bow_tie));
    }

    #[test]
    fn scanline_raster_matches_point_sampling_reference() {
        let polygons = [
            vec![
                Vec2::new(1.25, 1.5),
                Vec2::new(13.75, 2.25),
                Vec2::new(7.5, 14.25),
            ],
            vec![
                Vec2::new(1.0, 1.0),
                Vec2::new(14.0, 1.0),
                Vec2::new(14.0, 14.0),
                Vec2::new(8.0, 7.0),
                Vec2::new(1.0, 14.0),
            ],
            vec![
                Vec2::new(1.0, 1.0),
                Vec2::new(14.0, 14.0),
                Vec2::new(1.0, 14.0),
                Vec2::new(14.0, 1.0),
            ],
            vec![
                Vec2::new(1.0, 1.0),
                Vec2::new(14.0, 1.0),
                Vec2::new(14.0, 1.0),
                Vec2::new(7.5, 14.0),
                Vec2::new(1.0, 1.0),
            ],
        ];

        for points in polygons {
            let actual = rasterize_polygon_r8(&points, [16, 16]);
            let expected = point_sampled_polygon_r8(&points, [16, 16]);
            assert_eq!(actual, expected, "points={points:?}");
        }
    }

    fn point_sampled_polygon_r8(points: &[Vec2], size: [u32; 2]) -> Vec<u8> {
        let mut mask = vec![0; size[0] as usize * size[1] as usize];
        for y in 0..size[1] {
            for x in 0..size[0] {
                let sample = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                if point_in_polygon_even_odd(sample, points) {
                    mask[(y * size[0] + x) as usize] = 255;
                }
            }
        }
        mask
    }
}
