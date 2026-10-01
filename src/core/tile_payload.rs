use std::{ops::Deref, sync::Arc};

use anyhow::{Result, anyhow, bail};

use crate::core::{
    geometry::RectU32,
    image::Rgba8Snapshot,
    tile::{TileCoord, TileGrid},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelBytes(Arc<[u8]>);

impl PixelBytes {
    pub fn from_vec(rgba8: Vec<u8>) -> Self {
        Self(rgba8.into())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl From<Vec<u8>> for PixelBytes {
    fn from(rgba8: Vec<u8>) -> Self {
        Self::from_vec(rgba8)
    }
}

impl AsRef<[u8]> for PixelBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for PixelBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl PartialEq<Vec<u8>> for PixelBytes {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<PixelBytes> for Vec<u8> {
    fn eq(&self, other: &PixelBytes) -> bool {
        self.as_slice() == other.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilePayload {
    pub coord: TileCoord,
    pub rect: RectU32,
    pub rgba8: PixelBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiledRgbaSnapshot {
    pub texture_size: [u32; 2],
    pub tile_size: u32,
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub tiles: Vec<TilePayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelSnapshotData {
    Contiguous(PixelBytes),
    Tiled(TiledRgbaSnapshot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelSnapshotStorageKind {
    Contiguous,
    Tiled,
}

pub const PIXEL_HISTORY_TILE_SIZE: u32 = 256;
pub const PIXEL_HISTORY_TILED_THRESHOLD_BYTES: usize = 512 * 1024;

impl PixelSnapshotData {
    pub fn contiguous(rgba8: Vec<u8>) -> Self {
        Self::Contiguous(PixelBytes::from_vec(rgba8))
    }

    pub fn from_rgba_snapshot_contiguous(snapshot: &Rgba8Snapshot) -> Self {
        Self::Contiguous(PixelBytes::from_vec(snapshot.rgba8.clone()))
    }

    pub fn from_rgba_snapshot_contiguous_owned(snapshot: Rgba8Snapshot) -> Self {
        Self::contiguous(snapshot.rgba8)
    }

    pub fn from_rgba_snapshot_tiled(snapshot: &Rgba8Snapshot, tile_size: u32) -> Result<Self> {
        Ok(Self::Tiled(split_rgba_rect_into_tiles(
            snapshot, tile_size,
        )?))
    }

    pub fn to_rgba_snapshot(
        &self,
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Rgba8Snapshot> {
        match self {
            PixelSnapshotData::Contiguous(rgba8) => {
                Rgba8Snapshot::new(texture_size, origin, size, rgba8.to_vec())
            }
            PixelSnapshotData::Tiled(tiled) => {
                if tiled.texture_size != texture_size
                    || tiled.origin != origin
                    || tiled.size != size
                {
                    bail!("tiled snapshot metadata does not match requested rect");
                }
                assemble_tiles_to_rgba_rect(tiled)
            }
        }
    }

    pub fn byte_len(&self) -> usize {
        match self {
            PixelSnapshotData::Contiguous(rgba8) => rgba8.len(),
            PixelSnapshotData::Tiled(tiled) => {
                tiled.tiles.iter().map(|tile| tile.rgba8.len()).sum()
            }
        }
    }

    pub fn len(&self) -> usize {
        self.byte_len()
    }

    pub fn is_empty(&self) -> bool {
        self.byte_len() == 0
    }

    pub fn storage_kind(&self) -> PixelSnapshotStorageKind {
        match self {
            PixelSnapshotData::Contiguous(_) => PixelSnapshotStorageKind::Contiguous,
            PixelSnapshotData::Tiled(_) => PixelSnapshotStorageKind::Tiled,
        }
    }

    pub fn to_vec_rgba8(
        &self,
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Vec<u8>> {
        Ok(self.to_rgba_snapshot(texture_size, origin, size)?.rgba8)
    }
}

pub fn validate_pixel_snapshot_data_for_rect(
    data: &PixelSnapshotData,
    texture_size: [u32; 2],
    origin: [u32; 2],
    size: [u32; 2],
) -> Result<()> {
    let rect = RectU32 { origin, size };
    validate_rect_in(texture_size, rect)?;
    match data {
        PixelSnapshotData::Contiguous(rgba8) => validate_rgba_len(size, rgba8.len()),
        PixelSnapshotData::Tiled(tiled) => {
            if tiled.texture_size != texture_size || tiled.origin != origin || tiled.size != size {
                bail!("tiled snapshot metadata does not match requested rect");
            }
            validate_tiled_snapshot(tiled)
        }
    }
}

pub fn crop_pixel_snapshot_data(
    data: &PixelSnapshotData,
    texture_size: [u32; 2],
    source_rect: RectU32,
    target_rect: RectU32,
) -> Result<PixelSnapshotData> {
    validate_pixel_snapshot_data_for_rect(
        data,
        texture_size,
        source_rect.origin,
        source_rect.size,
    )?;
    validate_rect_in(texture_size, target_rect)?;
    if intersect_rects(source_rect, target_rect) != Some(target_rect) {
        bail!("target rect is outside source snapshot rect");
    }
    if source_rect == target_rect {
        return Ok(data.clone());
    }

    let mut rgba8 = vec![0; rgba8_len(target_rect.size)?];
    match data {
        PixelSnapshotData::Contiguous(source) => {
            copy_rect_region_rgba8(source, source_rect, &mut rgba8, target_rect, target_rect)?
        }
        PixelSnapshotData::Tiled(tiled) => {
            for tile in &tiled.tiles {
                let Some(copy_rect) = intersect_rects(tile.rect, target_rect) else {
                    continue;
                };
                copy_rect_region_rgba8(&tile.rgba8, tile.rect, &mut rgba8, target_rect, copy_rect)?;
            }
        }
    }

    let snapshot = Rgba8Snapshot::new(texture_size, target_rect.origin, target_rect.size, rgba8)?;
    match choose_pixel_snapshot_storage(target_rect, snapshot.rgba8.len()) {
        PixelSnapshotStorageKind::Contiguous => Ok(PixelSnapshotData::contiguous(snapshot.rgba8)),
        PixelSnapshotStorageKind::Tiled => {
            PixelSnapshotData::from_rgba_snapshot_tiled(&snapshot, PIXEL_HISTORY_TILE_SIZE)
        }
    }
}

impl PartialEq<Vec<u8>> for PixelSnapshotData {
    fn eq(&self, other: &Vec<u8>) -> bool {
        matches!(self, PixelSnapshotData::Contiguous(rgba8) if rgba8.as_slice() == other.as_slice())
    }
}

pub fn choose_pixel_snapshot_storage(_rect: RectU32, rgba8_len: usize) -> PixelSnapshotStorageKind {
    if rgba8_len >= PIXEL_HISTORY_TILED_THRESHOLD_BYTES {
        PixelSnapshotStorageKind::Tiled
    } else {
        PixelSnapshotStorageKind::Contiguous
    }
}

pub fn split_rgba_rect_into_tiles(
    snapshot: &Rgba8Snapshot,
    tile_size: u32,
) -> Result<TiledRgbaSnapshot> {
    if tile_size == 0 {
        bail!("tile size must be > 0");
    }
    let snapshot_rect = RectU32 {
        origin: snapshot.origin,
        size: snapshot.size,
    };
    validate_rect_in(snapshot.texture_size, snapshot_rect)?;
    validate_rgba_len(snapshot.size, snapshot.rgba8.len())?;

    let grid = TileGrid::new(snapshot.texture_size, tile_size);
    let mut tiles = Vec::new();
    for coord in grid.tiles_for_rect(snapshot_rect) {
        let tile_rect = grid.rect_for_tile(coord);
        let Some(copy_rect) = intersect_rects(snapshot_rect, tile_rect) else {
            continue;
        };
        let mut rgba8 = vec![0; rgba8_len(copy_rect.size)?];
        let dst_rect = RectU32 {
            origin: copy_rect.origin,
            size: copy_rect.size,
        };
        copy_rect_region_rgba8(
            &snapshot.rgba8,
            snapshot_rect,
            &mut rgba8,
            dst_rect,
            copy_rect,
        )?;
        tiles.push(TilePayload {
            coord,
            rect: copy_rect,
            rgba8: PixelBytes::from_vec(rgba8),
        });
    }
    tiles.sort_by_key(|tile| (tile.coord.y, tile.coord.x));

    Ok(TiledRgbaSnapshot {
        texture_size: snapshot.texture_size,
        tile_size,
        origin: snapshot.origin,
        size: snapshot.size,
        tiles,
    })
}

pub fn assemble_tiles_to_rgba_rect(tiled: &TiledRgbaSnapshot) -> Result<Rgba8Snapshot> {
    validate_tiled_snapshot(tiled)?;
    let dst_rect = RectU32 {
        origin: tiled.origin,
        size: tiled.size,
    };
    let mut rgba8 = vec![0; rgba8_len(tiled.size)?];
    for tile in &tiled.tiles {
        copy_rect_region_rgba8(&tile.rgba8, tile.rect, &mut rgba8, dst_rect, tile.rect)?;
    }
    Rgba8Snapshot::new(tiled.texture_size, tiled.origin, tiled.size, rgba8)
}

pub fn intersect_rects(a: RectU32, b: RectU32) -> Option<RectU32> {
    let a_end = rect_end(a).ok()?;
    let b_end = rect_end(b).ok()?;
    let origin = [a.origin[0].max(b.origin[0]), a.origin[1].max(b.origin[1])];
    let end = [a_end[0].min(b_end[0]), a_end[1].min(b_end[1])];
    if origin[0] >= end[0] || origin[1] >= end[1] {
        return None;
    }
    Some(RectU32 {
        origin,
        size: [end[0] - origin[0], end[1] - origin[1]],
    })
}

pub fn copy_rect_region_rgba8(
    src: &[u8],
    src_rect: RectU32,
    dst: &mut [u8],
    dst_rect: RectU32,
    copy_rect: RectU32,
) -> Result<()> {
    validate_rect(src_rect)?;
    validate_rect(dst_rect)?;
    validate_rect(copy_rect)?;
    validate_rgba_len(src_rect.size, src.len())?;
    validate_rgba_len(dst_rect.size, dst.len())?;
    if intersect_rects(src_rect, copy_rect) != Some(copy_rect) {
        bail!("copy rect is outside source rect");
    }
    if intersect_rects(dst_rect, copy_rect) != Some(copy_rect) {
        bail!("copy rect is outside destination rect");
    }

    let src_stride = row_stride(src_rect.size[0])?;
    let dst_stride = row_stride(dst_rect.size[0])?;
    let copy_bytes = row_stride(copy_rect.size[0])?;
    for row in 0..copy_rect.size[1] {
        let src_row = copy_rect.origin[1] - src_rect.origin[1] + row;
        let src_col = copy_rect.origin[0] - src_rect.origin[0];
        let dst_row = copy_rect.origin[1] - dst_rect.origin[1] + row;
        let dst_col = copy_rect.origin[0] - dst_rect.origin[0];
        let src_start = byte_offset(src_row, src_col, src_stride)?;
        let dst_start = byte_offset(dst_row, dst_col, dst_stride)?;
        let src_end = src_start
            .checked_add(copy_bytes)
            .ok_or_else(|| anyhow!("source copy range overflows"))?;
        let dst_end = dst_start
            .checked_add(copy_bytes)
            .ok_or_else(|| anyhow!("destination copy range overflows"))?;
        dst[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
    }
    Ok(())
}

fn validate_tiled_snapshot(tiled: &TiledRgbaSnapshot) -> Result<()> {
    if tiled.tile_size == 0 {
        bail!("tile size must be > 0");
    }
    let snapshot_rect = RectU32 {
        origin: tiled.origin,
        size: tiled.size,
    };
    validate_rect_in(tiled.texture_size, snapshot_rect)?;
    let grid = TileGrid::new(tiled.texture_size, tiled.tile_size);
    let expected = grid.tiles_for_rect(snapshot_rect);
    if expected.len() != tiled.tiles.len() {
        bail!(
            "tiled snapshot has {} tiles, expected {}",
            tiled.tiles.len(),
            expected.len()
        );
    }
    for (index, tile) in tiled.tiles.iter().enumerate() {
        if tile.coord != expected[index] {
            bail!("tile ordering is not deterministic at index {index}");
        }
        validate_rect_in(tiled.texture_size, tile.rect)?;
        validate_rgba_len(tile.rect.size, tile.rgba8.len())?;
        let tile_rect = grid.rect_for_tile(tile.coord);
        let expected_rect = intersect_rects(snapshot_rect, tile_rect)
            .ok_or_else(|| anyhow!("tile does not intersect snapshot rect"))?;
        if tile.rect != expected_rect {
            bail!("tile rect does not match expected clipped tile rect");
        }
    }
    Ok(())
}

fn validate_rect(rect: RectU32) -> Result<()> {
    if rect.size[0] == 0 || rect.size[1] == 0 {
        bail!("rect size must be > 0");
    }
    rect_end(rect)?;
    Ok(())
}

fn validate_rect_in(texture_size: [u32; 2], rect: RectU32) -> Result<()> {
    validate_rect(rect)?;
    let end = rect_end(rect)?;
    if end[0] > texture_size[0] || end[1] > texture_size[1] {
        bail!("rect is outside texture bounds");
    }
    Ok(())
}

fn rect_end(rect: RectU32) -> Result<[u32; 2]> {
    Ok([
        rect.origin[0]
            .checked_add(rect.size[0])
            .ok_or_else(|| anyhow!("rect x range overflows"))?,
        rect.origin[1]
            .checked_add(rect.size[1])
            .ok_or_else(|| anyhow!("rect y range overflows"))?,
    ])
}

fn validate_rgba_len(size: [u32; 2], len: usize) -> Result<()> {
    let expected = rgba8_len(size)?;
    if len != expected {
        bail!("invalid RGBA payload size: got {len}, expected {expected}");
    }
    Ok(())
}

fn rgba8_len(size: [u32; 2]) -> Result<usize> {
    (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("RGBA payload size overflows usize"))
}

fn row_stride(width: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("row stride overflows usize"))
}

fn byte_offset(row: u32, col: u32, stride: usize) -> Result<usize> {
    (row as usize)
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add((col as usize).checked_mul(4)?))
        .ok_or_else(|| anyhow!("byte offset overflows usize"))
}
