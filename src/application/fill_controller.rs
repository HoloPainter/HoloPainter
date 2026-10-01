use std::collections::HashMap;

use anyhow::Result;
use glam::Vec2;

use crate::{
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        damage::DamageMap,
        document::{Document, MeshId, SurfaceHit},
        math::ray_from_viewport_px,
        surface::PaintSurfaceId,
        tool::FillScope,
        viewport_visibility::ViewportSceneVisibility,
    },
    renderer::{
        ApplyCommand, ApplyOperation, EditCommand, FillCoverage, FillCoverageDelta,
        FillStrokeCommand,
    },
};

use super::{
    GEOMETRY_DAMAGE_GUARD_PX, PointerSample, PointerSampleKind, ReducerOutput, RendererCommitSpec,
    ToolInputEvent,
    damage::{mesh_fill_damage_for_surfaces, uv_rect_to_pixel_rect},
    history_capture::PixelHistoryCapture,
    state::{AppState, FillStrokeInputSpace, FillStrokeSession, FillTargetKey, PaintTargetScope},
    stroke_controller::StrokeSampleScratch,
};

const FILL_SAMPLE_STEP_PX: f32 = 2.0;
const MAX_FILL_SAMPLES_PER_EVENT: usize = 1024;

#[derive(Debug)]
struct ResolvedFillTarget {
    deltas: Vec<FillCoverageDelta>,
    damage: DamageMap,
    surfaces: Vec<PaintSurfaceId>,
}

pub(crate) fn handle_fill_input(
    state: &mut AppState,
    scope: FillScope,
    event: ToolInputEvent,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    match event {
        ToolInputEvent::PointerDown(sample) => begin_fill_stroke(state, scope, sample, scratch),
        ToolInputEvent::PointerMove(sample) => continue_fill_stroke(state, scope, sample, scratch),
        ToolInputEvent::PointerUp(sample) => finish_fill_stroke(state, scope, sample, scratch),
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before fill dispatch"),
    }
}

fn begin_fill_stroke(
    state: &mut AppState,
    scope: FillScope,
    sample: PointerSample,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    if !state.tool.is_idle() {
        return Ok(ReducerOutput::default());
    }
    let input_space = match sample.kind {
        PointerSampleKind::Uv { view, .. } => FillStrokeInputSpace::Uv {
            view,
            material_index: state.focused_material_index().into(),
            scene_visibility: state.viewport_scene_visibility().clone(),
        },
        PointerSampleKind::Surface { view, .. } => FillStrokeInputSpace::Surface {
            view,
            scene_visibility: state.viewport_scene_visibility().clone(),
        },
    };
    state
        .tool
        .set_session(super::state::ToolSession::Fill(FillStrokeSession {
            scope,
            input_space,
            last_screen_px: sample.screen_px(),
            visited_targets: Default::default(),
            affected_surfaces: Vec::new(),
            damage: DamageMap::default(),
        }));

    let document = match state.document() {
        Some(document) => document,
        None => {
            state.tool.clear_session();
            return Ok(ReducerOutput::default());
        }
    };
    let mut output = ReducerOutput::default();
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::Begin {
            operation: ApplyOperation::SolidColorPaint {
                color: state.current_color(),
            },
            params: ApplyParams::new(
                state.fill_options().opacity,
                TextureCompositeMode::SourceOver,
            ),
            active_selection: document.active_selection.clone(),
        },
    )));
    output.append_outcome(process_fill_segment(
        state,
        sample.screen_px(),
        sample.screen_px(),
        scratch,
    )?);
    Ok(output)
}

fn continue_fill_stroke(
    state: &mut AppState,
    scope: FillScope,
    sample: PointerSample,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let Some(session) = state.tool.fill_session() else {
        return Ok(ReducerOutput::default());
    };
    if session.scope != scope || session.input_space.space() != sample.space() {
        return Ok(ReducerOutput::default());
    }
    let from = session.last_screen_px;
    process_fill_segment(state, from, sample.screen_px(), scratch)
}

