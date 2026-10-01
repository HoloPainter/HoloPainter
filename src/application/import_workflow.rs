use anyhow::{Context, Result, ensure};

use crate::{
    application::Command,
    core::{document::MaterialSpec, image::MaterialPayload, image_resize::resize_rgba8},
    import::{ImportedAsset, ImportedTexture},
};

pub fn create_new_project_command(
    imported: &ImportedAsset,
    materials: Vec<MaterialSpec>,
    base_color_textures: Option<&[Option<ImportedTexture>]>,
    max_texture_dimension_2d: u32,
) -> Result<Command> {
    ensure!(
        !materials.is_empty(),
        "source mesh does not contain any materials"
    );
    ensure!(
        materials.len() == imported.materials.len(),
        "material settings do not match the source mesh"
    );
    if let Some(textures) = base_color_textures {
        ensure!(
            textures.len() == materials.len(),
            "imported Base Color Textures do not match the source materials"
        );
    }
    let material_payloads = materials
        .iter()
        .enumerate()
        .map(|(index, material)| {
            let texture = base_color_textures
                .and_then(|textures| textures.get(index))
                .and_then(Option::as_ref);
            material_payload(material.texture_size, texture, max_texture_dimension_2d)
                .with_context(|| format!("preparing material {:?}", material.name))
        })
        .collect::<Result<Vec<_>>>()?;
    let core_asset = crate::core::document::ImportedAsset {
        mesh: imported.mesh.clone(),
        materials,
    };

    Ok(Command::AssetLoaded {
        asset: core_asset,
        materials: material_payloads,
    })
}

