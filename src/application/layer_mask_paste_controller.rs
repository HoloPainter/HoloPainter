use anyhow::{Result, anyhow, ensure};

use crate::{
    core::{
        geometry::RectU32,
        surface::{LayerId, LayerMaskInitMode, PaintSurfaceId},
        tile_payload::PixelSnapshotData,
    },
    renderer::GpuDocumentCommand,
};

use super::{
    AppState, HistoryAtom, LayerMaskClipboardPayload, PixelEdit, ReducerOutput, layer_reducer,
};

const PASTE_HISTORY_LABEL: &str = "Paste Layer Mask";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaskPastePlacement {
    source_origin: [u32; 2],
    destination_origin: [u32; 2],
    size: [u32; 2],
}

pub(crate) fn paste_layer_mask(
    state: &mut AppState,
    layer_id: LayerId,
    material_index: usize,
    payload: LayerMaskClipboardPayload,
) -> Result<ReducerOutput> {
    validate_payload(&payload)?;
    ensure!(
        !state.is_tool_interacting(),
        "finish or cancel the current operation before pasting a layer mask"
    );
    ensure!(
        state.active_layer_id() == layer_id,
        "layer mask paste target changed before the edit was applied"
    );
    ensure!(
        state.focused_material_index() == material_index,
        "layer mask paste material changed before the edit was applied"
    );

    let (target_size, had_mask, previous_active_part) = {
        let document = state
            .document()
            .ok_or_else(|| anyhow!("layer mask paste requires a loaded document"))?;
        ensure!(
            document.layer_tree.can_have_layer_mask(layer_id),
            "the active layer cannot have a layer mask"
        );
        ensure!(
            !document.layer_tree.is_effectively_locked(layer_id),
            "the active layer is locked"
        );
        ensure!(
            document.layer_tree.is_effectively_visible(layer_id),
            "the active layer is hidden"
        );
        let material_id = document
            .material_id(material_index.into())
            .ok_or_else(|| anyhow!("the focused material is unavailable"))?;
        ensure!(
            document
                .layer_tree
                .effective_material_allowed(layer_id, material_id),
            "the active layer does not include the focused material"
        );
        let target_size = document
            .materials
            .get(material_index)
            .map(|material| material.texture_size)
            .ok_or_else(|| anyhow!("the focused material is unavailable"))?;
        (
            target_size,
            document.layer_tree.has_layer_mask(layer_id),
            state.document.editor.resolved(document).active_part,
        )
    };

    let placement = paste_placement(&payload, target_size)
        .ok_or_else(|| anyhow!("the copied layer mask does not overlap the target canvas"))?;

    let mut output = if had_mask {
        ReducerOutput::default()
    } else {
        layer_reducer::add_layer_mask(state, layer_id, LayerMaskInitMode::RevealAll)?
            .relabel_last_history_transaction(PASTE_HISTORY_LABEL)
    };

    let apply_result = apply_mask_payload(
        state,
        layer_id,
        material_index,
        &payload,
        placement,
        had_mask,
        &mut output,
    );
    if let Err(err) = apply_result {
        if !had_mask {
            rollback_created_mask(state, layer_id, previous_active_part);
        }
        return Err(err);
    }

    state.document.select_layer_mask_if_present(layer_id);
    state.set_status_key("status-layer-mask-pasted");
    Ok(output)
}

