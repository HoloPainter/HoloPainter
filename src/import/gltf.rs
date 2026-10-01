use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::{
    core::document::{MeshData, MeshId, MeshObject, SubMesh},
    core::material::{MaterialRenderMode, MaterialRenderSettings},
};

use super::{
    ImportedAsset, ImportedMaterial, ImportedTexture, build_wireframe_edges,
    decode_imported_texture,
};

struct ImportedMeshInstance<'a> {
    mesh: gltf::Mesh<'a>,
    transform: Mat4,
    name: Option<String>,
}

fn traverse_nodes<'a>(
    node: gltf::Node<'a>,
    parent_transform: Mat4,
    items: &mut Vec<ImportedMeshInstance<'a>>,
) {
    let local_transform = Mat4::from_cols_array_2d(&node.transform().matrix());
    let global_transform = parent_transform * local_transform;

    if let Some(mesh) = node.mesh() {
        let name = node.name().or_else(|| mesh.name()).map(str::to_owned);
        items.push(ImportedMeshInstance {
            mesh,
            transform: global_transform,
            name,
        });
    }

    for child in node.children() {
        traverse_nodes(child, global_transform, items);
    }
}

pub(super) fn load(path: &Path) -> Result<ImportedAsset> {
    let gltf = gltf::Gltf::open(path)
        .with_context(|| format!("failed to open glTF file: {}", path.display()))?;
    let gltf::Gltf { document, blob } = gltf;
    let buffers = gltf::import_buffers(&document, path.parent(), blob)
        .with_context(|| format!("failed to import glTF buffers: {}", path.display()))?;

    load_document(document, buffers)
}

pub(super) fn load_bytes(bytes: &[u8]) -> Result<ImportedAsset> {
    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(bytes)?;
    let buffers = gltf::import_buffers(&document, None, blob)?;
    load_document(document, buffers)
}

