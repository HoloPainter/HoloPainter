use std::{cell::RefCell, collections::HashMap};

use eframe::egui_wgpu::wgpu;

const ACTIVE_RUN_ABOVE_STEP_CHECKPOINT_BUDGET: usize = 8;
const GENERAL_PREFIX_CHECKPOINT_MIN_CHILDREN: usize = 4;

use crate::{
    core::{
        geometry::RectU32,
        render_report::{GpuTextureMetrics, RenderMetrics},
        surface::{CompositeGroup, CompositeNode, CompositeTree, LayerId, PaintSurfaceId},
    },
    renderer::{
        document::surfaces::SurfacePixelFormat,
        features::composite::{
            dirty::CompositeDirtyRegion,
            planner::{GroupDamageDependency, group_damage_dependency, mirror_rect_for_uv_mirror},
        },
        gpu::{
            create_composite_output_texture, create_mask_texture, create_paint_texture,
            create_render_scratch_texture,
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositeNeed {
    Clean,
    PartialDirty,
    FullDirty,
    SignatureChanged,
}

struct CompositeTexture {
    size: [u32; 2],
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    dirty: CompositeDirtyRegion,
    signature: Option<CompositeTree>,
}

struct ActiveRunStepCache {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct ActiveRunBoundaryCache {
    _below_texture: wgpu::Texture,
    below_view: wgpu::TextureView,
    above_steps: Vec<ActiveRunStepCache>,
}

struct ActiveRunCheckpoints {
    size: [u32; 2],
    active_surfaces: Vec<PaintSurfaceId>,
    signature: CompositeTree,
    boundaries: Vec<ActiveRunBoundaryCache>,
    valid: bool,
}

struct ActiveRunSourceCacheEntry {
    source: PaintSurfaceId,
    size: [u32; 2],
    format: SurfacePixelFormat,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct ActiveRunSourceCache {
    active_surfaces: Vec<PaintSurfaceId>,
    entries: Vec<ActiveRunSourceCacheEntry>,
}

pub(crate) struct IsolatedGroupCacheEntry {
    pub(crate) layer_id: LayerId,
    pub(crate) size: [u32; 2],
    signature: Vec<CompositeNode>,
    surfaces: Vec<PaintSurfaceId>,
    pub(crate) _texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) dirty: CompositeDirtyRegion,
}

pub(crate) struct GeneralPrefixCheckpoint {
    pub(crate) size: [u32; 2],
    pub(crate) prefix_len: usize,
    signature: Vec<CompositeNode>,
    surfaces: Vec<PaintSurfaceId>,
    pub(crate) _texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) valid: bool,
}

pub(crate) struct GeneralCompositeMaterialCache {
    enabled: bool,
    isolated_groups: HashMap<LayerId, IsolatedGroupCacheEntry>,
    prefix_checkpoint: Option<GeneralPrefixCheckpoint>,
}

impl Default for GeneralCompositeMaterialCache {
    fn default() -> Self {
        Self {
            enabled: true,
            isolated_groups: HashMap::new(),
            prefix_checkpoint: None,
        }
    }
}

pub(crate) struct ActiveRunBoundaryCheckpointViews<'a> {
    pub(crate) below: &'a wgpu::TextureView,
    pub(crate) above_steps: Vec<&'a wgpu::TextureView>,
}

pub(crate) struct ActiveRunCheckpointViews<'a> {
    pub(crate) boundaries: Vec<ActiveRunBoundaryCheckpointViews<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveCompositeHint {
    surfaces: Vec<PaintSurfaceId>,
}

impl ActiveCompositeHint {
    pub(crate) fn from_surfaces(
        surfaces: impl IntoIterator<Item = PaintSurfaceId>,
    ) -> Option<Self> {
        let mut deduped = Vec::new();
        for surface in surfaces {
            if !deduped.contains(&surface) {
                deduped.push(surface);
            }
        }
        (!deduped.is_empty()).then_some(Self { surfaces: deduped })
    }

    pub(crate) fn contains(&self, surface: PaintSurfaceId) -> bool {
        self.surfaces.contains(&surface)
    }

    pub(crate) fn primary_surface(&self) -> PaintSurfaceId {
        self.surfaces[0]
    }

    pub(crate) fn surfaces(&self) -> &[PaintSurfaceId] {
        &self.surfaces
    }

    pub(crate) fn is_single(&self) -> bool {
        self.surfaces.len() == 1
    }
}

impl GeneralCompositeMaterialCache {
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            isolated_groups: HashMap::new(),
            prefix_checkpoint: None,
        }
    }

    pub(crate) fn take_isolated_group_cache(
        &mut self,
        device: &wgpu::Device,
        group: &CompositeGroup,
        size: [u32; 2],
        metrics: &mut RenderMetrics,
    ) -> Option<IsolatedGroupCacheEntry> {
        if !self.enabled {
            return None;
        }
        let layer_id = group.layer_id?;
        let signature = group.children.clone();
        let surfaces = surfaces_from_nodes(&signature);
        let mut entry = self.isolated_groups.remove(&layer_id).unwrap_or_else(|| {
            let (texture, view) =
                create_render_scratch_texture(device, size, "general_isolated_group_cache");
            IsolatedGroupCacheEntry {
                layer_id,
                size,
                signature: signature.clone(),
                surfaces: surfaces.clone(),
                _texture: texture,
                view,
                dirty: CompositeDirtyRegion::full(),
            }
        });
        if entry.size != size {
            let (texture, view) =
                create_render_scratch_texture(device, size, "general_isolated_group_cache");
            entry = IsolatedGroupCacheEntry {
                layer_id,
                size,
                signature,
                surfaces,
                _texture: texture,
                view,
                dirty: CompositeDirtyRegion::full(),
            };
        } else if entry.signature != signature {
            entry.signature = signature;
            entry.surfaces = surfaces;
            entry.dirty = CompositeDirtyRegion::full();
        }
        if entry.dirty.full || !entry.dirty.rects.is_empty() {
            metrics.composite_checkpoint_rebuilds =
                metrics.composite_checkpoint_rebuilds.saturating_add(1);
        } else {
            metrics.composite_checkpoint_reuses =
                metrics.composite_checkpoint_reuses.saturating_add(1);
        }
        Some(entry)
    }

    pub(crate) fn restore_isolated_group_cache(&mut self, entry: IsolatedGroupCacheEntry) {
        self.isolated_groups.insert(entry.layer_id, entry);
    }

    pub(crate) fn take_prefix_checkpoint(
        &mut self,
        device: &wgpu::Device,
        tree: &CompositeTree,
        size: [u32; 2],
        metrics: &mut RenderMetrics,
    ) -> Option<GeneralPrefixCheckpoint> {
        if !self.enabled {
            return None;
        }
        let child_count = tree.root.children.len();
        if child_count < GENERAL_PREFIX_CHECKPOINT_MIN_CHILDREN {
            self.prefix_checkpoint = None;
            return None;
        }
        let prefix_len = child_count / 2;
        let signature = tree.root.children[..prefix_len].to_vec();
        let surfaces = surfaces_from_nodes(&signature);
        let mut checkpoint = self.prefix_checkpoint.take().unwrap_or_else(|| {
            let (texture, view) =
                create_render_scratch_texture(device, size, "general_prefix_checkpoint");
            GeneralPrefixCheckpoint {
                size,
                prefix_len,
                signature: signature.clone(),
                surfaces: surfaces.clone(),
                _texture: texture,
                view,
                valid: false,
            }
        });
        if checkpoint.size != size {
            let (texture, view) =
                create_render_scratch_texture(device, size, "general_prefix_checkpoint");
            checkpoint = GeneralPrefixCheckpoint {
                size,
                prefix_len,
                signature,
                surfaces,
                _texture: texture,
                view,
                valid: false,
            };
        } else if checkpoint.prefix_len != prefix_len || checkpoint.signature != signature {
            checkpoint.prefix_len = prefix_len;
            checkpoint.signature = signature;
            checkpoint.surfaces = surfaces;
            checkpoint.valid = false;
        }
        if checkpoint.valid {
            metrics.composite_checkpoint_reuses =
                metrics.composite_checkpoint_reuses.saturating_add(1);
        } else {
            metrics.composite_checkpoint_rebuilds =
                metrics.composite_checkpoint_rebuilds.saturating_add(1);
        }
        Some(checkpoint)
    }

    pub(crate) fn restore_prefix_checkpoint(&mut self, checkpoint: GeneralPrefixCheckpoint) {
        self.prefix_checkpoint = Some(checkpoint);
    }

    fn mark_surface_dirty(&mut self, surface: PaintSurfaceId, rect: Option<RectU32>) {
        for entry in self.isolated_groups.values_mut() {
            if !entry.surfaces.contains(&surface) {
                continue;
            }
            if let Some(rect) = rect {
                mark_group_output_dirty(&mut entry.dirty, entry.size, &entry.signature, rect);
            } else {
                entry.dirty = CompositeDirtyRegion::full();
            }
        }
        if self
            .prefix_checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.surfaces.contains(&surface))
            && let Some(checkpoint) = self.prefix_checkpoint.as_mut()
        {
            checkpoint.valid = false;
        }
    }
}

