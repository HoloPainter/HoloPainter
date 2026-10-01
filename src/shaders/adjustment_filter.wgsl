const EPSILON: f32 = 0.00001;

const ADJUSTMENT_BRIGHTNESS_CONTRAST: u32 = 0u;
const ADJUSTMENT_LEVELS: u32 = 1u;
const ADJUSTMENT_HUE_SATURATION: u32 = 2u;
const ADJUSTMENT_CURVES: u32 = 3u;
const ADJUSTMENT_INVERT: u32 = 4u;
const TARGET_DOMAIN_SCALAR: u32 = 1u;

struct AdjustmentFilterUniform {
    adjustment_kind: u32,
    selection_enabled: u32,
    target_domain: u32,
    _pad: u32,
    adjustment_params: array<vec4<f32>, 8>,
};

struct AdjustmentLut {
    samples: array<vec4<f32>, 256>,
};

@group(0) @binding(0)
var source_image: texture_2d<f32>;

@group(0) @binding(1)
var selection_mask: texture_2d<f32>;

@group(0) @binding(2)
var destination_image: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(3)
var<uniform> params: AdjustmentFilterUniform;

@group(0) @binding(4)
var<uniform> adjustment_lut: AdjustmentLut;

fn unpremultiply(color: vec4<f32>) -> vec3<f32> {
    if color.a <= EPSILON {
        return vec3<f32>(0.0);
    }
    return clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0));
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
    let base = select(
        select(select(vec3<f32>(chroma, x, 0.0), vec3<f32>(x, chroma, 0.0), sector == 1u), select(vec3<f32>(0.0, chroma, x), vec3<f32>(0.0, x, chroma), sector == 3u), sector >= 2u),
        select(vec3<f32>(x, 0.0, chroma), vec3<f32>(chroma, 0.0, x), sector == 5u),
        sector >= 4u
    );
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

fn apply_curves(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        sample_adjustment_lut(color.r).r,
        sample_adjustment_lut(color.g).g,
        sample_adjustment_lut(color.b).b
    );
}

fn apply_adjustment(color: vec3<f32>) -> vec3<f32> {
    if params.adjustment_kind == ADJUSTMENT_BRIGHTNESS_CONTRAST {
        return apply_curves(color);
    }
    if params.adjustment_kind == ADJUSTMENT_LEVELS {
        return apply_curves(color);
    }
    if params.adjustment_kind == ADJUSTMENT_HUE_SATURATION {
        return apply_hsl(color);
    }
    if params.adjustment_kind == ADJUSTMENT_CURVES {
        return apply_curves(color);
    }
    if params.adjustment_kind == ADJUSTMENT_INVERT {
        return vec3<f32>(1.0) - color;
    }
    return color;
}

fn apply_scalar(value: f32) -> f32 {
    if params.adjustment_kind == ADJUSTMENT_BRIGHTNESS_CONTRAST {
        return sample_adjustment_lut(value).r;
    }
    if params.adjustment_kind == ADJUSTMENT_LEVELS {
        return sample_adjustment_lut(value).r;
    }
    if params.adjustment_kind == ADJUSTMENT_CURVES {
        return sample_adjustment_lut(value).r;
    }
    if params.adjustment_kind == ADJUSTMENT_INVERT {
        return 1.0 - value;
    }
    return value;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let size = textureDimensions(source_image, 0);
    if global_id.x >= size.x || global_id.y >= size.y {
        return;
    }
    let pixel = vec2<i32>(global_id.xy);
    let raw = textureLoad(source_image, pixel, 0);
    var amount = 1.0;
    if params.selection_enabled != 0u {
        amount = clamp(textureLoad(selection_mask, pixel, 0).r, 0.0, 1.0);
    }
    if params.target_domain == TARGET_DOMAIN_SCALAR {
        let original = clamp(raw.a, 0.0, 1.0);
        let filtered = apply_scalar(original);
        let result = mix(original, filtered, amount);
        textureStore(destination_image, pixel, vec4<f32>(result));
        return;
    }
    let straight = unpremultiply(raw);
    let adjusted = apply_adjustment(straight);
    let result = mix(straight, adjusted, amount);
    textureStore(destination_image, pixel, vec4<f32>(result * raw.a, raw.a));
}
