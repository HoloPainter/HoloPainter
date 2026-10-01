//! CPU-shadow backed surface preparation boundary.

use anyhow::Result;

use crate::{
    core::surface::PaintSurfaceId,
    renderer::{engine::gpu_state::RendererGpuState, gpu::frame::GpuFrame},
};

use super::{SurfacePrepareStats, SurfaceRepository};

impl SurfaceRepository {
    pub(crate) fn prepare_read_surface_for_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
    ) -> Result<SurfacePrepareStats> {
        self.ensure_surface_gpu_resident_into_frame(gpu, frame, surface)
    }

    pub(crate) fn prepare_edit_surface_for_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
    ) -> Result<SurfacePrepareStats> {
        let stats = self.ensure_surface_gpu_resident_into_frame(gpu, frame, surface)?;
        if surface.is_mask() {
            self.ensure_mask_edit_proxy_into_frame(gpu, frame, surface)?;
        }
        Ok(stats)
    }
}
