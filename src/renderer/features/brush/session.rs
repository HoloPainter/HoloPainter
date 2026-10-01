use std::{collections::HashMap, sync::Arc};

use anyhow::{Result, bail};
use glam::Vec2;

use crate::{
    core::{
        damage::DamageMap,
        selection::ActiveSelection,
        stroke::{PaintSurfaceSet, StrokeContext, StrokeDab, SurfaceDab, SurfaceProjectionId},
        surface::PaintSurfaceId,
    },
    renderer::{
        command::{RendererStrokeStyle, StrokeDabPayload, StrokeSpace},
        features::brush::types::BrushDabInstance,
        gpu::frame::GpuFrame,
    },
};

use super::{
    engine_types::BrushEngineUniform, executor::BrushPassExecutor, pass_deps::BrushPassDeps,
    viewport_cache::SurfaceViewportRuntimeCache,
};

pub(crate) struct BrushStrokeSession {
    style: RendererStrokeStyle,
    brush_uniform: BrushEngineUniform,
    space: StrokeSpace,
    context: Option<Arc<StrokeContext>>,
    active_selection: Arc<ActiveSelection>,
    last_uv_dab: Option<StrokeDab>,
    last_surface_dab: HashMap<SurfaceProjectionId, SurfaceDab>,
    last_surface_viewport_dab: HashMap<SurfaceProjectionId, StrokeDab>,
    surface_viewport_runtime_cache: SurfaceViewportRuntimeCache,
}

impl BrushStrokeSession {
    pub(crate) fn uv(
        target: PaintSurfaceId,
        style: &RendererStrokeStyle,
        brush_uniform: BrushEngineUniform,
        active_selection: Arc<ActiveSelection>,
    ) -> Self {
        Self {
            style: style.clone(),
            brush_uniform,
            space: StrokeSpace::Uv { target },
            context: None,
            active_selection,
            last_uv_dab: None,
            last_surface_dab: HashMap::new(),
            last_surface_viewport_dab: HashMap::new(),
            surface_viewport_runtime_cache: SurfaceViewportRuntimeCache::default(),
        }
    }

    pub(crate) fn surface(
        surfaces: PaintSurfaceSet,
        context: Arc<StrokeContext>,
        style: &RendererStrokeStyle,
        brush_uniform: BrushEngineUniform,
        active_selection: Arc<ActiveSelection>,
    ) -> Self {
        Self {
            style: style.clone(),
            brush_uniform,
            space: StrokeSpace::Surface { surfaces },
            context: Some(context),
            active_selection,
            last_uv_dab: None,
            last_surface_dab: HashMap::new(),
            last_surface_viewport_dab: HashMap::new(),
            surface_viewport_runtime_cache: SurfaceViewportRuntimeCache::default(),
        }
    }

    pub(crate) fn space(&self) -> &StrokeSpace {
        &self.space
    }

    pub(crate) fn style(&self) -> &RendererStrokeStyle {
        &self.style
    }

    pub(crate) fn preview_surfaces_for_dabs(
        &self,
        dabs: &StrokeDabPayload,
    ) -> Result<PaintSurfaceSet> {
        match (self.space(), dabs) {
            (StrokeSpace::Uv { .. }, StrokeDabPayload::Stroke(_)) => Ok(self.space().surfaces()),
            (
                StrokeSpace::Surface { .. },
                StrokeDabPayload::Surface {
                    target_surfaces, ..
                },
            ) => Ok(target_surfaces.clone()),
            _ => bail!("stroke dabs submitted for the wrong stroke space"),
        }
    }

