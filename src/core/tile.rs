use crate::core::geometry::RectU32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileGrid {
    pub texture_size: [u32; 2],
    pub tile_size: u32,
    pub cols: u32,
    pub rows: u32,
}

impl TileGrid {
    pub fn new(texture_size: [u32; 2], tile_size: u32) -> Self {
        assert!(tile_size > 0, "tile size must be > 0");
        Self {
            texture_size,
            tile_size,
            cols: texture_size[0].div_ceil(tile_size),
            rows: texture_size[1].div_ceil(tile_size),
        }
    }

    pub fn tiles_for_rect(&self, rect: RectU32) -> Vec<TileCoord> {
        if rect.size == [0, 0] || rect.size[0] == 0 || rect.size[1] == 0 {
            return Vec::new();
        }
        let min_x = rect.origin[0].min(self.texture_size[0]);
        let min_y = rect.origin[1].min(self.texture_size[1]);
        let max_x = rect.origin[0]
            .saturating_add(rect.size[0])
            .min(self.texture_size[0]);
        let max_y = rect.origin[1]
            .saturating_add(rect.size[1])
            .min(self.texture_size[1]);
        if min_x >= max_x || min_y >= max_y {
            return Vec::new();
        }

        let first_col = min_x / self.tile_size;
        let last_col = (max_x - 1) / self.tile_size;
        let first_row = min_y / self.tile_size;
        let last_row = (max_y - 1) / self.tile_size;
        let mut tiles = Vec::new();
        for y in first_row..=last_row {
            for x in first_col..=last_col {
                tiles.push(TileCoord { x, y });
            }
        }
        tiles
    }

    pub fn rect_for_tile(&self, tile: TileCoord) -> RectU32 {
        let origin = [
            tile.x
                .saturating_mul(self.tile_size)
                .min(self.texture_size[0]),
            tile.y
                .saturating_mul(self.tile_size)
                .min(self.texture_size[1]),
        ];
        let end = [
            origin[0]
                .saturating_add(self.tile_size)
                .min(self.texture_size[0]),
            origin[1]
                .saturating_add(self.tile_size)
                .min(self.texture_size[1]),
        ];
        RectU32 {
            origin,
            size: [
                end[0].saturating_sub(origin[0]),
                end[1].saturating_sub(origin[1]),
            ],
        }
    }
}
