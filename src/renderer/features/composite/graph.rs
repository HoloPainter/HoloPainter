use crate::core::{
    composite::LayerBlendMode,
    surface::{CompositeGroup, CompositeNode, CompositeProps, CompositeTree, PaintSurfaceId},
};

const OPACITY_EPSILON: f32 = 0.000001;

#[derive(Debug, Clone, Copy)]
pub(super) enum ActiveAboveStep<'a> {
    SourceOverRun(&'a [CompositeNode]),
    NonNormalSource(&'a CompositeNode),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ActiveBoundaryComposite {
    pub(super) props: CompositeProps,
    pub(super) mask: Option<PaintSurfaceId>,
}

#[derive(Debug, Clone)]
pub(super) struct ActiveRunBoundaryPlan<'a> {
    pub(super) below: &'a [CompositeNode],
    pub(super) below_uses_shader_blending: bool,
    pub(super) active_child: ActiveBoundaryComposite,
    pub(super) above_steps: Vec<ActiveAboveStep<'a>>,
    pub(super) has_non_normal_above: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ActiveRunLeaf<'a> {
    Raster(&'a CompositeNode),
    GroupMaskInner,
}

#[derive(Debug, Clone)]
pub(super) struct ActiveRunPlan<'a> {
    pub(super) active: ActiveRunLeaf<'a>,
    /// Boundary plans ordered from the group that directly contains the active
    /// raster/mask to the root boundary. Each boundary bakes the active-path
    /// independent siblings into below/above checkpoints and keeps the
    /// active-containing child dynamic.
    pub(super) boundaries: Vec<ActiveRunBoundaryPlan<'a>>,
    pub(super) has_non_normal_above: bool,
}

#[derive(Debug, Clone)]
pub(super) enum ActiveSetStep<'a> {
    StaticRun {
        nodes: &'a [CompositeNode],
        checkpoint_index: usize,
        uses_shader_blending: bool,
    },
    ActiveRaster(&'a CompositeNode),
    ActiveGroup {
        group: &'a CompositeGroup,
        plan: ActiveSetRunPlan<'a>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct ActiveSetRunPlan<'a> {
    pub(super) steps: Vec<ActiveSetStep<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveCompositePlanMiss {
    NoActiveSurface,
    UnsupportedRootProps,
    UnsupportedGroupProps,
    UnsupportedNonNormalAboveActive,
}

impl<'a> ActiveSetRunPlan<'a> {
    pub(super) fn checkpoint_count(&self) -> usize {
        self.steps
            .iter()
            .map(|step| match step {
                ActiveSetStep::StaticRun { .. } => 1,
                ActiveSetStep::ActiveRaster(_) => 0,
                ActiveSetStep::ActiveGroup { plan, .. } => plan.checkpoint_count(),
            })
            .sum()
    }

    pub(super) fn boundary_checkpoint_shape(&self) -> Vec<usize> {
        vec![0; self.checkpoint_count()]
    }
}

impl<'a> ActiveRunPlan<'a> {
    pub(super) fn checkpoint_count(&self) -> usize {
        self.boundaries
            .iter()
            .map(|boundary| boundary.above_steps.len())
            .sum()
    }

    pub(super) fn boundary_checkpoint_shape(&self) -> Vec<usize> {
        self.boundaries
            .iter()
            .map(|boundary| boundary.above_steps.len())
            .collect()
    }
}

/// Builds an active-set plan for strokes that edit more than one surface in the
/// same material. Static sibling runs are baked into checkpoint textures;
/// active rasters and active-containing groups remain dynamic and recursive.
#[cfg(test)]
fn build_flat_normal_active_set_run_plan<'a>(
    tree: &'a CompositeTree,
    active_surfaces: &[PaintSurfaceId],
) -> Option<ActiveSetRunPlan<'a>> {
    build_active_set_run_plan(tree, active_surfaces).ok()
}

pub(super) fn build_active_set_run_plan<'a>(
    tree: &'a CompositeTree,
    active_surfaces: &[PaintSurfaceId],
) -> Result<ActiveSetRunPlan<'a>, ActiveCompositePlanMiss> {
    if active_surfaces.is_empty() {
        return Err(ActiveCompositePlanMiss::NoActiveSurface);
    }
    if !identity_normal_props(tree.root.props) {
        return Err(ActiveCompositePlanMiss::UnsupportedRootProps);
    }

    let mut checkpoint_index = 0;
    build_active_set_plan_in_group(&tree.root, active_surfaces, &mut checkpoint_index)?
        .ok_or(ActiveCompositePlanMiss::NoActiveSurface)
}

