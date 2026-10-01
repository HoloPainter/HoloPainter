use std::sync::mpsc;

use anyhow::{Result, anyhow, bail};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        image::{Rgba8Snapshot, rgba8_len},
        selection::SelectionMaskId,
        surface::PaintSurfaceId,
    },
    renderer::{
        gpu::frame::GpuFrame,
        readback::{ReadbackRequest, ReadbackResult},
    },
};

use super::{core::RenderEngine, metrics::record_surface_read_metrics};

impl RenderEngine {
    pub fn readback(&mut self, request: ReadbackRequest) -> Result<ReadbackResult> {
        match request {
            ReadbackRequest::SurfaceRgba8 { surface, rect } => {
                let snapshot = match rect {
                    Some(rect) => self.read_surface_rect_rgba8(surface, rect.origin, rect.size)?,
                    None => self.read_surface_rgba8(surface)?,
                };
                Ok(ReadbackResult::Rgba8(snapshot))
            }
            ReadbackRequest::CompositeRgba8 {
                material_index,
                rect,
            } => {
                let snapshot = match rect {
                    Some(rect) => {
                        self.read_composite_rect_rgba8(material_index, rect.origin, rect.size)?
                    }
                    None => self.read_composite_rgba8(material_index)?,
                };
                Ok(ReadbackResult::Rgba8(snapshot))
            }
            ReadbackRequest::SelectionMaskR8 { mask_id } => Ok(ReadbackResult::SelectionMaskR8(
                self.read_selection_mask_r8(mask_id),
            )),
        }
    }

    fn read_surface_rgba8(&mut self, target: PaintSurfaceId) -> Result<Rgba8Snapshot> {
        let surface_size = self
            .state
            .document
            .surfaces
            .surface_texture_size(target)
            .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", target))?;
        self.read_surface_rect_rgba8(target, [0, 0], surface_size)
    }

    fn read_surface_rect_rgba8(
        &mut self,
        target: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Rgba8Snapshot> {
        let result = self.state.document.surfaces.read_surface_rect_rgba8(
            &self.state.gpu,
            self.state.gpu.ctx.queue(),
            target,
            origin,
            size,
        )?;
        {
            record_surface_read_metrics(self.metrics.get_mut(), &result.source, &result.snapshot);
        }
        Ok(result.snapshot)
    }

    fn read_selection_mask_r8(&self, mask_id: SelectionMaskId) -> Option<Vec<u8>> {
        self.state
            .document
            .selections
            .pixels(mask_id)
            .map(Vec::from)
    }

    fn read_composite_rgba8(&mut self, material_index: usize) -> Result<Rgba8Snapshot> {
        let texture_size = self
            .features
            .composite
            .texture_size(material_index)
            .ok_or_else(|| anyhow!("composite texture does not exist: {material_index}"))?;
        self.read_composite_rect_rgba8(material_index, [0, 0], texture_size)
    }

    fn read_composite_rect_rgba8(
        &mut self,
        material_index: usize,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<Rgba8Snapshot> {
        let texture = self
            .features
            .composite
            .texture(material_index)
            .ok_or_else(|| anyhow!("composite texture does not exist: {material_index}"))?;
        let texture_size = self
            .features
            .composite
            .texture_size(material_index)
            .ok_or_else(|| anyhow!("composite texture does not exist: {material_index}"))?;
        let snapshot = blocking_read_texture_rect_rgba8(
            self.state.gpu.ctx.device(),
            self.state.gpu.ctx.queue(),
            texture,
            texture_size,
            origin,
            size,
        )?;
        self.metrics.get_mut().record_readback(
            snapshot.texture_size,
            snapshot.origin,
            snapshot.size,
        );
        Ok(snapshot)
    }
}

pub(crate) fn blocking_read_texture_rect_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    texture_size: [u32; 2],
    origin: [u32; 2],
    size: [u32; 2],
) -> Result<Rgba8Snapshot> {
    blocking_read_texture_rect_rgba8_with_device_queue(
        device,
        queue,
        texture,
        texture_size,
        origin,
        size,
        "blocking_read_texture_rect_rgba8",
    )
}

pub(crate) fn blocking_read_texture_rect_r8_as_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    texture_size: [u32; 2],
    origin: [u32; 2],
    size: [u32; 2],
) -> Result<Rgba8Snapshot> {
    validate_readback_rect(texture_size, origin, size)?;
    let unpadded_bytes_per_row = size[0];
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = padded_bytes_per_row as u64 * size[1] as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r8_readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut frame = GpuFrame::new(device, "blocking_read_texture_rect_r8");
    frame.encoder().copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: origin[0],
                y: origin[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    let (tx, rx) = mpsc::channel();
    frame.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    submit_readback_frame(queue, frame);

    let slice = readback.slice(..);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|err| anyhow!("GPU poll failed during texture readback: {err:?}"))?;
    rx.recv()
        .map_err(|err| anyhow!("texture readback callback dropped: {err}"))?
        .map_err(|err| anyhow!("texture readback mapping failed: {err:?}"))?;

    let mut rgba8 = vec![0; rgba8_len(size)?];
    {
        let mapped = slice.get_mapped_range()?;
        for row in 0..size[1] as usize {
            let src_start = row * padded_bytes_per_row as usize;
            let src_end = src_start + size[0] as usize;
            let dst_start = row * size[0] as usize * 4;
            if src_end > mapped.len() {
                bail!("R8 readback copy range is outside buffer bounds");
            }
            for col in 0..size[0] as usize {
                let value = mapped[src_start + col];
                let dst = dst_start + col * 4;
                rgba8[dst..dst + 4].copy_from_slice(&[value, value, value, value]);
            }
        }
    }
    readback.unmap();

    Rgba8Snapshot::new(texture_size, origin, size, rgba8)
}

