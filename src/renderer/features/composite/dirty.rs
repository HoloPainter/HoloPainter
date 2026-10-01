use crate::{
    core::{
        geometry::RectU32,
        tile::{TileCoord, TileGrid},
    },
    renderer::features::composite::rects::{merge_touching_rects, normalize_rect_to_texture},
};

const DIRTY_TILE_SIZE: u32 = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompositeDirtyRegion {
    pub full: bool,
    pub rects: Vec<RectU32>,
    pub tiles: Vec<TileCoord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirtyRectUpdate {
    pub(crate) tiles_touched: usize,
}

impl CompositeDirtyRegion {
    pub(crate) fn full() -> Self {
        Self {
            full: true,
            ..Self::default()
        }
    }

    pub(crate) fn mark_rect(
        &mut self,
        texture_size: [u32; 2],
        rect: RectU32,
    ) -> Option<DirtyRectUpdate> {
        let rect = normalize_rect_to_texture(texture_size, rect)?;
        if self.full {
            return None;
        }

        let grid = TileGrid::new(texture_size, DIRTY_TILE_SIZE);
        let rect_tiles = grid.tiles_for_rect(rect);
        self.rects.push(rect);
        merge_touching_rects(&mut self.rects);
        self.tiles = tiles_for_rects(&grid, &self.rects);

        Some(DirtyRectUpdate {
            tiles_touched: rect_tiles.len(),
        })
    }
}

fn tiles_for_rects(grid: &TileGrid, rects: &[RectU32]) -> Vec<TileCoord> {
    let mut tiles: Vec<_> = rects
        .iter()
        .flat_map(|rect| grid.tiles_for_rect(*rect))
        .collect();
    tiles.sort_unstable();
    tiles.dedup();
    tiles
}