fn mark_group_output_dirty(
    dirty: &mut CompositeDirtyRegion,
    texture_size: [u32; 2],
    nodes: &[CompositeNode],
    rect: RectU32,
) {
    match group_damage_dependency(nodes) {
        GroupDamageDependency::Local => {
            dirty.mark_rect(texture_size, rect);
        }
        GroupDamageDependency::SingleUvMirror(mirror) => {
            dirty.mark_rect(texture_size, rect);
            if let Some(mirrored) = mirror_rect_for_uv_mirror(texture_size, rect, mirror) {
                dirty.mark_rect(texture_size, mirrored);
            }
        }
        GroupDamageDependency::FullFallback => {
            *dirty = CompositeDirtyRegion::full();
        }
    }
}

#[derive(Default)]
pub struct CompositeCache {
    materials: Vec<CompositeTexture>,
    active_hints: Vec<Option<ActiveCompositeHint>>,
    active_run_checkpoints: Vec<Option<ActiveRunCheckpoints>>,
    active_run_source_caches: Vec<Option<ActiveRunSourceCache>>,
    general_material_caches: Vec<GeneralCompositeMaterialCache>,
    metrics: RenderMetrics,
    durable_error_metrics: RefCell<RenderMetrics>,
}

impl CompositeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure_material(
        &mut self,
        device: &wgpu::Device,
        material_index: usize,
        size: [u32; 2],
    ) {
        if material_index >= self.materials.len() {
            self.materials.resize_with(material_index + 1, || {
                let (texture, view) =
                    create_composite_output_texture(device, [1, 1], "composite_cache");
                CompositeTexture {
                    size: [1, 1],
                    texture: Some(texture),
                    view: Some(view),
                    dirty: CompositeDirtyRegion::full(),
                    signature: None,
                }
            });
        }

        if material_index >= self.active_hints.len() {
            self.active_hints.resize(material_index + 1, None);
        }
        if material_index >= self.active_run_checkpoints.len() {
            self.active_run_checkpoints
                .resize_with(material_index + 1, || None);
        }
        if material_index >= self.active_run_source_caches.len() {
            self.active_run_source_caches
                .resize_with(material_index + 1, || None);
        }
        if material_index >= self.general_material_caches.len() {
            self.general_material_caches
                .resize_with(material_index + 1, GeneralCompositeMaterialCache::default);
        }

        if self.materials[material_index].size == size {
            return;
        }

        let (texture, view) = create_composite_output_texture(device, size, "composite_cache");
        self.materials[material_index] = CompositeTexture {
            size,
            texture: Some(texture),
            view: Some(view),
            dirty: CompositeDirtyRegion::full(),
            signature: None,
        };
        self.invalidate_active_run_checkpoints(material_index);
        self.clear_general_material_cache(material_index);
    }

    pub fn texture_view(&self, material_index: usize) -> Option<&wgpu::TextureView> {
        self.materials
            .get(material_index)
            .and_then(|entry| entry.view.as_ref())
    }

    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    pub fn retain_material_count(&mut self, material_count: usize) {
        self.materials.truncate(material_count);
        self.active_hints.truncate(material_count);
        self.active_run_checkpoints.truncate(material_count);
        self.active_run_source_caches.truncate(material_count);
        self.general_material_caches.truncate(material_count);
    }

    pub fn texture(&self, material_index: usize) -> Option<&wgpu::Texture> {
        self.materials
            .get(material_index)
            .and_then(|entry| entry.texture.as_ref())
    }

    pub fn texture_size(&self, material_index: usize) -> Option<[u32; 2]> {
        self.materials.get(material_index).map(|entry| entry.size)
    }

    pub fn output_view(&self, material_index: usize) -> Option<&wgpu::TextureView> {
        self.texture_view(material_index)
    }

    pub(crate) fn general_composite_parts(
        &mut self,
        material_index: usize,
    ) -> Option<(
        &wgpu::TextureView,
        &mut GeneralCompositeMaterialCache,
        &mut RenderMetrics,
    )> {
        let Self {
            materials,
            general_material_caches,
            metrics,
            ..
        } = self;
        let output_view = materials.get(material_index)?.view.as_ref()?;
        let cache = general_material_caches.get_mut(material_index)?;
        Some((output_view, cache, metrics))
    }

    pub(crate) fn set_active_hint_for_surfaces(
        &mut self,
        surfaces: impl IntoIterator<Item = PaintSurfaceId>,
    ) {
        let Some(hint) = ActiveCompositeHint::from_surfaces(surfaces) else {
            return;
        };
        let material_index = hint.primary_surface().material_index().as_usize();
        if material_index >= self.active_hints.len() {
            self.active_hints.resize(material_index + 1, None);
        }
        self.active_hints[material_index] = Some(hint);
    }

    pub(crate) fn merge_active_hint_for_surfaces(
        &mut self,
        surfaces: impl IntoIterator<Item = PaintSurfaceId>,
    ) {
        let Some(incoming) = ActiveCompositeHint::from_surfaces(surfaces) else {
            return;
        };
        let material_index = incoming.primary_surface().material_index().as_usize();
        if material_index >= self.active_hints.len() {
            self.active_hints.resize(material_index + 1, None);
        }
        let Some(existing) = self.active_hints[material_index].as_mut() else {
            self.active_hints[material_index] = Some(incoming);
            return;
        };
        for surface in incoming.surfaces {
            if surface.material_index().as_usize() == material_index
                && !existing.surfaces.contains(&surface)
            {
                existing.surfaces.push(surface);
            }
        }
    }

    pub(crate) fn mark_preview_dirty(&mut self, surface: PaintSurfaceId) {
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
        if let Some(entry) = self.materials.get_mut(surface.material_index().as_usize()) {
            entry.dirty = CompositeDirtyRegion::full();
        }
        self.mark_general_surface_dirty(surface, None);
    }

    pub(crate) fn mark_preview_dirty_rect(
        &mut self,
        active_surface: PaintSurfaceId,
        material_index: usize,
        rect: RectU32,
    ) {
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        let dirty_surface = PaintSurfaceId {
            material_index: material_index.into(),
            ..active_surface
        };
        self.mark_general_surface_dirty(dirty_surface, Some(rect));
        let Some(entry) = self.materials.get_mut(material_index) else {
            return;
        };
        if rect.origin == [0, 0] && rect.size == entry.size {
            entry.dirty = CompositeDirtyRegion::full();
            self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
            return;
        }
        let Some(update) = entry.dirty.mark_rect(entry.size, rect) else {
            return;
        };
        self.metrics.partial_composite_count =
            self.metrics.partial_composite_count.saturating_add(1);
        self.metrics.dirty_rect_count = self.metrics.dirty_rect_count.saturating_add(1);
        self.metrics.dirty_tile_count = self
            .metrics
            .dirty_tile_count
            .saturating_add(update.tiles_touched);
    }

    pub(crate) fn active_hint(&self, material_index: usize) -> Option<ActiveCompositeHint> {
        self.active_hints
            .get(material_index)
            .and_then(|hint| hint.clone())
    }

    pub fn mark_dirty(&mut self, material_index: usize) {
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
        if let Some(entry) = self.materials.get_mut(material_index) {
            entry.dirty = CompositeDirtyRegion::full();
        }
        self.clear_active_hint(material_index);
        self.invalidate_active_run_checkpoints(material_index);
        self.clear_general_material_cache(material_index);
    }

    pub fn mark_layer_value_dirty(&mut self, material_index: usize) {
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
        if let Some(entry) = self.materials.get_mut(material_index) {
            entry.dirty = CompositeDirtyRegion::full();
        }
        self.clear_active_hint(material_index);
        self.invalidate_active_run_checkpoints(material_index);
    }

    pub fn mark_layer_value_dirty_rect(&mut self, material_index: usize, rect: RectU32) {
        let Some(entry) = self.materials.get(material_index) else {
            return;
        };
        if rect.origin == [0, 0] && rect.size == entry.size {
            self.mark_layer_value_dirty(material_index);
            return;
        }
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.clear_active_hint(material_index);
        self.invalidate_active_run_checkpoints(material_index);
        let Some(entry) = self.materials.get_mut(material_index) else {
            return;
        };
        let Some(update) = entry.dirty.mark_rect(entry.size, rect) else {
            return;
        };
        self.metrics.partial_composite_count =
            self.metrics.partial_composite_count.saturating_add(1);
        self.metrics.dirty_rect_count = self.metrics.dirty_rect_count.saturating_add(1);
        self.metrics.dirty_tile_count = self
            .metrics
            .dirty_tile_count
            .saturating_add(update.tiles_touched);
    }

    pub fn mark_surface_dirty(&mut self, surface: PaintSurfaceId) {
        let material_index = surface.material_index().as_usize();
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
        let preserve_checkpoints = self.surface_preserves_active_run(material_index, surface);
        if let Some(entry) = self.materials.get_mut(material_index) {
            entry.dirty = CompositeDirtyRegion::full();
        }
        if preserve_checkpoints {
            self.clear_active_run_source_cache_if_source(material_index, surface);
        } else {
            self.invalidate_active_run_checkpoints(material_index);
        }
        self.mark_general_surface_dirty(surface, None);
    }

    pub fn mark_surface_dirty_rect(&mut self, surface: PaintSurfaceId, rect: RectU32) {
        let material_index = surface.material_index().as_usize();
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        let preserve_checkpoints = self.surface_preserves_active_run(material_index, surface);
        if preserve_checkpoints {
            self.clear_active_run_source_cache_if_source(material_index, surface);
        } else {
            self.invalidate_active_run_checkpoints(material_index);
        }
        self.mark_general_surface_dirty(surface, Some(rect));
        let Some(entry) = self.materials.get_mut(material_index) else {
            return;
        };
        let Some(update) = entry.dirty.mark_rect(entry.size, rect) else {
            return;
        };
        self.metrics.partial_composite_count =
            self.metrics.partial_composite_count.saturating_add(1);
        self.metrics.dirty_rect_count = self.metrics.dirty_rect_count.saturating_add(1);
        self.metrics.dirty_tile_count = self
            .metrics
            .dirty_tile_count
            .saturating_add(update.tiles_touched);
    }

    pub fn mark_all_dirty(&mut self) {
        self.metrics.composite_mark_dirty_calls = self
            .metrics
            .composite_mark_dirty_calls
            .saturating_add(self.materials.len());
        self.metrics.full_composite_count = self
            .metrics
            .full_composite_count
            .saturating_add(self.materials.len());
        for entry in &mut self.materials {
            entry.dirty = CompositeDirtyRegion::full();
        }
        for checkpoint in &mut self.active_run_checkpoints {
            *checkpoint = None;
        }
        for source_cache in &mut self.active_run_source_caches {
            *source_cache = None;
        }
        for active_hint in &mut self.active_hints {
            *active_hint = None;
        }
        for cache in &mut self.general_material_caches {
            *cache = GeneralCompositeMaterialCache::default();
        }
    }

    pub fn composite_need(&self, material_index: usize, tree: &CompositeTree) -> CompositeNeed {
        let Some(entry) = self.materials.get(material_index) else {
            return CompositeNeed::FullDirty;
        };
        if entry
            .signature
            .as_ref()
            .is_some_and(|signature| signature != tree)
        {
            return CompositeNeed::SignatureChanged;
        }
        if entry.dirty.full {
            return CompositeNeed::FullDirty;
        }
        if !entry.dirty.rects.is_empty() || !entry.dirty.tiles.is_empty() {
            return CompositeNeed::PartialDirty;
        }
        if entry.signature.is_none() {
            return CompositeNeed::FullDirty;
        }
        CompositeNeed::Clean
    }

    pub fn record_signature_changed_invalidation(&mut self, material_index: usize) {
        // Structural tree mutations clear the general cache at mutation-invalidation time.
        // Layer-value patches intentionally keep it here: each isolated-group and root-prefix
        // cache validates its own node signature and only rebuilds the affected checkpoint.
        if self
            .materials
            .get(material_index)
            .is_some_and(|entry| entry.dirty.full)
        {
            return;
        }
        self.metrics.composite_mark_dirty_calls =
            self.metrics.composite_mark_dirty_calls.saturating_add(1);
        self.metrics.full_composite_count = self.metrics.full_composite_count.saturating_add(1);
    }

    pub fn mark_clean(&mut self, material_index: usize, tree: &CompositeTree) {
        if let Some(entry) = self.materials.get_mut(material_index) {
            entry.dirty = CompositeDirtyRegion::default();
            entry.signature = Some(tree.clone());
        }
    }

    pub fn dirty_state(&self, material_index: usize) -> CompositeDirtyRegion {
        self.materials
            .get(material_index)
            .map(|entry| entry.dirty.clone())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn ensure_test_material_state(&mut self, material_index: usize, size: [u32; 2]) {
        if material_index >= self.materials.len() {
            self.materials
                .resize_with(material_index + 1, || CompositeTexture {
                    size: [1, 1],
                    texture: None,
                    view: None,
                    dirty: CompositeDirtyRegion::default(),
                    signature: None,
                });
        }
        if material_index >= self.general_material_caches.len() {
            self.general_material_caches
                .resize_with(material_index + 1, GeneralCompositeMaterialCache::default);
        }
        self.materials[material_index].size = size;
        self.materials[material_index].dirty = CompositeDirtyRegion::default();
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        for material in &self.materials {
            if material.texture.is_some() {
                let bytes = rgba8_texture_bytes(material.size);
                metrics.add_total_bytes(bytes);
                metrics.composite_bytes = metrics.composite_bytes.saturating_add(bytes);
                metrics.composite_texture_count = metrics.composite_texture_count.saturating_add(1);
            }
        }
        for checkpoints in self.active_run_checkpoints.iter().flatten() {
            for boundary in &checkpoints.boundaries {
                let below_bytes = rgba8_texture_bytes(checkpoints.size);
                metrics.add_total_bytes(below_bytes);
                metrics.checkpoint_bytes = metrics.checkpoint_bytes.saturating_add(below_bytes);
                metrics.checkpoint_texture_count =
                    metrics.checkpoint_texture_count.saturating_add(1);
                for _ in &boundary.above_steps {
                    let bytes = rgba8_texture_bytes(checkpoints.size);
                    metrics.add_total_bytes(bytes);
                    metrics.checkpoint_bytes = metrics.checkpoint_bytes.saturating_add(bytes);
                    metrics.checkpoint_texture_count =
                        metrics.checkpoint_texture_count.saturating_add(1);
                }
            }
        }
        for cache in &self.general_material_caches {
            for entry in cache.isolated_groups.values() {
                let bytes = rgba8_texture_bytes(entry.size);
                metrics.add_total_bytes(bytes);
                metrics.checkpoint_bytes = metrics.checkpoint_bytes.saturating_add(bytes);
                metrics.checkpoint_texture_count =
                    metrics.checkpoint_texture_count.saturating_add(1);
            }
            if let Some(checkpoint) = cache.prefix_checkpoint.as_ref() {
                let bytes = rgba8_texture_bytes(checkpoint.size);
                metrics.add_total_bytes(bytes);
                metrics.checkpoint_bytes = metrics.checkpoint_bytes.saturating_add(bytes);
                metrics.checkpoint_texture_count =
                    metrics.checkpoint_texture_count.saturating_add(1);
            }
        }
        for source_cache in self.active_run_source_caches.iter().flatten() {
            for entry in &source_cache.entries {
                let bytes = entry.format.saturating_byte_len(entry.size);
                metrics.add_total_bytes(bytes);
                metrics.checkpoint_bytes = metrics.checkpoint_bytes.saturating_add(bytes);
                metrics.checkpoint_texture_count =
                    metrics.checkpoint_texture_count.saturating_add(1);
            }
        }
        metrics
    }

    pub(crate) fn active_run_above_step_checkpoint_budget(&self) -> usize {
        ACTIVE_RUN_ABOVE_STEP_CHECKPOINT_BUDGET
    }

    pub(crate) fn clear_active_preview_state(&mut self, material_index: usize) {
        self.clear_active_hint(material_index);
        self.invalidate_active_run_checkpoints(material_index);
        self.clear_active_run_source_cache(material_index);
    }

    pub(crate) fn record_active_prime_plan_miss(&mut self) {
        self.metrics.active_composite_prime_plan_misses = self
            .metrics
            .active_composite_prime_plan_misses
            .saturating_add(1);
    }

    pub(crate) fn record_active_prime_checkpoint_budget_fallback(
        &mut self,
        _material_index: usize,
        _requested_above_steps: usize,
    ) {
        self.metrics.composite_checkpoint_budget_fallbacks = self
            .metrics
            .composite_checkpoint_budget_fallbacks
            .saturating_add(1);
        self.metrics
            .active_composite_prime_checkpoint_budget_fallbacks = self
            .metrics
            .active_composite_prime_checkpoint_budget_fallbacks
            .saturating_add(1);
    }

    pub(crate) fn record_stroke_preview_checkpoint_budget_fallback(
        &mut self,
        _material_index: usize,
        _requested_above_steps: usize,
    ) {
        self.metrics.composite_checkpoint_budget_fallbacks = self
            .metrics
            .composite_checkpoint_budget_fallbacks
            .saturating_add(1);
        self.metrics.stroke_composite_checkpoint_budget_fallbacks = self
            .metrics
            .stroke_composite_checkpoint_budget_fallbacks
            .saturating_add(1);
    }

    pub(crate) fn record_stroke_composite_plan_miss(&mut self) {
        self.metrics.stroke_composite_plan_misses =
            self.metrics.stroke_composite_plan_misses.saturating_add(1);
    }

    pub(crate) fn record_stroke_composite_upload_attempt(&self, bytes: usize) {
        let mut metrics = self.durable_error_metrics.borrow_mut();
        metrics.stroke_composite_upload_attempts =
            metrics.stroke_composite_upload_attempts.saturating_add(1);
        metrics.stroke_composite_upload_bytes =
            metrics.stroke_composite_upload_bytes.saturating_add(bytes);
    }

    pub(crate) fn active_run_source_cache_view(
        &self,
        material_index: usize,
        active_surfaces: &[PaintSurfaceId],
        source: PaintSurfaceId,
        size: [u32; 2],
    ) -> Option<&wgpu::TextureView> {
        let cache = self
            .active_run_source_caches
            .get(material_index)?
            .as_ref()?;
        if cache.active_surfaces != active_surfaces {
            return None;
        }
        cache
            .entries
            .iter()
            .find(|entry| entry.source == source && entry.size == size)
            .map(|entry| &entry.view)
    }

    pub(crate) fn active_run_source_cache_matches(
        &self,
        material_index: usize,
        active_surfaces: &[PaintSurfaceId],
        source: PaintSurfaceId,
        size: [u32; 2],
        format: SurfacePixelFormat,
    ) -> bool {
        self.active_run_source_caches
            .get(material_index)
            .and_then(Option::as_ref)
            .is_some_and(|cache| {
                cache.active_surfaces == active_surfaces
                    && cache.entries.iter().any(|entry| {
                        entry.source == source && entry.size == size && entry.format == format
                    })
            })
    }

    pub(crate) fn active_run_checkpoints_ready(
        &self,
        material_index: usize,
        size: [u32; 2],
        active_surfaces: &[PaintSurfaceId],
        tree: &CompositeTree,
        boundary_above_step_counts: &[usize],
    ) -> bool {
        self.active_run_checkpoints
            .get(material_index)
            .and_then(Option::as_ref)
            .is_some_and(|entry| {
                entry.valid
                    && entry.size == size
                    && entry.active_surfaces == active_surfaces
                    && entry.signature == *tree
                    && entry.boundaries.len() == boundary_above_step_counts.len()
                    && entry
                        .boundaries
                        .iter()
                        .zip(boundary_above_step_counts.iter())
                        .all(|(boundary, above_step_count)| {
                            boundary.above_steps.len() == *above_step_count
                        })
            })
    }

    pub(crate) fn replace_active_run_source_cache(
        &mut self,
        device: &wgpu::Device,
        material_index: usize,
        active_surfaces: &[PaintSurfaceId],
        source: PaintSurfaceId,
        size: [u32; 2],
        format: SurfacePixelFormat,
    ) -> &wgpu::Texture {
        if material_index >= self.active_run_source_caches.len() {
            self.active_run_source_caches
                .resize_with(material_index + 1, || None);
        }
        let reset_cache = self.active_run_source_caches[material_index]
            .as_ref()
            .map_or(true, |cache| cache.active_surfaces != active_surfaces);
        if reset_cache {
            self.active_run_source_caches[material_index] = Some(ActiveRunSourceCache {
                active_surfaces: active_surfaces.to_vec(),
                entries: Vec::new(),
            });
        }
        let cache = self.active_run_source_caches[material_index]
            .as_mut()
            .expect("source cache exists");
        if let Some(index) = cache
            .entries
            .iter()
            .position(|entry| entry.source == source)
        {
            cache.entries.remove(index);
        }
        let (texture, view) = match format {
            SurfacePixelFormat::Rgba8 => {
                create_paint_texture(device, size, "active_run_source_cache_rgba8")
            }
            SurfacePixelFormat::R8 => {
                create_mask_texture(device, size, "active_run_source_cache_r8")
            }
        };
        cache.entries.push(ActiveRunSourceCacheEntry {
            source,
            size,
            format,
            _texture: texture,
            view,
        });
        &cache
            .entries
            .last()
            .expect("source cache entry was just stored")
            ._texture
    }

    pub(crate) fn active_run_source_cache_contains_source(
        &self,
        material_index: usize,
        source: PaintSurfaceId,
    ) -> bool {
        self.active_run_source_caches
            .get(material_index)
            .and_then(Option::as_ref)
            .is_some_and(|cache| cache.entries.iter().any(|entry| entry.source == source))
    }

    pub(crate) fn retain_active_run_source_cache_sources(
        &mut self,
        material_index: usize,
        active_surfaces: &[PaintSurfaceId],
        sources: &[PaintSurfaceId],
    ) {
        let Some(cache) = self
            .active_run_source_caches
            .get_mut(material_index)
            .and_then(Option::as_mut)
        else {
            return;
        };
        if cache.active_surfaces != active_surfaces {
            self.clear_active_run_source_cache(material_index);
            return;
        }
        cache
            .entries
            .retain(|entry| sources.contains(&entry.source));
        if cache.entries.is_empty() {
            self.clear_active_run_source_cache(material_index);
        }
    }

    pub(crate) fn clear_active_run_source_cache(&mut self, material_index: usize) {
        if let Some(entry) = self.active_run_source_caches.get_mut(material_index) {
            *entry = None;
        }
    }

    pub(crate) fn ensure_active_run_checkpoints(
        &mut self,
        device: &wgpu::Device,
        material_index: usize,
        size: [u32; 2],
        active_surfaces: &[PaintSurfaceId],
        tree: &CompositeTree,
        boundary_above_step_counts: &[usize],
    ) -> bool {
        if material_index >= self.active_run_checkpoints.len() {
            self.active_run_checkpoints
                .resize_with(material_index + 1, || None);
        }
        if material_index >= self.active_run_source_caches.len() {
            self.active_run_source_caches
                .resize_with(material_index + 1, || None);
        }

        let rebuild = self.active_run_checkpoints[material_index]
            .as_ref()
            .map_or(true, |entry| {
                entry.size != size
                    || entry.active_surfaces != active_surfaces
                    || entry.signature != *tree
                    || entry.boundaries.len() != boundary_above_step_counts.len()
                    || entry
                        .boundaries
                        .iter()
                        .zip(boundary_above_step_counts.iter())
                        .any(|(boundary, above_step_count)| {
                            boundary.above_steps.len() != *above_step_count
                        })
            });
        if rebuild {
            self.metrics.composite_checkpoint_rebuilds =
                self.metrics.composite_checkpoint_rebuilds.saturating_add(1);
            let boundaries = boundary_above_step_counts
                .iter()
                .enumerate()
                .map(|(boundary_index, above_step_count)| {
                    let (below_texture, below_view) = create_render_scratch_texture(
                        device,
                        size,
                        &format!("active_run_boundary_{boundary_index}_below_checkpoint"),
                    );
                    let above_steps = (0..*above_step_count)
                        .map(|step_index| {
                            let (texture, view) = create_render_scratch_texture(
                                device,
                                size,
                                &format!(
                                    "active_run_boundary_{boundary_index}_above_step_{step_index}_checkpoint"
                                ),
                            );
                            ActiveRunStepCache {
                                _texture: texture,
                                view,
                            }
                        })
                        .collect();
                    ActiveRunBoundaryCache {
                        _below_texture: below_texture,
                        below_view,
                        above_steps,
                    }
                })
                .collect();
            self.active_run_checkpoints[material_index] = Some(ActiveRunCheckpoints {
                size,
                active_surfaces: active_surfaces.to_vec(),
                signature: tree.clone(),
                boundaries,
                valid: false,
            });
        }

        let needs_rebuild = self.active_run_checkpoints[material_index]
            .as_ref()
            .map_or(true, |entry| !entry.valid);
        if !needs_rebuild {
            self.metrics.composite_checkpoint_reuses =
                self.metrics.composite_checkpoint_reuses.saturating_add(1);
        }
        needs_rebuild
    }

    pub(crate) fn active_run_checkpoint_views(
        &self,
        material_index: usize,
    ) -> Option<ActiveRunCheckpointViews<'_>> {
        let entry = self.active_run_checkpoints.get(material_index)?.as_ref()?;
        Some(ActiveRunCheckpointViews {
            boundaries: entry
                .boundaries
                .iter()
                .map(|boundary| ActiveRunBoundaryCheckpointViews {
                    below: &boundary.below_view,
                    above_steps: boundary.above_steps.iter().map(|step| &step.view).collect(),
                })
                .collect(),
        })
    }

    pub(crate) fn mark_active_run_checkpoints_valid(&mut self, material_index: usize) {
        if let Some(Some(entry)) = self.active_run_checkpoints.get_mut(material_index) {
            entry.valid = true;
        }
    }

    fn invalidate_active_run_checkpoints(&mut self, material_index: usize) {
        if let Some(entry) = self.active_run_checkpoints.get_mut(material_index) {
            *entry = None;
        }
        self.clear_active_run_source_cache(material_index);
    }

    fn clear_active_hint(&mut self, material_index: usize) {
        if let Some(hint) = self.active_hints.get_mut(material_index) {
            *hint = None;
        }
    }

    fn surface_preserves_active_run(&self, material_index: usize, surface: PaintSurfaceId) -> bool {
        self.active_hints
            .get(material_index)
            .and_then(Option::as_ref)
            .is_some_and(|hint| hint.contains(surface))
            || self
                .active_run_checkpoints
                .get(material_index)
                .and_then(Option::as_ref)
                .is_some_and(|checkpoints| checkpoints.active_surfaces.contains(&surface))
            || self.active_run_source_cache_contains_source(material_index, surface)
    }

    fn clear_active_run_source_cache_if_source(
        &mut self,
        material_index: usize,
        source: PaintSurfaceId,
    ) {
        let Some(cache) = self
            .active_run_source_caches
            .get_mut(material_index)
            .and_then(Option::as_mut)
        else {
            return;
        };
        cache.entries.retain(|entry| entry.source != source);
        if cache.entries.is_empty() {
            self.clear_active_run_source_cache(material_index);
        }
    }

    fn mark_general_surface_dirty(&mut self, surface: PaintSurfaceId, rect: Option<RectU32>) {
        if let Some(cache) = self
            .general_material_caches
            .get_mut(surface.material_index().as_usize())
        {
            cache.mark_surface_dirty(surface, rect);
        }
    }

    fn clear_general_material_cache(&mut self, material_index: usize) {
        if let Some(cache) = self.general_material_caches.get_mut(material_index) {
            *cache = GeneralCompositeMaterialCache::default();
        }
    }

    pub fn take_metrics(&mut self) -> RenderMetrics {
        let mut metrics = std::mem::take(&mut self.metrics);
        metrics.merge(std::mem::take(
            &mut *self.durable_error_metrics.borrow_mut(),
        ));
        metrics
    }
}