/// Synchronously waits for GPU readback completion.
///
/// This path is for explicit readback/debug/export style operations and must
/// not be used for normal renderer commit finalization.
pub(crate) fn blocking_read_texture_rect_rgba8_with_device_queue(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    texture_size: [u32; 2],
    origin: [u32; 2],
    size: [u32; 2],
    label: &'static str,
) -> Result<Rgba8Snapshot> {
    validate_readback_rect(texture_size, origin, size)?;
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size[0] * bytes_per_pixel;
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = padded_bytes_per_row as u64 * size[1] as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rgba8_readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut frame = GpuFrame::new(device, label);
    frame.encoder().copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: origin[0],
                y: origin[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    let (tx, rx) = mpsc::channel();
    frame.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    submit_readback_frame(queue, frame);

    let slice = readback.slice(..);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|err| anyhow!("GPU poll failed during texture readback: {err:?}"))?;
    rx.recv()
        .map_err(|err| anyhow!("texture readback callback dropped: {err}"))?
        .map_err(|err| anyhow!("texture readback mapping failed: {err:?}"))?;

    let mut rgba8 = vec![0; rgba8_len(size)?];
    {
        let mapped = slice.get_mapped_range()?;
        for row in 0..size[1] as usize {
            let src_start = row * padded_bytes_per_row as usize;
            let src_end = src_start + unpadded_bytes_per_row as usize;
            let dst_start = row * unpadded_bytes_per_row as usize;
            let dst_end = dst_start + unpadded_bytes_per_row as usize;
            rgba8[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
        }
    }
    readback.unmap();

    Rgba8Snapshot::new(texture_size, origin, size, rgba8)
}

fn submit_readback_frame(queue: &wgpu::Queue, frame: GpuFrame) -> wgpu::SubmissionIndex {
    frame.finish().submit(queue)
}

fn validate_readback_rect(texture_size: [u32; 2], origin: [u32; 2], size: [u32; 2]) -> Result<()> {
    if size[0] == 0 || size[1] == 0 {
        bail!("readback rect size must be > 0");
    }
    let max_x = origin[0]
        .checked_add(size[0])
        .ok_or_else(|| anyhow!("readback rect x range overflows"))?;
    let max_y = origin[1]
        .checked_add(size[1])
        .ok_or_else(|| anyhow!("readback rect y range overflows"))?;
    if max_x > texture_size[0] || max_y > texture_size[1] {
        bail!(
            "readback rect origin {:?} size {:?} is outside texture {:?}",
            origin,
            size,
            texture_size
        );
    }
    Ok(())
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