fn build_active_set_plan_in_group<'a>(
    group: &'a CompositeGroup,
    active_surfaces: &[PaintSurfaceId],
    checkpoint_index: &mut usize,
) -> Result<Option<ActiveSetRunPlan<'a>>, ActiveCompositePlanMiss> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return Ok(None);
    }

    if group
        .mask
        .is_some_and(|mask| active_surfaces.contains(&mask))
    {
        let steps = if group.children.iter().any(node_props_require_source) {
            let checkpoint_index_value = *checkpoint_index;
            *checkpoint_index += 1;
            vec![ActiveSetStep::StaticRun {
                nodes: &group.children,
                checkpoint_index: checkpoint_index_value,
                uses_shader_blending: group.children.iter().any(node_uses_shader_blend_mode),
            }]
        } else {
            Vec::new()
        };
        return Ok(Some(ActiveSetRunPlan { steps }));
    }

    let mut steps = Vec::new();
    let mut static_run_start: Option<usize> = None;
    let mut active_count = 0;
    let mut has_dynamic_active = false;

    for (index, child) in group.children.iter().enumerate() {
        match child {
            CompositeNode::Raster { surface, mask, .. }
                if active_surfaces.contains(surface)
                    || mask.is_some_and(|mask| active_surfaces.contains(&mask)) =>
            {
                flush_active_set_static_run(
                    group,
                    index,
                    &mut static_run_start,
                    checkpoint_index,
                    &mut steps,
                    has_dynamic_active,
                )?;
                steps.push(ActiveSetStep::ActiveRaster(child));
                active_count += 1;
                has_dynamic_active = true;
            }
            CompositeNode::SolidFill { mask, .. } | CompositeNode::Adjustment { mask, .. }
                if mask.is_some_and(|mask| active_surfaces.contains(&mask)) =>
            {
                flush_active_set_static_run(
                    group,
                    index,
                    &mut static_run_start,
                    checkpoint_index,
                    &mut steps,
                    has_dynamic_active,
                )?;
                steps.push(ActiveSetStep::ActiveRaster(child));
                active_count += 1;
                has_dynamic_active = true;
            }
            CompositeNode::Group(child_group)
                if active_surfaces
                    .iter()
                    .any(|surface| group_contains_surface(child_group, *surface)) =>
            {
                if !node_props_require_source(child) {
                    continue;
                }
                if child_group.props.blend_mode != LayerBlendMode::Normal {
                    return Err(ActiveCompositePlanMiss::UnsupportedGroupProps);
                }
                let child_plan =
                    build_active_set_plan_in_group(child_group, active_surfaces, checkpoint_index)?
                        .ok_or(ActiveCompositePlanMiss::NoActiveSurface)?;
                flush_active_set_static_run(
                    group,
                    index,
                    &mut static_run_start,
                    checkpoint_index,
                    &mut steps,
                    has_dynamic_active,
                )?;
                steps.push(ActiveSetStep::ActiveGroup {
                    group: child_group,
                    plan: child_plan,
                });
                active_count += 1;
                has_dynamic_active = true;
            }
            _ if node_props_require_source(child) => {
                static_run_start.get_or_insert(index);
            }
            _ => {}
        }
    }

    flush_active_set_static_run(
        group,
        group.children.len(),
        &mut static_run_start,
        checkpoint_index,
        &mut steps,
        has_dynamic_active,
    )?;

    Ok((active_count > 0).then_some(ActiveSetRunPlan { steps }))
}

