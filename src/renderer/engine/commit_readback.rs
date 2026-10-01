use std::sync::mpsc::{self, Receiver, TryRecvError};

use anyhow::{Result, anyhow, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        geometry::RectU32,
        tile::TileGrid,
        tile_payload::{
            PIXEL_HISTORY_TILE_SIZE, PixelBytes, PixelSnapshotData, PixelSnapshotStorageKind,
            TilePayload, TiledRgbaSnapshot, choose_pixel_snapshot_storage, intersect_rects,
        },
    },
    renderer::{
        document::surfaces::SurfacePixelFormat,
        frame::CommitRequest,
        gpu::frame::GpuFrame,
        readback::{
            SelectionCommitReadbackRequest, SurfaceCommitReadbackRequest,
            selection_commit_readback_requests_from_selection,
            surface_commit_readback_requests_from_damage,
        },
        report::DiagnosticReadbackRequest,
        result::{
            CompletedRenderCommit, RenderCommitArtifacts, RenderCommitId, RendererDiagnostic,
            SelectionCommit, StartedRenderCommit, SurfaceCommit,
        },
    },
};

use super::core::RenderEngine;

pub(super) struct PendingCommitReadback {
    id: RenderCommitId,
    surface_readbacks: Vec<PendingSurfaceReadback>,
    selection_readbacks: Vec<PendingSelectionReadback>,
    diagnostic_readbacks: Vec<PendingDiagnosticReadback>,
}

struct PendingSurfaceReadback {
    surface: crate::core::surface::PaintSurfaceId,
    rect: RectU32,
    texture_size: [u32; 2],
    pixel_format: SurfacePixelFormat,
    buffer: wgpu::Buffer,
    padded_bytes_per_row: u32,
    state: PendingMapState,
}

struct PendingDiagnosticReadback {
    kind: PendingDiagnosticKind,
    buffer: wgpu::Buffer,
    state: PendingMapState,
}

enum PendingDiagnosticKind {
    SpatialBlur {
        target_stride_px: u32,
        actual_stride_px: u32,
    },
}

struct PendingSelectionReadback {
    material_index: usize,
    texture_size: [u32; 2],
    buffer: wgpu::Buffer,
    padded_bytes_per_row: u32,
    state: PendingMapState,
}

enum PendingMapState {
    Waiting(Receiver<Result<(), wgpu::BufferAsyncError>>),
    Ready,
    Failed(String),
}

impl RenderEngine {
    pub(super) fn enqueue_surface_commit_readbacks(
        &mut self,
        frame: &mut GpuFrame,
        mutations: &crate::renderer::mutation::MutationLog,
        commit_request: Option<&CommitRequest>,
        diagnostic_requests: Vec<DiagnosticReadbackRequest>,
    ) -> Result<Option<StartedRenderCommit>> {
        let Some(commit_request) = commit_request else {
            return Ok(None);
        };
        if commit_request.is_empty() {
            return Ok(None);
        }
        self.ensure_no_outstanding_commit()?;
        let requests = surface_commit_readback_requests_from_damage(
            mutations.surface_commits.iter(),
            |surface| {
                self.state
                    .document
                    .surfaces
                    .surface_tile_shadow_stale_full(surface)
            },
        )
        .into_iter()
        .filter(|request| commit_request.finalized_surfaces.contains(&request.surface))
        .collect::<Vec<_>>();
        let selection_requests = self.commit_selection_after_requests(commit_request);
        if requests.is_empty() && selection_requests.is_empty() && diagnostic_requests.is_empty() {
            return Ok(None);
        }

        let id = self.next_commit_id();
        let mut surface_readbacks = Vec::with_capacity(requests.len());
        for request in requests {
            surface_readbacks.push(self.encode_surface_commit_readback(frame, request)?);
        }
        let mut selection_readbacks = Vec::with_capacity(selection_requests.len());
        for request in selection_requests {
            selection_readbacks.push(self.encode_selection_commit_readback(frame, request)?);
        }
        let mut diagnostic_readbacks = Vec::with_capacity(diagnostic_requests.len());
        for request in diagnostic_requests {
            diagnostic_readbacks.push(Self::encode_diagnostic_readback(frame, request));
        }
        self.pending_commit_readbacks
            .push_back(PendingCommitReadback {
                id,
                surface_readbacks,
                selection_readbacks,
                diagnostic_readbacks,
            });
        Ok(Some(StartedRenderCommit { id }))
    }

