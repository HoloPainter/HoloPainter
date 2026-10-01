struct SpatialBlurUniform {
    grid_origin: vec4<f32>,
    radius_world: f32,
    sigma_world: f32,
    normal_threshold_cos: f32,
    reference_radius_px: f32,
    material_count: u32,
    current_material: u32,
    stride_px: u32,
    sample_capacity: u32,
    bucket_mask: u32,
    cross_meshes: u32,
    orientation_mode: u32,
    gutter_radius: u32,
    current_size: vec2<u32>,
    triangle_count: u32,
    selection_enabled: u32,
};

@group(0) @binding(0)
var triangle_map: texture_2d<u32>;

@group(0) @binding(1)
var<storage, read_write> triangle_sample_counts: array<atomic<u32>>;

@group(0) @binding(2)
var<uniform> params: SpatialBlurUniform;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let grid_size = (params.current_size + vec2<u32>(params.stride_px - 1u)) / params.stride_px;
    if global_id.x >= grid_size.x || global_id.y >= grid_size.y {
        return;
    }
    let offset = params.stride_px / 2u;
    let pixel = min(
        global_id.xy * params.stride_px + vec2<u32>(offset),
        params.current_size - vec2<u32>(1u),
    );
    let packed_triangle = textureLoad(triangle_map, vec2<i32>(pixel), 0).x;
    if packed_triangle == 0u {
        return;
    }
    let triangle_index = packed_triangle - 1u;
    if triangle_index >= params.triangle_count {
        return;
    }
    atomicAdd(&triangle_sample_counts[triangle_index], 1u);
}
