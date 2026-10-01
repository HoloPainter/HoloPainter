use crate::core::{
    document::{ActiveLayerTarget, Document, MeshId},
    material::MaterialIndex,
    stroke::PaintSurfaceSet,
    surface::PaintSurfaceId,
};

use super::state::{AppState, DocumentSession, EditorDocumentState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::application) enum PaintTargetScope {
    FocusedMaterial,
    Material(MaterialIndex),
    Materials(Vec<MaterialIndex>),
    AllMaterials,
    Mesh(MeshId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintEditBlockReason {
    ActiveLayerIsGroup,
    ActiveLayerLocked,
    ActiveLayerHidden,
    MaterialExcluded,
    ActiveTargetNotEditable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintEditDecision<T> {
    Allowed(T),
    Blocked(PaintEditBlockReason),
    Unavailable,
}

impl<T> PaintEditDecision<T> {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed(_))
    }

    pub fn blocked_reason(&self) -> Option<PaintEditBlockReason> {
        match self {
            Self::Blocked(reason) => Some(*reason),
            Self::Allowed(_) | Self::Unavailable => None,
        }
    }

    pub(in crate::application) fn into_allowed(self) -> Option<T> {
        match self {
            Self::Allowed(value) => Some(value),
            Self::Blocked(_) | Self::Unavailable => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct ResolvedPaintTarget {
    target: PaintSurfaceId,
    surfaces: PaintSurfaceSet,
}

impl ResolvedPaintTarget {
    fn single(surface: PaintSurfaceId) -> Self {
        Self {
            target: surface,
            surfaces: PaintSurfaceSet::single(surface),
        }
    }

    fn from_surfaces(surfaces: Vec<PaintSurfaceId>) -> Option<Self> {
        let target = surfaces.first().copied()?;
        Some(Self {
            target,
            surfaces: PaintSurfaceSet::from_vec(surfaces),
        })
    }

    pub(in crate::application) fn target(&self) -> PaintSurfaceId {
        self.target
    }

    pub(in crate::application) fn surfaces_vec(&self) -> Vec<PaintSurfaceId> {
        self.surfaces.as_slice().to_vec()
    }
}

impl DocumentSession {
    pub(in crate::application) fn resolve_paint_target(
        &self,
        scope: PaintTargetScope,
    ) -> Option<ResolvedPaintTarget> {
        let document = self.document()?;
        self.editor.resolve_paint_target(document, scope)
    }

    pub(in crate::application) fn paint_edit_permission(
        &self,
        anchor_material_index: Option<usize>,
    ) -> PaintEditDecision<()> {
        let Some(document) = self.document() else {
            return PaintEditDecision::Unavailable;
        };
        let active = self.editor.resolved(document);
        active.paint_edit_permission(document, anchor_material_index)
    }

    pub(in crate::application) fn resolve_paint_edit_target(
        &self,
        scope: PaintTargetScope,
    ) -> PaintEditDecision<ResolvedPaintTarget> {
        let Some(document) = self.document() else {
            return PaintEditDecision::Unavailable;
        };
        let active = self.editor.resolved(document);
        if let PaintEditDecision::Blocked(reason) = active.paint_edit_permission(document, None) {
            return PaintEditDecision::Blocked(reason);
        }
        let Some(target) = active.resolve_paint_target(document, scope) else {
            return PaintEditDecision::Unavailable;
        };
        let allowed_surfaces = target
            .surfaces_vec()
            .into_iter()
            .filter(|surface| {
                document
                    .material_id(surface.material_index())
                    .is_some_and(|material_id| {
                        document
                            .layer_tree
                            .effective_material_allowed(active.active_layer_id, material_id)
                    })
            })
            .collect::<Vec<_>>();
        match ResolvedPaintTarget::from_surfaces(allowed_surfaces) {
            Some(target) => PaintEditDecision::Allowed(target),
            None => PaintEditDecision::Blocked(PaintEditBlockReason::MaterialExcluded),
        }
    }
}

impl EditorDocumentState {
    fn paint_edit_permission(
        &self,
        document: &Document,
        anchor_material_index: Option<usize>,
    ) -> PaintEditDecision<()> {
        if let Some(reason) = self.paint_edit_block_reason(document) {
            return PaintEditDecision::Blocked(reason);
        }
        let Some(material_index) = anchor_material_index else {
            return PaintEditDecision::Allowed(());
        };
        let Some(material_id) = document.material_id(material_index.into()) else {
            return PaintEditDecision::Unavailable;
        };
        if document
            .layer_tree
            .effective_material_allowed(self.active_layer_id, material_id)
        {
            PaintEditDecision::Allowed(())
        } else {
            PaintEditDecision::Blocked(PaintEditBlockReason::MaterialExcluded)
        }
    }

    fn paint_edit_block_reason(&self, document: &Document) -> Option<PaintEditBlockReason> {
        match self.active_target(document) {
            ActiveLayerTarget::Raster => {
                if document
                    .layer_tree
                    .can_edit_layer_pixels(self.active_layer_id)
                {
                    None
                } else if document.layer_tree.is_group(self.active_layer_id) {
                    Some(PaintEditBlockReason::ActiveLayerIsGroup)
                } else if !document.layer_tree.is_paintable(self.active_layer_id) {
                    Some(PaintEditBlockReason::ActiveTargetNotEditable)
                } else if document
                    .layer_tree
                    .is_effectively_locked(self.active_layer_id)
                {
                    Some(PaintEditBlockReason::ActiveLayerLocked)
                } else if !document
                    .layer_tree
                    .is_effectively_visible(self.active_layer_id)
                {
                    Some(PaintEditBlockReason::ActiveLayerHidden)
                } else {
                    Some(PaintEditBlockReason::ActiveTargetNotEditable)
                }
            }
            ActiveLayerTarget::SolidFill | ActiveLayerTarget::Adjustment => {
                Some(PaintEditBlockReason::ActiveTargetNotEditable)
            }
            ActiveLayerTarget::LayerMask => {
                if document
                    .layer_tree
                    .can_edit_layer_mask_pixels(self.active_layer_id)
                {
                    None
                } else if !document.layer_tree.has_layer_mask(self.active_layer_id) {
                    Some(PaintEditBlockReason::ActiveTargetNotEditable)
                } else if document
                    .layer_tree
                    .is_effectively_locked(self.active_layer_id)
                {
                    Some(PaintEditBlockReason::ActiveLayerLocked)
                } else if !document
                    .layer_tree
                    .is_effectively_visible(self.active_layer_id)
                {
                    Some(PaintEditBlockReason::ActiveLayerHidden)
                } else {
                    Some(PaintEditBlockReason::ActiveTargetNotEditable)
                }
            }
            ActiveLayerTarget::EmbeddedImage | ActiveLayerTarget::Structure => {
                if document.layer_tree.is_group(self.active_layer_id) {
                    Some(PaintEditBlockReason::ActiveLayerIsGroup)
                } else {
                    Some(PaintEditBlockReason::ActiveTargetNotEditable)
                }
            }
        }
    }

    pub(in crate::application) fn active_surface_for_material(
        &self,
        document: &Document,
        material_index: MaterialIndex,
    ) -> Option<PaintSurfaceId> {
        let active = self.resolved(document);
        match active.active_target(document) {
            ActiveLayerTarget::Raster => document
                .layer_tree
                .is_paintable(active.active_layer_id)
                .then_some(PaintSurfaceId::raster(
                    material_index,
                    active.active_layer_id,
                )),
            ActiveLayerTarget::SolidFill | ActiveLayerTarget::Adjustment => None,
            ActiveLayerTarget::LayerMask => document
                .layer_tree
                .has_layer_mask(active.active_layer_id)
                .then_some(PaintSurfaceId::layer_mask(
                    material_index,
                    active.active_layer_id,
                )),
            ActiveLayerTarget::EmbeddedImage | ActiveLayerTarget::Structure => None,
        }
    }

    pub(in crate::application) fn resolve_paint_target(
        &self,
        document: &Document,
        scope: PaintTargetScope,
    ) -> Option<ResolvedPaintTarget> {
        let active = self.resolved(document);
        let surface_for_material =
            |material_index| active.active_surface_for_material(document, material_index);
        match scope {
            PaintTargetScope::FocusedMaterial => (active.focused_material_index.as_usize()
                < document.materials.len())
            .then(|| surface_for_material(active.focused_material_index))
            .flatten()
            .map(ResolvedPaintTarget::single),
            PaintTargetScope::Material(material_index) => (material_index.as_usize()
                < document.materials.len())
            .then(|| surface_for_material(material_index))
            .flatten()
            .map(ResolvedPaintTarget::single),
            PaintTargetScope::Materials(mut material_indices) => {
                material_indices.sort_unstable();
                material_indices.dedup();
                ResolvedPaintTarget::from_surfaces(
                    material_indices
                        .into_iter()
                        .filter(|&index| index.as_usize() < document.materials.len())
                        .filter_map(surface_for_material)
                        .collect(),
                )
            }
            PaintTargetScope::AllMaterials => ResolvedPaintTarget::from_surfaces(
                (0..document.materials.len())
                    .map(MaterialIndex::from)
                    .filter_map(surface_for_material)
                    .collect(),
            ),
            PaintTargetScope::Mesh(mesh_id) => {
                let mut material_indices = document
                    .mesh
                    .triangle_mesh_ids
                    .iter()
                    .enumerate()
                    .filter_map(|(triangle_index, &id)| {
                        (id == mesh_id).then_some(MaterialIndex::from(
                            document.mesh.material_index_for_triangle(triangle_index),
                        ))
                    })
                    .collect::<Vec<_>>();
                material_indices.sort_unstable();
                material_indices.dedup();
                ResolvedPaintTarget::from_surfaces(
                    material_indices
                        .into_iter()
                        .filter(|&index| index.as_usize() < document.materials.len())
                        .filter_map(surface_for_material)
                        .collect(),
                )
            }
        }
    }
}

impl AppState {
    pub fn paint_edit_permission(
        &self,
        anchor_material_index: Option<usize>,
    ) -> PaintEditDecision<()> {
        self.document.paint_edit_permission(anchor_material_index)
    }
}
