#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectU32 {
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

impl RectU32 {
    pub fn full(texture_size: [u32; 2]) -> Self {
        Self {
            origin: [0, 0],
            size: texture_size,
        }
    }
}
