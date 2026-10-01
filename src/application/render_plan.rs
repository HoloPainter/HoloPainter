use anyhow::{Result, bail};

use std::sync::Arc;

use crate::{
    application::{
        PendingRendererHistoryTransaction, RendererCommitSpec,
        history_capture::PixelHistoryCapture, state::AppState,
    },
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        damage::DamageMap,
        mask::MaskSource,
        selection::ActiveSelection,
        stroke::{PaintSurfaceSet, StrokeContext, StrokeRenderDescriptor},
        stroke_style::ResolvedStrokeStyle,
        surface::PaintSurfaceId,
        tool_operation::{PaintSource, ToolOperation},
    },
    renderer::{
        ApplyCommand, ApplyOperation, CommitRequest, EditCommand, FilterCommand,
        GpuDocumentCommand, RendererFramePlan, RendererStrokeOperation, RendererStrokeStyle,
        StrokeCommand, StrokeDabPayload, StrokeTarget, ViewRequest,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct ApplyOneShotRenderPlan {
    history_label: &'static str,
    target: PaintSurfaceId,
    surfaces: Vec<PaintSurfaceId>,
    mask: MaskSource,
    operation: ToolOperation,
    params: ApplyParams<TextureCompositeMode>,
    active_selection: ActiveSelection,
    damage: Option<DamageMap>,
    history_capture: Option<PixelHistoryCapture>,
}

impl ApplyOneShotRenderPlan {
    pub(crate) fn new(
        history_label: &'static str,
        target: PaintSurfaceId,
        surfaces: Vec<PaintSurfaceId>,
        mask: MaskSource,
        operation: ToolOperation,
        params: ApplyParams<TextureCompositeMode>,
        active_selection: ActiveSelection,
        damage: Option<DamageMap>,
        history_capture: Option<PixelHistoryCapture>,
    ) -> Self {
        Self {
            history_label,
            target,
            surfaces,
            mask,
            operation,
            params,
            active_selection,
            damage,
            history_capture,
        }
    }

    pub(crate) fn renderer_commit(&self) -> RendererCommitSpec {
        RendererCommitSpec::with_finalized_surfaces(self.surfaces.clone())
    }

    pub(crate) fn history_label(&self) -> &'static str {
        self.history_label
    }

    pub(crate) fn into_edit_command_and_history(
        self,
        state: &AppState,
    ) -> Result<(EditCommand, Option<PendingRendererHistoryTransaction>)> {
        let Self {
            history_label: _,
            target,
            surfaces,
            mask,
            operation,
            params,
            active_selection,
            damage,
            history_capture,
        } = self;
        let history_transaction = match history_capture {
            Some(capture) => capture.capture(state)?,
            None => None,
        };
        let Some(operation) = apply_operation_from_tool_operation(&operation) else {
            bail!("unsupported one-shot apply operation");
        };
        Ok((
            EditCommand::Apply(ApplyCommand::OneShot {
                target,
                surfaces,
                mask,
                operation,
                params,
                active_selection,
                damage,
            }),
            history_transaction,
        ))
    }
}

#[derive(Debug, Default)]
pub(crate) struct RendererPlanBuilder {
    document_commands: Vec<GpuDocumentCommand>,
    edit_commands: Vec<EditCommand>,
}

impl RendererPlanBuilder {
    pub(crate) fn clear(&mut self) {
        self.document_commands.clear();
        self.edit_commands.clear();
    }

    pub(crate) fn push_document_command(&mut self, command: GpuDocumentCommand) {
        self.document_commands.push(command);
    }

    pub(crate) fn push_edit_command(&mut self, command: EditCommand) {
        self.push_edit_command_unrecorded(command);
    }

