use std::collections::HashSet;

use anyhow::{Result, anyhow};

use crate::{
    application::{AppState, PendingRendererHistoryTransaction, RectU32, SurfaceCommit},
    core::surface::PaintSurfaceId,
};

#[derive(Debug)]
pub(crate) struct PreparedSurfaceCommits {
    commits: Vec<PreparedSurfaceCommit>,
}

impl PreparedSurfaceCommits {
    pub(crate) fn changes_document(&self) -> bool {
        self.commits
            .iter()
            .any(|commit| commit.before.after != commit.after.after)
    }
}

#[derive(Debug)]
struct PreparedSurfaceCommit {
    after: SurfaceCommit,
    before: SurfaceCommit,
}

pub(crate) fn prepare_finalized_surface_commits(
    state: &AppState,
    finalized_surfaces: &[PaintSurfaceId],
    surface_commits: &[SurfaceCommit],
    pending_transactions: &[&PendingRendererHistoryTransaction],
) -> Result<PreparedSurfaceCommits> {
    validate_pending_pixel_commits(pending_transactions, finalized_surfaces, surface_commits)?;

    if finalized_surfaces.is_empty() || surface_commits.is_empty() {
        return Ok(PreparedSurfaceCommits {
            commits: Vec::new(),
        });
    }

    let finalized: HashSet<_> = finalized_surfaces.iter().copied().collect();
    let Some(document) = state.document() else {
        return Ok(PreparedSurfaceCommits {
            commits: Vec::new(),
        });
    };

    let applicable_commits = surface_commits
        .iter()
        .filter(|commit| finalized.contains(&commit.surface))
        .collect::<Vec<_>>();
    let mut commits = Vec::with_capacity(applicable_commits.len());
    for commit in &applicable_commits {
        document
            .tiles
            .validate_surface_rect_write(commit.surface, commit.rect, &commit.after)
            .map_err(|err| anyhow!("Pixel commit validation failed: {err:#}"))?;
        let before = document
            .tiles
            .read_surface_rect(commit.surface, commit.rect)
            .map_err(|err| anyhow!("Pixel commit rollback snapshot failed: {err:#}"))?;
        commits.push(PreparedSurfaceCommit {
            after: (*commit).clone(),
            before: SurfaceCommit {
                surface: commit.surface,
                rect: commit.rect,
                after: before,
            },
        });
    }

    Ok(PreparedSurfaceCommits { commits })
}

pub(crate) fn apply_prepared_surface_commits_to_document(
    state: &mut AppState,
    prepared: &PreparedSurfaceCommits,
) -> Result<()> {
    let Some(document) = state.document_mut() else {
        return Ok(());
    };

    let mut applied = Vec::new();
    for prepared_commit in &prepared.commits {
        if let Err(err) = document.tiles.write_surface_rect(
            prepared_commit.after.surface,
            prepared_commit.after.rect,
            &prepared_commit.after.after,
        ) {
            let rollback_error = rollback_surface_commits(document, &applied);
            if let Some(rollback_error) = rollback_error {
                return Err(anyhow!(
                    "Pixel commit write failed: {err:#}; rollback failed: {rollback_error:#}"
                ));
            }
            return Err(anyhow!("Pixel commit write failed: {err:#}"));
        }
        applied.push(prepared_commit);
    }

    Ok(())
}

fn rollback_surface_commits(
    document: &mut crate::core::document::Document,
    applied: &[&PreparedSurfaceCommit],
) -> Option<anyhow::Error> {
    for prepared_commit in applied.iter().rev() {
        if let Err(err) = document
            .tiles
            .write_surface_rect(
                prepared_commit.before.surface,
                prepared_commit.before.rect,
                &prepared_commit.before.after,
            )
            .map_err(|err| anyhow!("{err:#}"))
        {
            return Some(err);
        }
    }
    None
}

