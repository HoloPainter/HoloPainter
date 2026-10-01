//! Persistent GPU paint surfaces owned by the loaded document.
//!
//! The loaded document owns persistent paint surfaces through this module.
//! Repository access is kept separate from mutation commit semantics so
//! stroke/apply code can evolve without growing a broad renderer facade again.

mod mutation;
mod preparation;
mod readback;
mod record;
mod rects;
mod repository;
mod residency;
mod storage;
mod texture;
mod tile_shadow;
mod types;
mod upload;

pub(crate) use mutation::{
    SurfaceEditContext, SurfaceMutationCommit, SurfaceMutationContext, SurfaceUploadCommit,
};
pub(crate) use record::SurfacePixelFormat;
pub(crate) use repository::SurfaceRepository;
pub(crate) use types::{
    EncodedSurfaceReadback, StrokeSurfaceTarget, SurfaceContentState, SurfaceEditTarget,
    SurfaceInitialPixels, SurfacePrepareStats, SurfaceReadResult, SurfaceReadSource,
};
