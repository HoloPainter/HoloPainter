use super::{stroke::StrokeDab, stroke_preset::StrokeStrategy};

const MIN_UV_DAB_SPACING_PX: f32 = 0.18;
const MAX_SPACING_ALPHA_SCALE: f32 = 256.0;
const MIN_STROKE_SPACING: f32 = 1e-5;
const MIN_CONTINUOUS_RATE_HZ: f32 = 1e-3;
const DEFAULT_CONTINUOUS_RATE_HZ: f32 = 60.0;

pub(crate) fn min_uv_spacing_for_texture_size(texture_size: [u32; 2]) -> f32 {
    let tex_scale = ((texture_size[0].max(1) as f32) * (texture_size[1].max(1) as f32)).sqrt();
    MIN_UV_DAB_SPACING_PX / tex_scale.max(1.0)
}

pub(crate) fn normalized_stroke_strategy(strategy: &StrokeStrategy) -> StrokeStrategy {
    match strategy {
        StrokeStrategy::RawEvent => StrokeStrategy::RawEvent,
        StrokeStrategy::SpacingDab { spacing } => StrokeStrategy::SpacingDab {
            spacing: normalized_spacing(*spacing),
        },
        StrokeStrategy::ContinuousDab { spacing, rate_hz } => StrokeStrategy::ContinuousDab {
            spacing: normalized_spacing(*spacing),
            rate_hz: normalized_continuous_rate_hz(*rate_hz),
        },
    }
}

fn normalized_spacing(spacing: f32) -> f32 {
    if spacing.is_finite() {
        spacing.max(MIN_STROKE_SPACING)
    } else {
        MIN_STROKE_SPACING
    }
}

fn normalized_continuous_rate_hz(rate_hz: f32) -> f32 {
    if rate_hz.is_finite() {
        rate_hz.max(MIN_CONTINUOUS_RATE_HZ)
    } else {
        DEFAULT_CONTINUOUS_RATE_HZ
    }
}

pub(crate) fn sample_segment_with_radius_and_min_spacing_into(
    from: Option<StrokeDab>,
    to: StrokeDab,
    strategy: &StrokeStrategy,
    radius: f32,
    min_spacing: f32,
    out: &mut Vec<StrokeDab>,
) {
    if matches!(strategy, StrokeStrategy::RawEvent) {
        out.push(to);
        return;
    }

    let spacing_ratio = match strategy {
        StrokeStrategy::SpacingDab { spacing } | StrokeStrategy::ContinuousDab { spacing, .. } => {
            *spacing
        }
        StrokeStrategy::RawEvent => unreachable!("handled above"),
    };

    let Some(mut last) = from else {
        out.push(to);
        return;
    };
    let initial_len = out.len();

    loop {
        let delta = to.position - last.position;
        let distance = delta.length();
        if distance <= f32::EPSILON {
            if out.len() == initial_len && dab_style_changed(last, to) {
                out.push(to);
            }
            return;
        }

        let effective_radius = radius * ((last.radius_scale + to.radius_scale) * 0.5);
        let ideal_spacing = (effective_radius * spacing_ratio).max(1e-6);
        let spacing = ideal_spacing.max(min_spacing.max(0.0));
        if distance < spacing {
            return;
        }

        let spacing_alpha_scale = (spacing / ideal_spacing).clamp(1.0, MAX_SPACING_ALPHA_SCALE);
        let ratio = spacing / distance;
        let dab = StrokeDab::with_scales(
            last.position + delta * ratio,
            last.pressure + (to.pressure - last.pressure) * ratio,
            last.radius_scale + (to.radius_scale - last.radius_scale) * ratio,
        )
        .with_spacing_alpha_scale(spacing_alpha_scale);
        out.push(dab);
        last = dab;
    }
}

