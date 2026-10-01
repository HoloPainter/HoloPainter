use crate::core::{
    adjustment::Adjustment,
    composite::{GroupCompositeMode, LayerBlendMode},
    document::{ActiveLayerPart, MaterialTextureStateSnapshot},
    geometry::RectU32,
    image::LayerImageSnapshot,
    material::MaterialIndex,
    selection::ActiveSelection,
    surface::{LayerId, LayerMaterialMask, LayerTree, PaintSurfaceId},
};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{PixelSnapshotData, TileCoord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HistoryTransactionId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryTransaction {
    pub id: HistoryTransactionId,
    pub label: String,
    pub atoms: Vec<HistoryAtom>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerTreeSnapshot {
    pub tree: LayerTree,
    pub active_layer_id: LayerId,
    pub active_part: ActiveLayerPart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelEdit {
    pub surface: PaintSurfaceId,
    pub texture_size: [u32; 2],
    pub rect: RectU32,
    pub before: PixelSnapshotData,
    pub after: PixelSnapshotData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionTileEdit {
    pub material_index: MaterialIndex,
    pub texture_size: [u32; 2],
    pub coord: TileCoord,
    pub rect: RectU32,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HistoryAtom {
    PixelEdit {
        edits: Vec<PixelEdit>,
    },
    SelectionEdit {
        before: ActiveSelection,
        after: ActiveSelection,
        tiles: Vec<SelectionTileEdit>,
    },
    LayerPropertyEdit {
        edit: LayerPropertyEdit,
    },
    LayerTreeEdit {
        edit: LayerTreeEdit,
    },
    MaterialTextureResize {
        before: MaterialTextureStateSnapshot,
        after: MaterialTextureStateSnapshot,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerCompositeSettings {
    pub blend_mode: LayerBlendMode,
    pub group_mode: GroupCompositeMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayerPropertyEdit {
    Rename {
        layer_id: LayerId,
        before: String,
        after: String,
    },
    Visibility {
        layer_id: LayerId,
        before: bool,
        after: bool,
    },
    Lock {
        layer_id: LayerId,
        before: bool,
        after: bool,
    },
    LockMany {
        layers: Vec<(LayerId, bool, bool)>,
    },
    MaterialMask {
        layer_id: LayerId,
        before: LayerMaterialMask,
        after: LayerMaterialMask,
    },
    Opacity {
        layer_id: LayerId,
        before: f32,
        after: f32,
    },
    FillColor {
        layer_id: LayerId,
        edit_session: u64,
        before: [f32; 3],
        after: [f32; 3],
    },
    Adjustment {
        layer_id: LayerId,
        edit_session: u64,
        before: Adjustment,
        after: Adjustment,
    },
    EmbeddedImageTransform {
        layer_id: LayerId,
        before: crate::core::embedded_image::EmbeddedImageTransform,
        after: crate::core::embedded_image::EmbeddedImageTransform,
    },
    OpacityMany {
        layers: Vec<(LayerId, f32, f32)>,
    },
    CompositeMode {
        layer_id: LayerId,
        before: LayerCompositeSettings,
        after: LayerCompositeSettings,
    },
    CompositeModeMany {
        layers: Vec<(LayerId, LayerCompositeSettings, LayerCompositeSettings)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerTreeEdit {
    pub before: LayerTreeSnapshot,
    pub after: LayerTreeSnapshot,
    pub before_images: Vec<LayerImageSnapshot>,
    pub after_images: Vec<LayerImageSnapshot>,
    pub retained_embedded_images:
        Vec<std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>>,
}

impl HistoryTransaction {
    pub fn single(label: impl Into<String>, atom: HistoryAtom) -> Self {
        Self::new(HistoryTransactionId::next(), label, vec![atom])
    }
    pub fn new(
        id: HistoryTransactionId,
        label: impl Into<String>,
        atoms: Vec<HistoryAtom>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            atoms,
        }
    }

    #[cfg(test)]
    pub fn test(label: impl Into<String>, atoms: Vec<HistoryAtom>) -> Self {
        Self::new(HistoryTransactionId(0), label, atoms)
    }

    pub fn byte_len(&self) -> usize {
        self.label.len().saturating_add(
            self.atoms
                .iter()
                .map(HistoryAtom::byte_len)
                .fold(0usize, usize::saturating_add),
        )
    }

    pub fn is_noop(&self) -> bool {
        self.atoms.is_empty() || self.atoms.iter().all(HistoryAtom::is_noop)
    }

    pub fn try_merge(&mut self, next: &HistoryTransaction) -> bool {
        if self.label != next.label || self.atoms.len() != 1 || next.atoms.len() != 1 {
            return false;
        }
        self.atoms[0].try_merge(&next.atoms[0])
    }
}

impl HistoryTransactionId {
    pub(crate) fn next() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl HistoryAtom {
    pub fn byte_len(&self) -> usize {
        match self {
            HistoryAtom::PixelEdit { edits } => edits
                .iter()
                .map(|edit| edit.before.byte_len().saturating_add(edit.after.byte_len()))
                .sum(),
            HistoryAtom::SelectionEdit { tiles, .. } => tiles
                .iter()
                .map(|tile| tile.before.len().saturating_add(tile.after.len()))
                .sum(),
            HistoryAtom::LayerPropertyEdit { edit } => match edit {
                LayerPropertyEdit::Rename { before, after, .. } => {
                    before.len().saturating_add(after.len()).saturating_add(32)
                }
                LayerPropertyEdit::Visibility { .. }
                | LayerPropertyEdit::Lock { .. }
                | LayerPropertyEdit::Opacity { .. }
                | LayerPropertyEdit::CompositeMode { .. } => 32,
                LayerPropertyEdit::FillColor { .. } => 48,
                LayerPropertyEdit::EmbeddedImageTransform { .. } => 64,
                LayerPropertyEdit::Adjustment { .. } => std::mem::size_of::<Adjustment>()
                    .saturating_mul(2)
                    .saturating_add(32),
                LayerPropertyEdit::MaterialMask { before, after, .. } => {
                    layer_material_mask_byte_len(before)
                        .saturating_add(layer_material_mask_byte_len(after))
                        .saturating_add(32)
                }
                LayerPropertyEdit::OpacityMany { layers } => layers.len().saturating_mul(32),
                LayerPropertyEdit::LockMany { layers } => layers.len().saturating_mul(24),
                LayerPropertyEdit::CompositeModeMany { layers } => layers.len().saturating_mul(32),
            },
            HistoryAtom::LayerTreeEdit { edit } => approx_layer_tree_snapshot_len(&edit.before)
                .saturating_add(approx_layer_tree_snapshot_len(&edit.after))
                .saturating_add(image_snapshots_len(&edit.before_images))
                .saturating_add(image_snapshots_len(&edit.after_images))
                .saturating_add(
                    edit.retained_embedded_images
                        .iter()
                        .map(|image| image.rgba8.len())
                        .sum::<usize>(),
                ),
            HistoryAtom::MaterialTextureResize { before, after } => {
                material_texture_snapshot_len(before)
                    .saturating_add(material_texture_snapshot_len(after))
            }
        }
    }

    pub fn is_noop(&self) -> bool {
        match self {
            HistoryAtom::PixelEdit { edits } => {
                edits.is_empty()
                    || edits.iter().all(|edit| {
                        edit.before == edit.after
                            || edit.before.byte_len() == 0 && edit.after.byte_len() == 0
                    })
            }
            HistoryAtom::SelectionEdit {
                before,
                after,
                tiles,
            } => before == after && tiles.is_empty(),
            HistoryAtom::LayerPropertyEdit { edit } => edit.is_noop(),
            HistoryAtom::LayerTreeEdit { edit } => {
                edit.before == edit.after
                    && edit.before_images.is_empty()
                    && edit.after_images.is_empty()
            }
            HistoryAtom::MaterialTextureResize { before, after } => before == after,
        }
    }

    pub fn try_merge(&mut self, next: &HistoryAtom) -> bool {
        let HistoryAtom::LayerPropertyEdit { edit } = self else {
            return false;
        };
        let HistoryAtom::LayerPropertyEdit { edit: next } = next else {
            return false;
        };
        edit.try_merge(next)
    }
}

impl LayerPropertyEdit {
    pub fn is_noop(&self) -> bool {
        match self {
            LayerPropertyEdit::Rename { before, after, .. } => before == after,
            LayerPropertyEdit::Visibility { before, after, .. } => before == after,
            LayerPropertyEdit::Lock { before, after, .. } => before == after,
            LayerPropertyEdit::LockMany { layers } => {
                layers.is_empty() || layers.iter().all(|(_, before, after)| before == after)
            }
            LayerPropertyEdit::MaterialMask { before, after, .. } => before == after,
            LayerPropertyEdit::Opacity { before, after, .. } => before == after,
            LayerPropertyEdit::FillColor { before, after, .. } => before == after,
            LayerPropertyEdit::Adjustment { before, after, .. } => before == after,
            LayerPropertyEdit::EmbeddedImageTransform { before, after, .. } => before == after,
            LayerPropertyEdit::OpacityMany { layers } => {
                layers.is_empty() || layers.iter().all(|(_, before, after)| before == after)
            }
            LayerPropertyEdit::CompositeMode { before, after, .. } => before == after,
            LayerPropertyEdit::CompositeModeMany { layers } => {
                layers.is_empty() || layers.iter().all(|(_, before, after)| before == after)
            }
        }
    }

    fn try_merge(&mut self, next: &LayerPropertyEdit) -> bool {
        match (self, next) {
            (
                LayerPropertyEdit::Rename {
                    layer_id, after, ..
                },
                LayerPropertyEdit::Rename {
                    layer_id: next_layer_id,
                    after: next_after,
                    ..
                },
            ) if *layer_id == *next_layer_id => {
                *after = next_after.clone();
                true
            }
            (
                LayerPropertyEdit::Opacity {
                    layer_id, after, ..
                },
                LayerPropertyEdit::Opacity {
                    layer_id: next_layer_id,
                    after: next_after,
                    ..
                },
            ) if *layer_id == *next_layer_id => {
                *after = next_after.clone();
                true
            }
            (
                LayerPropertyEdit::CompositeMode {
                    layer_id, after, ..
                },
                LayerPropertyEdit::CompositeMode {
                    layer_id: next_layer_id,
                    after: next_after,
                    ..
                },
            ) if *layer_id == *next_layer_id => {
                *after = next_after.clone();
                true
            }
            (
                LayerPropertyEdit::FillColor {
                    layer_id,
                    edit_session,
                    after,
                    ..
                },
                LayerPropertyEdit::FillColor {
                    layer_id: next_layer_id,
                    edit_session: next_edit_session,
                    after: next_after,
                    ..
                },
            ) if *layer_id == *next_layer_id && *edit_session == *next_edit_session => {
                *after = *next_after;
                true
            }
            (
                LayerPropertyEdit::Adjustment {
                    layer_id,
                    edit_session,
                    after,
                    ..
                },
                LayerPropertyEdit::Adjustment {
                    layer_id: next_layer_id,
                    edit_session: next_edit_session,
                    after: next_after,
                    ..
                },
            ) if *layer_id == *next_layer_id && *edit_session == *next_edit_session => {
                *after = next_after.clone();
                true
            }
            (
                LayerPropertyEdit::OpacityMany { layers },
                LayerPropertyEdit::OpacityMany {
                    layers: next_layers,
                },
            ) if layers
                .iter()
                .map(|(layer_id, _, _)| *layer_id)
                .eq(next_layers.iter().map(|(layer_id, _, _)| *layer_id)) =>
            {
                for ((_, _, after), (_, _, next_after)) in layers.iter_mut().zip(next_layers) {
                    *after = *next_after;
                }
                true
            }
            (
                LayerPropertyEdit::CompositeModeMany { layers },
                LayerPropertyEdit::CompositeModeMany {
                    layers: next_layers,
                },
            ) if layers
                .iter()
                .map(|(layer_id, _, _)| *layer_id)
                .eq(next_layers.iter().map(|(layer_id, _, _)| *layer_id)) =>
            {
                for ((_, _, after), (_, _, next_after)) in layers.iter_mut().zip(next_layers) {
                    *after = *next_after;
                }
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPixelEdit {
    pub surface: PaintSurfaceId,
    pub texture_size: [u32; 2],
    pub rect: RectU32,
    pub before: PixelSnapshotData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PendingLayerPropertyEdit {
    Rename {
        layer_id: LayerId,
        before: String,
    },
    Visibility {
        layer_id: LayerId,
        before: bool,
    },
    Lock {
        layer_id: LayerId,
        before: bool,
    },
    LockMany {
        layers: Vec<(LayerId, bool)>,
        locked: bool,
    },
    MaterialMask {
        layer_id: LayerId,
        before: LayerMaterialMask,
    },
    Opacity {
        layer_id: LayerId,
        before: f32,
    },
    FillColor {
        layer_id: LayerId,
        edit_session: u64,
        before: [f32; 3],
    },
    Adjustment {
        layer_id: LayerId,
        edit_session: u64,
        before: Adjustment,
    },
    EmbeddedImageTransform {
        layer_id: LayerId,
        before: crate::core::embedded_image::EmbeddedImageTransform,
    },
    OpacityMany {
        layers: Vec<(LayerId, f32)>,
    },
    CompositeMode {
        layer_id: LayerId,
        before: LayerCompositeSettings,
    },
    CompositeModeMany {
        layers: Vec<(LayerId, LayerCompositeSettings)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingRendererHistoryTransaction {
    Pixel {
        edits: Vec<PendingPixelEdit>,
    },
    Selection {
        before: ActiveSelection,
        after: ActiveSelection,
    },
    ImportedLayer {
        before: LayerTreeSnapshot,
        target: PaintSurfaceId,
        added_surfaces: Vec<PaintSurfaceId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingCpuHistoryTransaction {
    LayerProperty {
        before: PendingLayerPropertyEdit,
    },
    LayerTree {
        before: LayerTreeSnapshot,
        before_images: Vec<LayerImageSnapshot>,
        synthesize_transparent_after_images: bool,
        retained_embedded_images:
            Vec<std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>>,
    },
}

impl PendingCpuHistoryTransaction {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::LayerProperty { before } => match before {
                PendingLayerPropertyEdit::Rename { .. } => "Rename Layer",
                PendingLayerPropertyEdit::Visibility { .. } => "Layer Visibility",
                PendingLayerPropertyEdit::Lock { before, .. } => {
                    if *before {
                        "Unlock Layer"
                    } else {
                        "Lock Layer"
                    }
                }
                PendingLayerPropertyEdit::LockMany { locked, .. } => {
                    if *locked {
                        "Lock Layers"
                    } else {
                        "Unlock Layers"
                    }
                }
                PendingLayerPropertyEdit::MaterialMask { .. } => "Material Mask",
                PendingLayerPropertyEdit::Opacity { .. }
                | PendingLayerPropertyEdit::OpacityMany { .. } => "Layer Opacity",
                PendingLayerPropertyEdit::FillColor { .. } => "Fill Layer Color",
                PendingLayerPropertyEdit::Adjustment { .. } => "Edit Adjustment Layer",
                PendingLayerPropertyEdit::EmbeddedImageTransform { .. } => "Transform Image Layer",
                PendingLayerPropertyEdit::CompositeMode { .. }
                | PendingLayerPropertyEdit::CompositeModeMany { .. } => "Layer Composite Mode",
            },
            Self::LayerTree { .. } => "Layer Structure",
        }
    }
}

fn layer_material_mask_byte_len(mask: &LayerMaterialMask) -> usize {
    match mask {
        LayerMaterialMask::Unspecified => 1,
        LayerMaterialMask::Specified(material_ids) => {
            1_usize.saturating_add(material_ids.len().saturating_mul(8))
        }
    }
}

fn material_texture_snapshot_len(snapshot: &MaterialTextureStateSnapshot) -> usize {
    image_snapshots_len(&snapshot.surfaces)
        .saturating_add(
            snapshot
                .selection_mask
                .as_ref()
                .map_or(0, |selection| selection.r8.len()),
        )
        .saturating_add(32)
}

fn image_snapshots_len(images: &[LayerImageSnapshot]) -> usize {
    images
        .iter()
        .map(|image| image.rgba8.len().saturating_add(32))
        .sum()
}

fn approx_layer_tree_snapshot_len(snapshot: &LayerTreeSnapshot) -> usize {
    snapshot
        .tree
        .rows()
        .into_iter()
        .filter_map(|row| snapshot.tree.get(row.layer_id))
        .map(|node| node.props.name.len().saturating_add(96))
        .fold(64usize, usize::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_mask_edits_do_not_merge() {
        let layer_id = LayerTree::new_default_raster()
            .default_raster_layer()
            .unwrap();
        let mut first = HistoryAtom::LayerPropertyEdit {
            edit: LayerPropertyEdit::MaterialMask {
                layer_id,
                before: LayerMaterialMask::Unspecified,
                after: LayerMaterialMask::Specified(Default::default()),
            },
        };
        let next = HistoryAtom::LayerPropertyEdit {
            edit: LayerPropertyEdit::MaterialMask {
                layer_id,
                before: LayerMaterialMask::Specified(Default::default()),
                after: LayerMaterialMask::Unspecified,
            },
        };

        assert!(!first.try_merge(&next));
    }

    #[test]
    fn solid_fill_color_edits_merge_only_within_the_same_picker_session() {
        let layer_id = LayerTree::new_default_raster()
            .default_raster_layer()
            .unwrap();
        let mut first = HistoryAtom::LayerPropertyEdit {
            edit: LayerPropertyEdit::FillColor {
                layer_id,
                edit_session: 4,
                before: [0.0, 0.0, 0.0],
                after: [0.25, 0.5, 0.75],
            },
        };
        let same_session = HistoryAtom::LayerPropertyEdit {
            edit: LayerPropertyEdit::FillColor {
                layer_id,
                edit_session: 4,
                before: [0.25, 0.5, 0.75],
                after: [1.0, 0.5, 0.25],
            },
        };
        let next_session = HistoryAtom::LayerPropertyEdit {
            edit: LayerPropertyEdit::FillColor {
                layer_id,
                edit_session: 5,
                before: [1.0, 0.5, 0.25],
                after: [0.0, 1.0, 0.0],
            },
        };

        assert!(first.try_merge(&same_session));
        assert!(matches!(
            &first,
            HistoryAtom::LayerPropertyEdit {
                edit: LayerPropertyEdit::FillColor {
                    before: [0.0, 0.0, 0.0],
                    after: [1.0, 0.5, 0.25],
                    ..
                }
            }
        ));
        assert!(!first.try_merge(&next_session));
    }

    #[test]
    fn all_history_constructors_share_the_same_id_allocator() {
        let transaction = HistoryTransaction::single(
            "Empty Pixel Edit",
            HistoryAtom::PixelEdit { edits: Vec::new() },
        );
        let next = HistoryTransactionId::next();

        assert_ne!(transaction.id, next);
        assert!(transaction.id.0 < next.0);
    }
}
