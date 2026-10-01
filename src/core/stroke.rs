use std::sync::Arc;

use glam::{Mat4, Vec2, Vec3};

use super::{
    camera::{CameraProjection, OrbitCamera},
    document::{MeshId, SurfaceHit},
    selection::ActiveSelection,
    stroke_style::ResolvedStrokeStyle,
    surface::PaintSurfaceId,
    viewport_visibility::ViewportSceneVisibility,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeSpace {
    Uv,
    Surface,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UvStrokeContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SurfaceProjectionId {
    Primary,
    MirrorX,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceProjectionContext {
    pub id: SurfaceProjectionId,
    pub view_proj_matrix_gl: Mat4,
    pub camera_world: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceCameraParams {
    pub projection: CameraProjection,
    pub fov_y_radians: f32,
    pub orthographic_height: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceStrokeContext {
    pub viewport_size: [u32; 2],
    pub camera: SurfaceCameraParams,
    pub projections: Arc<[SurfaceProjectionContext]>,
    pub scene_visibility: ViewportSceneVisibility,
}

impl SurfaceStrokeContext {
    pub fn projection(&self, id: SurfaceProjectionId) -> Option<&SurfaceProjectionContext> {
        self.projections
            .iter()
            .find(|projection| projection.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrokeContext {
    Uv(UvStrokeContext),
    Surface(SurfaceStrokeContext),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaintSurfaceSet {
    Single(PaintSurfaceId),
    Shared(Arc<[PaintSurfaceId]>),
}

impl PaintSurfaceSet {
    pub fn single(surface: PaintSurfaceId) -> Self {
        Self::Single(surface)
    }

    pub fn from_vec(surfaces: Vec<PaintSurfaceId>) -> Self {
        match surfaces.as_slice() {
            [surface] => Self::Single(*surface),
            _ => Self::Shared(surfaces.into()),
        }
    }

    pub fn as_slice(&self) -> &[PaintSurfaceId] {
        match self {
            Self::Single(surface) => std::slice::from_ref(surface),
            Self::Shared(surfaces) => surfaces.as_ref(),
        }
    }

    pub fn first(&self) -> Option<PaintSurfaceId> {
        self.as_slice().first().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = PaintSurfaceId> + '_ {
        self.as_slice().iter().copied()
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

impl From<PaintSurfaceId> for PaintSurfaceSet {
    fn from(surface: PaintSurfaceId) -> Self {
        Self::single(surface)
    }
}

impl From<Vec<PaintSurfaceId>> for PaintSurfaceSet {
    fn from(surfaces: Vec<PaintSurfaceId>) -> Self {
        Self::from_vec(surfaces)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeRenderDescriptor {
    pub target: PaintSurfaceId,
    pub surfaces: PaintSurfaceSet,
    pub style: Arc<ResolvedStrokeStyle>,
    pub active_selection: Arc<ActiveSelection>,
    pub context: Arc<StrokeContext>,
}

impl StrokeRenderDescriptor {
    pub fn new(
        target: PaintSurfaceId,
        surfaces: PaintSurfaceSet,
        style: Arc<ResolvedStrokeStyle>,
        active_selection: Arc<ActiveSelection>,
        context: Arc<StrokeContext>,
    ) -> Self {
        Self {
            target,
            surfaces,
            style,
            active_selection,
            context,
        }
    }

    pub fn single_uv(
        target: PaintSurfaceId,
        style: Arc<ResolvedStrokeStyle>,
        active_selection: Arc<ActiveSelection>,
    ) -> Self {
        Self::new(
            target,
            PaintSurfaceSet::single(target),
            style,
            active_selection,
            Arc::new(StrokeContext::uv()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeDab {
    pub position: Vec2,
    /// Corrected tablet pressure after input normalization, clamping, and smoothing.
    pub pressure: f32,
    pub radius_scale: f32,
    /// Alpha compensation for dabs emitted at a larger spacing than their ideal spacing.
    pub spacing_alpha_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDab {
    pub screen_px: Vec2,
    pub world_pos: Vec3,
    pub world_normal: Vec3,
    pub material_index: usize,
    pub uv: Option<Vec2>,
    pub uv_edge_distance: Option<f32>,
    pub uv_paint_boundary_distance: Option<f32>,
    pub triangle_index: Option<usize>,
    pub mesh_id: Option<MeshId>,
    /// Ray distance captured from the surface hit that produced this dab.
    /// Used to avoid re-raycasting the dab center during surface footprint checks.
    pub ray_t: Option<f32>,
    pub tangent_x: Vec3,
    pub tangent_y: Vec3,
    /// Corrected tablet pressure after input normalization, clamping, and smoothing.
    pub pressure: f32,
    pub radius_scale: f32,
}

impl SurfaceDab {
    pub fn with_scales(
        screen_px: Vec2,
        world_pos: Vec3,
        world_normal: Vec3,
        material_index: usize,
        pressure: f32,
        radius_scale: f32,
    ) -> Self {
        let world_normal = normalized_or(world_normal, Vec3::Z);
        let (tangent_x, tangent_y) = tangent_basis_from_normal(world_normal);
        Self {
            screen_px,
            world_pos,
            world_normal,
            material_index,
            uv: None,
            uv_edge_distance: None,
            uv_paint_boundary_distance: None,
            triangle_index: None,
            mesh_id: None,
            ray_t: None,
            tangent_x,
            tangent_y,
            pressure: pressure.clamp(0.0, 1.0),
            radius_scale: radius_scale.max(0.0),
        }
    }

    pub fn from_hit_with_scales(
        screen_px: Vec2,
        hit: SurfaceHit,
        pressure: f32,
        radius_scale: f32,
    ) -> Self {
        let mut dab = Self::with_scales(
            screen_px,
            hit.world_pos,
            hit.world_normal,
            hit.material_index.as_usize(),
            pressure,
            radius_scale,
        );
        dab.uv = Some(hit.uv);
        dab.uv_edge_distance = Some(hit.uv_edge_distance);
        dab.uv_paint_boundary_distance = Some(hit.uv_paint_boundary_distance);
        dab.triangle_index = Some(hit.triangle_index);
        dab.mesh_id = Some(hit.mesh_id);
        dab.ray_t = Some(hit.t);
        dab
    }
}

fn tangent_basis_from_normal(normal: Vec3) -> (Vec3, Vec3) {
    // Fixed orientation derived from normal and a world reference axis.
    // This may flip near the reference-axis threshold; follow-stroke orientation
    // should replace this if stable brush orientation across curved surfaces is needed.
    let n = normalized_or(normal, Vec3::Z);
    let reference = if n.y.abs() < 0.95 { Vec3::Y } else { Vec3::X };
    let tangent_x = normalized_or(reference.cross(n), Vec3::X);
    let tangent_y = normalized_or(n.cross(tangent_x), Vec3::Y);
    (tangent_x, tangent_y)
}

fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let n = v.normalize_or_zero();
    if n.length_squared() > f32::EPSILON {
        n
    } else {
        fallback
    }
}

impl StrokeDab {
    pub fn new(position: Vec2, pressure: f32) -> Self {
        Self {
            position,
            pressure: pressure.clamp(0.0, 1.0),
            radius_scale: 1.0,
            spacing_alpha_scale: 1.0,
        }
    }

    pub fn with_scales(position: Vec2, pressure: f32, radius_scale: f32) -> Self {
        Self {
            position,
            pressure: pressure.clamp(0.0, 1.0),
            radius_scale: radius_scale.max(0.0),
            spacing_alpha_scale: 1.0,
        }
    }

    pub fn with_spacing_alpha_scale(mut self, spacing_alpha_scale: f32) -> Self {
        self.spacing_alpha_scale = spacing_alpha_scale.max(1.0);
        self
    }
}

impl StrokeContext {
    pub fn uv() -> Self {
        Self::Uv(UvStrokeContext)
    }

    pub fn surface(
        camera: OrbitCamera,
        viewport_view_proj_matrix_gl: Mat4,
        viewport_size: [u32; 2],
        camera_world: [f32; 3],
        scene_visibility: ViewportSceneVisibility,
    ) -> Self {
        Self::surface_projections(
            camera,
            viewport_size,
            vec![SurfaceProjectionContext {
                id: SurfaceProjectionId::Primary,
                view_proj_matrix_gl: viewport_view_proj_matrix_gl,
                camera_world,
            }],
            scene_visibility,
        )
    }

    pub fn surface_projections(
        camera: OrbitCamera,
        viewport_size: [u32; 2],
        projections: Vec<SurfaceProjectionContext>,
        scene_visibility: ViewportSceneVisibility,
    ) -> Self {
        Self::Surface(SurfaceStrokeContext {
            viewport_size,
            camera: SurfaceCameraParams {
                projection: camera.projection,
                fov_y_radians: camera.fov_y_radians,
                orthographic_height: camera.orthographic_height,
                near: camera.near,
                far: camera.far,
            },
            projections: projections.into(),
            scene_visibility,
        })
    }

    pub fn space(&self) -> StrokeSpace {
        match self {
            StrokeContext::Uv(_) => StrokeSpace::Uv,
            StrokeContext::Surface(_) => StrokeSpace::Surface,
        }
    }
}
