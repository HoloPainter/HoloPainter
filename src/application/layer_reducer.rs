use std::collections::{BTreeSet, HashSet};

#[cfg(test)]
use super::CompositeSync;
use super::{CompositeImpact, ReducerOutput, state::AppState};
use crate::{
    application::{
        HistoryAtom, InitialPixels, LayerCompositeSettings, LayerImageSnapshot, LayerTreeEdit,
        LayerTreeSnapshot, PendingCpuHistoryTransaction, PendingLayerPropertyEdit,
        PendingPixelEdit, PendingRendererHistoryTransaction, RectU32, RendererCommitSpec,
    },
    core::{
        adjustment::{Adjustment, AdjustmentKind},
        composite::{GroupCompositeMode, LayerBlendMode},
        document::{ActiveLayerTarget, Document, LayerInsertion},
        image::LayerInitialPixels,
        surface::{
            LayerContent, LayerId, LayerMaskInitMode, LayerMaterialMask, PaintSurfaceId,
            RemoveLayerResult,
        },
        tile_payload::TilePayload,
    },
    renderer::{CompositeBakeTarget, CompositeCommand, EditCommand, GpuDocumentCommand},
};
use anyhow::{Result, anyhow};

pub fn add_layer(state: &mut AppState) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), true);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let active = editor_document.resolved(doc);
        let name = next_numbered_layer_name(&doc.layer_tree, "Layer");
        let insertion = if active.active_target(doc) == ActiveLayerTarget::Structure
            && doc.layer_tree.is_group(active.active_layer_id)
        {
            LayerInsertion::IntoGroup(active.active_layer_id)
        } else {
            LayerInsertion::Above(active.active_layer_id)
        };
        if let Some(added) = doc
            .add_raster_layer(insertion, name)
            .map_err(|err| anyhow!("Document layer surface create failed: {err:#}"))?
        {
            let id = added.layer_id;
            editor_document.active_layer_id = id;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
            let effects = added
                .created_surfaces
                .into_iter()
                .map(|target| create_surface_command(target, LayerInitialPixels::Transparent))
                .collect();
            return Ok(with_structure_impact(with_optional_history(
                effects,
                history_transaction,
            )));
        }
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn add_solid_fill_layer(state: &mut AppState, color: [f32; 3]) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let active = editor_document.resolved(doc);
        let name = next_numbered_layer_name(&doc.layer_tree, "Fill Layer");
        let new_id = if active.active_target(doc) == ActiveLayerTarget::Structure
            && doc.layer_tree.is_group(active.active_layer_id)
        {
            doc.layer_tree
                .add_solid_fill_layer_to_group(active.active_layer_id, name, color)
        } else {
            doc.layer_tree
                .add_solid_fill_layer_above(active.active_layer_id, name, color)
        };
        if let Some(id) = new_id {
            editor_document.active_layer_id = id;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn add_adjustment_layer(state: &mut AppState, kind: AdjustmentKind) -> Result<ReducerOutput> {
    let focused_material = if kind == AdjustmentKind::UvMirror {
        let material_index = state.focused_material_index();
        let Some(material_id) = state
            .document()
            .and_then(|document| document.material_id(material_index.into()))
        else {
            return Ok(ReducerOutput::default());
        };
        Some((material_index, material_id))
    } else {
        None
    };
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let active = editor_document.resolved(doc);
        let name = next_numbered_layer_name(&doc.layer_tree, adjustment_default_name(kind));
        let adjustment = kind.default_adjustment();
        let new_id = if active.active_target(doc) == ActiveLayerTarget::Structure
            && doc.layer_tree.is_group(active.active_layer_id)
        {
            doc.layer_tree
                .add_adjustment_layer_to_group(active.active_layer_id, name, adjustment)
        } else {
            doc.layer_tree
                .add_adjustment_layer_above(active.active_layer_id, name, adjustment)
        };
        if let Some(id) = new_id {
            if let Some((_, material_id)) = focused_material {
                doc.layer_tree.set_layer_material_mask(
                    id,
                    LayerMaterialMask::Specified(BTreeSet::from([material_id])),
                );
            }
            editor_document.active_layer_id = id;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }
    }
    let output = with_optional_history(Vec::new(), history_transaction);
    Ok(if let Some((material_index, _)) = focused_material {
        with_composite_impact(
            output,
            state.document(),
            CompositeImpact::MaterialTreesScoped(BTreeSet::from([material_index.into()])),
        )
    } else {
        with_structure_impact(output)
    })
}

pub fn add_group(state: &mut AppState) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let active = editor_document.resolved(doc);
        let name = next_numbered_layer_name(&doc.layer_tree, "Group");
        let new_group = if active.active_target(doc) == ActiveLayerTarget::Structure
            && doc.layer_tree.is_group(active.active_layer_id)
        {
            doc.layer_tree
                .add_group_to_group(active.active_layer_id, name)
        } else {
            let anchor = if doc.layer_tree.contains(active.active_layer_id) {
                active.active_layer_id
            } else {
                doc.layer_tree
                    .default_raster_layer()
                    .unwrap_or(doc.layer_tree.root())
            };
            doc.layer_tree.add_group_above(anchor, name)
        };
        if let Some(id) = new_group {
            editor_document.active_layer_id = id;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

fn next_numbered_layer_name(tree: &crate::core::surface::LayerTree, prefix: &str) -> String {
    let mut used_names = HashSet::new();
    for row in tree.rows() {
        if let Some(node) = tree.get(row.layer_id) {
            used_names.insert(node.props.name.as_str());
        }
    }

    for index in 1usize.. {
        let candidate = format!("{prefix} {index}");
        if !used_names.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("usize iteration should not exhaust while naming layers")
}

fn adjustment_default_name(kind: AdjustmentKind) -> &'static str {
    match kind {
        AdjustmentKind::BrightnessContrast => "Brightness / Contrast",
        AdjustmentKind::Levels => "Levels",
        AdjustmentKind::Curves => "Curves",
        AdjustmentKind::HueSaturation => "Hue / Saturation",
        AdjustmentKind::Invert => "Invert",
        AdjustmentKind::GradientMap => "Gradient Map",
        AdjustmentKind::UvMirror => "UV Mirror",
    }
}

pub fn delete_layer(state: &mut AppState, layer_id: LayerId) -> Result<ReducerOutput> {
    let history_transaction = state
        .document()
        .map(|document| {
            capture_layer_images(document, document.layer_surfaces_in_subtree(layer_id)).map(
                |before_images| PendingCpuHistoryTransaction::LayerTree {
                    before: layer_tree_snapshot(state, document),
                    before_images,
                    synthesize_transparent_after_images: false,
                    retained_embedded_images: document
                        .embedded_images_referenced_by(&document.layer_tree),
                },
            )
        })
        .transpose()
        .map_err(|err| anyhow!("Layer delete snapshot failed: {err:#}"))?;
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let (removed, targets) = doc
            .remove_layer_with_surfaces(layer_id)
            .map_err(|err| anyhow!("Document layer surface delete failed: {err:#}"))?;
        let active_layer = editor_document.active_layer_id;
        if !doc.layer_tree.contains(active_layer)
            && let Some(replacement) = doc.layer_tree.rows().first().map(|row| row.layer_id)
        {
            editor_document.active_layer_id = replacement;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }
        if matches!(removed, RemoveLayerResult::Removed) {
            let mut effects = targets
                .into_iter()
                .map(delete_surface_command)
                .collect::<Vec<_>>();
            effects.extend(
                doc.prune_unreferenced_embedded_images()
                    .into_iter()
                    .map(|image| GpuDocumentCommand::RemoveEmbeddedImage { image_id: image.id }),
            );
            return Ok(with_structure_impact(with_optional_history(
                effects,
                history_transaction,
            )));
        }
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn delete_layers(state: &mut AppState, layer_ids: Vec<LayerId>) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let layer_ids = effective_layer_roots(&document.layer_tree, &layer_ids);
    if layer_ids.is_empty() {
        return Ok(ReducerOutput::default());
    }
    let targets = layer_ids
        .iter()
        .flat_map(|layer_id| document.layer_surfaces_in_subtree(*layer_id))
        .collect::<Vec<_>>();
    let history_transaction = capture_layer_images(document, targets.clone())
        .map(|before_images| PendingCpuHistoryTransaction::LayerTree {
            before: layer_tree_snapshot(state, document),
            before_images,
            synthesize_transparent_after_images: false,
            retained_embedded_images: document.embedded_images_referenced_by(&document.layer_tree),
        })
        .map(Some)
        .map_err(|err| anyhow!("Layer delete snapshot failed: {err:#}"))?;

    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let removing_rasters = layer_ids
            .iter()
            .map(|layer_id| doc.layer_tree.raster_layers_in_subtree(*layer_id).len())
            .sum::<usize>();
        if removing_rasters >= doc.layer_tree.ordered_raster_layers().len() {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        let before_rows = doc
            .layer_tree
            .rows()
            .into_iter()
            .map(|row| row.layer_id)
            .collect::<Vec<_>>();
        let first_removed_index = before_rows
            .iter()
            .position(|layer_id| layer_ids.contains(layer_id))
            .unwrap_or(0);
        let Some(targets) = doc
            .remove_layers_with_surfaces(&layer_ids)
            .map_err(|err| anyhow!("Document layer surface delete failed: {err:#}"))?
        else {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        };

        if !doc.layer_tree.contains(editor_document.active_layer_id) {
            let after_rows = doc
                .layer_tree
                .rows()
                .into_iter()
                .map(|row| row.layer_id)
                .collect::<Vec<_>>();
            let replacement = after_rows
                .get(first_removed_index.min(after_rows.len().saturating_sub(1)))
                .copied()
                .or_else(|| doc.layer_tree.first_paintable_layer());
            if let Some(layer_id) = replacement {
                editor_document.active_layer_id = layer_id;
                editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
            }
        }

        let mut effects = targets
            .into_iter()
            .map(delete_surface_command)
            .collect::<Vec<_>>();
        effects.extend(
            doc.prune_unreferenced_embedded_images()
                .into_iter()
                .map(|image| GpuDocumentCommand::RemoveEmbeddedImage { image_id: image.id }),
        );
        return Ok(with_structure_impact(with_optional_history(
            effects,
            history_transaction,
        )));
    }

    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn duplicate_layer(state: &mut AppState, layer_id: LayerId) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        if layer_id == doc.layer_tree.root() || !doc.layer_tree.contains(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        if let Some(duplicate) = doc.layer_tree.duplicate_layer(layer_id) {
            let new_id = duplicate.root;
            let previous_active = *editor_document;
            editor_document.active_layer_id = new_id;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
            let material_count = doc.materials.len();
            let mut duplicated_surfaces = Vec::new();
            let mut effects = Vec::new();
            let layer_map = duplicate.layer_map;
            for material_index in 0..material_count {
                for &(from_layer, to_layer) in &layer_map {
                    let mut surface_pairs = Vec::new();
                    if doc.layer_tree.is_paintable(from_layer) {
                        surface_pairs.push((
                            PaintSurfaceId::raster(material_index.into(), from_layer),
                            PaintSurfaceId::raster(material_index.into(), to_layer),
                        ));
                    }
                    if doc.layer_tree.has_layer_mask(from_layer) {
                        surface_pairs.push((
                            PaintSurfaceId::layer_mask(material_index.into(), from_layer),
                            PaintSurfaceId::layer_mask(material_index.into(), to_layer),
                        ));
                    }
                    for (from, to) in surface_pairs {
                        let changed = match doc.tiles.duplicate_surface(from, to) {
                            Ok(changed) => changed,
                            Err(err) => {
                                rollback_duplicate_layer(doc, duplicated_surfaces, new_id);
                                *editor_document = previous_active;
                                return Err(anyhow!("Document layer duplicate failed: {err:#}"));
                            }
                        };
                        duplicated_surfaces.push(to);
                        effects.push(create_surface_command(to, LayerInitialPixels::Transparent));
                        effects.push(upload_surface_tiles_command(
                            to,
                            changed.surface_revision,
                            changed.tiles,
                        ));
                    }
                }
            }
            return Ok(with_structure_impact(with_optional_history(
                effects,
                history_transaction,
            )));
        }
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn merge_layers(state: &mut AppState, layer_ids: Vec<LayerId>) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let effective_roots = effective_layer_roots(&document.layer_tree, &layer_ids);
    let Some(plan) = document.layer_tree.layer_merge_plan(&effective_roots) else {
        return Ok(ReducerOutput::default());
    };
    if document.materials.is_empty() {
        return Ok(ReducerOutput::default());
    }
    let source_surfaces = plan
        .layer_ids
        .iter()
        .flat_map(|layer_id| document.layer_surfaces_in_subtree(*layer_id))
        .collect::<Vec<_>>();
    let before_images = capture_layer_images(document, source_surfaces.clone())
        .map_err(|err| anyhow!("Layer merge snapshot failed: {err:#}"))?;
    let cpu_history = Some(PendingCpuHistoryTransaction::LayerTree {
        before: layer_tree_snapshot(state, document),
        before_images,
        synthesize_transparent_after_images: false,
        retained_embedded_images: document.embedded_images_referenced_by(&document.layer_tree),
    });
    let bake_trees = document
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, _)| {
            document.composite_tree_for_layers(material_index, &plan.layer_ids)
        })
        .collect::<Vec<_>>();

    let Some((doc, editor_document)) = state.document_and_editor_document_mut() else {
        return Ok(ReducerOutput::default());
    };
    let Some(merged) = doc
        .replace_layers_with_raster(&plan)
        .map_err(|err| anyhow!("Layer merge replacement failed: {err:#}"))?
    else {
        return Ok(ReducerOutput::default());
    };
    let merged_layer_id = merged.layer_id;
    debug_assert_eq!(merged.deleted_surfaces, source_surfaces);

    let material_sizes = doc
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| (material_index, material.texture_size))
        .collect::<Vec<_>>();
    let mut target_surfaces = Vec::with_capacity(material_sizes.len());
    let mut pending_pixel_edits = Vec::with_capacity(material_sizes.len());
    let mut document_commands = Vec::with_capacity(material_sizes.len());
    for (material_index, size) in material_sizes {
        let target = PaintSurfaceId::raster(material_index.into(), merged_layer_id);
        let rect = RectU32::full(size);
        let before = doc
            .tiles
            .read_surface_rect(target, rect)
            .map_err(|err| anyhow!("Layer merge undo snapshot failed: {err:#}"))?;
        target_surfaces.push(target);
        pending_pixel_edits.push(PendingPixelEdit {
            surface: target,
            texture_size: size,
            rect,
            before,
        });
        document_commands.push(create_surface_command(
            target,
            LayerInitialPixels::Transparent,
        ));
    }
    editor_document.active_layer_id = merged_layer_id;
    editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
    let delete_embedded_images_after_bake = doc
        .prune_unreferenced_embedded_images()
        .into_iter()
        .map(|image| image.id)
        .collect();

    let bake_targets = bake_trees
        .into_iter()
        .enumerate()
        .map(|(material_index, tree)| CompositeBakeTarget {
            target: PaintSurfaceId::raster(material_index.into(), merged_layer_id),
            tree,
        })
        .collect::<Vec<_>>();
    let renderer_history = Some(PendingRendererHistoryTransaction::Pixel {
        edits: pending_pixel_edits,
    });
    let output = ReducerOutput::default()
        .with_optional_named_cpu_history_transaction("Merge Layers", cpu_history)
        .with_optional_renderer_history_transaction("Merge Layers", renderer_history)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(target_surfaces));
    let mut output = with_structure_impact(output);
    for command in document_commands {
        output.push_document_command(command);
    }
    output.push_edit_command(EditCommand::Composite(CompositeCommand::BakeToSurfaces {
        targets: bake_targets,
        delete_sources_after_bake: source_surfaces,
        delete_embedded_images_after_bake,
    }));
    Ok(output)
}

