use std::collections::HashSet;

use anyhow::{Result, anyhow};

use crate::{
    application::{
        CompositeImpact, CompositeSync, HistoryAtom, HistoryTransaction, LayerImageSnapshot,
        LayerPropertyEdit, LayerTreeSnapshot, PixelEdit, RectU32, ReducerOutput, SelectionTileEdit,
        SelectionTilePayload, state::AppState,
    },
    core::{image::LayerInitialPixels, surface::PaintSurfaceId, tile_payload::TilePayload},
    renderer::GpuDocumentCommand,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryDirection {
    Undo,
    Redo,
}

pub fn undo(state: &mut AppState) -> Result<ReducerOutput> {
    step(state, HistoryDirection::Undo)
}

pub fn redo(state: &mut AppState) -> Result<ReducerOutput> {
    step(state, HistoryDirection::Redo)
}

fn step(state: &mut AppState, direction: HistoryDirection) -> Result<ReducerOutput> {
    if state.is_tool_interacting() {
        return Ok(ReducerOutput::default());
    }
    let entry = match direction {
        HistoryDirection::Undo => state.pop_undo_history(),
        HistoryDirection::Redo => state.pop_redo_history(),
    };
    let Some(entry) = entry else {
        return Ok(ReducerOutput::default());
    };

    // History application only mutates the document session. Keep the popped entry
    // owned locally so a failure can restore both pieces without cloning AppState,
    // including its potentially large history stacks and unrelated UI state.
    let document_rollback = state.document.clone();
    let composite_sync = composite_sync_for_transaction(state, &entry);
    let effects = match apply_history_transaction(state, &entry, direction) {
        Ok(effects) => effects,
        Err(err) => {
            state.document = document_rollback;
            match direction {
                HistoryDirection::Undo => state.push_undo_history_raw(entry),
                HistoryDirection::Redo => state.push_redo_history_raw(entry),
            }
            return Err(err);
        }
    };
    let label = entry.label.clone();
    let status_key = match direction {
        HistoryDirection::Undo => {
            state.push_redo_history_raw(entry);
            "status-history-undo"
        }
        HistoryDirection::Redo => {
            state.push_undo_history_raw(entry);
            "status-history-redo"
        }
    };
    state.set_status_message(super::StatusMessage::localized(status_key).arg("label", label));
    Ok(with_document_commands(effects)
        .with_composite_sync(composite_sync)
        .with_project_change())
}

fn composite_sync_for_transaction(
    state: &AppState,
    transaction: &HistoryTransaction,
) -> CompositeSync {
    let impact = transaction
        .atoms
        .iter()
        .fold(CompositeImpact::None, |impact, atom| {
            impact.merge(CompositeImpact::for_history_atom(state.document(), atom))
        });
    impact.into_composite_sync(state.document())
}

fn apply_history_transaction(
    state: &mut AppState,
    transaction: &HistoryTransaction,
    direction: HistoryDirection,
) -> Result<Vec<GpuDocumentCommand>> {
    let mut effects = Vec::new();
    match direction {
        HistoryDirection::Undo => {
            for atom in transaction.atoms.iter().rev() {
                effects.extend(apply_history_atom(state, atom, direction)?);
            }
        }
        HistoryDirection::Redo => {
            for atom in &transaction.atoms {
                effects.extend(apply_history_atom(state, atom, direction)?);
            }
        }
    }
    Ok(effects)
}

pub(crate) fn apply_history_atom(
    state: &mut AppState,
    entry: &HistoryAtom,
    direction: HistoryDirection,
) -> Result<Vec<GpuDocumentCommand>> {
    match entry {
        HistoryAtom::PixelEdit { edits } => apply_pixel_edits(state, edits, direction),
        HistoryAtom::SelectionEdit {
            before,
            after,
            tiles,
        } => {
            let selection = if direction == HistoryDirection::Undo {
                before.clone()
            } else {
                after.clone()
            };
            Ok(apply_selection_edit(state, selection, tiles, direction))
        }
        HistoryAtom::LayerPropertyEdit { edit } => {
            apply_layer_property_edit(state, edit, direction);
            Ok(Vec::new())
        }
        HistoryAtom::LayerTreeEdit { edit } => {
            let (snapshot, images) = match direction {
                HistoryDirection::Undo => (&edit.before, &edit.before_images),
                HistoryDirection::Redo => (&edit.after, &edit.after_images),
            };
            apply_layer_tree_snapshot(state, snapshot, images, &edit.retained_embedded_images)
        }
        HistoryAtom::MaterialTextureResize { before, after } => {
            let snapshot = match direction {
                HistoryDirection::Undo => before,
                HistoryDirection::Redo => after,
            };
            let Some(document) = state.document_mut() else {
                return Ok(Vec::new());
            };
            document
                .restore_material_texture_state(snapshot)
                .map_err(|err| anyhow!("History material texture restore failed: {err:#}"))?;
            let active_selection = document.active_selection.clone();
            Ok(vec![GpuDocumentCommand::ResizeMaterialTexture {
                snapshot: snapshot.clone(),
                active_selection,
            }])
        }
    }
}

fn apply_selection_edit(
    state: &mut AppState,
    selection: crate::core::selection::ActiveSelection,
    tiles: &[SelectionTileEdit],
    direction: HistoryDirection,
) -> Vec<GpuDocumentCommand> {
    let tiles = tiles
        .iter()
        .map(|tile| SelectionTilePayload {
            material_index: tile.material_index,
            rect: tile.rect,
            r8: if direction == HistoryDirection::Redo {
                tile.after.clone()
            } else {
                tile.before.clone()
            },
        })
        .collect::<Vec<_>>();
    let selection_patch_error = if let Some(document) = state.document_mut() {
        document.active_selection = selection.clone();
        document
            .selection_masks
            .patch_tiles(&document.materials, &tiles)
            .err()
            .map(|err| format!("{err:#}"))
    } else {
        None
    };
    if let Some(error) = selection_patch_error {
        state.set_status_message(
            crate::application::StatusMessage::localized("status-history-selection-restore-failed")
                .arg("error", error),
        );
    }
    vec![GpuDocumentCommand::UploadSelectionTiles {
        active_selection: selection,
        tiles,
    }]
}

fn apply_pixel_edits(
    state: &mut AppState,
    edits: &[PixelEdit],
    direction: HistoryDirection,
) -> Result<Vec<GpuDocumentCommand>> {
    let mut effects = Vec::with_capacity(edits.len());
    for edit in edits {
        let data = if direction == HistoryDirection::Redo {
            &edit.after
        } else {
            &edit.before
        };
        if let Some(document) = state.document_mut() {
            let changed = document
                .tiles
                .write_surface_rect(edit.surface, edit.rect, data)
                .map_err(|err| anyhow!("History pixel restore failed: {err:#}"))?;
            effects.push(upload_tiles_or_empty_rect(changed, edit.rect));
            continue;
        }
        let rgba8 = data
            .to_vec_rgba8(edit.texture_size, edit.rect.origin, edit.rect.size)
            .map_err(|err| anyhow!("History pixel snapshot invalid: {err:#}"))?;
        effects.push(upload_surface_rgba8_command(
            edit.surface,
            edit.rect,
            rgba8,
            None,
        ));
    }
    Ok(effects)
}

fn upload_tiles_or_empty_rect(
    changed: crate::core::document_tile_store::ChangedTiles,
    _fallback_rect: RectU32,
) -> GpuDocumentCommand {
    upload_surface_tiles_command(changed.surface, changed.surface_revision, changed.tiles)
}

fn apply_layer_property_edit(
    state: &mut AppState,
    edit: &LayerPropertyEdit,
    direction: HistoryDirection,
) {
    let Some(document) = state.document_mut() else {
        return;
    };
    match edit {
        LayerPropertyEdit::Rename {
            layer_id,
            before,
            after,
        } => document.layer_tree.rename_layer(
            *layer_id,
            if direction == HistoryDirection::Undo {
                before.clone()
            } else {
                after.clone()
            },
        ),
        LayerPropertyEdit::Visibility {
            layer_id,
            before,
            after,
        } => document.layer_tree.set_visible(
            *layer_id,
            if direction == HistoryDirection::Undo {
                *before
            } else {
                *after
            },
        ),
        LayerPropertyEdit::Lock {
            layer_id,
            before,
            after,
        } => {
            document.layer_tree.set_locked(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    before.clone()
                } else {
                    after.clone()
                },
            );
        }
        LayerPropertyEdit::LockMany { layers } => {
            for (layer_id, before, after) in layers {
                document.layer_tree.set_locked(
                    *layer_id,
                    if direction == HistoryDirection::Undo {
                        *before
                    } else {
                        *after
                    },
                );
            }
        }
        LayerPropertyEdit::MaterialMask {
            layer_id,
            before,
            after,
        } => {
            document.layer_tree.set_layer_material_mask(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    before.clone()
                } else {
                    after.clone()
                },
            );
        }
        LayerPropertyEdit::Opacity {
            layer_id,
            before,
            after,
        } => {
            document.layer_tree.restore_opacity(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    before.clone()
                } else {
                    after.clone()
                },
            );
        }
        LayerPropertyEdit::FillColor {
            layer_id,
            before,
            after,
            ..
        } => {
            document.layer_tree.set_solid_fill_color(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    *before
                } else {
                    *after
                },
            );
        }
        LayerPropertyEdit::Adjustment {
            layer_id,
            before,
            after,
            ..
        } => {
            document.layer_tree.set_adjustment(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    before.clone()
                } else {
                    after.clone()
                },
            );
        }
        LayerPropertyEdit::EmbeddedImageTransform {
            layer_id,
            before,
            after,
        } => {
            document.layer_tree.set_embedded_image_transform(
                *layer_id,
                if direction == HistoryDirection::Undo {
                    *before
                } else {
                    *after
                },
            );
        }
        LayerPropertyEdit::OpacityMany { layers } => {
            for (layer_id, before, after) in layers {
                document.layer_tree.restore_opacity(
                    *layer_id,
                    if direction == HistoryDirection::Undo {
                        *before
                    } else {
                        *after
                    },
                );
            }
        }
        LayerPropertyEdit::CompositeMode {
            layer_id,
            before,
            after,
        } => {
            let settings = if direction == HistoryDirection::Undo {
                *before
            } else {
                *after
            };
            document.layer_tree.restore_composite_settings(
                *layer_id,
                settings.blend_mode,
                settings.group_mode,
            );
        }
        LayerPropertyEdit::CompositeModeMany { layers } => {
            for (layer_id, before, after) in layers {
                let settings = if direction == HistoryDirection::Undo {
                    *before
                } else {
                    *after
                };
                document.layer_tree.restore_composite_settings(
                    *layer_id,
                    settings.blend_mode,
                    settings.group_mode,
                );
            }
        }
    }
}