fn apply_mask_payload(
    state: &mut AppState,
    layer_id: LayerId,
    material_index: usize,
    payload: &LayerMaskClipboardPayload,
    placement: MaskPastePlacement,
    had_mask: bool,
    output: &mut ReducerOutput,
) -> Result<()> {
    let target = PaintSurfaceId::layer_mask(material_index.into(), layer_id);
    let document = state
        .document_mut()
        .ok_or_else(|| anyhow!("layer mask paste requires a loaded document"))?;
    let target_size = document
        .texture_size_for_surface(target)
        .ok_or_else(|| anyhow!("layer mask paste surface is unavailable"))?;
    let rect = RectU32 {
        origin: placement.destination_origin,
        size: placement.size,
    };
    let before = document
        .tiles
        .read_surface_rect(target, rect)
        .map_err(|err| anyhow!("reading destination layer mask pixels: {err:#}"))?;
    let before_rgba8 = before
        .to_vec_rgba8(target_size, rect.origin, rect.size)
        .map_err(|err| anyhow!("decoding destination layer mask pixels: {err:#}"))?;
    let mut after_rgba8 = before_rgba8.clone();
    blend_mask_payload(payload, placement, &mut after_rgba8)?;

    if after_rgba8 == before_rgba8 {
        return Ok(());
    }

    let after = PixelSnapshotData::contiguous(after_rgba8);
    let changed = document
        .tiles
        .write_surface_rect(target, rect, &after)
        .map_err(|err| anyhow!("writing destination layer mask pixels: {err:#}"))?;
    if !changed.tiles.is_empty() {
        output.push_document_command(GpuDocumentCommand::UploadSurfaceTiles {
            surface: target,
            surface_revision: changed.surface_revision,
            tiles: changed.tiles,
        });
    }

    if had_mask {
        let atom = HistoryAtom::PixelEdit {
            edits: vec![PixelEdit {
                surface: target,
                texture_size: target_size,
                rect,
                before,
                after,
            }],
        };
        let replaced =
            std::mem::take(output).with_finalized_history_atom(PASTE_HISTORY_LABEL, atom);
        *output = replaced;
    }
    Ok(())
}

fn validate_payload(payload: &LayerMaskClipboardPayload) -> Result<()> {
    ensure!(
        payload.size[0] > 0 && payload.size[1] > 0,
        "layer mask clipboard payload is empty"
    );
    let expected = pixel_len(payload.size)?;
    ensure!(
        payload.mask_r8.len() == expected,
        "layer mask clipboard payload length does not match its size"
    );
    ensure!(
        payload.coverage_r8.len() == expected,
        "layer mask clipboard coverage length does not match its size"
    );
    Ok(())
}

fn paste_placement(
    payload: &LayerMaskClipboardPayload,
    destination_size: [u32; 2],
) -> Option<MaskPastePlacement> {
    if destination_size[0] == 0 || destination_size[1] == 0 {
        return None;
    }
    let start = if payload.canvas_size == destination_size {
        [i64::from(payload.origin[0]), i64::from(payload.origin[1])]
    } else {
        [
            (i64::from(destination_size[0]) - i64::from(payload.size[0])).div_euclid(2),
            (i64::from(destination_size[1]) - i64::from(payload.size[1])).div_euclid(2),
        ]
    };
    let (source_x, destination_x, width) =
        clipped_axis(payload.size[0], destination_size[0], start[0])?;
    let (source_y, destination_y, height) =
        clipped_axis(payload.size[1], destination_size[1], start[1])?;
    Some(MaskPastePlacement {
        source_origin: [source_x, source_y],
        destination_origin: [destination_x, destination_y],
        size: [width, height],
    })
}

fn clipped_axis(source_len: u32, destination_len: u32, start: i64) -> Option<(u32, u32, u32)> {
    let source_start = u32::try_from((-start).max(0)).ok()?;
    let destination_start = u32::try_from(start.max(0)).ok()?;
    if source_start >= source_len || destination_start >= destination_len {
        return None;
    }
    let len = (source_len - source_start).min(destination_len - destination_start);
    (len > 0).then_some((source_start, destination_start, len))
}