fn dab_style_changed(from: StrokeDab, to: StrokeDab) -> bool {
    (from.pressure - to.pressure).abs() > f32::EPSILON
        || (from.radius_scale - to.radius_scale).abs() > f32::EPSILON
        || (from.spacing_alpha_scale - to.spacing_alpha_scale).abs() > f32::EPSILON
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::{
        min_uv_spacing_for_texture_size, normalized_stroke_strategy,
        sample_segment_with_radius_and_min_spacing_into,
    };
    use crate::core::{stroke::StrokeDab, stroke_preset::StrokeStrategy};

    #[test]
    fn normalized_strategy_clamps_non_positive_spacing() {
        assert_eq!(
            normalized_stroke_strategy(&StrokeStrategy::SpacingDab { spacing: 0.0 }),
            StrokeStrategy::SpacingDab { spacing: 1e-5 }
        );
    }

    #[test]
    fn normalized_continuous_strategy_clamps_invalid_values() {
        assert_eq!(
            normalized_stroke_strategy(&StrokeStrategy::ContinuousDab {
                spacing: f32::INFINITY,
                rate_hz: f32::NAN,
            }),
            StrokeStrategy::ContinuousDab {
                spacing: 1e-5,
                rate_hz: 60.0,
            }
        );
    }

    #[test]
    fn continuous_strategy_uses_spacing_sampling_for_movement() {
        let from = StrokeDab::with_scales(Vec2::ZERO, 0.1, 1.0);
        let to = StrokeDab::with_scales(Vec2::new(1.0, 0.0), 0.1, 1.0);
        let mut spacing = Vec::new();
        let mut continuous = Vec::new();

        sample_segment_with_radius_and_min_spacing_into(
            Some(from),
            to,
            &StrokeStrategy::SpacingDab { spacing: 0.25 },
            1.0,
            0.0,
            &mut spacing,
        );
        sample_segment_with_radius_and_min_spacing_into(
            Some(from),
            to,
            &StrokeStrategy::ContinuousDab {
                spacing: 0.25,
                rate_hz: 60.0,
            },
            1.0,
            0.0,
            &mut continuous,
        );

        assert_eq!(continuous, spacing);
    }

    #[test]
    fn min_spacing_records_alpha_compensation() {
        let mut out = Vec::new();
        let from = StrokeDab::with_scales(Vec2::ZERO, 0.1, 0.1);
        let to = StrokeDab::with_scales(Vec2::new(1.0, 0.0), 0.1, 0.1);

        sample_segment_with_radius_and_min_spacing_into(
            Some(from),
            to,
            &StrokeStrategy::SpacingDab { spacing: 0.18 },
            0.001,
            0.01,
            &mut out,
        );

        assert!(!out.is_empty());
        assert!((out[0].position.x - 0.01).abs() < 1e-6);
        assert!(out[0].spacing_alpha_scale > 1.0);
    }

    #[test]
    fn zero_min_spacing_keeps_default_alpha_scale() {
        let mut out = Vec::new();
        let from = StrokeDab::with_scales(Vec2::ZERO, 0.1, 1.0);
        let to = StrokeDab::with_scales(Vec2::new(1.0, 0.0), 0.1, 1.0);

        sample_segment_with_radius_and_min_spacing_into(
            Some(from),
            to,
            &StrokeStrategy::SpacingDab { spacing: 0.25 },
            1.0,
            0.0,
            &mut out,
        );

        assert!(!out.is_empty());
        assert!((out[0].spacing_alpha_scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn preexisting_output_does_not_suppress_stationary_style_change() {
        let mut out = vec![StrokeDab::new(Vec2::new(-1.0, 0.0), 1.0)];
        let from = StrokeDab::with_scales(Vec2::ZERO, 0.5, 0.75).with_spacing_alpha_scale(2.0);
        let to = StrokeDab::with_scales(Vec2::ZERO, 0.5, 0.75);

        sample_segment_with_radius_and_min_spacing_into(
            Some(from),
            to,
            &StrokeStrategy::SpacingDab { spacing: 0.18 },
            1.0,
            0.0,
            &mut out,
        );

        assert_eq!(out.len(), 2);
        assert_eq!(out[1], to);
    }

    #[test]
    fn preview_sized_texture_spacing_is_subpixel_in_uv_space() {
        let spacing = min_uv_spacing_for_texture_size([256, 72]);

        assert!(spacing > 0.0);
        assert!(spacing < 1.0 / 72.0);
    }
}