fn flush_active_set_static_run<'a>(
    group: &'a CompositeGroup,
    end: usize,
    static_run_start: &mut Option<usize>,
    checkpoint_index: &mut usize,
    steps: &mut Vec<ActiveSetStep<'a>>,
    after_dynamic_active: bool,
) -> Result<(), ActiveCompositePlanMiss> {
    let Some(start) = static_run_start.take() else {
        return Ok(());
    };
    let nodes = &group.children[start..end];
    if after_dynamic_active && nodes.iter().any(node_uses_shader_blend_mode) {
        return Err(ActiveCompositePlanMiss::UnsupportedNonNormalAboveActive);
    }
    steps.push(ActiveSetStep::StaticRun {
        nodes,
        checkpoint_index: *checkpoint_index,
        uses_shader_blending: nodes.iter().any(node_uses_shader_blend_mode),
    });
    *checkpoint_index += 1;
    Ok(())
}

/// Builds the optimized active-run evaluation shape around the active raster or
/// mask surface. Active leaves may live inside nested Normal groups. The plan
/// records one boundary for every group on the active path so stroke-time
/// rendering can fold the dynamic active source from the innermost group back to
/// the root without resolving active-independent siblings from CPU shadows.
pub(super) fn build_flat_normal_active_run_plan<'a>(
    tree: &'a CompositeTree,
    active_surface: PaintSurfaceId,
) -> Option<ActiveRunPlan<'a>> {
    if !identity_normal_props(tree.root.props) {
        return None;
    }

    build_active_run_plan_in_group(&tree.root, active_surface).map(|result| result.plan)
}

struct ActiveRunBuildResult<'a> {
    plan: ActiveRunPlan<'a>,
}

fn build_active_run_plan_in_group<'a>(
    group: &'a CompositeGroup,
    active_surface: PaintSurfaceId,
) -> Option<ActiveRunBuildResult<'a>> {
    if !group.props.visible || group.props.opacity <= 0.0 {
        return None;
    }

    if group.mask == Some(active_surface) {
        let boundary = build_group_mask_inner_boundary_plan(group);
        return Some(ActiveRunBuildResult {
            plan: ActiveRunPlan {
                active: ActiveRunLeaf::GroupMaskInner,
                has_non_normal_above: boundary.has_non_normal_above,
                boundaries: vec![boundary],
            },
        });
    }

    for (active_child_index, child) in group.children.iter().enumerate() {
        match child {
            CompositeNode::Raster {
                surface,
                mask,
                props,
            } => {
                if *surface != active_surface && *mask != Some(active_surface) {
                    continue;
                }
                let boundary = build_boundary_plan(
                    group,
                    active_child_index,
                    ActiveBoundaryComposite {
                        props: *props,
                        mask: None,
                    },
                )?;
                return Some(ActiveRunBuildResult {
                    plan: ActiveRunPlan {
                        active: ActiveRunLeaf::Raster(child),
                        has_non_normal_above: boundary.has_non_normal_above,
                        boundaries: vec![boundary],
                    },
                });
            }
            CompositeNode::SolidFill { mask, props, .. }
            | CompositeNode::EmbeddedImage { mask, props, .. }
            | CompositeNode::Adjustment { mask, props, .. } => {
                if *mask != Some(active_surface) {
                    continue;
                }
                let boundary = build_boundary_plan(
                    group,
                    active_child_index,
                    ActiveBoundaryComposite {
                        props: *props,
                        mask: None,
                    },
                )?;
                return Some(ActiveRunBuildResult {
                    plan: ActiveRunPlan {
                        active: ActiveRunLeaf::Raster(child),
                        has_non_normal_above: boundary.has_non_normal_above,
                        boundaries: vec![boundary],
                    },
                });
            }
            CompositeNode::Group(child_group) => {
                if !group_contains_surface(child_group, active_surface) {
                    continue;
                }
                if !node_props_require_source(child) {
                    return None;
                }
                let mut result = build_active_run_plan_in_group(child_group, active_surface)?;
                let boundary = build_boundary_plan(
                    group,
                    active_child_index,
                    ActiveBoundaryComposite {
                        props: child_group.props,
                        mask: child_group.mask,
                    },
                )?;
                result.plan.has_non_normal_above |= boundary.has_non_normal_above;
                result.plan.boundaries.push(boundary);
                return Some(result);
            }
        }
    }

    None
}

