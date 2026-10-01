use crate::core::{
    geometry::RectU32, material::MaterialIndex, surface::PaintSurfaceId,
    tile_payload::PixelSnapshotData,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCommit {
    pub surface: PaintSurfaceId,
    pub rect: RectU32,
    pub after: PixelSnapshotData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionCommit {
    pub material_index: MaterialIndex,
    pub texture_size: [u32; 2],
    pub r8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererDiagnostic {
    SpatialBlur {
        target_stride_px: u32,
        actual_stride_px: u32,
        traversal_overflow_texels: u32,
        capacity_overflow_samples: u32,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderCommitArtifacts {
    pub surface_commits: Vec<SurfaceCommit>,
    pub selection_before_commits: Vec<SelectionCommit>,
    pub selection_after_commits: Vec<SelectionCommit>,
    pub diagnostics: Vec<RendererDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderCommitId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedRenderCommit {
    pub id: RenderCommitId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedRenderCommit {
    pub id: RenderCommitId,
    pub artifacts: Result<RenderCommitArtifacts, String>,
}
