use std::cell::RefCell;

use anyhow::{Result, anyhow, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        embedded_image::{EmbeddedImageId, EmbeddedImageTransform},
        render_report::RenderMetrics,
        surface::{LayerId, PaintSurfaceId},
    },
    renderer::{
        document::{GpuDocument, surfaces::SurfacePixelFormat},
        engine::gpu_state::RendererGpuState,
        gpu::{create_mask_texture, create_paint_texture, frame::GpuFrame},
        pixel::rgba8_alpha_to_r8,
    },
};

use super::cache::{ActiveCompositeHint, CompositeCache};

/// Resolves composite inputs without making inactive document layers GPU-resident.
///
/// The composite feature treats GPU textures produced here as either the active
/// edit texture, an active-run working-set cache, or a transient source upload.
/// Inactive document surfaces are read from the CPU tile shadow when it is
/// fresh; a resident GPU texture is used only for the active surface, a planned
/// working-set source, or as a safety fallback while a GPU edit is still awaiting
/// CPU readback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompositeSourceResolveMode {
    /// Uploads are allowed while rebuilding active-run checkpoints.
    CheckpointRebuild,
    /// Uploads are forbidden while rendering active stroke-preview composites.
    StrokePreviewNoUpload,
    /// Uploads are allowed for non-stroke/general composite rendering.
    GeneralComposite,
}

pub(super) struct CompositeSourceProvider<'a> {
    document: &'a GpuDocument,
    active_hint: Option<ActiveCompositeHint>,
    working_set: Option<&'a CompositeCache>,
    mode: CompositeSourceResolveMode,
    record_prime_uploads: bool,
    metrics: RefCell<RenderMetrics>,
}

pub(super) enum CompositeSourceTexture<'a> {
    Borrowed {
        view: &'a wgpu::TextureView,
    },
    Uploaded {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
    },
    Derived {
        _texture: std::sync::Arc<wgpu::Texture>,
        view: std::sync::Arc<wgpu::TextureView>,
    },
    Transformed {
        _texture: std::sync::Arc<wgpu::Texture>,
        view: std::sync::Arc<wgpu::TextureView>,
        transform: EmbeddedImageTransform,
    },
}

impl<'a> CompositeSourceTexture<'a> {
    pub(super) fn view(&self) -> &wgpu::TextureView {
        match self {
            Self::Borrowed { view, .. } => view,
            Self::Uploaded { view, .. } => view,
            Self::Derived { view, .. } => view,
            Self::Transformed { view, .. } => view,
        }
    }

    pub(super) fn transformed(&self) -> Option<EmbeddedImageTransform> {
        match self {
            Self::Transformed { transform, .. } => Some(*transform),
            _ => None,
        }
    }
}

impl<'a> CompositeSourceProvider<'a> {
    pub(super) fn new(
        document: &'a GpuDocument,
        active_hint: Option<ActiveCompositeHint>,
        working_set: Option<&'a CompositeCache>,
        mode: CompositeSourceResolveMode,
    ) -> Self {
        Self {
            document,
            active_hint,
            working_set,
            mode,
            record_prime_uploads: false,
            metrics: RefCell::new(RenderMetrics::default()),
        }
    }

    pub(super) fn for_active_prime(
        document: &'a GpuDocument,
        active_hint: Option<ActiveCompositeHint>,
        working_set: Option<&'a CompositeCache>,
        mode: CompositeSourceResolveMode,
    ) -> Self {
        Self {
            document,
            active_hint,
            working_set,
            mode,
            record_prime_uploads: true,
            metrics: RefCell::new(RenderMetrics::default()),
        }
    }

    pub(super) fn take_metrics(&self) -> RenderMetrics {
        std::mem::take(&mut *self.metrics.borrow_mut())
    }

    pub(super) fn resolve(
        &'a self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
        material_index: usize,
        expected_size: [u32; 2],
    ) -> Result<Option<CompositeSourceTexture<'a>>> {
        if surface.material_index.as_usize() != material_index {
            return Ok(None);
        }
        let Some(size) = self.document.surfaces.surface_texture_size(surface) else {
            return Ok(None);
        };
        if size != expected_size {
            return Ok(None);
        }

        if self
            .active_hint
            .as_ref()
            .is_some_and(|hint| hint.contains(surface))
            && let Some(view) = self.document.surfaces.surface_record_texture_view(surface)
        {
            return Ok(Some(CompositeSourceTexture::Borrowed { view }));
        }

        if let Some(active_hint) = self.active_hint.as_ref()
            && let Some(view) = self.working_set.and_then(|cache| {
                cache.active_run_source_cache_view(
                    material_index,
                    active_hint.surfaces(),
                    surface,
                    size,
                )
            })
        {
            return Ok(Some(CompositeSourceTexture::Borrowed { view }));
        }