    pub(crate) fn add_surface_targets(&mut self, surfaces: &PaintSurfaceSet) -> PaintSurfaceSet {
        let StrokeSpace::Surface {
            surfaces: active_surfaces,
        } = &self.space
        else {
            return PaintSurfaceSet::from_vec(Vec::new());
        };
        let mut active = active_surfaces.as_slice().to_vec();
        let mut added = Vec::new();
        for surface in surfaces.iter() {
            if !active.contains(&surface) {
                active.push(surface);
                added.push(surface);
            }
        }
        self.space = StrokeSpace::Surface {
            surfaces: PaintSurfaceSet::from_vec(active),
        };
        PaintSurfaceSet::from_vec(added)
    }

    fn active_selection(&self) -> &ActiveSelection {
        self.active_selection.as_ref()
    }

    pub(crate) fn stamp_uv_batch(
        &mut self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
        dabs: &[StrokeDab],
    ) -> Result<()> {
        let instances = directional_instances(self.last_uv_dab, dabs);
        executor.stamp_uv_batch(
            frame,
            txn,
            target,
            dabs,
            &instances,
            &self.style,
            &self.brush_uniform,
            self.active_selection(),
        )?;
        self.last_uv_dab = dabs.last().copied().or(self.last_uv_dab);
        Ok(())
    }

    pub(crate) fn finalize_uv_stroke(
        &self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        target: PaintSurfaceId,
    ) {
        executor.finalize_uv_stroke(frame, txn, target);
    }

    pub(crate) fn cancel(
        &self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
    ) {
        let surfaces = self.space().surfaces();
        executor.cancel_stroke(frame, txn, surfaces.as_slice());
    }

    pub(crate) fn stamp_surface_batch(
        &mut self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        source_surfaces: &PaintSurfaceSet,
        target_surfaces: &PaintSurfaceSet,
        batch: &crate::renderer::command::SurfaceProjectionDabBatch,
    ) -> Result<()> {
        let dabs = &batch.dabs;
        let target_materials_by_dab = &batch.target_materials_by_dab;
        let previous_surface_dab = continued_surface_dab(
            &self.last_surface_dab,
            batch.projection_id,
            batch.continues_from_previous,
        );
        let surface_directions = surface_dab_directions(previous_surface_dab, dabs);
        let needs_viewport_instances =
            executor.surface_stamp_needs_viewport_instances(txn, &self.style)?;
        let last_viewport_dab = dabs.last().copied().map(surface_dab_to_viewport_dab);
        let viewport_instances = if needs_viewport_instances {
            let viewport_dabs: Vec<StrokeDab> = dabs
                .iter()
                .copied()
                .map(surface_dab_to_viewport_dab)
                .collect();
            let previous = batch
                .continues_from_previous
                .then(|| {
                    self.last_surface_viewport_dab
                        .get(&batch.projection_id)
                        .copied()
                })
                .flatten();
            directional_instances(previous, &viewport_dabs)
        } else {
            Vec::new()
        };
        let style = self.style.clone();
        let brush_uniform = self.brush_uniform;
        let context = self
            .context
            .as_ref()
            .expect("surface stroke sessions carry a locked context")
            .clone();
        let active_selection = Arc::clone(&self.active_selection);
        executor.stamp_surface_batch(
            frame,
            txn,
            source_surfaces.as_slice(),
            target_surfaces.as_slice(),
            dabs,
            &surface_directions,
            target_materials_by_dab,
            &viewport_instances,
            batch.projection_id,
            &style,
            &brush_uniform,
            context.as_ref(),
            active_selection.as_ref(),
            &mut self.surface_viewport_runtime_cache,
        )?;
        if let Some(last) = dabs.last().copied() {
            self.last_surface_dab.insert(batch.projection_id, last);
        }
        if let Some(last) = last_viewport_dab {
            self.last_surface_viewport_dab
                .insert(batch.projection_id, last);
        }
        Ok(())
    }

    pub(crate) fn clear_surface_viewport_material_bind_groups(&mut self) {
        self.surface_viewport_runtime_cache
            .clear_material_bind_groups();
    }

    pub(crate) fn begin_added_surface_targets(
        &self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &PaintSurfaceSet,
    ) {
        executor.begin_surface_stroke(frame, txn, surfaces.as_slice(), &self.style);
    }

