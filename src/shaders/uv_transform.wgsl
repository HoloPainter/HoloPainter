struct TransformUniform {
    inverse_row0: vec4<f32>,
    inverse_row1: vec4<f32>,
    source_min: vec2<f32>,
    source_max: vec2<f32>,
    selection_enabled: u32,
    clear_original: u32,
    _pad0: vec2<u32>,
};

@group(0) @binding(0) var base_surface: texture_2d<f32>;
@group(0) @binding(1) var transform_source: texture_2d<f32>;
@group(0) @binding(2) var source_selection: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: TransformUniform;

@vertex
fn vs_fullscreen_triangle(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

fn source_contains(pixel: vec2<i32>) -> bool {
    let source_min = vec2<i32>(params.source_min);
    let source_max = vec2<i32>(params.source_max);
    return all(pixel >= source_min) && all(pixel < source_max);
}

fn load_selection(pixel: vec2<i32>) -> f32 {
    if (!source_contains(pixel)) {
        return 0.0;
    }
    if (params.selection_enabled == 0u) {
        return 1.0;
    }
    return clamp(textureLoad(source_selection, pixel, 0).r, 0.0, 1.0);
}

fn load_selected_surface(pixel: vec2<i32>) -> vec4<f32> {
    let selection = load_selection(pixel);
    if (selection <= 0.0) {
        return vec4<f32>(0.0);
    }
    return textureLoad(transform_source, pixel, 0) * selection;
}

fn bilinear_surface(source_center: vec2<f32>) -> vec4<f32> {
    let coordinate = source_center - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let top = mix(
        load_selected_surface(origin),
        load_selected_surface(origin + vec2<i32>(1, 0)),
        fraction.x,
    );
    let bottom = mix(
        load_selected_surface(origin + vec2<i32>(0, 1)),
        load_selected_surface(origin + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}

fn bilinear_selection(source_center: vec2<f32>) -> f32 {
    let coordinate = source_center - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let top = mix(
        load_selection(origin),
        load_selection(origin + vec2<i32>(1, 0)),
        fraction.x,
    );
    let bottom = mix(
        load_selection(origin + vec2<i32>(0, 1)),
        load_selection(origin + vec2<i32>(1, 1)),
        fraction.x,
    );
    return clamp(mix(top, bottom, fraction.y), 0.0, 1.0);
}

fn inverse_transform(position: vec2<f32>) -> vec2<f32> {
    let homogeneous = vec3<f32>(position, 1.0);
    return vec2<f32>(
        dot(params.inverse_row0.xyz, homogeneous),
        dot(params.inverse_row1.xyz, homogeneous),
    );
}

@fragment
fn fs_surface(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let original = textureLoad(base_surface, pixel, 0);
    var cleared = original;
    if (params.clear_original != 0u) {
        cleared = original * (1.0 - load_selection(pixel));
    }
    let moved = bilinear_surface(inverse_transform(position.xy));
    let inverse_alpha = 1.0 - clamp(moved.a, 0.0, 1.0);
    return vec4<f32>(
        moved.rgb + cleared.rgb * inverse_alpha,
        moved.a + cleared.a * inverse_alpha,
    );
}

@fragment
fn fs_selection(@builtin(position) position: vec4<f32>) -> @location(0) f32 {
    let pixel = vec2<i32>(position.xy);
    let original = textureLoad(source_selection, pixel, 0).r;
    let cleared = select(original, 0.0, source_contains(pixel));
    let moved = bilinear_selection(inverse_transform(position.xy));
    return max(cleared, moved);
}
