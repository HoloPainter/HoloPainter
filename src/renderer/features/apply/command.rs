use glam::Vec2;

use crate::{
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        damage::DamageMap,
        decal::{DecalApplyPlan, DecalImageAsset, DecalProjection},
        mask::MaskSource,
        selection::ActiveSelection,
        surface::PaintSurfaceId,
    },
    renderer::features::apply::operation::ApplyOperation,
};

#[derive(Debug, Clone, PartialEq)]
pub enum FillCoverage {
    Full,
    Triangles(Vec<[Vec2; 3]>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FillCoverageDelta {
    pub surface: PaintSurfaceId,
    pub coverage: FillCoverage,
}

#[derive(Debug, Clone)]
pub enum FillStrokeCommand {
    Begin {
        operation: ApplyOperation,
        params: ApplyParams<TextureCompositeMode>,
        active_selection: ActiveSelection,
    },
    Extend {
        deltas: Vec<FillCoverageDelta>,
        preview_damage: Option<DamageMap>,
    },
    End {
        damage: Option<DamageMap>,
    },
    Cancel,
}

#[derive(Debug, Clone)]
pub enum ApplyCommand {
    FillStroke(FillStrokeCommand),
    OneShot {
        target: PaintSurfaceId,
        surfaces: Vec<PaintSurfaceId>,
        mask: MaskSource,
        operation: ApplyOperation,
        params: ApplyParams<TextureCompositeMode>,
        active_selection: ActiveSelection,
        damage: Option<DamageMap>,
    },
    Decal {
        plan: DecalApplyPlan,
        image: std::sync::Arc<DecalImageAsset>,
        projection: DecalProjection,
        opacity: f32,
        active_selection: ActiveSelection,
    },
}
