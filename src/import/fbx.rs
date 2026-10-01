use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use glam::{Vec2, Vec3};

use crate::{
    core::document::{MeshData, MeshId, MeshObject, SubMesh},
    core::material::{MaterialRenderMode, MaterialRenderSettings},
};

use super::{
    ImportedAsset, ImportedMaterial, ImportedTexture, build_wireframe_edges,
    decode_imported_texture,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct IndexedVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

fn load_scene(path: &Path) -> Result<ufbx::SceneRoot> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("FBX import failed: could not read {}", path.display()))?;
    let filename = path.to_string_lossy().into_owned();
    let raw_filename = filename.as_bytes().to_vec();
    let opts = ufbx::LoadOpts {
        filename: filename.into(),
        raw_filename: raw_filename.into(),
        file_format: ufbx::FileFormat::Fbx,
        load_external_files: false,
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        normalize_normals: true,
        ignore_animation: true,
        ..Default::default()
    };
    ufbx::load_memory(&bytes, opts).map_err(|error| {
        anyhow::anyhow!(
            "FBX import failed: {} ({:?})",
            error.description,
            error.type_
        )
    })
}

pub(super) fn load(path: &Path) -> Result<ImportedAsset> {
    let scene = load_scene(path)?;

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut sub_meshes = Vec::new();
    let mut mesh_objects = Vec::new();
    let mut triangle_mesh_ids = Vec::new();
    let mut materials = Vec::new();
    let mut material_map = HashMap::new();

    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let mesh_index = mesh_objects.len();
        let mesh_id = MeshId(mesh_index);
        let object_name = object_name(node, mesh, mesh_index);

        ensure!(
            mesh.vertex_uv.exists,
            "FBX import failed: mesh {object_name:?} has no UV set; HoloPainter requires UV coordinates"
        );
        ensure!(
            mesh.vertex_normal.exists,
            "FBX import failed: mesh {object_name:?} has no normals after normal generation"
        );
        if mesh.num_point_faces > 0 || mesh.num_line_faces > 0 {
            bail!(
                "FBX import failed: mesh {object_name:?} contains point or line geometry; only polygon meshes are supported"
            );
        }

        let reverses_winding = ufbx::matrix_determinant(&node.geometry_to_world) < 0.0;
        let normal_matrix = ufbx::matrix_for_normals(&node.geometry_to_world);
        let mut triangle_corners = vec![0_u32; mesh.max_face_triangles.saturating_mul(3)];
        let object_triangle_start = indices.len();

        for part in &mesh.material_parts {
            if part.num_triangles == 0 {
                continue;
            }
            if part.num_point_faces > 0 || part.num_line_faces > 0 {
                bail!(
                    "FBX import failed: mesh {object_name:?}, material part {} contains point or line geometry; only polygon meshes are supported",
                    part.index
                );
            }
            let (material_index, material_name) =
                resolve_material(node, part, &object_name, &mut material_map, &mut materials)?;
            let mut vertices = Vec::with_capacity(part.num_triangles.saturating_mul(3));

            for &face_index in &part.face_indices {
                let face = *mesh.faces.get(face_index as usize).with_context(|| {
                    format!(
                        "FBX import failed: mesh {object_name:?}, material part {} references invalid face {face_index}",
                        part.index
                    )
                })?;
                let triangle_count = mesh.triangulate_face(&mut triangle_corners, face) as usize;
                let corner_count = triangle_count
                    .checked_mul(3)
                    .context("FBX import failed: triangle corner count overflow")?;
                for triangle in triangle_corners[..corner_count].chunks_exact(3) {
                    let order = if reverses_winding {
                        [triangle[0], triangle[2], triangle[1]]
                    } else {
                        [triangle[0], triangle[1], triangle[2]]
                    };
                    for corner in order {
                        let corner = corner as usize;
                        let source_position = mesh.vertex_position[corner];
                        let source_normal = mesh.vertex_normal[corner];
                        let source_uv = mesh.vertex_uv[corner];
                        let position =
                            ufbx::transform_position(&node.geometry_to_world, source_position);
                        let normal = ufbx::transform_direction(&normal_matrix, source_normal);
                        let position =
                            Vec3::new(position.x as f32, position.y as f32, position.z as f32);
                        let normal = Vec3::new(normal.x as f32, normal.y as f32, normal.z as f32)
                            .normalize_or_zero();
                        let uv = Vec2::new(source_uv.x as f32, 1.0 - source_uv.y as f32);
                        ensure!(
                            position.is_finite() && normal.is_finite() && uv.is_finite(),
                            "FBX import failed: mesh {object_name:?} contains non-finite geometry"
                        );
                        vertices.push(IndexedVertex {
                            position: position.to_array(),
                            normal: normal.to_array(),
                            uv: uv.to_array(),
                        });
                    }
                }
            }

            ensure!(
                !vertices.is_empty(),
                "FBX import failed: mesh {object_name:?}, material part {} has no polygon triangles",
                part.index
            );
            let mut local_indices = vec![0_u32; vertices.len()];
            let vertex_count = ufbx::generate_indices(
                &mut [ufbx::VertexStream::new(&mut vertices)],
                &mut local_indices,
                ufbx::AllocatorOpts::default(),
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "FBX import failed: mesh {object_name:?}, material part {} could not generate vertex indices: {} ({:?})",
                    part.index,
                    error.description,
                    error.type_
                )
            })?;
            vertices.truncate(vertex_count);

            let vertex_offset = u32::try_from(positions.len())
                .context("FBX import failed: vertex offset exceeds u32")?;
            let start_triangle = indices.len();
            for triangle in local_indices.chunks_exact(3) {
                let triangle = [
                    triangle[0]
                        .checked_add(vertex_offset)
                        .context("FBX import failed: vertex index overflow")?,
                    triangle[1]
                        .checked_add(vertex_offset)
                        .context("FBX import failed: vertex index overflow")?,
                    triangle[2]
                        .checked_add(vertex_offset)
                        .context("FBX import failed: vertex index overflow")?,
                ];
                indices.push(triangle);
                triangle_mesh_ids.push(mesh_id);
            }
            for vertex in vertices {
                positions.push(Vec3::from_array(vertex.position));
                normals.push(Vec3::from_array(vertex.normal));
                uvs.push(Vec2::from_array(vertex.uv));
            }

            let part_triangles = &indices[start_triangle..];
            sub_meshes.push(SubMesh {
                mesh_id,
                start_index: u32::try_from(start_triangle.saturating_mul(3))
                    .context("FBX import failed: sub-mesh start exceeds u32")?,
                index_count: u32::try_from(part_triangles.len().saturating_mul(3))
                    .context("FBX import failed: sub-mesh index count exceeds u32")?,
                material_index,
                wireframe_edges: build_wireframe_edges(part_triangles),
                material_name,
            });
        }
        ensure!(
            indices.len() > object_triangle_start,
            "FBX import failed: mesh {object_name:?} has no polygon triangles"
        );
        mesh_objects.push(MeshObject {
            id: mesh_id,
            name: object_name,
        });
    }

    if mesh_objects.is_empty() {
        bail!("FBX import failed: scene has no mesh nodes");
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
    .context("FBX import failed: imported mesh is invalid")?;
    Ok(ImportedAsset { mesh, materials })
}

