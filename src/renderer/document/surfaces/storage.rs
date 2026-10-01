use anyhow::{Result, anyhow, bail};
use eframe::egui_wgpu::wgpu;

use crate::core::{
    damage::DamageMap,
    geometry::RectU32,
    image::{LayerInitialPixels, Rgba8Snapshot, rgba8_len, validate_rect},
    surface::PaintSurfaceId,
    tile_cache::TileCacheOpStats,
    tile_payload::{PixelSnapshotData, TilePayload},
};
use crate::renderer::{
    engine::{gpu_state::RendererGpuState, readback},
    features::brush::transient as stroke_transient,
    gpu::{create_paint_texture, frame::GpuFrame, frame::TextureUploadRegion},
    pixel::rgba8_alpha_to_r8,
    transient::TransientTextures,
};

use super::{
    EncodedSurfaceReadback, SurfaceEditTarget, SurfacePrepareStats, SurfaceReadResult,
    SurfaceReadSource, SurfaceRepository,
    record::{ResidentSurfaceTexture, SurfacePixelFormat, SurfaceRecord},
    rects::snapshot_rect,
    tile_shadow::SurfaceTileShadow,
};

pub(super) struct MaskProxyConvertResources {
    bind_group_layout: wgpu::BindGroupLayout,
    mask_to_proxy_pipeline: wgpu::RenderPipeline,
    proxy_to_mask_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl MaskProxyConvertResources {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer_mask_proxy_convert_shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_MASK_PROXY_CONVERT_WGSL.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer_mask_proxy_convert_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer_mask_proxy_convert_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let mask_to_proxy_pipeline = create_mask_convert_pipeline(
            device,
            &pipeline_layout,
            &shader,
            wgpu::TextureFormat::Rgba8Unorm,
            "fs_mask_to_proxy",
            "layer_mask_to_proxy",
        );
        let proxy_to_mask_pipeline = create_mask_convert_pipeline(
            device,
            &pipeline_layout,
            &shader,
            wgpu::TextureFormat::R8Unorm,
            "fs_proxy_to_mask",
            "layer_mask_proxy_to_r8",
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer_mask_proxy_convert_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            bind_group_layout,
            mask_to_proxy_pipeline,
            proxy_to_mask_pipeline,
            sampler,
        }
    }
}

fn create_mask_convert_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    fragment_entry: &str,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen_triangle"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

pub(crate) struct StrokeSurfaceRecordTarget<'a> {
    pub surface: PaintSurfaceId,
    pub texture_size: [u32; 2],
    pub read_texture: &'a wgpu::Texture,
    pub read_view: &'a wgpu::TextureView,
    pub write_texture: &'a wgpu::Texture,
    pub write_view: &'a wgpu::TextureView,
}

