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

const SELECTION_COMPOSITE_REPLACE: u32 = 0u;
const SELECTION_COMPOSITE_ADD: u32 = 1u;
const SELECTION_COMPOSITE_SUBTRACT: u32 = 2u;
const SELECTION_COMPOSITE_INTERSECT: u32 = 3u;
const SELECTION_COMPOSITE_DIFFERENCE: u32 = 4u;
const SELECTION_COMPOSITE_CLEAR: u32 = 5u;
const SELECTION_COMPOSITE_INVERT: u32 = 6u;

struct SelectionCompositeParams {
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: SelectionCompositeParams;

const PROJECTION_SELECTION_DILATION_RADIUS: i32 = 1;

fn dilated_overlay_at(coord: vec2<i32>, max_coord: vec2<i32>) -> f32 {
    // The 3D viewport-selection path feeds this shader with the same projection
    // mask used by viewport paint. Paint then runs UV-island bleed
    // so seams do not expose unpainted border texels; mirror that behavior for
    // selection by dilating the projected mask before compositing it into the
    // semantic selection mask. The shader is only used by the GPU projection
    // path, so UV-view shape selection keeps its exact CPU mask.
    var value = 0.0;
    for (var y = -PROJECTION_SELECTION_DILATION_RADIUS; y <= PROJECTION_SELECTION_DILATION_RADIUS; y = y + 1) {
        for (var x = -PROJECTION_SELECTION_DILATION_RADIUS; x <= PROJECTION_SELECTION_DILATION_RADIUS; x = x + 1) {
            let sample_coord = clamp(coord + vec2<i32>(x, y), vec2<i32>(0, 0), max_coord);
            value = max(value, textureLoad(overlay_tex, sample_coord, 0).r);
        }
    }
    return value;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) f32 {
    let dims = vec2<i32>(textureDimensions(base_tex));
    let max_coord = max(dims - vec2<i32>(1, 1), vec2<i32>(0, 0));
    let coord = clamp(vec2<i32>(floor(in.uv * vec2<f32>(dims))), vec2<i32>(0, 0), max_coord);
    let base = textureLoad(base_tex, coord, 0).r;
    let overlay = dilated_overlay_at(coord, max_coord);

    if (params.mode == SELECTION_COMPOSITE_REPLACE) {
        return overlay;
    }
    if (params.mode == SELECTION_COMPOSITE_ADD) {
        return max(base, overlay);
    }
    if (params.mode == SELECTION_COMPOSITE_SUBTRACT) {
        return base * (1.0 - overlay);
    }
    if (params.mode == SELECTION_COMPOSITE_INTERSECT) {
        return min(base, overlay);
    }
    if (params.mode == SELECTION_COMPOSITE_DIFFERENCE) {
        return abs(base - overlay);
    }
    if (params.mode == SELECTION_COMPOSITE_CLEAR) {
        return 0.0;
    }
    if (params.mode == SELECTION_COMPOSITE_INVERT) {
        return 1.0 - base;
    }
    return base;
}
