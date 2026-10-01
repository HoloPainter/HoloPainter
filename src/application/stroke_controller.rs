use std::sync::Arc;

use anyhow::Result;
use glam::Vec2;

use crate::core::{
    damage::DamageMap,
    document::{RaycastScratch, SurfaceHit},
    selection::ActiveSelection,
    stroke::{
        PaintSurfaceSet, StrokeContext, StrokeDab, StrokeRenderDescriptor, StrokeSpace, SurfaceDab,
        SurfaceProjectionContext, SurfaceProjectionId,
    },
    stroke_preset::StrokeToolPreset,
    stroke_sampling::normalized_stroke_strategy,
    stroke_style::ResolvedStrokeStyle,
    tool_operation::ToolOperation,
};

use super::{
    RendererCommitSpec,
    command::{PointerSample, PointerSampleKind, ToolInputEvent, ViewportInputContext},
    damage::{
        estimate_surface_stroke_damage_from_dabs, estimate_uv_stroke_damage_from_dabs,
        estimate_uv_stroke_pointer_up_damage,
    },
    history_capture::{PixelHistoryCapture, pixel_history_capture_for_full_surfaces},
    render_plan::StrokeRenderPlanSink,
    state::{
        AppState, ContinuousStrokeAnchor, ContinuousStrokeRuntime, PaintTargetScope,
        SurfaceMirrorXSessionData, ViewportStrokeSessionData, begin_stroke_surface,
        begin_stroke_uv,
    },
    stroke::{
        input_filter::{FilteredStrokeInput, RawStrokeInput, StrokeInputFilterRuntime},
        sampling::{sampled_dabs_for_uv_into, stroke_dab_from_pressure},
        surface_mirror::{mirror_x_dab, mirror_x_view},
        surface_path::{
            sample_surface_segment_into, surface_dab_from_hit,
            visible_material_indices_for_surface_dabs,
            visible_surface_material_indices_for_surface_dabs,
        },
    },
};
use crate::renderer::command::SurfaceProjectionDabBatch;

const MAX_CONTINUOUS_DABS_PER_TICK: usize = 16;

fn effective_stroke_preset(state: &AppState) -> &StrokeToolPreset {
    state
        .effective_tool_preset()
        .expect("stroke controller requires an effective stroke preset")
}

fn effective_stroke_radius_world(state: &AppState) -> f32 {
    state
        .effective_brush_radius_world()
        .expect("stroke controller requires an effective brush radius")
}

#[cfg(test)]
use super::stroke::{sampling::sample_segment, surface_path::sample_surface_segment};

#[derive(Debug, Default)]
pub(crate) struct StrokeSampleScratch {
    stroke_dabs: Vec<StrokeDab>,
    surface_dabs: Vec<SurfaceDab>,
    raycast_scratch: RaycastScratch,
}

impl StrokeSampleScratch {
    pub(crate) fn clear(&mut self) {
        self.stroke_dabs.clear();
        self.surface_dabs.clear();
    }

    pub(crate) fn raycast_scratch_mut(&mut self) -> &mut RaycastScratch {
        &mut self.raycast_scratch
    }
}

#[derive(Debug, Default)]
pub(crate) struct StrokeToolInputOutput {
    pub history_capture: Option<PixelHistoryCapture>,
    pub renderer_commit: RendererCommitSpec,
}

impl StrokeToolInputOutput {
    fn with_history_capture_and_renderer_commit(
        history_capture: Option<PixelHistoryCapture>,
        renderer_commit: RendererCommitSpec,
    ) -> Self {
        Self {
            history_capture,
            renderer_commit,
        }
    }
}

pub(crate) fn handle_tool_input_into_with_scratch<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    event: ToolInputEvent,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    match event {
        ToolInputEvent::PointerDown(sample) => {
            handle_pointer_down_into(state, sample, effects, scratch)
        }
        ToolInputEvent::PointerMove(sample) => {
            handle_pointer_move_into(state, sample, effects, scratch)
        }
        ToolInputEvent::PointerUp(sample) => {
            handle_pointer_up_into(state, sample, effects, scratch)
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled by tool controller"),
    }
}

fn handle_pointer_down_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    match sample.kind.clone() {
        PointerSampleKind::Uv { uv, .. } => {
            let first_dab =
                stroke_dab_from_pressure(uv, sample.pressure(), effective_stroke_preset(state));
            begin_uv_stroke_into(state, first_dab, &sample, effects, scratch);
            Ok(StrokeToolInputOutput::default())
        }
        PointerSampleKind::Surface { view, hit } => begin_surface_stroke_into(
            state,
            sample.screen_px(),
            sample.pressure(),
            sample.time_s(),
            view,
            hit,
            effects,
            scratch,
        )
        .map(|_| StrokeToolInputOutput::default()),
    }
}

fn handle_pointer_move_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    match sample.kind.clone() {
        PointerSampleKind::Uv { .. } => {
            append_uv_sampled_sample_into(state, &sample, effects, scratch);
            Ok(StrokeToolInputOutput::default())
        }
        PointerSampleKind::Surface { view, hit } => {
            append_surface_sampled_into(state, &sample, view, hit, effects, scratch)
                .map(|_| StrokeToolInputOutput::default())
        }
    }
}

fn handle_pointer_up_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    advance_active_stroke_into(state, sample.time_s(), effects, scratch);
    match sample.kind.clone() {
        PointerSampleKind::Uv { .. } => handle_uv_pointer_up_into(state, &sample, effects, scratch),
        PointerSampleKind::Surface { view, hit } => {
            handle_surface_pointer_up_into(state, &sample, view, hit, effects, scratch)
        }
    }
}

fn handle_uv_pointer_up_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    flush_uv_stroke_into(state, sample, effects, scratch);

    let uv_damage_after_flush = state
        .tool
        .stroke_session(StrokeSpace::Uv)
        .map(|session| session.stroke_damage.clone())
        .filter(|damage| !damage.is_empty())
        .or_else(|| {
            state
                .tool
                .stroke_session(StrokeSpace::Uv)
                .map(|_| estimate_uv_stroke_pointer_up_damage(state, sample))
        });

    let mut finalized_surfaces = Vec::new();
    if let Some(surfaces) = state.tool.finish_stroke_session(StrokeSpace::Uv) {
        finalized_surfaces.extend_from_slice(surfaces.as_slice());
        let damage = uv_damage_after_flush.clone().unwrap_or_default();
        effects.push_stroke_end(surfaces, (!damage.is_empty()).then_some(damage));
    }

    let history_capture = uv_damage_after_flush.and_then(PixelHistoryCapture::from_damage);
    Ok(StrokeToolInputOutput {
        history_capture,
        renderer_commit: RendererCommitSpec::with_finalized_surfaces(finalized_surfaces),
    })
}

fn handle_surface_pointer_up_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<StrokeToolInputOutput> {
    flush_surface_stroke_into(state, sample, view, hit, effects, scratch);

    let surface_damage_after_flush = state
        .tool
        .stroke_session(StrokeSpace::Surface)
        .map(|session| session.stroke_damage.clone())
        .filter(|damage| !damage.is_empty());

    let mut finalized_surfaces = Vec::new();
    let mut history_capture = None;
    if let Some((surfaces, pending_history_capture)) = state
        .tool
        .finish_stroke_session_with_history(StrokeSpace::Surface)
    {
        let damage = surface_damage_after_flush.clone();
        history_capture = pending_history_capture
            .or_else(|| PixelHistoryCapture::from_optional_damage(damage.clone()))
            .or_else(|| pixel_history_capture_for_full_surfaces(state, surfaces.as_slice()));
        finalized_surfaces.extend_from_slice(surfaces.as_slice());
        effects.push_surface_stroke_end(surfaces, damage);
    }

    Ok(
        StrokeToolInputOutput::with_history_capture_and_renderer_commit(
            history_capture,
            RendererCommitSpec::with_finalized_surfaces(finalized_surfaces),
        ),
    )
}

fn begin_surface_stroke_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    position_px: Vec2,
    pressure: f32,
    time_s: f64,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<Option<PixelHistoryCapture>> {
    if !state.tool.is_idle() {
        return Ok(None);
    }
    let Some(first_dab) =
        surface_dab_from_hit(position_px, pressure, effective_stroke_preset(state), hit)
    else {
        return Ok(None);
    };
    let mut raycast_scratch = RaycastScratch::default();
    let mut initial_materials = visible_material_indices_for_surface_dabs(
        state.document(),
        state.viewport_scene_visibility(),
        view,
        std::slice::from_ref(&first_dab),
        effective_stroke_radius_world(state),
        &mut raycast_scratch,
    );
    if let Some(hit) = hit
        && state
            .viewport_scene_visibility()
            .geometry_visible(hit.mesh_id, hit.material_index.into())
        && !initial_materials.contains(&hit.material_index.as_usize())
    {
        initial_materials.push(hit.material_index.as_usize());
    }
    let Some(paint_target) = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Materials(
            initial_materials.into_iter().map(Into::into).collect(),
        ))
        .into_allowed()
    else {
        return Ok(None);
    };
    let target = paint_target.target();
    let surfaces = paint_target.surfaces_vec();
    let style = resolved_style_from_state(state);
    let active_selection = active_selection_snapshot(state);
    let mirror_options = state.surface_mirror_options();
    let mirror_x = if mirror_options.x_enabled {
        mirror_x_view(view, mirror_options.x_plane).map(|mirror_view| SurfaceMirrorXSessionData {
            plane_x: mirror_options.x_plane,
            view: mirror_view,
            continuous: false,
            last_triangle: None,
        })
    } else {
        None
    };
    let mut projections = vec![SurfaceProjectionContext {
        id: SurfaceProjectionId::Primary,
        view_proj_matrix_gl: view.view_proj,
        camera_world: view.camera_world,
    }];
    if let Some(mirror) = mirror_x {
        projections.push(SurfaceProjectionContext {
            id: SurfaceProjectionId::MirrorX,
            view_proj_matrix_gl: mirror.view.view_proj,
            camera_world: mirror.view.camera_world,
        });
    }
    let context = Arc::new(StrokeContext::surface_projections(
        state.camera().clone(),
        view.size,
        projections,
        state.viewport_scene_visibility().clone(),
    ));
    let descriptor = Arc::new(StrokeRenderDescriptor::new(
        target,
        PaintSurfaceSet::from_vec(surfaces),
        style,
        active_selection,
        context,
    ));
    let input_filter = new_input_filter_runtime(state, position_px, pressure, time_s);
    let continuous = continuous_runtime(
        &effective_stroke_preset(state).stroke_strategy,
        ContinuousStrokeAnchor::Surface(first_dab),
        time_s,
    );
    let emit_initial_dab = continuous.is_some();
    state.tool.set_session(begin_stroke_surface(
        first_dab,
        descriptor.clone(),
        input_filter,
        ViewportStrokeSessionData {
            locked_viewport_view_proj: view.view_proj,
            locked_viewport_inv_view_proj: view.inv_view_proj,
            locked_camera_world: view.camera_world,
            locked_viewport_size: view.size,
            mirror_x,
        },
        continuous,
    ));
    effects.push_surface_stroke_begin(descriptor.clone());
    if emit_initial_dab {
        scratch.surface_dabs.clear();
        scratch.surface_dabs.push(first_dab);
        append_surface_dabs_into(
            state,
            &mut scratch.surface_dabs,
            effects,
            &mut scratch.raycast_scratch,
        );
    }
    Ok(None)
}