fn finish_fill_stroke(
    state: &mut AppState,
    scope: FillScope,
    sample: PointerSample,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let Some(session) = state.tool.fill_session() else {
        return Ok(ReducerOutput::default());
    };
    if session.scope != scope || session.input_space.space() != sample.space() {
        return Ok(ReducerOutput::default());
    }
    let from = session.last_screen_px;
    let mut output = process_fill_segment(state, from, sample.screen_px(), scratch)?;
    let Some(session) = state.tool.take_fill_session() else {
        return Ok(output);
    };
    let damage = (!session.damage.is_empty()).then_some(session.damage.clone());
    let history = match PixelHistoryCapture::from_optional_damage(damage.clone()) {
        Some(capture) => capture.capture(state)?,
        None => None,
    };
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::End {
            damage: damage.clone(),
        },
    )));
    if !session.affected_surfaces.is_empty() {
        output = output
            .with_optional_renderer_history_transaction(fill_history_label(scope), history)
            .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
                session.affected_surfaces,
            ));
    }
    Ok(output)
}

fn process_fill_segment(
    state: &mut AppState,
    from: Vec2,
    to: Vec2,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let Some(session) = state.tool.fill_session() else {
        return Ok(ReducerOutput::default());
    };
    let scope = session.scope;
    let input_space = session.input_space.clone();
    let samples = fill_segment_samples(from, to);
    let mut new_targets = Vec::new();
    for point in samples {
        let Some(target) = resolve_fill_target_at(state, scope, &input_space, point, scratch)
        else {
            continue;
        };
        if state
            .tool
            .fill_session()
            .is_some_and(|session| session.visited_targets.contains(&target))
        {
            continue;
        }
        if let Some(session) = state.tool.fill_session_mut() {
            session.visited_targets.insert(target);
        }
        new_targets.push(target);
    }
    if let Some(session) = state.tool.fill_session_mut() {
        session.last_screen_px = to;
    }

    let mut deltas = Vec::new();
    let mut preview_damage = DamageMap::default();
    for target in new_targets {
        let Some(resolved) = expand_fill_target(state, target, input_space.scene_visibility())
        else {
            continue;
        };
        deltas.extend(resolved.deltas);
        merge_damage(&mut preview_damage, &resolved.damage);
        if let Some(session) = state.tool.fill_session_mut() {
            merge_damage(&mut session.damage, &resolved.damage);
            for surface in resolved.surfaces {
                if !session.affected_surfaces.contains(&surface) {
                    session.affected_surfaces.push(surface);
                }
            }
        }
    }
    if deltas.is_empty() {
        return Ok(ReducerOutput::default());
    }
    let mut output = ReducerOutput::default();
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::Extend {
            deltas,
            preview_damage: (!preview_damage.is_empty()).then_some(preview_damage),
        },
    )));
    Ok(output)
}

fn resolve_fill_target_at(
    state: &AppState,
    scope: FillScope,
    input_space: &FillStrokeInputSpace,
    point: Vec2,
    scratch: &mut StrokeSampleScratch,
) -> Option<FillTargetKey> {
    match input_space {
        FillStrokeInputSpace::Uv {
            view,
            material_index,
            ..
        } => {
            if scope == FillScope::Material {
                return Some(FillTargetKey::Material {
                    material_index: *material_index,
                });
            }
            let document = state.document()?;
            let uv = view
                .transform
                .view_px_to_uv(point, view.size, view.canvas_size);
            let hit = document.mesh.pick_uv(uv, Some(material_index.as_usize()))?;
            fill_target_from_hit(
                scope,
                hit.mesh_id,
                hit.triangle_index,
                hit.material_index.into(),
            )
        }
        FillStrokeInputSpace::Surface {
            view,
            scene_visibility,
        } => {
            let document = state.document()?;
            let (ray_origin, ray_dir) = ray_from_viewport_px(point, view.size, view.inv_view_proj)?;
            let hit = document.raycast_visible_with_scratch(
                ray_origin,
                ray_dir,
                scene_visibility,
                scratch.raycast_scratch_mut(),
            )?;
            fill_target_from_surface_hit(scope, hit)
        }
    }
}

