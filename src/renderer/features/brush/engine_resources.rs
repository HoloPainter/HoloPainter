use std::collections::HashMap;

use anyhow::{Result, ensure};
use eframe::egui_wgpu::wgpu;

use crate::core::{
    brush_engine::TextureResourceFormat,
    texture::{TextureCatalog, TextureResourceDefinition},
};

pub(crate) struct BrushResources {
    textures: TextureGpuStore,
}

impl BrushResources {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        catalog: &TextureCatalog,
    ) -> Result<Self> {
        Ok(Self {
            textures: TextureGpuStore::new(device, queue, catalog)?,
        })
    }

    pub(crate) fn textures(&self) -> &TextureGpuStore {
        &self.textures
    }

    pub(crate) fn insert_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &TextureResourceDefinition,
    ) -> Result<()> {
        self.textures.insert(device, queue, definition)
    }

    pub(crate) fn remove_texture(&mut self, id: &str) {
        self.textures.remove(id);
    }
}

pub(crate) struct GpuTextureResource {
    pub(crate) _texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
}

pub(crate) struct TextureGpuStore {
    textures: HashMap<String, GpuTextureResource>,
    pub(crate) solid_white: GpuTextureResource,
    pub(crate) solid_black: GpuTextureResource,
    pub(crate) nearest_clamp_sampler: wgpu::Sampler,
    pub(crate) linear_repeat_sampler: wgpu::Sampler,
    pub(crate) nearest_repeat_sampler: wgpu::Sampler,
}

impl TextureGpuStore {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        catalog: &TextureCatalog,
    ) -> Result<Self> {
        let nearest_clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture_nearest_clamp_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let linear_repeat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture_linear_repeat_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let nearest_repeat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture_nearest_repeat_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let solid_white = create_r8_texture_resource(
            device,
            queue,
            "texture_solid_white_internal",
            [1, 1],
            &[255],
            false,
        )?;
        let solid_black = create_r8_texture_resource(
            device,
            queue,
            "texture_solid_black_internal",
            [1, 1],
            &[0],
            false,
        )?;
        let mut textures = HashMap::new();
        let mut definitions = catalog.textures().collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.id.cmp(&right.id));
        for definition in definitions {
            textures.insert(
                definition.id.clone(),
                upload_texture_resource(device, queue, definition)?,
            );
        }
        Ok(Self {
            textures,
            solid_white,
            solid_black,
            nearest_clamp_sampler,
            linear_repeat_sampler,
            nearest_repeat_sampler,
        })
    }

    pub(crate) fn resolve(&self, id: &str) -> Result<&GpuTextureResource> {
        self.textures
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown texture resource {:?}", id))
    }

    pub(crate) fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &TextureResourceDefinition,
    ) -> Result<()> {
        ensure!(
            !self.textures.contains_key(&definition.id),
            "duplicate GPU texture resource {:?}",
            definition.id
        );
        let texture = upload_texture_resource(device, queue, definition)?;
        self.textures.insert(definition.id.clone(), texture);
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.textures.remove(id);
    }
}

fn upload_texture_resource(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    definition: &TextureResourceDefinition,
) -> Result<GpuTextureResource> {
    ensure!(
        definition.format == TextureResourceFormat::R8Unorm,
        "texture resource {:?} uses unsupported GPU format {:?}",
        definition.id,
        definition.format
    );
    create_r8_texture_resource(
        device,
        queue,
        &definition.id,
        definition.size,
        &definition.r8,
        definition.mipmaps,
    )
}

fn create_r8_texture_resource(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    size: [u32; 2],
    data: &[u8],
    mipmaps: bool,
) -> Result<GpuTextureResource> {
    let size = [size[0].max(1), size[1].max(1)];
    let expected_len = size[0] as usize * size[1] as usize;
    ensure!(
        data.len() == expected_len,
        "texture resource {:?} expected {} R8 bytes, got {}",
        label,
        expected_len,
        data.len()
    );
    let mips = if mipmaps {
        build_r8_mips(size, data)
    } else {
        vec![(size, data.to_vec())]
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (mip_level, (mip_size, mip_data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip_level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            mip_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip_size[0]),
                rows_per_image: Some(mip_size[1]),
            },
            wgpu::Extent3d {
                width: mip_size[0],
                height: mip_size[1],
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(GpuTextureResource {
        _texture: texture,
        view,
    })
}

fn build_r8_mips(size: [u32; 2], data: &[u8]) -> Vec<([u32; 2], Vec<u8>)> {
    let mut levels = Vec::new();
    let mut current_size = [size[0].max(1), size[1].max(1)];
    let mut current = data.to_vec();
    levels.push((current_size, current.clone()));

    while current_size[0] > 1 || current_size[1] > 1 {
        // WebGPU mip dimensions use floor division. For example, a 27x27
        // texture has five levels: 27, 13, 6, 3, 1.
        let next_size = [(current_size[0] / 2).max(1), (current_size[1] / 2).max(1)];
        let mut next = vec![0; (next_size[0] * next_size[1]) as usize];

        for y in 0..next_size[1] {
            let sy_start = y * current_size[1] / next_size[1];
            let sy_end = (y + 1) * current_size[1] / next_size[1];

            for x in 0..next_size[0] {
                let sx_start = x * current_size[0] / next_size[0];
                let sx_end = (x + 1) * current_size[0] / next_size[0];
                let mut sum = 0u64;
                let mut count = 0u64;

                for sy in sy_start..sy_end {
                    for sx in sx_start..sx_end {
                        let index = (sy * current_size[0] + sx) as usize;
                        sum += current[index] as u64;
                        count += 1;
                    }
                }

                let dst = (y * next_size[0] + x) as usize;
                next[dst] = ((sum + count / 2) / count) as u8;
            }
        }

        current_size = next_size;
        current = next;
        levels.push((current_size, current.clone()));
    }

    levels
}
