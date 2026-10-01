use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use bytemuck::Pod;
use eframe::egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

pub(crate) type SharedStagingBelt = Arc<Mutex<wgpu::util::StagingBelt>>;

/// Single renderer recording unit.
///
/// Feature code records passes into this frame, then hands the finished command
/// buffer back to the renderer-owned submission boundary. `GpuFrame` deliberately
/// does not carry a queue or expose submit policy.
///
/// Per-pass uploads must also be recorded into this frame. Using
/// `Queue::write_buffer` or `Queue::write_texture` while several passes are being
/// accumulated in one command buffer can overwrite shared GPU resources before
/// earlier passes consume them. Frame uploads are encoded as copy commands, so
/// they execute in-order with the passes that follow them.
pub(crate) struct GpuFrame {
    encoder: wgpu::CommandEncoder,
    staging_belt: Option<SharedStagingBelt>,
    has_staging_belt_writes: bool,
    upload_buffers: Vec<wgpu::Buffer>,
    retained_textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
}

pub(crate) struct GpuFrameSubmission {
    command_buffer: wgpu::CommandBuffer,
    staging_belt: Option<SharedStagingBelt>,
    has_staging_belt_writes: bool,
    upload_buffers: Vec<wgpu::Buffer>,
    retained_textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
}

pub(crate) struct TextureUploadRegion<'a> {
    pub(crate) origin: [u32; 2],
    pub(crate) size: [u32; 2],
    pub(crate) unpadded_bytes_per_row: u32,
    pub(crate) data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackedTextureUploadRegion {
    offset: wgpu::BufferAddress,
    padded_bytes_per_row: u32,
    size: [u32; 2],
}

impl GpuFrameSubmission {
    pub(crate) fn submit(self, queue: &wgpu::Queue) -> wgpu::SubmissionIndex {
        let Self {
            command_buffer,
            staging_belt,
            has_staging_belt_writes,
            upload_buffers,
            retained_textures,
        } = self;
        let submission = queue.submit(Some(command_buffer));
        if has_staging_belt_writes && let Some(staging_belt) = staging_belt {
            staging_belt
                .lock()
                .expect("staging belt mutex poisoned")
                .recall();
        }
        drop(upload_buffers);
        drop(retained_textures);
        submission
    }
}

