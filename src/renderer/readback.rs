use crate::{
    core::{
        damage::{rects_touch, union_rect},
        geometry::RectU32,
        image::Rgba8Snapshot,
        selection::{ActiveSelection, SelectionMaskId},
        surface::PaintSurfaceId,
    },
    renderer::mutation::SurfaceDamage,
};

#[derive(Debug, Clone)]
pub enum ReadbackRequest {
    SurfaceRgba8 {
        surface: PaintSurfaceId,
        rect: Option<RectU32>,
    },
    CompositeRgba8 {
        material_index: usize,
        rect: Option<RectU32>,
    },
    SelectionMaskR8 {
        mask_id: SelectionMaskId,
    },
}

#[derive(Debug, Clone)]
pub enum ReadbackResult {
    Rgba8(Rgba8Snapshot),
    SelectionMaskR8(Option<Vec<u8>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceCommitReadbackRequest {
    pub(crate) surface: PaintSurfaceId,
    pub(crate) rect: Option<RectU32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionCommitReadbackRequest {
    pub(crate) mask_id: SelectionMaskId,
    pub(crate) material_index: usize,
    pub(crate) texture_size: [u32; 2],
}

pub(crate) fn selection_commit_readback_requests_from_selection(
    active_selection: &ActiveSelection,
    materials: &crate::renderer::document::materials::MaterialRegistry,
) -> Vec<SelectionCommitReadbackRequest> {
    active_selection
        .masks
        .iter()
        .filter_map(|mask| {
            Some(SelectionCommitReadbackRequest {
                mask_id: mask.mask_id?,
                material_index: mask.material_index.as_usize(),
                texture_size: materials.texture_size(mask.material_index.as_usize())?,
            })
        })
        .collect()
}

pub(crate) fn surface_commit_readback_requests_from_damage<'a>(
    damage: impl Iterator<Item = &'a SurfaceDamage>,
    mut surface_requires_full_readback: impl FnMut(PaintSurfaceId) -> bool,
) -> Vec<SurfaceCommitReadbackRequest> {
    let mut requests = Vec::new();
    for damage in damage {
        match damage {
            SurfaceDamage::Preview { .. }
            | SurfaceDamage::Transient { .. }
            | SurfaceDamage::Cancelled { .. } => {}
            SurfaceDamage::Full { surface } => {
                push_full_surface_commit_readback_request(&mut requests, *surface);
            }
            SurfaceDamage::Rects { surface, damage } if damage.is_empty() => {
                push_full_surface_commit_readback_request(&mut requests, *surface);
            }
            SurfaceDamage::Rects { surface, damage } => {
                if surface_requires_full_readback(*surface) {
                    push_full_surface_commit_readback_request(&mut requests, *surface);
                    continue;
                }
                for pixel in &damage.pixels {
                    if surface_requires_full_readback(pixel.surface) {
                        push_full_surface_commit_readback_request(&mut requests, pixel.surface);
                        continue;
                    }
                    push_rect_surface_commit_readback_request(
                        &mut requests,
                        pixel.surface,
                        pixel.rect,
                    );
                }
            }
        }
    }
    requests
}

fn push_full_surface_commit_readback_request(
    requests: &mut Vec<SurfaceCommitReadbackRequest>,
    surface: PaintSurfaceId,
) {
    requests.retain(|request| request.surface != surface);
    requests.push(SurfaceCommitReadbackRequest {
        surface,
        rect: None,
    });
}

const MAX_RECT_READBACK_REQUESTS_PER_SURFACE: usize = 16;

fn push_rect_surface_commit_readback_request(
    requests: &mut Vec<SurfaceCommitReadbackRequest>,
    surface: PaintSurfaceId,
    rect: RectU32,
) {
    if requests
        .iter()
        .any(|request| request.surface == surface && request.rect.is_none())
    {
        return;
    }
    requests.push(SurfaceCommitReadbackRequest {
        surface,
        rect: Some(rect),
    });
    merge_touching_surface_readback_rects(requests, surface);
    compact_surface_readback_rects(requests, surface);
}

fn merge_touching_surface_readback_rects(
    requests: &mut Vec<SurfaceCommitReadbackRequest>,
    surface: PaintSurfaceId,
) {
    loop {
        let mut merge_pair = None;
        'search: for left in 0..requests.len() {
            if requests[left].surface != surface {
                continue;
            }
            let Some(left_rect) = requests[left].rect else {
                continue;
            };
            for right in left + 1..requests.len() {
                if requests[right].surface != surface {
                    continue;
                }
                let Some(right_rect) = requests[right].rect else {
                    continue;
                };
                if rects_touch(left_rect, right_rect) {
                    merge_pair = Some((left, right, union_rect(left_rect, right_rect)));
                    break 'search;
                }
            }
        }
        let Some((left, right, merged)) = merge_pair else {
            break;
        };
        requests[left].rect = Some(merged);
        requests.remove(right);
    }
}

fn compact_surface_readback_rects(
    requests: &mut Vec<SurfaceCommitReadbackRequest>,
    surface: PaintSurfaceId,
) {
    while requests
        .iter()
        .filter(|request| request.surface == surface)
        .count()
        > MAX_RECT_READBACK_REQUESTS_PER_SURFACE
    {
        let indices = requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| {
                (request.surface == surface && request.rect.is_some()).then_some(index)
            })
            .collect::<Vec<_>>();
        let mut best_pair = None;
        for (left_offset, &left) in indices.iter().enumerate() {
            let left_rect = requests[left]
                .rect
                .expect("surface rect request was filtered above");
            for &right in &indices[left_offset + 1..] {
                let right_rect = requests[right]
                    .rect
                    .expect("surface rect request was filtered above");
                let merged = union_rect(left_rect, right_rect);
                let inflation = rect_area(merged)
                    .saturating_sub(rect_area(left_rect))
                    .saturating_sub(rect_area(right_rect));
                let candidate = (inflation, left, right, merged);
                if best_pair
                    .as_ref()
                    .is_none_or(|best: &(u64, usize, usize, RectU32)| {
                        candidate.0 < best.0
                            || (candidate.0 == best.0
                                && (candidate.1, candidate.2) < (best.1, best.2))
                    })
                {
                    best_pair = Some(candidate);
                }
            }
        }
        let Some((_, left, right, merged)) = best_pair else {
            break;
        };
        requests[left].rect = Some(merged);
        requests.remove(right);
        merge_touching_surface_readback_rects(requests, surface);
    }
}

