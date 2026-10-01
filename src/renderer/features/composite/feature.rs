use anyhow::{Result, anyhow, bail};
use eframe::egui_wgpu::wgpu;

use super::{
    cache::CompositeCache,
    command::{CompositeBakeTarget, CompositeCommand},
    compositor::MaterialCompositor,
    invalidation::apply_mutation_invalidation,
    outputs::CompositeViewResources,
    pipelines::CompositePipelines,
    prune::{KnownEmptyPruneStats, flatten_identity_pass_through_groups, prune_known_empty},
};

use crate::{
    core::{
        render_report::{GpuTextureMetrics, RenderMetrics},
        stroke::PaintSurfaceSet,
        surface::CompositeTree,
    },
    renderer::{
        document::{GpuDocument, surfaces::SurfaceContentState},
        engine::gpu_state::RendererGpuState,
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        presentation::OutputRequests,
        report::{CommandResult, record_surface_prepare_metrics},
        transient::TransientTextures,
    },
};

/// Target vertical boundary for composite renderer work.
pub(crate) struct CompositeFeature {
    pipelines: CompositePipelines,
    cache: CompositeCache,
    compositor: MaterialCompositor,
}

impl CompositeFeature {
    pub(crate) fn new(device: &wgpu::Device, tex_size: [u32; 2]) -> Self {
        let mut cache = CompositeCache::new();
        cache.ensure_material(device, 0, tex_size);
        Self {
            pipelines: CompositePipelines::new(device),
            cache,
            compositor: MaterialCompositor::default(),
        }
    }