impl GpuFrame {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        Self {
            encoder,
            staging_belt: None,
            has_staging_belt_writes: false,
            upload_buffers: Vec::new(),
            retained_textures: Vec::new(),
        }
    }

    pub(crate) fn new_with_staging_belt(
        device: &wgpu::Device,
        label: &'static str,
        staging_belt: SharedStagingBelt,
    ) -> Self {
        let mut frame = Self::new(device, label);
        frame.staging_belt = Some(staging_belt);
        frame
    }

    pub(crate) fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    pub(crate) fn map_buffer_on_submit<S>(
        &mut self,
        buffer: &wgpu::Buffer,
        mode: wgpu::MapMode,
        bounds: S,
        callback: impl FnOnce(Result<(), wgpu::BufferAsyncError>) + Send + 'static,
    ) where
        S: std::ops::RangeBounds<wgpu::BufferAddress>,
    {
        self.encoder
            .map_buffer_on_submit(buffer, mode, bounds, callback);
    }

    pub(crate) fn retain_texture_until_submit(
        &mut self,
        texture: wgpu::Texture,
        view: wgpu::TextureView,
    ) {
        self.retained_textures.push((texture, view));
    }

    pub(crate) fn create_buffer_from_slice<T: Pod>(
        &mut self,
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        values: &[T],
    ) -> wgpu::Buffer {
        let data = bytemuck::cast_slice(values);
        let size = (data.len() as u64).max(wgpu::COPY_BUFFER_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.write_buffer_bytes(device, &buffer, 0, data);
        buffer
    }

    pub(crate) fn write_buffer_bytes(
        &mut self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        offset: u64,
        data: &[u8],
    ) {
        if data.is_empty() {
            return;
        }
        debug_assert_eq!(offset % wgpu::COPY_BUFFER_ALIGNMENT, 0);
        debug_assert_eq!(data.len() as u64 % wgpu::COPY_BUFFER_ALIGNMENT, 0);

        if let Some(staging_belt) = &self.staging_belt {
            let size = wgpu::BufferSize::new(data.len() as u64)
                .expect("non-empty frame buffer upload must have non-zero size");
            {
                let mut belt = staging_belt.lock().expect("staging belt mutex poisoned");
                let mut view = belt.write_buffer(&mut self.encoder, buffer, offset, size);
                view.copy_from_slice(data);
            }
            self.has_staging_belt_writes = true;
        } else {
            let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("frame_buffer_upload"),
                contents: data,
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            self.encoder
                .copy_buffer_to_buffer(&upload, 0, buffer, offset, data.len() as u64);
            self.upload_buffers.push(upload);
        }
    }

    pub(crate) fn write_buffer_pod<T: Pod>(
        &mut self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        offset: u64,
        value: &T,
    ) {
        self.write_buffer_bytes(device, buffer, offset, bytemuck::bytes_of(value));
    }

    pub(crate) fn write_buffer_slice<T: Pod>(
        &mut self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        offset: u64,
        values: &[T],
    ) {
        self.write_buffer_bytes(device, buffer, offset, bytemuck::cast_slice(values));
    }

    pub(crate) fn write_texture_bytes(
        &mut self,
        device: &wgpu::Device,
        destination: wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
        unpadded_bytes_per_row: u32,
        data: &[u8],
        label: &'static str,
    ) {
        if size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 {
            return;
        }
        assert!(
            unpadded_bytes_per_row > 0,
            "texture upload row pitch must be non-zero"
        );

        let row_count = total_texture_copy_rows(size);
        let expected_len = texture_upload_data_len(unpadded_bytes_per_row, row_count);
        assert_eq!(
            data.len(),
            expected_len,
            "texture upload data must be tightly packed"
        );

        let padded_bytes_per_row =
            align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let upload_data = pad_texture_upload_rows(
            data,
            unpadded_bytes_per_row,
            padded_bytes_per_row,
            row_count,
        );
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: upload_data.as_ref(),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        self.encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &upload,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(size.height),
                },
            },
            destination,
            size,
        );
        self.upload_buffers.push(upload);
    }

    pub(crate) fn write_texture_bytes_regions(
        &mut self,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        regions: &[TextureUploadRegion<'_>],
        label: &'static str,
    ) {
        let (upload_data, packed_regions) = pack_texture_upload_regions(regions);
        if upload_data.is_empty() {
            return;
        }
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: &upload_data,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        for (region, packed) in regions
            .iter()
            .filter(|region| !region.is_empty())
            .zip(&packed_regions)
        {
            self.encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &upload,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: packed.offset,
                        bytes_per_row: Some(packed.padded_bytes_per_row),
                        rows_per_image: Some(packed.size[1]),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: region.origin[0],
                        y: region.origin[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: packed.size[0],
                    height: packed.size[1],
                    depth_or_array_layers: 1,
                },
            );
        }
        self.upload_buffers.push(upload);
    }

    pub(crate) fn write_texture_r8(
        &mut self,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        origin: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) {
        self.write_texture_bytes(
            device,
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
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            size[0],
            pixels,
            "frame_texture_upload_r8",
        );
    }

    pub(crate) fn write_texture_rgba8(
        &mut self,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        origin: [u32; 2],
        size: [u32; 2],
        rgba8: &[u8],
    ) {
        self.write_texture_bytes(
            device,
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
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            size[0]
                .checked_mul(4)
                .expect("RGBA8 upload row pitch overflowed"),
            rgba8,
            "frame_texture_upload_rgba8",
        );
    }

    pub(crate) fn write_texture_rgba8_regions(
        &mut self,
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        regions: &[TextureUploadRegion<'_>],
    ) {
        for region in regions {
            assert_eq!(
                region.unpadded_bytes_per_row,
                region.size[0]
                    .checked_mul(4)
                    .expect("RGBA8 upload row pitch overflowed"),
                "RGBA8 upload region row pitch must match width"
            );
        }
        self.write_texture_bytes_regions(device, texture, regions, "frame_texture_upload_rgba8");
    }

    pub(crate) fn finish(self) -> GpuFrameSubmission {
        if self.has_staging_belt_writes
            && let Some(staging_belt) = &self.staging_belt
        {
            staging_belt
                .lock()
                .expect("staging belt mutex poisoned")
                .finish();
        }
        GpuFrameSubmission {
            command_buffer: self.encoder.finish(),
            staging_belt: self.staging_belt,
            has_staging_belt_writes: self.has_staging_belt_writes,
            upload_buffers: self.upload_buffers,
            retained_textures: self.retained_textures,
        }
    }
}

fn total_texture_copy_rows(size: wgpu::Extent3d) -> u32 {
    size.height
        .checked_mul(size.depth_or_array_layers)
        .expect("texture upload row count overflowed")
}

fn texture_upload_data_len(unpadded_bytes_per_row: u32, row_count: u32) -> usize {
    let bytes = unpadded_bytes_per_row
        .checked_mul(row_count)
        .expect("texture upload byte length overflowed");
    bytes as usize
}

