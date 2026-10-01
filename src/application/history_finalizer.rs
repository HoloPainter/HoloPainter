use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

use crate::{
    application::{
        LayerCompositeSettings, LayerImageSnapshot, LayerPropertyEdit,
        PendingCpuHistoryTransaction, PendingLayerPropertyEdit, PendingPixelEdit,
        PendingRendererHistoryTransaction, RectU32, SelectionCommit, SelectionTileEdit,
        SurfaceCommit, TileGrid,
    },
    core::{
        surface::PaintSurfaceId,
        tile_payload::{crop_pixel_snapshot_data, intersect_rects},
    },
};

#[cfg(test)]
use crate::application::PixelSnapshotData;

#[cfg(test)]
use super::history_capture::snapshot_data_from_rgba_snapshot;
use super::{AppState, HistoryAtom, PixelEdit};

#[derive(Debug, Default)]
pub(crate) struct HistoryRecordResult {
    pub entry: Option<HistoryAtom>,
    pub pending_transaction: Option<PendingRendererHistoryTransaction>,
}

pub(crate) fn finalize_cpu_history_transaction(
    state: &mut AppState,
    pending: PendingCpuHistoryTransaction,
) -> Result<Option<HistoryAtom>> {
    match pending {
        PendingCpuHistoryTransaction::LayerProperty { before } => {
            Ok(finalize_layer_property_transaction(state, before))
        }
        PendingCpuHistoryTransaction::LayerTree {
            before,
            before_images,
            synthesize_transparent_after_images,
            retained_embedded_images,
        } => finalize_layer_tree_transaction(
            state,
            before,
            before_images,
            synthesize_transparent_after_images,
            retained_embedded_images,
        ),
    }
}

pub(crate) fn finalize_renderer_history_transaction(
    state: &mut AppState,
    pending: PendingRendererHistoryTransaction,
    finalized_surfaces: &[PaintSurfaceId],
    surface_commits: &[SurfaceCommit],
    selection_commits_required: bool,
    _selection_before_commits: &[SelectionCommit],
    selection_after_commits: &[SelectionCommit],
) -> Result<HistoryRecordResult> {
    let entry = match pending {
        PendingRendererHistoryTransaction::Pixel { edits } => {
            if finalized_surfaces.is_empty() {
                let pending = PendingRendererHistoryTransaction::Pixel { edits };
                return Ok(HistoryRecordResult {
                    entry: None,
                    pending_transaction: Some(pending),
                });
            }
            finalize_pixel_transaction(finalized_surfaces, surface_commits, edits)?
        }
        PendingRendererHistoryTransaction::Selection { before, after } => {
            let selection_before_commits = selection_commits_from_document(state, &before)?;
            finalize_selection_transaction(
                before,
                after,
                selection_commits_required,
                &selection_before_commits,
                selection_after_commits,
            )?
        }
        PendingRendererHistoryTransaction::ImportedLayer {
            before,
            target,
            added_surfaces,
        } => {
            if finalized_surfaces.is_empty() {
                return Ok(HistoryRecordResult {
                    entry: None,
                    pending_transaction: Some(PendingRendererHistoryTransaction::ImportedLayer {
                        before,
                        target,
                        added_surfaces,
                    }),
                });
            }
            finalize_imported_layer_transaction(
                state,
                before,
                target,
                added_surfaces,
                finalized_surfaces,
                surface_commits,
            )?
        }
    };
    Ok(HistoryRecordResult {
        entry,
        pending_transaction: None,
    })
}

fn finalize_imported_layer_transaction(
    state: &AppState,
    before: crate::application::LayerTreeSnapshot,
    target: PaintSurfaceId,
    added_surfaces: Vec<PaintSurfaceId>,
    finalized_surfaces: &[PaintSurfaceId],
    surface_commits: &[SurfaceCommit],
) -> Result<Option<HistoryAtom>> {
    if !finalized_surfaces.contains(&target) {
        anyhow::bail!("Renderer did not finalize imported image surface {target:?}");
    }
    let document = state
        .document()
        .ok_or_else(|| anyhow!("Imported image history requires a document"))?;
    let texture_size = document
        .texture_size_for_surface(target)
        .ok_or_else(|| anyhow!("Imported image history target metadata is missing"))?;
    let full_rect = RectU32::full(texture_size);
    let commit = surface_commits
        .iter()
        .find(|commit| commit.surface == target && commit.rect == full_rect)
        .ok_or_else(|| anyhow!("Renderer did not provide a full imported image surface commit"))?;
    let target_rgba8 = commit
        .after
        .to_vec_rgba8(texture_size, [0, 0], texture_size)
        .map_err(|err| anyhow!("Imported image redo snapshot failed: {err:#}"))?;
    let mut after_images = Vec::with_capacity(added_surfaces.len());
    for surface in added_surfaces {
        let surface_size = document
            .texture_size_for_surface(surface)
            .ok_or_else(|| anyhow!("Imported image layer surface metadata is missing"))?;
        let rgba8 = if surface == target {
            target_rgba8.clone()
        } else {
            vec![0; crate::core::image::rgba8_len(surface_size)?]
        };
        after_images.push(LayerImageSnapshot {
            surface,
            texture_size: surface_size,
            rgba8,
        });
    }
    let after = layer_tree_snapshot(state, document);
    let entry = HistoryAtom::LayerTreeEdit {
        edit: crate::application::LayerTreeEdit {
            before,
            after,
            before_images: Vec::new(),
            after_images,
            retained_embedded_images: Vec::new(),
        },
    };
    Ok((!entry.is_noop()).then_some(entry))
}

