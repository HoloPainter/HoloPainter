use eframe::egui_wgpu::wgpu;

use super::cache::CompositeCache;

/// Read-only composite outputs exposed to features that only need prepared
/// material textures for presentation or view rendering.
#[derive(Clone, Copy)]
pub(crate) struct CompositeOutputViews<'a> {
    cache: &'a CompositeCache,
}

impl<'a> CompositeOutputViews<'a> {
    pub(super) fn new(cache: &'a CompositeCache) -> Self {
        Self { cache }
    }

    pub(crate) fn texture_view(&self, material_index: usize) -> Option<&'a wgpu::TextureView> {
        self.cache.texture_view(material_index)
    }

    pub(crate) fn output_view(&self, material_index: usize) -> Option<&'a wgpu::TextureView> {
        self.texture_view(material_index)
    }

    pub(crate) fn material_count(&self) -> usize {
        self.cache.material_count()
    }
}

/// Narrow read-only view of composite resources needed by view rendering.
#[derive(Clone, Copy)]
pub(crate) struct CompositeViewResources<'a> {
    outputs: CompositeOutputViews<'a>,
}

impl<'a> CompositeViewResources<'a> {
    pub(super) fn new(cache: &'a CompositeCache) -> Self {
        Self {
            outputs: CompositeOutputViews::new(cache),
        }
    }

    pub(crate) fn outputs(&self) -> CompositeOutputViews<'a> {
        self.outputs
    }
}
