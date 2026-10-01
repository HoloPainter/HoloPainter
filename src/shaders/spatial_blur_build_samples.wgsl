const GEOMETRY_EPSILON: f32 = 0.000000000001;

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
var triangle_map: texture_2d<u32>;
@group(0) @binding(1)
var<storage, read> triangles: array<SurfaceTriangle>;
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

fn triangle_uv(triangle: SurfaceTriangle, corner: u32) -> vec2<f32> {
    if corner == 0u { return triangle.uv01.xy; }
    if corner == 1u { return triangle.uv01.zw; }
    return triangle.uv2_pad.xy;
}

fn barycentric_from_uv(triangle: SurfaceTriangle, uv: vec2<f32>) -> vec3<f32> {
    let uv0 = triangle_uv(triangle, 0u);
    let e0 = triangle_uv(triangle, 1u) - uv0;
    let e1 = triangle_uv(triangle, 2u) - uv0;
    let delta = uv - uv0;
    let determinant = e0.x * e1.y - e0.y * e1.x;
    if abs(determinant) <= GEOMETRY_EPSILON {
        return vec3<f32>(-1.0);
    }
    let b1 = (delta.x * e1.y - delta.y * e1.x) / determinant;
    let b2 = (e0.x * delta.y - e0.y * delta.x) / determinant;
    return vec3<f32>(1.0 - b1 - b2, b1, b2);
}

fn position_from_barycentric(triangle: SurfaceTriangle, bary: vec3<f32>) -> vec3<f32> {
    return triangle.p0.xyz * bary.x + triangle.p1.xyz * bary.y + triangle.p2.xyz * bary.z;
}

fn cell_for_position(position: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor((position - params.grid_origin.xyz) / params.radius_world));
}

fn hash_cell(cell: vec3<i32>) -> u32 {
    let x = bitcast<u32>(cell.x);
    let y = bitcast<u32>(cell.y);
    let z = bitcast<u32>(cell.z);
    return (x * 73856093u) ^ (y * 19349663u) ^ (z * 83492791u);
}

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
    let count = triangle_sample_counts[triangle_index];
    if count == 0u {
        return;
    }
    let triangle = triangles[triangle_index];
    let uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / vec2<f32>(params.current_size);
    let bary = barycentric_from_uv(triangle, uv);
    if any(bary < vec3<f32>(-0.001)) {
        return;
    }
    let position = position_from_barycentric(triangle, bary);
    let sample_index = atomicAdd(&counters.sample_count, 1u);
    if sample_index >= params.sample_capacity {
        atomicAdd(&counters.capacity_overflow_samples, 1u);
        return;
    }
    samples[sample_index].position_area = vec4<f32>(position, triangle.uv2_pad.z / f32(count));
    samples[sample_index].uv = uv;
    samples[sample_index].triangle_index = triangle_index;
    let bucket = hash_cell(cell_for_position(position)) & params.bucket_mask;
    let old_head = atomicExchange(&bucket_heads[bucket], sample_index + 1u);
    samples[sample_index].next = old_head;
}
