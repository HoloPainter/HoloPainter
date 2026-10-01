struct RasterizeParams {
    output_uv_to_source_uv_row0: vec4<f32>,
    output_uv_to_source_uv_row1: vec4<f32>,
};

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: RasterizeParams;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VSOut {
    var triangle = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    let pos = triangle[i];
    var out: VSOut;
    out.clip_pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = vec2<f32>(pos.x * 0.5 + 0.5, 1.0 - (pos.y * 0.5 + 0.5));
    return out;
}

fn load_premultiplied(pixel: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source_tex));
    if (any(pixel < vec2<i32>(0)) || any(pixel >= size)) {
        return vec4<f32>(0.0);
    }
    let straight = textureLoad(source_tex, pixel, 0);
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}

fn sample_premultiplied_bilinear(uv: vec2<f32>) -> vec4<f32> {
    let coordinate = uv * vec2<f32>(textureDimensions(source_tex)) - vec2<f32>(0.5);
    let origin = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let top = mix(
        load_premultiplied(origin),
        load_premultiplied(origin + vec2<i32>(1, 0)),
        fraction.x,
    );
    let bottom = mix(
        load_premultiplied(origin + vec2<i32>(0, 1)),
        load_premultiplied(origin + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let homogeneous = vec3<f32>(in.uv, 1.0);
    let source_uv = vec2<f32>(
        dot(params.output_uv_to_source_uv_row0.xyz, homogeneous),
        dot(params.output_uv_to_source_uv_row1.xyz, homogeneous),
    );
    return sample_premultiplied_bilinear(source_uv);
}
