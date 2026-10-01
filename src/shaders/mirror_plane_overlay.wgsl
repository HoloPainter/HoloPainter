struct MirrorPlaneOverlayUniform {
    view_proj: mat4x4<f32>,
    center_half_y: vec4<f32>,
    half_z_opacity: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> overlay: MirrorPlaneOverlayUniform;

fn world_position(y_sign: f32, z_sign: f32) -> vec3<f32> {
    return vec3<f32>(
        overlay.center_half_y.x,
        overlay.center_half_y.y + overlay.center_half_y.w * y_sign,
        overlay.center_half_y.z + overlay.half_z_opacity.x * z_sign,
    );
}

@vertex
fn vs_face(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var y_sign = -1.0;
    var z_sign = -1.0;
    switch vertex_index {
        case 1u: {
            y_sign = 1.0;
        }
        case 2u: {
            z_sign = 1.0;
        }
        case 3u: {
            y_sign = 1.0;
            z_sign = 1.0;
        }
        default: {}
    }
    return overlay.view_proj * vec4<f32>(world_position(y_sign, z_sign), 1.0);
}

fn line_world_position(vertex_index: u32) -> vec3<f32> {
    switch vertex_index {
        case 0u: { return world_position(-1.0, -1.0); }
        case 1u: { return world_position(1.0, -1.0); }
        case 2u: { return world_position(1.0, -1.0); }
        case 3u: { return world_position(1.0, 1.0); }
        case 4u: { return world_position(1.0, 1.0); }
        case 5u: { return world_position(-1.0, 1.0); }
        case 6u: { return world_position(-1.0, 1.0); }
        case 7u: { return world_position(-1.0, -1.0); }
        case 8u: { return world_position(-1.0, 0.0); }
        case 9u: { return world_position(1.0, 0.0); }
        case 10u: { return world_position(0.0, -1.0); }
        default: { return world_position(0.0, 1.0); }
    }
}

@vertex
fn vs_line(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    return overlay.view_proj * vec4<f32>(line_world_position(vertex_index), 1.0);
}

@fragment
fn fs_face() -> @location(0) vec4<f32> {
    return vec4<f32>(overlay.color.rgb, overlay.half_z_opacity.y);
}

@fragment
fn fs_line_visible() -> @location(0) vec4<f32> {
    return vec4<f32>(overlay.color.rgb, overlay.half_z_opacity.z);
}

@fragment
fn fs_line_occluded() -> @location(0) vec4<f32> {
    return vec4<f32>(overlay.color.rgb, overlay.half_z_opacity.w);
}