fn load_document(
    document: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
) -> Result<ImportedAsset> {
    let mut items = Vec::new();
    for scene in document.scenes() {
        for node in scene.nodes() {
            traverse_nodes(node, Mat4::IDENTITY, &mut items);
        }
    }

    if items.is_empty() {
        bail!("glTF has no mesh");
    }

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut sub_meshes = Vec::new();
    let mut mesh_objects = Vec::with_capacity(items.len());
    let mut triangle_mesh_ids = Vec::new();
    let mut materials = Vec::new();

    let mut material_map = std::collections::HashMap::new();

    for (mesh_index, item) in items.into_iter().enumerate() {
        let mesh_id = MeshId(mesh_index);
        mesh_objects.push(MeshObject {
            id: mesh_id,
            name: item.name.unwrap_or_else(|| format!("Mesh {mesh_index}")),
        });
        let normal_transform = item.transform.inverse().transpose();
        let reverses_winding = Mat3::from_mat4(item.transform).determinant() < 0.0;

        for primitive in item.mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                bail!(
                    "unsupported glTF primitive mode {:?}; only TRIANGLES is supported",
                    primitive.mode()
                );
            }
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let mut local_positions: Vec<Vec3> = reader
                .read_positions()
                .context("missing POSITION attribute")?
                .map(|p| item.transform.transform_point3(Vec3::new(p[0], p[1], p[2])))
                .collect();

            let mut local_uvs: Vec<Vec2> = reader
                .read_tex_coords(0)
                .context("missing TEXCOORD_0 attribute")?
                .into_f32()
                .map(|uv| Vec2::new(uv[0], uv[1]))
                .collect();

            let mut raw_indices: Vec<u32> = reader
                .read_indices()
                .context("missing indices")?
                .into_u32()
                .collect();

            if raw_indices.len() % 3 != 0 {
                bail!("indices are not triangles");
            }
            if reverses_winding {
                for triangle in raw_indices.chunks_exact_mut(3) {
                    triangle.swap(1, 2);
                }
            }

            let mut local_normals: Vec<Vec3> = if let Some(ns) = reader.read_normals() {
                ns.map(|n| {
                    normal_transform
                        .transform_vector3(Vec3::new(n[0], n[1], n[2]))
                        .normalize_or_zero()
                })
                .collect()
            } else {
                let mut accum = vec![Vec3::ZERO; local_positions.len()];
                for tri in raw_indices.chunks_exact(3) {
                    let i0 = tri[0] as usize;
                    let i1 = tri[1] as usize;
                    let i2 = tri[2] as usize;
                    if i0 >= local_positions.len()
                        || i1 >= local_positions.len()
                        || i2 >= local_positions.len()
                    {
                        bail!("triangle index out of range");
                    }
                    let p0 = local_positions[i0];
                    let p1 = local_positions[i1];
                    let p2 = local_positions[i2];
                    let face_n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
                    accum[i0] += face_n;
                    accum[i1] += face_n;
                    accum[i2] += face_n;
                }
                accum.into_iter().map(|n| n.normalize_or_zero()).collect()
            };

            let start_index = (indices.len() * 3) as u32;
            let index_count = raw_indices.len() as u32;

            let vertex_offset = positions.len() as u32;
            let triangle_start = indices.len();
            for c in raw_indices.chunks_exact(3) {
                let i0 = c[0] + vertex_offset;
                let i1 = c[1] + vertex_offset;
                let i2 = c[2] + vertex_offset;
                indices.push([i0, i1, i2]);
                triangle_mesh_ids.push(mesh_id);
            }
            let wireframe_edges = build_wireframe_edges(&indices[triangle_start..]);

            positions.append(&mut local_positions);
            uvs.append(&mut local_uvs);
            normals.append(&mut local_normals);
            let gltf_mat = primitive.material();
            let gltf_mat_id = gltf_mat.index().unwrap_or(usize::MAX);
            let material_name = gltf_mat
                .name()
                .unwrap_or_else(|| {
                    if gltf_mat_id == usize::MAX {
                        "Default Material"
                    } else {
                        "Unnamed Material"
                    }
                })
                .to_string();
            let material_index = *material_map.entry(gltf_mat_id).or_insert_with(|| {
                let idx = materials.len();
                materials.push(ImportedMaterial {
                    source_material_index: gltf_mat.index(),
                    name: material_name.clone(),
                    render_settings: gltf_material_render_settings(&gltf_mat),
                });
                idx
            });

            sub_meshes.push(SubMesh {
                mesh_id,
                start_index,
                index_count,
                material_index,
                wireframe_edges,
                material_name,
            });
        }
    }

    let mesh = MeshData::new(
        positions,
        uvs,
        normals,
        indices,
        sub_meshes,
        mesh_objects,
        triangle_mesh_ids,
    )?;
    Ok(ImportedAsset { mesh, materials })
}

pub(super) fn load_base_color_textures(
    path: &Path,
    materials: &[ImportedMaterial],
) -> Result<Vec<Option<ImportedTexture>>> {
    let gltf = gltf::Gltf::open(path)
        .with_context(|| format!("failed to open glTF file: {}", path.display()))?;
    let gltf::Gltf { document, blob } = gltf;
    let buffers = gltf::import_buffers(&document, path.parent(), blob)
        .with_context(|| format!("failed to import glTF buffers: {}", path.display()))?;
    let source_materials = document.materials().collect::<Vec<_>>();
    let mut decoded_images = HashMap::<usize, ImportedTexture>::new();
    let mut result = Vec::with_capacity(materials.len());

    for material in materials {
        let Some(source_material_index) = material.source_material_index else {
            result.push(None);
            continue;
        };
        let source_material = source_materials
            .get(source_material_index)
            .with_context(|| {
                format!(
                    "Base Color Texture import failed for material {:?}: source material {} no longer exists",
                    material.name, source_material_index
                )
            })?;
        let Some(info) = source_material
            .pbr_metallic_roughness()
            .base_color_texture()
        else {
            result.push(None);
            continue;
        };
        ensure!(
            info.tex_coord() == 0,
            "Base Color Texture import failed for material {:?}: uses TEXCOORD_{}; only TEXCOORD_0 is supported",
            material.name,
            info.tex_coord()
        );
        ensure!(
            info.texture_transform().is_none(),
            "Base Color Texture import failed for material {:?}: unsupported KHR_texture_transform",
            material.name
        );

        let image = info.texture().source();
        let image_index = image.index();
        if !decoded_images.contains_key(&image_index) {
            let texture = decode_gltf_image(path, image.source(), &buffers).with_context(|| {
                format!(
                    "Base Color Texture import failed for material {:?}",
                    material.name
                )
            })?;
            decoded_images.insert(image_index, texture);
        }
        result.push(decoded_images.get(&image_index).cloned());
    }

    Ok(result)
}

