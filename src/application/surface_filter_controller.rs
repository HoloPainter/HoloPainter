use anyhow::{Result, ensure};

use crate::{
    core::{
        adjustment::{Adjustment, AdjustmentKind},
        document::ActiveLayerTarget,
        surface::{LayerId, PaintSurfaceId, PaintSurfaceRole},
        surface_filter::{SpatialBlurOrientation, SurfaceFilterDomain},
    },
    renderer::{EditCommand, FilterCommand, SpatialBlurParams, SurfaceBlurParams},
};

use super::{
    PaintEditBlockReason, PendingRendererHistoryTransaction, ReducerOutput, RendererCommitSpec,
    StatusMessage,
    history_capture::pixel_history_capture_for_full_surfaces,
    state::{AdjustmentFilterSession, AppState, PaintTargetScope},
};

pub(crate) const SURFACE_BLUR_MIN_RADIUS_PX: f32 = 0.5;
pub(crate) const SURFACE_BLUR_MAX_RADIUS_PX: f32 = 16.0;
pub(crate) const SPATIAL_BLUR_MIN_RADIUS_PX: f32 = 0.5;
pub(crate) const SPATIAL_BLUR_MAX_RADIUS_PX: f32 = 64.0;
pub(crate) const SPATIAL_BLUR_MIN_ANGLE_DEGREES: f32 = 0.0;
pub(crate) const SPATIAL_BLUR_MAX_ANGLE_DEGREES: f32 = 180.0;

struct PreparedSurfaceFilter {
    surfaces: Vec<PaintSurfaceId>,
    role: PaintSurfaceRole,
}

#[derive(Clone)]
struct PreparedBlurFilter {
    source_surfaces: Vec<PaintSurfaceId>,
    affected_surfaces: Vec<PaintSurfaceId>,
    active_selection: crate::core::selection::ActiveSelection,
}

impl AppState {
    pub(crate) fn adjustment_filter_available(&self, kind: AdjustmentKind) -> bool {
        active_filter_role(self.active_layer_target()).is_some_and(|role| {
            kind.supports_destructive_filter(role.into())
                && self
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                    .is_allowed()
        })
    }

    pub(crate) fn blur_filter_available(&self) -> bool {
        active_filter_role(self.active_layer_target()).is_some()
            && self
                .document
                .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                .is_allowed()
    }
}

pub(crate) fn apply_surface_blur(state: &mut AppState, radius_px: f32) -> Result<ReducerOutput> {
    ensure_reference_radius(
        "Surface Blur",
        radius_px,
        SURFACE_BLUR_MIN_RADIUS_PX,
        SURFACE_BLUR_MAX_RADIUS_PX,
    )?;
    let Some(prepared) = prepare_blur_filter(state, "Surface Blur")? else {
        return Ok(ReducerOutput::default());
    };
    let radius_world =
        resolve_radius_world(state, "Surface Blur", &prepared.source_surfaces, radius_px)?;
    let history_transaction = capture_filter_history(state, &prepared.affected_surfaces)?;
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction("Surface Blur", history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            prepared.affected_surfaces.clone(),
        ));
    output.push_edit_command(EditCommand::Filter(FilterCommand::SurfaceBlur {
        source_surfaces: prepared.source_surfaces,
        affected_surfaces: prepared.affected_surfaces,
        params: SurfaceBlurParams { radius_world },
        active_selection: prepared.active_selection,
    }));
    state.set_status_message(
        StatusMessage::localized("status-surface-blur-applied").arg("radius", radius_px),
    );
    Ok(output)
}

