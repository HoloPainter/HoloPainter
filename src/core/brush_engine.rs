use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de, de::VariantAccess, ser::SerializeStruct,
};

use crate::core::{
    stroke_preset::{DynamicSource, PressureDynamics, StrokeStrategy},
    texture::{TextureCatalog, texture_tags_match},
};

pub const BRUSH_ENGINE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrushEngineDefinition {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub capabilities: BrushCapabilities,
    pub default_stroke: StrokeStrategy,
    pub surface_source_material_scope: SurfaceSourceMaterialScope,
    pub params: Vec<ParamSpec>,
    #[serde(default)]
    pub(crate) transient_overrides: HashMap<String, BrushTransientOverrideDefinition>,
    pub resources: HashMap<String, BrushResourceDefinition>,
    pub pipelines: HashMap<String, PipelineDefinition>,
    pub passes: HashMap<BrushSpace, Vec<PassInstance>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SurfaceSourceMaterialScope {
    AllMaterials,
    BrushFootprint {
        #[serde(default)]
        extra_radius: Vec<SurfaceSourceRadiusTerm>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SurfaceSourceRadiusTerm {
    BrushRadiusRatio { param: String, scale: f32 },
}

impl SurfaceSourceMaterialScope {
    pub fn extra_radius_px(
        &self,
        radius_px: f32,
        pressure: f32,
        params: &[(String, ParamValue)],
        param_dynamics: &[ParamDynamicsBinding],
    ) -> Option<f32> {
        match self {
            Self::AllMaterials => None,
            Self::BrushFootprint { extra_radius } => Some(
                extra_radius
                    .iter()
                    .map(|term| term.extra_radius_px(radius_px, pressure, params, param_dynamics))
                    .sum::<f32>()
                    .max(0.0),
            ),
        }
    }
}

impl SurfaceSourceRadiusTerm {
    fn extra_radius_px(
        &self,
        radius_px: f32,
        pressure: f32,
        params: &[(String, ParamValue)],
        param_dynamics: &[ParamDynamicsBinding],
    ) -> f32 {
        match self {
            Self::BrushRadiusRatio { param, scale } => {
                radius_px.max(0.0)
                    * resolve_source_scope_f32_param(params, param_dynamics, param, pressure)
                    * *scale
            }
        }
    }

    fn param_name(&self) -> &str {
        match self {
            Self::BrushRadiusRatio { param, .. } => param,
        }
    }

    fn scale(&self) -> f32 {
        match self {
            Self::BrushRadiusRatio { scale, .. } => *scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamDynamicsBinding {
    pub parameter: String,
    pub target: String,
    pub source: DynamicSource,
    pub default: PressureDynamics,
}

pub fn apply_f32_param_dynamics(
    base: f32,
    params: &[(String, ParamValue)],
    bindings: &[ParamDynamicsBinding],
    pressure: f32,
) -> f32 {
    bindings.iter().fold(base, |value, binding| {
        let dynamics = params
            .iter()
            .find_map(|(name, value)| (name == &binding.parameter).then_some(value))
            .and_then(|value| match value {
                ParamValue::Dynamics(value) => Some(value),
                _ => None,
            })
            .unwrap_or(&binding.default);
        let scale = match binding.source {
            DynamicSource::Pressure => dynamics.evaluate(pressure),
        };
        value * scale
    })
}

fn resolve_source_scope_f32_param(
    params: &[(String, ParamValue)],
    param_dynamics: &[ParamDynamicsBinding],
    name: &str,
    pressure: f32,
) -> f32 {
    let Some((_, value)) = params.iter().find(|(param, _)| param == name) else {
        panic!("validated surface_source_material_scope param {name:?} is missing");
    };
    let ParamValue::F32(base) = value else {
        panic!("validated surface_source_material_scope param {name:?} is not F32");
    };
    let bindings = param_dynamics
        .iter()
        .filter(|binding| binding.target == name)
        .cloned()
        .collect::<Vec<_>>();
    apply_f32_param_dynamics(*base, params, &bindings, pressure)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrushCapabilities {
    pub spaces: Vec<BrushSpace>,
    pub stroke_kinds: Vec<BrushStrokeKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrushSpace {
    Uv,
    Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrushStrokeKind {
    SpacingDab,
    ContinuousDab,
    RawEvent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub label: String,
    pub ty: ParamType,
    pub editor: ParamEditor,
    pub exposure: ParamExposure,
    pub default: ParamValue,
    pub range: Option<(f32, f32)>,
    pub description: Option<String>,
    pub section: Option<String>,
    pub active_when: ParamCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParamEditor {
    #[default]
    Default,
    ResourceThumbnailGrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamType {
    F32,
    Dynamics {
        target: String,
        source: DynamicSource,
    },
    Bool,
    U32,
    Enum {
        variants: Vec<EnumParamVariant>,
    },
    ResourceRef {
        kind: ResourceRefKind,
        #[serde(default)]
        tags: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumParamVariant {
    pub name: String,
    pub label: String,
    pub value: u32,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamExposure {
    Public,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceRefKind {
    Texture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BrushTransientOverrideDefinition {
    pub(crate) display_name: String,
    pub(crate) params: HashMap<String, RawParamValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamCondition {
    Always,
    Equals {
        param: String,
        value: ParamConditionValue,
    },
}

impl Default for ParamCondition {
    fn default() -> Self {
        Self::Always
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamConditionValue {
    Bool(bool),
    U32(u32),
    Enum(String),
    ResourceRef(ResourceRefValue),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Bool(bool),
    U32(u32),
    F32(f32),
    Dynamics(PressureDynamics),
    Enum(u32),
    ResourceRef(ResourceRefValue),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceRefValue {
    None,
    Resource(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawParamValue {
    None,
    Resource(String),
    Bool(bool),
    U32(u32),
    F32(f32),
    String(String),
    Dynamics(PressureDynamics),
}

impl Serialize for RawParamValue {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::None => serializer.serialize_unit_variant("RawParamValue", 0, "None"),
            Self::Resource(id) => {
                serializer.serialize_newtype_variant("RawParamValue", 1, "Resource", id)
            }
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::U32(value) => serializer.serialize_u32(*value),
            Self::F32(value) => serializer.serialize_f32(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Dynamics(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RawParamValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawParamValueVisitor;

        impl<'de> de::Visitor<'de> for RawParamValueVisitor {
            type Value = RawParamValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bool, number, string, None, or Resource(id)")
            }

            fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
                Ok(RawParamValue::Bool(value))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let value = u32::try_from(value)
                    .map_err(|_| E::custom(format!("unsigned integer {value} exceeds u32")))?;
                Ok(RawParamValue::U32(value))
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let value = u32::try_from(value).map_err(|_| {
                    E::custom(format!("integer {value} must be non-negative and fit u32"))
                })?;
                Ok(RawParamValue::U32(value))
            }

            fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RawParamValue::F32(value as f32))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
                Ok(RawParamValue::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
                Ok(RawParamValue::String(value))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
                Ok(RawParamValue::None)
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
                Ok(RawParamValue::None)
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                RawParamValue::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let id: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(2, &self));
                }
                Ok(RawParamValue::Resource(id))
            }

            fn visit_enum<A>(self, data: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: de::EnumAccess<'de>,
            {
                let (variant, access) = data.variant::<String>()?;
                match variant.as_str() {
                    "None" => {
                        access.unit_variant()?;
                        Ok(RawParamValue::None)
                    }
                    "Resource" => access.newtype_variant().map(RawParamValue::Resource),
                    _ => Err(de::Error::unknown_variant(&variant, &["None", "Resource"])),
                }
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut enabled = None;
                let mut curve = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "enabled" => enabled = Some(map.next_value::<bool>()?),
                        "curve" => curve = Some(map.next_value()?),
                        _ => {
                            return Err(de::Error::unknown_field(&key, &["enabled", "curve"]));
                        }
                    }
                }
                Ok(RawParamValue::Dynamics(PressureDynamics {
                    enabled: enabled.ok_or_else(|| de::Error::missing_field("enabled"))?,
                    curve: curve.ok_or_else(|| de::Error::missing_field("curve"))?,
                }))
            }
        }

        deserializer.deserialize_any(RawParamValueVisitor)
    }
}

impl<'de> Deserialize<'de> for ParamSpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawParamSpec {
            name: String,
            label: String,
            ty: ParamType,
            #[serde(default)]
            editor: ParamEditor,
            exposure: ParamExposure,
            default: RawParamValue,
            #[serde(default, deserialize_with = "deserialize_optional_range")]
            range: Option<(f32, f32)>,
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            section: Option<String>,
            #[serde(default)]
            active_when: ParamCondition,
        }

        let raw = RawParamSpec::deserialize(deserializer)?;
        let default = parse_param_value_for_type(&raw.ty, raw.default, "default")
            .map_err(de::Error::custom)?;

        Ok(Self {
            name: raw.name,
            label: raw.label,
            ty: raw.ty,
            editor: raw.editor,
            exposure: raw.exposure,
            default,
            range: raw.range,
            description: raw.description,
            section: raw.section,
            active_when: raw.active_when,
        })
    }
}

impl Serialize for ParamSpec {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let default =
            raw_param_value_for_spec(self, &self.default).map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("ParamSpec", 11)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("label", &self.label)?;
        state.serialize_field("ty", &self.ty)?;
        state.serialize_field("editor", &self.editor)?;
        state.serialize_field("exposure", &self.exposure)?;
        state.serialize_field("default", &default)?;
        if let Some(range) = &self.range {
            state.serialize_field("range", range)?;
        }
        state.serialize_field("description", &self.description)?;
        state.serialize_field("section", &self.section)?;
        state.serialize_field("active_when", &self.active_when)?;
        state.end()
    }
}

fn deserialize_optional_range<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<(f32, f32)>, D::Error>
where
    D: Deserializer<'de>,
{
    <(f32, f32)>::deserialize(deserializer).map(Some)
}

pub(crate) fn parse_param_value_for_type(
    ty: &ParamType,
    value: RawParamValue,
    role: &str,
) -> std::result::Result<ParamValue, String> {
    match (ty, value) {
        (ParamType::F32, RawParamValue::F32(value)) => Ok(ParamValue::F32(value)),
        (ParamType::Dynamics { .. }, RawParamValue::Dynamics(value)) => {
            Ok(ParamValue::Dynamics(value))
        }
        (ParamType::Bool, RawParamValue::Bool(value)) => Ok(ParamValue::Bool(value)),
        (ParamType::U32, RawParamValue::U32(value)) => Ok(ParamValue::U32(value)),
        (ParamType::Enum { variants }, RawParamValue::U32(value)) => {
            if variants.iter().any(|variant| variant.value == value) {
                Ok(ParamValue::Enum(value))
            } else {
                Err(format!("enum {role} value {value} is not declared"))
            }
        }
        (ParamType::Enum { variants }, RawParamValue::String(name)) => variants
            .iter()
            .find(|variant| variant.name == name)
            .map(|variant| ParamValue::Enum(variant.value))
            .ok_or_else(|| format!("enum {role} variant {name:?} is not declared")),
        (ParamType::ResourceRef { .. }, RawParamValue::None) => {
            Ok(ParamValue::ResourceRef(ResourceRefValue::None))
        }
        (ParamType::ResourceRef { .. }, RawParamValue::Resource(id)) => {
            Ok(ParamValue::ResourceRef(ResourceRefValue::Resource(id)))
        }
        (ParamType::F32, _) => Err(format!("F32 {role} must be a floating-point value")),
        (ParamType::Dynamics { .. }, _) => Err(format!("Dynamics {role} must be a dynamics value")),
        (ParamType::Bool, _) => Err(format!("Bool {role} must be a boolean value")),
        (ParamType::U32, _) => Err(format!("U32 {role} must be an unsigned integer value")),
        (ParamType::Enum { .. }, _) => Err(format!(
            "Enum {role} must be a variant name string or unsigned integer value"
        )),
        (ParamType::ResourceRef { .. }, _) => {
            Err(format!("ResourceRef {role} must be None or Resource(id)"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BrushResourceDefinition {
    Texture {
        format: TextureResourceFormat,
        extent: TextureResourceExtent,
        lifetime: TextureResourceLifetime,
        access: Vec<TextureAccess>,
    },
    SurfaceTexture {
        source: SurfaceTextureSource,
        extent: TextureResourceExtent,
        access: Vec<TextureAccess>,
        sync: SurfaceTextureSync,
    },
    TextureAsset {
        source: TextureAssetSource,
        format: TextureResourceFormat,
        access: Vec<TextureAccess>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureAccess {
    Sampled,
    RenderAttachment,
    StorageRead,
    StorageWrite,
    StorageReadWrite,
    CopySrc,
    CopyDst,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextureResourceExtent {
    PaintSurface,
    Viewport,
    Fixed { width: u32, height: u32 },
    MatchResource(String),
    ScaleOf { resource: String, scale: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SurfaceTextureSource {
    Canvas,
    ViewportColor,
    ViewportDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SurfaceTextureSync {
    Live,
    StrokeBeginSnapshot,
    BeforeEachBatch,
    BeforeEachPass,
    BeforeSurfaceViewportPass,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureAssetSource {
    RefParam { param: String },
    Static { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TextureResourceFormat {
    R8Unorm,
    Rgba8Unorm,
    Rgba16Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TextureResourceLifetime {
    Pass,
    DabBatch,
    Stroke,
    Document,
    Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrushPassDomain {
    UvTexture,
    Viewport,
    SurfaceProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassInstance {
    pub pipeline: String,
    pub domain: BrushPassDomain,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PipelineDefinition {
    pub raster: RasterMode,
    #[serde(default, deserialize_with = "deserialize_optional_dab_input")]
    pub dab_input: Option<DabInputDefinition>,
    pub inputs: HashMap<String, PassBinding>,
    pub color_targets: Vec<PassAttachment>,
    pub fragment: String,
}

impl Serialize for PipelineDefinition {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PipelineDefinition", 6)?;
        state.serialize_field("raster", &self.raster)?;
        if let Some(dab_input) = &self.dab_input {
            state.serialize_field("dab_input", dab_input)?;
        }
        state.serialize_field("inputs", &self.inputs)?;
        state.serialize_field("color_targets", &self.color_targets)?;
        state.serialize_field("fragment", &self.fragment)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DabInputDefinition {
    pub stream: DabInputStream,
    pub coordinate: DabInputCoordinate,
    pub fields: Vec<DabInputField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DabInputStream {
    CurrentBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DabInputCoordinate {
    Uv,
    Surface,
    Viewport,
    World,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DabInputField {
    Position,
    Radius,
    RadiusWorld,
    Direction,
    DirectionScreen,
    Pressure,
    Tilt,
    Velocity,
    WorldPosition,
    WorldNormal,
    TangentX,
    TangentY,
}

fn deserialize_optional_dab_input<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<DabInputDefinition>, D::Error>
where
    D: Deserializer<'de>,
{
    DabInputDefinition::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PassBinding {
    SampledTexture {
        resource: String,
        sample_type: TextureSampleType,
        view_dimension: TextureViewDimension,
        sampler: SamplerKind,
    },
    StorageTexture {
        resource: String,
        access: StorageTextureAccess,
        format: TextureResourceFormat,
        view_dimension: TextureViewDimension,
    },
    DepthTexture {
        resource: String,
        view_dimension: TextureViewDimension,
        sampler: SamplerKind,
    },
    ExternalTexture {
        resource: String,
    },
    Builtin(BuiltinPassInput),
    Sampler(SamplerKind),
    Uniform(UniformKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureSampleType {
    Float { filterable: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureViewDimension {
    D2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageTextureAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinPassInput {
    CurrentDabBatch,
    StrokeDabs,
    SelectionMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SamplerKind {
    LinearClamp,
    NearestClamp,
    LinearRepeat,
    NearestRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UniformKind {
    BrushParams,
    ViewProj,
    BakeParams,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PassAttachment {
    pub name: String,
    pub resource: String,
    pub access: TextureAccess,
    pub load: PassOutputLoad,
    #[serde(default, deserialize_with = "deserialize_optional_clear_value")]
    pub clear_value: Option<PassClearValue>,
    pub blend: PassBlend,
}

impl Serialize for PassAttachment {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PassAttachment", 6)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("resource", &self.resource)?;
        state.serialize_field("access", &self.access)?;
        state.serialize_field("load", &self.load)?;
        state.serialize_field("blend", &self.blend)?;
        if let Some(clear_value) = &self.clear_value {
            state.serialize_field("clear_value", clear_value)?;
        }
        state.end()
    }
}

impl PassAttachment {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PassOutputLoad {
    Load,
    ClearEveryPass,
    ClearOnStrokeBegin,
    DontCare,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PassClearValue {
    F32(f32),
    Rgba((f32, f32, f32, f32)),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PassBlend {
    Replace,
    AddClamp,
    Max,
    AlphaAccumulate,
}

fn deserialize_optional_clear_value<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<PassClearValue>, D::Error>
where
    D: Deserializer<'de>,
{
    PassClearValue::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RasterMode {
    FullscreenTriangle,
    TargetMeshUv,
    /// UV-space dab quad instancing. Surface passes do not have the required
    /// per-dab UV instance data and should use TargetMeshUv or FullscreenTriangle.
    UVDabQuadInstances,
    ViewportDabQuadInstances,
}

impl BrushStrokeKind {
    pub(crate) fn from_strategy(strategy: &StrokeStrategy) -> Self {
        match strategy {
            StrokeStrategy::SpacingDab { .. } => Self::SpacingDab,
            StrokeStrategy::ContinuousDab { .. } => Self::ContinuousDab,
            StrokeStrategy::RawEvent => Self::RawEvent,
        }
    }
}

pub(crate) fn raw_param_value_for_spec(
    spec: &ParamSpec,
    value: &ParamValue,
) -> std::result::Result<RawParamValue, String> {
    match (&spec.ty, value) {
        (ParamType::F32, ParamValue::F32(value)) => Ok(RawParamValue::F32(*value)),
        (ParamType::Dynamics { .. }, ParamValue::Dynamics(value)) => {
            Ok(RawParamValue::Dynamics(value.clone()))
        }
        (ParamType::Bool, ParamValue::Bool(value)) => Ok(RawParamValue::Bool(*value)),
        (ParamType::U32, ParamValue::U32(value)) => Ok(RawParamValue::U32(*value)),
        (ParamType::Enum { variants }, ParamValue::Enum(value)) => variants
            .iter()
            .find(|variant| variant.value == *value)
            .map(|variant| RawParamValue::String(variant.name.clone()))
            .ok_or_else(|| {
                format!(
                    "param {:?} enum value {} has no declared persistent variant",
                    spec.name, value
                )
            }),
        (ParamType::ResourceRef { .. }, ParamValue::ResourceRef(ResourceRefValue::None)) => {
            Ok(RawParamValue::None)
        }
        (
            ParamType::ResourceRef { .. },
            ParamValue::ResourceRef(ResourceRefValue::Resource(id)),
        ) => Ok(RawParamValue::Resource(id.clone())),
        _ => Err(format!(
            "param {:?} runtime value does not match declared type {:?}",
            spec.name, spec.ty
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrushEngineOrigin {
    Builtin,
    User,
}

#[derive(Debug, Clone, Default)]
pub struct BrushEngineRegistry {
    builtin: HashMap<String, BrushEngineDefinition>,
    user: HashMap<String, BrushEngineDefinition>,
}

impl BrushEngineRegistry {
    pub(crate) fn load_effective(
        user: Vec<BrushEngineDefinition>,
        textures: &TextureCatalog,
    ) -> Result<Self> {
        let mut registry = Self::default();

        for file in crate::embedded_resources::files_in("brushes/engines")
            .filter(|file| file.path.ends_with(".ron"))
        {
            let source_name = format!("embedded resource {:?}", file.path);
            let engine = BrushEngineDefinition::from_ron_bytes(file.bytes, &source_name)?;
            engine
                .validate(textures)
                .with_context(|| format!("validating brush engine {source_name}"))?;
            ensure!(
                !registry.builtin.contains_key(&engine.id),
                "duplicate brush engine id {:?}",
                engine.id
            );
            registry.builtin.insert(engine.id.clone(), engine);
        }

        for engine in user {
            engine
                .validate(textures)
                .with_context(|| format!("validating user brush engine {:?}", engine.id))?;
            ensure!(
                !registry.builtin.contains_key(&engine.id),
                "user brush engine {:?} conflicts with a built-in engine",
                engine.id
            );
            ensure!(
                !registry.user.contains_key(&engine.id),
                "duplicate user brush engine id {:?}",
                engine.id
            );
            registry.user.insert(engine.id.clone(), engine);
        }

        Ok(registry)
    }

    pub fn get(&self, id: &str) -> Option<&BrushEngineDefinition> {
        self.user.get(id).or_else(|| self.builtin.get(id))
    }

    pub fn engines(&self) -> impl Iterator<Item = &BrushEngineDefinition> {
        self.builtin.values().chain(self.user.values())
    }

    pub(crate) fn origin(&self, id: &str) -> Option<BrushEngineOrigin> {
        self.user
            .contains_key(id)
            .then_some(BrushEngineOrigin::User)
            .or_else(|| {
                self.builtin
                    .contains_key(id)
                    .then_some(BrushEngineOrigin::Builtin)
            })
    }

    pub(crate) fn user_definitions(&self) -> Vec<BrushEngineDefinition> {
        let mut definitions = self.user.values().cloned().collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.id.cmp(&right.id));
        definitions
    }

    pub(crate) fn insert_or_replace_user(
        &mut self,
        engine: BrushEngineDefinition,
        textures: &TextureCatalog,
    ) -> Result<Option<BrushEngineDefinition>> {
        engine.validate(textures)?;
        ensure!(
            !self.builtin.contains_key(&engine.id),
            "brush engine {:?} is built-in and cannot be replaced",
            engine.id
        );
        Ok(self.user.insert(engine.id.clone(), engine))
    }

    pub(crate) fn remove_user(&mut self, id: &str) -> Result<BrushEngineDefinition> {
        ensure!(
            !self.builtin.contains_key(id),
            "brush engine {id:?} is built-in and cannot be deleted"
        );
        self.user
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("unknown user brush engine {id:?}"))
    }

    pub(crate) fn set_validated_user(
        &mut self,
        engine: BrushEngineDefinition,
    ) -> Option<BrushEngineDefinition> {
        debug_assert!(!self.builtin.contains_key(&engine.id));
        self.user.insert(engine.id.clone(), engine)
    }

    pub fn len(&self) -> usize {
        self.builtin.len() + self.user.len()
    }

    pub fn is_empty(&self) -> bool {
        self.builtin.is_empty() && self.user.is_empty()
    }
}

impl BrushEngineDefinition {
    pub(crate) fn from_ron_bytes(bytes: &[u8], source_name: &str) -> Result<Self> {
        ron::de::from_bytes(bytes).with_context(|| format!("parsing brush engine {source_name:?}"))
    }
}

impl BrushEngineDefinition {
    pub(crate) fn supports_stroke_strategy(&self, strategy: &StrokeStrategy) -> bool {
        self.capabilities
            .stroke_kinds
            .contains(&BrushStrokeKind::from_strategy(strategy))
    }

    pub(crate) fn param_dynamics_bindings(&self) -> Vec<ParamDynamicsBinding> {
        validate_param_dynamics(&self.params).expect("validated brush engine dynamics")
    }

    pub(crate) fn default_public_params(&self) -> Result<BTreeMap<String, RawParamValue>> {
        self.params
            .iter()
            .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
            .map(|spec| {
                raw_param_value_for_spec(spec, &spec.default)
                    .map(|value| (spec.name.clone(), value))
                    .map_err(anyhow::Error::msg)
            })
            .collect()
    }

    pub fn param_is_active(&self, name: &str, params: &[(String, ParamValue)]) -> bool {
        let Some(spec) = self.params.iter().find(|spec| spec.name == name) else {
            return true;
        };
        spec.active_when.matches(&self.params, params)
    }

    pub fn validate(&self, textures: &TextureCatalog) -> Result<()> {
        ensure!(
            self.schema_version == BRUSH_ENGINE_SCHEMA_VERSION,
            "unsupported schema_version {}",
            self.schema_version
        );
        ensure!(!self.id.trim().is_empty(), "engine id must not be empty");
        ensure!(
            !self.display_name.trim().is_empty(),
            "engine display_name must not be empty"
        );
        ensure!(
            !self.capabilities.spaces.is_empty(),
            "capabilities.spaces must not be empty"
        );
        ensure!(
            !self.capabilities.stroke_kinds.is_empty(),
            "capabilities.stroke_kinds must not be empty"
        );

        let mut spaces = HashSet::new();
        for space in &self.capabilities.spaces {
            ensure!(spaces.insert(*space), "duplicate brush space {:?}", space);
            ensure!(
                self.has_pass_for_capability(*space),
                "space {:?} is declared in capabilities but has no passes",
                space
            );
        }
        for space in [BrushSpace::Uv, BrushSpace::Surface] {
            ensure!(
                spaces.contains(&space) || !self.has_pass_for_capability(space),
                "space {:?} has passes but is not declared in capabilities",
                space
            );
        }
        let mut stroke_kinds = HashSet::new();
        for kind in &self.capabilities.stroke_kinds {
            ensure!(
                stroke_kinds.insert(*kind),
                "duplicate brush stroke kind {:?}",
                kind
            );
        }
        let default_stroke_kind = BrushStrokeKind::from_strategy(&self.default_stroke);
        ensure!(
            stroke_kinds.contains(&default_stroke_kind),
            "default_stroke requires stroke kind {:?}, but capabilities.stroke_kinds does not declare it",
            default_stroke_kind
        );

        let mut params = HashSet::new();
        let mut u32_param_count = 0usize;
        for param in &self.params {
            let name = param.name.as_str();
            ensure!(!name.trim().is_empty(), "param name must not be empty");
            ensure!(
                is_valid_identifier(name),
                "param name {:?} is not a valid WGSL field identifier",
                name
            );
            ensure!(
                !matches!(
                    name,
                    "color"
                        | "base_radius_world"
                        | "base_radius_px"
                        | "base_radius"
                        | "radius"
                        | "radius_px"
                        | "radius_world"
                        | "pressure"
                        | "position"
                        | "position_px"
                        | "uv"
                        | "direction"
                        | "distance"
                        | "tex_size"
                        | "params0"
                        | "params1"
                        | "params2"
                        | "params3"
                        | "params_u32_0"
                        | "params_u32_1"
                        | "params_u32_2"
                        | "params_u32_3"
                ),
                "param name {:?} is reserved",
                name
            );
            ensure!(
                params.insert(name.to_owned()),
                "duplicate param name {:?}",
                name
            );
            ensure!(
                !param.label.trim().is_empty(),
                "param label must not be empty"
            );
            match &param.ty {
                ParamType::Bool | ParamType::U32 | ParamType::Enum { .. } => u32_param_count += 1,
                ParamType::F32 | ParamType::Dynamics { .. } | ParamType::ResourceRef { .. } => {}
            }
            ensure_param_spec_is_valid(param)?;
            if let ParamType::ResourceRef {
                kind: ResourceRefKind::Texture,
                tags,
            } = &param.ty
            {
                validate_resource_ref_value(textures, tags, &param.default)
                    .with_context(|| format!("param {:?} default", param.name))?;
            }
        }
        let param_dynamics = validate_param_dynamics(&self.params)?;
        for param in &self.params {
            validate_param_condition(param, &self.params, textures)?;
        }
        let dynamic_targets = param_dynamics
            .iter()
            .map(|binding| binding.target.as_str())
            .collect::<HashSet<_>>();
        let static_f32_param_count = self
            .params
            .iter()
            .filter(|param| {
                matches!(param.ty, ParamType::F32) && !dynamic_targets.contains(param.name.as_str())
            })
            .count();
        let dynamic_f32_param_count = self
            .params
            .iter()
            .filter(|param| {
                matches!(param.ty, ParamType::F32) && dynamic_targets.contains(param.name.as_str())
            })
            .count();
        ensure!(
            static_f32_param_count <= 16,
            "brush engines support up to 16 static F32 params"
        );
        ensure!(
            dynamic_f32_param_count <= 16,
            "brush engines support up to 16 dynamic F32 params"
        );
        ensure!(
            u32_param_count <= 16,
            "brush engines support up to 16 Bool/U32/Enum params"
        );
        validate_surface_source_material_scope(&self.surface_source_material_scope, &self.params)?;

        for (id, definition) in &self.transient_overrides {
            ensure!(
                !id.trim().is_empty(),
                "transient override id must not be empty"
            );
            ensure!(
                !definition.display_name.trim().is_empty(),
                "transient override {:?} display_name must not be empty",
                id
            );
            ensure!(
                !definition.params.is_empty(),
                "transient override {:?} params must not be empty",
                id
            );
            for (name, raw) in &definition.params {
                let spec = self
                    .params
                    .iter()
                    .find(|spec| spec.name == *name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "transient override {:?} references unknown param {:?}",
                            id,
                            name
                        )
                    })?;
                ensure!(
                    matches!(spec.exposure, ParamExposure::Public),
                    "transient override {:?} sets internal param {:?}",
                    id,
                    name
                );
                let value = parse_param_value_for_type(&spec.ty, raw.clone(), "transient override")
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("transient override {:?} param {:?}", id, name))?;
                validate_param_value_resources(textures, &spec.ty, &value)
                    .with_context(|| format!("transient override {:?} param {:?}", id, name))?;
            }
        }

        ensure!(!self.resources.is_empty(), "resources must not be empty");
        for (name, resource) in &self.resources {
            ensure!(!name.trim().is_empty(), "resource name must not be empty");
            ensure!(
                is_valid_identifier(name),
                "resource name {:?} is not a valid WGSL identifier",
                name
            );
            validate_resource_definition(name, resource, &self.params, textures)?;
        }

        ensure!(!self.pipelines.is_empty(), "pipelines must not be empty");
        for (name, pipeline) in &self.pipelines {
            self.validate_pipeline(name, pipeline)?;
        }

        for (space, passes) in &self.passes {
            for instance in passes {
                ensure!(
                    !instance.pipeline.trim().is_empty(),
                    "pass pipeline name must not be empty"
                );
                ensure!(
                    self.pipelines.contains_key(&instance.pipeline),
                    "pass references unknown pipeline {:?}",
                    instance.pipeline
                );
                match space {
                    BrushSpace::Uv => ensure!(
                        instance.domain == BrushPassDomain::UvTexture,
                        "uv pass {:?} uses non-UV domain {:?}",
                        instance.pipeline,
                        instance.domain
                    ),
                    BrushSpace::Surface => ensure!(
                        matches!(
                            instance.domain,
                            BrushPassDomain::Viewport | BrushPassDomain::SurfaceProjection
                        ),
                        "surface pass {:?} uses non-surface domain {:?}",
                        instance.pipeline,
                        instance.domain
                    ),
                }
            }
        }
        self.validate_pass_raster_domains()?;

        Ok(())
    }

    fn has_pass_for_capability(&self, space: BrushSpace) -> bool {
        self.passes
            .get(&space)
            .is_some_and(|passes| !passes.is_empty())
    }

    fn validate_pass_raster_domains(&self) -> Result<()> {
        for passes in self.passes.values() {
            for instance in passes {
                let pipeline = self
                    .pipelines
                    .get(&instance.pipeline)
                    .expect("validated pipeline exists");
                match instance.domain {
                    BrushPassDomain::UvTexture => {
                        ensure!(
                            !matches!(
                                pipeline.raster,
                                RasterMode::TargetMeshUv | RasterMode::ViewportDabQuadInstances
                            ),
                            "uv pass {:?} uses non-UV raster {:?}",
                            instance.pipeline,
                            pipeline.raster
                        );
                    }
                    BrushPassDomain::Viewport => {
                        ensure!(
                            matches!(
                                pipeline.raster,
                                RasterMode::FullscreenTriangle
                                    | RasterMode::ViewportDabQuadInstances
                            ),
                            "viewport pass {:?} uses non-viewport raster {:?}",
                            instance.pipeline,
                            pipeline.raster
                        );
                        for output in &pipeline.color_targets {
                            ensure!(
                                matches!(
                                    self.resources.get(output.resource()),
                                    Some(BrushResourceDefinition::Texture {
                                        extent: TextureResourceExtent::Viewport,
                                        ..
                                    })
                                ),
                                "viewport pass {:?} writes non-viewport texture {:?}",
                                instance.pipeline,
                                output.resource()
                            );
                        }
                    }
                    BrushPassDomain::SurfaceProjection => {
                        ensure!(
                            !matches!(
                                pipeline.raster,
                                RasterMode::UVDabQuadInstances
                                    | RasterMode::ViewportDabQuadInstances
                            ),
                            "surface pass {:?} uses dab-quad raster {:?}",
                            instance.pipeline,
                            pipeline.raster
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_pipeline(&self, name: &str, pipeline: &PipelineDefinition) -> Result<()> {
        ensure!(!name.trim().is_empty(), "pipeline name must not be empty");
        ensure!(
            is_valid_identifier(name),
            "pipeline name {:?} is not a valid identifier",
            name
        );
        ensure!(
            !pipeline.fragment.trim().is_empty(),
            "pipeline {:?} fragment must not be empty",
            name
        );
        ensure!(
            !pipeline.color_targets.is_empty(),
            "pipeline {:?} must declare color_targets",
            name
        );
        let mut input_textures = HashSet::new();
        for (binding_name, input) in &pipeline.inputs {
            ensure!(
                is_valid_identifier(binding_name),
                "pipeline {:?} input {:?} is not a valid WGSL identifier",
                name,
                binding_name
            );
            match input {
                PassBinding::SampledTexture {
                    resource,
                    sample_type,
                    view_dimension,
                    ..
                } => {
                    let resource_definition = self.resources.get(resource).ok_or_else(|| {
                        anyhow::anyhow!("pipeline {:?} reads unknown resource {:?}", name, resource)
                    })?;
                    ensure!(
                        resource_can_be_sampled(resource_definition),
                        "pipeline {:?} reads non-sampled resource {:?}",
                        name,
                        resource
                    );
                    validate_sampled_texture_binding(
                        name,
                        binding_name,
                        sample_type,
                        view_dimension,
                    )?;
                    input_textures.insert(resource.as_str());
                }
                PassBinding::StorageTexture { resource, .. } => {
                    ensure!(
                        self.resources.contains_key(resource),
                        "pipeline {:?} references unknown storage texture resource {:?}",
                        name,
                        resource
                    );
                    bail!(
                        "pipeline {:?} input {:?} uses unsupported StorageTexture binding",
                        name,
                        binding_name
                    );
                }
                PassBinding::DepthTexture { resource, .. } => {
                    ensure!(
                        self.resources.contains_key(resource),
                        "pipeline {:?} references unknown depth texture resource {:?}",
                        name,
                        resource
                    );
                    bail!(
                        "pipeline {:?} input {:?} uses unsupported DepthTexture binding",
                        name,
                        binding_name
                    );
                }
                PassBinding::ExternalTexture { resource } => {
                    ensure!(
                        self.resources.contains_key(resource),
                        "pipeline {:?} references unknown external texture resource {:?}",
                        name,
                        resource
                    );
                    bail!(
                        "pipeline {:?} input {:?} uses unsupported ExternalTexture binding",
                        name,
                        binding_name
                    );
                }
                PassBinding::Builtin(BuiltinPassInput::StrokeDabs) => {
                    bail!(
                        "pipeline {:?} uses Builtin(StrokeDabs), which is not supported yet",
                        name
                    );
                }
                PassBinding::Builtin(_) | PassBinding::Sampler(_) | PassBinding::Uniform(_) => {}
            }
        }

        validate_dab_input(name, pipeline)?;

        if pipeline.raster == RasterMode::ViewportDabQuadInstances {
            ensure_viewport_dab_quad_inputs(name, pipeline, &self.resources)?;
        }

        for output in &pipeline.color_targets {
            ensure!(
                is_valid_identifier(output.name()),
                "pipeline {:?} color target {:?} is not a valid WGSL identifier",
                name,
                output.name()
            );
            let resource_name = output.resource();
            ensure!(
                self.resources.contains_key(resource_name),
                "pipeline {:?} writes unknown resource {:?}",
                name,
                resource_name
            );
            ensure!(
                !input_textures.contains(resource_name),
                "pipeline {:?} reads and writes resource {:?} in the same pass",
                name,
                resource_name
            );
            let resource = self.resources.get(resource_name).expect("resource exists");
            ensure!(
                output.access == TextureAccess::RenderAttachment,
                "pipeline {:?} color target {:?} must declare access RenderAttachment",
                name,
                output.name()
            );
            ensure!(
                resource_can_be_render_attachment(resource),
                "resource {:?} cannot be used as a brush render attachment",
                resource_name
            );
            validate_output_state(name, output, resource_name, resource)?;
        }

        Ok(())
    }
}

fn validate_resource_definition(
    name: &str,
    resource: &BrushResourceDefinition,
    params: &[ParamSpec],
    textures: &TextureCatalog,
) -> Result<()> {
    match resource {
        BrushResourceDefinition::Texture {
            extent,
            lifetime,
            access,
            ..
        } => {
            ensure!(
                !access.is_empty(),
                "texture resource {:?} must declare access",
                name
            );
            ensure!(
                matches!(
                    extent,
                    TextureResourceExtent::PaintSurface | TextureResourceExtent::Viewport
                ),
                "texture resource {:?} uses unsupported extent {:?}",
                name,
                extent
            );
            ensure!(
                matches!(
                    lifetime,
                    TextureResourceLifetime::Pass
                        | TextureResourceLifetime::DabBatch
                        | TextureResourceLifetime::Stroke
                ),
                "texture resource {:?} uses unsupported lifetime {:?}",
                name,
                lifetime
            );
            ensure_supported_texture_access(name, access)?;
        }
        BrushResourceDefinition::SurfaceTexture {
            source,
            extent,
            access,
            sync,
        } => {
            ensure!(
                !access.is_empty(),
                "surface texture resource {:?} must declare access",
                name
            );
            ensure_supported_texture_access(name, access)?;
            match source {
                SurfaceTextureSource::Canvas => {
                    ensure!(
                        *extent == TextureResourceExtent::PaintSurface,
                        "canvas surface texture resource {:?} must use PaintSurface extent",
                        name
                    );
                    ensure!(
                        matches!(
                            sync,
                            SurfaceTextureSync::Live
                                | SurfaceTextureSync::StrokeBeginSnapshot
                                | SurfaceTextureSync::BeforeEachBatch
                                | SurfaceTextureSync::BeforeEachPass
                        ),
                        "canvas surface texture resource {:?} uses unsupported sync {:?}",
                        name,
                        sync
                    );
                }
                SurfaceTextureSource::ViewportColor | SurfaceTextureSource::ViewportDepth => {
                    ensure!(
                        *extent == TextureResourceExtent::Viewport,
                        "viewport surface texture resource {:?} must use Viewport extent",
                        name
                    );
                    ensure!(
                        *sync == SurfaceTextureSync::BeforeSurfaceViewportPass,
                        "viewport surface texture resource {:?} must sync BeforeSurfaceViewportPass",
                        name
                    );
                }
            }
        }
        BrushResourceDefinition::TextureAsset {
            source,
            format,
            access,
        } => {
            ensure!(
                !access.is_empty(),
                "texture asset resource {:?} must declare access",
                name
            );
            ensure!(
                access
                    .iter()
                    .all(|access| matches!(access, TextureAccess::Sampled)),
                "texture asset resource {:?} only supports Sampled access",
                name
            );
            ensure!(
                matches!(format, TextureResourceFormat::R8Unorm),
                "texture asset resource {:?} uses unsupported format {:?}",
                name,
                format
            );
            match source {
                TextureAssetSource::RefParam { param } => {
                    let Some(spec) = params.iter().find(|spec| spec.name == *param) else {
                        bail!(
                            "texture asset resource {:?} references unknown param {:?}",
                            name,
                            param
                        );
                    };
                    ensure!(
                        matches!(
                            &spec.ty,
                            ParamType::ResourceRef {
                                kind: ResourceRefKind::Texture,
                                ..
                            }
                        ),
                        "texture asset resource {:?} references non-texture ResourceRef param {:?}",
                        name,
                        param
                    );
                    let ParamType::ResourceRef { tags, .. } = &spec.ty else {
                        unreachable!("validated texture ResourceRef param")
                    };
                    validate_texture_asset_ref_value(textures, tags, &spec.default).with_context(
                        || {
                            format!(
                                "texture asset resource {:?} default param {:?}",
                                name, param
                            )
                        },
                    )?;
                }
                TextureAssetSource::Static { id } => {
                    ensure!(
                        textures.contains(id),
                        "texture asset resource {:?} references unknown texture {:?}",
                        name,
                        id
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_param_value_resources(
    textures: &TextureCatalog,
    ty: &ParamType,
    value: &ParamValue,
) -> Result<()> {
    match ty {
        ParamType::ResourceRef {
            kind: ResourceRefKind::Texture,
            tags,
        } => validate_resource_ref_value(textures, tags, value),
        _ => Ok(()),
    }
}

fn validate_resource_ref_value(
    textures: &TextureCatalog,
    required_tags: &[String],
    value: &ParamValue,
) -> Result<()> {
    let ParamValue::ResourceRef(value) = value else {
        return Ok(());
    };
    match value {
        ResourceRefValue::None => Ok(()),
        ResourceRefValue::Resource(id) => validate_texture_id(textures, required_tags, id),
    }
}

fn validate_texture_asset_ref_value(
    textures: &TextureCatalog,
    required_tags: &[String],
    value: &ParamValue,
) -> Result<()> {
    let ParamValue::ResourceRef(value) = value else {
        bail!("texture asset param value is not a ResourceRef");
    };
    match value {
        ResourceRefValue::None => bail!("texture asset ResourceRef must not be None"),
        ResourceRefValue::Resource(id) => validate_texture_id(textures, required_tags, id),
    }
}

fn validate_texture_id(
    textures: &TextureCatalog,
    required_tags: &[String],
    id: &str,
) -> Result<()> {
    let Some(texture) = textures.get(id) else {
        bail!("unknown texture resource {:?}", id);
    };
    ensure!(
        texture_tags_match(texture, required_tags),
        "texture resource {:?} does not match required tags {:?}",
        id,
        required_tags
    );
    Ok(())
}

fn ensure_supported_texture_access(name: &str, access: &[TextureAccess]) -> Result<()> {
    for item in access {
        ensure!(
            matches!(
                item,
                TextureAccess::Sampled
                    | TextureAccess::RenderAttachment
                    | TextureAccess::CopySrc
                    | TextureAccess::CopyDst
            ),
            "texture resource {:?} uses unsupported access {:?}",
            name,
            item
        );
    }
    Ok(())
}

fn resource_can_be_render_attachment(resource: &BrushResourceDefinition) -> bool {
    match resource {
        BrushResourceDefinition::Texture { access, .. }
        | BrushResourceDefinition::SurfaceTexture { access, .. } => {
            access.contains(&TextureAccess::RenderAttachment)
        }
        BrushResourceDefinition::TextureAsset { .. } => false,
    }
}

fn resource_can_be_sampled(resource: &BrushResourceDefinition) -> bool {
    match resource {
        BrushResourceDefinition::Texture { access, .. }
        | BrushResourceDefinition::SurfaceTexture { access, .. } => {
            access.contains(&TextureAccess::Sampled)
        }
        BrushResourceDefinition::TextureAsset { access, .. } => {
            access.contains(&TextureAccess::Sampled)
        }
    }
}

fn validate_sampled_texture_binding(
    pipeline_name: &str,
    binding_name: &str,
    sample_type: &TextureSampleType,
    view_dimension: &TextureViewDimension,
) -> Result<()> {
    ensure!(
        matches!(sample_type, TextureSampleType::Float { .. }),
        "pipeline {:?} input {:?} uses unsupported sampled texture sample type {:?}",
        pipeline_name,
        binding_name,
        sample_type
    );
    ensure!(
        *view_dimension == TextureViewDimension::D2,
        "pipeline {:?} input {:?} uses unsupported sampled texture view dimension {:?}",
        pipeline_name,
        binding_name,
        view_dimension
    );
    Ok(())
}

fn validate_dab_input(pipeline_name: &str, pipeline: &PipelineDefinition) -> Result<()> {
    let requires_dab_input = matches!(
        pipeline.raster,
        RasterMode::UVDabQuadInstances | RasterMode::ViewportDabQuadInstances
    ) || pipeline.inputs.values().any(|input| {
        matches!(
            input,
            PassBinding::Builtin(BuiltinPassInput::CurrentDabBatch)
        )
    });

    if requires_dab_input {
        ensure!(
            pipeline.dab_input.is_some(),
            "pipeline {:?} must declare dab_input for raster {:?}",
            pipeline_name,
            pipeline.raster
        );
    }
    let Some(dab_input) = &pipeline.dab_input else {
        return Ok(());
    };
    ensure!(
        !dab_input.fields.is_empty(),
        "pipeline {:?} dab_input must declare fields",
        pipeline_name
    );
    ensure!(
        dab_input.stream == DabInputStream::CurrentBatch,
        "pipeline {:?} uses unsupported dab_input stream {:?}",
        pipeline_name,
        dab_input.stream
    );
    let allowed_coordinate = match pipeline.raster {
        RasterMode::UVDabQuadInstances | RasterMode::FullscreenTriangle => DabInputCoordinate::Uv,
        RasterMode::ViewportDabQuadInstances => DabInputCoordinate::Viewport,
        RasterMode::TargetMeshUv => DabInputCoordinate::Surface,
    };
    ensure!(
        dab_input.coordinate == allowed_coordinate,
        "pipeline {:?} raster {:?} requires dab_input coordinate {:?}",
        pipeline_name,
        pipeline.raster,
        allowed_coordinate
    );
    for field in &dab_input.fields {
        ensure!(
            dab_field_supported_for_coordinate(*field, dab_input.coordinate),
            "pipeline {:?} dab_input field {:?} is not supported for coordinate {:?}",
            pipeline_name,
            field,
            dab_input.coordinate
        );
    }
    Ok(())
}

fn dab_field_supported_for_coordinate(
    field: DabInputField,
    coordinate: DabInputCoordinate,
) -> bool {
    match coordinate {
        DabInputCoordinate::Uv | DabInputCoordinate::Viewport => matches!(
            field,
            DabInputField::Position
                | DabInputField::Radius
                | DabInputField::Direction
                | DabInputField::Pressure
                | DabInputField::Tilt
                | DabInputField::Velocity
        ),
        DabInputCoordinate::Surface => matches!(
            field,
            DabInputField::WorldPosition
                | DabInputField::WorldNormal
                | DabInputField::TangentX
                | DabInputField::TangentY
                | DabInputField::RadiusWorld
                | DabInputField::Direction
                | DabInputField::DirectionScreen
                | DabInputField::Pressure
                | DabInputField::Tilt
                | DabInputField::Velocity
        ),
        DabInputCoordinate::World => matches!(
            field,
            DabInputField::WorldPosition
                | DabInputField::WorldNormal
                | DabInputField::RadiusWorld
                | DabInputField::Pressure
                | DabInputField::Tilt
                | DabInputField::Velocity
        ),
    }
}

fn ensure_viewport_dab_quad_inputs(
    pipeline_name: &str,
    pipeline: &PipelineDefinition,
    resources: &HashMap<String, BrushResourceDefinition>,
) -> Result<()> {
    ensure!(
        matches!(
            pipeline.inputs.get("viewport_depth_tex"),
            Some(PassBinding::SampledTexture { resource, .. })
                if matches!(
                    resources.get(resource),
                    Some(BrushResourceDefinition::SurfaceTexture {
                        source: SurfaceTextureSource::ViewportDepth,
                        ..
                    })
                )
        ),
        "pipeline {:?} ViewportDabQuadInstances requires input viewport_depth_tex: SampledTexture(SurfaceTexture ViewportDepth)",
        pipeline_name
    );
    ensure!(
        matches!(
            pipeline.inputs.get("brush"),
            Some(PassBinding::Uniform(UniformKind::BrushParams))
        ),
        "pipeline {:?} ViewportDabQuadInstances requires input brush: Uniform(BrushParams)",
        pipeline_name
    );
    ensure!(
        matches!(
            pipeline.inputs.get("bake"),
            Some(PassBinding::Uniform(UniformKind::BakeParams))
        ),
        "pipeline {:?} ViewportDabQuadInstances requires input bake: Uniform(BakeParams)",
        pipeline_name
    );
    Ok(())
}

fn validate_output_state(
    pipeline_name: &str,
    output: &PassAttachment,
    resource_name: &str,
    resource: &BrushResourceDefinition,
) -> Result<()> {
    let format = resource_format(resource);
    match output.load {
        PassOutputLoad::ClearEveryPass | PassOutputLoad::ClearOnStrokeBegin => ensure!(
            output.clear_value.is_some(),
            "pipeline {:?} color target {:?} must declare clear_value for {:?}",
            pipeline_name,
            output.name(),
            output.load
        ),
        PassOutputLoad::Load | PassOutputLoad::DontCare => {}
    }
    if output.load == PassOutputLoad::ClearOnStrokeBegin {
        ensure!(
            matches!(
                resource,
                BrushResourceDefinition::Texture {
                    lifetime: TextureResourceLifetime::Stroke,
                    ..
                }
            ),
            "pipeline {:?} color target {:?} uses ClearOnStrokeBegin on non-stroke resource {:?}",
            pipeline_name,
            output.name(),
            resource_name
        );
    }
    if let Some(clear_value) = &output.clear_value {
        match (format, clear_value) {
            (TextureResourceFormat::R8Unorm, PassClearValue::F32(_))
            | (TextureResourceFormat::Rgba8Unorm, PassClearValue::Rgba(_))
            | (TextureResourceFormat::Rgba16Float, PassClearValue::Rgba(_)) => {}
            _ => bail!(
                "pipeline {:?} color target {:?} clear_value does not match resource format",
                pipeline_name,
                output.name()
            ),
        }
    }
    match (format, output.blend) {
        (_, PassBlend::Replace)
        | (TextureResourceFormat::R8Unorm, PassBlend::AddClamp)
        | (TextureResourceFormat::R8Unorm, PassBlend::Max)
        | (TextureResourceFormat::R8Unorm, PassBlend::AlphaAccumulate)
        | (TextureResourceFormat::Rgba16Float, PassBlend::AddClamp) => Ok(()),
        _ => bail!(
            "pipeline {:?} color target {:?} blend {:?} is not supported for resource format {:?}",
            pipeline_name,
            output.name(),
            output.blend,
            format
        ),
    }
}

pub fn resource_format(resource: &BrushResourceDefinition) -> TextureResourceFormat {
    match resource {
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::Canvas | SurfaceTextureSource::ViewportColor,
            ..
        } => TextureResourceFormat::Rgba8Unorm,
        BrushResourceDefinition::SurfaceTexture {
            source: SurfaceTextureSource::ViewportDepth,
            ..
        } => TextureResourceFormat::R8Unorm,
        BrushResourceDefinition::Texture { format, .. }
        | BrushResourceDefinition::TextureAsset { format, .. } => *format,
    }
}

fn is_valid_identifier(value: &str) -> bool {
    // TODO: WGSL keyword validation
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn validate_surface_source_material_scope(
    scope: &SurfaceSourceMaterialScope,
    params: &[ParamSpec],
) -> Result<()> {
    match scope {
        SurfaceSourceMaterialScope::AllMaterials => Ok(()),
        SurfaceSourceMaterialScope::BrushFootprint { extra_radius } => {
            for term in extra_radius {
                ensure!(
                    term.scale().is_finite() && term.scale() >= 0.0,
                    "surface_source_material_scope radius term for param {:?} has invalid scale {}",
                    term.param_name(),
                    term.scale()
                );
                let Some(param) = params.iter().find(|param| param.name == term.param_name())
                else {
                    bail!(
                        "surface_source_material_scope references unknown param {:?}",
                        term.param_name()
                    );
                };
                ensure!(
                    matches!(&param.ty, ParamType::F32),
                    "surface_source_material_scope param {:?} must be F32",
                    param.name
                );
                ensure!(
                    matches!(param.exposure, ParamExposure::Public),
                    "surface_source_material_scope param {:?} must be public",
                    param.name
                );
            }
            Ok(())
        }
    }
}

impl ParamCondition {
    fn matches(&self, specs: &[ParamSpec], params: &[(String, ParamValue)]) -> bool {
        match self {
            Self::Always => true,
            Self::Equals { param, value } => {
                let spec = specs
                    .iter()
                    .find(|spec| spec.name == *param)
                    .expect("validated active_when param");
                let expected = resolve_condition_value(spec, value)
                    .expect("validated active_when condition value");
                let actual = params
                    .iter()
                    .find_map(|(name, value)| (name == param).then_some(value))
                    .unwrap_or(&spec.default);
                actual == &expected
            }
        }
    }
}

fn validate_param_condition(
    owner: &ParamSpec,
    specs: &[ParamSpec],
    textures: &TextureCatalog,
) -> Result<()> {
    let ParamCondition::Equals { param, value } = &owner.active_when else {
        return Ok(());
    };
    ensure!(
        !param.trim().is_empty(),
        "param {:?} active_when param must not be empty",
        owner.name
    );
    ensure!(
        param != &owner.name,
        "param {:?} active_when must not reference itself",
        owner.name
    );
    let source = specs
        .iter()
        .find(|spec| spec.name == *param)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "param {:?} active_when references unknown param {:?}",
                owner.name,
                param
            )
        })?;
    ensure!(
        matches!(source.exposure, ParamExposure::Public),
        "param {:?} active_when references internal param {:?}",
        owner.name,
        param
    );
    let expected = resolve_condition_value(source, value).with_context(|| {
        format!(
            "param {:?} active_when condition for {:?}",
            owner.name, param
        )
    })?;
    validate_param_value_resources(textures, &source.ty, &expected).with_context(|| {
        format!(
            "param {:?} active_when condition for {:?}",
            owner.name, param
        )
    })?;
    Ok(())
}

fn resolve_condition_value(spec: &ParamSpec, value: &ParamConditionValue) -> Result<ParamValue> {
    match (&spec.ty, value) {
        (ParamType::Bool, ParamConditionValue::Bool(value)) => Ok(ParamValue::Bool(*value)),
        (ParamType::U32, ParamConditionValue::U32(value)) => Ok(ParamValue::U32(*value)),
        (ParamType::Enum { variants }, ParamConditionValue::Enum(name)) => variants
            .iter()
            .find(|variant| variant.name == *name)
            .map(|variant| ParamValue::Enum(variant.value))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "enum condition value {:?} is not declared for param {:?}",
                    name,
                    spec.name
                )
            }),
        (ParamType::ResourceRef { .. }, ParamConditionValue::ResourceRef(value)) => {
            Ok(ParamValue::ResourceRef(value.clone()))
        }
        _ => bail!(
            "condition value {:?} does not match param {:?} type {:?}",
            value,
            spec.name,
            spec.ty
        ),
    }
}

fn ensure_param_spec_is_valid(param: &ParamSpec) -> Result<()> {
    ensure_param_value_matches_type(param, &param.default, "default")?;
    if matches!(param.editor, ParamEditor::ResourceThumbnailGrid) {
        ensure!(
            matches!(
                &param.ty,
                ParamType::ResourceRef {
                    kind: ResourceRefKind::Texture,
                    ..
                }
            ),
            "param {:?} ResourceThumbnailGrid editor requires a Texture ResourceRef",
            param.name
        );
    }
    match &param.ty {
        ParamType::F32 => {
            let ParamValue::F32(default) = &param.default else {
                unreachable!("validated F32 default")
            };
            ensure!(
                default.is_finite(),
                "param {:?} default must be finite",
                param.name
            );
            if let Some((min, max)) = param.range {
                ensure!(
                    min.is_finite() && max.is_finite(),
                    "param {:?} range values must be finite",
                    param.name
                );
                ensure!(
                    min <= max,
                    "param {:?} range min must be <= max",
                    param.name
                );
                ensure!(
                    *default >= min && *default <= max,
                    "param {:?} default must be inside range",
                    param.name
                );
            }
        }
        ParamType::Dynamics { source, .. } => {
            ensure!(
                param.range.is_none(),
                "param {:?} range is not supported for Dynamics",
                param.name
            );
            ensure!(
                matches!(source, DynamicSource::Pressure),
                "param {:?} uses unsupported dynamic source {:?}",
                param.name,
                source
            );
            let ParamValue::Dynamics(default) = &param.default else {
                unreachable!("validated Dynamics default")
            };
            validate_pressure_dynamics(&param.name, default)?;
        }
        ParamType::Bool => ensure_param_has_no_range(param)?,
        ParamType::U32 => {
            let ParamValue::U32(default) = &param.default else {
                unreachable!("validated U32 default")
            };
            let _ = default;
            ensure_param_has_no_range(param)?;
        }
        ParamType::Enum { variants } => {
            ensure_param_has_no_range(param)?;
            ensure!(
                !variants.is_empty(),
                "param {:?} enum variants must not be empty",
                param.name
            );
            let ParamValue::Enum(default) = &param.default else {
                unreachable!("validated Enum default")
            };
            let mut names = HashSet::new();
            let mut values = HashSet::new();
            for variant in variants {
                ensure!(
                    !variant.name.trim().is_empty(),
                    "param {:?} enum variant name must not be empty",
                    param.name
                );
                ensure!(
                    is_valid_identifier(&variant.name),
                    "param {:?} enum variant {:?} is not a valid WGSL identifier",
                    param.name,
                    variant.name
                );
                ensure!(
                    !variant.label.trim().is_empty(),
                    "param {:?} enum variant label must not be empty",
                    param.name
                );
                ensure!(
                    names.insert(variant.name.clone()),
                    "param {:?} has duplicate enum variant {:?}",
                    param.name,
                    variant.name
                );
                ensure!(
                    values.insert(variant.value),
                    "param {:?} has duplicate enum value {}",
                    param.name,
                    variant.value
                );
            }
            ensure!(
                values.contains(default),
                "param {:?} enum default value {} is not declared",
                param.name,
                default
            );
        }
        ParamType::ResourceRef { .. } => ensure_param_has_no_range(param)?,
    }
    Ok(())
}

fn validate_pressure_dynamics(name: &str, dynamics: &PressureDynamics) -> Result<()> {
    dynamics.curve.validate().map_err(|error| {
        anyhow::anyhow!("param {:?} dynamics curve is invalid: {}", name, error)
    })?;
    Ok(())
}

fn validate_param_dynamics(params: &[ParamSpec]) -> Result<Vec<ParamDynamicsBinding>> {
    let mut seen = HashSet::new();
    let mut bindings = Vec::new();
    for param in params {
        let ParamType::Dynamics { target, source } = &param.ty else {
            continue;
        };
        ensure!(
            target != &param.name,
            "dynamics param {:?} must not target itself",
            param.name
        );
        let target_spec = params
            .iter()
            .find(|candidate| candidate.name == *target)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "dynamics param {:?} references unknown target {:?}",
                    param.name,
                    target
                )
            })?;
        ensure!(
            matches!(target_spec.ty, ParamType::F32),
            "dynamics param {:?} target {:?} must be F32",
            param.name,
            target
        );
        ensure!(
            seen.insert((target.clone(), *source)),
            "duplicate dynamics source {:?} for target {:?}",
            source,
            target
        );
        let ParamValue::Dynamics(default) = &param.default else {
            unreachable!("validated Dynamics default")
        };
        bindings.push(ParamDynamicsBinding {
            parameter: param.name.clone(),
            target: target.clone(),
            source: *source,
            default: default.clone(),
        });
    }
    Ok(bindings)
}

fn ensure_param_value_matches_type(
    param: &ParamSpec,
    value: &ParamValue,
    role: &str,
) -> Result<()> {
    let matches = matches!(
        (&param.ty, value),
        (ParamType::F32, ParamValue::F32(_))
            | (ParamType::Dynamics { .. }, ParamValue::Dynamics(_))
            | (ParamType::Bool, ParamValue::Bool(_))
            | (ParamType::U32, ParamValue::U32(_))
            | (ParamType::Enum { .. }, ParamValue::Enum(_))
            | (ParamType::ResourceRef { .. }, ParamValue::ResourceRef(_))
    );
    ensure!(
        matches,
        "param {:?} {} value does not match declared type {:?}",
        param.name,
        role,
        param.ty
    );
    Ok(())
}

fn ensure_param_has_no_range(param: &ParamSpec) -> Result<()> {
    ensure!(
        param.range.is_none(),
        "param {:?} range is not supported for {:?}",
        param.name,
        param.ty
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_engine_defaults_tip_rotation_to_fixed_and_accepts_surface_direction() {
        let textures = TextureCatalog::from_image_assets(
            &crate::core::image_asset::ImageAssetCatalog::load_effective(None).unwrap(),
        )
        .unwrap();
        let engine: BrushEngineDefinition =
            ron::from_str(crate::embedded_resources::text("brushes/engines/paint.ron").unwrap())
                .unwrap();

        engine.validate(&textures).unwrap();
        let tip_rotation = engine
            .params
            .iter()
            .find(|param| param.name == "tip_rotation")
            .expect("paint engine tip_rotation param");
        assert_eq!(tip_rotation.default, ParamValue::Enum(0));
        assert!(matches!(
            tip_rotation.active_when,
            ParamCondition::Equals {
                ref param,
                value: ParamConditionValue::Enum(ref value),
            } if param == "shape" && value == "texture_mask"
        ));
        let surface = engine
            .pipelines
            .get("surface_stroke_mask")
            .and_then(|pipeline| pipeline.dab_input.as_ref())
            .expect("surface paint dab input");
        assert!(surface.fields.contains(&DabInputField::Direction));
    }

    fn minimal_engine(space: BrushSpace, domain: BrushPassDomain) -> BrushEngineDefinition {
        let mut resources = HashMap::new();
        resources.insert(
            "target".to_owned(),
            BrushResourceDefinition::Texture {
                format: TextureResourceFormat::R8Unorm,
                extent: TextureResourceExtent::PaintSurface,
                lifetime: TextureResourceLifetime::Pass,
                access: vec![TextureAccess::RenderAttachment],
            },
        );

        let mut pipelines = HashMap::new();
        pipelines.insert(
            "main".to_owned(),
            PipelineDefinition {
                raster: RasterMode::FullscreenTriangle,
                dab_input: None,
                inputs: HashMap::new(),
                color_targets: vec![PassAttachment {
                    name: "color".to_owned(),
                    resource: "target".to_owned(),
                    access: TextureAccess::RenderAttachment,
                    load: PassOutputLoad::DontCare,
                    clear_value: None,
                    blend: PassBlend::Replace,
                }],
                fragment: "return PassOutput();".to_owned(),
            },
        );

        let mut passes = HashMap::new();
        passes.insert(
            space,
            vec![PassInstance {
                pipeline: "main".to_owned(),
                domain,
            }],
        );

        BrushEngineDefinition {
            schema_version: BRUSH_ENGINE_SCHEMA_VERSION,
            id: "test".to_owned(),
            display_name: "Test".to_owned(),
            description: None,
            capabilities: BrushCapabilities {
                spaces: vec![space],
                stroke_kinds: vec![BrushStrokeKind::SpacingDab],
            },
            default_stroke: StrokeStrategy::SpacingDab { spacing: 0.25 },
            surface_source_material_scope: SurfaceSourceMaterialScope::BrushFootprint {
                extra_radius: Vec::new(),
            },
            params: Vec::new(),
            transient_overrides: HashMap::new(),
            resources,
            pipelines,
            passes,
        }
    }

    #[test]
    fn active_when_tracks_enum_param_value() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        engine.params = vec![
            ParamSpec {
                name: "shape".to_owned(),
                label: "Shape".to_owned(),
                ty: ParamType::Enum {
                    variants: vec![
                        EnumParamVariant {
                            name: "procedural_round".to_owned(),
                            label: "Round".to_owned(),
                            value: 0,
                            description: None,
                        },
                        EnumParamVariant {
                            name: "texture_mask".to_owned(),
                            label: "Texture Mask".to_owned(),
                            value: 1,
                            description: None,
                        },
                    ],
                },
                editor: ParamEditor::Default,
                exposure: ParamExposure::Public,
                default: ParamValue::Enum(0),
                range: None,
                description: None,
                section: None,
                active_when: ParamCondition::Always,
            },
            ParamSpec {
                name: "hardness".to_owned(),
                label: "Hardness".to_owned(),
                ty: ParamType::Bool,
                editor: ParamEditor::Default,
                exposure: ParamExposure::Public,
                default: ParamValue::Bool(true),
                range: None,
                description: None,
                section: None,
                active_when: ParamCondition::Equals {
                    param: "shape".to_owned(),
                    value: ParamConditionValue::Enum("procedural_round".to_owned()),
                },
            },
        ];
        engine.validate(&TextureCatalog::default()).unwrap();

        assert!(engine.param_is_active("hardness", &[("shape".to_owned(), ParamValue::Enum(0))]));
        assert!(!engine.param_is_active("hardness", &[("shape".to_owned(), ParamValue::Enum(1))]));
    }

    #[test]
    fn active_when_rejects_mismatched_condition_value() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        engine.params = vec![
            ParamSpec {
                name: "shape".to_owned(),
                label: "Shape".to_owned(),
                ty: ParamType::Enum {
                    variants: vec![EnumParamVariant {
                        name: "round".to_owned(),
                        label: "Round".to_owned(),
                        value: 0,
                        description: None,
                    }],
                },
                editor: ParamEditor::Default,
                exposure: ParamExposure::Public,
                default: ParamValue::Enum(0),
                range: None,
                description: None,
                section: None,
                active_when: ParamCondition::Always,
            },
            ParamSpec {
                name: "dependent".to_owned(),
                label: "Dependent".to_owned(),
                ty: ParamType::Bool,
                editor: ParamEditor::Default,
                exposure: ParamExposure::Public,
                default: ParamValue::Bool(true),
                range: None,
                description: None,
                section: None,
                active_when: ParamCondition::Equals {
                    param: "shape".to_owned(),
                    value: ParamConditionValue::Bool(true),
                },
            },
        ];

        let error = engine.validate(&TextureCatalog::default()).unwrap_err();

        let message = format!("{error:#}");
        assert!(message.contains("condition value"), "{message}");
    }

    fn f32_param(name: &str, default: f32) -> ParamSpec {
        ParamSpec {
            name: name.to_owned(),
            label: name.to_owned(),
            ty: ParamType::F32,
            editor: ParamEditor::Default,
            exposure: ParamExposure::Public,
            default: ParamValue::F32(default),
            range: Some((0.0, 1.0)),
            description: None,
            section: None,
            active_when: ParamCondition::Always,
        }
    }

    fn pressure_param(name: &str, target: &str, enabled: bool) -> ParamSpec {
        ParamSpec {
            name: name.to_owned(),
            label: name.to_owned(),
            ty: ParamType::Dynamics {
                target: target.to_owned(),
                source: DynamicSource::Pressure,
            },
            editor: ParamEditor::Default,
            exposure: ParamExposure::Public,
            default: ParamValue::Dynamics(PressureDynamics {
                enabled,
                curve: Default::default(),
            }),
            range: None,
            description: None,
            section: None,
            active_when: ParamCondition::Always,
        }
    }

    #[test]
    fn dynamics_param_is_separate_from_its_f32_target() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        engine.params = vec![
            f32_param("flow", 0.8),
            pressure_param("flow_pressure", "flow", true),
        ];

        engine.validate(&TextureCatalog::default()).unwrap();
        let bindings = engine.param_dynamics_bindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].parameter, "flow_pressure");
        assert_eq!(bindings[0].target, "flow");

        let params = vec![
            ("flow".to_owned(), ParamValue::F32(0.8)),
            (
                "flow_pressure".to_owned(),
                ParamValue::Dynamics(PressureDynamics::enabled()),
            ),
        ];
        assert!((apply_f32_param_dynamics(0.8, &params, &bindings, 0.5) - 0.4).abs() < 1.0e-6);
    }

    #[test]
    fn dynamics_param_rejects_unknown_or_non_f32_targets() {
        let mut missing = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        missing.params = vec![pressure_param("flow_pressure", "flow", true)];
        let error = missing.validate(&TextureCatalog::default()).unwrap_err();
        assert!(error.to_string().contains("unknown target"), "{error:#}");

        let mut non_f32 = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        let mut target = f32_param("flow", 0.8);
        target.ty = ParamType::Bool;
        target.default = ParamValue::Bool(true);
        target.range = None;
        non_f32.params = vec![target, pressure_param("flow_pressure", "flow", true)];
        let error = non_f32.validate(&TextureCatalog::default()).unwrap_err();
        assert!(error.to_string().contains("must be F32"), "{error:#}");
    }

    #[test]
    fn dynamics_param_rejects_self_target_and_duplicate_source() {
        let mut self_target = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        self_target.params = vec![pressure_param("flow_pressure", "flow_pressure", true)];
        let error = self_target
            .validate(&TextureCatalog::default())
            .unwrap_err();
        assert!(
            error.to_string().contains("must not target itself"),
            "{error:#}"
        );

        let mut duplicate = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        duplicate.params = vec![
            f32_param("flow", 0.8),
            pressure_param("flow_pressure", "flow", true),
            pressure_param("flow_pressure_alt", "flow", false),
        ];
        let error = duplicate.validate(&TextureCatalog::default()).unwrap_err();
        assert!(
            error.to_string().contains("duplicate dynamics source"),
            "{error:#}"
        );
    }

    #[test]
    fn dynamics_param_ron_parses_as_an_independent_parameter() {
        let source = r#"(
            name: "flow_pressure",
            label: "Flow Pressure",
            ty: Dynamics(target: "flow", source: Pressure),
            exposure: Public,
            default: (
                enabled: true,
                curve: [(x: 0.0, y: 0.0), (x: 1.0, y: 1.0)],
            ),
        )"#;

        let spec: ParamSpec = ron::from_str(source).unwrap();
        assert!(
            matches!(spec.ty, ParamType::Dynamics { ref target, source: DynamicSource::Pressure } if target == "flow")
        );
        assert!(matches!(
            spec.default,
            ParamValue::Dynamics(PressureDynamics { enabled: true, .. })
        ));
    }

    #[test]
    fn param_editor_defaults_and_thumbnail_grid_deserializes() {
        let default_source = r#"(
            name: "flow",
            label: "Flow",
            ty: F32,
            exposure: Public,
            default: 0.8,
            range: (0.0, 1.0),
        )"#;
        let default_spec: ParamSpec = ron::from_str(default_source).unwrap();
        assert_eq!(default_spec.editor, ParamEditor::Default);

        let thumbnail_source = r#"(
            name: "tip",
            label: "Tip",
            ty: ResourceRef(kind: Texture, tags: ["brush_tip_mask"]),
            editor: ResourceThumbnailGrid,
            exposure: Public,
            default: None,
        )"#;
        let thumbnail_spec: ParamSpec = ron::from_str(thumbnail_source).unwrap();
        assert_eq!(thumbnail_spec.editor, ParamEditor::ResourceThumbnailGrid);
    }

    #[test]
    fn thumbnail_grid_editor_requires_texture_resource_ref() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        let mut invalid = f32_param("flow", 0.8);
        invalid.editor = ParamEditor::ResourceThumbnailGrid;
        engine.params = vec![invalid];

        let error = engine.validate(&TextureCatalog::default()).unwrap_err();

        assert!(
            error.to_string().contains("requires a Texture ResourceRef"),
            "{error:#}"
        );
    }

    #[test]
    fn thumbnail_grid_editor_accepts_texture_resource_ref() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        engine.params = vec![ParamSpec {
            name: "tip".to_owned(),
            label: "Tip".to_owned(),
            ty: ParamType::ResourceRef {
                kind: ResourceRefKind::Texture,
                tags: vec!["brush_tip_mask".to_owned()],
            },
            editor: ParamEditor::ResourceThumbnailGrid,
            exposure: ParamExposure::Public,
            default: ParamValue::ResourceRef(ResourceRefValue::None),
            range: None,
            description: None,
            section: None,
            active_when: ParamCondition::Always,
        }];

        engine.validate(&TextureCatalog::default()).unwrap();
    }

    #[test]
    fn surface_source_scope_applies_separate_pressure_binding() {
        let scope = SurfaceSourceMaterialScope::BrushFootprint {
            extra_radius: vec![SurfaceSourceRadiusTerm::BrushRadiusRatio {
                param: "flow".to_owned(),
                scale: 2.0,
            }],
        };
        let params = vec![
            ("flow".to_owned(), ParamValue::F32(0.8)),
            (
                "flow_pressure".to_owned(),
                ParamValue::Dynamics(PressureDynamics::enabled()),
            ),
        ];
        let bindings = vec![ParamDynamicsBinding {
            parameter: "flow_pressure".to_owned(),
            target: "flow".to_owned(),
            source: DynamicSource::Pressure,
            default: PressureDynamics::default(),
        }];

        let extra = scope
            .extra_radius_px(10.0, 0.5, &params, &bindings)
            .unwrap();
        assert!((extra - 8.0).abs() < 1.0e-6);
    }

    #[test]
    fn grouped_passes_deserialize_from_brush_space_keys() {
        let source = r#"
(
    schema_version: 1,
    id: "test",
    display_name: "Test",
    capabilities: (
        spaces: [Uv],
        stroke_kinds: [SpacingDab],
    ),
    default_stroke: SpacingDab(spacing: 0.25),
    surface_source_material_scope: BrushFootprint(extra_radius: []),
    params: [],
    resources: {
        "target": Texture(
            format: R8Unorm,
            extent: PaintSurface,
            lifetime: Pass,
            access: [RenderAttachment],
        ),
    },
    pipelines: {
        "main": (
            raster: FullscreenTriangle,
            inputs: {},
            color_targets: [
                (
                    name: "color",
                    resource: "target",
                    access: RenderAttachment,
                    load: DontCare,
                    blend: Replace,
                ),
            ],
            fragment: "return PassOutput();",
        ),
    },
    passes: {
        Uv: [
            (pipeline: "main", domain: UvTexture),
        ],
    },
)
"#;

        let engine: BrushEngineDefinition = ron::from_str(source).unwrap();

        assert_eq!(
            engine.passes[&BrushSpace::Uv][0].domain,
            BrushPassDomain::UvTexture
        );
    }

    #[test]
    fn uv_group_rejects_non_uv_domain() {
        let engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::Viewport);

        let error = engine.validate(&TextureCatalog::default()).unwrap_err();

        assert!(
            error.to_string().contains("uses non-UV domain"),
            "{error:?}"
        );
    }

    #[test]
    fn surface_group_rejects_uv_texture_domain() {
        let engine = minimal_engine(BrushSpace::Surface, BrushPassDomain::UvTexture);

        let error = engine.validate(&TextureCatalog::default()).unwrap_err();

        assert!(
            error.to_string().contains("uses non-surface domain"),
            "{error:?}"
        );
    }
    #[test]
    fn default_stroke_must_be_declared_by_capabilities() {
        let mut engine = minimal_engine(BrushSpace::Uv, BrushPassDomain::UvTexture);
        engine.default_stroke = StrokeStrategy::ContinuousDab {
            spacing: 0.1,
            rate_hz: 60.0,
        };

        let error = engine.validate(&TextureCatalog::default()).unwrap_err();

        assert!(error.to_string().contains("default_stroke"));
    }

    #[test]
    fn engine_ron_requires_default_stroke_in_current_schema() {
        let source = r#"(
            schema_version: 1,
            id: "test",
            display_name: "Test",
            capabilities: (spaces: [Uv], stroke_kinds: [SpacingDab]),
            surface_source_material_scope: BrushFootprint(extra_radius: []),
            params: [],
            resources: {},
            pipelines: {},
            passes: {},
        )"#;

        let error = ron::from_str::<BrushEngineDefinition>(source).unwrap_err();

        assert!(error.to_string().contains("default_stroke"));
    }

    #[test]
    fn effective_registry_tracks_user_origin_and_rejects_builtin_override() {
        let textures = crate::core::texture::TextureCatalog::from_image_assets(
            &crate::core::image_asset::ImageAssetCatalog::load_effective(None).unwrap(),
        )
        .unwrap();
        let builtin = BrushEngineRegistry::load_effective(Vec::new(), &textures).unwrap();
        let mut user = builtin.get("paint").unwrap().clone();
        user.id = "user.engine.test".to_owned();
        let effective = BrushEngineRegistry::load_effective(vec![user], &textures).unwrap();
        assert_eq!(effective.origin("paint"), Some(BrushEngineOrigin::Builtin));
        assert_eq!(
            effective.origin("user.engine.test"),
            Some(BrushEngineOrigin::User)
        );

        let builtin_override = builtin.get("paint").unwrap().clone();
        assert!(
            BrushEngineRegistry::load_effective(vec![builtin_override], &textures)
                .unwrap_err()
                .to_string()
                .contains("conflicts with a built-in")
        );
    }
}
