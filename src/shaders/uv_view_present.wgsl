struct UvViewTransform {
    view_size: vec2<f32>,
    canvas_size: vec2<f32>,
    center_transform: vec4<f32>,
    rotation: vec4<f32>,
    background_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view_transform: UvViewTransform;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VSOut {
    var out: VSOut;
    let pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    out.clip_pos = vec4<f32>(pos[vertex_index], 0.0, 1.0);
    return out;
}

fn inverse_rotate(value: vec2<f32>) -> vec2<f32> {
    let sin_angle = view_transform.center_transform.w;
    let cos_angle = view_transform.rotation.x;
    return vec2<f32>(
        cos_angle * value.x + sin_angle * value.y,
        -sin_angle * value.x + cos_angle * value.y,
    );
}

fn uv_from_fragment(frag_pos: vec2<f32>) -> vec2<f32> {
    let view_size = max(view_transform.view_size, vec2<f32>(1.0));
    let canvas_size = max(view_transform.canvas_size, vec2<f32>(1.0));
    let zoom = max(view_transform.center_transform.z, 0.0001);
    let canvas_offset = inverse_rotate(frag_pos - view_size * 0.5);
    return view_transform.center_transform.xy + canvas_offset / (canvas_size * zoom);
}

fn checker_color(uv: vec2<f32>) -> vec3<f32> {
    let cell = vec2<i32>(floor(uv * 32.0));
    let odd = (cell.x + cell.y) & 1;
    return select(vec3<f32>(0.18), vec3<f32>(0.24), odd == 1);
}

@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_from_fragment(frag_pos.xy);
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return view_transform.background_color;
    }
    let source = textureSample(source_tex, source_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
    let checker = checker_color(uv);
    return vec4<f32>(source.rgb + checker * (1.0 - source.a), 1.0);
}
