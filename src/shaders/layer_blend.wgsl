const BLEND_NORMAL: u32 = 0u;
const BLEND_DARKEN: u32 = 1u;
const BLEND_MULTIPLY: u32 = 2u;
const BLEND_LIGHTEN: u32 = 3u;
const BLEND_SCREEN: u32 = 4u;
const BLEND_COLOR_DODGE: u32 = 5u;
const BLEND_LINEAR_DODGE: u32 = 6u;
const BLEND_OVERLAY: u32 = 7u;
const BLEND_SOFT_LIGHT: u32 = 8u;
const BLEND_HARD_LIGHT: u32 = 9u;
const BLEND_COLOR: u32 = 10u;
const EPSILON: f32 = 0.00001;

const SOURCE_TEXTURE: u32 = 0u;
const SOURCE_SOLID_COLOR: u32 = 1u;
const SOURCE_ADJUSTMENT: u32 = 2u;
const SOURCE_TRANSFORMED_TEXTURE: u32 = 3u;

const ADJUSTMENT_BRIGHTNESS_CONTRAST: u32 = 0u;
const ADJUSTMENT_LEVELS: u32 = 1u;
const ADJUSTMENT_HUE_SATURATION: u32 = 2u;
const ADJUSTMENT_CURVES: u32 = 3u;
const ADJUSTMENT_INVERT: u32 = 4u;
const ADJUSTMENT_GRADIENT_MAP: u32 = 5u;
const ADJUSTMENT_UV_MIRROR: u32 = 6u;

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

struct LayerBlendParams {
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

struct AdjustmentLut {
    samples: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var backdrop_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_smp: sampler;
@group(0) @binding(3) var<uniform> params: LayerBlendParams;
@group(0) @binding(4) var mask_tex: texture_2d<f32>;
@group(0) @binding(5) var<uniform> adjustment_lut: AdjustmentLut;

fn load_premultiplied_source(pixel: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source_tex));
    if (any(pixel < vec2<i32>(0)) || any(pixel >= size)) {
        return vec4<f32>(0.0);
    }
    let straight = textureLoad(source_tex, pixel, 0);
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}

