use bytemuck::{Pod, Zeroable};

use crate::core::adjustment::{
    ADJUSTMENT_LUT_SIZE, Adjustment, LevelsChannel, UvMirrorAxis, UvMirrorDirection,
};
use crate::core::adjustment_evaluator::build_adjustment_lut;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AdjustmentGpuParams {
    pub(crate) kind: u32,
    pub(crate) params: [[f32; 4]; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct AdjustmentLutUniform {
    pub(crate) samples: [[f32; 4]; ADJUSTMENT_LUT_SIZE],
}

pub(crate) fn adjustment_gpu_params(adjustment: &Adjustment) -> AdjustmentGpuParams {
    let mut params = [[0.0; 4]; 8];
    let kind = match adjustment {
        Adjustment::BrightnessContrast(_) => 0,
        Adjustment::Levels(value) => {
            write_levels_uniform(&mut params, 0, value.master);
            write_levels_uniform(&mut params, 2, value.red);
            write_levels_uniform(&mut params, 4, value.green);
            write_levels_uniform(&mut params, 6, value.blue);
            1
        }
        Adjustment::HueSaturation(value) => {
            params[0] = [
                value.hue as f32 / 360.0,
                value.saturation as f32 / 100.0,
                value.lightness as f32 / 100.0,
                0.0,
            ];
            2
        }
        Adjustment::Curves(_) => 3,
        Adjustment::Invert => 4,
        Adjustment::GradientMap(value) => {
            params[0][0] = if value.dither { 1.0 } else { 0.0 };
            5
        }
        Adjustment::UvMirror(value) => {
            params[0] = [
                match value.axis {
                    UvMirrorAxis::X => 0.0,
                    UvMirrorAxis::Y => 1.0,
                },
                value.position,
                match value.direction {
                    UvMirrorDirection::PositiveToNegative => 0.0,
                    UvMirrorDirection::NegativeToPositive => 1.0,
                },
                0.0,
            ];
            6
        }
    };
    AdjustmentGpuParams { kind, params }
}

fn write_levels_uniform(params: &mut [[f32; 4]; 8], index: usize, channel: LevelsChannel) {
    params[index] = [
        channel.input_black as f32 / 255.0,
        channel.input_white as f32 / 255.0,
        channel.gamma,
        channel.output_black as f32 / 255.0,
    ];
    params[index + 1][0] = channel.output_white as f32 / 255.0;
}

impl AdjustmentLutUniform {
    pub(crate) fn for_adjustment(adjustment: &Adjustment) -> Option<Self> {
        let lut = build_adjustment_lut(adjustment)?;
        Some(Self {
            samples: std::array::from_fn(|index| {
                let color = lut[index];
                [color[0], color[1], color[2], 1.0]
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adjustment::{
        BrightnessContrastAdjustment, CurveChannel, CurvePoint, CurvesAdjustment,
        GradientMapAdjustment, HueSaturationAdjustment, LevelsAdjustment,
    };

    #[test]
    fn adjustment_lut_has_expected_uniform_size() {
        assert_eq!(std::mem::size_of::<AdjustmentLutUniform>(), 4096);
    }

    #[test]
    fn gpu_params_preserve_composite_kind_ids_and_values() {
        let brightness = adjustment_gpu_params(&Adjustment::BrightnessContrast(
            BrightnessContrastAdjustment {
                brightness: 25,
                contrast: -40,
            },
        ));
        assert_eq!(brightness.kind, 0);
        assert_eq!(brightness.params[0], [0.0; 4]);

        let levels = adjustment_gpu_params(&Adjustment::Levels(LevelsAdjustment::default()));
        assert_eq!(levels.kind, 1);
        assert_eq!(levels.params[0], [0.0, 1.0, 1.0, 0.0]);
        assert_eq!(levels.params[1][0], 1.0);

        let hsv = adjustment_gpu_params(&Adjustment::HueSaturation(HueSaturationAdjustment {
            hue: 90,
            saturation: 50,
            lightness: -25,
        }));
        assert_eq!(hsv.kind, 2);
        assert_eq!(hsv.params[0], [0.25, 0.5, -0.25, 0.0]);
        assert_eq!(
            adjustment_gpu_params(&Adjustment::Curves(Default::default())).kind,
            3
        );
        assert_eq!(adjustment_gpu_params(&Adjustment::Invert).kind, 4);
        assert_eq!(
            adjustment_gpu_params(&Adjustment::GradientMap(Default::default())).kind,
            5
        );
        assert_eq!(
            adjustment_gpu_params(&Adjustment::UvMirror(Default::default())).kind,
            6
        );
    }

    #[test]
    fn default_curves_generate_identity_lut() {
        let lut =
            AdjustmentLutUniform::for_adjustment(&Adjustment::Curves(CurvesAdjustment::default()))
                .unwrap();
        for index in [0, 63, 127, 191, 255] {
            let expected = index as f32 / 255.0;
            assert!((lut.samples[index][0] - expected).abs() < 1.0e-6);
            assert!((lut.samples[index][1] - expected).abs() < 1.0e-6);
            assert!((lut.samples[index][2] - expected).abs() < 1.0e-6);
            assert_eq!(lut.samples[index][3], 1.0);
        }
    }

    #[test]
    fn curves_lut_applies_component_channels_before_master() {
        let mut curves = CurvesAdjustment::default();
        curves.master.insert_point(CurvePoint {
            input: 128,
            output: 191,
        });
        let mut red = CurveChannel::default();
        red.insert_point(CurvePoint {
            input: 191,
            output: 64,
        });
        curves.red = red;
        let lut = AdjustmentLutUniform::for_adjustment(&Adjustment::Curves(curves)).unwrap();
        let after_red = curves.red.evaluate(128.0 / 255.0);
        let expected_red = curves.master.evaluate(after_red);
        let expected_green = curves.master.evaluate(128.0 / 255.0);
        let reverse_order = curves.red.evaluate(expected_green);
        assert!((lut.samples[128][0] - expected_red).abs() < 1.0e-6);
        assert!((lut.samples[128][1] - expected_green).abs() < 1.0e-6);
        assert!((expected_red - reverse_order).abs() > 0.01);
    }

    #[test]
    fn default_gradient_generates_black_to_white_lut() {
        let lut = AdjustmentLutUniform::for_adjustment(&Adjustment::GradientMap(
            GradientMapAdjustment::default(),
        ))
        .unwrap();
        assert_eq!(lut.samples[0], [0.0, 0.0, 0.0, 1.0]);
        assert!((lut.samples[128][0] - 128.0 / 255.0).abs() < 1.0e-6);
        assert_eq!(lut.samples[255], [1.0, 1.0, 1.0, 1.0]);
    }
}
