use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::{
    core::surface::PaintSurfaceId,
    renderer::{
        engine::gpu_state::RendererGpuState, gpu::frame::GpuFrame, transient::TransientTextures,
    },
};

use super::{StrokeSurfaceTarget, SurfaceEditTarget, SurfaceInitialPixels, SurfaceRepository};

impl SurfaceRepository {
    pub(crate) fn create_surface_texture_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        initial: SurfaceInitialPixels,
    ) -> Result<()> {
        self.create_surface_record_into_frame(gpu, frame, target, size, initial)
    }

    pub(crate) fn delete_surface_texture(&mut self, target: PaintSurfaceId) {
        self.delete_surface_record(target);
    }

    pub(crate) fn duplicate_surface_texture_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        from: PaintSurfaceId,
        to: PaintSurfaceId,
    ) -> Result<()> {
        self.duplicate_surface_record_into_frame(gpu, frame, from, to)
    }

    pub(crate) fn surface_texture_size(&self, target: PaintSurfaceId) -> Option<[u32; 2]> {
        self.surface_record_texture_size(target)
    }

    pub(crate) fn is_surface_texture_resident(&self, target: PaintSurfaceId) -> bool {
        self.surface_texture_view(target).is_some()
    }

    pub(crate) fn surface_texture(&self, target: PaintSurfaceId) -> Option<&wgpu::Texture> {
        self.surface_record_texture(target)
    }

    pub(crate) fn surface_texture_view(
        &self,
        target: PaintSurfaceId,
    ) -> Option<&wgpu::TextureView> {
        self.surface_record_texture_view(target)
    }

    pub(crate) fn edit_surface_target(
        &self,
        target: PaintSurfaceId,
    ) -> Option<SurfaceEditTarget<'_>> {
        self.surface_record_edit_target(target)
    }

    pub(crate) fn stroke_surface_target<'a>(
        &'a self,
        target: PaintSurfaceId,
        scratch: &'a TransientTextures,
    ) -> Option<StrokeSurfaceTarget<'a>> {
        self.stroke_surface_record_target(target, scratch)
            .map(|target| {
                let _ = target.surface;
                StrokeSurfaceTarget {
                    texture_size: target.texture_size,
                    read_texture: target.read_texture,
                    read_view: target.read_view,
                    write_texture: target.write_texture,
                    write_view: target.write_view,
                }
            })
    }
}
