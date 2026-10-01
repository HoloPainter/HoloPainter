use eframe::egui;

use crate::{
    core::render_report::{GpuTextureMetrics, RenderMetrics},
    localization::Localization,
};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeMetricsUiData {
    pub last: RenderMetrics,
    pub session: RenderMetrics,
    pub textures: GpuTextureMetrics,
    pub peak_gpu_texture_bytes: usize,
}

impl RuntimeMetricsUiData {
    pub fn observe_render_metrics(&mut self, metrics: RenderMetrics) {
        if metrics == RenderMetrics::default() {
            return;
        }
        self.last = metrics.clone();
        self.session.merge(metrics);
    }

    pub fn observe_texture_metrics(&mut self, metrics: GpuTextureMetrics) {
        self.peak_gpu_texture_bytes = self.peak_gpu_texture_bytes.max(metrics.total_bytes);
        self.textures = metrics;
    }
}

pub fn draw_top_bar_metrics(
    ui: &mut egui::Ui,
    l10n: &Localization,
    metrics: &RuntimeMetricsUiData,
) {
    ui.separator();
    let summary = metric_value(
        l10n,
        "runtime-metrics-top-summary",
        &[
            ("textures", format_bytes(metrics.textures.total_bytes)),
            ("upload", format_bytes(metrics.last.upload_bytes)),
            ("readback", format_bytes(metrics.last.readback_bytes)),
            ("composite", composite_summary(l10n, &metrics.last)),
        ],
    );
    if metrics.last.partial_composite_full_fallbacks > 0
        || metrics.last.composite_checkpoint_budget_fallbacks > 0
        || metrics.last.readback_bytes >= 64 * 1024 * 1024
    {
        ui.colored_label(egui::Color32::YELLOW, summary);
    } else {
        ui.label(summary);
    }
}

pub fn draw_texture_summary(
    ui: &mut egui::Ui,
    l10n: &Localization,
    metrics: &RuntimeMetricsUiData,
) {
    egui::Grid::new("runtime_metrics_texture_summary_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-gpu-texture-usage",
                format_bytes(metrics.textures.total_bytes),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-composite",
                composite_summary(l10n, &metrics.last),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-upload-readback",
                format!(
                    "{} / {}",
                    format_bytes(metrics.last.upload_bytes),
                    format_bytes(metrics.last.readback_bytes)
                ),
            );
        });
}

pub fn draw_runtime_metrics_window(
    ctx: &egui::Context,
    l10n: &Localization,
    open: &mut bool,
    metrics: &RuntimeMetricsUiData,
) {
    egui::Window::new(l10n.text("runtime-metrics-title"))
        .open(open)
        .default_size([430.0, 560.0])
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("runtime_metrics_window_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    draw_runtime_metrics_content(ui, l10n, metrics);
                });
        });
}

fn draw_runtime_metrics_content(
    ui: &mut egui::Ui,
    l10n: &Localization,
    metrics: &RuntimeMetricsUiData,
) {
    draw_summary(ui, l10n, metrics);
    ui.separator();
    draw_texture_metrics(ui, l10n, metrics);
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-gpu-transfers"))
        .default_open(false)
        .show(ui, |ui| draw_transfer_metrics(ui, l10n, &metrics.last));
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-composite"))
        .default_open(false)
        .show(ui, |ui| draw_composite_metrics(ui, l10n, &metrics.last));
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-surface-paint"))
        .default_open(false)
        .show(ui, |ui| draw_surface_paint_metrics(ui, l10n, &metrics.last));
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-decal"))
        .default_open(false)
        .show(ui, |ui| draw_decal_metrics(ui, l10n, &metrics.last));
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-cache-residency"))
        .default_open(false)
        .show(ui, |ui| {
            draw_cache_metrics(ui, l10n, &metrics.last, &metrics.textures)
        });
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-raw-counters"))
        .default_open(false)
        .show(ui, |ui| draw_raw_counters(ui, &metrics.last));
}

