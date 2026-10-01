use anyhow::{Context, Result, ensure};

use crate::{
    core::{
        document::{ActiveLayerPart, Document, ImportedAsset, LayerInsertion, MaterialSpec},
        document_tile_store::InitialPixels,
        surface::PaintSurfaceId,
        texture_size::DEFAULT_TEXTURE_SIZE,
    },
    project::{
        DocumentFocusState, LayerTreeEditorState, LoadedProject, LoadedSurface, ProjectEditorState,
    },
};

/// Build the untouched startup document before submitting it to the normal project loader.
pub(crate) fn create_startup_project(max_texture_dimension_2d: u32) -> Result<LoadedProject> {
    ensure!(
        DEFAULT_TEXTURE_SIZE <= max_texture_dimension_2d,
        "startup texture exceeds GPU texture limit"
    );
    let imported = crate::import::load_builtin_cube()?;
    let materials = imported
        .materials
        .into_iter()
        .map(|material| {
            MaterialSpec::new(material.name, [DEFAULT_TEXTURE_SIZE; 2])
                .with_render_settings(material.render_settings)
        })
        .collect();
    let mut document = Document::from_imported(ImportedAsset {
        mesh: imported.mesh,
        materials,
    });
    let background = document
        .layer_tree
        .default_raster_layer()
        .context("startup background layer is missing")?;
    document
        .layer_tree
        .rename_layer(background, "Background".into());
    let background_surface = PaintSurfaceId::raster(0.into(), background);
    document.tiles.delete_surface(background_surface)?;
    let background_pixels = document.tiles.create_surface(
        background_surface,
        [DEFAULT_TEXTURE_SIZE; 2],
        InitialPixels::SolidRgba8([255; 4]),
    )?;
    let paint_layer = document
        .add_raster_layer(LayerInsertion::Above(background), "Layer 1")?
        .context("creating startup paint layer")?
        .layer_id;
    let surfaces = [
        background_surface,
        PaintSurfaceId::raster(0.into(), paint_layer),
    ]
    .into_iter()
    .map(|surface| LoadedSurface {
        surface,
        default_rgba8: document
            .tiles
            .surface_default_rgba8(surface)
            .expect("startup surface exists"),
        surface_revision: document
            .tiles
            .surface_revision(surface)
            .expect("startup surface exists"),
        tiles: if surface == background_surface {
            background_pixels.tiles.clone()
        } else {
            Vec::new()
        },
    })
    .collect();
    Ok(LoadedProject {
        document,
        editor_state: ProjectEditorState {
            document_focus: DocumentFocusState {
                active_layer_id: paint_layer,
                active_layer_part: ActiveLayerPart::Content,
                focused_material_index: 0,
            },
            view: crate::project::default_editor_view_state(),
            layer_tree: LayerTreeEditorState {
                collapsed_layer_group_ids: Vec::new(),
            },
        },
        surfaces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{AppState, ApplicationRuntime, Command};
    use glam::Vec3;

    #[test]
    fn cube_has_outward_flat_faces_and_separate_equal_square_uv_islands() {
        let project = create_startup_project(4096).unwrap();
        let mesh = &project.document.mesh;
        assert_eq!(mesh.mesh_objects.len(), 1);
        assert_eq!(mesh.mesh_objects[0].name, "Cube");
        assert_eq!(mesh.positions.len(), 24);
        assert_eq!(mesh.indices.len(), 12);
        assert_eq!(
            mesh.scene_bounds(),
            Some((Vec3::splat(-0.5), Vec3::splat(0.5)))
        );
        let mut islands = Vec::new();
        for face in 0..6 {
            let start = face * 4;
            let normal = mesh.normals[start];
            assert_eq!(normal.length(), 1.0);
            assert!(mesh.normals[start..start + 4].iter().all(|n| *n == normal));
            for triangle in &mesh.indices[face * 2..face * 2 + 2] {
                let [a, b, c] = triangle.map(|i| mesh.positions[i as usize]);
                assert!((b - a).cross(c - a).normalize().abs_diff_eq(normal, 1e-6));
                assert!(normal.dot(a) > 0.0);
            }
            let min = mesh.uvs[start];
            let max = mesh.uvs[start + 2];
            let size = max - min;
            assert!((size.x - size.y).abs() < 1e-6);
            assert!((size.x - (1.0 / 3.0 - 8.0 / 1024.0)).abs() < 1e-6);
            assert!(min.min_element() > 0.0 && max.max_element() < 1.0);
            for (other_min, other_max) in &islands {
                let other_min: &glam::Vec2 = other_min;
                let other_max: &glam::Vec2 = other_max;
                assert!(
                    max.x < other_min.x
                        || min.x > other_max.x
                        || max.y < other_min.y
                        || min.y > other_max.y
                );
            }
            islands.push((min, max));
        }
    }

    #[test]
    fn startup_load_has_white_background_transparent_active_layer_and_no_history() {
        let project = create_startup_project(4096).unwrap();
        assert_eq!(project.document.materials.len(), 1);
        assert_eq!(project.document.materials[0].texture_size, [1024; 2]);
        assert_eq!(
            project.document.materials[0].render_settings.render_mode,
            crate::core::material::MaterialRenderMode::Opaque
        );
        let background = project.surfaces[0].surface;
        let paint = project.surfaces[1].surface;
        assert_eq!(
            project
                .document
                .tiles
                .read_surface_full(background)
                .unwrap()
                .rgba8,
            vec![255; 1024 * 1024 * 4]
        );
        assert_eq!(
            project
                .document
                .tiles
                .read_surface_full(paint)
                .unwrap()
                .rgba8,
            vec![0; 1024 * 1024 * 4]
        );
        let active = project.editor_state.document_focus.active_layer_id;
        let mut runtime = ApplicationRuntime::new(AppState::with_status("ready"));
        runtime.dispatch(Command::ProjectLoaded(project)).unwrap();
        assert_eq!(runtime.state.active_layer_id(), active);
        assert!(!runtime.state.can_undo());
        assert!(!runtime.state.can_redo());
        assert!(!runtime.has_pending_history_transaction());
    }

    #[test]
    fn startup_project_roundtrips_without_a_source_model_path() {
        let project = create_startup_project(4096).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cube.holopaint");
        crate::project::save_project(&project.document, &project.editor_state, &path).unwrap();
        let restored = crate::project::open_project(&path).unwrap();
        assert_eq!(
            restored.document.mesh.positions,
            project.document.mesh.positions
        );
        assert_eq!(restored.document.mesh.uvs, project.document.mesh.uvs);
        assert_eq!(
            restored.document.mesh.normals,
            project.document.mesh.normals
        );
        assert_eq!(
            restored.document.mesh.indices,
            project.document.mesh.indices
        );
        assert_eq!(restored.document.materials, project.document.materials);
        assert_eq!(restored.editor_state, project.editor_state);
        for surface in project.surfaces {
            assert_eq!(
                restored
                    .document
                    .tiles
                    .read_surface_full(surface.surface)
                    .unwrap(),
                project
                    .document
                    .tiles
                    .read_surface_full(surface.surface)
                    .unwrap()
            );
        }
    }

    #[test]
    fn startup_rejects_an_insufficient_gpu_texture_limit() {
        assert!(create_startup_project(512).is_err());
    }
}
