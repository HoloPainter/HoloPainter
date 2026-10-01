use super::{brush_engine::ParamValue, stroke_preset::StrokeOp};

#[derive(Debug, Clone, PartialEq)]
pub enum ToolOperation {
    Paint(PaintOperation),
    BrushEngine(BrushEngineOperation),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaintOperation {
    pub source: PaintSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaintSource {
    SolidColor([f32; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushEngineOperation {
    pub engine_id: String,
    pub color: [f32; 3],
    pub params: Vec<(String, ParamValue)>,
}

impl PaintOperation {
    pub fn solid_color(color: [f32; 3]) -> Self {
        Self {
            source: PaintSource::SolidColor(color),
        }
    }
}

impl ToolOperation {
    pub fn from_stroke_op_and_color(stroke_op: &StrokeOp, current_color: [f32; 3]) -> Self {
        match stroke_op {
            StrokeOp::BrushEngine {
                engine_id, params, ..
            } => Self::BrushEngine(BrushEngineOperation {
                engine_id: engine_id.clone(),
                color: current_color,
                params: params.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{
        brush_engine::SurfaceSourceMaterialScope,
        stroke_preset::{PressureDynamics, StrokeOp},
    };

    use super::{PaintOperation, ToolOperation};

    #[test]
    fn stroke_operation_keeps_rgb_without_alpha_conversion() {
        let stroke_op = StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 1.0,
            radius_pressure: PressureDynamics::default(),
            params: Vec::new(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: SurfaceSourceMaterialScope::AllMaterials,
        };

        let operation = ToolOperation::from_stroke_op_and_color(&stroke_op, [0.2, 0.4, 0.6]);
        let ToolOperation::BrushEngine(operation) = operation else {
            panic!("expected brush engine operation");
        };
        assert_eq!(operation.color, [0.2, 0.4, 0.6]);
    }

    #[test]
    fn solid_color_keeps_rgb_without_alpha_conversion() {
        let operation = PaintOperation::solid_color([0.2, 0.4, 0.6]);
        assert_eq!(
            operation.source,
            super::PaintSource::SolidColor([0.2, 0.4, 0.6])
        );
    }
}