    fn push_edit_command_unrecorded(&mut self, command: EditCommand) {
        let command = if let Some(last) = self.edit_commands.last_mut() {
            match merge_edit_command(last, command) {
                Ok(()) => return,
                Err(command) => command,
            }
        } else {
            command
        };
        self.edit_commands.push(command);
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.document_commands.append(&mut other.document_commands);
        for command in other.edit_commands {
            self.push_edit_command_unrecorded(command);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.document_commands.is_empty() && self.edit_commands.is_empty()
    }

    pub(crate) fn drain_into_frame_plan(
        &mut self,
        requests: Vec<ViewRequest>,
        commit_request: Option<CommitRequest>,
    ) -> RendererFramePlan {
        let builder = std::mem::take(self);
        builder.into_frame_plan(requests, commit_request)
    }

    pub(crate) fn into_frame_plan(
        self,
        requests: Vec<ViewRequest>,
        commit_request: Option<CommitRequest>,
    ) -> RendererFramePlan {
        RendererFramePlan {
            document_commands: self.document_commands,
            edit_commands: self.edit_commands,
            view_requests: requests,
            color_sample_request: None,
            commit_request,
        }
    }
}

pub(crate) trait StrokeRenderPlanSink {
    fn push_stroke_begin(&mut self, stroke: Arc<StrokeRenderDescriptor>);
    fn push_stroke_dabs(
        &mut self,
        dabs: &mut Vec<crate::core::stroke::StrokeDab>,
        preview_damage: Option<DamageMap>,
    );
    fn push_stroke_end(&mut self, _surfaces: PaintSurfaceSet, damage: Option<DamageMap>);
    fn push_surface_stroke_begin(&mut self, stroke: Arc<StrokeRenderDescriptor>);
    fn push_surface_dabs(
        &mut self,
        source_surfaces: PaintSurfaceSet,
        target_surfaces: PaintSurfaceSet,
        projection_batches: Vec<crate::renderer::command::SurfaceProjectionDabBatch>,
        preview_damage: Option<DamageMap>,
    );
    fn push_surface_stroke_end(&mut self, _surfaces: PaintSurfaceSet, damage: Option<DamageMap>);
}

impl StrokeRenderPlanSink for RendererPlanBuilder {
    fn push_stroke_begin(&mut self, stroke: Arc<StrokeRenderDescriptor>) {
        self.push_edit_command(stroke_begin_command(
            stroke_target(&stroke),
            Arc::clone(&stroke.style),
            Arc::clone(&stroke.active_selection),
        ));
    }

    fn push_stroke_dabs(
        &mut self,
        dabs: &mut Vec<crate::core::stroke::StrokeDab>,
        preview_damage: Option<DamageMap>,
    ) {
        if dabs.is_empty() {
            return;
        }
        self.push_edit_command(stroke_add_dabs_command(
            StrokeDabPayload::Stroke(std::mem::take(dabs)),
            preview_damage,
        ));
    }

    fn push_stroke_end(&mut self, _surfaces: PaintSurfaceSet, damage: Option<DamageMap>) {
        self.push_edit_command(stroke_end_command(damage));
    }

    fn push_surface_stroke_begin(&mut self, stroke: Arc<StrokeRenderDescriptor>) {
        self.push_edit_command(stroke_begin_command(
            surface_stroke_target(&stroke),
            Arc::clone(&stroke.style),
            Arc::clone(&stroke.active_selection),
        ));
    }

    fn push_surface_dabs(
        &mut self,
        source_surfaces: PaintSurfaceSet,
        target_surfaces: PaintSurfaceSet,
        projection_batches: Vec<crate::renderer::command::SurfaceProjectionDabBatch>,
        preview_damage: Option<DamageMap>,
    ) {
        if projection_batches.is_empty() {
            return;
        }
        debug_assert!(
            projection_batches
                .iter()
                .all(|batch| { batch.target_materials_by_dab.len() == batch.dabs.len() })
        );
        self.push_edit_command(stroke_add_dabs_command(
            StrokeDabPayload::Surface {
                source_surfaces,
                target_surfaces,
                projection_batches,
            },
            preview_damage,
        ));
    }