fn apply_layer_tree_snapshot(
    state: &mut AppState,
    snapshot: &LayerTreeSnapshot,
    images: &[LayerImageSnapshot],
    embedded_images: &[std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>],
) -> Result<Vec<GpuDocumentCommand>> {
    let Some((document, editor_document)) = state.document_and_editor_document_mut() else {
        return Ok(Vec::new());
    };

    let before_image_ids = document
        .embedded_images()
        .map(|image| image.id)
        .collect::<HashSet<_>>();
    let (change, _) = document
        .restore_layer_tree_with_embedded_images(snapshot.tree.clone(), images, embedded_images)
        .map_err(|err| anyhow!("History layer tree restore failed: {err:#}"))?;
    let after_images = document.embedded_images().cloned().collect::<Vec<_>>();
    let after_image_ids = after_images
        .iter()
        .map(|image| image.id)
        .collect::<HashSet<_>>();
    if document.layer_tree.contains(snapshot.active_layer_id) {
        editor_document.active_layer_id = snapshot.active_layer_id;
        editor_document.active_part = snapshot.active_part;
    } else if let Some(paintable) = document.layer_tree.first_paintable_layer() {
        editor_document.active_layer_id = paintable;
        editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
    }

    let mut effects = change
        .deleted_surfaces
        .into_iter()
        .map(delete_surface_command)
        .collect::<Vec<_>>();
    effects.extend(
        after_images
            .into_iter()
            .filter(|image| !before_image_ids.contains(&image.id))
            .map(|image| GpuDocumentCommand::UpsertEmbeddedImage { image }),
    );
    effects.extend(
        before_image_ids
            .difference(&after_image_ids)
            .copied()
            .map(|image_id| GpuDocumentCommand::RemoveEmbeddedImage { image_id }),
    );
    for changed in change.created_surfaces {
        let image = images
            .iter()
            .find(|image| image.surface == changed.surface)
            .expect("validated restored surface must have an image");
        effects.push(create_surface_command(
            changed.surface,
            LayerInitialPixels::Transparent,
        ));
        effects.push(upload_tiles_or_full_image(changed, image));
    }
    for changed in change.updated_surfaces {
        let image = images
            .iter()
            .find(|image| image.surface == changed.surface)
            .expect("validated updated surface must have an image");
        effects.push(upload_tiles_or_full_image(changed, image));
    }
    Ok(effects)
}