pub(super) fn load_base_color_textures(
    path: &Path,
    materials: &[ImportedMaterial],
) -> Result<Vec<Option<ImportedTexture>>> {
    let scene = load_scene(path).context("FBX Base Color Texture import failed")?;
    let mut decoded_files = HashMap::<u32, ImportedTexture>::new();
    let mut textures = Vec::with_capacity(materials.len());

    for material in materials {
        let Some(source_index) = material.source_material_index else {
            textures.push(None);
            continue;
        };
        let source = scene.materials.get(source_index).with_context(|| {
            format!(
                "FBX Base Color Texture import failed: material {:?} references missing source material {source_index}",
                material.name
            )
        })?;
        let map = &source.pbr.base_color;
        let Some(texture) = map.texture.as_ref() else {
            textures.push(None);
            continue;
        };
        if !map.texture_enabled || map.feature_disabled {
            textures.push(None);
            continue;
        }

        validate_texture_mapping(&scene, source.element.typed_id, texture, &material.name)?;
        ensure!(
            !texture.has_uv_transform,
            "FBX Base Color Texture import failed: material {:?} uses an unsupported Base Color UV transform",
            material.name
        );
        let file_texture = match texture.file_textures.len() {
            0 => bail!(
                "FBX Base Color Texture import failed: material {:?} Base Color texture has no file texture",
                material.name
            ),
            1 => texture.file_textures.first().unwrap(),
            count => bail!(
                "FBX Base Color Texture import failed: material {:?} Base Color texture resolves to {count} file textures; layered/composite textures are not supported",
                material.name
            ),
        };
        validate_texture_mapping(
            &scene,
            source.element.typed_id,
            file_texture,
            &material.name,
        )?;
        ensure!(
            !file_texture.has_uv_transform,
            "FBX Base Color Texture import failed: material {:?} uses an unsupported Base Color UV transform",
            material.name
        );
        ensure!(
            file_texture.has_file,
            "FBX Base Color Texture import failed: material {:?} Base Color texture does not identify a file",
            material.name
        );
        let file_index = file_texture.file_index;
        if let Some(decoded) = decoded_files.get(&file_index) {
            textures.push(Some(decoded.clone()));
            continue;
        }

        let texture_file = scene
            .texture_files
            .get(file_index as usize)
            .with_context(|| {
                format!(
                    "FBX Base Color Texture import failed: material {:?} references missing texture file {file_index}",
                    material.name
                )
            })?;
        let encoded = if !texture_file.content.is_empty() {
            texture_file.content.to_vec()
        } else if !file_texture.content.is_empty() {
            file_texture.content.to_vec()
        } else if let Some(video) = file_texture.video.as_ref() {
            if video.content.is_empty() {
                read_external_texture(path, texture_file, &material.name)?
            } else {
                video.content.to_vec()
            }
        } else {
            read_external_texture(path, texture_file, &material.name)?
        };
        let decoded = decode_imported_texture(&encoded).map_err(|error| {
            anyhow::anyhow!(
                "FBX Base Color Texture import failed: could not decode texture for material {:?}: {error:#}",
                material.name
            )
        })?;
        decoded_files.insert(file_index, decoded.clone());
        textures.push(Some(decoded));
    }

    Ok(textures)
}

fn validate_texture_mapping(
    scene: &ufbx::Scene,
    material_id: u32,
    texture: &ufbx::Texture,
    material_name: &str,
) -> Result<()> {
    if texture.uv_set.is_empty() {
        return Ok(());
    }
    for node in &scene.nodes {
        if !node
            .materials
            .iter()
            .any(|material| material.element.typed_id == material_id)
        {
            continue;
        }
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let primary_uv = mesh
            .uv_sets
            .first()
            .map(|set| set.name.as_ref())
            .unwrap_or("");
        ensure!(
            texture.uv_set == primary_uv,
            "FBX Base Color Texture import failed: material {material_name:?} requests UV set {:?}, but mesh {:?} uses primary UV set {primary_uv:?}",
            texture.uv_set,
            node.element.name
        );
    }
    Ok(())
}

