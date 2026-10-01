struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VSOut {
    var quad = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    var out: VSOut;
    let pos = quad[i];
    out.clip_pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = vec2<f32>(pos.x * 0.5 + 0.5, 1.0 - (pos.y * 0.5 + 0.5));
    return out;
}

struct PassThroughGateParams {
    opacity: f32,
    _mode: u32,
    mask_enabled: u32,
    _pad0: u32,
};

@group(0) @binding(0) var after_tex: texture_2d<f32>;
@group(0) @binding(1) var before_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_smp: sampler;
@group(0) @binding(3) var<uniform> params: PassThroughGateParams;
@group(0) @binding(4) var mask_tex: texture_2d<f32>;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let before = textureSampleLevel(before_tex, layer_smp, in.uv, 0.0);
    let after = textureSampleLevel(after_tex, layer_smp, in.uv, 0.0);
    var mask_alpha = 1.0;
    if (params.mask_enabled != 0u) {
        mask_alpha = clamp(textureSampleLevel(mask_tex, layer_smp, in.uv, 0.0).r, 0.0, 1.0);
    }
    let gate = clamp(params.opacity * mask_alpha, 0.0, 1.0);
    return before + (after - before) * gate;
}