pub(crate) fn apply_spatial_blur(
    state: &mut AppState,
    radius_px: f32,
    cross_meshes: bool,
    orientation: SpatialBlurOrientation,
    maximum_normal_angle_degrees: f32,
) -> Result<ReducerOutput> {
    ensure!(
        maximum_normal_angle_degrees.is_finite()
            && (SPATIAL_BLUR_MIN_ANGLE_DEGREES..=SPATIAL_BLUR_MAX_ANGLE_DEGREES)
                .contains(&maximum_normal_angle_degrees),
        "spatial blur maximum normal angle must be between {SPATIAL_BLUR_MIN_ANGLE_DEGREES} and {SPATIAL_BLUR_MAX_ANGLE_DEGREES} degrees"
    );
    ensure_reference_radius(
        "Spatial Blur",
        radius_px,
        SPATIAL_BLUR_MIN_RADIUS_PX,
        SPATIAL_BLUR_MAX_RADIUS_PX,
    )?;
    let Some(prepared) = prepare_blur_filter(state, "Spatial Blur")? else {
        return Ok(ReducerOutput::default());
    };
    let radius_world =
        resolve_radius_world(state, "Spatial Blur", &prepared.source_surfaces, radius_px)?;
    let history_transaction = capture_filter_history(state, &prepared.affected_surfaces)?;
    let normal_threshold_cos = maximum_normal_angle_degrees.to_radians().cos();
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction("Spatial Blur", history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            prepared.affected_surfaces.clone(),
        ));
    output.push_edit_command(EditCommand::Filter(FilterCommand::SpatialBlur {
        source_surfaces: prepared.source_surfaces,
        affected_surfaces: prepared.affected_surfaces,
        params: SpatialBlurParams {
            radius_world,
            reference_radius_px: radius_px,
            cross_meshes,
            orientation,
            normal_threshold_cos,
        },
        active_selection: prepared.active_selection,
    }));
    let status = match orientation {
        SpatialBlurOrientation::Ignore if cross_meshes => {
            StatusMessage::localized("status-spatial-blur-applied-all-meshes")
                .arg("radius", radius_px)
        }
        SpatialBlurOrientation::Ignore => {
            StatusMessage::localized("status-spatial-blur-applied-same-mesh")
                .arg("radius", radius_px)
        }
        SpatialBlurOrientation::SimilarNormals => {
            StatusMessage::localized("status-spatial-blur-applied-similar-normals")
                .arg("radius", radius_px)
                .arg("angle", maximum_normal_angle_degrees)
        }
    };
    state.set_status_message(status);
    Ok(output)
}

pub(crate) fn apply_adjustment_filter(
    state: &mut AppState,
    layer_id: LayerId,
    adjustment: Adjustment,
) -> Result<ReducerOutput> {
    let filter_name = adjustment_history_label(adjustment.kind());
    let Some(prepared) = prepare_adjustment_filter(state, filter_name, layer_id)? else {
        return Ok(ReducerOutput::default());
    };
    let adjustment = normalize_destructive_adjustment(adjustment, prepared.role.into())?;
    if adjustment.is_identity() {
        return Ok(ReducerOutput::default());
    }
    build_adjustment_filter_commit(
        state,
        filter_name,
        prepared.surfaces,
        adjustment,
        prepared.active_selection,
    )
}

pub(crate) fn begin_adjustment_filter_session(
    state: &mut AppState,
    layer_id: LayerId,
    kind: AdjustmentKind,
) -> Result<ReducerOutput> {
    ensure!(
        state.adjustment_filter_session.is_none(),
        "an adjustment filter preview session is already active"
    );
    ensure!(
        !state.is_tool_interacting(),
        "finish or cancel the current tool operation before previewing a filter"
    );

    let Some(prepared) = prepare_adjustment_filter(state, kind.id(), layer_id)? else {
        return Ok(ReducerOutput::default());
    };
    ensure!(
        kind.supports_destructive_filter_preview(prepared.role.into()),
        "{} does not support destructive filter preview for the selected target",
        kind.id()
    );
    state.adjustment_filter_session = Some(AdjustmentFilterSession {
        document_generation: state.document_generation(),
        layer_id,
        kind,
        target_role: prepared.role,
        surfaces: prepared.surfaces,
        active_selection: prepared.active_selection,
        renderer_preview_started: false,
    });
    Ok(ReducerOutput::default())
}

pub(crate) fn preview_adjustment_filter(
    state: &mut AppState,
    layer_id: LayerId,
    adjustment: Option<Adjustment>,
) -> Result<ReducerOutput> {
    let session = adjustment_filter_session(state, layer_id)?;
    let normalized = adjustment
        .map(|adjustment| normalize_preview_adjustment(adjustment, session.target_role.into()))
        .transpose()?;
    if let Some(ref adjustment) = normalized {
        ensure!(
            adjustment.kind() == session.kind,
            "adjustment filter preview kind changed during the session"
        );
    }
    let effective = normalized.filter(|adjustment| !adjustment.is_identity());

    if !session.renderer_preview_started {
        let Some(adjustment) = effective else {
            return Ok(ReducerOutput::default());
        };
        let surfaces = session.surfaces.clone();
        let active_selection = session.active_selection.clone();
        let material_sizes = state
            .document()
            .expect("adjustment filter preview session requires a document")
            .materials
            .iter()
            .map(|material| material.texture_size)
            .collect::<Vec<_>>();
        crate::renderer::features::filter::validate_adjustment_preview_working_set(
            &material_sizes,
            &surfaces,
        )?;
        state
            .adjustment_filter_session
            .as_mut()
            .expect("adjustment filter session checked above")
            .renderer_preview_started = true;
        let mut output = ReducerOutput::default();
        output.push_edit_command(EditCommand::Filter(FilterCommand::AdjustmentPreviewBegin {
            surfaces,
            adjustment,
            active_selection,
        }));
        return Ok(output);
    }

    let mut output = ReducerOutput::default();
    output.push_edit_command(EditCommand::Filter(
        FilterCommand::AdjustmentPreviewUpdate {
            adjustment: effective,
        },
    ));
    Ok(output)
}

