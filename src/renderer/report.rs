use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        render_report::{RenderMetrics, RenderReport},
        surface::PaintSurfaceId,
    },
    renderer::{
        document::surfaces::SurfacePrepareStats,
        mutation::{MutationLog, PresentDirty},
        result::StartedRenderCommit,
    },
};

/// Shadow observation for mutation-derived presentation updates.
///
/// This diagnostic state records what renderer commands claim to mutate and
/// which presentation channels are dirtied from those mutations.
/// `RenderChanges::present_dirty` remains the active presentation contract
/// consumed by `RenderHost`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct MutationShadow {
    pub(crate) observed: MutationLog,
    pub(crate) derived_present_dirty: PresentDirty,
}

impl MutationShadow {
    pub(crate) fn observe(&mut self, mutations: &MutationLog) {
        self.observed.merge(mutations.clone());
        self.derived_present_dirty.merge(mutations.present_dirty());
    }
}

#[derive(Debug)]
pub(crate) enum DiagnosticReadbackRequest {
    SpatialBlur {
        buffer: wgpu::Buffer,
        target_stride_px: u32,
        actual_stride_px: u32,
    },
}

#[derive(Debug, Default)]
pub(crate) struct CommandResult {
    pub(crate) mutations: MutationLog,
    pub(crate) metrics: RenderMetrics,
    pub(crate) newly_active_paint_surfaces: Vec<PaintSurfaceId>,
    pub(crate) active_paint_surfaces: Vec<PaintSurfaceId>,
    pub(crate) diagnostic_readbacks: Vec<DiagnosticReadbackRequest>,
}

impl CommandResult {
    pub(crate) fn from_mutations(mutations: MutationLog) -> Self {
        Self::new(mutations, RenderMetrics::default())
    }

    pub(crate) fn new(mutations: MutationLog, metrics: RenderMetrics) -> Self {
        Self {
            mutations,
            metrics,
            newly_active_paint_surfaces: Vec::new(),
            active_paint_surfaces: Vec::new(),
            diagnostic_readbacks: Vec::new(),
        }
    }

    pub(crate) fn with_active_paint_surfaces(
        mut self,
        newly_active_paint_surfaces: impl IntoIterator<Item = PaintSurfaceId>,
        active_paint_surfaces: impl IntoIterator<Item = PaintSurfaceId>,
    ) -> Self {
        self.newly_active_paint_surfaces = newly_active_paint_surfaces.into_iter().collect();
        self.active_paint_surfaces = active_paint_surfaces.into_iter().collect();
        self
    }

    pub(crate) fn with_diagnostic_readback(mut self, request: DiagnosticReadbackRequest) -> Self {
        self.diagnostic_readbacks.push(request);
        self
    }

    pub(crate) fn has_newly_active_paint_surfaces(&self) -> bool {
        !self.newly_active_paint_surfaces.is_empty()
    }

    pub(crate) fn into_parts(self) -> (MutationLog, RenderMetrics, Vec<DiagnosticReadbackRequest>) {
        (self.mutations, self.metrics, self.diagnostic_readbacks)
    }
}

/// Accumulates renderer-side presentation changes produced by a batch.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RenderChanges {
    pub(crate) present_dirty: PresentDirty,
    pub(crate) mutation_shadow: MutationShadow,
}

impl RenderChanges {
    pub(crate) fn request_present(&mut self, dirty: PresentDirty) {
        self.present_dirty.merge(dirty);
    }

    pub(crate) fn observe_mutations(&mut self, mutations: &MutationLog) {
        self.mutation_shadow.observe(mutations);
    }
}

#[derive(Debug, Clone)]
pub struct RenderExecutionReport {
    pub(crate) report: RenderReport,
    pub(crate) changes: RenderChanges,
    pub(crate) started_commit: std::result::Result<Option<StartedRenderCommit>, String>,
}

impl RenderExecutionReport {
    pub fn is_ok(&self) -> bool {
        self.report.is_ok()
    }

    pub fn error_message(&self) -> Option<&str> {
        self.report.error.as_deref()
    }

    pub fn metrics(&self) -> &RenderMetrics {
        &self.report.metrics
    }

    pub(crate) fn ok_with_started_commit(
        metrics: RenderMetrics,
        changes: RenderChanges,
        started_commit: std::result::Result<Option<StartedRenderCommit>, String>,
    ) -> Self {
        Self {
            report: RenderReport::ok(metrics),
            changes,
            started_commit,
        }
    }

    pub(crate) fn error(error: String, metrics: RenderMetrics, changes: RenderChanges) -> Self {
        Self {
            report: RenderReport::error(error, metrics),
            changes,
            started_commit: Err("render batch failed before surface commit readback".to_owned()),
        }
    }
}

pub(crate) fn record_surface_prepare_metrics(
    metrics: &mut RenderMetrics,
    stats: &SurfacePrepareStats,
) {
    if stats.evicted {
        metrics.layer_gpu_eviction_count = metrics.layer_gpu_eviction_count.saturating_add(1);
    }
    if stats.rehydrated {
        metrics.layer_gpu_rehydration_count = metrics.layer_gpu_rehydration_count.saturating_add(1);
        metrics.layer_gpu_rehydration_tile_count = metrics
            .layer_gpu_rehydration_tile_count
            .saturating_add(stats.uploaded_tiles);
        metrics.layer_gpu_rehydration_bytes = metrics
            .layer_gpu_rehydration_bytes
            .saturating_add(stats.uploaded_bytes);
    }
}