fn begin_uv_stroke_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    first_dab: StrokeDab,
    sample: &PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) {
    if !state.tool.is_idle() {
        return;
    }
    let Some(paint_target) = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
        .into_allowed()
    else {
        return;
    };
    let target = paint_target.target();
    let descriptor = Arc::new(StrokeRenderDescriptor::single_uv(
        target,
        resolved_style_from_state(state),
        active_selection_snapshot(state),
    ));
    let input_filter = new_input_filter_runtime(
        state,
        sample.screen_px(),
        sample.pressure(),
        sample.time_s(),
    );
    let continuous = continuous_runtime(
        &effective_stroke_preset(state).stroke_strategy,
        ContinuousStrokeAnchor::Uv(first_dab),
        sample.time_s(),
    );
    let emit_initial_dab = continuous.is_some();
    state.tool.set_session(begin_stroke_uv(
        first_dab,
        descriptor.clone(),
        input_filter,
        continuous,
    ));
    effects.push_stroke_begin(descriptor.clone());
    if emit_initial_dab {
        scratch.stroke_dabs.clear();
        scratch.stroke_dabs.push(first_dab);
        append_uv_dabs_into(state, &mut scratch.stroke_dabs, effects);
    }
}

fn append_uv_sampled_sample_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) {
    if state.tool.is_idle() {
        if let PointerSampleKind::Uv { uv, .. } = sample.kind {
            let first_dab =
                stroke_dab_from_pressure(uv, sample.pressure(), effective_stroke_preset(state));
            begin_uv_stroke_into(state, first_dab, sample, effects, scratch);
        }
        return;
    }
    let Some(stabilized) = update_input_filter_for_sample(state, sample) else {
        return;
    };
    let Some(dab) = uv_dab_from_stabilized(state, sample, stabilized) else {
        if let Some(session) = state.tool.stroke_session_mut(StrokeSpace::Uv) {
            session.last_dab = None;
            if let Some(continuous) = session.continuous.as_mut() {
                continuous.update_anchor(None, sample.time_s(), false);
            }
        }
        return;
    };
    scratch.stroke_dabs.clear();
    sampled_dabs_for_uv_into(state, dab, &mut scratch.stroke_dabs);
    let emitted_movement_dabs = !scratch.stroke_dabs.is_empty();
    append_uv_dabs_into(state, &mut scratch.stroke_dabs, effects);
    update_continuous_anchor(
        state,
        StrokeSpace::Uv,
        Some(ContinuousStrokeAnchor::Uv(dab)),
        sample.time_s(),
        emitted_movement_dabs,
    );
}

fn uv_dab_from_stabilized(
    state: &AppState,
    sample: &PointerSample,
    stabilized: FilteredStrokeInput,
) -> Option<StrokeDab> {
    let view = sample.uv_view_context()?;
    let uv = view
        .transform
        .view_px_to_uv(stabilized.screen_px, view.size, view.canvas_size);
    if !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y) {
        return None;
    }
    Some(stroke_dab_from_pressure(
        uv,
        stabilized.pressure,
        effective_stroke_preset(state),
    ))
}

fn append_uv_dabs_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    dabs: &mut Vec<StrokeDab>,
    effects: &mut S,
) {
    if dabs.is_empty() {
        return;
    }
    if state.tool.stroke_session(StrokeSpace::Uv).is_none() {
        return;
    }
    let preview_damage = uv_stroke_damage_for_dabs(state, dabs);
    if let Some(damage) = preview_damage.as_ref() {
        accumulate_uv_stroke_damage(state, damage);
    }
    if let Some(session) = state.tool.stroke_session_mut(StrokeSpace::Uv) {
        session.last_dab = dabs.last().copied();
        session.has_drawn_dabs = true;
    }
    effects.push_stroke_dabs(dabs, preview_damage);
}

fn uv_stroke_damage_for_dabs(state: &AppState, dabs: &[StrokeDab]) -> Option<DamageMap> {
    let document = state.document()?;
    let session = state.tool.stroke_session(StrokeSpace::Uv)?;
    estimate_uv_stroke_damage_from_dabs(
        document,
        session.target(),
        session.style().as_ref(),
        dabs.iter().copied(),
    )
}

fn accumulate_uv_stroke_damage(state: &mut AppState, damage: &DamageMap) {
    accumulate_stroke_damage(state, StrokeSpace::Uv, damage);
}

fn surface_stroke_damage_for_dabs(
    state: &AppState,
    target_surfaces: &PaintSurfaceSet,
    target_materials_by_dab: &[Vec<usize>],
    dabs: &[SurfaceDab],
) -> Option<DamageMap> {
    let document = state.document()?;
    let session = state.tool.stroke_session(StrokeSpace::Surface)?;
    estimate_surface_stroke_damage_from_dabs(
        document,
        target_surfaces,
        target_materials_by_dab,
        session.style().as_ref(),
        dabs,
    )
}

fn accumulate_surface_stroke_damage(state: &mut AppState, damage: &DamageMap) {
    accumulate_stroke_damage(state, StrokeSpace::Surface, damage);
}

fn accumulate_stroke_damage(state: &mut AppState, space: StrokeSpace, damage: &DamageMap) {
    if let Some(session) = state.tool.stroke_session_mut(space) {
        for pixel in &damage.pixels {
            session.stroke_damage.add_rect(pixel.surface, pixel.rect);
        }
    }
}

fn append_surface_sampled_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) -> Result<Option<PixelHistoryCapture>> {
    if state.tool.is_idle() {
        return begin_surface_stroke_into(
            state,
            sample.screen_px(),
            sample.pressure(),
            sample.time_s(),
            view,
            hit,
            effects,
            scratch,
        );
    }
    let Some(stabilized) = update_input_filter_for_sample(state, sample) else {
        return Ok(None);
    };
    let screen_dab = stroke_dab_from_pressure(
        stabilized.screen_px,
        stabilized.pressure,
        effective_stroke_preset(state),
    );
    scratch.surface_dabs.clear();
    sampled_dabs_for_surface_into(
        state,
        sample.screen_px(),
        view,
        hit,
        screen_dab,
        &mut scratch.surface_dabs,
        &mut scratch.raycast_scratch,
    );
    let emitted_movement_dabs = !scratch.surface_dabs.is_empty();
    append_surface_dabs_into(
        state,
        &mut scratch.surface_dabs,
        effects,
        &mut scratch.raycast_scratch,
    );
    let visible_hit = hit.filter(|hit| {
        state
            .viewport_scene_visibility()
            .geometry_visible(hit.mesh_id, hit.material_index.into())
    });
    let anchor = surface_dab_from_hit(
        sample.screen_px(),
        stabilized.pressure,
        effective_stroke_preset(state),
        visible_hit,
    )
    .map(ContinuousStrokeAnchor::Surface);
    update_continuous_anchor(
        state,
        StrokeSpace::Surface,
        anchor,
        sample.time_s(),
        emitted_movement_dabs,
    );
    Ok(None)
}

fn sampled_dabs_for_surface_into(
    state: &AppState,
    position_px: Vec2,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    to: StrokeDab,
    out: &mut Vec<SurfaceDab>,
    raycast_scratch: &mut RaycastScratch,
) {
    let tool = effective_stroke_preset(state);
    let strategy = normalized_stroke_strategy(&tool.stroke_strategy);
    let radius_world = effective_stroke_radius_world(state).max(1e-6);
    let last = state
        .tool
        .stroke_session(StrokeSpace::Surface)
        .and_then(|session| session.last_surface_dab);
    sample_surface_segment_into(
        state.document(),
        state.viewport_scene_visibility(),
        position_px,
        view,
        hit,
        last,
        to,
        &strategy,
        radius_world,
        out,
        raycast_scratch,
    );
}