pub(crate) fn commit_adjustment_filter_session(
    state: &mut AppState,
    layer_id: LayerId,
    adjustment: Adjustment,
) -> Result<ReducerOutput> {
    let session = adjustment_filter_session(state, layer_id)?.clone();
    let adjustment = normalize_preview_adjustment(adjustment, session.target_role.into())?;
    ensure!(
        !adjustment.is_identity(),
        "{} adjustment filter must not be an identity operation",
        adjustment.kind().id()
    );
    ensure!(
        adjustment.kind() == session.kind,
        "adjustment filter commit kind changed during the session"
    );

    let filter_name = adjustment_history_label(session.kind);
    let history_transaction = capture_filter_history(state, &session.surfaces)?;
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction(filter_name, history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            session.surfaces.clone(),
        ));
    if session.renderer_preview_started {
        output.push_edit_command(EditCommand::Filter(
            FilterCommand::AdjustmentPreviewCommit { adjustment },
        ));
    } else {
        output.push_edit_command(EditCommand::Filter(FilterCommand::Adjustment {
            surfaces: session.surfaces,
            adjustment,
            active_selection: session.active_selection,
        }));
    }
    state.adjustment_filter_session = None;
    state.set_status_message(adjustment_applied_status(session.kind));
    Ok(output)
}

pub(crate) fn cancel_adjustment_filter_session(
    state: &mut AppState,
    layer_id: LayerId,
) -> Result<ReducerOutput> {
    let session = adjustment_filter_session(state, layer_id)?.clone();
    state.adjustment_filter_session = None;
    if !session.renderer_preview_started {
        return Ok(ReducerOutput::default());
    }
    let mut output = ReducerOutput::default();
    output.push_edit_command(EditCommand::Filter(FilterCommand::AdjustmentPreviewCancel));
    Ok(output)
}

#[derive(Clone)]
struct PreparedAdjustmentFilter {
    surfaces: Vec<PaintSurfaceId>,
    role: PaintSurfaceRole,
    active_selection: crate::core::selection::ActiveSelection,
}

fn prepare_adjustment_filter(
    state: &mut AppState,
    filter_name: &str,
    layer_id: LayerId,
) -> Result<Option<PreparedAdjustmentFilter>> {
    let Some(mut prepared) = prepare_surface_filter(state, filter_name, Some(layer_id))? else {
        return Ok(None);
    };
    let active_selection = state
        .document()
        .expect("prepared raster filter requires a document")
        .active_selection
        .clone();
    if active_selection.enabled {
        prepared.surfaces.retain(|surface| {
            active_selection
                .material_mask(surface.material_index())
                .and_then(|mask| mask.mask_id)
                .is_some()
        });
        if prepared.surfaces.is_empty() {
            state.set_status_key("status-filter-selection-no-surface");
            return Ok(None);
        }
    }
    Ok(Some(PreparedAdjustmentFilter {
        surfaces: prepared.surfaces,
        role: prepared.role,
        active_selection,
    }))
}

fn prepare_blur_filter(
    state: &mut AppState,
    filter_name: &str,
) -> Result<Option<PreparedBlurFilter>> {
    let Some(prepared) = prepare_surface_filter(state, filter_name, None)? else {
        return Ok(None);
    };
    let active_selection = state
        .document()
        .expect("prepared raster filter requires a document")
        .active_selection
        .clone();
    let source_surfaces = prepared.surfaces;
    let affected_surfaces = if active_selection.enabled {
        source_surfaces
            .iter()
            .copied()
            .filter(|surface| {
                active_selection
                    .material_mask(surface.material_index())
                    .and_then(|mask| mask.mask_id)
                    .is_some()
            })
            .collect()
    } else {
        source_surfaces.clone()
    };
    if affected_surfaces.is_empty() {
        state.set_status_key(match filter_name {
            "Surface Blur" => "status-surface-blur-selection-no-surface",
            "Spatial Blur" => "status-spatial-blur-selection-no-surface",
            _ => "status-filter-selection-no-surface",
        });
        return Ok(None);
    }
    Ok(Some(PreparedBlurFilter {
        source_surfaces,
        affected_surfaces,
        active_selection,
    }))
}

fn build_adjustment_filter_commit(
    state: &mut AppState,
    filter_name: &str,
    surfaces: Vec<PaintSurfaceId>,
    adjustment: Adjustment,
    active_selection: crate::core::selection::ActiveSelection,
) -> Result<ReducerOutput> {
    let adjustment_kind = adjustment.kind();
    let history_transaction = capture_filter_history(state, &surfaces)?;
    let mut output = ReducerOutput::default()
        .with_optional_renderer_history_transaction(filter_name, history_transaction)
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(
            surfaces.clone(),
        ));
    output.push_edit_command(EditCommand::Filter(FilterCommand::Adjustment {
        surfaces,
        adjustment,
        active_selection,
    }));
    state.set_status_message(adjustment_applied_status(adjustment_kind));
    Ok(output)
}

