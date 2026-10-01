use anyhow::Result;

use crate::{
    core::{
        image::Rgba8Snapshot, surface::PaintSurfaceId, tile_cache::TileCacheOpStats,
        tile_payload::TilePayload,
    },
    renderer::{engine::gpu_state::RendererGpuState, gpu::frame::GpuFrame},
};

use super::SurfaceRepository;

impl SurfaceRepository {
    pub(crate) fn upload_surface_rgba8_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        snapshot: &Rgba8Snapshot,
        document_revision: Option<u64>,
    ) -> Result<TileCacheOpStats> {
        self.upload_surface_record_rgba8_into_frame(gpu, frame, target, snapshot, document_revision)
    }

    pub(crate) fn upload_surface_rgba8_tiles_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        tiles: &[TilePayload],
        document_revision: Option<u64>,
    ) -> Result<TileCacheOpStats> {
        self.upload_surface_record_rgba8_tiles_into_frame(
            gpu,
            frame,
            target,
            tiles,
            document_revision,
        )
    }
}
