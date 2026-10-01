use crate::core::{
    adjustment::Adjustment,
    composite::{GroupCompositeMode, LayerBlendMode},
    embedded_image::{EmbeddedImageId, EmbeddedImageTransform},
    material::{MaterialId, MaterialIndex},
};
use slotmap::{SlotMap, new_key_type};
use std::collections::{BTreeSet, HashMap, HashSet};

new_key_type! {
    pub struct LayerId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PersistentLayerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaintSurfaceRole {
    Raster,
    LayerMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaintSurfaceId {
    pub material_index: MaterialIndex,
    pub layer_id: LayerId,
    pub role: PaintSurfaceRole,
}

impl PaintSurfaceId {
    pub fn raster(material_index: MaterialIndex, layer_id: LayerId) -> Self {
        Self {
            material_index,
            layer_id,
            role: PaintSurfaceRole::Raster,
        }
    }

    pub fn layer_mask(material_index: MaterialIndex, layer_id: LayerId) -> Self {
        Self {
            material_index,
            layer_id,
            role: PaintSurfaceRole::LayerMask,
        }
    }

    pub fn material_index(self) -> MaterialIndex {
        self.material_index
    }

    pub fn is_mask(self) -> bool {
        self.role == PaintSurfaceRole::LayerMask
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompositeTree {
    pub root: CompositeGroup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompositeGroup {
    pub layer_id: Option<LayerId>,
    pub mode: GroupCompositeMode,
    pub props: CompositeProps,
    pub mask: Option<PaintSurfaceId>,
    pub children: Vec<CompositeNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompositeNode {
    Raster {
        surface: PaintSurfaceId,
        mask: Option<PaintSurfaceId>,
        props: CompositeProps,
    },
    EmbeddedImage {
        layer_id: LayerId,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        mask: Option<PaintSurfaceId>,
        props: CompositeProps,
    },
    SolidFill {
        layer_id: LayerId,
        color: [f32; 3],
        mask: Option<PaintSurfaceId>,
        props: CompositeProps,
    },
    Adjustment {
        layer_id: LayerId,
        adjustment: Adjustment,
        mask: Option<PaintSurfaceId>,
        props: CompositeProps,
    },
    Group(CompositeGroup),
}

impl CompositeNode {
    pub fn props(&self) -> CompositeProps {
        match self {
            Self::Raster { props, .. }
            | Self::EmbeddedImage { props, .. }
            | Self::SolidFill { props, .. }
            | Self::Adjustment { props, .. } => *props,
            Self::Group(group) => group.props,
        }
    }

    pub fn mask(&self) -> Option<PaintSurfaceId> {
        match self {
            Self::Raster { mask, .. }
            | Self::EmbeddedImage { mask, .. }
            | Self::SolidFill { mask, .. }
            | Self::Adjustment { mask, .. } => *mask,
            Self::Group(group) => group.mask,
        }
    }

    pub fn leaf_contains_surface(&self, surface: PaintSurfaceId) -> bool {
        match self {
            Self::Raster {
                surface: raster,
                mask,
                ..
            } => *raster == surface || *mask == Some(surface),
            Self::SolidFill { mask, .. } | Self::Adjustment { mask, .. } => *mask == Some(surface),
            Self::EmbeddedImage { mask, .. } => *mask == Some(surface),
            Self::Group(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompositeProps {
    pub visible: bool,
    pub opacity: f32,
    pub blend_mode: LayerBlendMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerTreeRow {
    pub layer_id: LayerId,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub struct LayerTree {
    nodes: SlotMap<LayerId, LayerNode>,
    root: LayerId,
    persistent_id_high_water: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerNode {
    pub persistent_id: PersistentLayerId,
    pub parent: Option<LayerId>,
    pub props: LayerProperties,
    pub content: LayerContent,
    pub mask: Option<LayerMaskProperties>,
    pub material_mask: LayerMaterialMask,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RestoredLayerNode {
    pub persistent_id: PersistentLayerId,
    pub parent_id: Option<PersistentLayerId>,
    pub order: u32,
    pub props: LayerProperties,
    pub content: LayerContent,
    pub mask: Option<LayerMaskProperties>,
    pub material_mask: LayerMaterialMask,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LayerMaterialMask {
    #[default]
    Unspecified,
    Specified(BTreeSet<MaterialId>),
}

impl LayerMaterialMask {
    pub fn allows(&self, material_id: MaterialId) -> bool {
        match self {
            Self::Unspecified => true,
            Self::Specified(material_ids) => material_ids.contains(&material_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerMaskProperties {
    pub enabled: bool,
}

impl Default for LayerMaskProperties {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerMaskInitMode {
    RevealAll,
    HideAll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerProperties {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend_mode: LayerBlendMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayerContent {
    Raster,
    EmbeddedImage {
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
    },
    SolidFill {
        color: [f32; 3],
    },
    Adjustment {
        adjustment: Adjustment,
    },
    Group {
        children: Vec<LayerId>,
        composite_mode: GroupCompositeMode,
    },
}

impl LayerContent {
    pub fn requires_single_material(&self) -> bool {
        matches!(
            self,
            Self::EmbeddedImage { .. }
                | Self::Adjustment {
                    adjustment: Adjustment::UvMirror(_)
                }
        )
    }

    pub fn requires_full_opacity(&self) -> bool {
        matches!(
            self,
            Self::Adjustment {
                adjustment: Adjustment::UvMirror(_)
            }
        )
    }

    pub fn is_uv_mirror(&self) -> bool {
        matches!(
            self,
            Self::Adjustment {
                adjustment: Adjustment::UvMirror(_)
            }
        )
    }

    fn group_children(&self) -> Option<&[LayerId]> {
        match self {
            Self::Group { children, .. } => Some(children),
            _ => None,
        }
    }
}

impl LayerNode {
    pub fn effective_opacity(&self) -> f32 {
        if self.content.requires_full_opacity() {
            1.0
        } else {
            self.props.opacity
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveLayerResult {
    Removed,
    NotFound,
    CannotRemoveRoot,
    CannotRemoveLastRasterLayer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateLayerResult {
    pub root: LayerId,
    pub layer_map: Vec<(LayerId, LayerId)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerMergePlan {
    pub parent_id: LayerId,
    pub layer_ids: Vec<LayerId>,
    pub insertion_index: usize,
    pub output_name: String,
}

impl LayerTree {
    pub(crate) fn restore(
        restored: Vec<RestoredLayerNode>,
        persistent_id_high_water: u64,
    ) -> anyhow::Result<(Self, HashMap<PersistentLayerId, LayerId>)> {
        let mut nodes = SlotMap::with_key();
        let mut runtime_ids = HashMap::with_capacity(restored.len());
        for source in &restored {
            let content = match source.content.clone() {
                LayerContent::Group { composite_mode, .. } => LayerContent::Group {
                    children: Vec::new(),
                    composite_mode,
                },
                content => content,
            };
            if content.is_uv_mirror() && source.mask.is_some() {
                anyhow::bail!(
                    "UV Mirror layer {} cannot have a layer mask",
                    source.persistent_id.0
                );
            }
            let mut props = source.props.clone();
            if content.requires_full_opacity() {
                props.opacity = 1.0;
            }
            let runtime_id = nodes.insert(LayerNode {
                persistent_id: source.persistent_id,
                parent: None,
                props,
                content,
                mask: source.mask,
                material_mask: source.material_mask.clone(),
            });
            if runtime_ids
                .insert(source.persistent_id, runtime_id)
                .is_some()
            {
                anyhow::bail!("duplicate persistent layer ID {}", source.persistent_id.0);
            }
        }

        let roots = restored
            .iter()
            .filter(|source| source.parent_id.is_none())
            .collect::<Vec<_>>();
        if roots.len() != 1 {
            anyhow::bail!("project layer tree must contain exactly one root");
        }
        let root = runtime_ids[&roots[0].persistent_id];
        let mut children = HashMap::<LayerId, Vec<(u32, LayerId)>>::new();
        for source in &restored {
            let runtime_id = runtime_ids[&source.persistent_id];
            let Some(parent_persistent) = source.parent_id else {
                continue;
            };
            let parent = runtime_ids
                .get(&parent_persistent)
                .copied()
                .ok_or_else(|| {
                    anyhow::anyhow!("layer {} has an unknown parent", source.persistent_id.0)
                })?;
            if !matches!(nodes[parent].content, LayerContent::Group { .. }) {
                anyhow::bail!("layer parent is not a group");
            }
            nodes[runtime_id].parent = Some(parent);
            children
                .entry(parent)
                .or_default()
                .push((source.order, runtime_id));
        }
        for (parent, mut ordered) in children {
            ordered.sort_by_key(|(order, _)| *order);
            if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                anyhow::bail!("sibling layer order is duplicated");
            }
            let LayerContent::Group { children, .. } = &mut nodes[parent].content else {
                unreachable!("parent type was checked above");
            };
            children.extend(ordered.into_iter().map(|(_, id)| id));
        }
        for source in &restored {
            let mut seen = HashSet::new();
            let mut current = Some(runtime_ids[&source.persistent_id]);
            while let Some(id) = current {
                if !seen.insert(id) {
                    anyhow::bail!("project layer tree contains a cycle");
                }
                current = nodes[id].parent;
            }
        }
        let maximum_id = restored
            .iter()
            .map(|source| source.persistent_id.0)
            .max()
            .unwrap_or(0);
        Ok((
            Self {
                nodes,
                root,
                persistent_id_high_water: persistent_id_high_water.max(maximum_id),
            },
            runtime_ids,
        ))
    }

    pub fn new_default_raster() -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(LayerNode {
            persistent_id: PersistentLayerId(1),
            parent: None,
            props: LayerProperties {
                name: "Root".to_owned(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Group {
                children: Vec::new(),
                composite_mode: GroupCompositeMode::PassThrough,
            },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        let default_layer = nodes.insert(LayerNode {
            persistent_id: PersistentLayerId(2),
            parent: Some(root),
            props: LayerProperties {
                name: "Default Layer".to_owned(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Raster,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        let LayerContent::Group { children, .. } = &mut nodes[root].content else {
            unreachable!("root must be a group");
        };
        children.push(default_layer);
        Self {
            nodes,
            root,
            persistent_id_high_water: 2,
        }
    }

    pub fn persistent_id(&self, layer_id: LayerId) -> Option<PersistentLayerId> {
        self.nodes.get(layer_id).map(|node| node.persistent_id)
    }

    pub fn persistent_id_high_water(&self) -> u64 {
        self.persistent_id_high_water
    }

    pub fn preserve_persistent_id_high_water(&mut self, high_water: u64) {
        self.persistent_id_high_water = self.persistent_id_high_water.max(high_water);
    }

    fn allocate_persistent_id(&mut self) -> Option<PersistentLayerId> {
        let value = self.persistent_id_high_water.checked_add(1)?;
        self.persistent_id_high_water = value;
        Some(PersistentLayerId(value))
    }

    pub fn root(&self) -> LayerId {
        self.root
    }

    pub fn get(&self, layer_id: LayerId) -> Option<&LayerNode> {
        self.nodes.get(layer_id)
    }

    pub fn children(&self, layer_id: LayerId) -> Option<&[LayerId]> {
        self.group_children(layer_id)
    }

    pub fn group_composite_mode(&self, layer_id: LayerId) -> Option<GroupCompositeMode> {
        match &self.nodes.get(layer_id)?.content {
            LayerContent::Group { composite_mode, .. } => Some(*composite_mode),
            _ => None,
        }
    }

    fn group_children(&self, layer_id: LayerId) -> Option<&[LayerId]> {
        match &self.nodes.get(layer_id)?.content {
            LayerContent::Group { children, .. } => Some(children),
            _ => None,
        }
    }

    fn group_children_mut(&mut self, layer_id: LayerId) -> Option<&mut Vec<LayerId>> {
        match &mut self.nodes.get_mut(layer_id)?.content {
            LayerContent::Group { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn layer_material_mask(&self, layer_id: LayerId) -> Option<&LayerMaterialMask> {
        self.nodes.get(layer_id).map(|node| &node.material_mask)
    }

    pub fn set_layer_material_mask(
        &mut self,
        layer_id: LayerId,
        material_mask: LayerMaterialMask,
    ) -> bool {
        if layer_id == self.root {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        if node.content.requires_single_material()
            && !matches!(&material_mask, LayerMaterialMask::Specified(ids) if ids.len() == 1)
        {
            return false;
        }
        if node.material_mask == material_mask {
            return false;
        }
        node.material_mask = material_mask;
        true
    }

    pub fn local_material_allowed(&self, layer_id: LayerId, material_id: MaterialId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| node.material_mask.allows(material_id))
    }

    pub fn ancestor_material_allowed(&self, layer_id: LayerId, material_id: MaterialId) -> bool {
        let Some(node) = self.nodes.get(layer_id) else {
            return false;
        };
        self.material_allowed_from(node.parent, material_id)
    }

    pub fn effective_material_allowed(&self, layer_id: LayerId, material_id: MaterialId) -> bool {
        if !self.contains(layer_id) {
            return false;
        }
        self.material_allowed_from(Some(layer_id), material_id)
    }

    pub fn remove_material_from_masks(&mut self, material_id: MaterialId) {
        for (_, node) in self.nodes.iter_mut() {
            if let LayerMaterialMask::Specified(material_ids) = &mut node.material_mask {
                material_ids.remove(&material_id);
            }
        }
    }

    pub fn contains(&self, layer_id: LayerId) -> bool {
        self.nodes.contains_key(layer_id)
    }

    pub fn default_raster_layer(&self) -> Option<LayerId> {
        self.nodes
            .get(self.root)?
            .content
            .group_children()?
            .iter()
            .copied()
            .find(|id| {
                self.nodes
                    .get(*id)
                    .is_some_and(|node| matches!(node.content, LayerContent::Raster))
            })
    }

    pub fn is_paintable(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| matches!(node.content, LayerContent::Raster))
    }

    pub fn is_embedded_image(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| matches!(node.content, LayerContent::EmbeddedImage { .. }))
    }

    pub fn embedded_image(
        &self,
        layer_id: LayerId,
    ) -> Option<(EmbeddedImageId, EmbeddedImageTransform)> {
        match self.nodes.get(layer_id)?.content {
            LayerContent::EmbeddedImage {
                image_id,
                transform,
            } => Some((image_id, transform)),
            _ => None,
        }
    }

    pub fn set_embedded_image_transform(
        &mut self,
        layer_id: LayerId,
        transform: EmbeddedImageTransform,
    ) -> bool {
        if !transform.is_valid() {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let LayerContent::EmbeddedImage {
            transform: current, ..
        } = &mut node.content
        else {
            return false;
        };
        if *current == transform {
            return false;
        }
        *current = transform;
        true
    }

    pub fn is_solid_fill(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| matches!(node.content, LayerContent::SolidFill { .. }))
    }

    pub fn can_rasterize_layer(&self, layer_id: LayerId) -> bool {
        !self.is_effectively_locked(layer_id)
            && self.nodes.get(layer_id).is_some_and(|node| {
                matches!(
                    node.content,
                    LayerContent::EmbeddedImage { .. } | LayerContent::SolidFill { .. }
                )
            })
    }

    pub fn rasterize_layer(&mut self, layer_id: LayerId) -> bool {
        if !self.can_rasterize_layer(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        node.content = LayerContent::Raster;
        true
    }

    pub fn solid_fill_color(&self, layer_id: LayerId) -> Option<[f32; 3]> {
        match self.nodes.get(layer_id)?.content {
            LayerContent::SolidFill { color } => Some(color),
            _ => None,
        }
    }

    pub fn set_solid_fill_color(&mut self, layer_id: LayerId, color: [f32; 3]) -> bool {
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let LayerContent::SolidFill {
            color: current_color,
        } = &mut node.content
        else {
            return false;
        };
        let color = color.map(|channel| channel.clamp(0.0, 1.0));
        if *current_color == color {
            return false;
        }
        *current_color = color;
        true
    }

    pub fn is_adjustment(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| matches!(node.content, LayerContent::Adjustment { .. }))
    }

    pub fn adjustment(&self, layer_id: LayerId) -> Option<Adjustment> {
        match &self.nodes.get(layer_id)?.content {
            LayerContent::Adjustment { adjustment } => Some(adjustment.clone()),
            _ => None,
        }
    }

    pub fn set_adjustment(&mut self, layer_id: LayerId, adjustment: Adjustment) -> bool {
        let Some(adjustment) = adjustment.try_normalized() else {
            return false;
        };
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        if matches!(adjustment, Adjustment::UvMirror(_)) && node.mask.is_some() {
            return false;
        }
        let LayerContent::Adjustment {
            adjustment: current_adjustment,
        } = &mut node.content
        else {
            return false;
        };
        let adjustment_changed = *current_adjustment != adjustment;
        let opacity_changed =
            matches!(adjustment, Adjustment::UvMirror(_)) && node.props.opacity != 1.0;
        if !adjustment_changed && !opacity_changed {
            return false;
        }
        *current_adjustment = adjustment;
        if opacity_changed {
            node.props.opacity = 1.0;
        }
        true
    }

    pub fn is_group(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| matches!(node.content, LayerContent::Group { .. }))
    }

    pub fn is_effectively_visible(&self, layer_id: LayerId) -> bool {
        let mut current = Some(layer_id);
        while let Some(current_id) = current {
            let Some(node) = self.nodes.get(current_id) else {
                return false;
            };
            if !node.props.visible {
                return false;
            }
            current = node.parent;
        }
        true
    }

    pub fn is_locked(&self, layer_id: LayerId) -> bool {
        self.nodes
            .get(layer_id)
            .is_some_and(|node| node.props.locked)
    }

    pub fn is_effectively_locked(&self, layer_id: LayerId) -> bool {
        let mut current = Some(layer_id);
        while let Some(current_id) = current {
            let Some(node) = self.nodes.get(current_id) else {
                return false;
            };
            if node.props.locked {
                return true;
            }
            current = node.parent;
        }
        false
    }

    pub fn can_set_layer_locked(&self, layer_id: LayerId) -> bool {
        layer_id != self.root && self.contains(layer_id)
    }

    pub fn can_edit_layer_pixels(&self, layer_id: LayerId) -> bool {
        self.is_paintable(layer_id)
            && self.is_effectively_visible(layer_id)
            && !self.is_effectively_locked(layer_id)
    }

    pub fn can_edit_layer_mask_pixels(&self, layer_id: LayerId) -> bool {
        self.has_layer_mask(layer_id)
            && self.is_effectively_visible(layer_id)
            && !self.is_effectively_locked(layer_id)
    }

    pub fn can_set_layer_opacity(&self, layer_id: LayerId) -> bool {
        self.contains(layer_id)
            && !self.is_effectively_locked(layer_id)
            && !matches!(self.adjustment(layer_id), Some(Adjustment::UvMirror(_)))
    }

    pub fn can_set_layer_blend_mode(&self, layer_id: LayerId) -> bool {
        self.contains(layer_id)
            && !self.is_effectively_locked(layer_id)
            && !matches!(self.adjustment(layer_id), Some(Adjustment::UvMirror(_)))
    }

    pub fn can_set_group_composite_mode(&self, layer_id: LayerId) -> bool {
        layer_id != self.root && self.is_group(layer_id) && !self.is_effectively_locked(layer_id)
    }

    pub fn can_use_identity_pass_through(&self, layer_id: LayerId) -> bool {
        if layer_id == self.root {
            return false;
        }
        self.nodes.get(layer_id).is_some_and(|node| {
            matches!(node.content, LayerContent::Group { .. })
                && node.props.opacity == 1.0
                && node.mask.is_none()
        })
    }

    pub fn is_identity_pass_through_group(&self, layer_id: LayerId) -> bool {
        self.nodes.get(layer_id).is_some_and(|node| {
            matches!(
                node.content,
                LayerContent::Group {
                    composite_mode: GroupCompositeMode::PassThrough,
                    ..
                }
            ) && node.props.opacity == 1.0
                && node.mask.is_none()
        })
    }

    pub fn layer_mask(&self, layer_id: LayerId) -> Option<&LayerMaskProperties> {
        self.nodes.get(layer_id)?.mask.as_ref()
    }

    pub fn has_layer_mask(&self, layer_id: LayerId) -> bool {
        self.layer_mask(layer_id).is_some()
    }

    pub fn has_enabled_layer_mask(&self, layer_id: LayerId) -> bool {
        self.layer_mask(layer_id).is_some_and(|mask| mask.enabled)
    }

    pub fn can_have_layer_mask(&self, layer_id: LayerId) -> bool {
        if layer_id == self.root {
            return false;
        }
        self.nodes.get(layer_id).is_some_and(|node| {
            if node.content.is_uv_mirror() {
                return false;
            }
            matches!(
                node.content,
                LayerContent::Raster
                    | LayerContent::EmbeddedImage { .. }
                    | LayerContent::SolidFill { .. }
                    | LayerContent::Adjustment { .. }
                    | LayerContent::Group { .. }
            )
        })
    }

    pub fn can_add_layer_mask(&self, layer_id: LayerId) -> bool {
        self.can_have_layer_mask(layer_id)
            && !self.has_layer_mask(layer_id)
            && !self.is_effectively_locked(layer_id)
    }

    pub fn can_apply_layer_mask(&self, layer_id: LayerId) -> bool {
        self.has_layer_mask(layer_id)
            && !self.is_effectively_locked(layer_id)
            && self.nodes.get(layer_id).is_some_and(|node| {
                matches!(
                    node.content,
                    LayerContent::Raster
                        | LayerContent::EmbeddedImage { .. }
                        | LayerContent::SolidFill { .. }
                )
            })
    }

    pub fn apply_layer_mask(&mut self, layer_id: LayerId) -> bool {
        if !self.can_apply_layer_mask(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        node.content = LayerContent::Raster;
        node.mask = None;
        true
    }

    pub fn can_move_layer_mask(&self, source_layer_id: LayerId, target_layer_id: LayerId) -> bool {
        source_layer_id != target_layer_id
            && self.has_layer_mask(source_layer_id)
            && self.can_have_layer_mask(target_layer_id)
            && !self.is_effectively_locked(source_layer_id)
            && !self.is_effectively_locked(target_layer_id)
    }

    pub fn move_layer_mask(&mut self, source_layer_id: LayerId, target_layer_id: LayerId) -> bool {
        if !self.can_move_layer_mask(source_layer_id, target_layer_id) {
            return false;
        }
        let Some(mask) = self
            .nodes
            .get_mut(source_layer_id)
            .and_then(|node| node.mask.take())
        else {
            return false;
        };
        let Some(target) = self.nodes.get_mut(target_layer_id) else {
            return false;
        };
        target.mask = Some(mask);
        true
    }

    pub fn add_layer_mask(&mut self, layer_id: LayerId) -> bool {
        if !self.can_add_layer_mask(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        node.mask = Some(LayerMaskProperties::default());
        true
    }

    pub fn delete_layer_mask(&mut self, layer_id: LayerId) -> bool {
        if !self.can_have_layer_mask(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        node.mask.take().is_some()
    }

    pub fn set_layer_mask_enabled(&mut self, layer_id: LayerId, enabled: bool) {
        if let Some(mask) = self
            .nodes
            .get_mut(layer_id)
            .and_then(|node| node.mask.as_mut())
        {
            mask.enabled = enabled;
        }
    }

    pub fn insert_raster_layer(
        &mut self,
        parent_id: LayerId,
        index: usize,
        name: impl Into<String>,
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) {
            return None;
        }
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Raster,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        let children = self.group_children_mut(parent_id)?;
        children.insert(index.min(children.len()), new_layer);
        Some(new_layer)
    }

    pub fn add_raster_layer_above(
        &mut self,
        anchor: LayerId,
        name: impl Into<String>,
    ) -> Option<LayerId> {
        let parent_id = self.nodes.get(anchor)?.parent?;
        let persistent_id = self.allocate_persistent_id()?;

        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Raster,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });

        let children = self.group_children_mut(parent_id).unwrap();
        if let Some(pos) = children.iter().position(|&id| id == anchor) {
            children.insert(pos + 1, new_layer);
        } else {
            children.push(new_layer);
        }

        Some(new_layer)
    }

    pub fn add_raster_layer_to_root(&mut self, name: impl Into<String>) -> Option<LayerId> {
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(self.root),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Raster,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        self.group_children_mut(self.root)
            .expect("root must be a group")
            .push(new_layer);
        Some(new_layer)
    }

    pub fn add_raster_layer_to_group(
        &mut self,
        parent_id: LayerId,
        name: impl Into<String>,
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) {
            return None;
        }
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Raster,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        self.group_children_mut(parent_id)?.push(new_layer);
        Some(new_layer)
    }

    pub fn add_embedded_image_layer_above(
        &mut self,
        anchor: LayerId,
        name: impl Into<String>,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        material_id: MaterialId,
    ) -> Option<LayerId> {
        let parent_id = self.nodes.get(anchor)?.parent?;
        self.insert_embedded_image_layer(
            parent_id,
            self.group_children(parent_id)?
                .iter()
                .position(|id| *id == anchor)?
                .saturating_add(1),
            name,
            image_id,
            transform,
            material_id,
        )
    }

    pub fn add_embedded_image_layer_to_group(
        &mut self,
        parent_id: LayerId,
        name: impl Into<String>,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        material_id: MaterialId,
    ) -> Option<LayerId> {
        let index = self.group_children(parent_id)?.len();
        self.insert_embedded_image_layer(parent_id, index, name, image_id, transform, material_id)
    }

    fn insert_embedded_image_layer(
        &mut self,
        parent_id: LayerId,
        index: usize,
        name: impl Into<String>,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        material_id: MaterialId,
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) || image_id.0 == 0 || !transform.is_valid() {
            return None;
        }
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::EmbeddedImage {
                image_id,
                transform,
            },
            mask: None,
            material_mask: LayerMaterialMask::Specified(BTreeSet::from([material_id])),
        });
        let children = self.group_children_mut(parent_id)?;
        children.insert(index.min(children.len()), new_layer);
        Some(new_layer)
    }

    pub fn add_solid_fill_layer_above(
        &mut self,
        anchor: LayerId,
        name: impl Into<String>,
        color: [f32; 3],
    ) -> Option<LayerId> {
        let parent_id = self.nodes.get(anchor)?.parent?;
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::SolidFill {
                color: color.map(|channel| channel.clamp(0.0, 1.0)),
            },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });

        let children = self.group_children_mut(parent_id).unwrap();
        if let Some(pos) = children.iter().position(|&id| id == anchor) {
            children.insert(pos + 1, new_layer);
        } else {
            children.push(new_layer);
        }
        Some(new_layer)
    }

    pub fn add_solid_fill_layer_to_group(
        &mut self,
        parent_id: LayerId,
        name: impl Into<String>,
        color: [f32; 3],
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) {
            return None;
        }
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::SolidFill {
                color: color.map(|channel| channel.clamp(0.0, 1.0)),
            },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        self.group_children_mut(parent_id)?.push(new_layer);
        Some(new_layer)
    }

    pub fn add_adjustment_layer_above(
        &mut self,
        anchor: LayerId,
        name: impl Into<String>,
        adjustment: Adjustment,
    ) -> Option<LayerId> {
        let adjustment = adjustment.try_normalized()?;
        let parent_id = self.nodes.get(anchor)?.parent?;
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Adjustment { adjustment },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });

        let children = self.group_children_mut(parent_id).unwrap();
        if let Some(pos) = children.iter().position(|&id| id == anchor) {
            children.insert(pos + 1, new_layer);
        } else {
            children.push(new_layer);
        }
        Some(new_layer)
    }

    pub fn add_adjustment_layer_to_group(
        &mut self,
        parent_id: LayerId,
        name: impl Into<String>,
        adjustment: Adjustment,
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) {
            return None;
        }
        let adjustment = adjustment.try_normalized()?;
        let persistent_id = self.allocate_persistent_id()?;
        let new_layer = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Adjustment { adjustment },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        self.group_children_mut(parent_id)?.push(new_layer);
        Some(new_layer)
    }

    pub fn add_group_above(&mut self, anchor: LayerId, name: impl Into<String>) -> Option<LayerId> {
        let parent_id = self.nodes.get(anchor)?.parent?;
        let persistent_id = self.allocate_persistent_id()?;
        let new_group = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Group {
                children: Vec::new(),
                composite_mode: GroupCompositeMode::Isolated,
            },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });

        let children = self.group_children_mut(parent_id).unwrap();
        if let Some(pos) = children.iter().position(|&id| id == anchor) {
            children.insert(pos + 1, new_group);
        } else {
            children.push(new_group);
        }

        Some(new_group)
    }

    pub fn add_group_to_group(
        &mut self,
        parent_id: LayerId,
        name: impl Into<String>,
    ) -> Option<LayerId> {
        if !self.is_group(parent_id) {
            return None;
        }
        let persistent_id = self.allocate_persistent_id()?;
        let new_group = self.nodes.insert(LayerNode {
            persistent_id,
            parent: Some(parent_id),
            props: LayerProperties {
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                blend_mode: LayerBlendMode::Normal,
            },
            content: LayerContent::Group {
                children: Vec::new(),
                composite_mode: GroupCompositeMode::Isolated,
            },
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
        });
        self.group_children_mut(parent_id)?.push(new_group);
        Some(new_group)
    }

    pub fn remove_layer(&mut self, layer_id: LayerId) -> RemoveLayerResult {
        if layer_id == self.root {
            return RemoveLayerResult::CannotRemoveRoot;
        }

        let node = match self.nodes.get(layer_id) {
            Some(n) => n.clone(),
            None => return RemoveLayerResult::NotFound,
        };
        let removing_rasters = self.raster_layers_in_subtree(layer_id).len();
        if removing_rasters >= self.ordered_raster_layers().len() {
            return RemoveLayerResult::CannotRemoveLastRasterLayer;
        }

        if let Some(parent_id) = node.parent {
            if let Some(children) = self.group_children_mut(parent_id) {
                children.retain(|&id| id != layer_id);
            }
        }

        self.remove_subtree(layer_id);
        RemoveLayerResult::Removed
    }

    pub fn duplicate_layer(&mut self, layer_id: LayerId) -> Option<DuplicateLayerResult> {
        let node = self.nodes.get(layer_id)?.clone();
        if layer_id == self.root {
            return None; // cannot duplicate root
        }

        let new_name = self.next_duplicate_name(node.parent?, &node.props.name)?;

        let mut layer_map = Vec::new();
        let new_layer =
            self.duplicate_subtree(layer_id, node.parent, Some(new_name), &mut layer_map)?;

        if let Some(parent_id) = node.parent {
            let children = self.group_children_mut(parent_id).unwrap();
            if let Some(pos) = children.iter().position(|&id| id == layer_id) {
                children.insert(pos + 1, new_layer);
            } else {
                children.push(new_layer);
            }
        }

        Some(DuplicateLayerResult {
            root: new_layer,
            layer_map,
        })
    }

    fn next_duplicate_name(&self, parent: LayerId, source_name: &str) -> Option<String> {
        let base = duplicate_name_base(source_name);
        let name_exists = |candidate: &str| {
            self.group_children(parent).is_some_and(|children| {
                children.iter().any(|child| {
                    self.nodes
                        .get(*child)
                        .is_some_and(|node| node.props.name == candidate)
                })
            })
        };
        let copy_name = format!("{base} (copy)");
        if !name_exists(&copy_name) {
            return Some(copy_name);
        }
        for number in 2usize.. {
            let candidate = format!("{base} ({number})");
            if !name_exists(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    pub fn rename_layer(&mut self, layer_id: LayerId, name: String) {
        if let Some(node) = self.nodes.get_mut(layer_id) {
            if node.props.name == name {
                return;
            }
            node.props.name = name;
        }
    }

    pub fn set_visible(&mut self, layer_id: LayerId, visible: bool) {
        if let Some(node) = self.nodes.get_mut(layer_id) {
            if node.props.visible == visible {
                return;
            }
            node.props.visible = visible;
        }
    }

    pub fn set_locked(&mut self, layer_id: LayerId, locked: bool) -> bool {
        if !self.can_set_layer_locked(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        if node.props.locked == locked {
            return false;
        }
        node.props.locked = locked;
        true
    }

    pub fn set_opacity(&mut self, layer_id: LayerId, opacity: f32) -> bool {
        if !self.can_set_layer_opacity(layer_id) {
            return false;
        }
        self.set_opacity_unchecked(layer_id, opacity)
    }

    pub fn set_blend_mode(&mut self, layer_id: LayerId, blend_mode: LayerBlendMode) -> bool {
        if !self.can_set_layer_blend_mode(layer_id) {
            return false;
        }
        self.set_blend_mode_unchecked(layer_id, blend_mode)
    }

    pub(crate) fn restore_opacity(&mut self, layer_id: LayerId, opacity: f32) -> bool {
        self.set_opacity_unchecked(layer_id, opacity)
    }

    pub(crate) fn restore_composite_settings(
        &mut self,
        layer_id: LayerId,
        blend_mode: LayerBlendMode,
        group_mode: GroupCompositeMode,
    ) -> bool {
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let changed = node.props.blend_mode != blend_mode
            || matches!(
                node.content,
                LayerContent::Group { composite_mode, .. } if composite_mode != group_mode
            );
        node.props.blend_mode = blend_mode;
        if let LayerContent::Group { composite_mode, .. } = &mut node.content {
            *composite_mode = group_mode;
        }
        changed
    }

    pub fn set_group_composite_mode(
        &mut self,
        layer_id: LayerId,
        mode: GroupCompositeMode,
    ) -> bool {
        if !self.can_set_group_composite_mode(layer_id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let LayerContent::Group { composite_mode, .. } = &mut node.content else {
            return false;
        };
        if *composite_mode == mode {
            return false;
        }
        *composite_mode = mode;
        true
    }

    fn set_opacity_unchecked(&mut self, layer_id: LayerId, opacity: f32) -> bool {
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let opacity = if node.content.requires_full_opacity() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        if node.props.opacity == opacity {
            return false;
        }
        node.props.opacity = opacity;
        true
    }

    fn set_blend_mode_unchecked(&mut self, layer_id: LayerId, blend_mode: LayerBlendMode) -> bool {
        let Some(node) = self.nodes.get_mut(layer_id) else {
            return false;
        };
        let group_mode_changed = matches!(
            node.content,
            LayerContent::Group { composite_mode, .. }
                if composite_mode != GroupCompositeMode::Isolated
        );
        if node.props.blend_mode == blend_mode && !group_mode_changed {
            return false;
        }
        node.props.blend_mode = blend_mode;
        if let LayerContent::Group { composite_mode, .. } = &mut node.content {
            *composite_mode = GroupCompositeMode::Isolated;
        }
        true
    }

    pub fn move_layer(&mut self, layer_id: LayerId, new_parent: LayerId, new_index: usize) -> bool {
        if layer_id == self.root || layer_id == new_parent {
            return false;
        }

        let old_parent = match self.nodes.get(layer_id) {
            Some(n) => n.parent,
            None => return false,
        };

        if !self.is_group(new_parent) || self.is_descendant(new_parent, layer_id) {
            return false;
        }

        if old_parent == Some(new_parent) {
            if let Some((_, old_index)) = self.parent_and_index(layer_id) {
                if old_index == new_index {
                    return false;
                }
            }
        }

        if let Some(p_id) = old_parent {
            if let Some(children) = self.group_children_mut(p_id) {
                children.retain(|&id| id != layer_id);
            }
        }

        if let Some(node) = self.nodes.get_mut(layer_id) {
            node.parent = Some(new_parent);
        }

        let children = self.group_children_mut(new_parent).unwrap();
        let insert_idx = new_index.min(children.len());
        children.insert(insert_idx, layer_id);

        true
    }

    pub fn move_layers(
        &mut self,
        layer_ids: &[LayerId],
        new_parent: LayerId,
        new_index: usize,
    ) -> bool {
        if layer_ids.is_empty() || !self.is_group(new_parent) {
            return false;
        }

        let moving = layer_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        if moving.len() != layer_ids.len() {
            return false;
        }

        for &layer_id in layer_ids {
            if layer_id == self.root
                || layer_id == new_parent
                || !self.contains(layer_id)
                || self.is_descendant(new_parent, layer_id)
            {
                return false;
            }
        }

        let before = self.clone();
        let adjusted_index = self
            .group_children(new_parent)
            .map(|children| {
                children
                    .iter()
                    .take(new_index)
                    .filter(|child| moving.contains(child))
                    .count()
            })
            .map_or(new_index, |removed_before| {
                new_index.saturating_sub(removed_before)
            });

        let parents = layer_ids
            .iter()
            .filter_map(|layer_id| self.nodes.get(*layer_id).and_then(|node| node.parent))
            .collect::<std::collections::HashSet<_>>();
        for parent_id in parents {
            if let Some(children) = self.group_children_mut(parent_id) {
                children.retain(|child| !moving.contains(child));
            }
        }

        for &layer_id in layer_ids {
            if let Some(node) = self.nodes.get_mut(layer_id) {
                node.parent = Some(new_parent);
            }
        }

        let children = self.group_children_mut(new_parent).unwrap();
        let insert_idx = adjusted_index.min(children.len());
        for (offset, layer_id) in layer_ids.iter().copied().enumerate() {
            children.insert(insert_idx + offset, layer_id);
        }

        if *self == before {
            return false;
        }

        true
    }

    pub fn first_paintable_layer(&self) -> Option<LayerId> {
        self.ordered_raster_layers().into_iter().next()
    }

    pub fn ordered_raster_layers(&self) -> Vec<LayerId> {
        let mut result = Vec::new();
        self.collect_raster_layers(self.root, &mut result);
        result
    }

    pub fn rows(&self) -> Vec<LayerTreeRow> {
        let mut result = Vec::new();
        self.collect_rows(self.root, 0, &mut result);
        result
    }

    pub fn rows_for_panel(&self) -> Vec<LayerTreeRow> {
        let mut result = Vec::new();
        if let Some(children) = self.group_children(self.root) {
            for &child in children.iter().rev() {
                self.collect_panel_rows(child, 1, &mut result);
            }
        }
        result
    }

    pub fn raster_layers_in_subtree(&self, layer_id: LayerId) -> Vec<LayerId> {
        let mut result = Vec::new();
        self.collect_raster_layers(layer_id, &mut result);
        result
    }

    pub fn layer_surface_owners(&self) -> Vec<(LayerId, bool)> {
        let mut result = Vec::new();
        self.collect_layer_surface_owners(self.root, &mut result);
        result
    }

    pub fn parent_and_index(&self, layer_id: LayerId) -> Option<(LayerId, usize)> {
        let parent = self.nodes.get(layer_id)?.parent?;
        let index = self
            .group_children(parent)?
            .iter()
            .position(|&id| id == layer_id)?;
        Some((parent, index))
    }

    pub fn layer_merge_plan(&self, layer_ids: &[LayerId]) -> Option<LayerMergePlan> {
        if layer_ids.is_empty() {
            return None;
        }
        let unique = layer_ids.iter().copied().collect::<HashSet<_>>();
        if unique.len() != layer_ids.len() || unique.contains(&self.root) {
            return None;
        }

        if unique.len() == 1 {
            let layer_id = *unique.iter().next()?;
            let node = self.nodes.get(layer_id)?;
            if !matches!(node.content, LayerContent::Group { .. }) {
                return None;
            }
        }

        let mut indexed = Vec::with_capacity(layer_ids.len());
        let mut parent_id = None;
        for layer_id in unique {
            let node = self.nodes.get(layer_id)?;
            let parent = node.parent?;
            if parent_id.is_some_and(|expected| expected != parent) {
                return None;
            }
            parent_id = Some(parent);
            let index = self
                .group_children(parent)?
                .iter()
                .position(|child| *child == layer_id)?;
            indexed.push((index, layer_id));
        }

        indexed.sort_unstable_by_key(|(index, _)| *index);
        let insertion_index = indexed.first()?.0;
        if indexed
            .iter()
            .enumerate()
            .any(|(offset, (index, _))| *index != insertion_index + offset)
        {
            return None;
        }
        let output_name = self.nodes.get(indexed.last()?.1)?.props.name.clone();
        Some(LayerMergePlan {
            parent_id: parent_id?,
            layer_ids: indexed.into_iter().map(|(_, layer_id)| layer_id).collect(),
            insertion_index,
            output_name,
        })
    }

    fn collect_raster_layers(&self, current: LayerId, out: &mut Vec<LayerId>) {
        if let Some(node) = self.nodes.get(current) {
            if matches!(node.content, LayerContent::Raster) {
                out.push(current);
            }
            for &child in node.content.group_children().unwrap_or_default() {
                self.collect_raster_layers(child, out);
            }
        }
    }

    fn collect_rows(&self, current: LayerId, depth: usize, out: &mut Vec<LayerTreeRow>) {
        if let Some(node) = self.nodes.get(current) {
            if current != self.root {
                out.push(LayerTreeRow {
                    layer_id: current,
                    depth,
                });
            }
            for &child in node.content.group_children().unwrap_or_default() {
                self.collect_rows(child, depth + 1, out);
            }
        }
    }

    fn material_allowed_from(&self, mut current: Option<LayerId>, material_id: MaterialId) -> bool {
        while let Some(layer_id) = current {
            let Some(node) = self.nodes.get(layer_id) else {
                return false;
            };
            if !node.material_mask.allows(material_id) {
                return false;
            }
            current = node.parent;
        }
        true
    }

    fn collect_panel_rows(&self, current: LayerId, depth: usize, out: &mut Vec<LayerTreeRow>) {
        if let Some(node) = self.nodes.get(current) {
            out.push(LayerTreeRow {
                layer_id: current,
                depth,
            });
            for &child in node
                .content
                .group_children()
                .unwrap_or_default()
                .iter()
                .rev()
            {
                self.collect_panel_rows(child, depth + 1, out);
            }
        }
    }

    fn remove_subtree(&mut self, layer_id: LayerId) {
        let children = self
            .group_children(layer_id)
            .map(<[LayerId]>::to_vec)
            .unwrap_or_default();
        for child in children {
            self.remove_subtree(child);
        }
        self.nodes.remove(layer_id);
    }

    fn duplicate_subtree(
        &mut self,
        source_id: LayerId,
        new_parent: Option<LayerId>,
        override_name: Option<String>,
        layer_map: &mut Vec<(LayerId, LayerId)>,
    ) -> Option<LayerId> {
        let source = self.nodes.get(source_id)?.clone();
        let children = source.content.group_children().unwrap_or_default().to_vec();
        let content = match source.content {
            LayerContent::Group { composite_mode, .. } => LayerContent::Group {
                children: Vec::new(),
                composite_mode,
            },
            content => content,
        };
        let opacity = if content.requires_full_opacity() {
            1.0
        } else {
            source.props.opacity
        };
        let persistent_id = self.allocate_persistent_id()?;
        let new_id = self.nodes.insert(LayerNode {
            persistent_id,
            parent: new_parent,
            props: LayerProperties {
                name: override_name.unwrap_or(source.props.name),
                visible: source.props.visible,
                locked: source.props.locked,
                opacity,
                blend_mode: source.props.blend_mode,
            },
            content,
            mask: source.mask,
            material_mask: source.material_mask,
        });
        layer_map.push((source_id, new_id));

        for child_id in children {
            let child_copy = self.duplicate_subtree(child_id, Some(new_id), None, layer_map)?;
            self.group_children_mut(new_id)?.push(child_copy);
        }

        Some(new_id)
    }

    fn collect_layer_surface_owners(&self, current: LayerId, out: &mut Vec<(LayerId, bool)>) {
        if let Some(node) = self.nodes.get(current) {
            if current != self.root {
                out.push((current, matches!(node.content, LayerContent::Raster)));
            }
            for &child in node.content.group_children().unwrap_or_default() {
                self.collect_layer_surface_owners(child, out);
            }
        }
    }

    fn is_descendant(&self, layer_id: LayerId, possible_ancestor: LayerId) -> bool {
        let mut current = self.nodes.get(layer_id).and_then(|node| node.parent);
        while let Some(parent) = current {
            if parent == possible_ancestor {
                return true;
            }
            current = self.nodes.get(parent).and_then(|node| node.parent);
        }
        false
    }
}

fn duplicate_name_base(name: &str) -> &str {
    if let Some(base) = name.strip_suffix(" (copy)")
        && !base.is_empty()
    {
        return base;
    }

    let Some(without_close) = name.strip_suffix(')') else {
        return name;
    };
    let Some((base, number)) = without_close.rsplit_once(" (") else {
        return name;
    };
    if !base.is_empty()
        && number
            .parse::<usize>()
            .ok()
            .is_some_and(|number| number >= 2)
    {
        return base;
    }

    name
}

impl PartialEq for LayerTree {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
            && self.persistent_id_high_water == other.persistent_id_high_water
            && self.nodes.len() == other.nodes.len()
            && self
                .nodes
                .iter()
                .all(|(layer_id, node)| other.nodes.get(layer_id) == Some(node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specified(material_ids: impl IntoIterator<Item = MaterialId>) -> LayerMaterialMask {
        LayerMaterialMask::Specified(material_ids.into_iter().collect())
    }

    #[test]
    fn persistent_layer_ids_are_unique_and_deleted_ids_are_not_reused() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let first = tree.add_raster_layer_above(base, "First").unwrap();
        let first_persistent = tree.persistent_id(first).unwrap();

        assert_eq!(tree.remove_layer(first), RemoveLayerResult::Removed);
        let second = tree.add_raster_layer_above(base, "Second").unwrap();
        let second_persistent = tree.persistent_id(second).unwrap();

        assert!(first_persistent.0 > 0);
        assert!(second_persistent.0 > first_persistent.0);
        assert_eq!(tree.persistent_id_high_water(), second_persistent.0);
    }

    #[test]
    fn restored_tree_preserves_the_newer_persistent_id_high_water() {
        let mut restored = LayerTree::new_default_raster();
        let mut current = restored.clone();
        let base = current.default_raster_layer().unwrap();
        let allocated = current.add_raster_layer_above(base, "Allocated").unwrap();
        let allocated_id = current.persistent_id(allocated).unwrap();

        restored.preserve_persistent_id_high_water(current.persistent_id_high_water());
        let restored_base = restored.default_raster_layer().unwrap();
        let next = restored
            .add_raster_layer_above(restored_base, "After restore")
            .unwrap();

        assert!(restored.persistent_id(next).unwrap().0 > allocated_id.0);
    }

    #[test]
    fn restored_uv_mirror_normalizes_legacy_opacity_to_full() {
        let root_id = PersistentLayerId(1);
        let uv_mirror_id = PersistentLayerId(2);
        let restored = vec![
            RestoredLayerNode {
                persistent_id: root_id,
                parent_id: None,
                order: 0,
                props: LayerProperties {
                    name: "Root".to_owned(),
                    visible: true,
                    locked: false,
                    opacity: 1.0,
                    blend_mode: LayerBlendMode::Normal,
                },
                content: LayerContent::Group {
                    children: Vec::new(),
                    composite_mode: GroupCompositeMode::PassThrough,
                },
                mask: None,
                material_mask: LayerMaterialMask::Unspecified,
            },
            RestoredLayerNode {
                persistent_id: uv_mirror_id,
                parent_id: Some(root_id),
                order: 0,
                props: LayerProperties {
                    name: "UV Mirror".to_owned(),
                    visible: true,
                    locked: false,
                    opacity: 0.25,
                    blend_mode: LayerBlendMode::Normal,
                },
                content: LayerContent::Adjustment {
                    adjustment: crate::core::adjustment::AdjustmentKind::UvMirror
                        .default_adjustment(),
                },
                mask: None,
                material_mask: LayerMaterialMask::Unspecified,
            },
        ];

        let (tree, runtime_ids) = LayerTree::restore(restored, 2).unwrap();
        let uv_mirror = runtime_ids[&uv_mirror_id];

        assert_eq!(tree.get(uv_mirror).unwrap().props.opacity, 1.0);
        assert!(!tree.can_set_layer_opacity(uv_mirror));
    }

    #[test]
    fn material_masks_distinguish_unspecified_empty_and_selected_states() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let material_a = MaterialId(1);
        let material_b = MaterialId(2);

        assert_eq!(
            tree.layer_material_mask(layer),
            Some(&LayerMaterialMask::Unspecified)
        );
        assert!(tree.local_material_allowed(layer, material_a));
        assert!(tree.local_material_allowed(layer, material_b));

        assert!(tree.set_layer_material_mask(layer, specified([])));
        assert!(!tree.local_material_allowed(layer, material_a));
        assert!(!tree.local_material_allowed(layer, material_b));

        assert!(tree.set_layer_material_mask(layer, specified([material_a])));
        assert!(tree.local_material_allowed(layer, material_a));
        assert!(!tree.local_material_allowed(layer, material_b));
    }

    #[test]
    fn effective_material_mask_is_the_intersection_of_ancestors_and_layer() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.move_layer(layer, group, 0));
        let material_a = MaterialId(1);
        let material_b = MaterialId(2);

        assert!(tree.set_layer_material_mask(group, specified([material_a])));
        assert!(tree.set_layer_material_mask(layer, specified([material_a, material_b])));

        assert!(tree.ancestor_material_allowed(layer, material_a));
        assert!(!tree.ancestor_material_allowed(layer, material_b));
        assert!(tree.effective_material_allowed(layer, material_a));
        assert!(!tree.effective_material_allowed(layer, material_b));
    }

    #[test]
    fn duplicate_copies_local_material_masks_and_move_recomputes_effective_mask() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group_a = tree.add_group_above(layer, "A").unwrap();
        let group_b = tree.add_group_above(group_a, "B").unwrap();
        let material_a = MaterialId(1);
        let material_b = MaterialId(2);
        assert!(tree.set_layer_material_mask(layer, specified([material_a, material_b])));
        assert!(tree.set_layer_material_mask(group_b, specified([material_b])));

        let duplicate = tree.duplicate_layer(layer).unwrap().root;
        assert_eq!(
            tree.layer_material_mask(duplicate),
            Some(&specified([material_a, material_b]))
        );
        assert!(tree.effective_material_allowed(duplicate, material_a));
        assert!(tree.effective_material_allowed(duplicate, material_b));

        assert!(tree.move_layer(duplicate, group_b, 0));
        assert!(!tree.effective_material_allowed(duplicate, material_a));
        assert!(tree.effective_material_allowed(duplicate, material_b));
        assert_eq!(
            tree.layer_material_mask(duplicate),
            Some(&specified([material_a, material_b]))
        );
    }

    #[test]
    fn removing_material_id_keeps_an_explicit_empty_mask() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let material = MaterialId(1);
        assert!(tree.set_layer_material_mask(layer, specified([material])));

        tree.remove_material_from_masks(material);

        assert_eq!(tree.layer_material_mask(layer), Some(&specified([])));
        assert!(!tree.local_material_allowed(layer, material));
    }

    #[test]
    fn root_material_mask_cannot_be_changed() {
        let mut tree = LayerTree::new_default_raster();

        assert!(!tree.set_layer_material_mask(tree.root(), specified([])));
        assert_eq!(
            tree.layer_material_mask(tree.root()),
            Some(&LayerMaterialMask::Unspecified)
        );
    }

    #[test]
    fn effective_lock_follows_locked_ancestors() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.move_layer(layer, group, 0));

        assert!(!tree.is_locked(layer));
        assert!(!tree.is_effectively_locked(layer));
        assert!(tree.set_locked(group, true));

        assert!(tree.is_locked(group));
        assert!(!tree.is_locked(layer));
        assert!(tree.is_effectively_locked(layer));
        assert!(!tree.can_edit_layer_pixels(layer));
        assert!(!tree.can_set_layer_opacity(layer));
        assert!(!tree.can_set_layer_blend_mode(layer));
    }

    #[test]
    fn local_lock_survives_parent_unlock() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.move_layer(layer, group, 0));
        assert!(tree.set_locked(group, true));
        assert!(tree.set_locked(layer, true));

        assert!(tree.set_locked(group, false));

        assert!(tree.is_locked(layer));
        assert!(tree.is_effectively_locked(layer));
    }

    #[test]
    fn root_cannot_be_locked() {
        let mut tree = LayerTree::new_default_raster();

        assert!(!tree.set_locked(tree.root(), true));
        assert!(!tree.is_effectively_locked(tree.root()));
    }

    #[test]
    fn duplicate_preserves_local_lock_only() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.move_layer(layer, group, 0));
        assert!(tree.set_locked(group, true));

        let copied_layer = tree.duplicate_layer(layer).unwrap().root;
        assert!(!tree.is_locked(copied_layer));
        assert!(tree.is_effectively_locked(copied_layer));

        assert!(tree.set_locked(layer, true));
        let copied_locked_layer = tree.duplicate_layer(layer).unwrap().root;
        assert!(tree.is_locked(copied_locked_layer));
    }

    #[test]
    fn effective_visibility_follows_hidden_ancestors() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.move_layer(layer, group, 0));

        assert!(tree.is_effectively_visible(layer));
        assert!(tree.can_edit_layer_pixels(layer));

        tree.set_visible(group, false);

        assert!(!tree.is_effectively_visible(layer));
        assert!(!tree.can_edit_layer_pixels(layer));
        assert!(tree.can_set_layer_opacity(layer));
        assert!(tree.can_set_layer_blend_mode(layer));
    }

    #[test]
    fn hidden_layer_mask_pixels_are_not_editable() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        assert!(tree.add_layer_mask(layer));
        assert!(tree.can_edit_layer_mask_pixels(layer));

        tree.set_visible(layer, false);

        assert!(!tree.can_edit_layer_mask_pixels(layer));
    }

    #[test]
    fn property_predicates_ignore_visibility_and_enforce_node_kind() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(raster, "Group").unwrap();
        tree.set_visible(group, false);

        assert!(tree.can_set_layer_opacity(raster));
        assert!(tree.can_set_layer_blend_mode(raster));
        assert!(!tree.can_set_group_composite_mode(raster));
        assert!(tree.can_set_group_composite_mode(group));
        assert!(!tree.can_set_group_composite_mode(tree.root()));
        assert!(!tree.can_set_layer_opacity(LayerId::default()));
    }

    #[test]
    fn normal_property_setters_enforce_lock_and_layer_kind_invariants() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(raster, "Group").unwrap();
        let uv_mirror = tree
            .add_adjustment_layer_above(
                group,
                "UV Mirror",
                crate::core::adjustment::AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();

        assert!(tree.set_locked(raster, true));
        assert!(!tree.set_opacity(raster, 0.25));
        assert!(!tree.set_blend_mode(raster, LayerBlendMode::Multiply));
        assert_eq!(tree.get(raster).unwrap().props.opacity, 1.0);
        assert_eq!(
            tree.get(raster).unwrap().props.blend_mode,
            LayerBlendMode::Normal
        );

        assert!(!tree.can_set_layer_opacity(uv_mirror));
        assert!(!tree.set_opacity(uv_mirror, 0.25));
        assert!(!tree.restore_opacity(uv_mirror, 0.25));
        assert_eq!(tree.get(uv_mirror).unwrap().props.opacity, 1.0);
        assert_eq!(tree.get(uv_mirror).unwrap().effective_opacity(), 1.0);
        assert!(!tree.set_blend_mode(uv_mirror, LayerBlendMode::Multiply));
        assert_eq!(
            tree.get(uv_mirror).unwrap().props.blend_mode,
            LayerBlendMode::Normal
        );

        assert!(tree.set_locked(group, true));
        assert!(!tree.set_group_composite_mode(group, GroupCompositeMode::PassThrough));
        assert!(!tree.set_group_composite_mode(tree.root(), GroupCompositeMode::Isolated));
        assert_eq!(
            tree.group_composite_mode(group),
            Some(GroupCompositeMode::Isolated)
        );
        assert_eq!(
            tree.group_composite_mode(tree.root()),
            Some(GroupCompositeMode::PassThrough)
        );
    }

    #[test]
    fn changing_an_adjustment_to_uv_mirror_resets_and_preserves_full_opacity() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let adjustment = tree
            .add_adjustment_layer_above(
                raster,
                "Adjustment",
                crate::core::adjustment::AdjustmentKind::BrightnessContrast.default_adjustment(),
            )
            .unwrap();
        assert!(tree.set_opacity(adjustment, 0.25));

        assert!(tree.set_adjustment(
            adjustment,
            crate::core::adjustment::AdjustmentKind::UvMirror.default_adjustment(),
        ));
        assert_eq!(tree.get(adjustment).unwrap().props.opacity, 1.0);

        let duplicate = tree.duplicate_layer(adjustment).unwrap().root;
        assert_eq!(tree.get(duplicate).unwrap().props.opacity, 1.0);
        assert!(!tree.can_set_layer_opacity(duplicate));
    }

    #[test]
    fn uv_mirror_cannot_have_or_receive_a_layer_mask() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let source = tree.add_raster_layer_above(raster, "Source").unwrap();
        assert!(tree.add_layer_mask(source));
        let uv_mirror = tree
            .add_adjustment_layer_above(
                source,
                "UV Mirror",
                crate::core::adjustment::AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();

        assert!(!tree.can_have_layer_mask(uv_mirror));
        assert!(!tree.can_add_layer_mask(uv_mirror));
        assert!(!tree.add_layer_mask(uv_mirror));
        assert!(!tree.can_move_layer_mask(source, uv_mirror));
        assert!(!tree.move_layer_mask(source, uv_mirror));
        assert!(tree.has_layer_mask(source));
    }

    #[test]
    fn masked_adjustment_must_remove_its_mask_before_becoming_uv_mirror() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let adjustment = tree
            .add_adjustment_layer_above(
                raster,
                "Adjustment",
                crate::core::adjustment::AdjustmentKind::Curves.default_adjustment(),
            )
            .unwrap();
        assert!(tree.add_layer_mask(adjustment));

        assert!(!tree.set_adjustment(
            adjustment,
            crate::core::adjustment::AdjustmentKind::UvMirror.default_adjustment(),
        ));
        assert_eq!(
            tree.adjustment(adjustment).unwrap().kind(),
            crate::core::adjustment::AdjustmentKind::Curves
        );
    }

    #[test]
    fn history_restore_bypasses_lock_and_restores_both_composite_settings() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(raster, "Group").unwrap();
        assert!(tree.set_locked(group, true));

        assert!(tree.restore_opacity(group, 0.4));
        assert!(tree.restore_composite_settings(
            group,
            LayerBlendMode::Multiply,
            GroupCompositeMode::PassThrough,
        ));
        assert_eq!(tree.get(group).unwrap().props.opacity, 0.4);
        assert_eq!(
            tree.get(group).unwrap().props.blend_mode,
            LayerBlendMode::Multiply
        );
        assert_eq!(
            tree.group_composite_mode(group),
            Some(GroupCompositeMode::PassThrough)
        );
    }

    #[test]
    fn solid_fill_is_non_paintable_maskable_and_has_no_raster_surface_owner() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let fill = tree
            .add_solid_fill_layer_above(raster, "Fill", [0.25, 0.5, 0.75])
            .unwrap();

        assert!(tree.is_solid_fill(fill));
        assert!(!tree.is_paintable(fill));
        assert_eq!(tree.solid_fill_color(fill), Some([0.25, 0.5, 0.75]));
        assert!(tree.can_add_layer_mask(fill));
        assert!(tree.add_layer_mask(fill));
        assert!(tree.can_edit_layer_mask_pixels(fill));
        assert_eq!(
            tree.layer_surface_owners()
                .into_iter()
                .find(|(layer_id, _)| *layer_id == fill),
            Some((fill, false))
        );
    }

    #[test]
    fn solid_fill_duplicate_preserves_color_and_mask_without_becoming_raster() {
        let mut tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let fill = tree
            .add_solid_fill_layer_above(raster, "Fill", [0.1, 0.2, 0.3])
            .unwrap();
        assert!(tree.add_layer_mask(fill));

        let duplicate = tree.duplicate_layer(fill).unwrap().root;

        assert_eq!(tree.solid_fill_color(duplicate), Some([0.1, 0.2, 0.3]));
        assert!(tree.has_layer_mask(duplicate));
        assert!(!tree.is_paintable(duplicate));
        assert_eq!(
            tree.layer_surface_owners()
                .into_iter()
                .find(|(layer_id, _)| *layer_id == duplicate),
            Some((duplicate, false))
        );
    }

    #[test]
    fn moving_layer_mask_transfers_properties_and_overwrites_target() {
        let mut tree = LayerTree::new_default_raster();
        let source = tree.default_raster_layer().unwrap();
        let target = tree.add_group_above(source, "Group").unwrap();
        assert!(tree.add_layer_mask(source));
        tree.set_layer_mask_enabled(source, false);
        assert!(tree.add_layer_mask(target));

        assert!(tree.can_move_layer_mask(source, target));
        assert!(tree.move_layer_mask(source, target));

        assert!(!tree.has_layer_mask(source));
        assert_eq!(
            tree.layer_mask(target).map(|mask| mask.enabled),
            Some(false)
        );
    }

    #[test]
    fn moving_layer_mask_rejects_same_or_locked_layers() {
        let mut tree = LayerTree::new_default_raster();
        let source = tree.default_raster_layer().unwrap();
        let target = tree.add_group_above(source, "Group").unwrap();
        assert!(tree.add_layer_mask(source));

        assert!(!tree.can_move_layer_mask(source, source));
        assert!(!tree.move_layer_mask(source, source));
        assert!(tree.set_locked(target, true));
        assert!(!tree.can_move_layer_mask(source, target));
        assert!(!tree.move_layer_mask(source, target));
        assert!(tree.has_layer_mask(source));
        assert!(!tree.has_layer_mask(target));
    }

    #[test]
    fn layer_masks_can_be_added_to_groups_but_not_root() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();

        assert!(tree.can_add_layer_mask(group));
        assert!(tree.add_layer_mask(group));
        assert!(tree.has_layer_mask(group));
        assert!(!tree.can_add_layer_mask(group));

        assert!(!tree.can_add_layer_mask(tree.root()));
        assert!(!tree.add_layer_mask(tree.root()));
    }

    #[test]
    fn pass_through_supports_opacity_and_masks_but_only_identity_groups_flatten() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();

        tree.set_opacity(group, 0.5);
        assert!(!tree.can_use_identity_pass_through(group));
        assert!(tree.set_group_composite_mode(group, GroupCompositeMode::PassThrough));
        assert!(!tree.is_identity_pass_through_group(group));
        assert!(tree.can_set_layer_opacity(group));
        assert!(tree.can_add_layer_mask(group));

        tree.set_opacity(group, 1.0);
        assert!(tree.add_layer_mask(group));
        assert!(!tree.can_use_identity_pass_through(group));
        assert!(!tree.is_identity_pass_through_group(group));

        assert!(tree.delete_layer_mask(group));
        assert!(tree.can_use_identity_pass_through(group));
        assert!(tree.is_identity_pass_through_group(group));

        tree.set_opacity(group, 0.25);
        assert_eq!(tree.get(group).unwrap().props.opacity, 0.25);
        assert!(!tree.is_identity_pass_through_group(group));
    }

    #[test]
    fn selecting_a_blend_mode_restores_group_isolation() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(layer, "Group").unwrap();
        assert!(tree.set_group_composite_mode(group, GroupCompositeMode::PassThrough));

        tree.set_blend_mode(group, LayerBlendMode::Multiply);

        let props = &tree.get(group).unwrap().props;
        assert_eq!(props.blend_mode, LayerBlendMode::Multiply);
        assert_eq!(
            tree.group_composite_mode(group),
            Some(GroupCompositeMode::Isolated)
        );
    }

    #[test]
    fn duplicate_group_copies_subtree_and_returns_layer_map() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(base, "Group").unwrap();
        tree.set_visible(group, false);
        tree.add_layer_mask(group);
        let child = tree.add_raster_layer_to_group(group, "Child").unwrap();
        let nested = tree.add_group_to_group(group, "Nested").unwrap();
        let nested_child = tree
            .add_raster_layer_to_group(nested, "Nested Child")
            .unwrap();

        let duplicate = tree.duplicate_layer(group).unwrap();

        assert_eq!(
            duplicate
                .layer_map
                .iter()
                .map(|(from, _)| *from)
                .collect::<Vec<_>>(),
            vec![group, child, nested, nested_child]
        );
        let copied_group = duplicate.root;
        let copied_group_node = tree.get(copied_group).unwrap();
        assert_eq!(copied_group_node.props.name, "Group (copy)");
        assert!(!copied_group_node.props.visible);
        assert!(copied_group_node.mask.is_some());

        let copied_child = duplicate.layer_map[1].1;
        let copied_nested = duplicate.layer_map[2].1;
        let copied_nested_child = duplicate.layer_map[3].1;
        assert_eq!(tree.get(copied_child).unwrap().props.name, "Child");
        assert_eq!(tree.get(copied_nested).unwrap().props.name, "Nested");
        assert_eq!(
            tree.get(copied_nested_child).unwrap().props.name,
            "Nested Child"
        );
        assert_eq!(tree.get(copied_child).unwrap().parent, Some(copied_group));
        assert_eq!(tree.get(copied_nested).unwrap().parent, Some(copied_group));
        assert_eq!(
            tree.get(copied_nested_child).unwrap().parent,
            Some(copied_nested)
        );

        let (parent, index) = tree.parent_and_index(group).unwrap();
        assert_eq!(
            tree.parent_and_index(copied_group),
            Some((parent, index + 1))
        );
    }

    #[test]
    fn duplicate_root_is_rejected() {
        let mut tree = LayerTree::new_default_raster();

        assert!(tree.duplicate_layer(tree.root()).is_none());
    }

    #[test]
    fn duplicate_names_use_copy_then_lowest_available_number() {
        let mut tree = LayerTree::new_default_raster();
        let layer = tree.default_raster_layer().unwrap();
        tree.rename_layer(layer, "Layer".to_owned());

        let copy = tree.duplicate_layer(layer).unwrap().root;
        let second = tree.duplicate_layer(copy).unwrap().root;
        let third = tree.duplicate_layer(layer).unwrap().root;

        assert_eq!(tree.get(copy).unwrap().props.name, "Layer (copy)");
        assert_eq!(tree.get(second).unwrap().props.name, "Layer (2)");
        assert_eq!(tree.get(third).unwrap().props.name, "Layer (3)");

        assert_eq!(tree.remove_layer(second), RemoveLayerResult::Removed);
        let reused = tree.duplicate_layer(layer).unwrap().root;
        assert_eq!(tree.get(reused).unwrap().props.name, "Layer (2)");
    }

    #[test]
    fn duplicate_names_continue_numeric_suffix_sequence() {
        let mut tree = LayerTree::new_default_raster();
        let numeric_source = tree.default_raster_layer().unwrap();
        tree.rename_layer(numeric_source, "Other (2)".to_owned());
        tree.add_raster_layer_to_root("Other (copy)").unwrap();

        let numeric_copy = tree.duplicate_layer(numeric_source).unwrap().root;

        assert_eq!(tree.get(numeric_copy).unwrap().props.name, "Other (3)");
    }

    #[test]
    fn duplicate_name_availability_is_scoped_to_the_parent() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group_a = tree.add_group_above(base, "A").unwrap();
        let group_b = tree.add_group_above(group_a, "B").unwrap();
        let layer_a = tree.add_raster_layer_to_group(group_a, "Layer").unwrap();
        let layer_b = tree.add_raster_layer_to_group(group_b, "Layer").unwrap();

        let copy_a = tree.duplicate_layer(layer_a).unwrap().root;
        let copy_b = tree.duplicate_layer(layer_b).unwrap().root;

        assert_eq!(tree.get(copy_a).unwrap().props.name, "Layer (copy)");
        assert_eq!(tree.get(copy_b).unwrap().props.name, "Layer (copy)");
    }

    #[test]
    fn layer_merge_plan_orders_adjacent_siblings_and_uses_top_name() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        tree.rename_layer(base, "Base".to_owned());
        let top = tree.add_raster_layer_above(base, "Top").unwrap();

        let plan = tree.layer_merge_plan(&[top, base]).unwrap();

        assert_eq!(plan.parent_id, tree.root());
        assert_eq!(plan.layer_ids, vec![base, top]);
        assert_eq!(plan.insertion_index, 0);
        assert_eq!(plan.output_name, "Top");

        let merged = tree
            .insert_raster_layer(plan.parent_id, plan.insertion_index, plan.output_name)
            .unwrap();
        assert_eq!(tree.parent_and_index(merged), Some((tree.root(), 0)));
    }

    #[test]
    fn layer_merge_plan_rejects_non_contiguous_and_different_parent_layers() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let middle = tree.add_raster_layer_above(base, "Middle").unwrap();
        let top = tree.add_raster_layer_above(middle, "Top").unwrap();
        assert!(tree.layer_merge_plan(&[base, top]).is_none());

        let group = tree.add_group_above(top, "Group").unwrap();
        let child = tree.add_raster_layer_to_group(group, "Child").unwrap();
        assert!(tree.layer_merge_plan(&[top, child]).is_none());
    }

    #[test]
    fn layer_merge_plan_accepts_adjustment_and_non_normal_roots() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let adjustment = tree
            .add_adjustment_layer_above(
                base,
                "Invert",
                crate::core::adjustment::AdjustmentKind::Invert.default_adjustment(),
            )
            .unwrap();
        let adjustment_plan = tree.layer_merge_plan(&[base, adjustment]).unwrap();
        assert_eq!(adjustment_plan.layer_ids, vec![base, adjustment]);

        let pass_through = tree.add_group_above(adjustment, "Pass Through").unwrap();
        assert!(tree.set_group_composite_mode(pass_through, GroupCompositeMode::PassThrough));
        let sibling = tree
            .add_raster_layer_above(pass_through, "Sibling")
            .unwrap();
        let pass_through_plan = tree.layer_merge_plan(&[pass_through, sibling]).unwrap();
        assert_eq!(pass_through_plan.layer_ids, vec![pass_through, sibling]);

        let blended = tree.add_raster_layer_above(sibling, "Multiply").unwrap();
        tree.set_blend_mode(blended, LayerBlendMode::Multiply);
        let blended_plan = tree.layer_merge_plan(&[sibling, blended]).unwrap();
        assert_eq!(blended_plan.layer_ids, vec![sibling, blended]);
    }

    #[test]
    fn layer_merge_plan_accepts_an_isolated_group_root() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(base, "Group").unwrap();
        tree.add_raster_layer_to_group(group, "Child").unwrap();

        let plan = tree.layer_merge_plan(&[base, group]).unwrap();

        assert_eq!(plan.layer_ids, vec![base, group]);
        assert_eq!(plan.output_name, "Group");
    }

    #[test]
    fn layer_merge_plan_accepts_a_single_group() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(base, "Group").unwrap();
        tree.add_raster_layer_to_group(group, "Child").unwrap();

        let plan = tree.layer_merge_plan(&[group]).unwrap();

        assert_eq!(plan.parent_id, tree.root());
        assert_eq!(plan.layer_ids, vec![group]);
        assert_eq!(plan.insertion_index, 1);
        assert_eq!(plan.output_name, "Group");
        assert!(tree.layer_merge_plan(&[base]).is_none());

        assert!(tree.set_group_composite_mode(group, GroupCompositeMode::PassThrough));
        let pass_through_plan = tree.layer_merge_plan(&[group]).unwrap();
        assert_eq!(pass_through_plan.layer_ids, vec![group]);
    }
}
