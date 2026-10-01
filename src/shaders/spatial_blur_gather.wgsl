const GEOMETRY_EPSILON: f32 = 0.000000000001;
const MAX_VISITED_SAMPLES_PER_TEXEL: u32 = 4096u;

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
var source_atlas: texture_2d<f32>;
@group(0) @binding(1)
var triangle_map: texture_2d<u32>;
@group(0) @binding(2)
var<storage, read> triangles: array<SurfaceTriangle>;
@group(0) @binding(3)
var<storage, read> materials: array<MaterialInfo>;
@group(0) @binding(4)
var<storage, read> samples: array<SpatialSample>;
@group(0) @binding(5)
var<storage, read_write> bucket_heads: array<atomic<u32>>;
@group(0) @binding(6)
var<storage, read_write> counters: SpatialCounters;
@group(0) @binding(7)
var output_image: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(8)
var<uniform> params: SpatialBlurUniform;
@group(0) @binding(9)
var selection_mask: texture_2d<f32>;

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

fn triangle_normal(triangle: SurfaceTriangle) -> vec3<f32> {
    return vec3<f32>(triangle.p0.w, triangle.p1.w, triangle.p2.w);
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

fn atlas_color(material_index: u32, uv: vec2<f32>) -> vec4<f32> {
    let material = materials[material_index];
    let maximum = material.size - vec2<u32>(1u);
    let local = min(vec2<u32>(max(uv, vec2<f32>(0.0)) * vec2<f32>(material.size)), maximum);
    return textureLoad(source_atlas, vec2<i32>(material.origin + local), 0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if global_id.x >= params.current_size.x || global_id.y >= params.current_size.y {
        return;
    }
    let packed_triangle = textureLoad(triangle_map, vec2<i32>(global_id.xy), 0).x;
    if packed_triangle == 0u {
        return;
    }
    let query_triangle_index = packed_triangle - 1u;
    if query_triangle_index >= params.triangle_count {
        return;
    }
    let query_triangle = triangles[query_triangle_index];
    let query_uv = (vec2<f32>(global_id.xy) + vec2<f32>(0.5)) / vec2<f32>(params.current_size);
    let query_bary = barycentric_from_uv(query_triangle, query_uv);
    let current_material = materials[params.current_material];
    let original_color = textureLoad(
        source_atlas,
        vec2<i32>(current_material.origin + global_id.xy),
        0,
    );
    if any(query_bary < vec3<f32>(-0.001)) {
        textureStore(output_image, vec2<i32>(global_id.xy), original_color);
        return;
    }

    let query_position = position_from_barycentric(query_triangle, query_bary);
    let query_normal = triangle_normal(query_triangle);
    let query_mesh = query_triangle.metadata.y;
    let query_cell = cell_for_position(query_position);
    let radius2 = params.radius_world * params.radius_world;
    let gaussian_denominator = 2.0 * params.sigma_world * params.sigma_world;
    let sample_limit = min(atomicLoad(&counters.sample_count), params.sample_capacity);
    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    var visited = 0u;
    var overflow = false;

    for (var dz = -1; dz <= 1; dz += 1) {
        if overflow { break; }
        for (var dy = -1; dy <= 1; dy += 1) {
            if overflow { break; }
            for (var dx = -1; dx <= 1; dx += 1) {
                if overflow { break; }
                let target_cell = query_cell + vec3<i32>(dx, dy, dz);
                let bucket = hash_cell(target_cell) & params.bucket_mask;
                var head = atomicLoad(&bucket_heads[bucket]);
                while head != 0u {
                    if visited >= MAX_VISITED_SAMPLES_PER_TEXEL {
                        overflow = true;
                        break;
                    }
                    visited += 1u;
                    let sample_index = head - 1u;
                    if sample_index >= sample_limit {
                        overflow = true;
                        break;
                    }
                    let sample = samples[sample_index];
                    head = sample.next;
                    if any(cell_for_position(sample.position_area.xyz) != target_cell) {
                        continue;
                    }
                    let delta = sample.position_area.xyz - query_position;
                    let distance2 = dot(delta, delta);
                    if distance2 > radius2 {
                        continue;
                    }
                    let sample_triangle = triangles[sample.triangle_index];
                    if params.cross_meshes == 0u && sample_triangle.metadata.y != query_mesh {
                        continue;
                    }
                    if params.orientation_mode != 0u
                        && params.normal_threshold_cos > -1.0
                        && dot(query_normal, triangle_normal(sample_triangle)) < params.normal_threshold_cos
                    {
                        continue;
                    }
                    let gaussian = exp(-distance2 / gaussian_denominator);
                    let weight = sample.position_area.w * gaussian;
                    color_sum += atlas_color(sample_triangle.metadata.x, sample.uv) * weight;
                    weight_sum += weight;
                }
            }
        }
    }

    if overflow {
        atomicAdd(&counters.traversal_overflow_texels, 1u);
    }
    var blurred_color = original_color;
    if weight_sum > 0.0 {
        blurred_color = color_sum / weight_sum;
    }
    var selection = 1.0;
    if params.selection_enabled != 0u {
        selection = clamp(
            textureLoad(selection_mask, vec2<i32>(global_id.xy), 0).r,
            0.0,
            1.0,
        );
    }
    textureStore(
        output_image,
        vec2<i32>(global_id.xy),
        mix(original_color, blurred_color, selection),
    );
}
