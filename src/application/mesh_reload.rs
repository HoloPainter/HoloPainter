use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, ensure};

use crate::{
    core::{
        document::{Document, MeshData, MeshId},
        material::{MaterialId, MaterialRenderSettings, MaterialSpec},
    },
    import::ImportedAsset,
};

use super::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshReloadMaterialTarget {
    Existing(MaterialId),
    CreateNew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshReloadMatchConfidence {
    High,
    Medium,
    Low,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshReloadMaterialMatch {
    pub imported_material_index: usize,
    pub target: MeshReloadMaterialTarget,
    pub confidence: MeshReloadMatchConfidence,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshReloadMaterialBinding {
    pub target: MeshReloadMaterialTarget,
    pub new_material: Option<MaterialSpec>,
}

#[derive(Debug, Clone)]
pub struct MeshReloadRequest {
    pub mesh: MeshData,
    pub new_materials: Vec<MaterialSpec>,
}

#[derive(Debug, Clone)]
struct MaterialSignature {
    name: String,
    normalized_name: String,
    name_tokens: HashSet<String>,
    object_names: HashSet<String>,
    primitive_count: usize,
    triangle_count: usize,
    surface_area_world: f32,
    uv_area: f32,
    render_settings: MaterialRenderSettings,
    original_index: usize,
}

#[derive(Debug, Clone)]
struct MatchPair {
    imported_index: usize,
    existing_index: usize,
    score: f32,
    name_similarity: f32,
    object_similarity: f32,
    geometry_similarity: f32,
}

pub fn build_mesh_reload_matches(
    document: &Document,
    imported: &ImportedAsset,
) -> Vec<MeshReloadMaterialMatch> {
    let existing_signatures = build_existing_signatures(document);
    let imported_signatures = build_imported_signatures(imported);
    let mut result = vec![None; imported.materials.len()];
    let mut used_existing = HashSet::new();

    assign_unique_name_matches(
        document,
        &existing_signatures,
        &imported_signatures,
        &mut result,
        &mut used_existing,
        |signature| signature.name.clone(),
        MeshReloadMatchConfidence::High,
        "Exact name",
    );
    assign_unique_name_matches(
        document,
        &existing_signatures,
        &imported_signatures,
        &mut result,
        &mut used_existing,
        |signature| signature.normalized_name.clone(),
        MeshReloadMatchConfidence::High,
        "Normalized name",
    );

    let mut pairs = Vec::new();
    for imported_signature in &imported_signatures {
        if result[imported_signature.original_index].is_some() {
            continue;
        }
        for existing_signature in &existing_signatures {
            if used_existing.contains(&existing_signature.original_index) {
                continue;
            }
            pairs.push(score_pair(imported_signature, existing_signature));
        }
    }
    pairs.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.imported_index.cmp(&right.imported_index))
            .then_with(|| left.existing_index.cmp(&right.existing_index))
    });

    let mut used_imported = result
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.is_some().then_some(index))
        .collect::<HashSet<_>>();
    for pair in pairs {
        if pair.score < 0.38
            || used_imported.contains(&pair.imported_index)
            || used_existing.contains(&pair.existing_index)
        {
            continue;
        }
        let confidence = if pair.score >= 0.72 {
            MeshReloadMatchConfidence::Medium
        } else {
            MeshReloadMatchConfidence::Low
        };
        let reason = pair_reason(&pair);
        result[pair.imported_index] = Some(MeshReloadMaterialMatch {
            imported_material_index: pair.imported_index,
            target: MeshReloadMaterialTarget::Existing(document.materials[pair.existing_index].id),
            confidence,
            reason,
        });
        used_imported.insert(pair.imported_index);
        used_existing.insert(pair.existing_index);
    }

    result
        .into_iter()
        .enumerate()
        .map(|(imported_material_index, matched)| {
            matched.unwrap_or_else(|| MeshReloadMaterialMatch {
                imported_material_index,
                target: MeshReloadMaterialTarget::CreateNew,
                confidence: MeshReloadMatchConfidence::New,
                reason: "No reliable existing material match".to_owned(),
            })
        })
        .collect()
}