fn read_external_texture(
    fbx_path: &Path,
    texture_file: &ufbx::TextureFile,
    material_name: &str,
) -> Result<Vec<u8>> {
    let directory = fbx_path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidates = Vec::<PathBuf>::new();
    push_texture_candidate(
        &mut candidates,
        directory,
        &texture_file.relative_filename,
        false,
    )?;
    push_texture_candidate(&mut candidates, directory, &texture_file.filename, false)?;
    push_texture_candidate(
        &mut candidates,
        directory,
        &texture_file.absolute_filename,
        true,
    )?;

    ensure!(
        !candidates.is_empty(),
        "FBX Base Color Texture import failed: material {material_name:?} has no supported local texture path"
    );
    for candidate in &candidates {
        match std::fs::read(candidate) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "FBX Base Color Texture import failed: could not read {} for material {material_name:?}",
                        candidate.display()
                    )
                });
            }
        }
    }
    bail!(
        "FBX Base Color Texture import failed: texture file for material {material_name:?} was not found (tried {})",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn push_texture_candidate(
    candidates: &mut Vec<PathBuf>,
    directory: &Path,
    filename: &str,
    absolute_only: bool,
) -> Result<()> {
    if filename.is_empty() {
        return Ok(());
    }
    let path = Path::new(filename);
    if !path.is_absolute() && has_uri_scheme(filename) {
        bail!(
            "FBX Base Color Texture import failed: remote texture URI is unsupported: {filename}"
        );
    }
    if absolute_only && !path.is_absolute() {
        return Ok(());
    }
    let candidate = if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    };
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
    Ok(())
}

fn has_uri_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || (index > 0 && matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.'))
        })
}

fn resolve_material(
    node: &ufbx::Node,
    part: &ufbx::MeshPart,
    object_name: &str,
    material_map: &mut HashMap<Option<u32>, usize>,
    materials: &mut Vec<ImportedMaterial>,
) -> Result<(usize, String)> {
    let source_material = if node.materials.is_empty() {
        None
    } else {
        let slot = part.index as usize;
        Some(node.materials.get(slot).with_context(|| {
            format!(
                "FBX import failed: mesh {object_name:?}, material part {} references invalid material slot {slot}",
                part.index
            )
        })?)
    };
    let source_id = source_material.map(|material| material.element.typed_id);
    let material_index = if let Some(&material_index) = material_map.get(&source_id) {
        material_index
    } else {
        let material_index = materials.len();
        let imported = match source_material {
            Some(material) => ImportedMaterial {
                source_material_index: Some(material.element.typed_id as usize),
                name: material_name(material),
                render_settings: material_render_settings(material),
            },
            None => ImportedMaterial {
                source_material_index: None,
                name: "Default Material".to_owned(),
                render_settings: MaterialRenderSettings::default(),
            },
        };
        materials.push(imported);
        material_map.insert(source_id, material_index);
        material_index
    };
    Ok((material_index, materials[material_index].name.clone()))
}

fn material_name(material: &ufbx::Material) -> String {
    let name = material.element.name.trim();
    if name.is_empty() {
        "Unnamed Material".to_owned()
    } else {
        name.to_owned()
    }
}

fn material_render_settings(material: &ufbx::Material) -> MaterialRenderSettings {
    let opacity = &material.pbr.opacity;
    let opacity_value_is_transparent = opacity.has_value && opacity.value_vec4.x < 1.0;
    let has_opacity_texture = opacity.texture_enabled && opacity.texture.is_some();
    let render_mode = if material.features.opacity.enabled
        && (opacity_value_is_transparent || has_opacity_texture)
    {
        MaterialRenderMode::Blend
    } else {
        MaterialRenderMode::Opaque
    };
    MaterialRenderSettings {
        double_sided: material.features.double_sided.enabled,
        render_mode,
        ..MaterialRenderSettings::default()
    }
}

