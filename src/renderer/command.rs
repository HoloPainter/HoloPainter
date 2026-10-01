//! Renderer-owned feature command payloads carried by `RendererFramePlan`.

use std::sync::Arc;

use crate::core::{
    brush_engine::ParamValue,
    damage::DamageMap,
    selection::ActiveSelection,
    stroke::{PaintSurfaceSet, StrokeContext, StrokeDab, SurfaceDab},
    stroke_preset::StrokeOp,
    surface::PaintSurfaceId,
};

#[derive(Debug, Clone)]
pub enum TransformCommand {
    Begin {
        target: PaintSurfaceId,
        source_bounds: crate::core::geometry::RectU32,
        active_selection: ActiveSelection,
    },
    BeginImported {
        target: PaintSurfaceId,
        source_size: [u32; 2],
        source_rgba8: Arc<[u8]>,
    },
    Preview {
        transform: crate::core::transform::UvTransform,
        result_damage: DamageMap,
    },
    Commit {
        transform: crate::core::transform::UvTransform,
        result_damage: DamageMap,
    },
    Cancel,
    Discard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RendererStrokeStyle {
    pub stroke_op: StrokeOp,
    pub operation: RendererStrokeOperation,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RendererStrokeOperation {
    BrushEngine {
        color: [f32; 3],
        params: Vec<(String, ParamValue)>,
    },
}

impl RendererStrokeStyle {
    pub fn brush_engine(
        stroke_op: StrokeOp,
        color: [f32; 3],
        params: Vec<(String, ParamValue)>,
    ) -> Self {
        Self {
            stroke_op,
            operation: RendererStrokeOperation::BrushEngine { color, params },
        }
    }
}

#[derive(Debug, Clone)]
pub enum StrokeCommand {
    Begin {
        target: StrokeTarget,
        style: Arc<RendererStrokeStyle>,
        active_selection: Arc<ActiveSelection>,
    },
    AddDabs {
        dabs: StrokeDabPayload,
        preview_damage: Option<DamageMap>,
    },
    End {
        damage: Option<DamageMap>,
    },
    Cancel,
}

#[derive(Debug, Clone)]
pub enum StrokeTarget {
    Uv {
        surface: PaintSurfaceId,
    },
    Surface {
        surfaces: PaintSurfaceSet,
        context: Arc<StrokeContext>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StrokeSpace {
    Uv { target: PaintSurfaceId },
    Surface { surfaces: PaintSurfaceSet },
}

impl StrokeSpace {
    pub(crate) fn surfaces(&self) -> PaintSurfaceSet {
        match self {
            Self::Uv { target } => PaintSurfaceSet::single(*target),
            Self::Surface { surfaces } => surfaces.clone(),
        }
    }

    pub(crate) fn matches_uv(&self, target: PaintSurfaceId) -> bool {
        matches!(self, Self::Uv { target: active } if *active == target)
    }

    pub(crate) fn matches_surface(&self, surfaces: &PaintSurfaceSet) -> bool {
        matches!(self, Self::Surface { surfaces: active } if active == surfaces)
    }
}

impl StrokeTarget {
    pub fn surfaces(&self) -> PaintSurfaceSet {
        match self {
            Self::Uv { surface } => PaintSurfaceSet::single(*surface),
            Self::Surface { surfaces, .. } => surfaces.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum StrokeDabPayload {
    Stroke(Vec<StrokeDab>),
    Surface {
        source_surfaces: PaintSurfaceSet,
        target_surfaces: PaintSurfaceSet,
        projection_batches: Vec<SurfaceProjectionDabBatch>,
    },
}

#[derive(Debug, Clone)]
pub struct SurfaceProjectionDabBatch {
    pub projection_id: crate::core::stroke::SurfaceProjectionId,
    pub continues_from_previous: bool,
    pub dabs: Vec<SurfaceDab>,
    pub target_materials_by_dab: Vec<Vec<usize>>,
}

impl SurfaceProjectionDabBatch {
    pub fn primary(dabs: Vec<SurfaceDab>, target_materials_by_dab: Vec<Vec<usize>>) -> Self {
        debug_assert_eq!(dabs.len(), target_materials_by_dab.len());
        Self {
            projection_id: crate::core::stroke::SurfaceProjectionId::Primary,
            continues_from_previous: true,
            dabs,
            target_materials_by_dab,
        }
    }
}
