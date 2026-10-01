use std::collections::BTreeSet;

use crate::{
    application::{
        HistoryAtom, HistoryTransactionId, PendingCpuHistoryTransaction,
        PendingRendererHistoryTransaction, RendererPlanBuilder,
    },
    core::{
        material::MaterialIndex,
        selection::ActiveSelection,
        surface::{LayerId, PaintSurfaceId},
    },
    renderer::{
        CommitRequest, EditCommand, GpuDocumentCommand, RenderCommitId, RendererFramePlan,
        SelectionCommitRequest,
    },
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum CompositeSync {
    #[default]
    None,
    MaterialTrees,
    MaterialTreesScoped(BTreeSet<MaterialIndex>),
    LayerProps(BTreeSet<LayerId>),
    AdjustmentLayers(BTreeSet<LayerId>),
}

#[derive(Debug, Default)]
pub struct ReducerOutput {
    renderer_plan: RendererPlanBuilder,
    pub(crate) pending_transaction: Option<PendingEditTransaction>,
    project_changed: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct PendingEditTransaction {
    composite_sync: CompositeSync,
    finalization: RendererFinalizationPayload,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RendererFinalizationPayload {
    history_transactions: Vec<PendingHistoryTransaction>,
    document_commit: Option<PendingDocumentCommit>,
    commit: RendererCommitSpec,
}

#[must_use = "prepared renderer submissions must be accepted or rolled back"]
#[derive(Debug)]
pub(crate) struct PreparedRendererSubmission {
    finalization: RendererFinalizationPayload,
}

#[must_use = "submitted renderer transactions must be completed or rolled back"]
#[derive(Debug)]
pub(crate) struct SubmittedRendererTransaction {
    id: RenderCommitId,
    finalization: RendererFinalizationPayload,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingHistoryTransaction {
    pub(crate) id: HistoryTransactionId,
    pub(crate) label: String,
    pub(crate) atoms: Vec<PendingHistoryAtom>,
    pub(crate) sealed: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum PendingHistoryAtom {
    Cpu(PendingCpuHistoryTransaction),
    Renderer(PendingRendererHistoryTransaction),
    Finalized(HistoryAtom),
}

#[must_use = "renderer submission work contains a transaction that must be accepted"]
#[derive(Debug)]
pub(crate) struct RendererSubmissionWork {
    pub(crate) renderer_plan: RendererFramePlan,
    pub(crate) submission: PreparedRendererSubmission,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingDocumentCommit {
    Selection { after: ActiveSelection },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RendererCommitSpec {
    pub finalized_surfaces: Vec<PaintSurfaceId>,
    pub selection: Option<SelectionCommitSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectionCommitSpec {
    pub before: ActiveSelection,
    pub after: ActiveSelection,
}

impl ReducerOutput {
    pub(crate) fn with_renderer_plan(renderer_plan: RendererPlanBuilder) -> Self {
        Self {
            renderer_plan,
            ..Self::default()
        }
    }

    pub(crate) fn into_renderer_plan(self) -> RendererPlanBuilder {
        self.renderer_plan
    }

    pub(crate) fn has_renderer_work(&self) -> bool {
        !self.renderer_plan.is_empty()
    }

    pub(crate) fn project_changed(&self) -> bool {
        self.project_changed
    }

    pub(crate) fn with_project_change(mut self) -> Self {
        self.project_changed = true;
        self
    }

    pub(crate) fn with_optional_cpu_history_transaction(
        mut self,
        history_transaction: Option<PendingCpuHistoryTransaction>,
    ) -> Self {
        if let Some(history_transaction) = history_transaction {
            self.project_changed = true;
            self.pending_transaction_mut()
                .append_cpu_history_transaction(history_transaction);
        }
        self
    }

    pub(crate) fn with_optional_named_cpu_history_transaction(
        mut self,
        label: impl Into<String>,
        history_transaction: Option<PendingCpuHistoryTransaction>,
    ) -> Self {
        if let Some(history_transaction) = history_transaction {
            self.project_changed = true;
            self.pending_transaction_mut()
                .append_named_cpu_history_transaction(label, history_transaction);
        }
        self
    }

    pub(crate) fn with_optional_renderer_history_transaction(
        mut self,
        label: impl Into<String>,
        history_transaction: Option<PendingRendererHistoryTransaction>,
    ) -> Self {
        if let Some(history_transaction) = history_transaction {
            self.pending_transaction_mut()
                .append_renderer_history_transaction(label, history_transaction);
        }
        self
    }

    pub(crate) fn with_finalized_history_atom(
        mut self,
        label: impl Into<String>,
        atom: HistoryAtom,
    ) -> Self {
        self.project_changed = true;
        self.pending_transaction_mut()
            .append_finalized_history_atom(label, atom);
        self
    }

    pub(crate) fn relabel_last_history_transaction(mut self, label: impl Into<String>) -> Self {
        if let Some(transaction) = self
            .pending_transaction
            .as_mut()
            .and_then(|pending| pending.finalization.history_transactions.last_mut())
            && !transaction.sealed
        {
            transaction.label = label.into();
        }
        self
    }

    pub(crate) fn with_composite_sync(mut self, composite_sync: CompositeSync) -> Self {
        if composite_sync != CompositeSync::None {
            self.pending_transaction_mut()
                .append_composite_sync(composite_sync);
        }
        self
    }

    pub(crate) fn with_renderer_commit(mut self, renderer_commit: RendererCommitSpec) -> Self {
        if !renderer_commit.is_empty() {
            self.pending_transaction_mut()
                .append_renderer_commit(renderer_commit);
        }
        self
    }

    pub(crate) fn with_document_commit(mut self, document_commit: PendingDocumentCommit) -> Self {
        self.pending_transaction_mut()
            .append_document_commit(document_commit);
        self
    }

    pub(crate) fn push_document_command(&mut self, command: GpuDocumentCommand) {
        self.renderer_plan.push_document_command(command);
    }

    pub(crate) fn push_edit_command(&mut self, command: EditCommand) {
        self.renderer_plan.push_edit_command(command);
    }

    pub(crate) fn append_outcome(&mut self, mut other: ReducerOutput) {
        self.renderer_plan.append(other.renderer_plan);
        self.project_changed |= other.project_changed;
        if let Some(pending_transaction) = other.pending_transaction.take() {
            self.pending_transaction_mut()
                .append_transaction(pending_transaction);
        }
    }

    fn pending_transaction_mut(&mut self) -> &mut PendingEditTransaction {
        self.pending_transaction
            .get_or_insert_with(PendingEditTransaction::default)
    }
}

impl PendingEditTransaction {
    #[cfg(test)]
    pub(crate) fn append_history_transaction(
        &mut self,
        label: impl Into<String>,
        atoms: Vec<PendingHistoryAtom>,
    ) {
        self.finalization
            .history_transactions
            .push(PendingHistoryTransaction {
                id: HistoryTransactionId::next(),
                label: label.into(),
                atoms,
                sealed: false,
            });
    }
    pub(crate) fn append_transaction(&mut self, transaction: Self) {
        let Self {
            composite_sync,
            finalization,
        } = transaction;
        self.append_composite_sync(composite_sync);
        self.finalization.append(finalization);
    }

    pub(crate) fn append_cpu_history_transaction(
        &mut self,
        transaction: PendingCpuHistoryTransaction,
    ) {
        let label = transaction.label();
        self.append_history_atom(label, PendingHistoryAtom::Cpu(transaction));
    }

    pub(crate) fn append_named_cpu_history_transaction(
        &mut self,
        label: impl Into<String>,
        transaction: PendingCpuHistoryTransaction,
    ) {
        let label = label.into();
        self.append_history_atom(&label, PendingHistoryAtom::Cpu(transaction));
    }

    pub(crate) fn append_renderer_history_transaction(
        &mut self,
        label: impl Into<String>,
        transaction: PendingRendererHistoryTransaction,
    ) {
        let label = label.into();
        self.append_history_atom(&label, PendingHistoryAtom::Renderer(transaction));
    }

    pub(crate) fn append_finalized_history_atom(
        &mut self,
        label: impl Into<String>,
        atom: HistoryAtom,
    ) {
        let label = label.into();
        self.append_history_atom(&label, PendingHistoryAtom::Finalized(atom));
    }

    pub(crate) fn append_document_commit(&mut self, commit: PendingDocumentCommit) {
        self.finalization.append_document_commit(commit);
    }

    pub(crate) fn append_composite_sync(&mut self, composite_sync: CompositeSync) {
        self.composite_sync = std::mem::take(&mut self.composite_sync).merge(composite_sync);
    }

    pub(crate) fn append_renderer_commit(&mut self, commit: RendererCommitSpec) {
        self.finalization.commit.append_transaction(commit);
    }

    fn append_history_atom(&mut self, label: &str, atom: PendingHistoryAtom) {
        if let Some(transaction) = self.finalization.history_transactions.last_mut()
            && !transaction.sealed
            && transaction.label == label
        {
            append_adjacent_history_atom(&mut transaction.atoms, atom);
            return;
        }
        self.finalization
            .history_transactions
            .push(PendingHistoryTransaction {
                id: HistoryTransactionId::next(),
                label: label.to_owned(),
                atoms: vec![atom],
                sealed: false,
            });
    }

    pub(crate) fn take_composite_sync(&mut self) -> CompositeSync {
        std::mem::take(&mut self.composite_sync)
    }

    pub(crate) fn take_renderer_submission(&mut self) -> PreparedRendererSubmission {
        PreparedRendererSubmission {
            finalization: std::mem::take(&mut self.finalization),
        }
    }

    pub(crate) fn history_transactions_mut(&mut self) -> &mut Vec<PendingHistoryTransaction> {
        &mut self.finalization.history_transactions
    }

    #[cfg(test)]
    pub(crate) fn history_transactions(&self) -> &[PendingHistoryTransaction] {
        &self.finalization.history_transactions
    }

    pub(crate) fn seal_history_transactions(&mut self) {
        for transaction in &mut self.finalization.history_transactions {
            transaction.sealed = true;
        }
    }

    pub(crate) fn has_pending_renderer_finalization(&self) -> bool {
        !self.finalization.commit.is_empty()
            || self.finalization.document_commit.is_some()
            || self
                .finalization
                .history_transactions
                .iter()
                .any(|transaction| {
                    transaction
                        .atoms
                        .iter()
                        .any(|atom| matches!(atom, PendingHistoryAtom::Renderer(_)))
                })
    }

    #[cfg(test)]
    pub(crate) fn has_pending_history_transaction(&self) -> bool {
        !self.finalization.history_transactions.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn renderer_history_transaction(
        &self,
    ) -> Option<&PendingRendererHistoryTransaction> {
        self.finalization
            .history_transactions
            .iter()
            .find_map(|transaction| {
                transaction.atoms.iter().find_map(|atom| match atom {
                    PendingHistoryAtom::Renderer(pending) => Some(pending),
                    _ => None,
                })
            })
    }

    #[cfg(test)]
    pub(crate) fn renderer_commit_request(&self) -> Option<CommitRequest> {
        self.finalization.commit_request()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

impl PendingEditTransaction {
    pub(crate) fn finalization(&self) -> &RendererFinalizationPayload {
        &self.finalization
    }

    pub(crate) fn append_finalization(&mut self, finalization: RendererFinalizationPayload) {
        self.finalization.append(finalization);
    }
}

impl RendererFinalizationPayload {
    fn append(&mut self, finalization: Self) {
        let Self {
            history_transactions,
            document_commit,
            commit,
        } = finalization;
        self.history_transactions.extend(history_transactions);
        if let Some(document_commit) = document_commit {
            self.append_document_commit(document_commit);
        }
        self.commit.append_transaction(commit);
    }

    fn append_document_commit(&mut self, commit: PendingDocumentCommit) {
        self.document_commit = merge_document_commits(self.document_commit.take(), commit);
    }

    pub(crate) fn take_document_commit(&mut self) -> Option<PendingDocumentCommit> {
        self.document_commit.take()
    }

    pub(crate) fn take_commit(&mut self) -> RendererCommitSpec {
        std::mem::take(&mut self.commit)
    }

    pub(crate) fn history_transactions_mut(&mut self) -> &mut Vec<PendingHistoryTransaction> {
        &mut self.history_transactions
    }

    pub(crate) fn history_transactions(&self) -> &[PendingHistoryTransaction] {
        &self.history_transactions
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.history_transactions.is_empty()
            && self.document_commit.is_none()
            && self.commit.is_empty()
    }

    pub(crate) fn commit_request(&self) -> Option<CommitRequest> {
        (!self.commit.is_empty()).then(|| CommitRequest {
            finalized_surfaces: self.commit.finalized_surfaces.clone(),
            selection: self
                .commit
                .selection
                .clone()
                .map(|selection| SelectionCommitRequest {
                    before: selection.before,
                    after: selection.after,
                }),
        })
    }
}

impl PreparedRendererSubmission {
    pub(crate) fn is_empty(&self) -> bool {
        self.finalization.is_empty()
    }

    pub(crate) fn commit_request(&self) -> Option<CommitRequest> {
        self.finalization.commit_request()
    }

    pub(crate) fn into_finalization(self) -> RendererFinalizationPayload {
        self.finalization
    }

    pub(crate) fn into_submitted(self, id: RenderCommitId) -> SubmittedRendererTransaction {
        assert!(
            !self.is_empty(),
            "empty renderer submissions cannot enter the readback state"
        );
        SubmittedRendererTransaction {
            id,
            finalization: self.finalization,
        }
    }
}

impl SubmittedRendererTransaction {
    pub(crate) fn id(&self) -> RenderCommitId {
        self.id
    }

    pub(crate) fn into_finalization(self) -> RendererFinalizationPayload {
        self.finalization
    }
}

fn append_adjacent_history_atom(atoms: &mut Vec<PendingHistoryAtom>, incoming: PendingHistoryAtom) {
    let Some(last) = atoms.last_mut() else {
        atoms.push(incoming);
        return;
    };
    match (last, incoming) {
        (
            PendingHistoryAtom::Renderer(PendingRendererHistoryTransaction::Pixel { edits }),
            PendingHistoryAtom::Renderer(PendingRendererHistoryTransaction::Pixel {
                edits: mut incoming,
            }),
        ) => {
            incoming.retain(|next| {
                !edits
                    .iter()
                    .any(|edit| edit.surface == next.surface && edit.rect == next.rect)
            });
            edits.append(&mut incoming);
        }
        (
            PendingHistoryAtom::Renderer(PendingRendererHistoryTransaction::Selection {
                after,
                ..
            }),
            PendingHistoryAtom::Renderer(PendingRendererHistoryTransaction::Selection {
                after: incoming_after,
                ..
            }),
        ) => *after = incoming_after,
        (_, incoming) => atoms.push(incoming),
    }
}

impl CompositeSync {
    pub(crate) fn material_trees_scoped(
        material_indices: impl IntoIterator<Item = MaterialIndex>,
    ) -> Self {
        Self::MaterialTreesScoped(material_indices.into_iter().collect())
    }

    pub(crate) fn layer_props(layer_id: LayerId) -> Self {
        Self::LayerProps(BTreeSet::from([layer_id]))
    }

    pub(crate) fn adjustment(layer_id: LayerId) -> Self {
        Self::AdjustmentLayers(BTreeSet::from([layer_id]))
    }

    pub(crate) fn merge(self, incoming: Self) -> Self {
        match (self, incoming) {
            (_, CompositeSync::MaterialTrees) | (CompositeSync::MaterialTrees, _) => {
                CompositeSync::MaterialTrees
            }
            (
                CompositeSync::MaterialTreesScoped(mut existing),
                CompositeSync::MaterialTreesScoped(incoming),
            ) => {
                existing.extend(incoming);
                CompositeSync::MaterialTreesScoped(existing)
            }
            (CompositeSync::LayerProps(mut existing), CompositeSync::LayerProps(incoming)) => {
                existing.extend(incoming);
                CompositeSync::LayerProps(existing)
            }
            (
                CompositeSync::AdjustmentLayers(mut existing),
                CompositeSync::AdjustmentLayers(incoming),
            ) => {
                existing.extend(incoming);
                CompositeSync::AdjustmentLayers(existing)
            }
            (CompositeSync::None, incoming) => incoming,
            (existing, CompositeSync::None) => existing,
            (CompositeSync::LayerProps(_), CompositeSync::AdjustmentLayers(_))
            | (CompositeSync::AdjustmentLayers(_), CompositeSync::LayerProps(_))
            | (CompositeSync::MaterialTreesScoped(_), CompositeSync::LayerProps(_))
            | (CompositeSync::LayerProps(_), CompositeSync::MaterialTreesScoped(_))
            | (CompositeSync::MaterialTreesScoped(_), CompositeSync::AdjustmentLayers(_))
            | (CompositeSync::AdjustmentLayers(_), CompositeSync::MaterialTreesScoped(_)) => {
                CompositeSync::MaterialTrees
            }
        }
    }
}

fn merge_document_commits(
    _existing: Option<PendingDocumentCommit>,
    incoming: PendingDocumentCommit,
) -> Option<PendingDocumentCommit> {
    match incoming {
        PendingDocumentCommit::Selection { after } => {
            Some(PendingDocumentCommit::Selection { after })
        }
    }
}

impl RendererCommitSpec {
    pub fn with_finalized_surfaces(finalized_surfaces: Vec<PaintSurfaceId>) -> Self {
        Self {
            finalized_surfaces,
            ..Self::default()
        }
    }

    pub fn with_selection_commit(before: ActiveSelection, after: ActiveSelection) -> Self {
        Self {
            selection: Some(SelectionCommitSpec { before, after }),
            ..Self::default()
        }
    }

    pub fn append_transaction(&mut self, mut other: Self) {
        self.finalized_surfaces
            .append(&mut other.finalized_surfaces);
        if other.selection.is_some() {
            self.selection = other.selection.take();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.finalized_surfaces.is_empty() && self.selection.is_none()
    }
}