    pub(crate) fn finalize_surface_stroke(
        &self,
        executor: &BrushPassExecutor,
        frame: &mut GpuFrame,
        txn: &mut BrushPassDeps<'_, '_>,
        surfaces: &PaintSurfaceSet,
        damage: Option<&DamageMap>,
    ) {
        executor.finalize_surface_stroke(frame, txn, surfaces.as_slice(), &self.style, damage);
    }
}

fn surface_dab_to_viewport_dab(dab: SurfaceDab) -> StrokeDab {
    StrokeDab::with_scales(dab.screen_px, dab.pressure, dab.radius_scale)
}

fn directional_instances(
    previous_dab: Option<StrokeDab>,
    dabs: &[StrokeDab],
) -> Vec<BrushDabInstance> {
    let directions = dab_directions(previous_dab, dabs);
    dabs.iter()
        .zip(directions)
        .map(|(dab, dir)| BrushDabInstance {
            center_px: [dab.position.x, dab.position.y],
            radius_scale: dab.radius_scale,
            pressure: dab.pressure,
            spacing_alpha_scale: dab.spacing_alpha_scale,
            _pad0: 0.0,
            dir: [dir.x, dir.y],
            dynamic0: [0.0; 4],
            dynamic1: [0.0; 4],
            dynamic2: [0.0; 4],
            dynamic3: [0.0; 4],
        })
        .collect()
}

fn dab_directions(previous_dab: Option<StrokeDab>, dabs: &[StrokeDab]) -> Vec<Vec2> {
    let mut directions = Vec::with_capacity(dabs.len());
    let mut previous = previous_dab;
    for dab in dabs {
        let dir = previous
            .map(|prev| normalized_or_zero(dab.position - prev.position))
            .unwrap_or(Vec2::ZERO);
        directions.push(dir);
        previous = Some(*dab);
    }

    let fallback = directions
        .iter()
        .copied()
        .find(|dir| dir.length_squared() > 1e-8)
        .unwrap_or(Vec2::ZERO);
    for dir in &mut directions {
        if dir.length_squared() <= 1e-8 {
            *dir = fallback;
        }
    }
    directions
}

fn normalized_or_zero(v: Vec2) -> Vec2 {
    let len2 = v.length_squared();
    if len2 <= 1e-8 {
        Vec2::ZERO
    } else {
        v / len2.sqrt()
    }
}

fn surface_dab_directions(previous_dab: Option<SurfaceDab>, dabs: &[SurfaceDab]) -> Vec<Vec2> {
    let mut directions = Vec::with_capacity(dabs.len());
    let mut previous = previous_dab;
    for dab in dabs {
        let direction = previous
            .map(|previous| {
                let delta_world = dab.world_pos - previous.world_pos;
                let planar = delta_world - dab.world_normal * delta_world.dot(dab.world_normal);
                normalized_or_zero(Vec2::new(
                    planar.dot(dab.tangent_x),
                    planar.dot(dab.tangent_y),
                ))
            })
            .unwrap_or(Vec2::ZERO);
        directions.push(direction);
        previous = Some(*dab);
    }

    let fallback = directions
        .iter()
        .copied()
        .find(|direction| direction.length_squared() > 1e-8)
        .unwrap_or(Vec2::ZERO);
    for direction in &mut directions {
        if direction.length_squared() <= 1e-8 {
            *direction = fallback;
        }
    }
    directions
}