fn sample_transformed_source(output_uv: vec2<f32>) -> vec4<f32> {
    let homogeneous = vec3<f32>(output_uv, 1.0);
    let uv = vec2<f32>(
        dot(params.output_uv_to_source_uv_row0.xyz, homogeneous),
        dot(params.output_uv_to_source_uv_row1.xyz, homogeneous),
    );
    let coordinate = uv * vec2<f32>(textureDimensions(source_tex)) - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let top = mix(load_premultiplied_source(origin), load_premultiplied_source(origin + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(load_premultiplied_source(origin + vec2<i32>(0, 1)), load_premultiplied_source(origin + vec2<i32>(1, 1)), fraction.x);
    return mix(top, bottom, fraction.y);
}

fn unpremultiply(color: vec4<f32>) -> vec3<f32> {
    if color.a <= EPSILON {
        return vec3<f32>(0.0);
    }
    return clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn blend_overlay(source: vec3<f32>, backdrop: vec3<f32>) -> vec3<f32> {
    let low = 2.0 * source * backdrop;
    let high = 1.0 - 2.0 * (1.0 - source) * (1.0 - backdrop);
    return select(high, low, backdrop <= vec3<f32>(0.5));
}

fn blend_soft_light(source: vec3<f32>, backdrop: vec3<f32>) -> vec3<f32> {
    let low = backdrop - (vec3<f32>(1.0) - 2.0 * source) * backdrop * (vec3<f32>(1.0) - backdrop);
    let backdrop_curve = select(
        sqrt(backdrop),
        ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop,
        backdrop <= vec3<f32>(0.25),
    );
    let high = backdrop + (2.0 * source - vec3<f32>(1.0)) * (backdrop_curve - backdrop);
    return select(high, low, source <= vec3<f32>(0.5));
}

fn blend_hard_light(source: vec3<f32>, backdrop: vec3<f32>) -> vec3<f32> {
    let low = 2.0 * source * backdrop;
    let high = 1.0 - 2.0 * (1.0 - source) * (1.0 - backdrop);
    return select(high, low, source <= vec3<f32>(0.5));
}

fn blend_color_dodge(source: vec3<f32>, backdrop: vec3<f32>) -> vec3<f32> {
    let dodged = min(vec3<f32>(1.0), backdrop / max(vec3<f32>(EPSILON), vec3<f32>(1.0) - source));
    let source_white = select(dodged, vec3<f32>(1.0), source >= vec3<f32>(1.0 - EPSILON));
    return select(source_white, vec3<f32>(0.0), backdrop <= vec3<f32>(EPSILON));
}

fn blend_lum(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.3, 0.59, 0.11));
}

fn blend_clip_color(color: vec3<f32>) -> vec3<f32> {
    let lum = blend_lum(color);
    let minimum = min(color.r, min(color.g, color.b));
    let maximum = max(color.r, max(color.g, color.b));
    var clipped = color;
    if minimum < 0.0 {
        clipped = vec3<f32>(lum)
            + ((clipped - vec3<f32>(lum)) * lum) / max(lum - minimum, EPSILON);
    }
    if maximum > 1.0 {
        clipped = vec3<f32>(lum)
            + ((clipped - vec3<f32>(lum)) * (1.0 - lum)) / max(maximum - lum, EPSILON);
    }
    return clipped;
}

fn blend_set_lum(color: vec3<f32>, lum: f32) -> vec3<f32> {
    let adjusted = color + vec3<f32>(lum - blend_lum(color));
    return blend_clip_color(adjusted);
}

fn blend_color(source: vec3<f32>, backdrop: vec3<f32>) -> vec3<f32> {
    return blend_set_lum(source, blend_lum(backdrop));
}

fn blend_rgb(source: vec3<f32>, backdrop: vec3<f32>, mode: u32) -> vec3<f32> {
    if mode == BLEND_DARKEN {
        return min(source, backdrop);
    }
    if mode == BLEND_MULTIPLY {
        return source * backdrop;
    }
    if mode == BLEND_LIGHTEN {
        return max(source, backdrop);
    }
    if mode == BLEND_SCREEN {
        return source + backdrop - source * backdrop;
    }
    if mode == BLEND_COLOR_DODGE {
        return blend_color_dodge(source, backdrop);
    }
    if mode == BLEND_LINEAR_DODGE {
        return min(vec3<f32>(1.0), source + backdrop);
    }
    if mode == BLEND_OVERLAY {
        return blend_overlay(source, backdrop);
    }
    if mode == BLEND_SOFT_LIGHT {
        return blend_soft_light(source, backdrop);
    }
    if mode == BLEND_HARD_LIGHT {
        return blend_hard_light(source, backdrop);
    }
    if mode == BLEND_COLOR {
        return blend_color(source, backdrop);
    }
    return source;
}

fn rgb_to_hsl(color: vec3<f32>) -> vec3<f32> {
    let max_value = max(color.r, max(color.g, color.b));
    let min_value = min(color.r, min(color.g, color.b));
    let delta = max_value - min_value;
    var hue = 0.0;
    if delta > EPSILON {
        if max_value == color.r {
            hue = (color.g - color.b) / delta;
            if hue < 0.0 {
                hue = hue + 6.0;
            }
        } else if max_value == color.g {
            hue = (color.b - color.r) / delta + 2.0;
        } else {
            hue = (color.r - color.g) / delta + 4.0;
        }
        hue = hue / 6.0;
    }
    let lightness = (max_value + min_value) * 0.5;
    let denominator = 1.0 - abs(2.0 * lightness - 1.0);
    let saturation = select(0.0, delta / max(denominator, EPSILON), delta > EPSILON);
    return vec3<f32>(hue, saturation, lightness);
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    let hue6 = fract(hsl.x) * 6.0;
    let sector = u32(floor(hue6)) % 6u;
    let fraction = hue6 - floor(hue6);
    let chroma = (1.0 - abs(2.0 * hsl.z - 1.0)) * hsl.y;
    let x = chroma * (1.0 - abs((hue6 % 2.0) - 1.0));
    var base = vec3<f32>(chroma, x, 0.0);
    if sector == 1u { base = vec3<f32>(x, chroma, 0.0); }
    if sector == 2u { base = vec3<f32>(0.0, chroma, x); }
    if sector == 3u { base = vec3<f32>(0.0, x, chroma); }
    if sector == 4u { base = vec3<f32>(x, 0.0, chroma); }
    if sector == 5u { base = vec3<f32>(chroma, 0.0, x); }
    return base + vec3<f32>(hsl.z - chroma * 0.5);
}

fn apply_hsl(color: vec3<f32>) -> vec3<f32> {
    let lightness_delta = params.adjustment_params[0].z;
    let lightness_adjusted = select(
        color * (1.0 + lightness_delta),
        color * (1.0 - lightness_delta) + vec3<f32>(lightness_delta),
        lightness_delta >= 0.0
    );
    var hsl = rgb_to_hsl(lightness_adjusted);
    hsl.x = fract(hsl.x + params.adjustment_params[0].x + 1.0);
    let saturation_delta = params.adjustment_params[0].y;
    if hsl.y > EPSILON {
        hsl.y = select(
            hsl.y * (1.0 + saturation_delta),
            hsl.y / (1.0 - saturation_delta + 0.01),
            saturation_delta > 0.0
        );
    }
    return clamp(hsl_to_rgb(clamp(hsl, vec3<f32>(0.0), vec3<f32>(1.0))), vec3<f32>(0.0), vec3<f32>(1.0));
}


fn sample_adjustment_lut(value: f32) -> vec4<f32> {
    let index = u32(floor(clamp(value, 0.0, 1.0) * 255.0));
    return adjustment_lut.samples[index];
}

fn sample_gradient_lut(value: f32) -> vec4<f32> {
    let scaled = clamp(value, 0.0, 1.0) * 255.0;
    let lower_index = u32(floor(scaled));
    let upper_index = min(lower_index + 1u, 255u);
    return mix(
        adjustment_lut.samples[lower_index],
        adjustment_lut.samples[upper_index],
        fract(scaled)
    );
}

fn apply_curves(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        sample_adjustment_lut(color.r).r,
        sample_adjustment_lut(color.g).g,
        sample_adjustment_lut(color.b).b
    );
}

