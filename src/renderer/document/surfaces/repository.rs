use std::{cell::Cell, collections::HashMap};

use eframe::egui_wgpu::wgpu;

use crate::{
    core::{damage::DamageMap, render_report::GpuTextureMetrics, surface::PaintSurfaceId},
    renderer::document::surfaces::SurfaceMutationCommit,
};

use super::{
    record::{ResidentSurfaceTexture, SurfaceRecord},
    storage::MaskProxyConvertResources,
};

/// Document-facing owner for persistent paint surfaces.
///
/// `SurfaceRepository` owns the paint surface records used by the renderer.
/// Role-specific surface modules expose texture, upload, readback, tile-shadow,
/// and residency operations without routing through a GPU-global surface store.
pub(crate) struct SurfaceRepository {
    pub(crate) surfaces: HashMap<PaintSurfaceId, SurfaceRecord>,
    pub(crate) mask_edit_proxies: HashMap<PaintSurfaceId, ResidentSurfaceTexture>,
    pub(super) mask_proxy_convert_resources: Option<MaskProxyConvertResources>,
    pub(crate) use_frame: Cell<u64>,
}

impl SurfaceRepository {
    pub(crate) fn new() -> Self {
        Self {
            surfaces: HashMap::new(),
            mask_edit_proxies: HashMap::new(),
            mask_proxy_convert_resources: None,
            use_frame: Cell::new(0),
        }
    }

    pub(crate) fn commit_damage(
        &mut self,
        target: PaintSurfaceId,
        damage: Option<&DamageMap>,
    ) -> SurfaceMutationCommit {
        let prepare = self.mark_surface_tile_cache_stale_damage(damage, target);
        SurfaceMutationCommit { prepare }
    }

    pub(crate) fn commit_full_damage(&mut self, target: PaintSurfaceId) -> SurfaceMutationCommit {
        let prepare = self.mark_surface_tile_cache_stale(target);
        SurfaceMutationCommit { prepare }
    }

    pub(super) fn ensure_mask_proxy_convert_resources(&mut self, device: &wgpu::Device) {
        if self.mask_proxy_convert_resources.is_none() {
            self.mask_proxy_convert_resources = Some(MaskProxyConvertResources::new(device));
        }
    }

    pub(super) fn mask_proxy_convert_resources(&self) -> &MaskProxyConvertResources {
        self.mask_proxy_convert_resources
            .as_ref()
            .expect("mask proxy convert resources must be initialized before use")
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        for surface in self.surfaces.values() {
            if surface.gpu.is_some() {
                let bytes = surface
                    .format
                    .estimated_gpu_bytes(surface.texture_size)
                    .unwrap_or(0);
                metrics.add_total_bytes(bytes);
                metrics.resident_surface_count = metrics.resident_surface_count.saturating_add(1);
                if surface.target.is_mask() {
                    metrics.mask_surface_bytes = metrics.mask_surface_bytes.saturating_add(bytes);
                } else {
                    metrics.paint_surface_bytes = metrics.paint_surface_bytes.saturating_add(bytes);
                }
            } else {
                metrics.nonresident_surface_count =
                    metrics.nonresident_surface_count.saturating_add(1);
            }
        }
        for target in self.mask_edit_proxies.keys() {
            if let Some(surface) = self.surfaces.get(target) {
                let bytes = rgba8_texture_bytes(surface.texture_size);
                metrics.add_total_bytes(bytes);
                metrics.mask_surface_bytes = metrics.mask_surface_bytes.saturating_add(bytes);
                metrics.resident_surface_count = metrics.resident_surface_count.saturating_add(1);
            }
        }
        metrics
    }
}

fn rgba8_texture_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize)
        .saturating_mul(size[1] as usize)
        .saturating_mul(4)
}

impl Default for SurfaceRepository {
    fn default() -> Self {
        Self::new()
    }
}
