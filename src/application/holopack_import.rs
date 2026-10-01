use std::{collections::HashSet, io::Cursor};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    application::image_decode::{DecodedRgba8Image, decode_png_bytes},
    core::{
        brush_engine::{BrushEngineDefinition, BrushEngineOrigin, BrushEngineRegistry},
        brush_preset::{BrushPresetCatalog, BrushPresetDefinition},
        texture::TextureCatalog,
    },
    holopack::{HoloPackManifest, HoloPackReader},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoloPackConflictKind {
    BrushEngine,
    BrushPreset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoloPackConflict {
    pub(crate) kind: HoloPackConflictKind,
    pub(crate) id: String,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedHoloPackTexture {
    pub(crate) path: String,
    pub(crate) encoded_png: Vec<u8>,
    pub(crate) image: DecodedRgba8Image,
}

#[derive(Debug, Clone)]
pub(crate) struct HoloPackImportPlan {
    pub(crate) manifest: HoloPackManifest,
    pub(crate) brush_engines: Vec<BrushEngineDefinition>,
    pub(crate) brush_presets: Vec<BrushPresetDefinition>,
    pub(crate) textures: Vec<PreparedHoloPackTexture>,
    pub(crate) conflicts: Vec<HoloPackConflict>,
}

impl HoloPackImportPlan {
    pub(crate) fn build(
        bytes: &[u8],
        existing_engines: &BrushEngineRegistry,
        existing_presets: &BrushPresetCatalog,
        textures: &TextureCatalog,
        max_texture_dimension_2d: u32,
    ) -> Result<Self> {
        let content = HoloPackReader::read(Cursor::new(bytes)).context("reading HoloPack")?;
        let mut prepared_textures = Vec::new();
        let mut brush_engines = Vec::new();
        let mut brush_presets = Vec::new();

        for resource in content.resources {
            match resource.entry.kind.as_str() {
                "texture" => {
                    let file_name = resource
                        .entry
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or("Texture.png");
                    let image =
                        decode_png_bytes(&resource.bytes, file_name, max_texture_dimension_2d)
                            .with_context(|| {
                                format!("invalid Texture {:?}", resource.entry.path)
                            })?;
                    prepared_textures.push(PreparedHoloPackTexture {
                        path: resource.entry.path,
                        encoded_png: resource.bytes,
                        image,
                    });
                }
                "brush_engine" => brush_engines.push(
                    BrushEngineDefinition::from_ron_bytes(&resource.bytes, &resource.entry.path)
                        .with_context(|| {
                            format!("invalid Brush Engine {:?}", resource.entry.path)
                        })?,
                ),
                "brush_preset" => brush_presets.push(
                    BrushPresetDefinition::from_ron_bytes(&resource.bytes, &resource.entry.path)
                        .with_context(|| {
                            format!("invalid Brush Preset {:?}", resource.entry.path)
                        })?,
                ),
                kind => bail!(
                    "unsupported HoloPack resource kind {kind:?} at {:?}",
                    resource.entry.path
                ),
            }
        }

        ensure_unique_engine_ids(&brush_engines)?;
        ensure_unique_preset_ids(&brush_presets)?;

        let mut candidate_engines = existing_engines.clone();
        let mut conflicts = Vec::new();
        for engine in &brush_engines {
            match existing_engines.origin(&engine.id) {
                Some(BrushEngineOrigin::Builtin) => bail!(
                    "Brush Engine {:?} conflicts with an immutable built-in resource",
                    engine.id
                ),
                Some(BrushEngineOrigin::User) => conflicts.push(HoloPackConflict {
                    kind: HoloPackConflictKind::BrushEngine,
                    id: engine.id.clone(),
                    display_name: engine.display_name.clone(),
                }),
                None => {}
            }
            candidate_engines
                .insert_or_replace_user(engine.clone(), textures)
                .with_context(|| format!("validating Brush Engine {:?}", engine.id))?;
        }

        let mut candidate_presets = existing_presets.clone();
        for preset in &brush_presets {
            if let Some(existing) = existing_presets
                .definitions()
                .find(|existing| existing.id() == preset.id())
            {
                conflicts.push(HoloPackConflict {
                    kind: HoloPackConflictKind::BrushPreset,
                    id: preset.id().to_owned(),
                    display_name: existing.display_name().to_owned(),
                });
            }
            candidate_presets.insert_or_replace_user(preset.clone())?;
        }

        for preset in candidate_presets.definitions() {
            let engine = candidate_engines.get(preset.engine_id()).ok_or_else(|| {
                anyhow::anyhow!(
                    "Brush Preset {:?} references unknown Brush Engine {:?}",
                    preset.id(),
                    preset.engine_id()
                )
            })?;
            preset.resolve(engine, textures).with_context(|| {
                format!(
                    "validating Brush Preset {:?} against imported resources",
                    preset.id()
                )
            })?;
        }

        Ok(Self {
            manifest: content.manifest,
            brush_engines,
            brush_presets,
            textures: prepared_textures,
            conflicts,
        })
    }
}

fn ensure_unique_engine_ids(engines: &[BrushEngineDefinition]) -> Result<()> {
    let mut ids = HashSet::new();
    for engine in engines {
        ensure!(
            ids.insert(engine.id.as_str()),
            "duplicate incoming Brush Engine id {:?}",
            engine.id
        );
    }
    Ok(())
}

fn ensure_unique_preset_ids(presets: &[BrushPresetDefinition]) -> Result<()> {
    let mut ids = HashSet::new();
    for preset in presets {
        ensure!(
            ids.insert(preset.id()),
            "duplicate incoming Brush Preset id {:?}",
            preset.id()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::core::image_asset::ImageAssetCatalog;

    fn context() -> (BrushEngineRegistry, BrushPresetCatalog, TextureCatalog) {
        let images = ImageAssetCatalog::load_effective(None).unwrap();
        let textures = TextureCatalog::from_image_assets(&images).unwrap();
        let engines = BrushEngineRegistry::load_effective(Vec::new(), &textures).unwrap();
        let presets = BrushPresetCatalog::load_effective(None).unwrap();
        (engines, presets, textures)
    }

    fn pack_resources(resources: &[(&str, &str, &[u8])]) -> Vec<u8> {
        let entries = resources
            .iter()
            .map(|(kind, path, _)| format!("(kind:\"{kind}\",path:\"{path}\")"))
            .collect::<Vec<_>>()
            .join(",");
        let manifest = format!(
            "(format_version:1,id:\"com.example.test\",name:\"Test Pack\",version:\"1.0\",resources:[{entries}])"
        );
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut bytes);
            let options = SimpleFileOptions::default();
            writer.start_file("manifest.ron", options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            for (_, path, resource) in resources {
                writer.start_file(*path, options).unwrap();
                writer.write_all(resource).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }

    fn pack(kind: &str, path: &str, resource: &[u8]) -> Vec<u8> {
        pack_resources(&[(kind, path, resource)])
    }

    fn imported_engine_ron(id: &str) -> Vec<u8> {
        crate::embedded_resources::text("brushes/engines/paint.ron")
            .unwrap()
            .replacen("id: \"paint\"", &format!("id: \"{id}\""), 1)
            .into_bytes()
    }

    fn imported_preset_ron(id: &str) -> Vec<u8> {
        crate::embedded_resources::text("brushes/presets/basic_brush.ron")
            .unwrap()
            .replacen(
                "id: \"builtin.brush.paint.basic_brush\"",
                &format!("id: \"{id}\""),
                1,
            )
            .into_bytes()
    }

    fn one_pixel_png() -> Vec<u8> {
        let image = image::RgbaImage::from_raw(1, 1, vec![12, 34, 56, 255]).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn rejects_unknown_resource_kind_at_importer_boundary() {
        let (engines, presets, textures) = context();
        let error = HoloPackImportPlan::build(
            &pack("future_kind", "future.bin", b"data"),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported HoloPack resource kind")
        );
    }

    #[test]
    fn prepares_new_brush_preset_without_mutating_existing_catalogs() {
        let (engines, presets, textures) = context();
        let preset = crate::embedded_resources::text("brushes/presets/basic_brush.ron")
            .unwrap()
            .replace(
                "builtin.brush.paint.basic_brush",
                "user.brush.imported_test",
            );
        let before_engine_count = engines.len();
        let before_preset_count = presets.definitions().count();
        let plan = HoloPackImportPlan::build(
            &pack("brush_preset", "presets/imported.ron", preset.as_bytes()),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert_eq!(plan.brush_presets.len(), 1);
        assert!(plan.conflicts.is_empty());
        assert_eq!(engines.len(), before_engine_count);
        assert_eq!(presets.definitions().count(), before_preset_count);
    }

    #[test]
    fn rejects_builtin_brush_engine_conflict() {
        let (engines, presets, textures) = context();
        let engine = crate::embedded_resources::bytes("brushes/engines/paint.ron").unwrap();
        let error = HoloPackImportPlan::build(
            &pack("brush_engine", "engines/paint.ron", engine),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap_err();
        assert!(error.to_string().contains("immutable built-in"));
    }

    #[test]
    fn prepares_each_supported_resource_kind_independently() {
        let (engines, presets, textures) = context();
        let engine = imported_engine_ron("user.engine.only");
        let preset = imported_preset_ron("user.preset.only");
        let png = one_pixel_png();

        let engine_plan = HoloPackImportPlan::build(
            &pack("brush_engine", "engine.ron", &engine),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert_eq!(engine_plan.brush_engines.len(), 1);

        let preset_plan = HoloPackImportPlan::build(
            &pack("brush_preset", "preset.ron", &preset),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert_eq!(preset_plan.brush_presets.len(), 1);

        let texture_plan = HoloPackImportPlan::build(
            &pack("texture", "texture.png", &png),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert_eq!(texture_plan.textures.len(), 1);
    }

    #[test]
    fn mixed_pack_result_does_not_depend_on_manifest_resource_order() {
        let (engines, presets, textures) = context();
        let engine = imported_engine_ron("user.engine.mixed");
        let preset = imported_preset_ron("user.preset.mixed");
        let png = one_pixel_png();
        let first = [
            ("brush_preset", "preset.ron", preset.as_slice()),
            ("texture", "texture.png", png.as_slice()),
            ("brush_engine", "engine.ron", engine.as_slice()),
        ];
        let second = [first[2], first[0], first[1]];

        for resources in [&first[..], &second[..]] {
            let plan = HoloPackImportPlan::build(
                &pack_resources(resources),
                &engines,
                &presets,
                &textures,
                4096,
            )
            .unwrap();
            assert_eq!(plan.brush_engines.len(), 1);
            assert_eq!(plan.brush_presets.len(), 1);
            assert_eq!(plan.textures.len(), 1);
            assert!(plan.conflicts.is_empty());
        }
    }

    #[test]
    fn invalid_resource_and_incoming_duplicate_leave_catalogs_unchanged() {
        let (engines, presets, textures) = context();
        let before_engine_count = engines.len();
        let before_preset_count = presets.definitions().count();
        let engine = imported_engine_ron("user.engine.duplicate");
        let duplicate_pack = pack_resources(&[
            ("brush_engine", "first.ron", engine.as_slice()),
            ("brush_engine", "second.ron", engine.as_slice()),
        ]);
        assert!(
            HoloPackImportPlan::build(&duplicate_pack, &engines, &presets, &textures, 4096,)
                .unwrap_err()
                .to_string()
                .contains("duplicate incoming Brush Engine")
        );
        assert!(
            HoloPackImportPlan::build(
                &pack("texture", "bad.png", b"not a png"),
                &engines,
                &presets,
                &textures,
                4096,
            )
            .is_err()
        );
        assert_eq!(engines.len(), before_engine_count);
        assert_eq!(presets.definitions().count(), before_preset_count);
    }

    #[test]
    fn detects_existing_user_conflict_by_resource_id_not_package_id() {
        let (mut engines, presets, textures) = context();
        let engine = imported_engine_ron("user.engine.conflict");
        let definition = BrushEngineDefinition::from_ron_bytes(&engine, "engine.ron").unwrap();
        engines
            .insert_or_replace_user(definition, &textures)
            .unwrap();

        let plan = HoloPackImportPlan::build(
            &pack("brush_engine", "engine.ron", &engine),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].id, "user.engine.conflict");

        let different_engine = imported_engine_ron("user.engine.same_package_new_resource");
        let plan = HoloPackImportPlan::build(
            &pack("brush_engine", "engine.ron", &different_engine),
            &engines,
            &presets,
            &textures,
            4096,
        )
        .unwrap();
        assert!(plan.conflicts.is_empty());
    }
}