fn normalize_destructive_adjustment(
    adjustment: Adjustment,
    domain: SurfaceFilterDomain,
) -> Result<Adjustment> {
    let kind = adjustment.kind();
    ensure!(
        kind.supports_destructive_filter(domain),
        "{} is not available as a destructive filter for the selected target",
        kind.id()
    );
    adjustment
        .try_normalized_for_domain(domain)
        .ok_or_else(|| anyhow::anyhow!("adjustment filter parameters must be finite"))
}

fn normalize_preview_adjustment(
    adjustment: Adjustment,
    domain: SurfaceFilterDomain,
) -> Result<Adjustment> {
    let adjustment = normalize_destructive_adjustment(adjustment, domain)?;
    ensure!(
        adjustment
            .kind()
            .supports_destructive_filter_preview(domain),
        "{} does not support destructive filter preview",
        adjustment.kind().id()
    );
    Ok(adjustment)
}

fn adjustment_filter_session(
    state: &AppState,
    layer_id: LayerId,
) -> Result<&AdjustmentFilterSession> {
    let session = state
        .adjustment_filter_session
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no adjustment filter preview session is active"))?;
    ensure!(
        session.document_generation == state.document_generation(),
        "adjustment filter preview document changed during the session"
    );
    ensure!(
        session.layer_id == layer_id && state.active_layer_id() == layer_id,
        "adjustment filter preview target changed during the session"
    );
    ensure!(
        active_filter_role(state.active_layer_target()) == Some(session.target_role),
        "adjustment filter preview target role changed during the session"
    );
    Ok(session)
}

fn ensure_reference_radius(
    filter_name: &str,
    radius_px: f32,
    minimum_radius_px: f32,
    maximum_radius_px: f32,
) -> Result<()> {
    ensure!(
        radius_px.is_finite() && (minimum_radius_px..=maximum_radius_px).contains(&radius_px),
        "{} reference radius must be between {} and {} average texels",
        filter_name.to_lowercase(),
        minimum_radius_px,
        maximum_radius_px
    );
    Ok(())
}

fn prepare_surface_filter(
    state: &mut AppState,
    filter_name: &str,
    expected_layer_id: Option<LayerId>,
) -> Result<Option<PreparedSurfaceFilter>> {
    let Some(role) = active_filter_role(state.active_layer_target()) else {
        state.set_status_key("status-filter-select-editable-target");
        return Ok(None);
    };
    if expected_layer_id.is_some_and(|layer_id| state.active_layer_id() != layer_id) {
        state.set_status_key("status-filter-select-editable-target");
        return Ok(None);
    }

    let paint_edit = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::AllMaterials);
    let blocked_reason = paint_edit.blocked_reason();
    let Some(paint_target) = paint_edit.into_allowed() else {
        set_surface_filter_unavailable_status(state, filter_name, blocked_reason);
        return Ok(None);
    };
    Ok(Some(PreparedSurfaceFilter {
        surfaces: paint_target.surfaces_vec(),
        role,
    }))
}

fn active_filter_role(target: ActiveLayerTarget) -> Option<PaintSurfaceRole> {
    match target {
        ActiveLayerTarget::Raster => Some(PaintSurfaceRole::Raster),
        ActiveLayerTarget::LayerMask => Some(PaintSurfaceRole::LayerMask),
        ActiveLayerTarget::EmbeddedImage
        | ActiveLayerTarget::SolidFill
        | ActiveLayerTarget::Adjustment
        | ActiveLayerTarget::Structure => None,
    }
}

fn resolve_radius_world(
    state: &AppState,
    filter_name: &str,
    surfaces: &[PaintSurfaceId],
    radius_px: f32,
) -> Result<f32> {
    let Some(document) = state.document() else {
        anyhow::bail!("{} requires a loaded document", filter_name.to_lowercase());
    };
    let material_sizes = document
        .materials
        .iter()
        .map(|material| material.texture_size)
        .collect::<Vec<_>>();
    let mut enabled_materials = vec![false; material_sizes.len()];
    for surface in surfaces {
        if let Some(enabled) = enabled_materials.get_mut(surface.material_index().as_usize()) {
            *enabled = true;
        }
    }
    let texel_density = document
        .mesh
        .surface_filter_reference_texel_density(&material_sizes, &enabled_materials);
    let radius_world = radius_px / texel_density;
    ensure!(
        radius_world.is_finite() && radius_world > 0.0,
        "{} could not resolve a valid world-space radius",
        filter_name.to_lowercase()
    );
    Ok(radius_world)
}

