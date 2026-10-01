use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::core::{
    brush_engine::{
        BrushEngineDefinition, BrushStrokeKind, ParamExposure, ParamValue, RawParamValue,
        parse_param_value_for_type, raw_param_value_for_spec, validate_param_value_resources,
    },
    stroke_preset::{
        PressureDynamics, PressureResponse, QuickParamTarget, RangedF32, StrokeInputFilter,
        StrokePresetOp, StrokeStrategy, StrokeToolPreset,
    },
    texture::TextureCatalog,
};

const BRUSH_PRESET_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BrushPresetHandle(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrushPresetOrigin {
    Builtin,
    UserOverride,
    User,
}

#[derive(Debug, Clone)]
struct BrushPresetRecord {
    definition: BrushPresetDefinition,
    builtin: Option<BrushPresetDefinition>,
    origin: BrushPresetOrigin,
}

#[derive(Debug, Clone)]
pub(crate) struct BrushPresetCatalog {
    presets: Vec<Option<BrushPresetRecord>>,
    by_id: HashMap<String, BrushPresetHandle>,
}

impl BrushPresetCatalog {
    pub(crate) fn load_effective(user_path: Option<&Path>) -> Result<Self> {
        let builtin = crate::embedded_resources::files_in("brushes/presets")
            .filter(|file| file.path.ends_with(".ron"))
            .map(|file| {
                BrushPresetDefinition::from_ron_bytes(
                    file.bytes,
                    &format!("embedded resource {:?}", file.path),
                )
            });
        Self::load_with_builtin(builtin, user_path)
    }

    #[cfg(test)]
    pub(crate) fn load_from_fixtures(
        path: impl AsRef<Path>,
        user_path: Option<&Path>,
    ) -> Result<Self> {
        let files = ron_files_in(path.as_ref())?;
        Self::load_with_builtin(files.iter().map(|file| load_preset_file(file)), user_path)
    }

    fn load_with_builtin(
        builtin: impl IntoIterator<Item = Result<BrushPresetDefinition>>,
        user_path: Option<&Path>,
    ) -> Result<Self> {
        let mut presets = Vec::new();
        let mut by_id = HashMap::new();
        for preset in builtin {
            let preset = preset?;
            ensure!(
                !by_id.contains_key(preset.id()),
                "duplicate brush preset id {:?}",
                preset.id()
            );
            let handle = BrushPresetHandle(presets.len());
            by_id.insert(preset.id.clone(), handle);
            presets.push(Some(BrushPresetRecord {
                definition: preset.clone(),
                builtin: Some(preset),
                origin: BrushPresetOrigin::Builtin,
            }));
        }

        if let Some(user_path) = user_path
            && user_path.exists()
        {
            for file in ron_files_in(user_path)? {
                let preset = match load_preset_file(&file) {
                    Ok(preset) => preset,
                    Err(error) => {
                        eprintln!(
                            "Ignoring invalid user brush preset {}: {error:#}",
                            file.display()
                        );
                        continue;
                    }
                };
                let expected_file_name = format!("{}.ron", preset.id());
                ensure!(
                    file.file_name().and_then(|name| name.to_str())
                        == Some(expected_file_name.as_str()),
                    "user brush preset {:?} must be stored as {:?}",
                    preset.id(),
                    expected_file_name
                );
                if let Some(handle) = by_id.get(preset.id()).copied() {
                    let record = presets[handle.0]
                        .as_mut()
                        .expect("brush preset handle must reference a record");
                    ensure!(
                        matches!(record.origin, BrushPresetOrigin::Builtin),
                        "duplicate user brush preset id {:?}",
                        preset.id()
                    );
                    record.definition = preset;
                    record.origin = BrushPresetOrigin::UserOverride;
                } else {
                    let handle = BrushPresetHandle(presets.len());
                    by_id.insert(preset.id.clone(), handle);
                    presets.push(Some(BrushPresetRecord {
                        definition: preset,
                        builtin: None,
                        origin: BrushPresetOrigin::User,
                    }));
                }
            }
        }

        Ok(Self { presets, by_id })
    }

    pub(crate) fn get(&self, handle: BrushPresetHandle) -> Option<&BrushPresetDefinition> {
        self.presets
            .get(handle.0)
            .and_then(Option::as_ref)
            .map(|record| &record.definition)
    }

    pub(crate) fn get_mut(
        &mut self,
        handle: BrushPresetHandle,
    ) -> Option<&mut BrushPresetDefinition> {
        let record = self.presets.get_mut(handle.0)?.as_mut()?;
        if matches!(record.origin, BrushPresetOrigin::Builtin) {
            record.origin = BrushPresetOrigin::UserOverride;
        }
        Some(&mut record.definition)
    }

    pub(crate) fn handle(&self, id: &str) -> Option<BrushPresetHandle> {
        self.by_id.get(id).copied()
    }

    pub(crate) fn insert_user(
        &mut self,
        preset: BrushPresetDefinition,
    ) -> Result<BrushPresetHandle> {
        preset.validate_shape()?;
        ensure!(
            !self.by_id.contains_key(preset.id()),
            "duplicate brush preset id {:?}",
            preset.id()
        );
        let handle = BrushPresetHandle(self.presets.len());
        self.by_id.insert(preset.id.clone(), handle);
        self.presets.push(Some(BrushPresetRecord {
            definition: preset,
            builtin: None,
            origin: BrushPresetOrigin::User,
        }));
        Ok(handle)
    }

    pub(crate) fn insert_or_replace_user(
        &mut self,
        preset: BrushPresetDefinition,
    ) -> Result<Option<BrushPresetDefinition>> {
        preset.validate_shape()?;
        let Some(handle) = self.handle(preset.id()) else {
            self.insert_user(preset)?;
            return Ok(None);
        };
        let record = self.presets[handle.0]
            .as_mut()
            .expect("brush preset handle must reference a record");
        let previous = record.definition.clone();
        if matches!(record.origin, BrushPresetOrigin::Builtin) {
            record.origin = BrushPresetOrigin::UserOverride;
        }
        record.definition = preset;
        Ok(Some(previous))
    }

    pub(crate) fn definitions(&self) -> impl Iterator<Item = &BrushPresetDefinition> {
        self.presets
            .iter()
            .filter_map(Option::as_ref)
            .map(|record| &record.definition)
    }

    pub(crate) fn origin(&self, id: &str) -> Option<BrushPresetOrigin> {
        let handle = self.handle(id)?;
        self.presets
            .get(handle.0)
            .and_then(Option::as_ref)
            .map(|record| record.origin)
    }

    pub(crate) fn delete_user_state(&mut self, id: &str) -> Result<()> {
        let Some(handle) = self.handle(id) else {
            bail!("unknown brush preset {:?}", id);
        };
        let record = self.presets[handle.0]
            .as_mut()
            .expect("brush preset handle must reference a record");
        match record.origin {
            BrushPresetOrigin::Builtin => {}
            BrushPresetOrigin::UserOverride => {
                record.definition = record
                    .builtin
                    .clone()
                    .expect("user override must retain builtin definition");
                record.origin = BrushPresetOrigin::Builtin;
            }
            BrushPresetOrigin::User => {
                self.presets[handle.0] = None;
                self.by_id.remove(id);
            }
        }
        Ok(())
    }

    pub(crate) fn user_definitions(&self) -> Vec<BrushPresetDefinition> {
        let mut definitions = self
            .presets
            .iter()
            .filter_map(Option::as_ref)
            .filter(|record| !matches!(record.origin, BrushPresetOrigin::Builtin))
            .map(|record| record.definition.clone())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.id.cmp(&right.id));
        definitions
    }
}

