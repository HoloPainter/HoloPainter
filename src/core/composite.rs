#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApplyParams<C> {
    pub opacity: f32,
    pub composite: C,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayerBlendMode {
    #[default]
    Normal = 0,
    Darken = 1,
    Multiply = 2,
    Lighten = 3,
    Screen = 4,
    ColorDodge = 5,
    LinearDodge = 6,
    Overlay = 7,
    SoftLight = 8,
    HardLight = 9,
    Color = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GroupCompositeMode {
    #[default]
    Isolated,
    PassThrough,
}

impl LayerBlendMode {
    pub const ALL: [Self; 11] = [
        Self::Normal,
        Self::Darken,
        Self::Multiply,
        Self::Lighten,
        Self::Screen,
        Self::ColorDodge,
        Self::LinearDodge,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::Color,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureCompositeMode {
    SourceOver,
    DestinationOut,
    Clear,
    Multiply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCompositeMode {
    Replace,
    Add,
    Subtract,
    Intersect,
    Difference,
    Clear,
    Invert,
}

impl<C> ApplyParams<C> {
    pub fn new(opacity: f32, composite: C) -> Self {
        Self { opacity, composite }
    }
}

#[cfg(test)]
mod tests {
    use super::LayerBlendMode;

    #[test]
    fn layer_blend_modes_use_gpu_ids_in_display_order() {
        let expected = [
            (LayerBlendMode::Normal, 0),
            (LayerBlendMode::Darken, 1),
            (LayerBlendMode::Multiply, 2),
            (LayerBlendMode::Lighten, 3),
            (LayerBlendMode::Screen, 4),
            (LayerBlendMode::ColorDodge, 5),
            (LayerBlendMode::LinearDodge, 6),
            (LayerBlendMode::Overlay, 7),
            (LayerBlendMode::SoftLight, 8),
            (LayerBlendMode::HardLight, 9),
            (LayerBlendMode::Color, 10),
        ];

        assert_eq!(LayerBlendMode::ALL.len(), expected.len());
        for (index, (mode, gpu_id)) in expected.into_iter().enumerate() {
            assert_eq!(LayerBlendMode::ALL[index], mode);
            assert_eq!(mode as u32, gpu_id);
        }
    }
}
