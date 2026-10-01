use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;

use crate::core::{stroke::SurfaceProjectionId, surface::PaintSurfaceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceViewportSourceSnapshotKey {
    pub(crate) viewport_size: [u32; 2],
    pub(crate) view_proj_bits: [u32; 16],
    pub(crate) camera_world_bits: [u32; 3],
    pub(crate) source_material_indices: Vec<usize>,
    pub(crate) scene_visibility_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SurfaceViewportMaterialSourceKey {
    StrokeSource(usize),
    BatchSource(usize),
    PersistentSurface(PaintSurfaceId),
}

#[derive(Default)]
pub(crate) struct SurfaceViewportRuntimeCache {
    source_snapshot_keys: HashMap<SurfaceProjectionId, SurfaceViewportSourceSnapshotKey>,
    material_bind_groups: HashMap<SurfaceViewportMaterialSourceKey, wgpu::BindGroup>,
}

impl SurfaceViewportRuntimeCache {
    pub(crate) fn source_snapshot_key(
        &self,
        projection_id: SurfaceProjectionId,
    ) -> Option<&SurfaceViewportSourceSnapshotKey> {
        self.source_snapshot_keys.get(&projection_id)
    }

    pub(crate) fn set_source_snapshot_key(
        &mut self,
        projection_id: SurfaceProjectionId,
        key: SurfaceViewportSourceSnapshotKey,
    ) {
        self.source_snapshot_keys.insert(projection_id, key);
    }

    pub(crate) fn material_bind_group(
        &self,
        key: SurfaceViewportMaterialSourceKey,
    ) -> Option<&wgpu::BindGroup> {
        self.material_bind_groups.get(&key)
    }

    pub(crate) fn insert_material_bind_group(
        &mut self,
        key: SurfaceViewportMaterialSourceKey,
        bind_group: wgpu::BindGroup,
    ) {
        self.material_bind_groups.insert(key, bind_group);
    }

    pub(crate) fn clear_material_bind_groups(&mut self) {
        self.material_bind_groups.clear();
    }
}

pub(crate) fn viewport_source_color_cache_name(projection_id: SurfaceProjectionId) -> &'static str {
    match projection_id {
        SurfaceProjectionId::Primary => "__surface_viewport_source_color_cache_primary",
        SurfaceProjectionId::MirrorX => "__surface_viewport_source_color_cache_mirror_x",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SurfaceViewportMaterialSourceKey, SurfaceViewportRuntimeCache,
        SurfaceViewportSourceSnapshotKey, viewport_source_color_cache_name,
    };
    use crate::core::stroke::SurfaceProjectionId;

    #[test]
    fn viewport_source_cache_names_are_projection_specific() {
        assert_ne!(
            viewport_source_color_cache_name(SurfaceProjectionId::Primary),
            viewport_source_color_cache_name(SurfaceProjectionId::MirrorX)
        );
    }

    #[test]
    fn viewport_source_snapshot_keys_are_kept_per_projection() {
        let primary = SurfaceViewportSourceSnapshotKey {
            viewport_size: [64, 64],
            view_proj_bits: [1; 16],
            camera_world_bits: [2; 3],
            source_material_indices: vec![0],
            scene_visibility_revision: 3,
        };
        let mirror = SurfaceViewportSourceSnapshotKey {
            viewport_size: [64, 64],
            view_proj_bits: [3; 16],
            camera_world_bits: [4; 3],
            source_material_indices: vec![0],
            scene_visibility_revision: 3,
        };
        let mut cache = SurfaceViewportRuntimeCache::default();
        cache.set_source_snapshot_key(SurfaceProjectionId::Primary, primary.clone());
        cache.set_source_snapshot_key(SurfaceProjectionId::MirrorX, mirror.clone());

        assert_eq!(
            cache.source_snapshot_key(SurfaceProjectionId::Primary),
            Some(&primary)
        );
        assert_eq!(
            cache.source_snapshot_key(SurfaceProjectionId::MirrorX),
            Some(&mirror)
        );
    }

    #[test]
    fn viewport_material_source_keys_keep_source_kinds_distinct() {
        assert_ne!(
            SurfaceViewportMaterialSourceKey::StrokeSource(2),
            SurfaceViewportMaterialSourceKey::BatchSource(2)
        );
    }
}
