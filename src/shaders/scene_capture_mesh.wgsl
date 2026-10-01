struct ViewProj {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> vp: ViewProj;
@group(0) @binding(1) var paint_tex: texture_2d<f32>;
@group(0) @binding(2) var paint_smp: sampler;

struct ViewportParams {
    camera_world: vec4<f32>,
    shading: vec4<f32>,
};
@group(0) @binding(3) var<uniform> params: ViewportParams;

struct MaterialPresentationParams {
    alpha_cutoff: f32,
    _padding0: f32,
    _padding1: f32,
    _padding2: f32,
};
@group(0) @binding(4) var<uniform> material: MaterialPresentationParams;

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) mesh_uv: vec2<f32>,
    @location(1) viewport_uv: vec2<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) world_normal: vec3<f32>,
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
    out.viewport_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
    out.world_pos = position;
    out.world_normal = normalize(normal);
    return out;
}

fn sample_material(in: VSOut) -> vec4<f32> {
    return textureSampleLevel(paint_tex, paint_smp, in.mesh_uv, 0.0);
}

fn unpremultiply_rgb(src: vec4<f32>) -> vec3<f32> {
    if src.a <= 1e-5 {
        return vec3<f32>(0.0);
    }
    return src.rgb / src.a;
}

const SKY_INTENSITY: f32 = 1.0;
const GROUND_INTENSITY: f32 = 0.55;

fn presentation_rgb(rgb: vec3<f32>, world_normal: vec3<f32>) -> vec3<f32> {
    if params.shading.x < 0.5 {
        return rgb;
    }
    let hemi = normalize(world_normal).y * 0.5 + 0.5;
    let shade = mix(GROUND_INTENSITY, SKY_INTENSITY, hemi);
    return rgb * shade;
}

@fragment
fn fs_presentation_opaque(in: VSOut) -> @location(0) vec4<f32> {
    let src = sample_material(in);
    return vec4<f32>(presentation_rgb(unpremultiply_rgb(src), in.world_normal), 1.0);
}

@fragment
fn fs_presentation_cutoff(in: VSOut) -> @location(0) vec4<f32> {
    let src = sample_material(in);
    if src.a < material.alpha_cutoff {
        discard;
    }
    return vec4<f32>(presentation_rgb(unpremultiply_rgb(src), in.world_normal), 1.0);
}

@fragment
fn fs_presentation_blend(in: VSOut) -> @location(0) vec4<f32> {
    let src = sample_material(in);
    return vec4<f32>(presentation_rgb(src.rgb, in.world_normal), src.a);
}

@fragment
fn fs_surface_source(in: VSOut) -> @location(0) vec4<f32> {
    return sample_material(in);
}