fn material_payload(
    texture_size: [u32; 2],
    imported_texture: Option<&ImportedTexture>,
    max_texture_dimension_2d: u32,
) -> Result<MaterialPayload> {
    ensure!(
        texture_size[0] > 0 && texture_size[1] > 0,
        "material texture dimensions must be non-zero"
    );
    ensure!(
        texture_size[0] <= max_texture_dimension_2d && texture_size[1] <= max_texture_dimension_2d,
        "material texture size {}x{} exceeds GPU texture limit {}",
        texture_size[0],
        texture_size[1],
        max_texture_dimension_2d
    );
    if let Some(imported_texture) = imported_texture {
        let rgba8 = resize_rgba8(imported_texture.size, texture_size, &imported_texture.rgba8)?;
        return Ok(MaterialPayload {
            width: texture_size[0],
            height: texture_size[1],
            rgba8,
        });
    }
    let byte_len = crate::core::image::rgba8_len(texture_size)?;
    let mut rgba8 = Vec::new();
    rgba8
        .try_reserve_exact(byte_len)
        .map_err(|error| anyhow::anyhow!("allocating opaque white material texture: {error}"))?;
    rgba8.resize(byte_len, 255);
    Ok(MaterialPayload {
        width: texture_size[0],
        height: texture_size[1],
        rgba8,
    })
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use crate::{
        application::{AppState, ApplicationRuntime},
        core::{
            document::{MeshData, MeshId, MeshObject, SubMesh},
            material::{MaterialRenderMode, MaterialRenderSettings},
            surface::PaintSurfaceId,
        },
        import::{ImportedAsset, ImportedMaterial},
    };

    use super::*;

    fn imported_asset() -> ImportedAsset {
        let mesh_id = MeshId(0);
        let mesh = MeshData::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y],
            vec![Vec3::Z; 3],
            vec![[0, 1, 2]],
            vec![SubMesh {
                mesh_id,
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Body".to_owned(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".to_owned(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        ImportedAsset {
            mesh,
            materials: vec![ImportedMaterial {
                source_material_index: None,
                name: "Body".to_owned(),
                render_settings: MaterialRenderSettings::default(),
            }],
        }
    }

    #[test]
    fn new_project_uses_independent_dimensions_settings_and_opaque_white_pixels() {
        let imported = imported_asset();
        let settings = MaterialRenderSettings {
            double_sided: false,
            render_mode: MaterialRenderMode::Blend,
            alpha_cutoff: 91,
        };
        let command = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Configured", [4, 2]).with_render_settings(settings)],
            None,
            4096,
        )
        .unwrap();
        let mut runtime = ApplicationRuntime::new(AppState::with_status("ready"));
        runtime.dispatch(command).unwrap();

        let document = runtime.state.document().unwrap();
        assert_eq!(document.materials[0].name, "Configured");
        assert_eq!(document.materials[0].texture_size, [4, 2]);
        assert_eq!(document.materials[0].render_settings, settings);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let image = document
            .tiles
            .read_surface_full(PaintSurfaceId::raster(0.into(), layer))
            .unwrap();
        assert_eq!(image.texture_size, [4, 2]);
        assert_eq!(image.rgba8, vec![255; 4 * 2 * 4]);

        let first_project_id = document.project_id;
        let replacement = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Replacement", [2, 2])],
            None,
            4096,
        )
        .unwrap();
        runtime.dispatch(replacement).unwrap();
        assert_ne!(
            runtime.state.document().unwrap().project_id,
            first_project_id
        );
    }

    #[test]
    fn new_project_rejects_material_mismatch_and_gpu_limit() {
        let imported = imported_asset();
        assert!(create_new_project_command(&imported, Vec::new(), None, 4096).is_err());
        let error = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Body", [4096, 4096])],
            None,
            2048,
        )
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("exceeds GPU texture limit"));
    }

    #[test]
    fn imported_base_color_pixels_round_trip_without_a_source_mesh_path() {
        let imported = imported_asset();
        let expected = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 0, 0, 10, 20, 30, 40, 50, 60,
            70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ];
        let textures = vec![Some(ImportedTexture {
            size: [4, 2],
            rgba8: expected.clone(),
        })];
        let command = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Body", [4, 2])],
            Some(&textures),
            4096,
        )
        .unwrap();
        let mut runtime = ApplicationRuntime::new(AppState::with_status("ready"));
        runtime.dispatch(command).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("standalone.holopaint");
        let editor_state = runtime
            .state
            .project_editor_state(Vec::new())
            .expect("project editor state");
        crate::project::save_project(runtime.state.document().unwrap(), &editor_state, &path)
            .unwrap();

        let loaded = crate::project::open_project(&path).unwrap();
        assert_eq!(loaded.document.materials[0].texture_size, [4, 2]);
        let layer = loaded.document.layer_tree.default_raster_layer().unwrap();
        let image = loaded
            .document
            .tiles
            .read_surface_full(PaintSurfaceId::raster(0.into(), layer))
            .unwrap();
        assert_eq!(image.rgba8, expected);
    }

    #[test]
    fn imported_base_color_is_resized_and_failure_does_not_replace_project() {
        let imported = imported_asset();
        let textures = vec![Some(ImportedTexture {
            size: [1, 1],
            rgba8: vec![40, 80, 120, 128],
        })];
        let command = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Body", [2, 2])],
            Some(&textures),
            4096,
        )
        .unwrap();
        let mut runtime = ApplicationRuntime::new(AppState::with_status("ready"));
        runtime.dispatch(command).unwrap();
        let original_project_id = runtime.state.document().unwrap().project_id;
        let layer = runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let image = runtime
            .state
            .document()
            .unwrap()
            .tiles
            .read_surface_full(PaintSurfaceId::raster(0.into(), layer))
            .unwrap();
        assert_eq!(image.texture_size, [2, 2]);
        assert_eq!(image.rgba8, [40, 80, 120, 128].repeat(4));

        let invalid_textures = vec![Some(ImportedTexture {
            size: [2, 2],
            rgba8: vec![0; 3],
        })];
        let error = create_new_project_command(
            &imported,
            vec![MaterialSpec::new("Replacement", [2, 2])],
            Some(&invalid_textures),
            4096,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("preparing material"));
        assert_eq!(
            runtime.state.document().unwrap().project_id,
            original_project_id
        );
    }
}
