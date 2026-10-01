const PI: f32 = 3.14159265358979323846;
const INVALID_TRIANGLE: u32 = 0xffffffffu;
const WALK_EPSILON: f32 = 0.00001;
const GEOMETRY_EPSILON: f32 = 0.000000000001;
const MAX_EDGE_CROSSINGS: u32 = 128u;
const SAMPLE_RING_COUNT: u32 = 5u;

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

struct BlurUniform {
    radius_world: f32,
    sigma_world: f32,
    material_count: u32,
    current_material: u32,
    gutter_radius: u32,
    selection_enabled: u32,
    _pad0: u32,
    _pad1: u32,
};

struct WalkResult {
    triangle_index: u32,
    bary: vec3<f32>,
    valid: u32,
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
var output_image: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(5)
var<uniform> params: BlurUniform;

@group(0) @binding(6)
var selection_mask: texture_2d<f32>;


fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(value, value);
    if length_squared <= 0.0 {
        return vec3<f32>(0.0);
    }
    return value * inverseSqrt(length_squared);
}

fn triangle_position(triangle: SurfaceTriangle, corner: u32) -> vec3<f32> {
    if corner == 0u {
        return triangle.p0.xyz;
    }
    if corner == 1u {
        return triangle.p1.xyz;
    }
    return triangle.p2.xyz;
}

fn triangle_uv(triangle: SurfaceTriangle, corner: u32) -> vec2<f32> {
    if corner == 0u {
        return triangle.uv01.xy;
    }
    if corner == 1u {
        return triangle.uv01.zw;
    }
    return triangle.uv2_pad.xy;
}

fn material_index(triangle: SurfaceTriangle) -> u32 {
    return triangle.metadata.x;
}

fn edge_start_corner(edge: u32) -> u32 {
    if edge == 0u {
        return 0u;
    }
    if edge == 1u {
        return 1u;
    }
    return 2u;
}

fn edge_end_corner(edge: u32) -> u32 {
    if edge == 0u {
        return 1u;
    }
    if edge == 1u {
        return 2u;
    }
    return 0u;
}

fn edge_opposite_corner(edge: u32) -> u32 {
    if edge == 0u {
        return 2u;
    }
    if edge == 1u {
        return 0u;
    }
    return 1u;
}

fn edge_for_zero_barycentric(corner: u32) -> u32 {
    if corner == 0u {
        return 1u;
    }
    if corner == 1u {
        return 2u;
    }
    return 0u;
}

fn neighbor_triangle(triangle: SurfaceTriangle, edge: u32) -> u32 {
    if edge == 0u {
        return triangle.neighbors.x;
    }
    if edge == 1u {
        return triangle.neighbors.y;
    }
    return triangle.neighbors.z;
}

fn neighbor_edge(triangle: SurfaceTriangle, edge: u32) -> u32 {
    if edge == 0u {
        return triangle.neighbor_edges.x;
    }
    if edge == 1u {
        return triangle.neighbor_edges.y;
    }
    return triangle.neighbor_edges.z;
}

fn bary_component(bary: vec3<f32>, corner: u32) -> f32 {
    if corner == 0u {
        return bary.x;
    }
    if corner == 1u {
        return bary.y;
    }
    return bary.z;
}

fn set_bary_component(bary: vec3<f32>, corner: u32, value: f32) -> vec3<f32> {
    if corner == 0u {
        return vec3<f32>(value, bary.y, bary.z);
    }
    if corner == 1u {
        return vec3<f32>(bary.x, value, bary.z);
    }
    return vec3<f32>(bary.x, bary.y, value);
}