fn decode_gltf_image(
    gltf_path: &Path,
    source: gltf::image::Source<'_>,
    buffers: &[gltf::buffer::Data],
) -> Result<ImportedTexture> {
    let encoded = match source {
        gltf::image::Source::View { view, mime_type } => {
            ensure_supported_image_mime(mime_type)?;
            let buffer = buffers
                .get(view.buffer().index())
                .context("image buffer does not exist")?;
            let end = view
                .offset()
                .checked_add(view.length())
                .context("image buffer view range overflow")?;
            buffer
                .get(view.offset()..end)
                .context("image buffer view is out of range")?
                .to_vec()
        }
        gltf::image::Source::Uri { uri, mime_type } => {
            if let Some(data) = uri.strip_prefix("data:") {
                decode_image_data_uri(data, mime_type)?
            } else {
                ensure!(
                    !has_uri_scheme(uri),
                    "remote image URI is unsupported: {uri}"
                );
                if let Some(mime_type) = mime_type {
                    ensure_supported_image_mime(mime_type)?;
                }
                let decoded_uri = urlencoding::decode(uri)
                    .with_context(|| format!("invalid image URI: {uri}"))?;
                let image_path = gltf_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(decoded_uri.as_ref());
                std::fs::read(&image_path)
                    .with_context(|| format!("failed to read image: {}", image_path.display()))?
            }
        }
    };

    decode_imported_texture(&encoded)
}

fn decode_image_data_uri(data: &str, declared_mime_type: Option<&str>) -> Result<Vec<u8>> {
    let (metadata, payload) = data.split_once(',').context("invalid image data URI")?;
    let mut parts = metadata.split(';');
    let mime_type = parts.next().unwrap_or_default();
    ensure_supported_image_mime(mime_type)?;
    if let Some(declared_mime_type) = declared_mime_type {
        ensure_supported_image_mime(declared_mime_type)?;
    }
    ensure!(
        parts.any(|part| part.eq_ignore_ascii_case("base64")),
        "image data URI must use base64 encoding"
    );
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("invalid base64 image data")
}

fn ensure_supported_image_mime(mime_type: &str) -> Result<()> {
    ensure!(
        matches!(mime_type, "image/png" | "image/jpeg"),
        "unsupported image MIME type: {mime_type}; only image/png and image/jpeg are supported"
    );
    Ok(())
}

fn has_uri_scheme(uri: &str) -> bool {
    let Some(colon) = uri.find(':') else {
        return false;
    };
    let prefix = &uri[..colon];
    !prefix.is_empty()
        && prefix.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' => true,
            b'0'..=b'9' | b'+' | b'-' | b'.' => index > 0,
            _ => false,
        })
}

fn gltf_material_render_settings(material: &gltf::Material<'_>) -> MaterialRenderSettings {
    let render_mode = match material.alpha_mode() {
        gltf::material::AlphaMode::Opaque => MaterialRenderMode::Opaque,
        gltf::material::AlphaMode::Mask => MaterialRenderMode::Cutoff,
        gltf::material::AlphaMode::Blend => MaterialRenderMode::Blend,
    };
    let alpha_cutoff = if render_mode == MaterialRenderMode::Cutoff {
        normalized_alpha_cutoff_u8(material.alpha_cutoff().unwrap_or(0.5))
    } else {
        MaterialRenderSettings::default().alpha_cutoff
    };
    MaterialRenderSettings {
        double_sided: material.double_sided(),
        render_mode,
        alpha_cutoff,
    }
}