    pub(super) fn ensure_no_outstanding_commit(&self) -> Result<()> {
        if let Some(pending) = self.pending_commit_readbacks.front() {
            bail!(
                "renderer commit {:?} is still pending; a new commit cannot be enqueued",
                pending.id
            );
        }
        if let Some(commit_id) = self.awaiting_commit_acceptance {
            bail!(
                "renderer commit {:?} is awaiting acceptance; a new commit cannot be enqueued",
                commit_id
            );
        }
        Ok(())
    }

    fn commit_selection_after_requests(
        &self,
        commit_request: &CommitRequest,
    ) -> Vec<SelectionCommitReadbackRequest> {
        let Some(selection) = commit_request.selection.as_ref() else {
            return Vec::new();
        };
        selection_commit_readback_requests_from_selection(
            &selection.after,
            &self.state.document.materials,
        )
    }

    pub fn poll_completed_commits(&mut self) -> Vec<CompletedRenderCommit> {
        self.poll_completed_commits_with(wgpu::PollType::Poll)
    }

    #[cfg(test)]
    pub(crate) fn wait_completed_commits_for_test(&mut self) -> Vec<CompletedRenderCommit> {
        self.poll_completed_commits_with(wgpu::PollType::wait_indefinitely())
    }

    fn poll_completed_commits_with(
        &mut self,
        poll_type: wgpu::PollType,
    ) -> Vec<CompletedRenderCommit> {
        if self.pending_commit_readbacks.is_empty() {
            return Vec::new();
        }
        if let Err(err) = self.state.gpu.ctx.device().poll(poll_type) {
            let message = format!("GPU poll failed during async commit readback: {err:?}");
            return self.fail_all_pending_commits(message);
        }

        let mut completed = Vec::new();
        loop {
            let Some(front) = self.pending_commit_readbacks.front_mut() else {
                break;
            };
            match poll_commit(front) {
                CommitPoll::Pending => break,
                CommitPoll::Failed(error) => {
                    let id = front.id;
                    if let Some(pending) = self.pending_commit_readbacks.pop_front() {
                        pending.unmap_ready_readbacks();
                    }
                    completed.push(CompletedRenderCommit {
                        id,
                        artifacts: Err(error),
                    });
                }
                CommitPoll::Ready => {
                    let pending = self
                        .pending_commit_readbacks
                        .pop_front()
                        .expect("front was just observed");
                    completed.push(self.finish_pending_commit(pending));
                }
            }
        }
        completed
    }

    fn next_commit_id(&mut self) -> RenderCommitId {
        let id = self.next_commit_id;
        self.next_commit_id = self.next_commit_id.saturating_add(1);
        RenderCommitId(id)
    }

