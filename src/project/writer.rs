use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Write,
    path::Path,
};

use tempfile::NamedTempFile;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::core::{
    adjustment::{Adjustment, UvMirrorAxis, UvMirrorDirection},
    camera::CameraProjection,
    composite::{GroupCompositeMode, LayerBlendMode},
    document::{ActiveLayerPart, Document},
    material::{MaterialIndex, MaterialRenderMode},
    surface::{LayerContent, LayerMaterialMask, PaintSurfaceId},
};

use super::{ProjectEditorState, v1::*, validate_project_v1};

#[derive(Debug)]
pub struct SaveProjectError {
    message: String,
}

impl SaveProjectError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SaveProjectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for SaveProjectError {}

pub fn save_project(
    document: &Document,
    editor_state: &ProjectEditorState,
    destination: &Path,
) -> Result<(), SaveProjectError> {
    let active_layer_persistent_id = document
        .layer_tree
        .persistent_id(editor_state.document_focus.active_layer_id)
        .ok_or_else(|| SaveProjectError::new("active layer persistent ID is missing"))?;
    let focused_material_id = document
        .materials
        .get(editor_state.document_focus.focused_material_index)
        .ok_or_else(|| SaveProjectError::new("focused material is missing"))?
        .id;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        SaveProjectError::new(format!("creating temporary project file: {error}"))
    })?;

    {
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        let metadata_options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let surface_options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        let (mesh, geometry) = encode_geometry(document)?;
        archive
            .start_file("geometry.bin", metadata_options)
            .map_err(|error| SaveProjectError::new(format!("starting geometry.bin: {error}")))?;
        archive
            .write_all(&geometry)
            .map_err(|error| SaveProjectError::new(format!("writing geometry.bin: {error}")))?;

        let materials = convert_materials(document);
        let layers = convert_layers(document)?;
        let embedded_images = write_embedded_images(document, &mut archive, metadata_options)?;
        let surfaces = write_surfaces(document, &mut archive, surface_options)?;
        let project = ProjectV1 {
            format_version: 1,
            project_id: document.project_id.0.hyphenated().to_string(),
            created_with: format!("HoloPainter {}", env!("CARGO_PKG_VERSION")),
            editor_state: convert_editor_state(
                document,
                editor_state,
                active_layer_persistent_id.0,
                focused_material_id.0,
            )?,
            id_counters: IdCountersV1 {
                mesh_object: document.mesh_object_id_high_water(),
                material: document.material_id_high_water(),
                layer: document.layer_tree.persistent_id_high_water(),
                embedded_image: document.embedded_image_id_high_water(),
            },
            mesh,
            materials,
            embedded_images,
            layers,
            surfaces,
        };
        validate_project_v1(&project, geometry.len() as u64).map_err(|error| {
            SaveProjectError::new(format!("project validation failed: {error}"))
        })?;

        let project_ron = ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
            .map_err(|error| {
            SaveProjectError::new(format!("serializing project.ron: {error}"))
        })?;
        archive
            .start_file("project.ron", metadata_options)
            .map_err(|error| SaveProjectError::new(format!("starting project.ron: {error}")))?;
        archive
            .write_all(project_ron.as_bytes())
            .map_err(|error| SaveProjectError::new(format!("writing project.ron: {error}")))?;
        archive.finish().map_err(|error| {
            SaveProjectError::new(format!("finishing project archive: {error}"))
        })?;
    }

    temporary
        .as_file()
        .sync_all()
        .map_err(|error| SaveProjectError::new(format!("syncing project archive: {error}")))?;
    temporary.persist(destination).map_err(|error| {
        SaveProjectError::new(format!(
            "replacing destination project file: {}",
            error.error
        ))
    })?;
    Ok(())
}