fn blend_mask_payload(
    payload: &LayerMaskClipboardPayload,
    placement: MaskPastePlacement,
    destination_rgba8: &mut [u8],
) -> Result<()> {
    let destination_len = pixel_len(placement.size)?
        .checked_mul(4)
        .ok_or_else(|| anyhow!("destination layer mask payload size overflows usize"))?;
    ensure!(
        destination_rgba8.len() == destination_len,
        "destination layer mask payload length does not match the paste rectangle"
    );

    let source_width = usize::try_from(payload.size[0])
        .map_err(|_| anyhow!("source layer mask width overflows usize"))?;
    let destination_width = usize::try_from(placement.size[0])
        .map_err(|_| anyhow!("destination layer mask width overflows usize"))?;
    for y in 0..placement.size[1] {
        for x in 0..placement.size[0] {
            let source_x = placement.source_origin[0] + x;
            let source_y = placement.source_origin[1] + y;
            let source_index = source_y as usize * source_width + source_x as usize;
            let destination_index = (y as usize * destination_width + x as usize) * 4;
            let mask = u32::from(payload.mask_r8[source_index]);
            let coverage = u32::from(payload.coverage_r8[source_index]);
            let destination = u32::from(destination_rgba8[destination_index + 3]);
            let value = ((destination * (255 - coverage) + mask * coverage + 127) / 255) as u8;
            destination_rgba8[destination_index..destination_index + 4].fill(value);
        }
    }
    Ok(())
}

fn rollback_created_mask(
    state: &mut AppState,
    layer_id: LayerId,
    previous_active_part: crate::core::document::ActiveLayerPart,
) {
    let Some((document, editor_document)) = state.document_and_editor_document_mut() else {
        return;
    };
    for material_index in 0..document.materials.len() {
        let _ = document
            .tiles
            .delete_surface(PaintSurfaceId::layer_mask(material_index.into(), layer_id));
    }
    let _ = document.layer_tree.delete_layer_mask(layer_id);
    editor_document.active_layer_id = layer_id;
    editor_document.active_part = previous_active_part;
}

