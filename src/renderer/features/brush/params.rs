use bytemuck::Zeroable;

use crate::{
    core::{
        brush_engine::{ParamType, ParamValue, apply_f32_param_dynamics},
        stroke::StrokeDab,
        stroke_preset::StrokeOp,
    },
    renderer::{
        command::{RendererStrokeOperation, RendererStrokeStyle},
        features::brush::engine_pipelines::{BrushParamLayout, BrushParamSlot},
    },
};

use super::engine_types::BrushEngineUniform;

pub(super) fn resolve_brush_engine_uniform(
    style: &RendererStrokeStyle,
    layout: &BrushParamLayout,
) -> BrushEngineUniform {
    let mut uniform = BrushEngineUniform::zeroed();
    let StrokeOp::BrushEngine { radius_world, .. } = &style.stroke_op;
    let RendererStrokeOperation::BrushEngine { color, params } = &style.operation;
    uniform.color_rgb_pad = rgb_with_padding(*color);
    uniform.base_radius[0] = radius_world.max(0.0);
    for field in layout.fields() {
        let value = params
            .iter()
            .find_map(|(candidate, value)| (candidate == &field.name).then_some(value))
            .unwrap_or(&field.default);
        match field.slot {
            BrushParamSlot::F32(index) => {
                if let Some(value) =
                    param_value_as_f32(value).or_else(|| param_value_as_f32(&field.default))
                {
                    write_f32_param_index(&mut uniform, index, value);
                }
            }
            BrushParamSlot::U32(index) => {
                if let Some(value) = param_value_as_u32(value, &field.ty)
                    .or_else(|| param_value_as_u32(&field.default, &field.ty))
                {
                    write_u32_param_index(&mut uniform, index, value);
                }
            }
            BrushParamSlot::PerDabF32(_) => {}
        }
    }
    uniform
}

fn rgb_with_padding(color: [f32; 3]) -> [f32; 4] {
    [color[0], color[1], color[2], 0.0]
}

pub(super) fn resolve_per_dab_f32_slots(
    style: &RendererStrokeStyle,
    layout: &BrushParamLayout,
    pressure: f32,
) -> [[f32; 4]; 4] {
    let RendererStrokeOperation::BrushEngine { params, .. } = &style.operation;
    let mut slots = [[0.0; 4]; 4];
    for field in layout.dynamic_fields() {
        let BrushParamSlot::PerDabF32(index) = field.slot else {
            continue;
        };
        if index >= 16 {
            continue;
        }
        let base = params
            .iter()
            .find_map(|(candidate, value)| (candidate == &field.name).then_some(value))
            .and_then(param_value_as_f32)
            .or_else(|| param_value_as_f32(&field.default))
            .unwrap_or(0.0);
        let resolved = apply_f32_param_dynamics(base, params, &field.dynamics, pressure);
        slots[index / 4][index % 4] = resolved;
    }
    slots
}

pub(super) fn resolve_per_dab_f32_slots_for_dab(
    style: &RendererStrokeStyle,
    layout: &BrushParamLayout,
    dab: &StrokeDab,
) -> [[f32; 4]; 4] {
    resolve_per_dab_f32_slots(style, layout, dab.pressure)
}

fn param_value_as_f32(value: &ParamValue) -> Option<f32> {
    match value {
        ParamValue::F32(value) => Some(*value),
        _ => None,
    }
}

fn param_value_as_u32(value: &ParamValue, ty: &ParamType) -> Option<u32> {
    match (ty, value) {
        (ParamType::Bool, ParamValue::Bool(value)) => Some(u32::from(*value)),
        (ParamType::U32, ParamValue::U32(value)) => Some(*value),
        (ParamType::Enum { .. }, ParamValue::Enum(value)) => Some(*value),
        _ => None,
    }
}

fn write_f32_param_index(uniform: &mut BrushEngineUniform, index: usize, value: f32) {
    match index {
        0..=3 => uniform.params0[index] = value,
        4..=7 => uniform.params1[index - 4] = value,
        8..=11 => uniform.params2[index - 8] = value,
        12..=15 => uniform.params3[index - 12] = value,
        _ => {}
    }
}

fn write_u32_param_index(uniform: &mut BrushEngineUniform, index: usize, value: u32) {
    match index {
        0..=3 => uniform.params_u32_0[index] = value,
        4..=7 => uniform.params_u32_1[index - 4] = value,
        8..=11 => uniform.params_u32_2[index - 8] = value,
        12..=15 => uniform.params_u32_3[index - 12] = value,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::rgb_with_padding;

    #[test]
    fn rgb_uniform_slot_uses_zero_padding_instead_of_alpha() {
        assert_eq!(rgb_with_padding([0.2, 0.4, 0.6]), [0.2, 0.4, 0.6, 0.0]);
    }
}