fn append_surface_dabs_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    dabs: &mut Vec<SurfaceDab>,
    effects: &mut S,
    raycast_scratch: &mut RaycastScratch,
) {
    if dabs.is_empty() {
        return;
    }
    let Some(session) = state.tool.stroke_session(StrokeSpace::Surface) else {
        return;
    };
    let Some(viewport) = session.viewport.clone() else {
        return;
    };
    let view = ViewportInputContext {
        size: viewport.locked_viewport_size,
        view_proj: viewport.locked_viewport_view_proj,
        inv_view_proj: viewport.locked_viewport_inv_view_proj,
        camera_world: viewport.locked_camera_world,
    };
    let primary_dabs = std::mem::take(dabs);
    let primary_last = primary_dabs.last().copied();
    let (
        projection_batches,
        target_materials,
        source_materials,
        mirror_continuous,
        mirror_last_triangle,
    ) = {
        let radius_world = effective_stroke_radius_world(state);
        let (source_scope, params, param_dynamics) = match &effective_stroke_preset(state).stroke_op
        {
            crate::core::stroke_preset::StrokePresetOp::BrushEngine {
                params,
                param_dynamics,
                surface_source_material_scope,
                ..
            } => (
                surface_source_material_scope,
                params.as_slice(),
                param_dynamics.as_slice(),
            ),
        };
        let document = state.document();
        let mut mirror_batches = Vec::new();
        let mut mirror_continuous = false;
        let mut mirror_last_triangle = None;
        if let (Some(document), Some(mirror)) = (document, viewport.mirror_x) {
            let mirror_view = mirror.view;
            let mut segment = Vec::with_capacity(primary_dabs.len());
            let mut continues = mirror.continuous;
            mirror_last_triangle = mirror.last_triangle;
            for primary in &primary_dabs {
                if let Some(mirrored) = mirror_x_dab(
                    document,
                    *primary,
                    mirror_view,
                    mirror.plane_x,
                    radius_world,
                    mirror_last_triangle,
                    state.viewport_scene_visibility(),
                    raycast_scratch,
                ) {
                    mirror_last_triangle = mirrored.triangle_index;
                    segment.push(mirrored);
                    mirror_continuous = true;
                } else {
                    if !segment.is_empty() {
                        mirror_batches.push((
                            SurfaceProjectionDabBatch {
                                projection_id: SurfaceProjectionId::MirrorX,
                                continues_from_previous: continues,
                                dabs: std::mem::take(&mut segment),
                                target_materials_by_dab: Vec::new(),
                            },
                            mirror_view,
                        ));
                    }
                    continues = false;
                    mirror_continuous = false;
                    mirror_last_triangle = None;
                }
            }
            if !segment.is_empty() {
                mirror_batches.push((
                    SurfaceProjectionDabBatch {
                        projection_id: SurfaceProjectionId::MirrorX,
                        continues_from_previous: continues,
                        dabs: segment,
                        target_materials_by_dab: Vec::new(),
                    },
                    mirror_view,
                ));
            }
        }

        let mut raw_batches = Vec::with_capacity(1 + mirror_batches.len());
        raw_batches.push((
            SurfaceProjectionDabBatch {
                projection_id: SurfaceProjectionId::Primary,
                continues_from_previous: true,
                dabs: primary_dabs,
                target_materials_by_dab: Vec::new(),
            },
            view,
        ));
        raw_batches.extend(mirror_batches);

        let mut target_materials = Vec::new();
        let mut source_materials = Some(Vec::new());
        let mut projection_batches = Vec::with_capacity(raw_batches.len());
        for (mut batch, projection_view) in raw_batches {
            let visible = visible_surface_material_indices_for_surface_dabs(
                document,
                state.viewport_scene_visibility(),
                projection_view,
                &batch.dabs,
                radius_world,
                source_scope,
                params,
                param_dynamics,
                raycast_scratch,
            );
            for material in visible.target {
                if !target_materials.contains(&material) {
                    target_materials.push(material);
                }
            }
            match (&mut source_materials, visible.source) {
                (Some(all), Some(source)) => {
                    for material in source {
                        if !all.contains(&material) {
                            all.push(material);
                        }
                    }
                }
                (_, None) => source_materials = None,
                (None, Some(_)) => {}
            }
            batch.target_materials_by_dab = visible.target_by_dab;
            projection_batches.push(batch);
        }
        (
            projection_batches,
            target_materials,
            source_materials,
            mirror_continuous,
            mirror_last_triangle,
        )
    };
    let Some(paint_target) = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Materials(
            target_materials.into_iter().map(Into::into).collect(),
        ))
        .into_allowed()
    else {
        return;
    };
    let target_surfaces = PaintSurfaceSet::from_vec(paint_target.surfaces_vec());
    if target_surfaces.is_empty() {
        return;
    }

    let source_target_scope = match source_materials {
        None => PaintTargetScope::AllMaterials,
        Some(source_material_indices) => PaintTargetScope::Materials(
            source_material_indices
                .into_iter()
                .map(Into::into)
                .collect(),
        ),
    };
    let Some(source_target) = state
        .document
        .resolve_paint_edit_target(source_target_scope)
        .into_allowed()
    else {
        return;
    };
    let source_surfaces = PaintSurfaceSet::from_vec(source_target.surfaces_vec());
    if source_surfaces.is_empty() {
        return;
    }
    let mut preview_damage = Some(DamageMap::default());
    for batch in &projection_batches {
        let damage = surface_stroke_damage_for_dabs(
            state,
            &target_surfaces,
            &batch.target_materials_by_dab,
            &batch.dabs,
        );
        match (&mut preview_damage, damage) {
            (Some(total), Some(damage)) => {
                for pixel in damage.pixels {
                    total.add_rect(pixel.surface, pixel.rect);
                }
            }
            (_, None) => preview_damage = None,
            (None, Some(_)) => {}
        }
    }
    if let Some(damage) = preview_damage.as_ref() {
        accumulate_surface_stroke_damage(state, damage);
    }
    state
        .tool
        .extend_stroke_session_surfaces(StrokeSpace::Surface, &target_surfaces);
    if let Some(session) = state.tool.stroke_session_mut(StrokeSpace::Surface) {
        session.last_surface_dab = primary_last;
        session.has_drawn_dabs = true;
        if let Some(mirror) = session
            .viewport
            .as_mut()
            .and_then(|viewport| viewport.mirror_x.as_mut())
        {
            mirror.continuous = mirror_continuous;
            mirror.last_triangle = mirror_last_triangle;
        }
        effects.push_surface_dabs(
            source_surfaces,
            target_surfaces,
            projection_batches,
            preview_damage,
        );
    }
}

fn continuous_runtime(
    strategy: &crate::core::stroke_preset::StrokeStrategy,
    anchor: ContinuousStrokeAnchor,
    time_s: f64,
) -> Option<ContinuousStrokeRuntime> {
    match normalized_stroke_strategy(strategy) {
        crate::core::stroke_preset::StrokeStrategy::ContinuousDab { rate_hz, .. } => {
            Some(ContinuousStrokeRuntime::new(anchor, time_s, rate_hz))
        }
        crate::core::stroke_preset::StrokeStrategy::SpacingDab { .. }
        | crate::core::stroke_preset::StrokeStrategy::RawEvent => None,
    }
}

fn update_continuous_anchor(
    state: &mut AppState,
    space: StrokeSpace,
    anchor: Option<ContinuousStrokeAnchor>,
    time_s: f64,
    reset_deadline: bool,
) {
    let Some(session) = state.tool.stroke_session_mut(space) else {
        return;
    };
    let Some(continuous) = session.continuous.as_mut() else {
        return;
    };
    continuous.update_anchor(anchor, time_s, reset_deadline);
}

pub(crate) fn advance_active_stroke_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    time_s: f64,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) {
    let Some(session) = state.tool.active_stroke_session_mut() else {
        return;
    };
    let Some(continuous) = session.continuous.as_mut() else {
        return;
    };
    let emit_count = continuous.take_due_count(time_s, MAX_CONTINUOUS_DABS_PER_TICK);
    let Some(anchor) = continuous.anchor else {
        return;
    };
    if emit_count == 0 {
        return;
    }

    match anchor {
        ContinuousStrokeAnchor::Uv(dab) => {
            scratch.stroke_dabs.clear();
            scratch.stroke_dabs.resize(emit_count, dab);
            append_uv_dabs_into(state, &mut scratch.stroke_dabs, effects);
        }
        ContinuousStrokeAnchor::Surface(dab) => {
            scratch.surface_dabs.clear();
            scratch.surface_dabs.resize(emit_count, dab);
            append_surface_dabs_into(
                state,
                &mut scratch.surface_dabs,
                effects,
                &mut scratch.raycast_scratch,
            );
        }
    }
}

fn active_selection_snapshot(state: &AppState) -> Arc<ActiveSelection> {
    Arc::new(
        state
            .document()
            .map(|document| document.active_selection.clone())
            .unwrap_or_else(|| {
                ActiveSelection::disabled_for_materials(std::iter::empty::<
                    crate::core::material::MaterialIndex,
                >())
            }),
    )
}

fn resolved_style_from_state(state: &AppState) -> Arc<ResolvedStrokeStyle> {
    let preset = effective_stroke_preset(state);
    let mut preset_op = preset.stroke_op.clone();
    for id in state.transient_tool_override_ids() {
        let Some(patch) = preset.transient_overrides.get(id) else {
            continue;
        };
        apply_transient_param_patch(&mut preset_op, patch);
    }
    let scene_diagonal = state
        .document()
        .map(|document| document.mesh.scene_diagonal())
        .unwrap_or(0.0);
    let stroke_op = preset_op.resolve(scene_diagonal);
    let operation = ToolOperation::from_stroke_op_and_color(&stroke_op, state.current_color());
    Arc::new(ResolvedStrokeStyle {
        stroke_op,
        operation,
    })
}