    pub(crate) fn view_resources(&self) -> CompositeViewResources<'_> {
        CompositeViewResources::new(&self.cache)
    }

    pub(crate) fn texture_view(&self, material_index: usize) -> Option<&wgpu::TextureView> {
        self.cache.texture_view(material_index)
    }

    pub(crate) fn texture(&self, material_index: usize) -> Option<&wgpu::Texture> {
        self.cache.texture(material_index)
    }

    pub(crate) fn texture_size(&self, material_index: usize) -> Option<[u32; 2]> {
        self.cache.texture_size(material_index)
    }

    pub(crate) fn ensure_material(
        &mut self,
        device: &wgpu::Device,
        material_index: usize,
        size: [u32; 2],
    ) {
        self.cache.ensure_material(device, material_index, size);
    }

    pub(crate) fn retain_material_count(&mut self, material_count: usize) {
        self.cache.retain_material_count(material_count);
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        self.cache.take_metrics()
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        self.cache.texture_metrics()
    }

    pub(crate) fn invalidate_mutations(&mut self, mutations: &MutationLog) {
        apply_mutation_invalidation(&mut self.cache, mutations);
    }

    pub(crate) fn request_outputs_for_mutations(
        &self,
        document: &GpuDocument,
        mutations: &MutationLog,
        outputs: &mut OutputRequests,
    ) {
        for material_index in mutations.composites.material_indices() {
            outputs.push_material_composite(material_index);
        }

        if mutations.scene.has_any() {
            for material_index in document.composites.material_indices() {
                outputs.push_material_composite(material_index);
            }
            return;
        }

        for damage in mutations.surfaces.iter() {
            for material_index in damage.material_indices() {
                outputs.push_material_composite(material_index);
            }
        }
    }

    pub(crate) fn execute_command(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        command: CompositeCommand,
    ) -> Result<CommandResult> {
        match command {
            CompositeCommand::BakeToSurfaces {
                targets,
                delete_sources_after_bake,
                delete_embedded_images_after_bake,
            } => self.bake_to_surfaces(
                frame,
                gpu_state,
                document,
                scratch,
                targets,
                delete_sources_after_bake,
                delete_embedded_images_after_bake,
            ),
        }
    }

    fn bake_to_surfaces(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        targets: Vec<CompositeBakeTarget>,
        delete_sources_after_bake: Vec<crate::core::surface::PaintSurfaceId>,
        delete_embedded_images_after_bake: Vec<crate::core::embedded_image::EmbeddedImageId>,
    ) -> Result<CommandResult> {
        if targets.is_empty() {
            bail!("composite bake requires at least one target");
        }

        let mut bake_cache = CompositeCache::new();
        let mut metrics = RenderMetrics::default();
        let mut mutations = MutationLog::default();
        for CompositeBakeTarget { target, mut tree } in targets {
            if target.is_mask() {
                bail!("composite bake target must be a raster surface: {target:?}");
            }
            let material_index = target.material_index().as_usize();
            let size = document
                .materials
                .texture_size(material_index)
                .ok_or_else(|| anyhow!("composite bake material is missing: {material_index}"))?;
            let target_size = document
                .surfaces
                .surface_texture_size(target)
                .ok_or_else(|| anyhow!("composite bake target is missing: {target:?}"))?;
            if target_size != size {
                bail!(
                    "composite bake target size mismatch for {target:?}: expected {size:?}, got {target_size:?}"
                );
            }
            let prepare = document
                .surfaces
                .prepare_edit_surface_for_frame(gpu_state, frame, target)?;
            record_surface_prepare_metrics(&mut metrics, &prepare);

            let prune_stats = prune_document_tree(&mut tree, document, None);
            bake_cache.ensure_material(gpu_state.device(), material_index, size);
            let draw_calls_before = self.pipelines.composite_draw_call_count();
            let mut bake_metrics = self.compositor.prepare_material_composite(
                frame,
                gpu_state,
                &self.pipelines,
                document,
                scratch,
                &mut bake_cache,
                material_index,
                &tree,
            )?;
            bake_metrics.composite_draw_call_count =
                bake_metrics.composite_draw_call_count.saturating_add(
                    self.pipelines
                        .composite_draw_call_count()
                        .saturating_sub(draw_calls_before),
                );
            record_prune_metrics(&mut bake_metrics, prune_stats);
            metrics.merge(bake_metrics);

            let source = bake_cache
                .texture(material_index)
                .ok_or_else(|| anyhow!("composite bake output is missing: {material_index}"))?;
            let destination = document
                .surfaces
                .surface_texture(target)
                .ok_or_else(|| anyhow!("composite bake target is not resident: {target:?}"))?;
            frame.encoder().copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: source,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: destination,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
            );
            mutations.surfaces.full(target);
            mutations.surface_commits.full(target);
        }

        for source in delete_sources_after_bake {
            document.surfaces.delete_surface_texture(source);
            mutations.surfaces.full(source);
        }
        for image_id in delete_embedded_images_after_bake {
            document.embedded_images.remove(image_id);
        }

        Ok(CommandResult::new(mutations, metrics))
    }

    pub(crate) fn prepare_material_composite(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        material_index: usize,
    ) -> Result<RenderMetrics> {
        let Some(mut tree) = document.composites.get(material_index).cloned() else {
            return Ok(RenderMetrics::default());
        };
        let active_hint = self.cache.active_hint(material_index);
        let prune_stats = prune_document_tree(&mut tree, document, active_hint.as_ref());
        let draw_calls_before = self.pipelines.composite_draw_call_count();
        let mut metrics = self.compositor.prepare_material_composite(
            frame,
            gpu_state,
            &self.pipelines,
            document,
            scratch,
            &mut self.cache,
            material_index,
            &tree,
        )?;
        metrics.composite_draw_call_count = metrics.composite_draw_call_count.saturating_add(
            self.pipelines
                .composite_draw_call_count()
                .saturating_sub(draw_calls_before),
        );
        record_prune_metrics(&mut metrics, prune_stats);
        Ok(metrics)
    }

    pub(crate) fn prime_active_run_working_set(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &RendererGpuState,
        document: &mut GpuDocument,
        scratch: &mut TransientTextures,
        active_surfaces: PaintSurfaceSet,
    ) -> Result<RenderMetrics> {
        let Some(active_hint) =
            super::cache::ActiveCompositeHint::from_surfaces(active_surfaces.iter())
        else {
            return Ok(RenderMetrics::default());
        };
        let material_index = active_hint.primary_surface().material_index().as_usize();
        let Some(mut tree) = document.composites.get(material_index).cloned() else {
            return Ok(RenderMetrics::default());
        };
        let prune_stats = prune_document_tree(&mut tree, document, Some(&active_hint));
        let draw_calls_before = self.pipelines.composite_draw_call_count();
        let mut metrics = self.compositor.prime_active_run_working_set(
            frame,
            gpu_state,
            &self.pipelines,
            document,
            scratch,
            &mut self.cache,
            material_index,
            active_hint,
            &tree,
        )?;
        metrics.composite_draw_call_count = metrics.composite_draw_call_count.saturating_add(
            self.pipelines
                .composite_draw_call_count()
                .saturating_sub(draw_calls_before),
        );
        record_prune_metrics(&mut metrics, prune_stats);
        Ok(metrics)
    }
}

fn prune_document_tree(
    tree: &mut CompositeTree,
    document: &GpuDocument,
    active_hint: Option<&super::cache::ActiveCompositeHint>,
) -> KnownEmptyPruneStats {
    let stats = prune_known_empty(
        tree,
        &|surface| {
            matches!(
                document.surfaces.surface_content_state(surface),
                Some(SurfaceContentState::Empty)
            )
        },
        &|surface| active_hint.is_some_and(|hint| hint.contains(surface)),
    );
    flatten_identity_pass_through_groups(tree);
    stats
}

fn record_prune_metrics(metrics: &mut RenderMetrics, stats: KnownEmptyPruneStats) {
    metrics.composite_known_empty_raster_skips = metrics
        .composite_known_empty_raster_skips
        .saturating_add(stats.raster_skips);
    metrics.composite_known_zero_mask_skips = metrics
        .composite_known_zero_mask_skips
        .saturating_add(stats.zero_mask_skips);
    metrics.composite_known_empty_group_skips = metrics
        .composite_known_empty_group_skips
        .saturating_add(stats.group_skips);
}
