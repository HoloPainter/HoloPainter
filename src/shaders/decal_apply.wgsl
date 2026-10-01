struct DecalApply {
    projector_view_proj: mat4x4<f32>,
    visibility_view_proj: mat4x4<f32>,
    center_depth: vec4<f32>,
    axis_x_half_width: vec4<f32>,
    axis_y_half_height: vec4<f32>,
    normal_opacity: vec4<f32>,
    params: vec4<f32>,
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> decal: DecalApply;
@group(0) @binding(1) var decal_image: texture_2d<f32>;
@group(0) @binding(2) var decal_sampler: sampler;
@group(0) @binding(3) var projector_depth: texture_2d<f32>;
@group(0) @binding(4) var layer_source: texture_2d<f32>;
@group(0) @binding(5) var selection_texture: texture_2d<f32>;

const BASE_DEPTH_EPSILON: f32 = 0.0005;
const MAX_DEPTH_EPSILON: f32 = 0.02;
const DEPTH_SLOPE_PIXEL_SCALE: f32 = 0.75;
const MIN_GEOMETRIC_NORMAL_SIN_SQ: f32 = 1e-8;

struct VSOut {
    @builtin(position) uv_clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) surface_uv: vec2<f32>,
    @location(3) projector_clip: vec4<f32>,
    @location(4) visibility_clip: vec4<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) edit_mask: f32,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> VSOut {
    var out: VSOut;
    out.uv_clip_pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    let world_position = vec4<f32>(position, 1.0);
    out.world_pos = position;
    out.world_normal = normalize(normal);
    out.surface_uv = uv;
    out.projector_clip = decal.projector_view_proj * world_position;
    if decal.flags.y != 0u {
        out.visibility_clip = decal.visibility_view_proj * world_position;
    } else {
        out.visibility_clip = out.projector_clip;
    }
    return out;
}

fn projected_depth_epsilon(ndc_z: f32, projector_px: vec2<f32>) -> f32 {
    let projector_px_dx = dpdx(projector_px);
    let projector_px_dy = dpdy(projector_px);
    let ndc_z_dx = dpdx(ndc_z);
    let ndc_z_dy = dpdy(ndc_z);
    let det = projector_px_dx.x * projector_px_dy.y - projector_px_dx.y * projector_px_dy.x;
    if abs(det) <= 1e-6 {
        return MAX_DEPTH_EPSILON;
    }

    let depth_grad = vec2<f32>(
        (ndc_z_dx * projector_px_dy.y - projector_px_dx.y * ndc_z_dy) / det,
        (projector_px_dx.x * ndc_z_dy - ndc_z_dx * projector_px_dy.x) / det,
    );
    return clamp(
        BASE_DEPTH_EPSILON + DEPTH_SLOPE_PIXEL_SCALE * (abs(depth_grad.x) + abs(depth_grad.y)),
        BASE_DEPTH_EPSILON,
        MAX_DEPTH_EPSILON,
    );
}

