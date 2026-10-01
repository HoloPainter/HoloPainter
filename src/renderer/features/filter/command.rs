use crate::core::{
    adjustment::Adjustment, selection::ActiveSelection, surface::PaintSurfaceId,
    surface_filter::SpatialBlurOrientation,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBlurParams {
    pub radius_world: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialBlurParams {
    pub radius_world: f32,
    pub reference_radius_px: f32,
    pub cross_meshes: bool,
    pub orientation: SpatialBlurOrientation,
    pub normal_threshold_cos: f32,
}

#[derive(Debug, Clone)]
pub enum FilterCommand {
    SurfaceBlur {
        source_surfaces: Vec<PaintSurfaceId>,
        affected_surfaces: Vec<PaintSurfaceId>,
        params: SurfaceBlurParams,
        active_selection: ActiveSelection,
    },
    SpatialBlur {
        source_surfaces: Vec<PaintSurfaceId>,
        affected_surfaces: Vec<PaintSurfaceId>,
        params: SpatialBlurParams,
        active_selection: ActiveSelection,
    },
    Adjustment {
        surfaces: Vec<PaintSurfaceId>,
        adjustment: Adjustment,
        active_selection: ActiveSelection,
    },
    AdjustmentPreviewBegin {
        surfaces: Vec<PaintSurfaceId>,
        adjustment: Adjustment,
        active_selection: ActiveSelection,
    },
    AdjustmentPreviewUpdate {
        adjustment: Option<Adjustment>,
    },
    AdjustmentPreviewCommit {
        adjustment: Adjustment,
    },
    AdjustmentPreviewCancel,
}