fn draw_surface_paint_metrics(ui: &mut egui::Ui, l10n: &Localization, metrics: &RenderMetrics) {
    egui::Grid::new("runtime_metrics_surface_paint_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-targets-materials",
                format!(
                    "{} / {}",
                    metrics.surface_target_surface_count_max,
                    metrics.surface_target_material_count_max
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-new-stroke-targets",
                metric_value(
                    l10n,
                    "runtime-metric-value-surfaces-materials",
                    &[
                        (
                            "surfaces",
                            metrics.surface_new_stroke_surface_count.to_string(),
                        ),
                        (
                            "materials",
                            metrics.surface_new_stroke_material_count.to_string(),
                        ),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-batches-dabs-passes",
                format!(
                    "{} / {} / {}",
                    metrics.surface_projection_batch_count,
                    metrics.surface_dab_count,
                    metrics.surface_brush_pass_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-material-dab-refs-multi-dabs",
                format!(
                    "{} / {}",
                    metrics.surface_target_material_dab_reference_count,
                    metrics.surface_multi_material_dab_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-mesh-uv-scissored-full",
                format!(
                    "{} / {}",
                    metrics.surface_target_mesh_uv_scissored_pass_count,
                    metrics.surface_target_mesh_uv_full_pass_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-scissor-rects-pixels",
                format!(
                    "{} / {}",
                    metrics.surface_target_mesh_uv_scissor_rect_count,
                    format_pixels(metrics.surface_target_mesh_uv_scissor_pixel_area)
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-full-mesh-uv-pixels",
                format_pixels(metrics.surface_target_mesh_uv_full_pixel_area),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-scissor-fallbacks",
                metric_value(
                    l10n,
                    "runtime-metric-value-scissor-fallbacks",
                    &[
                        (
                            "missing",
                            metrics.surface_scissor_fallback_missing_data.to_string(),
                        ),
                        (
                            "invalid",
                            metrics.surface_scissor_fallback_invalid_input.to_string(),
                        ),
                        (
                            "boundary",
                            metrics.surface_scissor_fallback_boundary_lookup.to_string(),
                        ),
                        (
                            "no_hit",
                            metrics.surface_scissor_fallback_no_contribution.to_string(),
                        ),
                        (
                            "full",
                            metrics.surface_scissor_fallback_full_rect.to_string(),
                        ),
                        (
                            "rect_limit",
                            metrics.surface_scissor_fallback_rect_limit.to_string(),
                        ),
                        (
                            "area_limit",
                            metrics.surface_scissor_fallback_area_limit.to_string(),
                        ),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-damage-partial-full",
                metric_value(
                    l10n,
                    "runtime-metric-value-rects-surfaces",
                    &[
                        (
                            "rects",
                            metrics.surface_partial_damage_rect_count.to_string(),
                        ),
                        ("surfaces", metrics.surface_full_damage_count.to_string()),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-snapshot-begin-clear",
                format!(
                    "{} / {}",
                    format_bytes(metrics.surface_snapshot_copy_bytes),
                    format_bytes(metrics.surface_stroke_begin_clear_bytes)
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-sync-copy-clipped-full",
                format!(
                    "{} / {}",
                    format_bytes(metrics.surface_clipped_sync_copy_bytes),
                    format_bytes(metrics.surface_full_sync_copy_bytes)
                ),
            );
        });
}

fn draw_summary(ui: &mut egui::Ui, l10n: &Localization, metrics: &RuntimeMetricsUiData) {
    ui.label(l10n.text("runtime-metrics-last-activity"));
    egui::Grid::new("runtime_metrics_summary_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-gpu-texture-usage",
                format_bytes(metrics.textures.total_bytes),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-peak-gpu-texture-usage",
                format_bytes(metrics.peak_gpu_texture_bytes),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-effects",
                metrics.last.executed_effect_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-upload-readback",
                format!(
                    "{} / {}",
                    format_bytes(metrics.last.upload_bytes),
                    format_bytes(metrics.last.readback_bytes)
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-composite",
                composite_summary(l10n, &metrics.last),
            );
        });
}

fn draw_texture_metrics(ui: &mut egui::Ui, l10n: &Localization, metrics: &RuntimeMetricsUiData) {
    egui::CollapsingHeader::new(l10n.text("runtime-metrics-gpu-textures"))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("runtime_metrics_texture_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-total",
                        format_bytes(metrics.textures.total_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-paint-surfaces",
                        format_bytes(metrics.textures.paint_surface_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-layer-masks",
                        format_bytes(metrics.textures.mask_surface_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-selection-masks",
                        format_bytes(metrics.textures.selection_mask_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-composite-outputs",
                        format_bytes(metrics.textures.composite_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-composite-checkpoints",
                        format_bytes(metrics.textures.checkpoint_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-view-targets",
                        format_bytes(metrics.textures.view_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-scratch",
                        format_bytes(metrics.textures.scratch_bytes),
                    );
                    metric_row(
                        ui,
                        l10n,
                        "runtime-metric-resident-nonresident-surfaces",
                        format!(
                            "{} / {}",
                            metrics.textures.resident_surface_count,
                            metrics.textures.nonresident_surface_count
                        ),
                    );
                });
        });
}

fn draw_transfer_metrics(ui: &mut egui::Ui, l10n: &Localization, metrics: &RenderMetrics) {
    egui::Grid::new("runtime_metrics_transfer_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-upload",
                metric_value(
                    l10n,
                    "runtime-metric-value-calls-bytes",
                    &[
                        ("calls", metrics.upload_calls.to_string()),
                        ("bytes", format_bytes(metrics.upload_bytes)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-full-rect-upload",
                format!(
                    "{} / {}",
                    metrics.full_upload_calls, metrics.rect_upload_calls
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-readback",
                metric_value(
                    l10n,
                    "runtime-metric-value-calls-bytes",
                    &[
                        ("calls", metrics.readback_calls.to_string()),
                        ("bytes", format_bytes(metrics.readback_bytes)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-full-rect-readback",
                format!(
                    "{} / {}",
                    metrics.full_readback_calls, metrics.rect_readback_calls
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-selection-upload",
                metric_value(
                    l10n,
                    "runtime-metric-value-calls-bytes",
                    &[
                        ("calls", metrics.selection_upload_calls.to_string()),
                        ("bytes", format_bytes(metrics.selection_upload_bytes)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-selection-full-rect",
                format!(
                    "{} / {}",
                    metrics.selection_full_upload_calls, metrics.selection_rect_upload_calls
                ),
            );
        });
}

fn draw_composite_metrics(ui: &mut egui::Ui, l10n: &Localization, metrics: &RenderMetrics) {
    egui::Grid::new("runtime_metrics_composite_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-mode",
                composite_summary(l10n, metrics),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-mark-dirty",
                metrics.composite_mark_dirty_calls.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-requested-full-partial",
                format!(
                    "{} / {}",
                    metrics.full_composite_count, metrics.partial_composite_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-executed-full-partial",
                format!(
                    "{} / {}",
                    metrics.full_composite_executions, metrics.partial_composite_executions
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-dirty-rects",
                metrics.dirty_rect_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-dirty-tiles",
                metrics.dirty_tile_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-partial-rect-area",
                format_pixels(metrics.partial_composite_pixel_area),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-full-fallbacks",
                metrics.partial_composite_full_fallbacks.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-draw-calls",
                metrics.composite_draw_call_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-checkpoint-rebuilds-reuses",
                format!(
                    "{} / {}",
                    metrics.composite_checkpoint_rebuilds, metrics.composite_checkpoint_reuses
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-checkpoint-budget-fallbacks",
                metrics.composite_checkpoint_budget_fallbacks.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-active-prime-plan-misses",
                metrics.active_composite_prime_plan_misses.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-active-prime-budget-fallbacks",
                metrics
                    .active_composite_prime_checkpoint_budget_fallbacks
                    .to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-incremental-prime-calls",
                metrics.active_composite_incremental_prime_calls.to_string(),
            );
        });
}

fn draw_decal_metrics(ui: &mut egui::Ui, l10n: &Localization, metrics: &RenderMetrics) {
    egui::Grid::new("runtime_metrics_decal_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-plan-build",
                format!("{} us", metrics.decal_plan_build_time_us),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-candidate-intersected-triangles",
                format!(
                    "{} / {}",
                    metrics.decal_candidate_triangle_count, metrics.decal_intersection_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-targets",
                metrics.decal_target_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-footprint-damage-rects",
                format!(
                    "{} / {}",
                    metrics.decal_footprint_rect_count, metrics.decal_damage_rect_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-depth-draw-ranges",
                format!(
                    "{} / {}",
                    metrics.decal_depth_index_range_count, metrics.decal_draw_index_range_count
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-draw-batches",
                metrics.decal_draw_batch_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-draw-calls",
                metrics.decal_draw_call_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-depth-target-recreates",
                metrics.decal_depth_target_recreate_count.to_string(),
            );
        });
}

fn draw_cache_metrics(
    ui: &mut egui::Ui,
    l10n: &Localization,
    metrics: &RenderMetrics,
    textures: &GpuTextureMetrics,
) {
    egui::Grid::new("runtime_metrics_cache_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            metric_row(
                ui,
                l10n,
                "runtime-metric-surface-residency",
                metric_value(
                    l10n,
                    "runtime-metric-value-residency",
                    &[
                        ("resident", textures.resident_surface_count.to_string()),
                        (
                            "nonresident",
                            textures.nonresident_surface_count.to_string(),
                        ),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-evictions",
                metrics.layer_gpu_eviction_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-rehydrations",
                metric_value(
                    l10n,
                    "runtime-metric-value-surfaces-bytes",
                    &[
                        ("surfaces", metrics.layer_gpu_rehydration_count.to_string()),
                        ("bytes", format_bytes(metrics.layer_gpu_rehydration_bytes)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-rehydrated-tiles",
                metrics.layer_gpu_rehydration_tile_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-transient-textures",
                metric_value(
                    l10n,
                    "runtime-metric-value-allocated-reused",
                    &[
                        (
                            "allocated",
                            metrics.transient_texture_allocation_count.to_string(),
                        ),
                        ("reused", metrics.transient_texture_reuse_count.to_string()),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-brush-buffer-reallocations",
                metrics.brush_buffer_reallocation_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-uv-island-masks-generated",
                metrics.uv_island_mask_generation_count.to_string(),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-tile-cache-updates",
                metric_value(
                    l10n,
                    "runtime-metric-value-ops-tiles-bytes",
                    &[
                        ("ops", metrics.tile_cache_update_count.to_string()),
                        ("tiles", metrics.tile_cache_updated_tile_count.to_string()),
                        ("bytes", format_bytes(metrics.tile_cache_bytes_written)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-tile-cache-reads",
                metric_value(
                    l10n,
                    "runtime-metric-value-ops-tiles-bytes",
                    &[
                        ("ops", metrics.tile_cache_read_count.to_string()),
                        ("tiles", metrics.tile_cache_read_tile_count.to_string()),
                        ("bytes", format_bytes(metrics.tile_cache_bytes_read)),
                    ],
                ),
            );
            metric_row(
                ui,
                l10n,
                "runtime-metric-tiled-snapshots",
                metric_value(
                    l10n,
                    "runtime-metric-value-snapshots-tiles-bytes",
                    &[
                        ("snapshots", metrics.tiled_snapshot_count.to_string()),
                        ("tiles", metrics.tiled_snapshot_tile_count.to_string()),
                        ("bytes", format_bytes(metrics.tiled_snapshot_bytes)),
                    ],
                ),
            );
        });
}

fn draw_raw_counters(ui: &mut egui::Ui, metrics: &RenderMetrics) {
    ui.monospace(format!("effect_batches: {}", metrics.effect_batches));
    ui.monospace(format!(
        "executed_effect_count: {}",
        metrics.executed_effect_count
    ));
    ui.monospace(format!("upload_calls: {}", metrics.upload_calls));
    ui.monospace(format!("upload_bytes: {}", metrics.upload_bytes));
    ui.monospace(format!("readback_calls: {}", metrics.readback_calls));
    ui.monospace(format!("readback_bytes: {}", metrics.readback_bytes));
    ui.monospace(format!(
        "partial_composite_executions: {}",
        metrics.partial_composite_executions
    ));
    ui.monospace(format!(
        "full_composite_executions: {}",
        metrics.full_composite_executions
    ));
    ui.monospace(format!(
        "partial_composite_full_fallbacks: {}",
        metrics.partial_composite_full_fallbacks
    ));
    ui.monospace(format!(
        "composite_draw_call_count: {}",
        metrics.composite_draw_call_count
    ));
    ui.monospace(format!(
        "layer_gpu_rehydration_bytes: {}",
        metrics.layer_gpu_rehydration_bytes
    ));
    ui.monospace(format!(
        "transient_texture_allocation_count: {}",
        metrics.transient_texture_allocation_count
    ));
    ui.monospace(format!(
        "transient_texture_reuse_count: {}",
        metrics.transient_texture_reuse_count
    ));
    ui.monospace(format!(
        "brush_buffer_reallocation_count: {}",
        metrics.brush_buffer_reallocation_count
    ));
    ui.monospace(format!(
        "uv_island_mask_generation_count: {}",
        metrics.uv_island_mask_generation_count
    ));
    ui.monospace(format!(
        "decal_plan_build_time_us: {}",
        metrics.decal_plan_build_time_us
    ));
    ui.monospace(format!(
        "decal_candidate_triangle_count: {}",
        metrics.decal_candidate_triangle_count
    ));
    ui.monospace(format!(
        "decal_intersection_count: {}",
        metrics.decal_intersection_count
    ));
    ui.monospace(format!(
        "decal_draw_batch_count: {}",
        metrics.decal_draw_batch_count
    ));
    ui.monospace(format!(
        "decal_draw_call_count: {}",
        metrics.decal_draw_call_count
    ));
    ui.monospace(format!(
        "decal_depth_target_recreate_count: {}",
        metrics.decal_depth_target_recreate_count
    ));
}

fn metric_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    label_key: &'static str,
    value: impl Into<String>,
) {
    ui.label(l10n.text(label_key));
    ui.monospace(value.into());
    ui.end_row();
}

fn metric_value(
    l10n: &Localization,
    key: &'static str,
    values: &[(&'static str, String)],
) -> String {
    let mut args = fluent::FluentArgs::new();
    for (name, value) in values {
        args.set(*name, value.clone());
    }
    l10n.format(key, Some(&args))
}

fn composite_summary(l10n: &Localization, metrics: &RenderMetrics) -> String {
    let key = if metrics.partial_composite_full_fallbacks > 0 {
        "runtime-metric-composite-full-fallback"
    } else if metrics.partial_composite_executions > 0 {
        "runtime-metric-composite-partial"
    } else if metrics.full_composite_executions > 0 {
        "runtime-metric-composite-full"
    } else if metrics.partial_composite_count > 0 {
        "runtime-metric-composite-partial-pending"
    } else if metrics.full_composite_count > 0 {
        "runtime-metric-composite-full-pending"
    } else {
        "runtime-metric-composite-idle"
    };
    l10n.text(key)
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.1} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.0} KiB", bytes_f / KIB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_pixels(pixels: usize) -> String {
    if pixels >= 1_000_000 {
        format!("{:.1} MPx", pixels as f64 / 1_000_000.0)
    } else if pixels >= 1_000 {
        format!("{:.0} KPx", pixels as f64 / 1_000.0)
    } else {
        format!("{} px", pixels)
    }
}
