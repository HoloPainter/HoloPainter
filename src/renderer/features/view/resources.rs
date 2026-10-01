use eframe::egui_wgpu::wgpu;
use glam::Mat4;

use crate::{
    core::{
        decal::{DecalPreviewGeometry, DecalProjection},
        document::MeshData,
    },
    renderer::decal_image::DecalImageKey,
};

pub(super) struct DecalTextureResources {
    sampler: wgpu::Sampler,
    bind_group: Option<CachedDecalBindGroup>,
    preview_geometry: Option<CachedDecalPreviewGeometry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DecalPreviewGeometryKey {
    mesh_generation: u64,
    projection: DecalProjection,
    viewport_view_proj: Mat4,
    viewport_size: [u32; 2],
}

struct CachedDecalPreviewGeometry {
    key: DecalPreviewGeometryKey,
    geometry: DecalPreviewGeometry,
}

struct CachedDecalBindGroup {
    key: DecalBindGroupKey,
    bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DecalBindGroupKey {
    image: DecalImageKey,
    depth: DecalDepthBindingKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DecalDepthBindingKey {
    source: DecalDepthSource,
    size: [u32; 2],
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecalDepthSource {
    Main,
    Decal,
}

impl DecalTextureResources {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("decal_image_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            sampler,
            bind_group: None,
            preview_geometry: None,
        }
    }

    pub(super) fn preview_geometry(
        &mut self,
        mesh: &MeshData,
        mesh_generation: u64,
        projection: DecalProjection,
        viewport_view_proj: Mat4,
        viewport_size: [u32; 2],
    ) -> Option<DecalPreviewGeometry> {
        let key = DecalPreviewGeometryKey {
            mesh_generation,
            projection,
            viewport_view_proj,
            viewport_size,
        };
        if self
            .preview_geometry
            .as_ref()
            .is_none_or(|cached| cached.key != key)
        {
            self.preview_geometry = Some(CachedDecalPreviewGeometry {
                key,
                geometry: DecalPreviewGeometry::build(
                    mesh,
                    projection,
                    viewport_view_proj,
                    viewport_size,
                )?,
            });
        }
        self.preview_geometry
            .as_ref()
            .map(|cached| cached.geometry.clone())
    }

    pub(super) fn ensure_bind_group<'a>(
        &'a mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        image_key: DecalImageKey,
        image_view: &wgpu::TextureView,
        projector_depth: &wgpu::TextureView,
        depth_source: DecalDepthSource,
        depth_size: [u32; 2],
        depth_generation: u64,
    ) -> &'a wgpu::BindGroup {
        let key = DecalBindGroupKey {
            image: image_key,
            depth: DecalDepthBindingKey {
                source: depth_source,
                size: depth_size,
                generation: depth_generation,
            },
        };
        if self
            .bind_group
            .as_ref()
            .is_none_or(|cached| cached.key != key)
        {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("decal_image_bg"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(image_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(projector_depth),
                    },
                ],
            });
            self.bind_group = Some(CachedDecalBindGroup { key, bind_group });
        }
        &self
            .bind_group
            .as_ref()
            .expect("Decal bind group was initialized")
            .bind_group
    }
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use crate::core::decal::{DecalProjection, DecalTransform};

    use crate::core::decal::DecalImageId;
    use crate::renderer::decal_image::DecalImageKey;

    use super::{
        DecalBindGroupKey, DecalDepthBindingKey, DecalDepthSource, DecalPreviewGeometryKey,
    };

    #[test]
    fn recreated_same_size_depth_requires_a_new_bind_group() {
        let first = DecalDepthBindingKey {
            source: DecalDepthSource::Decal,
            size: [1024, 768],
            generation: 1,
        };
        let recreated = DecalDepthBindingKey {
            source: DecalDepthSource::Decal,
            size: [1024, 768],
            generation: 2,
        };

        assert_ne!(first, recreated);
    }

    #[test]
    fn changing_decal_image_requires_a_new_bind_group() {
        let depth = DecalDepthBindingKey {
            source: DecalDepthSource::Decal,
            size: [1024, 768],
            generation: 3,
        };
        let first = DecalBindGroupKey {
            image: DecalImageKey {
                id: DecalImageId(1),
                size: [32, 16],
            },
            depth,
        };
        let changed = DecalBindGroupKey {
            image: DecalImageKey {
                id: DecalImageId(2),
                size: [32, 16],
            },
            depth,
        };

        assert_ne!(first, changed);
    }

    #[test]
    fn main_and_decal_depth_require_distinct_bind_groups() {
        let main = DecalDepthBindingKey {
            source: DecalDepthSource::Main,
            size: [1024, 768],
            generation: 0,
        };
        let decal = DecalDepthBindingKey {
            source: DecalDepthSource::Decal,
            size: [1024, 768],
            generation: 0,
        };

        assert_ne!(main, decal);
    }

    #[test]
    fn uploaded_mesh_generation_invalidates_preview_geometry() {
        let projection = DecalProjection::Surface(DecalTransform {
            center_world: glam::Vec3::ZERO,
            axis_x_world: glam::Vec3::X,
            axis_y_world: glam::Vec3::Y,
            normal_world: glam::Vec3::Z,
            size_world: glam::Vec2::ONE,
            projection_depth_world: 1.0,
        });
        let first = DecalPreviewGeometryKey {
            mesh_generation: 1,
            projection,
            viewport_view_proj: Mat4::IDENTITY,
            viewport_size: [1024, 768],
        };
        let uploaded = DecalPreviewGeometryKey {
            mesh_generation: 2,
            ..first
        };

        assert_ne!(first, uploaded);
    }
}
