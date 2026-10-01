use anyhow::{Result, anyhow, ensure};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        damage::{DamageMap, union_rect},
        geometry::RectU32,
        image::rgba8_len,
        render_report::RenderMetrics,
        selection::{ActiveSelection, SelectionMaskId},
        surface::PaintSurfaceId,
        transform::UvTransform,
    },
    renderer::{
        TransformCommand,
        document::{selection::SelectionMasks, surfaces::SurfaceRepository},
        engine::gpu_state::RendererGpuState,
        gpu::{copy_a_to_b, create_mask_texture, create_paint_texture, frame::GpuFrame},
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
    },
};

use super::pipelines::{TransformPipelines, TransformUniform};

pub(crate) struct TransformFeature {
    pipelines: TransformPipelines,
    active: Option<TransformGpuSession>,
}

pub(crate) struct TransformFeatureDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) surfaces: &'a mut SurfaceRepository,
    pub(crate) selections: &'a mut SelectionMasks,
}

struct OwnedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct TransformGpuSession {
    target: PaintSurfaceId,
    texture_size: [u32; 2],
    source_bounds: RectU32,
    active_selection: ActiveSelection,
    selection_mask_id: Option<SelectionMaskId>,
    base_surface: OwnedTexture,
    transform_source: Option<OwnedTexture>,
    source_selection: Option<OwnedTexture>,
    clear_original: bool,
    previous_result_damage: Option<DamageMap>,
}