fn pixel_len(size: [u32; 2]) -> Result<usize> {
    let width = usize::try_from(size[0]).map_err(|_| anyhow!("image width overflows usize"))?;
    let height = usize::try_from(size[1]).map_err(|_| anyhow!("image height overflows usize"))?;
    width
        .checked_mul(height)
        .ok_or_else(|| anyhow!("image pixel count overflows usize"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::{ApplicationRuntime, Command},
        core::document::{Document, MaterialSpec, MeshData},
    };

    fn state_with_materials(materials: Vec<MaterialSpec>) -> (AppState, LayerId) {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(MeshData::empty(), materials));
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        (state, layer_id)
    }

    #[test]
    fn same_canvas_preserves_copy_origin() {
        let payload = LayerMaskClipboardPayload {
            canvas_size: [16, 12],
            origin: [5, 3],
            size: [4, 2],
            mask_r8: vec![0; 8],
            coverage_r8: vec![255; 8],
        };
        assert_eq!(
            paste_placement(&payload, [16, 12]),
            Some(MaskPastePlacement {
                source_origin: [0, 0],
                destination_origin: [5, 3],
                size: [4, 2],
            })
        );
    }

    #[test]
    fn different_canvas_centers_and_clips_without_scaling() {
        let payload = LayerMaskClipboardPayload {
            canvas_size: [20, 20],
            origin: [2, 2],
            size: [8, 6],
            mask_r8: vec![0; 48],
            coverage_r8: vec![255; 48],
        };
        assert_eq!(
            paste_placement(&payload, [4, 4]),
            Some(MaskPastePlacement {
                source_origin: [2, 1],
                destination_origin: [0, 0],
                size: [4, 4],
            })
        );
    }

    #[test]
    fn mask_blend_uses_coverage_independently_from_mask_value() {
        let payload = LayerMaskClipboardPayload {
            canvas_size: [2, 1],
            origin: [0, 0],
            size: [2, 1],
            mask_r8: vec![0, 255],
            coverage_r8: vec![255, 128],
        };
        let placement = MaskPastePlacement {
            source_origin: [0, 0],
            destination_origin: [0, 0],
            size: [2, 1],
        };
        let mut destination = vec![255; 8];
        blend_mask_payload(&payload, placement, &mut destination).unwrap();
        assert_eq!(&destination[..4], &[0, 0, 0, 0]);
        assert_eq!(&destination[4..], &[255, 255, 255, 255]);

        let payload = LayerMaskClipboardPayload {
            mask_r8: vec![0, 0],
            coverage_r8: vec![128, 0],
            ..payload
        };
        let mut destination = vec![255; 8];
        blend_mask_payload(&payload, placement, &mut destination).unwrap();
        assert_eq!(&destination[..4], &[127, 127, 127, 127]);
        assert_eq!(&destination[4..], &[255, 255, 255, 255]);
    }

    #[test]
    fn paste_existing_mask_blends_pixels_and_records_pixel_history() {
        let (mut state, layer_id) = state_with_materials(vec![MaterialSpec::new("A", [2, 1])]);
        layer_reducer::add_layer_mask(&mut state, layer_id, LayerMaskInitMode::RevealAll).unwrap();
        let mut runtime = ApplicationRuntime::new(state);
        runtime
            .dispatch(Command::PasteLayerMask {
                layer_id,
                material_index: 0,
                payload: LayerMaskClipboardPayload {
                    canvas_size: [2, 1],
                    origin: [0, 0],
                    size: [2, 1],
                    mask_r8: vec![0, 0],
                    coverage_r8: vec![255, 128],
                },
            })
            .unwrap();

        let target = PaintSurfaceId::layer_mask(0.into(), layer_id);
        let image = runtime
            .state
            .document()
            .unwrap()
            .tiles
            .read_surface_full(target)
            .unwrap();
        assert_eq!(&image.rgba8[..4], &[0, 0, 0, 0]);
        assert_eq!(&image.rgba8[4..], &[127, 127, 127, 127]);
        assert_eq!(
            runtime.state.active_layer_target(),
            crate::core::document::ActiveLayerTarget::LayerMask
        );
        let history = runtime.state.history.undo_stack.last().unwrap();
        assert_eq!(history.label, PASTE_HISTORY_LABEL);
        assert!(matches!(
            history.atoms.as_slice(),
            [HistoryAtom::PixelEdit { .. }]
        ));
    }

    #[test]
    fn paste_without_mask_creates_reveal_all_masks_and_undoes_atomically() {
        let (state, layer_id) = state_with_materials(vec![
            MaterialSpec::new("A", [2, 1]),
            MaterialSpec::new("B", [3, 1]),
        ]);
        let mut runtime = ApplicationRuntime::new(state);
        runtime
            .dispatch(Command::PasteLayerMask {
                layer_id,
                material_index: 0,
                payload: LayerMaskClipboardPayload {
                    canvas_size: [2, 1],
                    origin: [0, 0],
                    size: [1, 1],
                    mask_r8: vec![0],
                    coverage_r8: vec![255],
                },
            })
            .unwrap();

        let material_a = PaintSurfaceId::layer_mask(0.into(), layer_id);
        let material_b = PaintSurfaceId::layer_mask(1.into(), layer_id);
        let document = runtime.state.document().unwrap();
        assert!(document.layer_tree.has_layer_mask(layer_id));
        assert_eq!(
            document.tiles.read_surface_full(material_a).unwrap().rgba8,
            vec![0, 0, 0, 0, 255, 255, 255, 255]
        );
        assert_eq!(
            document.tiles.read_surface_full(material_b).unwrap().rgba8,
            vec![255; 3 * 4]
        );
        let history = runtime.state.history.undo_stack.last().unwrap();
        assert_eq!(history.label, PASTE_HISTORY_LABEL);
        assert!(matches!(
            history.atoms.as_slice(),
            [HistoryAtom::LayerTreeEdit { .. }]
        ));

        runtime.drain_renderer_frame_plan(Vec::new());
        runtime.dispatch(Command::Undo).unwrap();
        runtime.drain_renderer_frame_plan(Vec::new());
        assert!(
            !runtime
                .state
                .document()
                .unwrap()
                .layer_tree
                .has_layer_mask(layer_id)
        );

        runtime.dispatch(Command::Redo).unwrap();
        runtime.drain_renderer_frame_plan(Vec::new());
        let document = runtime.state.document().unwrap();
        assert!(document.layer_tree.has_layer_mask(layer_id));
        assert_eq!(
            document.tiles.read_surface_full(material_a).unwrap().rgba8,
            vec![0, 0, 0, 0, 255, 255, 255, 255]
        );
        assert_eq!(
            document.tiles.read_surface_full(material_b).unwrap().rgba8,
            vec![255; 3 * 4]
        );
    }
}
