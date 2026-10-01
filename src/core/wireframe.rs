#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireframeStyle {
    pub color: [f32; 3],
    pub opacity: f32,
}

impl WireframeStyle {
    pub const fn new(color: [f32; 3], opacity: f32) -> Self {
        Self { color, opacity }
    }

    pub fn normalized(self) -> Self {
        Self {
            color: self.color.map(|value| value.clamp(0.0, 1.0)),
            opacity: self.opacity.clamp(0.0, 1.0),
        }
    }
}