@fragment
fn fs_main(in: VSOut) -> FragmentOutput {
    let opacity = decal.normal_opacity.w;
    let depth_texture_size = max(decal.params.zw, vec2<f32>(1.0, 1.0));
    let view_projection = decal.flags.y != 0u;

    var surface_decal_uv = vec2<f32>(0.0, 0.0);
    if !view_projection {
        let center = decal.center_depth.xyz;
        let projection_depth = decal.center_depth.w;
        let axis_x = decal.axis_x_half_width.xyz;
        let axis_y = decal.axis_y_half_height.xyz;
        let half_width = decal.axis_x_half_width.w;
        let half_height = decal.axis_y_half_height.w;
        let decal_normal = decal.normal_opacity.xyz;
        let normal_threshold = decal.params.x;
        let front_projection_depth = decal.params.y;
        let delta = in.world_pos - center;
        let local_x = dot(delta, axis_x) / max(half_width, 1e-6);
        let local_y = dot(delta, axis_y) / max(half_height, 1e-6);
        let local_depth = -dot(delta, decal_normal);

        if abs(local_x) > 1.0 || abs(local_y) > 1.0 {
            discard;
        }
        if local_depth < -front_projection_depth || local_depth > projection_depth {
            discard;
        }

        let world_dx = dpdx(in.world_pos);
        let world_dy = dpdy(in.world_pos);
        let geometric_normal_cross = cross(world_dx, world_dy);
        let geometric_normal_length_sq = dot(geometric_normal_cross, geometric_normal_cross);
        let derivative_scale_sq = dot(world_dx, world_dx) * dot(world_dy, world_dy);
        let interpolated_normal_length_sq = dot(in.world_normal, in.world_normal);
        var geometric_normal = vec3<f32>(0.0);
        // The cross-product magnitude scales with world units squared. Compare it
        // with the derivative magnitudes so small meshes are not treated as degenerate.
        if (
            derivative_scale_sq > 0.0 &&
            geometric_normal_length_sq > derivative_scale_sq * MIN_GEOMETRIC_NORMAL_SIN_SQ
        ) {
            geometric_normal = geometric_normal_cross * inverseSqrt(geometric_normal_length_sq);
            if (
                interpolated_normal_length_sq > 1e-12 &&
                dot(geometric_normal, in.world_normal) < 0.0
            ) {
                geometric_normal = -geometric_normal;
            }
        } else {
            if interpolated_normal_length_sq <= 1e-12 {
                discard;
            }
            geometric_normal =
                in.world_normal * inverseSqrt(interpolated_normal_length_sq);
        }
        if dot(geometric_normal, decal_normal) <= normal_threshold {
            discard;
        }

        surface_decal_uv = vec2<f32>(
            clamp(local_x * 0.5 + 0.5, 0.0, 1.0),
            clamp(0.5 - local_y * 0.5, 0.0, 1.0),
        );
    }

    if in.projector_clip.w <= 1e-6 {
        discard;
    }
    let projector_ndc = in.projector_clip.xyz / in.projector_clip.w;
    if (
        projector_ndc.x < -1.0 || projector_ndc.x > 1.0 ||
        projector_ndc.y < -1.0 || projector_ndc.y > 1.0 ||
        projector_ndc.z < 0.0 || projector_ndc.z > 1.0
    ) {
        discard;
    }
    if view_projection {
        if in.visibility_clip.w <= 1e-6 {
            discard;
        }
        let visibility_ndc = in.visibility_clip.xyz / in.visibility_clip.w;
        if (
            visibility_ndc.x < -1.0 || visibility_ndc.x > 1.0 ||
            visibility_ndc.y < -1.0 || visibility_ndc.y > 1.0 ||
            visibility_ndc.z < 0.0 || visibility_ndc.z > 1.0
        ) {
            discard;
        }
    }

    let projector_px = vec2<f32>(
        (projector_ndc.x * 0.5 + 0.5) * depth_texture_size.x,
        (1.0 - (projector_ndc.y * 0.5 + 0.5)) * depth_texture_size.y,
    );
    let depth_epsilon = projected_depth_epsilon(projector_ndc.z, projector_px);
    let depth_ix = clamp(
        vec2<i32>(i32(floor(projector_px.x)), i32(floor(projector_px.y))),
        vec2<i32>(0, 0),
        vec2<i32>(i32(depth_texture_size.x), i32(depth_texture_size.y)) - vec2<i32>(1, 1),
    );
    let scene_depth = textureLoad(projector_depth, depth_ix, 0).r;
    if projector_ndc.z > scene_depth + depth_epsilon {
        discard;
    }

    var decal_uv = surface_decal_uv;
    if view_projection {
        decal_uv = vec2<f32>(
            clamp(projector_ndc.x * 0.5 + 0.5, 0.0, 1.0),
            clamp(0.5 - projector_ndc.y * 0.5, 0.0, 1.0),
        );
    }
    let sampled = textureSampleLevel(decal_image, decal_sampler, decal_uv, 0.0);
    let surface_uv = clamp(in.surface_uv, vec2<f32>(0.0), vec2<f32>(1.0));
    var selection = 1.0;
    if decal.flags.x != 0u {
        selection = textureSampleLevel(selection_texture, decal_sampler, surface_uv, 0.0).r;
    }
    if selection <= 1e-6 {
        discard;
    }
    let source_scale = clamp(opacity * selection, 0.0, 1.0);
    let source = sampled * source_scale;
    let destination = textureSampleLevel(layer_source, decal_sampler, surface_uv, 0.0);
    let one_minus_source_alpha = 1.0 - source.a;

    var out: FragmentOutput;
    out.color = vec4<f32>(
        source.rgb + destination.rgb * one_minus_source_alpha,
        source.a + destination.a * one_minus_source_alpha,
    );
    // Bleed must cover the whole valid projector footprint, including transparent
    // decal texels. Otherwise dilation skips transparent texels and can pull an
    // unrelated opaque decal color across a UV seam.
    out.edit_mask = selection;
    return out;
}