fn fill_target_from_surface_hit(scope: FillScope, hit: SurfaceHit) -> Option<FillTargetKey> {
    fill_target_from_hit(
        scope,
        hit.mesh_id,
        hit.triangle_index,
        hit.material_index.into(),
    )
}

fn fill_target_from_hit(
    scope: FillScope,
    mesh_id: MeshId,
    triangle_index: usize,
    material_index: crate::core::material::MaterialIndex,
) -> Option<FillTargetKey> {
    Some(match scope {
        FillScope::Polygon => FillTargetKey::Polygon {
            mesh_id,
            triangle_index,
        },
        FillScope::Mesh => FillTargetKey::Mesh { mesh_id },
        FillScope::Material => FillTargetKey::Material { material_index },
    })
}

fn expand_fill_target(
    state: &AppState,
    target: FillTargetKey,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<ResolvedFillTarget> {
    let document = state.document()?;
    match target {
        FillTargetKey::Material { material_index } => {
            expand_material_target(state, document, material_index)
        }
        FillTargetKey::Mesh { mesh_id } => {
            expand_mesh_target(state, document, mesh_id, scene_visibility)
        }
        FillTargetKey::Polygon {
            mesh_id,
            triangle_index,
        } => expand_polygon_target(state, document, mesh_id, triangle_index, scene_visibility),
    }
}

fn expand_material_target(
    state: &AppState,
    document: &Document,
    material_index: crate::core::material::MaterialIndex,
) -> Option<ResolvedFillTarget> {
    let paint_target = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Material(material_index))
        .into_allowed()?;
    let surface = paint_target.target();
    let texture_size = document.texture_size_for_surface(surface)?;
    let mut damage = DamageMap::default();
    damage.add_full_surface(surface, texture_size);
    Some(ResolvedFillTarget {
        deltas: vec![FillCoverageDelta {
            surface,
            coverage: FillCoverage::Full,
        }],
        damage,
        surfaces: vec![surface],
    })
}

fn expand_mesh_target(
    state: &AppState,
    document: &Document,
    mesh_id: MeshId,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<ResolvedFillTarget> {
    let paint_target = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Mesh(mesh_id))
        .into_allowed()?;
    let surfaces = paint_target.surfaces_vec();
    let surface_by_material = surfaces
        .iter()
        .copied()
        .map(|surface| (surface.material_index().as_usize(), surface))
        .collect::<HashMap<_, _>>();
    let mut triangles_by_surface = HashMap::<PaintSurfaceId, Vec<[Vec2; 3]>>::new();
    for (triangle_index, tri) in document.mesh.indices.iter().enumerate() {
        if document.mesh.triangle_mesh_ids.get(triangle_index).copied() != Some(mesh_id) {
            continue;
        }
        let material_index = document.mesh.material_index_for_triangle(triangle_index);
        if !scene_visibility.geometry_visible(mesh_id, material_index.into()) {
            continue;
        }
        let Some(&surface) = surface_by_material.get(&material_index) else {
            continue;
        };
        let uv = triangle_uv(document, *tri)?;
        triangles_by_surface.entry(surface).or_default().push(uv);
    }
    let deltas = triangles_by_surface
        .into_iter()
        .map(|(surface, triangles)| FillCoverageDelta {
            surface,
            coverage: FillCoverage::Triangles(triangles),
        })
        .collect::<Vec<_>>();
    if deltas.is_empty() {
        return None;
    }
    let damage = mesh_fill_damage_for_surfaces(
        document,
        mesh_id,
        &surfaces,
        scene_visibility,
        GEOMETRY_DAMAGE_GUARD_PX,
    );
    Some(ResolvedFillTarget {
        deltas,
        damage,
        surfaces,
    })
}

