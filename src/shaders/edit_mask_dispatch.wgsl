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

struct DispatchArgs {
    groups_x: u32,
    groups_y: u32,
    groups_z: u32,
};

struct Params {
    tex_size: vec2<u32>,
    dilation_radius: u32,
    _pad0: u32,
    scan_origin: vec2<u32>,
    scan_size: vec2<u32>,
};

@group(0) @binding(0) var<storage, read_write> bbox: StrokeBBox;
@group(0) @binding(1) var<storage, read_write> dispatch_args: DispatchArgs;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(1, 1, 1)
fn cs_main() {
    let count = atomicLoad(&bbox.stroke_count);
    if (count == 0u) {
        dispatch_args.groups_x = 0u;
        dispatch_args.groups_y = 0u;
        dispatch_args.groups_z = 1u;
        atomicStore(&bbox.min_x, 0u);
        atomicStore(&bbox.min_y, 0u);
        atomicStore(&bbox.max_x, 0u);
        atomicStore(&bbox.max_y, 0u);
        return;
    }

    let tex_w = params.tex_size.x;
    let tex_h = params.tex_size.y;
    let r = params.dilation_radius;

    let min_x = atomicLoad(&bbox.min_x);
    let min_y = atomicLoad(&bbox.min_y);
    let max_x = atomicLoad(&bbox.max_x);
    let max_y = atomicLoad(&bbox.max_y);

    let ex_min_x = select(min_x - r, 0u, min_x < r);
    let ex_min_y = select(min_y - r, 0u, min_y < r);
    let ex_max_x = min(max_x + r, tex_w - 1u);
    let ex_max_y = min(max_y + r, tex_h - 1u);

    atomicStore(&bbox.min_x, ex_min_x);
    atomicStore(&bbox.min_y, ex_min_y);
    atomicStore(&bbox.max_x, ex_max_x);
    atomicStore(&bbox.max_y, ex_max_y);

    let width = ex_max_x - ex_min_x + 1u;
    let height = ex_max_y - ex_min_y + 1u;
    dispatch_args.groups_x = (width + 7u) / 8u;
    dispatch_args.groups_y = (height + 7u) / 8u;
    dispatch_args.groups_z = 1u;
}
