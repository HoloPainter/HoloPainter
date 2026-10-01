use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct WireframeUniform {
    pub color_opacity: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct ViewportUniform {
    pub camera_world: [f32; 4],
    pub shading: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SurfaceBrushOverlayUniform {
    pub view_proj: [[f32; 4]; 4],
    pub center_radius: [f32; 4],
    pub normal_params: [f32; 4],
    pub view_direction: [f32; 4],
    pub params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvBrushOverlayUniform {
    pub view_size: [f32; 2],
    pub canvas_size: [f32; 2],
    pub center_px: [f32; 2],
    pub _pad0: [f32; 2],
    // x: radius px, y: thickness px, z: opacity, w: zoom.
    pub brush_params: [f32; 4],
    // xy: center UV, z: rotation sin, w: rotation cos.
    pub center_transform: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvViewTransformUniform {
    pub view_size: [f32; 2],
    pub canvas_size: [f32; 2],
    // xy: center UV, z: zoom, w: rotation sin.
    pub center_transform: [f32; 4],
    // x: rotation cos.
    pub rotation: [f32; 4],
    pub background_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvSelectionOverlayUniform {
    pub view_size: [f32; 2],
    pub texture_size: [f32; 2],
    pub params: [f32; 4],
    pub color: [f32; 4],
    // xy: center UV, z: zoom, w: rotation sin.
    pub center_transform: [f32; 4],
    // x: rotation cos.
    pub rotation: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MirrorPlaneOverlayUniform {
    pub view_proj: [[f32; 4]; 4],
    pub center_half_y: [f32; 4],
    pub half_z_opacity: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SelectionOverlayUniform {
    pub view_size: [f32; 2],
    pub texture_size: [f32; 2],
    // x: phase, y: reserved, z: edge opacity, w: edge thickness px.
    pub params: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct DecalOverlayUniform {
    pub view_proj: [[f32; 4]; 4],
    pub projector_view_proj: [[f32; 4]; 4],
    pub visibility_view_proj: [[f32; 4]; 4],
    pub center_depth: [f32; 4],
    pub axis_x_half_width: [f32; 4],
    pub axis_y_half_height: [f32; 4],
    pub normal_opacity: [f32; 4],
    pub params: [f32; 4],
    pub flags: [u32; 4],
}