impl SurfaceRepository {
    pub(crate) fn upload_scene_materials_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        materials: &[crate::renderer::document::materials::MaterialUpload],
        surfaces: &[PaintSurfaceId],
    ) -> Result<()> {
        self.surfaces.clear();
        self.mask_edit_proxies.clear();
        for (index, material) in materials.iter().enumerate() {
            if let Some(target) = surfaces
                .iter()
                .copied()
                .find(|surface| surface.material_index.as_usize() == index)
            {
                self.create_surface_record_from_rgba8_into_frame(
                    gpu,
                    frame,
                    target,
                    [material.width, material.height],
                    &material.rgba8,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn create_surface_record_into_frame(
        &mut self,
        _gpu: &RendererGpuState,
        _frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        initial: LayerInitialPixels,
    ) -> Result<()> {
        match initial {
            LayerInitialPixels::Transparent => {
                let layer = SurfaceRecord::new_transparent_nonresident(target, size)?;
                self.surfaces.insert(target, layer);
                self.mark_surface_used(target);
                Ok(())
            }
            LayerInitialPixels::SolidRgba8(rgba) => {
                let pixel_count = (size[0] as usize).saturating_mul(size[1] as usize);
                let mut rgba8 = Vec::with_capacity(pixel_count.saturating_mul(4));
                for _ in 0..pixel_count {
                    rgba8.extend_from_slice(&rgba);
                }
                let snapshot = Rgba8Snapshot::new(size, [0, 0], size, rgba8)?;
                let layer = SurfaceRecord::new_transparent_nonresident(target, size)?;
                layer.tile_shadow.borrow_mut().apply_cpu_upload(&snapshot)?;
                self.surfaces.insert(target, layer);
                self.mark_surface_used(target);
                Ok(())
            }
        }
    }

    pub(crate) fn delete_surface_record(&mut self, target: PaintSurfaceId) {
        self.surfaces.remove(&target);
        self.mask_edit_proxies.remove(&target);
    }

    pub(crate) fn duplicate_surface_record_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        from: PaintSurfaceId,
        to: PaintSurfaceId,
    ) -> Result<()> {
        let from_layer = self
            .surfaces
            .get(&from)
            .ok_or_else(|| anyhow!("source surface texture does not exist: {:?}", from))?;
        let from_shadow = from_layer.tile_shadow.borrow().clone();
        let new_layer = if from_shadow.is_fully_fresh() {
            SurfaceRecord::new_transparent_nonresident(to, from_layer.texture_size)?
        } else if let Some(from_texture) = from_layer.gpu_texture() {
            let new_layer = SurfaceRecord::new_with_format(
                gpu.device(),
                to,
                from_layer.texture_size,
                from_layer.format,
            )?;
            copy_texture(
                frame.encoder(),
                from_texture,
                new_layer.require_gpu_texture()?,
                from_layer.texture_size,
            );
            new_layer
        } else {
            bail!("cannot duplicate non-resident surface while tile shadow is stale");
        };
        *new_layer.tile_shadow.borrow_mut() = from_shadow;
        self.surfaces.insert(to, new_layer);
        self.mask_edit_proxies.remove(&to);
        self.mark_surface_used(to);
        Ok(())
    }

    // Readback is synchronous and blocks the UI thread. Normal commit
    // finalization must use the async commit readback queue instead.
    pub(crate) fn read_surface_record_rect_rgba8(
        &self,
        gpu: &RendererGpuState,
        queue: &wgpu::Queue,
        target: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<SurfaceReadResult> {
        let layer = self.surface_record(target)?;
        validate_rect(layer.texture_size, origin, size)?;

        if let Some((snapshot, stats)) = layer.tile_shadow.borrow().read_if_fresh(origin, size)? {
            self.mark_surface_used(target);
            return Ok(SurfaceReadResult {
                snapshot,
                source: SurfaceReadSource::TileCache(stats),
            });
        }

        let snapshot = match layer.format {
            SurfacePixelFormat::Rgba8 => readback::blocking_read_texture_rect_rgba8(
                gpu.device(),
                queue,
                layer.require_gpu_texture()?,
                layer.texture_size,
                origin,
                size,
            )?,
            SurfacePixelFormat::R8 => readback::blocking_read_texture_rect_r8_as_rgba8(
                gpu.device(),
                queue,
                layer.require_gpu_texture()?,
                layer.texture_size,
                origin,
                size,
            )?,
        };
        self.mark_surface_used(target);
        let mut shadow = layer.tile_shadow.borrow_mut();
        let stats = shadow.cache.apply_rgba_snapshot(&snapshot)?;
        shadow.mark_fresh_rect(snapshot_rect(&snapshot));
        Ok(SurfaceReadResult {
            snapshot,
            source: SurfaceReadSource::GpuReadback {
                cache_update: Some(stats),
            },
        })
    }

    pub(crate) fn encode_surface_record_readback_rgba8(
        &self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
    ) -> Result<EncodedSurfaceReadback> {
        let layer = self.surface_record(target)?;
        validate_rect(layer.texture_size, origin, size)?;
        let bytes_per_pixel = layer.format.bytes_per_pixel();
        let unpadded_bytes_per_row = size[0] * bytes_per_pixel;
        let padded_bytes_per_row =
            align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer_size = padded_bytes_per_row as u64 * size[1] as u64;
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("surface_commit_readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        frame.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: layer.require_gpu_texture()?,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
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
        Ok(EncodedSurfaceReadback {
            texture_size: layer.texture_size,
            pixel_format: layer.format,
            buffer,
            padded_bytes_per_row,
        })
    }

    pub(crate) fn apply_surface_readback_data(
        &mut self,
        target: PaintSurfaceId,
        origin: [u32; 2],
        size: [u32; 2],
        data: &PixelSnapshotData,
    ) -> Result<TileCacheOpStats> {
        let layer = self.surface_record(target)?;
        let texture_size = layer.texture_size;
        let stats = layer.tile_shadow.borrow_mut().apply_pixel_snapshot_data(
            texture_size,
            origin,
            size,
            data,
        )?;
        self.mark_surface_used(target);
        Ok(stats)
    }

    pub(crate) fn surface_tile_shadow_stale_full(&self, target: PaintSurfaceId) -> bool {
        self.surfaces
            .get(&target)
            .is_some_and(|layer| layer.tile_shadow.borrow().stale_full)
    }

    pub(crate) fn upload_surface_record_rgba8_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        snapshot: &Rgba8Snapshot,
        document_revision: Option<u64>,
    ) -> Result<TileCacheOpStats> {
        let layer = self.surface_record(target)?;
        let tex_size = layer.texture_size;
        if snapshot.texture_size != tex_size {
            bail!(
                "snapshot texture size {:?} does not match target texture size {:?}",
                snapshot.texture_size,
                tex_size
            );
        }
        validate_rect(tex_size, snapshot.origin, snapshot.size)?;
        let expected_len = rgba8_len(snapshot.size)?;
        if snapshot.rgba8.len() != expected_len {
            bail!(
                "invalid RGBA payload size: got {}, expected {}",
                snapshot.rgba8.len(),
                expected_len
            );
        }

        let uploaded_to_gpu = {
            let layer = self.surface_record(target)?;
            if let Some(texture) = layer.gpu_texture() {
                match layer.format {
                    SurfacePixelFormat::Rgba8 => frame.write_texture_rgba8(
                        gpu.device(),
                        texture,
                        snapshot.origin,
                        snapshot.size,
                        &snapshot.rgba8,
                    ),
                    SurfacePixelFormat::R8 => {
                        let r8 = rgba8_alpha_to_r8(&snapshot.rgba8);
                        frame.write_texture_r8(
                            gpu.device(),
                            texture,
                            snapshot.origin,
                            snapshot.size,
                            &r8,
                        );
                    }
                }
                true
            } else {
                false
            }
        };
        if target.is_mask() {
            self.mask_edit_proxies.remove(&target);
        }
        if uploaded_to_gpu {
            self.mark_surface_used(target);
        }
        let layer = self.surface_record(target)?;
        layer.uploaded_document_revision.set(document_revision);
        layer.tile_shadow.borrow_mut().apply_cpu_upload(snapshot)
    }

    pub(crate) fn upload_surface_record_rgba8_tiles_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        tiles: &[TilePayload],
        document_revision: Option<u64>,
    ) -> Result<TileCacheOpStats> {
        let layer = self.surface_record(target)?;
        let tex_size = layer.texture_size;
        if tiles.is_empty() {
            return Ok(TileCacheOpStats::default());
        }
        for tile in tiles {
            validate_rect(tex_size, tile.rect.origin, tile.rect.size)?;
            let expected_len = rgba8_len(tile.rect.size)?;
            if tile.rgba8.len() != expected_len {
                bail!(
                    "invalid RGBA payload size: got {}, expected {}",
                    tile.rgba8.len(),
                    expected_len
                );
            }
        }

        let layer = self.surface_record(target)?;
        layer.uploaded_document_revision.set(document_revision);
        let mut total_stats = TileCacheOpStats::default();
        for tile in tiles {
            let stats = layer
                .tile_shadow
                .borrow_mut()
                .apply_rgba8_tile_payload(tex_size, tile)?;
            total_stats.tile_count = total_stats.tile_count.saturating_add(stats.tile_count);
            total_stats.bytes = total_stats.bytes.saturating_add(stats.bytes);
        }

        let uploaded_to_gpu = {
            let layer = self.surface_record(target)?;
            if let Some(texture) = layer.gpu_texture() {
                match layer.format {
                    SurfacePixelFormat::Rgba8 => {
                        let regions = tiles
                            .iter()
                            .map(|tile| TextureUploadRegion {
                                origin: tile.rect.origin,
                                size: tile.rect.size,
                                unpadded_bytes_per_row: tile.rect.size[0]
                                    .checked_mul(4)
                                    .expect("RGBA8 upload row pitch overflowed"),
                                data: tile.rgba8.as_slice(),
                            })
                            .collect::<Vec<_>>();
                        frame.write_texture_rgba8_regions(gpu.device(), texture, &regions);
                    }
                    SurfacePixelFormat::R8 => {
                        for tile in tiles {
                            let r8 = rgba8_alpha_to_r8(tile.rgba8.as_slice());
                            frame.write_texture_r8(
                                gpu.device(),
                                texture,
                                tile.rect.origin,
                                tile.rect.size,
                                &r8,
                            );
                        }
                    }
                }
                true
            } else {
                false
            }
        };
        if target.is_mask() {
            self.mask_edit_proxies.remove(&target);
        }
        if uploaded_to_gpu {
            self.mark_surface_used(target);
        }
        Ok(total_stats)
    }

    pub(crate) fn mark_surface_cache_stale(
        &mut self,
        surface: PaintSurfaceId,
    ) -> SurfacePrepareStats {
        if let Some(layer) = self.surfaces.get(&surface) {
            layer.tile_shadow.borrow_mut().mark_stale_full();
        }
        SurfacePrepareStats::default()
    }

    pub(crate) fn mark_surface_cache_stale_rect(
        &mut self,
        surface: PaintSurfaceId,
        rect: RectU32,
    ) -> SurfacePrepareStats {
        if let Some(layer) = self.surfaces.get(&surface) {
            layer.tile_shadow.borrow_mut().mark_stale_rect(rect);
        }
        SurfacePrepareStats::default()
    }

    pub(crate) fn mark_surface_cache_stale_damage(
        &mut self,
        damage: Option<&DamageMap>,
        fallback_surface: PaintSurfaceId,
    ) -> SurfacePrepareStats {
        if damage.is_none() || damage.is_some_and(DamageMap::is_empty) {
            return self.mark_surface_cache_stale(fallback_surface);
        }
        let damage = damage.expect("checked above");
        let mut stats = SurfacePrepareStats::default();
        for pixel in &damage.pixels {
            merge_prepare_stats(
                &mut stats,
                self.mark_surface_cache_stale_rect(pixel.surface, pixel.rect),
            );
        }
        stats
    }

    pub(crate) fn surface_record_texture_size(&self, target: PaintSurfaceId) -> Option<[u32; 2]> {
        self.surfaces.get(&target).map(|layer| layer.texture_size)
    }

    pub(crate) fn surface_record_texture_view(
        &self,
        target: PaintSurfaceId,
    ) -> Option<&wgpu::TextureView> {
        self.surfaces.get(&target).and_then(SurfaceRecord::gpu_view)
    }

    pub(crate) fn surface_record_edit_target(
        &self,
        target: PaintSurfaceId,
    ) -> Option<SurfaceEditTarget<'_>> {
        let layer = self.surfaces.get(&target)?;
        if target.is_mask() {
            let proxy = self.mask_edit_proxies.get(&target)?;
            return Some(SurfaceEditTarget {
                texture_size: layer.texture_size,
                texture: &proxy.texture,
                view: &proxy.view,
            });
        }
        Some(SurfaceEditTarget {
            texture_size: layer.texture_size,
            texture: layer.gpu_texture()?,
            view: layer.gpu_view()?,
        })
    }

    pub(crate) fn surface_record_texture(&self, target: PaintSurfaceId) -> Option<&wgpu::Texture> {
        self.surfaces
            .get(&target)
            .and_then(SurfaceRecord::gpu_texture)
    }

    pub(crate) fn stroke_surface_record_target<'a>(
        &'a self,
        target: PaintSurfaceId,
        scratch: &'a TransientTextures,
    ) -> Option<StrokeSurfaceRecordTarget<'a>> {
        let layer = self.surfaces.get(&target)?;
        let read_texture = stroke_transient::material_source_uv_texture(
            scratch,
            target.material_index().as_usize(),
        )?;
        let read_view =
            stroke_transient::material_source_uv_view(scratch, target.material_index().as_usize())?;
        if target.is_mask() {
            let proxy = self.mask_edit_proxies.get(&target)?;
            return Some(StrokeSurfaceRecordTarget {
                surface: target,
                texture_size: layer.texture_size,
                read_texture,
                read_view,
                write_texture: &proxy.texture,
                write_view: &proxy.view,
            });
        }
        Some(StrokeSurfaceRecordTarget {
            surface: target,
            texture_size: layer.texture_size,
            read_texture,
            read_view,
            write_texture: layer.require_gpu_texture().ok()?,
            write_view: layer.require_gpu_view().ok()?,
        })
    }

    pub(crate) fn replace_surface_record_from_rgba8_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        rgba8: &[u8],
    ) -> Result<()> {
        if !self.surfaces.contains_key(&target) {
            bail!("surface texture does not exist: {:?}", target);
        }
        self.create_surface_record_from_rgba8_into_frame(gpu, frame, target, size, rgba8)?;
        self.mask_edit_proxies.remove(&target);
        Ok(())
    }

    pub(crate) fn create_surface_record_from_rgba8_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        size: [u32; 2],
        rgba8: &[u8],
    ) -> Result<()> {
        if rgba8.len() != rgba8_len(size)? {
            bail!("invalid RGBA payload size");
        }
        let layer = SurfaceRecord::new(gpu.device(), target, size)?;
        match layer.format {
            SurfacePixelFormat::Rgba8 => frame.write_texture_rgba8(
                gpu.device(),
                layer.require_gpu_texture()?,
                [0, 0],
                size,
                rgba8,
            ),
            SurfacePixelFormat::R8 => {
                let r8 = rgba8_alpha_to_r8(rgba8);
                frame.write_texture_r8(
                    gpu.device(),
                    layer.require_gpu_texture()?,
                    [0, 0],
                    size,
                    &r8,
                );
            }
        }
        let snapshot = Rgba8Snapshot::new(size, [0, 0], size, rgba8.to_vec())?;
        *layer.tile_shadow.borrow_mut() = SurfaceTileShadow::from_full_snapshot(&snapshot)?;
        self.surfaces.insert(target, layer);
        self.mark_surface_used(target);
        Ok(())
    }

    fn surface_record(&self, target: PaintSurfaceId) -> Result<&SurfaceRecord> {
        self.surfaces
            .get(&target)
            .ok_or_else(|| anyhow!("surface texture does not exist: {:?}", target))
    }
}

