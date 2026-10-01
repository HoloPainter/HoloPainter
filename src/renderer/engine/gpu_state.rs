use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::renderer::gpu::{
    buffer::create_viewport_quad_buffer,
    context::GpuContext,
    sampler::{create_paint_sampler, create_stroke_sampler},
};

pub(crate) struct RendererGpuState {
    pub(crate) ctx: GpuContext,
    paint_sampler: wgpu::Sampler,
    stroke_sampler: wgpu::Sampler,
    viewport_quad: wgpu::Buffer,
}

impl RendererGpuState {
    pub(crate) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        _tex_size: [u32; 2],
    ) -> Result<Self> {
        let ctx = GpuContext::new(device.clone(), queue.clone());
        let paint_sampler = create_paint_sampler(ctx.device());
        let stroke_sampler = create_stroke_sampler(ctx.device());
        let viewport_quad = create_viewport_quad_buffer(ctx.device());
        Ok(Self {
            ctx,
            paint_sampler,
            stroke_sampler,
            viewport_quad,
        })
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        self.ctx.device()
    }

    pub(crate) fn paint_sampler(&self) -> &wgpu::Sampler {
        &self.paint_sampler
    }

    pub(crate) fn stroke_sampler(&self) -> &wgpu::Sampler {
        &self.stroke_sampler
    }

    pub(crate) fn viewport_quad(&self) -> &wgpu::Buffer {
        &self.viewport_quad
    }
}