fn capture_filter_history(
    state: &mut AppState,
    surfaces: &[PaintSurfaceId],
) -> Result<Option<PendingRendererHistoryTransaction>> {
    pixel_history_capture_for_full_surfaces(state, surfaces)
        .map(|capture| capture.capture(state))
        .transpose()
        .map(Option::flatten)
}

fn set_surface_filter_unavailable_status(
    state: &mut AppState,
    _filter_name: &str,
    reason: Option<PaintEditBlockReason>,
) {
    let key = match reason {
        Some(PaintEditBlockReason::MaterialExcluded) => "status-filter-material-excluded",
        Some(PaintEditBlockReason::ActiveLayerLocked) => "status-filter-layer-locked",
        Some(PaintEditBlockReason::ActiveLayerHidden) => "status-filter-layer-hidden",
        Some(PaintEditBlockReason::ActiveLayerIsGroup)
        | Some(PaintEditBlockReason::ActiveTargetNotEditable) => {
            "status-filter-select-editable-target"
        }
        None => "status-filter-load-document",
    };
    state.set_status_key(key);
}

fn adjustment_applied_status(kind: AdjustmentKind) -> StatusMessage {
    let key = match kind {
        AdjustmentKind::BrightnessContrast => "status-filter-applied-brightness-contrast",
        AdjustmentKind::HueSaturation => "status-filter-applied-hsl",
        AdjustmentKind::Invert => "status-filter-applied-invert",
        AdjustmentKind::Levels => "status-filter-applied-levels",
        AdjustmentKind::Curves => "status-filter-applied-curves",
        AdjustmentKind::GradientMap => "status-filter-applied-gradient-map",
        AdjustmentKind::UvMirror => "status-filter-applied-uv-mirror",
    };
    StatusMessage::localized(key)
}