        if matches!(self.mode, CompositeSourceResolveMode::StrokePreviewNoUpload) {
            let bytes = self
                .document
                .surfaces
                .surface_record_pixel_format(surface)
                .map_or(0, |format| format.saturating_byte_len(size));
            if let Some(cache) = self.working_set {
                cache.record_stroke_composite_upload_attempt(bytes);
            } else {
                let mut metrics = self.metrics.borrow_mut();
                metrics.stroke_composite_upload_attempts =
                    metrics.stroke_composite_upload_attempts.saturating_add(1);
                metrics.stroke_composite_upload_bytes =
                    metrics.stroke_composite_upload_bytes.saturating_add(bytes);
            }
            bail!(
                "stroke preview attempted to upload inactive composite source: {:?}",
                surface
            );
        }

        if let Some(source) = self.upload_fresh_cpu_shadow(gpu, frame, surface, size)? {
            return Ok(Some(source));
        }

        // Compatibility fallback: a just-edited inactive surface can have a stale
        // CPU tile shadow until its commit readback is accepted.  Treat that GPU
        // view as a composite source cache, not as an inactive residency target.
        if let Some(view) = self.document.surfaces.surface_record_texture_view(surface) {
            return Ok(Some(CompositeSourceTexture::Borrowed { view }));
        }

        bail!(
            "composite source is unavailable because CPU shadow is stale and no GPU cache exists: {:?}",
            surface
        );
    }

    pub(super) fn resolve_embedded(
        &'a self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        layer_id: LayerId,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        output_size: [u32; 2],
    ) -> Result<Option<CompositeSourceTexture<'a>>> {
        Ok(self
            .document
            .embedded_images
            .resolve(gpu, frame, layer_id, image_id, transform, output_size)
            .map(|resolved| {
                match resolved {
                crate::renderer::document::embedded_images::ResolvedEmbeddedImage::Derived {
                    texture,
                    view,
                } => CompositeSourceTexture::Derived {
                    _texture: texture,
                    view,
                },
                crate::renderer::document::embedded_images::ResolvedEmbeddedImage::Transformed {
                    texture,
                    view,
                    transform,
                } => CompositeSourceTexture::Transformed {
                    _texture: texture,
                    view,
                    transform,
                },
            }
            }))
    }

    fn upload_fresh_cpu_shadow(
        &self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
        size: [u32; 2],
    ) -> Result<Option<CompositeSourceTexture<'a>>> {
        let Some((snapshot, _stats)) =
            self.document
                .surfaces
                .read_fresh_surface_snapshot(surface, [0, 0], size)?
        else {
            return Ok(None);
        };
        let format = self
            .document
            .surfaces
            .surface_record_pixel_format(surface)
            .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", surface))?;
        let bytes = format.saturating_byte_len(size);
        {
            let mut metrics = self.metrics.borrow_mut();
            metrics.composite_transient_source_uploads =
                metrics.composite_transient_source_uploads.saturating_add(1);
            metrics.composite_transient_source_upload_bytes = metrics
                .composite_transient_source_upload_bytes
                .saturating_add(bytes);
        }
        if matches!(self.mode, CompositeSourceResolveMode::CheckpointRebuild) {
            let mut metrics = self.metrics.borrow_mut();
            metrics.active_composite_checkpoint_rebuild_uploads = metrics
                .active_composite_checkpoint_rebuild_uploads
                .saturating_add(1);
            if self.record_prime_uploads {
                metrics.active_composite_prime_upload_calls = metrics
                    .active_composite_prime_upload_calls
                    .saturating_add(1);
                metrics.active_composite_prime_upload_bytes = metrics
                    .active_composite_prime_upload_bytes
                    .saturating_add(bytes);
            }
        }
        let (texture, view) = match format {
            SurfacePixelFormat::Rgba8 => {
                let (texture, view) = create_paint_texture(
                    gpu.device(),
                    size,
                    "composite_transient_source_upload_rgba8",
                );
                frame.write_texture_rgba8(gpu.device(), &texture, [0, 0], size, &snapshot.rgba8);
                (texture, view)
            }
            SurfacePixelFormat::R8 => {
                let (texture, view) =
                    create_mask_texture(gpu.device(), size, "composite_transient_source_upload_r8");
                let r8 = rgba8_alpha_to_r8(&snapshot.rgba8);
                frame.write_texture_r8(gpu.device(), &texture, [0, 0], size, &r8);
                (texture, view)
            }
        };
        Ok(Some(CompositeSourceTexture::Uploaded {
            _texture: texture,
            view,
        }))
    }
}
