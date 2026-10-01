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

struct LayerCompositeParams {
    opacity: f32,
    blend_mode: u32,
    mask_enabled: u32,
    source_kind: u32,
    solid_color: vec4<f32>,
    adjustment_meta: vec4<u32>,
    adjustment_params: array<vec4<f32>, 8>,
    output_uv_to_source_uv_row0: vec4<f32>,
    output_uv_to_source_uv_row1: vec4<f32>,
};

const SOURCE_TEXTURE: u32 = 0u;
const SOURCE_SOLID_COLOR: u32 = 1u;
const SOURCE_TRANSFORMED_TEXTURE: u32 = 3u;

@group(0) @binding(0) var layer_tex: texture_2d<f32>;
@group(0) @binding(1) var layer_smp: sampler;
@group(0) @binding(2) var<uniform> params: LayerCompositeParams;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;

fn load_premultiplied(pixel: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(layer_tex));
    if (any(pixel < vec2<i32>(0)) || any(pixel >= size)) {
        return vec4<f32>(0.0);
    }
    let straight = textureLoad(layer_tex, pixel, 0);
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}

fn sample_transformed(output_uv: vec2<f32>) -> vec4<f32> {
    let homogeneous = vec3<f32>(output_uv, 1.0);
    let uv = vec2<f32>(
        dot(params.output_uv_to_source_uv_row0.xyz, homogeneous),
        dot(params.output_uv_to_source_uv_row1.xyz, homogeneous),
    );
    let coordinate = uv * vec2<f32>(textureDimensions(layer_tex)) - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let top = mix(load_premultiplied(origin), load_premultiplied(origin + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(load_premultiplied(origin + vec2<i32>(0, 1)), load_premultiplied(origin + vec2<i32>(1, 1)), fraction.x);
    return mix(top, bottom, fraction.y);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    var src = textureSampleLevel(layer_tex, layer_smp, in.uv, 0.0);
    if (params.source_kind == SOURCE_SOLID_COLOR) {
        src = vec4<f32>(params.solid_color.rgb, 1.0);
    } else if (params.source_kind == SOURCE_TRANSFORMED_TEXTURE) {
        src = sample_transformed(in.uv);
    }
    var mask_alpha = 1.0;
    if (params.mask_enabled != 0u) {
        mask_alpha = clamp(textureSampleLevel(mask_tex, layer_smp, in.uv, 0.0).r, 0.0, 1.0);
    }
    let alpha_scale = params.opacity * mask_alpha;
    let alpha = clamp(src.a * alpha_scale, 0.0, 1.0);
    return vec4<f32>(src.rgb * alpha_scale, alpha);
}
