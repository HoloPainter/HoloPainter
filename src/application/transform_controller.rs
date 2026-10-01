use std::collections::HashSet;

use anyhow::{Result, anyhow, ensure};
use glam::{Mat3, Vec2};

use crate::{
    core::{
        damage::{DamageMap, union_rect},
        geometry::RectU32,
        material::MaterialIndex,
        selection::ActiveSelection,
        surface::{LayerId, PaintSurfaceId},
        transform::{
            TransformHandle, TransformNumericValues, TransformScaleConstraint, UvTransform,
            UvTransformPreview, hit_test_transform_handle,
        },
    },
    renderer::{EditCommand, GpuDocumentCommand, TransformCommand},
};

use super::{
    CompositeSync, ImportedImagePayload, LayerTreeSnapshot, PendingDocumentCommit,
    PendingRendererHistoryTransaction, ReducerOutput, RendererCommitSpec, ToolInputEvent,
    command::{InputModifiers, PointerSampleKind, ToolCancelReason, TransformNumericEdit},
    history_capture::PixelHistoryCapture,
    state::{
        AppState, EmbeddedImageTransformSession, PaintTargetScope, ToolSession, TransformGesture,
        TransformSession, TransformSessionSource,
    },
};

const HANDLE_HIT_RADIUS_PX: f32 = 9.0;
const MIN_SCALE: f32 = 0.01;
const DAMAGE_GUARD_PX: u32 = 2;
const PREVIEW_INTERVAL_S: f64 = 1.0 / 30.0;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransformSource {
    pub(crate) target: PaintSurfaceId,
    pub(crate) material_index: MaterialIndex,
    pub(crate) texture_size: [u32; 2],
    pub(crate) source_bounds: RectU32,
    pub(crate) selection_before: ActiveSelection,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransformBeginSpec {
    pub(crate) source: TransformSource,
    pub(crate) initial_transform: UvTransform,
    pub(crate) scale_constraint: TransformScaleConstraint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransformCancelRendererAction {
    RestoreSource,
    DiscardSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformSessionFinish {
    Commit,
    Cancel(TransformCancelRendererAction),
}

#[derive(Debug)]
pub(crate) struct FinishedTransformSession {
    pub(crate) session: TransformSession,
    pub(crate) output: ReducerOutput,
}

impl FinishedTransformSession {
    fn into_parts(self) -> (TransformSession, ReducerOutput) {
        (self.session, self.output)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformGestureFinish {
    AcceptCurrent,
    RestoreBase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TransformGestureUpdate {
    transform: UvTransform,
    scale_sign: Vec2,
}

pub(crate) fn idle_transform_preview(state: &AppState) -> Option<UvTransformPreview> {
    if let Some(context) = embedded_transform_context(state) {
        let transform = embedded_image_uv_transform(
            context.transform,
            context.source_size,
            context.material_size,
        )?;
        return Some(preview_for_bounds(
            RectU32::full(context.source_size),
            context.material_size,
            transform,
            None,
        ));
    }
    let source = cached_or_resolve_transform_source(state).ok().flatten()?;
    Some(preview_for_bounds(
        source.source_bounds,
        source.texture_size,
        UvTransform::identity(),
        None,
    ))
}

pub(crate) fn transform_numeric_values(state: &AppState) -> Option<TransformNumericValues> {
    if let Some(session) = state.tool.transform_session() {
        return Some(numeric_values_for(
            session.source_bounds,
            session.transform,
            session.scale_sign,
        ));
    }
    if let Some(session) = state.tool.embedded_image_transform_session() {
        return Some(numeric_values_for(
            session.source_bounds(),
            session.transform,
            session.scale_sign,
        ));
    }
    if let Some(context) = embedded_transform_context(state) {
        return Some(TransformNumericValues {
            center_px: context.transform.center_uv
                * Vec2::from_array(context.material_size.map(|v| v as f32)),
            size_px: context.transform.size_uv
                * Vec2::from_array(context.material_size.map(|v| v as f32)),
            rotation_degrees: context.transform.rotation_radians.to_degrees(),
        });
    }
    let source = cached_or_resolve_transform_source(state).ok().flatten()?;
    Some(numeric_values_for(
        source.source_bounds,
        UvTransform::identity(),
        Vec2::ONE,
    ))
}

pub(crate) fn update_transform_numeric(
    state: &mut AppState,
    edit: TransformNumericEdit,
) -> Result<ReducerOutput> {
    if !edit.value().is_finite()
        || state
            .tool
            .transform_session()
            .is_some_and(|session| session.gesture.is_some())
        || state
            .tool
            .embedded_image_transform_session()
            .is_some_and(|session| session.gesture.is_some())
    {
        return Ok(ReducerOutput::default());
    }

    if state.tool.transform_session().is_none()
        && state.tool.embedded_image_transform_session().is_none()
        && embedded_transform_context(state).is_some()
    {
        begin_embedded_image_transform_session(state)?;
    }
    if let Some(session) = state.tool.embedded_image_transform_session_mut() {
        let mut values = numeric_values_for(
            session.source_bounds(),
            session.transform,
            session.scale_sign,
        );
        let source_size = Vec2::from_array(session.source_size.map(|v| v as f32));
        match edit {
            TransformNumericEdit::CenterX(value) => values.center_px.x = value,
            TransformNumericEdit::CenterY(value) => values.center_px.y = value,
            TransformNumericEdit::Width(value) => {
                values.size_px.x =
                    clamp_signed_magnitude(value, source_size.x * MIN_SCALE, session.scale_sign.x);
            }
            TransformNumericEdit::Height(value) => {
                values.size_px.y =
                    clamp_signed_magnitude(value, source_size.y * MIN_SCALE, session.scale_sign.y);
            }
            TransformNumericEdit::RotationDegrees(value) => values.rotation_degrees = value,
        }
        let Some(transform) = UvTransform::from_numeric_values(Vec2::ZERO, source_size, values)
        else {
            return Ok(ReducerOutput::default());
        };
        session.transform = transform;
        session.scale_sign = scale_sign_for_size(values.size_px);
        let output = submit_embedded_preview_if_changed(session);
        state.set_status_key("status-image-transform-editing");
        return Ok(output);
    }

    let mut output = ReducerOutput::default();
    if state.tool.transform_session().is_none() {
        let Some(source) = cached_or_resolve_transform_source(state)? else {
            return Ok(output);
        };
        output.append_outcome(begin_transform_session(
            state,
            TransformBeginSpec {
                source,
                initial_transform: UvTransform::identity(),
                scale_constraint: TransformScaleConstraint::Free,
            },
        )?);
    }

    let session = state
        .tool
        .transform_session_mut()
        .expect("numeric edit started or found a Transform session");
    let mut values =
        numeric_values_for(session.source_bounds, session.transform, session.scale_sign);
    let source_size = Vec2::new(
        session.source_bounds.size[0] as f32,
        session.source_bounds.size[1] as f32,
    );
    match edit {
        TransformNumericEdit::CenterX(value) => values.center_px.x = value,
        TransformNumericEdit::CenterY(value) => values.center_px.y = value,
        TransformNumericEdit::Width(value) => {
            values.size_px.x =
                clamp_signed_magnitude(value, source_size.x * MIN_SCALE, session.scale_sign.x);
            if session.scale_constraint == TransformScaleConstraint::PreserveAspect {
                values.size_px.y =
                    values.size_px.x.abs() * source_size.y / source_size.x * session.scale_sign.y;
            }
        }
        TransformNumericEdit::Height(value) => {
            values.size_px.y =
                clamp_signed_magnitude(value, source_size.y * MIN_SCALE, session.scale_sign.y);
            if session.scale_constraint == TransformScaleConstraint::PreserveAspect {
                values.size_px.x =
                    values.size_px.y.abs() * source_size.x / source_size.y * session.scale_sign.x;
            }
        }
        TransformNumericEdit::RotationDegrees(value) => values.rotation_degrees = value,
    }
    let source_origin = Vec2::new(
        session.source_bounds.origin[0] as f32,
        session.source_bounds.origin[1] as f32,
    );
    let Some(transform) = UvTransform::from_numeric_values(source_origin, source_size, values)
    else {
        return Ok(output);
    };
    session.transform = transform;
    session.scale_sign = scale_sign_for_size(values.size_px);
    output.append_outcome(submit_transform_preview_if_changed(session));
    state.set_status_key("status-transform-ready");
    Ok(output)
}

struct EmbeddedTransformContext {
    layer_id: LayerId,
    transform: crate::core::embedded_image::EmbeddedImageTransform,
    material_index: MaterialIndex,
    material_size: [u32; 2],
    source_size: [u32; 2],
}

fn embedded_transform_context(state: &AppState) -> Option<EmbeddedTransformContext> {
    let document = state.document()?;
    let layer_id = state.active_layer_id();
    let (image_id, transform) = document.layer_tree.embedded_image(layer_id)?;
    if document.layer_tree.is_effectively_locked(layer_id) {
        return None;
    }
    let material_id = match document.layer_tree.layer_material_mask(layer_id)? {
        crate::core::surface::LayerMaterialMask::Specified(ids) if ids.len() == 1 => {
            *ids.first()?
        }
        _ => return None,
    };
    let material_index = document.material_index(material_id)?;
    if material_index.as_usize() != state.focused_material_index() {
        return None;
    }
    let size = document.material(material_index)?.texture_size;
    let image = document.embedded_image(image_id)?.clone();
    Some(EmbeddedTransformContext {
        layer_id,
        source_size: image.size,
        transform,
        material_index,
        material_size: size,
    })
}

fn embedded_image_uv_transform(
    transform: crate::core::embedded_image::EmbeddedImageTransform,
    source_size: [u32; 2],
    material_size: [u32; 2],
) -> Option<UvTransform> {
    let source_size = Vec2::from_array(source_size.map(|value| value as f32));
    let material_size = Vec2::from_array(material_size.map(|value| value as f32));
    let center_px = transform.center_uv * material_size;
    let size_px = transform.size_uv * material_size;
    UvTransform::from_matrix(
        Mat3::from_translation(center_px)
            * Mat3::from_angle(transform.rotation_radians)
            * Mat3::from_scale(size_px / source_size)
            * Mat3::from_translation(-source_size * 0.5),
    )
}

fn embedded_image_transform_from_session(
    session: &EmbeddedImageTransformSession,
) -> Option<crate::core::embedded_image::EmbeddedImageTransform> {
    let values = numeric_values_for(
        session.source_bounds(),
        session.transform,
        session.scale_sign,
    );
    let material_size = Vec2::from_array(session.texture_size.map(|value| value as f32));
    let transform = crate::core::embedded_image::EmbeddedImageTransform {
        center_uv: values.center_px / material_size,
        size_uv: values.size_px / material_size,
        rotation_radians: values.rotation_degrees.to_radians(),
    };
    transform.is_valid().then_some(transform)
}

pub(crate) fn begin_embedded_image_transform_session(
    state: &mut AppState,
) -> Result<ReducerOutput> {
    ensure!(
        state.tool.is_idle(),
        "a Transform session is already active"
    );
    let context = embedded_transform_context(state)
        .ok_or_else(|| anyhow!("embedded image Transform target is unavailable"))?;
    ensure!(
        !state
            .document()
            .is_some_and(|document| document.layer_tree.is_effectively_locked(context.layer_id)),
        "embedded image Transform target is locked"
    );
    let transform = embedded_image_uv_transform(
        context.transform,
        context.source_size,
        context.material_size,
    )
    .ok_or_else(|| anyhow!("embedded image Transform is invalid"))?;
    let session = EmbeddedImageTransformSession {
        layer_id: context.layer_id,
        material_index: context.material_index,
        texture_size: context.material_size,
        source_size: context.source_size,
        before: context.transform,
        transform,
        scale_sign: scale_sign_for_size(context.transform.size_uv),
        scale_constraint: TransformScaleConstraint::Free,
        gesture: None,
        last_preview_time_s: None,
        last_submitted_transform: Some(transform),
    };
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::SetEmbeddedImagePreview {
        layer_id: session.layer_id,
        material_index: session.material_index.as_usize(),
        transform: Some(context.transform),
        damage: embedded_transform_damage(&session, transform, transform),
    });
    state
        .tool
        .set_session(ToolSession::EmbeddedImageTransform(session));
    Ok(output)
}

pub(crate) fn begin_transform_numeric_edit(state: &mut AppState) -> Result<ReducerOutput> {
    if state.active_layer_target() != crate::core::document::ActiveLayerTarget::EmbeddedImage
        || state.tool.embedded_image_transform_session().is_some()
        || embedded_transform_context(state).is_none()
    {
        return Ok(ReducerOutput::default());
    }
    begin_embedded_image_transform_session(state)
}

pub(crate) fn commit_transform_numeric_edit(state: &mut AppState) -> Result<ReducerOutput> {
    if state.tool.embedded_image_transform_session().is_none() {
        return Ok(ReducerOutput::default());
    }
    commit_active_embedded_image_transform(state)
}

fn numeric_values_for(
    bounds: RectU32,
    transform: UvTransform,
    scale_sign: Vec2,
) -> TransformNumericValues {
    transform.numeric_values_with_scale_sign(
        Vec2::new(bounds.origin[0] as f32, bounds.origin[1] as f32),
        Vec2::new(bounds.size[0] as f32, bounds.size[1] as f32),
        scale_sign,
    )
}

fn clamp_signed_magnitude(value: f32, minimum: f32, fallback_sign: f32) -> f32 {
    if value.abs() >= minimum {
        return value;
    }
    let sign = if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else if fallback_sign < 0.0 {
        -1.0
    } else {
        1.0
    };
    minimum * sign
}

fn signed_magnitude(sign_source: f32, magnitude: f32) -> f32 {
    if sign_source < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

fn scale_sign_for_size(size_px: Vec2) -> Vec2 {
    Vec2::new(
        if size_px.x < 0.0 { -1.0 } else { 1.0 },
        if size_px.y < 0.0 { -1.0 } else { 1.0 },
    )
}

pub(crate) fn begin_transform_session(
    state: &mut AppState,
    spec: TransformBeginSpec,
) -> Result<ReducerOutput> {
    ensure!(
        state.tool.is_idle(),
        "a Transform session is already active"
    );
    let TransformBeginSpec {
        source,
        initial_transform,
        scale_constraint,
    } = spec;
    ensure!(
        source.source_bounds.size[0] > 0 && source.source_bounds.size[1] > 0,
        "Transform source bounds are empty"
    );
    ensure!(
        source.source_bounds.origin[0]
            .checked_add(source.source_bounds.size[0])
            .is_some_and(|end| end <= source.texture_size[0])
            && source.source_bounds.origin[1]
                .checked_add(source.source_bounds.size[1])
                .is_some_and(|end| end <= source.texture_size[1]),
        "Transform source bounds are outside the texture"
    );

    let mut session = TransformSession {
        target: source.target,
        material_index: source.material_index.into(),
        texture_size: source.texture_size,
        source_bounds: source.source_bounds,
        source: TransformSessionSource::ExistingSurface,
        selection_before: source.selection_before.clone(),
        transform: initial_transform,
        scale_sign: Vec2::ONE,
        scale_constraint,
        gesture: None,
        last_preview_time_s: None,
        last_submitted_transform: None,
    };
    let mut output = transform_command_output(TransformCommand::Begin {
        target: source.target,
        source_bounds: source.source_bounds,
        active_selection: source.selection_before,
    });
    if !initial_transform.is_effectively_identity() {
        session.last_submitted_transform = Some(initial_transform);
        output.push_edit_command(EditCommand::Transform(TransformCommand::Preview {
            transform: initial_transform,
            result_damage: transform_damage(&session),
        }));
    }

    state.tool.invalidate_transform_idle_cache();
    state.tool.set_session(ToolSession::Transform(session));
    Ok(output)
}

pub(crate) fn begin_imported_image_transform_session(
    state: &mut AppState,
    target: PaintSurfaceId,
    target_texture_size: [u32; 2],
    image: ImportedImagePayload,
    before: LayerTreeSnapshot,
    added_surfaces: Vec<PaintSurfaceId>,
) -> Result<ReducerOutput> {
    ensure!(
        state.tool.is_idle(),
        "a Transform session is already active"
    );
    ensure!(
        image.size[0] > 0 && image.size[1] > 0,
        "imported image is empty"
    );
    let source_bounds = RectU32::full(image.size);
    let initial_transform =
        imported_image_initial_transform(target_texture_size, image.size, image.initial_origin)?;
    let selection_before = state
        .document()
        .map(|document| document.active_selection.clone())
        .ok_or_else(|| anyhow!("image import requires a document"))?;
    let session = TransformSession {
        target,
        material_index: target.material_index(),
        texture_size: target_texture_size,
        source_bounds,
        source: TransformSessionSource::ImportedImage {
            before,
            added_surfaces,
            file_name: image.file_name.clone(),
        },
        selection_before,
        transform: initial_transform,
        scale_sign: Vec2::ONE,
        scale_constraint: TransformScaleConstraint::PreserveAspect,
        gesture: None,
        last_preview_time_s: None,
        last_submitted_transform: Some(initial_transform),
    };
    let mut output = transform_command_output(TransformCommand::BeginImported {
        target,
        source_size: image.size,
        source_rgba8: image.rgba8,
    });
    output.push_edit_command(EditCommand::Transform(TransformCommand::Preview {
        transform: initial_transform,
        result_damage: transform_damage(&session),
    }));

    state.tool.invalidate_transform_idle_cache();
    state.tool.set_session(ToolSession::Transform(session));
    Ok(output)
}

pub(crate) fn commit_imported_image(
    target: PaintSurfaceId,
    target_texture_size: [u32; 2],
    image: ImportedImagePayload,
    before: LayerTreeSnapshot,
    added_surfaces: Vec<PaintSurfaceId>,
) -> Result<ReducerOutput> {
    ensure!(
        image.size[0] > 0 && image.size[1] > 0,
        "imported image is empty"
    );
    let transform =
        imported_image_initial_transform(target_texture_size, image.size, image.initial_origin)?;
    let mut damage = DamageMap::default();
    damage.add_rect(target, RectU32::full(target_texture_size));

    let mut output = transform_command_output(TransformCommand::BeginImported {
        target,
        source_size: image.size,
        source_rgba8: image.rgba8,
    });
    output.push_edit_command(EditCommand::Transform(TransformCommand::Commit {
        transform,
        result_damage: damage,
    }));
    Ok(output
        .with_optional_renderer_history_transaction(
            "Import Image",
            Some(PendingRendererHistoryTransaction::ImportedLayer {
                before,
                target,
                added_surfaces,
            }),
        )
        .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(vec![target])))
}

fn imported_image_initial_transform(
    target_texture_size: [u32; 2],
    image_size: [u32; 2],
    initial_origin: Option<[u32; 2]>,
) -> Result<UvTransform> {
    let translation = initial_origin.map_or_else(
        || {
            Vec2::new(
                ((target_texture_size[0] as i64 - image_size[0] as i64).div_euclid(2)) as f32,
                ((target_texture_size[1] as i64 - image_size[1] as i64).div_euclid(2)) as f32,
            )
        },
        |origin| Vec2::new(origin[0] as f32, origin[1] as f32),
    );
    UvTransform::from_matrix(Mat3::from_translation(translation))
        .ok_or_else(|| anyhow!("imported image initial transform is invalid"))
}

pub(crate) fn handle_transform_input(
    state: &mut AppState,
    event: ToolInputEvent,
) -> Result<ReducerOutput> {
    match event {
        ToolInputEvent::PointerDown(sample) => {
            let PointerSampleKind::Uv { uv, view } = sample.kind else {
                return Ok(ReducerOutput::default());
            };
            if let Some(session) = state.tool.embedded_image_transform_session() {
                if session.gesture.is_some() {
                    return Ok(ReducerOutput::default());
                }
                let handle = hit_test_handle(
                    session.source_bounds(),
                    session.texture_size,
                    session.transform,
                    sample.screen_px(),
                    view,
                );
                let start_texture_px = uv_to_texture_px(uv, session.texture_size);
                let session = state
                    .tool
                    .embedded_image_transform_session_mut()
                    .expect("embedded image Transform session checked above");
                begin_embedded_transform_gesture(session, handle, start_texture_px);
                state.set_status_key("status-transforming");
                return Ok(ReducerOutput::default());
            }
            if let Some(session) = state.tool.transform_session() {
                if session.gesture.is_some() {
                    return Ok(ReducerOutput::default());
                }
                let handle = hit_test_handle(
                    session.source_bounds,
                    session.texture_size,
                    session.transform,
                    sample.screen_px(),
                    view,
                );
                let start_texture_px = uv_to_texture_px(uv, session.texture_size);
                let session = state
                    .tool
                    .transform_session_mut()
                    .expect("Transform session checked above");
                begin_transform_gesture(session, handle, start_texture_px);
                state.set_status_key("status-transforming");
                return Ok(ReducerOutput::default());
            }
            if !state.tool.is_idle() {
                return Ok(ReducerOutput::default());
            }
            if state.active_layer_target()
                == crate::core::document::ActiveLayerTarget::EmbeddedImage
            {
                if embedded_transform_context(state).is_none() {
                    return Ok(ReducerOutput::default());
                }
                let output = begin_embedded_image_transform_session(state)?;
                let session = state
                    .tool
                    .embedded_image_transform_session()
                    .expect("embedded image Transform session started above");
                let handle = hit_test_handle(
                    session.source_bounds(),
                    session.texture_size,
                    session.transform,
                    sample.screen_px(),
                    view,
                );
                let start_texture_px = uv_to_texture_px(uv, session.texture_size);
                let session = state
                    .tool
                    .embedded_image_transform_session_mut()
                    .expect("embedded image Transform session started above");
                begin_embedded_transform_gesture(session, handle, start_texture_px);
                state.set_status_key("status-transforming");
                return Ok(output);
            }
            let Some(source) = cached_or_resolve_transform_source(state)? else {
                state.set_status_key("status-transform-target-empty");
                return Ok(ReducerOutput::default());
            };
            let initial_transform = UvTransform::identity();
            let handle = hit_test_handle(
                source.source_bounds,
                source.texture_size,
                initial_transform,
                sample.screen_px(),
                view,
            );
            let start_texture_px = uv_to_texture_px(uv, source.texture_size);
            let output = begin_transform_session(
                state,
                TransformBeginSpec {
                    source,
                    initial_transform,
                    scale_constraint: TransformScaleConstraint::Free,
                },
            )?;
            let session = state
                .tool
                .transform_session_mut()
                .expect("Transform session started above");
            begin_transform_gesture(session, handle, start_texture_px);
            state.set_status_key("status-transforming");
            Ok(output)
        }
        ToolInputEvent::PointerMove(sample) => {
            let PointerSampleKind::Uv { uv, .. } = sample.kind else {
                return Ok(ReducerOutput::default());
            };
            if let Some(session) = state.tool.embedded_image_transform_session_mut() {
                let Some(gesture) = session.gesture else {
                    return Ok(ReducerOutput::default());
                };
                let current_px = uv_to_texture_px(uv, session.texture_size);
                let update = transform_from_gesture(
                    session.source_bounds(),
                    gesture,
                    current_px,
                    sample.modifiers(),
                    session.scale_constraint,
                );
                session.transform = update.transform;
                session.scale_sign = update.scale_sign;
                if !should_submit_embedded_preview(session, sample.meta.time_s) {
                    return Ok(ReducerOutput::default());
                }
                session.last_preview_time_s = Some(sample.meta.time_s);
                return Ok(submit_embedded_preview_if_changed(session));
            }
            let Some(session) = state.tool.transform_session_mut() else {
                return Ok(ReducerOutput::default());
            };
            let Some(gesture) = session.gesture else {
                return Ok(ReducerOutput::default());
            };
            let current_px = uv_to_texture_px(uv, session.texture_size);
            let update = transform_from_gesture(
                session.source_bounds,
                gesture,
                current_px,
                sample.modifiers(),
                session.scale_constraint,
            );
            session.transform = update.transform;
            session.scale_sign = update.scale_sign;
            if !should_submit_preview(session, sample.meta.time_s) {
                return Ok(ReducerOutput::default());
            }
            session.last_preview_time_s = Some(sample.meta.time_s);
            session.last_submitted_transform = Some(session.transform);
            Ok(transform_command_output(TransformCommand::Preview {
                transform: session.transform,
                result_damage: transform_damage(session),
            }))
        }
        ToolInputEvent::PointerUp(sample) => {
            let PointerSampleKind::Uv { uv, .. } = sample.kind else {
                return Ok(ReducerOutput::default());
            };
            if state.tool.embedded_image_transform_session().is_some() {
                {
                    let session = state
                        .tool
                        .embedded_image_transform_session_mut()
                        .expect("embedded image Transform session checked above");
                    let Some(gesture) = session.gesture else {
                        return Ok(ReducerOutput::default());
                    };
                    let current_px = uv_to_texture_px(uv, session.texture_size);
                    let update = transform_from_gesture(
                        session.source_bounds(),
                        gesture,
                        current_px,
                        sample.modifiers(),
                        session.scale_constraint,
                    );
                    session.transform = update.transform;
                    session.scale_sign = update.scale_sign;
                    session.gesture.take();
                }
                return commit_active_embedded_image_transform(state);
            }
            let output = {
                let Some(session) = state.tool.transform_session_mut() else {
                    return Ok(ReducerOutput::default());
                };
                let Some(gesture) = session.gesture else {
                    return Ok(ReducerOutput::default());
                };
                let current_px = uv_to_texture_px(uv, session.texture_size);
                let update = transform_from_gesture(
                    session.source_bounds,
                    gesture,
                    current_px,
                    sample.modifiers(),
                    session.scale_constraint,
                );
                session.transform = update.transform;
                session.scale_sign = update.scale_sign;
                let output =
                    finish_transform_gesture(session, TransformGestureFinish::AcceptCurrent);
                if output.has_renderer_work() {
                    session.last_preview_time_s = Some(sample.meta.time_s);
                }
                output
            };
            state.set_status_key("status-transform-ready");
            Ok(output)
        }
        ToolInputEvent::Cancel { .. } => unreachable!("cancel is handled before tool dispatch"),
    }
}

fn begin_transform_gesture(
    session: &mut TransformSession,
    handle: TransformHandle,
    start_texture_px: Vec2,
) {
    session.gesture = Some(TransformGesture {
        handle,
        start_texture_px,
        base_transform: session.transform,
        base_scale_sign: session.scale_sign,
    });
    session.last_preview_time_s = None;
}

fn begin_embedded_transform_gesture(
    session: &mut EmbeddedImageTransformSession,
    handle: TransformHandle,
    start_texture_px: Vec2,
) {
    session.gesture = Some(TransformGesture {
        handle,
        start_texture_px,
        base_transform: session.transform,
        base_scale_sign: session.scale_sign,
    });
    session.last_preview_time_s = None;
}

fn submit_embedded_preview_if_changed(
    session: &mut EmbeddedImageTransformSession,
) -> ReducerOutput {
    if session.last_submitted_transform == Some(session.transform) {
        return ReducerOutput::default();
    }
    let Some(transform) = embedded_image_transform_from_session(session) else {
        return ReducerOutput::default();
    };
    let previous = session
        .last_submitted_transform
        .unwrap_or(session.transform);
    let damage = embedded_transform_damage(session, previous, session.transform);
    session.last_submitted_transform = Some(session.transform);
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::SetEmbeddedImagePreview {
        layer_id: session.layer_id,
        material_index: session.material_index.as_usize(),
        transform: Some(transform),
        damage,
    });
    output
}

fn finish_transform_gesture(
    session: &mut TransformSession,
    finish: TransformGestureFinish,
) -> ReducerOutput {
    let Some(gesture) = session.gesture.take() else {
        return ReducerOutput::default();
    };
    if finish == TransformGestureFinish::RestoreBase {
        session.transform = gesture.base_transform;
        session.scale_sign = gesture.base_scale_sign;
        session.last_preview_time_s = None;
    }
    submit_transform_preview_if_changed(session)
}

fn submit_transform_preview_if_changed(session: &mut TransformSession) -> ReducerOutput {
    if session.last_submitted_transform == Some(session.transform) {
        return ReducerOutput::default();
    }
    session.last_submitted_transform = Some(session.transform);
    transform_command_output(TransformCommand::Preview {
        transform: session.transform,
        result_damage: transform_damage(session),
    })
}

pub(crate) fn apply_active_transform(state: &mut AppState) -> Result<ReducerOutput> {
    if state.tool.embedded_image_transform_session().is_some() {
        return Ok(ReducerOutput::default());
    }
    let Some(session) = state.tool.transform_session() else {
        return Ok(ReducerOutput::default());
    };
    if session.transform.is_effectively_identity()
        && matches!(&session.source, TransformSessionSource::ExistingSurface)
    {
        return Ok(cancel_active_transform(state));
    }

    let finished = finish_active_transform(state, TransformSessionFinish::Commit)?
        .expect("Transform session checked above");
    let (session, output) = finished.into_parts();
    match &session.source {
        TransformSessionSource::ImportedImage { file_name, .. } => {
            state.set_status_message(
                super::StatusMessage::localized("status-imported-layer")
                    .arg("file_name", file_name),
            );
        }
        TransformSessionSource::ExistingSurface => state.set_status_key("status-transform-applied"),
    }
    Ok(output)
}

pub(crate) fn commit_active_embedded_image_transform(
    state: &mut AppState,
) -> Result<ReducerOutput> {
    let session = state
        .tool
        .take_embedded_image_transform_session()
        .expect("embedded image Transform session checked above");
    let after = embedded_image_transform_from_session(&session)
        .ok_or_else(|| anyhow!("embedded image Transform is invalid"))?;
    let previous = session
        .last_submitted_transform
        .unwrap_or(session.transform);
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::SetEmbeddedImagePreview {
        layer_id: session.layer_id,
        material_index: session.material_index.as_usize(),
        transform: None,
        damage: embedded_transform_damage(&session, previous, session.transform),
    });
    output.append_outcome(super::layer_reducer::set_embedded_image_transform(
        state,
        session.layer_id,
        after,
    )?);
    state.set_status_key("status-image-transform-updated");
    state.tool.invalidate_transform_idle_cache();
    Ok(output)
}

pub(crate) fn cancel_active_transform(state: &mut AppState) -> ReducerOutput {
    if state.tool.embedded_image_transform_session().is_some() {
        return cancel_active_embedded_image_transform(state);
    }
    let Some(finished) =
        cancel_active_transform_with_action(state, TransformCancelRendererAction::RestoreSource)
    else {
        return ReducerOutput::default();
    };
    let (session, output) = finished.into_parts();
    match session.source {
        TransformSessionSource::ImportedImage { .. } => {
            state.set_status_key("status-image-import-cancelled");
        }
        TransformSessionSource::ExistingSurface => {
            state.set_status_key("status-transform-cancelled")
        }
    }
    output
}

fn cancel_active_embedded_image_transform(state: &mut AppState) -> ReducerOutput {
    let session = state
        .tool
        .take_embedded_image_transform_session()
        .expect("embedded image Transform session checked above");
    let previous = session
        .last_submitted_transform
        .unwrap_or(session.transform);
    let before =
        embedded_image_uv_transform(session.before, session.source_size, session.texture_size)
            .unwrap_or(session.transform);
    let mut output = ReducerOutput::default();
    output.push_document_command(GpuDocumentCommand::SetEmbeddedImagePreview {
        layer_id: session.layer_id,
        material_index: session.material_index.as_usize(),
        transform: None,
        damage: embedded_transform_damage(&session, previous, before),
    });
    state.set_status_key("status-image-transform-cancelled");
    state.tool.invalidate_transform_idle_cache();
    output
}

fn cancel_active_transform_with_action(
    state: &mut AppState,
    action: TransformCancelRendererAction,
) -> Option<FinishedTransformSession> {
    finish_active_transform(state, TransformSessionFinish::Cancel(action))
        .expect("cancelling a Transform session is infallible")
}

#[allow(dead_code)]
pub(crate) fn discard_active_transform_session(
    state: &mut AppState,
) -> Option<FinishedTransformSession> {
    cancel_active_transform_with_action(state, TransformCancelRendererAction::DiscardSession)
}

fn finish_active_transform(
    state: &mut AppState,
    finish: TransformSessionFinish,
) -> Result<Option<FinishedTransformSession>> {
    let Some(active_session) = state.tool.transform_session() else {
        return Ok(None);
    };

    match finish {
        TransformSessionFinish::Commit => {
            if let TransformSessionSource::ImportedImage {
                before,
                added_surfaces,
                ..
            } = &active_session.source
            {
                let before = before.clone();
                let added_surfaces = added_surfaces.clone();
                let target = active_session.target;
                let transform = active_session.transform;
                let mut damage = DamageMap::default();
                damage.add_rect(target, RectU32::full(active_session.texture_size));
                let session = state
                    .tool
                    .take_transform_session()
                    .expect("Transform session checked above");
                let output = transform_command_output(TransformCommand::Commit {
                    transform,
                    result_damage: damage,
                })
                .with_optional_renderer_history_transaction(
                    "Import Image",
                    Some(PendingRendererHistoryTransaction::ImportedLayer {
                        before,
                        target,
                        added_surfaces,
                    }),
                )
                .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(vec![target]));
                state.tool.invalidate_transform_idle_cache();
                return Ok(Some(FinishedTransformSession { session, output }));
            }

            let damage = transform_damage(active_session);
            let pixel_history = PixelHistoryCapture::from_damage(damage.clone())
                .map(|capture| capture.capture(state))
                .transpose()?
                .flatten();
            let session = state
                .tool
                .take_transform_session()
                .expect("Transform session checked above");
            let mut output = transform_command_output(TransformCommand::Commit {
                transform: session.transform,
                result_damage: damage,
            })
            .with_optional_renderer_history_transaction("Transform", pixel_history)
            .with_renderer_commit(RendererCommitSpec::with_finalized_surfaces(vec![
                session.target,
            ]));

            if transforms_selection(&session) {
                let after = session.selection_before.clone();
                output = output
                    .with_optional_renderer_history_transaction(
                        "Transform",
                        Some(PendingRendererHistoryTransaction::Selection {
                            before: session.selection_before.clone(),
                            after: after.clone(),
                        }),
                    )
                    .with_document_commit(PendingDocumentCommit::Selection {
                        after: after.clone(),
                    })
                    .with_renderer_commit(RendererCommitSpec::with_selection_commit(
                        session.selection_before.clone(),
                        after,
                    ));
            }
            state.tool.invalidate_transform_idle_cache();
            return Ok(Some(FinishedTransformSession { session, output }));
        }
        TransformSessionFinish::Cancel(action) => {
            let session = state
                .tool
                .take_transform_session()
                .expect("Transform session checked above");
            let mut output = match (&session.source, action) {
                (TransformSessionSource::ImportedImage { .. }, _) => {
                    transform_command_output(TransformCommand::Discard)
                }
                (_, TransformCancelRendererAction::RestoreSource) => {
                    transform_command_output(TransformCommand::Cancel)
                }
                (_, TransformCancelRendererAction::DiscardSession) => {
                    transform_command_output(TransformCommand::Discard)
                }
            };
            if let TransformSessionSource::ImportedImage {
                before,
                added_surfaces,
                ..
            } = &session.source
            {
                if let Some((document, editor_document)) = state.document_and_editor_document_mut()
                {
                    let change = document
                        .restore_layer_tree(before.tree.clone(), &[])
                        .map_err(|err| anyhow!("Imported image cancel failed: {err:#}"))?;
                    debug_assert_eq!(
                        change.deleted_surfaces.iter().collect::<HashSet<_>>(),
                        added_surfaces.iter().collect::<HashSet<_>>()
                    );
                    for surface in change.deleted_surfaces {
                        output.push_document_command(GpuDocumentCommand::DeleteSurface {
                            target: surface,
                        });
                    }
                    editor_document.active_layer_id = before.active_layer_id;
                    editor_document.active_part = before.active_part;
                }
                output = output.with_composite_sync(CompositeSync::MaterialTrees);
            }
            state.tool.invalidate_transform_idle_cache();
            return Ok(Some(FinishedTransformSession { session, output }));
        }
    };
}

pub(crate) fn handle_transform_cancel(
    state: &mut AppState,
    reason: ToolCancelReason,
) -> ReducerOutput {
    match reason {
        ToolCancelReason::Escape | ToolCancelReason::ToolChanged => cancel_active_transform(state),
        ToolCancelReason::FocusLost
        | ToolCancelReason::PointerCaptureLost
        | ToolCancelReason::TouchCancelled => cancel_active_transform_gesture(state),
    }
}

fn cancel_active_transform_gesture(state: &mut AppState) -> ReducerOutput {
    if state.tool.embedded_image_transform_session().is_some() {
        return cancel_active_embedded_image_transform(state);
    }
    let output = {
        let Some(session) = state.tool.transform_session_mut() else {
            return ReducerOutput::default();
        };
        finish_transform_gesture(session, TransformGestureFinish::RestoreBase)
    };
    state.set_status_key("status-transform-ready");
    output
}

fn transform_command_output(command: TransformCommand) -> ReducerOutput {
    let mut output = ReducerOutput::default();
    output.push_edit_command(EditCommand::Transform(command));
    output
}

fn should_submit_preview(session: &TransformSession, time_s: f64) -> bool {
    if session.last_submitted_transform == Some(session.transform) {
        return false;
    }
    let Some(last_time_s) = session.last_preview_time_s else {
        return true;
    };
    time_s <= 0.0 || last_time_s <= 0.0 || time_s - last_time_s >= PREVIEW_INTERVAL_S
}

fn should_submit_embedded_preview(session: &EmbeddedImageTransformSession, time_s: f64) -> bool {
    if session.last_submitted_transform == Some(session.transform) {
        return false;
    }
    let Some(last_time_s) = session.last_preview_time_s else {
        return true;
    };
    time_s <= 0.0 || last_time_s <= 0.0 || time_s - last_time_s >= PREVIEW_INTERVAL_S
}

fn transforms_selection(session: &TransformSession) -> bool {
    matches!(&session.source, TransformSessionSource::ExistingSurface)
        && session.selection_before.enabled
        && session
            .selection_before
            .material_mask(session.material_index)
            .and_then(|mask| mask.mask_id)
            .is_some()
}

fn cached_or_resolve_transform_source(state: &AppState) -> Result<Option<TransformSource>> {
    let Some(document) = state.document() else {
        return Ok(None);
    };
    let Some(target) = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
        .into_allowed()
        .map(|target| target.target())
    else {
        return Ok(None);
    };
    let Some(surface_revision) = document.tiles.surface_revision(target) else {
        return Ok(None);
    };
    let selection_revision = document.selection_masks.revision();
    if let Some(source_bounds) = state.tool.cached_transform_idle_bounds(
        target,
        surface_revision,
        selection_revision,
        &document.active_selection,
    ) {
        let texture_size = document
            .texture_size_for_surface(target)
            .ok_or_else(|| anyhow!("Transform target texture size is unavailable"))?;
        return Ok(source_bounds.map(|source_bounds| TransformSource {
            target,
            material_index: target.material_index(),
            texture_size,
            source_bounds,
            selection_before: document.active_selection.clone(),
        }));
    }

    let source = resolve_transform_source(state)?;
    state.tool.cache_transform_idle_bounds(
        target,
        surface_revision,
        selection_revision,
        document.active_selection.clone(),
        source.as_ref().map(|source| source.source_bounds),
    );
    Ok(source)
}

fn resolve_transform_source(state: &AppState) -> Result<Option<TransformSource>> {
    let Some(document) = state.document() else {
        return Ok(None);
    };
    let Some(target) = state
        .document
        .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
        .into_allowed()
        .map(|target| target.target())
    else {
        return Ok(None);
    };
    let snapshot = document
        .tiles
        .read_surface_full(target)
        .map_err(|err| anyhow!("Transform source snapshot failed: {err:#}"))?;
    let material_index = target.material_index();
    let selection_before = document.active_selection.clone();
    let selection_r8 = if selection_before.enabled {
        let material_mask = selection_before.material_mask(material_index.into());
        if material_mask.and_then(|mask| mask.mask_id).is_none() {
            return Ok(None);
        }
        Some(
            document
                .selection_masks
                .mask_bytes_or_zeros(material_index.into(), snapshot.texture_size)
                .map_err(|err| anyhow!("Transform selection snapshot failed: {err:#}"))?,
        )
    } else {
        None
    };
    let Some(source_bounds) = content_bounds(
        target.is_mask(),
        snapshot.texture_size,
        &snapshot.rgba8,
        selection_r8.as_deref(),
    ) else {
        return Ok(None);
    };
    Ok(Some(TransformSource {
        target,
        material_index,
        texture_size: snapshot.texture_size,
        source_bounds,
        selection_before,
    }))
}

fn content_bounds(
    target_is_mask: bool,
    texture_size: [u32; 2],
    rgba8: &[u8],
    selection_r8: Option<&[u8]>,
) -> Option<RectU32> {
    let width = texture_size[0] as usize;
    let height = texture_size[1] as usize;
    if rgba8.len() != width.saturating_mul(height).saturating_mul(4) {
        return None;
    }
    if selection_r8.is_some_and(|selection| selection.len() != width.saturating_mul(height)) {
        return None;
    }
    let mut min_x = texture_size[0];
    let mut min_y = texture_size[1];
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let pixel_index = y as usize * width + x as usize;
            let included = match selection_r8 {
                Some(selection) => selection[pixel_index] != 0,
                None => {
                    let offset = pixel_index * 4;
                    if target_is_mask {
                        rgba8[offset] != 0
                    } else {
                        rgba8[offset + 3] != 0
                    }
                }
            };
            if !included {
                continue;
            }
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + 1);
            max_y = max_y.max(y + 1);
        }
    }
    found.then(|| RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

fn transform_from_gesture(
    source_bounds: RectU32,
    gesture: TransformGesture,
    current_px: Vec2,
    modifiers: InputModifiers,
    scale_constraint: TransformScaleConstraint,
) -> TransformGestureUpdate {
    match gesture.handle {
        TransformHandle::Move => TransformGestureUpdate {
            transform: UvTransform::from_matrix(
                Mat3::from_translation(current_px - gesture.start_texture_px)
                    * gesture.base_transform.matrix(),
            )
            .unwrap_or(gesture.base_transform),
            scale_sign: gesture.base_scale_sign,
        },
        TransformHandle::Rotate => {
            let pivot_source = rect_center(source_bounds);
            let pivot_world = gesture.base_transform.transform_point(pivot_source);
            let start = gesture.start_texture_px - pivot_world;
            let current = current_px - pivot_world;
            if start.length_squared() <= 1.0e-6 || current.length_squared() <= 1.0e-6 {
                return TransformGestureUpdate {
                    transform: gesture.base_transform,
                    scale_sign: gesture.base_scale_sign,
                };
            }
            let mut rotation_radians = current.y.atan2(current.x) - start.y.atan2(start.x);
            if modifiers.shift {
                const SNAP_RADIANS: f32 = std::f32::consts::PI / 12.0;
                rotation_radians = (rotation_radians / SNAP_RADIANS).round() * SNAP_RADIANS;
            }
            TransformGestureUpdate {
                transform: UvTransform::from_matrix(
                    Mat3::from_translation(pivot_world)
                        * Mat3::from_angle(rotation_radians)
                        * Mat3::from_translation(-pivot_world)
                        * gesture.base_transform.matrix(),
                )
                .unwrap_or(gesture.base_transform),
                scale_sign: gesture.base_scale_sign,
            }
        }
        handle => {
            let Some(inverse_base) = gesture.base_transform.inverse_matrix() else {
                return TransformGestureUpdate {
                    transform: gesture.base_transform,
                    scale_sign: gesture.base_scale_sign,
                };
            };
            let start_local = (inverse_base * gesture.start_texture_px.extend(1.0)).truncate();
            let current_local = (inverse_base * current_px.extend(1.0)).truncate();
            let anchor = scale_anchor(source_bounds, handle, modifiers.alt);
            let mut raw_scale = Vec2::ONE;
            if handle.affects_x() {
                let start_delta = start_local.x - anchor.x;
                if start_delta.abs() > 1.0e-4 {
                    raw_scale.x = (current_local.x - anchor.x) / start_delta;
                }
            }
            if handle.affects_y() {
                let start_delta = start_local.y - anchor.y;
                if start_delta.abs() > 1.0e-4 {
                    raw_scale.y = (current_local.y - anchor.y) / start_delta;
                }
            }
            let preserve_aspect =
                modifiers.shift || scale_constraint == TransformScaleConstraint::PreserveAspect;
            let scale = if preserve_aspect {
                let uniform = if handle.affects_x() && handle.affects_y() {
                    if (raw_scale.x - 1.0).abs() >= (raw_scale.y - 1.0).abs() {
                        raw_scale.x
                    } else {
                        raw_scale.y
                    }
                } else if handle.affects_x() {
                    raw_scale.x
                } else {
                    raw_scale.y
                };
                let uniform_magnitude = uniform.abs().max(MIN_SCALE);
                let mut scale = Vec2::splat(uniform_magnitude);
                if handle.affects_x() {
                    scale.x = signed_magnitude(raw_scale.x, uniform_magnitude);
                }
                if handle.affects_y() {
                    scale.y = signed_magnitude(raw_scale.y, uniform_magnitude);
                }
                scale
            } else {
                Vec2::new(
                    if handle.affects_x() {
                        clamp_signed_magnitude(raw_scale.x, MIN_SCALE, 1.0)
                    } else {
                        1.0
                    },
                    if handle.affects_y() {
                        clamp_signed_magnitude(raw_scale.y, MIN_SCALE, 1.0)
                    } else {
                        1.0
                    },
                )
            };
            let local_scale = Mat3::from_translation(anchor)
                * Mat3::from_scale(scale)
                * Mat3::from_translation(-anchor);
            let Some(transform) =
                UvTransform::from_matrix(gesture.base_transform.matrix() * local_scale)
            else {
                return TransformGestureUpdate {
                    transform: gesture.base_transform,
                    scale_sign: gesture.base_scale_sign,
                };
            };
            TransformGestureUpdate {
                transform,
                scale_sign: gesture.base_scale_sign * scale_sign_for_size(scale),
            }
        }
    }
}

fn scale_anchor(bounds: RectU32, handle: TransformHandle, from_center: bool) -> Vec2 {
    let origin = Vec2::new(bounds.origin[0] as f32, bounds.origin[1] as f32);
    let end = origin + Vec2::new(bounds.size[0] as f32, bounds.size[1] as f32);
    let center = (origin + end) * 0.5;
    if from_center {
        return center;
    }
    match handle {
        TransformHandle::ScaleLeft => Vec2::new(end.x, center.y),
        TransformHandle::ScaleRight => Vec2::new(origin.x, center.y),
        TransformHandle::ScaleTop => Vec2::new(center.x, end.y),
        TransformHandle::ScaleBottom => Vec2::new(center.x, origin.y),
        TransformHandle::ScaleTopLeft => end,
        TransformHandle::ScaleTopRight => Vec2::new(origin.x, end.y),
        TransformHandle::ScaleBottomLeft => Vec2::new(end.x, origin.y),
        TransformHandle::ScaleBottomRight => origin,
        TransformHandle::Move | TransformHandle::Rotate => center,
    }
}

fn transform_damage(session: &TransformSession) -> DamageMap {
    let transformed = transformed_bounds(
        session.source_bounds,
        session.transform,
        session.texture_size,
    );
    let mut damage = DamageMap::default();
    if matches!(&session.source, TransformSessionSource::ExistingSurface) {
        damage.add_rect(
            session.target,
            expand_rect(session.texture_size, session.source_bounds, DAMAGE_GUARD_PX),
        );
    }
    if let Some(transformed) = transformed {
        damage.add_rect(
            session.target,
            expand_rect(session.texture_size, transformed, DAMAGE_GUARD_PX),
        );
    }
    damage
}

fn embedded_transform_damage(
    session: &EmbeddedImageTransformSession,
    previous: UvTransform,
    current: UvTransform,
) -> Option<RectU32> {
    let previous = transformed_bounds(session.source_bounds(), previous, session.texture_size)
        .map(|rect| expand_rect(session.texture_size, rect, DAMAGE_GUARD_PX));
    let current = transformed_bounds(session.source_bounds(), current, session.texture_size)
        .map(|rect| expand_rect(session.texture_size, rect, DAMAGE_GUARD_PX));
    match (previous, current) {
        (Some(previous), Some(current)) => Some(union_rect(previous, current)),
        (Some(rect), None) | (None, Some(rect)) => Some(rect),
        (None, None) => None,
    }
}

fn transformed_bounds(
    bounds: RectU32,
    transform: UvTransform,
    target_texture_size: [u32; 2],
) -> Option<RectU32> {
    let origin = Vec2::new(bounds.origin[0] as f32, bounds.origin[1] as f32);
    let end = origin + Vec2::new(bounds.size[0] as f32, bounds.size[1] as f32);
    let corners = [
        origin,
        Vec2::new(end.x, origin.y),
        end,
        Vec2::new(origin.x, end.y),
    ]
    .map(|point| transform.transform_point(point));
    if corners.iter().any(|point| !point.is_finite()) {
        return None;
    }
    let min = corners
        .iter()
        .copied()
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = corners
        .iter()
        .copied()
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    let min_x = min.x.floor() as i64;
    let min_y = min.y.floor() as i64;
    let max_x = max.x.ceil() as i64;
    let max_y = max.y.ceil() as i64;
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    let clipped_min_x = min_x.max(0).min(target_texture_size[0] as i64);
    let clipped_min_y = min_y.max(0).min(target_texture_size[1] as i64);
    let clipped_max_x = max_x.max(0).min(target_texture_size[0] as i64);
    let clipped_max_y = max_y.max(0).min(target_texture_size[1] as i64);
    (clipped_max_x > clipped_min_x && clipped_max_y > clipped_min_y).then_some(RectU32 {
        origin: [clipped_min_x as u32, clipped_min_y as u32],
        size: [
            (clipped_max_x - clipped_min_x) as u32,
            (clipped_max_y - clipped_min_y) as u32,
        ],
    })
}

fn expand_rect(texture_size: [u32; 2], rect: RectU32, guard: u32) -> RectU32 {
    let min_x = rect.origin[0].saturating_sub(guard);
    let min_y = rect.origin[1].saturating_sub(guard);
    let max_x = rect.origin[0]
        .saturating_add(rect.size[0])
        .saturating_add(guard)
        .min(texture_size[0]);
    let max_y = rect.origin[1]
        .saturating_add(rect.size[1])
        .saturating_add(guard)
        .min(texture_size[1]);
    RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    }
}