impl TransformFeature {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: TransformPipelines::new(device),
            active: None,
        }
    }

    pub(crate) fn abort(&mut self) {
        self.active = None;
    }

    pub(crate) fn execute(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut TransformFeatureDeps<'_>,
        command: TransformCommand,
    ) -> Result<CommandResult> {
        match command {
            TransformCommand::Begin {
                target,
                source_bounds,
                active_selection,
            } => self.begin(frame, deps, target, source_bounds, active_selection),
            TransformCommand::BeginImported {
                target,
                source_size,
                source_rgba8,
            } => self.begin_imported(frame, deps, target, source_size, &source_rgba8),
            TransformCommand::Preview {
                transform,
                result_damage,
            } => self.apply(frame, deps, transform, result_damage, false),
            TransformCommand::Commit {
                transform,
                result_damage,
            } => self.apply(frame, deps, transform, result_damage, true),
            TransformCommand::Cancel => self.cancel(frame, deps),
            TransformCommand::Discard => Ok(self.discard(frame)),
        }
    }

    fn begin(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut TransformFeatureDeps<'_>,
        target: PaintSurfaceId,
        source_bounds: RectU32,
        active_selection: ActiveSelection,
    ) -> Result<CommandResult> {
        ensure!(
            self.active.is_none(),
            "a Transform session is already active"
        );

        let mut metrics = RenderMetrics::default();
        let prepare = deps
            .surfaces
            .prepare_edit_surface_for_frame(deps.gpu, frame, target)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let edit_target = deps
            .surfaces
            .edit_surface_target(target)
            .ok_or_else(|| anyhow!("Transform target is not GPU resident: {target:?}"))?;
        ensure!(
            source_bounds.size[0] > 0 && source_bounds.size[1] > 0,
            "Transform source bounds are empty"
        );

        let base_surface = copy_paint_texture(
            frame,
            deps.gpu.device(),
            edit_target.texture,
            edit_target.texture_size,
            "transform_base_surface",
        );

        let material_index = target.material_index().as_usize();
        let selection_mask_id = active_selection
            .enabled
            .then(|| active_selection.material_mask(material_index.into()))
            .flatten()
            .and_then(|mask| mask.mask_id);
        let source_selection = if let Some(mask_id) = selection_mask_id {
            let _material_mask = active_selection
                .material_mask(material_index.into())
                .ok_or_else(|| anyhow!("Transform selection material mask is missing"))?;
            {
                let selection = deps.selections.ensure_texture(
                    deps.gpu.device(),
                    mask_id,
                    material_index,
                    edit_target.texture_size,
                );
                if !selection.is_initialized() {
                    selection.upload_full_into_frame(deps.gpu.device(), frame);
                }
            }
            let selection_texture = deps
                .selections
                .texture(mask_id)
                .ok_or_else(|| anyhow!("Transform selection texture does not exist"))?;
            let (texture, view) = create_mask_texture(
                deps.gpu.device(),
                edit_target.texture_size,
                "transform_source_selection",
            );
            copy_a_to_b(
                frame.encoder(),
                selection_texture,
                &texture,
                edit_target.texture_size,
            );
            Some(OwnedTexture { texture, view })
        } else {
            None
        };

        self.active = Some(TransformGpuSession {
            target,
            texture_size: edit_target.texture_size,
            source_bounds,
            active_selection,
            selection_mask_id,
            base_surface,
            transform_source: None,
            source_selection,
            clear_original: true,
            previous_result_damage: None,
        });
        Ok(CommandResult::new(MutationLog::default(), metrics))
    }

    fn begin_imported(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut TransformFeatureDeps<'_>,
        target: PaintSurfaceId,
        source_size: [u32; 2],
        source_rgba8: &[u8],
    ) -> Result<CommandResult> {
        ensure!(
            self.active.is_none(),
            "a Transform session is already active"
        );
        ensure!(
            source_size[0] > 0 && source_size[1] > 0,
            "imported image is empty"
        );
        ensure!(
            source_rgba8.len() == rgba8_len(source_size)?,
            "imported image pixel length mismatch"
        );

        let mut metrics = RenderMetrics::default();
        let prepare = deps
            .surfaces
            .prepare_edit_surface_for_frame(deps.gpu, frame, target)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let edit_target = deps
            .surfaces
            .edit_surface_target(target)
            .ok_or_else(|| anyhow!("Transform target is not GPU resident: {target:?}"))?;
        let base_surface = copy_paint_texture(
            frame,
            deps.gpu.device(),
            edit_target.texture,
            edit_target.texture_size,
            "transform_import_base_surface",
        );
        let (texture, view) =
            create_paint_texture(deps.gpu.device(), source_size, "transform_import_source");
        frame.write_texture_rgba8(
            deps.gpu.device(),
            &texture,
            [0, 0],
            source_size,
            source_rgba8,
        );
        let active_selection = ActiveSelection::disabled_for_materials([target.material_index()]);
        self.active = Some(TransformGpuSession {
            target,
            texture_size: edit_target.texture_size,
            source_bounds: RectU32::full(source_size),
            active_selection,
            selection_mask_id: None,
            base_surface,
            transform_source: Some(OwnedTexture { texture, view }),
            source_selection: None,
            clear_original: false,
            previous_result_damage: None,
        });
        Ok(CommandResult::new(MutationLog::default(), metrics))
    }

    fn apply(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut TransformFeatureDeps<'_>,
        transform: UvTransform,
        result_damage: DamageMap,
        commit: bool,
    ) -> Result<CommandResult> {
        let session = self
            .active
            .as_ref()
            .ok_or_else(|| anyhow!("Transform preview has no active session"))?;
        let damage_rect =
            transform_damage_rect(session.target, session.texture_size, &result_damage)?;
        let transition_damage =
            union_damage_maps(session.previous_result_damage.as_ref(), &result_damage);
        let uniform = transform_uniform(session, transform)?;
        let mut metrics = RenderMetrics::default();

        let prepare =
            deps.surfaces
                .prepare_edit_surface_for_frame(deps.gpu, frame, session.target)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let edit_target = deps
            .surfaces
            .edit_surface_target(session.target)
            .ok_or_else(|| anyhow!("Transform target is not GPU resident: {:?}", session.target))?;
        ensure!(
            edit_target.texture_size == session.texture_size,
            "Transform target texture size changed during the session"
        );

        copy_a_to_b(
            frame.encoder(),
            &session.base_surface.texture,
            edit_target.texture,
            session.texture_size,
        );
        self.pipelines
            .write_uniform(frame, deps.gpu.device(), &uniform);
        let transform_source_view = session
            .transform_source
            .as_ref()
            .map_or(&session.base_surface.view, |source| &source.view);
        let source_selection_view = session
            .source_selection
            .as_ref()
            .map_or(&session.base_surface.view, |selection| &selection.view);
        let bind_group = self.pipelines.bind_group(
            deps.gpu.device(),
            &session.base_surface.view,
            transform_source_view,
            source_selection_view,
        );
        if let Some(damage_rect) = damage_rect {
            self.pipelines
                .record_surface_pass(frame, &bind_group, edit_target.view, damage_rect);
        }

        if session.target.is_mask() {
            deps.surfaces
                .resolve_mask_edit_proxy_into_frame(deps.gpu, frame, session.target)?;
        }
        if let (Some(mask_id), Some(source_selection), Some(damage_rect)) = (
            session.selection_mask_id,
            session.source_selection.as_ref(),
            damage_rect,
        ) {
            let target_selection = deps
                .selections
                .texture(mask_id)
                .ok_or_else(|| anyhow!("Transform selection texture disappeared"))?;
            copy_a_to_b(
                frame.encoder(),
                &source_selection.texture,
                target_selection,
                session.texture_size,
            );
            let target_selection = deps
                .selections
                .texture_mut(mask_id)
                .ok_or_else(|| anyhow!("Transform selection texture disappeared"))?;
            self.pipelines.record_selection_pass(
                frame,
                &bind_group,
                target_selection.view(),
                damage_rect,
            );
            target_selection.mark_gpu_written();
        }

        let mut mutations = MutationLog::default();
        if commit {
            let stale = deps
                .surfaces
                .mark_surface_cache_stale_damage(Some(&result_damage), session.target);
            record_surface_prepare_metrics(&mut metrics, &stale);
            mutations.surfaces.rects(session.target, transition_damage);
            mutations
                .surface_commits
                .rects(session.target, result_damage.clone());
        } else {
            mutations
                .surfaces
                .transient_damage(session.target, transition_damage);
        }
        if session.selection_mask_id.is_some() {
            mutations
                .selections
                .mask_changed(session.active_selection.clone());
        }

        if commit {
            let session = self
                .active
                .take()
                .expect("active Transform session checked above");
            retain_session_textures(frame, session);
        } else {
            self.active
                .as_mut()
                .expect("active Transform session checked above")
                .previous_result_damage = Some(result_damage);
        }
        Ok(CommandResult::new(mutations, metrics))
    }

    fn discard(&mut self, frame: &mut GpuFrame) -> CommandResult {
        let Some(session) = self.active.take() else {
            return CommandResult::default();
        };
        retain_session_textures(frame, session);
        CommandResult::default()
    }

    fn cancel(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut TransformFeatureDeps<'_>,
    ) -> Result<CommandResult> {
        let Some(session) = self.active.take() else {
            return Ok(CommandResult::default());
        };
        let mut metrics = RenderMetrics::default();
        let prepare =
            deps.surfaces
                .prepare_edit_surface_for_frame(deps.gpu, frame, session.target)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let edit_target = deps
            .surfaces
            .edit_surface_target(session.target)
            .ok_or_else(|| anyhow!("Transform target is not GPU resident: {:?}", session.target))?;
        copy_a_to_b(
            frame.encoder(),
            &session.base_surface.texture,
            edit_target.texture,
            session.texture_size,
        );
        if session.target.is_mask() {
            deps.surfaces
                .resolve_mask_edit_proxy_into_frame(deps.gpu, frame, session.target)?;
        }
        if let (Some(mask_id), Some(source_selection)) =
            (session.selection_mask_id, session.source_selection.as_ref())
        {
            let selection = deps
                .selections
                .texture(mask_id)
                .ok_or_else(|| anyhow!("Transform selection texture disappeared"))?;
            copy_a_to_b(
                frame.encoder(),
                &source_selection.texture,
                selection,
                session.texture_size,
            );
            deps.selections
                .texture_mut(mask_id)
                .expect("Transform selection checked above")
                .mark_gpu_written();
        }

        let mut mutations = MutationLog::default();
        mutations.surfaces.cancelled(session.target);
        if session.selection_mask_id.is_some() {
            mutations
                .selections
                .mask_changed(session.active_selection.clone());
        }
        retain_session_textures(frame, session);
        Ok(CommandResult::new(mutations, metrics))
    }
}