pub fn rasterize_layer(state: &mut AppState, layer_id: LayerId) -> Result<ReducerOutput> {
    bake_layer_to_raster(state, layer_id, false, "Rasterize Layer")
}

pub fn apply_layer_mask(state: &mut AppState, layer_id: LayerId) -> Result<ReducerOutput> {
    bake_layer_to_raster(state, layer_id, true, "Apply Layer Mask")
}

fn bake_layer_to_raster(
    state: &mut AppState,
    layer_id: LayerId,
    apply_mask: bool,
    history_label: &'static str,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let allowed = if apply_mask {
        document.layer_tree.can_apply_layer_mask(layer_id)
    } else {
        document.layer_tree.can_rasterize_layer(layer_id)
    };
    if !allowed || document.materials.is_empty() {
        return Ok(ReducerOutput::default());
    }

    let source_is_raster = document
        .layer_tree
        .get(layer_id)
        .is_some_and(|node| matches!(node.content, LayerContent::Raster));
    let bake_trees = document
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, _)| {
            document
                .composite_tree_for_layer_bake(material_index, layer_id, apply_mask)
                .expect("validated layer content must have a bake tree")
        })
        .collect::<Vec<_>>();
    let mask_surfaces = apply_mask
        .then(|| document.layer_mask_surfaces(layer_id))
        .unwrap_or_default();
    let before_images = capture_layer_images(document, mask_surfaces.clone())
        .map_err(|err| anyhow!("{history_label} mask snapshot failed: {err:#}"))?;
    let cpu_history = Some(PendingCpuHistoryTransaction::LayerTree {
        before: layer_tree_snapshot(state, document),
        before_images,
        synthesize_transparent_after_images: false,
        retained_embedded_images: document.embedded_images_referenced_by(&document.layer_tree),
    });

    let Some((doc, editor_document)) = state.document_and_editor_document_mut() else {
        return Ok(ReducerOutput::default());
    };
    let previous_document = doc.clone();
    let previous_editor_document = *editor_document;
    let material_sizes = doc
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| (material_index, material.texture_size))
        .collect::<Vec<_>>();
    let target_surfaces = material_sizes
        .iter()
        .map(|(material_index, _)| PaintSurfaceId::raster((*material_index).into(), layer_id))
        .collect::<Vec<_>>();
    let created_surfaces = if source_is_raster {
        Vec::new()
    } else {
        material_sizes
            .iter()
            .map(|(material_index, size)| {
                (
                    PaintSurfaceId::raster((*material_index).into(), layer_id),
                    *size,
                    InitialPixels::Transparent,
                )
            })
            .collect()
    };
    if let Err(err) = doc
        .tiles
        .change_surfaces_atomically(&mask_surfaces, created_surfaces)
    {
        return Err(anyhow!("{history_label} surface change failed: {err:#}"));
    }
    let changed = if apply_mask {
        doc.layer_tree.apply_layer_mask(layer_id)
    } else {
        doc.layer_tree.rasterize_layer(layer_id)
    };
    if !changed {
        *doc = previous_document;
        *editor_document = previous_editor_document;
        return Ok(ReducerOutput::default());
    }
    if apply_mask
        && editor_document.active_layer_id == layer_id
        && editor_document.active_part == crate::core::document::ActiveLayerPart::LayerMask
    {
        editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
    }
    let delete_embedded_images_after_bake = doc
        .prune_unreferenced_embedded_images()
        .into_iter()
        .map(|image| image.id)
        .collect::<Vec<_>>();

    let mut pending_pixel_edits = Vec::with_capacity(material_sizes.len());
    for ((_, size), target) in material_sizes.iter().zip(&target_surfaces) {
        let rect = RectU32::full(*size);
        let before = match doc.tiles.read_surface_rect(*target, rect) {
            Ok(before) => before,
            Err(err) => {
                *doc = previous_document;
                *editor_document = previous_editor_document;
                return Err(anyhow!("{history_label} undo snapshot failed: {err:#}"));
            }
        };
        pending_pixel_edits.push(PendingPixelEdit {
            surface: *target,
            texture_size: *size,
            rect,
            before,
        });
    }
    let renderer_history = Some(PendingRendererHistoryTransaction::Pixel {
        edits: pending_pixel_edits,
    });
    let bake_targets = bake_trees
        .into_iter()
        .zip(&target_surfaces)
        .map(|(tree, target)| CompositeBakeTarget {
            target: *target,
            tree,
        })
        .collect();
    let output = ReducerOutput::default()
        .with_optional_named_cpu_history_transaction(history_label, cpu_history)
        .with_optional_renderer_history_transaction(history_label, renderer_history)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            target_surfaces.clone(),
        ));
    let mut output = with_structure_impact(output);
    if !source_is_raster {
        for target in &target_surfaces {
            output.push_document_command(create_surface_command(
                *target,
                LayerInitialPixels::Transparent,
            ));
        }
    }
    output.push_edit_command(EditCommand::Composite(CompositeCommand::BakeToSurfaces {
        targets: bake_targets,
        delete_sources_after_bake: mask_surfaces,
        delete_embedded_images_after_bake,
    }));
    Ok(output)
}

