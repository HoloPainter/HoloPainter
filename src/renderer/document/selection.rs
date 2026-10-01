use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        composite::{ApplyParams, SelectionCompositeMode},
        geometry::RectU32,
        mask::MaskSource,
        render_report::GpuTextureMetrics,
        selection::{
            ActiveSelection, SelectionMaskId, SelectionMaskSnapshot, SelectionTilePayload,
            selection_mask_id_for_material, selection_mask_len,
        },
    },
    renderer::{gpu::frame::GpuFrame, mutation::MutationLog},
};

pub(crate) struct SelectionTexture {
    material_index: usize,
    size: [u32; 2],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pixels: Vec<u8>,
    initialized: bool,
}

#[derive(Default)]
pub struct SelectionStore {
    textures: HashMap<SelectionMaskId, SelectionTexture>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionUploadStats {
    pub(crate) upload_count: usize,
    pub(crate) uploaded_bytes: usize,
    pub(crate) full_upload_count: usize,
    pub(crate) rect_upload_count: usize,
}

pub(crate) struct SelectionReadbackCopy {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) padded_bytes_per_row: u32,
}

impl SelectionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn view(&self, mask_id: SelectionMaskId) -> Option<&wgpu::TextureView> {
        self.textures.get(&mask_id).map(|selection| &selection.view)
    }

    pub fn pixels(&self, mask_id: SelectionMaskId) -> Option<&[u8]> {
        self.textures
            .get(&mask_id)
            .map(|selection| selection.pixels.as_slice())
    }

    pub(crate) fn texture(&self, mask_id: SelectionMaskId) -> Option<&wgpu::Texture> {
        self.textures
            .get(&mask_id)
            .map(|selection| &selection.texture)
    }

    pub(crate) fn texture_mut(
        &mut self,
        mask_id: SelectionMaskId,
    ) -> Option<&mut SelectionTexture> {
        self.textures.get_mut(&mask_id)
    }

    pub(crate) fn apply_selection_readback_data(
        &mut self,
        material_index: usize,
        texture_size: [u32; 2],
        r8: &[u8],
    ) -> anyhow::Result<()> {
        let Some(selection) = self.textures.values_mut().find(|selection| {
            selection.material_index == material_index && selection.size == texture_size
        }) else {
            anyhow::bail!(
                "selection texture for material {} texture {:?} does not exist",
                material_index,
                texture_size
            );
        };
        let expected_len = texture_size[0] as usize * texture_size[1] as usize;
        anyhow::ensure!(
            r8.len() == expected_len,
            "selection readback payload size mismatch: got {}, expected {}",
            r8.len(),
            expected_len
        );
        selection.pixels.clear();
        selection.pixels.extend_from_slice(r8);
        selection.initialized = true;
        Ok(())
    }

    pub(crate) fn encode_selection_record_readback_r8(
        &self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        mask_id: SelectionMaskId,
        texture_size: [u32; 2],
    ) -> anyhow::Result<SelectionReadbackCopy> {
        let selection = self
            .textures
            .get(&mask_id)
            .ok_or_else(|| anyhow::anyhow!("selection texture {:?} does not exist", mask_id))?;
        anyhow::ensure!(
            selection.size == texture_size,
            "selection texture {:?} size mismatch: stored {:?}, requested {:?}",
            mask_id,
            selection.size,
            texture_size
        );
        let unpadded_bytes_per_row = texture_size[0].max(1);
        let padded_bytes_per_row =
            align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer_size = padded_bytes_per_row as u64 * texture_size[1].max(1) as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("selection_commit_readback_r8"),
            size: buffer_size.max(wgpu::COPY_BUFFER_ALIGNMENT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        frame.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &selection.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(texture_size[1].max(1)),
                },
            },
            wgpu::Extent3d {
                width: texture_size[0].max(1),
                height: texture_size[1].max(1),
                depth_or_array_layers: 1,
            },
        );
        Ok(SelectionReadbackCopy {
            buffer,
            padded_bytes_per_row,
        })
    }

    pub(crate) fn patch_tiles(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        materials: &super::materials::MaterialRegistry,
        active_selection: &ActiveSelection,
        tiles: &[SelectionTilePayload],
    ) -> anyhow::Result<SelectionUploadStats> {
        let mut stats = SelectionUploadStats::default();
        for tile in tiles {
            let Some(material_mask) = active_selection.material_mask(tile.material_index) else {
                continue;
            };
            let Some(mask_id) = material_mask.mask_id else {
                continue;
            };
            let texture_size = materials
                .texture_size(tile.material_index.as_usize())
                .ok_or_else(|| {
                    anyhow::anyhow!("selection material does not exist: {}", tile.material_index)
                })?;
            let selection = self.ensure_texture(
                device,
                mask_id,
                tile.material_index.as_usize(),
                texture_size,
            );
            patch_r8_rect(selection.pixels_mut(), texture_size, tile.rect, &tile.r8)?;
            stats.merge(selection.upload_rect_into_frame(device, frame, tile.rect));
        }
        Ok(stats)
    }

    pub(crate) fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        mask_id: SelectionMaskId,
        material_index: usize,
        size: [u32; 2],
    ) -> &mut SelectionTexture {
        let recreate = self.textures.get(&mask_id).is_none_or(|selection| {
            selection.size != size || selection.material_index != material_index
        });
        if recreate {
            self.textures.insert(
                mask_id,
                create_selection_texture(device, material_index, size),
            );
        }
        self.textures
            .get_mut(&mask_id)
            .expect("selection texture was just ensured")
    }

    pub(crate) fn replace_material_mask(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        material_index: usize,
        texture_size: [u32; 2],
        snapshot: Option<&SelectionMaskSnapshot>,
    ) -> anyhow::Result<SelectionUploadStats> {
        let mask_id = selection_mask_id_for_material(material_index.into());
        let Some(snapshot) = snapshot else {
            self.textures.remove(&mask_id);
            return Ok(SelectionUploadStats::default());
        };
        anyhow::ensure!(
            snapshot.texture_size == texture_size,
            "selection snapshot size mismatch for material {}",
            material_index
        );
        let expected_len = selection_mask_len(texture_size)?;
        anyhow::ensure!(
            snapshot.r8.len() == expected_len,
            "selection snapshot payload size mismatch for material {}",
            material_index
        );
        let mut selection = create_selection_texture(device, material_index, texture_size);
        selection.pixels.copy_from_slice(&snapshot.r8);
        let stats = selection.upload_full_into_frame(device, frame);
        self.textures.insert(mask_id, selection);
        Ok(stats)
    }

    pub(crate) fn texture_size(&self, mask_id: SelectionMaskId) -> Option<[u32; 2]> {
        self.textures.get(&mask_id).map(|selection| selection.size)
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        for selection in self.textures.values() {
            let bytes = r8_texture_bytes(selection.size);
            metrics.add_total_bytes(bytes);
            metrics.selection_mask_bytes = metrics.selection_mask_bytes.saturating_add(bytes);
            metrics.selection_mask_count = metrics.selection_mask_count.saturating_add(1);
        }
        metrics
    }
}