fn copy_paint_texture(
    frame: &mut GpuFrame,
    device: &wgpu::Device,
    source: &wgpu::Texture,
    size: [u32; 2],
    label: &'static str,
) -> OwnedTexture {
    let (texture, view) = create_paint_texture(device, size, label);
    copy_a_to_b(frame.encoder(), source, &texture, size);
    OwnedTexture { texture, view }
}

fn transform_damage_rect(
    target: PaintSurfaceId,
    _texture_size: [u32; 2],
    damage: &DamageMap,
) -> Result<Option<RectU32>> {
    let mut rect = None;
    for pixel in &damage.pixels {
        ensure!(
            pixel.surface == target,
            "Transform damage references a different surface"
        );
        rect = Some(match rect {
            Some(current) => union_rect(current, pixel.rect),
            None => pixel.rect,
        });
    }
    Ok(rect)
}

fn union_damage_maps(previous: Option<&DamageMap>, current: &DamageMap) -> DamageMap {
    let mut union = previous.cloned().unwrap_or_default();
    for damage in &current.pixels {
        union.add_rect(damage.surface, damage.rect);
    }
    union
}

fn transform_uniform(
    session: &TransformGpuSession,
    transform: UvTransform,
) -> Result<TransformUniform> {
    let inverse = transform
        .inverse_matrix()
        .ok_or_else(|| anyhow!("Transform matrix is not invertible"))?;
    let source_max = [
        session.source_bounds.origin[0] as f32 + session.source_bounds.size[0] as f32,
        session.source_bounds.origin[1] as f32 + session.source_bounds.size[1] as f32,
    ];
    Ok(TransformUniform {
        inverse_row0: [inverse.x_axis.x, inverse.y_axis.x, inverse.z_axis.x, 0.0],
        inverse_row1: [inverse.x_axis.y, inverse.y_axis.y, inverse.z_axis.y, 0.0],
        source_min: [
            session.source_bounds.origin[0] as f32,
            session.source_bounds.origin[1] as f32,
        ],
        source_max,
        selection_enabled: if session.selection_mask_id.is_some() {
            1
        } else {
            0
        },
        clear_original: if session.clear_original { 1 } else { 0 },
        _pad0: [0; 2],
    })
}

