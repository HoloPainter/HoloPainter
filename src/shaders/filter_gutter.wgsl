struct MaterialInfo {
    origin: vec2<u32>,
    size: vec2<u32>,
    enabled: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct FilterGutterUniform {
    radius_world: f32,
    sigma_world: f32,
    material_count: u32,
    current_material: u32,
    gutter_radius: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0)
var filtered_image: texture_2d<f32>;

@group(0) @binding(1)
var triangle_map: texture_2d<u32>;

@group(0) @binding(2)
var<storage, read> materials: array<MaterialInfo>;

@group(0) @binding(3)
var destination_image: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(4)
var<uniform> params: FilterGutterUniform;

fn material_is_active(index: u32) -> bool {
    return index < params.material_count && materials[index].enabled != 0u;
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

    let pixel = vec2<i32>(global_id.xy);
    if textureLoad(triangle_map, pixel, 0).x != 0u {
        textureStore(destination_image, pixel, textureLoad(filtered_image, pixel, 0));
        return;
    }

    let radius = i32(params.gutter_radius);
    let maximum = vec2<i32>(size) - vec2<i32>(1);
    var best_distance2 = 2147483647;
    var best_pixel = pixel;
    var found = false;
    for (var offset_y = -radius; offset_y <= radius; offset_y += 1) {
        for (var offset_x = -radius; offset_x <= radius; offset_x += 1) {
            let distance2 = offset_x * offset_x + offset_y * offset_y;
            if distance2 > radius * radius || distance2 >= best_distance2 {
                continue;
            }
            let candidate = pixel + vec2<i32>(offset_x, offset_y);
            if any(candidate < vec2<i32>(0)) || any(candidate > maximum) {
                continue;
            }
            if textureLoad(triangle_map, candidate, 0).x == 0u {
                continue;
            }
            best_distance2 = distance2;
            best_pixel = candidate;
            found = true;
        }
    }

    if found {
        textureStore(
            destination_image,
            pixel,
            textureLoad(filtered_image, best_pixel, 0),
        );
    }
}
