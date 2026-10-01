#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GpuTextureMetrics {
    pub total_bytes: usize,
    pub paint_surface_bytes: usize,
    pub mask_surface_bytes: usize,
    pub selection_mask_bytes: usize,
    pub composite_bytes: usize,
    pub checkpoint_bytes: usize,
    pub view_bytes: usize,
    pub scratch_bytes: usize,
    pub resident_surface_count: usize,
    pub nonresident_surface_count: usize,
    pub selection_mask_count: usize,
    pub composite_texture_count: usize,
    pub checkpoint_texture_count: usize,
    pub view_texture_count: usize,
    pub scratch_texture_count: usize,
}

impl GpuTextureMetrics {
    pub fn merge(&mut self, other: Self) {
        self.total_bytes = self.total_bytes.saturating_add(other.total_bytes);
        self.paint_surface_bytes = self
            .paint_surface_bytes
            .saturating_add(other.paint_surface_bytes);
        self.mask_surface_bytes = self
            .mask_surface_bytes
            .saturating_add(other.mask_surface_bytes);
        self.selection_mask_bytes = self
            .selection_mask_bytes
            .saturating_add(other.selection_mask_bytes);
        self.composite_bytes = self.composite_bytes.saturating_add(other.composite_bytes);
        self.checkpoint_bytes = self.checkpoint_bytes.saturating_add(other.checkpoint_bytes);
        self.view_bytes = self.view_bytes.saturating_add(other.view_bytes);
        self.scratch_bytes = self.scratch_bytes.saturating_add(other.scratch_bytes);
        self.resident_surface_count = self
            .resident_surface_count
            .saturating_add(other.resident_surface_count);
        self.nonresident_surface_count = self
            .nonresident_surface_count
            .saturating_add(other.nonresident_surface_count);
        self.selection_mask_count = self
            .selection_mask_count
            .saturating_add(other.selection_mask_count);
        self.composite_texture_count = self
            .composite_texture_count
            .saturating_add(other.composite_texture_count);
        self.checkpoint_texture_count = self
            .checkpoint_texture_count
            .saturating_add(other.checkpoint_texture_count);
        self.view_texture_count = self
            .view_texture_count
            .saturating_add(other.view_texture_count);
        self.scratch_texture_count = self
            .scratch_texture_count
            .saturating_add(other.scratch_texture_count);
    }

