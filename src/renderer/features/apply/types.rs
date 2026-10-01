use bytemuck::{Pod, Zeroable};

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UvTextureCompositeMode {
    SourceOver = 0,
    DestinationOut = 1,
    Clear = 2,
    Multiply = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvCompositeUniform {
    pub paint_rgb_opacity: [f32; 4],
    pub composite_mode: u32,
    pub selection_enabled: u32,
    pub _pad0: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct DecalApplyUniform {
    pub(crate) projector_view_proj: [[f32; 4]; 4],
    pub(crate) visibility_view_proj: [[f32; 4]; 4],
    pub(crate) center_depth: [f32; 4],
    pub(crate) axis_x_half_width: [f32; 4],
    pub(crate) axis_y_half_height: [f32; 4],
    pub(crate) normal_opacity: [f32; 4],
    pub(crate) params: [f32; 4],
    pub(crate) flags: [u32; 4],
}
