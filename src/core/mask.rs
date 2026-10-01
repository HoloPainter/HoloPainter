use glam::{Mat4, Vec2};

use super::{
    document::{MeshId, SurfaceHit},
    stroke::{StrokeDab, SurfaceDab, SurfaceProjectionId},
    viewport_visibility::ViewportSceneVisibility,
};

#[derive(Debug, Clone, PartialEq)]
pub enum MaskSource {
    Brush(BrushMaskSource),
    Shape(ShapeMaskSource),
    Full(FullMaskSource),
    Geometry(GeometryMaskSource),
    Projection(ProjectionMaskSource),
    Flood(FloodMaskSource),
    ExistingSelection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushMaskSource {
    pub space: BrushInputSpace,
    pub samples: BrushSamples,
    pub brush: BrushMaskParams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushInputSpace {
    Uv,
    Surface,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BrushSamples {
    Uv(Vec<StrokeDab>),
    Surface(Vec<SurfaceDab>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushMaskParams {
    pub radius_world: f32,
    pub hardness: f32,
    pub effect_base: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeMaskSource {
    Rectangle(RectangleMaskSource),
    Polygon(PolygonMaskSource),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectangleMaskSource {
    pub min_uv: Vec2,
    pub max_uv: Vec2,
    pub material_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolygonMaskSource {
    pub points_uv: Vec<Vec2>,
    pub material_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeometryMaskSource {
    Mesh(MeshGeometryMaskSource),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshGeometryMaskSource {
    pub mesh_id: MeshId,
    pub triangles: Vec<GeometryMaskTriangle>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryMaskTriangle {
    pub uv: [Vec2; 3],
    pub material_index: usize,
    pub mesh_id: MeshId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionMaskSource {
    ViewportRect(ViewportRectProjectionMaskSource),
    ViewportPolygon(ViewportPolygonProjectionMaskSource),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportProjection {
    pub id: SurfaceProjectionId,
    pub view_proj: Mat4,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportRectProjectionMaskSource {
    pub min_px: Vec2,
    pub max_px: Vec2,
    pub viewport_size: [u32; 2],
    pub projections: Vec<ViewportProjection>,
    pub material_index: Option<usize>,
    pub scene_visibility: ViewportSceneVisibility,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportPolygonProjectionMaskSource {
    pub points_px: Vec<Vec2>,
    pub viewport_size: [u32; 2],
    pub projections: Vec<ViewportProjection>,
    pub material_index: Option<usize>,
    pub scene_visibility: ViewportSceneVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullMaskSource {
    Material { material_index: usize },
    MeshAllMaterials,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FloodMaskSource {
    UvContiguous {
        seed_uv: [f32; 2],
        threshold: f32,
    },
    SurfaceContiguous {
        seed_hit: SurfaceSeed,
        threshold: f32,
        sample_mode: FillSampleMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSeed {
    pub hit: SurfaceHit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillSampleMode {
    ActiveLayer,
    Composite,
}

impl BrushMaskSource {
    pub fn uv(samples: Vec<StrokeDab>, brush: BrushMaskParams) -> Self {
        Self {
            space: BrushInputSpace::Uv,
            samples: BrushSamples::Uv(samples),
            brush,
        }
    }

    pub fn surface(samples: Vec<SurfaceDab>, brush: BrushMaskParams) -> Self {
        Self {
            space: BrushInputSpace::Surface,
            samples: BrushSamples::Surface(samples),
            brush,
        }
    }
}
