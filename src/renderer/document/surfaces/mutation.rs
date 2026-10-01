use std::ops::{Deref, DerefMut};

use crate::{
    core::{
        damage::DamageMap, geometry::RectU32, image::Rgba8Snapshot, stroke::PaintSurfaceSet,
        surface::PaintSurfaceId, tile_cache::TileCacheOpStats, tile_payload::TilePayload,
    },
    renderer::{
        document::surfaces::{
            SurfaceInitialPixels, SurfacePrepareStats, repository::SurfaceRepository,
        },
        engine::gpu_state::RendererGpuState,
        gpu::frame::GpuFrame,
        mutation::MutationLog,
    },
};

/// Result of committing a persistent surface mutation.
///
/// GPU passes are responsible for producing pixels. The document surface boundary
/// is responsible for committing those pixels as document damage: CPU tile
/// shadow staleness and the residency metrics produced by those cache updates.
/// Composite dirty state is derived later from
/// the command `MutationLog` by `CompositeFeature`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SurfaceMutationCommit {
    pub(crate) prepare: SurfacePrepareStats,
}

/// Result of uploading CPU pixels into a persistent surface.
///
/// Upload is a document-surface mutation just like a GPU stroke finalization:
/// it refreshes the CPU tile shadow with the uploaded pixels. Composite
/// invalidation is derived from the upload command's `MutationLog`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SurfaceUploadCommit {
    pub(crate) prepare: SurfacePrepareStats,
    pub(crate) tile_cache_update: TileCacheOpStats,
}

pub(crate) struct SurfaceMutationContext<'a> {
    surfaces: &'a mut SurfaceRepository,
    log: &'a mut MutationLog,
}

impl<'a> SurfaceMutationContext<'a> {
    pub(crate) fn new(surfaces: &'a mut SurfaceRepository, log: &'a mut MutationLog) -> Self {
        Self { surfaces, log }
    }

    pub(crate) fn upload_scene_materials_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        materials: &[crate::renderer::document::materials::MaterialUpload],
        surfaces: &[PaintSurfaceId],
    ) -> anyhow::Result<()> {
        self.surfaces
            .upload_scene_materials_into_frame(gpu, frame, materials, surfaces)?;
        for surface in surfaces {
            self.log.surfaces.full(*surface);
        }
        Ok(())
    }

    pub(crate) fn create_surface_texture_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        initial: SurfaceInitialPixels,
    ) -> anyhow::Result<()> {
        self.surfaces
            .create_surface_texture_into_frame(gpu, frame, target, size, initial)?;
        self.log.surfaces.full(target);
        Ok(())
    }

    pub(crate) fn delete_surface_texture(&mut self, target: PaintSurfaceId) {
        self.surfaces.delete_surface_texture(target);
        self.log.surfaces.full(target);
    }

    pub(crate) fn duplicate_surface_texture_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        from: PaintSurfaceId,
        to: PaintSurfaceId,
    ) -> anyhow::Result<()> {
        self.surfaces
            .duplicate_surface_texture_into_frame(gpu, frame, from, to)?;
        self.log.surfaces.full(to);
        Ok(())
    }

    pub(crate) fn replace_surface_from_rgba8_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        rgba8: &[u8],
    ) -> anyhow::Result<()> {
        self.surfaces
            .replace_surface_record_from_rgba8_into_frame(gpu, frame, target, size, rgba8)?;
        self.log.surfaces.full(target);
        Ok(())
    }

    pub(crate) fn upload_rgba8_with_revision(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        snapshot: &Rgba8Snapshot,
        document_revision: Option<u64>,
    ) -> anyhow::Result<SurfaceUploadCommit> {
        let prepare = SurfacePrepareStats::default();
        let tile_cache_update = self.surfaces.upload_surface_rgba8_into_frame(
            gpu,
            frame,
            target,
            snapshot,
            document_revision,
        )?;
        let mut damage = crate::core::damage::DamageMap::default();
        damage.add_rect(
            target,
            crate::core::geometry::RectU32 {
                origin: snapshot.origin,
                size: snapshot.size,
            },
        );
        self.log.surfaces.rects(target, damage);
        Ok(SurfaceUploadCommit {
            prepare,
            tile_cache_update,
        })
    }

    pub(crate) fn upload_rgba8_tiles_with_revision(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        tiles: &[TilePayload],
        document_revision: Option<u64>,
    ) -> anyhow::Result<SurfaceUploadCommit> {
        let prepare = SurfacePrepareStats::default();
        let tile_cache_update = self.surfaces.upload_surface_rgba8_tiles_into_frame(
            gpu,
            frame,
            target,
            tiles,
            document_revision,
        )?;
        if !tiles.is_empty() {
            let mut damage = DamageMap::default();
            for tile in tiles {
                damage.add_rect(target, tile.rect);
            }
            self.log.surfaces.rects(target, damage);
        }
        Ok(SurfaceUploadCommit {
            prepare,
            tile_cache_update,
        })
    }
}

