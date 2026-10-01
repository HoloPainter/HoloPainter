struct SurfaceTriangle {
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    uv01: vec4<f32>,
    uv2_pad: vec4<f32>,
    metadata: vec4<u32>,
    neighbors: vec4<u32>,
    neighbor_edges: vec4<u32>,
};

struct MaterialInfo {
    origin: vec2<u32>,
    size: vec2<u32>,
    enabled: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct SpatialSample {
    position_area: vec4<f32>,
    uv: vec2<f32>,
    triangle_index: u32,
    next: u32,
};

struct SpatialCounters {
    sample_count: atomic<u32>,
    traversal_overflow_texels: atomic<u32>,
    capacity_overflow_samples: atomic<u32>,
    _pad0: atomic<u32>,
};

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
var<storage, read> triangles: array<SurfaceTriangle>;
@group(0) @binding(1)
var<storage, read> materials: array<MaterialInfo>;
@group(0) @binding(2)
var<storage, read> triangle_sample_counts: array<u32>;
@group(0) @binding(3)
var<storage, read_write> samples: array<SpatialSample>;
@group(0) @binding(4)
var<storage, read_write> bucket_heads: array<atomic<u32>>;
@group(0) @binding(5)
var<storage, read_write> counters: SpatialCounters;
@group(0) @binding(6)
var<uniform> params: SpatialBlurUniform;

fn cell_for_position(position: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor((position - params.grid_origin.xyz) / params.radius_world));
}

fn hash_cell(cell: vec3<i32>) -> u32 {
    let x = bitcast<u32>(cell.x);
    let y = bitcast<u32>(cell.y);
    let z = bitcast<u32>(cell.z);
    return (x * 73856093u) ^ (y * 19349663u) ^ (z * 83492791u);
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let triangle_index = global_id.x;
    if triangle_index >= params.triangle_count || triangle_sample_counts[triangle_index] != 0u {
        return;
    }
    let triangle = triangles[triangle_index];
    let material_index = triangle.metadata.x;
    if material_index >= params.material_count || materials[material_index].enabled == 0u {
        return;
    }
    let position = (triangle.p0.xyz + triangle.p1.xyz + triangle.p2.xyz) / 3.0;
    let uv = (triangle.uv01.xy + triangle.uv01.zw + triangle.uv2_pad.xy) / 3.0;
    let sample_index = atomicAdd(&counters.sample_count, 1u);
    if sample_index >= params.sample_capacity {
        atomicAdd(&counters.capacity_overflow_samples, 1u);
        return;
    }
    samples[sample_index].position_area = vec4<f32>(position, triangle.uv2_pad.z);
    samples[sample_index].uv = uv;
    samples[sample_index].triangle_index = triangle_index;
    let bucket = hash_cell(cell_for_position(position)) & params.bucket_mask;
    let old_head = atomicExchange(&bucket_heads[bucket], sample_index + 1u);
    samples[sample_index].next = old_head;
}