fn convert_editor_state(
    document: &Document,
    editor_state: &ProjectEditorState,
    active_layer_id: u64,
    focused_material_id: u64,
) -> Result<ProjectEditorStateV1, SaveProjectError> {
    let mut hidden_mesh_object_ids = document
        .mesh
        .mesh_objects
        .iter()
        .filter(|object| !editor_state.view.scene_visibility.mesh_visible(object.id))
        .map(|object| {
            document
                .mesh_object_persistent_id(object.id)
                .map(|id| id.0)
                .ok_or_else(|| SaveProjectError::new("hidden mesh persistent ID is missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    hidden_mesh_object_ids.sort_unstable();

    let mut hidden_material_ids = document
        .materials
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !editor_state
                .view
                .scene_visibility
                .material_visible((*index).into())
        })
        .map(|(_, material)| material.id.0)
        .collect::<Vec<_>>();
    hidden_material_ids.sort_unstable();

    let mut collapsed_layer_group_ids = editor_state
        .layer_tree
        .collapsed_layer_group_ids
        .iter()
        .filter(|layer_id| document.layer_tree.is_group(**layer_id))
        .filter_map(|layer_id| document.layer_tree.persistent_id(*layer_id))
        .map(|id| id.0)
        .collect::<Vec<_>>();
    collapsed_layer_group_ids.sort_unstable();
    collapsed_layer_group_ids.dedup();

    let camera = &editor_state.view.camera;
    Ok(ProjectEditorStateV1 {
        document_focus: DocumentFocusStateV1 {
            active_layer_id,
            active_layer_part: match editor_state.document_focus.active_layer_part {
                ActiveLayerPart::Content => ActiveLayerPartV1::Content,
                ActiveLayerPart::LayerMask => ActiveLayerPartV1::LayerMask,
            },
            focused_material_id,
        },
        view: EditorViewStateV1 {
            viewport_3d: Viewport3dEditorStateV1 {
                camera: Camera3dStateV1 {
                    target: camera.target.to_array(),
                    orientation: camera.orientation.to_array(),
                    distance: camera.distance,
                    projection: match camera.projection {
                        CameraProjection::Perspective => CameraProjectionV1::Perspective,
                        CameraProjection::Orthographic => CameraProjectionV1::Orthographic,
                    },
                    fov_y_radians: camera.fov_y_radians,
                    orthographic_height: camera.orthographic_height,
                },
                shading: match editor_state.view.shading {
                    crate::core::viewport_shading::ViewportShading::Unlit => {
                        ViewportShadingV1::Unlit
                    }
                    crate::core::viewport_shading::ViewportShading::Shade => {
                        ViewportShadingV1::Shade
                    }
                },
                gizmo_visible: editor_state.view.gizmo_visible,
                background_color: editor_state.view.background_color,
                wireframe: WireframeViewStateV1 {
                    visible: editor_state.view.viewport_wireframe_visible,
                    color: editor_state.view.viewport_wireframe.color,
                    opacity: editor_state.view.viewport_wireframe.opacity,
                },
                scene_visibility: SceneVisibilityStateV1 {
                    hidden_mesh_object_ids,
                    hidden_material_ids,
                },
                surface_mirror: SurfaceMirrorViewStateV1 {
                    x_enabled: editor_state.view.surface_mirror_x_enabled,
                    x_plane: editor_state.view.surface_mirror_x_plane,
                    x_plane_visible: editor_state.view.surface_mirror_x_plane_visible,
                },
            },
            uv: UvEditorStateV1 {
                transform: UvViewTransformV1 {
                    center_uv: editor_state.view.uv_view_transform.center_uv.to_array(),
                    zoom: editor_state.view.uv_view_transform.zoom,
                    rotation_radians: editor_state.view.uv_view_transform.rotation_radians,
                },
                background_color: editor_state.view.uv_background_color,
                wireframe: WireframeViewStateV1 {
                    visible: editor_state.view.uv_wireframe_visible,
                    color: editor_state.view.uv_wireframe.color,
                    opacity: editor_state.view.uv_wireframe.opacity,
                },
            },
        },
        layer_tree: LayerTreeEditorStateV1 {
            collapsed_layer_group_ids,
        },
    })
}

fn encode_geometry(document: &Document) -> Result<(MeshV1, Vec<u8>), SaveProjectError> {
    let mesh = &document.mesh;
    let vertex_count = u32::try_from(mesh.positions.len())
        .map_err(|_| SaveProjectError::new("mesh vertex count exceeds u32"))?;
    let triangle_count = u64::try_from(mesh.indices.len())
        .map_err(|_| SaveProjectError::new("mesh triangle count exceeds u64"))?;
    if mesh.normals.len() != mesh.positions.len() || mesh.uvs.len() != mesh.positions.len() {
        return Err(SaveProjectError::new(
            "mesh position, normal, and UV counts differ",
        ));
    }

    let mut geometry = Vec::new();
    let position_offset = geometry.len() as u64;
    for (index, position) in mesh.positions.iter().enumerate() {
        if !position.is_finite() {
            return Err(SaveProjectError::new(format!(
                "mesh position {index} is non-finite"
            )));
        }
        write_f32_components(&mut geometry, position.to_array());
    }
    let normal_offset = geometry.len() as u64;
    for (index, normal) in mesh.normals.iter().enumerate() {
        if !normal.is_finite() {
            return Err(SaveProjectError::new(format!(
                "mesh normal {index} is non-finite"
            )));
        }
        write_f32_components(&mut geometry, normal.to_array());
    }
    let texcoord_offset = geometry.len() as u64;
    for (index, texcoord) in mesh.uvs.iter().enumerate() {
        if !texcoord.is_finite() {
            return Err(SaveProjectError::new(format!(
                "mesh UV {index} is non-finite"
            )));
        }
        write_f32_components(&mut geometry, texcoord.to_array());
    }
    let index_offset = geometry.len() as u64;
    for (triangle_index, triangle) in mesh.indices.iter().enumerate() {
        for index in triangle {
            if *index >= vertex_count {
                return Err(SaveProjectError::new(format!(
                    "mesh triangle {triangle_index} references vertex {index} outside the vertex array"
                )));
            }
            geometry.extend_from_slice(&index.to_le_bytes());
        }
    }

    let objects = mesh
        .mesh_objects
        .iter()
        .map(|object| {
            Ok(MeshObjectV1 {
                id: document
                    .mesh_object_persistent_id(object.id)
                    .ok_or_else(|| SaveProjectError::new("mesh object persistent ID is missing"))?
                    .0,
                name: object.name.clone(),
            })
        })
        .collect::<Result<Vec<_>, SaveProjectError>>()?;
    let primitives = mesh
        .sub_meshes
        .iter()
        .map(|primitive| {
            let material = document
                .materials
                .get(primitive.material_index)
                .ok_or_else(|| SaveProjectError::new("primitive material is missing"))?;
            Ok(PrimitiveV1 {
                object_id: document
                    .mesh_object_persistent_id(primitive.mesh_id)
                    .ok_or_else(|| SaveProjectError::new("primitive mesh object is missing"))?
                    .0,
                material_id: material.id.0,
                first_index: u64::from(primitive.start_index),
                index_count: u64::from(primitive.index_count),
            })
        })
        .collect::<Result<Vec<_>, SaveProjectError>>()?;

    Ok((
        MeshV1 {
            vertex_count,
            triangle_count,
            position: VertexArrayV1 {
                offset: position_offset,
                count: vertex_count,
            },
            normal: VertexArrayV1 {
                offset: normal_offset,
                count: vertex_count,
            },
            texcoord0: TexcoordArrayV1 {
                offset: texcoord_offset,
                count: vertex_count,
            },
            indices: IndexArrayV1 {
                offset: index_offset,
                count: triangle_count
                    .checked_mul(3)
                    .ok_or_else(|| SaveProjectError::new("index count overflows u64"))?,
            },
            objects,
            primitives,
        },
        geometry,
    ))
}

fn write_f32_components<const N: usize>(output: &mut Vec<u8>, values: [f32; N]) {
    for value in values {
        output.extend_from_slice(&value.to_le_bytes());
    }
}

fn convert_materials(document: &Document) -> Vec<MaterialV1> {
    document
        .materials
        .iter()
        .map(|material| MaterialV1 {
            id: material.id.0,
            name: material.name.clone(),
            texture_size: TextureSizeV1 {
                width: material.texture_size[0],
                height: material.texture_size[1],
            },
            render_settings: MaterialRenderSettingsV1 {
                double_sided: material.render_settings.double_sided,
                render_mode: match material.render_settings.render_mode {
                    MaterialRenderMode::Opaque => MaterialRenderModeV1::Opaque,
                    MaterialRenderMode::Cutoff => MaterialRenderModeV1::Cutoff,
                    MaterialRenderMode::Blend => MaterialRenderModeV1::Blend,
                },
                alpha_cutoff: material.render_settings.alpha_cutoff,
            },
            export_image_file_name: material.export_image_file_name.clone(),
            export_psd_file_name: material.export_psd_file_name.clone(),
        })
        .collect()
}

fn convert_layers(document: &Document) -> Result<Vec<LayerV1>, SaveProjectError> {
    let tree = &document.layer_tree;
    let layer_ids = std::iter::once(tree.root())
        .chain(tree.rows().into_iter().map(|row| row.layer_id))
        .collect::<Vec<_>>();
    let persistent_ids = layer_ids
        .iter()
        .copied()
        .map(|layer_id| {
            tree.persistent_id(layer_id)
                .map(|persistent| (layer_id, persistent.0))
                .ok_or_else(|| SaveProjectError::new("layer persistent ID is missing"))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    layer_ids
        .into_iter()
        .map(|layer_id| {
            let node = tree
                .get(layer_id)
                .ok_or_else(|| SaveProjectError::new("layer node is missing"))?;
            let parent_id = node
                .parent
                .map(|parent| {
                    persistent_ids.get(&parent).copied().ok_or_else(|| {
                        SaveProjectError::new("layer parent persistent ID is missing")
                    })
                })
                .transpose()?;
            let order = if layer_id == tree.root() {
                0
            } else {
                u32::try_from(
                    tree.parent_and_index(layer_id)
                        .ok_or_else(|| SaveProjectError::new("layer sibling order is missing"))?
                        .1,
                )
                .map_err(|_| SaveProjectError::new("layer sibling order exceeds u32"))?
            };
            let material_mask = match &node.material_mask {
                LayerMaterialMask::Unspecified => None,
                LayerMaterialMask::Specified(ids) => {
                    Some(ids.iter().map(|material_id| material_id.0).collect())
                }
            };
            let mask = node.mask.map(|mask| LayerMaskStateV1 {
                enabled: mask.enabled,
            });
            let content = match &node.content {
                LayerContent::Raster => LayerContentV1::Raster,
                LayerContent::EmbeddedImage {
                    image_id,
                    transform,
                } => LayerContentV1::EmbeddedImage {
                    image_id: image_id.0,
                    transform: EmbeddedImageTransformV1 {
                        center_uv: transform.center_uv.to_array(),
                        size_uv: transform.size_uv.to_array(),
                        rotation_radians: transform.rotation_radians,
                    },
                },
                LayerContent::SolidFill { color } => LayerContentV1::SolidFill { color: *color },
                LayerContent::Adjustment { adjustment } => LayerContentV1::Adjustment {
                    adjustment: convert_adjustment(adjustment.clone()),
                },
                LayerContent::Group { composite_mode, .. } => LayerContentV1::Group {
                    composite_mode: match composite_mode {
                        GroupCompositeMode::Isolated => GroupCompositeModeV1::Isolated,
                        GroupCompositeMode::PassThrough => GroupCompositeModeV1::PassThrough,
                    },
                },
            };
            Ok(LayerV1 {
                id: persistent_ids[&layer_id],
                parent_id,
                order,
                name: node.props.name.clone(),
                visible: node.props.visible,
                locked: node.props.locked,
                opacity: node.effective_opacity(),
                blend_mode: convert_blend_mode(node.props.blend_mode),
                material_mask,
                mask,
                content,
            })
        })
        .collect()
}

fn write_embedded_images(
    document: &Document,
    archive: &mut ZipWriter<&mut File>,
    options: SimpleFileOptions,
) -> Result<Vec<EmbeddedImageV1>, SaveProjectError> {
    let referenced = document
        .layer_tree
        .rows()
        .into_iter()
        .filter_map(|row| {
            document
                .layer_tree
                .embedded_image(row.layer_id)
                .map(|(id, _)| id)
        })
        .collect::<HashSet<_>>();
    let mut assets = referenced
        .into_iter()
        .map(|id| {
            document
                .embedded_image(id)
                .cloned()
                .ok_or_else(|| SaveProjectError::new(format!("embedded image {} is missing", id.0)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    assets.sort_by_key(|asset| asset.id);
    let mut metadata = Vec::with_capacity(assets.len());
    for asset in assets {
        archive
            .start_file(embedded_image_entry_name(asset.id.0), options)
            .map_err(|error| {
                SaveProjectError::new(format!("starting embedded image {}: {error}", asset.id.0))
            })?;
        archive.write_all(&asset.rgba8).map_err(|error| {
            SaveProjectError::new(format!("writing embedded image {}: {error}", asset.id.0))
        })?;
        metadata.push(EmbeddedImageV1 {
            id: asset.id.0,
            file_name: asset.file_name.clone(),
            width: asset.size[0],
            height: asset.size[1],
            pixel_format: EmbeddedImagePixelFormatV1::Rgba8Straight,
        });
    }
    Ok(metadata)
}

pub fn embedded_image_entry_name(image_id: u64) -> String {
    format!("images/{image_id:016x}.rgba8")
}

fn convert_blend_mode(mode: LayerBlendMode) -> BlendModeV1 {
    match mode {
        LayerBlendMode::Normal => BlendModeV1::Normal,
        LayerBlendMode::Darken => BlendModeV1::Darken,
        LayerBlendMode::Multiply => BlendModeV1::Multiply,
        LayerBlendMode::Lighten => BlendModeV1::Lighten,
        LayerBlendMode::Screen => BlendModeV1::Screen,
        LayerBlendMode::ColorDodge => BlendModeV1::ColorDodge,
        LayerBlendMode::LinearDodge => BlendModeV1::LinearDodge,
        LayerBlendMode::Overlay => BlendModeV1::Overlay,
        LayerBlendMode::SoftLight => BlendModeV1::SoftLight,
        LayerBlendMode::HardLight => BlendModeV1::HardLight,
        LayerBlendMode::Color => BlendModeV1::Color,
    }
}

fn convert_adjustment(adjustment: Adjustment) -> AdjustmentV1 {
    match adjustment {
        Adjustment::BrightnessContrast(value) => AdjustmentV1::BrightnessContrast {
            brightness: value.brightness,
            contrast: value.contrast,
        },
        Adjustment::Levels(value) => AdjustmentV1::Levels {
            master: convert_levels_channel(value.master),
            red: convert_levels_channel(value.red),
            green: convert_levels_channel(value.green),
            blue: convert_levels_channel(value.blue),
        },
        Adjustment::Curves(value) => AdjustmentV1::Curves {
            master: value
                .master
                .points()
                .iter()
                .map(convert_curve_point)
                .collect(),
            red: value.red.points().iter().map(convert_curve_point).collect(),
            green: value
                .green
                .points()
                .iter()
                .map(convert_curve_point)
                .collect(),
            blue: value
                .blue
                .points()
                .iter()
                .map(convert_curve_point)
                .collect(),
        },
        Adjustment::HueSaturation(value) => AdjustmentV1::HueSaturation {
            hue: value.hue,
            saturation: value.saturation,
            lightness: value.lightness,
        },
        Adjustment::Invert => AdjustmentV1::Invert,
        Adjustment::GradientMap(value) => AdjustmentV1::GradientMap {
            reverse: value.reverse,
            dither: value.dither,
            stops: value
                .stops()
                .iter()
                .map(|stop| GradientStopV1 {
                    location: stop.location,
                    midpoint: stop.midpoint,
                    color: stop.color,
                })
                .collect(),
        },
        Adjustment::UvMirror(value) => AdjustmentV1::UvMirror {
            axis: match value.axis {
                UvMirrorAxis::X => UvAxisV1::X,
                UvMirrorAxis::Y => UvAxisV1::Y,
            },
            position: value.position,
            direction: match value.direction {
                UvMirrorDirection::PositiveToNegative => UvMirrorDirectionV1::PositiveToNegative,
                UvMirrorDirection::NegativeToPositive => UvMirrorDirectionV1::NegativeToPositive,
            },
        },
    }
}

fn convert_levels_channel(value: crate::core::adjustment::LevelsChannel) -> LevelsChannelV1 {
    LevelsChannelV1 {
        input_black: value.input_black,
        input_white: value.input_white,
        gamma: value.gamma,
        output_black: value.output_black,
        output_white: value.output_white,
    }
}

fn convert_curve_point(value: &crate::core::adjustment::CurvePoint) -> CurvePointV1 {
    CurvePointV1 {
        input: value.input,
        output: value.output,
    }
}

fn write_surfaces(
    document: &Document,
    archive: &mut ZipWriter<&mut File>,
    options: SimpleFileOptions,
) -> Result<Vec<SurfaceV1>, SaveProjectError> {
    let mut specifications = Vec::new();
    for row in document.layer_tree.rows() {
        let node = document
            .layer_tree
            .get(row.layer_id)
            .ok_or_else(|| SaveProjectError::new("surface layer node is missing"))?;
        let persistent_layer_id = document
            .layer_tree
            .persistent_id(row.layer_id)
            .ok_or_else(|| SaveProjectError::new("surface layer persistent ID is missing"))?
            .0;
        for (material_index, material) in document.materials.iter().enumerate() {
            if matches!(node.content, LayerContent::Raster) {
                specifications.push((
                    persistent_layer_id,
                    material.id.0,
                    SurfaceRoleV1::Raster,
                    PaintSurfaceId::raster(MaterialIndex(material_index), row.layer_id),
                ));
            }
            if row.layer_id != document.layer_tree.root() && node.mask.is_some() {
                specifications.push((
                    persistent_layer_id,
                    material.id.0,
                    SurfaceRoleV1::LayerMask,
                    PaintSurfaceId::layer_mask(MaterialIndex(material_index), row.layer_id),
                ));
            }
        }
    }
    specifications.sort_by_key(|(layer, material, role, _)| (*layer, *material, *role));

    specifications
        .into_iter()
        .map(|(layer_id, material_id, role, runtime_surface)| {
            let size = document
                .tiles
                .texture_size(runtime_surface)
                .ok_or_else(|| {
                    SaveProjectError::new(format!("surface {runtime_surface:?} is missing"))
                })?;
            let default_rgba8 = document
                .tiles
                .surface_default_rgba8(runtime_surface)
                .ok_or_else(|| SaveProjectError::new("surface default fill is missing"))?;
            match role {
                SurfaceRoleV1::Raster if default_rgba8 != [0; 4] => {
                    return Err(SaveProjectError::new(format!(
                        "surface {runtime_surface:?} has unsupported raster default {default_rgba8:?}"
                    )));
                }
                SurfaceRoleV1::LayerMask
                    if default_rgba8 != [0; 4] && default_rgba8 != [255; 4] =>
                {
                    return Err(SaveProjectError::new(format!(
                        "surface {runtime_surface:?} has unsupported layer mask default {default_rgba8:?}"
                    )));
                }
                _ => {}
            }
            let mut tiles = document
                .tiles
                .surface_tiles(runtime_surface)
                .ok_or_else(|| SaveProjectError::new("surface tile store is missing"))?
                .collect::<Vec<_>>();
            tiles.sort_by_key(|tile| (tile.coord.y, tile.coord.x));

            let entry_name = surface_entry_name(layer_id, material_id, role);
            archive.start_file(&entry_name, options).map_err(|error| {
                SaveProjectError::new(format!("starting {entry_name}: {error}"))
            })?;
            let mut persistent_tiles = Vec::new();
            for tile in tiles {
                let expected_rgba_len = usize::try_from(tile.rect.size[0])
                    .ok()
                    .and_then(|width| {
                        usize::try_from(tile.rect.size[1])
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .and_then(|pixels| pixels.checked_mul(4))
                    .ok_or_else(|| {
                        SaveProjectError::new("tile RGBA payload size overflows usize")
                    })?;
                if tile.rgba8.len() != expected_rgba_len {
                    return Err(SaveProjectError::new(format!(
                        "{entry_name} tile ({}, {}) has an unexpected RGBA payload size",
                        tile.coord.x, tile.coord.y
                    )));
                }
                let raw = match role {
                    SurfaceRoleV1::Raster => {
                        if tile.rgba8.iter().all(|channel| *channel == 0) {
                            continue;
                        }
                        tile.rgba8.to_vec()
                    }
                    SurfaceRoleV1::LayerMask => {
                        let coverage = tile
                            .rgba8
                            .chunks_exact(4)
                            .map(|pixel| pixel[0])
                            .collect::<Vec<_>>();
                        if coverage.iter().all(|sample| *sample == default_rgba8[0]) {
                            continue;
                        }
                        coverage
                    }
                };
                let compressed = zstd::stream::encode_all(raw.as_slice(), 3).map_err(|error| {
                    SaveProjectError::new(format!(
                        "compressing {entry_name} tile ({}, {}): {error}",
                        tile.coord.x, tile.coord.y
                    ))
                })?;
                let compressed_size = u32::try_from(compressed.len()).map_err(|_| {
                    SaveProjectError::new(format!(
                        "{entry_name} tile ({}, {}) compressed size exceeds u32",
                        tile.coord.x, tile.coord.y
                    ))
                })?;
                archive.write_all(&compressed).map_err(|error| {
                    SaveProjectError::new(format!(
                        "writing {entry_name} tile ({}, {}): {error}",
                        tile.coord.x, tile.coord.y
                    ))
                })?;
                persistent_tiles.push(TileV1 {
                    x: tile.coord.x,
                    y: tile.coord.y,
                    compressed_size,
                });
            }

            Ok(SurfaceV1 {
                layer_id,
                material_id,
                role,
                width: size[0],
                height: size[1],
                pixel_format: match role {
                    SurfaceRoleV1::Raster => PixelFormatV1::Rgba8Unorm,
                    SurfaceRoleV1::LayerMask => PixelFormatV1::R8Unorm,
                },
                tile_size: document.tiles.tile_size(),
                default_fill: match role {
                    SurfaceRoleV1::Raster => SurfaceDefaultV1::TransparentBlack,
                    SurfaceRoleV1::LayerMask if default_rgba8[0] == 255 => {
                        SurfaceDefaultV1::MaskRevealed
                    }
                    SurfaceRoleV1::LayerMask => SurfaceDefaultV1::MaskHidden,
                },
                tiles: persistent_tiles,
            })
        })
        .collect()
}

pub fn surface_entry_name(layer_id: u64, material_id: u64, role: SurfaceRoleV1) -> String {
    let role = match role {
        SurfaceRoleV1::Raster => "raster",
        SurfaceRoleV1::LayerMask => "layer_mask",
    };
    format!("surfaces/{layer_id:016x}_{material_id:016x}_{role}.bin")
}

#[cfg(test)]
mod tests {
    use std::{io::Read, sync::Arc};

    use glam::{Vec2, Vec3};

    use crate::core::{
        document::{MeshData, MeshId, MeshObject, SubMesh},
        document_tile_store::InitialPixels,
        geometry::RectU32,
        material::MaterialSpec,
        surface::LayerId,
        tile_payload::PixelSnapshotData,
    };

    use super::*;

    fn document() -> Document {
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
                material_name: "Material".to_owned(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".to_owned(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        Document::new(mesh, vec![MaterialSpec::new("Material", [2, 2])])
    }

    fn editor_state(document: &Document, active_layer_id: LayerId) -> ProjectEditorState {
        ProjectEditorState::for_document(document, active_layer_id)
    }

    #[test]
    fn writes_v1_archive_with_stored_independent_surface_frames() {
        let mut document = document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let raster = PaintSurfaceId::raster(0.into(), layer);
        document
            .tiles
            .write_surface_rect(
                raster,
                RectU32::full([2, 2]),
                &PixelSnapshotData::contiguous(vec![1, 2, 3, 4].repeat(4)),
            )
            .unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        let mask = PaintSurfaceId::layer_mask(0.into(), layer);
        document
            .tiles
            .create_surface(
                mask,
                [2, 2],
                InitialPixels::Rgba8(vec![64, 9, 8, 255].repeat(4)),
            )
            .unwrap();

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.holopaint");
        std::fs::write(&path, b"old project").unwrap();
        save_project(&document, &editor_state(&document, layer), &path).unwrap();

        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(
            archive.by_name("geometry.bin").unwrap().compression(),
            CompressionMethod::Deflated
        );
        let project: ProjectV1 = {
            let mut entry = archive.by_name("project.ron").unwrap();
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            let mut source = String::new();
            entry.read_to_string(&mut source).unwrap();
            assert!(!source.contains("#![enable"));
            ron::from_str(&source).unwrap()
        };
        assert_eq!(project.format_version, 1);
        assert_eq!(
            project.created_with,
            format!("HoloPainter {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(project.surfaces.len(), 2);
        validate_project_v1(&project, archive.by_name("geometry.bin").unwrap().size()).unwrap();
        let mut invalid_created_with = project.clone();
        invalid_created_with.created_with = " \t\n".to_owned();
        assert!(
            validate_project_v1(
                &invalid_created_with,
                archive.by_name("geometry.bin").unwrap().size(),
            )
            .is_err()
        );

        for surface in &project.surfaces {
            let name = surface_entry_name(surface.layer_id, surface.material_id, surface.role);
            let mut entry = archive.by_name(&name).unwrap();
            assert_eq!(entry.compression(), CompressionMethod::Stored);
            let mut blob = Vec::new();
            entry.read_to_end(&mut blob).unwrap();
            assert_eq!(
                blob.len(),
                surface
                    .tiles
                    .iter()
                    .map(|tile| tile.compressed_size as usize)
                    .sum::<usize>()
            );
            assert_eq!(surface.tiles.len(), 1);
            let decoded = zstd::stream::decode_all(blob.as_slice()).unwrap();
            match surface.role {
                SurfaceRoleV1::Raster => assert_eq!(decoded, vec![1, 2, 3, 4].repeat(4)),
                SurfaceRoleV1::LayerMask => assert_eq!(decoded, vec![64; 4]),
            }
        }
    }

    #[test]
    fn embedded_image_round_trip_writes_shared_pixels_once() {
        let mut document = document();
        let base = document.layer_tree.default_raster_layer().unwrap();
        let added = document
            .add_embedded_image_layer(
                crate::core::document::LayerInsertion::Above(base),
                "Photo",
                "photo.png".to_owned(),
                [1, 1],
                Arc::from([200, 100, 50, 128]),
                0usize.into(),
            )
            .unwrap()
            .unwrap();
        let duplicate = document
            .layer_tree
            .duplicate_layer(added.layer_id)
            .unwrap()
            .root;
        let (original_id, original_transform) =
            document.layer_tree.embedded_image(added.layer_id).unwrap();
        let (duplicate_id, _) = document.layer_tree.embedded_image(duplicate).unwrap();
        assert_eq!(original_id, duplicate_id);
        let mut moved = original_transform;
        moved.center_uv.x += 0.25;
        assert!(
            document
                .layer_tree
                .set_embedded_image_transform(duplicate, moved)
        );
        assert_eq!(
            document
                .layer_tree
                .embedded_image(added.layer_id)
                .unwrap()
                .1,
            original_transform
        );

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("embedded.holopaint");
        save_project(&document, &editor_state(&document, added.layer_id), &path).unwrap();
        let loaded = crate::project::open_project(&path).unwrap();
        assert_eq!(loaded.document.embedded_images().count(), 1);
        let layers = loaded
            .document
            .layer_tree
            .rows()
            .into_iter()
            .filter_map(|row| loaded.document.layer_tree.embedded_image(row.layer_id))
            .collect::<Vec<_>>();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].0, layers[1].0);
        assert_eq!(
            loaded
                .document
                .embedded_image(layers[0].0)
                .unwrap()
                .rgba8
                .as_ref(),
            &[200, 100, 50, 128]
        );

        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let image_entries = (0..archive.len())
            .filter(|index| {
                archive
                    .by_index(*index)
                    .unwrap()
                    .name()
                    .starts_with("images/")
            })
            .count();
        assert_eq!(image_entries, 1);
    }

    #[test]
    fn failed_save_preserves_existing_destination() {
        let mut document = document();
        Arc::make_mut(&mut document.mesh.positions)[0].x = f32::NAN;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.holopaint");
        std::fs::write(&path, b"existing project").unwrap();

        let active_layer = document.layer_tree.default_raster_layer().unwrap();
        assert!(save_project(&document, &editor_state(&document, active_layer), &path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"existing project");
    }

    #[test]
    fn invalid_active_layer_is_a_save_error() {
        let document = document();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-active-layer.holopaint");

        let error = save_project(
            &document,
            &editor_state(&document, LayerId::default()),
            &path,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("active layer persistent ID is missing"));
        assert!(!path.exists());
    }

    #[test]
    fn rejects_unsupported_raster_default() {
        let mut document = document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let raster = PaintSurfaceId::raster(0.into(), layer);
        document.tiles.delete_surface(raster).unwrap();
        document
            .tiles
            .create_surface(
                raster,
                [2, 2],
                InitialPixels::SparseDefaultRgba8([123, 17, 90, 191]),
            )
            .unwrap();

        let directory = tempfile::tempdir().unwrap();
        let error = save_project(
            &document,
            &editor_state(&document, layer),
            &directory.path().join("invalid.holopaint"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("unsupported raster default"));
    }

    #[test]
    fn rejects_unsupported_layer_mask_default() {
        let mut document = document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        document
            .tiles
            .create_surface(
                PaintSurfaceId::layer_mask(0.into(), layer),
                [2, 2],
                InitialPixels::SparseDefaultRgba8([73; 4]),
            )
            .unwrap();

        let directory = tempfile::tempdir().unwrap();
        let error = save_project(
            &document,
            &editor_state(&document, layer),
            &directory.path().join("invalid.holopaint"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("unsupported layer mask default"));
    }
}
