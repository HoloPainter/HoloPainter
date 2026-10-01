use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::Read,
    path::Path,
};

use glam::{Vec2, Vec3};
use uuid::Uuid;
use zip::ZipArchive;

use crate::core::{
    adjustment::{
        Adjustment, BrightnessContrastAdjustment, CurveChannel, CurvePoint, CurvesAdjustment,
        GradientMapAdjustment, GradientStop, HueSaturationAdjustment, LevelsAdjustment,
        LevelsChannel, MAX_CURVE_POINTS, UvMirrorAdjustment, UvMirrorAxis, UvMirrorDirection,
    },
    composite::{GroupCompositeMode, LayerBlendMode},
    document::{
        ActiveLayerPart, Document, MeshData, MeshId, MeshObject, MeshObjectId, ProjectId, SubMesh,
    },
    document_tile_store::{DocumentTileStore, InitialPixels},
    embedded_image::{EmbeddedImageAsset, EmbeddedImageId, EmbeddedImageTransform, pixel_byte_len},
    geometry::RectU32,
    material::{
        MaterialId, MaterialIndex, MaterialRenderMode, MaterialRenderSettings, MaterialSpec,
    },
    surface::{
        LayerContent, LayerMaskProperties, LayerMaterialMask, LayerProperties, LayerTree,
        PaintSurfaceId, PersistentLayerId, RestoredLayerNode,
    },
    tile::{TileCoord, TileGrid},
    tile_payload::{PixelSnapshotData, TilePayload},
    viewport_visibility::ViewportSceneVisibility,
};

use super::{
    DocumentFocusState, LayerTreeEditorState, ProjectEditorState, decode_editor_view_state,
    v1::*,
    validate_project_v1,
    writer::{embedded_image_entry_name, surface_entry_name},
};

#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub document: Document,
    pub editor_state: ProjectEditorState,
    pub surfaces: Vec<LoadedSurface>,
}

#[derive(Debug, Clone)]
pub struct LoadedSurface {
    pub surface: PaintSurfaceId,
    pub default_rgba8: [u8; 4],
    pub surface_revision: u64,
    pub tiles: Vec<TilePayload>,
}

#[derive(Debug)]
pub struct OpenProjectError(String);

impl OpenProjectError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for OpenProjectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for OpenProjectError {}

type OpenResult<T> = Result<T, OpenProjectError>;

pub fn open_project(path: &Path) -> OpenResult<LoadedProject> {
    let file = File::open(path)
        .map_err(|error| OpenProjectError::new(format!("opening project archive: {error}")))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| OpenProjectError::new(format!("reading project archive: {error}")))?;
    let project_source = read_archive_entry(&mut archive, "project.ron")?;
    let project: ProjectV1 = ron::de::from_bytes(&project_source)
        .map_err(|error| OpenProjectError::new(format!("decoding project.ron: {error}")))?;
    if project.format_version != 1 {
        return Err(OpenProjectError::new(format!(
            "unsupported project format version {}",
            project.format_version
        )));
    }
    let geometry = read_archive_entry(&mut archive, "geometry.bin")?;
    validate_project_v1(&project, geometry.len() as u64)
        .map_err(|error| OpenProjectError::new(format!("validating project: {error}")))?;
    let (mesh, mesh_ids) = decode_mesh(&project, &geometry)?;
    let material_indices: HashMap<u64, MaterialIndex> = project
        .materials
        .iter()
        .enumerate()
        .map(|(index, material)| (material.id, index.into()))
        .collect::<HashMap<_, _>>();
    let materials = decode_materials(&project);
    let embedded_images = decode_embedded_images(&mut archive, &project)?;
    let restored_layers = project
        .layers
        .iter()
        .map(decode_layer)
        .collect::<OpenResult<Vec<_>>>()?;
    let (layer_tree, layer_ids) = LayerTree::restore(restored_layers, project.id_counters.layer)
        .map_err(|error| OpenProjectError::new(format!("constructing layer tree: {error:#}")))?;
    let active_layer_id = layer_ids
        .get(&PersistentLayerId(
            project.editor_state.document_focus.active_layer_id,
        ))
        .copied()
        .unwrap_or_else(|| {
            layer_tree
                .default_raster_layer()
                .unwrap_or_else(|| layer_tree.root())
        });
    let active_layer_part = match project.editor_state.document_focus.active_layer_part {
        ActiveLayerPartV1::LayerMask if layer_tree.has_layer_mask(active_layer_id) => {
            ActiveLayerPart::LayerMask
        }
        ActiveLayerPartV1::Content | ActiveLayerPartV1::LayerMask => ActiveLayerPart::Content,
    };
    let focused_material_index = material_indices
        .get(&project.editor_state.document_focus.focused_material_id)
        .copied()
        .unwrap_or(0.into())
        .as_usize();
    let persistent_mesh_ids = mesh_ids
        .iter()
        .map(|(runtime, persistent)| (persistent.0, *runtime))
        .collect::<HashMap<_, _>>();
    let hidden_meshes = project
        .editor_state
        .view
        .viewport_3d
        .scene_visibility
        .hidden_mesh_object_ids
        .iter()
        .filter_map(|id| persistent_mesh_ids.get(id).copied());
    let hidden_materials = project
        .editor_state
        .view
        .viewport_3d
        .scene_visibility
        .hidden_material_ids
        .iter()
        .filter_map(|id| material_indices.get(id).copied());
    let collapsed_layer_group_ids = project
        .editor_state
        .layer_tree
        .collapsed_layer_group_ids
        .iter()
        .filter_map(|id| layer_ids.get(&PersistentLayerId(*id)).copied())
        .filter(|layer_id| layer_tree.is_group(*layer_id))
        .collect::<Vec<_>>();
    let mut view = decode_editor_view_state(&project.editor_state.view);
    view.scene_visibility = ViewportSceneVisibility::from_hidden(hidden_meshes, hidden_materials);
    let editor_state = ProjectEditorState {
        document_focus: DocumentFocusState {
            active_layer_id,
            active_layer_part,
            focused_material_index,
        },
        view,
        layer_tree: LayerTreeEditorState {
            collapsed_layer_group_ids,
        },
    };

    let tile_size = project
        .surfaces
        .first()
        .map_or(256, |surface| surface.tile_size);
    let mut tiles = DocumentTileStore::new(tile_size)
        .map_err(|error| OpenProjectError::new(format!("constructing surface store: {error:#}")))?;
    for surface in &project.surfaces {
        let runtime_surface = runtime_surface(surface, &layer_ids, &material_indices)?;
        let default = match surface.default_fill {
            SurfaceDefaultV1::TransparentBlack | SurfaceDefaultV1::MaskHidden => [0, 0, 0, 0],
            SurfaceDefaultV1::MaskRevealed => [255, 255, 255, 255],
        };
        tiles
            .create_surface(
                runtime_surface,
                [surface.width, surface.height],
                InitialPixels::SparseDefaultRgba8(default),
            )
            .map_err(|error| OpenProjectError::new(format!("creating surface: {error:#}")))?;
        decode_surface(&mut archive, surface, runtime_surface, &mut tiles)?;
    }
    let surfaces = project
        .surfaces
        .iter()
        .map(|surface| {
            let runtime = runtime_surface(surface, &layer_ids, &material_indices)?;
            let default_rgba8 = tiles
                .surface_default_rgba8(runtime)
                .ok_or_else(|| OpenProjectError::new("loaded surface default is missing"))?;
            let surface_revision = tiles
                .surface_revision(runtime)
                .ok_or_else(|| OpenProjectError::new("loaded surface revision is missing"))?;
            let loaded_tiles = tiles
                .surface_tiles(runtime)
                .ok_or_else(|| OpenProjectError::new("loaded surface tiles are missing"))?
                .map(|tile| TilePayload {
                    coord: tile.coord,
                    rect: tile.rect,
                    rgba8: tile.rgba8.clone(),
                })
                .collect();
            Ok(LoadedSurface {
                surface: runtime,
                default_rgba8,
                surface_revision,
                tiles: loaded_tiles,
            })
        })
        .collect::<OpenResult<Vec<_>>>()?;
    let project_id = Uuid::parse_str(&project.project_id)
        .map_err(|error| OpenProjectError::new(format!("decoding project ID: {error}")))?;
    let document = Document::restore(
        ProjectId(project_id),
        mesh,
        mesh_ids,
        project.id_counters.mesh_object,
        materials,
        project.id_counters.material,
        embedded_images,
        project.id_counters.embedded_image,
        layer_tree,
        tiles,
    );
    Ok(LoadedProject {
        document,
        editor_state,
        surfaces,
    })
}

