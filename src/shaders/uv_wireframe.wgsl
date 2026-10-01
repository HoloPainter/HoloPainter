struct UvViewTransform {
    view_size: vec2<f32>,
    canvas_size: vec2<f32>,
    center_transform: vec4<f32>,
    rotation: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view_transform: UvViewTransform;

struct WireframeStyle {
    color_opacity: vec4<f32>,
};

@group(0) @binding(1) var<uniform> wireframe: WireframeStyle;

struct VSIn {
    @location(1) uv: vec2<f32>,
};

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
};

fn rotate_clockwise(value: vec2<f32>) -> vec2<f32> {
    let sin_angle = view_transform.center_transform.w;
    let cos_angle = view_transform.rotation.x;
    return vec2<f32>(
        cos_angle * value.x - sin_angle * value.y,
        sin_angle * value.x + cos_angle * value.y,
    );
}

@vertex
fn vs_main(in: VSIn) -> VSOut {
    var out: VSOut;
    let view_size = max(view_transform.view_size, vec2<f32>(1.0));
    let canvas_size = max(view_transform.canvas_size, vec2<f32>(1.0));
    let zoom = view_transform.center_transform.z;
    let screen_px = view_size * 0.5
        + rotate_clockwise(
            (in.uv - view_transform.center_transform.xy) * canvas_size * zoom
        );
    let screen = screen_px / view_size;
    let clip = vec2<f32>(screen.x * 2.0 - 1.0, 1.0 - screen.y * 2.0);
    out.clip_pos = vec4<f32>(clip, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return wireframe.color_opacity;
}
