use crate::{
    application::{
        CompositeSync, HistoryAtom, MeshReloadRequest, PixelSnapshotData, RectU32, ReducerOutput,
    },
    core::{
        document::{Document, ImportedAsset},
        image::{LayerInitialPixels, MaterialPayload},
        material::MaterialIndex,
    },
    renderer::{GpuDocumentCommand, MaterialRegistration, MaterialUpload},
};

use anyhow::{Result, anyhow, bail, ensure};

use super::state::{AppState, EditorDocumentState, PaintTargetScope};

pub fn asset_loaded(
    state: &mut AppState,
    asset: ImportedAsset,
    materials: Vec<MaterialPayload>,
) -> Result<ReducerOutput> {
    let mesh = asset.mesh.clone();
    let mut document = Document::from_imported(asset);
    let editor_document = EditorDocumentState::from_document(&document);
    let active_layer = editor_document.active_layer_id;

    for (material_index, payload) in materials.iter().enumerate() {
        let surface =
            crate::core::surface::PaintSurfaceId::raster(material_index.into(), active_layer);
        let rect = RectU32::full([payload.width, payload.height]);
        document
            .tiles
            .write_surface_rect(
                surface,
                rect,
                &PixelSnapshotData::contiguous(payload.rgba8.clone()),
            )
            .map_err(|err| anyhow!("Document tile import failed: {err:#}"))?;
    }

    let surfaces = editor_document
        .resolve_paint_target(&document, PaintTargetScope::AllMaterials)
        .map(|target| target.surfaces_vec())
        .unwrap_or_default();
    let material_uploads = materials
        .into_iter()
        .zip(document.materials.iter())
        .map(|(texture, material)| MaterialUpload {
            texture,
            render_settings: material.render_settings,
        })
        .collect();

    state.replace_document_with_editor(document, editor_document);

    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::UploadScene {
        mesh,
        materials: material_uploads,
        surfaces,
    });
    output.push_document_command(GpuDocumentCommand::SyncEmbeddedImages { images: Vec::new() });
    Ok(output.with_composite_sync(CompositeSync::MaterialTrees))
}

