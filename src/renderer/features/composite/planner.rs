use std::collections::HashSet;

use crate::{
    core::{
        adjustment::{Adjustment, UvMirrorAdjustment, UvMirrorAxis},
        composite::LayerBlendMode,
        geometry::RectU32,
        surface::{CompositeGroup, CompositeNode, CompositeTree, PaintSurfaceId},
    },
    renderer::features::composite::rects::normalize_rects_to_texture,
};

const PARTIAL_COMPOSITE_MAX_AREA_RATIO: f32 = 0.35;
const PARTIAL_COMPOSITE_MAX_RECTS: usize = 64;
const OPACITY_EPSILON: f32 = 0.000001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialCompositeDecision {
    Partial,
    FullFallback,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GroupDamageDependency {
    Local,
    SingleUvMirror(UvMirrorAdjustment),
    /// Multiple or nested non-local adjustments require conservative invalidation.
    FullFallback,
}

pub fn normalize_composite_clip_rects(texture_size: [u32; 2], rects: &[RectU32]) -> Vec<RectU32> {
    normalize_rects_to_texture(texture_size, rects)
}

pub(crate) fn partial_composite_working_clips(
    texture_size: [u32; 2],
    tree: &CompositeTree,
    rects: &[RectU32],
) -> Vec<RectU32> {
    let mut clips = normalize_composite_clip_rects(texture_size, rects);
    let Some(mirror) = root_uv_mirror_adjustment(tree) else {
        return clips;
    };

    let mirrored: Vec<_> = clips
        .iter()
        .filter_map(|rect| mirror_rect_for_uv_mirror(texture_size, *rect, mirror))
        .collect();
    clips.extend(mirrored);
    normalize_composite_clip_rects(texture_size, &clips)
}

pub(crate) fn root_uv_mirror_adjustment(tree: &CompositeTree) -> Option<UvMirrorAdjustment> {
    let mut found = None;
    for child in &tree.root.children {
        if let CompositeNode::Adjustment {
            adjustment: Adjustment::UvMirror(value),
            props,
            ..
        } = child
            && props.visible
            && props.opacity > 0.0
        {
            if found.is_some() {
                return None;
            }
            found = Some(*value);
        }
    }
    found
}

pub(crate) fn group_damage_dependency(nodes: &[CompositeNode]) -> GroupDamageDependency {
    let mut direct_mirror = None;
    for node in nodes {
        match node {
            CompositeNode::Adjustment {
                adjustment: Adjustment::UvMirror(value),
                props,
                ..
            } if props.visible && props.opacity > 0.0 => {
                if direct_mirror.is_some() {
                    return GroupDamageDependency::FullFallback;
                }
                direct_mirror = Some(*value);
            }
            CompositeNode::Group(group)
                if group.props.visible
                    && group.props.opacity > 0.0
                    && nodes_contain_active_uv_mirror(&group.children) =>
            {
                return GroupDamageDependency::FullFallback;
            }
            _ => {}
        }
    }
    direct_mirror.map_or(GroupDamageDependency::Local, |mirror| {
        GroupDamageDependency::SingleUvMirror(mirror)
    })
}

fn nodes_contain_active_uv_mirror(nodes: &[CompositeNode]) -> bool {
    nodes.iter().any(|node| match node {
        CompositeNode::Adjustment {
            adjustment: Adjustment::UvMirror(_),
            props,
            ..
        } => props.visible && props.opacity > 0.0,
        CompositeNode::Group(group) => {
            group.props.visible
                && group.props.opacity > 0.0
                && nodes_contain_active_uv_mirror(&group.children)
        }
        _ => false,
    })
}

pub fn partial_composite_decision(
    texture_size: [u32; 2],
    tree: &CompositeTree,
    clips: &[RectU32],
) -> PartialCompositeDecision {
    if clips.is_empty() {
        return PartialCompositeDecision::Skip;
    }
    if clips.len() > PARTIAL_COMPOSITE_MAX_RECTS || !tree_supports_partial_composite(tree) {
        return PartialCompositeDecision::FullFallback;
    }
    let texture_area = (texture_size[0] as usize).saturating_mul(texture_size[1] as usize);
    if texture_area == 0 {
        return PartialCompositeDecision::FullFallback;
    }
    if total_rect_area(clips) as f32 / texture_area as f32 >= PARTIAL_COMPOSITE_MAX_AREA_RATIO {
        return PartialCompositeDecision::FullFallback;
    }
    PartialCompositeDecision::Partial
}

pub fn tree_supports_partial_composite(tree: &CompositeTree) -> bool {
    root_group_supports_partial_composite(&tree.root)
        && root_children_support_clipped_partial(&tree.root)
}

fn root_group_supports_partial_composite(group: &CompositeGroup) -> bool {
    group.props.visible
        && (group.props.opacity - 1.0).abs() <= OPACITY_EPSILON
        && group.props.blend_mode == LayerBlendMode::Normal
        && group.mask.is_none()
}

fn root_children_support_clipped_partial(group: &CompositeGroup) -> bool {
    let mut uv_mirror_count = 0usize;
    for child in &group.children {
        match child {
            CompositeNode::Adjustment {
                adjustment: Adjustment::UvMirror(_),
                props,
                ..
            } if props.visible && props.opacity > 0.0 => {
                uv_mirror_count = uv_mirror_count.saturating_add(1);
                if uv_mirror_count > 1 {
                    return false;
                }
            }
            CompositeNode::Group(group) if !group_children_support_clipped_partial(group) => {
                return false;
            }
            _ => {}
        }
    }
    true
}

fn group_children_support_clipped_partial(group: &CompositeGroup) -> bool {
    group.children.iter().all(node_supports_clipped_partial)
}

fn node_supports_clipped_partial(node: &CompositeNode) -> bool {
    match node {
        CompositeNode::Raster { .. }
        | CompositeNode::EmbeddedImage { .. }
        | CompositeNode::SolidFill { .. } => true,
        CompositeNode::Adjustment { adjustment, .. } => {
            !matches!(adjustment, Adjustment::UvMirror(_))
        }
        CompositeNode::Group(group) => group_children_support_clipped_partial(group),
    }
}

pub(crate) fn mirror_rect_for_uv_mirror(
    texture_size: [u32; 2],
    rect: RectU32,
    mirror: UvMirrorAdjustment,
) -> Option<RectU32> {
    let axis_index = match mirror.axis {
        UvMirrorAxis::X => 0,
        UvMirrorAxis::Y => 1,
    };
    let extent = texture_size[axis_index];
    if extent == 0 {
        return None;
    }

    let start = rect.origin[axis_index].min(extent);
    let end = rect.origin[axis_index]
        .saturating_add(rect.size[axis_index])
        .min(extent);
    if start >= end {
        return None;
    }

    // Keep the mirror axis on a half-texel grid. The shader uses the same snap,
    // so reflected texel centers always land exactly on texel centers and the
    // partial dependency is the exact reflected interval. Expanding only the
    // mirrored destination would make its guard pixels sample outside the
    // rebuilt source clip, which can expose recycled transient-texture contents.
    let axis_twice = (mirror.position.clamp(0.0, 1.0) * extent as f32 * 2.0)
        .round()
        .clamp(0.0, extent as f32 * 2.0) as i64;
    let mirrored_start = (axis_twice - end as i64).clamp(0, extent as i64) as u32;
    let mirrored_end = (axis_twice - start as i64).clamp(0, extent as i64) as u32;
    if mirrored_start >= mirrored_end {
        return None;
    }

    let mut mirrored = rect;
    mirrored.origin[axis_index] = mirrored_start;
    mirrored.size[axis_index] = mirrored_end - mirrored_start;
    Some(mirrored)
}

pub fn visible_surfaces_from_composite_tree(tree: &CompositeTree) -> HashSet<PaintSurfaceId> {
    let mut surfaces = HashSet::new();
    collect_visible_surfaces(&tree.root, &mut surfaces);
    surfaces
}

pub(crate) fn total_rect_area(rects: &[RectU32]) -> usize {
    rects.iter().fold(0usize, |area, rect| {
        area.saturating_add((rect.size[0] as usize).saturating_mul(rect.size[1] as usize))
    })
}

fn collect_visible_surfaces(group: &CompositeGroup, surfaces: &mut HashSet<PaintSurfaceId>) {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return;
    }
    if let Some(mask) = group.mask {
        surfaces.insert(mask);
    }
    for child in &group.children {
        match child {
            CompositeNode::Raster {
                surface,
                mask,
                props,
            } => {
                if props.visible && props.opacity > 0.0 {
                    surfaces.insert(*surface);
                    if let Some(mask) = mask {
                        surfaces.insert(*mask);
                    }
                }
            }
            CompositeNode::SolidFill { mask, props, .. }
            | CompositeNode::EmbeddedImage { mask, props, .. }
            | CompositeNode::Adjustment { mask, props, .. } => {
                if props.visible && props.opacity > 0.0 {
                    if let Some(mask) = mask {
                        surfaces.insert(*mask);
                    }
                }
            }
            CompositeNode::Group(group) => collect_visible_surfaces(group, surfaces),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        adjustment::{UvMirrorAdjustment, UvMirrorAxis},
        composite::{GroupCompositeMode, LayerBlendMode},
        surface::{CompositeProps, LayerId},
    };
    use slotmap::SlotMap;

    fn layer_ids(count: usize) -> Vec<LayerId> {
        let mut slots: SlotMap<LayerId, ()> = SlotMap::with_key();
        (0..count).map(|_| slots.insert(())).collect()
    }

    fn props(blend_mode: LayerBlendMode) -> CompositeProps {
        CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode,
        }
    }

    fn normal_props() -> CompositeProps {
        props(LayerBlendMode::Normal)
    }

    fn raster(
        material_index: usize,
        layer_id: LayerId,
        blend_mode: LayerBlendMode,
    ) -> CompositeNode {
        CompositeNode::Raster {
            surface: PaintSurfaceId::raster(material_index.into(), layer_id),
            mask: None,
            props: props(blend_mode),
        }
    }

    fn group(children: Vec<CompositeNode>) -> CompositeNode {
        CompositeNode::Group(CompositeGroup {
            layer_id: None,
            mode: GroupCompositeMode::Isolated,
            props: normal_props(),
            mask: None,
            children,
        })
    }

    fn tree(children: Vec<CompositeNode>) -> CompositeTree {
        CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children,
            },
        }
    }

    fn clip(origin: [u32; 2], size: [u32; 2]) -> RectU32 {
        RectU32 { origin, size }
    }

    #[test]
    fn partial_allows_flat_shader_blend_raster() {
        let ids = layer_ids(2);
        let tree = tree(vec![
            raster(0, ids[0], LayerBlendMode::Normal),
            raster(0, ids[1], LayerBlendMode::Multiply),
        ]);
        let clips = [clip([0, 0], [16, 16])];

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::Partial
        );
    }

    #[test]
    fn partial_allows_nested_normal_group() {
        let ids = layer_ids(1);
        let tree = tree(vec![group(vec![raster(0, ids[0], LayerBlendMode::Normal)])]);
        let clips = [clip([8, 8], [16, 16])];

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::Partial
        );
    }

    #[test]
    fn partial_allows_nested_shader_blend_group() {
        let ids = layer_ids(2);
        let tree = tree(vec![
            raster(0, ids[0], LayerBlendMode::Normal),
            group(vec![raster(0, ids[1], LayerBlendMode::Screen)]),
        ]);
        let clips = [clip([8, 8], [16, 16])];

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::Partial
        );
    }

    fn uv_mirror(layer_id: LayerId, value: UvMirrorAdjustment) -> CompositeNode {
        CompositeNode::Adjustment {
            layer_id,
            adjustment: Adjustment::UvMirror(value),
            mask: None,
            props: normal_props(),
        }
    }

    #[test]
    fn partial_allows_one_root_uv_mirror() {
        let ids = layer_ids(1);
        let tree = tree(vec![uv_mirror(ids[0], UvMirrorAdjustment::default())]);
        let clips = partial_composite_working_clips([128, 128], &tree, &[clip([96, 40], [8, 12])]);

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::Partial
        );
        assert_eq!(
            clips,
            vec![clip([24, 40], [8, 12]), clip([96, 40], [8, 12])]
        );
    }

    #[test]
    fn partial_mirrors_y_dependencies_around_off_center_axis() {
        let ids = layer_ids(1);
        let tree = tree(vec![uv_mirror(
            ids[0],
            UvMirrorAdjustment {
                axis: UvMirrorAxis::Y,
                position: 0.25,
                ..UvMirrorAdjustment::default()
            },
        )]);

        let clips = partial_composite_working_clips([128, 128], &tree, &[clip([20, 40], [12, 8])]);

        assert_eq!(
            clips,
            vec![clip([20, 16], [12, 8]), clip([20, 40], [12, 8])]
        );
    }

    #[test]
    fn partial_snaps_fractional_uv_mirror_axis_without_unpaired_guard_pixels() {
        let ids = layer_ids(1);
        let tree = tree(vec![uv_mirror(
            ids[0],
            UvMirrorAdjustment {
                position: 0.53,
                ..UvMirrorAdjustment::default()
            },
        )]);

        let clips = partial_composite_working_clips([128, 128], &tree, &[clip([96, 40], [8, 12])]);

        // 0.53 * 128 * 2 = 135.68, which snaps to 136 => axis x = 68 texels.
        // [96, 104) therefore reflects exactly to [32, 40), with no destination-only guard.
        assert_eq!(
            clips,
            vec![clip([32, 40], [8, 12]), clip([96, 40], [8, 12])]
        );
    }

    #[test]
    fn partial_applies_area_threshold_after_uv_mirror_expansion() {
        let ids = layer_ids(1);
        let tree = tree(vec![uv_mirror(ids[0], UvMirrorAdjustment::default())]);
        let clips = partial_composite_working_clips([128, 128], &tree, &[clip([96, 0], [24, 128])]);

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::FullFallback
        );
    }

    #[test]
    fn partial_rejects_multiple_root_uv_mirror_adjustments() {
        let ids = layer_ids(2);
        let tree = tree(vec![
            uv_mirror(ids[0], UvMirrorAdjustment::default()),
            uv_mirror(ids[1], UvMirrorAdjustment::default()),
        ]);

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &[clip([0, 0], [8, 8])]),
            PartialCompositeDecision::FullFallback
        );
    }

    #[test]
    fn partial_rejects_nested_uv_mirror_adjustment() {
        let ids = layer_ids(1);
        let tree = tree(vec![group(vec![uv_mirror(
            ids[0],
            UvMirrorAdjustment::default(),
        )])]);

        assert_eq!(
            partial_composite_decision([128, 128], &tree, &[clip([0, 0], [8, 8])]),
            PartialCompositeDecision::FullFallback
        );
    }

    #[test]
    fn partial_rejects_non_identity_root() {
        let ids = layer_ids(1);
        let mut tree = tree(vec![raster(0, ids[0], LayerBlendMode::Normal)]);
        let clips = [clip([0, 0], [8, 8])];

        tree.root.props.opacity = 0.5;
        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::FullFallback
        );

        tree.root.props.opacity = 1.0;
        tree.root.props.blend_mode = LayerBlendMode::Multiply;
        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::FullFallback
        );

        tree.root.props.blend_mode = LayerBlendMode::Normal;
        tree.root.mask = Some(PaintSurfaceId::raster(0.into(), ids[0]));
        assert_eq!(
            partial_composite_decision([128, 128], &tree, &clips),
            PartialCompositeDecision::FullFallback
        );
    }
}