impl SurfaceRepository {
    pub(crate) fn ensure_mask_edit_proxy_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
    ) -> Result<()> {
        if !target.is_mask() || self.mask_edit_proxies.contains_key(&target) {
            return Ok(());
        }
        let record = self.surface_record(target)?;
        if record.format != SurfacePixelFormat::R8 {
            return Ok(());
        }
        self.ensure_mask_proxy_convert_resources(gpu.device());
        let record = self.surface_record(target)?;
        let mask_view = record.require_gpu_view()?;
        let size = record.texture_size;
        let (texture, view) = create_paint_texture(gpu.device(), size, "layer_mask_edit_proxy");
        record_mask_to_proxy_pass(
            gpu.device(),
            self.mask_proxy_convert_resources(),
            frame,
            mask_view,
            &view,
            size,
        );
        self.mask_edit_proxies
            .insert(target, ResidentSurfaceTexture { texture, view });
        self.mark_surface_used(target);
        Ok(())
    }

    pub(crate) fn resolve_mask_edit_proxy_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
    ) -> Result<()> {
        if !target.is_mask() {
            return Ok(());
        }
        let texture_size = self.surface_record(target)?.texture_size;
        self.resolve_mask_edit_proxy_rects_into_frame(
            gpu,
            frame,
            target,
            std::slice::from_ref(&RectU32::full(texture_size)),
        )
    }

    pub(crate) fn resolve_mask_edit_proxy_rects_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        rects: &[RectU32],
    ) -> Result<()> {
        if !target.is_mask() || rects.is_empty() {
            return Ok(());
        }
        self.ensure_mask_edit_proxy_into_frame(gpu, frame, target)?;
        self.ensure_mask_proxy_convert_resources(gpu.device());
        let proxy = self
            .mask_edit_proxies
            .get(&target)
            .ok_or_else(|| anyhow!("layer mask edit proxy does not exist: {:?}", target))?;
        let record = self.surface_record(target)?;
        if record.format != SurfacePixelFormat::R8 {
            return Ok(());
        }
        for rect in rects {
            validate_rect(record.texture_size, rect.origin, rect.size)?;
        }
        let resources = self.mask_proxy_convert_resources();
        record_proxy_to_mask_rects_pass(
            gpu.device(),
            resources,
            frame,
            &proxy.view,
            record.require_gpu_view()?,
            record.texture_size,
            rects,
        );
        self.mark_surface_used(target);
        Ok(())
    }

    pub(crate) fn resolve_mask_edit_proxies_into_frame(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        targets: impl IntoIterator<Item = PaintSurfaceId>,
    ) -> Result<()> {
        for target in targets {
            self.resolve_mask_edit_proxy_into_frame(gpu, frame, target)?;
        }
        Ok(())
    }
}