fn upload_tiles_or_full_image(
    changed: crate::core::document_tile_store::ChangedTiles,
    image: &LayerImageSnapshot,
) -> GpuDocumentCommand {
    if changed.tiles.is_empty() {
        upload_surface_rgba8_command(
            image.surface,
            RectU32::full(image.texture_size),
            image.rgba8.clone(),
            Some(changed.surface_revision),
        )
    } else {
        upload_surface_tiles_command(changed.surface, changed.surface_revision, changed.tiles)
    }
}

fn with_document_commands(document_commands: Vec<GpuDocumentCommand>) -> ReducerOutput {
    let mut output = ReducerOutput::default();
    for command in document_commands {
        output.push_document_command(command);
    }
    output
}

fn create_surface_command(
    target: PaintSurfaceId,
    initial: LayerInitialPixels,
) -> GpuDocumentCommand {
    GpuDocumentCommand::CreateSurface { target, initial }
}

fn delete_surface_command(target: PaintSurfaceId) -> GpuDocumentCommand {
    GpuDocumentCommand::DeleteSurface { target }
}

fn upload_surface_rgba8_command(
    surface: PaintSurfaceId,
    rect: RectU32,
    rgba8: Vec<u8>,
    document_revision: Option<u64>,
) -> GpuDocumentCommand {
    GpuDocumentCommand::UploadSurfaceRgba8 {
        surface,
        rect,
        rgba8,
        document_revision,
    }
}