pub fn create_mesh_reload_command(
    document: &Document,
    imported: &ImportedAsset,
    bindings: &[MeshReloadMaterialBinding],
    max_texture_dimension_2d: u32,
) -> Result<Command> {
    ensure!(
        bindings.len() == imported.materials.len(),
        "mesh reload material mapping count does not match imported materials"
    );
    ensure!(
        !imported.materials.is_empty(),
        "source mesh does not contain any materials"
    );

    let mut final_indices = Vec::with_capacity(bindings.len());
    let mut new_materials = Vec::new();
    for (imported_index, binding) in bindings.iter().enumerate() {
        match binding.target {
            MeshReloadMaterialTarget::Existing(material_id) => {
                let material_index = document.material_index(material_id).ok_or_else(|| {
                    anyhow!(
                        "mapped project material no longer exists for imported material {}",
                        imported.materials[imported_index].name
                    )
                })?;
                final_indices.push(material_index.as_usize());
            }
            MeshReloadMaterialTarget::CreateNew => {
                let material = binding.new_material.clone().ok_or_else(|| {
                    anyhow!(
                        "new material settings are missing for imported material {}",
                        imported.materials[imported_index].name
                    )
                })?;
                ensure!(
                    material.texture_size[0] > 0 && material.texture_size[1] > 0,
                    "{} has an invalid texture size",
                    material.name
                );
                ensure!(
                    material.texture_size[0] <= max_texture_dimension_2d
                        && material.texture_size[1] <= max_texture_dimension_2d,
                    "{} texture size {}x{} exceeds GPU texture limit {}",
                    material.name,
                    material.texture_size[0],
                    material.texture_size[1],
                    max_texture_dimension_2d
                );
                let material_index = document
                    .materials
                    .len()
                    .checked_add(new_materials.len())
                    .ok_or_else(|| anyhow!("material count overflows usize"))?;
                final_indices.push(material_index);
                new_materials.push(material);
            }
        }
    }

    let mut mesh = imported.mesh.clone();
    for sub_mesh in std::sync::Arc::make_mut(&mut mesh.sub_meshes) {
        let imported_index = sub_mesh.material_index;
        let Some(&material_index) = final_indices.get(imported_index) else {
            return Err(anyhow!(
                "reloaded mesh sub-mesh references missing imported material index {imported_index}"
            ));
        };
        sub_mesh.material_index = material_index;
        sub_mesh.material_name = document
            .materials
            .get(material_index)
            .map(|material| material.name.clone())
            .or_else(|| {
                new_materials
                    .get(material_index.saturating_sub(document.materials.len()))
                    .map(|material| material.name.clone())
            })
            .unwrap_or_else(|| sub_mesh.material_name.clone());
    }

    Ok(Command::ReloadMesh {
        request: MeshReloadRequest {
            mesh,
            new_materials,
        },
    })
}

fn assign_unique_name_matches(
    document: &Document,
    existing: &[MaterialSignature],
    imported: &[MaterialSignature],
    result: &mut [Option<MeshReloadMaterialMatch>],
    used_existing: &mut HashSet<usize>,
    key: impl Fn(&MaterialSignature) -> String,
    confidence: MeshReloadMatchConfidence,
    reason: &str,
) {
    let mut existing_by_key = HashMap::<String, Vec<usize>>::new();
    for signature in existing {
        if used_existing.contains(&signature.original_index) {
            continue;
        }
        existing_by_key
            .entry(key(signature))
            .or_default()
            .push(signature.original_index);
    }
    let mut imported_by_key = HashMap::<String, Vec<usize>>::new();
    for signature in imported {
        if result[signature.original_index].is_some() {
            continue;
        }
        imported_by_key
            .entry(key(signature))
            .or_default()
            .push(signature.original_index);
    }
    for (name, imported_indices) in imported_by_key {
        if name.is_empty() || imported_indices.len() != 1 {
            continue;
        }
        let Some(existing_indices) = existing_by_key.get(&name) else {
            continue;
        };
        if existing_indices.len() != 1 {
            continue;
        }
        let imported_index = imported_indices[0];
        let existing_index = existing_indices[0];
        result[imported_index] = Some(MeshReloadMaterialMatch {
            imported_material_index: imported_index,
            target: MeshReloadMaterialTarget::Existing(document.materials[existing_index].id),
            confidence,
            reason: reason.to_owned(),
        });
        used_existing.insert(existing_index);
    }
}