fn r8_texture_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize).saturating_mul(size[1] as usize)
}

impl SelectionUploadStats {
    pub(crate) fn record(&mut self, texture_size: [u32; 2], rect: RectU32) {
        self.upload_count = self.upload_count.saturating_add(1);
        self.uploaded_bytes = self
            .uploaded_bytes
            .saturating_add(rect.size[0] as usize * rect.size[1] as usize);
        if rect.origin == [0, 0] && rect.size == texture_size {
            self.full_upload_count = self.full_upload_count.saturating_add(1);
        } else {
            self.rect_upload_count = self.rect_upload_count.saturating_add(1);
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.upload_count = self.upload_count.saturating_add(other.upload_count);
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(other.uploaded_bytes);
        self.full_upload_count = self
            .full_upload_count
            .saturating_add(other.full_upload_count);
        self.rect_upload_count = self
            .rect_upload_count
            .saturating_add(other.rect_upload_count);
    }
}

fn patch_r8_rect(
    dst: &mut [u8],
    texture_size: [u32; 2],
    rect: RectU32,
    r8: &[u8],
) -> anyhow::Result<()> {
    validate_r8_rect(texture_size, rect)?;
    let expected = (rect.size[0] as usize).saturating_mul(rect.size[1] as usize);
    anyhow::ensure!(r8.len() == expected, "selection tile payload size mismatch");
    let width = texture_size[0] as usize;
    for row in 0..rect.size[1] as usize {
        let dst_start = (rect.origin[1] as usize + row) * width + rect.origin[0] as usize;
        let src_start = row * rect.size[0] as usize;
        dst[dst_start..dst_start + rect.size[0] as usize]
            .copy_from_slice(&r8[src_start..src_start + rect.size[0] as usize]);
    }
    Ok(())
}

impl SelectionTexture {
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub(crate) fn mark_gpu_initialized(&mut self) {
        self.initialized = true;
    }

    pub(crate) fn mark_gpu_written(&mut self) {
        self.initialized = true;
    }

    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub(crate) fn upload_into_frame(&self, device: &wgpu::Device, frame: &mut GpuFrame) {
        frame.write_texture_r8(device, &self.texture, [0, 0], self.size, &self.pixels);
    }

    pub(crate) fn upload_full_into_frame(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
    ) -> SelectionUploadStats {
        self.upload_into_frame(device, frame);
        self.initialized = true;
        let rect = RectU32 {
            origin: [0, 0],
            size: self.size,
        };
        let mut stats = SelectionUploadStats::default();
        stats.record(self.size, rect);
        stats
    }

