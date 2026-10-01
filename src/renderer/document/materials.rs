use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::material::MaterialRenderSettings,
    renderer::{MaterialRegistration, MaterialUpload as SceneMaterialUpload, gpu::frame::GpuFrame},
};

#[derive(Debug, Clone)]
pub struct MaterialUpload {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    pub render_settings: MaterialRenderSettings,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentMaterialUploads {
    pub materials: Vec<MaterialUpload>,
}

impl DocumentMaterialUploads {
    pub(crate) fn from_uploads(uploads: Vec<SceneMaterialUpload>) -> Self {
        let materials = uploads
            .into_iter()
            .map(|upload| MaterialUpload {
                width: upload.texture.width,
                height: upload.texture.height,
                rgba8: upload.texture.rgba8,
                render_settings: upload.render_settings,
            })
            .collect();
        Self { materials }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MaterialPresentationUniform {
    pub(crate) alpha_cutoff: f32,
    _padding: [f32; 3],
}

impl From<MaterialRenderSettings> for MaterialPresentationUniform {
    fn from(settings: MaterialRenderSettings) -> Self {
        Self {
            alpha_cutoff: f32::from(settings.alpha_cutoff) / 255.0,
            _padding: [0.0; 3],
        }
    }
}

#[derive(Debug)]
pub(crate) struct MaterialRecord {
    pub(crate) texture_size: [u32; 2],
    pub(crate) render_settings: MaterialRenderSettings,
    pub(crate) presentation_uniform: wgpu::Buffer,
}

#[derive(Debug, Default)]
pub(crate) struct MaterialRegistry {
    materials: Vec<MaterialRecord>,
}

impl MaterialRegistry {
    pub(crate) fn replace_from_uploads(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        uploads: &DocumentMaterialUploads,
    ) {
        self.materials = uploads
            .materials
            .iter()
            .map(|material| MaterialRecord {
                texture_size: [material.width, material.height],
                render_settings: material.render_settings,
                presentation_uniform: create_presentation_uniform(
                    device,
                    frame,
                    material.render_settings,
                ),
            })
            .collect();
    }

    pub(crate) fn replace_with_sizes<I>(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        sizes: I,
    ) where
        I: IntoIterator<Item = [u32; 2]>,
    {
        self.materials = sizes
            .into_iter()
            .map(|size| MaterialRecord {
                texture_size: [size[0].max(1), size[1].max(1)],
                render_settings: MaterialRenderSettings::default(),
                presentation_uniform: create_presentation_uniform(
                    device,
                    frame,
                    MaterialRenderSettings::default(),
                ),
            })
            .collect();
    }

    pub(crate) fn append(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        materials: &[MaterialRegistration],
    ) {
        self.materials
            .extend(materials.iter().map(|material| MaterialRecord {
                texture_size: [
                    material.texture_size[0].max(1),
                    material.texture_size[1].max(1),
                ],
                render_settings: material.render_settings,
                presentation_uniform: create_presentation_uniform(
                    device,
                    frame,
                    material.render_settings,
                ),
            }));
    }

    pub(crate) fn material_count(&self) -> usize {
        self.materials.len()
    }

    pub(crate) fn get(&self, material_index: usize) -> Option<&MaterialRecord> {
        self.materials.get(material_index)
    }

    pub(crate) fn texture_size(&self, material_index: usize) -> Option<[u32; 2]> {
        self.get(material_index)
            .map(|material| material.texture_size)
    }

    pub(crate) fn render_settings(&self, material_index: usize) -> Option<MaterialRenderSettings> {
        self.get(material_index)
            .map(|material| material.render_settings)
    }

    pub(crate) fn presentation_uniform(&self, material_index: usize) -> Option<&wgpu::Buffer> {
        self.get(material_index)
            .map(|material| &material.presentation_uniform)
    }

    pub(crate) fn set_texture_size(
        &mut self,
        material_index: usize,
        texture_size: [u32; 2],
    ) -> Result<bool> {
        if texture_size[0] == 0 || texture_size[1] == 0 {
            return Err(anyhow!("material texture size must be > 0"));
        }
        let material = self
            .materials
            .get_mut(material_index)
            .ok_or_else(|| anyhow!("material index {material_index} is out of range"))?;
        if material.texture_size == texture_size {
            return Ok(false);
        }
        material.texture_size = texture_size;
        Ok(true)
    }

    pub(crate) fn set_render_settings(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        material_index: usize,
        settings: MaterialRenderSettings,
    ) -> Result<bool> {
        let material = self
            .materials
            .get_mut(material_index)
            .ok_or_else(|| anyhow!("material index {material_index} is out of range"))?;
        if material.render_settings == settings {
            return Ok(false);
        }
        material.render_settings = settings;
        frame.write_buffer_pod(
            device,
            &material.presentation_uniform,
            0,
            &MaterialPresentationUniform::from(settings),
        );
        Ok(true)
    }

    pub(crate) fn texture_size_usize(&self, material_index: usize) -> [usize; 2] {
        let size = self.materials[material_index].texture_size;
        [size[0] as usize, size[1] as usize]
    }
}

fn create_presentation_uniform(
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    settings: MaterialRenderSettings,
) -> wgpu::Buffer {
    frame.create_buffer_from_slice(
        device,
        "material_presentation_uniform",
        wgpu::BufferUsages::UNIFORM,
        &[MaterialPresentationUniform::from(settings)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_presentation_uniform_normalizes_u8_cutoff() {
        let settings = MaterialRenderSettings {
            alpha_cutoff: 64,
            ..MaterialRenderSettings::default()
        };
        assert!(
            (MaterialPresentationUniform::from(settings).alpha_cutoff - 64.0 / 255.0).abs() < 1e-6
        );
    }
}