    fn encode_surface_commit_readback(
        &mut self,
        frame: &mut GpuFrame,
        request: SurfaceCommitReadbackRequest,
    ) -> Result<PendingSurfaceReadback> {
        let rect = match request.rect {
            Some(rect) => rect,
            None => {
                let texture_size = self
                    .state
                    .document
                    .surfaces
                    .surface_texture_size(request.surface)
                    .ok_or_else(|| {
                        anyhow!("surface texture does not exist: {:?}", request.surface)
                    })?;
                RectU32::full(texture_size)
            }
        };
        let copy = self
            .state
            .document
            .surfaces
            .encode_surface_record_readback_rgba8(
                &self.state.gpu,
                frame,
                request.surface,
                rect.origin,
                rect.size,
            )?;
        self.metrics
            .get_mut()
            .record_readback(copy.texture_size, rect.origin, rect.size);
        let (tx, rx) = mpsc::channel();
        frame.map_buffer_on_submit(&copy.buffer, wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        Ok(PendingSurfaceReadback {
            surface: request.surface,
            rect,
            texture_size: copy.texture_size,
            pixel_format: copy.pixel_format,
            buffer: copy.buffer,
            padded_bytes_per_row: copy.padded_bytes_per_row,
            state: PendingMapState::Waiting(rx),
        })
    }

    fn encode_selection_commit_readback(
        &mut self,
        frame: &mut GpuFrame,
        request: SelectionCommitReadbackRequest,
    ) -> Result<PendingSelectionReadback> {
        let copy = self
            .state
            .document
            .selections
            .encode_selection_record_readback_r8(
                self.state.gpu.ctx.device(),
                frame,
                request.mask_id,
                request.texture_size,
            )?;
        let (tx, rx) = mpsc::channel();
        frame.map_buffer_on_submit(&copy.buffer, wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        Ok(PendingSelectionReadback {
            material_index: request.material_index,
            texture_size: request.texture_size,
            buffer: copy.buffer,
            padded_bytes_per_row: copy.padded_bytes_per_row,
            state: PendingMapState::Waiting(rx),
        })
    }

    fn encode_diagnostic_readback(
        frame: &mut GpuFrame,
        request: DiagnosticReadbackRequest,
    ) -> PendingDiagnosticReadback {
        let (kind, buffer) = match request {
            DiagnosticReadbackRequest::SpatialBlur {
                buffer,
                target_stride_px,
                actual_stride_px,
            } => (
                PendingDiagnosticKind::SpatialBlur {
                    target_stride_px,
                    actual_stride_px,
                },
                buffer,
            ),
        };
        let (tx, rx) = mpsc::channel();
        frame.map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        PendingDiagnosticReadback {
            kind,
            buffer,
            state: PendingMapState::Waiting(rx),
        }
    }

    fn finish_pending_commit(&mut self, pending: PendingCommitReadback) -> CompletedRenderCommit {
        let id = pending.id;
        let artifacts = self.finish_pending_commit_artifacts(pending);
        if artifacts.is_ok() {
            self.awaiting_commit_acceptance = Some(id);
        }
        CompletedRenderCommit { id, artifacts }
    }

    fn finish_pending_commit_artifacts(
        &mut self,
        pending: PendingCommitReadback,
    ) -> Result<RenderCommitArtifacts, String> {
        let mut surface_commits = Vec::with_capacity(pending.surface_readbacks.len());
        let mut selection_after_commits = Vec::with_capacity(pending.selection_readbacks.len());
        let mut diagnostics = Vec::with_capacity(pending.diagnostic_readbacks.len());
        let result: Result<(), String> = (|| {
            for readback in &pending.surface_readbacks {
                let after = pixel_snapshot_data_from_mapped_readback(readback)
                    .map_err(|err| format!("{err:#}"))?;
                surface_commits.push(SurfaceCommit {
                    surface: readback.surface,
                    rect: readback.rect,
                    after,
                });
            }
            for readback in &pending.selection_readbacks {
                let r8 = selection_r8_from_mapped_readback(readback)
                    .map_err(|err| format!("{err:#}"))?;
                selection_after_commits.push(SelectionCommit {
                    material_index: readback.material_index.into(),
                    texture_size: readback.texture_size,
                    r8,
                });
            }
            for readback in &pending.diagnostic_readbacks {
                diagnostics.push(diagnostic_from_mapped_readback(readback)?);
            }
            Ok(())
        })();
        pending.unmap_ready_readbacks();
        result?;
        Ok(RenderCommitArtifacts {
            surface_commits,
            selection_before_commits: Vec::new(),
            selection_after_commits,
            diagnostics,
        })
    }

    fn fail_all_pending_commits(&mut self, error: String) -> Vec<CompletedRenderCommit> {
        self.pending_commit_readbacks
            .drain(..)
            .map(|pending| {
                pending.unmap_ready_readbacks();
                CompletedRenderCommit {
                    id: pending.id,
                    artifacts: Err(error.clone()),
                }
            })
            .collect()
    }

    pub fn accept_commit(
        &mut self,
        commit_id: RenderCommitId,
        artifacts: &RenderCommitArtifacts,
    ) -> Result<(), String> {
        let Some(expected_commit_id) = self.awaiting_commit_acceptance else {
            return Err(format!(
                "renderer accepted commit {:?} but no completed commit is awaiting acceptance",
                commit_id
            ));
        };
        if expected_commit_id != commit_id {
            return Err(format!(
                "renderer accepted commit out of order: expected {:?}, got {:?}",
                expected_commit_id, commit_id
            ));
        }
        self.awaiting_commit_acceptance = None;

        for (index, commit) in artifacts.surface_commits.iter().enumerate() {
            if let Err(err) = self.state.document.surfaces.apply_surface_readback_data(
                commit.surface,
                commit.rect.origin,
                commit.rect.size,
                &commit.after,
            ) {
                for surface in
                    stale_surfaces_after_shadow_apply_failure(&artifacts.surface_commits, index)
                {
                    self.state
                        .document
                        .surfaces
                        .mark_surface_tile_cache_stale(surface);
                }
                return Err(format!("surface tile shadow update failed: {err:#}"));
            }
        }
        for commit in &artifacts.selection_after_commits {
            if let Err(err) = self
                .state
                .document
                .selections
                .apply_selection_readback_data(
                    commit.material_index.as_usize(),
                    commit.texture_size,
                    &commit.r8,
                )
            {
                return Err(format!("selection mask shadow update failed: {err:#}"));
            }
        }
        Ok(())
    }
}

fn diagnostic_from_mapped_readback(
    readback: &PendingDiagnosticReadback,
) -> Result<RendererDiagnostic, String> {
    let bytes = readback
        .buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| format!("diagnostic readback mapped range failed: {error}"))?;
    if bytes.len() < 16 {
        return Err("Spatial Blur diagnostic readback is shorter than 16 bytes".to_owned());
    }
    let read_u32 = |offset: usize| {
        u32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("four-byte slice"),
        )
    };
    let traversal_overflow_texels = read_u32(4);
    let capacity_overflow_samples = read_u32(8);
    match readback.kind {
        PendingDiagnosticKind::SpatialBlur {
            target_stride_px,
            actual_stride_px,
        } => Ok(RendererDiagnostic::SpatialBlur {
            target_stride_px,
            actual_stride_px,
            traversal_overflow_texels,
            capacity_overflow_samples,
        }),
    }
}