fn adjustment_history_label(kind: AdjustmentKind) -> &'static str {
    match kind {
        AdjustmentKind::BrightnessContrast => "Brightness / Contrast",
        AdjustmentKind::Levels => "Levels",
        AdjustmentKind::Curves => "Curves",
        AdjustmentKind::HueSaturation => "Hue / Saturation",
        AdjustmentKind::Invert => "Invert",
        AdjustmentKind::GradientMap => "Gradient Map",
        AdjustmentKind::UvMirror => "UV Mirror",
    }
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use crate::{
        application::{Command, reduce, state::EditorDocumentState},
        core::{
            adjustment::{
                Adjustment, AdjustmentKind, BrightnessContrastAdjustment, GradientMapAdjustment,
            },
            document::{
                ActiveLayerPart, Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh,
            },
            surface::{LayerMaterialMask, PaintSurfaceId, PaintSurfaceRole},
            surface_filter::SpatialBlurOrientation,
        },
        renderer::{EditCommand, FilterCommand},
    };

    use super::AppState;

    #[test]
    fn surface_blur_targets_allowed_materials_and_converts_pixels_to_world_radius() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [16, 16]),
        ]);
        let layer_id = state.active_layer_id();
        let material_a = state.document().unwrap().materials[0].id;
        assert!(
            state
                .document
                .document
                .as_mut()
                .unwrap()
                .layer_tree
                .set_layer_material_mask(
                    layer_id,
                    LayerMaterialMask::Specified([material_a].into_iter().collect()),
                )
        );

        let plan = reduce(&mut state, Command::ApplySurfaceBlur { radius_px: 4.0 });

        let expected = PaintSurfaceId::raster(0.into(), layer_id);
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::SurfaceBlur {
                source_surfaces,
                affected_surfaces,
                params,
                active_selection,
            })]
                if source_surfaces == &[expected]
                    && affected_surfaces == source_surfaces
                    && !active_selection.enabled
                    && (params.radius_world - 0.5).abs() < 1.0e-5
        ));
        assert_eq!(
            plan.commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.as_slice()),
            Some([expected].as_slice())
        );
    }

    #[test]
    fn adjustment_filter_preview_session_lazily_starts_renderer_preview() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();
        let kind = crate::core::adjustment::AdjustmentKind::BrightnessContrast;

        let begin = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession { layer_id, kind },
        );
        assert!(begin.is_empty());
        assert!(state.adjustment_filter_session_matches(layer_id, kind));
        assert!(state.is_document_edit_interacting());

        let identity = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: None,
            },
        );
        assert!(identity.is_empty());
        assert!(
            !state
                .adjustment_filter_session
                .as_ref()
                .unwrap()
                .renderer_preview_started
        );

        let adjustment = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 20,
            contrast: 0,
        });
        let first = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: Some(adjustment.clone()),
            },
        );
        assert!(matches!(
            first.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::AdjustmentPreviewBegin {
                adjustment: actual,
                ..
            })] if *actual == adjustment
        ));
        assert!(first.commit_request.is_none());
        assert!(
            state
                .adjustment_filter_session
                .as_ref()
                .unwrap()
                .renderer_preview_started
        );

        let second = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: None,
            },
        );
        assert!(matches!(
            second.edit_commands.as_slice(),
            [EditCommand::Filter(
                FilterCommand::AdjustmentPreviewUpdate { adjustment: None }
            )]
        ));
        assert!(second.commit_request.is_none());
    }

    #[test]
    fn adjustment_filter_preview_rejects_oversized_gpu_working_set_before_renderer_work() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8192, 8192]),
            MaterialSpec::new("B", [8192, 8192]),
        ]);
        let layer_id = state.active_layer_id();
        let kind = crate::core::adjustment::AdjustmentKind::BrightnessContrast;
        let _ = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession { layer_id, kind },
        );

        let plan = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: Some(Adjustment::BrightnessContrast(
                    BrightnessContrastAdjustment {
                        brightness: 20,
                        contrast: 0,
                    },
                )),
            },
        );

        assert!(plan.is_empty());
        assert!(state.status().contains("temporary GPU memory"));
        assert!(
            !state
                .adjustment_filter_session
                .as_ref()
                .unwrap()
                .renderer_preview_started
        );
    }

    #[test]
    fn adjustment_filter_preview_commit_and_cancel_follow_session_state() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();
        let kind = crate::core::adjustment::AdjustmentKind::BrightnessContrast;
        let adjustment = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 15,
            contrast: -5,
        });

        let _ = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession { layer_id, kind },
        );
        let _ = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: Some(adjustment.clone()),
            },
        );
        let commit = reduce(
            &mut state,
            Command::CommitAdjustmentFilterSession {
                layer_id,
                adjustment: adjustment.clone(),
            },
        );
        assert!(matches!(
            commit.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::AdjustmentPreviewCommit {
                adjustment: actual,
            })] if *actual == adjustment
        ));
        assert!(commit.commit_request.is_some());
        assert!(!state.has_adjustment_filter_session());

        let _ = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession { layer_id, kind },
        );
        let _ = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: Some(adjustment),
            },
        );
        let cancel = reduce(
            &mut state,
            Command::CancelAdjustmentFilterSession { layer_id },
        );
        assert!(matches!(
            cancel.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::AdjustmentPreviewCancel)]
        ));
        assert!(cancel.commit_request.is_none());
        assert!(!state.has_adjustment_filter_session());
    }

    #[test]
    fn adjustment_filter_preview_blocks_other_document_edits() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();
        let kind = crate::core::adjustment::AdjustmentKind::Levels;
        let layer_count = state.document().unwrap().layer_tree.rows().len();

        let _ = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession { layer_id, kind },
        );
        let blocked = reduce(&mut state, Command::AddLayer);

        assert!(blocked.is_empty());
        assert_eq!(
            state.document().unwrap().layer_tree.rows().len(),
            layer_count
        );
        assert!(state.status().contains("active filter preview"));
    }

    #[test]
    fn invert_does_not_support_adjustment_filter_preview_session() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();

        let plan = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession {
                layer_id,
                kind: crate::core::adjustment::AdjustmentKind::Invert,
            },
        );

        assert!(plan.is_empty());
        assert!(!state.has_adjustment_filter_session());
        assert!(
            state
                .status()
                .contains("does not support destructive filter preview")
        );
    }

    #[test]
    fn adjustment_filter_targets_allowed_materials_and_snapshots_hidden_selection() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);
        let layer_id = state.active_layer_id();
        let material_a = state.document().unwrap().materials[0].id;
        assert!(
            state
                .document
                .document
                .as_mut()
                .unwrap()
                .layer_tree
                .set_layer_material_mask(
                    layer_id,
                    LayerMaterialMask::Specified([material_a].into_iter().collect()),
                )
        );
        let document = state.document.document.as_mut().unwrap();
        document.active_selection.enable_material_mask(0.into());
        document.active_selection.visible = false;
        let adjustment = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 25,
            contrast: -10,
        });

        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: adjustment.clone(),
            },
        );

        let expected = PaintSurfaceId::raster(0.into(), layer_id);
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::Adjustment {
                surfaces,
                adjustment: actual,
                active_selection,
            })]
                if surfaces == &[expected]
                    && *actual == adjustment
                    && active_selection.enabled
                    && !active_selection.visible
                    && active_selection.material_mask(0.into()).is_some_and(|mask| mask.mask_id.is_some())
        ));
        assert_eq!(
            plan.commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.as_slice()),
            Some([expected].as_slice())
        );
    }

    #[test]
    fn adjustment_filter_skips_materials_without_active_selection_masks() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);
        let layer_id = state.active_layer_id();
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .active_selection
            .enable_material_mask(0.into());

        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: Adjustment::Invert,
            },
        );

        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::Adjustment { surfaces, .. })]
                if surfaces == &[PaintSurfaceId::raster(0.into(), layer_id)]
        ));
    }

    #[test]
    fn identity_adjustment_filter_produces_no_renderer_work() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();

        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: Adjustment::BrightnessContrast(Default::default()),
            },
        );

        assert!(plan.is_empty());
    }

    #[test]
    fn adjustment_filter_rejects_unsupported_adjustments() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();

        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: Adjustment::GradientMap(GradientMapAdjustment::default()),
            },
        );

        assert!(plan.is_empty());
        assert!(
            state
                .status()
                .contains("not available as a destructive filter")
        );
    }

    #[test]
    fn adjustment_filter_rejects_dialog_target_after_active_layer_changes() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let original_layer_id = state.active_layer_id();
        let _ = reduce(&mut state, Command::AddLayer);
        assert_ne!(state.active_layer_id(), original_layer_id);

        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id: original_layer_id,
                adjustment: Adjustment::Invert,
            },
        );

        assert!(plan.is_empty());
        assert!(state.status().contains("editable raster layer"));
    }

    #[test]
    fn spatial_blur_resolves_all_materials_and_precomputes_normal_threshold() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);
        let layer_id = state.active_layer_id();
        let plan = reduce(
            &mut state,
            Command::ApplySpatialBlur {
                radius_px: 6.0,
                cross_meshes: false,
                orientation: SpatialBlurOrientation::SimilarNormals,
                maximum_normal_angle_degrees: 60.0,
            },
        );

        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::SpatialBlur {
                source_surfaces,
                affected_surfaces,
                params,
                active_selection,
            })]
                if source_surfaces == &[
                    PaintSurfaceId::raster(0.into(), layer_id),
                    PaintSurfaceId::raster(1.into(), layer_id),
                ]
                && affected_surfaces == source_surfaces
                && !active_selection.enabled
                && !params.cross_meshes
                && params.orientation == SpatialBlurOrientation::SimilarNormals
                && (params.normal_threshold_cos - 0.5).abs() < 1.0e-5
                && (params.reference_radius_px - 6.0).abs() < 1.0e-5
        ));
    }

    #[test]
    fn blur_filters_keep_all_sources_and_only_affect_hidden_selected_materials() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);
        let layer_id = state.active_layer_id();
        let document = state.document.document.as_mut().unwrap();
        document.active_selection.enable_material_mask(1.into());
        document.active_selection.visible = false;
        let sources = [
            PaintSurfaceId::raster(0.into(), layer_id),
            PaintSurfaceId::raster(1.into(), layer_id),
        ];
        let affected = [sources[1]];

        let surface = reduce(&mut state, Command::ApplySurfaceBlur { radius_px: 4.0 });
        assert!(matches!(
            surface.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::SurfaceBlur {
                source_surfaces,
                affected_surfaces,
                active_selection,
                ..
            })] if source_surfaces == &sources
                && affected_surfaces == &affected
                && active_selection.enabled
                && !active_selection.visible
        ));
        assert_eq!(
            surface
                .commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.as_slice()),
            Some(affected.as_slice())
        );

        let spatial = reduce(
            &mut state,
            Command::ApplySpatialBlur {
                radius_px: 6.0,
                cross_meshes: true,
                orientation: SpatialBlurOrientation::Ignore,
                maximum_normal_angle_degrees: 180.0,
            },
        );
        assert!(matches!(
            spatial.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::SpatialBlur {
                source_surfaces,
                affected_surfaces,
                active_selection,
                ..
            })] if source_surfaces == &sources
                && affected_surfaces == &affected
                && active_selection.enabled
                && !active_selection.visible
        ));
        assert_eq!(
            spatial
                .commit_request
                .as_ref()
                .map(|request| request.finalized_surfaces.as_slice()),
            Some(affected.as_slice())
        );
    }

    #[test]
    fn blur_filters_skip_renderer_work_when_selection_has_no_source_mask() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .active_selection
            .enabled = true;

        let surface = reduce(&mut state, Command::ApplySurfaceBlur { radius_px: 4.0 });
        assert!(surface.is_empty());
        assert!(
            state
                .status()
                .contains("does not cover any surface for Surface Blur")
        );

        let spatial = reduce(
            &mut state,
            Command::ApplySpatialBlur {
                radius_px: 4.0,
                cross_meshes: false,
                orientation: SpatialBlurOrientation::Ignore,
                maximum_normal_angle_degrees: 180.0,
            },
        );
        assert!(spatial.is_empty());
        assert!(
            state
                .status()
                .contains("does not cover any surface for Spatial Blur")
        );
    }

    #[test]
    fn surface_blur_targets_layer_mask_surfaces() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);
        let layer_id = state.active_layer_id();
        let _ = reduce(
            &mut state,
            Command::AddLayerMask {
                layer_id,
                mode: crate::core::surface::LayerMaskInitMode::RevealAll,
            },
        );
        let _ = reduce(&mut state, Command::SelectLayerMask { layer_id });

        let plan = reduce(&mut state, Command::ApplySurfaceBlur { radius_px: 4.0 });

        let expected = [
            PaintSurfaceId::layer_mask(0.into(), layer_id),
            PaintSurfaceId::layer_mask(1.into(), layer_id),
        ];
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::SurfaceBlur {
                source_surfaces,
                affected_surfaces,
                ..
            })] if source_surfaces == &expected && affected_surfaces == &expected
        ));
    }

    #[test]
    fn layer_mask_adjustments_use_scalar_capabilities_and_role_identity() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let layer_id = state.active_layer_id();
        let _ = reduce(
            &mut state,
            Command::AddLayerMask {
                layer_id,
                mode: crate::core::surface::LayerMaskInitMode::RevealAll,
            },
        );
        let _ = reduce(&mut state, Command::SelectLayerMask { layer_id });

        assert!(state.adjustment_filter_available(AdjustmentKind::BrightnessContrast));
        assert!(state.adjustment_filter_available(AdjustmentKind::Invert));
        assert!(!state.adjustment_filter_available(AdjustmentKind::HueSaturation));
        assert!(state.blur_filter_available());

        let invert = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: Adjustment::Invert,
            },
        );
        assert!(matches!(
            invert.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::Adjustment { surfaces, .. })]
                if surfaces == &[PaintSurfaceId::layer_mask(0.into(), layer_id)]
        ));

        let hsv = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: AdjustmentKind::HueSaturation.default_adjustment(),
            },
        );
        assert!(hsv.is_empty());
        assert!(
            state
                .status()
                .contains("not available as a destructive filter")
        );

        let _ = reduce(
            &mut state,
            Command::BeginAdjustmentFilterSession {
                layer_id,
                kind: AdjustmentKind::Levels,
            },
        );
        assert_eq!(
            state
                .adjustment_filter_session
                .as_ref()
                .map(|session| session.target_role),
            Some(PaintSurfaceRole::LayerMask)
        );
        state.document.editor.active_part = ActiveLayerPart::Content;
        let invalid = reduce(
            &mut state,
            Command::PreviewAdjustmentFilter {
                layer_id,
                adjustment: Some(AdjustmentKind::Levels.default_adjustment()),
            },
        );
        assert!(invalid.is_empty());
        assert!(state.status().contains("target role changed"));
    }

    #[test]
    fn disabled_solid_fill_mask_remains_a_filter_target() {
        let mut state = state_with_materials(vec![MaterialSpec::new("A", [8, 8])]);
        let _ = reduce(
            &mut state,
            Command::AddSolidFillLayer {
                color: [0.25, 0.5, 0.75],
            },
        );
        let layer_id = state.active_layer_id();
        let _ = reduce(
            &mut state,
            Command::AddLayerMask {
                layer_id,
                mode: crate::core::surface::LayerMaskInitMode::RevealAll,
            },
        );
        let _ = reduce(&mut state, Command::SelectLayerMask { layer_id });
        let _ = reduce(
            &mut state,
            Command::SetLayerMaskEnabled {
                layer_id,
                enabled: false,
            },
        );

        assert!(state.adjustment_filter_available(AdjustmentKind::Invert));
        let plan = reduce(
            &mut state,
            Command::ApplyAdjustmentFilter {
                layer_id,
                adjustment: Adjustment::Invert,
            },
        );
        assert!(matches!(
            plan.edit_commands.as_slice(),
            [EditCommand::Filter(FilterCommand::Adjustment { surfaces, .. })]
                if surfaces == &[PaintSurfaceId::layer_mask(0.into(), layer_id)]
        ));
    }

    #[test]
    fn surface_blur_rejects_out_of_range_radius_without_renderer_work() {
        let mut state = state_with_materials(vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ]);

        let plan = reduce(&mut state, Command::ApplySurfaceBlur { radius_px: 32.0 });

        assert!(plan.is_empty());
        assert!(state.status().contains("between 0.5 and 16"));
    }

    fn state_with_materials(materials: Vec<MaterialSpec>) -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(two_material_mesh(), materials));
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        state
    }

    fn two_material_mesh() -> MeshData {
        let mesh_id = MeshId(1);
        MeshData::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE],
            vec![Vec3::Z; 4],
            vec![[0, 1, 2], [2, 1, 3]],
            vec![
                SubMesh {
                    mesh_id,
                    start_index: 0,
                    index_count: 3,
                    material_index: 0,
                    material_name: "A".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id,
                    start_index: 3,
                    index_count: 3,
                    material_index: 1,
                    material_name: "B".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: mesh_id,
                name: "Mesh".to_owned(),
            }],
            vec![mesh_id, mesh_id],
        )
        .expect("two material mesh")
    }
}