fn build_boundary_plan<'a>(
    group: &'a CompositeGroup,
    active_child_index: usize,
    active_child: ActiveBoundaryComposite,
) -> Option<ActiveRunBoundaryPlan<'a>> {
    let (below, active_and_above) = group.children.split_at(active_child_index);
    let (_, above) = active_and_above.split_first()?;
    let below_uses_shader_blending = below.iter().any(node_uses_shader_blend_mode);
    let (above_steps, has_non_normal_above) = build_above_steps(above);

    Some(ActiveRunBoundaryPlan {
        below,
        below_uses_shader_blending,
        active_child,
        above_steps,
        has_non_normal_above,
    })
}

fn build_group_mask_inner_boundary_plan<'a>(
    group: &'a CompositeGroup,
) -> ActiveRunBoundaryPlan<'a> {
    ActiveRunBoundaryPlan {
        below: &group.children,
        below_uses_shader_blending: group.children.iter().any(node_uses_shader_blend_mode),
        active_child: ActiveBoundaryComposite {
            props: identity_normal_props_for_composite(),
            mask: None,
        },
        above_steps: Vec::new(),
        has_non_normal_above: false,
    }
}

fn build_above_steps<'a>(above: &'a [CompositeNode]) -> (Vec<ActiveAboveStep<'a>>, bool) {
    let mut above_steps = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut has_non_normal_above = false;

    for (index, node) in above.iter().enumerate() {
        if !node_props_require_source(node) {
            continue;
        }
        if is_source_over_checkpoint_node(node) {
            run_start.get_or_insert(index);
            continue;
        }
        if let Some(start) = run_start.take() {
            above_steps.push(ActiveAboveStep::SourceOverRun(&above[start..index]));
        }
        above_steps.push(ActiveAboveStep::NonNormalSource(node));
        has_non_normal_above = true;
    }
    if let Some(start) = run_start {
        above_steps.push(ActiveAboveStep::SourceOverRun(&above[start..]));
    }

    (above_steps, has_non_normal_above)
}

fn identity_normal_props_for_composite() -> CompositeProps {
    CompositeProps {
        visible: true,
        opacity: 1.0,
        blend_mode: LayerBlendMode::Normal,
    }
}

fn identity_normal_props(props: CompositeProps) -> bool {
    props.visible
        && (props.opacity - 1.0).abs() <= OPACITY_EPSILON
        && props.blend_mode == LayerBlendMode::Normal
}

fn is_source_over_checkpoint_node(node: &CompositeNode) -> bool {
    match node {
        CompositeNode::Raster { props, .. }
        | CompositeNode::EmbeddedImage { props, .. }
        | CompositeNode::SolidFill { props, .. } => props.blend_mode == LayerBlendMode::Normal,
        CompositeNode::Adjustment { .. } => false,
        CompositeNode::Group(group) => group.props.blend_mode == LayerBlendMode::Normal,
    }
}

fn node_uses_shader_blend_mode(node: &CompositeNode) -> bool {
    match node {
        CompositeNode::Raster { props, .. }
        | CompositeNode::EmbeddedImage { props, .. }
        | CompositeNode::SolidFill { props, .. } => props.blend_mode != LayerBlendMode::Normal,
        CompositeNode::Adjustment { .. } => true,
        CompositeNode::Group(group) => {
            group.props.blend_mode != LayerBlendMode::Normal
                || group.children.iter().any(node_uses_shader_blend_mode)
        }
    }
}