    pub(crate) fn upload_rect_into_frame(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        rect: RectU32,
    ) -> SelectionUploadStats {
        if !self.initialized {
            return self.upload_full_into_frame(device, frame);
        }
        let Ok(r8) = copy_r8_rect(&self.pixels, self.size, rect) else {
            return SelectionUploadStats::default();
        };
        frame.write_texture_r8(device, &self.texture, rect.origin, rect.size, &r8);
        let mut stats = SelectionUploadStats::default();
        stats.record(self.size, rect);
        stats
    }
}

fn create_selection_texture(
    device: &wgpu::Device,
    material_index: usize,
    size: [u32; 2],
) -> SelectionTexture {
    let size = [size[0].max(1), size[1].max(1)];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("selection_mask_r8"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    SelectionTexture {
        material_index,
        size,
        texture,
        view,
        pixels: vec![0; size[0] as usize * size[1] as usize],
        initialized: false,
    }
}

fn copy_r8_rect(src: &[u8], texture_size: [u32; 2], rect: RectU32) -> anyhow::Result<Vec<u8>> {
    validate_r8_rect(texture_size, rect)?;
    let expected = texture_size[0] as usize * texture_size[1] as usize;
    anyhow::ensure!(src.len() == expected, "selection source size mismatch");
    let mut dst = vec![0; rect.size[0] as usize * rect.size[1] as usize];
    let width = texture_size[0] as usize;
    for row in 0..rect.size[1] as usize {
        let src_start = (rect.origin[1] as usize + row) * width + rect.origin[0] as usize;
        let dst_start = row * rect.size[0] as usize;
        dst[dst_start..dst_start + rect.size[0] as usize]
            .copy_from_slice(&src[src_start..src_start + rect.size[0] as usize]);
    }
    Ok(dst)
}

fn validate_r8_rect(texture_size: [u32; 2], rect: RectU32) -> anyhow::Result<()> {
    anyhow::ensure!(
        rect.size[0] > 0 && rect.size[1] > 0,
        "selection rect is empty"
    );
    let end_x = rect.origin[0]
        .checked_add(rect.size[0])
        .ok_or_else(|| anyhow::anyhow!("selection rect x range overflows"))?;
    let end_y = rect.origin[1]
        .checked_add(rect.size[1])
        .ok_or_else(|| anyhow::anyhow!("selection rect y range overflows"))?;
    anyhow::ensure!(
        end_x <= texture_size[0] && end_y <= texture_size[1],
        "selection rect is outside texture bounds"
    );
    Ok(())
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

pub(crate) type SelectionMasks = SelectionStore;

/// Mutation boundary for renderer-side selection mask synchronization.
///
/// CPU-owned selection pixels enter the GPU document through this context.
/// The upload and the corresponding `TilesUploaded` mutation are recorded as a
/// single boundary operation.
pub(crate) struct SelectionSyncContext<'a> {
    selections: &'a mut SelectionMasks,
    log: &'a mut MutationLog,
}

impl<'a> SelectionSyncContext<'a> {
    pub(crate) fn new(selections: &'a mut SelectionMasks, log: &'a mut MutationLog) -> Self {
        Self { selections, log }
    }

    pub(crate) fn replace_material_mask(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        material_index: usize,
        texture_size: [u32; 2],
        snapshot: Option<&SelectionMaskSnapshot>,
        active_selection: &ActiveSelection,
    ) -> anyhow::Result<SelectionUploadStats> {
        let stats = self.selections.replace_material_mask(
            device,
            frame,
            material_index,
            texture_size,
            snapshot,
        )?;
        self.log.selections.tiles_uploaded(active_selection.clone());
        Ok(stats)
    }

    pub(crate) fn upload_tiles(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        materials: &super::materials::MaterialRegistry,
        active_selection: &ActiveSelection,
        tiles: &[SelectionTilePayload],
    ) -> anyhow::Result<SelectionUploadStats> {
        let stats =
            self.selections
                .patch_tiles(device, frame, materials, active_selection, tiles)?;
        self.log.selections.tiles_uploaded(active_selection.clone());
        Ok(stats)
    }
}

/// Mutation-aware boundary for GPU-produced selection mask edits.
///
/// Edit commands update selection masks through this context so mask writes and
/// `MutationLog` damage are inseparable at the selection feature boundary.
pub(crate) struct SelectionEditContext<'a> {
    selections: &'a mut SelectionMasks,
    log: &'a mut MutationLog,
}

impl<'a> SelectionEditContext<'a> {
    pub(crate) fn new(selections: &'a mut SelectionMasks, log: &'a mut MutationLog) -> Self {
        Self { selections, log }
    }

    pub(crate) fn update_mask(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        scene: &crate::renderer::document::scene::SceneResources,
        materials: &crate::renderer::document::materials::MaterialRegistry,
        mask: &MaskSource,
        params: ApplyParams<SelectionCompositeMode>,
        active_selection: &ActiveSelection,
    ) -> anyhow::Result<SelectionUploadStats> {
        let stats = crate::renderer::features::selection::apply::apply_to_selection_mask(
            self.selections,
            device,
            frame,
            scene,
            materials,
            mask,
            params,
            active_selection,
        )?;
        self.log.selections.mask_changed(active_selection.clone());
        Ok(stats)
    }
}