fn decode_embedded_images(
    archive: &mut ZipArchive<File>,
    project: &ProjectV1,
) -> OpenResult<HashMap<EmbeddedImageId, std::sync::Arc<EmbeddedImageAsset>>> {
    project
        .embedded_images
        .iter()
        .map(|image| {
            let id = EmbeddedImageId(image.id);
            let rgba8 = read_archive_entry(archive, &embedded_image_entry_name(image.id))?;
            let expected = pixel_byte_len([image.width, image.height]).ok_or_else(|| {
                OpenProjectError::new(format!(
                    "embedded image {} dimensions are invalid",
                    image.id
                ))
            })?;
            if rgba8.len() != expected {
                return Err(OpenProjectError::new(format!(
                    "embedded image {} payload size mismatch",
                    image.id
                )));
            }
            let asset = EmbeddedImageAsset::new(
                id,
                image.file_name.clone(),
                [image.width, image.height],
                rgba8.into(),
            )
            .ok_or_else(|| {
                OpenProjectError::new(format!("embedded image {} is invalid", image.id))
            })?;
            Ok((id, std::sync::Arc::new(asset)))
        })
        .collect()
}

fn read_archive_entry(archive: &mut ZipArchive<File>, name: &str) -> OpenResult<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .map_err(|error| OpenProjectError::new(format!("opening {name}: {error}")))?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| OpenProjectError::new(format!("reading {name}: {error}")))?;
    Ok(bytes)
}