fn stale_surfaces_after_shadow_apply_failure(
    surface_commits: &[SurfaceCommit],
    failed_index: usize,
) -> impl Iterator<Item = crate::core::surface::PaintSurfaceId> + '_ {
    surface_commits[failed_index..]
        .iter()
        .map(|commit| commit.surface)
}

impl PendingCommitReadback {
    fn unmap_ready_readbacks(&self) {
        for readback in &self.surface_readbacks {
            readback.unmap_if_ready();
        }
        for readback in &self.selection_readbacks {
            readback.unmap_if_ready();
        }
        for readback in &self.diagnostic_readbacks {
            readback.unmap_if_ready();
        }
    }
}

impl PendingSurfaceReadback {
    fn unmap_if_ready(&self) {
        if matches!(self.state, PendingMapState::Ready) {
            self.buffer.unmap();
        }
    }
}

impl PendingDiagnosticReadback {
    fn unmap_if_ready(&self) {
        if matches!(self.state, PendingMapState::Ready) {
            self.buffer.unmap();
        }
    }
}

impl PendingSelectionReadback {
    fn unmap_if_ready(&self) {
        if matches!(self.state, PendingMapState::Ready) {
            self.buffer.unmap();
        }
    }
}

enum CommitPoll {
    Pending,
    Ready,
    Failed(String),
}

