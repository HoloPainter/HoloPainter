use std::cell::{Cell, RefCell};

use anyhow::{Result, anyhow};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::surface::PaintSurfaceId,
    renderer::gpu::{create_mask_texture, create_paint_texture},
};

use super::{residency::SurfaceResidencyMetadata, tile_shadow::SurfaceTileShadow};

pub(crate) struct SurfaceRecord {
    pub(crate) target: PaintSurfaceId,
    pub(crate) format: SurfacePixelFormat,
    pub(crate) texture_size: [u32; 2],
    pub(crate) gpu: Option<ResidentSurfaceTexture>,
    pub(crate) tile_shadow: RefCell<SurfaceTileShadow>,
    pub(crate) uploaded_document_revision: Cell<Option<u64>>,
    pub(crate) residency_metadata: RefCell<SurfaceResidencyMetadata>,
}

pub(crate) struct ResidentSurfaceTexture {
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfacePixelFormat {
    Rgba8,
    R8,
}

impl SurfacePixelFormat {
    pub(crate) fn for_surface(target: PaintSurfaceId) -> Self {
        if target.is_mask() {
            Self::R8
        } else {
            Self::Rgba8
        }
    }

    pub(crate) fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 => 4,
            Self::R8 => 1,
        }
    }

    pub(crate) fn estimated_gpu_bytes(self, size: [u32; 2]) -> Result<usize> {
        let pixels = (size[0] as usize)
            .checked_mul(size[1] as usize)
            .ok_or_else(|| anyhow!("surface pixel count overflows usize"))?;
        pixels
            .checked_mul(self.bytes_per_pixel() as usize)
            .ok_or_else(|| anyhow!("surface byte size overflows usize"))
    }

    pub(crate) fn saturating_byte_len(self, size: [u32; 2]) -> usize {
        (size[0] as usize)
            .saturating_mul(size[1] as usize)
            .saturating_mul(self.bytes_per_pixel() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_formats_calculate_saturating_byte_lengths() {
        assert_eq!(SurfacePixelFormat::Rgba8.saturating_byte_len([3, 2]), 24);
        assert_eq!(SurfacePixelFormat::R8.saturating_byte_len([3, 2]), 6);
        assert_eq!(SurfacePixelFormat::Rgba8.saturating_byte_len([0, 9]), 0);
        assert_eq!(
            SurfacePixelFormat::Rgba8.saturating_byte_len([u32::MAX, u32::MAX]),
            (u32::MAX as usize)
                .saturating_mul(u32::MAX as usize)
                .saturating_mul(4)
        );
    }

    #[test]
    fn checked_surface_byte_length_reports_overflow() {
        assert!(
            SurfacePixelFormat::Rgba8
                .estimated_gpu_bytes([u32::MAX, u32::MAX])
                .is_err()
        );
    }
}

impl SurfaceRecord {
    pub(super) fn new(
        device: &wgpu::Device,
        target: PaintSurfaceId,
        size: [u32; 2],
    ) -> Result<Self> {
        Self::new_with_format(
            device,
            target,
            size,
            SurfacePixelFormat::for_surface(target),
        )
    }

    pub(super) fn new_with_format(
        device: &wgpu::Device,
        target: PaintSurfaceId,
        size: [u32; 2],
        format: SurfacePixelFormat,
    ) -> Result<Self> {
        let (texture, view) = match format {
            SurfacePixelFormat::Rgba8 => create_paint_texture(device, size, "surface_texture"),
            SurfacePixelFormat::R8 => create_mask_texture(device, size, "surface_mask_texture"),
        };
        Ok(Self {
            target,
            format,
            texture_size: size,
            gpu: Some(ResidentSurfaceTexture { texture, view }),
            tile_shadow: RefCell::new(SurfaceTileShadow::new_transparent(size)?),
            uploaded_document_revision: Cell::new(None),
            residency_metadata: RefCell::new(SurfaceResidencyMetadata::new(
                format.estimated_gpu_bytes(size)?,
            )),
        })
    }

    pub(super) fn new_transparent_nonresident(
        target: PaintSurfaceId,
        size: [u32; 2],
    ) -> Result<Self> {
        let format = SurfacePixelFormat::for_surface(target);
        Ok(Self {
            target,
            format,
            texture_size: size,
            gpu: None,
            tile_shadow: RefCell::new(SurfaceTileShadow::new_transparent(size)?),
            uploaded_document_revision: Cell::new(None),
            residency_metadata: RefCell::new(SurfaceResidencyMetadata::new(0)),
        })
    }

    pub(super) fn gpu_texture(&self) -> Option<&wgpu::Texture> {
        self.gpu.as_ref().map(|gpu| &gpu.texture)
    }

    pub(super) fn gpu_view(&self) -> Option<&wgpu::TextureView> {
        self.gpu.as_ref().map(|gpu| &gpu.view)
    }

    pub(super) fn require_gpu_texture(&self) -> Result<&wgpu::Texture> {
        self.gpu_texture()
            .ok_or_else(|| anyhow!("surface GPU texture is non-resident: {:?}", self.target))
    }

    pub(super) fn require_gpu_view(&self) -> Result<&wgpu::TextureView> {
        self.gpu_view()
            .ok_or_else(|| anyhow!("surface GPU texture is non-resident: {:?}", self.target))
    }
}