const SELECTION_HISTORY_TILE_SIZE: u32 = crate::application::PIXEL_HISTORY_TILE_SIZE;

fn selection_commits_from_document(
    state: &AppState,
    selection: &crate::core::selection::ActiveSelection,
) -> Result<Vec<SelectionCommit>> {
    let Some(document) = state.document() else {
        return Ok(Vec::new());
    };
    selection
        .masks
        .iter()
        .filter(|mask| mask.mask_id.is_some())
        .map(|mask| {
            let texture_size = document
                .material(mask.material_index)
                .ok_or_else(|| {
                    anyhow!("selection material does not exist: {}", mask.material_index)
                })?
                .texture_size;
            let r8 = document
                .selection_masks
                .mask_bytes_or_zeros(mask.material_index, texture_size)?;
            Ok(SelectionCommit {
                material_index: mask.material_index,
                texture_size,
                r8,
            })
        })
        .collect()
}

fn finalize_pixel_transaction(
    finalized_surfaces: &[PaintSurfaceId],
    surface_commits: &[SurfaceCommit],
    pending: Vec<PendingPixelEdit>,
) -> Result<Option<HistoryAtom>> {
    if finalized_surfaces.is_empty() || pending.is_empty() {
        return Ok(None);
    }

    let finalized: HashSet<_> = finalized_surfaces.iter().copied().collect();
    let mut edits = Vec::with_capacity(pending.len());
    for pending_edit in pending {
        if !finalized.contains(&pending_edit.surface) {
            anyhow::bail!(
                "Renderer did not finalize pending surface {:?} rect origin {:?} size {:?}",
                pending_edit.surface,
                pending_edit.rect.origin,
                pending_edit.rect.size
            );
        }
        let commit = surface_commits
            .iter()
            .find(|commit| {
                commit.surface == pending_edit.surface
                    && intersect_rects(commit.rect, pending_edit.rect) == Some(pending_edit.rect)
            })
            .ok_or_else(|| {
                anyhow!(
                    "Renderer did not provide surface commit for finalized surface {:?} rect origin {:?} size {:?}",
                    pending_edit.surface,
                    pending_edit.rect.origin,
                    pending_edit.rect.size
                )
            })?;
        let after = crop_pixel_snapshot_data(
            &commit.after,
            pending_edit.texture_size,
            commit.rect,
            pending_edit.rect,
        )
        .map_err(|err| anyhow!("Redo renderer commit snapshot failed: {err:#}"))?;
        edits.push(PixelEdit {
            surface: pending_edit.surface,
            texture_size: pending_edit.texture_size,
            rect: pending_edit.rect,
            before: pending_edit.before,
            after,
        });
    }

    if edits.iter().any(|edit| edit.before != edit.after) {
        return Ok(Some(HistoryAtom::PixelEdit { edits }));
    }

    Ok(None)
}

fn finalize_selection_transaction(
    before: crate::core::selection::ActiveSelection,
    after: crate::core::selection::ActiveSelection,
    selection_commits_required: bool,
    selection_before_commits: &[SelectionCommit],
    selection_after_commits: &[SelectionCommit],
) -> Result<Option<HistoryAtom>> {
    validate_selection_commits(selection_before_commits)
        .map_err(|err| anyhow!("Selection undo snapshot failed: {err:#}"))?;
    validate_selection_commits(selection_after_commits)
        .map_err(|err| anyhow!("Selection redo snapshot failed: {err:#}"))?;
    if selection_commits_required {
        require_selection_commits_for_masks(selection_before_commits, &before, "before")?;
        require_selection_commits_for_masks(selection_after_commits, &after, "after")?;
    }
    let tiles = diff_selection_masks(
        selection_before_commits,
        selection_after_commits,
        &before,
        &after,
    )?;
    let entry = HistoryAtom::SelectionEdit {
        before,
        after,
        tiles,
    };
    Ok((!entry.is_noop()).then_some(entry))
}

pub(crate) fn build_selection_history_atom(
    before: crate::core::selection::ActiveSelection,
    after: crate::core::selection::ActiveSelection,
    before_commits: &[SelectionCommit],
    after_commits: &[SelectionCommit],
) -> Result<Option<HistoryAtom>> {
    finalize_selection_transaction(before, after, true, before_commits, after_commits)
}

fn validate_selection_commits(commits: &[SelectionCommit]) -> Result<()> {
    for commit in commits {
        let len = selection_mask_len(commit.texture_size)?;
        if commit.r8.len() != len {
            return Err(anyhow!(
                "invalid selection mask payload size for material {}: got {}, expected {}",
                commit.material_index,
                commit.r8.len(),
                len
            ));
        }
    }
    Ok(())
}