fn surfaces_from_nodes(nodes: &[CompositeNode]) -> Vec<PaintSurfaceId> {
    let mut surfaces = Vec::new();
    for node in nodes {
        collect_node_surfaces(node, &mut surfaces);
    }
    surfaces
}

fn collect_node_surfaces(node: &CompositeNode, surfaces: &mut Vec<PaintSurfaceId>) {
    match node {
        CompositeNode::Raster { surface, mask, .. } => {
            push_unique_surface(surfaces, *surface);
            if let Some(mask) = mask {
                push_unique_surface(surfaces, *mask);
            }
        }
        CompositeNode::SolidFill { mask, .. } | CompositeNode::Adjustment { mask, .. } => {
            if let Some(mask) = mask {
                push_unique_surface(surfaces, *mask);
            }
        }
        CompositeNode::EmbeddedImage { mask, .. } => {
            if let Some(mask) = mask {
                push_unique_surface(surfaces, *mask);
            }
        }
        CompositeNode::Group(group) => {
            if let Some(mask) = group.mask {
                push_unique_surface(surfaces, mask);
            }
            for child in &group.children {
                collect_node_surfaces(child, surfaces);
            }
        }
    }
}

fn push_unique_surface(surfaces: &mut Vec<PaintSurfaceId>, surface: PaintSurfaceId) {
    if !surfaces.contains(&surface) {
        surfaces.push(surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        adjustment::{Adjustment, UvMirrorAdjustment},
        composite::{GroupCompositeMode, LayerBlendMode},
        surface::{CompositeProps, LayerId},
    };
    use slotmap::SlotMap;

    fn test_surface(material_index: usize) -> PaintSurfaceId {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        PaintSurfaceId::raster(material_index.into(), layers.insert(()))
    }

    fn test_surfaces(material_index: usize, count: usize) -> Vec<PaintSurfaceId> {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        (0..count)
            .map(|_| PaintSurfaceId::raster(material_index.into(), layers.insert(())))
            .collect()
    }

    fn assert_hint_surfaces(
        cache: &CompositeCache,
        material_index: usize,
        expected: &[PaintSurfaceId],
    ) {
        let hint = cache
            .active_hint(material_index)
            .expect("active hint should exist");
        assert_eq!(hint.surfaces().len(), expected.len());
        for surface in expected {
            assert!(
                hint.surfaces().contains(surface),
                "hint surfaces {:?} should contain {:?}",
                hint.surfaces(),
                surface
            );
        }
    }

    fn test_props() -> CompositeProps {
        CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        }
    }

    fn test_uv_mirror(layer_id: LayerId) -> CompositeNode {
        CompositeNode::Adjustment {
            layer_id,
            adjustment: Adjustment::UvMirror(UvMirrorAdjustment::default()),
            mask: None,
            props: test_props(),
        }
    }

    #[test]
    fn mark_dirty_clears_active_hint() {
        let mut cache = CompositeCache::new();
        let surface = test_surface(1);
        cache.set_active_hint_for_surfaces([surface]);

        assert!(cache.active_hint(1).is_some());
        cache.mark_dirty(1);

        assert!(cache.active_hint(1).is_none());
    }

    #[test]
    fn mark_all_dirty_clears_active_hints() {
        let mut cache = CompositeCache::new();
        let first = test_surface(0);
        let second = test_surface(2);
        cache.set_active_hint_for_surfaces([first]);
        cache.set_active_hint_for_surfaces([second]);

        assert!(cache.active_hint(0).is_some());
        assert!(cache.active_hint(2).is_some());
        cache.mark_all_dirty();

        assert!(cache.active_hint(0).is_none());
        assert!(cache.active_hint(2).is_none());
    }

    #[test]
    fn clear_active_preview_state_clears_active_hint() {
        let mut cache = CompositeCache::new();
        let surface = test_surface(1);
        cache.set_active_hint_for_surfaces([surface]);

        cache.clear_active_preview_state(1);

        assert!(cache.active_hint(1).is_none());
    }

    #[test]
    fn merge_active_hint_adds_surface_without_removing_existing() {
        let mut cache = CompositeCache::new();
        let surfaces = test_surfaces(0, 2);
        cache.set_active_hint_for_surfaces([surfaces[0]]);

        cache.merge_active_hint_for_surfaces([surfaces[1]]);

        assert_hint_surfaces(&cache, 0, &surfaces);
    }

    #[test]
    fn merge_active_hint_does_not_shrink_existing_hint() {
        let mut cache = CompositeCache::new();
        let surfaces = test_surfaces(0, 2);
        cache.set_active_hint_for_surfaces(surfaces.iter().copied());

        cache.merge_active_hint_for_surfaces([surfaces[1]]);

        assert_hint_surfaces(&cache, 0, &surfaces);
    }

    #[test]
    fn merge_active_hint_seeds_empty_material_hint() {
        let mut cache = CompositeCache::new();
        let surface = test_surface(2);

        cache.merge_active_hint_for_surfaces([surface]);

        assert_hint_surfaces(&cache, 2, &[surface]);
    }

    #[test]
    fn general_cache_surface_collection_includes_nested_masks_without_duplicates() {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        let raster_layer = layers.insert(());
        let group_layer = layers.insert(());
        let fill_layer = layers.insert(());
        let raster = PaintSurfaceId::raster(0.into(), raster_layer);
        let raster_mask = PaintSurfaceId::layer_mask(0.into(), raster_layer);
        let group_mask = PaintSurfaceId::layer_mask(0.into(), group_layer);
        let fill_mask = PaintSurfaceId::layer_mask(0.into(), fill_layer);
        let props = CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        };
        let nodes = vec![CompositeNode::Group(CompositeGroup {
            layer_id: Some(group_layer),
            mode: GroupCompositeMode::Isolated,
            props,
            mask: Some(group_mask),
            children: vec![
                CompositeNode::Raster {
                    surface: raster,
                    mask: Some(raster_mask),
                    props,
                },
                CompositeNode::Raster {
                    surface: raster,
                    mask: None,
                    props,
                },
                CompositeNode::SolidFill {
                    layer_id: fill_layer,
                    color: [0.25, 0.5, 0.75],
                    mask: Some(fill_mask),
                    props,
                },
            ],
        })];

        let surfaces = surfaces_from_nodes(&nodes);

        assert_eq!(surfaces, vec![group_mask, raster, raster_mask, fill_mask]);
    }

    #[test]
    fn isolated_group_dirty_expands_single_uv_mirror_dependency() {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        let mirror = test_uv_mirror(layers.insert(()));
        let source = RectU32 {
            origin: [400, 100],
            size: [20, 30],
        };
        let reflected = RectU32 {
            origin: [92, 100],
            size: [20, 30],
        };
        let mut dirty = CompositeDirtyRegion::default();

        mark_group_output_dirty(&mut dirty, [512, 512], &[mirror], source);

        assert!(!dirty.full);
        assert!(dirty.rects.contains(&source));
        assert!(dirty.rects.contains(&reflected));
    }

    #[test]
    fn isolated_group_dirty_falls_back_to_full_for_multiple_uv_mirrors() {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        let mirrors = [
            test_uv_mirror(layers.insert(())),
            test_uv_mirror(layers.insert(())),
        ];
        let mut dirty = CompositeDirtyRegion::default();

        mark_group_output_dirty(
            &mut dirty,
            [512, 512],
            &mirrors,
            RectU32 {
                origin: [400, 100],
                size: [20, 30],
            },
        );

        assert!(dirty.full);
    }

    #[test]
    fn isolated_group_dirty_falls_back_to_full_for_nested_uv_mirror() {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        let nested = CompositeNode::Group(CompositeGroup {
            layer_id: Some(layers.insert(())),
            mode: GroupCompositeMode::Isolated,
            props: test_props(),
            mask: None,
            children: vec![test_uv_mirror(layers.insert(()))],
        });
        let mut dirty = CompositeDirtyRegion::default();

        mark_group_output_dirty(
            &mut dirty,
            [512, 512],
            &[nested],
            RectU32 {
                origin: [400, 100],
                size: [20, 30],
            },
        );

        assert!(dirty.full);
    }

    #[test]
    fn no_upload_violation_metrics_survive_take_metrics() {
        let mut cache = CompositeCache::new();
        cache.record_stroke_composite_upload_attempt(64);

        let metrics = cache.take_metrics();

        assert_eq!(metrics.stroke_composite_upload_attempts, 1);
        assert_eq!(metrics.stroke_composite_upload_bytes, 64);
    }

    #[test]
    fn prime_and_preview_checkpoint_budget_metrics_are_split() {
        let mut cache = CompositeCache::new();

        cache.record_active_prime_checkpoint_budget_fallback(0, 9);
        let metrics = cache.take_metrics();
        assert_eq!(metrics.composite_checkpoint_budget_fallbacks, 1);
        assert_eq!(
            metrics.active_composite_prime_checkpoint_budget_fallbacks,
            1
        );
        assert_eq!(metrics.stroke_composite_checkpoint_budget_fallbacks, 0);

        cache.record_stroke_preview_checkpoint_budget_fallback(0, 9);
        let metrics = cache.take_metrics();
        assert_eq!(metrics.composite_checkpoint_budget_fallbacks, 1);
        assert_eq!(
            metrics.active_composite_prime_checkpoint_budget_fallbacks,
            0
        );
        assert_eq!(metrics.stroke_composite_checkpoint_budget_fallbacks, 1);
    }
}

fn rgba8_texture_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize)
        .saturating_mul(size[1] as usize)
        .saturating_mul(4)
}
