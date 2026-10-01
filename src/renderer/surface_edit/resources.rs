use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::renderer::gpu::buffer::{create_initialized_buffer, create_uniform_buffer};

pub(crate) const EDIT_MASK_BLEED_WORKGROUP_SIZE: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UvIslandBleedParams {
    pub tex_size: [u32; 2],
    pub dilation_radius: u32,
    pub _pad0: u32,
    pub scan_origin: [u32; 2],
    pub scan_size: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct EditMaskBoundsGpu {
    pub min_xy: [u32; 2],
    pub max_xy: [u32; 2],
    pub stroke_count: u32,
    pub _pad0: [u32; 3],
}

pub(crate) struct SurfaceEditResources {
    bleed_uniform: wgpu::Buffer,
    bounds_buffer: wgpu::Buffer,
    bounds_reset_buffer: wgpu::Buffer,
    dispatch_buffer: wgpu::Buffer,
    dispatch_reset_buffer: wgpu::Buffer,
}

impl SurfaceEditResources {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let bleed_uniform =
            create_uniform_buffer::<UvIslandBleedParams>(device, "uv_island_bleed_uniform");
        let bounds_init = EditMaskBoundsGpu {
            min_xy: [u32::MAX, u32::MAX],
            max_xy: [0, 0],
            stroke_count: 0,
            _pad0: [0, 0, 0],
        };
        let bounds_buffer = create_initialized_buffer(
            device,
            "edit_mask_bounds_buffer",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            std::slice::from_ref(&bounds_init),
        );
        let bounds_reset_buffer = create_initialized_buffer(
            device,
            "edit_mask_bounds_reset_buffer",
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            std::slice::from_ref(&bounds_init),
        );
        let dispatch_init = [0_u32, 0, 1];
        let dispatch_buffer = create_initialized_buffer(
            device,
            "edit_mask_dispatch_buffer",
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            std::slice::from_ref(&dispatch_init),
        );
        let dispatch_reset_buffer = create_initialized_buffer(
            device,
            "edit_mask_dispatch_reset_buffer",
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            std::slice::from_ref(&dispatch_init),
        );

        Self {
            bleed_uniform,
            bounds_buffer,
            bounds_reset_buffer,
            dispatch_buffer,
            dispatch_reset_buffer,
        }
    }

    pub(crate) fn bleed_uniform(&self) -> &wgpu::Buffer {
        &self.bleed_uniform
    }

    pub(crate) fn bounds_buffer(&self) -> &wgpu::Buffer {
        &self.bounds_buffer
    }

    pub(crate) fn bounds_reset_buffer(&self) -> &wgpu::Buffer {
        &self.bounds_reset_buffer
    }

    pub(crate) fn dispatch_buffer(&self) -> &wgpu::Buffer {
        &self.dispatch_buffer
    }

    pub(crate) fn dispatch_reset_buffer(&self) -> &wgpu::Buffer {
        &self.dispatch_reset_buffer
    }
}
