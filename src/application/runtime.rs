use std::collections::VecDeque;

use crate::application::command::CommandBlocker;
use crate::application::{
    Command, CompositeSync, EditorActionBlockReason, HistoryTransaction, PendingDocumentCommit,
    PendingEditTransaction, PendingHistoryAtom, PreparedRendererSubmission, ReducerOutput,
    RenderCommitArtifacts, RenderMetrics, RendererFinalizationPayload, RendererPlanBuilder,
    RendererSubmissionWork, SubmittedRendererTransaction, commit_finalizer, history_finalizer,
    reducer::reduce_to_outcome, state::AppState, stroke_controller::StrokeSampleScratch,
};
use crate::core::{material::MaterialIndex, surface::CompositeProps};
use crate::renderer::{
    CompletedRenderCommit, GpuDocumentCommand, RendererDiagnostic, StartedRenderCommit,
};

#[derive(Debug)]
pub struct ApplicationRuntime {
    pub state: AppState,
    last_effect_metrics: RenderMetrics,
    renderer_plan: RendererPlanBuilder,
    stroke_scratch: StrokeSampleScratch,
    pending_transaction: PendingEditTransaction,
    pending_render_finalizations: VecDeque<SubmittedRendererTransaction>,
    fatal_edit_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetachedTransactionPosition {
    Newest,
    Oldest,
}

impl ApplicationRuntime {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            last_effect_metrics: RenderMetrics::default(),
            renderer_plan: RendererPlanBuilder::default(),
            stroke_scratch: StrokeSampleScratch::default(),
            pending_transaction: PendingEditTransaction::default(),
            pending_render_finalizations: VecDeque::new(),
            fatal_edit_error: None,
        }
    }

    pub fn take_last_effect_metrics(&mut self) -> RenderMetrics {
        std::mem::take(&mut self.last_effect_metrics)
    }

    pub fn dispatch(&mut self, command: Command) -> Result<bool, String> {
        if let Some(error) = &self.fatal_edit_error
            && !command
                .policy()
                .allowed_during(CommandBlocker::FatalRendererError)
        {
            return Err(format!("Editing disabled after renderer failure: {error}"));
        }
        if (self.has_pending_render_finalizations()
            || self.pending_transaction.has_pending_renderer_finalization())
            && !command
                .policy()
                .allowed_during(CommandBlocker::RenderCommitPending)
        {
            let err = "Render commit pending; edit command ignored until GPU readback completes"
                .to_owned();
            self.state.set_status_key("status-render-commit-pending");
            return Err(err);
        }
        let clears_pending_transaction = command.policy().clears_pending_transaction();
        let outcome = reduce_to_outcome(&mut self.state, command, &mut self.stroke_scratch)
            .map_err(|err| self.handle_reducer_error(err))?;
        let project_changed = outcome.project_changed();
        if clears_pending_transaction {
            self.pending_transaction.clear();
        }
        self.apply_outcome(outcome)?;
        Ok(project_changed)
    }

    #[cfg(test)]
    pub(crate) fn has_pending_history_transaction(&self) -> bool {
        self.pending_transaction.has_pending_history_transaction()
    }

    fn apply_outcome(&mut self, mut outcome: ReducerOutput) -> Result<(), String> {
        if let Some(mut pending_transaction) = outcome.pending_transaction.take() {
            self.finalize_cpu_history_atoms(&mut pending_transaction)?;
            pending_transaction.seal_history_transactions();
            self.pending_transaction
                .append_transaction(pending_transaction);
        }
        self.renderer_plan.append(outcome.into_renderer_plan());
        match self.pending_transaction.take_composite_sync() {
            CompositeSync::None => {}
            CompositeSync::MaterialTrees => {
                material_composite_tree_sync_effects_into(&self.state, &mut self.renderer_plan);
            }
            CompositeSync::MaterialTreesScoped(material_indices) => {
                material_composite_tree_sync_effects_for_indices_into(
                    &self.state,
                    material_indices,
                    &mut self.renderer_plan,
                );
            }
            CompositeSync::LayerProps(layer_ids) => {
                for layer_id in layer_ids {
                    composite_layer_props_effect_into(
                        &self.state,
                        layer_id,
                        &mut self.renderer_plan,
                    );
                }
            }
            CompositeSync::AdjustmentLayers(layer_ids) => {
                for layer_id in layer_ids {
                    composite_adjustment_effect_into(
                        &self.state,
                        layer_id,
                        &mut self.renderer_plan,
                    );
                }
            }
        }
        Ok(())
    }

    fn finalize_cpu_history_atoms(
        &mut self,
        pending: &mut PendingEditTransaction,
    ) -> Result<(), String> {
        let mut completed = Vec::new();
        let mut waiting = Vec::new();
        for mut transaction in pending.history_transactions_mut().drain(..) {
            let mut atoms = Vec::new();
            for atom in transaction.atoms.drain(..) {
                match atom {
                    PendingHistoryAtom::Cpu(cpu) => {
                        let finalized = history_finalizer::finalize_cpu_history_transaction(
                            &mut self.state,
                            cpu,
                        )
                        .map_err(|err| format!("{err:#}"))?;
                        if let Some(atom) = finalized {
                            atoms.push(PendingHistoryAtom::Finalized(atom));
                        }
                    }
                    other => atoms.push(other),
                }
            }
            transaction.atoms = atoms;
            let has_renderer = transaction
                .atoms
                .iter()
                .any(|atom| matches!(atom, PendingHistoryAtom::Renderer(_)));
            if has_renderer {
                waiting.push(transaction);
            } else {
                let atoms = transaction
                    .atoms
                    .into_iter()
                    .filter_map(|atom| match atom {
                        PendingHistoryAtom::Finalized(atom) => Some(atom),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if !atoms.is_empty() {
                    completed.push(HistoryTransaction::new(
                        transaction.id,
                        transaction.label,
                        atoms,
                    ));
                }
            }
        }
        *pending.history_transactions_mut() = waiting;
        for history in completed {
            self.state.push_history(
                history,
                AppState::MAX_HISTORY_ENTRIES,
                AppState::MAX_HISTORY_BYTES,
            );
        }
        Ok(())
    }

    fn handle_reducer_error(&mut self, err: anyhow::Error) -> String {
        let err = format!("{err:#}");
        self.state.set_status_message(
            crate::application::StatusMessage::localized("status-operation-failed")
                .arg("error", err.clone()),
        );
        self.pending_transaction.clear();
        self.pending_render_finalizations.clear();
        self.renderer_plan.clear();
        err
    }

    pub fn discard_renderer_frame_plan(&mut self) {
        self.renderer_plan.clear();
    }

    pub(crate) fn drain_renderer_submission_work(
        &mut self,
        requests: Vec<crate::renderer::ViewRequest>,
    ) -> RendererSubmissionWork {
        let submission = self.pending_transaction.take_renderer_submission();
        let commit_request = submission.commit_request();
        let renderer_plan = self
            .renderer_plan
            .drain_into_frame_plan(requests, commit_request);
        RendererSubmissionWork {
            renderer_plan,
            submission,
        }
    }

    #[cfg(test)]
    pub fn drain_renderer_frame_plan(
        &mut self,
        requests: Vec<crate::renderer::ViewRequest>,
    ) -> crate::renderer::RendererFramePlan {
        self.drain_renderer_submission_work(requests).renderer_plan
    }

    #[cfg(test)]
    pub fn observe_render_report(
        &mut self,
        report: crate::core::render_report::RenderReport,
    ) -> Result<(), String> {
        self.last_effect_metrics.merge(report.metrics);
        if let Some(err) = report.error {
            self.abort_render_finalization(err.clone());
            return Err(err);
        }
        Ok(())
    }

    pub(crate) fn accept_renderer_submission(
        &mut self,
        submission: PreparedRendererSubmission,
        report: crate::core::render_report::RenderReport,
        started_commit: Result<Option<StartedRenderCommit>, String>,
    ) -> Result<(), String> {
        self.last_effect_metrics.merge(report.metrics);
        if let Some(error) = report.error {
            self.abort_prepared_submission(error.clone(), submission);
            return Err(error);
        }

        let started_commit = match started_commit {
            Ok(started_commit) => started_commit,
            Err(error) => {
                self.abort_prepared_submission(
                    format!("Render commit readback failed: {error}"),
                    submission,
                );
                return Err(error);
            }
        };

        match (submission.is_empty(), started_commit) {
            (true, None) => Ok(()),
            (true, Some(started_commit)) => {
                let error = format!(
                    "Renderer started commit {:?} without pending render finalization",
                    started_commit.id
                );
                self.abort_prepared_submission(error.clone(), submission);
                Err(error)
            }
            (false, None) => {
                let error =
                    "Renderer did not start a commit for pending render finalization".to_owned();
                self.abort_prepared_submission(error.clone(), submission);
                Err(error)
            }
            (false, Some(started_commit)) => {
                self.pending_render_finalizations
                    .push_back(submission.into_submitted(started_commit.id));
                Ok(())
            }
        }
    }

    pub fn abort_render_finalization(&mut self, error: impl Into<String>) {
        self.abort_render_finalization_with_detached(error, None);
    }

    fn abort_prepared_submission(
        &mut self,
        error: impl Into<String>,
        submission: PreparedRendererSubmission,
    ) {
        self.abort_render_finalization_with_detached(
            error,
            Some((
                submission.into_finalization(),
                DetachedTransactionPosition::Newest,
            )),
        );
    }

    fn abort_render_finalization_with_detached(
        &mut self,
        error: impl Into<String>,
        detached_transaction: Option<(RendererFinalizationPayload, DetachedTransactionPosition)>,
    ) {
        let error = error.into();
        let pending_transaction = std::mem::take(&mut self.pending_transaction);
        let mut rollback_error =
            self.rollback_finalized_cpu_atoms(pending_transaction.finalization());
        if rollback_error.is_none() {
            if let Some((transaction, DetachedTransactionPosition::Newest)) =
                detached_transaction.as_ref()
            {
                rollback_error = self.rollback_finalized_cpu_atoms(transaction);
            }
        }
        let staged = self
            .pending_render_finalizations
            .drain(..)
            .map(SubmittedRendererTransaction::into_finalization)
            .collect::<Vec<_>>();
        for transaction in staged.iter().rev() {
            if rollback_error.is_none() {
                rollback_error = self.rollback_finalized_cpu_atoms(transaction);
            }
        }
        if rollback_error.is_none() {
            if let Some((transaction, DetachedTransactionPosition::Oldest)) =
                detached_transaction.as_ref()
            {
                rollback_error = self.rollback_finalized_cpu_atoms(transaction);
            }
        }
        let fatal = if let Some(rollback_error) = rollback_error {
            format!("{error}; CPU rollback failed: {rollback_error}")
        } else {
            error
        };
        self.state.set_status_message(
            crate::application::StatusMessage::localized("status-editing-disabled-error")
                .arg("error", fatal.clone()),
        );
        self.fatal_edit_error = Some(fatal);
        self.state.tool_mut().clear_session();
        self.renderer_plan.clear();
    }

    fn rollback_finalized_cpu_atoms(
        &mut self,
        pending: &RendererFinalizationPayload,
    ) -> Option<String> {
        for transaction in pending.history_transactions().iter().rev() {
            for atom in transaction.atoms.iter().rev() {
                let PendingHistoryAtom::Finalized(atom) = atom else {
                    continue;
                };
                if let Err(err) = crate::application::history_reducer::apply_history_atom(
                    &mut self.state,
                    atom,
                    crate::application::history_reducer::HistoryDirection::Undo,
                ) {
                    return Some(format!("{err:#}"));
                }
            }
        }
        None
    }

    pub(crate) fn resolve_pending_edits_without_renderer(&mut self) -> Result<(), String> {
        let submission = self.pending_transaction.take_renderer_submission();
        self.finalize_renderer_payload(
            submission.into_finalization(),
            &RenderCommitArtifacts::default(),
        )
        .map(|_| ())
    }

    #[cfg(test)]
    pub fn finalize_pending_renderer_edits(
        &mut self,
        commits: &RenderCommitArtifacts,
    ) -> Result<(), String> {
        let submission = self.pending_transaction.take_renderer_submission();
        self.finalize_renderer_payload(submission.into_finalization(), commits)
            .map(|_| ())
    }

    pub fn finalize_completed_render_commit(
        &mut self,
        completed: CompletedRenderCommit,
    ) -> Result<(RenderCommitArtifacts, bool), String> {
        let Some(front) = self.pending_render_finalizations.front() else {
            let err = format!(
                "Render commit {:?} completed without pending render finalization",
                completed.id
            );
            self.abort_render_finalization(err.clone());
            return Err(err);
        };
        if front.id() != completed.id {
            let err = format!(
                "Render commit completed out of order: expected {:?}, got {:?}",
                front.id(),
                completed.id
            );
            self.abort_render_finalization(err.clone());
            return Err(err);
        }
        let pending = self
            .pending_render_finalizations
            .pop_front()
            .expect("front was just observed");
        let transaction = pending.into_finalization();
        let commits = match completed.artifacts {
            Ok(commits) => commits,
            Err(error) => {
                self.abort_render_finalization_with_detached(
                    format!("Render commit readback failed: {error}"),
                    Some((transaction, DetachedTransactionPosition::Oldest)),
                );
                return Err(error);
            }
        };
        let project_changed = self.finalize_renderer_payload(transaction, &commits)?;
        Ok((commits, project_changed))
    }

    pub fn has_pending_render_finalizations(&self) -> bool {
        !self.pending_render_finalizations.is_empty()
    }

    pub fn is_render_commit_pending(&self) -> bool {
        self.has_pending_render_finalizations()
            || self.pending_transaction.has_pending_renderer_finalization()
    }

    pub fn editor_action_block_reason(&self) -> Option<EditorActionBlockReason> {
        if self.fatal_edit_error.is_some() {
            Some(EditorActionBlockReason::FatalRendererError)
        } else if self.is_render_commit_pending() {
            Some(EditorActionBlockReason::RenderCommitPending)
        } else {
            None
        }
    }

    fn finalize_renderer_payload(
        &mut self,
        mut renderer_finalization: RendererFinalizationPayload,
        commits: &RenderCommitArtifacts,
    ) -> Result<bool, String> {
        let rollback_snapshot = renderer_finalization.clone();
        let renderer_commit = renderer_finalization.take_commit();
        let pending_document_commit = renderer_finalization.take_document_commit();
        let pending_renderer_atoms = renderer_finalization
            .history_transactions()
            .iter()
            .flat_map(|transaction| transaction.atoms.iter())
            .filter_map(|atom| match atom {
                PendingHistoryAtom::Renderer(pending) => Some(pending),
                _ => None,
            })
            .collect::<Vec<_>>();
        let prepared_surface_commits = match commit_finalizer::prepare_finalized_surface_commits(
            &self.state,
            &renderer_commit.finalized_surfaces,
            &commits.surface_commits,
            &pending_renderer_atoms,
        ) {
            Ok(prepared) => prepared,
            Err(err) => {
                let err = format!("{err:#}");
                self.pending_transaction
                    .append_finalization(rollback_snapshot.clone());
                self.abort_render_finalization(err.clone());
                return Err(err);
            }
        };
        let project_changed = prepared_surface_commits.changes_document();
        if let Err(err) = self.validate_pending_document_commit(pending_document_commit.as_ref()) {
            self.pending_transaction
                .append_finalization(rollback_snapshot.clone());
            self.abort_render_finalization(err.clone());
            return Err(err);
        }

        drop(pending_renderer_atoms);
        for transaction in renderer_finalization.history_transactions_mut() {
            let mut atoms = Vec::with_capacity(transaction.atoms.len());
            for atom in transaction.atoms.drain(..) {
                let PendingHistoryAtom::Renderer(pending) = atom else {
                    atoms.push(atom);
                    continue;
                };
                let result = match history_finalizer::finalize_renderer_history_transaction(
                    &mut self.state,
                    pending,
                    &renderer_commit.finalized_surfaces,
                    &commits.surface_commits,
                    renderer_commit.selection.is_some(),
                    &commits.selection_before_commits,
                    &commits.selection_after_commits,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        let err = format!("{err:#}");
                        self.pending_transaction
                            .append_finalization(rollback_snapshot.clone());
                        self.abort_render_finalization(err.clone());
                        return Err(err);
                    }
                };
                if let Some(entry) = result.entry {
                    atoms.push(PendingHistoryAtom::Finalized(entry));
                }
                if let Some(pending) = result.pending_transaction {
                    atoms.push(PendingHistoryAtom::Renderer(pending));
                }
            }
            transaction.atoms = atoms;
        }

        if let Err(err) = commit_finalizer::apply_prepared_surface_commits_to_document(
            &mut self.state,
            &prepared_surface_commits,
        ) {
            let err = format!("{err:#}");
            self.pending_transaction
                .append_finalization(rollback_snapshot.clone());
            self.abort_render_finalization(err.clone());
            return Err(err);
        }
        if let Some(document_commit) = pending_document_commit {
            if let Err(err) = self
                .apply_pending_document_commit(document_commit, &commits.selection_after_commits)
            {
                self.pending_transaction
                    .append_finalization(rollback_snapshot);
                self.abort_render_finalization(err.clone());
                return Err(err);
            }
        }
        let mut waiting = Vec::new();
        for transaction in renderer_finalization.history_transactions_mut().drain(..) {
            if transaction
                .atoms
                .iter()
                .any(|atom| matches!(atom, PendingHistoryAtom::Renderer(_)))
            {
                waiting.push(transaction);
                continue;
            }
            let atoms = transaction
                .atoms
                .into_iter()
                .filter_map(|atom| match atom {
                    PendingHistoryAtom::Finalized(atom) => Some(atom),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !atoms.is_empty() {
                self.state.push_history(
                    HistoryTransaction::new(transaction.id, transaction.label, atoms),
                    AppState::MAX_HISTORY_ENTRIES,
                    AppState::MAX_HISTORY_BYTES,
                );
            }
        }
        *renderer_finalization.history_transactions_mut() = waiting;
        if !renderer_finalization.is_empty() {
            self.pending_transaction
                .append_finalization(renderer_finalization);
        }
        apply_renderer_diagnostics(&mut self.state, &commits.diagnostics);
        self.renderer_plan.clear();
        Ok(project_changed)
    }

    fn validate_pending_document_commit(
        &self,
        document_commit: Option<&PendingDocumentCommit>,
    ) -> Result<(), String> {
        match document_commit {
            Some(PendingDocumentCommit::Selection { .. }) if self.state.document().is_none() => {
                Err("Selection commit completed without an active document".to_owned())
            }
            _ => Ok(()),
        }
    }

    fn apply_pending_document_commit(
        &mut self,
        document_commit: PendingDocumentCommit,
        selection_after_commits: &[crate::renderer::SelectionCommit],
    ) -> Result<(), String> {
        match document_commit {
            PendingDocumentCommit::Selection { after } => {
                let document = self.state.document_mut().ok_or_else(|| {
                    "Selection commit completed without an active document".to_owned()
                })?;
                for commit in selection_after_commits {
                    document
                        .selection_masks
                        .write_mask(
                            commit.material_index,
                            commit.texture_size,
                            commit.r8.clone(),
                        )
                        .map_err(|err| format!("Selection document commit failed: {err:#}"))?;
                }
                document.active_selection = after;
                Ok(())
            }
        }
    }
}

fn apply_renderer_diagnostics(state: &mut AppState, diagnostics: &[RendererDiagnostic]) {
    for diagnostic in diagnostics {
        match *diagnostic {
            RendererDiagnostic::SpatialBlur {
                target_stride_px,
                actual_stride_px,
                traversal_overflow_texels,
                capacity_overflow_samples,
            } => {
                if traversal_overflow_texels > 0 || capacity_overflow_samples > 0 {
                    state.set_status_key("status-spatial-blur-reduced-accuracy");
                } else if actual_stride_px > target_stride_px {
                    state.set_status_message(
                        super::StatusMessage::localized("status-spatial-blur-reduced-sampling")
                            .arg("stride", actual_stride_px),
                    );
                }
            }
        }
    }
}

fn composite_layer_props_effect_into(
    state: &AppState,
    layer_id: crate::core::surface::LayerId,
    renderer_plan: &mut RendererPlanBuilder,
) {
    let Some(document) = state.document() else {
        return;
    };
    let Some(node) = document.layer_tree.get(layer_id) else {
        return;
    };
    let props = CompositeProps {
        visible: node.props.visible,
        opacity: node.props.opacity,
        blend_mode: node.props.blend_mode,
    };
    renderer_plan.push_document_command(GpuDocumentCommand::UpdateLayerProps { layer_id, props });
}

fn composite_adjustment_effect_into(
    state: &AppState,
    layer_id: crate::core::surface::LayerId,
    renderer_plan: &mut RendererPlanBuilder,
) {
    let Some(document) = state.document() else {
        return;
    };
    let Some(adjustment) = document.layer_tree.adjustment(layer_id) else {
        return;
    };
    renderer_plan.push_document_command(GpuDocumentCommand::UpdateAdjustment {
        layer_id,
        adjustment,
    });
}

fn material_composite_tree_sync_effects_into(
    state: &AppState,
    renderer_plan: &mut RendererPlanBuilder,
) {
    let Some(document) = state.document() else {
        return;
    };
    material_composite_tree_sync_effects_for_indices_into(
        state,
        (0..document.materials.len()).map(MaterialIndex),
        renderer_plan,
    );
}

fn material_composite_tree_sync_effects_for_indices_into(
    state: &AppState,
    material_indices: impl IntoIterator<Item = MaterialIndex>,
    renderer_plan: &mut RendererPlanBuilder,
) {
    let Some(document) = state.document() else {
        return;
    };

    for material_index in material_indices {
        let material_index = material_index.as_usize();
        if let Some(tree) = document.composite_tree(material_index) {
            renderer_plan.push_document_command(GpuDocumentCommand::SyncMaterialTree {
                material_index,
                tree,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{
            AppState, ApplicationRuntime, Command, EditorActionBlockReason, HistoryEntry,
            InputHoldToken, InputModifiers, LayerPropertyEdit, PendingEditTransaction,
            PendingHistoryAtom, PendingPixelEdit, PendingRendererHistoryTransaction,
            PixelSnapshotData, RectU32, RenderCommitArtifacts, RenderMetrics, RendererCommitSpec,
            SubmittedRendererTransaction, SurfaceCommit, ViewPointerEvent, ViewPointerMeta,
            ViewPointerPhase,
        },
        core::{
            adjustment::{Adjustment, BrightnessContrastAdjustment},
            document::{Document, MaterialSpec, MeshData},
            render_report::RenderReport,
            tool::ToolId,
            wireframe::WireframeStyle,
        },
        renderer::{
            CompletedRenderCommit, GpuDocumentCommand, RenderCommitId, StartedRenderCommit,
        },
    };

    fn submitted_transaction(
        id: RenderCommitId,
        mut pending: PendingEditTransaction,
    ) -> SubmittedRendererTransaction {
        if !pending.has_pending_renderer_finalization() {
            pending.append_renderer_history_transaction(
                "Test finalization",
                PendingRendererHistoryTransaction::Pixel { edits: Vec::new() },
            );
        }
        pending.take_renderer_submission().into_submitted(id)
    }

    #[test]
    fn viewport_view_commands_are_allowed_while_render_pending() {
        let allowed = |command: &Command| {
            command
                .policy()
                .allowed_during(super::CommandBlocker::RenderCommitPending)
        };

        assert!(allowed(&Command::SetViewportGizmoVisible(false)));
        assert!(allowed(&Command::SetViewportBackgroundColor([
            0.1, 0.2, 0.3,
        ])));
        assert!(allowed(&Command::SetUvViewBackgroundColor(
            [0.4, 0.5, 0.6,]
        )));
        assert!(allowed(&Command::SetViewportWireframeStyle(
            WireframeStyle::new([0.1, 0.2, 0.3], 0.4,)
        )));
        assert!(allowed(&Command::SetUvWireframeStyle(WireframeStyle::new(
            [0.5, 0.6, 0.7],
            0.8,
        ))));
        assert!(allowed(&Command::SetViewportMeshVisible {
            mesh_id: crate::core::document::MeshId(1),
            visible: false,
        }));
        assert!(allowed(&Command::SetViewportMaterialVisible {
            material_index: 2,
            visible: false,
        }));
    }

    #[test]
    fn input_cleanup_commands_run_while_render_commit_is_pending() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let selected_tool = runtime.state.active_tool_id();
        let momentary_token = InputHoldToken(41);
        let transient_token = InputHoldToken(42);

        runtime
            .dispatch(Command::BeginMomentaryTool {
                token: momentary_token,
                tool_id: ToolId::RectangleSelection,
            })
            .unwrap();
        runtime
            .dispatch(Command::BeginTransientToolOverride {
                token: transient_token,
                id: "eraser".to_owned(),
            })
            .unwrap();
        assert_eq!(
            runtime.state.effective_tool_id(),
            ToolId::RectangleSelection
        );
        assert_eq!(runtime.state.transient_tool_override_ids().count(), 1);

        runtime
            .pending_render_finalizations
            .push_back(submitted_transaction(
                RenderCommitId(43),
                PendingEditTransaction::default(),
            ));
        assert!(runtime.is_render_commit_pending());
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::RenderCommitPending)
        );

        runtime
            .dispatch(Command::CompleteMomentaryTool {
                token: momentary_token,
                tool_id: ToolId::RectangleSelection,
                select_tool: false,
            })
            .unwrap();
        runtime
            .dispatch(Command::EndTransientToolOverride {
                token: transient_token,
            })
            .unwrap();

        assert_eq!(runtime.state.active_tool_id(), selected_tool);
        assert_eq!(runtime.state.effective_tool_id(), selected_tool);
        assert!(!runtime.state.is_tool_interacting());
        assert_eq!(runtime.state.transient_tool_override_ids().count(), 0);
    }

    #[test]
    fn fatal_editor_action_block_reason_takes_priority_over_pending_render() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        runtime
            .pending_render_finalizations
            .push_back(submitted_transaction(
                RenderCommitId(99),
                PendingEditTransaction::default(),
            ));
        runtime.fatal_edit_error = Some("fatal".to_owned());

        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
    }

    fn runtime_with_pending_composite_edit() -> (ApplicationRuntime, crate::core::surface::LayerId)
    {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let layer_id = state
            .document
            .document
            .as_ref()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);
        let mut runtime = ApplicationRuntime::new(state);
        runtime.pending_transaction.append_history_transaction(
            "Composite Edit",
            vec![
                PendingHistoryAtom::Finalized(HistoryEntry::LayerPropertyEdit {
                    edit: LayerPropertyEdit::Visibility {
                        layer_id,
                        before: true,
                        after: false,
                    },
                }),
                PendingHistoryAtom::Renderer(PendingRendererHistoryTransaction::Pixel {
                    edits: Vec::new(),
                }),
            ],
        );
        (runtime, layer_id)
    }

    fn assert_composite_edit_rolled_back(
        runtime: &ApplicationRuntime,
        layer_id: crate::core::surface::LayerId,
    ) {
        assert!(
            runtime
                .state
                .document()
                .unwrap()
                .layer_tree
                .get(layer_id)
                .unwrap()
                .props
                .visible
        );
        assert!(runtime.state.history.undo_stack.is_empty());
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
    }

    #[test]
    fn renderer_failure_rolls_back_finalized_cpu_atoms_and_blocks_editing() {
        let (mut runtime, layer_id) = runtime_with_pending_composite_edit();

        runtime.abort_render_finalization("forced failure");

        assert_composite_edit_rolled_back(&runtime, layer_id);
    }

    #[test]
    fn empty_submission_without_started_commit_completes_without_staging() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let submission = runtime
            .drain_renderer_submission_work(Vec::new())
            .submission;

        runtime
            .accept_renderer_submission(submission, RenderReport::default(), Ok(None))
            .unwrap();

        assert!(!runtime.has_pending_render_finalizations());
        assert_eq!(runtime.editor_action_block_reason(), None);
    }

    #[test]
    fn empty_submission_with_started_commit_is_a_fatal_protocol_error() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let submission = runtime
            .drain_renderer_submission_work(Vec::new())
            .submission;

        let result = runtime.accept_renderer_submission(
            submission,
            RenderReport::default(),
            Ok(Some(StartedRenderCommit {
                id: RenderCommitId(16),
            })),
        );

        assert!(matches!(
            result,
            Err(error) if error.contains("without pending render finalization")
        ));
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
    }

    #[test]
    fn missing_started_commit_rolls_back_detached_cpu_atoms_and_blocks_editing() {
        let (mut runtime, layer_id) = runtime_with_pending_composite_edit();
        let submission = runtime.pending_transaction.take_renderer_submission();

        let result =
            runtime.accept_renderer_submission(submission, RenderReport::default(), Ok(None));

        assert!(matches!(
            result,
            Err(err) if err.contains("did not start a commit")
        ));
        assert_composite_edit_rolled_back(&runtime, layer_id);
    }

    #[test]
    fn submission_report_error_rolls_back_detached_cpu_atoms() {
        let (mut runtime, layer_id) = runtime_with_pending_composite_edit();
        let submission = runtime.pending_transaction.take_renderer_submission();

        let result = runtime.accept_renderer_submission(
            submission,
            RenderReport::error("render failed".to_owned(), RenderMetrics::default()),
            Ok(None),
        );

        assert_eq!(result, Err("render failed".to_owned()));
        assert_composite_edit_rolled_back(&runtime, layer_id);
    }

    #[test]
    fn started_commit_error_rolls_back_prepared_cpu_atoms() {
        let (mut runtime, layer_id) = runtime_with_pending_composite_edit();
        let submission = runtime.pending_transaction.take_renderer_submission();

        let result = runtime.accept_renderer_submission(
            submission,
            RenderReport::default(),
            Err("map failed".to_owned()),
        );

        assert_eq!(result, Err("map failed".to_owned()));
        assert_composite_edit_rolled_back(&runtime, layer_id);
    }

    #[test]
    fn readback_error_rolls_back_staged_cpu_atoms() {
        let (mut runtime, layer_id) = runtime_with_pending_composite_edit();
        let submission = runtime.pending_transaction.take_renderer_submission();
        runtime
            .accept_renderer_submission(
                submission,
                RenderReport::default(),
                Ok(Some(StartedRenderCommit {
                    id: RenderCommitId(17),
                })),
            )
            .unwrap();

        let result = runtime.finalize_completed_render_commit(CompletedRenderCommit {
            id: RenderCommitId(17),
            artifacts: Err("readback failed".to_owned()),
        });

        assert_eq!(result, Err("readback failed".to_owned()));
        assert_composite_edit_rolled_back(&runtime, layer_id);
    }

    #[test]
    fn readback_error_rolls_back_remaining_staged_transactions_before_oldest() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let layer_id = state
            .document
            .document
            .as_ref()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let mut runtime = ApplicationRuntime::new(state);

        let mut oldest = PendingEditTransaction::default();
        oldest.append_history_transaction(
            "Hide Layer",
            vec![PendingHistoryAtom::Finalized(
                HistoryEntry::LayerPropertyEdit {
                    edit: LayerPropertyEdit::Visibility {
                        layer_id,
                        before: true,
                        after: false,
                    },
                },
            )],
        );
        let mut newest = PendingEditTransaction::default();
        newest.append_history_transaction(
            "Show Layer",
            vec![PendingHistoryAtom::Finalized(
                HistoryEntry::LayerPropertyEdit {
                    edit: LayerPropertyEdit::Visibility {
                        layer_id,
                        before: false,
                        after: true,
                    },
                },
            )],
        );
        runtime
            .pending_render_finalizations
            .push_back(submitted_transaction(RenderCommitId(21), oldest));
        runtime
            .pending_render_finalizations
            .push_back(submitted_transaction(RenderCommitId(22), newest));

        let result = runtime.finalize_completed_render_commit(CompletedRenderCommit {
            id: RenderCommitId(21),
            artifacts: Err("readback failed".to_owned()),
        });

        assert_eq!(result, Err("readback failed".to_owned()));
        assert!(
            runtime
                .state
                .document()
                .unwrap()
                .layer_tree
                .get(layer_id)
                .unwrap()
                .props
                .visible
        );
    }

    #[test]
    fn observe_render_report_preserves_error_handling() {
        let mut runtime = ApplicationRuntime::new(AppState::default());

        let result = runtime.observe_render_report(RenderReport::error(
            "boom".to_owned(),
            RenderMetrics::default(),
        ));

        assert_eq!(result, Err("boom".to_owned()));
        assert_eq!(runtime.state.status(), "Editing disabled: boom");
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
        assert_eq!(runtime.take_last_effect_metrics(), RenderMetrics::default());
    }

    #[test]
    fn observe_render_report_accepts_successful_reports() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let result = runtime.observe_render_report(RenderReport::ok(RenderMetrics {
            executed_effect_count: 7,
            ..RenderMetrics::default()
        }));

        assert_eq!(result, Ok(()));
        assert_eq!(runtime.take_last_effect_metrics().executed_effect_count, 7);
        assert_eq!(runtime.take_last_effect_metrics(), RenderMetrics::default());
    }

    #[test]
    fn observe_render_report_propagates_metrics_from_error_reports() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let _ = runtime.observe_render_report(RenderReport::error(
            "boom".to_owned(),
            RenderMetrics {
                effect_batches: 1,
                executed_effect_count: 3,
                ..RenderMetrics::default()
            },
        ));
        let metrics = runtime.take_last_effect_metrics();

        assert_eq!(metrics.effect_batches, 1);
        assert_eq!(metrics.executed_effect_count, 3);
    }

    #[test]
    fn finalize_fill_without_renderer_returns_error_and_does_not_mutate_history() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        state.tool.current_color = [1.0, 0.0, 0.0];
        let surface = state.active_paint_surface().unwrap();
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::FillMaterial {
                material_index: 0,
                opacity: 1.0,
            })
            .unwrap();
        runtime.discard_renderer_frame_plan();
        let result = runtime.finalize_pending_renderer_edits(&RenderCommitArtifacts::default());

        assert!(
            matches!(result, Err(err) if err.contains("Renderer did not provide surface commit"))
        );
        assert!(runtime.state.history.undo_stack.is_empty());
        assert!(!runtime.has_pending_history_transaction());
        assert_eq!(
            runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(surface)
                .unwrap()
                .rgba8,
            vec![0, 0, 0, 0]
        );
    }

    #[test]
    fn renderer_finalization_waits_for_completed_async_commit() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let surface = state.active_paint_surface().unwrap();
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::FillMaterial {
                material_index: 0,
                opacity: 1.0,
            })
            .unwrap();
        let submission = runtime
            .drain_renderer_submission_work(Vec::new())
            .submission;
        runtime
            .accept_renderer_submission(
                submission,
                RenderReport::default(),
                Ok(Some(StartedRenderCommit {
                    id: RenderCommitId(7),
                })),
            )
            .unwrap();

        assert!(runtime.has_pending_render_finalizations());
        assert!(runtime.state.history.undo_stack.is_empty());
        assert!(runtime.dispatch(Command::Undo).is_err());

        let (artifacts, project_changed) = runtime
            .finalize_completed_render_commit(CompletedRenderCommit {
                id: RenderCommitId(7),
                artifacts: Ok(RenderCommitArtifacts {
                    surface_commits: vec![SurfaceCommit {
                        surface,
                        rect: RectU32::full([2, 2]),
                        after: PixelSnapshotData::contiguous(vec![8; 16]),
                    }],
                    ..Default::default()
                }),
            })
            .unwrap();

        assert!(project_changed);
        assert_eq!(artifacts.surface_commits.len(), 1);
        assert_eq!(artifacts.surface_commits[0].surface, surface);
        assert!(!runtime.has_pending_render_finalizations());
        assert_eq!(
            runtime
                .state
                .history
                .undo_stack
                .last()
                .map(|transaction| transaction.label.as_str()),
            Some("Fill Material")
        );
        assert!(matches!(
            runtime
                .state
                .history
                .undo_stack
                .last()
                .and_then(|tx| tx.atoms.first()),
            Some(HistoryEntry::PixelEdit { .. })
        ));
        assert_eq!(
            runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(surface)
                .unwrap()
                .rgba8,
            vec![8; 16]
        );
    }

    #[test]
    fn render_report_error_discards_staged_finalization_and_blocks_editing() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::FillMaterial {
                material_index: 0,
                opacity: 1.0,
            })
            .unwrap();
        let submission = runtime
            .drain_renderer_submission_work(Vec::new())
            .submission;
        runtime
            .accept_renderer_submission(
                submission,
                RenderReport::default(),
                Ok(Some(StartedRenderCommit {
                    id: RenderCommitId(7),
                })),
            )
            .unwrap();

        let result = runtime.observe_render_report(RenderReport::error(
            "view render failed".to_owned(),
            RenderMetrics::default(),
        ));

        assert_eq!(result, Err("view render failed".to_owned()));
        assert!(!runtime.has_pending_render_finalizations());
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
        assert!(runtime.state.history.undo_stack.is_empty());
        assert!(runtime.dispatch(Command::Undo).is_err());
    }

    #[test]
    fn renderer_history_failure_does_not_apply_prepared_document_commits() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let surface = state.active_paint_surface().unwrap();
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .pending_transaction
            .append_renderer_history_transaction(
                "Paint",
                PendingRendererHistoryTransaction::Pixel {
                    edits: vec![PendingPixelEdit {
                        surface,
                        texture_size: [2, 2],
                        rect: RectU32::full([2, 2]),
                        before: PixelSnapshotData::contiguous(vec![0; 16]),
                    }],
                },
            );
        runtime
            .pending_transaction
            .append_renderer_commit(RendererCommitSpec {
                finalized_surfaces: vec![crate::core::surface::PaintSurfaceId::raster(
                    (surface.material_index().as_usize() + 1).into(),
                    surface.layer_id,
                )],
                selection: None,
            });
        let submission = runtime
            .drain_renderer_submission_work(Vec::new())
            .submission;
        runtime
            .accept_renderer_submission(
                submission,
                RenderReport::default(),
                Ok(Some(StartedRenderCommit {
                    id: RenderCommitId(7),
                })),
            )
            .unwrap();

        let result = runtime.finalize_completed_render_commit(CompletedRenderCommit {
            id: RenderCommitId(7),
            artifacts: Ok(RenderCommitArtifacts {
                surface_commits: vec![SurfaceCommit {
                    surface,
                    rect: RectU32::full([2, 2]),
                    after: PixelSnapshotData::contiguous(vec![8; 16]),
                }],
                ..Default::default()
            }),
        });

        assert!(matches!(
            result,
            Err(err) if err.contains("Renderer did not finalize pending surface")
        ));
        assert!(runtime.state.history.undo_stack.is_empty());
        assert_eq!(
            runtime
                .state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(surface)
                .unwrap()
                .rgba8,
            vec![0; 16]
        );
    }

    #[test]
    fn completed_render_commit_without_pending_finalization_is_error() {
        let mut runtime = ApplicationRuntime::new(AppState::default());

        let result = runtime.finalize_completed_render_commit(CompletedRenderCommit {
            id: RenderCommitId(99),
            artifacts: Ok(RenderCommitArtifacts::default()),
        });

        assert!(matches!(
            result,
            Err(err) if err.contains("completed without pending render finalization")
        ));
        assert_eq!(
            runtime.state.status(),
            "Editing disabled: Render commit RenderCommitId(99) completed without pending render finalization"
        );
        assert_eq!(
            runtime.editor_action_block_reason(),
            Some(EditorActionBlockReason::FatalRendererError)
        );
    }

    #[test]
    fn undo_is_blocked_while_renderer_transaction_is_not_yet_staged() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::FillMaterial {
                material_index: 0,
                opacity: 1.0,
            })
            .unwrap();
        let result = runtime.dispatch(Command::Undo);

        assert!(matches!(result, Err(err) if err.contains("Render commit pending")));
        assert!(runtime.has_pending_history_transaction());

        let plan = runtime.drain_renderer_frame_plan(Vec::new());
        assert!(plan.commit_request.is_some());
        assert!(matches!(
            plan.edit_commands.first(),
            Some(crate::renderer::EditCommand::Apply(_))
        ));
    }

    #[test]
    fn view_pointer_is_blocked_while_renderer_transaction_is_pending() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::FillMaterial {
                material_index: 0,
                opacity: 1.0,
            })
            .unwrap();
        let result = runtime.dispatch(Command::ViewPointer(ViewPointerEvent::Uv {
            meta: ViewPointerMeta {
                phase: ViewPointerPhase::Down,
                position_px: glam::Vec2::new(1.0, 1.0),
                pressure: 1.0,
                time_s: 0.0,
                modifiers: InputModifiers::default(),
            },
            view: crate::application::UvViewInputContext {
                size: [2, 2],
                canvas_size: [2, 2],
                transform: crate::core::uv_view::UvViewTransform::default(),
            },
        }));

        assert!(matches!(result, Err(err) if err.contains("Render commit pending")));
        assert!(runtime.has_pending_history_transaction());
    }

    #[test]
    fn dispatch_accumulates_effects_until_frame_drain() {
        let asset = crate::core::document::ImportedAsset {
            mesh: empty_mesh(),
            materials: vec![MaterialSpec::new("M", [1, 1])],
        };
        let mut runtime = ApplicationRuntime::new(AppState::default());

        runtime
            .dispatch(Command::AssetLoaded {
                asset,
                materials: vec![crate::core::image::MaterialPayload {
                    width: 1,
                    height: 1,
                    rgba8: vec![0; 4],
                }],
            })
            .unwrap();
        runtime
            .dispatch(Command::SetStatusMessage(
                crate::application::StatusMessage::raw("loaded"),
            ))
            .unwrap();

        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert!(matches!(
            plan.document_commands.first(),
            Some(GpuDocumentCommand::UploadScene { .. })
        ));
        assert!(
            plan.document_commands
                .iter()
                .any(|command| matches!(command, GpuDocumentCommand::SyncMaterialTree { .. }))
        );
    }

    #[test]
    fn specified_material_mask_change_syncs_only_affected_material_trees() {
        let mut state = AppState::default();
        let mut document = Document::new(
            empty_mesh(),
            vec![
                MaterialSpec::new("A", [1, 1]),
                MaterialSpec::new("B", [1, 1]),
                MaterialSpec::new("C", [1, 1]),
            ],
        );
        let layer_id = document.layer_tree.default_raster_layer().unwrap();
        let material_a = document.materials[0].id;
        let material_b = document.materials[1].id;
        document.layer_tree.set_layer_material_mask(
            layer_id,
            crate::core::surface::LayerMaterialMask::Specified([material_a].into_iter().collect()),
        );
        state.document.document = Some(document);
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::SetLayerMaterialMask {
                layer_id,
                material_mask: crate::core::surface::LayerMaterialMask::Specified(
                    [material_b].into_iter().collect(),
                ),
            })
            .unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());
        let synced_materials = plan
            .document_commands
            .iter()
            .filter_map(|command| match command {
                GpuDocumentCommand::SyncMaterialTree { material_index, .. } => {
                    Some(*material_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(synced_materials, vec![0, 1]);
    }

    #[test]
    fn layer_property_updates_emit_composite_patch_without_full_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        let layer_id = state
            .document
            .editor
            .resolved(state.document().unwrap())
            .active_layer_id;
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::SetLayerVisible {
                layer_id,
                visible: false,
            })
            .unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::UpdateLayerProps {
                layer_id: patched_layer,
                props
            } if *patched_layer == layer_id && !props.visible
        )));
        assert!(
            !plan
                .document_commands
                .iter()
                .any(|command| matches!(command, GpuDocumentCommand::SyncMaterialTree { .. }))
        );
    }

    #[test]
    fn adjustment_updates_emit_value_patch_without_full_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        let raster = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let layer_id = state
            .document
            .document
            .as_mut()
            .unwrap()
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "Adjustment",
                Adjustment::BrightnessContrast(Default::default()),
            )
            .unwrap();
        let adjustment = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 25,
            contrast: -10,
        });
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::SetAdjustment {
                layer_id,
                adjustment: adjustment.clone(),
                edit_session: 1,
            })
            .unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::UpdateAdjustment {
                layer_id: patched_layer,
                adjustment: patched_adjustment,
            } if *patched_layer == layer_id && *patched_adjustment == adjustment
        )));
        assert!(
            !plan
                .document_commands
                .iter()
                .any(|command| matches!(command, GpuDocumentCommand::SyncMaterialTree { .. }))
        );
    }

    #[test]
    fn layer_property_history_survives_renderer_error() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        let layer_id = state
            .document
            .editor
            .resolved(state.document().unwrap())
            .active_layer_id;
        let mut runtime = ApplicationRuntime::new(state);

        runtime
            .dispatch(Command::SetLayerVisible {
                layer_id,
                visible: false,
            })
            .unwrap();
        let result = runtime.observe_render_report(RenderReport::error(
            "renderer sync failed".to_owned(),
            RenderMetrics::default(),
        ));

        assert_eq!(result, Err("renderer sync failed".to_owned()));
        assert!(matches!(
            runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
            Some(HistoryEntry::LayerPropertyEdit {
                edit: LayerPropertyEdit::Visibility { before, after, .. }
            }) if *before && !*after
        ));
        assert!(
            !runtime
                .state
                .document()
                .unwrap()
                .layer_tree
                .get(layer_id)
                .unwrap()
                .props
                .visible
        );
        assert!(!runtime.has_pending_history_transaction());
    }

    #[test]
    fn undo_layer_property_update_emits_composite_patch_without_full_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        let layer_id = state
            .document
            .editor
            .resolved(state.document().unwrap())
            .active_layer_id;
        state
            .document
            .document
            .as_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);
        state
            .history
            .undo_stack
            .push(crate::application::HistoryTransaction::test(
                "Test",
                vec![HistoryEntry::LayerPropertyEdit {
                    edit: LayerPropertyEdit::Visibility {
                        layer_id,
                        before: true,
                        after: false,
                    },
                }],
            ));
        let mut runtime = ApplicationRuntime::new(state);

        runtime.dispatch(Command::Undo).unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::UpdateLayerProps {
                layer_id: patched_layer,
                props
            } if *patched_layer == layer_id && props.visible
        )));
        assert!(
            !plan
                .document_commands
                .iter()
                .any(|command| matches!(command, GpuDocumentCommand::SyncMaterialTree { .. }))
        );
    }

    #[test]
    fn undo_adjustment_update_emits_value_patch_without_full_tree_sync() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [1, 1])],
        ));
        let raster = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        let before = Adjustment::BrightnessContrast(Default::default());
        let after = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 30,
            contrast: 15,
        });
        let layer_id = state
            .document
            .document
            .as_mut()
            .unwrap()
            .layer_tree
            .add_adjustment_layer_above(raster, "Adjustment", after.clone())
            .unwrap();
        state
            .history
            .undo_stack
            .push(crate::application::HistoryTransaction::test(
                "Test",
                vec![HistoryEntry::LayerPropertyEdit {
                    edit: LayerPropertyEdit::Adjustment {
                        layer_id,
                        edit_session: 1,
                        before: before.clone(),
                        after,
                    },
                }],
            ));
        let mut runtime = ApplicationRuntime::new(state);

        runtime.dispatch(Command::Undo).unwrap();
        let plan = runtime.drain_renderer_frame_plan(Vec::new());

        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::UpdateAdjustment {
                layer_id: patched_layer,
                adjustment,
            } if *patched_layer == layer_id && *adjustment == before
        )));
        assert!(
            !plan
                .document_commands
                .iter()
                .any(|command| matches!(command, GpuDocumentCommand::SyncMaterialTree { .. }))
        );
    }

    fn empty_mesh() -> MeshData {
        MeshData::empty()
    }
}
