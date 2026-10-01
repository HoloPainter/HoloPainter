use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;

use crate::{
    core::render_report::{GpuTextureMetrics, RenderMetrics},
    renderer::gpu::{clear_rgba_target, frame::GpuFrame},
};

#[derive(Debug)]
struct TransientTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    spec: TransientTextureSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TransientLifetime {
    Submission,
    Operation,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TransientTextureSpec {
    tex_size: [u32; 2],
    format: wgpu::TextureFormat,
    usage_bits: u32,
}

impl TransientTextureSpec {
    pub(crate) fn new(
        tex_size: [u32; 2],
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        Self {
            tex_size: [tex_size[0].max(1), tex_size[1].max(1)],
            format,
            usage_bits: usage.bits(),
        }
    }

    fn usage(self) -> wgpu::TextureUsages {
        wgpu::TextureUsages::from_bits_retain(self.usage_bits)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TransientTextureScope {
    Viewport,
    Material(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TransientTextureKey {
    namespace: &'static str,
    scope: TransientTextureScope,
    name: String,
    lifetime: TransientLifetime,
}

impl TransientTextureKey {
    pub(crate) fn new(
        namespace: &'static str,
        scope: TransientTextureScope,
        name: impl Into<String>,
        lifetime: TransientLifetime,
    ) -> Self {
        Self {
            namespace,
            scope,
            name: name.into(),
            lifetime,
        }
    }
}

struct TransientTextureRequest<'a> {
    spec: TransientTextureSpec,
    texture_label: &'a str,
    clear_label: Option<&'a str>,
}

pub struct TransientTextures {
    active_textures: HashMap<TransientTextureKey, TransientTexture>,
    idle_textures: HashMap<TransientTextureSpec, Vec<TransientTexture>>,
    allocation_count: usize,
    reuse_count: usize,
}

impl TransientTextures {
    pub fn new(_device: &wgpu::Device) -> Self {
        Self {
            active_textures: HashMap::new(),
            idle_textures: HashMap::new(),
            allocation_count: 0,
            reuse_count: 0,
        }
    }

    pub(crate) fn ensure_transient_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: TransientTextureKey,
        tex_size: [u32; 2],
        format: wgpu::TextureFormat,
        texture_label: &str,
        clear_label: &str,
    ) {
        self.ensure_transient_texture_with_usage(
            device,
            encoder,
            key,
            tex_size,
            format,
            Self::default_scratch_usage(),
            texture_label,
            clear_label,
        );
    }

    pub(crate) fn ensure_transient_texture_without_clear(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: TransientTextureKey,
        tex_size: [u32; 2],
        format: wgpu::TextureFormat,
        texture_label: &str,
    ) {
        self.ensure_transient_texture_with_spec_optional(
            device,
            encoder,
            key,
            TransientTextureSpec::new(tex_size, format, Self::default_scratch_usage()),
            texture_label,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ensure_transient_texture_with_usage(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: TransientTextureKey,
        tex_size: [u32; 2],
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        texture_label: &str,
        clear_label: &str,
    ) {
        self.ensure_transient_texture_with_spec_optional(
            device,
            encoder,
            key,
            TransientTextureSpec::new(tex_size, format, usage),
            texture_label,
            Some(clear_label),
        );
    }

    fn ensure_transient_texture_with_spec_optional(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: TransientTextureKey,
        spec: TransientTextureSpec,
        texture_label: &str,
        clear_label: Option<&str>,
    ) {
        if self.active_texture_matches(&key, spec) {
            return;
        }
        self.active_textures.remove(&key);

        let (texture, reused) = Self::acquire_transient_texture(
            device,
            encoder,
            &mut self.idle_textures,
            TransientTextureRequest {
                spec,
                texture_label,
                clear_label,
            },
        );
        if reused {
            self.reuse_count = self.reuse_count.saturating_add(1);
        } else {
            self.allocation_count = self.allocation_count.saturating_add(1);
        }
        self.active_textures.insert(key, texture);
    }

    pub(crate) fn release_transient_textures_after_submit(
        &mut self,
        frame: &mut GpuFrame,
        scope: TransientTextureScope,
        namespace: &'static str,
        lifetime: TransientLifetime,
    ) {
        let _ = frame;
        let textures = self.drain_active_textures(|key| {
            key.scope == scope && key.namespace == namespace && key.lifetime == lifetime
        });
        Self::release_transient_textures_to_idle(&mut self.idle_textures, textures);
    }

    pub(crate) fn retain_transient_texture_until_submit(
        &mut self,
        frame: &mut GpuFrame,
        key: &TransientTextureKey,
    ) {
        self.take_transient_texture(key)
            .into_iter()
            .for_each(|texture| Self::retain_until_submit(frame, texture));
    }

    pub(crate) fn release_transient_texture_to_idle(&mut self, key: &TransientTextureKey) {
        if let Some(texture) = self.take_transient_texture(key) {
            Self::release_transient_textures_to_idle(&mut self.idle_textures, vec![texture]);
        }
    }

    pub(crate) fn transient_texture_view(
        &self,
        key: &TransientTextureKey,
    ) -> Option<&wgpu::TextureView> {
        self.active_textures.get(key).map(|texture| &texture.view)
    }

    pub(crate) fn transient_texture(&self, key: &TransientTextureKey) -> Option<&wgpu::Texture> {
        self.active_textures
            .get(key)
            .map(|texture| &texture.texture)
    }

    fn take_transient_texture(&mut self, key: &TransientTextureKey) -> Option<TransientTexture> {
        self.active_textures.remove(key)
    }

    fn retain_until_submit(frame: &mut GpuFrame, texture: TransientTexture) {
        frame.retain_texture_until_submit(texture.texture, texture.view);
    }

    fn drain_active_textures(
        &mut self,
        mut predicate: impl FnMut(&TransientTextureKey) -> bool,
    ) -> Vec<TransientTexture> {
        let keys = self
            .active_textures
            .keys()
            .filter(|key| predicate(key))
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| self.active_textures.remove(&key))
            .collect()
    }

    fn acquire_transient_texture(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        idle: &mut HashMap<TransientTextureSpec, Vec<TransientTexture>>,
        request: TransientTextureRequest<'_>,
    ) -> (TransientTexture, bool) {
        let pooled = idle.get_mut(&request.spec).and_then(|idle| idle.pop());
        let reused = pooled.is_some();
        let texture = pooled.unwrap_or_else(|| {
            Self::create_transient_texture(device, request.spec, request.texture_label)
        });
        if let Some(clear_label) = request.clear_label {
            clear_rgba_target(encoder, &texture.view, [0.0, 0.0, 0.0, 0.0], clear_label);
        }
        (texture, reused)
    }

    fn release_transient_textures_to_idle(
        idle: &mut HashMap<TransientTextureSpec, Vec<TransientTexture>>,
        textures: Vec<TransientTexture>,
    ) {
        textures.into_iter().for_each(|texture| {
            idle.entry(texture.spec).or_default().push(texture);
        });
    }

    fn active_texture_matches(
        &self,
        key: &TransientTextureKey,
        spec: TransientTextureSpec,
    ) -> bool {
        self.active_textures
            .get(key)
            .is_some_and(|texture| texture.spec == spec)
    }

    fn create_transient_texture(
        device: &wgpu::Device,
        spec: TransientTextureSpec,
        texture_label: &str,
    ) -> TransientTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(texture_label),
            size: wgpu::Extent3d {
                width: spec.tex_size[0],
                height: spec.tex_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: spec.format,
            usage: spec.usage(),
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        TransientTexture {
            texture,
            view,
            spec,
        }
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        for texture in self.active_textures.values() {
            let bytes = texture_spec_bytes(texture.spec);
            metrics.add_total_bytes(bytes);
            metrics.scratch_bytes = metrics.scratch_bytes.saturating_add(bytes);
            metrics.scratch_texture_count = metrics.scratch_texture_count.saturating_add(1);
        }
        for textures in self.idle_textures.values() {
            for texture in textures {
                let bytes = texture_spec_bytes(texture.spec);
                metrics.add_total_bytes(bytes);
                metrics.scratch_bytes = metrics.scratch_bytes.saturating_add(bytes);
                metrics.scratch_texture_count = metrics.scratch_texture_count.saturating_add(1);
            }
        }
        metrics
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        RenderMetrics {
            transient_texture_allocation_count: std::mem::take(&mut self.allocation_count),
            transient_texture_reuse_count: std::mem::take(&mut self.reuse_count),
            ..RenderMetrics::default()
        }
    }

    fn default_scratch_usage() -> wgpu::TextureUsages {
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
    }
}

fn texture_spec_bytes(spec: TransientTextureSpec) -> usize {
    (spec.tex_size[0] as usize)
        .saturating_mul(spec.tex_size[1] as usize)
        .saturating_mul(texture_format_bytes_per_pixel(spec.format))
}

fn texture_format_bytes_per_pixel(format: wgpu::TextureFormat) -> usize {
    match format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::R16Float => 2,
        wgpu::TextureFormat::R32Float
        | wgpu::TextureFormat::Rgba8Unorm
        | wgpu::TextureFormat::Rgba8UnormSrgb
        | wgpu::TextureFormat::Bgra8Unorm
        | wgpu::TextureFormat::Bgra8UnormSrgb
        | wgpu::TextureFormat::Depth32Float => 4,
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => 4,
    }
}
