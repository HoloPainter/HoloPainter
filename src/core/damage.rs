use crate::core::{geometry::RectU32, surface::PaintSurfaceId};
use slotmap::Key;

pub const UV_ISLAND_BLEED_RADIUS_PX: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDamage {
    pub surface: PaintSurfaceId,
    pub rect: RectU32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DamageMap {
    pub pixels: Vec<PixelDamage>,
}

impl DamageMap {
    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    pub fn add_rect(&mut self, surface: PaintSurfaceId, rect: RectU32) {
        if rect.size[0] == 0 || rect.size[1] == 0 {
            return;
        }
        self.pixels.push(PixelDamage { surface, rect });
        self.merge_overlaps();
    }

    pub fn add_full_surface(&mut self, surface: PaintSurfaceId, texture_size: [u32; 2]) {
        self.add_rect(surface, RectU32::full(texture_size));
    }

    pub fn merge_overlaps(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            let mut merged: Vec<PixelDamage> = Vec::new();
            'outer: for damage in self.pixels.drain(..) {
                for existing in &mut merged {
                    if existing.surface == damage.surface && rects_touch(existing.rect, damage.rect)
                    {
                        existing.rect = union_rect(existing.rect, damage.rect);
                        changed = true;
                        continue 'outer;
                    }
                }
                merged.push(damage);
            }
            merged.sort_by_key(|d| {
                (
                    d.surface.material_index,
                    d.surface.layer_id.data().as_ffi(),
                    d.rect.origin[1],
                    d.rect.origin[0],
                    d.rect.size[1],
                    d.rect.size[0],
                )
            });
            self.pixels = merged;
        }
    }
}

pub fn rects_touch(a: RectU32, b: RectU32) -> bool {
    let ax1 = a.origin[0].saturating_add(a.size[0]);
    let ay1 = a.origin[1].saturating_add(a.size[1]);
    let bx1 = b.origin[0].saturating_add(b.size[0]);
    let by1 = b.origin[1].saturating_add(b.size[1]);
    a.origin[0] <= bx1 && b.origin[0] <= ax1 && a.origin[1] <= by1 && b.origin[1] <= ay1
}

pub fn union_rect(a: RectU32, b: RectU32) -> RectU32 {
    let x0 = a.origin[0].min(b.origin[0]);
    let y0 = a.origin[1].min(b.origin[1]);
    let x1 = a.origin[0]
        .saturating_add(a.size[0])
        .max(b.origin[0].saturating_add(b.size[0]));
    let y1 = a.origin[1]
        .saturating_add(a.size[1])
        .max(b.origin[1].saturating_add(b.size[1]));
    RectU32 {
        origin: [x0, y0],
        size: [x1.saturating_sub(x0), y1.saturating_sub(y0)],
    }
}
