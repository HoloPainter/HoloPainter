//! Surface-facing type vocabulary.
//!
//! These types keep document and feature code on the `SurfaceRepository`
//! boundary. New renderer code should import these names from
//! `document::surfaces` instead of depending on low-level GPU surface helpers directly.

use eframe::egui_wgpu::wgpu;

use crate::core::{image::Rgba8Snapshot, tile_cache::TileCacheOpStats};

pub(crate) use crate::core::image::LayerInitialPixels as SurfaceInitialPixels;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceContentState {
    Empty,
    Unknown,
}

/// Prepared read/write textures used by stroke and apply passes for one
/// persistent paint surface.
///
/// This is the surface-boundary target used by stroke and apply passes: callers
/// get only the GPU resources required for rendering, without importing
/// low-level surface storage details.

/// Persistent or proxy texture written by GPU editing features.
pub(crate) struct SurfaceEditTarget<'a> {
    pub(crate) texture_size: [u32; 2],
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) view: &'a wgpu::TextureView,
}

pub(crate) struct StrokeSurfaceTarget<'a> {
    pub(crate) texture_size: [u32; 2],
    pub(crate) read_texture: &'a wgpu::Texture,
    pub(crate) read_view: &'a wgpu::TextureView,
    pub(crate) write_texture: &'a wgpu::Texture,
    pub(crate) write_view: &'a wgpu::TextureView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SurfaceReadSource {
    TileCache(TileCacheOpStats),
    GpuReadback {
        cache_update: Option<TileCacheOpStats>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SurfaceReadResult {
    pub(crate) snapshot: Rgba8Snapshot,
    pub(crate) source: SurfaceReadSource,
}

pub(crate) struct EncodedSurfaceReadback {
    pub(crate) texture_size: [u32; 2],
    pub(crate) pixel_format: super::record::SurfacePixelFormat,
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) padded_bytes_per_row: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SurfacePrepareStats {
    pub(crate) evicted: bool,
    pub(crate) rehydrated: bool,
    pub(crate) uploaded_tiles: usize,
    pub(crate) uploaded_bytes: usize,
}