pub fn duplicate_layers(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    primary_layer_id: LayerId,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        let layer_ids = effective_layer_roots(&doc.layer_tree, &layer_ids);
        if layer_ids.is_empty() {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }

        let previous_document = doc.clone();
        let previous_editor_document = *editor_document;
        let material_count = doc.materials.len();
        let mut duplicated_roots = Vec::new();
        let mut duplicated_surfaces = Vec::new();
        let mut effects = Vec::new();
        let mut all_layer_maps = Vec::new();

        for layer_id in &layer_ids {
            let Some(duplicate) = doc.layer_tree.duplicate_layer(*layer_id) else {
                *doc = previous_document;
                *editor_document = previous_editor_document;
                return Ok(with_structure_impact(with_optional_history(
                    Vec::new(),
                    history_transaction,
                )));
            };
            duplicated_roots.push(duplicate.root);
            all_layer_maps.extend(duplicate.layer_map);
        }

        for material_index in 0..material_count {
            for &(from_layer, to_layer) in &all_layer_maps {
                let mut surface_pairs = Vec::new();
                if doc.layer_tree.is_paintable(from_layer) {
                    surface_pairs.push((
                        PaintSurfaceId::raster(material_index.into(), from_layer),
                        PaintSurfaceId::raster(material_index.into(), to_layer),
                    ));
                }
                if doc.layer_tree.has_layer_mask(from_layer) {
                    surface_pairs.push((
                        PaintSurfaceId::layer_mask(material_index.into(), from_layer),
                        PaintSurfaceId::layer_mask(material_index.into(), to_layer),
                    ));
                }
                for (from, to) in surface_pairs {
                    let changed = match doc.tiles.duplicate_surface(from, to) {
                        Ok(changed) => changed,
                        Err(err) => {
                            *doc = previous_document;
                            *editor_document = previous_editor_document;
                            return Err(anyhow!("Document layer duplicate failed: {err:#}"));
                        }
                    };
                    duplicated_surfaces.push(to);
                    effects.push(create_surface_command(to, LayerInitialPixels::Transparent));
                    effects.push(upload_surface_tiles_command(
                        to,
                        changed.surface_revision,
                        changed.tiles,
                    ));
                }
            }
        }

        let new_primary = all_layer_maps
            .iter()
            .find_map(|(from, to)| (*from == primary_layer_id).then_some(*to))
            .or_else(|| duplicated_roots.first().copied());
        if let Some(new_primary) = new_primary {
            editor_document.active_layer_id = new_primary;
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }

        return Ok(with_structure_impact(with_optional_history(
            effects,
            history_transaction,
        )));
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

fn rollback_duplicate_layer(
    doc: &mut crate::core::document::Document,
    duplicated_surfaces: Vec<PaintSurfaceId>,
    layer_id: LayerId,
) {
    for surface in duplicated_surfaces {
        let _ = doc.tiles.delete_surface(surface);
    }
    let _ = doc.layer_tree.remove_layer(layer_id);
}

pub fn add_layer_mask(
    state: &mut AppState,
    layer_id: LayerId,
    mode: LayerMaskInitMode,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        if !doc.layer_tree.can_add_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        let previous_document = doc.clone();
        let previous_editor_document = *editor_document;
        if !doc.layer_tree.add_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        editor_document.active_layer_id = layer_id;
        editor_document.active_part = crate::core::document::ActiveLayerPart::LayerMask;

        let material_sizes = doc
            .materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| (material_index, material.texture_size))
            .collect::<Vec<_>>();
        let mut created_surfaces = Vec::new();
        let mut effects = Vec::new();
        for (material_index, texture_size) in material_sizes {
            let target = PaintSurfaceId::layer_mask(material_index.into(), layer_id);
            let changed = match doc.tiles.create_surface(
                target,
                texture_size,
                layer_mask_initial_pixels(mode),
            ) {
                Ok(changed) => changed,
                Err(err) => {
                    for surface in created_surfaces {
                        let _ = doc.tiles.delete_surface(surface);
                    }
                    *doc = previous_document;
                    *editor_document = previous_editor_document;
                    return Err(anyhow!("Document layer mask create failed: {err:#}"));
                }
            };
            created_surfaces.push(target);
            // The CPU store keeps uniform masks sparse; the renderer independently
            // initializes its surface to the matching uniform value.
            effects.push(create_surface_command(
                target,
                match mode {
                    LayerMaskInitMode::RevealAll => LayerInitialPixels::SolidRgba8([255; 4]),
                    LayerMaskInitMode::HideAll => LayerInitialPixels::Transparent,
                },
            ));
            if !changed.tiles.is_empty() {
                effects.push(upload_surface_tiles_command(
                    target,
                    changed.surface_revision,
                    changed.tiles,
                ));
            }
        }
        return Ok(with_structure_impact(with_optional_history(
            effects,
            history_transaction,
        )));
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn add_layer_mask_from_selection(
    state: &mut AppState,
    layer_id: LayerId,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        if !doc.active_selection.is_active() || !doc.layer_tree.can_add_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }

        let material_masks = doc
            .materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| {
                let r8 = if doc
                    .active_selection
                    .material_mask(material_index.into())
                    .is_some_and(|mask| mask.mask_id.is_some())
                {
                    doc.selection_masks
                        .mask_bytes_or_zeros(material_index.into(), material.texture_size)?
                } else {
                    vec![0; crate::core::selection::selection_mask_len(material.texture_size)?]
                };
                Ok((
                    material_index,
                    material.texture_size,
                    selection_mask_rgba8(&r8),
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        let previous_document = doc.clone();
        let previous_editor_document = *editor_document;
        if !doc.layer_tree.add_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        editor_document.active_layer_id = layer_id;
        editor_document.active_part = crate::core::document::ActiveLayerPart::LayerMask;

        let mut created_surfaces = Vec::new();
        let mut effects = Vec::new();
        for (material_index, texture_size, rgba8) in material_masks {
            let target = PaintSurfaceId::layer_mask(material_index.into(), layer_id);
            let changed =
                match doc
                    .tiles
                    .create_surface(target, texture_size, InitialPixels::Rgba8(rgba8))
                {
                    Ok(changed) => changed,
                    Err(err) => {
                        for surface in created_surfaces {
                            let _ = doc.tiles.delete_surface(surface);
                        }
                        *doc = previous_document;
                        *editor_document = previous_editor_document;
                        return Err(anyhow!("Document layer mask create failed: {err:#}"));
                    }
                };
            created_surfaces.push(target);
            effects.push(create_surface_command(
                target,
                LayerInitialPixels::Transparent,
            ));
            if !changed.tiles.is_empty() {
                effects.push(upload_surface_tiles_command(
                    target,
                    changed.surface_revision,
                    changed.tiles,
                ));
            }
        }

        return Ok(with_structure_impact(with_optional_history(
            effects,
            history_transaction,
        )));
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn delete_layer_mask(state: &mut AppState, layer_id: LayerId) -> Result<ReducerOutput> {
    if state
        .document()
        .is_some_and(|document| document.layer_tree.is_effectively_locked(layer_id))
    {
        return Ok(ReducerOutput::default());
    }
    let history_transaction = state
        .document()
        .map(|document| {
            capture_layer_images(document, document.layer_mask_surfaces(layer_id)).map(
                |before_images| PendingCpuHistoryTransaction::LayerTree {
                    before: layer_tree_snapshot(state, document),
                    before_images,
                    synthesize_transparent_after_images: false,
                    retained_embedded_images: Vec::new(),
                },
            )
        })
        .transpose()
        .map_err(|err| anyhow!("Layer mask delete snapshot failed: {err:#}"))?;
    if let Some((doc, editor_document)) = state.document_and_editor_document_mut() {
        if !doc.layer_tree.has_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        let targets = doc.layer_mask_surfaces(layer_id);
        let previous_document = doc.clone();
        let previous_editor_document = *editor_document;
        if !doc.layer_tree.delete_layer_mask(layer_id) {
            return Ok(with_structure_impact(with_optional_history(
                Vec::new(),
                history_transaction,
            )));
        }
        if editor_document.active_layer_id == layer_id
            && editor_document.active_part == crate::core::document::ActiveLayerPart::LayerMask
        {
            editor_document.active_part = crate::core::document::ActiveLayerPart::Content;
        }
        let mut effects = Vec::new();
        for target in targets {
            if let Err(err) = doc.tiles.delete_surface(target) {
                *doc = previous_document;
                *editor_document = previous_editor_document;
                return Err(anyhow!("Document layer mask delete failed: {err:#}"));
            }
            effects.push(delete_surface_command(target));
        }
        return Ok(with_structure_impact(with_optional_history(
            effects,
            history_transaction,
        )));
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn move_layer_mask(
    state: &mut AppState,
    source_layer_id: LayerId,
    target_layer_id: LayerId,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if !document
        .layer_tree
        .can_move_layer_mask(source_layer_id, target_layer_id)
    {
        return Ok(ReducerOutput::default());
    }

    let target_had_mask = document.layer_tree.has_layer_mask(target_layer_id);
    let before = layer_tree_snapshot(state, document);
    let mut before_surfaces = document.layer_mask_surfaces(source_layer_id);
    if target_had_mask {
        before_surfaces.extend(document.layer_mask_surfaces(target_layer_id));
    }
    let before_images = capture_layer_images(document, before_surfaces)
        .map_err(|err| anyhow!("Layer mask move snapshot failed: {err:#}"))?;

    let Some((document, editor_document)) = state.document_and_editor_document_mut() else {
        return Ok(ReducerOutput::default());
    };
    let previous_document = document.clone();
    let previous_editor_document = *editor_document;
    if !document
        .layer_tree
        .move_layer_mask(source_layer_id, target_layer_id)
    {
        return Ok(ReducerOutput::default());
    }

    let mut effects = Vec::new();
    for material_index in 0..document.materials.len() {
        let source = PaintSurfaceId::layer_mask(material_index.into(), source_layer_id);
        let target = PaintSurfaceId::layer_mask(material_index.into(), target_layer_id);
        if target_had_mask {
            if let Err(err) = document.tiles.delete_surface(target) {
                *document = previous_document.clone();
                *editor_document = previous_editor_document;
                return Err(anyhow!("Document target layer mask delete failed: {err:#}"));
            }
            effects.push(delete_surface_command(target));
        }
        if let Err(err) = document.tiles.duplicate_surface(source, target) {
            *document = previous_document;
            *editor_document = previous_editor_document;
            return Err(anyhow!(
                "Document layer mask move duplicate failed: {err:#}"
            ));
        }
        effects.push(GpuDocumentCommand::DuplicateSurface {
            from: source,
            to: target,
        });
        if let Err(err) = document.tiles.delete_surface(source) {
            *document = previous_document;
            *editor_document = previous_editor_document;
            return Err(anyhow!("Document source layer mask delete failed: {err:#}"));
        }
        effects.push(delete_surface_command(source));
    }

    editor_document.active_layer_id = target_layer_id;
    editor_document.active_part = crate::core::document::ActiveLayerPart::LayerMask;
    let after = LayerTreeSnapshot {
        tree: document.layer_tree.clone(),
        active_layer_id: editor_document.active_layer_id,
        active_part: editor_document.active_part,
    };
    let after_images =
        match capture_layer_images(document, document.layer_mask_surfaces(target_layer_id)) {
            Ok(images) => images,
            Err(err) => {
                *document = previous_document;
                *editor_document = previous_editor_document;
                return Err(anyhow!("Moved layer mask snapshot failed: {err:#}"));
            }
        };
    let history = HistoryAtom::LayerTreeEdit {
        edit: LayerTreeEdit {
            before,
            after,
            before_images,
            after_images,
            retained_embedded_images: Vec::new(),
        },
    };
    let mut output =
        ReducerOutput::default().with_finalized_history_atom("Move Layer Mask", history);
    for effect in effects {
        output.push_document_command(effect);
    }
    Ok(with_structure_impact(output))
}

pub fn set_layer_mask_enabled(
    state: &mut AppState,
    layer_id: LayerId,
    enabled: bool,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some(doc) = state.document_mut() {
        if doc.layer_tree.is_effectively_locked(layer_id) {
            return Ok(ReducerOutput::default());
        }
        doc.layer_tree.set_layer_mask_enabled(layer_id, enabled);
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn select_adjustment_layer(
    state: &mut AppState,
    layer_id: crate::core::surface::LayerId,
) -> Result<ReducerOutput> {
    let output = commit_embedded_transform_before_selection(state)?;
    state.document.select_adjustment_layer_if_present(layer_id);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

pub fn select_layer_mask(
    state: &mut AppState,
    layer_id: crate::core::surface::LayerId,
) -> Result<ReducerOutput> {
    let output = commit_embedded_transform_before_selection(state)?;
    state.document.select_layer_mask_if_present(layer_id);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

fn layer_mask_initial_pixels(mode: LayerMaskInitMode) -> InitialPixels {
    match mode {
        LayerMaskInitMode::RevealAll => InitialPixels::SparseDefaultRgba8([255; 4]),
        LayerMaskInitMode::HideAll => InitialPixels::Transparent,
    }
}

fn selection_mask_rgba8(r8: &[u8]) -> Vec<u8> {
    let mut rgba8 = Vec::with_capacity(r8.len().saturating_mul(4));
    for &value in r8 {
        rgba8.extend_from_slice(&[value, value, value, value]);
    }
    rgba8
}

pub fn select_layer(
    state: &mut AppState,
    layer_id: crate::core::surface::LayerId,
) -> Result<ReducerOutput> {
    let output = commit_embedded_transform_before_selection(state)?;
    state.document.select_raster_layer_if_paintable(layer_id);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

pub fn select_solid_fill_layer(
    state: &mut AppState,
    layer_id: crate::core::surface::LayerId,
) -> Result<ReducerOutput> {
    let output = commit_embedded_transform_before_selection(state)?;
    state.document.select_solid_fill_layer_if_present(layer_id);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

pub fn select_layer_for_structure(
    state: &mut AppState,
    layer_id: crate::core::surface::LayerId,
) -> Result<ReducerOutput> {
    let output = commit_embedded_transform_before_selection(state)?;
    state.document.select_structure_layer_if_present(layer_id);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

fn commit_embedded_transform_before_selection(state: &mut AppState) -> Result<ReducerOutput> {
    if state.tool.embedded_image_transform_session().is_some() {
        super::transform_controller::commit_active_embedded_image_transform(state)
    } else {
        Ok(ReducerOutput::default())
    }
}

pub fn rename_layer(
    state: &mut AppState,
    layer_id: LayerId,
    name: String,
) -> Result<ReducerOutput> {
    let pending_edit = state.document().and_then(|document| {
        let before = document.layer_tree.get(layer_id)?.props.name.clone();
        Some(PendingLayerPropertyEdit::Rename { layer_id, before })
    });
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.rename_layer(layer_id, name);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layer_visible(
    state: &mut AppState,
    layer_id: LayerId,
    visible: bool,
) -> Result<ReducerOutput> {
    let pending_edit = state.document().and_then(|document| {
        let before = document.layer_tree.get(layer_id)?.props.visible;
        Some(PendingLayerPropertyEdit::Visibility { layer_id, before })
    });
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.set_visible(layer_id, visible);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layer_locked(
    state: &mut AppState,
    layer_id: LayerId,
    locked: bool,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if !document.layer_tree.can_set_layer_locked(layer_id) {
        return Ok(ReducerOutput::default());
    }
    let before = document.layer_tree.is_locked(layer_id);
    if before == locked {
        return Ok(ReducerOutput::default());
    }
    let mut output = if locked && state.tool.embedded_image_transform_session().is_some() {
        super::transform_controller::commit_active_embedded_image_transform(state)?
    } else {
        ReducerOutput::default()
    };
    let pending_edit = Some(PendingLayerPropertyEdit::Lock { layer_id, before });
    if let Some(document) = state.document_mut() {
        document.layer_tree.set_locked(layer_id, locked);
    }
    output.append_outcome(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ));
    Ok(output)
}

pub fn set_layers_locked(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    locked: bool,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let mut seen = HashSet::new();
    let layer_ids = layer_ids
        .into_iter()
        .filter(|layer_id| seen.insert(*layer_id))
        .collect::<Vec<_>>();
    if layer_ids.is_empty()
        || layer_ids
            .iter()
            .any(|layer_id| !document.layer_tree.can_set_layer_locked(*layer_id))
    {
        return Ok(ReducerOutput::default());
    }
    let layers = layer_ids
        .iter()
        .map(|layer_id| (*layer_id, document.layer_tree.is_locked(*layer_id)))
        .collect::<Vec<_>>();
    if layers.iter().all(|(_, before)| *before == locked) {
        return Ok(ReducerOutput::default());
    }
    let mut output = if locked && state.tool.embedded_image_transform_session().is_some() {
        super::transform_controller::commit_active_embedded_image_transform(state)?
    } else {
        ReducerOutput::default()
    };
    let pending_edit = Some(PendingLayerPropertyEdit::LockMany { layers, locked });
    if let Some(document) = state.document_mut() {
        for layer_id in layer_ids {
            document.layer_tree.set_locked(layer_id, locked);
        }
    }
    output.append_outcome(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ));
    Ok(output)
}

pub fn set_layer_material_mask(
    state: &mut AppState,
    layer_id: LayerId,
    material_mask: LayerMaterialMask,
) -> Result<ReducerOutput> {
    let mut output = if state.tool.embedded_image_transform_session().is_some() {
        super::transform_controller::commit_active_embedded_image_transform(state)?
    } else {
        ReducerOutput::default()
    };
    let Some(document) = state.document() else {
        return Ok(output);
    };
    if layer_id == document.layer_tree.root() || !document.layer_tree.contains(layer_id) {
        return Ok(output);
    }

    let existing_material_ids = document
        .materials
        .iter()
        .map(|material| material.id)
        .collect::<BTreeSet<_>>();
    let material_mask = match material_mask {
        LayerMaterialMask::Unspecified => LayerMaterialMask::Unspecified,
        LayerMaterialMask::Specified(material_ids) => LayerMaterialMask::Specified(
            material_ids
                .intersection(&existing_material_ids)
                .copied()
                .collect(),
        ),
    };
    let requires_single_material = document
        .layer_tree
        .get(layer_id)
        .is_some_and(|node| node.content.requires_single_material());
    if requires_single_material
        && !matches!(&material_mask, LayerMaterialMask::Specified(material_ids) if material_ids.len() == 1)
    {
        return Ok(output);
    }
    let Some(before) = document.layer_tree.layer_material_mask(layer_id).cloned() else {
        return Ok(output);
    };
    if before == material_mask {
        return Ok(output);
    }

    let pending_edit = Some(PendingLayerPropertyEdit::MaterialMask { layer_id, before });
    if let Some(document) = state.document_mut() {
        document
            .layer_tree
            .set_layer_material_mask(layer_id, material_mask);
    }
    output.append_outcome(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ));
    Ok(output)
}

pub fn set_embedded_image_transform(
    state: &mut AppState,
    layer_id: LayerId,
    transform: crate::core::embedded_image::EmbeddedImageTransform,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if document.layer_tree.is_effectively_locked(layer_id) || !transform.is_valid() {
        return Ok(ReducerOutput::default());
    }
    let Some((_, before)) = document.layer_tree.embedded_image(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    if before == transform {
        return Ok(ReducerOutput::default());
    }
    let pending_edit = Some(PendingLayerPropertyEdit::EmbeddedImageTransform { layer_id, before });
    if let Some(document) = state.document_mut() {
        if !document
            .layer_tree
            .set_embedded_image_transform(layer_id, transform)
        {
            return Ok(ReducerOutput::default());
        }
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_solid_fill_color(
    state: &mut AppState,
    layer_id: LayerId,
    color: [f32; 3],
    edit_session: u64,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if document.layer_tree.is_effectively_locked(layer_id) {
        return Ok(ReducerOutput::default());
    }
    let Some(before) = document.layer_tree.solid_fill_color(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    let color = color.map(|channel| channel.clamp(0.0, 1.0));
    if before == color {
        return Ok(ReducerOutput::default());
    }
    let pending_edit = Some(PendingLayerPropertyEdit::FillColor {
        layer_id,
        edit_session,
        before,
    });
    if let Some(document) = state.document_mut() {
        document.layer_tree.set_solid_fill_color(layer_id, color);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_adjustment(
    state: &mut AppState,
    layer_id: LayerId,
    adjustment: Adjustment,
    edit_session: u64,
) -> Result<ReducerOutput> {
    let Some(adjustment) = adjustment.try_normalized() else {
        return Ok(ReducerOutput::default());
    };
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if document.layer_tree.is_effectively_locked(layer_id) {
        return Ok(ReducerOutput::default());
    }
    let Some(before) = document.layer_tree.adjustment(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    if before == adjustment {
        return Ok(ReducerOutput::default());
    }
    let pending_edit = Some(PendingLayerPropertyEdit::Adjustment {
        layer_id,
        edit_session,
        before,
    });
    if let Some(document) = state.document_mut() {
        document.layer_tree.set_adjustment(layer_id, adjustment);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layer_opacity(
    state: &mut AppState,
    layer_id: LayerId,
    opacity: f32,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if !document.layer_tree.can_set_layer_opacity(layer_id) {
        return Ok(ReducerOutput::default());
    }

    let before = document
        .layer_tree
        .get(layer_id)
        .map(|node| node.props.opacity)
        .unwrap_or_default();
    let pending_edit = Some(PendingLayerPropertyEdit::Opacity { layer_id, before });
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.set_opacity(layer_id, opacity);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layers_opacity(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    opacity: f32,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if layer_ids.is_empty()
        || layer_ids
            .iter()
            .any(|layer_id| !document.layer_tree.can_set_layer_opacity(*layer_id))
    {
        return Ok(ReducerOutput::default());
    }

    let before_layers = layer_ids
        .iter()
        .filter_map(|layer_id| {
            let before = document.layer_tree.get(*layer_id)?.props.opacity;
            Some((*layer_id, before))
        })
        .collect::<Vec<_>>();
    let pending_edit = Some(PendingLayerPropertyEdit::OpacityMany {
        layers: before_layers,
    });
    if let Some(doc) = state.document_mut() {
        for layer_id in layer_ids {
            doc.layer_tree.set_opacity(layer_id, opacity);
        }
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layer_blend_mode(
    state: &mut AppState,
    layer_id: LayerId,
    blend_mode: LayerBlendMode,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if !document.layer_tree.can_set_layer_blend_mode(layer_id) {
        return Ok(ReducerOutput::default());
    }

    let Some(_node) = document.layer_tree.get(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    let before = layer_composite_settings(&document.layer_tree, layer_id);
    let pending_edit = Some(PendingLayerPropertyEdit::CompositeMode { layer_id, before });
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.set_blend_mode(layer_id, blend_mode);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layers_blend_mode(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    blend_mode: LayerBlendMode,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if layer_ids.is_empty()
        || layer_ids
            .iter()
            .any(|layer_id| !document.layer_tree.can_set_layer_blend_mode(*layer_id))
    {
        return Ok(ReducerOutput::default());
    }

    let before_layers = layer_ids
        .iter()
        .filter_map(|layer_id| {
            let _node = document.layer_tree.get(*layer_id)?;
            Some((
                *layer_id,
                layer_composite_settings(&document.layer_tree, *layer_id),
            ))
        })
        .collect::<Vec<_>>();
    let pending_edit = Some(PendingLayerPropertyEdit::CompositeModeMany {
        layers: before_layers,
    });
    if let Some(doc) = state.document_mut() {
        for layer_id in layer_ids {
            doc.layer_tree.set_blend_mode(layer_id, blend_mode);
        }
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layer_group_composite_mode(
    state: &mut AppState,
    layer_id: LayerId,
    mode: GroupCompositeMode,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if !document.layer_tree.can_set_group_composite_mode(layer_id) {
        return Ok(ReducerOutput::default());
    }

    let Some(_node) = document.layer_tree.get(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    let before = layer_composite_settings(&document.layer_tree, layer_id);
    let pending_edit = Some(PendingLayerPropertyEdit::CompositeMode { layer_id, before });
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.set_group_composite_mode(layer_id, mode);
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

pub fn set_layers_group_composite_mode(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    mode: GroupCompositeMode,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    if layer_ids.is_empty()
        || layer_ids
            .iter()
            .any(|layer_id| !document.layer_tree.can_set_group_composite_mode(*layer_id))
    {
        return Ok(ReducerOutput::default());
    }

    let before_layers = layer_ids
        .iter()
        .filter_map(|layer_id| {
            let _node = document.layer_tree.get(*layer_id)?;
            Some((
                *layer_id,
                layer_composite_settings(&document.layer_tree, *layer_id),
            ))
        })
        .collect::<Vec<_>>();
    let pending_edit = Some(PendingLayerPropertyEdit::CompositeModeMany {
        layers: before_layers,
    });
    if let Some(doc) = state.document_mut() {
        for layer_id in layer_ids {
            doc.layer_tree.set_group_composite_mode(layer_id, mode);
        }
    }
    Ok(with_optional_layer_property_history_and_impact(
        Vec::new(),
        pending_edit,
        state.document(),
    ))
}

fn layer_composite_settings(
    tree: &crate::core::surface::LayerTree,
    layer_id: LayerId,
) -> LayerCompositeSettings {
    let node = tree.get(layer_id).expect("editable layer must exist");
    LayerCompositeSettings {
        blend_mode: node.props.blend_mode,
        group_mode: tree
            .group_composite_mode(layer_id)
            .unwrap_or(GroupCompositeMode::Isolated),
    }
}

pub fn move_layer(
    state: &mut AppState,
    layer_id: LayerId,
    new_parent: LayerId,
    new_index: usize,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some(doc) = state.document_mut() {
        doc.layer_tree.move_layer(layer_id, new_parent, new_index);
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

pub fn move_layers(
    state: &mut AppState,
    layer_ids: Vec<LayerId>,
    new_parent: LayerId,
    new_index: usize,
) -> Result<ReducerOutput> {
    let history_transaction = layer_tree_transaction(state, Vec::new(), false);
    if let Some(doc) = state.document_mut() {
        doc.layer_tree
            .move_layers(&layer_ids, new_parent, new_index);
    }
    Ok(with_structure_impact(with_optional_history(
        Vec::new(),
        history_transaction,
    )))
}

fn effective_layer_roots(
    tree: &crate::core::surface::LayerTree,
    layer_ids: &[LayerId],
) -> Vec<LayerId> {
    let selected = layer_ids.iter().copied().collect::<HashSet<_>>();
    tree.rows()
        .into_iter()
        .map(|row| row.layer_id)
        .filter(|layer_id| selected.contains(layer_id))
        .filter(|layer_id| {
            let mut current = tree.get(*layer_id).and_then(|node| node.parent);
            while let Some(parent) = current {
                if selected.contains(&parent) {
                    return false;
                }
                current = tree.get(parent).and_then(|node| node.parent);
            }
            true
        })
        .collect()
}

fn layer_tree_transaction(
    state: &AppState,
    before_images: Vec<LayerImageSnapshot>,
    synthesize_transparent_after_images: bool,
) -> Option<PendingCpuHistoryTransaction> {
    let document = state.document()?;
    Some(PendingCpuHistoryTransaction::LayerTree {
        before: layer_tree_snapshot(state, document),
        before_images,
        synthesize_transparent_after_images,
        retained_embedded_images: document.embedded_images_referenced_by(&document.layer_tree),
    })
}

fn layer_tree_snapshot(state: &AppState, document: &Document) -> LayerTreeSnapshot {
    let active = state.resolved_editor_document(document);
    LayerTreeSnapshot {
        tree: document.layer_tree.clone(),
        active_layer_id: active.active_layer_id,
        active_part: active.active_part,
    }
}

fn capture_layer_images(
    document: &Document,
    surfaces: Vec<PaintSurfaceId>,
) -> Result<Vec<LayerImageSnapshot>> {
    surfaces
        .into_iter()
        .map(|surface| {
            let image = document.tiles.read_surface_full(surface)?;
            Ok(LayerImageSnapshot {
                surface,
                texture_size: image.texture_size,
                rgba8: image.rgba8,
            })
        })
        .collect()
}

fn with_structure_impact(output: ReducerOutput) -> ReducerOutput {
    with_composite_impact(output, None, CompositeImpact::LayerTreeStructure)
}

fn with_layer_property_impact(
    output: ReducerOutput,
    document: Option<&Document>,
    pending_edit: PendingLayerPropertyEdit,
) -> ReducerOutput {
    let Some(document) = document else {
        return output;
    };
    let Some(edit) =
        crate::application::history_finalizer::finalize_layer_property_edit(document, pending_edit)
    else {
        return output;
    };
    let impact = CompositeImpact::for_layer_property_edit(Some(document), &edit);
    with_composite_impact(output, Some(document), impact)
}

fn with_composite_impact(
    output: ReducerOutput,
    document: Option<&Document>,
    impact: CompositeImpact,
) -> ReducerOutput {
    output.with_composite_sync(impact.into_composite_sync(document))
}

fn with_optional_layer_property_history_and_impact(
    document_commands: Vec<GpuDocumentCommand>,
    pending_edit: Option<PendingLayerPropertyEdit>,
    document: Option<&Document>,
) -> ReducerOutput {
    let history_transaction =
        pending_edit
            .as_ref()
            .map(|pending_edit| PendingCpuHistoryTransaction::LayerProperty {
                before: pending_edit.clone(),
            });
    let output = with_optional_history(document_commands, history_transaction);
    match pending_edit {
        Some(pending_edit) => with_layer_property_impact(output, document, pending_edit),
        None => output,
    }
}

fn with_optional_history(
    document_commands: Vec<GpuDocumentCommand>,
    history_transaction: Option<PendingCpuHistoryTransaction>,
) -> ReducerOutput {
    let mut output =
        ReducerOutput::default().with_optional_cpu_history_transaction(history_transaction);
    for command in document_commands {
        output.push_document_command(command);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        adjustment::{Adjustment, AdjustmentKind, BrightnessContrastAdjustment},
        document::{Document, MaterialSpec, MeshData},
        material::MaterialId,
        surface::{CompositeNode, LayerTree},
    };

    fn state_with_default_layer() -> (AppState, LayerId) {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![MaterialSpec::new("A", [8, 8])],
        ));
        let layer_id = state
            .document
            .document
            .as_ref()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        (state, layer_id)
    }

    fn has_pending_transaction(output: &ReducerOutput) -> bool {
        output.pending_transaction.is_some()
    }

    #[test]
    fn layer_selection_commands_do_not_change_project() {
        let (mut state, raster) = state_with_default_layer();
        let (solid, adjustment, group) = {
            let tree = &mut state.document_mut().unwrap().layer_tree;
            assert!(tree.add_layer_mask(raster));
            let solid = tree
                .add_solid_fill_layer_above(raster, "Solid", [0.5; 3])
                .unwrap();
            let adjustment = tree
                .add_adjustment_layer_above(
                    solid,
                    "Adjustment",
                    AdjustmentKind::Invert.default_adjustment(),
                )
                .unwrap();
            let group = tree.add_group_above(adjustment, "Group").unwrap();
            (solid, adjustment, group)
        };

        for output in [
            select_layer(&mut state, raster).unwrap(),
            select_solid_fill_layer(&mut state, solid).unwrap(),
            select_adjustment_layer(&mut state, adjustment).unwrap(),
            select_layer_mask(&mut state, raster).unwrap(),
            select_layer_for_structure(&mut state, group).unwrap(),
        ] {
            assert!(!output.project_changed());
        }
    }

    #[test]
    fn add_layer_mask_from_selection_copies_coverage_for_all_materials() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [2, 1]),
                MaterialSpec::new("B", [2, 1]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        {
            let document = state.document_mut().unwrap();
            document.active_selection.enable_material_mask(0.into());
            document
                .selection_masks
                .write_mask(0.into(), [2, 1], vec![0, 128])
                .unwrap();
        }

        let output = add_layer_mask_from_selection(&mut state, layer_id).unwrap();

        let document = state.document().unwrap();
        assert!(document.layer_tree.has_layer_mask(layer_id));
        assert_eq!(state.active_layer_target(), ActiveLayerTarget::LayerMask);
        assert!(document.active_selection.is_active());
        assert_eq!(
            document
                .tiles
                .read_surface_full(PaintSurfaceId::layer_mask(0.into(), layer_id))
                .unwrap()
                .rgba8,
            vec![0, 0, 0, 0, 128, 128, 128, 128]
        );
        assert_eq!(
            document
                .tiles
                .read_surface_full(PaintSurfaceId::layer_mask(1.into(), layer_id))
                .unwrap()
                .rgba8,
            vec![0; 8]
        );
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn set_layer_material_mask_normalizes_ids_and_requests_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let document = state.document().unwrap();
        let layer_id = document.layer_tree.default_raster_layer().unwrap();
        let material_a = document.materials[0].id;

        let output = set_layer_material_mask(
            &mut state,
            layer_id,
            LayerMaterialMask::Specified([material_a, MaterialId(999)].into_iter().collect()),
        )
        .unwrap();

        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .layer_material_mask(layer_id),
            Some(&LayerMaterialMask::Specified(
                [material_a].into_iter().collect()
            ))
        );
        assert!(has_pending_transaction(&output));
        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(pending.take_composite_sync(), CompositeSync::MaterialTrees);
    }

    #[test]
    fn specified_material_mask_change_scopes_tree_sync_to_affected_materials() {
        let mut state = AppState::default();
        let document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
                MaterialSpec::new("C", [8, 8]),
            ],
        );
        let layer_id = document.layer_tree.default_raster_layer().unwrap();
        let material_a = document.materials[0].id;
        let material_b = document.materials[1].id;
        state.document.document = Some(document);
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([material_a].into_iter().collect()),
            );

        let output = set_layer_material_mask(
            &mut state,
            layer_id,
            LayerMaterialMask::Specified([material_b].into_iter().collect()),
        )
        .unwrap();

        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(
            pending.take_composite_sync(),
            CompositeSync::material_trees_scoped([0.into(), 1.into()])
        );
    }

    #[test]
    fn set_layer_material_mask_rejects_root_and_noop_changes_without_history() {
        let (mut state, layer_id) = state_with_default_layer();
        let root = state.document().unwrap().layer_tree.root();

        let noop =
            set_layer_material_mask(&mut state, layer_id, LayerMaterialMask::Unspecified).unwrap();
        let root_change = set_layer_material_mask(
            &mut state,
            root,
            LayerMaterialMask::Specified(BTreeSet::new()),
        )
        .unwrap();

        assert!(!has_pending_transaction(&noop));
        assert!(!has_pending_transaction(&root_change));
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .layer_material_mask(root),
            Some(&LayerMaterialMask::Unspecified)
        );
    }

    #[test]
    fn lock_changes_local_state_without_renderer_work() {
        let (mut state, layer_id) = state_with_default_layer();

        let output = set_layer_locked(&mut state, layer_id, true).unwrap();

        assert!(state.document().unwrap().layer_tree.is_locked(layer_id));
        assert!(has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
    }

    #[test]
    fn parent_lock_rejects_content_property_changes() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .move_layer(layer_id, group, 0)
        );
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .set_locked(group, true)
        );

        let opacity = set_layer_opacity(&mut state, layer_id, 0.25).unwrap();
        let blend = set_layer_blend_mode(&mut state, layer_id, LayerBlendMode::Multiply).unwrap();
        let mask = add_layer_mask(&mut state, layer_id, LayerMaskInitMode::RevealAll).unwrap();

        let node = state.document().unwrap().layer_tree.get(layer_id).unwrap();
        assert_eq!(node.props.opacity, 1.0);
        assert_eq!(node.props.blend_mode, LayerBlendMode::Normal);
        assert!(
            !state
                .document()
                .unwrap()
                .layer_tree
                .has_layer_mask(layer_id)
        );
        assert!(!has_pending_transaction(&opacity));
        assert!(!has_pending_transaction(&blend));
        assert!(!mask.has_renderer_work());
    }

    #[test]
    fn inherited_lock_still_allows_metadata_changes() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .move_layer(layer_id, group, 0)
        );
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .set_locked(group, true)
        );

        rename_layer(&mut state, layer_id, "Renamed".to_owned()).unwrap();
        set_layer_visible(&mut state, layer_id, false).unwrap();
        set_layer_material_mask(
            &mut state,
            layer_id,
            LayerMaterialMask::Specified(BTreeSet::new()),
        )
        .unwrap();

        let document = state.document().unwrap();
        let node = document.layer_tree.get(layer_id).unwrap();
        assert_eq!(node.props.name, "Renamed");
        assert!(!node.props.visible);
        assert_eq!(
            document.layer_tree.layer_material_mask(layer_id),
            Some(&LayerMaterialMask::Specified(BTreeSet::new()))
        );
    }

    #[test]
    fn hidden_layer_accepts_opacity_with_history() {
        let (mut state, layer_id) = state_with_default_layer();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);

        let output = set_layer_opacity(&mut state, layer_id, 0.25).unwrap();

        let opacity = state
            .document()
            .unwrap()
            .layer_tree
            .get(layer_id)
            .unwrap()
            .props
            .opacity;
        assert_eq!(opacity, 0.25);
        assert!(has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
    }

    #[test]
    fn uv_mirror_rejects_opacity_without_history() {
        let (mut state, raster) = state_with_default_layer();
        let layer_id = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "UV Mirror",
                AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();

        let output = set_layer_opacity(&mut state, layer_id, 0.25).unwrap();

        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .get(layer_id)
                .unwrap()
                .props
                .opacity,
            1.0
        );
        assert!(!has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
    }

    #[test]
    fn hidden_layer_accepts_blend_mode_with_history() {
        let (mut state, layer_id) = state_with_default_layer();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);

        let output = set_layer_blend_mode(&mut state, layer_id, LayerBlendMode::Multiply).unwrap();

        let blend_mode = state
            .document()
            .unwrap()
            .layer_tree
            .get(layer_id)
            .unwrap()
            .props
            .blend_mode;
        assert_eq!(blend_mode, LayerBlendMode::Multiply);
        assert!(has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
    }

    #[test]
    fn pass_through_group_accepts_opacity_and_switches_back_on_blend_selection() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();

        let output =
            set_layer_group_composite_mode(&mut state, group, GroupCompositeMode::PassThrough)
                .unwrap();
        assert!(has_pending_transaction(&output));
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::PassThrough)
        );

        let opacity_output = set_layer_opacity(&mut state, group, 0.5).unwrap();
        assert!(has_pending_transaction(&opacity_output));
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .get(group)
                .unwrap()
                .props
                .opacity,
            0.5
        );

        let blend_output =
            set_layer_blend_mode(&mut state, group, LayerBlendMode::Multiply).unwrap();
        assert!(has_pending_transaction(&blend_output));
        let props = &state
            .document()
            .unwrap()
            .layer_tree
            .get(group)
            .unwrap()
            .props;
        assert_eq!(props.blend_mode, LayerBlendMode::Multiply);
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::Isolated)
        );
    }

    #[test]
    fn pass_through_group_accepts_existing_mask() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_layer_mask(group);

        let output =
            set_layer_group_composite_mode(&mut state, group, GroupCompositeMode::PassThrough)
                .unwrap();

        assert!(has_pending_transaction(&output));
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::PassThrough)
        );
    }

    #[test]
    fn hidden_group_accepts_group_composite_mode() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(group, false);

        let output =
            set_layer_group_composite_mode(&mut state, group, GroupCompositeMode::PassThrough)
                .unwrap();

        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .group_composite_mode(group),
            Some(GroupCompositeMode::PassThrough)
        );
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn set_layers_opacity_applies_to_all_selected_layers() {
        let (mut state, layer_id) = state_with_default_layer();
        let second = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_raster_layer_above(layer_id, "Second")
            .unwrap();

        let output = set_layers_opacity(&mut state, vec![layer_id, second], 0.25).unwrap();
        let tree = &state.document().unwrap().layer_tree;

        assert_eq!(tree.get(layer_id).unwrap().props.opacity, 0.25);
        assert_eq!(tree.get(second).unwrap().props.opacity, 0.25);
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn set_layers_blend_mode_accepts_hidden_members() {
        let (mut state, layer_id) = state_with_default_layer();
        let second = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_raster_layer_above(layer_id, "Second")
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(second, false);

        let output =
            set_layers_blend_mode(&mut state, vec![layer_id, second], LayerBlendMode::Multiply)
                .unwrap();
        let tree = &state.document().unwrap().layer_tree;

        assert_eq!(
            tree.get(layer_id).unwrap().props.blend_mode,
            LayerBlendMode::Multiply
        );
        assert_eq!(
            tree.get(second).unwrap().props.blend_mode,
            LayerBlendMode::Multiply
        );
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn hidden_layer_still_allows_visibility_and_rename() {
        let (mut state, layer_id) = state_with_default_layer();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);

        let rename_output = rename_layer(&mut state, layer_id, "Renamed".to_owned()).unwrap();
        let visibility_output = set_layer_visible(&mut state, layer_id, true).unwrap();
        let node = state.document().unwrap().layer_tree.get(layer_id).unwrap();

        assert_eq!(node.props.name, "Renamed");
        assert!(node.props.visible);
        assert!(has_pending_transaction(&rename_output));
        assert!(has_pending_transaction(&visibility_output));
    }

    #[test]
    fn child_under_hidden_group_accepts_opacity() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        let tree = &mut state.document_mut().unwrap().layer_tree;
        assert!(tree.move_layer(layer_id, group, 0));
        tree.set_visible(group, false);

        let output = set_layer_opacity(&mut state, layer_id, 0.25).unwrap();

        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .get(layer_id)
                .unwrap()
                .props
                .opacity,
            0.25
        );
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn next_numbered_layer_name_fills_lowest_unused_gap() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        tree.add_raster_layer_above(base, "Layer 1").unwrap();
        tree.add_raster_layer_above(base, "Layer 3").unwrap();

        assert_eq!(next_numbered_layer_name(&tree, "Layer"), "Layer 2");
    }

    #[test]
    fn next_numbered_group_name_uses_next_available_number() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(base, "Group 1").unwrap();
        tree.add_group_above(group, "Group 2").unwrap();

        assert_eq!(next_numbered_layer_name(&tree, "Group"), "Group 3");
    }

    #[test]
    fn add_solid_fill_layer_creates_no_gpu_surface_and_selects_fill_target() {
        let (mut state, raster) = state_with_default_layer();

        let output = add_solid_fill_layer(&mut state, [0.25, 0.5, 0.75]).unwrap();
        let active = state.document.active_layer_id();

        assert_ne!(active, raster);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::SolidFill
        );
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .solid_fill_color(active),
            Some([0.25, 0.5, 0.75])
        );
        assert!(!state.document().unwrap().layer_tree.is_paintable(active));
        assert!(
            output
                .into_renderer_plan()
                .into_frame_plan(Vec::new(), None)
                .document_commands
                .is_empty()
        );
    }

    #[test]
    fn add_adjustment_layer_creates_no_gpu_surface_and_selects_adjustment_target() {
        let (mut state, raster) = state_with_default_layer();

        let output = add_adjustment_layer(&mut state, AdjustmentKind::BrightnessContrast).unwrap();
        let active = state.document.active_layer_id();

        assert_ne!(active, raster);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::Adjustment
        );
        assert_eq!(
            state.document().unwrap().layer_tree.adjustment(active),
            Some(AdjustmentKind::BrightnessContrast.default_adjustment())
        );
        assert!(!state.document().unwrap().layer_tree.is_paintable(active));
        assert!(!output.has_renderer_work());
        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(pending.take_composite_sync(), CompositeSync::MaterialTrees);
    }

    #[test]
    fn add_uv_mirror_layer_targets_focused_material_and_scopes_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        assert!(state.set_focused_material(1));
        let material_b = state.document().unwrap().materials[1].id;

        let output = add_adjustment_layer(&mut state, AdjustmentKind::UvMirror).unwrap();
        let active = state.document.active_layer_id();
        let document = state.document().unwrap();

        assert_eq!(
            document.layer_tree.adjustment(active),
            Some(AdjustmentKind::UvMirror.default_adjustment())
        );
        assert_eq!(
            document.layer_tree.layer_material_mask(active),
            Some(&LayerMaterialMask::Specified(
                [material_b].into_iter().collect()
            ))
        );
        assert!(!document.layer_tree.can_set_layer_blend_mode(active));
        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(
            pending.take_composite_sync(),
            CompositeSync::material_trees_scoped([1.into()])
        );
    }

    #[test]
    fn uv_mirror_material_mask_requires_exactly_one_existing_material() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let material_a = state.document().unwrap().materials[0].id;
        let material_b = state.document().unwrap().materials[1].id;
        add_adjustment_layer(&mut state, AdjustmentKind::UvMirror).unwrap();
        let layer_id = state.document.active_layer_id();

        let rejected = set_layer_material_mask(
            &mut state,
            layer_id,
            LayerMaterialMask::Specified([material_a, material_b].into_iter().collect()),
        )
        .unwrap();
        assert!(!has_pending_transaction(&rejected));
        assert_eq!(
            state
                .document()
                .unwrap()
                .layer_tree
                .layer_material_mask(layer_id),
            Some(&LayerMaterialMask::Specified(
                [material_a].into_iter().collect()
            ))
        );

        let accepted = set_layer_material_mask(
            &mut state,
            layer_id,
            LayerMaterialMask::Specified([material_b].into_iter().collect()),
        )
        .unwrap();
        assert!(has_pending_transaction(&accepted));
        let mut pending = accepted.pending_transaction.unwrap();
        assert_eq!(
            pending.take_composite_sync(),
            CompositeSync::material_trees_scoped([0.into(), 1.into()])
        );
    }

    #[test]
    fn adjustment_layer_mask_creates_only_the_mask_surface() {
        let (mut state, _) = state_with_default_layer();
        add_adjustment_layer(&mut state, AdjustmentKind::Levels).unwrap();
        let adjustment = state.document.active_layer_id();
        let raster_surface = PaintSurfaceId::raster(0.into(), adjustment);
        let mask_surface = PaintSurfaceId::layer_mask(0.into(), adjustment);

        let output = add_layer_mask(&mut state, adjustment, LayerMaskInitMode::RevealAll).unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert!(
            state
                .document()
                .unwrap()
                .tiles
                .contains_surface(mask_surface)
        );
        assert!(
            !state
                .document()
                .unwrap()
                .tiles
                .contains_surface(raster_surface)
        );
        let created_surfaces = plan
            .document_commands
            .iter()
            .filter_map(|command| match command {
                GpuDocumentCommand::CreateSurface { target, .. } => Some(*target),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(created_surfaces, vec![mask_surface]);
        assert!(plan.document_commands.iter().all(|command| match command {
            GpuDocumentCommand::CreateSurface { target, .. }
            | GpuDocumentCommand::UploadSurfaceTiles {
                surface: target, ..
            } => *target == mask_surface,
            _ => false,
        }));
    }

    #[test]
    fn solid_fill_layer_mask_creates_only_the_existing_mask_surface() {
        let (mut state, _) = state_with_default_layer();
        add_solid_fill_layer(&mut state, [0.25, 0.5, 0.75]).unwrap();
        let fill = state.document.active_layer_id();
        let fill_surface = PaintSurfaceId::raster(0.into(), fill);
        let mask_surface = PaintSurfaceId::layer_mask(0.into(), fill);
        assert!(
            !state
                .document()
                .unwrap()
                .tiles
                .contains_surface(fill_surface)
        );

        let output = add_layer_mask(&mut state, fill, LayerMaskInitMode::RevealAll).unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert!(
            state
                .document()
                .unwrap()
                .tiles
                .contains_surface(mask_surface)
        );
        assert!(
            !state
                .document()
                .unwrap()
                .tiles
                .contains_surface(fill_surface)
        );
        let created_surfaces = plan
            .document_commands
            .iter()
            .filter_map(|command| match command {
                GpuDocumentCommand::CreateSurface { target, .. } => Some(*target),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(created_surfaces, vec![mask_surface]);
        assert!(plan.document_commands.iter().all(|command| match command {
            GpuDocumentCommand::CreateSurface { target, .. }
            | GpuDocumentCommand::UploadSurfaceTiles {
                surface: target, ..
            } => *target == mask_surface,
            _ => false,
        }));
    }

    #[test]
    fn move_layer_mask_creates_mask_on_empty_target() {
        let (mut state, source) = state_with_default_layer();
        let target = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(source, "Target")
            .unwrap();
        add_layer_mask(&mut state, source, LayerMaskInitMode::HideAll).unwrap();
        let source_surface = PaintSurfaceId::layer_mask(0.into(), source);
        let target_surface = PaintSurfaceId::layer_mask(0.into(), target);

        let output = move_layer_mask(&mut state, source, target).unwrap();

        let document = state.document().unwrap();
        assert!(!document.layer_tree.has_layer_mask(source));
        assert!(document.layer_tree.has_layer_mask(target));
        assert!(!document.tiles.contains_surface(source_surface));
        assert!(document.tiles.contains_surface(target_surface));
        assert_eq!(state.active_layer_id(), target);
        assert_eq!(state.active_layer_target(), ActiveLayerTarget::LayerMask);
        assert!(has_pending_transaction(&output));
    }

    #[test]
    fn move_layer_mask_overwrites_target_across_materials_and_undoes_atomically() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [2, 1]),
                MaterialSpec::new("B", [2, 1]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let source = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let target = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(source, "Target")
            .unwrap();
        add_layer_mask(&mut state, source, LayerMaskInitMode::RevealAll).unwrap();
        add_layer_mask(&mut state, target, LayerMaskInitMode::RevealAll).unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_mask_enabled(source, false);

        let source_pixels = [vec![32; 8], vec![64; 8]];
        let target_pixels = [vec![192; 8], vec![224; 8]];
        for material_index in 0..2 {
            let document = state.document_mut().unwrap();
            let source_surface = PaintSurfaceId::layer_mask(material_index.into(), source);
            let target_surface = PaintSurfaceId::layer_mask(material_index.into(), target);
            let rect = RectU32::full([2, 1]);
            document
                .tiles
                .write_surface_rect(
                    source_surface,
                    rect,
                    &crate::application::PixelSnapshotData::contiguous(
                        source_pixels[material_index].clone(),
                    ),
                )
                .unwrap();
            document
                .tiles
                .write_surface_rect(
                    target_surface,
                    rect,
                    &crate::application::PixelSnapshotData::contiguous(
                        target_pixels[material_index].clone(),
                    ),
                )
                .unwrap();
        }

        let mut runtime = crate::application::ApplicationRuntime::new(state);
        runtime
            .dispatch(crate::application::Command::MoveLayerMask {
                source_layer_id: source,
                target_layer_id: target,
            })
            .unwrap();

        let document = runtime.state.document().unwrap();
        assert!(!document.layer_tree.has_layer_mask(source));
        assert_eq!(
            document
                .layer_tree
                .layer_mask(target)
                .map(|mask| mask.enabled),
            Some(false)
        );
        assert_eq!(runtime.state.active_layer_id(), target);
        assert_eq!(
            runtime.state.active_layer_target(),
            ActiveLayerTarget::LayerMask
        );
        for material_index in 0..2 {
            assert!(
                !document
                    .tiles
                    .contains_surface(PaintSurfaceId::layer_mask(material_index.into(), source))
            );
            assert_eq!(
                document
                    .tiles
                    .read_surface_full(PaintSurfaceId::layer_mask(material_index.into(), target))
                    .unwrap()
                    .rgba8,
                source_pixels[material_index]
            );
        }
        let history = runtime.state.history.undo_stack.last().unwrap();
        assert_eq!(history.label, "Move Layer Mask");
        assert!(matches!(
            history.atoms.as_slice(),
            [HistoryAtom::LayerTreeEdit { .. }]
        ));

        runtime.drain_renderer_frame_plan(Vec::new());
        runtime.dispatch(crate::application::Command::Undo).unwrap();
        runtime.drain_renderer_frame_plan(Vec::new());
        let document = runtime.state.document().unwrap();
        assert!(document.layer_tree.has_layer_mask(source));
        assert!(document.layer_tree.has_layer_mask(target));
        assert_eq!(
            document
                .layer_tree
                .layer_mask(source)
                .map(|mask| mask.enabled),
            Some(false)
        );
        assert_eq!(
            document
                .layer_tree
                .layer_mask(target)
                .map(|mask| mask.enabled),
            Some(true)
        );
        for material_index in 0..2 {
            assert_eq!(
                document
                    .tiles
                    .read_surface_full(PaintSurfaceId::layer_mask(material_index.into(), source))
                    .unwrap()
                    .rgba8,
                source_pixels[material_index]
            );
            assert_eq!(
                document
                    .tiles
                    .read_surface_full(PaintSurfaceId::layer_mask(material_index.into(), target))
                    .unwrap()
                    .rgba8,
                target_pixels[material_index]
            );
        }

        runtime.dispatch(crate::application::Command::Redo).unwrap();
        runtime.drain_renderer_frame_plan(Vec::new());
        let document = runtime.state.document().unwrap();
        assert!(!document.layer_tree.has_layer_mask(source));
        assert_eq!(
            document
                .layer_tree
                .layer_mask(target)
                .map(|mask| mask.enabled),
            Some(false)
        );
        for material_index in 0..2 {
            assert_eq!(
                document
                    .tiles
                    .read_surface_full(PaintSurfaceId::layer_mask(material_index.into(), target))
                    .unwrap()
                    .rgba8,
                source_pixels[material_index]
            );
        }
    }

    #[test]
    fn set_solid_fill_color_clamps_and_records_tree_sync_history() {
        let (mut state, raster) = state_with_default_layer();
        let fill = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_solid_fill_layer_above(raster, "Fill", [0.0, 0.0, 0.0])
            .unwrap();

        let output = set_solid_fill_color(&mut state, fill, [-1.0, 0.5, 2.0], 7).unwrap();

        assert_eq!(
            state.document().unwrap().layer_tree.solid_fill_color(fill),
            Some([0.0, 0.5, 1.0])
        );
        assert!(has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(pending.take_composite_sync(), CompositeSync::MaterialTrees);
    }

    #[test]
    fn set_solid_fill_color_rejects_raster_and_noop_without_history() {
        let (mut state, raster) = state_with_default_layer();
        let fill = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_solid_fill_layer_above(raster, "Fill", [0.1, 0.2, 0.3])
            .unwrap();

        let raster_output = set_solid_fill_color(&mut state, raster, [1.0, 0.0, 0.0], 1).unwrap();
        let noop_output = set_solid_fill_color(&mut state, fill, [0.1, 0.2, 0.3], 1).unwrap();

        assert!(!has_pending_transaction(&raster_output));
        assert!(!has_pending_transaction(&noop_output));
    }

    #[test]
    fn set_adjustment_normalizes_and_requests_adjustment_patch() {
        let (mut state, raster) = state_with_default_layer();
        let layer_id = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "Adjustment",
                AdjustmentKind::BrightnessContrast.default_adjustment(),
            )
            .unwrap();

        let output = set_adjustment(
            &mut state,
            layer_id,
            Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
                brightness: 150,
                contrast: -120,
            }),
            9,
        )
        .unwrap();

        assert_eq!(
            state.document().unwrap().layer_tree.adjustment(layer_id),
            Some(Adjustment::BrightnessContrast(
                BrightnessContrastAdjustment {
                    brightness: 150,
                    contrast: -50,
                }
            ))
        );
        assert!(has_pending_transaction(&output));
        assert!(!output.has_renderer_work());
        let mut pending = output.pending_transaction.unwrap();
        assert_eq!(
            pending.take_composite_sync(),
            CompositeSync::adjustment(layer_id)
        );
    }

    #[test]
    fn hide_all_mask_keeps_document_tiles_sparse() {
        let (mut state, layer_id) = state_with_default_layer();

        let output = add_layer_mask(&mut state, layer_id, LayerMaskInitMode::HideAll).unwrap();

        let mask = PaintSurfaceId::layer_mask(0.into(), layer_id);
        assert_eq!(
            state.document().unwrap().tiles.cached_tile_count(mask),
            Some(0)
        );
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        assert_eq!(plan.document_commands.len(), 1);
        assert!(matches!(
            plan.document_commands.first(),
            Some(GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::Transparent,
                ..
            }) if *target == mask
        ));
    }

    #[test]
    fn reveal_all_mask_uses_sparse_white_document_default() {
        let (mut state, layer_id) = state_with_default_layer();

        let output = add_layer_mask(&mut state, layer_id, LayerMaskInitMode::RevealAll).unwrap();

        let mask = PaintSurfaceId::layer_mask(0.into(), layer_id);
        let tiles = &state.document().unwrap().tiles;
        assert_eq!(tiles.surface_default_rgba8(mask), Some([255; 4]));
        assert_eq!(tiles.cached_tile_count(mask), Some(0));
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        assert_eq!(plan.document_commands.len(), 1);
        assert!(matches!(
            plan.document_commands.first(),
            Some(GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::SolidRgba8([255, 255, 255, 255]),
                ..
            }) if *target == mask
        ));
    }

    #[test]
    fn duplicate_group_copies_child_and_mask_surfaces() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .move_layer(layer_id, group, 0)
        );
        add_layer_mask(&mut state, group, LayerMaskInitMode::RevealAll).unwrap();
        add_layer_mask(&mut state, layer_id, LayerMaskInitMode::HideAll).unwrap();

        let output = duplicate_layer(&mut state, group).unwrap();

        assert!(has_pending_transaction(&output));
        assert!(output.has_renderer_work());
        let document = state.document().unwrap();
        let copied_group = state.document.editor.active_layer_id;
        assert_eq!(
            state.document.editor.active_target(document),
            ActiveLayerTarget::Structure
        );
        assert!(document.layer_tree.has_layer_mask(copied_group));
        let copied_child = document.layer_tree.children(copied_group).unwrap()[0];
        assert!(document.layer_tree.is_paintable(copied_child));
        assert!(document.layer_tree.has_layer_mask(copied_child));

        document
            .tiles
            .read_surface_full(PaintSurfaceId::layer_mask(0.into(), copied_group))
            .unwrap();
        document
            .tiles
            .read_surface_full(PaintSurfaceId::raster(0.into(), copied_child))
            .unwrap();
        document
            .tiles
            .read_surface_full(PaintSurfaceId::layer_mask(0.into(), copied_child))
            .unwrap();
    }

    #[test]
    fn merge_layers_replaces_adjacent_roots_and_builds_one_history_transaction() {
        let (mut state, base) = state_with_default_layer();
        add_layer(&mut state).unwrap();
        let top = state.document.active_layer_id();
        let base_surface = PaintSurfaceId::raster(0.into(), base);
        let top_surface = PaintSurfaceId::raster(0.into(), top);

        let output = merge_layers(&mut state, vec![top, base]).unwrap();
        let merged = state.document.active_layer_id();
        let target = PaintSurfaceId::raster(0.into(), merged);
        let document = state.document().unwrap();

        assert_ne!(merged, base);
        assert_ne!(merged, top);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::Raster
        );
        assert!(!document.layer_tree.contains(base));
        assert!(!document.layer_tree.contains(top));
        assert_eq!(
            document.layer_tree.get(merged).unwrap().props.name,
            "Layer 1"
        );
        assert_eq!(
            document
                .layer_tree
                .children(document.layer_tree.root())
                .unwrap(),
            &[merged]
        );
        assert!(document.tiles.contains_surface(target));
        assert!(!document.tiles.contains_surface(base_surface));
        assert!(!document.tiles.contains_surface(top_surface));

        let pending = output.pending_transaction.as_ref().unwrap();
        assert_eq!(pending.history_transactions().len(), 1);
        let history = &pending.history_transactions()[0];
        assert_eq!(history.label, "Merge Layers");
        assert_eq!(history.atoms.len(), 2);
        assert!(matches!(
            &history.atoms[0],
            crate::application::PendingHistoryAtom::Cpu(_)
        ));
        assert!(matches!(
            &history.atoms[1],
            crate::application::PendingHistoryAtom::Renderer(_)
        ));
        assert_eq!(
            pending
                .renderer_commit_request()
                .unwrap()
                .finalized_surfaces,
            vec![target]
        );

        let frame_plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        assert_eq!(frame_plan.document_commands.len(), 1);
        assert!(matches!(
            &frame_plan.document_commands[0],
            GpuDocumentCommand::CreateSurface { target: created, .. } if *created == target
        ));
        assert_eq!(frame_plan.edit_commands.len(), 1);
        let EditCommand::Composite(CompositeCommand::BakeToSurfaces {
            targets,
            delete_sources_after_bake,
            delete_embedded_images_after_bake,
        }) = &frame_plan.edit_commands[0]
        else {
            panic!("expected composite bake command");
        };
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target, target);
        assert_eq!(targets[0].tree.root.children.len(), 2);
        assert!(matches!(
            &targets[0].tree.root.children[0],
            CompositeNode::Raster { surface, .. } if *surface == base_surface
        ));
        assert!(matches!(
            &targets[0].tree.root.children[1],
            CompositeNode::Raster { surface, .. } if *surface == top_surface
        ));
        assert_eq!(
            delete_sources_after_bake.as_slice(),
            &[base_surface, top_surface]
        );
        assert!(delete_embedded_images_after_bake.is_empty());
    }

    #[test]
    fn rasterize_solid_fill_preserves_identity_and_mask_with_neutral_bake_props() {
        let (mut state, _) = state_with_default_layer();
        add_solid_fill_layer(&mut state, [0.25, 0.5, 0.75]).unwrap();
        let layer_id = state.document.active_layer_id();
        let persistent_id = state
            .document()
            .unwrap()
            .layer_tree
            .get(layer_id)
            .unwrap()
            .persistent_id;
        add_layer_mask(&mut state, layer_id, LayerMaskInitMode::HideAll).unwrap();
        select_layer_mask(&mut state, layer_id).unwrap();

        let output = rasterize_layer(&mut state, layer_id).unwrap();
        let document = state.document().unwrap();
        let node = document.layer_tree.get(layer_id).unwrap();
        assert!(matches!(node.content, LayerContent::Raster));
        assert_eq!(node.persistent_id, persistent_id);
        assert!(node.mask.is_some());
        assert_eq!(state.document.active_layer_id(), layer_id);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::LayerMask
        );

        let pending = output.pending_transaction.as_ref().unwrap();
        assert_eq!(pending.history_transactions().len(), 1);
        assert_eq!(pending.history_transactions()[0].label, "Rasterize Layer");
        assert_eq!(pending.history_transactions()[0].atoms.len(), 2);
        let frame_plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        let EditCommand::Composite(CompositeCommand::BakeToSurfaces {
            targets,
            delete_sources_after_bake,
            ..
        }) = &frame_plan.edit_commands[0]
        else {
            panic!("expected rasterize bake command");
        };
        assert!(delete_sources_after_bake.is_empty());
        let [CompositeNode::SolidFill { mask, props, .. }] =
            targets[0].tree.root.children.as_slice()
        else {
            panic!("expected solid fill bake source");
        };
        assert_eq!(*mask, None);
        assert!(props.visible);
        assert_eq!(props.opacity, 1.0);
        assert_eq!(props.blend_mode, LayerBlendMode::Normal);
    }

    #[test]
    fn apply_disabled_raster_mask_bakes_mask_and_selects_content() {
        let (mut state, layer_id) = state_with_default_layer();
        add_layer_mask(&mut state, layer_id, LayerMaskInitMode::HideAll).unwrap();
        set_layer_mask_enabled(&mut state, layer_id, false).unwrap();
        select_layer_mask(&mut state, layer_id).unwrap();
        let raster = PaintSurfaceId::raster(0.into(), layer_id);
        let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);

        let output = apply_layer_mask(&mut state, layer_id).unwrap();
        let document = state.document().unwrap();
        assert!(document.layer_tree.is_paintable(layer_id));
        assert!(!document.layer_tree.has_layer_mask(layer_id));
        assert!(document.tiles.contains_surface(raster));
        assert!(!document.tiles.contains_surface(mask_surface));
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::Raster
        );

        let pending = output.pending_transaction.as_ref().unwrap();
        assert_eq!(pending.history_transactions().len(), 1);
        assert_eq!(pending.history_transactions()[0].label, "Apply Layer Mask");
        let frame_plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        assert!(frame_plan.document_commands.is_empty());
        let EditCommand::Composite(CompositeCommand::BakeToSurfaces {
            targets,
            delete_sources_after_bake,
            ..
        }) = &frame_plan.edit_commands[0]
        else {
            panic!("expected mask apply bake command");
        };
        assert_eq!(delete_sources_after_bake, &[mask_surface]);
        let [CompositeNode::Raster { mask, props, .. }] = targets[0].tree.root.children.as_slice()
        else {
            panic!("expected raster bake source");
        };
        assert_eq!(*mask, Some(mask_surface));
        assert!(props.visible);
        assert_eq!(props.opacity, 1.0);
        assert_eq!(props.blend_mode, LayerBlendMode::Normal);
    }

    #[test]
    fn layer_bake_operations_reject_locked_and_unsupported_layers() {
        let (mut state, raster) = state_with_default_layer();
        add_solid_fill_layer(&mut state, [1.0, 0.0, 0.0]).unwrap();
        let fill = state.document.active_layer_id();
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .set_locked(fill, true)
        );
        assert!(
            !rasterize_layer(&mut state, fill)
                .unwrap()
                .has_renderer_work()
        );

        add_adjustment_layer(&mut state, AdjustmentKind::Invert).unwrap();
        let adjustment = state.document.active_layer_id();
        add_layer_mask(&mut state, adjustment, LayerMaskInitMode::RevealAll).unwrap();
        assert!(
            !apply_layer_mask(&mut state, adjustment)
                .unwrap()
                .has_renderer_work()
        );
        assert!(state.document().unwrap().layer_tree.contains(raster));
    }

    #[test]
    fn merge_layers_flattens_a_single_pass_through_group() {
        let (mut state, child) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(child, "Group")
            .unwrap();
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .move_layer(child, group, 0)
        );
        assert!(
            state
                .document_mut()
                .unwrap()
                .layer_tree
                .set_group_composite_mode(group, GroupCompositeMode::PassThrough,)
        );
        let child_surface = PaintSurfaceId::raster(0.into(), child);

        let output = merge_layers(&mut state, vec![group]).unwrap();
        let merged = state.document.active_layer_id();
        let target = PaintSurfaceId::raster(0.into(), merged);
        let document = state.document().unwrap();

        assert!(!document.layer_tree.contains(group));
        assert!(!document.layer_tree.contains(child));
        assert_eq!(document.layer_tree.get(merged).unwrap().props.name, "Group");
        assert_eq!(
            document
                .layer_tree
                .children(document.layer_tree.root())
                .unwrap(),
            &[merged]
        );
        assert!(document.tiles.contains_surface(target));
        assert!(!document.tiles.contains_surface(child_surface));

        let pending = output.pending_transaction.as_ref().unwrap();
        assert_eq!(pending.history_transactions().len(), 1);
        assert_eq!(pending.history_transactions()[0].label, "Merge Layers");

        let frame_plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        let EditCommand::Composite(CompositeCommand::BakeToSurfaces {
            targets,
            delete_sources_after_bake,
            delete_embedded_images_after_bake,
        }) = &frame_plan.edit_commands[0]
        else {
            panic!("expected composite bake command");
        };
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target, target);
        let [CompositeNode::Group(group_node)] = targets[0].tree.root.children.as_slice() else {
            panic!("expected the selected group as the bake root");
        };
        assert_eq!(group_node.layer_id, Some(group));
        assert_eq!(group_node.mode, GroupCompositeMode::PassThrough);
        assert_eq!(group_node.children.len(), 1);
        assert!(matches!(
            &group_node.children[0],
            CompositeNode::Raster { surface, .. } if *surface == child_surface
        ));
        assert_eq!(delete_sources_after_bake.as_slice(), &[child_surface]);
        assert!(delete_embedded_images_after_bake.is_empty());
    }

    #[test]
    fn merge_layers_creates_a_target_for_every_material() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [16, 4]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let base = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        add_layer(&mut state).unwrap();
        let top = state.document.active_layer_id();

        let output = merge_layers(&mut state, vec![base, top]).unwrap();
        let merged = state.document.active_layer_id();
        let target_a = PaintSurfaceId::raster(0.into(), merged);
        let target_b = PaintSurfaceId::raster(1.into(), merged);
        let document = state.document().unwrap();

        assert_eq!(document.texture_size_for_surface(target_a), Some([8, 8]));
        assert_eq!(document.texture_size_for_surface(target_b), Some([16, 4]));
        let frame_plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        let EditCommand::Composite(CompositeCommand::BakeToSurfaces { targets, .. }) =
            &frame_plan.edit_commands[0]
        else {
            panic!("expected composite bake command");
        };
        assert_eq!(
            targets
                .iter()
                .map(|target| target.target)
                .collect::<Vec<_>>(),
            vec![target_a, target_b]
        );
    }

    #[test]
    fn merge_layers_rejects_non_contiguous_roots_without_mutation() {
        let (mut state, base) = state_with_default_layer();
        add_layer(&mut state).unwrap();
        let middle = state.document.active_layer_id();
        add_layer(&mut state).unwrap();
        let top = state.document.active_layer_id();
        let before = state.document().unwrap().layer_tree.clone();

        let output = merge_layers(&mut state, vec![base, top]).unwrap();

        assert_eq!(state.document().unwrap().layer_tree, before);
        assert_eq!(state.document.active_layer_id(), top);
        assert!(state.document().unwrap().layer_tree.contains(middle));
        assert!(!output.has_renderer_work());
        assert!(!has_pending_transaction(&output));
    }

    #[test]
    fn next_numbered_layer_name_checks_all_node_names() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        tree.add_group_above(base, "Layer 1").unwrap();

        assert_eq!(next_numbered_layer_name(&tree, "Layer"), "Layer 2");
    }
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