fn validate_pending_pixel_commits(
    pending_transactions: &[&PendingRendererHistoryTransaction],
    finalized_surfaces: &[PaintSurfaceId],
    surface_commits: &[SurfaceCommit],
) -> Result<()> {
    if finalized_surfaces.is_empty() {
        return Ok(());
    }
    for edit in pending_transactions
        .iter()
        .filter_map(|pending| match pending {
            PendingRendererHistoryTransaction::Pixel { edits } => Some(edits.as_slice()),
            PendingRendererHistoryTransaction::Selection { .. }
            | PendingRendererHistoryTransaction::ImportedLayer { .. } => None,
        })
        .flatten()
    {
        if !finalized_surfaces.contains(&edit.surface) {
            continue;
        }
        if surface_commits
            .iter()
            .any(|commit| commit.surface == edit.surface && rect_contains(commit.rect, edit.rect))
        {
            continue;
        }
        anyhow::bail!(
            "Renderer did not provide surface commit for finalized surface {:?} rect origin {:?} size {:?}",
            edit.surface,
            edit.rect.origin,
            edit.rect.size
        );
    }
    Ok(())
}

fn rect_contains(outer: RectU32, inner: RectU32) -> bool {
    let Some(outer_end) = rect_end(outer) else {
        return false;
    };
    let Some(inner_end) = rect_end(inner) else {
        return false;
    };
    outer.origin[0] <= inner.origin[0]
        && outer.origin[1] <= inner.origin[1]
        && inner_end[0] <= outer_end[0]
        && inner_end[1] <= outer_end[1]
}

fn rect_end(rect: RectU32) -> Option<[u32; 2]> {
    Some([
        rect.origin[0].checked_add(rect.size[0])?,
        rect.origin[1].checked_add(rect.size[1])?,
    ])
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{
            AppState, PendingPixelEdit, PendingRendererHistoryTransaction, PixelSnapshotData,
            RectU32, SurfaceCommit,
        },
        core::document::{Document, MaterialSpec, MeshData},
    };

    fn state_with_one_material() -> AppState {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("M", [2, 2])],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document.document.as_ref().unwrap(),
        );
        state
    }

    fn empty_mesh() -> MeshData {
        MeshData::empty()
    }

    fn surface_commit(
        surface: crate::core::surface::PaintSurfaceId,
        rect: RectU32,
        value: u8,
    ) -> SurfaceCommit {
        SurfaceCommit {
            surface,
            rect,
            after: PixelSnapshotData::contiguous(vec![
                value;
                rect.size[0] as usize
                    * rect.size[1] as usize
                    * 4
            ]),
        }
    }

    #[test]
    fn finalized_surface_commits_update_document_without_pending_history() {
        let mut state = state_with_one_material();
        let surface = state.active_paint_surface().unwrap();
        let rect = RectU32::full([2, 2]);
        let commits = vec![surface_commit(surface, rect, 9)];

        let prepared =
            super::prepare_finalized_surface_commits(&state, &[surface], &commits, &[]).unwrap();
        assert_eq!(
            state
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
        super::apply_prepared_surface_commits_to_document(&mut state, &prepared).unwrap();

        assert_eq!(
            state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_full(surface)
                .unwrap()
                .rgba8,
            vec![9; 16]
        );
    }

    #[test]
    fn surface_commits_ignore_unfinalized_surfaces() {
        let mut state = state_with_one_material();
        let surface = state.active_paint_surface().unwrap();
        let rect = RectU32::full([2, 2]);
        let commits = vec![surface_commit(surface, rect, 9)];

        let prepared =
            super::prepare_finalized_surface_commits(&state, &[], &commits, &[]).unwrap();
        super::apply_prepared_surface_commits_to_document(&mut state, &prepared).unwrap();

        assert_eq!(
            state
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
    fn missing_required_commit_does_not_mutate_document_tiles() {
        let state = state_with_one_material();
        let surface = state.active_paint_surface().unwrap();
        let pending = PendingRendererHistoryTransaction::Pixel {
            edits: vec![PendingPixelEdit {
                surface,
                texture_size: [2, 2],
                rect: RectU32::full([2, 2]),
                before: PixelSnapshotData::contiguous(vec![0; 16]),
            }],
        };
        let partial_rect = RectU32 {
            origin: [0, 0],
            size: [1, 1],
        };
        let commits = vec![surface_commit(surface, partial_rect, 9)];

        let result =
            super::prepare_finalized_surface_commits(&state, &[surface], &commits, &[&pending]);

        assert!(matches!(
            result,
            Err(err) if format!("{err:#}").contains("Renderer did not provide surface commit")
        ));
        assert_eq!(
            state
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
}