fn poll_commit(pending: &mut PendingCommitReadback) -> CommitPoll {
    for readback in &mut pending.surface_readbacks {
        match poll_map_state(&mut readback.state) {
            CommitPoll::Ready => {}
            other => return other,
        }
    }
    for readback in &mut pending.selection_readbacks {
        match poll_map_state(&mut readback.state) {
            CommitPoll::Ready => {}
            other => return other,
        }
    }
    for readback in &mut pending.diagnostic_readbacks {
        match poll_map_state(&mut readback.state) {
            CommitPoll::Ready => {}
            other => return other,
        }
    }
    CommitPoll::Ready
}

fn poll_map_state(state: &mut PendingMapState) -> CommitPoll {
    match state {
        PendingMapState::Ready => CommitPoll::Ready,
        PendingMapState::Failed(error) => CommitPoll::Failed(error.clone()),
        PendingMapState::Waiting(rx) => match rx.try_recv() {
            Ok(Ok(())) => {
                *state = PendingMapState::Ready;
                CommitPoll::Ready
            }
            Ok(Err(err)) => {
                let error = format!("texture readback mapping failed: {err:?}");
                *state = PendingMapState::Failed(error.clone());
                CommitPoll::Failed(error)
            }
            Err(TryRecvError::Empty) => CommitPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                let error = "texture readback callback dropped".to_owned();
                *state = PendingMapState::Failed(error.clone());
                CommitPoll::Failed(error)
            }
        },
    }
}

fn selection_r8_from_mapped_readback(readback: &PendingSelectionReadback) -> Result<Vec<u8>> {
    let mapped = readback.buffer.slice(..).get_mapped_range()?;
    let width = readback.texture_size[0] as usize;
    let height = readback.texture_size[1] as usize;
    let padded = readback.padded_bytes_per_row as usize;
    let mut r8 = vec![0; width * height];
    for row in 0..height {
        let src_start = row * padded;
        let src_end = src_start + width;
        let dst_start = row * width;
        if src_end > mapped.len() {
            bail!("selection R8 readback copy range is outside buffer bounds");
        }
        r8[dst_start..dst_start + width].copy_from_slice(&mapped[src_start..src_end]);
    }
    Ok(r8)
}

fn pixel_snapshot_data_from_mapped_readback(
    readback: &PendingSurfaceReadback,
) -> Result<PixelSnapshotData> {
    let rect = readback.rect;
    let rgba8_len = rgba8_len(rect.size)?;
    match choose_pixel_snapshot_storage(rect, rgba8_len) {
        PixelSnapshotStorageKind::Contiguous => {
            let mapped = readback.buffer.slice(..).get_mapped_range()?;
            Ok(PixelSnapshotData::contiguous(
                copy_mapped_readback_rect_with_format(
                    &mapped,
                    readback.pixel_format,
                    rect.size,
                    readback.padded_bytes_per_row,
                    0,
                    0,
                )?,
            ))
        }
        PixelSnapshotStorageKind::Tiled => {
            let mapped = readback.buffer.slice(..).get_mapped_range()?;
            Ok(PixelSnapshotData::Tiled(
                tiled_snapshot_from_mapped_readback(readback, &mapped)?,
            ))
        }
    }
}

fn tiled_snapshot_from_mapped_readback(
    readback: &PendingSurfaceReadback,
    mapped: &[u8],
) -> Result<TiledRgbaSnapshot> {
    tiled_snapshot_from_mapped_rect(
        mapped,
        readback.pixel_format,
        readback.texture_size,
        readback.rect,
        readback.padded_bytes_per_row,
        PIXEL_HISTORY_TILE_SIZE,
    )
}

