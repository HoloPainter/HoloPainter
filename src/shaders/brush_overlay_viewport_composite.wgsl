@group(0) @binding(0) var brush_overlay_tex: texture_2d<f32>;
@group(0) @binding(1) var brush_overlay_smp: sampler;

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VSOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    var out: VSOut;
    let pos = positions[vertex_index];
    out.clip_pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = pos * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let mask = textureSample(brush_overlay_tex, brush_overlay_smp, in.uv).r;
    if (mask <= 0.0) {
        discard;
    }
    let color = vec3<f32>(0.05, 0.9, 1.0);
    return vec4<f32>(color * mask, mask * 0.85);
}
