use glam::Vec2;

use crate::{
    application::{AppState, PointerSample},
    core::{
        stroke::{StrokeDab, StrokeSpace},
        stroke_preset::StrokeToolPreset,
        stroke_sampling::{
            min_uv_spacing_for_texture_size, normalized_stroke_strategy,
            sample_segment_with_radius_and_min_spacing_into,
        },
    },
};

#[cfg(test)]
use crate::core::stroke_preset::StrokeStrategy;

pub(crate) fn stroke_dab_from_pressure(
    position: Vec2,
    pressure: f32,
    tool: &StrokeToolPreset,
) -> StrokeDab {
    let pressure = tool.corrected_pressure(pressure);
    let radius_scale = tool.stroke_op.radius_scale(pressure);
    StrokeDab::with_scales(position, pressure, radius_scale)
}

pub(in crate::application) fn predicted_uv_flush_dabs(
    state: &AppState,
    sample: &PointerSample,
) -> Vec<StrokeDab> {
    let mut out = Vec::new();
    predicted_uv_flush_dabs_into(state, sample, &mut out);
    out
}

pub(in crate::application) fn predicted_uv_flush_dabs_into(
    state: &AppState,
    _sample: &PointerSample,
    out: &mut Vec<StrokeDab>,
) {
    let Some(session) = state.tool.stroke_session(StrokeSpace::Uv) else {
        return;
    };
    if !session.has_drawn_dabs {
        if let Some(dab) = session.last_dab {
            out.push(dab);
        }
        return;
    }
    // After a drag, PointerUp finalizes the stroke only. It must not predict or
    // draw extra dabs toward the lift-off position.
}

pub(in crate::application) fn sampled_dabs_for_uv_into(
    state: &AppState,
    dab: StrokeDab,
    out: &mut Vec<StrokeDab>,
) {
    let tool = state
        .effective_tool_preset()
        .expect("stroke sampling requires an effective stroke preset");
    let strategy = normalized_stroke_strategy(&tool.stroke_strategy);
    let radius_uv = state
        .document()
        .map(|document| {
            tool.stroke_op.radius_world(document.mesh.scene_diagonal())
                * document.mesh.uv_linear_scale
        })
        .unwrap_or(0.0)
        .max(1e-6);
    let min_spacing_uv = state
        .tool
        .stroke_session(StrokeSpace::Uv)
        .and_then(|session| {
            state
                .document()
                .and_then(|document| document.texture_size_for_surface(session.target()))
        })
        .map(min_uv_spacing_for_texture_size)
        .unwrap_or(0.0);
    let last = state
        .tool
        .stroke_session(StrokeSpace::Uv)
        .and_then(|session| session.last_dab);
    sample_segment_with_radius_and_min_spacing_into(
        last,
        dab,
        &strategy,
        radius_uv,
        min_spacing_uv,
        out,
    );
}

#[cfg(test)]
pub(in crate::application) fn sample_segment(
    from: Option<StrokeDab>,
    to: StrokeDab,
    strategy: &StrokeStrategy,
) -> Vec<StrokeDab> {
    let mut out = Vec::new();
    if matches!(strategy, StrokeStrategy::RawEvent) {
        out.push(to);
        return out;
    }

    let spacing = match strategy {
        StrokeStrategy::SpacingDab { spacing } | StrokeStrategy::ContinuousDab { spacing, .. } => {
            *spacing
        }
        StrokeStrategy::RawEvent => unreachable!("handled above"),
    };

    let Some(from) = from else {
        out.push(to);
        return out;
    };

    let delta = to.position - from.position;
    let distance = delta.length();
    if distance <= f32::EPSILON {
        out.push(to);
        return out;
    }
    if distance < spacing {
        return out;
    }

    let mut t = spacing.max(0.0001);
    while t <= distance {
        let ratio = t / distance;
        out.push(StrokeDab::with_scales(
            from.position + delta * ratio,
            from.pressure + (to.pressure - from.pressure) * ratio,
            from.radius_scale + (to.radius_scale - from.radius_scale) * ratio,
        ));
        t += spacing;
    }
    out
}
