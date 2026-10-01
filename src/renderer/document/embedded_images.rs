use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::core::{
    embedded_image::{EmbeddedImageAsset, EmbeddedImageId, EmbeddedImageTransform},
    surface::LayerId,
};

const DERIVED_CACHE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const SOURCE_CACHE_BUDGET_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DerivedImageKey {
    image_id: EmbeddedImageId,
    center_uv: [u32; 2],
    size_uv: [u32; 2],
    rotation_radians: u32,
    output_size: [u32; 2],
}

impl DerivedImageKey {
    fn new(
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        output_size: [u32; 2],
    ) -> Self {
        Self {
            image_id,
            center_uv: transform.center_uv.to_array().map(f32::to_bits),
            size_uv: transform.size_uv.to_array().map(f32::to_bits),
            rotation_radians: transform.rotation_radians.to_bits(),
            output_size,
        }
    }
}

#[derive(Debug)]
struct CachedTexture {
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    byte_len: usize,
    last_used: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum ResolvedEmbeddedImage {
    Derived {
        texture: Arc<wgpu::Texture>,
        view: Arc<wgpu::TextureView>,
    },
    Transformed {
        texture: Arc<wgpu::Texture>,
        view: Arc<wgpu::TextureView>,
        transform: EmbeddedImageTransform,
    },
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RasterizeUniform {
    output_uv_to_source_uv_row0: [f32; 4],
    output_uv_to_source_uv_row1: [f32; 4],
}

#[derive(Debug)]
struct RasterizePipeline {
    uniform: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl RasterizePipeline {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("embedded_image_rasterize_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/embedded_image_rasterize.wgsl").into(),
            ),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("embedded_image_rasterize_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("embedded_image_rasterize_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("embedded_image_rasterize_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
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
        });
        Self {
            uniform: crate::renderer::gpu::buffer::create_uniform_buffer::<RasterizeUniform>(
                device,
                "embedded_image_rasterize_uniform",
            ),
            bind_group_layout,
            pipeline,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EmbeddedImageRegistry {
    assets: HashMap<EmbeddedImageId, Arc<EmbeddedImageAsset>>,
    sources: RefCell<HashMap<EmbeddedImageId, CachedTexture>>,
    source_bytes: Cell<usize>,
    derived: RefCell<HashMap<DerivedImageKey, CachedTexture>>,
    derived_bytes: Cell<usize>,
    preview_transforms: RefCell<HashMap<LayerId, EmbeddedImageTransform>>,
    preview_sources: RefCell<HashMap<LayerId, EmbeddedImageId>>,
    access_epoch: Cell<u64>,
    rasterize_pipeline: RefCell<Option<RasterizePipeline>>,
}

impl EmbeddedImageRegistry {
    pub(crate) fn replace(&mut self, assets: Vec<Arc<EmbeddedImageAsset>>) {
        self.preview_transforms.get_mut().clear();
        self.preview_sources.get_mut().clear();
        self.evict_sources_for(0);
        let incoming = assets
            .into_iter()
            .map(|asset| (asset.id, asset))
            .collect::<HashMap<_, _>>();
        let removed = self
            .assets
            .keys()
            .filter(|id| !incoming.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        for id in removed {
            self.remove(id);
        }
        for asset in incoming.into_values() {
            self.upsert(asset);
        }
    }

    pub(crate) fn upsert(&mut self, asset: Arc<EmbeddedImageAsset>) {
        let changed = self
            .assets
            .get(&asset.id)
            .is_none_or(|current| current.as_ref() != asset.as_ref());
        if changed {
            self.invalidate_asset(asset.id);
        }
        self.assets.insert(asset.id, asset);
    }

    pub(crate) fn remove(&mut self, image_id: EmbeddedImageId) {
        self.assets.remove(&image_id);
        self.invalidate_asset(image_id);
        let preview_layers = self
            .preview_sources
            .get_mut()
            .iter()
            .filter_map(|(layer_id, source_id)| (*source_id == image_id).then_some(*layer_id))
            .collect::<Vec<_>>();
        for layer_id in preview_layers {
            self.preview_sources.get_mut().remove(&layer_id);
            self.preview_transforms.get_mut().remove(&layer_id);
        }
    }

    pub(crate) fn set_preview(&self, layer_id: LayerId, transform: Option<EmbeddedImageTransform>) {
        if let Some(transform) = transform {
            self.preview_transforms
                .borrow_mut()
                .insert(layer_id, transform);
        } else {
            self.preview_transforms.borrow_mut().remove(&layer_id);
            if self
                .preview_sources
                .borrow_mut()
                .remove(&layer_id)
                .is_some()
            {
                self.evict_sources_for(0);
            }
        }
    }

    pub(crate) fn resolve(
        &self,
        gpu: &crate::renderer::engine::gpu_state::RendererGpuState,
        frame: &mut crate::renderer::gpu::frame::GpuFrame,
        layer_id: LayerId,
        image_id: EmbeddedImageId,
        committed_transform: EmbeddedImageTransform,
        output_size: [u32; 2],
    ) -> Option<ResolvedEmbeddedImage> {
        let preview = self.preview_transforms.borrow().get(&layer_id).copied();
        let transform = preview.unwrap_or(committed_transform);
        if preview.is_some() || texture_byte_len(output_size)? > DERIVED_CACHE_BUDGET_BYTES {
            if preview.is_some() {
                self.pin_preview_source(layer_id, image_id);
            }
            let source = self.resolve_source(gpu, frame, image_id, preview.is_some())?;
            return Some(ResolvedEmbeddedImage::Transformed {
                texture: source.0,
                view: source.1,
                transform,
            });
        }
        self.resolve_derived(gpu, frame, image_id, transform, output_size)
            .map(|(texture, view)| ResolvedEmbeddedImage::Derived { texture, view })
    }

    fn resolve_derived(
        &self,
        gpu: &crate::renderer::engine::gpu_state::RendererGpuState,
        frame: &mut crate::renderer::gpu::frame::GpuFrame,
        image_id: EmbeddedImageId,
        transform: EmbeddedImageTransform,
        output_size: [u32; 2],
    ) -> Option<(Arc<wgpu::Texture>, Arc<wgpu::TextureView>)> {
        if !transform.is_valid() || output_size.contains(&0) {
            return None;
        }
        let key = DerivedImageKey::new(image_id, transform, output_size);
        let epoch = self.next_epoch();
        if let Some(cached) = self.derived.borrow_mut().get_mut(&key) {
            cached.last_used = epoch;
            return Some((cached.texture.clone(), cached.view.clone()));
        }
        let byte_len = texture_byte_len(output_size)?;
        let (source_texture, source_view) = self.resolve_source(gpu, frame, image_id, false)?;
        let (texture, view) = crate::renderer::gpu::create_paint_texture(
            gpu.device(),
            output_size,
            "embedded_image_derived_raster",
        );
        self.record_rasterize(gpu.device(), frame, &source_view, &view, transform);
        drop(source_texture);
        self.evict_derived_for(byte_len);
        let cached = CachedTexture {
            texture: Arc::new(texture),
            view: Arc::new(view),
            byte_len,
            last_used: epoch,
        };
        let result = (cached.texture.clone(), cached.view.clone());
        self.derived.borrow_mut().insert(key, cached);
        self.derived_bytes
            .set(self.derived_bytes.get().saturating_add(byte_len));
        Some(result)
    }

    fn resolve_source(
        &self,
        gpu: &crate::renderer::engine::gpu_state::RendererGpuState,
        frame: &mut crate::renderer::gpu::frame::GpuFrame,
        image_id: EmbeddedImageId,
        pin: bool,
    ) -> Option<(Arc<wgpu::Texture>, Arc<wgpu::TextureView>)> {
        let epoch = self.next_epoch();
        if let Some(cached) = self.sources.borrow_mut().get_mut(&image_id) {
            cached.last_used = epoch;
            return Some((cached.texture.clone(), cached.view.clone()));
        }
        let asset = self.assets.get(&image_id)?;
        let byte_len = asset.rgba8.len();
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("embedded_image_source"),
            size: wgpu::Extent3d {
                width: asset.size[0],
                height: asset.size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        frame.write_texture_rgba8(gpu.device(), &texture, [0, 0], asset.size, &asset.rgba8);
        let texture = Arc::new(texture);
        let view = Arc::new(view);
        if pin || byte_len <= SOURCE_CACHE_BUDGET_BYTES {
            self.evict_sources_for(byte_len);
            if can_retain_source(self.source_bytes.get(), byte_len, pin) {
                self.sources.borrow_mut().insert(
                    image_id,
                    CachedTexture {
                        texture: texture.clone(),
                        view: view.clone(),
                        byte_len,
                        last_used: epoch,
                    },
                );
                self.source_bytes
                    .set(self.source_bytes.get().saturating_add(byte_len));
            }
        }
        Some((texture, view))
    }

    fn record_rasterize(
        &self,
        device: &wgpu::Device,
        frame: &mut crate::renderer::gpu::frame::GpuFrame,
        source_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        transform: EmbeddedImageTransform,
    ) {
        if self.rasterize_pipeline.borrow().is_none() {
            *self.rasterize_pipeline.borrow_mut() = Some(RasterizePipeline::new(device));
        }
        let pipeline = self.rasterize_pipeline.borrow();
        let pipeline = pipeline.as_ref().expect("pipeline initialized above");
        let rows = output_uv_to_source_uv_rows(transform);
        frame.write_buffer_pod(
            device,
            &pipeline.uniform,
            0,
            &RasterizeUniform {
                output_uv_to_source_uv_row0: rows[0],
                output_uv_to_source_uv_row1: rows[1],
            },
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("embedded_image_rasterize_bg"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pipeline.uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("embedded_image_rasterize_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn invalidate_asset(&mut self, image_id: EmbeddedImageId) {
        if let Some(source) = self.sources.get_mut().remove(&image_id) {
            self.source_bytes
                .set(self.source_bytes.get().saturating_sub(source.byte_len));
        }
        let keys = self
            .derived
            .get_mut()
            .keys()
            .filter(|key| key.image_id == image_id)
            .copied()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entry) = self.derived.get_mut().remove(&key) {
                self.derived_bytes
                    .set(self.derived_bytes.get().saturating_sub(entry.byte_len));
            }
        }
    }

    fn evict_sources_for(&self, incoming: usize) {
        let pinned = self
            .preview_sources
            .borrow()
            .values()
            .copied()
            .collect::<HashSet<_>>();
        evict_lru_excluding(
            &mut self.sources.borrow_mut(),
            &self.source_bytes,
            incoming,
            SOURCE_CACHE_BUDGET_BYTES,
            &pinned,
        );
    }

    fn pin_preview_source(&self, layer_id: LayerId, image_id: EmbeddedImageId) {
        let previous = self.preview_sources.borrow_mut().insert(layer_id, image_id);
        if previous.is_some_and(|previous| previous != image_id) {
            self.evict_sources_for(0);
        }
    }

    fn evict_derived_for(&self, incoming: usize) {
        evict_lru(
            &mut self.derived.borrow_mut(),
            &self.derived_bytes,
            incoming,
            DERIVED_CACHE_BUDGET_BYTES,
        );
    }

    fn next_epoch(&self) -> u64 {
        let epoch = self.access_epoch.get().wrapping_add(1);
        self.access_epoch.set(epoch);
        epoch
    }
}

fn evict_lru<K: Copy + Eq + std::hash::Hash>(
    cache: &mut HashMap<K, CachedTexture>,
    resident_bytes: &Cell<usize>,
    incoming: usize,
    budget: usize,
) {
    while resident_bytes.get().saturating_add(incoming) > budget {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key)
        else {
            break;
        };
        if let Some(entry) = cache.remove(&oldest) {
            resident_bytes.set(resident_bytes.get().saturating_sub(entry.byte_len));
        }
    }
}

fn evict_lru_excluding<K: Copy + Eq + std::hash::Hash>(
    cache: &mut HashMap<K, CachedTexture>,
    resident_bytes: &Cell<usize>,
    incoming: usize,
    budget: usize,
    excluded: &HashSet<K>,
) {
    while resident_bytes.get().saturating_add(incoming) > budget {
        let Some(oldest) = cache
            .iter()
            .filter(|(key, _)| !excluded.contains(key))
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key)
        else {
            break;
        };
        if let Some(entry) = cache.remove(&oldest) {
            resident_bytes.set(resident_bytes.get().saturating_sub(entry.byte_len));
        }
    }
}

pub(crate) fn output_uv_to_source_uv_rows(transform: EmbeddedImageTransform) -> [[f32; 4]; 2] {
    let sin = transform.rotation_radians.sin();
    let cos = transform.rotation_radians.cos();
    let row0_x = cos / transform.size_uv.x;
    let row0_y = sin / transform.size_uv.x;
    let row1_x = -sin / transform.size_uv.y;
    let row1_y = cos / transform.size_uv.y;
    [
        [
            row0_x,
            row0_y,
            0.5 - row0_x * transform.center_uv.x - row0_y * transform.center_uv.y,
            0.0,
        ],
        [
            row1_x,
            row1_y,
            0.5 - row1_x * transform.center_uv.x - row1_y * transform.center_uv.y,
            0.0,
        ],
    ]
}

fn texture_byte_len(size: [u32; 2]) -> Option<usize> {
    usize::try_from(size[0])
        .ok()?
        .checked_mul(usize::try_from(size[1]).ok()?)?
        .checked_mul(4)
}

fn can_retain_source(resident_bytes: usize, incoming_bytes: usize, pinned: bool) -> bool {
    pinned || resident_bytes.saturating_add(incoming_bytes) <= SOURCE_CACHE_BUDGET_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    #[test]
    fn affine_rows_map_image_center_and_corners() {
        let transform = EmbeddedImageTransform {
            center_uv: Vec2::new(0.4, 0.6),
            size_uv: Vec2::new(0.5, 0.25),
            rotation_radians: 0.0,
        };
        let rows = output_uv_to_source_uv_rows(transform);
        let map = |uv: Vec2| {
            Vec2::new(
                rows[0][0] * uv.x + rows[0][1] * uv.y + rows[0][2],
                rows[1][0] * uv.x + rows[1][1] * uv.y + rows[1][2],
            )
        };
        assert!(map(transform.center_uv).abs_diff_eq(Vec2::splat(0.5), 1.0e-6));
        assert!(map(Vec2::new(0.15, 0.475)).abs_diff_eq(Vec2::ZERO, 1.0e-6));
        assert!(map(Vec2::new(0.65, 0.725)).abs_diff_eq(Vec2::ONE, 1.0e-6));
    }

    #[test]
    fn active_preview_source_may_exceed_the_soft_cache_budget() {
        assert!(can_retain_source(0, SOURCE_CACHE_BUDGET_BYTES + 1, true));
        assert!(!can_retain_source(0, SOURCE_CACHE_BUDGET_BYTES + 1, false));
    }

    #[test]
    fn ending_preview_releases_its_source_pin() {
        let mut layers = slotmap::SlotMap::<LayerId, ()>::with_key();
        let layer_id = layers.insert(());
        let image_id = EmbeddedImageId(7);
        let registry = EmbeddedImageRegistry::default();
        registry.set_preview(
            layer_id,
            Some(EmbeddedImageTransform {
                center_uv: Vec2::splat(0.5),
                size_uv: Vec2::splat(0.25),
                rotation_radians: 0.0,
            }),
        );
        registry.pin_preview_source(layer_id, image_id);
        assert_eq!(
            registry.preview_sources.borrow().get(&layer_id),
            Some(&image_id)
        );

        registry.set_preview(layer_id, None);

        assert!(!registry.preview_sources.borrow().contains_key(&layer_id));
    }
}