fn build_existing_signatures(document: &Document) -> Vec<MaterialSignature> {
    document
        .materials
        .iter()
        .enumerate()
        .map(|(index, material)| {
            material_signature(
                &document.mesh,
                index,
                &material.name,
                material.render_settings,
            )
        })
        .collect()
}

fn build_imported_signatures(imported: &ImportedAsset) -> Vec<MaterialSignature> {
    imported
        .materials
        .iter()
        .enumerate()
        .map(|(index, material)| {
            material_signature(
                &imported.mesh,
                index,
                &material.name,
                material.render_settings,
            )
        })
        .collect()
}

fn material_signature(
    mesh: &MeshData,
    material_index: usize,
    name: &str,
    render_settings: MaterialRenderSettings,
) -> MaterialSignature {
    let object_name_by_id = mesh
        .mesh_objects
        .iter()
        .map(|object| (object.id, normalize_name(&object.name)))
        .collect::<HashMap<MeshId, String>>();
    let mut object_names = HashSet::new();
    let mut primitive_count = 0usize;
    let mut triangle_count = 0usize;
    let mut surface_area_world = 0.0f32;
    let mut uv_area = 0.0f32;

    for sub_mesh in mesh
        .sub_meshes
        .iter()
        .filter(|sub_mesh| sub_mesh.material_index == material_index)
    {
        primitive_count += 1;
        if let Some(object_name) = object_name_by_id.get(&sub_mesh.mesh_id) {
            object_names.insert(object_name.clone());
        }
        let triangle_start = sub_mesh.start_index as usize / 3;
        let triangle_end = triangle_start + sub_mesh.index_count as usize / 3;
        for triangle in mesh.indices[triangle_start..triangle_end].iter() {
            let [i0, i1, i2] = *triangle;
            let p0 = mesh.positions[i0 as usize];
            let p1 = mesh.positions[i1 as usize];
            let p2 = mesh.positions[i2 as usize];
            surface_area_world += 0.5 * (p1 - p0).cross(p2 - p0).length();
            let uv0 = mesh.uvs[i0 as usize];
            let uv1 = mesh.uvs[i1 as usize];
            let uv2 = mesh.uvs[i2 as usize];
            uv_area += 0.5 * ((uv1 - uv0).perp_dot(uv2 - uv0)).abs();
            triangle_count += 1;
        }
    }

    MaterialSignature {
        name: name.to_owned(),
        normalized_name: normalize_name(name),
        name_tokens: name_tokens(name),
        object_names,
        primitive_count,
        triangle_count,
        surface_area_world,
        uv_area,
        render_settings,
        original_index: material_index,
    }
}

fn score_pair(imported: &MaterialSignature, existing: &MaterialSignature) -> MatchPair {
    let name_similarity = set_similarity(&imported.name_tokens, &existing.name_tokens);
    let object_similarity = set_similarity(&imported.object_names, &existing.object_names);
    let triangle_similarity = count_similarity(imported.triangle_count, existing.triangle_count);
    let primitive_similarity = count_similarity(imported.primitive_count, existing.primitive_count);
    let world_area_similarity =
        positive_float_similarity(imported.surface_area_world, existing.surface_area_world);
    let uv_area_similarity = positive_float_similarity(imported.uv_area, existing.uv_area);
    let render_similarity = if imported.render_settings == existing.render_settings {
        1.0
    } else {
        0.0
    };
    let index_similarity =
        1.0 / (1.0 + imported.original_index.abs_diff(existing.original_index) as f32);
    let geometry_similarity = 0.55 * triangle_similarity
        + 0.15 * primitive_similarity
        + 0.2 * world_area_similarity
        + 0.1 * uv_area_similarity;
    let score = 0.40 * name_similarity
        + 0.30 * object_similarity
        + 0.20 * geometry_similarity
        + 0.07 * render_similarity
        + 0.03 * index_similarity;
    MatchPair {
        imported_index: imported.original_index,
        existing_index: existing.original_index,
        score,
        name_similarity,
        object_similarity,
        geometry_similarity,
    }
}

