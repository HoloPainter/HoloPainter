struct SurfaceBrushOverlay {
    view_proj: mat4x4<f32>,
    center_radius: vec4<f32>,  // xyz = dab world center, w = radius_world
    normal_params: vec4<f32>,  // xyz = dab world normal
    view_direction: vec4<f32>, // xyz = direction toward camera, w = view offset ratio
    params: vec4<f32>,         // y = ring_width_ratio, z = fill_alpha
};

@group(0) @binding(0) var<uniform> overlay: SurfaceBrushOverlay;

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VSOut {
    let quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );

    let center = overlay.center_radius.xyz;
    let radius = max(overlay.center_radius.w, 1e-6);
    let dab_normal = normalize(overlay.normal_params.xyz);
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(dab_normal.y) > 0.98);
    let tangent = normalize(cross(helper, dab_normal));
    let bitangent = cross(dab_normal, tangent);
    let local = quad[vertex_index];
    let view_dir = normalize(overlay.view_direction.xyz);
    let view_offset = view_dir * radius * max(overlay.view_direction.w, 0.0);
    let position = center + view_offset + (tangent * local.x + bitangent * local.y) * radius;

    var out: VSOut;
    out.clip_pos = overlay.view_proj * vec4<f32>(position, 1.0);
    out.local_pos = local;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) f32 {
    let radius = max(overlay.center_radius.w, 1e-6);
    let ring_width_ratio = max(overlay.params.y, 0.0);
    let fill_alpha = clamp(overlay.params.z, 0.0, 1.0);

    let unit_distance = length(in.local_pos);
    if (unit_distance > 1.0) {
        discard;
    }

    let ring_width = max(ring_width_ratio, 1e-5);
    let edge_distance = abs(unit_distance - 1.0);
    let aa = max(fwidth(unit_distance), ring_width * 0.25);
    let ring_alpha = 1.0 - smoothstep(ring_width, ring_width + aa, edge_distance);
    let fill_alpha_value = fill_alpha * (1.0 - smoothstep(0.85, 1.0, unit_distance));
    let alpha = max(ring_alpha, fill_alpha_value);

    if (alpha <= 0.0) {
        discard;
    }

    return alpha;
}
