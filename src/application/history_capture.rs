use anyhow::{Result, anyhow};

use crate::{
    application::{
        AppState, DamageMap, PendingPixelEdit, PendingRendererHistoryTransaction,
        PixelSnapshotData, RectU32,
    },
    core::surface::PaintSurfaceId,
};

#[cfg(test)]
use crate::{
    application::{PixelSnapshotStorageKind, choose_pixel_snapshot_storage},
    core::image::Rgba8Snapshot,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PixelHistoryCapture {
    damage: DamageMap,
}

impl PixelHistoryCapture {
    pub(crate) fn from_damage(damage: DamageMap) -> Option<Self> {
        (!damage.is_empty()).then_some(Self { damage })
    }

    pub(crate) fn from_optional_damage(damage: Option<DamageMap>) -> Option<Self> {
        damage.and_then(Self::from_damage)
    }

    pub(crate) fn capture(
        self,
        state: &AppState,
    ) -> Result<Option<PendingRendererHistoryTransaction>> {
        pixel_history_transaction_for_damage(state, self.damage)
    }
}

pub(crate) fn pixel_history_capture_for_full_surfaces(
    state: &AppState,
    surfaces: &[PaintSurfaceId],
) -> Option<PixelHistoryCapture> {
    let document = state.document()?;
    let mut damage = DamageMap::default();
    for surface in surfaces.iter().copied() {
        if let Some(texture_size) = document.texture_size_for_surface(surface) {
            damage.add_full_surface(surface, texture_size);
        }
    }
    PixelHistoryCapture::from_damage(damage)
}

pub(crate) fn pixel_history_transaction_for_damage(
    state: &AppState,
    damages: DamageMap,
) -> Result<Option<PendingRendererHistoryTransaction>> {
    if damages.is_empty() {
        return Ok(None);
    }

    let pending = capture_pending_pixel_edits_for_damage(state, damages)?;
    Ok(
        (!pending.is_empty())
            .then_some(PendingRendererHistoryTransaction::Pixel { edits: pending }),
    )
}

pub(crate) fn read_document_snapshot_data(
    state: &AppState,
    surface: PaintSurfaceId,
    rect: RectU32,
) -> Result<PixelSnapshotData> {
    let document = state
        .document()
        .ok_or_else(|| anyhow!("document is not loaded"))?;
    document.tiles.read_surface_rect(surface, rect)
}

fn capture_pending_pixel_edits_for_damage(
    state: &AppState,
    damages: DamageMap,
) -> Result<Vec<PendingPixelEdit>> {
    let mut pending = Vec::with_capacity(damages.pixels.len());
    for damage in damages.pixels {
        let before = read_document_snapshot_data(state, damage.surface, damage.rect)
            .map_err(|err| anyhow!("Undo document snapshot failed: {err:#}"))?;
        pending.push(PendingPixelEdit {
            surface: damage.surface,
            texture_size: document_texture_size(state, damage.surface)?,
            rect: damage.rect,
            before,
        });
    }
    Ok(pending)
}

fn document_texture_size(state: &AppState, surface: PaintSurfaceId) -> Result<[u32; 2]> {
    state
        .document()
        .and_then(|document| document.texture_size_for_surface(surface))
        .ok_or_else(|| anyhow!("surface texture size is unavailable: {surface:?}"))
}

#[cfg(test)]
pub(crate) fn snapshot_data_from_rgba_snapshot(
    snapshot: &Rgba8Snapshot,
) -> Result<PixelSnapshotData> {
    let rect = RectU32 {
        origin: snapshot.origin,
        size: snapshot.size,
    };
    // TODO: Thread these counts into ApplicationRuntime metrics once history capture
    // can return side-band CPU metrics without changing command reduction.
    match choose_pixel_snapshot_storage(rect, snapshot.rgba8.len()) {
        PixelSnapshotStorageKind::Contiguous => {
            Ok(PixelSnapshotData::from_rgba_snapshot_contiguous(snapshot))
        }
        PixelSnapshotStorageKind::Tiled => PixelSnapshotData::from_rgba_snapshot_tiled(
            snapshot,
            crate::application::PIXEL_HISTORY_TILE_SIZE,
        ),
    }
}