fn require_selection_commits_for_masks(
    commits: &[SelectionCommit],
    selection: &crate::core::selection::ActiveSelection,
    label: &'static str,
) -> Result<()> {
    for mask in selection.masks.iter().filter(|mask| mask.mask_id.is_some()) {
        if commits
            .iter()
            .any(|commit| commit.material_index == mask.material_index)
        {
            continue;
        }
        return Err(anyhow!(
            "missing {label} selection commit for material {}",
            mask.material_index
        ));
    }
    Ok(())
}

fn diff_selection_masks(
    before_masks: &[SelectionCommit],
    after_masks: &[SelectionCommit],
    _before_selection: &crate::core::selection::ActiveSelection,
    _after_selection: &crate::core::selection::ActiveSelection,
) -> Result<Vec<SelectionTileEdit>> {
    let mut materials = before_masks
        .iter()
        .map(|mask| (mask.material_index, mask.texture_size))
        .collect::<Vec<_>>();
    for mask in after_masks {
        if !materials
            .iter()
            .any(|(material_index, _)| *material_index == mask.material_index)
        {
            materials.push((mask.material_index, mask.texture_size));
        }
    }
    materials.sort_by_key(|(material_index, _)| *material_index);

    let mut tiles = Vec::new();
    for (material_index, texture_size) in materials {
        let before = selection_mask_bytes(before_masks, material_index, texture_size)?;
        let after = selection_mask_bytes(after_masks, material_index, texture_size)?;
        if before == after {
            continue;
        }
        let grid = TileGrid::new(texture_size, SELECTION_HISTORY_TILE_SIZE);
        for coord in grid.tiles_for_rect(RectU32::full(texture_size)) {
            let rect = grid.rect_for_tile(coord);
            let before_tile = copy_selection_rect(&before, texture_size, rect)?;
            let after_tile = copy_selection_rect(&after, texture_size, rect)?;
            if before_tile != after_tile {
                tiles.push(SelectionTileEdit {
                    material_index,
                    texture_size,
                    coord,
                    rect,
                    before: before_tile,
                    after: after_tile,
                });
            }
        }
    }
    Ok(tiles)
}

fn selection_mask_bytes(
    masks: &[SelectionCommit],
    material_index: crate::core::material::MaterialIndex,
    texture_size: [u32; 2],
) -> Result<Vec<u8>> {
    if let Some(mask) = masks
        .iter()
        .find(|mask| mask.material_index == material_index)
    {
        if mask.texture_size != texture_size {
            return Err(anyhow!(
                "selection mask size mismatch for material {}: {:?} vs {:?}",
                material_index,
                mask.texture_size,
                texture_size
            ));
        }
        return Ok(mask.r8.clone());
    }
    Ok(vec![0; selection_mask_len(texture_size)?])
}

fn copy_selection_rect(src: &[u8], texture_size: [u32; 2], rect: RectU32) -> Result<Vec<u8>> {
    let width = texture_size[0] as usize;
    let rect_width = rect.size[0] as usize;
    let rect_height = rect.size[1] as usize;
    let mut out = vec![0; rect_width.saturating_mul(rect_height)];
    for row in 0..rect_height {
        let src_start = (rect.origin[1] as usize + row)
            .checked_mul(width)
            .and_then(|offset| offset.checked_add(rect.origin[0] as usize))
            .ok_or_else(|| anyhow!("selection tile source offset overflows"))?;
        let dst_start = row
            .checked_mul(rect_width)
            .ok_or_else(|| anyhow!("selection tile destination offset overflows"))?;
        out[dst_start..dst_start + rect_width]
            .copy_from_slice(&src[src_start..src_start + rect_width]);
    }
    Ok(out)
}

fn selection_mask_len(size: [u32; 2]) -> Result<usize> {
    (size[0] as usize)
        .checked_mul(size[1] as usize)
        .ok_or_else(|| anyhow!("selection mask size overflows usize"))
}

fn finalize_layer_property_transaction(
    state: &AppState,
    before: PendingLayerPropertyEdit,
) -> Option<HistoryAtom> {
    let document = state.document()?;
    let edit = finalize_layer_property_edit(document, before)?;
    let entry = HistoryAtom::LayerPropertyEdit { edit };
    (!entry.is_noop()).then_some(entry)
}