fn retain_session_textures(frame: &mut GpuFrame, session: TransformGpuSession) {
    frame.retain_texture_until_submit(session.base_surface.texture, session.base_surface.view);
    if let Some(source) = session.transform_source {
        frame.retain_texture_until_submit(source.texture, source.view);
    }
    if let Some(selection) = session.source_selection {
        frame.retain_texture_until_submit(selection.texture, selection.view);
    }
}

#[cfg(test)]
mod tests {
    use slotmap::SlotMap;

    use super::*;

    fn target() -> PaintSurfaceId {
        let mut layers = SlotMap::with_key();
        PaintSurfaceId::raster(0.into(), layers.insert(()))
    }

    #[test]
    fn transform_damage_rect_unions_damage_for_the_active_surface() {
        let target = target();
        let mut damage = DamageMap::default();
        damage.add_rect(
            target,
            RectU32 {
                origin: [1, 2],
                size: [3, 4],
            },
        );
        damage.add_rect(
            target,
            RectU32 {
                origin: [8, 9],
                size: [2, 3],
            },
        );
        let rect = transform_damage_rect(target, [16, 16], &damage)
            .unwrap()
            .unwrap();
        assert_eq!(rect.origin, [1, 2]);
        assert_eq!(rect.size, [9, 10]);
    }

    #[test]
    fn transition_damage_keeps_previous_and_current_preview_regions() {
        let target = target();
        let mut previous = DamageMap::default();
        previous.add_rect(
            target,
            RectU32 {
                origin: [1, 1],
                size: [4, 4],
            },
        );
        let mut current = DamageMap::default();
        current.add_rect(
            target,
            RectU32 {
                origin: [20, 20],
                size: [4, 4],
            },
        );

        let transition = union_damage_maps(Some(&previous), &current);

        assert_eq!(transition.pixels.len(), 2);
        assert!(
            transition
                .pixels
                .iter()
                .any(|damage| damage.rect.origin == [1, 1])
        );
        assert!(
            transition
                .pixels
                .iter()
                .any(|damage| damage.rect.origin == [20, 20])
        );
    }
}