fn pair_reason(pair: &MatchPair) -> String {
    if pair.object_similarity >= 0.99 && pair.name_similarity >= 0.5 {
        "Similar name / same mesh object".to_owned()
    } else if pair.object_similarity >= 0.99 {
        "Same mesh object / similar geometry".to_owned()
    } else if pair.name_similarity >= 0.5 {
        "Similar name / geometry".to_owned()
    } else if pair.geometry_similarity >= 0.8 {
        "Geometry similarity".to_owned()
    } else {
        "Heuristic match".to_owned()
    }
}

fn normalize_name(name: &str) -> String {
    let mut tokens = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if tokens
        .last()
        .is_some_and(|token| token.chars().all(|ch| ch.is_ascii_digit()))
    {
        tokens.pop();
    }
    if tokens
        .last()
        .is_some_and(|token| matches!(token.as_str(), "material" | "mat"))
    {
        tokens.pop();
    }
    tokens.join(" ")
}

fn name_tokens(name: &str) -> HashSet<String> {
    normalize_name(name)
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn set_similarity(left: &HashSet<String>, right: &HashSet<String>) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let union_count = left.union(right).count();
    if union_count == 0 {
        return 0.0;
    }
    left.intersection(right).count() as f32 / union_count as f32
}

fn count_similarity(left: usize, right: usize) -> f32 {
    if left == 0 || right == 0 {
        return 0.0;
    }
    let maximum = left.max(right);
    if maximum == 0 {
        0.0
    } else {
        left.min(right) as f32 / maximum as f32
    }
}