pub fn project_loaded(
    state: &mut AppState,
    loaded: crate::project::LoadedProject,
) -> Result<ReducerOutput> {
    let crate::project::LoadedProject {
        document,
        editor_state,
        surfaces: loaded_surfaces,
    } = loaded;
    let editor_document = EditorDocumentState {
        focused_material_index: editor_state.document_focus.focused_material_index.into(),
        active_layer_id: editor_state.document_focus.active_layer_id,
        active_part: editor_state.document_focus.active_layer_part,
    };
    let mesh = document.mesh.clone();
    let materials = document
        .materials
        .iter()
        .map(|material| {
            let pixel_count = (material.texture_size[0] as usize)
                .checked_mul(material.texture_size[1] as usize)
                .and_then(|count| count.checked_mul(4))
                .ok_or_else(|| anyhow!("project material texture size overflows"))?;
            Ok(MaterialUpload {
                texture: MaterialPayload {
                    width: material.texture_size[0],
                    height: material.texture_size[1],
                    rgba8: vec![0; pixel_count],
                },
                render_settings: material.render_settings,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let surface_initializations = loaded_surfaces
        .iter()
        .map(|loaded| {
            (
                loaded.surface,
                if loaded.default_rgba8 == [0; 4] {
                    LayerInitialPixels::Transparent
                } else {
                    LayerInitialPixels::SolidRgba8(loaded.default_rgba8)
                },
            )
        })
        .collect::<Vec<_>>();
    let uploads = loaded_surfaces
        .into_iter()
        .filter_map(|loaded| {
            (!loaded.tiles.is_empty()).then_some(GpuDocumentCommand::UploadSurfaceTiles {
                surface: loaded.surface,
                surface_revision: loaded.surface_revision,
                tiles: loaded.tiles,
            })
        })
        .collect::<Vec<_>>();

    let embedded_images = document.embedded_images().cloned().collect();
    state.replace_document_with_editor(document, editor_document);
    state.view.camera = editor_state.view.camera;
    state.view.shading = editor_state.view.shading;
    state.view.gizmo_visible = editor_state.view.gizmo_visible;
    state.view.background_color = editor_state.view.background_color;
    state.view.viewport_wireframe_visible = editor_state.view.viewport_wireframe_visible;
    state.view.viewport_wireframe = editor_state.view.viewport_wireframe;
    state.view.scene_visibility = editor_state.view.scene_visibility;
    state.view.surface_mirror_x_enabled = editor_state.view.surface_mirror_x_enabled;
    state.view.surface_mirror_x_plane = editor_state.view.surface_mirror_x_plane;
    state.view.surface_mirror_x_plane_visible = editor_state.view.surface_mirror_x_plane_visible;
    state.view.uv_view_transform = editor_state.view.uv_view_transform;
    state.view.uv_background_color = editor_state.view.uv_background_color;
    state.view.uv_wireframe_visible = editor_state.view.uv_wireframe_visible;
    state.view.uv_wireframe = editor_state.view.uv_wireframe;
    state.view.uv_view_transform_initialized = true;
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::UploadScene {
        mesh,
        materials,
        surfaces: Vec::new(),
    });
    output.push_document_command(GpuDocumentCommand::SyncEmbeddedImages {
        images: embedded_images,
    });
    for (target, initial) in surface_initializations {
        output.push_document_command(GpuDocumentCommand::CreateSurface { target, initial });
    }
    for upload in uploads {
        output.push_document_command(upload);
    }
    Ok(output.with_composite_sync(CompositeSync::MaterialTrees))
}

pub fn reload_mesh(state: &mut AppState, request: MeshReloadRequest) -> Result<ReducerOutput> {
    if state.is_document_edit_interacting() {
        return Ok(ReducerOutput::default());
    }
    let MeshReloadRequest {
        mesh,
        new_materials,
    } = request;
    let reload = {
        let Some(document) = state.document_mut() else {
            return Ok(ReducerOutput::default());
        };
        document.reload_mesh(mesh.clone(), new_materials)?
    };

    let document = state
        .document()
        .expect("reloaded document must still exist");
    let registrations = reload
        .appended_material_indices
        .iter()
        .filter_map(|&material_index| document.material(material_index))
        .map(|material| MaterialRegistration {
            texture_size: material.texture_size,
            render_settings: material.render_settings,
        })
        .collect::<Vec<_>>();
    let active_selection = document.active_selection.clone();
    let default_raster_layer = document.layer_tree.default_raster_layer();

    let mut output = ReducerOutput::default();
    if !registrations.is_empty() {
        output.push_document_command(GpuDocumentCommand::AppendMaterials {
            materials: registrations,
        });
    }
    for surface in &reload.created_surfaces {
        output.push_document_command(GpuDocumentCommand::CreateSurface {
            target: *surface,
            initial: if surface.is_mask() || Some(surface.layer_id) == default_raster_layer {
                LayerInitialPixels::SolidRgba8([255; 4])
            } else {
                LayerInitialPixels::Transparent
            },
        });
    }
    output.push_document_command(GpuDocumentCommand::ReplaceMesh { mesh });
    output.push_document_command(GpuDocumentCommand::UploadSelectionTiles {
        active_selection,
        tiles: Vec::new(),
    });

    state.finish_mesh_reload();
    state.set_status_message(
        super::StatusMessage::localized("status-mesh-reloaded")
            .arg("count", reload.appended_material_indices.len()),
    );
    Ok(output
        .with_composite_sync(CompositeSync::material_trees_scoped(
            reload.appended_material_indices,
        ))
        .with_project_change())
}

pub fn set_focused_material(state: &mut AppState, index: usize) -> Result<ReducerOutput> {
    if index == state.focused_material_index()
        || !state
            .document()
            .is_some_and(|document| document.materials.get(index).is_some())
    {
        return Ok(ReducerOutput::default());
    }
    let output = if state.tool.embedded_image_transform_session().is_some() {
        super::transform_controller::commit_active_embedded_image_transform(state)?
    } else {
        ReducerOutput::default()
    };
    state.set_focused_material(index);
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

pub fn set_material_render_settings(
    state: &mut AppState,
    material_index: usize,
    settings: crate::core::material::MaterialRenderSettings,
) -> ReducerOutput {
    let Some(material) = state
        .document_mut()
        .and_then(|document| document.materials.get_mut(material_index))
    else {
        return ReducerOutput::default();
    };
    if material.render_settings == settings {
        return ReducerOutput::default();
    }

    material.render_settings = settings;
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::SetMaterialRenderSettings {
        material_index,
        settings: material.render_settings,
    });
    output.with_project_change()
}

pub fn resize_material_texture(
    state: &mut AppState,
    material_index: usize,
    texture_size: [u32; 2],
) -> Result<ReducerOutput> {
    if state.is_document_edit_interacting() {
        return Ok(ReducerOutput::default());
    }
    let material_index = MaterialIndex(material_index);
    let resize = {
        let Some(document) = state.document_mut() else {
            return Ok(ReducerOutput::default());
        };
        document.resize_material_texture(material_index, texture_size)?
    };
    let Some(resize) = resize else {
        return Ok(ReducerOutput::default());
    };
    let active_selection = state
        .document()
        .expect("resized document must still exist")
        .active_selection
        .clone();
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::ResizeMaterialTexture {
        snapshot: resize.after.clone(),
        active_selection,
    });
    state.tool.invalidate_transform_idle_cache();
    state.set_status_message(
        super::StatusMessage::localized("status-texture-size-changed")
            .arg("width", texture_size[0])
            .arg("height", texture_size[1]),
    );
    Ok(output
        .with_finalized_history_atom(
            "Resize Material Texture",
            HistoryAtom::MaterialTextureResize {
                before: resize.before,
                after: resize.after,
            },
        )
        .with_composite_sync(CompositeSync::material_trees_scoped([material_index])))
}

pub fn set_material_export_image_file_names(
    state: &mut AppState,
    names: Vec<(crate::core::material::MaterialId, String)>,
) -> Result<ReducerOutput> {
    let document = state
        .document_mut()
        .ok_or_else(|| anyhow!("export filename update requires a loaded document"))?;
    let mut ids = std::collections::HashSet::with_capacity(names.len());
    for (material_id, file_name) in &names {
        ensure!(
            ids.insert(*material_id),
            "duplicate material id in export filename update: {}",
            material_id.0
        );
        ensure!(!file_name.is_empty(), "export filename must not be empty");
        if !document
            .materials
            .iter()
            .any(|material| material.id == *material_id)
        {
            bail!(
                "material {} does not exist for export filename update",
                material_id.0
            );
        }
    }

    let mut changed = false;
    for (material_id, file_name) in names {
        let material = document
            .materials
            .iter_mut()
            .find(|material| material.id == material_id)
            .expect("all export filename material ids were validated");
        if material.export_image_file_name.as_deref() != Some(file_name.as_str()) {
            material.export_image_file_name = Some(file_name);
            changed = true;
        }
    }
    Ok(if changed {
        ReducerOutput::default().with_project_change()
    } else {
        ReducerOutput::default()
    })
}

pub fn set_material_export_psd_file_names(
    state: &mut AppState,
    names: Vec<(crate::core::material::MaterialId, String)>,
) -> Result<ReducerOutput> {
    let document = state
        .document_mut()
        .ok_or_else(|| anyhow!("PSD export filename update requires a loaded document"))?;
    let mut ids = std::collections::HashSet::with_capacity(names.len());
    for (material_id, file_name) in &names {
        ensure!(
            ids.insert(*material_id),
            "duplicate material id in PSD export filename update: {}",
            material_id.0
        );
        ensure!(
            !file_name.is_empty(),
            "PSD export filename must not be empty"
        );
        if !document
            .materials
            .iter()
            .any(|material| material.id == *material_id)
        {
            bail!(
                "material {} does not exist for PSD export filename update",
                material_id.0
            );
        }
    }

    let mut changed = false;
    for (material_id, file_name) in names {
        let material = document
            .materials
            .iter_mut()
            .find(|material| material.id == material_id)
            .expect("all PSD export filename material ids were validated");
        if material.export_psd_file_name.as_deref() != Some(file_name.as_str()) {
            material.export_psd_file_name = Some(file_name);
            changed = true;
        }
    }
    Ok(if changed {
        ReducerOutput::default().with_project_change()
    } else {
        ReducerOutput::default()
    })
}

pub fn clear_document(state: &mut AppState) -> ReducerOutput {
    state.clear_document_state();
    ReducerOutput::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        document::{Document, MeshData},
        material::{MaterialId, MaterialSpec},
    };

    fn state_with_two_materials() -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        ));
        state
    }

    #[test]
    fn export_file_names_update_by_material_id_without_renderer_work() {
        let mut state = state_with_two_materials();
        let output = set_material_export_image_file_names(
            &mut state,
            vec![
                (MaterialId(2), "b.png".to_owned()),
                (MaterialId(1), "a.png".to_owned()),
            ],
        )
        .unwrap();
        assert!(!output.has_renderer_work());
        assert!(output.project_changed());
        let document = state.document().unwrap();
        assert_eq!(
            document.materials[0].export_image_file_name.as_deref(),
            Some("a.png")
        );
        assert_eq!(
            document.materials[1].export_image_file_name.as_deref(),
            Some("b.png")
        );
    }

    #[test]
    fn missing_material_does_not_partially_update_export_file_names() {
        let mut state = state_with_two_materials();
        let result = set_material_export_image_file_names(
            &mut state,
            vec![
                (MaterialId(1), "a.png".to_owned()),
                (MaterialId(99), "missing.png".to_owned()),
            ],
        );
        assert!(result.is_err());
        assert!(
            state
                .document()
                .unwrap()
                .materials
                .iter()
                .all(|material| material.export_image_file_name.is_none())
        );
    }

    #[test]
    fn psd_export_file_names_update_independently_from_image_names() {
        let mut state = state_with_two_materials();
        set_material_export_image_file_names(&mut state, vec![(MaterialId(1), "a.png".to_owned())])
            .unwrap();
        let output = set_material_export_psd_file_names(
            &mut state,
            vec![
                (MaterialId(2), "b.psd".to_owned()),
                (MaterialId(1), "a.psd".to_owned()),
            ],
        )
        .unwrap();

        assert!(!output.has_renderer_work());
        assert!(output.project_changed());
        let document = state.document().unwrap();
        assert_eq!(
            document.materials[0].export_image_file_name.as_deref(),
            Some("a.png")
        );
        assert_eq!(
            document.materials[0].export_psd_file_name.as_deref(),
            Some("a.psd")
        );
        assert_eq!(
            document.materials[1].export_psd_file_name.as_deref(),
            Some("b.psd")
        );
    }

    #[test]
    fn unchanged_export_file_name_is_not_a_project_change() {
        let mut state = state_with_two_materials();
        set_material_export_image_file_names(&mut state, vec![(MaterialId(1), "a.png".to_owned())])
            .unwrap();

        let output = set_material_export_image_file_names(
            &mut state,
            vec![(MaterialId(1), "a.png".to_owned())],
        )
        .unwrap();

        assert!(!output.project_changed());
    }
}
