struct UvSelectionOverlay {
    view_size: vec2<f32>,
    texture_size: vec2<f32>,
    params: vec4<f32>,
    color: vec4<f32>,
    center_transform: vec4<f32>,
    rotation: vec4<f32>,
};

@group(0) @binding(0) var<uniform> overlay: UvSelectionOverlay;
@group(0) @binding(1) var selection_tex: texture_2d<f32>;

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
    let sin_angle = overlay.center_transform.w;
    let cos_angle = overlay.rotation.x;
    return vec2<f32>(
        cos_angle * value.x + sin_angle * value.y,
        -sin_angle * value.x + cos_angle * value.y,
    );
}

fn uv_from_fragment(frag_pos: vec2<f32>) -> vec2<f32> {
    let view_size = max(overlay.view_size, vec2<f32>(1.0));
    let canvas_size = max(overlay.texture_size, vec2<f32>(1.0));
    let zoom = max(overlay.center_transform.z, 0.0001);
    let canvas_offset = inverse_rotate(frag_pos - view_size * 0.5);
    return overlay.center_transform.xy + canvas_offset / (canvas_size * zoom);
}

fn texture_size_i32() -> vec2<i32> {
    return vec2<i32>(
        max(i32(overlay.texture_size.x), 1),
        max(i32(overlay.texture_size.y), 1),
    );
}

fn coord_from_uv(uv: vec2<f32>) -> vec2<i32> {
    let raw_f = floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.999999)) * overlay.texture_size);
    let raw = vec2<i32>(i32(raw_f.x), i32(raw_f.y));
    let tex_size = texture_size_i32();
    return clamp(raw, vec2<i32>(0, 0), tex_size - vec2<i32>(1, 1));
}

fn load_mask_uv(uv: vec2<f32>) -> f32 {
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x >= 1.0 || uv.y >= 1.0) {
        return 0.0;
    }
    return textureLoad(selection_tex, coord_from_uv(uv), 0).r;
}

fn edge_strength(uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> f32 {
    let center_mask = load_mask_uv(uv);
    if center_mask <= 0.001 {
        return 0.0;
    }

    var min_mask = center_mask;
    var max_mask = center_mask;

    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let sample_uv = uv + f32(x) * uv_dx + f32(y) * uv_dy;
            let m = load_mask_uv(sample_uv);
            min_mask = min(min_mask, m);
            max_mask = max(max_mask, m);
        }
    }

    let crosses_boundary = max_mask > 0.001 && min_mask <= 0.001;
    return select(0.0, 1.0, crosses_boundary);
}

@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_from_fragment(frag_pos.xy);
    let canvas_size = max(overlay.texture_size, vec2<f32>(1.0));
    let zoom = max(overlay.center_transform.z, 0.0001);
    let uv_dx = inverse_rotate(vec2<f32>(1.0, 0.0)) / (canvas_size * zoom);
    let uv_dy = inverse_rotate(vec2<f32>(0.0, 1.0)) / (canvas_size * zoom);
    let phase = overlay.params.x;
    let edge_opacity = overlay.params.z;
    let edge = edge_strength(uv, uv_dx, uv_dy);
    if edge <= 0.001 {
        discard;
    }

    let ants = select(0.55, 1.0, fract((frag_pos.x + frag_pos.y) * 0.12 + phase * 1.75) > 0.5);
    let alpha = edge * edge_opacity * ants;
    return vec4<f32>(overlay.color.rgb, clamp(alpha, 0.0, 1.0));
}
