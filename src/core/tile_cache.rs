use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};

use crate::core::{
    geometry::RectU32,
    image::Rgba8Snapshot,
    tile::{TileCoord, TileGrid},
    tile_payload::{
        PIXEL_HISTORY_TILED_THRESHOLD_BYTES, PixelBytes, PixelSnapshotData, TilePayload,
        TiledRgbaSnapshot, copy_rect_region_rgba8, intersect_rects, split_rgba_rect_into_tiles,
    },
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileCacheOpStats {
    pub tile_count: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTile {
    pub coord: TileCoord,
    pub rect: RectU32,
    pub rgba8: PixelBytes,
    pub revision: u64,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerTileCache {
    pub texture_size: [u32; 2],
    pub tile_size: u32,
    default_rgba8: [u8; 4],
    revision: u64,
    tiles: BTreeMap<TileCoord, CachedTile>,
}

impl LayerTileCache {
    pub fn new(texture_size: [u32; 2], tile_size: u32) -> Result<Self> {
        Self::new_with_default(texture_size, tile_size, [0; 4])
    }

    pub fn new_with_default(
        texture_size: [u32; 2],
        tile_size: u32,
        default_rgba8: [u8; 4],
    ) -> Result<Self> {
        validate_texture_size(texture_size)?;
        if tile_size == 0 {
            bail!("tile size must be > 0");
        }
        Ok(Self {
            texture_size,
            tile_size,
            default_rgba8,
            revision: 0,
            tiles: BTreeMap::new(),
        })
    }

    pub fn from_full_snapshot(snapshot: &Rgba8Snapshot, tile_size: u32) -> Result<Self> {
        if snapshot.origin != [0, 0] || snapshot.size != snapshot.texture_size {
            bail!("full snapshot must cover the whole texture");
        }
        let mut cache = Self::new(snapshot.texture_size, tile_size)?;
        cache.apply_rgba_snapshot(snapshot)?;
        Ok(cache)
    }

    pub fn texture_size(&self) -> [u32; 2] {
        self.texture_size
    }

    pub fn default_rgba8(&self) -> [u8; 4] {
        self.default_rgba8
    }

    pub fn apply_rgba_snapshot(&mut self, snapshot: &Rgba8Snapshot) -> Result<TileCacheOpStats> {
        self.validate_snapshot_metadata(snapshot.texture_size)?;
        let tiled = split_rgba_rect_into_tiles(snapshot, self.tile_size)?;
        self.apply_tiled_snapshot(&tiled)
    }

    pub fn apply_tiled_snapshot(
        &mut self,
        snapshot: &TiledRgbaSnapshot,
    ) -> Result<TileCacheOpStats> {
        self.validate_tiled_snapshot_metadata(snapshot)?;

        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("tile cache revision overflow"))?;
        let revision = self.revision;
        let grid = TileGrid::new(self.texture_size, self.tile_size);
        let mut stats = TileCacheOpStats::default();
        for payload in &snapshot.tiles {
            let tile_rect = grid.rect_for_tile(payload.coord);
            let tile = self
                .tiles
                .entry(payload.coord)
                .or_insert_with(|| CachedTile {
                    coord: payload.coord,
                    rect: tile_rect,
                    rgba8: PixelBytes::from_vec(solid_rgba8(tile_rect.size, self.default_rgba8)),
                    revision: 0,
                    dirty: false,
                });
            if payload.rect == tile.rect {
                tile.rgba8 = payload.rgba8.clone();
            } else {
                let mut rgba8 = tile.rgba8.to_vec();
                copy_rect_region_rgba8(
                    payload.rgba8.as_slice(),
                    payload.rect,
                    &mut rgba8,
                    tile.rect,
                    payload.rect,
                )?;
                tile.rgba8 = PixelBytes::from_vec(rgba8);
            }
            tile.revision = revision;
            tile.dirty = true;
            stats.tile_count += 1;
            stats.bytes = stats.bytes.saturating_add(payload.rgba8.len());
        }
        Ok(stats)
    }

    pub fn read_rgba_rect(
        &self,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<(Rgba8Snapshot, TileCacheOpStats)> {
        let read_rect = RectU32 { origin, size };
        validate_rect(self.texture_size, read_rect)?;
        let mut rgba8 = solid_rgba8(size, self.default_rgba8);
        let mut stats = TileCacheOpStats::default();
        let grid = TileGrid::new(self.texture_size, self.tile_size);
        for coord in grid.tiles_for_rect(read_rect) {
            stats.tile_count += 1;
            let Some(tile) = self.tiles.get(&coord) else {
                continue;
            };
            if let Some(copy_rect) = intersect_rects(read_rect, tile.rect) {
                copy_rect_region_rgba8(&tile.rgba8, tile.rect, &mut rgba8, read_rect, copy_rect)?;
                stats.bytes = stats.bytes.saturating_add(rgba8_len(copy_rect.size)?);
            }
        }
        Ok((
            Rgba8Snapshot::new(self.texture_size, origin, size, rgba8)?,
            stats,
        ))
    }

    pub fn sample_rgba8_pixel(&self, x: u32, y: u32) -> Result<[u8; 4]> {
        if x >= self.texture_size[0] || y >= self.texture_size[1] {
            bail!(
                "pixel coordinate ({x}, {y}) is outside texture bounds {:?}",
                self.texture_size
            );
        }
        self.sample_rgba8_pixel_in_bounds(x, y)
    }

    pub fn dirty_tiles(&self) -> Vec<TileCoord> {
        self.tiles
            .values()
            .filter(|tile| tile.dirty)
            .map(|tile| tile.coord)
            .collect()
    }

    pub fn tiles_since_revision(&self, revision: u64) -> Vec<TileCoord> {
        self.tiles
            .values()
            .filter(|tile| tile.revision > revision)
            .map(|tile| tile.coord)
            .collect()
    }

    pub fn mark_all_clean(&mut self) {
        for tile in self.tiles.values_mut() {
            tile.dirty = false;
        }
    }

    pub fn current_revision(&self) -> u64 {
        self.revision
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn cached_tile(&self, coord: TileCoord) -> Option<&CachedTile> {
        self.tiles.get(&coord)
    }

    pub fn tiles(&self) -> impl Iterator<Item = &CachedTile> {
        self.tiles.values()
    }

    pub fn tile_payloads(&self) -> Vec<TilePayload> {
        self.tiles
            .values()
            .map(|tile| TilePayload {
                coord: tile.coord,
                rect: tile.rect,
                rgba8: tile.rgba8.clone(),
            })
            .collect()
    }

    pub fn to_tiled_snapshot_full(&self) -> Result<TiledRgbaSnapshot> {
        Ok(TiledRgbaSnapshot {
            texture_size: self.texture_size,
            tile_size: self.tile_size,
            origin: [0, 0],
            size: self.texture_size,
            tiles: self.full_tile_payloads()?,
        })
    }

    pub fn apply_pixel_snapshot_data(
        &mut self,
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
        data: &PixelSnapshotData,
    ) -> Result<TileCacheOpStats> {
        match data {
            PixelSnapshotData::Contiguous(rgba8) => {
                let snapshot = Rgba8Snapshot::new(texture_size, origin, size, rgba8.to_vec())?;
                self.apply_rgba_snapshot(&snapshot)
            }
            PixelSnapshotData::Tiled(tiled) => {
                if tiled.texture_size != texture_size
                    || tiled.origin != origin
                    || tiled.size != size
                {
                    bail!("tiled snapshot metadata does not match requested rect");
                }
                self.apply_tiled_snapshot(tiled)
            }
        }
    }

    pub fn read_pixel_snapshot_data(
        &self,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<(PixelSnapshotData, TileCacheOpStats)> {
        if rgba8_len(size)? >= PIXEL_HISTORY_TILED_THRESHOLD_BYTES {
            return self.read_tiled_snapshot_data(origin, size);
        }
        let (snapshot, stats) = self.read_rgba_rect(origin, size)?;
        Ok((PixelSnapshotData::contiguous(snapshot.rgba8), stats))
    }

    fn validate_snapshot_metadata(&self, texture_size: [u32; 2]) -> Result<()> {
        if texture_size != self.texture_size {
            bail!("snapshot texture size does not match cache");
        }
        Ok(())
    }

    fn validate_tiled_snapshot_metadata(&self, snapshot: &TiledRgbaSnapshot) -> Result<()> {
        if snapshot.texture_size != self.texture_size {
            bail!("tiled snapshot texture size does not match cache");
        }
        if snapshot.tile_size != self.tile_size {
            bail!("tiled snapshot tile size does not match cache");
        }
        let snapshot_rect = RectU32 {
            origin: snapshot.origin,
            size: snapshot.size,
        };
        validate_rect(self.texture_size, snapshot_rect)?;
        let grid = TileGrid::new(self.texture_size, self.tile_size);
        let expected = grid.tiles_for_rect(snapshot_rect);
        if expected.len() != snapshot.tiles.len() {
            bail!(
                "tiled snapshot has {} tiles, expected {}",
                snapshot.tiles.len(),
                expected.len()
            );
        }
        for (index, payload) in snapshot.tiles.iter().enumerate() {
            if payload.coord != expected[index] {
                bail!("tile ordering is not deterministic at index {index}");
            }
            validate_rect(self.texture_size, payload.rect)?;
            validate_rgba_len(payload.rect.size, payload.rgba8.len())?;
            let tile_rect = grid.rect_for_tile(payload.coord);
            let expected_rect = intersect_rects(snapshot_rect, tile_rect)
                .ok_or_else(|| anyhow!("tile does not intersect snapshot rect"))?;
            if payload.rect != expected_rect {
                bail!("tile rect does not match expected clipped tile rect");
            }
        }
        Ok(())
    }

    fn full_tile_payloads(&self) -> Result<Vec<TilePayload>> {
        let grid = TileGrid::new(self.texture_size, self.tile_size);
        let full_rect = RectU32 {
            origin: [0, 0],
            size: self.texture_size,
        };
        let mut payloads = Vec::new();
        for coord in grid.tiles_for_rect(full_rect) {
            let rect = grid.rect_for_tile(coord);
            let rgba8 = self
                .tiles
                .get(&coord)
                .map(|tile| tile.rgba8.clone())
                .unwrap_or_else(|| {
                    PixelBytes::from_vec(vec![0; rgba8_len(rect.size).unwrap_or(0)])
                });
            payloads.push(TilePayload { coord, rect, rgba8 });
        }
        Ok(payloads)
    }

    fn sample_rgba8_pixel_in_bounds(&self, x: u32, y: u32) -> Result<[u8; 4]> {
        let coord = TileCoord {
            x: x / self.tile_size,
            y: y / self.tile_size,
        };
        let Some(tile) = self.tiles.get(&coord) else {
            return Ok([0, 0, 0, 0]);
        };

        let local_x = x
            .checked_sub(tile.rect.origin[0])
            .ok_or_else(|| anyhow!("sample x is outside cached tile rect"))?;
        let local_y = y
            .checked_sub(tile.rect.origin[1])
            .ok_or_else(|| anyhow!("sample y is outside cached tile rect"))?;
        if local_x >= tile.rect.size[0] || local_y >= tile.rect.size[1] {
            bail!("sample coordinate is outside cached tile rect");
        }

        let index = (local_y as usize)
            .checked_mul(tile.rect.size[0] as usize)
            .and_then(|row| row.checked_add(local_x as usize))
            .and_then(|pixel| pixel.checked_mul(4))
            .ok_or_else(|| anyhow!("sample pixel index overflows usize"))?;
        let end = index
            .checked_add(4)
            .ok_or_else(|| anyhow!("sample pixel end index overflows usize"))?;
        let pixel = tile
            .rgba8
            .get(index..end)
            .ok_or_else(|| anyhow!("cached tile RGBA payload is truncated"))?;
        Ok(pixel
            .try_into()
            .expect("RGBA pixel slice must have 4 channels"))
    }

    fn read_tiled_snapshot_data(
        &self,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<(PixelSnapshotData, TileCacheOpStats)> {
        let read_rect = RectU32 { origin, size };
        validate_rect(self.texture_size, read_rect)?;
        let grid = TileGrid::new(self.texture_size, self.tile_size);
        let mut stats = TileCacheOpStats::default();
        let mut tiles = Vec::new();
        for coord in grid.tiles_for_rect(read_rect) {
            stats.tile_count += 1;
            let tile_rect = grid.rect_for_tile(coord);
            let Some(payload_rect) = intersect_rects(read_rect, tile_rect) else {
                continue;
            };
            let rgba8 = if let Some(tile) = self.tiles.get(&coord) {
                stats.bytes = stats.bytes.saturating_add(rgba8_len(payload_rect.size)?);
                if payload_rect == tile.rect {
                    tile.rgba8.clone()
                } else {
                    let mut rgba8 = vec![0; rgba8_len(payload_rect.size)?];
                    copy_rect_region_rgba8(
                        &tile.rgba8,
                        tile.rect,
                        &mut rgba8,
                        payload_rect,
                        payload_rect,
                    )?;
                    PixelBytes::from_vec(rgba8)
                }
            } else {
                PixelBytes::from_vec(vec![0; rgba8_len(payload_rect.size)?])
            };
            tiles.push(TilePayload {
                coord,
                rect: payload_rect,
                rgba8,
            });
        }
        Ok((
            PixelSnapshotData::Tiled(TiledRgbaSnapshot {
                texture_size: self.texture_size,
                tile_size: self.tile_size,
                origin,
                size,
                tiles,
            }),
            stats,
        ))
    }
}

fn validate_rgba_len(size: [u32; 2], len: usize) -> Result<()> {
    let expected = rgba8_len(size)?;
    if len != expected {
        bail!("invalid RGBA payload size: got {len}, expected {expected}");
    }
    Ok(())
}

fn validate_texture_size(texture_size: [u32; 2]) -> Result<()> {
    if texture_size[0] == 0 || texture_size[1] == 0 {
        bail!("texture size must be > 0");
    }
    Ok(())
}

fn validate_rect(texture_size: [u32; 2], rect: RectU32) -> Result<()> {
    validate_texture_size(texture_size)?;
    if rect.size[0] == 0 || rect.size[1] == 0 {
        bail!("rect size must be > 0");
    }
    let end_x = rect.origin[0]
        .checked_add(rect.size[0])
        .ok_or_else(|| anyhow!("rect x range overflows"))?;
    let end_y = rect.origin[1]
        .checked_add(rect.size[1])
        .ok_or_else(|| anyhow!("rect y range overflows"))?;
    if end_x > texture_size[0] || end_y > texture_size[1] {
        bail!("rect is outside texture bounds");
    }
    Ok(())
}

fn rgba8_len(size: [u32; 2]) -> Result<usize> {
    (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("RGBA payload size overflows usize"))
}

fn solid_rgba8(size: [u32; 2], rgba: [u8; 4]) -> Vec<u8> {
    let pixel_count = (size[0] as usize).saturating_mul(size[1] as usize);
    let mut output = Vec::with_capacity(pixel_count.saturating_mul(4));
    for _ in 0..pixel_count {
        output.extend_from_slice(&rgba);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba8(pixels: &[[u8; 4]]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|pixel| pixel.iter().copied())
            .collect()
    }

    #[test]
    fn sample_rgba8_pixel_returns_transparent_for_missing_tile() {
        let cache = LayerTileCache::new([4, 4], 2).unwrap();

        assert_eq!(cache.sample_rgba8_pixel(1, 1).unwrap(), [0, 0, 0, 0]);
    }

    #[test]
    fn sample_rgba8_pixel_reads_pixel_from_cached_tile() {
        let mut cache = LayerTileCache::new([4, 4], 2).unwrap();
        let snapshot = Rgba8Snapshot::new(
            [4, 4],
            [0, 0],
            [2, 2],
            rgba8(&[
                [10, 20, 30, 40],
                [50, 60, 70, 80],
                [90, 100, 110, 120],
                [130, 140, 150, 160],
            ]),
        )
        .unwrap();
        cache.apply_rgba_snapshot(&snapshot).unwrap();

        assert_eq!(cache.sample_rgba8_pixel(0, 0).unwrap(), [10, 20, 30, 40]);
        assert_eq!(cache.sample_rgba8_pixel(1, 0).unwrap(), [50, 60, 70, 80]);
        assert_eq!(cache.sample_rgba8_pixel(0, 1).unwrap(), [90, 100, 110, 120]);
        assert_eq!(
            cache.sample_rgba8_pixel(1, 1).unwrap(),
            [130, 140, 150, 160]
        );
    }

    #[test]
    fn sample_rgba8_pixel_rejects_out_of_bounds() {
        let cache = LayerTileCache::new([4, 4], 2).unwrap();

        assert!(cache.sample_rgba8_pixel(4, 0).is_err());
        assert!(cache.sample_rgba8_pixel(0, 4).is_err());
    }

    #[test]
    fn sample_rgba8_pixel_reads_from_clipped_edge_tile() {
        let mut cache = LayerTileCache::new([3, 3], 2).unwrap();
        let snapshot =
            Rgba8Snapshot::new([3, 3], [2, 2], [1, 1], rgba8(&[[200, 201, 202, 203]])).unwrap();
        cache.apply_rgba_snapshot(&snapshot).unwrap();

        assert_eq!(
            cache.sample_rgba8_pixel(2, 2).unwrap(),
            [200, 201, 202, 203]
        );
    }
}
