use crate::core::{damage::DamageMap, surface::PaintSurfaceId};

/// Surface mutations produced by renderer features.
///
/// `Preview` invalidates presentation/composite cache for in-progress stroke
/// feedback and opts into the stroke-specific active-preview compositor path.
/// It may carry a conservative damage footprint; `None` means the preview
/// producer explicitly dirtied the full active surface.
/// `Transient` invalidates presentation/composite cache for a temporary GPU
/// surface edit without opting into stroke-preview checkpoint reuse.
/// `Full` represents a committed full-surface pixel change.
/// `Rects` represents committed pixel changes limited to explicit damage rects.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SurfaceDamage {
    Preview {
        surface: PaintSurfaceId,
        damage: Option<DamageMap>,
    },
    Transient {
        surface: PaintSurfaceId,
        damage: DamageMap,
    },
    Cancelled {
        surface: PaintSurfaceId,
    },
    Full {
        surface: PaintSurfaceId,
    },
    Rects {
        surface: PaintSurfaceId,
        damage: DamageMap,
    },
}

impl SurfaceDamage {
    pub(crate) fn surface(&self) -> PaintSurfaceId {
        match self {
            SurfaceDamage::Preview { surface, .. }
            | SurfaceDamage::Transient { surface, .. }
            | SurfaceDamage::Cancelled { surface }
            | SurfaceDamage::Full { surface }
            | SurfaceDamage::Rects { surface, .. } => *surface,
        }
    }

    pub(crate) fn material_indices(&self) -> Vec<usize> {
        match self {
            SurfaceDamage::Preview {
                surface,
                damage: Some(damage),
            }
            | SurfaceDamage::Transient { surface, damage }
            | SurfaceDamage::Rects { surface, damage }
                if !damage.is_empty() =>
            {
                let mut material_indices = damage
                    .pixels
                    .iter()
                    .map(|pixel| pixel.surface.material_index)
                    .collect::<Vec<_>>();
                if material_indices.is_empty() {
                    material_indices.push(surface.material_index);
                }
                material_indices.sort_unstable();
                material_indices.dedup();
                material_indices
                    .into_iter()
                    .map(|index| index.as_usize())
                    .collect()
            }
            _ => vec![self.surface().material_index().as_usize()],
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct SurfaceDamageSet {
    entries: Vec<SurfaceDamage>,
}

impl SurfaceDamageSet {
    pub(crate) fn preview(&mut self, surface: PaintSurfaceId) {
        self.entries.push(SurfaceDamage::Preview {
            surface,
            damage: None,
        });
    }

    pub(crate) fn preview_damage(&mut self, surface: PaintSurfaceId, damage: DamageMap) {
        if damage.is_empty() {
            return;
        }
        self.entries.push(SurfaceDamage::Preview {
            surface,
            damage: Some(damage),
        });
    }

    pub(crate) fn transient_damage(&mut self, surface: PaintSurfaceId, damage: DamageMap) {
        if damage.is_empty() {
            return;
        }
        self.entries
            .push(SurfaceDamage::Transient { surface, damage });
    }

    pub(crate) fn full(&mut self, surface: PaintSurfaceId) {
        self.entries.push(SurfaceDamage::Full { surface });
    }

    pub(crate) fn cancelled(&mut self, surface: PaintSurfaceId) {
        self.entries.push(SurfaceDamage::Cancelled { surface });
    }

    pub(crate) fn rects(&mut self, surface: PaintSurfaceId, damage: DamageMap) {
        if damage.is_empty() {
            self.full(surface);
        } else {
            self.entries.push(SurfaceDamage::Rects { surface, damage });
        }
    }

    pub(crate) fn from_optional_damage(
        &mut self,
        surface: PaintSurfaceId,
        damage: Option<DamageMap>,
    ) {
        match damage {
            Some(damage) => self.rects(surface, damage),
            None => self.full(surface),
        }
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SurfaceDamage> {
        self.entries.iter()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