fn hit_test_handle(
    bounds: RectU32,
    texture_size: [u32; 2],
    transform: UvTransform,
    pointer_screen_px: Vec2,
    view: crate::application::UvViewInputContext,
) -> TransformHandle {
    let preview = preview_for_bounds(bounds, texture_size, transform, None);
    let corners = preview.corners_uv.map(|point| {
        view.transform
            .uv_to_view_px(point, view.size, view.canvas_size)
    });
    hit_test_transform_handle(pointer_screen_px, corners, HANDLE_HIT_RADIUS_PX)
}

fn preview_for_bounds(
    bounds: RectU32,
    texture_size: [u32; 2],
    transform: UvTransform,
    active_handle: Option<TransformHandle>,
) -> UvTransformPreview {
    let origin = Vec2::new(bounds.origin[0] as f32, bounds.origin[1] as f32);
    let end = origin + Vec2::new(bounds.size[0] as f32, bounds.size[1] as f32);
    let texture = Vec2::new(texture_size[0].max(1) as f32, texture_size[1].max(1) as f32);
    UvTransformPreview {
        corners_uv: [
            origin,
            Vec2::new(end.x, origin.y),
            end,
            Vec2::new(origin.x, end.y),
        ]
        .map(|point| transform.transform_point(point) / texture),
        pivot_uv: transform.transform_point(rect_center(bounds)) / texture,
        active_handle,
    }
}