fn upload_surface_tiles_command(
    surface: PaintSurfaceId,
    surface_revision: u64,
    tiles: Vec<TilePayload>,
) -> GpuDocumentCommand {
    GpuDocumentCommand::UploadSurfaceTiles {
        surface,
        surface_revision,
        tiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::{LayerCompositeSettings, PendingEditTransaction, state::EditorDocumentState},
        core::{
            composite::{GroupCompositeMode, LayerBlendMode},
            document::{Document, MaterialSpec, MeshData},
            surface::LayerMaterialMask,
        },
    };

    fn state_with_group() -> (AppState, crate::core::surface::LayerId) {
        let mut state = AppState::default();
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let group = document.layer_tree.add_group_above(layer, "Group").unwrap();
        state.document.editor = EditorDocumentState::from_document(&document);
        state.document.document = Some(document);
        (state, group)
    }

    fn take_composite_sync(mut output: ReducerOutput) -> CompositeSync {
        output
            .pending_transaction
            .as_mut()
            .map(PendingEditTransaction::take_composite_sync)
            .unwrap_or_default()
    }

    #[test]
    fn resize_history_restores_size_before_older_pixel_edit_is_applied() {
        use crate::core::tile_payload::PixelSnapshotData;

        let mut state = AppState::default();
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [2, 2])]);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface = crate::core::surface::PaintSurfaceId::raster(0.into(), layer);
        let full_2 = RectU32::full([2, 2]);
        let before_2 = document.tiles.read_surface_rect(surface, full_2).unwrap();
        let painted_2 = PixelSnapshotData::contiguous([10, 20, 30, 255].repeat(4));
        document
            .tiles
            .write_surface_rect(surface, full_2, &painted_2)
            .unwrap();
        state.document.editor = EditorDocumentState::from_document(&document);
        state.document.document = Some(document);
        state.push_undo_history_raw(HistoryTransaction::test(
            "Paint 2x2",
            vec![HistoryAtom::PixelEdit {
                edits: vec![PixelEdit {
                    surface,
                    texture_size: [2, 2],
                    rect: full_2,
                    before: before_2.clone(),
                    after: painted_2.clone(),
                }],
            }],
        ));

        let resize = state
            .document_mut()
            .unwrap()
            .resize_material_texture(0.into(), [4, 4])
            .unwrap()
            .unwrap();
        state.push_undo_history_raw(HistoryTransaction::test(
            "Resize Material Texture",
            vec![HistoryAtom::MaterialTextureResize {
                before: resize.before.clone(),
                after: resize.after.clone(),
            }],
        ));

        let pixel_rect = RectU32 {
            origin: [0, 0],
            size: [1, 1],
        };
        let before_4 = state
            .document()
            .unwrap()
            .tiles
            .read_surface_rect(surface, pixel_rect)
            .unwrap();
        let painted_4 = PixelSnapshotData::contiguous(vec![250, 240, 230, 255]);
        state
            .document_mut()
            .unwrap()
            .tiles
            .write_surface_rect(surface, pixel_rect, &painted_4)
            .unwrap();
        state.push_undo_history_raw(HistoryTransaction::test(
            "Paint 4x4",
            vec![HistoryAtom::PixelEdit {
                edits: vec![PixelEdit {
                    surface,
                    texture_size: [4, 4],
                    rect: pixel_rect,
                    before: before_4,
                    after: painted_4,
                }],
            }],
        ));

        undo(&mut state).unwrap();
        assert_eq!(state.document().unwrap().materials[0].texture_size, [4, 4]);
        undo(&mut state).unwrap();
        assert_eq!(state.document().unwrap().materials[0].texture_size, [2, 2]);
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_rect(surface, full_2)
                .unwrap(),
            painted_2
        );
        undo(&mut state).unwrap();
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_rect(surface, full_2)
                .unwrap(),
            before_2
        );

        redo(&mut state).unwrap();
        assert_eq!(state.document().unwrap().materials[0].texture_size, [2, 2]);
        redo(&mut state).unwrap();
        assert_eq!(state.document().unwrap().materials[0].texture_size, [4, 4]);
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_full(surface)
                .unwrap(),
            resize
                .after
                .surfaces
                .iter()
                .find(|snapshot| snapshot.surface == surface)
                .unwrap()
                .clone()
        );
        redo(&mut state).unwrap();
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_rect(surface, pixel_rect)
                .unwrap()
                .to_vec_rgba8([4, 4], [0, 0], [1, 1])
                .unwrap(),
            vec![250, 240, 230, 255]
        );
    }

    #[test]
    fn live_edit_undo_and_redo_share_the_same_composite_sync() {
        let (mut state, group) = state_with_group();
        let live =
            crate::application::layer_reducer::set_layer_opacity(&mut state, group, 0.4).unwrap();
        let live_sync = take_composite_sync(live);
        let transaction = HistoryTransaction::test(
            "Layer Opacity",
            vec![HistoryAtom::LayerPropertyEdit {
                edit: LayerPropertyEdit::Opacity {
                    layer_id: group,
                    before: 1.0,
                    after: 0.4,
                },
            }],
        );
        state.push_undo_history_raw(transaction);

        let undo_sync = take_composite_sync(undo(&mut state).unwrap());
        let redo_sync = take_composite_sync(redo(&mut state).unwrap());

        assert_eq!(live_sync, CompositeSync::layer_props(group));
        assert_eq!(undo_sync, live_sync);
        assert_eq!(redo_sync, live_sync);
    }

    #[test]
    fn material_mask_history_restores_unspecified_and_specified_states() {
        let (mut state, group) = state_with_group();
        let material_id = state.document().unwrap().materials[0].id;
        let specified = LayerMaterialMask::Specified([material_id].into_iter().collect());
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(group, specified.clone());
        let edit = LayerPropertyEdit::MaterialMask {
            layer_id: group,
            before: LayerMaterialMask::Unspecified,
            after: specified.clone(),
        };

        apply_layer_property_edit(&mut state, &edit, HistoryDirection::Undo);
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .layer_material_mask(group),
            Some(&LayerMaterialMask::Unspecified)
        );

        apply_layer_property_edit(&mut state, &edit, HistoryDirection::Redo);
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .layer_material_mask(group),
            Some(&specified)
        );
    }

    #[test]
    fn composite_mode_history_restores_pass_through_state() {
        let (mut state, group) = state_with_group();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_group_composite_mode(group, GroupCompositeMode::PassThrough);
        let edit = LayerPropertyEdit::CompositeMode {
            layer_id: group,
            before: LayerCompositeSettings {
                blend_mode: LayerBlendMode::Normal,
                group_mode: GroupCompositeMode::Isolated,
            },
            after: LayerCompositeSettings {
                blend_mode: LayerBlendMode::Normal,
                group_mode: GroupCompositeMode::PassThrough,
            },
        };
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .set_locked(group, true)
        );

        apply_layer_property_edit(&mut state, &edit, HistoryDirection::Undo);
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::Isolated)
        );

        apply_layer_property_edit(&mut state, &edit, HistoryDirection::Redo);
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::PassThrough)
        );
    }
}