fn pad_texture_upload_rows(
    data: &[u8],
    unpadded_bytes_per_row: u32,
    padded_bytes_per_row: u32,
    row_count: u32,
) -> Cow<'_, [u8]> {
    if padded_bytes_per_row == unpadded_bytes_per_row {
        return Cow::Borrowed(data);
    }

    let unpadded = unpadded_bytes_per_row as usize;
    let padded = padded_bytes_per_row as usize;
    let mut upload_data = vec![0; padded * row_count as usize];
    for row in 0..row_count as usize {
        let src = row * unpadded;
        let dst = row * padded;
        upload_data[dst..dst + unpadded].copy_from_slice(&data[src..src + unpadded]);
    }
    Cow::Owned(upload_data)
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn align_to_u64(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

impl TextureUploadRegion<'_> {
    fn is_empty(&self) -> bool {
        self.size[0] == 0 || self.size[1] == 0
    }
}

fn pack_texture_upload_regions(
    regions: &[TextureUploadRegion<'_>],
) -> (Vec<u8>, Vec<PackedTextureUploadRegion>) {
    let mut upload_data = Vec::new();
    let mut packed_regions = Vec::new();
    for region in regions {
        if region.is_empty() {
            continue;
        }
        assert!(
            region.unpadded_bytes_per_row > 0,
            "texture upload row pitch must be non-zero"
        );
        let row_count = region.size[1];
        let expected_len = texture_upload_data_len(region.unpadded_bytes_per_row, row_count);
        assert_eq!(
            region.data.len(),
            expected_len,
            "texture upload data must be tightly packed"
        );

        let padded_bytes_per_row = align_to(
            region.unpadded_bytes_per_row,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let offset = align_to_u64(upload_data.len() as u64, wgpu::COPY_BUFFER_ALIGNMENT);
        upload_data.resize(offset as usize, 0);
        append_padded_texture_upload_rows(
            &mut upload_data,
            region.data,
            region.unpadded_bytes_per_row,
            padded_bytes_per_row,
            row_count,
        );
        packed_regions.push(PackedTextureUploadRegion {
            offset,
            padded_bytes_per_row,
            size: region.size,
        });
    }
    (upload_data, packed_regions)
}

fn append_padded_texture_upload_rows(
    upload_data: &mut Vec<u8>,
    data: &[u8],
    unpadded_bytes_per_row: u32,
    padded_bytes_per_row: u32,
    row_count: u32,
) {
    let unpadded = unpadded_bytes_per_row as usize;
    let padded = padded_bytes_per_row as usize;
    for row in 0..row_count as usize {
        let src = row * unpadded;
        upload_data.extend_from_slice(&data[src..src + unpadded]);
        upload_data.resize(upload_data.len() + padded - unpadded, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_texture_upload_regions_with_padded_rows() {
        let first = vec![1u8; 12 * 2];
        let second = vec![2u8; 4];
        let regions = [
            TextureUploadRegion {
                origin: [0, 0],
                size: [3, 2],
                unpadded_bytes_per_row: 12,
                data: &first,
            },
            TextureUploadRegion {
                origin: [5, 7],
                size: [4, 1],
                unpadded_bytes_per_row: 4,
                data: &second,
            },
        ];

        let (upload_data, packed) = pack_texture_upload_regions(&regions);

        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].offset, 0);
        assert_eq!(
            packed[0].padded_bytes_per_row,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
        );
        assert_eq!(packed[0].size, [3, 2]);
        assert_eq!(
            packed[1].offset % wgpu::COPY_BUFFER_ALIGNMENT,
            0,
            "region offsets must be copy-buffer aligned"
        );
        assert_eq!(
            packed[1].padded_bytes_per_row,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
        );
        assert_eq!(upload_data[0..12], first[0..12]);
        assert!(
            upload_data[12..wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize]
                .iter()
                .all(|byte| *byte == 0)
        );
        let second_offset = packed[1].offset as usize;
        assert_eq!(upload_data[second_offset..second_offset + 4], second[..]);
    }

    #[test]
    #[should_panic(expected = "texture upload data must be tightly packed")]
    fn rejects_non_tightly_packed_region_data() {
        let data = vec![1u8; 7];
        let regions = [TextureUploadRegion {
            origin: [0, 0],
            size: [2, 1],
            unpadded_bytes_per_row: 8,
            data: &data,
        }];

        let _ = pack_texture_upload_regions(&regions);
    }

    #[test]
    fn skips_zero_sized_regions() {
        let data = vec![1u8; 4];
        let regions = [TextureUploadRegion {
            origin: [0, 0],
            size: [0, 1],
            unpadded_bytes_per_row: 4,
            data: &data,
        }];

        let (upload_data, packed) = pack_texture_upload_regions(&regions);

        assert!(upload_data.is_empty());
        assert!(packed.is_empty());
    }
}