fn record_mask_to_proxy_pass(
    device: &wgpu::Device,
    resources: &MaskProxyConvertResources,
    frame: &mut GpuFrame,
    mask_view: &wgpu::TextureView,
    proxy_view: &wgpu::TextureView,
    size: [u32; 2],
) {
    let full_rect = RectU32::full(size);
    record_mask_convert_pass(
        device,
        resources,
        frame,
        mask_view,
        proxy_view,
        &resources.mask_to_proxy_pipeline,
        "layer_mask_to_proxy",
        size,
        std::slice::from_ref(&full_rect),
    );
}

fn record_proxy_to_mask_rects_pass(
    device: &wgpu::Device,
    resources: &MaskProxyConvertResources,
    frame: &mut GpuFrame,
    proxy_view: &wgpu::TextureView,
    mask_view: &wgpu::TextureView,
    size: [u32; 2],
    rects: &[RectU32],
) {
    record_mask_convert_pass(
        device,
        resources,
        frame,
        proxy_view,
        mask_view,
        &resources.proxy_to_mask_pipeline,
        "layer_mask_proxy_to_r8",
        size,
        rects,
    );
}

fn record_mask_convert_pass(
    device: &wgpu::Device,
    resources: &MaskProxyConvertResources,
    frame: &mut GpuFrame,
    source_view: &wgpu::TextureView,
    target_view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    label: &'static str,
    size: [u32; 2],
    rects: &[RectU32],
) {
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("layer_mask_proxy_convert_bg"),
        layout: &resources.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&resources.sampler),
            },
        ],
    });
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.set_viewport(
        0.0,
        0.0,
        size[0].max(1) as f32,
        size[1].max(1) as f32,
        0.0,
        1.0,
    );
    for rect in rects {
        pass.set_scissor_rect(rect.origin[0], rect.origin[1], rect.size[0], rect.size[1]);
        pass.draw(0..3, 0..1);
    }
}

const LAYER_MASK_PROXY_CONVERT_WGSL: &str = r#"
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen_triangle(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    let pos = positions[vertex_index];
    var out: VsOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = vec2<f32>(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
    return out;
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@fragment
fn fs_mask_to_proxy(in: VsOut) -> @location(0) vec4<f32> {
    let m = clamp(textureSampleLevel(source_tex, source_sampler, in.uv, 0.0).r, 0.0, 1.0);
    return vec4<f32>(m, m, m, m);
}

@fragment
fn fs_proxy_to_mask(in: VsOut) -> @location(0) f32 {
    return clamp(textureSampleLevel(source_tex, source_sampler, in.uv, 0.0).a, 0.0, 1.0);
}
"#;

fn merge_prepare_stats(left: &mut SurfacePrepareStats, right: SurfacePrepareStats) {
    left.evicted |= right.evicted;
    left.rehydrated |= right.rehydrated;
    left.uploaded_tiles = left.uploaded_tiles.saturating_add(right.uploaded_tiles);
    left.uploaded_bytes = left.uploaded_bytes.saturating_add(right.uploaded_bytes);
}

fn copy_texture(
    encoder: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    size: [u32; 2],
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: src,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: dst,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
