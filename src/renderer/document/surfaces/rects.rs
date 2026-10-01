use crate::core::{geometry::RectU32, image::Rgba8Snapshot};

pub(super) fn subtract_rect_from_rect(stale: RectU32, fresh: RectU32) -> Vec<RectU32> {
    let Some(intersection) = intersect_rects(stale, fresh) else {
        return vec![stale];
    };
    let stale_end = rect_end(stale);
    let fresh_end = rect_end(intersection);
    let mut pieces = Vec::new();

    push_rect_piece(
        &mut pieces,
        stale.origin,
        [stale.size[0], intersection.origin[1] - stale.origin[1]],
    );
    push_rect_piece(
        &mut pieces,
        [stale.origin[0], fresh_end[1]],
        [stale.size[0], stale_end[1] - fresh_end[1]],
    );
    push_rect_piece(
        &mut pieces,
        [stale.origin[0], intersection.origin[1]],
        [
            intersection.origin[0] - stale.origin[0],
            intersection.size[1],
        ],
    );
    push_rect_piece(
        &mut pieces,
        [fresh_end[0], intersection.origin[1]],
        [stale_end[0] - fresh_end[0], intersection.size[1]],
    );

    pieces
}

pub(super) fn subtract_rect_from_rects(stale_rects: &mut Vec<RectU32>, fresh: RectU32) {
    let mut updated = Vec::new();
    for stale in stale_rects.drain(..) {
        updated.extend(subtract_rect_from_rect(stale, fresh));
    }
    merge_touching_rects(&mut updated);
    *stale_rects = updated;
}

fn push_rect_piece(pieces: &mut Vec<RectU32>, origin: [u32; 2], size: [u32; 2]) {
    if size[0] == 0 || size[1] == 0 {
        return;
    }
    pieces.push(RectU32 { origin, size });
}

pub(super) fn clamp_rect_to_texture(texture_size: [u32; 2], rect: RectU32) -> Option<RectU32> {
    let min_x = rect.origin[0].min(texture_size[0]);
    let min_y = rect.origin[1].min(texture_size[1]);
    let max_x = rect.origin[0]
        .saturating_add(rect.size[0])
        .min(texture_size[0]);
    let max_y = rect.origin[1]
        .saturating_add(rect.size[1])
        .min(texture_size[1]);
    (min_x < max_x && min_y < max_y).then_some(RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

fn intersect_rects(a: RectU32, b: RectU32) -> Option<RectU32> {
    let a_end = rect_end(a);
    let b_end = rect_end(b);
    let origin = [a.origin[0].max(b.origin[0]), a.origin[1].max(b.origin[1])];
    let end = [a_end[0].min(b_end[0]), a_end[1].min(b_end[1])];
    if origin[0] >= end[0] || origin[1] >= end[1] {
        return None;
    }
    Some(RectU32 {
        origin,
        size: [end[0] - origin[0], end[1] - origin[1]],
    })
}

pub(super) fn rects_intersect(a: RectU32, b: RectU32) -> bool {
    intersect_rects(a, b).is_some()
}

pub(super) fn merge_touching_rects(rects: &mut Vec<RectU32>) {
    rects.sort_by_key(|rect| (rect.origin[1], rect.origin[0], rect.size[1], rect.size[0]));
    let mut changed = true;
    while changed {
        changed = false;
        let mut merged = Vec::new();
        'outer: for rect in rects.drain(..) {
            for existing in &mut merged {
                if rects_touch(*existing, rect) {
                    *existing = union_rect(*existing, rect);
                    changed = true;
                    continue 'outer;
                }
            }
            merged.push(rect);
        }
        merged.sort_by_key(|rect| (rect.origin[1], rect.origin[0], rect.size[1], rect.size[0]));
        *rects = merged;
    }
}

fn rects_touch(a: RectU32, b: RectU32) -> bool {
    let a_end = rect_end(a);
    let b_end = rect_end(b);
    let overlaps = a.origin[0] < b_end[0]
        && b.origin[0] < a_end[0]
        && a.origin[1] < b_end[1]
        && b.origin[1] < a_end[1];
    let horizontal_neighbors = a.origin[1] == b.origin[1]
        && a.size[1] == b.size[1]
        && a_end[0] >= b.origin[0]
        && b_end[0] >= a.origin[0];
    let vertical_neighbors = a.origin[0] == b.origin[0]
        && a.size[0] == b.size[0]
        && a_end[1] >= b.origin[1]
        && b_end[1] >= a.origin[1];
    overlaps || horizontal_neighbors || vertical_neighbors
}

fn union_rect(a: RectU32, b: RectU32) -> RectU32 {
    let min_x = a.origin[0].min(b.origin[0]);
    let min_y = a.origin[1].min(b.origin[1]);
    let a_end = rect_end(a);
    let b_end = rect_end(b);
    let max_x = a_end[0].max(b_end[0]);
    let max_y = a_end[1].max(b_end[1]);
    RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    }
}

fn rect_end(rect: RectU32) -> [u32; 2] {
    [
        rect.origin[0].saturating_add(rect.size[0]),
        rect.origin[1].saturating_add(rect.size[1]),
    ]
}

pub(super) fn snapshot_rect(snapshot: &Rgba8Snapshot) -> RectU32 {
    RectU32 {
        origin: snapshot.origin,
        size: snapshot.size,
    }
}
