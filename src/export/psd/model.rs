use crate::core::{
    adjustment::Adjustment,
    composite::{GroupCompositeMode, LayerBlendMode},
};

#[derive(Debug, Clone, PartialEq)]
pub struct PsdExportDocument {
    pub width: u32,
    pub height: u32,
    /// PSD order: topmost layer first.
    pub layers: Vec<PsdExportLayer>,
    /// Straight-alpha RGBA8, top row first.
    pub composite_rgba8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PsdExportLayer {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend_mode: LayerBlendMode,
    pub mask: Option<PsdExportMask>,
    pub content: PsdExportLayerContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsdExportMask {
    pub enabled: bool,
    pub r8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PsdExportLayerContent {
    Raster {
        rgba8: Vec<u8>,
    },
    SolidFill {
        color: [f32; 3],
    },
    Adjustment {
        adjustment: Adjustment,
    },
    Group {
        composite_mode: GroupCompositeMode,
        children: Vec<PsdExportLayer>,
    },
}
