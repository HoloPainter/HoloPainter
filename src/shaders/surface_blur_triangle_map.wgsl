struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) triangle_id: u32,
};

@vertex
fn vs_main(
    @location(0) uv: vec2<f32>,
    @location(1) triangle_id: u32,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    out.triangle_id = triangle_id;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) u32 {
    return in.triangle_id;
}