    fn push_surface_stroke_end(&mut self, _surfaces: PaintSurfaceSet, damage: Option<DamageMap>) {
        self.push_edit_command(stroke_end_command(damage));
    }
}

fn stroke_begin_command(
    target: StrokeTarget,
    style: Arc<ResolvedStrokeStyle>,
    active_selection: Arc<ActiveSelection>,
) -> EditCommand {
    EditCommand::Stroke(StrokeCommand::Begin {
        target,
        style: Arc::new(renderer_stroke_style(style.as_ref())),
        active_selection,
    })
}

fn renderer_stroke_style(style: &ResolvedStrokeStyle) -> RendererStrokeStyle {
    let ToolOperation::BrushEngine(operation) = &style.operation else {
        unreachable!("stroke style must carry a brush operation");
    };
    RendererStrokeStyle {
        stroke_op: style.stroke_op.clone(),
        operation: RendererStrokeOperation::BrushEngine {
            color: operation.color,
            params: operation.params.clone(),
        },
    }
}

fn stroke_add_dabs_command(
    dabs: StrokeDabPayload,
    preview_damage: Option<DamageMap>,
) -> EditCommand {
    EditCommand::Stroke(StrokeCommand::AddDabs {
        dabs,
        preview_damage,
    })
}

fn stroke_end_command(damage: Option<crate::core::damage::DamageMap>) -> EditCommand {
    EditCommand::Stroke(StrokeCommand::End { damage })
}

fn stroke_target(stroke: &StrokeRenderDescriptor) -> StrokeTarget {
    match stroke.context.as_ref() {
        StrokeContext::Uv(_) => StrokeTarget::Uv {
            surface: stroke.target,
        },
        StrokeContext::Surface(_) => StrokeTarget::Surface {
            surfaces: stroke.surfaces.clone(),
            context: Arc::clone(&stroke.context),
        },
    }
}

fn surface_stroke_target(stroke: &StrokeRenderDescriptor) -> StrokeTarget {
    StrokeTarget::Surface {
        surfaces: stroke.surfaces.clone(),
        context: Arc::clone(&stroke.context),
    }
}

fn apply_operation_from_tool_operation(operation: &ToolOperation) -> Option<ApplyOperation> {
    match operation {
        ToolOperation::Paint(paint) => match &paint.source {
            PaintSource::SolidColor(color) => {
                Some(ApplyOperation::SolidColorPaint { color: *color })
            }
        },
        ToolOperation::BrushEngine(_) => None,
    }
}

fn merge_preview_damage(target: &mut Option<DamageMap>, next: Option<DamageMap>) {
    let Some(next) = next else {
        *target = None;
        return;
    };
    let Some(target) = target else {
        return;
    };
    for pixel in next.pixels {
        target.add_rect(pixel.surface, pixel.rect);
    }
}

fn merge_edit_command(last: &mut EditCommand, next: EditCommand) -> Result<(), EditCommand> {
    match (last, next) {
        (
            EditCommand::Filter(FilterCommand::AdjustmentPreviewUpdate {
                adjustment: last_adjustment,
            }),
            EditCommand::Filter(FilterCommand::AdjustmentPreviewUpdate { adjustment }),
        ) => {
            *last_adjustment = adjustment;
            Ok(())
        }
        (
            EditCommand::Stroke(StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(last_dabs),
                preview_damage: last_damage,
            }),
            EditCommand::Stroke(StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(mut dabs),
                preview_damage,
            }),
        ) => {
            last_dabs.append(&mut dabs);
            merge_preview_damage(last_damage, preview_damage);
            Ok(())
        }
        (
            EditCommand::Stroke(StrokeCommand::AddDabs {
                dabs:
                    StrokeDabPayload::Surface {
                        source_surfaces: last_source_surfaces,
                        target_surfaces: last_target_surfaces,
                        projection_batches: last_batches,
                    },
                preview_damage: last_damage,
            }),
            EditCommand::Stroke(StrokeCommand::AddDabs {
                dabs:
                    StrokeDabPayload::Surface {
                        source_surfaces,
                        target_surfaces,
                        mut projection_batches,
                    },
                preview_damage,
            }),
        ) => {
            if last_source_surfaces != &source_surfaces || last_target_surfaces != &target_surfaces
            {
                return Err(EditCommand::Stroke(StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Surface {
                        source_surfaces,
                        target_surfaces,
                        projection_batches,
                    },
                    preview_damage,
                }));
            }
            if let (Some(last), Some(first)) =
                (last_batches.last_mut(), projection_batches.first_mut())
                && last.projection_id == first.projection_id
                && first.continues_from_previous
            {
                last.dabs.append(&mut first.dabs);
                last.target_materials_by_dab
                    .append(&mut first.target_materials_by_dab);
                projection_batches.remove(0);
            }
            last_batches.append(&mut projection_batches);
            merge_preview_damage(last_damage, preview_damage);
            Ok(())
        }
        (_, next) => Err(next),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        core::adjustment::{Adjustment, BrightnessContrastAdjustment},
        renderer::{EditCommand, FilterCommand},
    };

    use super::merge_edit_command;

    #[test]
    fn adjustment_preview_updates_coalesce_to_the_latest_value() {
        let mut last = EditCommand::Filter(FilterCommand::AdjustmentPreviewUpdate {
            adjustment: Some(Adjustment::BrightnessContrast(
                BrightnessContrastAdjustment {
                    brightness: 10,
                    contrast: 0,
                },
            )),
        });

        assert!(
            merge_edit_command(
                &mut last,
                EditCommand::Filter(FilterCommand::AdjustmentPreviewUpdate { adjustment: None }),
            )
            .is_ok()
        );
        assert!(matches!(
            last,
            EditCommand::Filter(FilterCommand::AdjustmentPreviewUpdate { adjustment: None })
        ));
    }
}