fn tiled_snapshot_from_mapped_rect(
    mapped: &[u8],
    pixel_format: SurfacePixelFormat,
    texture_size: [u32; 2],
    rect: RectU32,
    padded_bytes_per_row: u32,
    tile_size: u32,
) -> Result<TiledRgbaSnapshot> {
    let grid = TileGrid::new(texture_size, tile_size);
    let mut tiles = Vec::new();
    for coord in grid.tiles_for_rect(rect) {
        let tile_rect = grid.rect_for_tile(coord);
        let Some(payload_rect) = intersect_rects(rect, tile_rect) else {
            continue;
        };
        let source_x = payload_rect.origin[0] - rect.origin[0];
        let source_y = payload_rect.origin[1] - rect.origin[1];
        tiles.push(TilePayload {
            coord,
            rect: payload_rect,
            rgba8: PixelBytes::from_vec(copy_mapped_readback_rect_with_format(
                mapped,
                pixel_format,
                payload_rect.size,
                padded_bytes_per_row,
                source_x,
                source_y,
            )?),
        });
    }
    tiles.sort_by_key(|tile| (tile.coord.y, tile.coord.x));
    Ok(TiledRgbaSnapshot {
        texture_size,
        tile_size,
        origin: rect.origin,
        size: rect.size,
        tiles,
    })
}

#[cfg(test)]
fn copy_mapped_readback_rect(
    mapped: &[u8],
    size: [u32; 2],
    padded_bytes_per_row: u32,
    source_x: u32,
    source_y: u32,
) -> Result<Vec<u8>> {
    copy_mapped_readback_rect_with_format(
        mapped,
        SurfacePixelFormat::Rgba8,
        size,
        padded_bytes_per_row,
        source_x,
        source_y,
    )
}

