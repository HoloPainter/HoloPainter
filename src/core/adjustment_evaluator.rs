use super::adjustment::{
    ADJUSTMENT_LUT_SIZE, Adjustment, BrightnessContrastAdjustment, LevelsAdjustment, LevelsChannel,
};

pub type AdjustmentLut = [[f32; 3]; ADJUSTMENT_LUT_SIZE];

pub fn build_adjustment_lut(adjustment: &Adjustment) -> Option<AdjustmentLut> {
    match adjustment {
        Adjustment::BrightnessContrast(value) => Some(brightness_contrast_lut(*value)),
        Adjustment::Levels(value) => Some(levels_lut(*value)),
        Adjustment::Curves(value) => Some(std::array::from_fn(|index| {
            value.evaluate_rgb([index as f32 / 255.0; 3])
        })),
        Adjustment::GradientMap(value) => Some(std::array::from_fn(|index| {
            value.evaluate(index as f32 / 255.0)
        })),
        Adjustment::HueSaturation(_) | Adjustment::Invert | Adjustment::UvMirror(_) => None,
    }
}

fn brightness_contrast_lut(value: BrightnessContrastAdjustment) -> AdjustmentLut {
    let brightness = value.brightness as f32 / 150.0;
    let contrast = value.contrast as f32 / 100.0;
    let contrast_x = [0.0, 63.0 / 255.0, 191.0 / 255.0, 1.0];
    let contrast_y = [
        0.0,
        contrast_x[1] - contrast * (25.0 / 255.0),
        contrast_x[2] + contrast * (25.0 / 255.0),
        1.0,
    ];
    let domain: [f32; ADJUSTMENT_LUT_SIZE] = std::array::from_fn(|index| index as f32 / 255.0);
    let contrast_curve = domain.map(|input| natural_cubic_spline(&contrast_x, &contrast_y, input));
    let brightness_offset = domain.map(|input| {
        let absolute = brightness.abs();
        let polynomial = |coefficient: f32, power: f32| coefficient * input.powf(power);
        let shape = 0.5
            * (absolute * (polynomial(1.65, 0.35) + polynomial(-1.0, 10.0))
                + (1.0 - absolute) * (polynomial(1.96, 0.4) + polynomial(1.0, 4.0))
                + polynomial(1.0, 1.25));
        brightness * input * (1.0 - input) * shape
    });
    let rotated_x: [f32; ADJUSTMENT_LUT_SIZE] =
        std::array::from_fn(|index| domain[index] - brightness_offset[index]);
    let rotated_y: [f32; ADJUSTMENT_LUT_SIZE] =
        std::array::from_fn(|index| contrast_curve[index] + brightness_offset[index]);
    std::array::from_fn(|index| {
        let output = interpolate_sorted(&rotated_x, &rotated_y, domain[index]).clamp(0.0, 1.0);
        [output; 3]
    })
}

fn interpolate_sorted(x: &[f32], y: &[f32], input: f32) -> f32 {
    if input <= x[0] {
        return y[0];
    }
    if input >= x[x.len() - 1] {
        return y[y.len() - 1];
    }
    let right = x.partition_point(|value| *value < input);
    let left = right - 1;
    let amount = (input - x[left]) / (x[right] - x[left]);
    y[left] + (y[right] - y[left]) * amount
}

pub(crate) fn natural_cubic_spline(x: &[f32], y: &[f32], input: f32) -> f32 {
    debug_assert_eq!(x.len(), y.len());
    debug_assert!(x.len() >= 2 && x.len() <= super::adjustment::MAX_CURVE_POINTS);
    let count = x.len();
    let mut h = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut alpha = [0.0; super::adjustment::MAX_CURVE_POINTS];
    for index in 0..count - 1 {
        h[index] = x[index + 1] - x[index];
    }
    for index in 1..count - 1 {
        alpha[index] = 3.0 * (y[index + 1] - y[index]) / h[index]
            - 3.0 * (y[index] - y[index - 1]) / h[index - 1];
    }
    let mut l = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut mu = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut z = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut c = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut b = [0.0; super::adjustment::MAX_CURVE_POINTS];
    let mut d = [0.0; super::adjustment::MAX_CURVE_POINTS];
    l[0] = 1.0;
    for index in 1..count - 1 {
        l[index] = 2.0 * (x[index + 1] - x[index - 1]) - h[index - 1] * mu[index - 1];
        mu[index] = h[index] / l[index];
        z[index] = (alpha[index] - h[index - 1] * z[index - 1]) / l[index];
    }
    for index in (0..count - 1).rev() {
        c[index] = z[index] - mu[index] * c[index + 1];
        b[index] =
            (y[index + 1] - y[index]) / h[index] - h[index] * (c[index + 1] + 2.0 * c[index]) / 3.0;
        d[index] = (c[index + 1] - c[index]) / (3.0 * h[index]);
    }
    let clamped = input.clamp(x[0], x[count - 1]);
    let segment = x
        .partition_point(|value| *value < clamped)
        .clamp(1, count - 1)
        - 1;
    let delta = clamped - x[segment];
    y[segment] + b[segment] * delta + c[segment] * delta.powi(2) + d[segment] * delta.powi(3)
}

fn levels_lut(value: LevelsAdjustment) -> AdjustmentLut {
    std::array::from_fn(|index| {
        let input = index as f32 / 255.0;
        let channels = [
            apply_levels_channel(input, value.red),
            apply_levels_channel(input, value.green),
            apply_levels_channel(input, value.blue),
        ];
        channels.map(|channel| apply_levels_channel(channel, value.master))
    })
}