    pub fn add_total_bytes(&mut self, bytes: usize) {
        self.total_bytes = self.total_bytes.saturating_add(bytes);
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RenderMetrics {
    pub effect_batches: usize,

    pub readback_calls: usize,
    pub full_readback_calls: usize,
    pub rect_readback_calls: usize,
    pub readback_bytes: usize,

    pub upload_calls: usize,
    pub full_upload_calls: usize,
    pub rect_upload_calls: usize,
    pub upload_bytes: usize,

    pub selection_upload_calls: usize,
    pub selection_full_upload_calls: usize,
    pub selection_rect_upload_calls: usize,
    pub selection_upload_bytes: usize,

    pub composite_mark_dirty_calls: usize,
    pub full_composite_count: usize,
    pub partial_composite_count: usize,
    pub full_composite_executions: usize,
    pub partial_composite_executions: usize,
    pub partial_composite_rect_count: usize,
    pub partial_composite_pixel_area: usize,
    pub partial_composite_full_fallbacks: usize,
    pub composite_draw_call_count: usize,

    pub composite_checkpoint_rebuilds: usize,
    pub composite_checkpoint_reuses: usize,
    pub composite_checkpoint_budget_fallbacks: usize,
    pub stroke_composite_general_fallbacks: usize,
    pub stroke_composite_plan_misses: usize,
    pub stroke_composite_checkpoint_budget_fallbacks: usize,
    pub stroke_composite_upload_attempts: usize,
    pub stroke_composite_upload_bytes: usize,
    pub active_composite_prime_plan_misses: usize,
    pub active_composite_prime_checkpoint_budget_fallbacks: usize,
    pub active_composite_incremental_prime_calls: usize,
    pub active_composite_incremental_prime_surfaces: usize,
    pub active_composite_incremental_prime_materials: usize,
    pub active_composite_prime_upload_calls: usize,
    pub active_composite_prime_upload_bytes: usize,
    pub active_composite_checkpoint_rebuild_uploads: usize,
    pub active_composite_source_cache_uploads: usize,
    pub composite_transient_source_uploads: usize,
    pub composite_transient_source_upload_bytes: usize,
    pub composite_known_empty_raster_skips: usize,
    pub composite_known_zero_mask_skips: usize,
    pub composite_known_empty_group_skips: usize,

    pub dirty_rect_count: usize,
    pub dirty_tile_count: usize,

    pub tiled_snapshot_count: usize,
    pub tiled_snapshot_tile_count: usize,
    pub tiled_snapshot_bytes: usize,

    pub tile_cache_update_count: usize,
    pub tile_cache_updated_tile_count: usize,
    pub tile_cache_read_count: usize,
    pub tile_cache_read_tile_count: usize,
    pub tile_cache_bytes_written: usize,
    pub tile_cache_bytes_read: usize,

    pub layer_gpu_eviction_count: usize,
    pub layer_gpu_rehydration_count: usize,
    pub layer_gpu_rehydration_bytes: usize,
    pub layer_gpu_rehydration_tile_count: usize,

    pub transient_texture_allocation_count: usize,
    pub transient_texture_reuse_count: usize,
    pub brush_buffer_reallocation_count: usize,
    pub uv_island_mask_generation_count: usize,

    // Surface-paint workload diagnostics. These stay as counters so the hot path
    // does not allocate strings; the opt-in frame log formats them after encoding.
    pub surface_projection_batch_count: usize,
    pub surface_dab_count: usize,
    pub surface_target_material_dab_reference_count: usize,
    pub surface_multi_material_dab_count: usize,
    pub surface_target_surface_count_max: usize,
    pub surface_target_material_count_max: usize,
    pub surface_new_stroke_surface_count: usize,
    pub surface_new_stroke_material_count: usize,
    pub surface_brush_pass_count: usize,
    pub surface_target_mesh_uv_pass_count: usize,
    pub surface_target_mesh_uv_scissored_pass_count: usize,
    pub surface_target_mesh_uv_full_pass_count: usize,
    pub surface_target_mesh_uv_scissor_rect_count: usize,
    pub surface_target_mesh_uv_scissor_pixel_area: usize,
    pub surface_target_mesh_uv_full_pixel_area: usize,
    pub surface_scissor_fallback_missing_data: usize,
    pub surface_scissor_fallback_invalid_input: usize,
    pub surface_scissor_fallback_boundary_lookup: usize,
    pub surface_scissor_fallback_no_contribution: usize,
    pub surface_scissor_fallback_full_rect: usize,
    pub surface_scissor_fallback_rect_limit: usize,
    pub surface_scissor_fallback_area_limit: usize,
    pub surface_partial_damage_rect_count: usize,
    pub surface_full_damage_count: usize,
    pub surface_snapshot_copy_bytes: usize,
    pub surface_stroke_begin_clear_bytes: usize,
    pub surface_clipped_sync_copy_bytes: usize,
    pub surface_full_sync_copy_bytes: usize,

    pub decal_plan_build_time_us: u64,
    pub decal_candidate_triangle_count: usize,
    pub decal_intersection_count: usize,
    pub decal_target_count: usize,
    pub decal_footprint_rect_count: usize,
    pub decal_damage_rect_count: usize,
    pub decal_depth_index_range_count: usize,
    pub decal_draw_batch_count: usize,
    pub decal_draw_index_range_count: usize,
    pub decal_draw_call_count: usize,
    pub decal_depth_target_recreate_count: usize,

    pub executed_effect_count: usize,
}

impl RenderMetrics {
    pub fn record_readback(&mut self, texture_size: [u32; 2], origin: [u32; 2], size: [u32; 2]) {
        self.readback_calls = self.readback_calls.saturating_add(1);
        self.readback_bytes = self.readback_bytes.saturating_add(rgba8_bytes(size));
        if is_full_rect(texture_size, origin, size) {
            self.full_readback_calls = self.full_readback_calls.saturating_add(1);
        } else {
            self.rect_readback_calls = self.rect_readback_calls.saturating_add(1);
        }
    }

    pub fn record_upload(&mut self, texture_size: [u32; 2], origin: [u32; 2], size: [u32; 2]) {
        self.upload_calls = self.upload_calls.saturating_add(1);
        self.upload_bytes = self.upload_bytes.saturating_add(rgba8_bytes(size));
        if is_full_rect(texture_size, origin, size) {
            self.full_upload_calls = self.full_upload_calls.saturating_add(1);
        } else {
            self.rect_upload_calls = self.rect_upload_calls.saturating_add(1);
        }
    }

    pub fn record_selection_upload(
        &mut self,
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
    ) {
        self.selection_upload_calls = self.selection_upload_calls.saturating_add(1);
        self.selection_upload_bytes = self.selection_upload_bytes.saturating_add(r8_bytes(size));
        if is_full_rect(texture_size, origin, size) {
            self.selection_full_upload_calls = self.selection_full_upload_calls.saturating_add(1);
        } else {
            self.selection_rect_upload_calls = self.selection_rect_upload_calls.saturating_add(1);
        }
    }

    pub fn merge(&mut self, other: RenderMetrics) {
        self.effect_batches = self.effect_batches.saturating_add(other.effect_batches);
        self.readback_calls = self.readback_calls.saturating_add(other.readback_calls);
        self.full_readback_calls = self
            .full_readback_calls
            .saturating_add(other.full_readback_calls);
        self.rect_readback_calls = self
            .rect_readback_calls
            .saturating_add(other.rect_readback_calls);
        self.readback_bytes = self.readback_bytes.saturating_add(other.readback_bytes);
        self.upload_calls = self.upload_calls.saturating_add(other.upload_calls);
        self.full_upload_calls = self
            .full_upload_calls
            .saturating_add(other.full_upload_calls);
        self.rect_upload_calls = self
            .rect_upload_calls
            .saturating_add(other.rect_upload_calls);
        self.upload_bytes = self.upload_bytes.saturating_add(other.upload_bytes);
        self.selection_upload_calls = self
            .selection_upload_calls
            .saturating_add(other.selection_upload_calls);
        self.selection_full_upload_calls = self
            .selection_full_upload_calls
            .saturating_add(other.selection_full_upload_calls);
        self.selection_rect_upload_calls = self
            .selection_rect_upload_calls
            .saturating_add(other.selection_rect_upload_calls);
        self.selection_upload_bytes = self
            .selection_upload_bytes
            .saturating_add(other.selection_upload_bytes);
        self.composite_mark_dirty_calls = self
            .composite_mark_dirty_calls
            .saturating_add(other.composite_mark_dirty_calls);
        self.full_composite_count = self
            .full_composite_count
            .saturating_add(other.full_composite_count);
        self.partial_composite_count = self
            .partial_composite_count
            .saturating_add(other.partial_composite_count);
        self.full_composite_executions = self
            .full_composite_executions
            .saturating_add(other.full_composite_executions);
        self.partial_composite_executions = self
            .partial_composite_executions
            .saturating_add(other.partial_composite_executions);
        self.partial_composite_rect_count = self
            .partial_composite_rect_count
            .saturating_add(other.partial_composite_rect_count);
        self.partial_composite_pixel_area = self
            .partial_composite_pixel_area
            .saturating_add(other.partial_composite_pixel_area);
        self.partial_composite_full_fallbacks = self
            .partial_composite_full_fallbacks
            .saturating_add(other.partial_composite_full_fallbacks);
        self.composite_draw_call_count = self
            .composite_draw_call_count
            .saturating_add(other.composite_draw_call_count);
        self.composite_checkpoint_rebuilds = self
            .composite_checkpoint_rebuilds
            .saturating_add(other.composite_checkpoint_rebuilds);
        self.composite_checkpoint_reuses = self
            .composite_checkpoint_reuses
            .saturating_add(other.composite_checkpoint_reuses);
        self.composite_checkpoint_budget_fallbacks = self
            .composite_checkpoint_budget_fallbacks
            .saturating_add(other.composite_checkpoint_budget_fallbacks);
        self.stroke_composite_general_fallbacks = self
            .stroke_composite_general_fallbacks
            .saturating_add(other.stroke_composite_general_fallbacks);
        self.stroke_composite_plan_misses = self
            .stroke_composite_plan_misses
            .saturating_add(other.stroke_composite_plan_misses);
        self.stroke_composite_checkpoint_budget_fallbacks = self
            .stroke_composite_checkpoint_budget_fallbacks
            .saturating_add(other.stroke_composite_checkpoint_budget_fallbacks);
        self.stroke_composite_upload_attempts = self
            .stroke_composite_upload_attempts
            .saturating_add(other.stroke_composite_upload_attempts);
        self.stroke_composite_upload_bytes = self
            .stroke_composite_upload_bytes
            .saturating_add(other.stroke_composite_upload_bytes);
        self.active_composite_prime_plan_misses = self
            .active_composite_prime_plan_misses
            .saturating_add(other.active_composite_prime_plan_misses);
        self.active_composite_prime_checkpoint_budget_fallbacks = self
            .active_composite_prime_checkpoint_budget_fallbacks
            .saturating_add(other.active_composite_prime_checkpoint_budget_fallbacks);
        self.active_composite_incremental_prime_calls = self
            .active_composite_incremental_prime_calls
            .saturating_add(other.active_composite_incremental_prime_calls);
        self.active_composite_incremental_prime_surfaces = self
            .active_composite_incremental_prime_surfaces
            .saturating_add(other.active_composite_incremental_prime_surfaces);
        self.active_composite_incremental_prime_materials = self
            .active_composite_incremental_prime_materials
            .saturating_add(other.active_composite_incremental_prime_materials);
        self.active_composite_prime_upload_calls = self
            .active_composite_prime_upload_calls
            .saturating_add(other.active_composite_prime_upload_calls);
        self.active_composite_prime_upload_bytes = self
            .active_composite_prime_upload_bytes
            .saturating_add(other.active_composite_prime_upload_bytes);
        self.active_composite_checkpoint_rebuild_uploads = self
            .active_composite_checkpoint_rebuild_uploads
            .saturating_add(other.active_composite_checkpoint_rebuild_uploads);
        self.active_composite_source_cache_uploads = self
            .active_composite_source_cache_uploads
            .saturating_add(other.active_composite_source_cache_uploads);
        self.composite_transient_source_uploads = self
            .composite_transient_source_uploads
            .saturating_add(other.composite_transient_source_uploads);
        self.composite_transient_source_upload_bytes = self
            .composite_transient_source_upload_bytes
            .saturating_add(other.composite_transient_source_upload_bytes);
        self.composite_known_empty_raster_skips = self
            .composite_known_empty_raster_skips
            .saturating_add(other.composite_known_empty_raster_skips);
        self.composite_known_zero_mask_skips = self
            .composite_known_zero_mask_skips
            .saturating_add(other.composite_known_zero_mask_skips);
        self.composite_known_empty_group_skips = self
            .composite_known_empty_group_skips
            .saturating_add(other.composite_known_empty_group_skips);
        self.dirty_rect_count = self.dirty_rect_count.saturating_add(other.dirty_rect_count);
        self.dirty_tile_count = self.dirty_tile_count.saturating_add(other.dirty_tile_count);
        self.tiled_snapshot_count = self
            .tiled_snapshot_count
            .saturating_add(other.tiled_snapshot_count);
        self.tiled_snapshot_tile_count = self
            .tiled_snapshot_tile_count
            .saturating_add(other.tiled_snapshot_tile_count);
        self.tiled_snapshot_bytes = self
            .tiled_snapshot_bytes
            .saturating_add(other.tiled_snapshot_bytes);
        self.tile_cache_update_count = self
            .tile_cache_update_count
            .saturating_add(other.tile_cache_update_count);
        self.tile_cache_updated_tile_count = self
            .tile_cache_updated_tile_count
            .saturating_add(other.tile_cache_updated_tile_count);
        self.tile_cache_read_count = self
            .tile_cache_read_count
            .saturating_add(other.tile_cache_read_count);
        self.tile_cache_read_tile_count = self
            .tile_cache_read_tile_count
            .saturating_add(other.tile_cache_read_tile_count);
        self.tile_cache_bytes_written = self
            .tile_cache_bytes_written
            .saturating_add(other.tile_cache_bytes_written);
        self.tile_cache_bytes_read = self
            .tile_cache_bytes_read
            .saturating_add(other.tile_cache_bytes_read);
        self.layer_gpu_eviction_count = self
            .layer_gpu_eviction_count
            .saturating_add(other.layer_gpu_eviction_count);
        self.layer_gpu_rehydration_count = self
            .layer_gpu_rehydration_count
            .saturating_add(other.layer_gpu_rehydration_count);
        self.layer_gpu_rehydration_bytes = self
            .layer_gpu_rehydration_bytes
            .saturating_add(other.layer_gpu_rehydration_bytes);
        self.layer_gpu_rehydration_tile_count = self
            .layer_gpu_rehydration_tile_count
            .saturating_add(other.layer_gpu_rehydration_tile_count);
        self.transient_texture_allocation_count = self
            .transient_texture_allocation_count
            .saturating_add(other.transient_texture_allocation_count);
        self.transient_texture_reuse_count = self
            .transient_texture_reuse_count
            .saturating_add(other.transient_texture_reuse_count);
        self.brush_buffer_reallocation_count = self
            .brush_buffer_reallocation_count
            .saturating_add(other.brush_buffer_reallocation_count);
        self.uv_island_mask_generation_count = self
            .uv_island_mask_generation_count
            .saturating_add(other.uv_island_mask_generation_count);
        self.surface_projection_batch_count = self
            .surface_projection_batch_count
            .saturating_add(other.surface_projection_batch_count);
        self.surface_dab_count = self
            .surface_dab_count
            .saturating_add(other.surface_dab_count);
        self.surface_target_material_dab_reference_count = self
            .surface_target_material_dab_reference_count
            .saturating_add(other.surface_target_material_dab_reference_count);
        self.surface_multi_material_dab_count = self
            .surface_multi_material_dab_count
            .saturating_add(other.surface_multi_material_dab_count);
        self.surface_target_surface_count_max = self
            .surface_target_surface_count_max
            .max(other.surface_target_surface_count_max);
        self.surface_target_material_count_max = self
            .surface_target_material_count_max
            .max(other.surface_target_material_count_max);
        self.surface_new_stroke_surface_count = self
            .surface_new_stroke_surface_count
            .saturating_add(other.surface_new_stroke_surface_count);
        self.surface_new_stroke_material_count = self
            .surface_new_stroke_material_count
            .saturating_add(other.surface_new_stroke_material_count);
        self.surface_brush_pass_count = self
            .surface_brush_pass_count
            .saturating_add(other.surface_brush_pass_count);
        self.surface_target_mesh_uv_pass_count = self
            .surface_target_mesh_uv_pass_count
            .saturating_add(other.surface_target_mesh_uv_pass_count);
        self.surface_target_mesh_uv_scissored_pass_count = self
            .surface_target_mesh_uv_scissored_pass_count
            .saturating_add(other.surface_target_mesh_uv_scissored_pass_count);
        self.surface_target_mesh_uv_full_pass_count = self
            .surface_target_mesh_uv_full_pass_count
            .saturating_add(other.surface_target_mesh_uv_full_pass_count);
        self.surface_target_mesh_uv_scissor_rect_count = self
            .surface_target_mesh_uv_scissor_rect_count
            .saturating_add(other.surface_target_mesh_uv_scissor_rect_count);
        self.surface_target_mesh_uv_scissor_pixel_area = self
            .surface_target_mesh_uv_scissor_pixel_area
            .saturating_add(other.surface_target_mesh_uv_scissor_pixel_area);
        self.surface_target_mesh_uv_full_pixel_area = self
            .surface_target_mesh_uv_full_pixel_area
            .saturating_add(other.surface_target_mesh_uv_full_pixel_area);
        self.surface_scissor_fallback_missing_data = self
            .surface_scissor_fallback_missing_data
            .saturating_add(other.surface_scissor_fallback_missing_data);
        self.surface_scissor_fallback_invalid_input = self
            .surface_scissor_fallback_invalid_input
            .saturating_add(other.surface_scissor_fallback_invalid_input);
        self.surface_scissor_fallback_boundary_lookup = self
            .surface_scissor_fallback_boundary_lookup
            .saturating_add(other.surface_scissor_fallback_boundary_lookup);
        self.surface_scissor_fallback_no_contribution = self
            .surface_scissor_fallback_no_contribution
            .saturating_add(other.surface_scissor_fallback_no_contribution);
        self.surface_scissor_fallback_full_rect = self
            .surface_scissor_fallback_full_rect
            .saturating_add(other.surface_scissor_fallback_full_rect);
        self.surface_scissor_fallback_rect_limit = self
            .surface_scissor_fallback_rect_limit
            .saturating_add(other.surface_scissor_fallback_rect_limit);
        self.surface_scissor_fallback_area_limit = self
            .surface_scissor_fallback_area_limit
            .saturating_add(other.surface_scissor_fallback_area_limit);
        self.surface_partial_damage_rect_count = self
            .surface_partial_damage_rect_count
            .saturating_add(other.surface_partial_damage_rect_count);
        self.surface_full_damage_count = self
            .surface_full_damage_count
            .saturating_add(other.surface_full_damage_count);
        self.surface_snapshot_copy_bytes = self
            .surface_snapshot_copy_bytes
            .saturating_add(other.surface_snapshot_copy_bytes);
        self.surface_stroke_begin_clear_bytes = self
            .surface_stroke_begin_clear_bytes
            .saturating_add(other.surface_stroke_begin_clear_bytes);
        self.surface_clipped_sync_copy_bytes = self
            .surface_clipped_sync_copy_bytes
            .saturating_add(other.surface_clipped_sync_copy_bytes);
        self.surface_full_sync_copy_bytes = self
            .surface_full_sync_copy_bytes
            .saturating_add(other.surface_full_sync_copy_bytes);
        self.decal_plan_build_time_us = self
            .decal_plan_build_time_us
            .saturating_add(other.decal_plan_build_time_us);
        self.decal_candidate_triangle_count = self
            .decal_candidate_triangle_count
            .saturating_add(other.decal_candidate_triangle_count);
        self.decal_intersection_count = self
            .decal_intersection_count
            .saturating_add(other.decal_intersection_count);
        self.decal_target_count = self
            .decal_target_count
            .saturating_add(other.decal_target_count);
        self.decal_footprint_rect_count = self
            .decal_footprint_rect_count
            .saturating_add(other.decal_footprint_rect_count);
        self.decal_damage_rect_count = self
            .decal_damage_rect_count
            .saturating_add(other.decal_damage_rect_count);
        self.decal_depth_index_range_count = self
            .decal_depth_index_range_count
            .saturating_add(other.decal_depth_index_range_count);
        self.decal_draw_batch_count = self
            .decal_draw_batch_count
            .saturating_add(other.decal_draw_batch_count);
        self.decal_draw_index_range_count = self
            .decal_draw_index_range_count
            .saturating_add(other.decal_draw_index_range_count);
        self.decal_draw_call_count = self
            .decal_draw_call_count
            .saturating_add(other.decal_draw_call_count);
        self.decal_depth_target_recreate_count = self
            .decal_depth_target_recreate_count
            .saturating_add(other.decal_depth_target_recreate_count);
        self.executed_effect_count = self
            .executed_effect_count
            .saturating_add(other.executed_effect_count);
    }
}

#[derive(Debug, Default, Clone)]
pub struct RenderReport {
    pub metrics: RenderMetrics,
    pub error: Option<String>,
}

impl RenderReport {
    pub fn ok(metrics: RenderMetrics) -> Self {
        Self {
            metrics,
            error: None,
        }
    }

    pub fn error(error: String, metrics: RenderMetrics) -> Self {
        Self {
            metrics,
            error: Some(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

fn is_full_rect(texture_size: [u32; 2], origin: [u32; 2], size: [u32; 2]) -> bool {
    origin == [0, 0] && size == texture_size
}

fn rgba8_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize)
        .saturating_mul(size[1] as usize)
        .saturating_mul(4)
}

fn r8_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize).saturating_mul(size[1] as usize)
}

#[cfg(test)]
mod tests {
    use super::RenderMetrics;

    #[test]
    fn merge_accumulates_decal_metrics() {
        let mut metrics = RenderMetrics {
            decal_plan_build_time_us: 10,
            decal_candidate_triangle_count: 20,
            decal_draw_batch_count: 4,
            decal_draw_call_count: 30,
            decal_depth_target_recreate_count: 1,
            composite_draw_call_count: 6,
            ..RenderMetrics::default()
        };

        metrics.merge(RenderMetrics {
            decal_plan_build_time_us: 5,
            decal_candidate_triangle_count: 7,
            decal_draw_batch_count: 3,
            decal_draw_call_count: 11,
            decal_depth_target_recreate_count: 2,
            composite_draw_call_count: 5,
            ..RenderMetrics::default()
        });

        assert_eq!(metrics.decal_plan_build_time_us, 15);
        assert_eq!(metrics.decal_candidate_triangle_count, 27);
        assert_eq!(metrics.decal_draw_batch_count, 7);
        assert_eq!(metrics.decal_draw_call_count, 41);
        assert_eq!(metrics.decal_depth_target_recreate_count, 3);
        assert_eq!(metrics.composite_draw_call_count, 11);
    }

    #[test]
    fn merge_accumulates_surface_work_and_preserves_peak_target_counts() {
        let mut metrics = RenderMetrics {
            surface_target_surface_count_max: 1,
            surface_target_material_count_max: 1,
            surface_dab_count: 3,
            surface_snapshot_copy_bytes: 4,
            ..RenderMetrics::default()
        };

        metrics.merge(RenderMetrics {
            surface_target_surface_count_max: 2,
            surface_target_material_count_max: 2,
            surface_dab_count: 5,
            surface_snapshot_copy_bytes: 16,
            ..RenderMetrics::default()
        });

        assert_eq!(metrics.surface_target_surface_count_max, 2);
        assert_eq!(metrics.surface_target_material_count_max, 2);
        assert_eq!(metrics.surface_dab_count, 8);
        assert_eq!(metrics.surface_snapshot_copy_bytes, 20);
    }
}
