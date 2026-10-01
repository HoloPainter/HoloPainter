use std::collections::HashSet;

use anyhow::{Result, anyhow, ensure};

use crate::{
    core::{
        document::{ActiveLayerTarget, LayerInsertion},
        image::LayerInitialPixels,
        surface::PaintSurfaceId,
    },
    renderer::GpuDocumentCommand,
};

use super::{
    AppState, CompositeSync, EmbeddedImageImportPayload, ImportedImagePayload, LayerTreeSnapshot,
    PendingCpuHistoryTransaction, ReducerOutput, StatusMessage, transform_controller,
};

pub(crate) fn begin_embedded(
    state: &mut AppState,
    image: EmbeddedImageImportPayload,
) -> Result<ReducerOutput> {
    ensure_image_import_allowed(state)?;
    let (before, embedded_image) = {
        let (document, editor_document) = state
            .document_and_editor_document_mut()
            .ok_or_else(|| anyhow!("image import requires a loaded document"))?;
        let active = editor_document.resolved(document);
        let before = LayerTreeSnapshot {
            tree: document.layer_tree.clone(),
            active_layer_id: active.active_layer_id,
            active_part: active.active_part,
        };
        let focused_material = editor_document.focused_material_index;
        let layer_name = unique_layer_name(&document.layer_tree, &image.layer_name);
        let insertion = if active.active_target(document) == ActiveLayerTarget::Structure
            && document.layer_tree.is_group(active.active_layer_id)
        {
            LayerInsertion::IntoGroup(active.active_layer_id)
        } else {
            LayerInsertion::Above(active.active_layer_id)
        };
        let added = document
            .add_embedded_image_layer(
                insertion,
                layer_name,
                image.file_name,
                image.size,
                image.rgba8_straight,
                focused_material,
            )?
            .ok_or_else(|| anyhow!("image import could not insert an embedded image layer"))?;
        editor_document.active_layer_id = added.layer_id;
        editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        let embedded_image = document
            .embedded_image(added.image_id)
            .expect("new embedded image asset must exist")
            .clone();
        (before, embedded_image)
    };
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::UpsertEmbeddedImage {
        image: embedded_image.clone(),
    });
    state.tool.invalidate_transform_idle_cache();
    state.set_status_message(
        StatusMessage::localized("status-imported-layer")
            .arg("file_name", &embedded_image.file_name),
    );
    Ok(output
        .with_optional_named_cpu_history_transaction(
            "Import Embedded Image",
            Some(PendingCpuHistoryTransaction::LayerTree {
                before,
                before_images: Vec::new(),
                synthesize_transparent_after_images: false,
                retained_embedded_images: vec![embedded_image],
            }),
        )
        .with_composite_sync(CompositeSync::MaterialTrees))
}

struct InsertedImageLayer {
    before: LayerTreeSnapshot,
    target: PaintSurfaceId,
    target_texture_size: [u32; 2],
    added_surfaces: Vec<PaintSurfaceId>,
    output: ReducerOutput,
}

pub(crate) fn begin(state: &mut AppState, image: ImportedImagePayload) -> Result<ReducerOutput> {
    ensure_image_import_allowed(state)?;
    let inserted = insert_image_layer(state, &image)?;
    let InsertedImageLayer {
        before,
        target,
        target_texture_size,
        added_surfaces,
        mut output,
    } = inserted;

    state.tool.begin_transform_mode();
    match transform_controller::begin_imported_image_transform_session(
        state,
        target,
        target_texture_size,
        image,
        before.clone(),
        added_surfaces.clone(),
    ) {
        Ok(transform_output) => {
            output.append_outcome(transform_output);
            output = output.with_composite_sync(CompositeSync::MaterialTrees);
            state.set_status_key("status-image-loaded-transform-ready");
            Ok(output)
        }
        Err(err) => {
            let rollback = rollback_image_layer(state, before);
            state.tool.clear_modal_tool();
            rollback.map_err(|rollback_err| {
                anyhow!("{err:#}; image import rollback failed: {rollback_err:#}")
            })?;
            Err(err)
        }
    }
}

