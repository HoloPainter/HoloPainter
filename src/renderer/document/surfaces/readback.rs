use anyhow::Result;

use eframe::egui_wgpu::wgpu;

use crate::{core::surface::PaintSurfaceId, renderer::engine::gpu_state::RendererGpuState};

use super::{SurfaceReadResult, SurfaceRepository};

impl SurfaceRepository {
    pub(crate) fn read_surface_rect_rgba8(
        &self,
        gpu: &RendererGpuState,
        queue: &wgpu::Queue,
        target: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<SurfaceReadResult> {
        self.read_surface_record_rect_rgba8(gpu, queue, target, origin, size)
    }
}
