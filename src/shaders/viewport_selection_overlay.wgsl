struct ViewProj {
    view_proj: mat4x4<f32>,
};

struct SelectionOverlay {
    view_size: vec2<f32>,
    texture_size: vec2<f32>,
    params: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> vp: ViewProj;
@group(0) @binding(1) var selection_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> overlay: SelectionOverlay;

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) mesh_uv: vec2<f32>,
    @location(1) screen_pos: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> VSOut {
    var out: VSOut;
    out.clip_pos = vp.view_proj * vec4<f32>(position, 1.0);
    out.mesh_uv = uv;
    let ndc = out.clip_pos.xy / max(out.clip_pos.w, 1e-6);
    out.screen_pos = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5)) * overlay.view_size;
    return out;
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

fn safe_screen_uv_delta(delta: vec2<f32>, fallback: vec2<f32>) -> vec2<f32> {
    if dot(delta, delta) <= 1e-12 {
        return fallback;
    }
    return delta;
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
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let texture_size = max(overlay.texture_size, vec2<f32>(1.0, 1.0));
    let uv_dx = safe_screen_uv_delta(dpdx(in.mesh_uv), vec2<f32>(1.0 / texture_size.x, 0.0));
    let uv_dy = safe_screen_uv_delta(dpdy(in.mesh_uv), vec2<f32>(0.0, 1.0 / texture_size.y));
    let phase = overlay.params.x;
    let edge_opacity = overlay.params.z;
    let edge = edge_strength(in.mesh_uv, uv_dx, uv_dy);
    if edge <= 0.001 {
        discard;
    }

    let ants = select(0.55, 1.0, fract((in.screen_pos.x + in.screen_pos.y) * 0.12 + phase * 1.75) > 0.5);
    let alpha = edge * edge_opacity * ants;
    return vec4<f32>(overlay.color.rgb, clamp(alpha, 0.0, 1.0));
}
