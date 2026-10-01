struct ViewProj {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> vp: ViewProj;

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
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(frag_pos.z, 0.0, 0.0, 1.0);
}