fn decode_mesh(
    project: &ProjectV1,
    geometry: &[u8],
) -> OpenResult<(MeshData, HashMap<MeshId, MeshObjectId>)> {
    let mesh = &project.mesh;
    let positions = decode_vec3(
        geometry,
        mesh.position.offset,
        mesh.position.count,
        "positions",
    )?;
    let normals = decode_vec3(geometry, mesh.normal.offset, mesh.normal.count, "normals")?;
    let uvs = decode_vec2(geometry, mesh.texcoord0.offset, mesh.texcoord0.count, "UVs")?;
    let flat_indices = decode_u32(geometry, mesh.indices.offset, mesh.indices.count, "indices")?;
    let indices = flat_indices
        .chunks_exact(3)
        .map(|v| [v[0], v[1], v[2]])
        .collect::<Vec<_>>();
    if flat_indices.len() % 3 != 0 {
        return Err(OpenProjectError::new("index array is not triangle-aligned"));
    }
    let object_runtime = mesh
        .objects
        .iter()
        .enumerate()
        .map(|(index, object)| (object.id, MeshId(index)))
        .collect::<HashMap<_, _>>();
    let mesh_objects = mesh
        .objects
        .iter()
        .enumerate()
        .map(|(index, object)| MeshObject {
            id: MeshId(index),
            name: object.name.clone(),
        })
        .collect::<Vec<_>>();
    let persistent_ids = mesh
        .objects
        .iter()
        .enumerate()
        .map(|(index, object)| (MeshId(index), MeshObjectId(object.id)))
        .collect::<HashMap<_, _>>();
    let material_indices = project
        .materials
        .iter()
        .enumerate()
        .map(|(index, material)| (material.id, index))
        .collect::<HashMap<_, _>>();
    let fallback_mesh = mesh_objects
        .first()
        .map(|object| object.id)
        .unwrap_or(MeshId(0));
    let mut triangle_mesh_ids = vec![fallback_mesh; indices.len()];
    let mut sub_meshes = Vec::with_capacity(mesh.primitives.len());
    for primitive in &mesh.primitives {
        let mesh_id = *object_runtime
            .get(&primitive.object_id)
            .ok_or_else(|| OpenProjectError::new("primitive references an unknown mesh object"))?;
        let material_index = *material_indices
            .get(&primitive.material_id)
            .ok_or_else(|| OpenProjectError::new("primitive references an unknown material"))?;
        let start_index = u32::try_from(primitive.first_index)
            .map_err(|_| OpenProjectError::new("primitive start index exceeds runtime range"))?;
        let index_count = u32::try_from(primitive.index_count)
            .map_err(|_| OpenProjectError::new("primitive index count exceeds runtime range"))?;
        let triangle_start = usize::try_from(primitive.first_index / 3).map_err(|_| {
            OpenProjectError::new("primitive triangle offset exceeds runtime range")
        })?;
        let triangle_count = usize::try_from(primitive.index_count / 3)
            .map_err(|_| OpenProjectError::new("primitive triangle count exceeds runtime range"))?;
        let triangle_end = triangle_start
            .checked_add(triangle_count)
            .ok_or_else(|| OpenProjectError::new("primitive triangle range overflows"))?;
        let range = triangle_mesh_ids
            .get_mut(triangle_start..triangle_end)
            .ok_or_else(|| OpenProjectError::new("primitive triangle range is outside the mesh"))?;
        range.fill(mesh_id);
        let primitive_indices = indices
            .get(triangle_start..triangle_end)
            .ok_or_else(|| OpenProjectError::new("primitive indices are outside the mesh"))?;
        sub_meshes.push(SubMesh {
            mesh_id,
            start_index,
            index_count,
            material_index,
            material_name: project.materials[material_index].name.clone(),
            wireframe_edges: wireframe_edges(primitive_indices),
        });
    }
    let mesh = MeshData::new(
        positions,
        uvs,
        normals,
        indices,
        sub_meshes,
        mesh_objects,
        triangle_mesh_ids,
    )
    .map_err(|error| OpenProjectError::new(format!("constructing mesh: {error:#}")))?;
    Ok((mesh, persistent_ids))
}

fn geometry_slice<'a>(
    bytes: &'a [u8],
    offset: u64,
    count: u64,
    stride: u64,
    label: &str,
) -> OpenResult<&'a [u8]> {
    let byte_len = count
        .checked_mul(stride)
        .ok_or_else(|| OpenProjectError::new(format!("{label} byte length overflows")))?;
    let end = offset
        .checked_add(byte_len)
        .ok_or_else(|| OpenProjectError::new(format!("{label} range overflows")))?;
    let start = usize::try_from(offset)
        .map_err(|_| OpenProjectError::new(format!("{label} offset exceeds runtime range")))?;
    let end = usize::try_from(end)
        .map_err(|_| OpenProjectError::new(format!("{label} end exceeds runtime range")))?;
    bytes
        .get(start..end)
        .ok_or_else(|| OpenProjectError::new(format!("{label} range exceeds geometry.bin")))
}

fn decode_vec3(bytes: &[u8], offset: u64, count: u32, label: &str) -> OpenResult<Vec<Vec3>> {
    Ok(geometry_slice(bytes, offset, u64::from(count), 12, label)?
        .chunks_exact(12)
        .map(|v| Vec3::new(read_f32(v, 0), read_f32(v, 4), read_f32(v, 8)))
        .collect())
}

fn decode_vec2(bytes: &[u8], offset: u64, count: u32, label: &str) -> OpenResult<Vec<Vec2>> {
    Ok(geometry_slice(bytes, offset, u64::from(count), 8, label)?
        .chunks_exact(8)
        .map(|v| Vec2::new(read_f32(v, 0), read_f32(v, 4)))
        .collect())
}