fn copy_mapped_readback_rect_with_format(
    mapped: &[u8],
    pixel_format: SurfacePixelFormat,
    size: [u32; 2],
    padded_bytes_per_row: u32,
    source_x: u32,
    source_y: u32,
) -> Result<Vec<u8>> {
    let row_bytes = row_stride(size[0])?;
    let mut rgba8 = vec![0; rgba8_len(size)?];
    for row in 0..size[1] as usize {
        let source_row = (source_y as usize)
            .checked_add(row)
            .ok_or_else(|| anyhow!("source row overflows"))?;
        let dst_start = row
            .checked_mul(row_bytes)
            .ok_or_else(|| anyhow!("mapped readback destination range overflows"))?;
        let dst_end = dst_start
            .checked_add(row_bytes)
            .ok_or_else(|| anyhow!("mapped readback destination range overflows"))?;
        if dst_end > rgba8.len() {
            bail!("mapped readback copy range is outside destination buffer bounds");
        }
        match pixel_format {
            SurfacePixelFormat::Rgba8 => {
                let src_start = source_row
                    .checked_mul(padded_bytes_per_row as usize)
                    .and_then(|offset| offset.checked_add((source_x as usize).checked_mul(4)?))
                    .ok_or_else(|| anyhow!("mapped readback source range overflows"))?;
                let src_end = src_start
                    .checked_add(row_bytes)
                    .ok_or_else(|| anyhow!("mapped readback source range overflows"))?;
                if src_end > mapped.len() {
                    bail!("mapped readback copy range is outside source buffer bounds");
                }
                rgba8[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
            }
            SurfacePixelFormat::R8 => {
                let src_start = source_row
                    .checked_mul(padded_bytes_per_row as usize)
                    .and_then(|offset| offset.checked_add(source_x as usize))
                    .ok_or_else(|| anyhow!("mapped readback source range overflows"))?;
                let src_end = src_start
                    .checked_add(size[0] as usize)
                    .ok_or_else(|| anyhow!("mapped readback source range overflows"))?;
                if src_end > mapped.len() {
                    bail!("mapped readback copy range is outside source buffer bounds");
                }
                for col in 0..size[0] as usize {
                    let value = mapped[src_start + col];
                    let dst = dst_start + col * 4;
                    rgba8[dst..dst + 4].copy_from_slice(&[value, value, value, value]);
                }
            }
        }
    }
    Ok(rgba8)
}

fn rgba8_len(size: [u32; 2]) -> Result<usize> {
    (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("RGBA payload size overflows usize"))
}

fn row_stride(width: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("row stride overflows usize"))
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{
            geometry::RectU32,
            surface::PaintSurfaceId,
            tile::TileCoord,
            tile_payload::{PixelSnapshotData, assemble_tiles_to_rgba_rect},
        },
        renderer::SurfaceCommit,
    };

    use super::{
        SurfacePixelFormat, copy_mapped_readback_rect, rgba8_len, row_stride,
        stale_surfaces_after_shadow_apply_failure, tiled_snapshot_from_mapped_rect,
    };

    #[test]
    fn mapped_readback_rect_copy_removes_padding_and_offsets_rows() {
        let padded_bytes_per_row = 16u32;
        let mut mapped = vec![0; padded_bytes_per_row as usize * 3];
        for row in 0..3usize {
            for col in 0..3usize {
                let start = row * padded_bytes_per_row as usize + col * 4;
                mapped[start..start + 4].copy_from_slice(&[row as u8, col as u8, 7, 255]);
            }
        }

        let copied =
            copy_mapped_readback_rect(&mapped, [2, 2], padded_bytes_per_row, 1, 1).unwrap();

        assert_eq!(
            copied,
            vec![1, 1, 7, 255, 1, 2, 7, 255, 2, 1, 7, 255, 2, 2, 7, 255,]
        );
    }

    #[test]
    fn readback_size_helpers_validate_overflow_boundaries() {
        assert_eq!(row_stride(2).unwrap(), 8);
        assert_eq!(rgba8_len([2, 3]).unwrap(), 24);
    }

    #[test]
    fn shadow_apply_failure_marks_failed_and_later_surfaces_stale() {
        let surface_a = PaintSurfaceId::raster(0.into(), crate::core::surface::LayerId::default());
        let surface_b = PaintSurfaceId::raster(1.into(), crate::core::surface::LayerId::default());
        let surface_c = PaintSurfaceId::raster(2.into(), crate::core::surface::LayerId::default());
        let rect = RectU32::full([1, 1]);
        let commits = vec![
            SurfaceCommit {
                surface: surface_a,
                rect,
                after: PixelSnapshotData::contiguous(vec![1; 4]),
            },
            SurfaceCommit {
                surface: surface_b,
                rect,
                after: PixelSnapshotData::contiguous(vec![2; 4]),
            },
            SurfaceCommit {
                surface: surface_c,
                rect,
                after: PixelSnapshotData::contiguous(vec![3; 4]),
            },
        ];

        let stale = stale_surfaces_after_shadow_apply_failure(&commits, 1).collect::<Vec<_>>();

        assert_eq!(stale, vec![surface_b, surface_c]);
    }

    #[test]
    fn mapped_readback_can_build_tiled_snapshot_without_contiguous_intermediate() {
        let padded_bytes_per_row = 20u32;
        let rect = RectU32 {
            origin: [1, 1],
            size: [3, 3],
        };
        let mut mapped = vec![0; padded_bytes_per_row as usize * rect.size[1] as usize];
        for row in 0..rect.size[1] as usize {
            for col in 0..rect.size[0] as usize {
                let start = row * padded_bytes_per_row as usize + col * 4;
                mapped[start..start + 4].copy_from_slice(&[
                    (rect.origin[1] as usize + row) as u8,
                    (rect.origin[0] as usize + col) as u8,
                    11,
                    255,
                ]);
            }
        }

        let tiled = tiled_snapshot_from_mapped_rect(
            &mapped,
            SurfacePixelFormat::Rgba8,
            [4, 4],
            rect,
            padded_bytes_per_row,
            2,
        )
        .unwrap();

        assert_eq!(tiled.tiles.len(), 4);
        assert_eq!(
            tiled
                .tiles
                .iter()
                .map(|tile| tile.coord)
                .collect::<Vec<_>>(),
            vec![
                TileCoord { x: 0, y: 0 },
                TileCoord { x: 1, y: 0 },
                TileCoord { x: 0, y: 1 },
                TileCoord { x: 1, y: 1 },
            ]
        );
        let assembled = assemble_tiles_to_rgba_rect(&tiled).unwrap();
        assert_eq!(assembled.size, [3, 3]);
        assert_eq!(assembled.rgba8[0..4], [1, 1, 11, 255]);
        assert_eq!(assembled.rgba8[32..36], [3, 3, 11, 255]);
    }
}
