use crate::{
    core::{image::Rgba8Snapshot, render_report::RenderMetrics},
    renderer::document::surfaces::SurfaceReadSource,
};

#[derive(Default)]
pub(crate) struct RenderMetricsState {
    metrics: RenderMetrics,
}

impl RenderMetricsState {
    pub(crate) fn get_mut(&mut self) -> &mut RenderMetrics {
        &mut self.metrics
    }

    pub(crate) fn take(&mut self) -> RenderMetrics {
        std::mem::take(&mut self.metrics)
    }
}

pub(super) fn record_surface_read_metrics(
    metrics: &mut RenderMetrics,
    source: &SurfaceReadSource,
    snapshot: &Rgba8Snapshot,
) {
    match source {
        SurfaceReadSource::TileCache(stats) => {
            metrics.tile_cache_read_count = metrics.tile_cache_read_count.saturating_add(1);
            metrics.tile_cache_read_tile_count = metrics
                .tile_cache_read_tile_count
                .saturating_add(stats.tile_count);
            metrics.tile_cache_bytes_read =
                metrics.tile_cache_bytes_read.saturating_add(stats.bytes);
        }
        SurfaceReadSource::GpuReadback { cache_update } => {
            metrics.record_readback(snapshot.texture_size, snapshot.origin, snapshot.size);
            if let Some(stats) = cache_update {
                record_tile_cache_update_metrics(metrics, stats);
            }
        }
    }
}

pub(super) fn record_tile_cache_update_metrics(
    metrics: &mut RenderMetrics,
    stats: &crate::core::tile_cache::TileCacheOpStats,
) {
    metrics.tile_cache_update_count = metrics.tile_cache_update_count.saturating_add(1);
    metrics.tile_cache_updated_tile_count = metrics
        .tile_cache_updated_tile_count
        .saturating_add(stats.tile_count);
    metrics.tile_cache_bytes_written = metrics.tile_cache_bytes_written.saturating_add(stats.bytes);
}