fn expand_polygon_target(
    state: &AppState,
    document: &Document,
    mesh_id: MeshId,
    triangle_index: usize,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<ResolvedFillTarget> {
    let tri = *document.mesh.indices.get(triangle_index)?;
    if document
        .mesh
        .triangle_mesh_ids
        .get(triangle_index)
        .copied()?
        != mesh_id
    {
        return None;
    }
    let material_index = document.mesh.material_index_for_triangle(triangle_index);
    if !scene_visibility.geometry_visible(mesh_id, material_index.into()) {
        return None;
    }
    let paint_target = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::Material(material_index.into()))
        .into_allowed()?;
    let surface = paint_target.target();
    let uv = triangle_uv(document, tri)?;
    let texture_size = document.texture_size_for_surface(surface)?;
    let min = uv[0].min(uv[1]).min(uv[2]);
    let max = uv[0].max(uv[1]).max(uv[2]);
    let mut damage = DamageMap::default();
    if let Some(rect) = uv_rect_to_pixel_rect(texture_size, min, max, GEOMETRY_DAMAGE_GUARD_PX) {
        damage.add_rect(surface, rect);
    } else {
        damage.add_full_surface(surface, texture_size);
    }
    Some(ResolvedFillTarget {
        deltas: vec![FillCoverageDelta {
            surface,
            coverage: FillCoverage::Triangles(vec![uv]),
        }],
        damage,
        surfaces: vec![surface],
    })
}

fn triangle_uv(document: &Document, tri: [u32; 3]) -> Option<[Vec2; 3]> {
    Some([
        *document.mesh.uvs.get(tri[0] as usize)?,
        *document.mesh.uvs.get(tri[1] as usize)?,
        *document.mesh.uvs.get(tri[2] as usize)?,
    ])
}

fn fill_segment_samples(from: Vec2, to: Vec2) -> Vec<Vec2> {
    let distance = from.distance(to);
    if !distance.is_finite() || distance <= f32::EPSILON {
        return vec![to];
    }
    let steps =
        ((distance / FILL_SAMPLE_STEP_PX).ceil() as usize).clamp(1, MAX_FILL_SAMPLES_PER_EVENT);
    (1..=steps)
        .map(|index| from.lerp(to, index as f32 / steps as f32))
        .collect()
}

fn merge_damage(target: &mut DamageMap, incoming: &DamageMap) {
    for pixel in &incoming.pixels {
        target.add_rect(pixel.surface, pixel.rect);
    }
}

fn fill_history_label(scope: FillScope) -> &'static str {
    match scope {
        FillScope::Polygon => "Fill Polygon",
        FillScope::Mesh => "Fill Mesh",
        FillScope::Material => "Fill Material",
    }
}

pub(crate) fn fill_once(
    state: &AppState,
    target: FillTargetKey,
    color: [f32; 3],
    opacity: f32,
) -> Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let Some(resolved) = expand_fill_target(state, target, state.viewport_scene_visibility())
    else {
        return Ok(ReducerOutput::default());
    };
    let history = match PixelHistoryCapture::from_optional_damage(Some(resolved.damage.clone())) {
        Some(capture) => capture.capture(state)?,
        None => None,
    };
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction(fill_target_history_label(target), history)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            resolved.surfaces.clone(),
        ));
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::Begin {
            operation: ApplyOperation::SolidColorPaint { color },
            params: ApplyParams::new(opacity, TextureCompositeMode::SourceOver),
            active_selection: document.active_selection.clone(),
        },
    )));
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::Extend {
            deltas: resolved.deltas,
            preview_damage: Some(resolved.damage.clone()),
        },
    )));
    output.push_edit_command(EditCommand::Apply(ApplyCommand::FillStroke(
        FillStrokeCommand::End {
            damage: Some(resolved.damage),
        },
    )));
    Ok(output)
}

fn fill_target_history_label(target: FillTargetKey) -> &'static str {
    match target {
        FillTargetKey::Polygon { .. } => "Fill Polygon",
        FillTargetKey::Mesh { .. } => "Fill Mesh",
        FillTargetKey::Material { .. } => "Fill Material",
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::fill_segment_samples;

    #[test]
    fn fill_segment_sampling_includes_end_and_intermediate_points() {
        let samples = fill_segment_samples(Vec2::ZERO, Vec2::new(5.0, 0.0));
        assert!(samples.len() >= 3);
        assert_eq!(samples.last().copied(), Some(Vec2::new(5.0, 0.0)));
    }
}