fn rect_center(rect: RectU32) -> Vec2 {
    Vec2::new(
        rect.origin[0] as f32 + rect.size[0] as f32 * 0.5,
        rect.origin[1] as f32 + rect.size[1] as f32 * 0.5,
    )
}

fn uv_to_texture_px(uv: Vec2, texture_size: [u32; 2]) -> Vec2 {
    uv * Vec2::new(texture_size[0].max(1) as f32, texture_size[1].max(1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_transformable_pixel() -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(crate::core::document::Document::new(
            crate::core::document::MeshData::empty(),
            vec![crate::core::document::MaterialSpec::new("A", [8, 8])],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let layer = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let surface = PaintSurfaceId::raster(0.into(), layer);
        state
            .document_mut()
            .unwrap()
            .tiles
            .write_surface_rect(
                surface,
                RectU32 {
                    origin: [2, 2],
                    size: [1, 1],
                },
                &crate::application::PixelSnapshotData::contiguous(vec![255; 4]),
            )
            .unwrap();
        state
    }

    fn bounds() -> RectU32 {
        RectU32 {
            origin: [2, 2],
            size: [2, 2],
        }
    }

    fn raster_surface() -> crate::core::surface::PaintSurfaceId {
        let mut layers = slotmap::SlotMap::<crate::core::surface::LayerId, ()>::with_key();
        crate::core::surface::PaintSurfaceId::raster(0.into(), layers.insert(()))
    }

    fn session() -> TransformSession {
        let transform = UvTransform::identity();
        TransformSession {
            target: raster_surface(),
            material_index: 0.into(),
            texture_size: [8, 8],
            source_bounds: bounds(),
            source: TransformSessionSource::ExistingSurface,
            selection_before: ActiveSelection::disabled_for_materials([0.into()]),
            transform,
            scale_sign: Vec2::ONE,
            scale_constraint: TransformScaleConstraint::Free,
            gesture: Some(TransformGesture {
                handle: TransformHandle::ScaleBottomRight,
                start_texture_px: Vec2::new(4.0, 4.0),
                base_transform: transform,
                base_scale_sign: Vec2::ONE,
            }),
            last_preview_time_s: None,
            last_submitted_transform: None,
        }
    }

    fn translated(delta: Vec2) -> UvTransform {
        UvTransform::from_matrix(Mat3::from_translation(delta)).unwrap()
    }

    #[test]
    fn empty_surface_has_no_content_bounds() {
        let rgba = vec![0; 8 * 8 * 4];
        assert_eq!(content_bounds(false, [8, 8], &rgba, None), None);
    }

    #[test]
    fn alpha_bounds_ignore_transparent_pixels() {
        let mut rgba = vec![0; 8 * 8 * 4];
        rgba[(3 * 8 + 4) * 4 + 3] = 255;
        let rect = content_bounds(false, [8, 8], &rgba, None).unwrap();
        assert_eq!(rect.origin, [4, 3]);
        assert_eq!(rect.size, [1, 1]);
    }

    #[test]
    fn move_keeps_existing_scale_sign() {
        let base_transform = UvTransform::from_numeric_values(
            Vec2::new(2.0, 2.0),
            Vec2::splat(2.0),
            TransformNumericValues {
                center_px: Vec2::new(3.0, 3.0),
                size_px: Vec2::new(-2.0, 2.0),
                rotation_degrees: 0.0,
            },
        )
        .unwrap();
        let update = transform_from_gesture(
            bounds(),
            TransformGesture {
                handle: TransformHandle::Move,
                start_texture_px: Vec2::new(3.0, 3.0),
                base_transform,
                base_scale_sign: Vec2::new(-1.0, 1.0),
            },
            Vec2::new(4.0, 3.0),
            InputModifiers::default(),
            TransformScaleConstraint::Free,
        );
        let values = numeric_values_for(bounds(), update.transform, update.scale_sign);

        assert_eq!(update.scale_sign, Vec2::new(-1.0, 1.0));
        assert!(values.center_px.abs_diff_eq(Vec2::new(4.0, 3.0), 1.0e-5));
        assert!(values.size_px.abs_diff_eq(Vec2::new(-2.0, 2.0), 1.0e-5));
    }

    #[test]
    fn free_scale_allows_independent_axes() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(5.0, 4.5),
            InputModifiers::default(),
            TransformScaleConstraint::Free,
        );
        let transformed = update.transform.transform_point(Vec2::new(4.0, 4.0));
        assert!((transformed - Vec2::new(5.0, 4.5)).length() < 1.0e-5);
    }

    #[test]
    fn shift_scale_uses_uniform_factor() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(5.0, 4.5),
            InputModifiers {
                shift: true,
                ..InputModifiers::default()
            },
            TransformScaleConstraint::Free,
        );
        let transformed = update.transform.transform_point(Vec2::new(4.0, 4.0));
        assert!((transformed - Vec2::new(5.0, 5.0)).length() < 1.0e-5);
    }

    #[test]
    fn preserve_aspect_scale_uses_uniform_factor_without_shift() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(5.0, 4.5),
            InputModifiers::default(),
            TransformScaleConstraint::PreserveAspect,
        );
        let transformed = update.transform.transform_point(Vec2::new(4.0, 4.0));
        assert!((transformed - Vec2::new(5.0, 5.0)).length() < 1.0e-5);
    }

    #[test]
    fn free_scale_flips_each_axis_after_crossing_the_anchor() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(1.0, 5.0),
            InputModifiers::default(),
            TransformScaleConstraint::Free,
        );

        assert_eq!(update.scale_sign, Vec2::new(-1.0, 1.0));
        assert!(
            (update.transform.transform_point(Vec2::new(4.0, 4.0)) - Vec2::new(1.0, 5.0)).length()
                < 1.0e-5
        );
    }

    #[test]
    fn preserve_aspect_keeps_crossed_axis_sign_independent() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(1.0, 5.0),
            InputModifiers::default(),
            TransformScaleConstraint::PreserveAspect,
        );

        assert_eq!(update.scale_sign, Vec2::new(-1.0, 1.0));
        assert!(
            (update.transform.transform_point(Vec2::new(4.0, 4.0)) - Vec2::new(1.0, 3.0)).length()
                < 1.0e-5
        );
    }

    #[test]
    fn scale_keeps_the_opposite_anchor_after_a_move() {
        let base_transform = translated(Vec2::X);
        let gesture = TransformGesture {
            handle: TransformHandle::ScaleBottomRight,
            start_texture_px: base_transform.transform_point(Vec2::new(4.0, 4.0)),
            base_transform,
            base_scale_sign: Vec2::ONE,
        };
        let update = transform_from_gesture(
            bounds(),
            gesture,
            Vec2::new(7.0, 6.0),
            InputModifiers::default(),
            TransformScaleConstraint::Free,
        );

        assert!(
            (update.transform.transform_point(Vec2::new(2.0, 2.0)) - Vec2::new(3.0, 2.0)).length()
                < 1.0e-5
        );
        assert!(
            (update.transform.transform_point(Vec2::new(4.0, 4.0)) - Vec2::new(7.0, 6.0)).length()
                < 1.0e-5
        );
    }

    #[test]
    fn alt_scale_uses_the_source_center_as_anchor() {
        let session = session();
        let update = transform_from_gesture(
            session.source_bounds,
            session.gesture.unwrap(),
            Vec2::new(5.0, 5.0),
            InputModifiers {
                alt: true,
                ..InputModifiers::default()
            },
            TransformScaleConstraint::Free,
        );

        assert!(
            (update.transform.transform_point(Vec2::new(3.0, 3.0)) - Vec2::new(3.0, 3.0)).length()
                < 1.0e-5
        );
    }

    #[test]
    fn begin_session_without_gesture_emits_only_begin_for_identity() {
        let mut state = AppState::default();
        let target = raster_surface();
        let source = TransformSource {
            target,
            material_index: 0.into(),
            texture_size: [8, 8],
            source_bounds: bounds(),
            selection_before: ActiveSelection::disabled_for_materials([0.into()]),
        };

        let output = begin_transform_session(
            &mut state,
            TransformBeginSpec {
                source,
                initial_transform: UvTransform::identity(),
                scale_constraint: TransformScaleConstraint::PreserveAspect,
            },
        )
        .unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert_eq!(plan.edit_commands.len(), 1);
        assert!(matches!(
            &plan.edit_commands[0],
            EditCommand::Transform(TransformCommand::Begin { .. })
        ));
        let session = state.tool.transform_session().unwrap();
        assert!(session.gesture.is_none());
        assert_eq!(
            session.scale_constraint,
            TransformScaleConstraint::PreserveAspect
        );
        assert_eq!(session.last_submitted_transform, None);
    }

    #[test]
    fn begin_session_with_initial_transform_emits_begin_and_preview() {
        let mut state = AppState::default();
        let target = raster_surface();
        let initial_transform = translated(Vec2::X);
        let source = TransformSource {
            target,
            material_index: 0.into(),
            texture_size: [8, 8],
            source_bounds: bounds(),
            selection_before: ActiveSelection::disabled_for_materials([0.into()]),
        };

        let output = begin_transform_session(
            &mut state,
            TransformBeginSpec {
                source,
                initial_transform,
                scale_constraint: TransformScaleConstraint::Free,
            },
        )
        .unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert_eq!(plan.edit_commands.len(), 2);
        assert!(matches!(
            &plan.edit_commands[0],
            EditCommand::Transform(TransformCommand::Begin { .. })
        ));
        assert!(matches!(
            &plan.edit_commands[1],
            EditCommand::Transform(TransformCommand::Preview { .. })
        ));
        assert_eq!(
            state
                .tool
                .transform_session()
                .unwrap()
                .last_submitted_transform,
            Some(initial_transform)
        );
    }

    #[test]
    fn numeric_edit_updates_active_session_and_emits_preview() {
        let mut state = AppState::default();
        let mut active = session();
        active.gesture = None;
        state.tool.set_session(ToolSession::Transform(active));

        let output =
            update_transform_numeric(&mut state, TransformNumericEdit::CenterX(10.0)).unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);
        let values = transform_numeric_values(&state).unwrap();

        assert_eq!(plan.edit_commands.len(), 1);
        assert!(matches!(
            &plan.edit_commands[0],
            EditCommand::Transform(TransformCommand::Preview { .. })
        ));
        assert!((values.center_px.x - 10.0).abs() < 1.0e-5);
        assert!((values.center_px.y - 3.0).abs() < 1.0e-5);
    }

    #[test]
    fn numeric_edit_starts_idle_session_and_emits_begin_before_preview() {
        let mut state = state_with_transformable_pixel();

        let output =
            update_transform_numeric(&mut state, TransformNumericEdit::CenterX(6.0)).unwrap();
        let plan = output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert!(state.tool.transform_session().is_some());
        assert_eq!(plan.edit_commands.len(), 2);
        assert!(matches!(
            &plan.edit_commands[0],
            EditCommand::Transform(TransformCommand::Begin { .. })
        ));
        assert!(matches!(
            &plan.edit_commands[1],
            EditCommand::Transform(TransformCommand::Preview { .. })
        ));
        assert!((transform_numeric_values(&state).unwrap().center_px.x - 6.0).abs() < 1.0e-5);
    }

    #[test]
    fn numeric_width_preserves_source_aspect_when_locked() {
        let mut state = AppState::default();
        let mut active = session();
        active.gesture = None;
        active.scale_constraint = TransformScaleConstraint::PreserveAspect;
        state.tool.set_session(ToolSession::Transform(active));

        let _ = update_transform_numeric(&mut state, TransformNumericEdit::Width(5.0)).unwrap();
        let values = transform_numeric_values(&state).unwrap();

        assert!(values.size_px.abs_diff_eq(Vec2::splat(5.0), 1.0e-5));
    }

    #[test]
    fn numeric_width_accepts_negative_values_and_preserves_the_sign() {
        let mut state = AppState::default();
        let mut active = session();
        active.gesture = None;
        state.tool.set_session(ToolSession::Transform(active));

        let output =
            update_transform_numeric(&mut state, TransformNumericEdit::Width(-5.0)).unwrap();
        let values = transform_numeric_values(&state).unwrap();

        assert!(output.has_renderer_work());
        assert!(values.size_px.abs_diff_eq(Vec2::new(-5.0, 2.0), 1.0e-5));
        assert_eq!(
            state.tool.transform_session().unwrap().scale_sign,
            Vec2::new(-1.0, 1.0)
        );
    }

    #[test]
    fn numeric_aspect_lock_preserves_the_other_axis_sign() {
        let mut state = AppState::default();
        let mut active = session();
        active.gesture = None;
        active.scale_constraint = TransformScaleConstraint::PreserveAspect;
        state.tool.set_session(ToolSession::Transform(active));

        let _ = update_transform_numeric(&mut state, TransformNumericEdit::Width(-5.0)).unwrap();
        let width_values = transform_numeric_values(&state).unwrap();
        let _ = update_transform_numeric(&mut state, TransformNumericEdit::Height(-4.0)).unwrap();
        let height_values = transform_numeric_values(&state).unwrap();

        assert!(
            width_values
                .size_px
                .abs_diff_eq(Vec2::new(-5.0, 5.0), 1.0e-5)
        );
        assert!(
            height_values
                .size_px
                .abs_diff_eq(Vec2::new(-4.0, -4.0), 1.0e-5)
        );
    }

    #[test]
    fn numeric_zero_uses_the_existing_axis_sign_at_minimum_size() {
        let mut state = AppState::default();
        let mut active = session();
        active.gesture = None;
        state.tool.set_session(ToolSession::Transform(active));

        let _ = update_transform_numeric(&mut state, TransformNumericEdit::Width(-5.0)).unwrap();
        let _ = update_transform_numeric(&mut state, TransformNumericEdit::Width(0.0)).unwrap();
        let values = transform_numeric_values(&state).unwrap();

        assert!((values.size_px.x + 0.02).abs() < 1.0e-5);
        assert_eq!(state.tool.transform_session().unwrap().scale_sign.x, -1.0);
    }

    #[test]
    fn numeric_edit_ignores_non_finite_values_and_active_gestures() {
        let mut state = AppState::default();
        let active = session();
        let before = active.transform;
        state.tool.set_session(ToolSession::Transform(active));

        let gesture_output =
            update_transform_numeric(&mut state, TransformNumericEdit::CenterX(9.0)).unwrap();
        let nan_output =
            update_transform_numeric(&mut state, TransformNumericEdit::CenterX(f32::NAN)).unwrap();

        assert!(!gesture_output.has_renderer_work());
        assert!(!nan_output.has_renderer_work());
        assert_eq!(state.tool.transform_session().unwrap().transform, before);
    }

    #[test]
    fn gesture_cancel_restores_transform_and_scale_sign() {
        let mut active = session();
        let base_transform = active.gesture.unwrap().base_transform;
        active.gesture = Some(TransformGesture {
            base_scale_sign: Vec2::new(-1.0, 1.0),
            ..active.gesture.unwrap()
        });
        active.transform = translated(Vec2::X);
        active.scale_sign = Vec2::ONE;

        let _ = finish_transform_gesture(&mut active, TransformGestureFinish::RestoreBase);

        assert_eq!(active.transform, base_transform);
        assert_eq!(active.scale_sign, Vec2::new(-1.0, 1.0));
        assert!(active.gesture.is_none());
    }

    #[test]
    fn discard_cancel_returns_session_and_emits_discard() {
        let mut state = AppState::default();
        let expected = session();
        state
            .tool
            .set_session(ToolSession::Transform(expected.clone()));

        let finished = cancel_active_transform_with_action(
            &mut state,
            TransformCancelRendererAction::DiscardSession,
        )
        .unwrap();
        let plan = finished
            .output
            .into_renderer_plan()
            .into_frame_plan(Vec::new(), None);

        assert_eq!(finished.session, expected);
        assert!(state.tool.is_idle());
        assert_eq!(plan.edit_commands.len(), 1);
        assert!(matches!(
            &plan.edit_commands[0],
            EditCommand::Transform(TransformCommand::Discard)
        ));
    }

    #[test]
    fn preview_updates_are_throttled_but_zero_timestamps_remain_testable() {
        let mut session = session();
        session.transform = translated(Vec2::X);
        assert!(should_submit_preview(&session, 1.0));
        session.last_preview_time_s = Some(1.0);
        session.last_submitted_transform = Some(session.transform);
        assert!(!should_submit_preview(&session, 1.01));
        session.transform = translated(Vec2::new(2.0, 0.0));
        assert!(!should_submit_preview(&session, 1.01));
        assert!(should_submit_preview(&session, 0.0));
        assert!(should_submit_preview(&session, 1.04));
    }

    #[test]
    fn damage_covers_original_and_translated_bounds() {
        let mut session = session();
        session.transform = translated(Vec2::new(3.0, 1.0));
        let damage = transform_damage(&session);
        assert_eq!(damage.pixels.len(), 1);
        let rect = damage.pixels[0].rect;
        assert_eq!(rect.origin, [0, 0]);
        assert_eq!(rect.size, [8, 7]);
    }
}
