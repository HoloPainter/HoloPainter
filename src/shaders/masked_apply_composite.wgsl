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
    out.clip_pos = vec4<f32>(quad[i], 0.0, 1.0);
    out.uv = vec2<f32>(quad[i].x * 0.5 + 0.5, 1.0 - (quad[i].y * 0.5 + 0.5));
    return out;
}

@group(0) @binding(0) var stroke_tex: texture_2d<f32>;
@group(0) @binding(1) var stroke_smp: sampler;
@group(0) @binding(2) var erase_tex: texture_2d<f32>;
@group(0) @binding(4) var selection_tex: texture_2d<f32>;

const UV_PAINT_COMPOSITE_MODE_SOURCE_OVER: u32 = 0u;
const UV_PAINT_COMPOSITE_MODE_DESTINATION_OUT: u32 = 1u;
const UV_PAINT_COMPOSITE_MODE_CLEAR: u32 = 2u;
const UV_PAINT_COMPOSITE_MODE_MULTIPLY: u32 = 3u;
const EPSILON: f32 = 0.00001;

struct CompositeParams {
    paint_rgb_opacity: vec4<f32>,
    composite_mode: u32,
    selection_enabled: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(3) var<uniform> params: CompositeParams;

fn unpremultiply(color: vec4<f32>) -> vec3<f32> {
    if (color.a <= EPSILON) {
        return vec3<f32>(0.0);
    }
    return clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let stroke_accum = textureSampleLevel(stroke_tex, stroke_smp, in.uv, 0.0).r;
    let selection_mask = textureSampleLevel(selection_tex, stroke_smp, in.uv, 0.0).r;
    let mask = stroke_accum * select(1.0, selection_mask, params.selection_enabled != 0u);
    let erase = textureSampleLevel(erase_tex, stroke_smp, in.uv, 0.0);
    let blend_alpha = clamp(mask * params.paint_rgb_opacity.w, 0.0, 1.0);

    if (params.composite_mode == UV_PAINT_COMPOSITE_MODE_SOURCE_OVER) {
        let rgb = mix(erase.rgb, params.paint_rgb_opacity.rgb, blend_alpha);
        let alpha = mix(erase.a, 1.0, blend_alpha);
        return vec4<f32>(rgb, alpha);
    }

    if (params.composite_mode == UV_PAINT_COMPOSITE_MODE_CLEAR) {
        let cleared_alpha = erase.a * (1.0 - blend_alpha);
        let rgb_scale = select(cleared_alpha / erase.a, 0.0, erase.a <= 1e-5);
        let rgb = erase.rgb * rgb_scale;
        return vec4<f32>(rgb, cleared_alpha);
    }

    if (params.composite_mode == UV_PAINT_COMPOSITE_MODE_MULTIPLY) {
        let sa = blend_alpha;
        let da = clamp(erase.a, 0.0, 1.0);
        let src = clamp(params.paint_rgb_opacity.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        let dst = unpremultiply(erase);
        let blended = src * dst;
        let out_a = sa + da * (1.0 - sa);
        let out_rgb = blended * sa * da
            + src * sa * (1.0 - da)
            + dst * da * (1.0 - sa);
        return vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
    }

    let erased_alpha = clamp(erase.a - blend_alpha, 0.0, 1.0);
    let rgb_scale = select(erased_alpha / erase.a, 0.0, erase.a <= 1e-5);
    let rgb = erase.rgb * rgb_scale;
    return vec4<f32>(rgb, erased_alpha);
}
