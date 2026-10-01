use crate::core::brush_engine::{
    ParamDynamicsBinding, ParamValue, ResourceRefValue, SurfaceSourceMaterialScope,
};
use crate::core::curve::Curve;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommonBrushParam {
    Size,
    SizePressure,
    Spacing,
    SprayRate,
    Stabilization,
    PressureFilter,
    PressureFeel,
}

impl CommonBrushParam {
    pub fn is_available_for(self, strategy: &StrokeStrategy) -> bool {
        match self {
            Self::Spacing => matches!(
                strategy,
                StrokeStrategy::SpacingDab { .. } | StrokeStrategy::ContinuousDab { .. }
            ),
            Self::SprayRate => matches!(strategy, StrokeStrategy::ContinuousDab { .. }),
            Self::Size
            | Self::SizePressure
            | Self::Stabilization
            | Self::PressureFilter
            | Self::PressureFeel => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuickParamTarget {
    Common(CommonBrushParam),
    Engine(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StrokeStrategy {
    /// 距離ベースの等間隔配置
    SpacingDab { spacing: f32 },
    /// 距離配置に加えて、押下中は時間経過でも同じ位置へ配置
    ContinuousDab { spacing: f32, rate_hz: f32 },
    /// 入力イベントごとにそのまま配置（補間なしのテスト用ダミー）
    RawEvent,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InputInfluence {
    pub enabled: bool,
    pub min: f32,
}

impl Default for InputInfluence {
    fn default() -> Self {
        Self {
            enabled: false,
            min: 1.0,
        }
    }
}

impl InputInfluence {
    pub fn pressure_default(min: f32) -> Self {
        Self { enabled: true, min }
    }

    pub fn evaluate(&self, input: f32) -> f32 {
        if self.enabled {
            let min = self.min.clamp(0.0, 1.0);
            min + (1.0 - min) * input.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DynamicSource {
    Pressure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureDynamics {
    pub enabled: bool,
    pub curve: Curve,
}

impl PressureDynamics {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            curve: Curve::default(),
        }
    }

    pub fn evaluate(&self, pressure: f32) -> f32 {
        if self.enabled {
            self.curve.evaluate(pressure)
        } else {
            1.0
        }
    }
}

impl Default for PressureDynamics {
    fn default() -> Self {
        Self {
            enabled: false,
            curve: Curve::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangedF32 {
    pub value: f32,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_range",
        serialize_with = "serialize_optional_range",
        skip_serializing_if = "Option::is_none"
    )]
    pub range: Option<(f32, f32)>,
}

pub const DEFAULT_DIAMETER_SCENE_RATIO_RANGE: (f32, f32) = (0.0005, 0.2);

fn deserialize_optional_range<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<(f32, f32)>, D::Error>
where
    D: Deserializer<'de>,
{
    <(f32, f32)>::deserialize(deserializer).map(Some)
}

fn serialize_optional_range<S>(
    range: &Option<(f32, f32)>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match range {
        Some(range) => range.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrokePresetOp {
    BrushEngine {
        engine_id: String,
        /// Brush size divided by the imported scene AABB diagonal.
        size_scene_ratio: RangedF32,
        size_pressure: PressureDynamics,
        params: Vec<(String, ParamValue)>,
        param_dynamics: Vec<ParamDynamicsBinding>,
        surface_source_material_scope: SurfaceSourceMaterialScope,
    },
}

impl StrokePresetOp {
    pub fn size_scene_ratio(&self) -> f32 {
        match self {
            Self::BrushEngine {
                size_scene_ratio, ..
            } => size_scene_ratio.value,
        }
    }

    pub fn set_size_scene_ratio(&mut self, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let Self::BrushEngine {
            size_scene_ratio, ..
        } = self;
        let (minimum, maximum) = size_scene_ratio
            .range
            .unwrap_or(DEFAULT_DIAMETER_SCENE_RATIO_RANGE);
        let value = value.clamp(minimum, maximum);
        if size_scene_ratio.value == value {
            return false;
        }
        size_scene_ratio.value = value;
        true
    }

    pub fn radius_world(&self, scene_diagonal: f32) -> f32 {
        let scene_radius = if scene_diagonal.is_finite() {
            scene_diagonal.max(0.0) * 0.5
        } else {
            0.0
        };
        self.size_scene_ratio().max(0.0) * scene_radius
    }

    pub fn radius_scale(&self, pressure: f32) -> f32 {
        match self {
            Self::BrushEngine { size_pressure, .. } => size_pressure.evaluate(pressure),
        }
    }

    pub fn resolve(&self, scene_diagonal: f32) -> StrokeOp {
        let scene_radius = if scene_diagonal.is_finite() {
            scene_diagonal.max(0.0) * 0.5
        } else {
            0.0
        };
        match self {
            Self::BrushEngine {
                engine_id,
                size_scene_ratio,
                size_pressure,
                params,
                param_dynamics,
                surface_source_material_scope,
            } => StrokeOp::BrushEngine {
                engine_id: engine_id.clone(),
                radius_world: size_scene_ratio.value.max(0.0) * scene_radius,
                radius_pressure: size_pressure.clone(),
                params: params.clone(),
                param_dynamics: param_dynamics.clone(),
                surface_source_material_scope: surface_source_material_scope.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrokeOp {
    BrushEngine {
        engine_id: String,
        radius_world: f32,
        radius_pressure: PressureDynamics,
        params: Vec<(String, ParamValue)>,
        param_dynamics: Vec<ParamDynamicsBinding>,
        surface_source_material_scope: SurfaceSourceMaterialScope,
    },
}

impl StrokeOp {
    pub fn radius_world(&self) -> f32 {
        match self {
            StrokeOp::BrushEngine { radius_world, .. } => *radius_world,
        }
    }

    pub fn radius_scale(&self, pressure: f32) -> f32 {
        match self {
            StrokeOp::BrushEngine {
                radius_pressure, ..
            } => radius_pressure.evaluate(pressure),
        }
    }
}

pub const INPUT_FILTER_CAP: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeInputFilter {
    #[serde(default)]
    pub stabilization: usize,
    #[serde(default)]
    pub pressure_filter: usize,
}

impl StrokeInputFilter {
    pub fn sanitized(&self) -> Self {
        Self {
            stabilization: self.stabilization.min(INPUT_FILTER_CAP),
            pressure_filter: self.pressure_filter.min(INPUT_FILTER_CAP),
        }
    }
}

impl Default for StrokeInputFilter {
    fn default() -> Self {
        Self {
            stabilization: 0,
            pressure_filter: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PressureResponse {
    #[serde(default = "default_pressure_feel")]
    pub feel: f32,
}

fn default_pressure_feel() -> f32 {
    100.0
}

impl PressureResponse {
    pub fn new(feel: f32) -> Self {
        Self { feel }
    }

    pub fn apply(&self, pressure: f32) -> f32 {
        let x = pressure.clamp(0.0, 1.0);
        let feel = self.feel.clamp(0.0, 200.0);
        if feel >= 100.0 {
            (x * feel / 100.0).clamp(0.0, 1.0)
        } else {
            x.powf((150.0 - feel) * 2.0 / 100.0).clamp(0.0, 1.0)
        }
    }
}

impl Default for PressureResponse {
    fn default() -> Self {
        Self {
            feel: default_pressure_feel(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeToolPreset {
    pub name: String,
    pub stroke_strategy: StrokeStrategy,
    pub input_filter: StrokeInputFilter,
    pub pressure_response: PressureResponse,
    pub stroke_op: StrokePresetOp,
    pub transient_overrides: HashMap<String, Vec<(String, ParamValue)>>,
    pub quick_params: Vec<QuickParamTarget>,
}

impl StrokeToolPreset {
    fn new(
        name: &str,
        stroke_strategy: StrokeStrategy,
        input_filter: StrokeInputFilter,
        pressure_response: PressureResponse,
        stroke_op: StrokePresetOp,
    ) -> Self {
        Self {
            name: name.to_owned(),
            stroke_strategy,
            input_filter,
            pressure_response,
            stroke_op,
            transient_overrides: HashMap::new(),
            quick_params: Vec::new(),
        }
    }

    pub fn brush_engine(
        name: impl Into<String>,
        stroke_strategy: StrokeStrategy,
        input_filter: StrokeInputFilter,
        pressure_response: PressureResponse,
        stroke_op: StrokePresetOp,
    ) -> Self {
        Self {
            name: name.into(),
            stroke_strategy,
            input_filter,
            pressure_response,
            stroke_op,
            transient_overrides: HashMap::new(),
            quick_params: Vec::new(),
        }
    }

    pub fn corrected_pressure(&self, pressure: f32) -> f32 {
        self.pressure_response.apply(pressure)
    }

    pub(crate) fn references_resource(&self, resource_id: &str) -> bool {
        let uses_resource = |value: &ParamValue| {
            matches!(
                value,
                ParamValue::ResourceRef(ResourceRefValue::Resource(id)) if id == resource_id
            )
        };
        let StrokePresetOp::BrushEngine { params, .. } = &self.stroke_op;
        params.iter().any(|(_, value)| uses_resource(value))
            || self
                .transient_overrides
                .values()
                .flatten()
                .any(|(_, value)| uses_resource(value))
    }

    pub fn quick_param_visible(&self, target: &QuickParamTarget) -> bool {
        self.quick_params.contains(target)
    }

    pub fn set_quick_param_visible(&mut self, target: QuickParamTarget, visible: bool) -> bool {
        if visible {
            if self.quick_params.contains(&target) {
                return false;
            }
            self.quick_params.push(target);
            true
        } else {
            let previous_len = self.quick_params.len();
            self.quick_params.retain(|candidate| candidate != &target);
            self.quick_params.len() != previous_len
        }
    }

    pub fn reorder_quick_params(&mut self, order: &[QuickParamTarget]) -> bool {
        let previous = self.quick_params.clone();
        self.quick_params.sort_by_key(|target| {
            order
                .iter()
                .position(|candidate| candidate == target)
                .unwrap_or(usize::MAX)
        });
        self.quick_params != previous
    }
}

impl Default for StrokeToolPreset {
    fn default() -> Self {
        Self::new(
            "Brush",
            StrokeStrategy::SpacingDab { spacing: 0.18 },
            StrokeInputFilter::default(),
            PressureResponse::default(),
            StrokePresetOp::BrushEngine {
                engine_id: "paint".to_owned(),
                size_scene_ratio: RangedF32 {
                    value: 0.01,
                    range: None,
                },
                size_pressure: PressureDynamics::enabled(),
                params: Vec::new(),
                param_dynamics: Vec::new(),
                surface_source_material_scope: SurfaceSourceMaterialScope::BrushFootprint {
                    extra_radius: Vec::new(),
                },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{brush_engine::SurfaceSourceMaterialScope, curve::CurvePoint};

    use super::{
        CommonBrushParam, DEFAULT_DIAMETER_SCENE_RATIO_RANGE, PressureDynamics, QuickParamTarget,
        RangedF32, StrokeOp, StrokePresetOp, StrokeToolPreset,
    };

    fn scene_relative_op(
        size_scene_ratio: RangedF32,
        size_pressure: PressureDynamics,
    ) -> StrokePresetOp {
        StrokePresetOp::BrushEngine {
            engine_id: "test".to_owned(),
            size_scene_ratio,
            size_pressure,
            params: Vec::new(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: SurfaceSourceMaterialScope::AllMaterials,
        }
    }

    #[test]
    fn pressure_dynamics_evaluates_only_its_curve_when_enabled() {
        let disabled = PressureDynamics::default();
        assert_eq!(disabled.evaluate(0.0), 1.0);
        assert_eq!(disabled.evaluate(0.5), 1.0);

        let mut pressure = PressureDynamics::enabled();
        assert_eq!(pressure.evaluate(0.0), 0.0);
        assert_eq!(pressure.evaluate(0.5), 0.5);
        assert_eq!(pressure.evaluate(1.0), 1.0);

        assert!(pressure.curve.set_point(0, CurvePoint { x: 0.0, y: 0.2 }));
        assert!(pressure.curve.set_point(1, CurvePoint { x: 1.0, y: 0.8 }));
        assert_eq!(pressure.evaluate(0.0), 0.2);
        assert_eq!(pressure.evaluate(1.0), 0.8);

        pressure.curve = Default::default();
        assert!(
            pressure
                .curve
                .insert_point(CurvePoint { x: 0.5, y: 0.25 })
                .is_some()
        );
        assert_eq!(pressure.evaluate(0.5), 0.25);
    }

    #[test]
    fn pressure_dynamics_round_trips_as_control_points() {
        let pressure = PressureDynamics::enabled();
        let encoded = ron::to_string(&pressure).unwrap();
        assert!(encoded.contains("(x:0.0,y:0.0)"));
        assert_eq!(
            ron::from_str::<PressureDynamics>(&encoded).unwrap(),
            pressure
        );
        assert!(ron::from_str::<PressureDynamics>("(enabled:true,curve:Linear)").is_err());
        assert!(
            ron::from_str::<PressureDynamics>(
                "(enabled:true,min:0.2,curve:[(x:0.0,y:0.0),(x:1.0,y:1.0)])"
            )
            .is_err()
        );
    }

    #[test]
    fn quick_param_visibility_toggle_preserves_order_and_avoids_duplicates() {
        let mut preset = StrokeToolPreset::default();
        let size = QuickParamTarget::Common(CommonBrushParam::Size);
        let size_pressure = QuickParamTarget::Common(CommonBrushParam::SizePressure);
        let opacity = QuickParamTarget::Engine("opacity".to_owned());

        assert!(preset.set_quick_param_visible(size.clone(), true));
        assert!(preset.set_quick_param_visible(size_pressure.clone(), true));
        assert!(preset.set_quick_param_visible(opacity.clone(), true));
        assert!(!preset.set_quick_param_visible(size.clone(), true));
        assert_eq!(
            preset.quick_params,
            vec![size.clone(), size_pressure.clone(), opacity.clone()]
        );
        assert!(preset.quick_param_visible(&size_pressure));

        assert!(preset.set_quick_param_visible(size_pressure.clone(), false));
        assert_eq!(preset.quick_params, vec![size, opacity]);
        assert!(!preset.quick_param_visible(&size_pressure));
        assert!(!preset.set_quick_param_visible(size_pressure, false));
    }

    #[test]
    fn quick_params_can_be_reordered_to_match_settings_order() {
        let mut preset = StrokeToolPreset::default();
        let size = QuickParamTarget::Common(CommonBrushParam::Size);
        let size_pressure = QuickParamTarget::Common(CommonBrushParam::SizePressure);
        let pressure_feel = QuickParamTarget::Common(CommonBrushParam::PressureFeel);
        let opacity = QuickParamTarget::Engine("opacity".to_owned());
        let flow = QuickParamTarget::Engine("flow".to_owned());
        let flow_pressure = QuickParamTarget::Engine("flow_pressure".to_owned());
        let unknown = QuickParamTarget::Engine("inactive".to_owned());
        let order = vec![
            size.clone(),
            pressure_feel.clone(),
            size_pressure.clone(),
            opacity.clone(),
            flow.clone(),
            flow_pressure.clone(),
        ];

        preset.quick_params = vec![
            flow_pressure.clone(),
            unknown.clone(),
            opacity.clone(),
            size.clone(),
            flow.clone(),
        ];
        assert!(preset.reorder_quick_params(&order));
        assert_eq!(
            preset.quick_params,
            vec![
                size.clone(),
                opacity.clone(),
                flow.clone(),
                flow_pressure.clone(),
                unknown.clone(),
            ]
        );
        assert!(!preset.reorder_quick_params(&order));

        assert!(preset.set_quick_param_visible(size_pressure.clone(), true));
        assert!(preset.reorder_quick_params(&order));
        assert_eq!(
            preset.quick_params,
            vec![size, size_pressure, opacity, flow, flow_pressure, unknown]
        );
    }

    #[test]
    fn scene_relative_size_resolves_to_world_radius() {
        let preset = scene_relative_op(
            RangedF32 {
                value: 0.05,
                range: Some((0.001, 0.2)),
            },
            PressureDynamics::default(),
        );

        assert!((preset.radius_world(2.0) - 0.05).abs() < 1.0e-6);
        assert!((preset.radius_world(200.0) - 5.0).abs() < 1.0e-6);
    }

    #[test]
    fn resolve_scales_value_and_keeps_pressure_separate() {
        let preset = scene_relative_op(
            RangedF32 {
                value: 0.1,
                range: None,
            },
            PressureDynamics::enabled(),
        );
        let StrokeOp::BrushEngine {
            radius_world,
            radius_pressure,
            ..
        } = preset.resolve(4.0);

        assert!((radius_world - 0.2).abs() < 1.0e-6);
        assert!((radius_pressure.evaluate(0.0) - 0.0).abs() < 1.0e-6);
        assert!((radius_pressure.evaluate(1.0) - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn size_setter_does_not_change_pressure() {
        let pressure = PressureDynamics::enabled();
        let mut preset = scene_relative_op(
            RangedF32 {
                value: 0.05,
                range: Some((0.01, 0.1)),
            },
            pressure.clone(),
        );

        assert!(preset.set_size_scene_ratio(1.0));
        assert!((preset.size_scene_ratio() - 0.1).abs() < 1.0e-6);
        let StrokePresetOp::BrushEngine { size_pressure, .. } = &preset;
        assert_eq!(size_pressure, &pressure);
    }

    #[test]
    fn size_setter_clamps_to_declared_range() {
        let mut preset = scene_relative_op(
            RangedF32 {
                value: 0.05,
                range: Some((0.01, 0.1)),
            },
            PressureDynamics::default(),
        );

        assert!(preset.set_size_scene_ratio(1.0));
        assert!((preset.size_scene_ratio() - 0.1).abs() < 1.0e-6);
        assert!(preset.set_size_scene_ratio(0.0));
        assert!((preset.size_scene_ratio() - 0.01).abs() < 1.0e-6);
        assert!(!preset.set_size_scene_ratio(f32::NAN));
    }

    #[test]
    fn size_setter_uses_default_range_when_unspecified() {
        let mut preset = scene_relative_op(
            RangedF32 {
                value: 0.01,
                range: None,
            },
            PressureDynamics::default(),
        );

        assert!(preset.set_size_scene_ratio(1.0));
        assert_eq!(
            preset.size_scene_ratio(),
            DEFAULT_DIAMETER_SCENE_RATIO_RANGE.1
        );
    }
}
