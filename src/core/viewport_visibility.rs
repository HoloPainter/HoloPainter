use std::collections::HashSet;

use crate::core::{document::MeshId, material::MaterialIndex};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewportSceneVisibility {
    hidden_meshes: HashSet<MeshId>,
    hidden_materials: HashSet<MaterialIndex>,
    revision: u64,
}

impl ViewportSceneVisibility {
    pub fn from_hidden(
        hidden_meshes: impl IntoIterator<Item = MeshId>,
        hidden_materials: impl IntoIterator<Item = MaterialIndex>,
    ) -> Self {
        Self {
            hidden_meshes: hidden_meshes.into_iter().collect(),
            hidden_materials: hidden_materials.into_iter().collect(),
            revision: 0,
        }
    }

    pub fn mesh_visible(&self, mesh_id: MeshId) -> bool {
        !self.hidden_meshes.contains(&mesh_id)
    }

    pub fn material_visible(&self, material_index: MaterialIndex) -> bool {
        !self.hidden_materials.contains(&material_index)
    }

    pub fn geometry_visible(&self, mesh_id: MeshId, material_index: MaterialIndex) -> bool {
        self.mesh_visible(mesh_id) && self.material_visible(material_index)
    }

    pub fn set_mesh_visible(&mut self, mesh_id: MeshId, visible: bool) -> bool {
        let changed = if visible {
            self.hidden_meshes.remove(&mesh_id)
        } else {
            self.hidden_meshes.insert(mesh_id)
        };
        self.bump_revision_if(changed);
        changed
    }

    pub fn set_material_visible(&mut self, material_index: MaterialIndex, visible: bool) -> bool {
        let changed = if visible {
            self.hidden_materials.remove(&material_index)
        } else {
            self.hidden_materials.insert(material_index)
        };
        self.bump_revision_if(changed);
        changed
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn clear(&mut self) -> bool {
        let changed = !self.hidden_meshes.is_empty() || !self.hidden_materials.is_empty();
        if changed {
            self.hidden_meshes.clear();
            self.hidden_materials.clear();
            self.bump_revision_if(true);
        }
        changed
    }

    pub fn clear_meshes(&mut self) -> bool {
        let changed = !self.hidden_meshes.is_empty();
        if changed {
            self.hidden_meshes.clear();
            self.bump_revision_if(true);
        }
        changed
    }

    fn bump_revision_if(&mut self, changed: bool) {
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::document::MeshId;

    use super::ViewportSceneVisibility;

    #[test]
    fn visibility_defaults_to_all_visible_and_combines_mesh_and_material() {
        let mut visibility = ViewportSceneVisibility::default();
        let mesh_id = MeshId(3);

        assert!(visibility.geometry_visible(mesh_id, 7.into()));
        assert!(visibility.set_mesh_visible(mesh_id, false));
        assert!(!visibility.geometry_visible(mesh_id, 7.into()));
        assert!(visibility.set_mesh_visible(mesh_id, true));
        assert!(visibility.set_material_visible(7.into(), false));
        assert!(!visibility.geometry_visible(mesh_id, 7.into()));
        assert!(visibility.geometry_visible(mesh_id, 8.into()));
    }

    #[test]
    fn revision_changes_only_when_visibility_changes() {
        let mut visibility = ViewportSceneVisibility::default();
        let mesh_id = MeshId(2);

        assert_eq!(visibility.revision(), 0);
        assert!(!visibility.set_mesh_visible(mesh_id, true));
        assert_eq!(visibility.revision(), 0);
        assert!(visibility.set_mesh_visible(mesh_id, false));
        assert_eq!(visibility.revision(), 1);
        assert!(!visibility.set_mesh_visible(mesh_id, false));
        assert_eq!(visibility.revision(), 1);
        assert!(visibility.set_material_visible(4.into(), false));
        assert_eq!(visibility.revision(), 2);
        assert!(visibility.clear());
        assert_eq!(visibility.revision(), 3);
        assert!(visibility.geometry_visible(mesh_id, 4.into()));
        assert!(!visibility.clear());
        assert_eq!(visibility.revision(), 3);
    }

    #[test]
    fn clearing_meshes_preserves_material_visibility() {
        let mut visibility = ViewportSceneVisibility::default();
        visibility.set_mesh_visible(MeshId(2), false);
        visibility.set_material_visible(4.into(), false);

        assert!(visibility.clear_meshes());
        assert!(visibility.mesh_visible(MeshId(2)));
        assert!(!visibility.material_visible(4.into()));
    }
}
