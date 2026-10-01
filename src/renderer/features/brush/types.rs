use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct CurrentDabBatchHeader {
    pub count: u32,
    pub _pad0: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SurfaceDabGpu {
    pub center_radius: [f32; 4],
    pub normal_pressure: [f32; 4],
    pub tangent_x_pad: [f32; 4],
    pub tangent_y_pad: [f32; 4],
    pub dynamic0: [f32; 4],
    pub dynamic1: [f32; 4],
    pub dynamic2: [f32; 4],
    pub dynamic3: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BrushDabInstance {
    pub center_px: [f32; 2],
    pub radius_scale: f32,
    pub pressure: f32,
    pub spacing_alpha_scale: f32,
    pub _pad0: f32,
    pub dir: [f32; 2],
    pub dynamic0: [f32; 4],
    pub dynamic1: [f32; 4],
    pub dynamic2: [f32; 4],
    pub dynamic3: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::SurfaceDabGpu;

    #[test]
    fn surface_direction_reuses_padding_without_changing_gpu_dab_size() {
        assert_eq!(std::mem::size_of::<SurfaceDabGpu>(), 128);
    }
}
