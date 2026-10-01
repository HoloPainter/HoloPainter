struct VSOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) _position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) _normal: vec3<f32>,
) -> VSOut {
    var out: VSOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) f32 {
    return 1.0;
}
