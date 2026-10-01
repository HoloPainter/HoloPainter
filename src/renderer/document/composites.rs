use crate::core::{
    adjustment::Adjustment,
    surface::{CompositeGroup, CompositeNode, CompositeProps, CompositeTree, LayerId},
};

#[derive(Debug, Default)]
pub(crate) struct CompositeRegistry {
    trees: Vec<Option<CompositeTree>>,
}

impl CompositeRegistry {
    pub(crate) fn set(&mut self, material_index: usize, tree: CompositeTree) {
        if material_index >= self.trees.len() {
            self.trees.resize_with(material_index + 1, || None);
        }
        self.trees[material_index] = Some(tree);
    }

    pub(crate) fn get(&self, material_index: usize) -> Option<&CompositeTree> {
        self.trees.get(material_index).and_then(Option::as_ref)
    }

    pub(crate) fn material_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.trees
            .iter()
            .enumerate()
            .filter_map(|(material_index, tree)| tree.as_ref().map(|_| material_index))
    }

    pub(crate) fn update_layer_props(
        &mut self,
        layer_id: LayerId,
        props: CompositeProps,
    ) -> impl Iterator<Item = usize> + '_ {
        self.trees
            .iter_mut()
            .enumerate()
            .filter_map(move |(material_index, tree)| {
                let tree = tree.as_mut()?;
                update_group_layer_props(&mut tree.root, layer_id, props).then_some(material_index)
            })
    }

    pub(crate) fn update_adjustment(
        &mut self,
        layer_id: LayerId,
        adjustment: Adjustment,
    ) -> impl Iterator<Item = usize> + '_ {
        self.trees
            .iter_mut()
            .enumerate()
            .filter_map(move |(material_index, tree)| {
                let tree = tree.as_mut()?;
                update_group_adjustment(&mut tree.root, layer_id, &adjustment)
                    .then_some(material_index)
            })
    }
}

/// Mutation boundary for GPU-side composite model edits.
///
/// Composite feature code mutates the document composite registry through this
/// context so each successful tree/model edit records its corresponding
/// `MutationLog` entry at the same boundary that performs the mutation.
pub(crate) struct CompositeMutationContext<'a> {
    composites: &'a mut CompositeRegistry,
    log: &'a mut crate::renderer::mutation::MutationLog,
}

impl<'a> CompositeMutationContext<'a> {
    pub(crate) fn new(
        composites: &'a mut CompositeRegistry,
        log: &'a mut crate::renderer::mutation::MutationLog,
    ) -> Self {
        Self { composites, log }
    }

    pub(crate) fn sync_material_tree(&mut self, material_index: usize, tree: CompositeTree) {
        self.composites.set(material_index, tree);
        self.log.composites.material(material_index);
    }

    pub(crate) fn update_layer_props(&mut self, layer_id: LayerId, props: CompositeProps) {
        for material_index in self.composites.update_layer_props(layer_id, props) {
            self.log.composites.layer_value(material_index);
        }
    }

    pub(crate) fn update_adjustment(&mut self, layer_id: LayerId, adjustment: Adjustment) {
        for material_index in self.composites.update_adjustment(layer_id, adjustment) {
            self.log.composites.layer_value(material_index);
        }
    }
}

fn update_group_layer_props(
    group: &mut CompositeGroup,
    layer_id: LayerId,
    props: CompositeProps,
) -> bool {
    let mut updated = false;
    if group.layer_id == Some(layer_id) {
        group.props = props;
        updated = true;
    }
    for child in &mut group.children {
        match child {
            CompositeNode::Raster {
                surface,
                props: raster_props,
                ..
            } if surface.layer_id == layer_id => {
                *raster_props = props;
                updated = true;
            }
            CompositeNode::EmbeddedImage {
                layer_id: image_layer_id,
                props: image_props,
                ..
            } if *image_layer_id == layer_id => {
                *image_props = props;
                updated = true;
            }
            CompositeNode::SolidFill {
                layer_id: fill_layer_id,
                props: fill_props,
                ..
            } if *fill_layer_id == layer_id => {
                *fill_props = props;
                updated = true;
            }
            CompositeNode::Adjustment {
                layer_id: adjustment_layer_id,
                props: adjustment_props,
                ..
            } if *adjustment_layer_id == layer_id => {
                *adjustment_props = props;
                updated = true;
            }
            CompositeNode::Group(child_group) => {
                updated |= update_group_layer_props(child_group, layer_id, props);
            }
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {}
        }
    }
    updated
}

fn update_group_adjustment(
    group: &mut CompositeGroup,
    layer_id: LayerId,
    adjustment: &Adjustment,
) -> bool {
    let mut updated = false;
    for child in &mut group.children {
        match child {
            CompositeNode::Adjustment {
                layer_id: adjustment_layer_id,
                adjustment: node_adjustment,
                ..
            } if *adjustment_layer_id == layer_id => {
                *node_adjustment = adjustment.clone();
                updated = true;
            }
            CompositeNode::Group(child_group) => {
                updated |= update_group_adjustment(child_group, layer_id, adjustment);
            }
            CompositeNode::Raster { .. }
            | CompositeNode::EmbeddedImage { .. }
            | CompositeNode::SolidFill { .. }
            | CompositeNode::Adjustment { .. } => {}
        }
    }
    updated
}