fn node_props_require_source(node: &CompositeNode) -> bool {
    match node {
        CompositeNode::Raster { props, .. }
        | CompositeNode::EmbeddedImage { props, .. }
        | CompositeNode::SolidFill { props, .. }
        | CompositeNode::Adjustment { props, .. } => props.visible && props.opacity > 0.0,
        CompositeNode::Group(group) => group.props.visible && group.props.opacity > 0.0,
    }
}

fn group_contains_surface(group: &CompositeGroup, surface: PaintSurfaceId) -> bool {
    group.mask == Some(surface)
        || group.children.iter().any(|child| match child {
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => child.leaf_contains_surface(surface),
            CompositeNode::Group(group) => group_contains_surface(group, surface),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        composite::GroupCompositeMode,
        surface::{LayerId, PaintSurfaceId},
    };
    use slotmap::SlotMap;

    fn layer_ids(count: usize) -> Vec<LayerId> {
        let mut slots: SlotMap<LayerId, ()> = SlotMap::with_key();
        (0..count).map(|_| slots.insert(())).collect()
    }

    fn normal_props() -> CompositeProps {
        CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        }
    }

    fn raster(material_index: usize, layer_id: LayerId) -> CompositeNode {
        CompositeNode::Raster {
            surface: PaintSurfaceId::raster(material_index.into(), layer_id),
            mask: None,
            props: normal_props(),
        }
    }

    fn active_set_step_count(plan: &ActiveSetRunPlan<'_>) -> usize {
        plan.steps.len()
            + plan
                .steps
                .iter()
                .map(|step| match step {
                    ActiveSetStep::ActiveGroup { plan, .. } => active_set_step_count(plan),
                    _ => 0,
                })
                .sum::<usize>()
    }

    #[test]
    fn splits_flat_normal_stack_around_active_raster() {
        let ids = layer_ids(4);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: ids.iter().copied().map(|id| raster(0, id)).collect(),
            },
        };

        let active = PaintSurfaceId::raster(0.into(), ids[2]);
        let plan = build_flat_normal_active_run_plan(&tree, active).expect("plan");

        assert_eq!(plan.boundaries.len(), 1);
        assert_eq!(plan.boundaries[0].below.len(), 2);
        assert!(!plan.boundaries[0].below_uses_shader_blending);
        assert_eq!(plan.boundaries[0].above_steps.len(), 1);
        assert!(
            matches!(plan.boundaries[0].above_steps[0], ActiveAboveStep::SourceOverRun(nodes) if nodes.len() == 1)
        );
        assert!(!plan.has_non_normal_above);
        assert!(
            matches!(plan.active, ActiveRunLeaf::Raster(CompositeNode::Raster { surface, .. }) if *surface == active)
        );
    }

    #[test]
    fn matches_active_mask_surface() {
        let ids = layer_ids(2);
        let active_mask = PaintSurfaceId::layer_mask(0.into(), ids[1]);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Raster {
                        surface: PaintSurfaceId::raster(0.into(), ids[1]),
                        mask: Some(active_mask),
                        props: normal_props(),
                    },
                ],
            },
        };

        let plan = build_flat_normal_active_run_plan(&tree, active_mask).expect("plan");
        assert_eq!(plan.boundaries[0].below.len(), 1);
        assert!(plan.boundaries[0].above_steps.is_empty());
    }

    #[test]
    fn accepts_active_independent_group_as_source_over_run() {
        let ids = layer_ids(2);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: CompositeProps {
                            opacity: 0.5,
                            ..normal_props()
                        },
                        mask: None,
                        children: vec![raster(0, ids[1])],
                    }),
                ],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[0]))
                .expect("plan");
        assert_eq!(plan.boundaries[0].below.len(), 0);
        assert_eq!(plan.boundaries[0].above_steps.len(), 1);
        assert!(
            matches!(plan.boundaries[0].above_steps[0], ActiveAboveStep::SourceOverRun(nodes) if nodes.len() == 1)
        );
    }

    #[test]
    fn accepts_active_raster_inside_single_normal_group() {
        let ids = layer_ids(1);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![CompositeNode::Group(CompositeGroup {
                    layer_id: None,
                    mode: GroupCompositeMode::Isolated,
                    props: normal_props(),
                    mask: None,
                    children: vec![raster(0, ids[0])],
                })],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[0]))
                .expect("plan");
        assert_eq!(plan.boundaries.len(), 2);
        assert_eq!(plan.boundaries[0].below.len(), 0);
        assert_eq!(plan.boundaries[1].below.len(), 0);
    }

    #[test]
    fn accepts_active_mask_inside_single_normal_group() {
        let ids = layer_ids(1);
        let active_mask = PaintSurfaceId::layer_mask(0.into(), ids[0]);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![CompositeNode::Group(CompositeGroup {
                    layer_id: None,
                    mode: GroupCompositeMode::Isolated,
                    props: CompositeProps {
                        opacity: 0.5,
                        ..normal_props()
                    },
                    mask: None,
                    children: vec![CompositeNode::Raster {
                        surface: PaintSurfaceId::raster(0.into(), ids[0]),
                        mask: Some(active_mask),
                        props: normal_props(),
                    }],
                })],
            },
        };

        let plan = build_flat_normal_active_run_plan(&tree, active_mask).expect("plan");
        assert_eq!(plan.boundaries.len(), 2);
        assert_eq!(plan.boundary_checkpoint_shape(), vec![0, 0]);
    }

    #[test]
    fn accepts_active_raster_inside_nested_normal_groups() {
        let ids = layer_ids(5);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: CompositeProps {
                            opacity: 0.75,
                            ..normal_props()
                        },
                        mask: None,
                        children: vec![
                            raster(0, ids[1]),
                            CompositeNode::Group(CompositeGroup {
                                layer_id: None,
                                mode: GroupCompositeMode::Isolated,
                                props: normal_props(),
                                mask: None,
                                children: vec![raster(0, ids[2]), raster(0, ids[3])],
                            }),
                        ],
                    }),
                    raster(0, ids[4]),
                ],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[3]))
                .expect("plan");
        assert_eq!(plan.boundaries.len(), 3);
        assert_eq!(plan.boundaries[0].below.len(), 1);
        assert_eq!(plan.boundaries[1].below.len(), 1);
        assert_eq!(plan.boundaries[2].below.len(), 1);
        assert_eq!(plan.boundary_checkpoint_shape(), vec![0, 0, 1]);
    }

    #[test]
    fn accepts_non_normal_layers_above_active_as_source_cache_steps() {
        let ids = layer_ids(4);
        let mut non_normal = raster(0, ids[2]);
        let CompositeNode::Raster { props, .. } = &mut non_normal else {
            unreachable!();
        };
        props.blend_mode = LayerBlendMode::Multiply;
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    raster(0, ids[1]),
                    non_normal,
                    raster(0, ids[3]),
                ],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[1]))
                .expect("plan");
        assert_eq!(plan.boundaries[0].below.len(), 1);
        assert!(plan.has_non_normal_above);
        assert_eq!(plan.boundaries[0].above_steps.len(), 2);
        assert!(matches!(
            plan.boundaries[0].above_steps[0],
            ActiveAboveStep::NonNormalSource(_)
        ));
        assert!(
            matches!(plan.boundaries[0].above_steps[1], ActiveAboveStep::SourceOverRun(nodes) if nodes.len() == 1)
        );
    }

    #[test]
    fn accepts_non_normal_group_above_active_as_source_cache_step() {
        let ids = layer_ids(2);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: CompositeProps {
                            blend_mode: LayerBlendMode::Multiply,
                            ..normal_props()
                        },
                        mask: None,
                        children: vec![raster(0, ids[1])],
                    }),
                ],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[0]))
                .expect("plan");
        assert!(plan.has_non_normal_above);
        assert_eq!(plan.boundaries[0].above_steps.len(), 1);
        assert!(
            matches!(plan.boundaries[0].above_steps[0], ActiveAboveStep::NonNormalSource(node) if matches!(node, CompositeNode::Group(_)))
        );
    }

    #[test]
    fn accepts_non_normal_layers_below_active_as_stable_shader_checkpoint() {
        let ids = layer_ids(2);
        let mut non_normal = raster(0, ids[0]);
        let CompositeNode::Raster { props, .. } = &mut non_normal else {
            unreachable!();
        };
        props.blend_mode = LayerBlendMode::Multiply;
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![non_normal, raster(0, ids[1])],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[1]))
                .expect("plan");
        assert_eq!(plan.boundaries[0].below.len(), 1);
        assert!(plan.boundaries[0].below_uses_shader_blending);
    }

    #[test]
    fn accepts_active_non_normal_raster_as_dynamic_blend_step() {
        let ids = layer_ids(1);
        let mut active = raster(0, ids[0]);
        let CompositeNode::Raster { props, .. } = &mut active else {
            unreachable!();
        };
        props.blend_mode = LayerBlendMode::Multiply;
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![active],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[0]))
                .expect("plan");
        assert_eq!(plan.boundaries.len(), 1);
        assert_eq!(
            plan.boundaries[0].active_child.props.blend_mode,
            LayerBlendMode::Multiply
        );
    }

    #[test]
    fn accepts_active_inside_non_normal_ancestor_group() {
        let ids = layer_ids(1);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![CompositeNode::Group(CompositeGroup {
                    layer_id: None,
                    mode: GroupCompositeMode::Isolated,
                    props: CompositeProps {
                        blend_mode: LayerBlendMode::Multiply,
                        ..normal_props()
                    },
                    mask: None,
                    children: vec![raster(0, ids[0])],
                })],
            },
        };

        let plan =
            build_flat_normal_active_run_plan(&tree, PaintSurfaceId::raster(0.into(), ids[0]))
                .expect("plan");
        assert_eq!(plan.boundaries.len(), 2);
        assert_eq!(
            plan.boundaries[1].active_child.props.blend_mode,
            LayerBlendMode::Multiply
        );
    }

    #[test]
    fn accepts_active_group_mask_inside_root_stack() {
        let ids = layer_ids(3);
        let group_mask = PaintSurfaceId::layer_mask(0.into(), ids[1]);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: Some(ids[1]),
                        mode: GroupCompositeMode::Isolated,
                        props: CompositeProps {
                            opacity: 0.75,
                            ..normal_props()
                        },
                        mask: Some(group_mask),
                        children: vec![raster(0, ids[2])],
                    }),
                ],
            },
        };

        let plan = build_flat_normal_active_run_plan(&tree, group_mask).expect("plan");

        assert!(matches!(plan.active, ActiveRunLeaf::GroupMaskInner));
        assert_eq!(plan.boundaries.len(), 2);
        assert_eq!(plan.boundaries[0].below.len(), 1);
        assert_eq!(plan.boundaries[1].below.len(), 1);
        assert_eq!(plan.boundaries[1].active_child.mask, Some(group_mask));
        assert_eq!(plan.boundaries[1].active_child.props.opacity, 0.75);
    }

    #[test]
    fn active_set_accepts_nested_group_with_multiple_active_surfaces() {
        let ids = layer_ids(5);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    raster(0, ids[0]),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: CompositeProps {
                            opacity: 0.75,
                            ..normal_props()
                        },
                        mask: None,
                        children: vec![raster(0, ids[1]), raster(0, ids[2]), raster(0, ids[3])],
                    }),
                    raster(0, ids[4]),
                ],
            },
        };

        let plan = build_flat_normal_active_set_run_plan(
            &tree,
            &[
                PaintSurfaceId::raster(0.into(), ids[1]),
                PaintSurfaceId::raster(0.into(), ids[3]),
            ],
        )
        .expect("plan");

        assert_eq!(plan.checkpoint_count(), 3);
        assert_eq!(active_set_step_count(&plan), 6);
        assert!(matches!(
            plan.steps[1],
            ActiveSetStep::ActiveGroup { group, .. } if (group.props.opacity - 0.75).abs() < 0.000001
        ));
    }

    #[test]
    fn active_set_accepts_active_surfaces_spread_across_sibling_groups() {
        let ids = layer_ids(4);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: normal_props(),
                        mask: None,
                        children: vec![raster(0, ids[0]), raster(0, ids[1])],
                    }),
                    CompositeNode::Group(CompositeGroup {
                        layer_id: None,
                        mode: GroupCompositeMode::Isolated,
                        props: normal_props(),
                        mask: None,
                        children: vec![raster(0, ids[2]), raster(0, ids[3])],
                    }),
                ],
            },
        };

        let plan = build_flat_normal_active_set_run_plan(
            &tree,
            &[
                PaintSurfaceId::raster(0.into(), ids[0]),
                PaintSurfaceId::raster(0.into(), ids[3]),
            ],
        )
        .expect("plan");

        assert_eq!(plan.checkpoint_count(), 2);
        assert_eq!(plan.steps.len(), 2);
        assert!(
            plan.steps
                .iter()
                .all(|step| matches!(step, ActiveSetStep::ActiveGroup { .. }))
        );
    }

    #[test]
    fn active_set_rejects_inactive_non_normal_sibling_above_active() {
        let ids = layer_ids(3);
        let mut multiply = raster(0, ids[2]);
        let CompositeNode::Raster { props, .. } = &mut multiply else {
            unreachable!();
        };
        props.blend_mode = LayerBlendMode::Multiply;
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![raster(0, ids[0]), raster(0, ids[1]), multiply],
            },
        };

        assert_eq!(
            build_active_set_run_plan(
                &tree,
                &[
                    PaintSurfaceId::raster(0.into(), ids[0]),
                    PaintSurfaceId::raster(0.into(), ids[1]),
                ],
            )
            .unwrap_err(),
            ActiveCompositePlanMiss::UnsupportedNonNormalAboveActive
        );
    }

    #[test]
    fn active_set_keeps_inactive_non_normal_sibling_below_active_static() {
        let ids = layer_ids(3);
        let mut multiply = raster(0, ids[0]);
        let CompositeNode::Raster { props, .. } = &mut multiply else {
            unreachable!();
        };
        props.blend_mode = LayerBlendMode::Multiply;
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![multiply, raster(0, ids[1]), raster(0, ids[2])],
            },
        };

        let plan = build_active_set_run_plan(
            &tree,
            &[
                PaintSurfaceId::raster(0.into(), ids[1]),
                PaintSurfaceId::raster(0.into(), ids[2]),
            ],
        )
        .expect("plan");

        assert_eq!(plan.checkpoint_count(), 1);
        assert!(matches!(
            plan.steps[0],
            ActiveSetStep::StaticRun {
                uses_shader_blending: true,
                ..
            }
        ));
    }

    #[test]
    fn active_set_rejects_active_non_normal_group() {
        let ids = layer_ids(1);
        let tree = CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: normal_props(),
                mask: None,
                children: vec![CompositeNode::Group(CompositeGroup {
                    layer_id: None,
                    mode: GroupCompositeMode::Isolated,
                    props: CompositeProps {
                        blend_mode: LayerBlendMode::Multiply,
                        ..normal_props()
                    },
                    mask: None,
                    children: vec![raster(0, ids[0])],
                })],
            },
        };

        assert_eq!(
            build_active_set_run_plan(&tree, &[PaintSurfaceId::raster(0.into(), ids[0])])
                .unwrap_err(),
            ActiveCompositePlanMiss::UnsupportedGroupProps
        );
    }
}