pub(crate) fn finalize_layer_property_edit(
    document: &crate::core::document::Document,
    before: PendingLayerPropertyEdit,
) -> Option<LayerPropertyEdit> {
    let edit = match before {
        PendingLayerPropertyEdit::Rename { layer_id, before } => {
            let after = document.layer_tree.get(layer_id)?.props.name.clone();
            LayerPropertyEdit::Rename {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::Visibility { layer_id, before } => {
            let after = document.layer_tree.get(layer_id)?.props.visible;
            LayerPropertyEdit::Visibility {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::Lock { layer_id, before } => {
            let after = document.layer_tree.get(layer_id)?.props.locked;
            LayerPropertyEdit::Lock {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::LockMany { layers, .. } => {
            let layers = layers
                .into_iter()
                .filter_map(|(layer_id, before)| {
                    let after = document.layer_tree.get(layer_id)?.props.locked;
                    Some((layer_id, before, after))
                })
                .collect();
            LayerPropertyEdit::LockMany { layers }
        }
        PendingLayerPropertyEdit::MaterialMask { layer_id, before } => {
            let after = document.layer_tree.layer_material_mask(layer_id)?.clone();
            LayerPropertyEdit::MaterialMask {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::Opacity { layer_id, before } => {
            let after = document.layer_tree.get(layer_id)?.props.opacity;
            LayerPropertyEdit::Opacity {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::FillColor {
            layer_id,
            edit_session,
            before,
        } => {
            let after = document.layer_tree.solid_fill_color(layer_id)?;
            LayerPropertyEdit::FillColor {
                layer_id,
                edit_session,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::Adjustment {
            layer_id,
            edit_session,
            before,
        } => {
            let after = document.layer_tree.adjustment(layer_id)?;
            LayerPropertyEdit::Adjustment {
                layer_id,
                edit_session,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::EmbeddedImageTransform { layer_id, before } => {
            let (_, after) = document.layer_tree.embedded_image(layer_id)?;
            LayerPropertyEdit::EmbeddedImageTransform {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::OpacityMany { layers } => {
            let layers = layers
                .into_iter()
                .filter_map(|(layer_id, before)| {
                    let after = document.layer_tree.get(layer_id)?.props.opacity;
                    Some((layer_id, before, after))
                })
                .collect();
            LayerPropertyEdit::OpacityMany { layers }
        }
        PendingLayerPropertyEdit::CompositeMode { layer_id, before } => {
            let props = &document.layer_tree.get(layer_id)?.props;
            let after = LayerCompositeSettings {
                blend_mode: props.blend_mode,
                group_mode: document
                    .layer_tree
                    .group_composite_mode(layer_id)
                    .unwrap_or(crate::core::composite::GroupCompositeMode::Isolated),
            };
            LayerPropertyEdit::CompositeMode {
                layer_id,
                before,
                after,
            }
        }
        PendingLayerPropertyEdit::CompositeModeMany { layers } => {
            let layers = layers
                .into_iter()
                .filter_map(|(layer_id, before)| {
                    let props = &document.layer_tree.get(layer_id)?.props;
                    let after = LayerCompositeSettings {
                        blend_mode: props.blend_mode,
                        group_mode: document
                            .layer_tree
                            .group_composite_mode(layer_id)
                            .unwrap_or(crate::core::composite::GroupCompositeMode::Isolated),
                    };
                    Some((layer_id, before, after))
                })
                .collect();
            LayerPropertyEdit::CompositeModeMany { layers }
        }
    };
    Some(edit)
}

fn finalize_layer_tree_transaction(
    state: &AppState,
    before: crate::application::LayerTreeSnapshot,
    before_images: Vec<LayerImageSnapshot>,
    synthesize_transparent_after_images: bool,
    retained_embedded_images: Vec<std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>>,
) -> Result<Option<HistoryAtom>> {
    let Some(document) = state.document() else {
        return Ok(None);
    };
    let after = layer_tree_snapshot(state, document);
    let before_surfaces = layer_surfaces_for_snapshot(&before, document.materials.len());
    let after_surfaces = layer_surfaces_for_snapshot(&after, document.materials.len());
    let added_surfaces = after_surfaces
        .difference(&before_surfaces)
        .copied()
        .collect::<Vec<_>>();
    let after_images = capture_added_layer_images(
        document,
        added_surfaces,
        synthesize_transparent_after_images,
    )?;
    let before_embedded_ids = embedded_image_ids(&before.tree);
    let after_embedded_ids = embedded_image_ids(&after.tree);
    let mut retained_ids = before_embedded_ids
        .symmetric_difference(&after_embedded_ids)
        .copied()
        .collect::<Vec<_>>();
    retained_ids.sort_unstable();
    let mut candidates = retained_embedded_images
        .into_iter()
        .map(|image| (image.id, image))
        .collect::<HashMap<_, _>>();
    candidates.extend(
        document
            .embedded_images_referenced_by(&after.tree)
            .into_iter()
            .map(|image| (image.id, image)),
    );
    let retained_embedded_images = retained_ids
        .into_iter()
        .map(|id| {
            candidates
                .remove(&id)
                .ok_or_else(|| anyhow!("Layer history is missing embedded image asset {}", id.0))
        })
        .collect::<Result<Vec<_>>>()?;
    let entry = HistoryAtom::LayerTreeEdit {
        edit: crate::application::LayerTreeEdit {
            before,
            after,
            before_images,
            after_images,
            retained_embedded_images,
        },
    };
    Ok((!entry.is_noop()).then_some(entry))
}

fn embedded_image_ids(
    tree: &crate::core::surface::LayerTree,
) -> HashSet<crate::core::embedded_image::EmbeddedImageId> {
    tree.rows()
        .into_iter()
        .filter_map(|row| tree.embedded_image(row.layer_id).map(|value| value.0))
        .collect()
}

fn layer_tree_snapshot(
    state: &AppState,
    document: &crate::core::document::Document,
) -> crate::application::LayerTreeSnapshot {
    let active = state.resolved_editor_document(document);
    crate::application::LayerTreeSnapshot {
        tree: document.layer_tree.clone(),
        active_layer_id: active.active_layer_id,
        active_part: active.active_part,
    }
}

fn capture_added_layer_images(
    document: &crate::core::document::Document,
    added_surfaces: Vec<PaintSurfaceId>,
    synthesize_transparent: bool,
) -> Result<Vec<LayerImageSnapshot>> {
    if added_surfaces.is_empty() {
        return Ok(Vec::new());
    }
    added_surfaces
        .into_iter()
        .map(|surface| match document.tiles.read_surface_full(surface) {
            Ok(image) => Ok(image),
            Err(_err) if synthesize_transparent => {
                let texture_size = document.texture_size_for_surface(surface).ok_or_else(|| {
                    anyhow!("Layer redo snapshot failed: missing surface metadata")
                })?;
                let pixel_count =
                    (texture_size[0] as usize).saturating_mul(texture_size[1] as usize);
                Ok(LayerImageSnapshot {
                    surface,
                    texture_size,
                    rgba8: vec![0; pixel_count.saturating_mul(4)],
                })
            }
            Err(err) => Err(anyhow!("Layer redo snapshot failed: {err:#}")),
        })
        .collect()
}

fn layer_surfaces_for_snapshot(
    snapshot: &crate::application::LayerTreeSnapshot,
    material_count: usize,
) -> HashSet<PaintSurfaceId> {
    snapshot
        .tree
        .layer_surface_owners()
        .into_iter()
        .flat_map(|(layer_id, include_raster)| {
            (0..material_count).flat_map(move |material_index| {
                let mut surfaces = Vec::new();
                if include_raster {
                    surfaces.push(PaintSurfaceId::raster(material_index.into(), layer_id));
                }
                if snapshot.tree.has_layer_mask(layer_id) {
                    surfaces.push(PaintSurfaceId::layer_mask(material_index.into(), layer_id));
                }
                surfaces
            })
        })
        .collect()
}

#[cfg(test)]
fn record_pixel_snapshot_metrics(
    metrics: &mut crate::core::render_report::RenderMetrics,
    data: &PixelSnapshotData,
) {
    if let PixelSnapshotData::Tiled(tiled) = data {
        metrics.tiled_snapshot_count = metrics.tiled_snapshot_count.saturating_add(1);
        metrics.tiled_snapshot_tile_count = metrics
            .tiled_snapshot_tile_count
            .saturating_add(tiled.tiles.len());
        metrics.tiled_snapshot_bytes = metrics.tiled_snapshot_bytes.saturating_add(data.byte_len());
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::{
        application::{
            AppState, ApplicationRuntime, Command, InitialPixels, InputModifiers, PendingPixelEdit,
            PendingRendererHistoryTransaction, PixelSnapshotData, PointerSample,
            RenderCommitArtifacts, RenderMetrics, SelectionCommit, SurfaceCommit, ToolInputEvent,
        },
        core::document::{Document, MaterialSpec, MeshData, MeshId},
        core::image::Rgba8Snapshot,
        core::surface::LayerMaterialMask,
        core::tool::ToolId,
    };

    use super::{
        finalize_renderer_history_transaction, record_pixel_snapshot_metrics,
        snapshot_data_from_rgba_snapshot,
    };

    fn state_with_materials(count: usize) -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::new(
                vec![
                    glam::Vec3::new(0.0, 0.0, 0.0),
                    glam::Vec3::new(1.0, 0.0, 0.0),
                    glam::Vec3::new(0.0, 1.0, 0.0),
                    glam::Vec3::new(1.0, 1.0, 0.0),
                ],
                vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(1.0, 0.0),
                    glam::Vec2::new(0.0, 1.0),
                    glam::Vec2::new(1.0, 1.0),
                ],
                vec![glam::Vec3::Z; 4],
                vec![[0, 1, 2], [1, 3, 2]],
                vec![
                    crate::core::document::SubMesh {
                        mesh_id: MeshId(0),
                        start_index: 0,
                        index_count: 3,
                        material_index: 0,
                        material_name: "M0".to_owned(),
                        wireframe_edges: Vec::new(),
                    },
                    crate::core::document::SubMesh {
                        mesh_id: MeshId(0),
                        start_index: 3,
                        index_count: 3,
                        material_index: 1,
                        material_name: "M1".to_owned(),
                        wireframe_edges: Vec::new(),
                    },
                ],
                vec![crate::core::document::MeshObject {
                    id: MeshId(0),
                    name: "Mesh".to_owned(),
                }],
                vec![MeshId(0), MeshId(0)],
            )
            .unwrap(),
            (0..count)
                .map(|index| MaterialSpec::new(format!("M{index}"), [2, 2]))
                .collect(),
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        state
    }

    fn pending_pixel_edits(transaction: &PendingRendererHistoryTransaction) -> &[PendingPixelEdit] {
        match transaction {
            PendingRendererHistoryTransaction::Pixel { edits } => edits,
            _ => &[],
        }
    }

    fn surface_commit(
        surface: crate::core::surface::PaintSurfaceId,
        texture_size: [u32; 2],
        rect: crate::core::geometry::RectU32,
        value: u8,
    ) -> SurfaceCommit {
        let snapshot = Rgba8Snapshot::new(
            texture_size,
            rect.origin,
            rect.size,
            vec![value; rect.size[0] as usize * rect.size[1] as usize * 4],
        )
        .unwrap();
        SurfaceCommit {
            surface,
            rect,
            after: snapshot_data_from_rgba_snapshot(&snapshot).unwrap(),
        }
    }

    fn commits_for_pending_pixels(
        transaction: &PendingRendererHistoryTransaction,
        value: u8,
    ) -> Vec<SurfaceCommit> {
        pending_pixel_edits(transaction)
            .iter()
            .map(|edit| surface_commit(edit.surface, edit.texture_size, edit.rect, value))
            .collect()
    }

    fn after_effects_with_commits(
        state: &mut AppState,
        pending: PendingRendererHistoryTransaction,
        finalized_surfaces: &[crate::core::surface::PaintSurfaceId],
        surface_commits: &[SurfaceCommit],
        selection_before_commits: &[SelectionCommit],
        selection_after_commits: &[SelectionCommit],
    ) -> Result<super::HistoryRecordResult> {
        let prepared = crate::application::commit_finalizer::prepare_finalized_surface_commits(
            state,
            finalized_surfaces,
            surface_commits,
            &[&pending],
        )?;
        let result = finalize_renderer_history_transaction(
            state,
            pending,
            finalized_surfaces,
            surface_commits,
            false,
            selection_before_commits,
            selection_after_commits,
        )?;
        crate::application::commit_finalizer::apply_prepared_surface_commits_to_document(
            state, &prepared,
        )?;
        Ok(result)
    }

    fn selection_commit(
        material_index: usize,
        texture_size: [u32; 2],
        value: u8,
    ) -> SelectionCommit {
        SelectionCommit {
            material_index: material_index.into(),
            texture_size,
            r8: vec![value; texture_size[0] as usize * texture_size[1] as usize],
        }
    }

    fn state_with_material_size(size: [u32; 2]) -> AppState {
        let mut state = state_with_materials(1);
        let surface = state.active_paint_surface().unwrap();
        let document = state.document.document.as_mut().unwrap();
        document.materials[0].texture_size = size;
        document.tiles.delete_surface(surface).unwrap();
        document
            .tiles
            .create_surface(surface, size, InitialPixels::Transparent)
            .unwrap();
        document.active_selection =
            crate::core::selection::ActiveSelection::disabled_for_materials([0.into()]);
        state
    }

    #[test]
    fn after_effects_returns_history_only_for_changed_pixels() {
        let mut state = state_with_materials(1);
        let target = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [2, 2],
                rect: crate::core::geometry::RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![1; 16]),
            }],
        };
        let commits = commits_for_pending_pixels(&pending, 2);
        let result =
            after_effects_with_commits(&mut state, pending, &[target], &commits, &[], &[]).unwrap();

        assert!(matches!(
            result.entry,
            Some(crate::application::HistoryEntry::PixelEdit { edits })
                if edits.len() == 1 && edits[0].before == vec![1; 16] && edits[0].after == vec![2; 16]
        ));
        assert!(result.pending_transaction.is_none());
    }

    #[test]
    fn after_effects_uses_surface_commit_after_without_document_read() {
        let mut state = state_with_materials(1);
        let target = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [2, 2],
                rect: crate::core::geometry::RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![0; 16]),
            }],
        };
        let commits = commits_for_pending_pixels(&pending, 9);
        let result = finalize_renderer_history_transaction(
            &mut state,
            pending,
            &[target],
            &commits,
            false,
            &[],
            &[],
        )
        .unwrap();

        assert!(matches!(
            result.entry,
            Some(crate::application::HistoryEntry::PixelEdit { edits })
                if edits.len() == 1 && edits[0].after == vec![9; 16]
        ));
        assert_eq!(
            state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(target)
                .unwrap()
                .rgba8,
            vec![0; 16]
        );
    }

    #[test]
    fn after_effects_commits_renderer_pixels_to_document_tiles() {
        let mut state = state_with_materials(1);
        let target = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [2, 2],
                rect: crate::core::geometry::RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![0; 16]),
            }],
        };
        let commits = commits_for_pending_pixels(&pending, 6);
        let result =
            after_effects_with_commits(&mut state, pending, &[target], &commits, &[], &[]).unwrap();

        assert!(result.entry.is_some());
        assert_eq!(
            state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(target)
                .unwrap()
                .rgba8,
            vec![6; 16]
        );
    }

    #[test]
    fn after_effects_crops_covering_surface_commit_to_pending_rect() {
        let mut state = state_with_material_size([4, 4]);
        let target = state.active_paint_surface().unwrap();
        let pending_rect = crate::core::geometry::RectU32 {
            origin: [1, 1],
            size: [2, 2],
        };
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [4, 4],
                rect: pending_rect,
                before: PixelSnapshotData::contiguous(vec![0; 16]),
            }],
        };
        let commits = vec![surface_commit(
            target,
            [4, 4],
            crate::core::geometry::RectU32::full([4, 4]),
            7,
        )];
        let result =
            after_effects_with_commits(&mut state, pending, &[target], &commits, &[], &[]).unwrap();

        assert!(matches!(
            result.entry,
            Some(crate::application::HistoryEntry::PixelEdit { edits })
                if edits.len() == 1
                    && edits[0].rect == pending_rect
                    && edits[0].after == vec![7; 16]
        ));
        assert_eq!(
            state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_rect(target, pending_rect)
                .unwrap(),
            PixelSnapshotData::contiguous(vec![7; 16])
        );
    }

    #[test]
    fn after_effects_errors_when_finalized_surface_commit_is_missing() {
        let mut state = state_with_materials(1);
        let target = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [2, 2],
                rect: crate::core::geometry::RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![1; 16]),
            }],
        };
        let error =
            after_effects_with_commits(&mut state, pending, &[target], &[], &[], &[]).unwrap_err();

        assert!(format!("{error:#}").contains("Renderer did not provide surface commit"));
    }

    #[test]
    fn after_effects_keeps_pending_pixels_until_stroke_finalizes() {
        let mut state = state_with_materials(1);
        let target = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![crate::application::PendingPixelEdit {
                surface: target,
                texture_size: [2, 2],
                rect: crate::core::geometry::RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![1; 16]),
            }],
        };
        let result = after_effects_with_commits(&mut state, pending, &[], &[], &[], &[]).unwrap();

        let pending = result.pending_transaction.as_ref().unwrap();
        assert!(result.entry.is_none());
        assert_eq!(pending_pixel_edits(pending).len(), 1);
        assert_eq!(pending_pixel_edits(pending)[0].before, vec![1; 16]);
    }

    #[test]
    fn material_mask_records_property_history_after_reducer_state() {
        let state = state_with_materials(2);
        let layer_id = state.document.editor.active_layer_id;
        let material_id = state.document().unwrap().materials[0].id;
        let specified = LayerMaterialMask::Specified([material_id].into_iter().collect());
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::SetLayerMaterialMask {
                layer_id,
                material_mask: specified.clone(),
            })
            .unwrap();
        runtime
            .finalize_pending_renderer_edits(&RenderCommitArtifacts::default())
            .unwrap();

        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerPropertyEdit {
                edit: crate::application::LayerPropertyEdit::MaterialMask { before, after, .. }
            }) if before == &LayerMaterialMask::Unspecified && after == &specified
        ));
    }

    #[test]
    fn rename_layer_records_property_history_after_reducer_state() {
        let state = state_with_materials(1);
        let layer_id = state.document.editor.active_layer_id;
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::RenameLayer {
                layer_id,
                name: "Layer ABC".to_owned(),
            })
            .unwrap();
        runtime
            .finalize_pending_renderer_edits(&RenderCommitArtifacts::default())
            .unwrap();

        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerPropertyEdit {
                edit: crate::application::LayerPropertyEdit::Rename { before, after, .. }
            }) if before == "Default Layer" && after == "Layer ABC"
        ));
    }

    #[test]
    fn add_layer_records_transparent_image_history_without_readback() {
        let state = state_with_materials(1);
        let mut runtime = ApplicationRuntime::new(state);

        runtime.dispatch(Command::AddLayer).unwrap();
        let new_layer = runtime.state.document.editor.active_layer_id;
        runtime
            .finalize_pending_renderer_edits(&RenderCommitArtifacts::default())
            .unwrap();

        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerTreeEdit {
                edit: crate::application::LayerTreeEdit { after_images, .. }
            }) if after_images.len() == 1
                && after_images[0].surface.layer_id == new_layer
                && after_images[0].rgba8 == vec![0; 16]
        ));
    }

    #[test]
    fn duplicate_layer_records_tree_history_without_readback() {
        let state = state_with_materials(1);
        let layer_id = state.document.editor.active_layer_id;
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::DuplicateLayer { layer_id })
            .unwrap();

        assert!(!runtime.has_pending_history_transaction());
        assert!(matches!(
            runtime
                .state
                .history
                .undo_stack
                .last()
                .and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerTreeEdit { .. })
        ));
    }

    #[test]
    fn duplicate_group_records_group_and_child_mask_images() {
        let mut state = state_with_materials(1);
        let layer_id = state.document.editor.active_layer_id;
        let group = state
            .document
            .document
            .as_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        assert!(
            state
                .document
                .document
                .as_mut()
                .unwrap()
                .layer_tree
                .move_layer(layer_id, group, 0)
        );
        let mut runtime = ApplicationRuntime::new(state);
        runtime
            .dispatch(Command::AddLayerMask {
                layer_id: group,
                mode: crate::core::surface::LayerMaskInitMode::RevealAll,
            })
            .unwrap();
        runtime
            .dispatch(Command::AddLayerMask {
                layer_id,
                mode: crate::core::surface::LayerMaskInitMode::HideAll,
            })
            .unwrap();
        runtime.state.history.undo_stack.clear();

        runtime
            .dispatch(Command::DuplicateLayer { layer_id: group })
            .unwrap();

        let copied_group = runtime.state.document.editor.active_layer_id;
        let copied_child = runtime
            .state
            .document
            .document
            .as_ref()
            .unwrap()
            .layer_tree
            .children(copied_group)
            .unwrap()[0];
        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerTreeEdit {
                edit: crate::application::LayerTreeEdit { after_images, .. }
            }) if after_images.iter().any(|image| image.surface
                    == crate::core::surface::PaintSurfaceId::layer_mask(0.into(), copied_group))
                && after_images.iter().any(|image| image.surface
                    == crate::core::surface::PaintSurfaceId::raster(0.into(), copied_child))
                && after_images.iter().any(|image| image.surface
                    == crate::core::surface::PaintSurfaceId::layer_mask(0.into(), copied_child))
        ));
    }

    #[test]
    fn delete_layer_captures_before_images_from_document_tiles() {
        let mut state = state_with_materials(1);
        let layer_id = state.document.editor.active_layer_id;
        let surface = state.active_paint_surface().unwrap();
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .tiles
            .write_surface_rect(
                surface,
                crate::core::geometry::RectU32::full([2, 2]),
                &PixelSnapshotData::contiguous(vec![8; 16]),
            )
            .unwrap();
        let mut runtime = ApplicationRuntime::new(state);

        runtime.dispatch(Command::DeleteLayer { layer_id }).unwrap();

        assert!(!runtime.has_pending_history_transaction());
        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(crate::application::HistoryEntry::LayerTreeEdit {
                edit: crate::application::LayerTreeEdit { before_images, .. }
            }) if before_images.len() == 1 && before_images[0].rgba8 == vec![8; 16]
        ));
    }

    #[test]
    fn tiled_snapshot_metrics_can_be_incremented_for_history_data() {
        let snapshot =
            Rgba8Snapshot::new([512, 512], [0, 0], [512, 512], vec![9; 512 * 512 * 4]).unwrap();
        let data = super::snapshot_data_from_rgba_snapshot(&snapshot).unwrap();
        let mut metrics = RenderMetrics::default();

        record_pixel_snapshot_metrics(&mut metrics, &data);

        assert_eq!(metrics.tiled_snapshot_count, 1);
        assert_eq!(metrics.tiled_snapshot_tile_count, 4);
        assert_eq!(metrics.tiled_snapshot_bytes, 512 * 512 * 4);
    }

    #[test]
    fn rectangle_selection_pointer_up_creates_selection_history() {
        let mut state = state_with_material_size([1024, 1024]);
        state.set_active_tool(ToolId::RectangleSelection);
        let down = PointerSample::uv(glam::Vec2::new(0.1, 0.1), InputModifiers::default());
        let up = PointerSample::uv(glam::Vec2::new(0.5, 0.5), InputModifiers::default());
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(down)))
            .unwrap();
        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(up)))
            .unwrap();
        assert!(
            !runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .active_selection
                .enabled
        );

        let after_commits = vec![selection_commit(0, [1024, 1024], 7)];
        let commits = RenderCommitArtifacts {
            selection_after_commits: after_commits,
            ..Default::default()
        };
        runtime.finalize_pending_renderer_edits(&commits).unwrap();
        let history = runtime
            .state
            .history
            .undo_stack
            .last()
            .and_then(|tx| tx.atoms.first())
            .unwrap();

        assert!(matches!(
            history,
            crate::application::HistoryEntry::SelectionEdit {
                before,
                after,
                tiles,
            } if !before.enabled
                && after.enabled
                && tiles.len() == 16
                && tiles
                    .iter()
                    .all(|tile| tile.material_index.as_usize() == 0)
                && tiles.iter().all(|tile| tile.before.iter().all(|value| *value == 0))
                && tiles.iter().all(|tile| tile.after.iter().all(|value| *value == 7))
        ));
        assert!(!runtime.has_pending_history_transaction());
        assert!(
            runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .active_selection
                .enabled
        );
    }

    #[test]
    fn rectangle_selection_requires_renderer_selection_commit() {
        let mut state = state_with_material_size([4, 4]);
        state.set_active_tool(ToolId::RectangleSelection);
        let down = PointerSample::uv(glam::Vec2::new(0.1, 0.1), InputModifiers::default());
        let up = PointerSample::uv(glam::Vec2::new(0.5, 0.5), InputModifiers::default());
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(down)))
            .unwrap();
        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(up)))
            .unwrap();

        let result = runtime.finalize_pending_renderer_edits(&RenderCommitArtifacts::default());

        assert!(matches!(
            result,
            Err(err) if err.contains("missing after selection commit")
        ));
        assert!(runtime.state.history.undo_stack.is_empty());
        assert!(
            !runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .active_selection
                .enabled
        );
    }

    #[test]
    fn rectangle_selection_render_error_does_not_mutate_document_selection() {
        let mut state = state_with_material_size([4, 4]);
        state.set_active_tool(ToolId::RectangleSelection);
        let down = PointerSample::uv(glam::Vec2::new(0.1, 0.1), InputModifiers::default());
        let up = PointerSample::uv(glam::Vec2::new(0.5, 0.5), InputModifiers::default());
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(down)))
            .unwrap();
        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(up)))
            .unwrap();
        let result =
            runtime.observe_render_report(crate::core::render_report::RenderReport::error(
                "selection failed".to_owned(),
                RenderMetrics::default(),
            ));

        assert_eq!(result, Err("selection failed".to_owned()));
        assert!(runtime.state.history.undo_stack.is_empty());
        assert!(!runtime.has_pending_history_transaction());
        assert!(
            !runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .active_selection
                .enabled
        );
    }
}