pub(crate) fn paste(state: &mut AppState, image: ImportedImagePayload) -> Result<ReducerOutput> {
    ensure_image_import_allowed(state)?;
    let inserted = insert_image_layer(state, &image)?;
    let InsertedImageLayer {
        before,
        target,
        target_texture_size,
        added_surfaces,
        mut output,
    } = inserted;

    match transform_controller::commit_imported_image(
        target,
        target_texture_size,
        image,
        before.clone(),
        added_surfaces.clone(),
    ) {
        Ok(transform_output) => {
            output.append_outcome(transform_output);
            output = output.with_composite_sync(CompositeSync::MaterialTrees);
            state.tool.invalidate_transform_idle_cache();
            state.set_status_key("status-image-pasted");
            Ok(output)
        }
        Err(err) => {
            rollback_image_layer(state, before).map_err(|rollback_err| {
                anyhow!("{err:#}; image paste rollback failed: {rollback_err:#}")
            })?;
            Err(err)
        }
    }
}

fn ensure_image_import_allowed(state: &AppState) -> Result<()> {
    ensure!(
        state.document().is_some(),
        "image import requires a loaded document"
    );
    ensure!(
        !state.is_tool_interacting(),
        "finish or cancel the current operation before importing an image"
    );
    Ok(())
}

fn insert_image_layer(
    state: &mut AppState,
    image: &ImportedImagePayload,
) -> Result<InsertedImageLayer> {
    let (document, editor_document) = state
        .document_and_editor_document_mut()
        .ok_or_else(|| anyhow!("image import requires a loaded document"))?;
    let active = editor_document.resolved(document);
    let before = LayerTreeSnapshot {
        tree: document.layer_tree.clone(),
        active_layer_id: active.active_layer_id,
        active_part: active.active_part,
    };
    let focused_material_index = editor_document.focused_material_index;
    let target_texture_size = document
        .materials
        .get(focused_material_index.as_usize())
        .map(|material| material.texture_size)
        .ok_or_else(|| anyhow!("image import target material is missing"))?;
    let material_sizes = document
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| (material_index, material.texture_size))
        .collect::<Vec<_>>();
    ensure!(
        !material_sizes.is_empty(),
        "image import requires a material"
    );
    let layer_name = unique_layer_name(&document.layer_tree, &image.layer_name);
    let insertion = if active.active_target(document) == ActiveLayerTarget::Structure
        && document.layer_tree.is_group(active.active_layer_id)
    {
        LayerInsertion::IntoGroup(active.active_layer_id)
    } else {
        LayerInsertion::Above(active.active_layer_id)
    };
    let added = document
        .add_raster_layer(insertion, layer_name)
        .map_err(|err| anyhow!("image import surface creation failed: {err:#}"))?
        .ok_or_else(|| anyhow!("image import could not insert a raster layer"))?;
    let layer_id = added.layer_id;

    editor_document.active_layer_id = layer_id;
    editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
    let target = PaintSurfaceId::raster(focused_material_index.into(), layer_id);
    let added_surfaces = added.created_surfaces;
    let mut output = ReducerOutput::default();
    for (material_index, _) in material_sizes {
        let surface = PaintSurfaceId::raster(material_index.into(), layer_id);
        output.push_document_command(GpuDocumentCommand::CreateSurface {
            target: surface,
            initial: LayerInitialPixels::Transparent,
        });
    }

    Ok(InsertedImageLayer {
        before,
        target,
        target_texture_size,
        added_surfaces,
        output,
    })
}

fn rollback_image_layer(state: &mut AppState, before: LayerTreeSnapshot) -> Result<()> {
    let (document, editor_document) = state
        .document_and_editor_document_mut()
        .ok_or_else(|| anyhow!("image import document is missing during rollback"))?;
    document.restore_layer_tree(before.tree.clone(), &[])?;
    editor_document.active_layer_id = before.active_layer_id;
    editor_document.active_part = before.active_part;
    Ok(())
}

fn unique_layer_name(tree: &crate::core::surface::LayerTree, requested: &str) -> String {
    let base = requested.trim();
    let base = if base.is_empty() {
        "Imported Image"
    } else {
        base
    };
    let used = tree
        .rows()
        .into_iter()
        .filter_map(|row| tree.get(row.layer_id))
        .map(|node| node.props.name.as_str())
        .collect::<HashSet<_>>();
    if !used.contains(base) {
        return base.to_owned();
    }
    for index in 2usize.. {
        let candidate = format!("{base} ({index})");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("usize iteration should not exhaust while naming imported layers")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::surface::LayerTree;

    #[test]
    fn imported_layer_name_uses_parenthesized_suffixes() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        tree.rename_layer(base, "image".to_owned());
        tree.add_group_above(base, "image (2)").unwrap();

        assert_eq!(unique_layer_name(&tree, "image"), "image (3)");
    }
}