fn apply_levels_channel(value: f32, levels: LevelsChannel) -> f32 {
    let input_black = levels.input_black as f32 / 255.0;
    let input_white = levels.input_white as f32 / 255.0;
    let output_black = levels.output_black as f32 / 255.0;
    let output_white = levels.output_white as f32 / 255.0;
    let normalized = ((value - input_black) / (input_white - input_black)).clamp(0.0, 1.0);
    output_black + (output_white - output_black) * normalized.powf(1.0 / levels.gamma)
}

pub fn apply_hue_saturation(
    mut color: [f32; 3],
    adjustment: super::adjustment::HueSaturationAdjustment,
) -> [f32; 3] {
    let lightness = adjustment.lightness as f32 / 100.0;
    color = color.map(|channel| {
        if lightness < 0.0 {
            channel * (1.0 + lightness)
        } else {
            channel * (1.0 - lightness) + lightness
        }
    });
    let [mut hue, mut saturation, hsl_lightness] = rgb_to_hsl(color);
    hue = (hue + adjustment.hue as f32 / 360.0).rem_euclid(1.0);
    let saturation_delta = adjustment.saturation as f32 / 100.0;
    saturation = if saturation_delta <= 0.0 {
        saturation * (1.0 + saturation_delta)
    } else {
        saturation / (1.0 - saturation_delta + 0.01)
    };
    hsl_to_rgb([hue, saturation.clamp(0.0, 1.0), hsl_lightness])
}

pub fn gradient_map_coordinate(color: [f32; 3]) -> f32 {
    // Photoshop's exact Gradient Map grayscale conversion is undocumented. This
    // integer form follows the Photoshop/PDF luminosity convention (0.30/0.59/0.11).
    ((color[0] * 77.0 + color[1] * 151.0 + color[2] * 28.0) / 256.0).clamp(0.0, 1.0)
}

fn rgb_to_hsl([red, green, blue]: [f32; 3]) -> [f32; 3] {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let delta = maximum - minimum;
    let lightness = (maximum + minimum) * 0.5;
    if delta <= f32::EPSILON {
        return [0.0, 0.0, lightness];
    }
    let hue = if maximum == red {
        ((green - blue) / delta).rem_euclid(6.0)
    } else if maximum == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    } / 6.0;
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    [hue, saturation, lightness]
}

fn hsl_to_rgb([hue, saturation, lightness]: [f32; 3]) -> [f32; 3] {
    let hue6 = hue.rem_euclid(1.0) * 6.0;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - (hue6.rem_euclid(2.0) - 1.0).abs());
    let base = match hue6.floor() as u32 % 6 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    let offset = lightness - chroma * 0.5;
    base.map(|channel| (channel + offset).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adjustment::HueSaturationAdjustment;

    #[test]
    fn identity_luts_preserve_all_byte_inputs() {
        for adjustment in [
            Adjustment::BrightnessContrast(Default::default()),
            Adjustment::Levels(Default::default()),
            Adjustment::Curves(Default::default()),
        ] {
            let lut = build_adjustment_lut(&adjustment).unwrap();
            for (index, output) in lut.iter().enumerate() {
                let expected = index as f32 / 255.0;
                assert!(
                    output
                        .iter()
                        .all(|channel| (channel - expected).abs() < 1.0e-5)
                );
            }
        }
    }

    #[test]
    fn hue_rotation_uses_lightness_not_hsv_value() {
        let adjusted = apply_hue_saturation(
            [1.0, 0.0, 0.0],
            HueSaturationAdjustment {
                hue: 120,
                saturation: 0,
                lightness: 0,
            },
        );
        assert!(adjusted[1] > 0.999 && adjusted[0] < 0.001 && adjusted[2] < 0.001);
    }

    #[test]
    fn non_legacy_brightness_uses_photoshop_style_tone_curve() {
        let lut = build_adjustment_lut(&Adjustment::BrightnessContrast(
            BrightnessContrastAdjustment {
                brightness: 100,
                contrast: 0,
            },
        ))
        .unwrap();
        for (input, expected) in [(64, 123), (128, 210), (192, 245)] {
            assert!((lut[input][0] - expected as f32 / 255.0).abs() < 0.003);
        }
    }

    #[test]
    fn master_lightness_is_applied_before_hsl_conversion() {
        let adjusted = apply_hue_saturation(
            [0.2, 0.4, 0.6],
            HueSaturationAdjustment {
                hue: 0,
                saturation: 0,
                lightness: 50,
            },
        );
        for (actual, expected) in adjusted.into_iter().zip([0.6, 0.7, 0.8]) {
            assert!((actual - expected).abs() < 1.0e-5);
        }
    }

    #[test]
    fn levels_apply_individual_channels_before_master() {
        let mut levels = LevelsAdjustment::default();
        levels.red.gamma = 2.0;
        levels.master.input_black = 64;
        let lut = build_adjustment_lut(&Adjustment::Levels(levels)).unwrap();
        let input = 128.0 / 255.0;
        let after_red = apply_levels_channel(input, levels.red);
        let expected = apply_levels_channel(after_red, levels.master);
        let reverse_order =
            apply_levels_channel(apply_levels_channel(input, levels.master), levels.red);
        assert!((lut[128][0] - expected).abs() < 1.0e-6);
        assert!((expected - reverse_order).abs() > 0.01);
    }
}
