use crate::core::geometry::RectU32;

pub(crate) fn normalize_rect_to_texture(texture_size: [u32; 2], rect: RectU32) -> Option<RectU32> {
    if rect.size[0] == 0 || rect.size[1] == 0 {
        return None;
    }

    let min_x = rect.origin[0].min(texture_size[0]);
    let min_y = rect.origin[1].min(texture_size[1]);
    let max_x = rect.origin[0]
        .saturating_add(rect.size[0])
        .min(texture_size[0]);
    let max_y = rect.origin[1]
        .saturating_add(rect.size[1])
        .min(texture_size[1]);

    if min_x >= max_x || min_y >= max_y {
        return None;
    }

    Some(RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

pub(crate) fn normalize_rects_to_texture(
    texture_size: [u32; 2],
    rects: &[RectU32],
) -> Vec<RectU32> {
    let mut normalized: Vec<_> = rects
        .iter()
        .filter_map(|rect| normalize_rect_to_texture(texture_size, *rect))
        .collect();
    merge_touching_rects(&mut normalized);
    normalized
}

pub(crate) fn merge_touching_rects(rects: &mut Vec<RectU32>) {
    rects.sort_by_key(rect_sort_key);

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

        merged.sort_by_key(rect_sort_key);
        *rects = merged;
    }
}

pub(crate) fn rects_touch(a: RectU32, b: RectU32) -> bool {
    let a_end = rect_end(a);
    let b_end = rect_end(b);
    a.origin[0] <= b_end[0]
        && b.origin[0] <= a_end[0]
        && a.origin[1] <= b_end[1]
        && b.origin[1] <= a_end[1]
}

pub(crate) fn union_rect(a: RectU32, b: RectU32) -> RectU32 {
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

pub(crate) fn rect_end(rect: RectU32) -> [u32; 2] {
    [
        rect.origin[0].saturating_add(rect.size[0]),
        rect.origin[1].saturating_add(rect.size[1]),
    ]
}

fn rect_sort_key(rect: &RectU32) -> (u32, u32, u32, u32) {
    (rect.origin[1], rect.origin[0], rect.size[1], rect.size[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(origin: [u32; 2], size: [u32; 2]) -> RectU32 {
        RectU32 { origin, size }
    }

    #[test]
    fn merge_touching_rects_merges_transitive_chain() {
        let mut rects = vec![
            rect([0, 0], [10, 10]),
            rect([20, 0], [10, 10]),
            rect([10, 0], [10, 10]),
        ];

        merge_touching_rects(&mut rects);

        assert_eq!(rects, vec![rect([0, 0], [30, 10])]);
    }

    #[test]
    fn merge_touching_rects_keeps_separated_rects() {
        let mut rects = vec![rect([0, 0], [10, 10]), rect([11, 0], [10, 10])];

        merge_touching_rects(&mut rects);

        assert_eq!(rects, vec![rect([0, 0], [10, 10]), rect([11, 0], [10, 10])]);
    }

    #[test]
    fn merge_touching_rects_merges_edge_touching_rects() {
        let mut rects = vec![rect([0, 0], [10, 10]), rect([10, 0], [10, 10])];

        merge_touching_rects(&mut rects);

        assert_eq!(rects, vec![rect([0, 0], [20, 10])]);
    }

    #[test]
    fn normalize_rects_to_texture_clamps_to_texture_bounds() {
        let rects = normalize_rects_to_texture([100, 100], &[rect([90, 80], [20, 40])]);

        assert_eq!(
            rects,
            vec![RectU32 {
                origin: [90, 80],
                size: [10, 20],
            }]
        );
    }

    #[test]
    fn normalize_rects_to_texture_drops_zero_and_outside_rects() {
        let rects = normalize_rects_to_texture(
            [100, 100],
            &[
                rect([0, 0], [0, 10]),
                rect([100, 0], [10, 10]),
                rect([0, 100], [10, 10]),
            ],
        );

        assert!(rects.is_empty());
    }
}
