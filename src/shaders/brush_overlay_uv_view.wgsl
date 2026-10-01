struct UvBrushOverlay {
    view_size: vec2<f32>,
    canvas_size: vec2<f32>,
    center_px: vec2<f32>,
    _pad0: vec2<f32>,
    brush_params: vec4<f32>,
    center_transform: vec4<f32>,
};

@group(0) @binding(0) var<uniform> brush_overlay: UvBrushOverlay;

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
    let rotation_sin = brush_overlay.center_transform.z;
    let rotation_cos = brush_overlay.center_transform.w;
    return vec2<f32>(
        rotation_cos * value.x + rotation_sin * value.y,
        -rotation_sin * value.x + rotation_cos * value.y,
    );
}

fn uv_from_fragment(frag_pos: vec2<f32>) -> vec2<f32> {
    let view_size = max(brush_overlay.view_size, vec2<f32>(1.0));
    let canvas_size = max(brush_overlay.canvas_size, vec2<f32>(1.0));
    let zoom = max(brush_overlay.brush_params.w, 0.0001);
    let canvas_offset = inverse_rotate(frag_pos - view_size * 0.5);
    return brush_overlay.center_transform.xy + canvas_offset / (canvas_size * zoom);
}

@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_from_fragment(frag_pos.xy);
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        discard;
    }
    let radius_px = brush_overlay.brush_params.x;
    let thickness_px = brush_overlay.brush_params.y;
    let opacity = brush_overlay.brush_params.z;
    let dist = distance(frag_pos.xy, brush_overlay.center_px);
    let half_thickness = max(thickness_px * 0.5, 0.5);
    let aa = 1.0;
    let ring = 1.0 - smoothstep(
        half_thickness - aa,
        half_thickness + aa,
        abs(dist - radius_px)
    );
    return vec4<f32>(vec3<f32>(0.05, 0.9, 1.0), ring * opacity);
}
