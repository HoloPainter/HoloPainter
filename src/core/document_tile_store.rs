use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::core::{
    geometry::RectU32,
    image::LayerImageSnapshot,
    surface::PaintSurfaceId,
    tile_cache::LayerTileCache,
    tile_payload::{
        PIXEL_HISTORY_TILE_SIZE, PixelSnapshotData, TilePayload,
        validate_pixel_snapshot_data_for_rect,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialPixels {
    Transparent,
    SolidRgba8([u8; 4]),
    SparseDefaultRgba8([u8; 4]),
    Rgba8(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedTiles {
    pub surface: PaintSurfaceId,
    pub surface_revision: u64,
    pub rects: Vec<RectU32>,
    pub tiles: Vec<TilePayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSurfaceChanges {
    pub created: Vec<ChangedTiles>,
    pub updated: Vec<ChangedTiles>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceThumbnailMode {
    Rgba,
    MaskRedAsGray,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceThumbnail {
    pub surface: PaintSurfaceId,
    pub surface_revision: u64,
    pub source_size: [u32; 2],
    pub image_size: [usize; 2],
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceTileStore {
    pub pixels: LayerTileCache,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTileStore {
    tile_size: u32,
    surfaces: HashMap<PaintSurfaceId, SurfaceTileStore>,
    revision: u64,
}

impl DocumentTileStore {
    pub fn new(tile_size: u32) -> Result<Self> {
        if tile_size == 0 {
            bail!("tile size must be > 0");
        }
        Ok(Self {
            tile_size,
            surfaces: HashMap::new(),
            revision: 0,
        })
    }

    pub fn with_default_tile_size() -> Self {
        Self::new(PIXEL_HISTORY_TILE_SIZE).expect("default tile size must be valid")
    }

    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn contains_surface(&self, surface: PaintSurfaceId) -> bool {
        self.surfaces.contains_key(&surface)
    }

    pub fn surface_revision(&self, surface: PaintSurfaceId) -> Option<u64> {
        self.surfaces.get(&surface).map(|surface| surface.revision)
    }

    pub fn texture_size(&self, surface: PaintSurfaceId) -> Option<[u32; 2]> {
        self.surfaces
            .get(&surface)
            .map(|surface| surface.pixels.texture_size())
    }

    pub fn surface_tiles(
        &self,
        surface: PaintSurfaceId,
    ) -> Option<impl Iterator<Item = &crate::core::tile_cache::CachedTile>> {
        Some(self.surfaces.get(&surface)?.pixels.tiles())
    }

    pub fn surface_default_rgba8(&self, surface: PaintSurfaceId) -> Option<[u8; 4]> {
        self.surfaces
            .get(&surface)
            .map(|surface| surface.pixels.default_rgba8())
    }

    #[cfg(test)]
    pub(crate) fn cached_tile_count(&self, surface: PaintSurfaceId) -> Option<usize> {
        self.surfaces
            .get(&surface)
            .map(|surface| surface.pixels.tile_count())
    }

    pub fn create_surface(
        &mut self,
        surface: PaintSurfaceId,
        size: [u32; 2],
        initial: InitialPixels,
    ) -> Result<ChangedTiles> {
        if self.surfaces.contains_key(&surface) {
            bail!("document tile surface already exists");
        }
        let default_rgba8 = match &initial {
            InitialPixels::SparseDefaultRgba8(rgba) => *rgba,
            _ => [0; 4],
        };
        let pixels = LayerTileCache::new_with_default(size, self.tile_size, default_rgba8)?;
        self.surfaces.insert(
            surface,
            SurfaceTileStore {
                pixels,
                revision: 0,
            },
        );

        match initial {
            InitialPixels::Transparent => Ok(ChangedTiles {
                surface,
                surface_revision: 0,
                rects: Vec::new(),
                tiles: Vec::new(),
            }),
            InitialPixels::SparseDefaultRgba8(_) => Ok(ChangedTiles {
                surface,
                surface_revision: 0,
                rects: Vec::new(),
                tiles: Vec::new(),
            }),
            InitialPixels::SolidRgba8(rgba) => {
                let pixel_count = (size[0] as usize).saturating_mul(size[1] as usize);
                let mut rgba8 = Vec::with_capacity(pixel_count.saturating_mul(4));
                for _ in 0..pixel_count {
                    rgba8.extend_from_slice(&rgba);
                }
                let rect = RectU32::full(size);
                self.write_surface_rect(surface, rect, &PixelSnapshotData::contiguous(rgba8))
            }
            InitialPixels::Rgba8(rgba8) => {
                let rect = RectU32::full(size);
                self.write_surface_rect(surface, rect, &PixelSnapshotData::contiguous(rgba8))
            }
        }
    }

    pub fn create_surfaces_atomically(
        &mut self,
        surfaces: Vec<(PaintSurfaceId, [u32; 2], InitialPixels)>,
    ) -> Result<Vec<ChangedTiles>> {
        self.change_surfaces_atomically(&[], surfaces)
    }

    pub fn change_surfaces_atomically(
        &mut self,
        deleted_surfaces: &[PaintSurfaceId],
        created_surfaces: Vec<(PaintSurfaceId, [u32; 2], InitialPixels)>,
    ) -> Result<Vec<ChangedTiles>> {
        Ok(self
            .change_surfaces_and_write_atomically(deleted_surfaces, created_surfaces, Vec::new())?
            .created)
    }

    pub fn change_surfaces_and_write_atomically(
        &mut self,
        deleted_surfaces: &[PaintSurfaceId],
        created_surfaces: Vec<(PaintSurfaceId, [u32; 2], InitialPixels)>,
        writes: Vec<(PaintSurfaceId, RectU32, PixelSnapshotData)>,
    ) -> Result<AtomicSurfaceChanges> {
        let mut deleted = HashSet::with_capacity(deleted_surfaces.len());
        for &surface in deleted_surfaces {
            if !self.surfaces.contains_key(&surface) {
                bail!("document tile surface is missing: {surface:?}");
            }
            if !deleted.insert(surface) {
                bail!("document tile surface is duplicated: {surface:?}");
            }
        }
        let mut created_ids = HashSet::with_capacity(created_surfaces.len());
        for (surface, _, _) in &created_surfaces {
            if self.surfaces.contains_key(surface) {
                bail!("document tile surface already exists: {surface:?}");
            }
            if deleted.contains(surface) {
                bail!(
                    "document tile surface cannot be created and deleted atomically: {surface:?}"
                );
            }
            if !created_ids.insert(*surface) {
                bail!("document tile surface creation is duplicated: {surface:?}");
            }
        }
        let mut written = HashSet::with_capacity(writes.len());
        for (surface, _, _) in &writes {
            if deleted.contains(surface) || created_ids.contains(surface) {
                bail!("document tile surface write conflicts with an atomic change: {surface:?}");
            }
            if !self.surfaces.contains_key(surface) {
                bail!("document tile surface is missing: {surface:?}");
            }
            if !written.insert(*surface) {
                bail!("document tile surface write is duplicated: {surface:?}");
            }
        }
        let mut staged = Self {
            tile_size: self.tile_size,
            surfaces: HashMap::with_capacity(created_surfaces.len() + writes.len()),
            revision: self.revision,
        };
        let mut created = Vec::with_capacity(created_surfaces.len());
        for (surface, size, initial) in created_surfaces {
            created.push(staged.create_surface(surface, size, initial)?);
        }
        let mut updated = Vec::with_capacity(writes.len());
        for (surface, rect, pixels) in writes {
            let existing = self
                .surfaces
                .get(&surface)
                .expect("validated written surface must exist")
                .clone();
            staged.surfaces.insert(surface, existing);
            updated.push(staged.write_surface_rect(surface, rect, &pixels)?);
        }
        let revision_increment = u64::try_from(deleted_surfaces.len())
            .map_err(|_| anyhow!("document tile surface count overflows u64"))?;
        let next_revision = staged
            .revision
            .checked_add(revision_increment)
            .ok_or_else(|| anyhow!("document tile store revision overflow"))?;

        for surface in deleted_surfaces {
            self.surfaces
                .remove(surface)
                .expect("validated surface must exist until atomic change commits");
        }
        self.revision = next_revision;
        self.surfaces.extend(staged.surfaces);
        Ok(AtomicSurfaceChanges { created, updated })
    }

    pub fn replace_surfaces_atomically(
        &mut self,
        replacements: Vec<(PaintSurfaceId, [u32; 2], InitialPixels)>,
    ) -> Result<()> {
        let mut replacement_ids = HashSet::with_capacity(replacements.len());
        for (surface, _, _) in &replacements {
            if !self.surfaces.contains_key(surface) {
                bail!("document tile surface is missing: {surface:?}");
            }
            if !replacement_ids.insert(*surface) {
                bail!("document tile surface replacement is duplicated: {surface:?}");
            }
        }

        let mut staged = Self {
            tile_size: self.tile_size,
            surfaces: HashMap::with_capacity(replacements.len()),
            revision: self.revision,
        };
        for (surface, size, initial) in replacements {
            staged.create_surface(surface, size, initial)?;
        }
        let replacement_increment = u64::try_from(staged.surfaces.len())
            .map_err(|_| anyhow!("document tile surface replacement count overflows u64"))?;
        let next_revision = staged
            .revision
            .checked_add(replacement_increment)
            .ok_or_else(|| anyhow!("document tile store revision overflow"))?;

        self.revision = next_revision;
        self.surfaces.extend(staged.surfaces);
        Ok(())
    }

    pub fn delete_surface(&mut self, surface: PaintSurfaceId) -> Result<()> {
        self.surfaces
            .remove(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        self.bump_revision()?;
        Ok(())
    }

    pub fn delete_surfaces_atomically(&mut self, surfaces: &[PaintSurfaceId]) -> Result<()> {
        self.change_surfaces_atomically(surfaces, Vec::new())?;
        Ok(())
    }

    pub fn duplicate_surface(
        &mut self,
        from: PaintSurfaceId,
        to: PaintSurfaceId,
    ) -> Result<ChangedTiles> {
        if self.surfaces.contains_key(&to) {
            bail!("destination document tile surface already exists");
        }
        let source = self
            .surfaces
            .get(&from)
            .ok_or_else(|| anyhow!("source document tile surface is missing"))?;
        let mut cloned = source.clone();
        cloned.revision = cloned.pixels.current_revision();
        self.surfaces.insert(to, cloned);
        self.bump_revision()?;

        let surface = self
            .surfaces
            .get(&to)
            .expect("duplicated surface must exist after insertion");
        Ok(ChangedTiles {
            surface: to,
            surface_revision: surface.revision,
            rects: vec![RectU32::full(surface.pixels.texture_size())],
            tiles: surface.pixels.tile_payloads(),
        })
    }

    pub fn validate_surface_rect_write(
        &self,
        surface: PaintSurfaceId,
        rect: RectU32,
        pixels: &PixelSnapshotData,
    ) -> Result<()> {
        let store = self
            .surfaces
            .get(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        let texture_size = store.pixels.texture_size();
        validate_pixel_snapshot_data_for_rect(pixels, texture_size, rect.origin, rect.size)
    }

    pub fn write_surface_rect(
        &mut self,
        surface: PaintSurfaceId,
        rect: RectU32,
        pixels: &PixelSnapshotData,
    ) -> Result<ChangedTiles> {
        let store = self
            .surfaces
            .get_mut(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        let texture_size = store.pixels.texture_size();
        let previous_revision = store.pixels.current_revision();
        store
            .pixels
            .apply_pixel_snapshot_data(texture_size, rect.origin, rect.size, pixels)?;
        store.revision = store.pixels.current_revision();
        let revision = store.revision;
        let changed_coords = store.pixels.tiles_since_revision(previous_revision);
        let changed_tiles = changed_coords
            .into_iter()
            .filter_map(|coord| store.pixels.cached_tile(coord))
            .map(|tile| TilePayload {
                coord: tile.coord,
                rect: tile.rect,
                rgba8: tile.rgba8.clone(),
            })
            .collect();
        self.bump_revision()?;
        Ok(ChangedTiles {
            surface,
            surface_revision: revision,
            rects: vec![rect],
            tiles: changed_tiles,
        })
    }

    pub fn read_surface_rect(
        &self,
        surface: PaintSurfaceId,
        rect: RectU32,
    ) -> Result<PixelSnapshotData> {
        let store = self
            .surfaces
            .get(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        Ok(store
            .pixels
            .read_pixel_snapshot_data(rect.origin, rect.size)?
            .0)
    }

    pub fn read_surface_full(&self, surface: PaintSurfaceId) -> Result<LayerImageSnapshot> {
        let store = self
            .surfaces
            .get(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        let texture_size = store.pixels.texture_size();
        let rect = RectU32::full(texture_size);
        let rgba8 = self.read_surface_rect(surface, rect)?.to_vec_rgba8(
            texture_size,
            [0, 0],
            texture_size,
        )?;
        Ok(LayerImageSnapshot {
            surface,
            texture_size,
            rgba8,
        })
    }

    pub fn read_surface_thumbnail(
        &self,
        surface: PaintSurfaceId,
        max_extent: u32,
        mode: SurfaceThumbnailMode,
    ) -> Result<SurfaceThumbnail> {
        if max_extent == 0 {
            bail!("thumbnail max extent must be > 0");
        }
        let store = self
            .surfaces
            .get(&surface)
            .ok_or_else(|| anyhow!("document tile surface is missing"))?;
        let image_size = [max_extent as usize, max_extent as usize];
        let texture_size = store.pixels.texture_size();
        let rgba8 = scaled_thumbnail_rgba8(texture_size, image_size, mode, |x, y| {
            store.pixels.sample_rgba8_pixel(x, y)
        })?;
        Ok(SurfaceThumbnail {
            surface,
            surface_revision: store.revision,
            source_size: texture_size,
            image_size,
            rgba8,
        })
    }

    fn bump_revision(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("document tile store revision overflow"))?;
        Ok(())
    }
}

fn scaled_thumbnail_rgba8(
    source_size: [u32; 2],
    image_size: [usize; 2],
    mode: SurfaceThumbnailMode,
    mut sample_pixel: impl FnMut(u32, u32) -> Result<[u8; 4]>,
) -> Result<Vec<u8>> {
    let source_width = source_size[0] as usize;
    let source_height = source_size[1] as usize;

    let image_width = image_size[0];
    let image_height = image_size[1];
    if image_width == 0 || image_height == 0 {
        bail!("thumbnail image size must be > 0");
    }

    let scale =
        (image_width as f32 / source_width as f32).min(image_height as f32 / source_height as f32);
    let target_width = ((source_width as f32 * scale).round() as usize).clamp(1, image_width);
    let target_height = ((source_height as f32 * scale).round() as usize).clamp(1, image_height);
    let offset_x = (image_width - target_width) / 2;
    let offset_y = (image_height - target_height) / 2;

    let output_len = image_width
        .checked_mul(image_height)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
        .ok_or_else(|| anyhow!("thumbnail RGBA payload size overflows usize"))?;
    let mut output = vec![0; output_len];

    for dst_y in 0..target_height {
        let src_y = sample_source_coord(dst_y, target_height, source_height);
        for dst_x in 0..target_width {
            let src_x = sample_source_coord(dst_x, target_width, source_width);
            let pixel = sample_pixel(src_x as u32, src_y as u32)?;
            let dst_index = ((offset_y + dst_y) * image_width + offset_x + dst_x) * 4;
            match mode {
                SurfaceThumbnailMode::Rgba => {
                    output[dst_index..dst_index + 4].copy_from_slice(&pixel);
                }
                SurfaceThumbnailMode::MaskRedAsGray => {
                    let value = pixel[0];
                    output[dst_index..dst_index + 4].copy_from_slice(&[value, value, value, 255]);
                }
            }
        }
    }

    Ok(output)
}

fn sample_source_coord(dst: usize, dst_extent: usize, source_extent: usize) -> usize {
    (((dst as f32 + 0.5) * source_extent as f32 / dst_extent as f32).floor() as usize)
        .min(source_extent.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::surface::LayerId;
    use slotmap::Key as _;

    fn surface() -> PaintSurfaceId {
        PaintSurfaceId::raster(0.into(), LayerId::null())
    }

    fn rgba8(pixels: &[[u8; 4]]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|pixel| pixel.iter().copied())
            .collect()
    }

    fn pixel_at(image: &[u8], image_size: [usize; 2], x: usize, y: usize) -> [u8; 4] {
        let index = (y * image_size[0] + x) * 4;
        image[index..index + 4]
            .try_into()
            .expect("pixel slice must contain RGBA")
    }

    #[test]
    fn atomic_surface_creation_does_not_commit_prepared_surfaces_after_later_failure() {
        let mut store = DocumentTileStore::new(2).unwrap();
        let first = PaintSurfaceId::raster(0.into(), LayerId::null());
        let invalid = PaintSurfaceId::raster(1.into(), LayerId::null());
        let revision_before = store.revision();

        assert!(
            store
                .create_surfaces_atomically(vec![
                    (first, [4, 4], InitialPixels::Transparent),
                    (invalid, [0, 0], InitialPixels::Transparent),
                ])
                .is_err()
        );

        assert!(!store.contains_surface(first));
        assert!(!store.contains_surface(invalid));
        assert_eq!(store.revision(), revision_before);
    }

    #[test]
    fn atomic_surface_deletion_rejects_duplicates_without_mutation() {
        let mut store = DocumentTileStore::new(2).unwrap();
        store
            .create_surface(surface(), [2, 2], InitialPixels::Transparent)
            .unwrap();
        let revision_before = store.revision();

        assert!(
            store
                .delete_surfaces_atomically(&[surface(), surface()])
                .is_err()
        );

        assert!(store.contains_surface(surface()));
        assert_eq!(store.revision(), revision_before);
    }

    #[test]
    fn transparent_surface_thumbnail_is_transparent() {
        let mut store = DocumentTileStore::new(2).unwrap();
        store
            .create_surface(surface(), [2, 2], InitialPixels::Transparent)
            .unwrap();

        let thumbnail = store
            .read_surface_thumbnail(surface(), 4, SurfaceThumbnailMode::Rgba)
            .unwrap();

        assert_eq!(thumbnail.surface, surface());
        assert_eq!(thumbnail.surface_revision, 0);
        assert_eq!(thumbnail.source_size, [2, 2]);
        assert_eq!(thumbnail.image_size, [4, 4]);
        assert!(thumbnail.rgba8.iter().all(|channel| *channel == 0));
    }

    #[test]
    fn raster_thumbnail_preserves_aspect_ratio_with_transparent_letterbox() {
        let mut store = DocumentTileStore::new(2).unwrap();
        let pixels = rgba8(&[
            [10, 20, 30, 255],
            [40, 50, 60, 255],
            [70, 80, 90, 255],
            [100, 110, 120, 255],
            [130, 140, 150, 255],
            [160, 170, 180, 255],
            [190, 200, 210, 255],
            [220, 230, 240, 255],
        ]);
        store
            .create_surface(surface(), [4, 2], InitialPixels::Rgba8(pixels))
            .unwrap();

        let thumbnail = store
            .read_surface_thumbnail(surface(), 4, SurfaceThumbnailMode::Rgba)
            .unwrap();

        assert_eq!(thumbnail.image_size, [4, 4]);
        assert_eq!(
            pixel_at(&thumbnail.rgba8, thumbnail.image_size, 0, 0),
            [0; 4]
        );
        assert_eq!(
            pixel_at(&thumbnail.rgba8, thumbnail.image_size, 0, 1),
            [10, 20, 30, 255]
        );
        assert_eq!(
            pixel_at(&thumbnail.rgba8, thumbnail.image_size, 3, 1),
            [100, 110, 120, 255]
        );
        assert_eq!(
            pixel_at(&thumbnail.rgba8, thumbnail.image_size, 0, 2),
            [130, 140, 150, 255]
        );
        assert_eq!(
            pixel_at(&thumbnail.rgba8, thumbnail.image_size, 0, 3),
            [0; 4]
        );
    }

    #[test]
    fn mask_thumbnail_uses_red_channel_as_opaque_grayscale() {
        let mut store = DocumentTileStore::new(2).unwrap();
        let pixels = rgba8(&[
            [0, 99, 99, 0],
            [64, 1, 2, 3],
            [128, 4, 5, 6],
            [255, 7, 8, 9],
        ]);
        store
            .create_surface(surface(), [2, 2], InitialPixels::Rgba8(pixels))
            .unwrap();

        let thumbnail = store
            .read_surface_thumbnail(surface(), 2, SurfaceThumbnailMode::MaskRedAsGray)
            .unwrap();

        assert_eq!(
            thumbnail.rgba8,
            rgba8(&[
                [0, 0, 0, 255],
                [64, 64, 64, 255],
                [128, 128, 128, 255],
                [255, 255, 255, 255],
            ])
        );
    }
}
