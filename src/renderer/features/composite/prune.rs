use crate::core::{
    composite::GroupCompositeMode,
    surface::{CompositeGroup, CompositeNode, CompositeTree, PaintSurfaceId},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct KnownEmptyPruneStats {
    pub(super) raster_skips: usize,
    pub(super) zero_mask_skips: usize,
    pub(super) group_skips: usize,
}

pub(super) fn flatten_identity_pass_through_groups(tree: &mut CompositeTree) -> usize {
    flatten_identity_pass_through_children(&mut tree.root)
}

fn flatten_identity_pass_through_children(group: &mut CompositeGroup) -> usize {
    let mut flattened = 0usize;
    let mut children = Vec::with_capacity(group.children.len());
    for child in std::mem::take(&mut group.children) {
        match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => children.push(child),
            CompositeNode::Group(mut child_group) => {
                flattened = flattened
                    .saturating_add(flatten_identity_pass_through_children(&mut child_group));
                if is_identity_pass_through_group(&child_group) {
                    flattened = flattened.saturating_add(1);
                    children.extend(child_group.children);
                } else {
                    children.push(CompositeNode::Group(child_group));
                }
            }
        }
    }
    group.children = children;
    flattened
}

fn is_identity_pass_through_group(group: &CompositeGroup) -> bool {
    group.mode == GroupCompositeMode::PassThrough
        && group.props.visible
        && group.props.opacity == 1.0
        && group.mask.is_none()
}

pub(super) fn prune_known_empty(
    tree: &mut CompositeTree,
    is_known_empty: &impl Fn(PaintSurfaceId) -> bool,
    is_protected: &impl Fn(PaintSurfaceId) -> bool,
) -> KnownEmptyPruneStats {
    let mut stats = KnownEmptyPruneStats::default();
    prune_group(
        &mut tree.root,
        true,
        is_known_empty,
        is_protected,
        &mut stats,
    );
    stats
}

fn prune_group(
    group: &mut CompositeGroup,
    is_root: bool,
    is_known_empty: &impl Fn(PaintSurfaceId) -> bool,
    is_protected: &impl Fn(PaintSurfaceId) -> bool,
    stats: &mut KnownEmptyPruneStats,
) -> bool {
    let group_contains_protected = group_contains_protected_surface(group, is_protected);
    if group
        .mask
        .as_ref()
        .is_some_and(|mask| is_known_empty(*mask) && !group_contains_protected)
    {
        stats.zero_mask_skips = stats.zero_mask_skips.saturating_add(1);
        if is_root {
            group.children.clear();
            group.mask = None;
            return true;
        }
        return false;
    }

    let mut retained = Vec::with_capacity(group.children.len());
    for mut child in std::mem::take(&mut group.children) {
        let keep = match &mut child {
            CompositeNode::Raster { surface, mask, .. } => {
                let node_is_protected =
                    is_protected(*surface) || mask.as_ref().is_some_and(|mask| is_protected(*mask));
                if mask.as_ref().is_some_and(|mask| is_known_empty(*mask)) && !node_is_protected {
                    stats.zero_mask_skips = stats.zero_mask_skips.saturating_add(1);
                    false
                } else if is_known_empty(*surface) && !node_is_protected {
                    stats.raster_skips = stats.raster_skips.saturating_add(1);
                    false
                } else {
                    true
                }
            }
            CompositeNode::SolidFill { .. } => true,
            CompositeNode::EmbeddedImage { mask, .. } => {
                let node_is_protected = mask.as_ref().is_some_and(|mask| is_protected(*mask));
                !(mask.as_ref().is_some_and(|mask| is_known_empty(*mask)) && !node_is_protected)
            }
            CompositeNode::Adjustment { mask, .. } => {
                let node_is_protected = mask.as_ref().is_some_and(|mask| is_protected(*mask));
                if mask.as_ref().is_some_and(|mask| is_known_empty(*mask)) && !node_is_protected {
                    stats.zero_mask_skips = stats.zero_mask_skips.saturating_add(1);
                    false
                } else {
                    true
                }
            }
            CompositeNode::Group(child_group) => {
                let keep = prune_group(child_group, false, is_known_empty, is_protected, stats);
                if !keep {
                    stats.group_skips = stats.group_skips.saturating_add(1);
                }
                keep
            }
        };
        if keep {
            retained.push(child);
        }
    }
    group.children = retained;

    is_root
        || !group.children.is_empty()
        || group.mask.as_ref().is_some_and(|mask| is_protected(*mask))
}

