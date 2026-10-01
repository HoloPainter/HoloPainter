//! GPU-side document state.
//!
//! This module groups state that belongs to the loaded painting document:
//! scene geometry, surface textures, and selection masks. Render passes access
//! these resources through document-shaped boundaries instead of unrelated
//! renderer state fields.

pub(crate) mod composites;
pub(crate) mod embedded_images;
pub(crate) mod materials;
pub(crate) mod scene;
pub(crate) mod selection;
pub(crate) mod surfaces;
pub(crate) mod tile_shadow;

use self::composites::{CompositeMutationContext, CompositeRegistry};
use self::embedded_images::EmbeddedImageRegistry;
use self::materials::MaterialRegistry;
use self::scene::SceneStore;
use self::selection::SelectionMasks;
use self::surfaces::SurfaceRepository;
use crate::{
    core::{
        adjustment::Adjustment,
        surface::{CompositeProps, CompositeTree, LayerId},
    },
    renderer::mutation::MutationLog,
};
use eframe::egui_wgpu::wgpu;

pub(crate) struct GpuDocument {
    pub(crate) composites: CompositeRegistry,
    pub(crate) embedded_images: EmbeddedImageRegistry,
    pub(crate) materials: MaterialRegistry,
    pub(crate) scene: SceneStore,
    pub(crate) surfaces: SurfaceRepository,
    pub(crate) selections: SelectionMasks,
}

impl GpuDocument {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            composites: CompositeRegistry::default(),
            embedded_images: EmbeddedImageRegistry::default(),
            materials: MaterialRegistry::default(),
            scene: SceneStore::new(device),
            surfaces: SurfaceRepository::new(),
            selections: SelectionMasks::new(),
        }
    }

    pub(crate) fn sync_material_tree(
        &mut self,
        material_index: usize,
        tree: CompositeTree,
    ) -> MutationLog {
        let mut mutations = MutationLog::default();
        CompositeMutationContext::new(&mut self.composites, &mut mutations)
            .sync_material_tree(material_index, tree);
        mutations
    }

    pub(crate) fn update_composite_layer_props(
        &mut self,
        layer_id: LayerId,
        props: CompositeProps,
    ) -> MutationLog {
        let mut mutations = MutationLog::default();
        CompositeMutationContext::new(&mut self.composites, &mut mutations)
            .update_layer_props(layer_id, props);
        mutations
    }

    pub(crate) fn update_composite_adjustment(
        &mut self,
        layer_id: LayerId,
        adjustment: Adjustment,
    ) -> MutationLog {
        let mut mutations = MutationLog::default();
        CompositeMutationContext::new(&mut self.composites, &mut mutations)
            .update_adjustment(layer_id, adjustment);
        mutations
    }
}