fn object_name(node: &ufbx::Node, mesh: &ufbx::Mesh, mesh_index: usize) -> String {
    for name in [node.element.name.as_ref(), mesh.element.name.as_ref()] {
        if !name.trim().is_empty() {
            return name.to_owned();
        }
    }
    format!("Mesh {mesh_index}")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{load, load_base_color_textures, load_scene};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/import")
            .join(name)
    }

    fn load_modified_fixture(
        mut modify: impl FnMut(String) -> String,
    ) -> anyhow::Result<super::ImportedAsset> {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, modify(source)).unwrap();
        load(&path)
    }

    fn add_single_material(source: String, name: &str, properties: &str) -> String {
        source
            .replace("    Count: 2", "    Count: 3")
            .replace(
                "    ObjectType: \"Model\" {\n        Count: 1\n    }",
                "    ObjectType: \"Model\" {\n        Count: 1\n    }\n    ObjectType: \"Material\" {\n        Count: 1\n    }",
            )
            .replace(
                "        Layer: 0 {",
                "        LayerElementMaterial: 0 {\n            Version: 101\n            Name: \"\"\n            MappingInformationType: \"AllSame\"\n            ReferenceInformationType: \"IndexToDirect\"\n            Materials: *1 {\n                a: 0\n            }\n        }\n        Layer: 0 {",
            )
            .replace(
                "            LayerElement:  {\n                Type: \"LayerElementUV\"\n                TypedIndex: 0\n            }",
                "            LayerElement:  {\n                Type: \"LayerElementUV\"\n                TypedIndex: 0\n            }\n            LayerElement:  {\n                Type: \"LayerElementMaterial\"\n                TypedIndex: 0\n            }",
            )
            .replace(
                "    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {",
                &format!(
                    "    Material: 3000, \"Material::{name}\", \"\" {{\n        Version: 102\n        ShadingModel: \"unknown\"\n        MultiLayer: 0\n        Properties70:  {{\n{properties}\n        }}\n    }}\n    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {{"
                ),
            )
            .replace(
                "    C: \"OO\",2000,0",
                "    C: \"OO\",3000,2000\n    C: \"OO\",2000,0",
            )
    }

    fn add_base_color_texture(source: String, filename: &str, uv_set: &str) -> String {
        add_single_material(source, "Body", "")
            .replace("    Count: 3", "    Count: 5")
            .replace(
                "    ObjectType: \"Material\" {\n        Count: 1\n    }",
                "    ObjectType: \"Material\" {\n        Count: 1\n    }\n    ObjectType: \"Texture\" {\n        Count: 1\n    }\n    ObjectType: \"Video\" {\n        Count: 1\n    }",
            )
            .replace("ShadingModel: \"unknown\"", "ShadingModel: \"phong\"")
            .replace(
                "    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {",
                &format!(
                    "    Texture: 5000, \"Texture::BaseColor\", \"\" {{\n        Type: \"TextureVideoClip\"\n        Version: 202\n        TextureName: \"Texture::BaseColor\"\n        Properties70:  {{\n            P: \"UVSet\", \"KString\", \"\", \"\",\"{uv_set}\"\n        }}\n        Media: \"Video::BaseColor\"\n        FileName: \"{filename}\"\n        RelativeFilename: \"{filename}\"\n        ModelUVTranslation: 0,0\n        ModelUVScaling: 1,1\n        Texture_Alpha_Source: \"None\"\n        Cropping: 0,0,0,0\n    }}\n    Video: 6000, \"Video::BaseColor\", \"Clip\" {{\n        Type: \"Clip\"\n        Properties70:  {{\n        }}\n        UseMipMap: 0\n        Filename: \"{filename}\"\n        RelativeFilename: \"{filename}\"\n    }}\n    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {{"
                ),
            )
            .replace(
                "    C: \"OO\",3000,2000",
                "    C: \"OO\",6000,5000\n    C: \"OP\",5000,3000,\"DiffuseColor\"\n    C: \"OO\",3000,2000",
            )
    }

    fn png_bytes(size: [u32; 2], rgba: &[u8]) -> Vec<u8> {
        use image::ImageEncoder;

        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(rgba, size[0], size[1], image::ExtendedColorType::Rgba8)
            .unwrap();
        encoded
    }

    fn jpeg_bytes(size: [u32; 2], rgb: &[u8]) -> Vec<u8> {
        use image::ImageEncoder;

        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 100)
            .write_image(rgb, size[0], size[1], image::ExtendedColorType::Rgb8)
            .unwrap();
        encoded
    }

    fn make_two_triangle_quad(source: String) -> String {
        source
            .replace(
                "Vertices: *9 {\n            a: 0,0,0,1,0,0,0,1,0",
                "Vertices: *12 {\n            a: 0,0,0,1,0,0,1,1,0,0,1,0",
            )
            .replace(
                "PolygonVertexIndex: *3 {\n            a: 0,1,-3",
                "PolygonVertexIndex: *6 {\n            a: 0,1,-3,0,2,-4",
            )
            .replace(
                "Normals: *9 {\n                a: 0,0,1,0,0,1,0,0,1",
                "Normals: *18 {\n                a: 0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1",
            )
            .replace(
                "UV: *6 {\n                a: 0,0,1,0,0,1",
                "UV: *12 {\n                a: 0,0,1,0,1,1,0,0,1,1,0,1",
            )
            .replace(
                "UVIndex: *3 {\n                a: 0,1,2",
                "UVIndex: *6 {\n                a: 0,1,2,3,4,5",
            )
    }

    #[test]
    fn imports_external_base_color_png() {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(
            &path,
            add_base_color_texture(source, "base_color.png", "UVChannel_1"),
        )
        .unwrap();
        fs::write(
            directory.path().join("base_color.png"),
            png_bytes([2, 1], &[255, 0, 0, 255, 0, 255, 0, 128]),
        )
        .unwrap();

        let asset = load(&path).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();

        assert_eq!(textures.len(), 1);
        let texture = textures[0].as_ref().unwrap();
        assert_eq!(texture.size, [2, 1]);
        assert_eq!(texture.rgba8, [255, 0, 0, 255, 0, 255, 0, 128]);
    }

    #[test]
    fn imports_external_base_color_jpeg_and_reuses_one_file() {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, add_base_color_texture(source, "base_color.jpg", "")).unwrap();
        fs::write(
            directory.path().join("base_color.jpg"),
            jpeg_bytes([1, 1], &[24, 80, 160]),
        )
        .unwrap();

        let asset = load(&path).unwrap();
        let repeated = [asset.materials[0].clone(), asset.materials[0].clone()];
        let textures = load_base_color_textures(&path, &repeated).unwrap();

        assert_eq!(textures.len(), 2);
        assert_eq!(textures[0], textures[1]);
        assert_eq!(textures[0].as_ref().unwrap().size, [1, 1]);
        assert_eq!(textures[0].as_ref().unwrap().rgba8[3], 255);
    }

    #[test]
    fn prefers_relocated_relative_texture_over_recorded_absolute_path() {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let original_directory = tempfile::tempdir().unwrap();
        let absolute = original_directory
            .path()
            .join("original.png")
            .to_string_lossy()
            .replace('\\', "/");
        let source = add_base_color_texture(source, "local.png", "")
            .replace(
                "FileName: \"local.png\"",
                &format!("FileName: \"{absolute}\""),
            )
            .replace(
                "\n        Filename: \"local.png\"",
                &format!("\n        Filename: \"{absolute}\""),
            );
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, source).unwrap();
        fs::write(
            directory.path().join("local.png"),
            png_bytes([1, 1], &[1, 2, 3, 4]),
        )
        .unwrap();
        fs::write(
            original_directory.path().join("original.png"),
            png_bytes([1, 1], &[100, 101, 102, 103]),
        )
        .unwrap();

        let asset = load(&path).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();

        assert_eq!(textures[0].as_ref().unwrap().rgba8, [1, 2, 3, 4]);

        fs::remove_file(directory.path().join("local.png")).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();
        assert_eq!(textures[0].as_ref().unwrap().rgba8, [100, 101, 102, 103]);
    }

    #[test]
    fn prefers_embedded_base_color_over_existing_external_file() {
        use base64::Engine;

        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let embedded =
            base64::engine::general_purpose::STANDARD.encode(png_bytes([1, 1], &[12, 34, 56, 78]));
        let source = add_base_color_texture(source, "missing.png", "UVChannel_1").replace(
            "        UseMipMap: 0",
            &format!("        Content: , \"{embedded}\"\n        UseMipMap: 0"),
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, source).unwrap();
        fs::write(
            directory.path().join("missing.png"),
            png_bytes([1, 1], &[200, 201, 202, 203]),
        )
        .unwrap();

        let asset = load(&path).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();

        let texture = textures[0].as_ref().unwrap();
        assert_eq!(texture.size, [1, 1]);
        assert_eq!(texture.rgba8, [12, 34, 56, 78]);
    }

    #[test]
    fn imports_embedded_base_color_jpeg_without_external_file() {
        use base64::Engine;

        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let embedded =
            base64::engine::general_purpose::STANDARD.encode(jpeg_bytes([1, 1], &[32, 96, 192]));
        let source = add_base_color_texture(source, "missing.jpg", "").replace(
            "        UseMipMap: 0",
            &format!("        Content: , \"{embedded}\"\n        UseMipMap: 0"),
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, source).unwrap();

        let asset = load(&path).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();

        let texture = textures[0].as_ref().unwrap();
        assert_eq!(texture.size, [1, 1]);
        assert_eq!(texture.rgba8[3], 255);
    }

    #[test]
    fn returns_none_without_texture_and_errors_for_stale_material_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        fs::write(&path, add_single_material(source, "Body", "")).unwrap();
        let asset = load(&path).unwrap();

        let without_texture = load_base_color_textures(&path, &asset.materials).unwrap();
        assert_eq!(without_texture, [None]);
        let no_source_material = load_base_color_textures(
            &path,
            &[super::ImportedMaterial {
                source_material_index: None,
                name: "Default Material".to_owned(),
                render_settings: Default::default(),
            }],
        )
        .unwrap();
        assert_eq!(no_source_material, [None]);

        let mut stale = asset.materials[0].clone();
        stale.source_material_index = Some(99);
        let error = load_base_color_textures(&path, &[stale])
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing source material 99"));
    }

    #[test]
    fn returns_none_when_base_color_texture_mapping_is_disabled() {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let source = add_base_color_texture(source, "missing.png", "")
            .replace("ShadingModel: \"phong\"", "ShadingModel: \"unknown\"")
            .replacen(
                "        Properties70:  {\n\n        }",
                "        Properties70:  {\n            P: \"3dsMax|ClassIDa\", \"int\", \"Integer\", \"\",1030429932\n            P: \"3dsMax|ClassIDb\", \"int\", \"Integer\", \"\",-559038463\n            P: \"3dsMax|Parameters|base_color_map_on\", \"bool\", \"\", \"\",0\n        }",
                1,
            )
            .replace(
                "C: \"OP\",5000,3000,\"DiffuseColor\"",
                "C: \"OP\",5000,3000,\"3dsMax|Parameters|base_color_map\"",
            );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.fbx");
        fs::write(&path, source).unwrap();

        let scene = load_scene(&path).unwrap();
        assert!(scene.materials[0].pbr.base_color.texture.is_some());
        assert!(!scene.materials[0].pbr.base_color.texture_enabled);
        let asset = load(&path).unwrap();
        let textures = load_base_color_textures(&path, &asset.materials).unwrap();
        assert_eq!(textures, [None]);
    }

    #[test]
    fn rejects_missing_unsupported_and_remote_base_color_sources() {
        for (filename, bytes, expected) in [
            ("missing.png", None, "was not found"),
            ("unsupported.gif", Some(b"GIF89a".as_slice()), "unsupported"),
            (
                "invalid.png",
                Some(b"not an encoded image".as_slice()),
                "invalid image encoding",
            ),
            ("https://example.com/base.png", None, "remote texture URI"),
        ] {
            let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("fixture.fbx");
            fs::write(&path, add_base_color_texture(source, filename, "")).unwrap();
            if let Some(bytes) = bytes {
                fs::write(directory.path().join(filename), bytes).unwrap();
            }
            let asset = load(&path).unwrap();

            let error = load_base_color_textures(&path, &asset.materials)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "unexpected error: {error}");
        }
    }

    #[test]
    fn rejects_non_primary_uv_set_and_uv_transform() {
        let source = fs::read_to_string(fixture("triangle_ascii.fbx")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("uv_set.fbx");
        fs::write(
            &path,
            add_base_color_texture(source.clone(), "missing.png", "UVChannel_2"),
        )
        .unwrap();
        let scene = load_scene(&path).unwrap();
        assert_eq!(
            scene.materials[0]
                .pbr
                .base_color
                .texture
                .as_ref()
                .unwrap()
                .uv_set,
            "UVChannel_2"
        );
        let asset = load(&path).unwrap();
        let error = load_base_color_textures(&path, &asset.materials)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("requests UV set"),
            "unexpected error: {error}"
        );

        let path = directory.path().join("uv_transform.fbx");
        let transformed = add_base_color_texture(source, "missing.png", "UVChannel_1").replace(
            "            P: \"UVSet\", \"KString\", \"\", \"\",\"UVChannel_1\"",
            "            P: \"UVSet\", \"KString\", \"\", \"\",\"UVChannel_1\"\n            P: \"Translation\", \"Vector\", \"\", \"A\",0.25,0,0",
        );
        fs::write(&path, transformed).unwrap();
        let asset = load(&path).unwrap();
        let error = load_base_color_textures(&path, &asset.materials)
            .unwrap_err()
            .to_string();
        assert!(error.contains("UV transform"), "unexpected error: {error}");
    }

    fn add_second_material(source: String, name: &str) -> String {
        source
            .replace("    Count: 3", "    Count: 4")
            .replace(
                "    ObjectType: \"Material\" {\n        Count: 1",
                "    ObjectType: \"Material\" {\n        Count: 2",
            )
            .replace(
                "MappingInformationType: \"AllSame\"\n            ReferenceInformationType: \"IndexToDirect\"\n            Materials: *1 {\n                a: 0",
                "MappingInformationType: \"ByPolygon\"\n            ReferenceInformationType: \"IndexToDirect\"\n            Materials: *2 {\n                a: 0,1",
            )
            .replace(
                "    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {",
                &format!(
                    "    Material: 4000, \"Material::{name}\", \"\" {{\n        Version: 102\n        ShadingModel: \"phong\"\n        MultiLayer: 0\n    }}\n    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {{"
                ),
            )
            .replace(
                "    C: \"OO\",2000,0",
                "    C: \"OO\",4000,2000\n    C: \"OO\",2000,0",
            )
    }

    fn add_instance_with_material(source: String, name: &str) -> String {
        source
            .replace("    Count: 3", "    Count: 5")
            .replace(
                "    ObjectType: \"Model\" {\n        Count: 1",
                "    ObjectType: \"Model\" {\n        Count: 2",
            )
            .replace(
                "    ObjectType: \"Material\" {\n        Count: 1",
                "    ObjectType: \"Material\" {\n        Count: 2",
            )
            .replace(
                "    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {",
                &format!(
                    "    Material: 4000, \"Material::{name}\", \"\" {{\n        Version: 102\n        ShadingModel: \"phong\"\n        MultiLayer: 0\n    }}\n    Model: 2001, \"Model::InstanceNode\", \"Mesh\" {{\n        Version: 232\n        Properties70:  {{\n            P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",100,0,0\n            P: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,0\n            P: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1,1,1\n        }}\n        Shading: T\n        Culling: \"CullingOff\"\n    }}\n    Model: 2000, \"Model::TriangleNode\", \"Mesh\" {{"
                ),
            )
            .replace(
                "    C: \"OO\",2000,0",
                "    C: \"OO\",1000,2001\n    C: \"OO\",4000,2001\n    C: \"OO\",2001,0\n    C: \"OO\",2000,0",
            )
    }

    fn remove_element_block(mut source: String, start_marker: &str, end_marker: &str) -> String {
        let start = source.find(start_marker).unwrap();
        let end = source[start..].find(end_marker).unwrap() + start;
        source.replace_range(start..end, "");
        source
    }

    #[test]
    fn imports_ascii_triangle_with_uv_and_normal() {
        let imported = load(&fixture("triangle_ascii.fbx")).unwrap();

        assert_eq!(imported.mesh.positions.len(), 3);
        assert_eq!(imported.mesh.uvs.len(), 3);
        assert_eq!(imported.mesh.normals.len(), 3);
        assert_eq!(imported.mesh.indices.len(), 1);
        assert_eq!(imported.mesh.mesh_objects[0].name, "TriangleNode");
        assert_eq!(imported.materials[0].name, "Default Material");
        assert!(
            imported
                .mesh
                .positions
                .iter()
                .any(|position| (*position - glam::Vec3::new(0.01, 0.0, 0.0)).length() < 1e-6)
        );
    }

    #[test]
    fn triangulates_quad_and_converts_corner_attributes_to_single_index_vertices() {
        let imported = load_modified_fixture(|source| {
            source
                .replace(
                    "Vertices: *9 {\n            a: 0,0,0,1,0,0,0,1,0",
                    "Vertices: *12 {\n            a: 0,0,0,1,0,0,1,1,0,0,1,0",
                )
                .replace(
                    "PolygonVertexIndex: *3 {\n            a: 0,1,-3",
                    "PolygonVertexIndex: *4 {\n            a: 0,1,2,-4",
                )
                .replace(
                    "Normals: *9 {\n                a: 0,0,1,0,0,1,0,0,1",
                    "Normals: *12 {\n                a: 0,0,1,0,0,1,0,0,1,0,0,1",
                )
                .replace(
                    "UV: *6 {\n                a: 0,0,1,0,0,1",
                    "UV: *8 {\n                a: 0,0,1,0,1,1,0,1",
                )
                .replace(
                    "UVIndex: *3 {\n                a: 0,1,2",
                    "UVIndex: *4 {\n                a: 0,1,2,3",
                )
        })
        .unwrap();

        assert_eq!(imported.mesh.positions.len(), 4);
        assert_eq!(imported.mesh.indices.len(), 2);
        assert_eq!(imported.mesh.sub_meshes[0].wireframe_edges.len(), 5);
    }

    #[test]
    fn rejects_mesh_without_uv_coordinates_with_object_context() {
        let error = load_modified_fixture(|source| {
            remove_element_block(source, "        LayerElementUV: 0 {", "        Layer: 0 {")
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("mesh \"TriangleNode\" has no UV set"));
    }

    #[test]
    fn imports_node_material_name_and_source_identity() {
        let imported =
            load_modified_fixture(|source| add_single_material(source, "Body", "")).unwrap();

        assert_eq!(imported.materials.len(), 1);
        assert_eq!(imported.materials[0].name, "Body");
        assert_eq!(imported.materials[0].source_material_index, Some(0));
        assert_eq!(imported.mesh.sub_meshes[0].material_index, 0);
        assert_eq!(imported.mesh.sub_meshes[0].material_name, "Body");
    }

    #[test]
    fn maps_unified_opacity_and_double_sided_material_settings() {
        let imported = load_modified_fixture(|source| {
            add_single_material(
                source,
                "Glass",
                "            P: \"3dsMax|ClassIDa\", \"int\", \"Integer\", \"\",943849874\n            P: \"3dsMax|ClassIDb\", \"int\", \"Integer\", \"\",1174294043\n            P: \"3dsMax|main|Alpha\", \"double\", \"Number\", \"\",0.25\n            P: \"3dsMax|main|DoubleSided\", \"bool\", \"\", \"\",1",
            )
        })
        .unwrap();

        assert_eq!(
            imported.materials[0].render_settings.render_mode,
            crate::core::material::MaterialRenderMode::Blend
        );
        assert!(imported.materials[0].render_settings.double_sided);
    }

    #[test]
    fn maps_material_parts_to_sub_meshes() {
        let imported = load_modified_fixture(|source| {
            add_second_material(
                add_single_material(make_two_triangle_quad(source), "Body", ""),
                "Eyes",
            )
        })
        .unwrap();

        assert_eq!(
            imported
                .materials
                .iter()
                .map(|material| material.name.as_str())
                .collect::<Vec<_>>(),
            ["Body", "Eyes"]
        );
        assert_eq!(imported.mesh.sub_meshes.len(), 2);
        assert_eq!(imported.mesh.sub_meshes[0].material_index, 0);
        assert_eq!(imported.mesh.sub_meshes[1].material_index, 1);
        assert_eq!(imported.mesh.sub_meshes[0].index_count, 3);
        assert_eq!(imported.mesh.sub_meshes[1].index_count, 3);
    }

    #[test]
    fn flattens_instances_and_uses_each_nodes_material_assignment() {
        let imported = load_modified_fixture(|source| {
            add_instance_with_material(add_single_material(source, "Body", ""), "Eyes")
        })
        .unwrap();

        assert_eq!(imported.mesh.mesh_objects.len(), 2);
        assert_eq!(imported.mesh.indices.len(), 2);
        let mut material_names = imported
            .materials
            .iter()
            .map(|material| material.name.as_str())
            .collect::<Vec<_>>();
        material_names.sort_unstable();
        assert_eq!(material_names, ["Body", "Eyes"]);
        let mut sub_mesh_material_names = imported
            .mesh
            .sub_meshes
            .iter()
            .map(|sub_mesh| sub_mesh.material_name.as_str())
            .collect::<Vec<_>>();
        sub_mesh_material_names.sort_unstable();
        assert_eq!(sub_mesh_material_names, ["Body", "Eyes"]);
        assert!(
            imported
                .mesh
                .positions
                .iter()
                .any(|position| position.x >= 1.0)
        );
    }

    #[test]
    fn triangulates_ngon() {
        let imported = load_modified_fixture(|source| {
            source
                .replace(
                    "Vertices: *9 {\n            a: 0,0,0,1,0,0,0,1,0",
                    "Vertices: *15 {\n            a: 0,0,0,1,0,0,1.5,0.5,0,0.5,1.5,0,-0.5,0.5,0",
                )
                .replace(
                    "PolygonVertexIndex: *3 {\n            a: 0,1,-3",
                    "PolygonVertexIndex: *5 {\n            a: 0,1,2,3,-5",
                )
                .replace(
                    "Normals: *9 {\n                a: 0,0,1,0,0,1,0,0,1",
                    "Normals: *15 {\n                a: 0,0,1,0,0,1,0,0,1,0,0,1,0,0,1",
                )
                .replace(
                    "UV: *6 {\n                a: 0,0,1,0,0,1",
                    "UV: *10 {\n                a: 0,0,1,0,1,0.5,0.5,1,0,0.5",
                )
                .replace(
                    "UVIndex: *3 {\n                a: 0,1,2",
                    "UVIndex: *5 {\n                a: 0,1,2,3,4",
                )
        })
        .unwrap();

        assert_eq!(imported.mesh.indices.len(), 3);
        assert_eq!(imported.mesh.positions.len(), 5);
    }

    #[test]
    fn splits_vertices_when_corner_uv_or_normal_indices_differ() {
        let imported = load_modified_fixture(|source| {
            make_two_triangle_quad(source)
                .replace(
                    "a: 0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1",
                    "a: 0,0,1,0,0,1,0,0,1,0,1,0,0,0,1,0,0,1",
                )
                .replace(
                    "a: 0,0,1,0,1,1,0,0,1,1,0,1",
                    "a: 0,0,1,0,1,1,0.5,0.5,1,1,0,1",
                )
        })
        .unwrap();

        assert_eq!(imported.mesh.indices.len(), 2);
        assert_eq!(imported.mesh.positions.len(), 5);
    }

    #[test]
    fn generates_missing_normals() {
        let imported = load_modified_fixture(|source| {
            remove_element_block(
                source,
                "        LayerElementNormal: 0 {",
                "        LayerElementUV: 0 {",
            )
        })
        .unwrap();

        assert_eq!(imported.mesh.normals.len(), 3);
        assert!(
            imported
                .mesh
                .normals
                .iter()
                .all(|normal| normal.is_finite() && normal.length() > 0.99)
        );
    }

    #[test]
    fn preserves_front_face_for_negative_scale() {
        let imported = load_modified_fixture(|source| {
            source.replace(
                "P: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1,1,1",
                "P: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",-1,1,1",
            )
        })
        .unwrap();
        let triangle = imported.mesh.indices[0];
        let p0 = imported.mesh.positions[triangle[0] as usize];
        let p1 = imported.mesh.positions[triangle[1] as usize];
        let p2 = imported.mesh.positions[triangle[2] as usize];
        let geometric_normal = (p1 - p0).cross(p2 - p0).normalize();

        assert!(geometric_normal.dot(imported.mesh.normals[triangle[0] as usize]) > 0.99);
    }

    #[test]
    fn bakes_fbx_geometric_transform() {
        let imported = load_modified_fixture(|source| {
            source.replace(
                "P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,0,0",
                "P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,0,0\n            P: \"GeometricTranslation\", \"Vector3D\", \"Vector\", \"\",100,0,0",
            )
        })
        .unwrap();

        assert!(
            imported
                .mesh
                .positions
                .iter()
                .all(|position| position.x >= 1.0)
        );
    }

    #[test]
    fn transforms_normals_with_full_geometric_transform() {
        let imported = load_modified_fixture(|source| {
            source
                .replace(
                    "a: 0,0,0,1,0,0,0,1,0",
                    "a: 0,0,0,1,0,0,0,1,1",
                )
                .replace(
                    "a: 0,0,1,0,0,1,0,0,1",
                    "a: 0,-0.70710678,0.70710678,0,-0.70710678,0.70710678,0,-0.70710678,0.70710678",
                )
                .replace(
                    "P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,0,0",
                    "P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,0,0\n            P: \"GeometricScaling\", \"Vector3D\", \"Vector\", \"\",2,3,4",
                )
        })
        .unwrap();
        let triangle = imported.mesh.indices[0];
        let p0 = imported.mesh.positions[triangle[0] as usize];
        let p1 = imported.mesh.positions[triangle[1] as usize];
        let p2 = imported.mesh.positions[triangle[2] as usize];
        let geometric_normal = (p1 - p0).cross(p2 - p0).normalize();

        for index in triangle {
            assert!(geometric_normal.dot(imported.mesh.normals[index as usize]) > 0.999);
        }
    }

    #[test]
    fn converts_z_up_centimeter_scene_to_y_up_meters() {
        let imported = load_modified_fixture(|source| {
            source
                .replace(
                    "P: \"UpAxis\", \"int\", \"Integer\", \"\",1",
                    "P: \"UpAxis\", \"int\", \"Integer\", \"\",2",
                )
                .replace(
                    "P: \"FrontAxis\", \"int\", \"Integer\", \"\",2",
                    "P: \"FrontAxis\", \"int\", \"Integer\", \"\",1",
                )
        })
        .unwrap();

        assert!(
            imported
                .mesh
                .positions
                .iter()
                .all(|position| position.length() <= 0.011)
        );
        assert!(
            imported
                .mesh
                .normals
                .iter()
                .all(|normal| normal.y.abs() > 0.99)
        );
    }

    #[test]
    fn rejects_line_geometry_with_object_context() {
        let error = load_modified_fixture(|source| {
            source
                .replace(
                    "PolygonVertexIndex: *3 {\n            a: 0,1,-3",
                    "PolygonVertexIndex: *2 {\n            a: 0,-2",
                )
                .replace(
                    "Normals: *9 {\n                a: 0,0,1,0,0,1,0,0,1",
                    "Normals: *6 {\n                a: 0,0,1,0,0,1",
                )
                .replace(
                    "UV: *6 {\n                a: 0,0,1,0,0,1",
                    "UV: *4 {\n                a: 0,0,1,0",
                )
                .replace(
                    "UVIndex: *3 {\n                a: 0,1,2",
                    "UVIndex: *2 {\n                a: 0,1",
                )
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("mesh \"TriangleNode\" contains point or line geometry"));
    }

    #[test]
    fn rejects_point_geometry_with_object_context() {
        let error = load_modified_fixture(|source| {
            source
                .replace(
                    "PolygonVertexIndex: *3 {\n            a: 0,1,-3",
                    "PolygonVertexIndex: *1 {\n            a: -1",
                )
                .replace(
                    "Normals: *9 {\n                a: 0,0,1,0,0,1,0,0,1",
                    "Normals: *3 {\n                a: 0,0,1",
                )
                .replace(
                    "UV: *6 {\n                a: 0,0,1,0,0,1",
                    "UV: *2 {\n                a: 0,0",
                )
                .replace(
                    "UVIndex: *3 {\n                a: 0,1,2",
                    "UVIndex: *1 {\n                a: 0",
                )
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("mesh \"TriangleNode\" contains point or line geometry"));
    }
}