fn apply_transient_param_patch(
    stroke_op: &mut crate::core::stroke_preset::StrokePresetOp,
    patch: &[(String, crate::core::brush_engine::ParamValue)],
) {
    let crate::core::stroke_preset::StrokePresetOp::BrushEngine { params, .. } = stroke_op;
    for (name, value) in patch {
        let Some((_, current)) = params.iter_mut().find(|(current, _)| current == name) else {
            continue;
        };
        *current = value.clone();
    }
}

fn flush_uv_stroke_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) {
    let Some(session) = state.tool.stroke_session(StrokeSpace::Uv) else {
        return;
    };
    if !session.has_drawn_dabs {
        if let Some(dab) = session.last_dab {
            scratch.stroke_dabs.clear();
            scratch.stroke_dabs.push(dab);
            append_uv_dabs_into(state, &mut scratch.stroke_dabs, effects);
        }
        return;
    }
    // After a drag has produced dabs, PointerUp is only a finalize trigger.
    // Do not draw toward the PointerUp position: lift-off events can be displaced
    // from the last real stroke sample and produce thread-like tails.
    let _ = (sample, effects, scratch);
}

fn flush_surface_stroke_into<S: StrokeRenderPlanSink>(
    state: &mut AppState,
    sample: &PointerSample,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    effects: &mut S,
    scratch: &mut StrokeSampleScratch,
) {
    let Some(session) = state.tool.stroke_session(StrokeSpace::Surface) else {
        return;
    };
    if !session.has_drawn_dabs {
        if let Some(dab) = session.last_surface_dab {
            scratch.surface_dabs.clear();
            scratch.surface_dabs.push(dab);
            append_surface_dabs_into(
                state,
                &mut scratch.surface_dabs,
                effects,
                &mut scratch.raycast_scratch,
            );
        }
        return;
    }
    // After a drag has produced dabs, PointerUp is only a finalize trigger.
    // Do not raycast or sample using the lift-off position.
    let _ = (sample, view, hit, effects, scratch);
}

fn new_input_filter_runtime(
    state: &AppState,
    screen_px: Vec2,
    pressure: f32,
    time_s: f64,
) -> StrokeInputFilterRuntime {
    let mut runtime = StrokeInputFilterRuntime::new();
    let raw = RawStrokeInput {
        screen_px,
        pressure,
        time_s,
    };
    let _ = runtime.update(&effective_stroke_preset(state).input_filter, raw);
    runtime
}

