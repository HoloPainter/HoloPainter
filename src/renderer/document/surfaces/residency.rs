use anyhow::{Result, anyhow, bail};

use crate::{
    core::surface::PaintSurfaceId,
    renderer::{
        engine::gpu_state::RendererGpuState,
        gpu::{create_mask_texture, create_paint_texture, frame::GpuFrame},
        pixel::rgba8_alpha_to_r8,
    },
};

use super::{
    SurfacePrepareStats, SurfaceRepository,
    record::{ResidentSurfaceTexture, SurfacePixelFormat},
};

const RECENT_SURFACE_RESIDENCY_WINDOW: u64 = 2;

/// Per-surface GPU residency bookkeeping.
///
/// This is deliberately local to the document surface boundary. It records the
/// minimal lifecycle data required by the current CPU-shadow rehydration path
/// without exposing an unfinished global residency policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SurfaceResidencyMetadata {
    pub(crate) last_used_frame: u64,
    pub(crate) last_resident_frame: u64,
    pub(crate) estimated_gpu_bytes: usize,
}

impl SurfaceResidencyMetadata {
    pub(crate) fn new(estimated_gpu_bytes: usize) -> Self {
        Self {
            last_used_frame: 0,
            last_resident_frame: 0,
            estimated_gpu_bytes,
        }
    }
}

impl SurfaceRepository {
    pub(crate) fn ensure_surface_gpu_resident_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
    ) -> Result<SurfacePrepareStats> {
        if self
            .surfaces
            .get(&surface)
            .and_then(|record| record.gpu.as_ref())
            .is_some()
        {
            self.mark_surface_used(surface);
            return Ok(SurfacePrepareStats::default());
        }

        let texture_size = {
            let record = self
                .surfaces
                .get(&surface)
                .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", surface))?;
            record.texture_size
        };
        let (snapshot, stats) = {
            let record = self
                .surfaces
                .get(&surface)
                .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", surface))?;
            let shadow = record.tile_shadow.borrow();
            if !shadow.is_fully_fresh() {
                bail!("cannot rehydrate non-resident surface while tile shadow is stale");
            }
            let (snapshot, stats) = shadow.cache.read_rgba_rect([0, 0], record.texture_size)?;
            (snapshot, stats)
        };
        let format = self
            .surfaces
            .get(&surface)
            .map(|record| record.format)
            .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", surface))?;
        let (texture, view) = match format {
            SurfacePixelFormat::Rgba8 => {
                let (texture, view) =
                    create_paint_texture(gpu.device(), texture_size, "surface_texture");
                frame.write_texture_rgba8(
                    gpu.device(),
                    &texture,
                    [0, 0],
                    texture_size,
                    &snapshot.rgba8,
                );
                (texture, view)
            }
            SurfacePixelFormat::R8 => {
                let (texture, view) =
                    create_mask_texture(gpu.device(), texture_size, "surface_mask_texture");
                let r8 = rgba8_alpha_to_r8(&snapshot.rgba8);
                frame.write_texture_r8(gpu.device(), &texture, [0, 0], texture_size, &r8);
                (texture, view)
            }
        };
        self.mark_surface_used(surface);
        if let Some(record) = self.surfaces.get_mut(&surface) {
            record.gpu = Some(ResidentSurfaceTexture { texture, view });
            let mut residency = record.residency_metadata.borrow_mut();
            residency.last_resident_frame = self.use_frame.get();
            residency.estimated_gpu_bytes = format.estimated_gpu_bytes(texture_size)?;
        }
        Ok(SurfacePrepareStats {
            rehydrated: true,
            uploaded_tiles: stats.tile_count,
            uploaded_bytes: stats.bytes,
            evicted: false,
        })
    }

    pub(super) fn mark_surface_used(&self, target: PaintSurfaceId) {
        if let Some(record) = self.surfaces.get(&target) {
            record.residency_metadata.borrow_mut().last_used_frame = self.use_frame.get();
        }
    }

    pub(crate) fn begin_residency_epoch(&self) {
        self.use_frame.set(self.use_frame.get().saturating_add(1));
    }

    pub(crate) fn evict_unused_resident_surfaces(&mut self) -> usize {
        let current_frame = self.use_frame.get();
        let mut evicted = 0usize;
        for record in self.surfaces.values_mut() {
            if record.gpu.is_none() || !record.tile_shadow.borrow().is_fully_fresh() {
                continue;
            }
            let last_used_frame = record.residency_metadata.borrow().last_used_frame;
            if current_frame.saturating_sub(last_used_frame) <= RECENT_SURFACE_RESIDENCY_WINDOW {
                continue;
            }
            record.gpu = None;
            record.residency_metadata.borrow_mut().estimated_gpu_bytes = 0;
            evicted = evicted.saturating_add(1);
        }
        evicted
    }
}
