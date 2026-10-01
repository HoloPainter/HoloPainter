use anyhow::{Result, anyhow};

use crate::core::{
    damage::DamageMap,
    geometry::RectU32,
    image::Rgba8Snapshot,
    surface::PaintSurfaceId,
    tile_cache::{LayerTileCache, TileCacheOpStats},
    tile_payload::{PixelSnapshotData, TilePayload, TiledRgbaSnapshot},
};

use super::{
    SurfaceContentState, SurfacePixelFormat, SurfacePrepareStats, SurfaceRepository,
    rects::{
        clamp_rect_to_texture, merge_touching_rects, rects_intersect, snapshot_rect,
        subtract_rect_from_rects,
    },
};

const TILE_SHADOW_TILE_SIZE: u32 = 256;

#[derive(Debug, Clone)]
pub(crate) struct SurfaceTileShadow {
    pub(crate) cache: LayerTileCache,
    pub(crate) stale_full: bool,
    pub(crate) stale_rects: Vec<RectU32>,
}

impl SurfaceTileShadow {
    pub(super) fn new_transparent(texture_size: [u32; 2]) -> Result<Self> {
        Ok(Self {
            cache: LayerTileCache::new(texture_size, TILE_SHADOW_TILE_SIZE)?,
            stale_full: false,
            stale_rects: Vec::new(),
        })
    }

    pub(super) fn from_full_snapshot(snapshot: &Rgba8Snapshot) -> Result<Self> {
        Ok(Self {
            cache: LayerTileCache::from_full_snapshot(snapshot, TILE_SHADOW_TILE_SIZE)?,
            stale_full: false,
            stale_rects: Vec::new(),
        })
    }

    pub(super) fn is_fully_fresh(&self) -> bool {
        !self.stale_full && self.stale_rects.is_empty()
    }

    pub(super) fn content_state(&self) -> SurfaceContentState {
        if self.is_fully_fresh() && self.cache.tile_count() == 0 {
            SurfaceContentState::Empty
        } else {
            SurfaceContentState::Unknown
        }
    }

    pub(super) fn mark_stale_full(&mut self) {
        self.stale_full = true;
        self.stale_rects.clear();
    }

    pub(super) fn mark_stale_rect(&mut self, rect: RectU32) {
        if self.stale_full {
            return;
        }
        let rect = RectU32 {
            origin: rect.origin,
            size: rect.size,
        };
        if let Some(rect) = clamp_rect_to_texture(self.cache.texture_size(), rect) {
            self.stale_rects.push(rect);
            merge_touching_rects(&mut self.stale_rects);
        }
    }

    pub(super) fn is_rect_fresh(&self, rect: RectU32) -> bool {
        if self.stale_full {
            return false;
        }
        !self
            .stale_rects
            .iter()
            .any(|stale| rects_intersect(*stale, rect))
    }

    pub(super) fn mark_fresh_rect(&mut self, rect: RectU32) {
        if rect.origin == [0, 0] && rect.size == self.cache.texture_size() {
            self.stale_full = false;
            self.stale_rects.clear();
            return;
        }
        if self.stale_full {
            return;
        }
        subtract_rect_from_rects(&mut self.stale_rects, rect);
    }

    pub(super) fn apply_cpu_upload(
        &mut self,
        snapshot: &Rgba8Snapshot,
    ) -> Result<TileCacheOpStats> {
        let stats = self.cache.apply_rgba_snapshot(snapshot)?;
        self.mark_upload_rect_fresh(snapshot_rect(snapshot));
        Ok(stats)
    }

    pub(super) fn apply_pixel_snapshot_data(
        &mut self,
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
        data: &PixelSnapshotData,
    ) -> Result<TileCacheOpStats> {
        let stats = self
            .cache
            .apply_pixel_snapshot_data(texture_size, origin, size, data)?;
        self.mark_upload_rect_fresh(RectU32 { origin, size });
        Ok(stats)
    }

    pub(super) fn apply_rgba8_tile_payload(
        &mut self,
        texture_size: [u32; 2],
        tile: &TilePayload,
    ) -> Result<TileCacheOpStats> {
        let tiled = TiledRgbaSnapshot {
            texture_size,
            tile_size: self.cache.tile_size,
            origin: tile.rect.origin,
            size: tile.rect.size,
            tiles: vec![tile.clone()],
        };
        let stats = self.cache.apply_tiled_snapshot(&tiled)?;
        self.mark_upload_rect_fresh(RectU32 {
            origin: tile.rect.origin,
            size: tile.rect.size,
        });
        Ok(stats)
    }

    fn mark_upload_rect_fresh(&mut self, rect: RectU32) {
        if rect.origin == [0, 0] && rect.size == self.cache.texture_size() {
            self.stale_full = false;
            self.stale_rects.clear();
        } else if !self.stale_full {
            subtract_rect_from_rects(&mut self.stale_rects, rect);
        }
    }

    pub(super) fn read_if_fresh(
        &self,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Option<(Rgba8Snapshot, TileCacheOpStats)>> {
        let rect = RectU32 { origin, size };
        if !self.is_rect_fresh(rect) {
            return Ok(None);
        }
        self.cache.read_rgba_rect(origin, size).map(Some)
    }
}

impl SurfaceRepository {
    pub(crate) fn read_fresh_surface_snapshot(
        &self,
        surface: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Option<(Rgba8Snapshot, TileCacheOpStats)>> {
        let record = self
            .surfaces
            .get(&surface)
            .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", surface))?;
        record.tile_shadow.borrow().read_if_fresh(origin, size)
    }

    pub(crate) fn surface_record_pixel_format(
        &self,
        surface: PaintSurfaceId,
    ) -> Option<SurfacePixelFormat> {
        self.surfaces.get(&surface).map(|record| record.format)
    }

    pub(crate) fn surface_content_state(
        &self,
        surface: PaintSurfaceId,
    ) -> Option<SurfaceContentState> {
        self.surfaces
            .get(&surface)
            .map(|record| record.tile_shadow.borrow().content_state())
    }

    pub(crate) fn mark_surface_tile_cache_stale(
        &mut self,
        surface: PaintSurfaceId,
    ) -> SurfacePrepareStats {
        self.mark_surface_cache_stale(surface)
    }

    pub(crate) fn mark_surface_tile_cache_stale_damage(
        &mut self,
        damage: Option<&DamageMap>,
        fallback_surface: PaintSurfaceId,
    ) -> SurfacePrepareStats {
        self.mark_surface_cache_stale_damage(damage, fallback_surface)
    }
}