fn apply_invert(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - color;
}

fn apply_gradient_map(color: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    var intensity = dot(color, vec3<f32>(77.0, 151.0, 28.0) / 256.0);
    if params.adjustment_params[0].x > 0.5 {
        // The PSD dither flag is preserved, but Adobe does not publish the noise pattern.
        let noise = fract(sin(dot(pixel, vec2<f32>(12.9898, 78.233))) * 43758.5453) - 0.5;
        intensity = intensity + noise / 255.0;
    }
    return sample_gradient_lut(intensity).rgb;
}

fn apply_uv_mirror(uv: vec2<f32>, raw_dst: vec4<f32>, opacity: f32) -> vec4<f32> {
    let axis = params.adjustment_params[0].x;
    let requested_position = params.adjustment_params[0].y;
    let direction = params.adjustment_params[0].z;
    let dimensions = textureDimensions(backdrop_tex, 0);
    let extent = max(select(f32(dimensions.x), f32(dimensions.y), axis >= 0.5), 1.0);
    // Snap to half-texel increments so reflection maps texel centers exactly to
    // texel centers. This keeps partial-composite mirror dependencies exact and
    // avoids sampling uninitialized pixels just outside a rebuilt scratch clip.
    let position = round(clamp(requested_position, 0.0, 1.0) * extent * 2.0) / (extent * 2.0);
    let coordinate = select(uv.x, uv.y, axis >= 0.5);
    let source_side = select(coordinate >= position, coordinate <= position, direction >= 0.5);
    if source_side {
        return raw_dst;
    }

    let reflected = 2.0 * position - coordinate;
    if reflected < 0.0 || reflected > 1.0 {
        return raw_dst;
    }

    var mirror_uv = uv;
    if axis < 0.5 {
        mirror_uv.x = reflected;
    } else {
        mirror_uv.y = reflected;
    }
    let mirrored = textureSampleLevel(backdrop_tex, layer_smp, mirror_uv, 0.0);
    return mix(raw_dst, mirrored, opacity);
}

fn apply_adjustment(color: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    if params.adjustment_meta.x == ADJUSTMENT_BRIGHTNESS_CONTRAST {
        return apply_curves(color);
    }
    if params.adjustment_meta.x == ADJUSTMENT_LEVELS {
        return apply_curves(color);
    }
    if params.adjustment_meta.x == ADJUSTMENT_HUE_SATURATION {
        return apply_hsl(color);
    }
    if params.adjustment_meta.x == ADJUSTMENT_CURVES {
        return apply_curves(color);
    }
    if params.adjustment_meta.x == ADJUSTMENT_INVERT {
        return apply_invert(color);
    }
    if params.adjustment_meta.x == ADJUSTMENT_GRADIENT_MAP {
        return apply_gradient_map(color, pixel);
    }
    return color;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let raw_dst = textureSampleLevel(backdrop_tex, layer_smp, in.uv, 0.0);
    var mask_alpha = 1.0;
    if (params.mask_enabled != 0u) {
        mask_alpha = clamp(textureSampleLevel(mask_tex, layer_smp, in.uv, 0.0).r, 0.0, 1.0);
    }
    let opacity = clamp(params.opacity * mask_alpha, 0.0, 1.0);

    if params.source_kind == SOURCE_ADJUSTMENT {
        if params.adjustment_meta.x == ADJUSTMENT_UV_MIRROR {
            return apply_uv_mirror(in.uv, raw_dst, opacity);
        }
        let dst = unpremultiply(raw_dst);
        let adjusted = apply_adjustment(dst, in.clip_pos.xy);
        let blended = clamp(blend_rgb(adjusted, dst, params.blend_mode), vec3<f32>(0.0), vec3<f32>(1.0));
        let output = mix(dst, blended, opacity);
        return vec4<f32>(output * raw_dst.a, raw_dst.a);
    }

    var raw_src = textureSampleLevel(source_tex, layer_smp, in.uv, 0.0);
    if (params.source_kind == SOURCE_SOLID_COLOR) {
        raw_src = vec4<f32>(params.solid_color.rgb, 1.0);
    } else if (params.source_kind == SOURCE_TRANSFORMED_TEXTURE) {
        raw_src = sample_transformed_source(in.uv);
    }

    let sa = clamp(raw_src.a * opacity, 0.0, 1.0);
    let da = clamp(raw_dst.a, 0.0, 1.0);
    let src = unpremultiply(raw_src);
    let dst = unpremultiply(raw_dst);
    let blended = clamp(blend_rgb(src, dst, params.blend_mode), vec3<f32>(0.0), vec3<f32>(1.0));
    let out_a = sa + da * (1.0 - sa);
    let out_rgb = blended * sa * da
        + src * sa * (1.0 - da)
        + dst * da * (1.0 - sa);
    return vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
