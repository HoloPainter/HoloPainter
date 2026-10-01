struct ViewProj {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> vp: ViewProj;

struct WireframeStyle {
    color_opacity: vec4<f32>,
};

@group(0) @binding(1) var<uniform> wireframe: WireframeStyle;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VSOut {
    var out: VSOut;
    out.pos = vp.view_proj * vec4<f32>(position, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return wireframe.color_opacity;
}
