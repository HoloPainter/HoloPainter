use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct LayerCompositeUniform {
    pub opacity: f32,
    pub blend_mode: u32,
    pub mask_enabled: u32,
    pub source_kind: u32,
    pub solid_color: [f32; 4],
    pub adjustment_kind: u32,
    pub _adjustment_pad: [u32; 3],
    pub adjustment_params: [[f32; 4]; 8],
    pub output_uv_to_source_uv_row0: [f32; 4],
    pub output_uv_to_source_uv_row1: [f32; 4],
}