fn update_input_filter_for_sample(
    state: &mut AppState,
    sample: &PointerSample,
) -> Option<FilteredStrokeInput> {
    let input_filter = effective_stroke_preset(state).input_filter.clone();
    let raw = RawStrokeInput {
        screen_px: sample.screen_px(),
        pressure: sample.pressure(),
        time_s: sample.time_s(),
    };
    let session = state.tool.stroke_session_mut(sample.space())?;
    Some(session.input_filter.update(&input_filter, raw))
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3};

    use super::{
        MAX_CONTINUOUS_DABS_PER_TICK, StrokeSampleScratch, effective_stroke_preset,
        handle_tool_input_into_with_scratch, normalized_stroke_strategy, sample_segment,
        sample_surface_segment, stroke_dab_from_pressure,
    };

    use crate::{
        application::state::ToolSession,
        application::{
            AppState, Command, DamageMap, InputModifiers, PendingRendererHistoryTransaction,
            RendererPlanBuilder, ToolInputEvent, ViewportInputContext, command::PointerSample,
            reduce,
        },
        core::{
            brush_engine::{ParamValue, SurfaceSourceMaterialScope, SurfaceSourceRadiusTerm},
            document::{Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh, SurfaceHit},
            stroke::{PaintSurfaceSet, StrokeDab, StrokeSpace, SurfaceDab, SurfaceProjectionId},
            stroke_preset::{
                PressureDynamics, PressureResponse, RangedF32, StrokeInputFilter, StrokeOp,
                StrokePresetOp, StrokeStrategy, StrokeToolPreset,
            },
            tool_operation::ToolOperation,
        },
        renderer::{EditCommand, RendererFramePlan, StrokeCommand, StrokeDabPayload, StrokeTarget},
    };

    fn active_stroke_preset_mut(state: &mut AppState) -> &mut StrokeToolPreset {
        state
            .active_tool_preset_mut()
            .expect("test requires an active stroke preset")
    }

    fn state_with_materials(count: usize) -> AppState {
        state_with_material_sizes((0..count).map(|_| [64, 64]))
    }

    fn state_with_material_sizes(sizes: impl IntoIterator<Item = [u32; 2]>) -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            sizes
                .into_iter()
                .enumerate()
                .map(|(index, size)| MaterialSpec::new(format!("M{index}"), size))
                .collect(),
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        state
    }

    fn state_with_unit_plane_material_size(size: [u32; 2]) -> AppState {
        let positions = vec![
            Vec3::new(-0.5, -0.5, 0.0),
            Vec3::new(0.5, -0.5, 0.0),
            Vec3::new(-0.5, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
        ];
        let mesh = MeshData::new(
            positions,
            vec![Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE],
            vec![Vec3::Z; 4],
            vec![[0, 1, 2], [1, 3, 2]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 6,
                material_index: 0,
                material_name: "M0".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Plane".to_owned(),
            }],
            vec![MeshId(0); 2],
        )
        .unwrap();
        let mut state = AppState::default();
        state.document.document = Some(Document::new(mesh, vec![MaterialSpec::new("M0", size)]));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        state
    }

    fn state_with_two_material_view_plane() -> AppState {
        let positions = vec![
            Vec3::new(-0.9, -0.8, 0.0),
            Vec3::new(0.0, -0.8, 0.0),
            Vec3::new(-0.9, 0.8, 0.0),
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::new(0.0, -0.8, 0.0),
            Vec3::new(0.9, -0.8, 0.0),
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::new(0.9, 0.8, 0.0),
        ];
        let uvs = vec![Vec2::ZERO; positions.len()];
        let normals = vec![Vec3::Z; positions.len()];
        let indices = vec![[0, 1, 2], [1, 3, 2], [4, 5, 6], [5, 7, 6]];
        let mesh = MeshData::new(
            positions,
            uvs,
            normals,
            indices,
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 6,
                    material_index: 0,
                    material_name: "Left".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 6,
                    index_count: 6,
                    material_index: 1,
                    material_name: "Right".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: MeshId(0),
                name: "Plane".to_owned(),
            }],
            vec![MeshId(0); 4],
        )
        .unwrap();
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            mesh,
            vec![
                MaterialSpec::new("Left", [64, 64]),
                MaterialSpec::new("Right", [64, 64]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        state
    }

    fn screen_from_world_identity(world: Vec3, viewport_size: [u32; 2]) -> Vec2 {
        Vec2::new(
            (world.x * 0.5 + 0.5) * viewport_size[0] as f32,
            (1.0 - (world.y * 0.5 + 0.5)) * viewport_size[1] as f32,
        )
    }

    fn surface_test_view_context(
        viewport_size: [u32; 2],
        camera_world: [f32; 3],
    ) -> ViewportInputContext {
        ViewportInputContext {
            size: viewport_size,
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world,
        }
    }

    fn surface_sample_at(world_pos: Vec3, material_index: usize) -> PointerSample {
        surface_sample_at_uv(world_pos, material_index, Vec2::ZERO, f32::INFINITY)
    }

    fn surface_sample_at_uv(
        world_pos: Vec3,
        material_index: usize,
        uv: Vec2,
        uv_edge_distance: f32,
    ) -> PointerSample {
        let viewport_size = [100, 100];
        let view = surface_test_view_context(viewport_size, [0.0, 0.0, -1.0]);
        PointerSample::surface_with_pressure_at(
            screen_from_world_identity(world_pos, viewport_size),
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            SurfaceHit {
                world_pos,
                world_normal: Vec3::Z,
                uv,
                uv_edge_distance,
                uv_paint_boundary_distance: uv_edge_distance,
                triangle_index: if material_index == 0 { 0 } else { 2 },
                material_index: material_index.into(),
                mesh_id: MeshId(0),
                t: 1.0,
            },
        )
    }

    fn surface_material_indices(surfaces: &PaintSurfaceSet) -> Vec<usize> {
        let mut indices: Vec<_> = surfaces
            .iter()
            .map(|surface| surface.material_index().as_usize())
            .collect();
        indices.sort_unstable();
        indices
    }

    fn brush_engine_preset_op(
        engine_id: &str,
        size_scene_ratio: f32,
        size_pressure_start: f32,
        opacity: f32,
    ) -> StrokePresetOp {
        StrokePresetOp::BrushEngine {
            engine_id: engine_id.to_owned(),
            size_scene_ratio: RangedF32 {
                value: size_scene_ratio,
                range: None,
            },
            size_pressure: {
                let mut pressure = PressureDynamics::enabled();
                pressure.curve.set_point(
                    0,
                    crate::core::curve::CurvePoint {
                        x: 0.0,
                        y: size_pressure_start,
                    },
                );
                pressure
            },
            params: vec![("opacity".to_owned(), ParamValue::F32(opacity))],
            param_dynamics: Vec::new(),
            surface_source_material_scope: SurfaceSourceMaterialScope::BrushFootprint {
                extra_radius: Vec::new(),
            },
        }
    }

    fn stroke_command(command: &EditCommand) -> Option<&StrokeCommand> {
        match command {
            EditCommand::Stroke(command) => Some(command),
            _ => None,
        }
    }

    fn first_surface_add_surfaces(plan: &RendererFramePlan) -> Option<&PaintSurfaceSet> {
        plan.edit_commands
            .iter()
            .find_map(|command| match stroke_command(command) {
                Some(StrokeCommand::AddDabs {
                    dabs:
                        StrokeDabPayload::Surface {
                            target_surfaces, ..
                        },
                    ..
                }) => Some(target_surfaces),
                _ => None,
            })
    }

    fn first_surface_add_source_surfaces(plan: &RendererFramePlan) -> Option<&PaintSurfaceSet> {
        plan.edit_commands
            .iter()
            .find_map(|command| match stroke_command(command) {
                Some(StrokeCommand::AddDabs {
                    dabs:
                        StrokeDabPayload::Surface {
                            source_surfaces, ..
                        },
                    ..
                }) => Some(source_surfaces),
                _ => None,
            })
    }

    fn first_surface_add_preview_damage(plan: &RendererFramePlan) -> Option<&Option<DamageMap>> {
        plan.edit_commands
            .iter()
            .find_map(|command| match stroke_command(command) {
                Some(StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Surface { .. },
                    preview_damage,
                }) => Some(preview_damage),
                _ => None,
            })
    }

    fn stroke_end_damage(plan: &RendererFramePlan) -> Option<&Option<DamageMap>> {
        plan.edit_commands
            .iter()
            .find_map(|command| match stroke_command(command) {
                Some(StrokeCommand::End { damage }) => Some(damage),
                _ => None,
            })
    }

    fn first_stroke_command(plan: &RendererFramePlan) -> Option<&StrokeCommand> {
        plan.edit_commands.first().and_then(stroke_command)
    }

    fn first_stroke_dabs(plan: &RendererFramePlan) -> Option<&[StrokeDab]> {
        match first_stroke_command(plan)? {
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(dabs),
                ..
            } => Some(dabs.as_slice()),
            _ => None,
        }
    }

    fn has_stroke_dabs(plan: &RendererFramePlan) -> bool {
        plan.edit_commands.iter().any(|command| {
            matches!(
                stroke_command(command),
                Some(StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Stroke(_),
                    ..
                })
            )
        })
    }

    fn stroke_dab_count(plan: &RendererFramePlan) -> usize {
        plan.edit_commands
            .iter()
            .filter_map(stroke_command)
            .map(|command| match command {
                StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Stroke(dabs),
                    ..
                } => dabs.len(),
                _ => 0,
            })
            .sum()
    }

    fn has_stroke_end(plan: &RendererFramePlan) -> bool {
        plan.edit_commands
            .iter()
            .any(|command| matches!(stroke_command(command), Some(StrokeCommand::End { .. })))
    }

    #[test]
    fn continuous_uv_stroke_emits_on_down_time_and_pointer_up() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.18,
            rate_hz: 10.0,
        };
        let sample = |time_s| {
            PointerSample::uv_with_pressure_at(
                Vec2::new(0.25, 0.25),
                Vec2::new(25.0, 25.0),
                [100, 100],
                1.0,
                time_s,
                InputModifiers::default(),
            )
        };

        let down_plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample(0.0))),
        );
        assert_eq!(stroke_dab_count(&down_plan), 1);
        assert!((state.active_stroke_next_emit_time_s().unwrap() - 0.1).abs() < 1e-9);

        let early_plan = reduce(&mut state, Command::AdvanceActiveStroke { time_s: 0.05 });
        assert_eq!(stroke_dab_count(&early_plan), 0);

        let tick_plan = reduce(&mut state, Command::AdvanceActiveStroke { time_s: 0.1 });
        assert_eq!(stroke_dab_count(&tick_plan), 1);

        let up_plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample(0.2))),
        );
        assert_eq!(stroke_dab_count(&up_plan), 1);
        assert!(has_stroke_end(&up_plan));
        assert_eq!(state.active_stroke_next_emit_time_s(), None);
    }

    #[test]
    fn continuous_uv_stroke_caps_dabs_after_a_long_frame() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.18,
            rate_hz: 120.0,
        };
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.25, 0.25),
            Vec2::new(25.0, 25.0),
            [100, 100],
            1.0,
            0.0,
            InputModifiers::default(),
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );

        let tick_plan = reduce(&mut state, Command::AdvanceActiveStroke { time_s: 10.0 });

        assert_eq!(stroke_dab_count(&tick_plan), MAX_CONTINUOUS_DABS_PER_TICK);
        assert!(state.active_stroke_next_emit_time_s().unwrap() > 10.0);
    }

    #[test]
    fn movement_dabs_restart_the_continuous_deadline() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.01,
            rate_hz: 10.0,
        };
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.1, 0.1),
            Vec2::new(10.0, 10.0),
            [100, 100],
            1.0,
            0.0,
            InputModifiers::default(),
        );
        let moved = PointerSample::uv_with_pressure_at(
            Vec2::new(0.4, 0.1),
            Vec2::new(40.0, 10.0),
            [100, 100],
            1.0,
            0.05,
            InputModifiers::default(),
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let move_plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );

        assert!(stroke_dab_count(&move_plan) > 0);
        assert!((state.active_stroke_next_emit_time_s().unwrap() - 0.15).abs() < 1e-9);
        let early_plan = reduce(&mut state, Command::AdvanceActiveStroke { time_s: 0.1 });
        assert_eq!(stroke_dab_count(&early_plan), 0);
    }

    #[test]
    fn active_stroke_rereads_pressure_settings_from_the_current_preset() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure.curve = Default::default();

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(
                PointerSample::uv_with_pressure_at(
                    Vec2::new(0.1, 0.1),
                    Vec2::new(10.0, 10.0),
                    [100, 100],
                    0.5,
                    0.0,
                    InputModifiers::default(),
                ),
            )),
        );

        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure
            .curve
            .set_point(0, crate::core::curve::CurvePoint { x: 0.0, y: 1.0 });
        let plan = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(
                PointerSample::uv_with_pressure_at(
                    Vec2::new(0.2, 0.1),
                    Vec2::new(20.0, 10.0),
                    [100, 100],
                    0.5,
                    0.1,
                    InputModifiers::default(),
                ),
            )),
        );

        let dabs = first_stroke_dabs(&plan).expect("pointer move should emit a dab");
        assert_eq!(dabs.last().unwrap().radius_scale, 1.0);
    }

    #[test]
    fn stroke_dab_radius_scale_follows_pressure_flags() {
        let mut state = state_with_materials(1);
        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure.curve = Default::default();
        size_pressure
            .curve
            .set_point(0, crate::core::curve::CurvePoint { x: 0.0, y: 0.2 });

        let dab = stroke_dab_from_pressure(Vec2::ZERO, 0.5, effective_stroke_preset(&state));
        assert_eq!(dab.radius_scale, 0.6);
        assert_eq!(dab.pressure, 0.5);

        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure.enabled = false;

        let disabled = stroke_dab_from_pressure(Vec2::ZERO, 0.0, effective_stroke_preset(&state));
        assert_eq!(disabled.radius_scale, 1.0);
        assert_eq!(disabled.pressure, 0.0);
    }

    #[test]
    fn stroke_dab_applies_pressure_feel_before_dynamics() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).pressure_response = PressureResponse::new(50.0);
        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure.curve = Default::default();

        let dab = stroke_dab_from_pressure(Vec2::ZERO, 0.5, effective_stroke_preset(&state));
        assert!((dab.pressure - 0.25).abs() < 1e-6);
        assert!((dab.radius_scale - 0.25).abs() < 1e-6);
    }

    #[test]
    fn stroke_dab_custom_curve_matches_simple_gpu_paint_size_response() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).pressure_response = PressureResponse::default();
        let StrokePresetOp::BrushEngine { size_pressure, .. } =
            &mut active_stroke_preset_mut(&mut state).stroke_op;
        size_pressure.curve = Default::default();
        size_pressure
            .curve
            .insert_point(crate::core::curve::CurvePoint { x: 0.5, y: 0.25 });

        let dab = stroke_dab_from_pressure(Vec2::ZERO, 0.5, effective_stroke_preset(&state));
        assert!((dab.pressure - 0.5).abs() < 1e-6);
        assert!((dab.radius_scale - 0.25).abs() < 1e-6);
    }

    #[test]
    fn pressure_feel_above_neutral_reaches_full_pressure_early() {
        let response = PressureResponse::new(200.0);

        assert!((response.apply(0.25) - 0.5).abs() < 1e-6);
        assert!((response.apply(0.75) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn uv_drag_move_dabs_keep_corrected_pressure() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.1, 0.1),
            Vec2::new(10.0, 10.0),
            [100, 100],
            1.0,
            0.0,
            InputModifiers::default(),
        );
        let moved = PointerSample::uv_with_pressure_at(
            Vec2::new(0.2, 0.1),
            Vec2::new(20.0, 10.0),
            [100, 100],
            0.25,
            0.1,
            InputModifiers::default(),
        );

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );

        let Some(dabs) = first_stroke_dabs(&effects) else {
            panic!("expected UV drag move to draw a dab");
        };
        assert_eq!(dabs.len(), 1);
        assert!((dabs[0].pressure - 0.25).abs() < 1e-6);
    }

    #[test]
    fn sample_segment_interpolates_position_and_pressure() {
        let points = sample_segment(
            Some(StrokeDab::new(Vec2::new(0.0, 0.0), 0.2)),
            StrokeDab::new(Vec2::new(1.0, 0.0), 0.8),
            &StrokeStrategy::SpacingDab { spacing: 0.5 },
        );
        assert_eq!(points.len(), 2);
        assert!((points[0].position.x - 0.5).abs() < 1e-6);
        assert!((points[0].pressure - 0.5).abs() < 1e-6);
        assert!((points[1].position.x - 1.0).abs() < 1e-6);
        assert!((points[1].pressure - 0.8).abs() < 1e-6);
    }

    #[test]
    fn surface_sampling_restarts_on_sharp_normal_change() {
        let state = state_with_materials(1);
        let from = SurfaceDab::with_scales(
            Vec2::new(0.0, 0.0),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
            0,
            1.0,
            1.0,
        );
        let sample = PointerSample::surface_with_pressure(
            Vec2::new(2.0, 0.0),
            1.0,
            InputModifiers::default(),
            surface_test_view_context([800, 600], [0.0, 0.0, 2.0]),
            SurfaceHit {
                world_pos: glam::Vec3::new(0.1, 0.0, 0.0),
                world_normal: glam::Vec3::X,
                uv: Vec2::ZERO,
                uv_edge_distance: f32::INFINITY,
                uv_paint_boundary_distance: f32::INFINITY,
                triangle_index: 0,
                material_index: 0.into(),
                mesh_id: MeshId(0),
                t: 1.0,
            },
        );

        let dabs = sample_surface_segment(
            &state,
            &sample,
            Some(from),
            StrokeDab::new(Vec2::new(2.0, 0.0), 1.0),
            &StrokeStrategy::SpacingDab { spacing: 1.0 },
            1.0,
        );

        assert_eq!(dabs.len(), 1);
        assert_eq!(dabs[0].world_normal, glam::Vec3::X);
    }

    #[test]
    fn surface_sampling_scales_spacing_with_pressure_radius() {
        let state = state_with_materials(1);
        let from = SurfaceDab::with_scales(
            Vec2::new(0.0, 0.0),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
            0,
            1.0,
            0.1,
        );
        let sample = PointerSample::surface_with_pressure(
            Vec2::new(2.0, 0.0),
            1.0,
            InputModifiers::default(),
            surface_test_view_context([800, 600], [0.0, 0.0, 2.0]),
            SurfaceHit {
                world_pos: glam::Vec3::new(0.2, 0.0, 0.0),
                world_normal: glam::Vec3::Z,
                uv: Vec2::ZERO,
                uv_edge_distance: f32::INFINITY,
                uv_paint_boundary_distance: f32::INFINITY,
                triangle_index: 0,
                material_index: 0.into(),
                mesh_id: MeshId(0),
                t: 1.0,
            },
        );
        let to = StrokeDab::with_scales(Vec2::new(2.0, 0.0), 1.0, 0.1);

        let dabs = sample_surface_segment(
            &state,
            &sample,
            Some(from),
            to,
            &StrokeStrategy::SpacingDab { spacing: 1.0 },
            1.0,
        );

        assert!(
            !dabs.is_empty(),
            "low pressure radius should reduce effective spacing"
        );
    }

    #[test]
    fn surface_strategy_keeps_spacing_as_ratio() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).stroke_strategy =
            StrokeStrategy::SpacingDab { spacing: 0.25 };
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.5, 0.1, 1.0);

        let strategy = normalized_stroke_strategy(&effective_stroke_preset(&state).stroke_strategy);

        assert_eq!(strategy, StrokeStrategy::SpacingDab { spacing: 0.25 });
    }

    #[test]
    fn moving_average_window_zero_bypasses_position_and_pressure() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter {
            stabilization: 0,
            pressure_filter: 0,
        };
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
            [1, 1],
            1.0,
            0.0,
            InputModifiers::default(),
        );
        let moved = PointerSample::uv_with_pressure_at(
            Vec2::new(0.5, 0.0),
            Vec2::new(0.5, 0.0),
            [1, 1],
            0.25,
            0.016,
            InputModifiers::default(),
        );

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let move_effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );
        let Some(dabs) = first_stroke_dabs(&move_effects) else {
            panic!("expected raw move dab");
        };
        assert!((dabs[0].position.x - 0.5).abs() < 1e-6);
        assert!((dabs[0].pressure - 0.25).abs() < 1e-6);
    }

    #[test]
    fn moving_average_filters_position_with_filtered_feedback() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter {
            stabilization: 2,
            pressure_filter: 0,
        };
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
            [1, 1],
            1.0,
            0.0,
            InputModifiers::default(),
        );
        let move_1 = PointerSample::uv_with_pressure_at(
            Vec2::new(0.1, 0.0),
            Vec2::new(10.0, 0.0),
            [100, 1],
            1.0,
            0.016,
            InputModifiers::default(),
        );
        let move_2 = PointerSample::uv_with_pressure_at(
            Vec2::new(0.1, 0.0),
            Vec2::new(10.0, 0.0),
            [100, 1],
            1.0,
            0.032,
            InputModifiers::default(),
        );

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let first = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(move_1)),
        );
        let Some(first_dabs) = first_stroke_dabs(&first) else {
            panic!("expected first filtered move dab");
        };
        assert!((first_dabs[0].position.x - 0.05).abs() < 1e-6);

        let second = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(move_2)),
        );
        let Some(second_dabs) = first_stroke_dabs(&second) else {
            panic!("expected second filtered move dab");
        };
        assert!((second_dabs[0].position.x - 0.075).abs() < 1e-6);
    }

    #[test]
    fn moving_average_filters_pressure_with_filtered_feedback() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter {
            stabilization: 0,
            pressure_filter: 2,
        };
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
            [1, 1],
            0.0,
            0.0,
            InputModifiers::default(),
        );
        let move_1 = PointerSample::uv_with_pressure_at(
            Vec2::new(0.5, 0.0),
            Vec2::new(0.5, 0.0),
            [1, 1],
            1.0,
            0.016,
            InputModifiers::default(),
        );
        let move_2 = PointerSample::uv_with_pressure_at(
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 0.0),
            [1, 1],
            1.0,
            0.032,
            InputModifiers::default(),
        );

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let first = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(move_1)),
        );
        let Some(first_dabs) = first_stroke_dabs(&first) else {
            panic!("expected first pressure-filtered move dab");
        };
        assert!((first_dabs[0].pressure - 0.5).abs() < 1e-6);

        let second = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(move_2)),
        );
        let Some(second_dabs) = first_stroke_dabs(&second) else {
            panic!("expected second pressure-filtered move dab");
        };
        assert!((second_dabs[0].pressure - 0.75).abs() < 1e-6);
    }

    #[test]
    fn moving_average_pointer_up_does_not_draw_to_lift_off_position_after_drag() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter {
            stabilization: 2,
            pressure_filter: 2,
        };
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down = PointerSample::uv_with_pressure_at(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
            [1, 1],
            0.25,
            0.0,
            InputModifiers::default(),
        );
        let moved = PointerSample::uv_with_pressure_at(
            Vec2::new(0.5, 0.0),
            Vec2::new(0.5, 0.0),
            [1, 1],
            0.4,
            0.016,
            InputModifiers::default(),
        );
        let up = PointerSample::uv_with_pressure_at(
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 0.0),
            [1, 1],
            1.0,
            0.032,
            InputModifiers::default(),
        );

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );
        let up_effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(up)),
        );
        assert!(!has_stroke_dabs(&up_effects));
        assert!(has_stroke_end(&up_effects));
    }

    #[test]
    fn uv_stroke_history_damage_matches_finalize_stroke_damage() {
        let mut state = state_with_material_sizes([[1024, 1024]]);
        active_stroke_preset_mut(&mut state).stroke_strategy =
            StrokeStrategy::SpacingDab { spacing: 0.25 };
        let down = PointerSample::uv(Vec2::new(0.1, 0.1), InputModifiers::default());
        let moved = PointerSample::uv(Vec2::new(0.2, 0.1), InputModifiers::default());
        let up = PointerSample::uv(Vec2::new(0.3, 0.1), InputModifiers::default());
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );

        let mut scratch = super::StrokeSampleScratch::default();
        let outcome = crate::application::reducer::reduce_to_outcome(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(up)),
            &mut scratch,
        )
        .expect("pointer up should reduce");
        let history_damage = match outcome
            .pending_transaction
            .as_ref()
            .and_then(|pending| pending.renderer_history_transaction())
        {
            Some(PendingRendererHistoryTransaction::Pixel { edits }) => {
                let mut damage = DamageMap::default();
                for edit in edits {
                    damage.add_rect(edit.surface, edit.rect);
                }
                damage
            }
            other => panic!("expected pixel history transaction, got {other:?}"),
        };
        let finalize_damage = outcome
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None)
            .edit_commands
            .into_iter()
            .find_map(|command| match command {
                EditCommand::Stroke(StrokeCommand::End { damage }) => damage,
                _ => None,
            });

        assert!(!history_damage.is_empty());
        assert_eq!(finalize_damage, Some(history_damage));
    }

    #[test]
    fn tool_input_uv_maps_to_stroke_effects() {
        let mut state = state_with_materials(1);
        let sample =
            PointerSample::uv_with_pressure(Vec2::new(0.2, 0.8), 1.0, InputModifiers::default());
        let down = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        assert!(matches!(
            first_stroke_command(&down),
            Some(StrokeCommand::Begin { .. })
        ));
        assert!(matches!(
            first_stroke_command(&down),
            Some(StrokeCommand::Begin {
                target: StrokeTarget::Uv { .. },
                ..
            })
        ));
        assert!(!has_stroke_dabs(&down));

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        assert!(matches!(
            first_stroke_command(&up),
            Some(StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(dabs),
                ..
            }) if dabs.len() == 1
        ));
        assert!(has_stroke_end(&up));
    }

    #[test]
    fn pointer_down_defers_dot_until_pointer_up_with_down_pressure() {
        let mut state = state_with_materials(1);
        let down_sample =
            PointerSample::uv_with_pressure(Vec2::new(0.2, 0.8), 0.25, InputModifiers::default());
        let up_sample =
            PointerSample::uv_with_pressure(Vec2::new(0.2, 0.8), 1.0, InputModifiers::default());

        let down = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down_sample)),
        );
        assert!(!has_stroke_dabs(&down));

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(up_sample)),
        );
        let Some(dabs) = first_stroke_dabs(&up) else {
            panic!("expected tap-to-dot dab on pointer up");
        };
        assert_eq!(dabs.len(), 1);
        assert!((dabs[0].position - Vec2::new(0.2, 0.8)).length() < 1e-6);
        assert!((dabs[0].pressure - 0.25).abs() < 1e-6);
        assert!(has_stroke_end(&up));
    }

    #[test]
    fn pointer_up_position_and_pressure_are_ignored_after_drag_started() {
        let mut state = state_with_materials(1);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        let down =
            PointerSample::uv_with_pressure(Vec2::new(0.0, 0.0), 0.25, InputModifiers::default());
        let moved =
            PointerSample::uv_with_pressure(Vec2::new(0.5, 0.0), 0.4, InputModifiers::default());
        let up =
            PointerSample::uv_with_pressure(Vec2::new(1.0, 0.0), 1.0, InputModifiers::default());

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(moved)),
        );
        let up_effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(up)),
        );

        assert!(!has_stroke_dabs(&up_effects));
        assert!(has_stroke_end(&up_effects));
    }

    #[test]
    fn tool_input_uses_active_tool_preset_from_state() {
        let mut state = state_with_materials(1);
        let blur_tool_id = state
            .tool_catalog()
            .iter()
            .find(|tool| tool.config_id == "builtin.brush.filter.blur_soft")
            .expect("blur brush preset should be registered")
            .id;
        state.set_active_tool(blur_tool_id);
        let blur_preset_index = state.active_stroke_preset_index().unwrap();
        state.tool.tool_presets[blur_preset_index].stroke_op =
            brush_engine_preset_op("blur", 0.1, 0.1, 0.8);
        let sample =
            PointerSample::uv_with_pressure(Vec2::new(0.4, 0.4), 1.0, InputModifiers::default());
        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample)),
        );
        assert!(matches!(
            first_stroke_command(&effects),
            Some(StrokeCommand::Begin { style, .. })
            if matches!(
                &style.stroke_op,
                StrokeOp::BrushEngine { engine_id, .. } if engine_id == "blur"
            )
        ));
        assert!(!has_stroke_dabs(&effects));
    }

    #[test]
    fn active_stroke_keeps_color_captured_at_pointer_down() {
        let mut state = state_with_materials(1);
        state.tool.current_color = [0.2, 0.3, 0.4];
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.1, 0.1, 0.75);
        let down =
            PointerSample::uv_with_pressure(Vec2::new(0.1, 0.1), 1.0, InputModifiers::default());

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down.clone())),
        );
        state.tool.current_color = [0.8, 0.7, 0.6];
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.2, 0.1, 1.0);

        assert!(matches!(
            state.tool.tool_session,
            ToolSession::Stroke(ref session)
                if matches!(
                    &session.style().operation,
                    ToolOperation::BrushEngine(op)
                        if op.color == [0.2, 0.3, 0.4]
                ) && matches!(
                    &session.style().stroke_op,
                    StrokeOp::BrushEngine {
                        engine_id,
                        params,
                        ..
                    } if engine_id == "paint" && params.iter().any(|(name, value)| name == "opacity" && matches!(value, ParamValue::F32(value) if (*value - 0.75).abs() < f32::EPSILON))
                )
        ));

        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(
                PointerSample::uv_with_pressure(
                    Vec2::new(0.5, 0.5),
                    1.0,
                    InputModifiers::default(),
                ),
            )),
        );

        assert!(matches!(
            first_stroke_command(&effects),
            Some(StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(_),
                ..
            })
        ));
    }

    #[test]
    fn surface_stroke_finalizes_even_when_pointer_up_has_no_hit() {
        let mut state = state_with_materials(1);
        let hit = SurfaceHit {
            world_pos: glam::Vec3::ZERO,
            world_normal: glam::Vec3::Z,
            uv: Vec2::ZERO,
            uv_edge_distance: f32::INFINITY,
            uv_paint_boundary_distance: f32::INFINITY,
            triangle_index: 0,
            material_index: 0.into(),
            mesh_id: MeshId(0),
            t: 1.0,
        };
        let down = PointerSample::surface_with_pressure(
            Vec2::new(10.0, 20.0),
            1.0,
            InputModifiers::default(),
            surface_test_view_context([800, 600], [1.0, 2.0, 3.0]),
            hit,
        );
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down)),
        );
        assert!(matches!(
            state.tool.tool_session,
            ToolSession::Stroke(ref session) if session.space == StrokeSpace::Surface
        ));

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(
                PointerSample::surface_without_hit_with_pressure(
                    Vec2::new(20.0, 30.0),
                    1.0,
                    InputModifiers::default(),
                    surface_test_view_context([800, 600], [1.0, 2.0, 3.0]),
                ),
            )),
        );

        assert!(state.tool.is_idle());
        assert!(has_stroke_end(&up));
    }

    #[test]
    fn surface_stroke_defers_history_capture_until_finalize() {
        let mut state = state_with_materials(1);
        let expected_surfaces = state.active_paint_surfaces();
        let hit = SurfaceHit {
            world_pos: glam::Vec3::ZERO,
            world_normal: glam::Vec3::Z,
            uv: Vec2::ZERO,
            uv_edge_distance: f32::INFINITY,
            uv_paint_boundary_distance: f32::INFINITY,
            triangle_index: 0,
            material_index: 0.into(),
            mesh_id: MeshId(0),
            t: 1.0,
        };
        let down = PointerSample::surface_with_pressure(
            Vec2::new(10.0, 20.0),
            1.0,
            InputModifiers::default(),
            surface_test_view_context([800, 600], [1.0, 2.0, 3.0]),
            hit,
        );
        let mut renderer_plan = RendererPlanBuilder::default();
        let mut scratch = StrokeSampleScratch::default();

        let down_output = handle_tool_input_into_with_scratch(
            &mut state,
            ToolInputEvent::PointerDown(down),
            &mut renderer_plan,
            &mut scratch,
        )
        .unwrap();
        let down_plan = renderer_plan.drain_into_frame_plan(Vec::new(), None);

        assert!(down_output.history_capture.is_none());
        assert!(down_output.renderer_commit.is_empty());
        assert!(matches!(
            first_stroke_command(&down_plan),
            Some(StrokeCommand::Begin {
                target: StrokeTarget::Surface { .. },
                ..
            })
        ));

        let mut renderer_plan = RendererPlanBuilder::default();
        let up_output = handle_tool_input_into_with_scratch(
            &mut state,
            ToolInputEvent::PointerUp(PointerSample::surface_without_hit_with_pressure(
                Vec2::new(20.0, 30.0),
                1.0,
                InputModifiers::default(),
                surface_test_view_context([800, 600], [1.0, 2.0, 3.0]),
            )),
            &mut renderer_plan,
            &mut scratch,
        )
        .unwrap();

        assert!(up_output.history_capture.is_some());
        assert_eq!(
            up_output.renderer_commit.finalized_surfaces,
            expected_surfaces
        );
        let up_plan = renderer_plan.drain_into_frame_plan(Vec::new(), None);
        assert!(has_stroke_end(&up_plan));
    }

    #[test]
    fn surface_stroke_preview_history_and_finalize_share_rect_damage() {
        let mut state = state_with_unit_plane_material_size([1024, 1024]);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.02, 0.02, 1.0);
        let sample = surface_sample_at_uv(Vec3::ZERO, 0, Vec2::new(0.5, 0.5), 1.0);

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let mut scratch = super::StrokeSampleScratch::default();
        let outcome = crate::application::reducer::reduce_to_outcome(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
            &mut scratch,
        )
        .expect("pointer up should reduce");
        let history_damage = match outcome
            .pending_transaction
            .as_ref()
            .and_then(|pending| pending.renderer_history_transaction())
        {
            Some(PendingRendererHistoryTransaction::Pixel { edits }) => {
                let mut damage = DamageMap::default();
                for edit in edits {
                    damage.add_rect(edit.surface, edit.rect);
                }
                damage
            }
            other => panic!("expected pixel history transaction, got {other:?}"),
        };
        let plan = outcome
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        let preview_damage = first_surface_add_preview_damage(&plan)
            .cloned()
            .expect("expected surface preview damage");
        let finalize_damage = stroke_end_damage(&plan)
            .cloned()
            .expect("expected stroke end damage");

        assert!(!history_damage.is_empty());
        assert_eq!(preview_damage, Some(history_damage.clone()));
        assert_eq!(finalize_damage, Some(history_damage.clone()));
        let rect = history_damage.pixels[0].rect;
        assert!(
            rect.size[0] < 1024 && rect.size[1] < 1024,
            "surface dab away from seams should use rect damage, got {rect:?}"
        );
    }

    #[test]
    fn surface_stroke_damage_falls_back_to_full_surface_near_uv_edge() {
        let mut state = state_with_material_sizes([[1024, 1024]]);
        active_stroke_preset_mut(&mut state).input_filter = StrokeInputFilter::default();
        active_stroke_preset_mut(&mut state).stroke_strategy = StrokeStrategy::RawEvent;
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.02, 0.02, 1.0);
        let sample = surface_sample_at_uv(Vec3::ZERO, 0, Vec2::new(0.5, 0.5), 0.0);

        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let damage = first_surface_add_preview_damage(&effects)
            .and_then(|damage| damage.as_ref())
            .expect("expected explicit full surface fallback damage");

        assert_eq!(damage.pixels.len(), 1);
        assert_eq!(damage.pixels[0].rect.origin, [0, 0]);
        assert_eq!(damage.pixels[0].rect.size, [1024, 1024]);
    }

    #[test]
    fn surface_stroke_uses_only_visible_materials_in_brush_footprint() {
        let mut state = state_with_two_material_view_plane();
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.1, 0.1, 1.0);
        let sample = surface_sample_at(Vec3::new(-0.55, 0.0, 0.0), 0);
        let down = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let Some(StrokeCommand::Begin {
            target: StrokeTarget::Surface { surfaces, .. },
            ..
        }) = first_stroke_command(&down)
        else {
            panic!("expected surface stroke begin");
        };
        assert_eq!(surface_material_indices(surfaces), vec![0]);

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let add_surfaces = first_surface_add_surfaces(&up).expect("expected surface dab payload");
        assert_eq!(surface_material_indices(add_surfaces), vec![0]);
        let source_surfaces =
            first_surface_add_source_surfaces(&up).expect("expected surface dab payload");
        assert_eq!(surface_material_indices(source_surfaces), vec![0]);
        assert_eq!(
            up.commit_request
                .as_ref()
                .map(|request| {
                    request
                        .finalized_surfaces
                        .iter()
                        .map(|surface| surface.material_index.as_usize())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            vec![0]
        );
    }

    #[test]
    fn surface_mirror_emits_primary_and_mirror_batches_around_configured_plane() {
        let mut state = state_with_two_material_view_plane();
        state.view.surface_mirror_x_enabled = true;
        state.view.surface_mirror_x_plane = 0.1;
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.1, 0.1, 1.0);
        let sample = surface_sample_at(Vec3::new(-0.55, 0.0, 0.0), 0);
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let frame = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let payload = frame
            .edit_commands
            .iter()
            .find_map(|command| match command {
                EditCommand::Stroke(StrokeCommand::AddDabs { dabs, .. }) => Some(dabs),
                _ => None,
            });
        let Some(StrokeDabPayload::Surface {
            target_surfaces,
            projection_batches,
            ..
        }) = payload
        else {
            panic!("expected mirrored surface payload");
        };
        assert_eq!(surface_material_indices(target_surfaces), vec![0, 1]);
        assert_eq!(projection_batches.len(), 2);
        assert_eq!(
            projection_batches[0].projection_id,
            SurfaceProjectionId::Primary
        );
        assert_eq!(
            projection_batches[1].projection_id,
            SurfaceProjectionId::MirrorX
        );
        assert_eq!(projection_batches[0].dabs.len(), 1);
        assert_eq!(projection_batches[1].dabs.len(), 1);
        assert!((projection_batches[1].dabs[0].world_pos.x - 0.75).abs() < 1e-4);
        assert!(
            frame
                .edit_commands
                .iter()
                .any(|command| matches!(command, EditCommand::Stroke(StrokeCommand::End { .. })))
        );
    }

    #[test]
    fn surface_mirror_plane_is_fixed_for_active_stroke() {
        let mut state = state_with_two_material_view_plane();
        let _ = reduce(&mut state, Command::SetSurfaceMirrorXPlane(0.1));
        let _ = reduce(&mut state, Command::SetSurfaceMirrorXEnabled(true));
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.1, 0.1, 1.0);
        let sample = surface_sample_at(Vec3::new(-0.55, 0.0, 0.0), 0);
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );

        let _ = reduce(&mut state, Command::SetSurfaceMirrorXPlane(-0.2));
        assert_eq!(state.surface_mirror_options().x_plane, 0.1);
        state.view.surface_mirror_x_plane = -0.2;

        let frame = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let mirror_x = frame
            .edit_commands
            .iter()
            .find_map(|command| match command {
                EditCommand::Stroke(StrokeCommand::AddDabs {
                    dabs:
                        StrokeDabPayload::Surface {
                            projection_batches, ..
                        },
                    ..
                }) => projection_batches
                    .iter()
                    .find(|batch| batch.projection_id == SurfaceProjectionId::MirrorX)
                    .and_then(|batch| batch.dabs.first())
                    .map(|dab| dab.world_pos.x),
                _ => None,
            })
            .expect("expected mirror dab");
        assert!((mirror_x - 0.75).abs() < 1e-4);
    }

    #[test]
    fn surface_mirror_does_not_duplicate_dab_on_configured_plane() {
        let mut state = state_with_two_material_view_plane();
        state.view.surface_mirror_x_enabled = true;
        state.view.surface_mirror_x_plane = -0.55;
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.1, 0.1, 1.0);
        let sample = surface_sample_at(Vec3::new(-0.55, 0.0, 0.0), 0);
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let frame = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let projection_batches = frame
            .edit_commands
            .iter()
            .find_map(|command| match command {
                EditCommand::Stroke(StrokeCommand::AddDabs {
                    dabs:
                        StrokeDabPayload::Surface {
                            projection_batches, ..
                        },
                    ..
                }) => Some(projection_batches),
                _ => None,
            })
            .expect("expected surface payload");
        assert_eq!(projection_batches.len(), 1);
        assert_eq!(
            projection_batches[0].projection_id,
            SurfaceProjectionId::Primary
        );
    }

    #[test]
    fn surface_stroke_source_materials_use_ron_declared_extra_radius() {
        let mut state = state_with_two_material_view_plane();
        active_stroke_preset_mut(&mut state).stroke_op = StrokePresetOp::BrushEngine {
            engine_id: "paint".to_owned(),
            size_scene_ratio: RangedF32 {
                value: 0.1,
                range: None,
            },
            size_pressure: PressureDynamics::enabled(),
            params: vec![("opacity".to_owned(), ParamValue::F32(1.0))],
            param_dynamics: Vec::new(),
            surface_source_material_scope: SurfaceSourceMaterialScope::BrushFootprint {
                extra_radius: vec![SurfaceSourceRadiusTerm::BrushRadiusRatio {
                    param: "opacity".to_owned(),
                    scale: 5.0,
                }],
            },
        };
        let sample = surface_sample_at(Vec3::new(-0.55, 0.0, 0.0), 0);
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let target_surfaces =
            first_surface_add_surfaces(&up).expect("expected surface dab payload");
        assert_eq!(surface_material_indices(target_surfaces), vec![0]);
        let source_surfaces =
            first_surface_add_source_surfaces(&up).expect("expected surface dab payload");
        assert_eq!(surface_material_indices(source_surfaces), vec![0, 1]);
    }

    #[test]
    fn surface_stroke_collects_visible_materials_across_brush_footprint_boundary() {
        let mut state = state_with_two_material_view_plane();
        active_stroke_preset_mut(&mut state).stroke_op =
            brush_engine_preset_op("paint", 0.3, 0.3, 1.0);
        let sample = surface_sample_at(Vec3::new(-0.05, 0.0, 0.0), 0);
        let down = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample.clone())),
        );
        let Some(StrokeCommand::Begin {
            target: StrokeTarget::Surface { surfaces, .. },
            ..
        }) = first_stroke_command(&down)
        else {
            panic!("expected surface stroke begin");
        };
        assert_eq!(surface_material_indices(surfaces), vec![0, 1]);

        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(sample)),
        );
        let add_surfaces = first_surface_add_surfaces(&up).expect("expected surface dab payload");
        assert_eq!(surface_material_indices(add_surfaces), vec![0, 1]);
        assert_eq!(
            up.commit_request
                .as_ref()
                .map(|request| {
                    let mut material_indices = request
                        .finalized_surfaces
                        .iter()
                        .map(|surface| surface.material_index.as_usize())
                        .collect::<Vec<_>>();
                    material_indices.sort_unstable();
                    material_indices
                })
                .unwrap_or_default(),
            vec![0, 1]
        );
    }

    #[test]
    fn uses_active_paint_surface_id() {
        let mut state = state_with_materials(1);
        let expected = state.active_paint_surface().unwrap();
        let sample =
            PointerSample::uv_with_pressure(Vec2::new(0.3, 0.3), 1.0, InputModifiers::default());
        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample)),
        );
        let Some(StrokeCommand::Begin {
            target: StrokeTarget::Uv { surface },
            ..
        }) = first_stroke_command(&effects)
        else {
            panic!("expected begin stroke command");
        };
        assert_eq!(surface.material_index.as_usize(), 0);
        assert_eq!(*surface, expected);
    }

    #[test]
    fn tool_input_without_active_paint_surface_emits_no_effects() {
        let mut state = AppState::default();
        let sample =
            PointerSample::uv_with_pressure(Vec2::new(0.3, 0.3), 1.0, InputModifiers::default());
        let effects = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(sample)),
        );
        assert!(effects.is_empty());
        assert!(state.tool.is_idle());
    }

    #[test]
    fn stroke_uses_target_locked_at_pointer_down() {
        let mut state = state_with_materials(2);
        let locked = state.active_paint_surface().unwrap();
        let down =
            PointerSample::uv_with_pressure(Vec2::new(0.1, 0.1), 1.0, InputModifiers::default());
        let _ = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerDown(down.clone())),
        );
        state.set_focused_material(1);
        let moved = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerMove(
                PointerSample::uv_with_pressure(
                    Vec2::new(0.5, 0.5),
                    1.0,
                    InputModifiers::default(),
                ),
            )),
        );
        let up = reduce(
            &mut state,
            Command::ToolInput(ToolInputEvent::PointerUp(down)),
        );

        assert!(
            moved
                .edit_commands
                .iter()
                .chain(up.edit_commands.iter())
                .all(|command| matches!(
                    stroke_command(command),
                    Some(StrokeCommand::AddDabs {
                        dabs: StrokeDabPayload::Stroke(_),
                        ..
                    }) | Some(StrokeCommand::End { .. })
                ))
        );
        assert_eq!(
            up.commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.as_slice()),
            Some([locked].as_slice())
        );
    }
}