fn decode_u32(bytes: &[u8], offset: u64, count: u64, label: &str) -> OpenResult<Vec<u32>> {
    Ok(geometry_slice(bytes, offset, count, 4, label)?
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect())
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn wireframe_edges(triangles: &[[u32; 3]]) -> Vec<[u32; 2]> {
    let mut edges = BTreeSet::new();
    for triangle in triangles {
        for [left, right] in [
            [triangle[0], triangle[1]],
            [triangle[1], triangle[2]],
            [triangle[2], triangle[0]],
        ] {
            edges.insert(if left <= right {
                [left, right]
            } else {
                [right, left]
            });
        }
    }
    edges.into_iter().collect()
}

fn decode_materials(project: &ProjectV1) -> Vec<(MaterialId, MaterialSpec)> {
    project
        .materials
        .iter()
        .map(|material| {
            let settings = MaterialRenderSettings {
                double_sided: material.render_settings.double_sided,
                render_mode: match material.render_settings.render_mode {
                    MaterialRenderModeV1::Opaque => MaterialRenderMode::Opaque,
                    MaterialRenderModeV1::Cutoff => MaterialRenderMode::Cutoff,
                    MaterialRenderModeV1::Blend => MaterialRenderMode::Blend,
                },
                alpha_cutoff: material.render_settings.alpha_cutoff,
            };
            (
                MaterialId(material.id),
                MaterialSpec::new(
                    material.name.clone(),
                    [material.texture_size.width, material.texture_size.height],
                )
                .with_render_settings(settings)
                .with_export_image_file_name(material.export_image_file_name.clone())
                .with_export_psd_file_name(material.export_psd_file_name.clone()),
            )
        })
        .collect()
}

fn decode_layer(layer: &LayerV1) -> OpenResult<RestoredLayerNode> {
    Ok(RestoredLayerNode {
        persistent_id: PersistentLayerId(layer.id),
        parent_id: layer.parent_id.map(PersistentLayerId),
        order: layer.order,
        props: LayerProperties {
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            opacity: layer.opacity,
            blend_mode: decode_blend_mode(layer.blend_mode),
        },
        content: decode_layer_content(&layer.content)?,
        mask: layer.mask.map(|mask| LayerMaskProperties {
            enabled: mask.enabled,
        }),
        material_mask: match &layer.material_mask {
            None => LayerMaterialMask::Unspecified,
            Some(ids) => {
                LayerMaterialMask::Specified(ids.iter().copied().map(MaterialId).collect())
            }
        },
    })
}

fn decode_blend_mode(mode: BlendModeV1) -> LayerBlendMode {
    match mode {
        BlendModeV1::Normal => LayerBlendMode::Normal,
        BlendModeV1::Darken => LayerBlendMode::Darken,
        BlendModeV1::Multiply => LayerBlendMode::Multiply,
        BlendModeV1::Lighten => LayerBlendMode::Lighten,
        BlendModeV1::Screen => LayerBlendMode::Screen,
        BlendModeV1::ColorDodge => LayerBlendMode::ColorDodge,
        BlendModeV1::LinearDodge => LayerBlendMode::LinearDodge,
        BlendModeV1::Overlay => LayerBlendMode::Overlay,
        BlendModeV1::SoftLight => LayerBlendMode::SoftLight,
        BlendModeV1::HardLight => LayerBlendMode::HardLight,
        BlendModeV1::Color => LayerBlendMode::Color,
    }
}

fn decode_layer_content(content: &LayerContentV1) -> OpenResult<LayerContent> {
    Ok(match content {
        LayerContentV1::Raster => LayerContent::Raster,
        LayerContentV1::EmbeddedImage {
            image_id,
            transform,
        } => LayerContent::EmbeddedImage {
            image_id: EmbeddedImageId(*image_id),
            transform: EmbeddedImageTransform {
                center_uv: Vec2::from_array(transform.center_uv),
                size_uv: Vec2::from_array(transform.size_uv),
                rotation_radians: transform.rotation_radians,
            },
        },
        LayerContentV1::SolidFill { color } => LayerContent::SolidFill { color: *color },
        LayerContentV1::Adjustment { adjustment } => LayerContent::Adjustment {
            adjustment: decode_adjustment(adjustment)?,
        },
        LayerContentV1::Group { composite_mode } => LayerContent::Group {
            children: Vec::new(),
            composite_mode: match composite_mode {
                GroupCompositeModeV1::Isolated => GroupCompositeMode::Isolated,
                GroupCompositeModeV1::PassThrough => GroupCompositeMode::PassThrough,
            },
        },
    })
}

fn decode_adjustment(value: &AdjustmentV1) -> OpenResult<Adjustment> {
    Ok(match value {
        AdjustmentV1::BrightnessContrast {
            brightness,
            contrast,
        } => Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: *brightness,
            contrast: *contrast,
        }),
        AdjustmentV1::Levels {
            master,
            red,
            green,
            blue,
        } => Adjustment::Levels(LevelsAdjustment {
            master: decode_levels(*master),
            red: decode_levels(*red),
            green: decode_levels(*green),
            blue: decode_levels(*blue),
        }),
        AdjustmentV1::Curves {
            master,
            red,
            green,
            blue,
        } => Adjustment::Curves(CurvesAdjustment {
            master: decode_curve(master)?,
            red: decode_curve(red)?,
            green: decode_curve(green)?,
            blue: decode_curve(blue)?,
        }),
        AdjustmentV1::HueSaturation {
            hue,
            saturation,
            lightness,
        } => Adjustment::HueSaturation(HueSaturationAdjustment {
            hue: *hue,
            saturation: *saturation,
            lightness: *lightness,
        }),
        AdjustmentV1::Invert => Adjustment::Invert,
        AdjustmentV1::GradientMap {
            stops,
            reverse,
            dither,
        } => Adjustment::GradientMap(decode_gradient(stops, *reverse, *dither)?),
        AdjustmentV1::UvMirror {
            axis,
            position,
            direction,
        } => Adjustment::UvMirror(UvMirrorAdjustment {
            axis: match axis {
                UvAxisV1::X => UvMirrorAxis::X,
                UvAxisV1::Y => UvMirrorAxis::Y,
            },
            position: *position,
            direction: match direction {
                UvMirrorDirectionV1::PositiveToNegative => UvMirrorDirection::PositiveToNegative,
                UvMirrorDirectionV1::NegativeToPositive => UvMirrorDirection::NegativeToPositive,
            },
        }),
    })
}

fn decode_levels(value: LevelsChannelV1) -> LevelsChannel {
    LevelsChannel {
        input_black: value.input_black,
        input_white: value.input_white,
        gamma: value.gamma,
        output_black: value.output_black,
        output_white: value.output_white,
    }
}

fn decode_curve(values: &[CurvePointV1]) -> OpenResult<CurveChannel> {
    if values.len() > MAX_CURVE_POINTS {
        return Err(OpenProjectError::new(
            "curve has too many points for the runtime",
        ));
    }
    let points = values
        .iter()
        .map(|source| CurvePoint {
            input: source.input,
            output: source.output,
        })
        .collect::<Vec<_>>();
    CurveChannel::from_points(&points)
        .ok_or_else(|| OpenProjectError::new("curve points are invalid"))
}