fn rect_area(rect: RectU32) -> u64 {
    u64::from(rect.size[0]) * u64::from(rect.size[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{damage::DamageMap, surface::LayerId};
    use slotmap::SlotMap;

    fn test_surface_for_material(material_index: usize) -> PaintSurfaceId {
        let mut layers: SlotMap<LayerId, ()> = SlotMap::with_key();
        PaintSurfaceId::raster(material_index.into(), layers.insert(()))
    }

    fn rect(origin: [u32; 2], size: [u32; 2]) -> RectU32 {
        RectU32 { origin, size }
    }

    #[test]
    fn surface_commit_readback_requests_are_derived_from_renderer_damage() {
        let surface_a = test_surface_for_material(0);
        let surface_b = test_surface_for_material(1);
        let mut damage = DamageMap::default();
        damage.add_rect(surface_a, rect([1, 2], [3, 4]));
        damage.add_rect(surface_b, rect([5, 6], [7, 8]));
        let damages = vec![
            SurfaceDamage::Preview {
                surface: surface_a,
                damage: None,
            },
            SurfaceDamage::Rects {
                surface: surface_a,
                damage,
            },
        ];

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |_| false);

        assert_eq!(
            requests,
            vec![
                SurfaceCommitReadbackRequest {
                    surface: surface_a,
                    rect: Some(rect([1, 2], [3, 4])),
                },
                SurfaceCommitReadbackRequest {
                    surface: surface_b,
                    rect: Some(rect([5, 6], [7, 8])),
                },
            ]
        );
    }

    #[test]
    fn full_surface_commit_readback_request_dominates_rect_requests() {
        let surface = test_surface_for_material(0);
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect([1, 2], [3, 4]));
        let damages = vec![
            SurfaceDamage::Rects { surface, damage },
            SurfaceDamage::Full { surface },
        ];

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |_| false);

        assert_eq!(
            requests,
            vec![SurfaceCommitReadbackRequest {
                surface,
                rect: None,
            }]
        );
    }

    #[test]
    fn disjoint_rect_readback_requests_are_preserved() {
        let surface = test_surface_for_material(0);
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect([1, 2], [3, 4]));
        damage.add_rect(surface, rect([20, 30], [5, 6]));
        let damages = vec![SurfaceDamage::Rects { surface, damage }];

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |_| false);

        assert_eq!(
            requests,
            vec![
                SurfaceCommitReadbackRequest {
                    surface,
                    rect: Some(rect([1, 2], [3, 4])),
                },
                SurfaceCommitReadbackRequest {
                    surface,
                    rect: Some(rect([20, 30], [5, 6])),
                },
            ]
        );
    }

    #[test]
    fn touching_rect_readback_requests_are_merged_across_damage_entries() {
        let surface = test_surface_for_material(0);
        let mut first = DamageMap::default();
        first.add_rect(surface, rect([1, 2], [3, 4]));
        let mut second = DamageMap::default();
        second.add_rect(surface, rect([4, 2], [5, 4]));
        let damages = vec![
            SurfaceDamage::Rects {
                surface,
                damage: first,
            },
            SurfaceDamage::Rects {
                surface,
                damage: second,
            },
        ];

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |_| false);

        assert_eq!(
            requests,
            vec![SurfaceCommitReadbackRequest {
                surface,
                rect: Some(rect([1, 2], [8, 4])),
            }]
        );
    }

    #[test]
    fn rect_readback_requests_are_compacted_to_the_per_surface_budget() {
        let surface = test_surface_for_material(0);
        let mut damages = Vec::new();
        for index in 0..=MAX_RECT_READBACK_REQUESTS_PER_SURFACE {
            let mut damage = DamageMap::default();
            damage.add_rect(
                surface,
                RectU32 {
                    origin: [index as u32 * 20, 8],
                    size: [2, 2],
                },
            );
            damages.push(SurfaceDamage::Rects { surface, damage });
        }

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |_| false);

        assert_eq!(requests.len(), MAX_RECT_READBACK_REQUESTS_PER_SURFACE);
        for index in 0..=MAX_RECT_READBACK_REQUESTS_PER_SURFACE {
            let point = [index as u32 * 20, 8];
            assert!(requests.iter().any(|request| {
                let rect = request
                    .rect
                    .expect("all requests should remain rectangular");
                rect.origin[0] <= point[0]
                    && point[0] < rect.origin[0] + rect.size[0]
                    && rect.origin[1] <= point[1]
                    && point[1] < rect.origin[1] + rect.size[1]
            }));
        }
    }

    #[test]
    fn stale_full_surface_promotes_rect_damage_to_full_readback() {
        let surface = test_surface_for_material(0);
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect([1, 2], [3, 4]));
        let damages = vec![SurfaceDamage::Rects { surface, damage }];

        let requests = surface_commit_readback_requests_from_damage(damages.iter(), |candidate| {
            candidate == surface
        });

        assert_eq!(
            requests,
            vec![SurfaceCommitReadbackRequest {
                surface,
                rect: None,
            }]
        );
    }
}
