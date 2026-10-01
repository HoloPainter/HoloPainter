struct StrokeBBox {
    min_x: atomic<u32>,
    min_y: atomic<u32>,
    max_x: atomic<u32>,
    max_y: atomic<u32>,
    stroke_count: atomic<u32>,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct Params {
    tex_size: vec2<u32>,
    dilation_radius: u32,
    _pad0: u32,
    scan_origin: vec2<u32>,
    scan_size: vec2<u32>,
};

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var stroke_tex: texture_2d<f32>;
@group(0) @binding(2) var island_mask_tex: texture_2d<f32>;
@group(0) @binding(3) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<storage, read> bbox: StrokeBBox;
@group(0) @binding(5) var<uniform> params: Params;

const COVERAGE_EPS: f32 = 1e-5;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let min_x = atomicLoad(&bbox.min_x);
    let min_y = atomicLoad(&bbox.min_y);
    let max_x = atomicLoad(&bbox.max_x);
    let max_y = atomicLoad(&bbox.max_y);
    let count = atomicLoad(&bbox.stroke_count);
    if (count == 0u || max_x < min_x || max_y < min_y) {
        return;
    }

    let width = max_x - min_x + 1u;
    let height = max_y - min_y + 1u;
    if (gid.x >= width || gid.y >= height) {
        return;
    }

    let x = min_x + gid.x;
    let y = min_y + gid.y;
    let px = vec2<i32>(i32(x), i32(y));
    let base = textureLoad(base_tex, px, 0);
    let stroke = textureLoad(stroke_tex, px, 0);
    if (stroke.r > COVERAGE_EPS) {
        // Base stroke color is already composited in finalize before dilation.
        textureStore(dst_tex, px, base);
        return;
    }

    let island = textureLoad(island_mask_tex, px, 0).r;
    if (island >= 0.5) {
        return;
    }

    let tex_w = i32(params.tex_size.x);
    let tex_h = i32(params.tex_size.y);
    let r = i32(params.dilation_radius);

    let radius2 = r * r;
    var best_pixel = px;
    var best_dist2 = radius2 + 1;
    var found = false;
    for (var oy = -r; oy <= r; oy = oy + 1) {
        for (var ox = -r; ox <= r; ox = ox + 1) {
            let dist2 = ox * ox + oy * oy;
            if (dist2 > radius2) {
                continue;
            }
            let nx = px.x + ox;
            let ny = px.y + oy;
            let npx = vec2<i32>(nx, ny);
            if (nx < 0 || ny < 0 || nx >= tex_w || ny >= tex_h) {
                continue;
            }
            if (textureLoad(island_mask_tex, npx, 0).r < 0.5) {
                continue;
            }
            if (!found || dist2 < best_dist2) {
                best_pixel = npx;
                best_dist2 = dist2;
                found = true;
            }
        }
    }

    if (!found) {
        return;
    }

    // Select the nearest island texel from geometry first. Looking for the
    // nearest edited texel instead would dilate tangentially along UV seams.
    if (textureLoad(stroke_tex, best_pixel, 0).r <= COVERAGE_EPS) {
        return;
    }
    textureStore(dst_tex, px, textureLoad(base_tex, best_pixel, 0));
}