fn decode_gradient(
    values: &[GradientStopV1],
    reverse: bool,
    dither: bool,
) -> OpenResult<GradientMapAdjustment> {
    let stops = values
        .iter()
        .map(|source| GradientStop {
            location: source.location,
            midpoint: source.midpoint,
            color: source.color,
        })
        .collect();
    Ok(GradientMapAdjustment {
        stops,
        reverse,
        dither,
    })
}

fn runtime_surface(
    surface: &SurfaceV1,
    layers: &HashMap<PersistentLayerId, crate::core::surface::LayerId>,
    materials: &HashMap<u64, crate::core::material::MaterialIndex>,
) -> OpenResult<PaintSurfaceId> {
    let layer_id = *layers
        .get(&PersistentLayerId(surface.layer_id))
        .ok_or_else(|| OpenProjectError::new("surface references an unknown layer"))?;
    let material_index = *materials
        .get(&surface.material_id)
        .ok_or_else(|| OpenProjectError::new("surface references an unknown material"))?;
    Ok(match surface.role {
        SurfaceRoleV1::Raster => PaintSurfaceId::raster(material_index, layer_id),
        SurfaceRoleV1::LayerMask => PaintSurfaceId::layer_mask(material_index, layer_id),
    })
}

fn decode_surface(
    archive: &mut ZipArchive<File>,
    surface: &SurfaceV1,
    runtime_surface: PaintSurfaceId,
    store: &mut DocumentTileStore,
) -> OpenResult<()> {
    let name = surface_entry_name(surface.layer_id, surface.material_id, surface.role);
    if surface.tile_size == 0 {
        return Err(OpenProjectError::new(format!(
            "{name} has an invalid zero tile size"
        )));
    }
    if surface.tile_size != store.tile_size() {
        return Err(OpenProjectError::new(format!(
            "{name} tile size {} differs from project tile size {}",
            surface.tile_size,
            store.tile_size()
        )));
    }
    let blob = read_archive_entry(archive, &name)?;
    let grid = TileGrid::new([surface.width, surface.height], surface.tile_size);
    let mut cursor = 0usize;
    for tile in &surface.tiles {
        let end = cursor
            .checked_add(tile.compressed_size as usize)
            .ok_or_else(|| OpenProjectError::new(format!("{name} tile range overflows")))?;
        let frame = blob
            .get(cursor..end)
            .ok_or_else(|| OpenProjectError::new(format!("{name} ends inside a tile frame")))?;
        cursor = end;
        let raw = zstd::stream::decode_all(frame).map_err(|error| {
            OpenProjectError::new(format!(
                "decoding {name} tile ({}, {}): {error}",
                tile.x, tile.y
            ))
        })?;
        let rect = grid.rect_for_tile(TileCoord {
            x: tile.x,
            y: tile.y,
        });
        let pixel_count = (rect.size[0] as usize)
            .checked_mul(rect.size[1] as usize)
            .ok_or_else(|| OpenProjectError::new("surface tile pixel count overflows"))?;
        let rgba8 = match surface.pixel_format {
            PixelFormatV1::Rgba8Unorm => {
                let expected = pixel_count.checked_mul(4).ok_or_else(|| {
                    OpenProjectError::new("surface tile payload length overflows")
                })?;
                if raw.len() != expected {
                    return Err(OpenProjectError::new(format!(
                        "{name} tile has an unexpected payload length"
                    )));
                }
                raw
            }
            PixelFormatV1::R8Unorm => {
                if raw.len() != pixel_count {
                    return Err(OpenProjectError::new(format!(
                        "{name} tile has an unexpected payload length"
                    )));
                }
                raw.into_iter()
                    .flat_map(|coverage| [coverage, coverage, coverage, coverage])
                    .collect()
            }
        };
        store
            .write_surface_rect(
                runtime_surface,
                RectU32 {
                    origin: rect.origin,
                    size: rect.size,
                },
                &PixelSnapshotData::contiguous(rgba8),
            )
            .map_err(|error| OpenProjectError::new(format!("storing {name} tile: {error:#}")))?;
    }
    if cursor != blob.len() {
        return Err(OpenProjectError::new(format!(
            "{name} has trailing tile bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        document::{MeshId, MeshObject},
        material::MaterialSpec,
        surface::LayerId,
    };
    use crate::project::save_project;
    use std::{io::Write, sync::Arc};

    fn editor_state(document: &Document, active_layer_id: LayerId) -> ProjectEditorState {
        ProjectEditorState::for_document(document, active_layer_id)
    }

    #[test]
    fn active_group_round_trip_is_restored_into_application_state() {
        use crate::application::{AppState, ApplicationRuntime, Command};

        let mut document = test_document();
        let base = document.layer_tree.default_raster_layer().unwrap();
        let selected = document
            .layer_tree
            .add_group_above(base, "Selected")
            .unwrap();
        let selected_persistent = document.layer_tree.persistent_id(selected).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("active-layer.holopaint");

        save_project(&document, &editor_state(&document, selected), &path).unwrap();
        let loaded = open_project(&path).unwrap();
        assert_eq!(
            loaded
                .document
                .layer_tree
                .persistent_id(loaded.editor_state.document_focus.active_layer_id),
            Some(selected_persistent)
        );
        let loaded_active_layer_id = loaded.editor_state.document_focus.active_layer_id;

        let mut runtime = ApplicationRuntime::new(AppState::default());
        runtime.dispatch(Command::ProjectLoaded(loaded)).unwrap();
        assert_eq!(runtime.state.active_layer_id(), loaded_active_layer_id);
    }

    #[test]
    fn complete_editor_state_round_trips_without_ui_range_clamping() {
        use crate::{
            application::{AppState, ApplicationRuntime, Command},
            core::{
                camera::CameraProjection, document::ActiveLayerPart,
                document_tile_store::InitialPixels, surface::PaintSurfaceId,
                uv_view::UvViewTransform, viewport_shading::ViewportShading,
                viewport_visibility::ViewportSceneVisibility, wireframe::WireframeStyle,
            },
        };

        let source = test_document();
        let mut document = Document::new(
            source.mesh.clone(),
            vec![
                MaterialSpec::new("M", [2, 2]),
                MaterialSpec::new("N", [2, 2]),
            ],
        );
        let layer = document.layer_tree.default_raster_layer().unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        for material_index in 0..2 {
            document
                .tiles
                .create_surface(
                    PaintSurfaceId::layer_mask(material_index.into(), layer),
                    [2, 2],
                    InitialPixels::SparseDefaultRgba8([255; 4]),
                )
                .unwrap();
        }
        let group = document.layer_tree.add_group_above(layer, "Group").unwrap();
        let mesh_id = document.mesh.mesh_objects[0].id;
        let mut visibility = ViewportSceneVisibility::default();
        visibility.set_mesh_visible(mesh_id, false);
        visibility.set_material_visible(1.into(), false);
        let mut camera = crate::core::camera::OrbitCamera::default();
        camera.target = Vec3::new(1.0e20, -2.0e20, 3.0e20);
        camera.distance = 1.0e12;
        camera.projection = CameraProjection::Orthographic;
        camera.fov_y_radians = 0.75;
        camera.orthographic_height = 2.0e12;
        let mut state = ProjectEditorState::for_document(&document, layer);
        state.document_focus.active_layer_part = ActiveLayerPart::LayerMask;
        state.document_focus.focused_material_index = 1;
        state.view.camera = camera.clone();
        state.view.shading = ViewportShading::Unlit;
        state.view.gizmo_visible = false;
        state.view.background_color = [0.1, 0.2, 0.3];
        state.view.viewport_wireframe_visible = false;
        state.view.viewport_wireframe = WireframeStyle::new([0.4, 0.5, 0.6], 0.7);
        state.view.uv_view_transform = UvViewTransform {
            center_uv: Vec2::new(1.0e10, -1.0e10),
            zoom: 1.0e8,
            rotation_radians: 1000.0,
        };
        state.view.scene_visibility = visibility;
        state.view.surface_mirror_x_enabled = true;
        state.view.surface_mirror_x_plane = -1.25;
        state.view.surface_mirror_x_plane_visible = false;
        state.view.uv_background_color = [0.7, 0.6, 0.5];
        state.view.uv_wireframe_visible = false;
        state.view.uv_wireframe = WireframeStyle::new([0.3, 0.2, 0.1], 0.4);
        state.layer_tree.collapsed_layer_group_ids = vec![group];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("editor-state.holopaint");

        save_project(&document, &state, &path).unwrap();
        let loaded = open_project(&path).unwrap();

        assert_eq!(
            loaded.editor_state.document_focus.active_layer_part,
            ActiveLayerPart::LayerMask
        );
        assert_eq!(loaded.editor_state.document_focus.focused_material_index, 1);
        assert_eq!(loaded.editor_state.view.camera, camera);
        assert_eq!(loaded.editor_state.view.shading, ViewportShading::Unlit);
        assert!(!loaded.editor_state.view.gizmo_visible);
        assert_eq!(loaded.editor_state.view.background_color, [0.1, 0.2, 0.3]);
        assert!(!loaded.editor_state.view.viewport_wireframe_visible);
        assert_eq!(
            loaded.editor_state.view.viewport_wireframe,
            WireframeStyle::new([0.4, 0.5, 0.6], 0.7)
        );
        assert_eq!(
            loaded.editor_state.view.uv_view_transform,
            state.view.uv_view_transform
        );
        assert!(loaded.editor_state.view.surface_mirror_x_enabled);
        assert_eq!(loaded.editor_state.view.surface_mirror_x_plane, -1.25);
        assert!(!loaded.editor_state.view.surface_mirror_x_plane_visible);
        assert_eq!(
            loaded.editor_state.view.uv_background_color,
            [0.7, 0.6, 0.5]
        );
        assert!(!loaded.editor_state.view.uv_wireframe_visible);
        assert_eq!(
            loaded.editor_state.view.uv_wireframe,
            WireframeStyle::new([0.3, 0.2, 0.1], 0.4)
        );
        assert!(
            !loaded
                .editor_state
                .view
                .scene_visibility
                .mesh_visible(mesh_id)
        );
        assert!(
            !loaded
                .editor_state
                .view
                .scene_visibility
                .material_visible(1.into())
        );
        assert_eq!(
            loaded
                .editor_state
                .layer_tree
                .collapsed_layer_group_ids
                .len(),
            1
        );
        assert!(
            loaded
                .document
                .layer_tree
                .is_group(loaded.editor_state.layer_tree.collapsed_layer_group_ids[0])
        );

        let mut runtime = ApplicationRuntime::new(AppState::default());
        runtime
            .dispatch(Command::SetSurfaceMirrorXPlane(99.0))
            .unwrap();
        runtime.dispatch(Command::ProjectLoaded(loaded)).unwrap();
        assert_eq!(runtime.state.camera(), &camera);
        assert_eq!(runtime.state.viewport_shading(), ViewportShading::Unlit);
        assert!(!runtime.state.viewport_gizmo_visible());
        assert_eq!(runtime.state.viewport_background_color(), [0.1, 0.2, 0.3]);
        assert!(!runtime.state.viewport_wireframe_visible());
        assert_eq!(runtime.state.surface_mirror_options().x_plane, -1.25);
        assert_eq!(runtime.state.uv_view_background_color(), [0.7, 0.6, 0.5]);
        assert!(!runtime.state.uv_wireframe_visible());
    }

    #[test]
    fn save_open_round_trip_restores_ids_geometry_and_pixels() {
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
                material_name: "M".into(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".into(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        let mut document = Document::new(
            mesh,
            vec![
                MaterialSpec::new("M", [2, 2])
                    .with_export_image_file_name(Some("avatar_body.png".to_owned()))
                    .with_export_psd_file_name(Some("avatar_body.psd".to_owned())),
            ],
        );
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface = PaintSurfaceId::raster(0.into(), layer);
        document
            .tiles
            .write_surface_rect(
                surface,
                RectU32::full([2, 2]),
                &PixelSnapshotData::contiguous(vec![1, 2, 3, 4].repeat(4)),
            )
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("roundtrip.holopaint");
        save_project(&document, &editor_state(&document, layer), &path).unwrap();
        let loaded = open_project(&path).unwrap();
        assert_eq!(loaded.document.project_id, document.project_id);
        assert_eq!(
            loaded.document.materials[0]
                .export_image_file_name
                .as_deref(),
            Some("avatar_body.png")
        );
        assert_eq!(
            loaded.document.materials[0].export_psd_file_name.as_deref(),
            Some("avatar_body.psd")
        );
        assert_eq!(
            loaded.document.mesh.positions,
            Arc::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y])
        );
        assert_eq!(loaded.surfaces.len(), 1);
        assert_eq!(loaded.surfaces[0].tiles.len(), 1);
        assert_eq!(
            loaded.surfaces[0].tiles[0].rgba8,
            vec![1, 2, 3, 4].repeat(4)
        );
    }

    #[test]
    fn revealed_mask_default_remains_sparse_across_open_and_save() {
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
                material_name: "M".into(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".into(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        let mut document = Document::new(mesh, vec![MaterialSpec::new("M", [2, 2])]);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        document
            .tiles
            .create_surface(
                PaintSurfaceId::layer_mask(0.into(), layer),
                [2, 2],
                InitialPixels::SparseDefaultRgba8([255; 4]),
            )
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.holopaint");
        let second = directory.path().join("second.holopaint");
        save_project(&document, &editor_state(&document, layer), &first).unwrap();

        let loaded = open_project(&first).unwrap();
        let loaded_mask = loaded
            .surfaces
            .iter()
            .find(|surface| surface.surface.is_mask())
            .unwrap();
        assert_eq!(loaded_mask.default_rgba8, [255; 4]);
        assert!(loaded_mask.tiles.is_empty());
        assert_eq!(
            loaded
                .document
                .tiles
                .surface_default_rgba8(loaded_mask.surface),
            Some([255; 4])
        );

        save_project(&loaded.document, &loaded.editor_state, &second).unwrap();
        let reopened = open_project(&second).unwrap();
        let reopened_mask = reopened
            .surfaces
            .iter()
            .find(|surface| surface.surface.is_mask())
            .unwrap();
        assert_eq!(
            reopened
                .document
                .tiles
                .surface_default_rgba8(reopened_mask.surface),
            Some([255; 4])
        );
    }

    #[test]
    fn zero_tile_size_is_rejected_without_panicking() {
        let document = test_document();
        let directory = tempfile::tempdir().unwrap();
        let valid_path = directory.path().join("valid.holopaint");
        let invalid_path = directory.path().join("invalid.holopaint");
        let active_layer = document.layer_tree.default_raster_layer().unwrap();
        save_project(
            &document,
            &editor_state(&document, active_layer),
            &valid_path,
        )
        .unwrap();

        let (mut project, geometry, surface_name, surface_blob) = {
            let file = File::open(&valid_path).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let project: ProjectV1 = {
                let mut entry = archive.by_name("project.ron").unwrap();
                let mut source = String::new();
                entry.read_to_string(&mut source).unwrap();
                ron::from_str(&source).unwrap()
            };
            let geometry = read_archive_entry(&mut archive, "geometry.bin").unwrap();
            let surface = &project.surfaces[0];
            let surface_name =
                surface_entry_name(surface.layer_id, surface.material_id, surface.role);
            let surface_blob = read_archive_entry(&mut archive, &surface_name).unwrap();
            (project, geometry, surface_name, surface_blob)
        };
        project.surfaces[0].tile_size = 0;

        let file = File::create(&invalid_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("geometry.bin", options).unwrap();
        archive.write_all(&geometry).unwrap();
        archive.start_file(surface_name, options).unwrap();
        archive.write_all(&surface_blob).unwrap();
        archive.start_file("project.ron", options).unwrap();
        archive
            .write_all(
                ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
        archive.finish().unwrap();

        let result = std::panic::catch_unwind(|| open_project(&invalid_path));
        assert!(result.is_ok());
        let error = result.unwrap().unwrap_err().to_string();
        assert!(error.contains("tile size"));
    }

    #[test]
    fn unknown_active_layer_falls_back_to_default_layer() {
        let document = test_document();
        let directory = tempfile::tempdir().unwrap();
        let valid_path = directory.path().join("valid.holopaint");
        let invalid_path = directory.path().join("invalid-active-layer.holopaint");
        let active_layer = document.layer_tree.default_raster_layer().unwrap();
        save_project(
            &document,
            &editor_state(&document, active_layer),
            &valid_path,
        )
        .unwrap();

        let (mut project, geometry, surface_name, surface_blob) = {
            let file = File::open(&valid_path).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let project: ProjectV1 = {
                let mut entry = archive.by_name("project.ron").unwrap();
                let mut source = String::new();
                entry.read_to_string(&mut source).unwrap();
                ron::from_str(&source).unwrap()
            };
            let geometry = read_archive_entry(&mut archive, "geometry.bin").unwrap();
            let surface = &project.surfaces[0];
            let surface_name =
                surface_entry_name(surface.layer_id, surface.material_id, surface.role);
            let surface_blob = read_archive_entry(&mut archive, &surface_name).unwrap();
            (project, geometry, surface_name, surface_blob)
        };
        project.editor_state.document_focus.active_layer_id = 999_999;
        project.editor_state.document_focus.focused_material_id = 999_999;

        let file = File::create(&invalid_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("geometry.bin", options).unwrap();
        archive.write_all(&geometry).unwrap();
        archive.start_file(surface_name, options).unwrap();
        archive.write_all(&surface_blob).unwrap();
        archive.start_file("project.ron", options).unwrap();
        archive
            .write_all(
                ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
        archive.finish().unwrap();

        let loaded = open_project(&invalid_path).unwrap();
        assert_eq!(
            loaded.editor_state.document_focus.active_layer_id,
            loaded.document.layer_tree.default_raster_layer().unwrap()
        );
        assert_eq!(loaded.editor_state.document_focus.focused_material_index, 0);
    }

    #[test]
    fn revealed_mask_first_is_created_white_independently_of_surface_order() {
        use crate::{
            application::{AppState, ApplicationRuntime, Command},
            core::image::LayerInitialPixels,
            renderer::GpuDocumentCommand,
        };

        let mut document = test_document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        let mask = PaintSurfaceId::layer_mask(0.into(), layer);
        document
            .tiles
            .create_surface(mask, [2, 2], InitialPixels::SparseDefaultRgba8([255; 4]))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mask-first.holopaint");
        save_project(&document, &editor_state(&document, layer), &path).unwrap();
        let mut loaded = open_project(&path).unwrap();
        loaded
            .surfaces
            .sort_by_key(|surface| !surface.surface.is_mask());

        let mut runtime = ApplicationRuntime::new(AppState::default());
        runtime.dispatch(Command::ProjectLoaded(loaded)).unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        let GpuDocumentCommand::UploadScene { surfaces, .. } = &plan.document_commands[0] else {
            panic!("expected scene upload");
        };
        assert!(surfaces.is_empty());
        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::SolidRgba8([255, 255, 255, 255]),
            } if *target == mask
        )));
        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::Transparent,
            } if !target.is_mask()
        )));
    }

    #[test]
    fn loaded_project_plan_keeps_empty_surfaces_sparse() {
        use crate::{
            application::{AppState, ApplicationRuntime, Command},
            renderer::GpuDocumentCommand,
        };

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
                material_name: "M".into(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".into(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        let mut document = Document::new(mesh, vec![MaterialSpec::new("M", [2, 2])]);
        let second = document
            .layer_tree
            .add_raster_layer_to_root("Second")
            .unwrap();
        document
            .tiles
            .create_surface(
                PaintSurfaceId::raster(0.into(), second),
                [2, 2],
                InitialPixels::Transparent,
            )
            .unwrap();
        let first = PaintSurfaceId::raster(
            0.into(),
            document.layer_tree.default_raster_layer().unwrap(),
        );
        document
            .tiles
            .write_surface_rect(
                first,
                RectU32::full([2, 2]),
                &PixelSnapshotData::contiguous(vec![10, 20, 30, 255].repeat(4)),
            )
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layers.holopaint");
        save_project(&document, &editor_state(&document, second), &path).unwrap();
        let loaded = open_project(&path).unwrap();
        let mut runtime = ApplicationRuntime::new(AppState::default());
        runtime.dispatch(Command::ProjectLoaded(loaded)).unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert_eq!(
            plan.document_commands
                .iter()
                .filter(|command| matches!(command, GpuDocumentCommand::CreateSurface { .. }))
                .count(),
            2
        );
        assert_eq!(
            plan.document_commands
                .iter()
                .filter(|command| matches!(command, GpuDocumentCommand::UploadSurfaceTiles { .. }))
                .count(),
            1
        );
        assert_eq!(
            plan.document_commands
                .iter()
                .filter(|command| matches!(command, GpuDocumentCommand::UploadSurfaceRgba8 { .. }))
                .count(),
            0
        );
    }

    #[test]
    fn all_blend_modes_round_trip_without_changing_project_version() {
        for mode in LayerBlendMode::ALL {
            let mut document = test_document();
            let layer = document.layer_tree.default_raster_layer().unwrap();
            document.layer_tree.set_blend_mode(layer, mode);

            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("blend_mode.holopaint");
            save_project(&document, &editor_state(&document, layer), &path).unwrap();

            let file = std::fs::File::open(&path).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let project: ProjectV1 = {
                let mut entry = archive.by_name("project.ron").unwrap();
                let mut source = String::new();
                entry.read_to_string(&mut source).unwrap();
                ron::from_str(&source).unwrap()
            };
            assert_eq!(project.format_version, 1, "{mode:?}");
            drop(archive);

            let loaded = open_project(&path).unwrap();
            let loaded_layer = loaded.document.layer_tree.default_raster_layer().unwrap();
            assert_eq!(
                loaded
                    .document
                    .layer_tree
                    .get(loaded_layer)
                    .unwrap()
                    .props
                    .blend_mode,
                mode,
                "{mode:?}"
            );
        }
    }

    fn test_document() -> Document {
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
                material_name: "M".into(),
                wireframe_edges: vec![[0, 1], [1, 2], [0, 2]],
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Triangle".into(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        Document::new(mesh, vec![MaterialSpec::new("M", [2, 2])])
    }
}
