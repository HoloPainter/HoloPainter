use bytemuck::{Pod, Zeroable};

use crate::core::stroke::StrokeDab;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BrushEngineUniform {
    pub color_rgb_pad: [f32; 4],
    pub base_radius: [f32; 4],
    pub params0: [f32; 4],
    pub params1: [f32; 4],
    pub params2: [f32; 4],
    pub params3: [f32; 4],
    pub params_u32_0: [u32; 4],
    pub params_u32_1: [u32; 4],
    pub params_u32_2: [u32; 4],
    pub params_u32_3: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvDabGpu {
    pub center_px: [f32; 2],
    pub radius_px: f32,
    pub pressure: f32,
    pub dir: [f32; 2],
    pub spacing_alpha_scale: f32,
    pub _pad0: f32,
    pub dynamic0: [f32; 4],
    pub dynamic1: [f32; 4],
    pub dynamic2: [f32; 4],
    pub dynamic3: [f32; 4],
}

impl UvDabGpu {
    pub(crate) fn from_dab(
        dab: &StrokeDab,
        radius_px: f32,
        tex_size: [f32; 2],
        dynamic_values: [[f32; 4]; 4],
    ) -> Self {
        Self {
            center_px: [dab.position.x * tex_size[0], dab.position.y * tex_size[1]],
            radius_px: radius_px * dab.radius_scale,
            pressure: dab.pressure,
            dir: [0.0, 0.0],
            spacing_alpha_scale: dab.spacing_alpha_scale,
            _pad0: 0.0,
            dynamic0: dynamic_values[0],
            dynamic1: dynamic_values[1],
            dynamic2: dynamic_values[2],
            dynamic3: dynamic_values[3],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BakeUniform {
    pub camera_world: [f32; 4],
    pub paint_color: [f32; 4],
    pub viewport_depth: [f32; 4],
    pub viewport_metrics: [f32; 4],
    pub depth_params: [f32; 4],
}
