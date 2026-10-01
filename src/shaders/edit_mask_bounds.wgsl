struct Params {
    tex_size: vec2<u32>,
    dilation_radius: u32,
    _pad0: u32,
    scan_origin: vec2<u32>,
    scan_size: vec2<u32>,
};

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

@group(0) @binding(0) var stroke_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bbox: StrokeBBox;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.scan_size.x || gid.y >= params.scan_size.y) {
        return;
    }
    let coord = params.scan_origin + gid.xy;
    let dims = textureDimensions(stroke_tex, 0);
    if (coord.x >= dims.x || coord.y >= dims.y) {
        return;
    }
    let a = textureLoad(stroke_tex, vec2<i32>(coord), 0).r;
    if (a <= 1e-5) {
        return;
    }
    atomicMin(&bbox.min_x, coord.x);
    atomicMin(&bbox.min_y, coord.y);
    atomicMax(&bbox.max_x, coord.x);
    atomicMax(&bbox.max_y, coord.y);
    _ = atomicAdd(&bbox.stroke_count, 1u);
}
