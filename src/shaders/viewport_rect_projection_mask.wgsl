struct ProjectionMask {
    view_proj: mat4x4<f32>,
    rect_min_max: vec4<f32>,
    viewport_texture_size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> mask_u: ProjectionMask;
@group(0) @binding(1) var viewport_depth: texture_2d<f32>;

const BASE_DEPTH_EPSILON: f32 = 0.0005;
const MAX_DEPTH_EPSILON: f32 = 0.02;
const DEPTH_SLOPE_PIXEL_SCALE: f32 = 0.75;

struct VSOut {
    @builtin(position) uv_clip_pos: vec4<f32>,
    @location(0) world_clip_pos: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> VSOut {
    var out: VSOut;
    out.uv_clip_pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.world_clip_pos = mask_u.view_proj * vec4<f32>(position, 1.0);
    return out;
}

fn projected_depth_epsilon(ndc_z: f32, viewport_px: vec2<f32>) -> f32 {
    let viewport_px_dx = dpdx(viewport_px);
    let viewport_px_dy = dpdy(viewport_px);
    let ndc_z_dx = dpdx(ndc_z);
    let ndc_z_dy = dpdy(ndc_z);
    let det = viewport_px_dx.x * viewport_px_dy.y - viewport_px_dx.y * viewport_px_dy.x;
    if (abs(det) <= 1e-6) {
        return MAX_DEPTH_EPSILON;
    }

    let depth_grad = vec2<f32>(
        (ndc_z_dx * viewport_px_dy.y - viewport_px_dx.y * ndc_z_dy) / det,
        (viewport_px_dx.x * ndc_z_dy - ndc_z_dx * viewport_px_dy.x) / det
    );
    return clamp(
        BASE_DEPTH_EPSILON + DEPTH_SLOPE_PIXEL_SCALE * (abs(depth_grad.x) + abs(depth_grad.y)),
        BASE_DEPTH_EPSILON,
        MAX_DEPTH_EPSILON
    );
}

@fragment
fn fs_main(in: VSOut) -> @location(0) f32 {
    let safe_w = select(
        in.world_clip_pos.w,
        select(-1e-6, 1e-6, in.world_clip_pos.w >= 0.0),
        abs(in.world_clip_pos.w) <= 1e-6
    );
    let ndc = in.world_clip_pos.xyz / safe_w;
    let viewport_size = max(mask_u.viewport_texture_size.xy, vec2<f32>(1.0, 1.0));
    let viewport_px = vec2<f32>(
        (ndc.x * 0.5 + 0.5) * viewport_size.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size.y
    );
    let depth_epsilon = projected_depth_epsilon(ndc.z, viewport_px);
    let min_px = min(mask_u.rect_min_max.xy, mask_u.rect_min_max.zw);
    let max_px = max(mask_u.rect_min_max.xy, mask_u.rect_min_max.zw);
    if (
        viewport_px.x < min_px.x ||
        viewport_px.y < min_px.y ||
        viewport_px.x > max_px.x ||
        viewport_px.y > max_px.y ||
        viewport_px.x < 0.0 ||
        viewport_px.y < 0.0 ||
        viewport_px.x >= viewport_size.x ||
        viewport_px.y >= viewport_size.y ||
        in.world_clip_pos.w <= 1e-6 ||
        ndc.z < 0.0 ||
        ndc.z > 1.0
    ) {
        discard;
    }

    let depth_ix = clamp(
        vec2<i32>(i32(floor(viewport_px.x)), i32(floor(viewport_px.y))),
        vec2<i32>(0, 0),
        vec2<i32>(i32(viewport_size.x), i32(viewport_size.y)) - vec2<i32>(1, 1)
    );
    let scene_depth = textureLoad(viewport_depth, depth_ix, 0).r;
    if (ndc.z > scene_depth + depth_epsilon) {
        discard;
    }

    return 1.0;
}