/// Mutation-aware boundary for GPU-produced surface edits.
///
/// Stroke/apply features use this context for write access to persistent
/// document surfaces.  The context keeps texture access and the corresponding
/// `MutationLog` updates in the same command-local object so edit paths do not
/// have to remember to update the log through unrelated helper functions.
pub(crate) struct SurfaceEditContext<'a> {
    surfaces: &'a mut SurfaceRepository,
    log: &'a mut MutationLog,
}

impl<'a> SurfaceEditContext<'a> {
    pub(crate) fn new(surfaces: &'a mut SurfaceRepository, log: &'a mut MutationLog) -> Self {
        Self { surfaces, log }
    }

    pub(crate) fn prepare_edit_surface_for_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
    ) -> anyhow::Result<SurfacePrepareStats> {
        self.surfaces
            .prepare_edit_surface_for_frame(gpu, frame, surface)
    }

    pub(crate) fn prepare_read_surface_for_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        surface: PaintSurfaceId,
    ) -> anyhow::Result<SurfacePrepareStats> {
        self.surfaces
            .prepare_read_surface_for_frame(gpu, frame, surface)
    }

    pub(crate) fn resolve_mask_edit_proxy_into_frame(
        &mut self,
        gpu: &mut RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
    ) -> anyhow::Result<()> {
        self.surfaces
            .resolve_mask_edit_proxy_into_frame(gpu, frame, target)
    }

    pub(crate) fn resolve_mask_edit_proxy_rects_into_frame(
        &mut self,
        gpu: &mut RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        rects: &[RectU32],
    ) -> anyhow::Result<()> {
        self.surfaces
            .resolve_mask_edit_proxy_rects_into_frame(gpu, frame, target, rects)
    }

    pub(crate) fn resolve_mask_edit_proxies_into_frame(
        &mut self,
        gpu: &mut RendererGpuState,
        frame: &mut GpuFrame,
        surfaces: impl IntoIterator<Item = PaintSurfaceId>,
    ) -> anyhow::Result<()> {
        self.surfaces
            .resolve_mask_edit_proxies_into_frame(gpu, frame, surfaces)
    }

    pub(crate) fn record_preview_damage(
        &mut self,
        surfaces: &PaintSurfaceSet,
        damage: Option<DamageMap>,
    ) {
        match damage {
            Some(damage) if !damage.is_empty() => {
                if let Some(surface) = surfaces.first() {
                    self.log.surfaces.preview_damage(surface, damage);
                }
            }
            Some(_) => {}
            None => {
                for surface in surfaces.iter() {
                    self.log.surfaces.preview(surface);
                }
            }
        }
    }

    pub(crate) fn record_cancelled_surfaces(&mut self, surfaces: &PaintSurfaceSet) {
        for surface in surfaces.iter() {
            self.log.surfaces.cancelled(surface);
        }
    }

    pub(crate) fn record_committed_target_damage(
        &mut self,
        target: PaintSurfaceId,
        damage: Option<DamageMap>,
    ) {
        self.log
            .surfaces
            .from_optional_damage(target, damage.clone());
        self.log
            .surface_commits
            .from_optional_damage(target, damage);
    }

    pub(crate) fn record_committed_surface_damage(
        &mut self,
        surfaces: &PaintSurfaceSet,
        damage: Option<DamageMap>,
    ) {
        match damage {
            Some(damage) if !damage.is_empty() => {
                if let Some(surface) = surfaces.first() {
                    self.log.surfaces.rects(surface, damage.clone());
                    self.log.surface_commits.rects(surface, damage);
                }
            }
            _ => {
                for surface in surfaces.iter() {
                    self.log.surfaces.full(surface);
                    self.log.surface_commits.full(surface);
                }
            }
        }
    }
}

impl Deref for SurfaceEditContext<'_> {
    type Target = SurfaceRepository;

    fn deref(&self) -> &Self::Target {
        &*self.surfaces
    }
}

impl DerefMut for SurfaceEditContext<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.surfaces
    }
}