fn normalized_alpha_cutoff_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use base64::Engine;
    use image::{ExtendedColorType, ImageEncoder};

    use crate::core::{document::MeshId, material::MaterialRenderSettings};

    use crate::import::ImportedMaterial;

    use super::{load, load_base_color_textures};

    fn imported_material(index: Option<usize>, name: &str) -> ImportedMaterial {
        ImportedMaterial {
            source_material_index: index,
            name: name.to_owned(),
            render_settings: MaterialRenderSettings::default(),
        }
    }

    fn png_bytes(size: [u32; 2], rgba8: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(rgba8, size[0], size[1], ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn jpeg_bytes(size: [u32; 2], rgb8: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 100)
            .encode(rgb8, size[0], size[1], ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn write_texture_gltf(directory: &Path, body: &str) -> std::path::PathBuf {
        let path = directory.join("textures.gltf");
        fs::write(&path, format!(r#"{{"asset":{{"version":"2.0"}},{body}}}"#)).unwrap();
        path
    }

    fn write_glb_with_png(directory: &Path, png: &[u8]) -> std::path::PathBuf {
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],"images":[{{"bufferView":0,"mimeType":"image/png"}}],"textures":[{{"source":0}}],"materials":[{{"name":"Embedded","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}]}}"#,
            png.len(),
            png.len()
        );
        let mut json_chunk = json.into_bytes();
        while !json_chunk.len().is_multiple_of(4) {
            json_chunk.push(b' ');
        }
        let mut bin_chunk = png.to_vec();
        while !bin_chunk.len().is_multiple_of(4) {
            bin_chunk.push(0);
        }
        let total_length = 12 + 8 + json_chunk.len() + 8 + bin_chunk.len();
        let mut glb = Vec::with_capacity(total_length);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(total_length as u32).to_le_bytes());
        glb.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4e4f534a_u32.to_le_bytes());
        glb.extend_from_slice(&json_chunk);
        glb.extend_from_slice(&(bin_chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004e4942_u32.to_le_bytes());
        glb.extend_from_slice(&bin_chunk);
        let path = directory.join("embedded.glb");
        fs::write(&path, glb).unwrap();
        path
    }

    #[test]
    fn alpha_cutoff_conversion_clamps_and_rounds_to_u8() {
        assert_eq!(super::normalized_alpha_cutoff_u8(-1.0), 0);
        assert_eq!(super::normalized_alpha_cutoff_u8(0.5), 128);
        assert_eq!(super::normalized_alpha_cutoff_u8(0.25), 64);
        assert_eq!(super::normalized_alpha_cutoff_u8(2.0), 255);
    }

    #[test]
    fn load_gltf_rejects_non_triangle_primitive_even_when_index_count_is_divisible_by_three() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "holopainter_import_primitive_mode_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&directory).unwrap();

        let mut buffer = Vec::new();
        for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for index in [0_u16, 1, 1, 2, 2, 0] {
            buffer.extend_from_slice(&index.to_le_bytes());
        }
        assert_eq!(buffer.len(), 72);
        fs::write(directory.join("mesh.bin"), buffer).unwrap();

        let gltf = r#"{
  "asset": { "version": "2.0" },
  "buffers": [{ "uri": "mesh.bin", "byteLength": 72 }],
  "bufferViews": [
    { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
    { "buffer": 0, "byteOffset": 36, "byteLength": 24 },
    { "buffer": 0, "byteOffset": 60, "byteLength": 12 }
  ],
  "accessors": [
    {
      "bufferView": 0,
      "componentType": 5126,
      "count": 3,
      "type": "VEC3",
      "min": [0.0, 0.0, 0.0],
      "max": [1.0, 1.0, 0.0]
    },
    { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" },
    { "bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR" }
  ],
  "meshes": [{
    "primitives": [{
      "attributes": { "POSITION": 0, "TEXCOORD_0": 1 },
      "indices": 2,
      "mode": 1
    }]
  }],
  "nodes": [{ "mesh": 0 }],
  "scenes": [{ "nodes": [0] }],
  "scene": 0
}"#;
        let gltf_path = directory.join("lines.gltf");
        fs::write(&gltf_path, gltf).unwrap();

        let error = load(&gltf_path).unwrap_err().to_string();
        let _ = fs::remove_dir_all(&directory);

        assert!(error.contains("unsupported glTF primitive mode Lines"));
        assert!(error.contains("only TRIANGLES is supported"));
    }

    #[test]
    fn load_gltf_preserves_mesh_data_without_loading_source_images() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "holopainter_import_mesh_identity_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&directory).unwrap();

        let mut buffer = Vec::new();
        for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for index in [0_u16, 1, 2] {
            buffer.extend_from_slice(&index.to_le_bytes());
        }
        assert_eq!(buffer.len(), 66);
        fs::write(directory.join("mesh.bin"), buffer).unwrap();

        let gltf = r#"{
  "asset": { "version": "2.0" },
  "buffers": [
    { "uri": "mesh.bin", "byteLength": 66 }
  ],
  "bufferViews": [
    { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
    { "buffer": 0, "byteOffset": 36, "byteLength": 24 },
    { "buffer": 0, "byteOffset": 60, "byteLength": 6 }
  ],
  "accessors": [
    {
      "bufferView": 0,
      "componentType": 5126,
      "count": 3,
      "type": "VEC3",
      "min": [0.0, 0.0, 0.0],
      "max": [1.0, 1.0, 0.0]
    },
    {
      "bufferView": 1,
      "componentType": 5126,
      "count": 3,
      "type": "VEC2"
    },
    {
      "bufferView": 2,
      "componentType": 5123,
      "count": 3,
      "type": "SCALAR"
    }
  ],
  "materials": [
    {
      "name": "Two Sided",
      "doubleSided": true,
      "alphaMode": "MASK",
      "alphaCutoff": 0.25,
      "pbrMetallicRoughness": {
        "baseColorTexture": { "index": 0 }
      }
    }
  ],
  "images": [
    { "uri": "missing-source-texture.png" }
  ],
  "textures": [
    { "source": 0 }
  ],
  "meshes": [
    {
      "name": "Shared Mesh",
      "primitives": [
        {
          "attributes": { "POSITION": 0, "TEXCOORD_0": 1 },
          "indices": 2,
          "material": 0
        }
      ]
    },
    {
      "primitives": [
        {
          "attributes": { "POSITION": 0, "TEXCOORD_0": 1 },
          "indices": 2,
          "material": 0
        }
      ]
    }
  ],
  "nodes": [
    { "name": "Named Node", "mesh": 0 },
    { "mesh": 0, "translation": [2.0, 0.0, 0.0] },
    { "mesh": 1, "translation": [4.0, 0.0, 0.0], "scale": [-1.0, 1.0, 1.0] }
  ],
  "scenes": [
    { "nodes": [0, 1, 2] }
  ],
  "scene": 0
}"#;
        let gltf_path = directory.join("mesh.gltf");
        fs::write(&gltf_path, gltf).unwrap();
        assert!(!directory.join("missing-source-texture.png").exists());

        let imported = load(&gltf_path);
        let _ = fs::remove_dir_all(&directory);
        let imported = imported.unwrap();

        let names = imported
            .mesh
            .mesh_objects
            .iter()
            .map(|mesh| mesh.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Named Node", "Shared Mesh", "Mesh 2"]);
        assert_eq!(
            imported.mesh.triangle_mesh_ids.as_ref(),
            &vec![MeshId(0), MeshId(1), MeshId(2)]
        );
        assert_eq!(
            imported
                .mesh
                .sub_meshes
                .iter()
                .map(|sub_mesh| sub_mesh.mesh_id)
                .collect::<Vec<_>>(),
            vec![MeshId(0), MeshId(1), MeshId(2)]
        );
        assert_eq!(imported.mesh.positions[3].x, 2.0);
        assert_eq!(imported.mesh.positions[6].x, 4.0);
        assert_eq!(imported.mesh.indices[0], [0, 1, 2]);
        assert_eq!(imported.mesh.indices[2], [6, 8, 7]);
        assert!(imported.materials[0].render_settings.double_sided);
        assert_eq!(
            imported.materials[0].render_settings.render_mode,
            crate::core::material::MaterialRenderMode::Cutoff
        );
        assert_eq!(imported.materials[0].render_settings.alpha_cutoff, 64);
        assert_eq!(imported.materials[0].source_material_index, Some(0));
    }

    #[test]
    fn base_color_loader_imports_external_png_and_shares_it_by_image_index() {
        let directory = tempfile::tempdir().unwrap();
        let rgba8 = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 0, 0,
        ];
        fs::write(directory.path().join("body.png"), png_bytes([2, 2], &rgba8)).unwrap();
        let path = write_texture_gltf(
            directory.path(),
            r#""images":[{"uri":"body.png"}],"textures":[{"source":0}],"materials":[{"name":"A","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}},{"name":"B","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}},{"name":"White"}]"#,
        );
        let materials = vec![
            imported_material(Some(0), "A"),
            imported_material(Some(1), "B"),
            imported_material(Some(2), "White"),
            imported_material(None, "Default Material"),
        ];

        let textures = load_base_color_textures(&path, &materials).unwrap();

        assert_eq!(textures[0].as_ref().unwrap().size, [2, 2]);
        assert_eq!(textures[0].as_ref().unwrap().rgba8, rgba8);
        assert_eq!(textures[1], textures[0]);
        assert_eq!(textures[2], None);
        assert_eq!(textures[3], None);
    }

    #[test]
    fn base_color_loader_imports_external_jpeg_and_data_uri_png() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("photo.jpg"),
            jpeg_bytes([1, 1], &[24, 80, 160]),
        )
        .unwrap();
        let data_rgba = [1, 2, 3, 4];
        let data_uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png_bytes([1, 1], &data_rgba))
        );
        let body = format!(
            r#""images":[{{"uri":"photo.jpg"}},{{"uri":"{data_uri}"}}],"textures":[{{"source":0}},{{"source":1}}],"materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}},{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":1}}}}}}]"#
        );
        let path = write_texture_gltf(directory.path(), &body);

        let textures = load_base_color_textures(
            &path,
            &[
                imported_material(Some(0), "JPEG"),
                imported_material(Some(1), "Data URI"),
            ],
        )
        .unwrap();

        let jpeg = textures[0].as_ref().unwrap();
        assert_eq!(jpeg.size, [1, 1]);
        assert_eq!(jpeg.rgba8[3], 255);
        assert_eq!(textures[1].as_ref().unwrap().rgba8, data_rgba);
    }

    #[test]
    fn base_color_loader_imports_glb_buffer_view_without_flipping() {
        let directory = tempfile::tempdir().unwrap();
        let rgba8 = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let path = write_glb_with_png(directory.path(), &png_bytes([1, 2], &rgba8));

        let textures =
            load_base_color_textures(&path, &[imported_material(Some(0), "Embedded")]).unwrap();

        assert_eq!(textures[0].as_ref().unwrap().size, [1, 2]);
        assert_eq!(textures[0].as_ref().unwrap().rgba8, rgba8);
    }

    #[test]
    fn base_color_loader_rejects_unsupported_mapping_and_broken_sources() {
        let directory = tempfile::tempdir().unwrap();
        let texcoord_path = write_texture_gltf(
            directory.path(),
            r#""images":[{"uri":"missing.png"}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0,"texCoord":1}}}]"#,
        );
        let error = load_base_color_textures(&texcoord_path, &[imported_material(Some(0), "UV1")])
            .unwrap_err()
            .to_string();
        assert!(error.contains("TEXCOORD_1"));

        let transform_path = write_texture_gltf(
            directory.path(),
            r#""extensionsUsed":["KHR_texture_transform"],"images":[{"uri":"missing.png"}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0,"extensions":{"KHR_texture_transform":{"offset":[0.5,0.0]}}}}}]"#,
        );
        let error = load_base_color_textures(
            &transform_path,
            &[imported_material(Some(0), "Transformed")],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("KHR_texture_transform"));

        let missing_path = write_texture_gltf(
            directory.path(),
            r#""images":[{"uri":"missing.png"}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]"#,
        );
        let error =
            load_base_color_textures(&missing_path, &[imported_material(Some(0), "Missing")])
                .unwrap_err()
                .to_string();
        assert!(error.contains("Base Color Texture import failed"));

        let remote_path = write_texture_gltf(
            directory.path(),
            r#""images":[{"uri":"https://example.com/body.png"}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]"#,
        );
        let error = load_base_color_textures(&remote_path, &[imported_material(Some(0), "Remote")])
            .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("remote image URI"));
    }
}