fn group_contains_protected_surface(
    group: &CompositeGroup,
    is_protected: &impl Fn(PaintSurfaceId) -> bool,
) -> bool {
    group.mask.as_ref().is_some_and(|mask| is_protected(*mask))
        || group.children.iter().any(|child| match child {
            CompositeNode::Raster { surface, mask, .. } => {
                is_protected(*surface) || mask.as_ref().is_some_and(|mask| is_protected(*mask))
            }
            CompositeNode::SolidFill { mask, .. } | CompositeNode::Adjustment { mask, .. } => {
                mask.as_ref().is_some_and(|mask| is_protected(*mask))
            }
            CompositeNode::EmbeddedImage { mask, .. } => {
                mask.as_ref().is_some_and(|mask| is_protected(*mask))
            }
            CompositeNode::Group(child_group) => {
                group_contains_protected_surface(child_group, is_protected)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        composite::LayerBlendMode,
        surface::{CompositeProps, LayerTree},
    };

    fn props() -> CompositeProps {
        CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        }
    }

    fn test_surfaces() -> (PaintSurfaceId, PaintSurfaceId, PaintSurfaceId) {
        let mut layers = LayerTree::new_default_raster();
        let first = layers.default_raster_layer().unwrap();
        let second = layers.add_raster_layer_above(first, "Second").unwrap();
        let third = layers.add_raster_layer_above(second, "Third").unwrap();
        (
            PaintSurfaceId::raster(0.into(), first),
            PaintSurfaceId::raster(0.into(), second),
            PaintSurfaceId::layer_mask(0.into(), third),
        )
    }

    #[test]
    fn removes_known_empty_rasters() {
        let (empty, retained, _) = test_surfaces();
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![
                    CompositeNode::Raster {
                        surface: empty,
                        mask: None,
                        props: props(),
                    },
                    CompositeNode::Raster {
                        surface: retained,
                        mask: None,
                        props: props(),
                    },
                ],
            },
        };

        let stats = prune_known_empty(&mut tree, &|surface| surface == empty, &|_| false);

        assert_eq!(stats.raster_skips, 1);
        assert_eq!(tree.root.children.len(), 1);
        assert!(matches!(
            tree.root.children[0],
            CompositeNode::Raster { surface, .. } if surface == retained
        ));
    }

    #[test]
    fn removes_nodes_suppressed_by_known_zero_masks() {
        let (surface, _, zero_mask) = test_surfaces();
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![CompositeNode::Raster {
                    surface,
                    mask: Some(zero_mask),
                    props: props(),
                }],
            },
        };

        let stats = prune_known_empty(&mut tree, &|candidate| candidate == zero_mask, &|_| false);

        assert_eq!(stats.zero_mask_skips, 1);
        assert!(tree.root.children.is_empty());
    }

    #[test]
    fn does_not_apply_known_zero_mask_pruning_to_solid_fill() {
        let (_, _, zero_mask) = test_surfaces();
        let mut layers = LayerTree::new_default_raster();
        let raster = layers.default_raster_layer().unwrap();
        let fill = layers
            .add_solid_fill_layer_above(raster, "Fill", [0.25, 0.5, 0.75])
            .unwrap();
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![CompositeNode::SolidFill {
                    layer_id: fill,
                    color: [0.25, 0.5, 0.75],
                    mask: Some(zero_mask),
                    props: props(),
                }],
            },
        };

        let stats = prune_known_empty(&mut tree, &|candidate| candidate == zero_mask, &|_| false);

        assert_eq!(stats, KnownEmptyPruneStats::default());
        assert!(matches!(
            tree.root.children.as_slice(),
            [CompositeNode::SolidFill { layer_id, .. }] if *layer_id == fill
        ));
    }

    #[test]
    fn keeps_active_empty_surfaces_in_the_composite_plan() {
        let (active, _, _) = test_surfaces();
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![CompositeNode::Raster {
                    surface: active,
                    mask: None,
                    props: props(),
                }],
            },
        };

        let stats = prune_known_empty(&mut tree, &|surface| surface == active, &|surface| {
            surface == active
        });

        assert_eq!(stats, KnownEmptyPruneStats::default());
        assert_eq!(tree.root.children.len(), 1);
    }
    #[test]
    fn flattens_identity_pass_through_groups_in_stack_order() {
        let (below, inside, above) = test_surfaces();
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![
                    CompositeNode::Raster {
                        surface: below,
                        mask: None,
                        props: props(),
                    },
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::PassThrough,
                        props: props(),
                        mask: None,
                        children: vec![CompositeNode::Raster {
                            surface: inside,
                            mask: None,
                            props: props(),
                        }],
                    }),
                    CompositeNode::Raster {
                        surface: above,
                        mask: None,
                        props: props(),
                    },
                ],
            },
        };

        assert_eq!(flatten_identity_pass_through_groups(&mut tree), 1);
        assert_eq!(tree.root.children.len(), 3);
        let surfaces = tree
            .root
            .children
            .iter()
            .filter_map(|node| match node {
                CompositeNode::Raster { surface, .. } => Some(*surface),
                CompositeNode::SolidFill { .. }
                | CompositeNode::EmbeddedImage { .. }
                | CompositeNode::Adjustment { .. }
                | CompositeNode::Group(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(surfaces, vec![below, inside, above]);
    }

    #[test]
    fn keeps_non_identity_pass_through_group_boundaries() {
        let (surface, _, mask) = test_surfaces();
        let mut group = CompositeGroup {
            layer_id: None,
            mode: GroupCompositeMode::PassThrough,
            props: props(),
            mask: Some(mask),
            children: vec![CompositeNode::Raster {
                surface,
                mask: None,
                props: props(),
            }],
        };
        let mut tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: props(),
                mask: None,
                children: vec![CompositeNode::Group(group.clone())],
            },
        };

        assert_eq!(flatten_identity_pass_through_groups(&mut tree), 0);
        assert!(matches!(tree.root.children[0], CompositeNode::Group(_)));

        group.mask = None;
        group.props.opacity = 0.5;
        tree.root.children = vec![CompositeNode::Group(group)];
        assert_eq!(flatten_identity_pass_through_groups(&mut tree), 0);
        assert!(matches!(tree.root.children[0], CompositeNode::Group(_)));
    }
}
