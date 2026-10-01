use eframe::egui_wgpu::wgpu;

use crate::core::render_report::GpuTextureMetrics;

use super::{pipelines::ViewPipelines, resources::DecalTextureResources, targets::ViewTargets};

/// Target vertical boundary for view renderer work.
pub(crate) struct ViewFeature {
    pub(super) pipelines: ViewPipelines,
    pub(super) targets: ViewTargets,
    pub(super) decal_resources: DecalTextureResources,
}

impl ViewFeature {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: ViewPipelines::new(device),
            targets: ViewTargets::new(device),
            decal_resources: DecalTextureResources::new(device),
        }
    }

    pub(crate) fn viewport_texture_view(&self) -> &wgpu::TextureView {
        self.targets.viewport_texture_view()
    }

    pub(crate) fn uv_view_texture_view(&self) -> &wgpu::TextureView {
        self.targets.uv_view_texture_view()
    }

    pub(crate) fn viewport_texture(&self) -> &wgpu::Texture {
        &self.targets.viewport_color
    }

    pub(crate) fn viewport_size(&self) -> [u32; 2] {
        self.targets.viewport_size
    }

    pub(crate) fn uv_view_texture(&self) -> &wgpu::Texture {
        &self.targets.uv_view_output
    }

    pub(crate) fn uv_view_size(&self) -> [u32; 2] {
        self.targets.uv_view_size
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        self.targets.texture_metrics()
    }
}
