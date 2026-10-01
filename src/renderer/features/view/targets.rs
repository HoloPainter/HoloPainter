use eframe::egui_wgpu::wgpu;

use crate::core::render_report::GpuTextureMetrics;

use crate::renderer::gpu::texture::{create_color_target, create_render_scratch_texture};

pub(crate) struct ViewTargets {
    pub(crate) viewport_size: [u32; 2],
    pub(crate) viewport_color: wgpu::Texture,
    pub(crate) viewport_color_view: wgpu::TextureView,

    pub(crate) uv_view_size: [u32; 2],
    pub(crate) uv_view_output: wgpu::Texture,
    pub(crate) uv_view_output_view: wgpu::TextureView,
}

impl ViewTargets {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let viewport_size = [512, 512];
        let (viewport_color, viewport_color_view) =
            create_color_target(device, viewport_size, "viewport_presentation_color");

        let uv_view_size = [1, 1];
        let (uv_view_output, uv_view_output_view) =
            create_render_scratch_texture(device, uv_view_size, "uv_view_output");

        Self {
            viewport_size,
            viewport_color,
            viewport_color_view,
            uv_view_size,
            uv_view_output,
            uv_view_output_view,
        }
    }

    pub(crate) fn viewport_texture_view(&self) -> &wgpu::TextureView {
        &self.viewport_color_view
    }

    pub(crate) fn uv_view_texture_view(&self) -> &wgpu::TextureView {
        &self.uv_view_output_view
    }

    pub(crate) fn ensure_viewport_size(
        &mut self,
        device: &wgpu::Device,
        viewport: [u32; 2],
    ) -> bool {
        if viewport == self.viewport_size {
            return false;
        }

        self.viewport_size = viewport;
        let (vc, vcv) = create_color_target(device, viewport, "viewport_presentation_color");
        self.viewport_color = vc;
        self.viewport_color_view = vcv;
        true
    }

    pub(crate) fn ensure_uv_view_size(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let desired = [size[0].max(1), size[1].max(1)];
        if desired == self.uv_view_size {
            return false;
        }

        self.uv_view_size = desired;
        let (texture, view) = create_render_scratch_texture(device, desired, "uv_view_output");
        self.uv_view_output = texture;
        self.uv_view_output_view = view;
        true
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let viewport_bytes = rgba8_texture_bytes(self.viewport_size);
        let uv_view_bytes = rgba8_texture_bytes(self.uv_view_size);
        let mut metrics = GpuTextureMetrics::default();
        metrics.add_total_bytes(viewport_bytes.saturating_add(uv_view_bytes));
        metrics.view_bytes = viewport_bytes.saturating_add(uv_view_bytes);
        metrics.view_texture_count = 2;
        metrics
    }
}

fn rgba8_texture_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize)
        .saturating_mul(size[1] as usize)
        .saturating_mul(4)
}
