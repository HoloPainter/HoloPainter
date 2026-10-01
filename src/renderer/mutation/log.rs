use std::collections::{BTreeMap, BTreeSet};

use crate::{
    core::{damage::union_rect, geometry::RectU32},
    renderer::mutation::{
        presentation::PresentDirty, selection_damage::SelectionDamageSet,
        surface_damage::SurfaceDamageSet,
    },
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneMutation {
    uploaded: bool,
}

impl SceneMutation {
    pub(crate) fn uploaded(&mut self) {
        self.uploaded = true;
    }

    pub(crate) fn has_any(self) -> bool {
        self.uploaded
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CompositeMutationSet {
    tree_material_indices: BTreeSet<usize>,
    value_material_indices: BTreeSet<usize>,
    value_rects: BTreeMap<usize, RectU32>,
}

impl CompositeMutationSet {
    pub(crate) fn material(&mut self, material_index: usize) {
        self.value_material_indices.remove(&material_index);
        self.value_rects.remove(&material_index);
        self.tree_material_indices.insert(material_index);
    }

    pub(crate) fn layer_value(&mut self, material_index: usize) {
        if !self.tree_material_indices.contains(&material_index) {
            self.value_rects.remove(&material_index);
            self.value_material_indices.insert(material_index);
        }
    }

    pub(crate) fn layer_value_rect(&mut self, material_index: usize, rect: RectU32) {
        if self.tree_material_indices.contains(&material_index)
            || self.value_material_indices.contains(&material_index)
        {
            return;
        }
        self.value_rects
            .entry(material_index)
            .and_modify(|current| *current = union_rect(*current, rect))
            .or_insert(rect);
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for material_index in other.tree_material_indices {
            self.material(material_index);
        }
        for material_index in other.value_material_indices {
            self.layer_value(material_index);
        }
        for (material_index, rect) in other.value_rects {
            self.layer_value_rect(material_index, rect);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.tree_material_indices.is_empty()
            && self.value_material_indices.is_empty()
            && self.value_rects.is_empty()
    }

    pub(crate) fn material_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.tree_material_indices
            .union(&self.value_material_indices)
            .copied()
            .chain(self.value_rects.keys().copied())
    }

    pub(crate) fn tree_material_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.tree_material_indices.iter().copied()
    }

    pub(crate) fn value_material_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.value_material_indices.iter().copied()
    }

    pub(crate) fn value_rects(&self) -> impl Iterator<Item = (usize, RectU32)> + '_ {
        self.value_rects.iter().map(|(index, rect)| (*index, *rect))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewMutation {
    viewport: bool,
    uv_view: bool,
}

impl ViewMutation {
    pub(crate) fn viewport(&mut self) {
        self.viewport = true;
    }

    pub(crate) fn uv_view(&mut self) {
        self.uv_view = true;
    }

    pub(crate) fn viewport_dirty(self) -> bool {
        self.viewport
    }

    pub(crate) fn uv_view_dirty(self) -> bool {
        self.uv_view
    }
}

/// Renderer mutation result, grouped by renderer-owned state rather than by UI
/// presentation output.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct MutationLog {
    /// Surface changes visible to composite/presentation invalidation.
    pub(crate) surfaces: SurfaceDamageSet,
    /// Persistent GPU-produced surface changes that must be committed to the
    /// document surface repository at the renderer batch boundary.
    pub(crate) surface_commits: SurfaceDamageSet,
    pub(crate) selections: SelectionDamageSet,
    /// Composite model/material changes visible to composite output planning.
    pub(crate) composites: CompositeMutationSet,
    pub(crate) scene: SceneMutation,
    pub(crate) view: ViewMutation,
}

impl MutationLog {
    pub(crate) fn merge(&mut self, other: Self) {
        self.surfaces.extend(other.surfaces);
        self.surface_commits.extend(other.surface_commits);
        self.selections.extend(other.selections);
        self.composites.extend(other.composites);
        if other.scene.has_any() {
            self.scene.uploaded();
        }
        if other.view.viewport_dirty() {
            self.view.viewport();
        }
        if other.view.uv_view_dirty() {
            self.view.uv_view();
        }
    }

    pub(crate) fn present_dirty(&self) -> PresentDirty {
        let mut dirty = PresentDirty::default();
        if !self.surfaces.is_empty() || self.scene.has_any() || !self.composites.is_empty() {
            dirty.request_material_textures();
        }
        if !self.selections.is_empty() {
            dirty.request_selection();
        }
        if self.view.viewport_dirty() {
            dirty.request_viewport();
        }
        if self.view.uv_view_dirty() {
            dirty.request_uv_view();
        }
        dirty
    }
}