fn normalized_barycentric(bary: vec3<f32>) -> vec3<f32> {
    let clamped = max(bary, vec3<f32>(0.0));
    let total = clamped.x + clamped.y + clamped.z;
    if total <= GEOMETRY_EPSILON {
        return vec3<f32>(1.0, 0.0, 0.0);
    }
    return clamped / total;
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

fn barycentric_from_position(triangle: SurfaceTriangle, position: vec3<f32>) -> vec3<f32> {
    let p0 = triangle_position(triangle, 0u);
    let e0 = triangle_position(triangle, 1u) - p0;
    let e1 = triangle_position(triangle, 2u) - p0;
    let delta = position - p0;
    let a = dot(e0, e0);
    let b = dot(e0, e1);
    let c = dot(e1, e1);
    let d0 = dot(delta, e0);
    let d1 = dot(delta, e1);
    let determinant = a * c - b * b;
    if determinant <= GEOMETRY_EPSILON {
        return vec3<f32>(-1.0);
    }
    let b1 = (d0 * c - d1 * b) / determinant;
    let b2 = (d1 * a - d0 * b) / determinant;
    return vec3<f32>(1.0 - b1 - b2, b1, b2);
}

fn position_from_barycentric(triangle: SurfaceTriangle, bary: vec3<f32>) -> vec3<f32> {
    return triangle_position(triangle, 0u) * bary.x
        + triangle_position(triangle, 1u) * bary.y
        + triangle_position(triangle, 2u) * bary.z;
}

fn uv_from_barycentric(triangle: SurfaceTriangle, bary: vec3<f32>) -> vec2<f32> {
    return triangle_uv(triangle, 0u) * bary.x
        + triangle_uv(triangle, 1u) * bary.y
        + triangle_uv(triangle, 2u) * bary.z;
}

fn barycentric_direction(triangle: SurfaceTriangle, direction: vec3<f32>) -> vec3<f32> {
    let p0 = triangle_position(triangle, 0u);
    let e0 = triangle_position(triangle, 1u) - p0;
    let e1 = triangle_position(triangle, 2u) - p0;
    let a = dot(e0, e0);
    let b = dot(e0, e1);
    let c = dot(e1, e1);
    let d0 = dot(direction, e0);
    let d1 = dot(direction, e1);
    let determinant = a * c - b * b;
    if determinant <= GEOMETRY_EPSILON {
        return vec3<f32>(0.0);
    }
    let db1 = (d0 * c - d1 * b) / determinant;
    let db2 = (d1 * a - d0 * b) / determinant;
    return vec3<f32>(-db1 - db2, db1, db2);
}

fn edge_inside_direction(triangle: SurfaceTriangle, edge: u32) -> vec3<f32> {
    let a = triangle_position(triangle, edge_start_corner(edge));
    let b = triangle_position(triangle, edge_end_corner(edge));
    let opposite = triangle_position(triangle, edge_opposite_corner(edge));
    let edge_direction = safe_normalize(b - a);
    if dot(edge_direction, edge_direction) <= 0.0 {
        return vec3<f32>(0.0);
    }
    let projected = a + edge_direction * dot(opposite - a, edge_direction);
    return safe_normalize(opposite - projected);
}

fn nudge_inside(bary: vec3<f32>, edge: u32) -> vec3<f32> {
    let opposite = edge_opposite_corner(edge);
    var result = bary * (1.0 - WALK_EPSILON);
    result = set_bary_component(
        result,
        opposite,
        bary_component(result, opposite) + WALK_EPSILON,
    );
    return normalized_barycentric(result);
}

fn material_is_active(index: u32) -> bool {
    return index < params.material_count && materials[index].enabled != 0u;
}

fn walk_surface(
    start_triangle: u32,
    start_bary: vec3<f32>,
    initial_direction: vec3<f32>,
    distance: f32,
) -> WalkResult {
    var triangle_index = start_triangle;
    var bary = normalized_barycentric(start_bary);
    var direction = safe_normalize(initial_direction);
    if dot(direction, direction) <= 0.0 {
        return WalkResult(start_triangle, normalized_barycentric(start_bary), 0u);
    }
    var remaining = distance;

    for (var crossing = 0u; crossing < MAX_EDGE_CROSSINGS; crossing += 1u) {
        if remaining <= GEOMETRY_EPSILON {
            return WalkResult(triangle_index, bary, 1u);
        }

        let triangle = triangles[triangle_index];
        let db = barycentric_direction(triangle, direction);
        if dot(db, db) <= GEOMETRY_EPSILON {
            return WalkResult(triangle_index, bary, 0u);
        }

        var edge_distance = remaining + 1.0;
        var zero_corner = 3u;
        if db.x < -GEOMETRY_EPSILON {
            let candidate = -bary.x / db.x;
            if candidate < edge_distance {
                edge_distance = candidate;
                zero_corner = 0u;
            }
        }
        if db.y < -GEOMETRY_EPSILON {
            let candidate = -bary.y / db.y;
            if candidate < edge_distance {
                edge_distance = candidate;
                zero_corner = 1u;
            }
        }
        if db.z < -GEOMETRY_EPSILON {
            let candidate = -bary.z / db.z;
            if candidate < edge_distance {
                edge_distance = candidate;
                zero_corner = 2u;
            }
        }

        if zero_corner == 3u || edge_distance >= remaining - WALK_EPSILON {
            bary = normalized_barycentric(bary + db * remaining);
            return WalkResult(triangle_index, bary, 1u);
        }

        let edge = edge_for_zero_barycentric(zero_corner);
        bary = normalized_barycentric(bary + db * max(edge_distance, 0.0));
        bary = set_bary_component(bary, zero_corner, 0.0);
        bary = normalized_barycentric(bary);
        remaining = max(remaining - max(edge_distance, 0.0), 0.0);

        let next_triangle_index = neighbor_triangle(triangle, edge);
        let next_is_active = next_triangle_index != INVALID_TRIANGLE
            && material_is_active(material_index(triangles[next_triangle_index]));
        if !next_is_active {
            let inside = edge_inside_direction(triangle, edge);
            if dot(inside, inside) <= 0.0 {
                return WalkResult(triangle_index, bary, 0u);
            }
            direction = safe_normalize(direction - 2.0 * dot(direction, inside) * inside);
            if dot(direction, direction) <= 0.0 {
                return WalkResult(triangle_index, bary, 0u);
            }
            bary = nudge_inside(bary, edge);
            continue;
        }

        let crossing_position = position_from_barycentric(triangle, bary);
        let next_triangle = triangles[next_triangle_index];
        let next_edge = neighbor_edge(triangle, edge);
        var next_bary = barycentric_from_position(next_triangle, crossing_position);
        if min(next_bary.x, min(next_bary.y, next_bary.z)) < -0.001 {
            return WalkResult(triangle_index, bary, 0u);
        }
        next_bary = nudge_inside(normalized_barycentric(next_bary), next_edge);

        let edge_start = triangle_position(triangle, edge_start_corner(edge));
        let edge_end = triangle_position(triangle, edge_end_corner(edge));
        let edge_direction = safe_normalize(edge_end - edge_start);
        let current_inside = edge_inside_direction(triangle, edge);
        let next_inside = edge_inside_direction(next_triangle, next_edge);
        let parallel = dot(direction, edge_direction);
        let outward = max(-dot(direction, current_inside), 0.0);
        direction = safe_normalize(edge_direction * parallel + next_inside * outward);
        if dot(direction, direction) <= 0.0 {
            return WalkResult(triangle_index, bary, 0u);
        }
        triangle_index = next_triangle_index;
        bary = next_bary;
    }

    return WalkResult(triangle_index, bary, 0u);
}

fn load_source(material: u32, pixel: vec2<i32>) -> vec4<f32> {
    let origin = vec2<i32>(materials[material].origin);
    return textureLoad(source_atlas, origin + pixel, 0);
}

fn sample_source(material: u32, uv: vec2<f32>) -> vec4<f32> {
    let size_u = materials[material].size;
    let size = vec2<f32>(size_u);
    let coordinate = uv * size - vec2<f32>(0.5);
    let base = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let maximum = vec2<i32>(size_u) - vec2<i32>(1);
    let p00 = clamp(base, vec2<i32>(0), maximum);
    let p10 = clamp(base + vec2<i32>(1, 0), vec2<i32>(0), maximum);
    let p01 = clamp(base + vec2<i32>(0, 1), vec2<i32>(0), maximum);
    let p11 = clamp(base + vec2<i32>(1, 1), vec2<i32>(0), maximum);
    let top = mix(load_source(material, p00), load_source(material, p10), fraction.x);
    let bottom = mix(load_source(material, p01), load_source(material, p11), fraction.x);
    return mix(top, bottom, fraction.y);
}

fn sample_endpoint(result: WalkResult) -> vec4<f32> {
    let triangle = triangles[result.triangle_index];
    return sample_source(material_index(triangle), uv_from_barycentric(triangle, result.bary));
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let material = params.current_material;
    if material >= params.material_count || !material_is_active(material) {
        return;
    }
    let size = materials[material].size;
    if global_id.x >= size.x || global_id.y >= size.y {
        return;
    }

    let packed_triangle_id = textureLoad(
        triangle_map,
        vec2<i32>(global_id.xy),
        0,
    ).x;
    if packed_triangle_id == 0u {
        return;
    }
    let triangle_index = packed_triangle_id - 1u;
    let triangle = triangles[triangle_index];
    let uv = (vec2<f32>(global_id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let start_bary = barycentric_from_uv(triangle, uv);
    if min(start_bary.x, min(start_bary.y, start_bary.z)) < -0.01 {
        return;
    }

    let p0 = triangle_position(triangle, 0u);
    let edge0 = triangle_position(triangle, 1u) - p0;
    let edge1 = triangle_position(triangle, 2u) - p0;
    let normal_raw = cross(edge0, edge1);
    if dot(edge0, edge0) <= 0.0 || dot(normal_raw, normal_raw) <= 0.0 {
        return;
    }
    let normal = safe_normalize(normal_raw);
    var reference_axis = vec3<f32>(0.0, 0.0, 1.0);
    if abs(normal.z) > 0.9 {
        reference_axis = vec3<f32>(0.0, 1.0, 0.0);
    }
    let tangent = safe_normalize(cross(reference_axis, normal));
    let bitangent = safe_normalize(cross(normal, tangent));
    if dot(tangent, tangent) <= 0.0 || dot(bitangent, bitangent) <= 0.0 {
        return;
    }

    var color_sum = sample_source(material, uv);
    var weight_sum = 1.0;
    for (var ring = 1u; ring <= SAMPLE_RING_COUNT; ring += 1u) {
        let sample_count = ring * 6u;
        let distance = params.radius_world * f32(ring) / f32(SAMPLE_RING_COUNT);
        let normalized_distance = distance / max(params.sigma_world, GEOMETRY_EPSILON);
        let weight = exp(-0.5 * normalized_distance * normalized_distance);
        for (var sample_index = 0u; sample_index < sample_count; sample_index += 1u) {
            let ring_offset = select(0.0, PI / f32(sample_count), ring % 2u == 0u);
            let angle = 2.0 * PI * f32(sample_index) / f32(sample_count) + ring_offset;
            let direction = tangent * cos(angle) + bitangent * sin(angle);
            let endpoint = walk_surface(triangle_index, start_bary, direction, distance);
            if endpoint.valid != 0u {
                color_sum += sample_endpoint(endpoint) * weight;
                weight_sum += weight;
            }
        }
    }

    let blurred_color = color_sum / max(weight_sum, GEOMETRY_EPSILON);
    let original_color = load_source(material, vec2<i32>(global_id.xy));
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
