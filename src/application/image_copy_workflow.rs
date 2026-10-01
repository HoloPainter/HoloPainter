use anyhow::{Result, anyhow, ensure};

use crate::core::{
    document::ActiveLayerTarget,
    image::unpremultiply_rgba8,
    surface::{LayerId, PaintSurfaceId},
};

use super::{AppState, PaintEditBlockReason, PaintEditDecision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedLayerImage {
    pub layer_name: String,
    pub canvas_size: [u32; 2],
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerMaskClipboardPayload {
    pub canvas_size: [u32; 2],
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub mask_r8: Vec<u8>,
    pub coverage_r8: Vec<u8>,
}

impl LayerMaskClipboardPayload {
    pub fn clipboard_rgba8(&self) -> Result<Vec<u8>> {
        let expected = r8_len(self.size)?;
        ensure!(
            self.mask_r8.len() == expected,
            "layer mask clipboard payload length does not match its size"
        );
        ensure!(
            self.coverage_r8.len() == expected,
            "layer mask clipboard coverage length does not match its size"
        );
        let capacity = expected
            .checked_mul(4)
            .ok_or_else(|| anyhow!("layer mask clipboard image size overflows usize"))?;
        let mut rgba8 = Vec::with_capacity(capacity);
        for (&mask, &coverage) in self.mask_r8.iter().zip(&self.coverage_r8) {
            rgba8.extend_from_slice(&[mask, mask, mask, coverage]);
        }
        Ok(rgba8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedLayerMask {
    pub layer_name: String,
    pub payload: LayerMaskClipboardPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopiedLayerContent {
    Raster(CopiedLayerImage),
    LayerMask(CopiedLayerMask),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedLayerCutAction {
    ClearSurface(PaintSurfaceId),
    DeleteLayerMask(LayerId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedLayerCut {
    pub copied: CopiedLayerContent,
    pub action: PreparedLayerCutAction,
}

pub fn can_cut_layer_content(state: &AppState, layer_id: LayerId) -> bool {
    state.document().is_some()
        && state.active_layer_id() == layer_id
        && active_copy_surface(state, layer_id).is_ok()
        && state
            .paint_edit_permission(Some(state.focused_material_index()))
            .is_allowed()
}

pub fn prepare_cut_layer_content(state: &AppState, layer_id: LayerId) -> Result<PreparedLayerCut> {
    ensure!(state.document().is_some(), "cut requires a loaded document");
    ensure!(
        state.active_layer_id() == layer_id,
        "cut requires the selected layer target to be active"
    );

    let material_index = state.focused_material_index();
    match state.paint_edit_permission(Some(material_index)) {
        PaintEditDecision::Allowed(()) => {}
        PaintEditDecision::Blocked(reason) => {
            return Err(anyhow!(cut_block_reason_message(reason)));
        }
        PaintEditDecision::Unavailable => {
            return Err(anyhow!("the cut target is unavailable"));
        }
    }

    let target = active_copy_surface(state, layer_id)?;
    let copied = copy_layer_content(state, layer_id)?;
    let delete_whole_mask = state.active_layer_target() == ActiveLayerTarget::LayerMask
        && state
            .document()
            .is_some_and(|document| document.active_selection.is_effectively_full());
    let action = if delete_whole_mask {
        PreparedLayerCutAction::DeleteLayerMask(layer_id)
    } else {
        PreparedLayerCutAction::ClearSurface(target)
    };
    Ok(PreparedLayerCut { copied, action })
}

fn cut_block_reason_message(reason: PaintEditBlockReason) -> &'static str {
    match reason {
        PaintEditBlockReason::ActiveLayerIsGroup => "the active layer target cannot be cut",
        PaintEditBlockReason::ActiveLayerLocked => "the active layer is locked",
        PaintEditBlockReason::ActiveLayerHidden => "the active layer is hidden",
        PaintEditBlockReason::MaterialExcluded => {
            "the active layer does not include the focused material"
        }
        PaintEditBlockReason::ActiveTargetNotEditable => "the active layer target cannot be cut",
    }
}

pub fn copy_layer_content(state: &AppState, layer_id: LayerId) -> Result<CopiedLayerContent> {
    ensure!(
        state.active_layer_id() == layer_id,
        "copy requires the selected layer target to be active"
    );
    match state.active_layer_target() {
        ActiveLayerTarget::Raster => {
            copy_raster_layer(state, layer_id).map(CopiedLayerContent::Raster)
        }
        ActiveLayerTarget::LayerMask => {
            copy_layer_mask(state, layer_id).map(CopiedLayerContent::LayerMask)
        }
        ActiveLayerTarget::SolidFill
        | ActiveLayerTarget::Adjustment
        | ActiveLayerTarget::EmbeddedImage
        | ActiveLayerTarget::Structure => {
            Err(anyhow!("copy requires a raster layer or layer mask"))
        }
    }
}

fn active_copy_surface(state: &AppState, layer_id: LayerId) -> Result<PaintSurfaceId> {
    let document = state
        .document()
        .ok_or_else(|| anyhow!("copy requires a loaded document"))?;
    let material_index = state.focused_material_index();
    match state.active_layer_target() {
        ActiveLayerTarget::Raster => {
            ensure!(
                document.layer_tree.is_paintable(layer_id),
                "copy requires a raster layer"
            );
            Ok(PaintSurfaceId::raster(material_index.into(), layer_id))
        }
        ActiveLayerTarget::LayerMask => {
            ensure!(
                document.layer_tree.has_layer_mask(layer_id),
                "copy requires an existing layer mask"
            );
            Ok(PaintSurfaceId::layer_mask(material_index.into(), layer_id))
        }
        ActiveLayerTarget::SolidFill
        | ActiveLayerTarget::Adjustment
        | ActiveLayerTarget::EmbeddedImage
        | ActiveLayerTarget::Structure => {
            Err(anyhow!("copy requires a raster layer or layer mask"))
        }
    }
}

fn copy_raster_layer(state: &AppState, layer_id: LayerId) -> Result<CopiedLayerImage> {
    let document = state
        .document()
        .ok_or_else(|| anyhow!("copy requires a loaded document"))?;
    ensure!(
        document.layer_tree.is_paintable(layer_id),
        "copy requires a raster layer"
    );
    let material_index = state.focused_material_index();
    let surface = PaintSurfaceId::raster(material_index.into(), layer_id);
    let snapshot = document
        .tiles
        .read_surface_full(surface)
        .map_err(|err| anyhow!("reading layer pixels: {err:#}"))?;
    let mut rgba8 = snapshot.rgba8;

    if document.active_selection.enabled {
        let _selection = document
            .active_selection
            .material_mask(material_index.into())
            .filter(|mask| mask.mask_id.is_some())
            .ok_or_else(|| anyhow!("the active selection is empty for this material"))?;
        let mask = document
            .selection_masks
            .mask_bytes_or_zeros(material_index.into(), snapshot.texture_size)
            .map_err(|err| anyhow!("reading selection pixels: {err:#}"))?;
        apply_selection_mask(&mut rgba8, &mask)?;
    }

    let (origin, size, mut cropped) =
        crop_to_alpha(snapshot.texture_size, &rgba8).ok_or_else(|| {
            anyhow!("the copied layer has no visible pixels inside the active selection")
        })?;
    unpremultiply_rgba8(&mut cropped);
    let layer_name = document
        .layer_tree
        .get(layer_id)
        .map(|node| node.props.name.clone())
        .ok_or_else(|| anyhow!("copied layer is missing"))?;
    Ok(CopiedLayerImage {
        layer_name,
        canvas_size: snapshot.texture_size,
        origin,
        size,
        rgba8: cropped,
    })
}

fn copy_layer_mask(state: &AppState, layer_id: LayerId) -> Result<CopiedLayerMask> {
    let document = state
        .document()
        .ok_or_else(|| anyhow!("copy requires a loaded document"))?;
    ensure!(
        document.layer_tree.has_layer_mask(layer_id),
        "copy requires an existing layer mask"
    );
    let material_index = state.focused_material_index();
    let surface = PaintSurfaceId::layer_mask(material_index.into(), layer_id);
    let snapshot = document
        .tiles
        .read_surface_full(surface)
        .map_err(|err| anyhow!("reading layer mask pixels: {err:#}"))?;
    let mask_r8 = snapshot
        .rgba8
        .chunks_exact(4)
        .map(|pixel| pixel[3])
        .collect::<Vec<_>>();

    let (origin, size, mask_r8, coverage_r8) = if document.active_selection.enabled {
        let _selection = document
            .active_selection
            .material_mask(material_index.into())
            .filter(|mask| mask.mask_id.is_some())
            .ok_or_else(|| anyhow!("the active selection is empty for this material"))?;
        let coverage = document
            .selection_masks
            .mask_bytes_or_zeros(material_index.into(), snapshot.texture_size)
            .map_err(|err| anyhow!("reading selection pixels: {err:#}"))?;
        crop_mask_to_coverage(snapshot.texture_size, &mask_r8, &coverage)
            .ok_or_else(|| anyhow!("the active selection is empty for this material"))?
    } else {
        let coverage_r8 = vec![255; r8_len(snapshot.texture_size)?];
        ([0, 0], snapshot.texture_size, mask_r8, coverage_r8)
    };

    let layer_name = document
        .layer_tree
        .get(layer_id)
        .map(|node| format!("{} Mask", node.props.name))
        .ok_or_else(|| anyhow!("copied layer is missing"))?;
    Ok(CopiedLayerMask {
        layer_name,
        payload: LayerMaskClipboardPayload {
            canvas_size: snapshot.texture_size,
            origin,
            size,
            mask_r8,
            coverage_r8,
        },
    })
}

fn apply_selection_mask(rgba8: &mut [u8], mask: &[u8]) -> Result<()> {
    ensure!(
        rgba8.len() == mask.len().saturating_mul(4),
        "selection mask length does not match layer pixels"
    );
    for (pixel, &coverage) in rgba8.chunks_exact_mut(4).zip(mask) {
        let coverage = u16::from(coverage);
        for channel in pixel {
            *channel = ((u16::from(*channel) * coverage + 127) / 255) as u8;
        }
    }
    Ok(())
}

fn crop_mask_to_coverage(
    texture_size: [u32; 2],
    mask_r8: &[u8],
    coverage_r8: &[u8],
) -> Option<([u32; 2], [u32; 2], Vec<u8>, Vec<u8>)> {
    let width = usize::try_from(texture_size[0]).ok()?;
    let height = usize::try_from(texture_size[1]).ok()?;
    let len = width.checked_mul(height)?;
    if mask_r8.len() != len || coverage_r8.len() != len {
        return None;
    }

    let mut min = texture_size;
    let mut max = [0, 0];
    let mut found = false;
    for y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let coverage = coverage_r8[y as usize * width + x as usize];
            if coverage != 0 {
                min[0] = min[0].min(x);
                min[1] = min[1].min(y);
                max[0] = max[0].max(x + 1);
                max[1] = max[1].max(y + 1);
                found = true;
            }
        }
    }
    if !found {
        return None;
    }

    let size = [max[0] - min[0], max[1] - min[1]];
    let cropped_len = r8_len(size).ok()?;
    let mut cropped_mask = Vec::with_capacity(cropped_len);
    let mut cropped_coverage = Vec::with_capacity(cropped_len);
    for y in min[1]..max[1] {
        let start = y as usize * width + min[0] as usize;
        let end = y as usize * width + max[0] as usize;
        cropped_mask.extend_from_slice(&mask_r8[start..end]);
        cropped_coverage.extend_from_slice(&coverage_r8[start..end]);
    }
    Some((min, size, cropped_mask, cropped_coverage))
}

fn crop_to_alpha(texture_size: [u32; 2], rgba8: &[u8]) -> Option<([u32; 2], [u32; 2], Vec<u8>)> {
    let width = usize::try_from(texture_size[0]).ok()?;
    let height = usize::try_from(texture_size[1]).ok()?;
    if rgba8.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let mut min = texture_size;
    let mut max = [0, 0];
    let mut found = false;
    for y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let alpha = rgba8[(y as usize * width + x as usize) * 4 + 3];
            if alpha != 0 {
                min[0] = min[0].min(x);
                min[1] = min[1].min(y);
                max[0] = max[0].max(x + 1);
                max[1] = max[1].max(y + 1);
                found = true;
            }
        }
    }
    if !found {
        return None;
    }
    let size = [max[0] - min[0], max[1] - min[1]];
    let mut cropped = Vec::with_capacity(
        usize::try_from(size[0])
            .ok()?
            .checked_mul(usize::try_from(size[1]).ok()?)?
            .checked_mul(4)?,
    );
    for y in min[1]..max[1] {
        let start = (y as usize * width + min[0] as usize) * 4;
        let end = (y as usize * width + max[0] as usize) * 4;
        cropped.extend_from_slice(&rgba8[start..end]);
    }
    Some((min, size, cropped))
}

fn r8_len(size: [u32; 2]) -> Result<usize> {
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
        application::{layer_reducer, state::EditorDocumentState},
        core::{
            document::{Document, MaterialSpec, MeshData},
            surface::LayerMaskInitMode,
        },
    };

    #[test]
    fn selection_mask_scales_premultiplied_rgba() {
        let mut rgba = vec![100, 50, 25, 128, 20, 10, 5, 255];
        apply_selection_mask(&mut rgba, &[128, 0]).unwrap();
        assert_eq!(rgba, [50, 25, 13, 64, 0, 0, 0, 0]);
    }

    #[test]
    fn crop_uses_nonzero_alpha_bounds() {
        let mut rgba = vec![0; 4 * 3 * 4];
        rgba[(1 * 4 + 2) * 4..(1 * 4 + 2) * 4 + 4].copy_from_slice(&[1, 2, 3, 4]);
        rgba[(2 * 4 + 3) * 4..(2 * 4 + 3) * 4 + 4].copy_from_slice(&[5, 6, 7, 8]);
        let (origin, size, cropped) = crop_to_alpha([4, 3], &rgba).unwrap();
        assert_eq!(origin, [2, 1]);
        assert_eq!(size, [2, 2]);
        assert_eq!(cropped.len(), 16);
    }

    #[test]
    fn mask_crop_uses_selection_coverage_not_mask_value() {
        let mask = vec![0, 0, 0, 0, 0, 0, 0, 0];
        let coverage = vec![0, 0, 0, 0, 0, 255, 128, 0];
        let (origin, size, cropped_mask, cropped_coverage) =
            crop_mask_to_coverage([4, 2], &mask, &coverage).unwrap();
        assert_eq!(origin, [1, 1]);
        assert_eq!(size, [2, 1]);
        assert_eq!(cropped_mask, vec![0, 0]);
        assert_eq!(cropped_coverage, vec![255, 128]);
    }

    #[test]
    fn layer_mask_clipboard_rgba_keeps_mask_and_coverage_separate() {
        let payload = LayerMaskClipboardPayload {
            canvas_size: [2, 1],
            origin: [0, 0],
            size: [2, 1],
            mask_r8: vec![0, 200],
            coverage_r8: vec![255, 128],
        };
        assert_eq!(
            payload.clipboard_rgba8().unwrap(),
            vec![0, 0, 0, 255, 200, 200, 200, 128]
        );
    }

    #[test]
    fn copying_black_layer_mask_without_selection_keeps_full_canvas() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![MaterialSpec::new("A", [2, 2])],
        ));
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        layer_reducer::add_layer_mask(&mut state, layer_id, LayerMaskInitMode::HideAll).unwrap();

        let CopiedLayerContent::LayerMask(copied) = copy_layer_content(&state, layer_id).unwrap()
        else {
            panic!("expected layer mask copy");
        };
        assert_eq!(copied.payload.origin, [0, 0]);
        assert_eq!(copied.payload.size, [2, 2]);
        assert_eq!(copied.payload.mask_r8, vec![0; 4]);
        assert_eq!(copied.payload.coverage_r8, vec![255; 4]);
    }

    #[test]
    fn cutting_whole_layer_mask_deletes_the_mask_node() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![MaterialSpec::new("A", [2, 2])],
        ));
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        layer_reducer::add_layer_mask(&mut state, layer_id, LayerMaskInitMode::RevealAll).unwrap();

        let prepared = prepare_cut_layer_content(&state, layer_id).unwrap();
        assert_eq!(
            prepared.action,
            PreparedLayerCutAction::DeleteLayerMask(layer_id)
        );
    }

    #[test]
    fn cutting_selected_layer_mask_pixels_keeps_the_mask_node() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![MaterialSpec::new("A", [2, 2])],
        ));
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        layer_reducer::add_layer_mask(&mut state, layer_id, LayerMaskInitMode::RevealAll).unwrap();

        let mask_id = state
            .document
            .document
            .as_mut()
            .unwrap()
            .active_selection
            .enable_material_mask(0.into());
        assert_eq!(
            mask_id,
            crate::core::selection::selection_mask_id_for_material(0.into())
        );
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .selection_masks
            .write_mask(0.into(), [2, 2], vec![255, 0, 0, 0])
            .unwrap();

        let prepared = prepare_cut_layer_content(&state, layer_id).unwrap();
        assert_eq!(
            prepared.action,
            PreparedLayerCutAction::ClearSurface(PaintSurfaceId::layer_mask(0.into(), layer_id))
        );
    }
}