fn continued_surface_dab(
    history: &HashMap<SurfaceProjectionId, SurfaceDab>,
    projection_id: SurfaceProjectionId,
    continues_from_previous: bool,
) -> Option<SurfaceDab> {
    continues_from_previous
        .then(|| history.get(&projection_id).copied())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface_dab(world_pos: glam::Vec3, normal: glam::Vec3) -> SurfaceDab {
        SurfaceDab::with_scales(Vec2::ZERO, world_pos, normal, 0, 1.0, 1.0)
    }

    #[test]
    fn mirror_smudge_restart_does_not_bridge_direction_across_missing_segment() {
        let before_gap = StrokeDab::new(Vec2::new(10.0, 4.0), 1.0);
        let resumed = StrokeDab::new(Vec2::new(20.0, 4.0), 1.0);

        let incorrectly_continued = directional_instances(Some(before_gap), &[resumed]);
        assert_eq!(incorrectly_continued[0].dir, [1.0, 0.0]);

        let restarted = directional_instances(None, &[resumed]);
        assert_eq!(
            restarted[0].dir,
            [0.0, 0.0],
            "the first MirrorX dab after a missing segment must not smudge across the gap"
        );
    }

    #[test]
    fn mirror_smudge_direction_resumes_after_restart_anchor() {
        let restart_anchor = StrokeDab::new(Vec2::new(20.0, 4.0), 1.0);
        let next = StrokeDab::new(Vec2::new(16.0, 4.0), 1.0);

        let resumed = directional_instances(Some(restart_anchor), &[next]);
        assert_eq!(
            resumed[0].dir,
            [-1.0, 0.0],
            "the dab after the restart anchor must use the new MirrorX segment direction"
        );
    }

    #[test]
    fn surface_direction_projects_world_movement_into_current_tangent_basis() {
        let first = surface_dab(glam::Vec3::ZERO, glam::Vec3::Z);
        let second = surface_dab(glam::Vec3::new(2.0, 3.0, 5.0), glam::Vec3::Z);

        let directions = surface_dab_directions(Some(first), &[second]);

        let expected = Vec2::new(2.0, 3.0).normalize();
        assert!((directions[0] - expected).length() < 1e-6);
    }

    #[test]
    fn surface_direction_ignores_world_normal_movement() {
        let first = surface_dab(glam::Vec3::ZERO, glam::Vec3::Z);
        let second = surface_dab(glam::Vec3::new(0.0, 0.0, 5.0), glam::Vec3::Z);

        assert_eq!(surface_dab_directions(Some(first), &[second]), [Vec2::ZERO]);
    }

    #[test]
    fn surface_direction_backfills_first_dab_from_same_batch() {
        let first = surface_dab(glam::Vec3::ZERO, glam::Vec3::Z);
        let second = surface_dab(glam::Vec3::X, glam::Vec3::Z);

        assert_eq!(
            surface_dab_directions(None, &[first, second]),
            [Vec2::X, Vec2::X]
        );
    }

    #[test]
    fn surface_direction_restart_does_not_use_previous_batch() {
        let previous = surface_dab(glam::Vec3::ZERO, glam::Vec3::Z);
        let resumed = surface_dab(glam::Vec3::X, glam::Vec3::Z);

        assert_eq!(surface_dab_directions(None, &[resumed]), [Vec2::ZERO]);
        assert_eq!(
            surface_dab_directions(Some(previous), &[resumed]),
            [Vec2::X]
        );
    }

    #[test]
    fn surface_direction_history_is_independent_per_projection_and_obeys_restart() {
        let primary = surface_dab(glam::Vec3::X, glam::Vec3::Z);
        let mirror = surface_dab(-glam::Vec3::X, glam::Vec3::Z);
        let history = HashMap::from([
            (SurfaceProjectionId::Primary, primary),
            (SurfaceProjectionId::MirrorX, mirror),
        ]);

        assert_eq!(
            continued_surface_dab(&history, SurfaceProjectionId::Primary, true),
            Some(primary)
        );
        assert_eq!(
            continued_surface_dab(&history, SurfaceProjectionId::MirrorX, true),
            Some(mirror)
        );
        assert_eq!(
            continued_surface_dab(&history, SurfaceProjectionId::Primary, false),
            None
        );
    }
}