fn ron_files_in(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(path)
        .with_context(|| format!("reading brush preset directory {}", path.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
        .collect();
    files.sort();
    Ok(files)
}

fn load_preset_file(file: &Path) -> Result<BrushPresetDefinition> {
    let source =
        fs::read(file).with_context(|| format!("reading brush preset {}", file.display()))?;
    BrushPresetDefinition::from_ron_bytes(&source, &file.display().to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct BrushPresetDefinition {
    schema_version: u32,
    id: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    engine_id: String,
    stroke: PresetStrokeDefinition,
    engine_params: BTreeMap<String, RawParamValue>,
    #[serde(default)]
    quick_params: Vec<QuickParamTarget>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PresetStrokeDefinition {
    strategy: StrokeStrategy,
    input_filter: StrokeInputFilter,
    #[serde(default)]
    pressure_response: PressureResponse,
    size_scene_ratio: RangedF32,
    size_pressure: PressureDynamics,
}

impl BrushPresetDefinition {
    pub(crate) fn from_ron_bytes(bytes: &[u8], source_name: &str) -> Result<Self> {
        let preset: Self = ron::de::from_bytes(bytes)
            .with_context(|| format!("parsing brush preset {source_name:?}"))?;
        preset
            .validate_shape()
            .with_context(|| format!("validating brush preset {source_name:?}"))?;
        Ok(preset)
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn engine_id(&self) -> &str {
        &self.engine_id
    }

    pub(crate) fn clone_as_user(
        &self,
        id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        let mut clone = self.clone();
        clone.id = id.into();
        clone.display_name = display_name.into();
        clone
    }

    pub(crate) fn retarget_engine(&mut self, engine: &BrushEngineDefinition) -> Result<()> {
        if !engine.supports_stroke_strategy(&self.stroke.strategy) {
            self.stroke.strategy = engine.default_stroke.clone();
        }
        self.engine_id = engine.id.clone();
        self.engine_params = engine.default_public_params()?;
        self.quick_params.retain(|target| match target {
            QuickParamTarget::Common(param) => param.is_available_for(&self.stroke.strategy),
            QuickParamTarget::Engine(name) => engine
                .params
                .iter()
                .any(|spec| spec.name == *name && matches!(spec.exposure, ParamExposure::Public)),
        });
        self.validate_shape()
    }

    pub(crate) fn reset_engine_defaults(&mut self, engine: &BrushEngineDefinition) -> Result<()> {
        self.stroke.strategy = engine.default_stroke.clone();
        self.engine_params = engine.default_public_params()?;
        self.validate_shape()
    }

    pub(crate) fn sync_from_runtime(
        &mut self,
        preset: &StrokeToolPreset,
        engine: &BrushEngineDefinition,
    ) -> Result<()> {
        let StrokePresetOp::BrushEngine {
            engine_id,
            size_scene_ratio,
            size_pressure,
            params,
            ..
        } = &preset.stroke_op;
        ensure!(
            engine_id == &engine.id,
            "runtime preset engine {:?} does not match engine definition {:?}",
            engine_id,
            engine.id
        );

        let mut engine_params = BTreeMap::new();
        for spec in engine
            .params
            .iter()
            .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
        {
            let value = params
                .iter()
                .find(|(name, _)| name == &spec.name)
                .map(|(_, value)| value)
                .unwrap_or(&spec.default);
            let raw = raw_param_value_for_spec(spec, value).map_err(anyhow::Error::msg)?;
            engine_params.insert(spec.name.clone(), raw);
        }

        self.display_name = preset.name.clone();
        self.engine_id = engine.id.clone();
        self.stroke.strategy = preset.stroke_strategy.clone();
        self.stroke.input_filter = preset.input_filter.clone();
        self.stroke.pressure_response = preset.pressure_response.clone();
        self.stroke.size_scene_ratio = size_scene_ratio.clone();
        self.stroke.size_pressure = size_pressure.clone();
        self.engine_params = engine_params;
        self.quick_params = preset.quick_params.clone();
        self.validate_shape()
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(
            self.schema_version == BRUSH_PRESET_SCHEMA_VERSION,
            "unsupported brush preset schema_version {}",
            self.schema_version
        );
        ensure!(
            !self.id.trim().is_empty(),
            "brush preset id must not be empty"
        );
        ensure!(
            self.id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')),
            "brush preset id {:?} contains characters that are not safe for persistence",
            self.id
        );
        ensure!(
            !self.display_name.trim().is_empty(),
            "brush preset display_name must not be empty"
        );
        ensure!(
            !self.engine_id.trim().is_empty(),
            "brush preset engine_id must not be empty"
        );
        let mut quick_targets = HashSet::new();
        for target in &self.quick_params {
            ensure!(
                quick_targets.insert(target.clone()),
                "duplicate quick_params target {:?}",
                target
            );
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        engine: &BrushEngineDefinition,
        textures: &TextureCatalog,
    ) -> Result<StrokeToolPreset> {
        let required_stroke_kind = BrushStrokeKind::from_strategy(&self.stroke.strategy);
        ensure!(
            engine
                .capabilities
                .stroke_kinds
                .contains(&required_stroke_kind),
            "brush preset {:?} requires stroke kind {:?}, but engine {:?} does not declare it",
            self.id,
            required_stroke_kind,
            engine.id
        );
        for target in &self.quick_params {
            match target {
                QuickParamTarget::Common(param) => ensure!(
                    param.is_available_for(&self.stroke.strategy),
                    "brush preset {:?} quick param {:?} is not available for stroke strategy {:?}",
                    self.id,
                    param,
                    self.stroke.strategy
                ),
                QuickParamTarget::Engine(name) => {
                    let spec = engine.params.iter().find(|spec| spec.name == *name).with_context(
                        || {
                            format!(
                                "brush preset {:?} quick param references unknown engine param {:?}",
                                self.id, name
                            )
                        },
                    )?;
                    ensure!(
                        matches!(spec.exposure, ParamExposure::Public),
                        "brush preset {:?} quick param references internal engine param {:?}",
                        self.id,
                        name
                    );
                }
            }
        }
        let mut params = Vec::new();

        for spec in &engine.params {
            let raw = self.engine_params.get(&spec.name).cloned();
            if matches!(spec.exposure, ParamExposure::Internal) {
                ensure!(
                    raw.is_none(),
                    "preset {:?} sets internal engine param {:?}",
                    self.id,
                    spec.name
                );
                continue;
            }
            let value = resolve_engine_param_value(spec, raw)
                .with_context(|| format!("preset {:?} param {:?}", self.id, spec.name))?;
            validate_param_value_resources(textures, &spec.ty, &value)
                .with_context(|| format!("preset {:?} param {:?}", self.id, spec.name))?;
            params.push((spec.name.clone(), value));
        }

        for name in self.engine_params.keys() {
            ensure!(
                engine.params.iter().any(|spec| spec.name == *name),
                "preset {:?} references unknown engine param {:?}",
                self.id,
                name
            );
        }

        let mut transient_overrides = HashMap::new();
        for (id, definition) in &engine.transient_overrides {
            let mut patch = Vec::new();
            for (name, raw) in &definition.params {
                let spec = engine
                    .params
                    .iter()
                    .find(|spec| spec.name == *name)
                    .expect("validated transient override param");
                let value =
                    resolve_engine_param_value(spec, Some(raw.clone())).with_context(|| {
                        format!(
                            "preset {:?} transient override {:?} param {:?}",
                            self.id, id, name
                        )
                    })?;
                validate_param_value_resources(textures, &spec.ty, &value).with_context(|| {
                    format!(
                        "preset {:?} transient override {:?} param {:?}",
                        self.id, id, name
                    )
                })?;
                patch.push((name.clone(), value));
            }
            transient_overrides.insert(id.clone(), patch);
        }

        let mut preset = StrokeToolPreset::brush_engine(
            self.display_name.clone(),
            self.stroke.strategy.clone(),
            self.stroke.input_filter.clone(),
            self.stroke.pressure_response.clone(),
            StrokePresetOp::BrushEngine {
                engine_id: self.engine_id.clone(),
                size_scene_ratio: self.stroke.size_scene_ratio.clone(),
                size_pressure: self.stroke.size_pressure.clone(),
                params,
                param_dynamics: engine.param_dynamics_bindings(),
                surface_source_material_scope: engine.surface_source_material_scope.clone(),
            },
        );
        preset.transient_overrides = transient_overrides;
        preset.quick_params = self.quick_params.clone();
        Ok(preset)
    }
}

fn resolve_engine_param_value(
    spec: &crate::core::brush_engine::ParamSpec,
    raw: Option<RawParamValue>,
) -> Result<ParamValue> {
    match raw {
        Some(raw) => {
            parse_param_value_for_type(&spec.ty, raw, "engine param").map_err(anyhow::Error::msg)
        }
        None => Ok(spec.default.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        brush_engine::BrushEngineRegistry,
        stroke_preset::{CommonBrushParam, QuickParamTarget},
    };

    fn preset_fixture_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brushes/presets")
    }

    fn preset_source(quick_params: &str) -> String {
        format!(
            r#"(
    schema_version: 1,
    id: "test.preset",
    display_name: "Test Preset",
    engine_id: "paint",
    stroke: (
        strategy: SpacingDab(spacing: 0.2),
        input_filter: (stabilization: 0, pressure_filter: 0),
        size_scene_ratio: (value: 0.01),
        size_pressure: (
            enabled: false,
            curve: [(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)],
        ),
    ),
    engine_params: {{}},
    quick_params: {quick_params},
)"#
        )
    }

    fn paint_engine_and_textures() -> (BrushEngineDefinition, TextureCatalog) {
        let textures = TextureCatalog::from_image_assets(
            &crate::core::image_asset::ImageAssetCatalog::load_effective(None).unwrap(),
        )
        .unwrap();
        let engines = BrushEngineRegistry::load_effective(Vec::new(), &textures).unwrap();
        (engines.get("paint").unwrap().clone(), textures)
    }

    fn parse(quick_params: &str) -> BrushPresetDefinition {
        let preset: BrushPresetDefinition = ron::from_str(&preset_source(quick_params)).unwrap();
        preset.validate_shape().unwrap();
        preset
    }

    #[test]
    fn engine_defaults_preserve_preset_metadata_and_settings_without_engine_defaults() {
        let mut preset = parse(r#"[Common(Size), Engine("opacity")]"#);
        let (engine, textures) = paint_engine_and_textures();
        preset.description = Some("Keep description".to_owned());
        preset.tags = vec!["keep-tag".to_owned()];
        preset.stroke.input_filter.stabilization = 7;
        let before = preset.clone();
        preset.reset_engine_defaults(&engine).unwrap();
        assert_eq!(preset.id, before.id);
        assert_eq!(preset.display_name, before.display_name);
        assert_eq!(preset.description, before.description);
        assert_eq!(preset.tags, before.tags);
        assert_eq!(preset.quick_params, before.quick_params);
        assert_eq!(preset.stroke.input_filter, before.stroke.input_filter);
        assert_eq!(
            preset.stroke.size_scene_ratio,
            before.stroke.size_scene_ratio
        );
        assert_eq!(preset.stroke.size_pressure, before.stroke.size_pressure);
        assert_eq!(
            preset.stroke.pressure_response,
            before.stroke.pressure_response
        );
        assert_eq!(preset.stroke.strategy, engine.default_stroke);
        let resolved = preset.resolve(&engine, &textures).unwrap();
        let StrokePresetOp::BrushEngine { params, .. } = resolved.stroke_op;
        for spec in engine
            .params
            .iter()
            .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
        {
            assert_eq!(
                params
                    .iter()
                    .find(|(name, _)| name == &spec.name)
                    .unwrap()
                    .1,
                spec.default
            );
        }
    }

    #[test]
    fn current_schema_quick_params_keep_order_after_resolve() {
        let preset = parse(r#"[Common(Size), Engine("opacity"), Engine("flow")]"#);
        let (engine, textures) = paint_engine_and_textures();

        let resolved = preset.resolve(&engine, &textures).unwrap();

        assert_eq!(
            resolved.quick_params,
            vec![
                QuickParamTarget::Common(CommonBrushParam::Size),
                QuickParamTarget::Engine("opacity".to_owned()),
                QuickParamTarget::Engine("flow".to_owned()),
            ]
        );
    }

    #[test]
    fn separated_dynamics_can_be_quick_params_independently() {
        let preset = parse(r#"[Common(SizePressure), Engine("flow_pressure"), Engine("flow")]"#);
        let (engine, textures) = paint_engine_and_textures();

        let resolved = preset.resolve(&engine, &textures).unwrap();

        assert_eq!(
            resolved.quick_params,
            vec![
                QuickParamTarget::Common(CommonBrushParam::SizePressure),
                QuickParamTarget::Engine("flow_pressure".to_owned()),
                QuickParamTarget::Engine("flow".to_owned()),
            ]
        );
    }

    #[test]
    fn duplicate_quick_target_is_rejected() {
        let preset: BrushPresetDefinition =
            ron::from_str(&preset_source(r#"[Engine("opacity"), Engine("opacity")]"#)).unwrap();

        let error = preset.validate_shape().unwrap_err();

        assert!(error.to_string().contains("duplicate quick_params"));
    }

    #[test]
    fn unknown_engine_quick_target_is_rejected() {
        let preset = parse(r#"[Engine("missing")]"#);
        let (engine, textures) = paint_engine_and_textures();

        let error = preset.resolve(&engine, &textures).unwrap_err();

        assert!(error.to_string().contains("unknown engine param"));
    }

    #[test]
    fn internal_engine_quick_target_is_rejected() {
        let preset = parse(r#"[Engine("opacity")]"#);
        let (mut engine, textures) = paint_engine_and_textures();
        engine
            .params
            .iter_mut()
            .find(|spec| spec.name == "opacity")
            .unwrap()
            .exposure = ParamExposure::Internal;

        let error = preset.resolve(&engine, &textures).unwrap_err();

        assert!(error.to_string().contains("internal engine param"));
    }

    #[test]
    fn strategy_incompatible_common_quick_target_is_rejected() {
        let preset = parse("[Common(SprayRate)]");
        let (engine, textures) = paint_engine_and_textures();

        let error = preset.resolve(&engine, &textures).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("not available for stroke strategy")
        );
    }

    #[test]
    fn missing_engine_param_uses_engine_default() {
        let preset = parse("[]");
        let (engine, textures) = paint_engine_and_textures();

        let resolved = preset.resolve(&engine, &textures).unwrap();
        let StrokePresetOp::BrushEngine { params, .. } = resolved.stroke_op;
        let opacity = params
            .iter()
            .find(|(name, _)| name == "opacity")
            .map(|(_, value)| value)
            .unwrap();
        let default = &engine
            .params
            .iter()
            .find(|spec| spec.name == "opacity")
            .unwrap()
            .default;

        assert_eq!(opacity, default);
    }

    #[test]
    fn directional_flat_fixture_resolves_stroke_rotation() {
        let (engine, textures) = paint_engine_and_textures();
        let presets = BrushPresetCatalog::load_from_fixtures(preset_fixture_dir(), None).unwrap();

        let resolved_param = |id: &str| {
            let preset = presets.get(presets.handle(id).unwrap()).unwrap();
            let resolved = preset.resolve(&engine, &textures).unwrap();
            let StrokePresetOp::BrushEngine { params, .. } = resolved.stroke_op;
            params
                .into_iter()
                .find(|(name, _)| name == "tip_rotation")
                .map(|(_, value)| value)
                .unwrap()
        };

        assert_eq!(
            resolved_param("builtin.brush.paint.directional_flat"),
            ParamValue::Enum(1)
        );
        assert_eq!(
            resolved_param("builtin.brush.paint.round_soft"),
            ParamValue::Enum(0)
        );
    }

    #[test]
    fn separated_engine_dynamics_round_trip_as_independent_params() {
        let mut preset = parse("[]");
        let (engine, textures) = paint_engine_and_textures();
        let mut runtime = preset.resolve(&engine, &textures).unwrap();
        let StrokePresetOp::BrushEngine { params, .. } = &mut runtime.stroke_op;
        let (_, flow) = params.iter_mut().find(|(name, _)| name == "flow").unwrap();
        let ParamValue::F32(flow) = flow else {
            panic!("flow should resolve as F32");
        };
        *flow = 0.42;
        let (_, flow_pressure) = params
            .iter_mut()
            .find(|(name, _)| name == "flow_pressure")
            .unwrap();
        let ParamValue::Dynamics(flow_pressure) = flow_pressure else {
            panic!("flow_pressure should resolve as Dynamics");
        };
        flow_pressure.enabled = false;
        assert!(
            flow_pressure
                .curve
                .set_point(0, crate::core::curve::CurvePoint { x: 0.0, y: 0.25 },)
        );

        preset.sync_from_runtime(&runtime, &engine).unwrap();
        let encoded =
            ron::ser::to_string_pretty(&preset, ron::ser::PrettyConfig::default()).unwrap();

        assert!(encoded.contains("flow"));
        assert!(encoded.contains("flow_pressure"));
        let decoded: BrushPresetDefinition = ron::from_str(&encoded).unwrap();
        assert_eq!(decoded, preset);
    }

    #[test]
    fn sync_from_runtime_persists_every_public_engine_param() {
        let mut preset = parse("[]");
        let (engine, textures) = paint_engine_and_textures();
        let runtime = preset.resolve(&engine, &textures).unwrap();

        preset.sync_from_runtime(&runtime, &engine).unwrap();

        let expected = engine
            .params
            .iter()
            .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            preset
                .engine_params
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            {
                let mut expected = expected;
                expected.sort_unstable();
                expected
            }
        );
        assert!(preset.engine_params.contains_key("tip_mask"));
    }

    #[test]
    fn retarget_engine_uses_engine_default_stroke_when_current_strategy_is_unsupported() {
        let mut preset: BrushPresetDefinition = ron::from_str(&preset_source(
            r#"[Common(Size), Common(SprayRate), Engine("opacity"), Engine("flow")]"#,
        ))
        .unwrap();
        preset.stroke.strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.12,
            rate_hz: 60.0,
        };
        let textures = TextureCatalog::from_image_assets(
            &crate::core::image_asset::ImageAssetCatalog::load_effective(None).unwrap(),
        )
        .unwrap();
        let engines = BrushEngineRegistry::load_effective(Vec::new(), &textures).unwrap();
        let blur = engines.get("blur").unwrap();

        preset.retarget_engine(blur).unwrap();

        assert_eq!(preset.engine_id(), "blur");
        assert_eq!(preset.stroke.strategy, blur.default_stroke);
        assert!(
            !preset
                .quick_params
                .contains(&QuickParamTarget::Common(CommonBrushParam::SprayRate))
        );
        assert!(
            preset
                .quick_params
                .contains(&QuickParamTarget::Engine("opacity".to_owned()))
        );
        assert!(
            !preset
                .quick_params
                .contains(&QuickParamTarget::Engine("flow".to_owned()))
        );
        assert_eq!(preset.engine_params, blur.default_public_params().unwrap());
    }

    #[test]
    fn invalid_user_override_is_ignored_and_builtin_remains_available() {
        let temp = tempfile::tempdir().expect("tempdir");
        let id = "builtin.brush.paint.round_soft";
        std::fs::write(temp.path().join(format!("{id}.ron")), "(schema_version: 2)")
            .expect("write invalid override");

        let effective =
            BrushPresetCatalog::load_from_fixtures(preset_fixture_dir(), Some(temp.path()))
                .expect("effective catalog");

        let handle = effective
            .handle(id)
            .expect("builtin preset remains available");
        assert_eq!(effective.get(handle).unwrap().display_name(), "Round Soft");
        assert!(effective.user_definitions().is_empty());
    }

    #[test]
    fn user_override_replaces_builtin_and_delete_reverts_to_builtin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let builtin = BrushPresetCatalog::load_from_fixtures(preset_fixture_dir(), None)
            .expect("builtin presets");
        let id = "builtin.brush.paint.round_soft";
        let handle = builtin.handle(id).expect("builtin preset");
        let builtin_definition = builtin.get(handle).expect("builtin definition").clone();
        let mut override_definition = builtin_definition.clone();
        override_definition.display_name = "Overridden Brush".to_owned();
        std::fs::write(
            temp.path().join(format!("{id}.ron")),
            ron::ser::to_string_pretty(&override_definition, ron::ser::PrettyConfig::default())
                .expect("serialize override"),
        )
        .expect("write override");

        let mut effective =
            BrushPresetCatalog::load_from_fixtures(preset_fixture_dir(), Some(temp.path()))
                .expect("effective catalog");
        let handle = effective.handle(id).expect("effective handle");
        assert_eq!(
            effective.get(handle).unwrap().display_name(),
            "Overridden Brush"
        );
        assert_eq!(effective.user_definitions().len(), 1);

        effective.delete_user_state(id).expect("delete override");
        assert_eq!(
            effective.get(handle).unwrap().display_name(),
            builtin_definition.display_name()
        );
        assert!(effective.user_definitions().is_empty());
    }

    #[test]
    fn embedded_preset_override_restores_the_embedded_definition() {
        let directory = tempfile::tempdir().unwrap();
        let builtin = BrushPresetCatalog::load_effective(None).unwrap();
        let id = "builtin.brush.default";
        let original = builtin.get(builtin.handle(id).unwrap()).unwrap().clone();
        let mut overridden = original.clone();
        overridden.display_name = "Custom default".to_owned();
        fs::write(
            directory.path().join(format!("{id}.ron")),
            ron::ser::to_string(&overridden).unwrap(),
        )
        .unwrap();
        let mut effective = BrushPresetCatalog::load_effective(Some(directory.path())).unwrap();
        let handle = effective.handle(id).unwrap();
        assert_eq!(effective.get(handle), Some(&overridden));
        assert_eq!(
            effective.presets[handle.0].as_ref().unwrap().origin,
            BrushPresetOrigin::UserOverride
        );
        effective.delete_user_state(id).unwrap();
        assert_eq!(effective.get(handle), Some(&original));
        assert_eq!(
            effective.presets[handle.0].as_ref().unwrap().origin,
            BrushPresetOrigin::Builtin
        );
    }
}