fn positive_float_similarity(left: f32, right: f32) -> f32 {
    if left <= f32::EPSILON || right <= f32::EPSILON {
        return 0.0;
    }
    let maximum = left.max(right);
    if !maximum.is_finite() || maximum <= 0.0 {
        return 0.0;
    }
    (left.min(right) / maximum).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use crate::{
        core::document::{MeshObject, SubMesh},
        import::ImportedMaterial,
    };

    use super::*;

    fn mesh(material_names: &[&str], object_names: &[&str]) -> MeshData {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        let mut sub_meshes = Vec::new();
        let mut triangle_mesh_ids = Vec::new();
        let mesh_objects = object_names
            .iter()
            .enumerate()
            .map(|(index, name)| MeshObject {
                id: MeshId(index),
                name: (*name).to_owned(),
            })
            .collect::<Vec<_>>();
        for (material_index, name) in material_names.iter().enumerate() {
            let mesh_id = MeshId(material_index.min(object_names.len().saturating_sub(1)));
            let base = positions.len() as u32;
            positions.extend([Vec3::ZERO, Vec3::X, Vec3::Y]);
            uvs.extend([Vec2::ZERO, Vec2::X, Vec2::Y]);
            normals.extend([Vec3::Z; 3]);
            let start_index = (indices.len() * 3) as u32;
            indices.push([base, base + 1, base + 2]);
            triangle_mesh_ids.push(mesh_id);
            sub_meshes.push(SubMesh {
                mesh_id,
                start_index,
                index_count: 3,
                material_index,
                material_name: (*name).to_owned(),
                wireframe_edges: vec![[base, base + 1], [base + 1, base + 2], [base, base + 2]],
            });
        }
        MeshData::new(
            positions,
            uvs,
            normals,
            indices,
            sub_meshes,
            mesh_objects,
            triangle_mesh_ids,
        )
        .unwrap()
    }

    fn imported(material_names: &[&str], object_names: &[&str]) -> ImportedAsset {
        ImportedAsset {
            mesh: mesh(material_names, object_names),
            materials: material_names
                .iter()
                .map(|name| ImportedMaterial {
                    source_material_index: None,
                    name: (*name).to_owned(),
                    render_settings: MaterialRenderSettings::default(),
                })
                .collect(),
        }
    }

    #[test]
    fn exact_names_match_existing_materials_with_high_confidence() {
        let document = Document::new(
            mesh(&["Body", "Eyes"], &["BodyObject", "EyeObject"]),
            vec![
                MaterialSpec::new("Body", [8, 8]),
                MaterialSpec::new("Eyes", [8, 8]),
            ],
        );
        let imported = imported(&["Eyes", "Body"], &["EyeObject", "BodyObject"]);

        let matches = build_mesh_reload_matches(&document, &imported);

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].confidence, MeshReloadMatchConfidence::High);
        assert_eq!(matches[1].confidence, MeshReloadMatchConfidence::High);
        assert_eq!(
            matches[0].target,
            MeshReloadMaterialTarget::Existing(document.materials[1].id)
        );
        assert_eq!(
            matches[1].target,
            MeshReloadMaterialTarget::Existing(document.materials[0].id)
        );
    }

    #[test]
    fn dcc_numeric_suffix_normalizes_to_existing_name() {
        let document = Document::new(
            mesh(&["Body Material"], &["Character"]),
            vec![MaterialSpec::new("Body Material", [8, 8])],
        );
        let imported = imported(&["body_material.001"], &["Character"]);

        let matches = build_mesh_reload_matches(&document, &imported);

        assert_eq!(matches[0].confidence, MeshReloadMatchConfidence::High);
        assert_eq!(
            matches[0].target,
            MeshReloadMaterialTarget::Existing(document.materials[0].id)
        );
    }

    #[test]
    fn missing_geometry_and_object_evidence_does_not_create_a_match() {
        let document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("Body", [8, 8]),
                MaterialSpec::new("Eyes", [8, 8]),
            ],
        );
        let imported = ImportedAsset {
            mesh: MeshData::empty(),
            materials: vec![
                ImportedMaterial {
                    source_material_index: None,
                    name: "Body".to_owned(),
                    render_settings: MaterialRenderSettings::default(),
                },
                ImportedMaterial {
                    source_material_index: None,
                    name: "Metal".to_owned(),
                    render_settings: MaterialRenderSettings::default(),
                },
            ],
        };

        let matches = build_mesh_reload_matches(&document, &imported);

        assert_eq!(
            matches[0].target,
            MeshReloadMaterialTarget::Existing(document.materials[0].id)
        );
        assert_eq!(matches[1].target, MeshReloadMaterialTarget::CreateNew);
    }

    #[test]
    fn automatic_matching_is_one_to_one_when_material_is_split() {
        let document = Document::new(
            mesh(&["Body"], &["Character"]),
            vec![MaterialSpec::new("Body", [8, 8])],
        );
        let imported = imported(&["Body Upper", "Body Lower"], &["Character", "Character"]);

        let matches = build_mesh_reload_matches(&document, &imported);
        let reused = matches
            .iter()
            .filter(|matched| matches!(matched.target, MeshReloadMaterialTarget::Existing(_)))
            .count();

        assert_eq!(reused, 1);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn command_allows_manual_many_to_one_mapping_and_appends_new_materials() {
        let document = Document::new(
            mesh(&["Body"], &["Character"]),
            vec![MaterialSpec::new("Body", [8, 8])],
        );
        let imported = imported(
            &["Body Upper", "Body Lower", "Metal"],
            &["Character", "Character", "Accessory"],
        );
        let body_id = document.materials[0].id;
        let bindings = vec![
            MeshReloadMaterialBinding {
                target: MeshReloadMaterialTarget::Existing(body_id),
                new_material: None,
            },
            MeshReloadMaterialBinding {
                target: MeshReloadMaterialTarget::Existing(body_id),
                new_material: None,
            },
            MeshReloadMaterialBinding {
                target: MeshReloadMaterialTarget::CreateNew,
                new_material: Some(MaterialSpec::new("Metal", [16, 16])),
            },
        ];

        let Command::ReloadMesh { request } =
            create_mesh_reload_command(&document, &imported, &bindings, 4096).unwrap()
        else {
            panic!("expected reload command");
        };

        assert_eq!(request.new_materials.len(), 1);
        assert_eq!(request.mesh.sub_meshes[0].material_index, 0);
        assert_eq!(request.mesh.sub_meshes[1].material_index, 0);
        assert_eq!(request.mesh.sub_meshes[2].material_index, 1);
    }
}
