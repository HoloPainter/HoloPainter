use std::{ops::Range, sync::Arc, time::Instant};

use glam::{Mat4, Vec2, Vec3, Vec4};
use slotmap::Key;

use crate::core::{
    damage::{DamageMap, UV_ISLAND_BLEED_RADIUS_PX, rects_touch, union_rect},
    document::{Document, MeshData, SurfaceHit},
    geometry::RectU32,
    surface::PaintSurfaceId,
    transform::TransformHandle,
    viewport_visibility::ViewportSceneVisibility,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecalImageId(pub u64);

pub const DECAL_HANDLE_RADIUS_PX: f32 = 5.0;
pub const DECAL_HANDLE_HIT_RADIUS_PX: f32 = 10.0;
pub const DECAL_ROTATE_HANDLE_OFFSET_PX: f32 = 28.0;
pub const DECAL_RASTER_GUARD_PX: u32 = 1;

const DECAL_CLIP_GUARD_SCALE: f32 = 1.0e-5;
const DECAL_CLIP_GUARD_MIN_WORLD: f32 = 1.0e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalHandleGeometry {
    pub corners: [Vec2; 4],
    pub edge_centers: [Vec2; 4],
    pub center: Vec2,
    pub rotate: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewProjectionDecalTransform {
    pub center_uv: Vec2,
    pub size_px: Vec2,
    pub rotation_radians: f32,
    pub viewport_size: [u32; 2],
}

impl ViewProjectionDecalTransform {
    pub fn initial(image_size: [u32; 2], viewport_size: [u32; 2]) -> Option<Self> {
        if image_size[0] == 0
            || image_size[1] == 0
            || viewport_size[0] == 0
            || viewport_size[1] == 0
        {
            return None;
        }
        let scale = (viewport_size[0] as f32 * 0.5 / image_size[0] as f32)
            .min(viewport_size[1] as f32 * 0.5 / image_size[1] as f32);
        let transform = Self {
            center_uv: Vec2::splat(0.5),
            size_px: Vec2::new(image_size[0] as f32, image_size[1] as f32) * scale,
            rotation_radians: 0.0,
            viewport_size,
        };
        transform.is_valid().then_some(transform)
    }

    pub fn resized(self, viewport_size: [u32; 2]) -> Option<Self> {
        if !self.is_valid() || viewport_size[0] == 0 || viewport_size[1] == 0 {
            return None;
        }
        let old_short = self.viewport_size[0].min(self.viewport_size[1]) as f32;
        let new_short = viewport_size[0].min(viewport_size[1]) as f32;
        let resized = Self {
            size_px: self.size_px * (new_short / old_short),
            viewport_size,
            ..self
        };
        resized.is_valid().then_some(resized)
    }

    pub fn center_px(self) -> Vec2 {
        self.center_uv * Vec2::new(self.viewport_size[0] as f32, self.viewport_size[1] as f32)
    }

    pub fn corners_px(self) -> [Vec2; 4] {
        let center = self.center_px();
        let half = self.size_px * 0.5;
        let (sin, cos) = self.rotation_radians.sin_cos();
        let rotate =
            |point: Vec2| Vec2::new(cos * point.x - sin * point.y, sin * point.x + cos * point.y);
        [
            center + rotate(Vec2::new(-half.x, -half.y)),
            center + rotate(Vec2::new(half.x, -half.y)),
            center + rotate(Vec2::new(half.x, half.y)),
            center + rotate(Vec2::new(-half.x, half.y)),
        ]
    }

    pub fn handle_geometry(self) -> Option<DecalHandleGeometry> {
        if !self.is_valid() {
            return None;
        }
        let corners = self.corners_px();
        let center = self.center_px();
        let edge_centers = [
            (corners[0] + corners[1]) * 0.5,
            (corners[1] + corners[2]) * 0.5,
            (corners[2] + corners[3]) * 0.5,
            (corners[3] + corners[0]) * 0.5,
        ];
        let direction = (edge_centers[0] - center).normalize_or_zero();
        Some(DecalHandleGeometry {
            corners,
            edge_centers,
            center,
            rotate: edge_centers[0] + direction * DECAL_ROTATE_HANDLE_OFFSET_PX,
        })
    }

    pub fn projector_view_projection(self, viewport_view_proj: Mat4) -> Option<Mat4> {
        if !self.is_valid() || !viewport_view_proj.is_finite() {
            return None;
        }
        let viewport = Vec2::new(self.viewport_size[0] as f32, self.viewport_size[1] as f32);
        let center = self.center_px();
        let (sin, cos) = self.rotation_radians.sin_cos();
        let half = self.size_px * 0.5;

        // Convert viewport clip coordinates to pixels, rotate into the image frame,
        // then normalize the image rectangle to projector clip coordinates.
        let ax = cos * viewport.x * 0.5 / half.x;
        let ay = -sin * viewport.y * 0.5 / half.x;
        let aw =
            (cos * (viewport.x * 0.5 - center.x) + sin * (viewport.y * 0.5 - center.y)) / half.x;
        let bx = sin * viewport.x * 0.5 / half.y;
        let by = cos * viewport.y * 0.5 / half.y;
        let bw =
            (sin * (viewport.x * 0.5 - center.x) - cos * (viewport.y * 0.5 - center.y)) / half.y;
        let clip_to_projector = Mat4::from_cols_array(&[
            ax, bx, 0.0, 0.0, ay, by, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, aw, bw, 0.0, 1.0,
        ]);
        let projection = clip_to_projector * viewport_view_proj;
        projection.is_finite().then_some(projection)
    }

    pub fn is_valid(self) -> bool {
        self.center_uv.is_finite()
            && self.size_px.is_finite()
            && self.rotation_radians.is_finite()
            && self.viewport_size[0] > 0
            && self.viewport_size[1] > 0
            && self.size_px.cmpgt(Vec2::ZERO).all()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecalImageAsset {
    pub id: DecalImageId,
    pub file_name: String,
    pub size: [u32; 2],
    pub rgba8: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecalToolOptions {
    pub opacity: f32,
    pub normal_mode: DecalNormalMode,
    pub smooth_angle_degrees: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalNormalMode {
    Face,
    Smooth,
}

impl Default for DecalToolOptions {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            normal_mode: DecalNormalMode::Face,
            smooth_angle_degrees: 60.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalTransform {
    pub center_world: Vec3,
    pub axis_x_world: Vec3,
    pub axis_y_world: Vec3,
    pub normal_world: Vec3,
    pub size_world: Vec2,
    pub projection_depth_world: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecalProjection {
    Surface(DecalTransform),
    ViewProjection {
        projector_view_proj: Mat4,
        viewport_view_proj: Mat4,
        depth_size: [u32; 2],
    },
}

impl DecalProjection {
    pub fn projector_view_projection(self) -> Option<Mat4> {
        match self {
            Self::Surface(transform) => transform.projector_view_projection(),
            Self::ViewProjection {
                projector_view_proj,
                ..
            } => projector_view_proj
                .is_finite()
                .then_some(projector_view_proj),
        }
    }

    pub fn visibility_view_projection(self) -> Option<Mat4> {
        match self {
            Self::Surface(transform) => transform.projector_view_projection(),
            Self::ViewProjection {
                viewport_view_proj, ..
            } => viewport_view_proj.is_finite().then_some(viewport_view_proj),
        }
    }

    pub fn depth_size(self, image_size: [u32; 2]) -> [u32; 2] {
        match self {
            Self::Surface(_) => image_size,
            Self::ViewProjection { depth_size, .. } => [depth_size[0].max(1), depth_size[1].max(1)],
        }
    }

    pub fn is_view_projection(self) -> bool {
        matches!(self, Self::ViewProjection { .. })
    }

    pub fn is_valid(self) -> bool {
        self.projector_view_projection().is_some()
            && self.visibility_view_projection().is_some()
            && match self {
                Self::Surface(transform) => transform.is_valid(),
                Self::ViewProjection { depth_size, .. } => depth_size[0] > 0 && depth_size[1] > 0,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecalDrawBatch {
    pub scissor: RectU32,
    pub index_ranges: Vec<Range<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecalSurfacePlan {
    pub surface: PaintSurfaceId,
    pub footprint_rects: Vec<RectU32>,
    pub damage_rects: Vec<RectU32>,
    pub draw_batches: Vec<DecalDrawBatch>,
}

impl DecalSurfacePlan {
    pub fn draw_index_range_count(&self) -> usize {
        self.draw_batches
            .iter()
            .map(|batch| batch.index_ranges.len())
            .sum()
    }

    pub fn draw_call_count(&self) -> usize {
        self.draw_index_range_count()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecalPlanMetrics {
    pub build_time_us: u64,
    pub candidate_triangle_count: usize,
    pub intersection_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecalApplyPlan {
    pub targets: Vec<DecalSurfacePlan>,
    pub depth_index_ranges: Vec<Range<u32>>,
    pub metrics: DecalPlanMetrics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DecalPreviewGeometry {
    pub(crate) index_ranges: Vec<Range<u32>>,
    pub(crate) screen_scissor: Option<RectU32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DecalTriangleIntersection {
    triangle_index: usize,
    material_index: usize,
    min_uv: Vec2,
    max_uv: Vec2,
}

struct DecalIntersectionResult {
    intersections: Vec<DecalTriangleIntersection>,
    candidate_triangle_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct DecalClipVertex {
    local: Vec3,
    uv: Vec2,
}

impl DecalApplyPlan {
    pub fn build(
        document: &Document,
        surfaces: &[PaintSurfaceId],
        transform: DecalTransform,
    ) -> Option<Self> {
        Self::build_with_visibility(
            document,
            surfaces,
            transform,
            &ViewportSceneVisibility::default(),
        )
    }

    pub fn build_with_visibility(
        document: &Document,
        surfaces: &[PaintSurfaceId],
        transform: DecalTransform,
        scene_visibility: &ViewportSceneVisibility,
    ) -> Option<Self> {
        Self::build_for_projection_with_visibility(
            document,
            surfaces,
            DecalProjection::Surface(transform),
            scene_visibility,
        )
    }

    pub fn build_for_projection(
        document: &Document,
        surfaces: &[PaintSurfaceId],
        projection: DecalProjection,
    ) -> Option<Self> {
        Self::build_for_projection_with_visibility(
            document,
            surfaces,
            projection,
            &ViewportSceneVisibility::default(),
        )
    }

    pub fn build_for_projection_with_visibility(
        document: &Document,
        surfaces: &[PaintSurfaceId],
        projection: DecalProjection,
        scene_visibility: &ViewportSceneVisibility,
    ) -> Option<Self> {
        let started = Instant::now();
        if !projection.is_valid() {
            return None;
        }
        if surfaces.is_empty() {
            return Some(Self {
                metrics: DecalPlanMetrics {
                    build_time_us: elapsed_micros_u64(started),
                    ..DecalPlanMetrics::default()
                },
                ..Self::default()
            });
        }

        let DecalIntersectionResult {
            intersections,
            candidate_triangle_count,
        } = match projection {
            DecalProjection::Surface(transform) => {
                decal_triangle_intersections(&document.mesh, transform, scene_visibility)?
            }
            DecalProjection::ViewProjection {
                projector_view_proj,
                viewport_view_proj,
                ..
            } => projective_triangle_intersections(
                &document.mesh,
                projector_view_proj,
                Some(viewport_view_proj),
                scene_visibility,
            )?,
        };
        let depth_index_ranges = sorted_triangle_indices_to_index_ranges(
            intersections
                .iter()
                .map(|intersection| intersection.triangle_index),
        )?;
        let mut intersections_by_material = vec![Vec::new(); document.materials.len()];
        for intersection in intersections.iter().copied() {
            if let Some(material_intersections) =
                intersections_by_material.get_mut(intersection.material_index)
            {
                material_intersections.push(intersection);
            }
        }
        let damage_guard_px = DECAL_RASTER_GUARD_PX.checked_add(UV_ISLAND_BLEED_RADIUS_PX)?;
        let mut targets = Vec::new();

        for surface in surfaces.iter().copied() {
            let texture_size = document.texture_size_for_surface(surface)?;
            let mut draw_batch_inputs = Vec::new();
            let mut damage_rects = Vec::new();
            let Some(material_intersections) =
                intersections_by_material.get(surface.material_index().as_usize())
            else {
                continue;
            };

            for intersection in material_intersections {
                let Some(footprint_rect) = uv_rect_to_pixel_rect(
                    texture_size,
                    intersection.min_uv,
                    intersection.max_uv,
                    DECAL_RASTER_GUARD_PX,
                ) else {
                    continue;
                };
                let Some(damage_rect) = uv_rect_to_pixel_rect(
                    texture_size,
                    intersection.min_uv,
                    intersection.max_uv,
                    damage_guard_px,
                ) else {
                    continue;
                };
                draw_batch_inputs.push((footprint_rect, intersection.triangle_index));
                damage_rects.push(damage_rect);
            }

            if damage_rects.is_empty() {
                continue;
            }
            let draw_batches = merge_touching_draw_batches(draw_batch_inputs)?;
            let footprint_rects = draw_batches.iter().map(|batch| batch.scissor).collect();
            targets.push(DecalSurfacePlan {
                surface,
                footprint_rects,
                damage_rects: merge_touching_rects(damage_rects),
                draw_batches,
            });
        }

        targets.sort_by_key(|target| {
            (
                target.surface.material_index,
                target.surface.layer_id.data().as_ffi(),
                target.surface.is_mask(),
            )
        });
        Some(Self {
            targets,
            depth_index_ranges,
            metrics: DecalPlanMetrics {
                build_time_us: elapsed_micros_u64(started),
                candidate_triangle_count,
                intersection_count: intersections.len(),
            },
        })
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn surfaces(&self) -> Vec<PaintSurfaceId> {
        self.targets.iter().map(|target| target.surface).collect()
    }

    pub fn damage(&self) -> DamageMap {
        let mut damage = DamageMap::default();
        for target in &self.targets {
            for rect in &target.damage_rects {
                damage.add_rect(target.surface, *rect);
            }
        }
        damage
    }
}

impl DecalPreviewGeometry {
    pub(crate) fn build(
        mesh: &MeshData,
        projection: DecalProjection,
        viewport_view_proj: Mat4,
        viewport_size: [u32; 2],
    ) -> Option<Self> {
        if !projection.is_valid()
            || !viewport_view_proj.is_finite()
            || viewport_size[0] == 0
            || viewport_size[1] == 0
        {
            return None;
        }

        let candidate_triangles = match projection {
            DecalProjection::Surface(transform) => match decal_volume_aabb(transform) {
                Some((bounds_min, bounds_max)) => {
                    exact_aabb_triangle_candidates(mesh, bounds_min, bounds_max)?
                }
                None => (0..mesh.indices.len()).collect(),
            },
            DecalProjection::ViewProjection {
                projector_view_proj,
                ..
            } => match projective_frustum_world_aabb(projector_view_proj) {
                Some((bounds_min, bounds_max)) => {
                    exact_aabb_triangle_candidates(mesh, bounds_min, bounds_max)?
                }
                None => (0..mesh.indices.len()).collect(),
            },
        };
        let mut index_ranges = sorted_triangle_indices_to_index_ranges(candidate_triangles)?;
        let screen_bounds = preview_screen_bounds(projection, viewport_view_proj, viewport_size);
        let screen_scissor = match screen_bounds {
            PreviewScreenBounds::Full => None,
            PreviewScreenBounds::Rect(rect) => Some(rect),
            PreviewScreenBounds::Empty => {
                index_ranges.clear();
                None
            }
        };
        Some(Self {
            index_ranges,
            screen_scissor,
        })
    }
}

impl DecalTransform {
    pub fn from_surface_hit(
        hit: SurfaceHit,
        surface_normal: Vec3,
        horizontal_axis: Vec3,
        fallback_axis: Vec3,
        scene_diagonal: f32,
        image_size: [u32; 2],
    ) -> Option<Self> {
        if !hit.world_pos.is_finite()
            || !surface_normal.is_finite()
            || !horizontal_axis.is_finite()
            || !fallback_axis.is_finite()
            || !scene_diagonal.is_finite()
            || scene_diagonal <= f32::EPSILON
            || image_size[0] == 0
            || image_size[1] == 0
        {
            return None;
        }

        let normal_world = surface_normal.normalize_or_zero();
        if normal_world.length_squared() <= f32::EPSILON {
            return None;
        }
        let axis_x_world = tangent_axis(horizontal_axis, normal_world)
            .or_else(|| tangent_axis(fallback_axis, normal_world))
            .or_else(|| fallback_tangent(normal_world))?;
        let axis_y_world = normal_world.cross(axis_x_world).normalize_or_zero();
        if axis_y_world.length_squared() <= f32::EPSILON {
            return None;
        }

        let long_edge = (scene_diagonal * 0.1).clamp(scene_diagonal * 0.001, scene_diagonal);
        let aspect = image_size[0] as f32 / image_size[1] as f32;
        if !aspect.is_finite() || aspect <= 0.0 {
            return None;
        }
        let size_world = if aspect >= 1.0 {
            Vec2::new(long_edge, long_edge / aspect)
        } else {
            Vec2::new(long_edge * aspect, long_edge)
        };
        let projection_depth_world = (long_edge * 2.0).max(scene_diagonal * 0.01);
        let transform = Self {
            center_world: hit.world_pos,
            axis_x_world,
            axis_y_world,
            normal_world,
            size_world,
            projection_depth_world,
        };
        transform.is_valid().then_some(transform)
    }

    pub fn front_projection_depth_world(self) -> f32 {
        (self.size_world.max_element() * 0.25)
            .min(self.projection_depth_world)
            .max(1.0e-4)
    }

    pub fn corners_world(self) -> [Vec3; 4] {
        let half_x = self.axis_x_world * self.size_world.x * 0.5;
        let half_y = self.axis_y_world * self.size_world.y * 0.5;
        [
            self.center_world - half_x - half_y,
            self.center_world + half_x - half_y,
            self.center_world + half_x + half_y,
            self.center_world - half_x + half_y,
        ]
    }

    pub fn handle_display_transform(
        self,
        surface_point: Vec3,
        face_normal: Vec3,
        visual_epsilon: f32,
    ) -> Option<Self> {
        let face_normal = face_normal.normalize_or_zero();
        if !surface_point.is_finite()
            || face_normal.length_squared() <= f32::EPSILON
            || !visual_epsilon.is_finite()
        {
            return None;
        }
        let min_distance = self
            .corners_world()
            .into_iter()
            .map(|corner| (corner - surface_point).dot(face_normal))
            .fold(f32::INFINITY, f32::min);
        let clearance = (-min_distance).max(0.0) + visual_epsilon.max(0.0);
        let display = Self {
            center_world: self.center_world + face_normal * clearance,
            ..self
        };
        display.is_valid().then_some(display)
    }

    pub fn handle_geometry(
        self,
        view_proj: Mat4,
        viewport_size: [u32; 2],
    ) -> Option<DecalHandleGeometry> {
        let corner_points = self
            .corners_world()
            .map(|world| project_world_to_viewport(view_proj, world, viewport_size))
            .into_iter()
            .collect::<Option<Vec<Vec2>>>()?;
        let corners: [Vec2; 4] = corner_points.try_into().ok()?;
        let center = project_world_to_viewport(view_proj, self.center_world, viewport_size)?;
        let edge_centers = [
            (corners[0] + corners[1]) * 0.5,
            (corners[1] + corners[2]) * 0.5,
            (corners[2] + corners[3]) * 0.5,
            (corners[3] + corners[0]) * 0.5,
        ];
        let rotate_direction = (edge_centers[0] - center).normalize_or_zero();
        if rotate_direction.length_squared() <= f32::EPSILON {
            return None;
        }
        Some(DecalHandleGeometry {
            corners,
            edge_centers,
            center,
            rotate: edge_centers[0] + rotate_direction * DECAL_ROTATE_HANDLE_OFFSET_PX,
        })
    }

    pub fn projector_view_projection(self) -> Option<Mat4> {
        if !self.is_valid() {
            return None;
        }

        let half_size = self.size_world * 0.5;
        let front_depth = self.front_projection_depth_world();
        let front_margin = (self.size_world.max_element() * 0.01)
            .max(self.projection_depth_world * 0.001)
            .max(1.0e-4);
        let eye = self.center_world + self.normal_world * (front_depth + front_margin);
        let view = glam::camera::rh::view::look_at_mat4(eye, self.center_world, self.axis_y_world);
        let projection = glam::camera::rh::proj::opengl::orthographic(
            -half_size.x,
            half_size.x,
            -half_size.y,
            half_size.y,
            front_margin * 0.25,
            front_margin + front_depth + self.projection_depth_world,
        );
        let view_proj = projection * view;
        view_proj
            .to_cols_array()
            .into_iter()
            .all(f32::is_finite)
            .then_some(view_proj)
    }

    pub fn is_valid(self) -> bool {
        self.center_world.is_finite()
            && self.axis_x_world.is_finite()
            && self.axis_y_world.is_finite()
            && self.normal_world.is_finite()
            && self.size_world.is_finite()
            && self.projection_depth_world.is_finite()
            && self.size_world.cmpgt(Vec2::ZERO).all()
            && self.projection_depth_world > 0.0
            && (self.axis_x_world.length_squared() - 1.0).abs() <= 1e-4
            && (self.axis_y_world.length_squared() - 1.0).abs() <= 1e-4
            && (self.normal_world.length_squared() - 1.0).abs() <= 1e-4
            && self.axis_x_world.dot(self.axis_y_world).abs() <= 1e-4
            && self.axis_x_world.dot(self.normal_world).abs() <= 1e-4
            && self.axis_y_world.dot(self.normal_world).abs() <= 1e-4
    }
}

fn elapsed_micros_u64(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn decal_triangle_intersections(
    mesh: &MeshData,
    transform: DecalTransform,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<DecalIntersectionResult> {
    let (bounds_min, bounds_max) = decal_volume_aabb(transform)?;
    let half_size = transform.size_world * 0.5;
    let front_depth = transform.front_projection_depth_world();
    let clip_guard = decal_clip_guard_world(transform);
    let mut intersections = Vec::new();
    let candidate_triangles = mesh
        .triangle_candidates_intersecting_aabb(bounds_min, bounds_max)
        .into_iter()
        .filter(|triangle_index| triangle_visible(mesh, *triangle_index, scene_visibility))
        .collect::<Vec<_>>();
    let candidate_triangle_count = candidate_triangles.len();
    let clip_planes = [
        ClipPlane::MinX(-half_size.x - clip_guard),
        ClipPlane::MaxX(half_size.x + clip_guard),
        ClipPlane::MinY(-half_size.y - clip_guard),
        ClipPlane::MaxY(half_size.y + clip_guard),
        ClipPlane::MinZ(-front_depth - clip_guard),
        ClipPlane::MaxZ(transform.projection_depth_world + clip_guard),
    ];
    let mut polygon = Vec::with_capacity(12);
    let mut clip_scratch = Vec::with_capacity(12);

    for triangle_index in candidate_triangles {
        let triangle = mesh.indices.get(triangle_index)?;
        polygon.clear();
        clip_scratch.clear();
        for vertex_index in triangle {
            let position = *mesh.positions.get(*vertex_index as usize)?;
            let uv = *mesh.uvs.get(*vertex_index as usize)?;
            if !position.is_finite() || !uv.is_finite() {
                return None;
            }
            let delta = position - transform.center_world;
            polygon.push(DecalClipVertex {
                local: Vec3::new(
                    delta.dot(transform.axis_x_world),
                    delta.dot(transform.axis_y_world),
                    -delta.dot(transform.normal_world),
                ),
                uv,
            });
        }

        clip_decal_polygon(&mut polygon, &mut clip_scratch, clip_planes);
        if polygon.is_empty() {
            continue;
        }

        let mut min_uv = Vec2::splat(f32::INFINITY);
        let mut max_uv = Vec2::splat(f32::NEG_INFINITY);
        for vertex in &polygon {
            min_uv = min_uv.min(vertex.uv);
            max_uv = max_uv.max(vertex.uv);
        }
        if !min_uv.is_finite() || !max_uv.is_finite() {
            return None;
        }
        intersections.push(DecalTriangleIntersection {
            triangle_index,
            material_index: mesh.material_index_for_triangle(triangle_index),
            min_uv,
            max_uv,
        });
    }

    debug_assert!(
        intersections
            .windows(2)
            .all(|pair| pair[0].triangle_index < pair[1].triangle_index)
    );
    Some(DecalIntersectionResult {
        intersections,
        candidate_triangle_count,
    })
}

#[derive(Debug, Clone, Copy)]
struct ProjectiveClipVertex {
    projector_clip: Vec4,
    visibility_clip: Vec4,
    uv: Vec2,
}

#[derive(Debug, Clone, Copy)]
enum ProjectiveClipSpace {
    Projector,
    Visibility,
}

impl ProjectiveClipVertex {
    fn clip(self, space: ProjectiveClipSpace) -> Vec4 {
        match space {
            ProjectiveClipSpace::Projector => self.projector_clip,
            ProjectiveClipSpace::Visibility => self.visibility_clip,
        }
    }

    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            projector_clip: self.projector_clip.lerp(other.projector_clip, t),
            visibility_clip: self.visibility_clip.lerp(other.visibility_clip, t),
            uv: self.uv.lerp(other.uv, t),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HomogeneousClipPlane {
    Left,
    Right,
    Bottom,
    Top,
    Near,
    Far,
}

impl HomogeneousClipPlane {
    fn signed_distance(self, clip: Vec4) -> f32 {
        match self {
            Self::Left => clip.x + clip.w,
            Self::Right => clip.w - clip.x,
            Self::Bottom => clip.y + clip.w,
            Self::Top => clip.w - clip.y,
            Self::Near => clip.z + clip.w,
            Self::Far => clip.w - clip.z,
        }
    }
}

fn projective_triangle_intersections(
    mesh: &MeshData,
    projector_view_proj: Mat4,
    visibility_view_proj: Option<Mat4>,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<DecalIntersectionResult> {
    if !projector_view_proj.is_finite()
        || visibility_view_proj.is_some_and(|matrix| !matrix.is_finite())
    {
        return None;
    }
    let candidate_triangles = projective_frustum_world_aabb(projector_view_proj)
        .map(|(bounds_min, bounds_max)| {
            mesh.triangle_candidates_intersecting_aabb(bounds_min, bounds_max)
        })
        .unwrap_or_else(|| (0..mesh.indices.len()).collect())
        .into_iter()
        .filter(|triangle_index| triangle_visible(mesh, *triangle_index, scene_visibility))
        .collect::<Vec<_>>();
    let candidate_triangle_count = candidate_triangles.len();
    let mut intersections = Vec::new();
    let mut polygon = Vec::with_capacity(12);
    let mut clip_scratch = Vec::with_capacity(12);
    for triangle_index in candidate_triangles {
        let triangle = mesh.indices.get(triangle_index)?;
        polygon.clear();
        clip_scratch.clear();
        for vertex_index in triangle {
            let world = *mesh.positions.get(*vertex_index as usize)?;
            let uv = *mesh.uvs.get(*vertex_index as usize)?;
            if !world.is_finite() || !uv.is_finite() {
                return None;
            }
            let world_h = world.extend(1.0);
            let projector_clip = projector_view_proj * world_h;
            let visibility_clip = visibility_view_proj
                .map(|matrix| matrix * world_h)
                .unwrap_or(Vec4::ZERO);
            if !projector_clip.is_finite()
                || visibility_view_proj.is_some() && !visibility_clip.is_finite()
            {
                return None;
            }
            polygon.push(ProjectiveClipVertex {
                projector_clip,
                visibility_clip,
                uv,
            });
        }
        clip_projective_polygon(
            &mut polygon,
            &mut clip_scratch,
            ProjectiveClipSpace::Projector,
        );
        if polygon.is_empty() {
            continue;
        }
        if visibility_view_proj.is_some() {
            clip_projective_polygon(
                &mut polygon,
                &mut clip_scratch,
                ProjectiveClipSpace::Visibility,
            );
            if polygon.is_empty() {
                continue;
            }
        }

        let mut min_uv = Vec2::splat(f32::INFINITY);
        let mut max_uv = Vec2::splat(f32::NEG_INFINITY);
        for vertex in &polygon {
            min_uv = min_uv.min(vertex.uv);
            max_uv = max_uv.max(vertex.uv);
        }
        if !min_uv.is_finite() || !max_uv.is_finite() {
            return None;
        }
        intersections.push(DecalTriangleIntersection {
            triangle_index,
            material_index: mesh.material_index_for_triangle(triangle_index),
            min_uv,
            max_uv,
        });
    }
    Some(DecalIntersectionResult {
        intersections,
        candidate_triangle_count,
    })
}

fn triangle_visible(
    mesh: &MeshData,
    triangle_index: usize,
    scene_visibility: &ViewportSceneVisibility,
) -> bool {
    mesh.triangle_mesh_ids
        .get(triangle_index)
        .copied()
        .is_some_and(|mesh_id| {
            scene_visibility.geometry_visible(
                mesh_id,
                mesh.material_index_for_triangle(triangle_index).into(),
            )
        })
}

fn projective_frustum_world_aabb(view_proj: Mat4) -> Option<(Vec3, Vec3)> {
    let inverse = view_proj.inverse();
    if !inverse.is_finite() {
        return None;
    }

    let mut bounds_min = Vec3::splat(f32::INFINITY);
    let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
    for z in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for x in [-1.0, 1.0] {
                let world_h = inverse * Vec4::new(x, y, z, 1.0);
                if !world_h.is_finite() || world_h.w.abs() <= f32::EPSILON {
                    return None;
                }
                let world = world_h.truncate() / world_h.w;
                if !world.is_finite() {
                    return None;
                }
                bounds_min = bounds_min.min(world);
                bounds_max = bounds_max.max(world);
            }
        }
    }

    if !bounds_min.is_finite() || !bounds_max.is_finite() || !bounds_min.cmple(bounds_max).all() {
        return None;
    }
    let guard = (bounds_max - bounds_min).abs() * DECAL_CLIP_GUARD_SCALE
        + Vec3::splat(DECAL_CLIP_GUARD_MIN_WORLD);
    let guarded_min = bounds_min - guard;
    let guarded_max = bounds_max + guard;
    (guarded_min.is_finite() && guarded_max.is_finite()).then_some((guarded_min, guarded_max))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewScreenBounds {
    Full,
    Rect(RectU32),
    Empty,
}

fn exact_aabb_triangle_candidates(
    mesh: &MeshData,
    bounds_min: Vec3,
    bounds_max: Vec3,
) -> Option<Vec<usize>> {
    let candidates = mesh.triangle_candidates_intersecting_aabb(bounds_min, bounds_max);
    let mut exact = Vec::with_capacity(candidates.len());
    for triangle_index in candidates {
        let triangle = mesh.indices.get(triangle_index)?;
        let [p0, p1, p2] = triangle.map(|index| mesh.positions.get(index as usize).copied());
        let [Some(p0), Some(p1), Some(p2)] = [p0, p1, p2] else {
            return None;
        };
        if !p0.is_finite() || !p1.is_finite() || !p2.is_finite() {
            return None;
        }
        let triangle_min = p0.min(p1).min(p2);
        let triangle_max = p0.max(p1).max(p2);
        if triangle_min.cmple(bounds_max).all() && bounds_min.cmple(triangle_max).all() {
            exact.push(triangle_index);
        }
    }
    Some(exact)
}

fn preview_screen_bounds(
    projection: DecalProjection,
    viewport_view_proj: Mat4,
    viewport_size: [u32; 2],
) -> PreviewScreenBounds {
    let points = match projection {
        DecalProjection::Surface(transform) => {
            let half_x = transform.axis_x_world * transform.size_world.x * 0.5;
            let half_y = transform.axis_y_world * transform.size_world.y * 0.5;
            let front = transform.normal_world * transform.front_projection_depth_world();
            let back = -transform.normal_world * transform.projection_depth_world;
            let mut points = Vec::with_capacity(8);
            for depth in [front, back] {
                for x in [-half_x, half_x] {
                    for y in [-half_y, half_y] {
                        let clip = viewport_view_proj
                            * (transform.center_world + depth + x + y).extend(1.0);
                        if !clip.is_finite() || clip.w <= f32::EPSILON {
                            return PreviewScreenBounds::Full;
                        }
                        points.push(clip.truncate() / clip.w);
                    }
                }
            }
            points
        }
        DecalProjection::ViewProjection {
            projector_view_proj,
            viewport_view_proj,
            ..
        } => {
            let viewport_inverse = viewport_view_proj.inverse();
            if !viewport_inverse.is_finite() {
                return PreviewScreenBounds::Full;
            }
            let clip_to_projector = projector_view_proj * viewport_inverse;
            let projector_to_clip = clip_to_projector.inverse();
            if !projector_to_clip.is_finite() {
                return PreviewScreenBounds::Full;
            }
            let mut points = Vec::with_capacity(4);
            for y in [-1.0, 1.0] {
                for x in [-1.0, 1.0] {
                    let clip = projector_to_clip * Vec4::new(x, y, 0.0, 1.0);
                    if !clip.is_finite() || clip.w.abs() <= f32::EPSILON {
                        return PreviewScreenBounds::Full;
                    }
                    points.push(clip.truncate() / clip.w);
                }
            }
            points
        }
    };
    ndc_points_to_screen_bounds(&points, viewport_size)
}

fn ndc_points_to_screen_bounds(points: &[Vec3], viewport_size: [u32; 2]) -> PreviewScreenBounds {
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let viewport = Vec2::new(viewport_size[0] as f32, viewport_size[1] as f32);
    for point in points {
        if !point.is_finite() {
            return PreviewScreenBounds::Full;
        }
        let pixel = Vec2::new(
            (point.x * 0.5 + 0.5) * viewport.x,
            (0.5 - point.y * 0.5) * viewport.y,
        );
        min = min.min(pixel);
        max = max.max(pixel);
    }
    if !min.is_finite() || !max.is_finite() {
        return PreviewScreenBounds::Full;
    }

    let snap_near_integer = |value: f32| {
        let rounded = value.round();
        if (value - rounded).abs() <= 1.0e-4 {
            rounded
        } else {
            value
        }
    };
    let min = min.map(snap_near_integer);
    let max = max.map(snap_near_integer);
    let guard = 1.0;
    let min_x = (min.x.floor() - guard).max(0.0).min(viewport.x) as u32;
    let min_y = (min.y.floor() - guard).max(0.0).min(viewport.y) as u32;
    let max_x = (max.x.ceil() + guard).max(0.0).min(viewport.x) as u32;
    let max_y = (max.y.ceil() + guard).max(0.0).min(viewport.y) as u32;
    if min_x >= max_x || min_y >= max_y {
        return PreviewScreenBounds::Empty;
    }
    if min_x == 0 && min_y == 0 && max_x == viewport_size[0] && max_y == viewport_size[1] {
        return PreviewScreenBounds::Full;
    }
    PreviewScreenBounds::Rect(RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

fn clip_projective_polygon(
    polygon: &mut Vec<ProjectiveClipVertex>,
    scratch: &mut Vec<ProjectiveClipVertex>,
    space: ProjectiveClipSpace,
) {
    for plane in [
        HomogeneousClipPlane::Left,
        HomogeneousClipPlane::Right,
        HomogeneousClipPlane::Bottom,
        HomogeneousClipPlane::Top,
        HomogeneousClipPlane::Near,
        HomogeneousClipPlane::Far,
    ] {
        clip_projective_polygon_to_plane(polygon, scratch, plane, space);
        std::mem::swap(polygon, scratch);
        if polygon.is_empty() {
            return;
        }
    }
}

fn clip_projective_polygon_to_plane(
    polygon: &[ProjectiveClipVertex],
    output: &mut Vec<ProjectiveClipVertex>,
    plane: HomogeneousClipPlane,
    space: ProjectiveClipSpace,
) {
    output.clear();
    let Some(mut previous) = polygon.last().copied() else {
        return;
    };
    let mut previous_distance = plane.signed_distance(previous.clip(space));
    for current in polygon.iter().copied() {
        let current_distance = plane.signed_distance(current.clip(space));
        let previous_inside = previous_distance >= 0.0;
        let current_inside = current_distance >= 0.0;
        if previous_inside != current_inside {
            let denominator = previous_distance - current_distance;
            if denominator.abs() > f32::EPSILON {
                let t = (previous_distance / denominator).clamp(0.0, 1.0);
                output.push(previous.lerp(current, t));
            }
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_distance = current_distance;
    }
}

#[derive(Debug, Clone, Copy)]
enum ClipPlane {
    MinX(f32),
    MaxX(f32),
    MinY(f32),
    MaxY(f32),
    MinZ(f32),
    MaxZ(f32),
}

impl ClipPlane {
    fn signed_distance(self, point: Vec3) -> f32 {
        match self {
            Self::MinX(min) => point.x - min,
            Self::MaxX(max) => max - point.x,
            Self::MinY(min) => point.y - min,
            Self::MaxY(max) => max - point.y,
            Self::MinZ(min) => point.z - min,
            Self::MaxZ(max) => max - point.z,
        }
    }
}

fn clip_decal_polygon(
    polygon: &mut Vec<DecalClipVertex>,
    scratch: &mut Vec<DecalClipVertex>,
    planes: [ClipPlane; 6],
) {
    for plane in planes {
        clip_polygon_to_plane(polygon, scratch, plane);
        std::mem::swap(polygon, scratch);
        if polygon.is_empty() {
            return;
        }
    }
}

fn clip_polygon_to_plane(
    polygon: &[DecalClipVertex],
    output: &mut Vec<DecalClipVertex>,
    plane: ClipPlane,
) {
    output.clear();
    let Some(mut previous) = polygon.last().copied() else {
        return;
    };
    let mut previous_distance = plane.signed_distance(previous.local);

    for current in polygon.iter().copied() {
        let current_distance = plane.signed_distance(current.local);
        let previous_inside = previous_distance >= 0.0;
        let current_inside = current_distance >= 0.0;
        if previous_inside != current_inside {
            let denominator = previous_distance - current_distance;
            if denominator.abs() > f32::EPSILON {
                let t = (previous_distance / denominator).clamp(0.0, 1.0);
                output.push(DecalClipVertex {
                    local: previous.local.lerp(current.local, t),
                    uv: previous.uv.lerp(current.uv, t),
                });
            }
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_distance = current_distance;
    }
}

fn decal_clip_guard_world(transform: DecalTransform) -> f32 {
    transform
        .size_world
        .max_element()
        .max(transform.projection_depth_world)
        .max(transform.front_projection_depth_world())
        * DECAL_CLIP_GUARD_SCALE
        + DECAL_CLIP_GUARD_MIN_WORLD
}

fn decal_volume_aabb(transform: DecalTransform) -> Option<(Vec3, Vec3)> {
    if !transform.is_valid() {
        return None;
    }
    let half_x = transform.axis_x_world * transform.size_world.x * 0.5;
    let half_y = transform.axis_y_world * transform.size_world.y * 0.5;
    let front = transform.normal_world * transform.front_projection_depth_world();
    let back = -transform.normal_world * transform.projection_depth_world;
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for depth in [front, back] {
        for x in [-half_x, half_x] {
            for y in [-half_y, half_y] {
                let point = transform.center_world + depth + x + y;
                min = min.min(point);
                max = max.max(point);
            }
        }
    }
    let guard = Vec3::splat(decal_clip_guard_world(transform));
    min -= guard;
    max += guard;
    (min.is_finite() && max.is_finite()).then_some((min, max))
}

fn triangle_indices_to_index_ranges(
    triangle_indices: impl IntoIterator<Item = usize>,
) -> Option<Vec<Range<u32>>> {
    let mut triangle_indices = triangle_indices.into_iter().collect::<Vec<_>>();
    triangle_indices.sort_unstable();
    triangle_indices.dedup();
    sorted_triangle_indices_to_index_ranges(triangle_indices)
}

fn sorted_triangle_indices_to_index_ranges(
    triangle_indices: impl IntoIterator<Item = usize>,
) -> Option<Vec<Range<u32>>> {
    let mut triangle_indices = triangle_indices.into_iter();
    let Some(first) = triangle_indices.next() else {
        return Some(Vec::new());
    };
    let mut ranges = Vec::new();
    let mut range_start = first;
    let mut previous = first;
    for triangle_index in triangle_indices {
        debug_assert!(triangle_index > previous);
        if triangle_index <= previous {
            continue;
        }
        if triangle_index == previous.saturating_add(1) {
            previous = triangle_index;
            continue;
        }
        ranges.push(triangle_range(range_start, previous)?);
        range_start = triangle_index;
        previous = triangle_index;
    }
    ranges.push(triangle_range(range_start, previous)?);
    Some(ranges)
}

fn triangle_range(first: usize, last: usize) -> Option<Range<u32>> {
    let start = u32::try_from(first).ok()?.checked_mul(3)?;
    let end = u32::try_from(last.checked_add(1)?).ok()?.checked_mul(3)?;
    Some(start..end)
}

fn uv_rect_to_pixel_rect(
    texture_size: [u32; 2],
    min_uv: Vec2,
    max_uv: Vec2,
    guard_px: u32,
) -> Option<RectU32> {
    if texture_size[0] == 0 || texture_size[1] == 0 || !min_uv.is_finite() || !max_uv.is_finite() {
        return None;
    }
    let min = min_uv.min(max_uv);
    let max = min_uv.max(max_uv);
    let min_x = (min.x * texture_size[0] as f32).floor() as i64 - guard_px as i64;
    let min_y = (min.y * texture_size[1] as f32).floor() as i64 - guard_px as i64;
    let max_x = (max.x * texture_size[0] as f32).ceil() as i64 + guard_px as i64;
    let max_y = (max.y * texture_size[1] as f32).ceil() as i64 + guard_px as i64;
    let min_x = min_x.clamp(0, texture_size[0] as i64) as u32;
    let min_y = min_y.clamp(0, texture_size[1] as i64) as u32;
    let max_x = max_x.clamp(0, texture_size[0] as i64) as u32;
    let max_y = max_y.clamp(0, texture_size[1] as i64) as u32;
    if min_x >= max_x || min_y >= max_y {
        return None;
    }
    Some(RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

#[derive(Debug)]
struct DecalDrawBatchBuilder {
    scissor: RectU32,
    triangle_indices: Vec<usize>,
}

fn merge_touching_draw_batches(inputs: Vec<(RectU32, usize)>) -> Option<Vec<DecalDrawBatch>> {
    let mut batches = inputs
        .into_iter()
        .map(|(scissor, triangle_index)| DecalDrawBatchBuilder {
            scissor,
            triangle_indices: vec![triangle_index],
        })
        .collect::<Vec<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged: Vec<DecalDrawBatchBuilder> = Vec::new();
        'next_batch: for mut batch in batches.drain(..) {
            for existing in &mut merged {
                if rects_touch(existing.scissor, batch.scissor) {
                    existing.scissor = union_rect(existing.scissor, batch.scissor);
                    existing
                        .triangle_indices
                        .append(&mut batch.triangle_indices);
                    changed = true;
                    continue 'next_batch;
                }
            }
            merged.push(batch);
        }
        batches = merged;
    }
    batches.sort_by_key(|batch| {
        (
            batch.scissor.origin[1],
            batch.scissor.origin[0],
            batch.scissor.size[1],
            batch.scissor.size[0],
        )
    });
    batches
        .into_iter()
        .map(|batch| {
            Some(DecalDrawBatch {
                scissor: batch.scissor,
                index_ranges: triangle_indices_to_index_ranges(batch.triangle_indices)?,
            })
        })
        .collect()
}

fn merge_touching_rects(mut rects: Vec<RectU32>) -> Vec<RectU32> {
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged = Vec::new();
        'next_rect: for rect in rects.drain(..) {
            for existing in &mut merged {
                if rects_touch(*existing, rect) {
                    *existing = union_rect(*existing, rect);
                    changed = true;
                    continue 'next_rect;
                }
            }
            merged.push(rect);
        }
        rects = merged;
    }
    rects.sort_by_key(|rect| (rect.origin[1], rect.origin[0], rect.size[1], rect.size[0]));
    rects
}

pub fn hit_test_decal_handle(
    pointer: Vec2,
    geometry: DecalHandleGeometry,
    handle_hit_radius: f32,
) -> Option<TransformHandle> {
    let handles = [
        (TransformHandle::ScaleTopLeft, geometry.corners[0]),
        (TransformHandle::ScaleTopRight, geometry.corners[1]),
        (TransformHandle::ScaleBottomRight, geometry.corners[2]),
        (TransformHandle::ScaleBottomLeft, geometry.corners[3]),
        (TransformHandle::ScaleTop, geometry.edge_centers[0]),
        (TransformHandle::ScaleRight, geometry.edge_centers[1]),
        (TransformHandle::ScaleBottom, geometry.edge_centers[2]),
        (TransformHandle::ScaleLeft, geometry.edge_centers[3]),
        (TransformHandle::Rotate, geometry.rotate),
    ];
    let hit_radius_squared = handle_hit_radius * handle_hit_radius;
    if let Some((handle, _)) = handles
        .into_iter()
        .find(|(_, point)| point.distance_squared(pointer) <= hit_radius_squared)
    {
        return Some(handle);
    }
    point_in_convex_quad(pointer, geometry.corners).then_some(TransformHandle::Move)
}

fn project_world_to_viewport(
    view_proj: Mat4,
    world: Vec3,
    viewport_size: [u32; 2],
) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if !clip.is_finite() || clip.w <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() || !(-1.0..=1.0).contains(&ndc.z) {
        return None;
    }
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0].max(1) as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1].max(1) as f32,
    ))
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

fn tangent_axis(candidate: Vec3, normal: Vec3) -> Option<Vec3> {
    let tangent = candidate - normal * candidate.dot(normal);
    let tangent = tangent.normalize_or_zero();
    (tangent.length_squared() > f32::EPSILON).then_some(tangent)
}

fn fallback_tangent(normal: Vec3) -> Option<Vec3> {
    [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .min_by(|left, right| normal.dot(*left).abs().total_cmp(&normal.dot(*right).abs()))
        .and_then(|axis| tangent_axis(axis, normal))
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3};

    use crate::core::{
        document::{Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh, SurfaceHit},
        geometry::RectU32,
        surface::PaintSurfaceId,
        viewport_visibility::ViewportSceneVisibility,
    };

    use super::{
        DecalApplyPlan, DecalPreviewGeometry, DecalProjection, DecalTransform,
        ViewProjectionDecalTransform, merge_touching_draw_batches,
        triangle_indices_to_index_ranges,
    };

    fn hit(normal: Vec3) -> SurfaceHit {
        SurfaceHit {
            world_pos: Vec3::new(1.0, 2.0, 3.0),
            world_normal: normal,
            uv: Vec2::new(0.5, 0.5),
            uv_edge_distance: 1.0,
            uv_paint_boundary_distance: 1.0,
            triangle_index: 0,
            material_index: 0.into(),
            mesh_id: MeshId(0),
            t: 1.0,
        }
    }

    fn decal_transform(center_world: Vec3, size_world: Vec2) -> DecalTransform {
        DecalTransform {
            center_world,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world,
            projection_depth_world: 1.0,
        }
    }

    fn document_with_mesh(mesh: MeshData, material_count: usize) -> Document {
        Document::new(
            mesh,
            (0..material_count)
                .map(|index| MaterialSpec::new(format!("M{index}"), [1024, 1024]))
                .collect(),
        )
    }

    fn raster_surface(document: &Document, material_index: usize) -> PaintSurfaceId {
        PaintSurfaceId::raster(
            material_index.into(),
            document
                .layer_tree
                .default_raster_layer()
                .expect("test document should have a raster layer"),
        )
    }

    fn mesh(
        positions: Vec<Vec3>,
        uvs: Vec<Vec2>,
        indices: Vec<[u32; 3]>,
        materials: Vec<usize>,
    ) -> MeshData {
        let sub_meshes = materials
            .iter()
            .enumerate()
            .map(|(triangle_index, material_index)| SubMesh {
                mesh_id: MeshId(0),
                start_index: triangle_index as u32 * 3,
                index_count: 3,
                material_index: *material_index,
                material_name: format!("M{material_index}"),
                wireframe_edges: Vec::new(),
            })
            .collect();
        MeshData::new(
            positions.clone(),
            uvs,
            vec![Vec3::Z; positions.len()],
            indices.clone(),
            sub_meshes,
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0); indices.len()],
        )
        .expect("test mesh should be valid")
    }

    #[test]
    fn initial_transform_is_orthonormal_and_centered_on_hit() {
        let surface_hit = hit(Vec3::Z);
        let transform = DecalTransform::from_surface_hit(
            surface_hit,
            Vec3::Z,
            Vec3::X,
            Vec3::Y,
            10.0,
            [512, 256],
        )
        .unwrap();

        assert!(transform.is_valid());
        assert_eq!(transform.center_world, surface_hit.world_pos);
        assert!(transform.axis_x_world.abs_diff_eq(Vec3::X, 1e-6));
        assert!(transform.axis_y_world.abs_diff_eq(Vec3::Y, 1e-6));
        assert!(transform.normal_world.abs_diff_eq(Vec3::Z, 1e-6));
    }

    #[test]
    fn initial_transform_preserves_image_aspect_ratio() {
        let wide = DecalTransform::from_surface_hit(
            hit(Vec3::Z),
            Vec3::Z,
            Vec3::X,
            Vec3::Y,
            20.0,
            [400, 100],
        )
        .unwrap();
        let tall = DecalTransform::from_surface_hit(
            hit(Vec3::Z),
            Vec3::Z,
            Vec3::X,
            Vec3::Y,
            20.0,
            [100, 400],
        )
        .unwrap();

        assert!((wide.size_world.x / wide.size_world.y - 4.0).abs() < 1e-6);
        assert!((tall.size_world.x / tall.size_world.y - 0.25).abs() < 1e-6);
        assert!((wide.size_world.x - 2.0).abs() < 1e-6);
        assert!((tall.size_world.y - 2.0).abs() < 1e-6);
    }

    #[test]
    fn initial_transform_falls_back_when_camera_right_matches_normal() {
        let transform =
            DecalTransform::from_surface_hit(hit(Vec3::X), Vec3::X, Vec3::X, Vec3::Y, 5.0, [1, 1])
                .unwrap();

        assert!(transform.is_valid());
        assert!(transform.axis_x_world.abs_diff_eq(Vec3::Y, 1e-6));
    }

    #[test]
    fn handle_geometry_projects_corners_and_explicit_rotate_handle() {
        let transform = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world: Vec2::ONE,
            projection_depth_world: 1.0,
        };
        let geometry = transform
            .handle_geometry(glam::Mat4::IDENTITY, [100, 100])
            .unwrap();

        assert!(geometry.center.abs_diff_eq(Vec2::splat(50.0), 1e-6));
        assert!(geometry.corners[0].abs_diff_eq(Vec2::new(25.0, 75.0), 1e-6));
        assert!(geometry.corners[2].abs_diff_eq(Vec2::new(75.0, 25.0), 1e-6));
        assert!(geometry.rotate.y > geometry.edge_centers[0].y);
    }

    #[test]
    fn handle_display_transform_clears_the_hit_face_without_moving_projector() {
        let transform = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::new(0.0, 1.0, 1.0).normalize(),
            normal_world: Vec3::new(0.0, -1.0, 1.0).normalize(),
            size_world: Vec2::ONE,
            projection_depth_world: 1.0,
        };

        let display = transform
            .handle_display_transform(Vec3::ZERO, Vec3::Z, 0.01)
            .unwrap();

        assert_eq!(transform.center_world, Vec3::ZERO);
        assert!(display.center_world.z > transform.center_world.z);
        assert!(
            display
                .corners_world()
                .into_iter()
                .all(|corner| corner.z >= 0.01 - 1.0e-6)
        );
    }

    #[test]
    fn decal_hit_test_uses_only_explicit_rotate_handle_outside_quad() {
        let transform = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world: Vec2::ONE,
            projection_depth_world: 1.0,
        };
        let geometry = transform
            .handle_geometry(glam::Mat4::IDENTITY, [100, 100])
            .unwrap();

        assert_eq!(
            super::hit_test_decal_handle(geometry.center, geometry, 8.0),
            Some(crate::core::transform::TransformHandle::Move)
        );
        assert_eq!(
            super::hit_test_decal_handle(geometry.rotate, geometry, 8.0),
            Some(crate::core::transform::TransformHandle::Rotate)
        );
        assert_eq!(
            super::hit_test_decal_handle(Vec2::new(5.0, 5.0), geometry, 8.0),
            None
        );
    }

    #[test]
    fn projector_view_projection_maps_decal_corners_to_clip_bounds() {
        let transform = DecalTransform::from_surface_hit(
            hit(Vec3::Z),
            Vec3::Z,
            Vec3::X,
            Vec3::Y,
            10.0,
            [512, 256],
        )
        .unwrap();
        let view_proj = transform.projector_view_projection().unwrap();
        let corners = transform.corners_world();
        let expected = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ];
        for (world, expected_xy) in corners.into_iter().zip(expected) {
            let clip = view_proj * world.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(ndc.truncate().abs_diff_eq(expected_xy, 1.0e-4));
            assert!((-1.0..=1.0).contains(&ndc.z));
        }
    }

    #[test]
    fn projector_view_projection_includes_surface_in_front_of_decal_plane() {
        let transform = DecalTransform::from_surface_hit(
            hit(Vec3::Z),
            Vec3::Z,
            Vec3::X,
            Vec3::Y,
            10.0,
            [512, 256],
        )
        .unwrap();
        let view_proj = transform.projector_view_projection().unwrap();
        let front_point = transform.center_world
            + transform.normal_world * transform.front_projection_depth_world() * 0.9;
        let clip = view_proj * front_point.extend(1.0);
        let ndc = clip.truncate() / clip.w;

        assert!((-1.0..=1.0).contains(&ndc.z));
    }

    #[test]
    fn apply_plan_clips_a_crossing_triangle_to_local_damage() {
        let document = document_with_mesh(
            mesh(
                vec![
                    Vec3::new(-1.0, -1.0, 0.0),
                    Vec3::new(1.0, -1.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ],
                vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(0.5, 1.0),
                ],
                vec![[0, 1, 2]],
                vec![0],
            ),
            1,
        );
        let surface = raster_surface(&document, 0);
        let plan = DecalApplyPlan::build(
            &document,
            &[surface],
            decal_transform(Vec3::ZERO, Vec2::splat(0.2)),
        )
        .expect("valid decal should produce a plan");

        assert_eq!(plan.depth_index_ranges, vec![0..3]);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.metrics.candidate_triangle_count, 1);
        assert_eq!(plan.metrics.intersection_count, 1);
        let target = &plan.targets[0];
        assert_eq!(target.surface, surface);
        assert_eq!(target.draw_batches.len(), 1);
        assert_eq!(target.draw_batches[0].index_ranges, vec![0..3]);
        assert_eq!(target.footprint_rects.len(), 1);
        assert_eq!(target.draw_batches[0].scissor, target.footprint_rects[0]);
        assert_eq!(target.damage_rects.len(), 1);
        let footprint = target.footprint_rects[0];
        let damage = target.damage_rects[0];
        assert!(footprint.origin[0] > 400 && footprint.origin[1] > 400);
        assert!(footprint.size[0] < 160 && footprint.size[1] < 160);
        assert_eq!(footprint.origin[0].saturating_sub(damage.origin[0]), 4);
        assert_eq!(footprint.origin[1].saturating_sub(damage.origin[1]), 4);
        assert_eq!(damage.size[0].saturating_sub(footprint.size[0]), 8);
        assert_eq!(damage.size[1].saturating_sub(footprint.size[1]), 8);
    }

    #[test]
    fn apply_plan_keeps_separated_uv_islands_as_separate_rects() {
        let document = document_with_mesh(
            mesh(
                vec![
                    Vec3::new(-0.4, -0.4, 0.0),
                    Vec3::new(-0.1, -0.4, 0.0),
                    Vec3::new(-0.4, -0.1, 0.0),
                    Vec3::new(0.1, 0.1, 0.0),
                    Vec3::new(0.4, 0.1, 0.0),
                    Vec3::new(0.1, 0.4, 0.0),
                ],
                vec![
                    Vec2::new(0.05, 0.05),
                    Vec2::new(0.15, 0.05),
                    Vec2::new(0.05, 0.15),
                    Vec2::new(0.8, 0.8),
                    Vec2::new(0.9, 0.8),
                    Vec2::new(0.8, 0.9),
                ],
                vec![[0, 1, 2], [3, 4, 5]],
                vec![0, 0],
            ),
            1,
        );
        let surface = raster_surface(&document, 0);
        let plan = DecalApplyPlan::build(
            &document,
            &[surface],
            decal_transform(Vec3::ZERO, Vec2::ONE),
        )
        .expect("valid decal should produce a plan");

        assert_eq!(plan.targets.len(), 1);
        let target = &plan.targets[0];
        assert_eq!(target.damage_rects.len(), 2);
        assert_eq!(target.footprint_rects.len(), 2);
        assert_eq!(target.draw_batches.len(), 2);
        assert_eq!(target.draw_batches[0].index_ranges, vec![0..3]);
        assert_eq!(target.draw_batches[1].index_ranges, vec![3..6]);
        assert_eq!(target.draw_call_count(), 2);
    }

    #[test]
    fn draw_batches_merge_touching_footprints_without_cross_drawing_separated_ones() {
        let batches = merge_touching_draw_batches(vec![
            (
                RectU32 {
                    origin: [0, 0],
                    size: [8, 8],
                },
                0,
            ),
            (
                RectU32 {
                    origin: [8, 0],
                    size: [8, 8],
                },
                1,
            ),
            (
                RectU32 {
                    origin: [32, 32],
                    size: [8, 8],
                },
                3,
            ),
        ])
        .expect("valid triangle indices should build draw batches");

        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].scissor.origin, [0, 0]);
        assert_eq!(batches[0].scissor.size, [16, 8]);
        assert_eq!(batches[0].index_ranges, vec![0..6]);
        assert_eq!(batches[1].scissor.origin, [32, 32]);
        assert_eq!(batches[1].index_ranges, vec![9..12]);
        let draw_call_count = batches
            .iter()
            .map(|batch| batch.index_ranges.len())
            .sum::<usize>();
        let legacy_range_count = triangle_indices_to_index_ranges([0, 1, 3])
            .expect("valid triangle indices should build index ranges")
            .len();
        assert_eq!(draw_call_count, 2);
        assert_eq!(batches.len() * legacy_range_count, 4);
    }

    #[test]
    fn preview_geometry_limits_surface_draws_and_scissors_to_projected_volume() {
        let mesh = mesh(
            vec![
                Vec3::new(-0.1, -0.1, 0.0),
                Vec3::new(0.1, -0.1, 0.0),
                Vec3::new(0.0, 0.1, 0.0),
                Vec3::new(2.0, 2.0, 0.0),
                Vec3::new(2.2, 2.0, 0.0),
                Vec3::new(2.0, 2.2, 0.0),
            ],
            vec![Vec2::ZERO; 6],
            vec![[0, 1, 2], [3, 4, 5]],
            vec![0, 0],
        );
        let geometry = DecalPreviewGeometry::build(
            &mesh,
            DecalProjection::Surface(decal_transform(Vec3::ZERO, Vec2::splat(0.5))),
            Mat4::IDENTITY,
            [100, 100],
        )
        .expect("valid surface decal should build preview geometry");

        assert_eq!(geometry.index_ranges, vec![0..3]);
        let scissor = geometry
            .screen_scissor
            .expect("small projected decal should have a viewport scissor");
        assert_eq!(scissor.origin, [36, 36]);
        assert_eq!(scissor.size, [28, 28]);
    }

    #[test]
    fn preview_geometry_recovers_view_projection_screen_rectangle() {
        let mesh = mesh(
            vec![
                Vec3::new(-0.1, -0.1, 0.0),
                Vec3::new(0.1, -0.1, 0.0),
                Vec3::new(0.0, 0.1, 0.0),
            ],
            vec![Vec2::ZERO; 3],
            vec![[0, 1, 2]],
            vec![0],
        );
        let transform = ViewProjectionDecalTransform {
            center_uv: Vec2::splat(0.5),
            size_px: Vec2::new(40.0, 20.0),
            rotation_radians: 0.0,
            viewport_size: [100, 100],
        };
        let projector_view_proj = transform
            .projector_view_projection(Mat4::IDENTITY)
            .expect("valid transform should build a projector");
        let geometry = DecalPreviewGeometry::build(
            &mesh,
            DecalProjection::ViewProjection {
                projector_view_proj,
                viewport_view_proj: Mat4::IDENTITY,
                depth_size: [40, 20],
            },
            Mat4::IDENTITY,
            [100, 100],
        )
        .expect("valid projected decal should build preview geometry");

        assert_eq!(geometry.index_ranges, vec![0..3]);
        let scissor = geometry
            .screen_scissor
            .expect("projected image rectangle should have a viewport scissor");
        assert_eq!(scissor.origin, [29, 39]);
        assert_eq!(scissor.size, [42, 22]);
    }

    #[test]
    fn apply_plan_omits_non_intersecting_materials() {
        let document = document_with_mesh(
            mesh(
                vec![
                    Vec3::new(-1.2, -0.2, 0.0),
                    Vec3::new(-0.8, -0.2, 0.0),
                    Vec3::new(-1.0, 0.2, 0.0),
                    Vec3::new(0.8, -0.2, 0.0),
                    Vec3::new(1.2, -0.2, 0.0),
                    Vec3::new(1.0, 0.2, 0.0),
                ],
                vec![
                    Vec2::new(0.1, 0.1),
                    Vec2::new(0.2, 0.1),
                    Vec2::new(0.15, 0.2),
                    Vec2::new(0.8, 0.1),
                    Vec2::new(0.9, 0.1),
                    Vec2::new(0.85, 0.2),
                ],
                vec![[0, 1, 2], [3, 4, 5]],
                vec![0, 1],
            ),
            2,
        );
        let surfaces = [raster_surface(&document, 0), raster_surface(&document, 1)];
        let plan = DecalApplyPlan::build(
            &document,
            &surfaces,
            decal_transform(Vec3::new(-1.0, 0.0, 0.0), Vec2::splat(0.5)),
        )
        .expect("valid decal should produce a plan");

        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].surface.material_index().as_usize(), 0);
        assert_eq!(plan.depth_index_ranges, vec![0..3]);
    }

    #[test]
    fn apply_plan_excludes_hidden_material_geometry() {
        let document = document_with_mesh(
            mesh(
                vec![
                    Vec3::new(-0.5, -0.5, 0.0),
                    Vec3::new(0.5, -0.5, 0.0),
                    Vec3::new(0.0, 0.5, 0.0),
                ],
                vec![Vec2::ZERO, Vec2::X, Vec2::Y],
                vec![[0, 1, 2]],
                vec![0],
            ),
            1,
        );
        let surface = raster_surface(&document, 0);
        let mut scene_visibility = ViewportSceneVisibility::default();
        scene_visibility.set_material_visible(0.into(), false);

        let plan = DecalApplyPlan::build_with_visibility(
            &document,
            &[surface],
            decal_transform(Vec3::ZERO, Vec2::ONE),
            &scene_visibility,
        )
        .expect("hidden geometry should still produce an empty valid plan");

        assert!(plan.is_empty());
        assert!(plan.depth_index_ranges.is_empty());
        assert_eq!(plan.metrics.candidate_triangle_count, 0);
        assert_eq!(plan.metrics.intersection_count, 0);
    }

    #[test]
    fn view_projection_transform_maps_screen_corners_to_projector_clip() {
        let transform = ViewProjectionDecalTransform::initial([2, 1], [200, 100]).unwrap();
        let projector = transform.projector_view_projection(Mat4::IDENTITY).unwrap();
        let expected = [
            Vec2::new(-1.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(-1.0, -1.0),
        ];
        for (screen, expected) in transform.corners_px().into_iter().zip(expected) {
            let viewport_ndc = Vec3::new(
                screen.x / 200.0 * 2.0 - 1.0,
                1.0 - screen.y / 100.0 * 2.0,
                0.0,
            );
            let clip = projector * viewport_ndc.extend(1.0);
            assert!(clip.truncate().truncate().abs_diff_eq(expected, 1.0e-5));
        }
    }

    #[test]
    fn view_projection_resize_preserves_relative_center_and_short_side_scale() {
        let mut transform = ViewProjectionDecalTransform::initial([1, 1], [200, 100]).unwrap();
        transform.center_uv = Vec2::new(0.25, 0.75);
        let resized = transform.resized([600, 200]).unwrap();
        assert_eq!(resized.center_uv, transform.center_uv);
        assert!(resized.size_px.abs_diff_eq(transform.size_px * 2.0, 1.0e-6));
    }

    #[test]
    fn projective_apply_plan_uses_bvh_candidates_before_exact_clipping() {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for triangle_index in 0..8 {
            let center_x = if triangle_index == 0 {
                0.0
            } else {
                10.0 + triangle_index as f32 * 2.0
            };
            let base = positions.len() as u32;
            positions.extend([
                Vec3::new(center_x - 0.25, -0.25, 0.0),
                Vec3::new(center_x + 0.25, -0.25, 0.0),
                Vec3::new(center_x, 0.25, 0.0),
            ]);
            uvs.extend([Vec2::ZERO, Vec2::X, Vec2::Y]);
            indices.push([base, base + 1, base + 2]);
        }
        let triangle_count = indices.len();
        let document =
            document_with_mesh(mesh(positions, uvs, indices, vec![0; triangle_count]), 1);
        let surface = raster_surface(&document, 0);
        let plan = DecalApplyPlan::build_for_projection(
            &document,
            &[surface],
            DecalProjection::ViewProjection {
                projector_view_proj: Mat4::IDENTITY,
                viewport_view_proj: Mat4::IDENTITY,
                depth_size: [64, 64],
            },
        )
        .expect("valid projective decal should produce a plan");

        assert!(plan.metrics.candidate_triangle_count < triangle_count);
        assert_eq!(plan.metrics.intersection_count, 1);
        assert_eq!(plan.depth_index_ranges, vec![0..3]);
    }

    #[test]
    fn projective_apply_plan_clips_to_viewport_and_image_frusta() {
        let document = document_with_mesh(
            mesh(
                vec![
                    Vec3::new(-0.5, -0.5, 0.0),
                    Vec3::new(0.5, -0.5, 0.0),
                    Vec3::new(0.0, 0.5, 0.0),
                ],
                vec![Vec2::ZERO, Vec2::X, Vec2::Y],
                vec![[0, 1, 2]],
                vec![0],
            ),
            1,
        );
        let surface = raster_surface(&document, 0);
        let plan = DecalApplyPlan::build_for_projection(
            &document,
            &[surface],
            DecalProjection::ViewProjection {
                projector_view_proj: Mat4::IDENTITY,
                viewport_view_proj: Mat4::IDENTITY,
                depth_size: [64, 64],
            },
        )
        .unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.depth_index_ranges, vec![0..3]);
        assert_eq!(plan.metrics.candidate_triangle_count, 1);
        assert_eq!(plan.metrics.intersection_count, 1);
    }
}
