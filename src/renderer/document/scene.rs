use std::{collections::HashMap, ops::Range};

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;
use glam::Mat4;

use crate::{
    core::{document::MeshData, stroke::SurfaceProjectionId},
    renderer::{
        engine::gpu_state::RendererGpuState,
        gpu::{
            buffer::create_uniform_buffer,
            frame::GpuFrame,
            mesh::{GpuMesh, create_gpu_mesh_from_document_mesh},
        },
        scene_capture::SceneCapture,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewProjUniform {
    view_proj: [[f32; 4]; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneDepthSlot {
    Main,
    Decal,
    SurfaceProjection(SurfaceProjectionId),
}

impl SceneDepthSlot {
    pub(crate) fn for_surface_projection(projection_id: SurfaceProjectionId) -> Self {
        match projection_id {
            SurfaceProjectionId::Primary => Self::Main,
            other => Self::SurfaceProjection(other),
        }
    }
}

pub struct SceneResources {
    mesh: Option<GpuMesh>,
    mesh_generation: u64,
    view_proj: wgpu::Buffer,
    depth_view_proj: wgpu::Buffer,
    scene_depth_cache: SceneDepthCache,
}

#[derive(Default)]
struct SceneDepthCache {
    entries: HashMap<SceneDepthSlot, SceneDepthCacheKey>,
}

#[derive(Clone, PartialEq, Eq)]
struct SceneDepthCacheKey {
    viewport_size: [u32; 2],
    view_proj_bits: [u32; 16],
    index_ranges: Option<Vec<Range<u32>>>,
}

impl SceneResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            mesh: None,
            mesh_generation: 0,
            view_proj: create_uniform_buffer::<ViewProjUniform>(device, "vp_uniform"),
            depth_view_proj: create_uniform_buffer::<ViewProjUniform>(device, "depth_vp_uniform"),
            scene_depth_cache: SceneDepthCache::default(),
        }
    }

    pub(crate) fn mesh(&self) -> Option<&GpuMesh> {
        self.mesh.as_ref()
    }

    pub(crate) fn set_material_render_settings(
        &mut self,
        material_index: usize,
        settings: crate::core::material::MaterialRenderSettings,
    ) {
        if let Some(mesh) = self.mesh.as_mut() {
            mesh.set_material_render_settings(material_index, settings);
        }
    }

    pub(crate) fn has_mesh(&self) -> bool {
        self.mesh.is_some()
    }

    pub(crate) fn mesh_generation(&self) -> u64 {
        self.mesh_generation
    }

    pub(crate) fn view_proj_uniform(&self) -> &wgpu::Buffer {
        &self.view_proj
    }

    pub(crate) fn upload_mesh(
        &mut self,
        gpu: &RendererGpuState,
        frame: &mut GpuFrame,
        mesh: &MeshData,
    ) {
        self.mesh = Some(create_gpu_mesh_from_document_mesh(
            gpu.device(),
            frame,
            mesh,
        ));
        self.mesh_generation = self.mesh_generation.wrapping_add(1);
        self.invalidate_all_depth_slots();
    }

    pub fn ensure_viewport_size(
        &mut self,
        gpu: &RendererGpuState,
        scene_capture: &mut SceneCapture,
        viewport: [u32; 2],
    ) {
        if scene_capture.viewport_size() != viewport {
            self.scene_depth_cache.remove(SceneDepthSlot::Main);
        }
        scene_capture.ensure_viewport_size(gpu.device(), viewport);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ensure_scene_depth_for_index_ranges(
        &mut self,
        gpu: &RendererGpuState,
        scene_capture: &mut SceneCapture,
        viewport_prepass_bgl: &wgpu::BindGroupLayout,
        viewport_prepass_pipeline: &crate::renderer::scene_capture::pipelines::MaterialPipelineVariants,
        frame: &mut GpuFrame,
        slot: SceneDepthSlot,
        viewport_size: [u32; 2],
        view_proj: Mat4,
        index_ranges: &[Range<u32>],
    ) {
        self.ensure_scene_depth_with_index_ranges(
            gpu,
            scene_capture,
            viewport_prepass_bgl,
            viewport_prepass_pipeline,
            frame,
            slot,
            viewport_size,
            view_proj,
            Some(index_ranges),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_scene_depth_with_index_ranges(
        &mut self,
        gpu: &RendererGpuState,
        scene_capture: &mut SceneCapture,
        viewport_prepass_bgl: &wgpu::BindGroupLayout,
        viewport_prepass_pipeline: &crate::renderer::scene_capture::pipelines::MaterialPipelineVariants,
        frame: &mut GpuFrame,
        slot: SceneDepthSlot,
        viewport_size: [u32; 2],
        view_proj: Mat4,
        index_ranges: Option<&[Range<u32>]>,
    ) {
        match slot {
            SceneDepthSlot::Main => self.ensure_viewport_size(gpu, scene_capture, viewport_size),
            _ => {
                if scene_capture.ensure_depth_slot(gpu.device(), slot, viewport_size) {
                    self.scene_depth_cache.remove(slot);
                }
            }
        }

        let key = SceneDepthCacheKey::new(viewport_size, view_proj, index_ranges);
        if self.scene_depth_cache.contains(slot, &key) {
            return;
        }

        self.write_depth_view_proj(gpu.device(), frame, view_proj);
        self.record_scene_depth_with_index_ranges(
            gpu,
            scene_capture,
            viewport_prepass_bgl,
            viewport_prepass_pipeline,
            frame.encoder(),
            slot,
            index_ranges,
        );
        self.scene_depth_cache.insert(slot, key);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_scene_depth_with_index_ranges(
        &mut self,
        gpu: &RendererGpuState,
        scene_capture: &SceneCapture,
        viewport_prepass_bgl: &wgpu::BindGroupLayout,
        viewport_prepass_pipeline: &crate::renderer::scene_capture::pipelines::MaterialPipelineVariants,
        encoder: &mut wgpu::CommandEncoder,
        slot: SceneDepthSlot,
        index_ranges: Option<&[Range<u32>]>,
    ) {
        let Some(mesh) = self.mesh.as_ref() else {
            return;
        };

        let viewport_prepass_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_prepass_bg"),
            layout: viewport_prepass_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.depth_view_proj.as_entire_binding(),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth_prepass_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scene_capture.depth_color_view(slot),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: scene_capture.depth_z_view(slot),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &viewport_prepass_bg, &[]);
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
        if let Some(index_ranges) = index_ranges {
            for sub_mesh in &mesh.sub_meshes {
                let sub_mesh_range = sub_mesh.index_range();
                for range in index_ranges {
                    let start = sub_mesh_range.start.max(range.start);
                    let end = sub_mesh_range.end.min(range.end);
                    if start < end {
                        pass.set_pipeline(
                            viewport_prepass_pipeline.select(sub_mesh.render_settings),
                        );
                        pass.draw_indexed(start..end, 0, 0..1);
                    }
                }
            }
        } else {
            for sm in &mesh.sub_meshes {
                pass.set_pipeline(viewport_prepass_pipeline.select(sm.render_settings));
                pass.draw_indexed(sm.index_start..(sm.index_start + sm.index_count), 0, 0..1);
            }
        }
    }

    pub(crate) fn invalidate_all_depth_slots(&mut self) {
        self.scene_depth_cache.clear();
    }

    fn write_depth_view_proj(&self, device: &wgpu::Device, frame: &mut GpuFrame, vp: Mat4) {
        let uniform = ViewProjUniform {
            view_proj: vp.to_cols_array_2d(),
        };
        frame.write_buffer_pod(device, &self.depth_view_proj, 0, &uniform);
    }

    pub(crate) fn write_view_proj(&self, device: &wgpu::Device, frame: &mut GpuFrame, vp: Mat4) {
        let uniform = ViewProjUniform {
            view_proj: vp.to_cols_array_2d(),
        };
        frame.write_buffer_pod(device, &self.view_proj, 0, &uniform);
    }
}

impl SceneDepthCache {
    fn contains(&self, slot: SceneDepthSlot, key: &SceneDepthCacheKey) -> bool {
        self.entries.get(&slot) == Some(key)
    }

    fn insert(&mut self, slot: SceneDepthSlot, key: SceneDepthCacheKey) {
        self.entries.insert(slot, key);
    }

    fn remove(&mut self, slot: SceneDepthSlot) {
        self.entries.remove(&slot);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

impl SceneDepthCacheKey {
    fn new(viewport_size: [u32; 2], view_proj: Mat4, index_ranges: Option<&[Range<u32>]>) -> Self {
        let mut view_proj_bits = [0; 16];
        for (dst, src) in view_proj_bits
            .iter_mut()
            .zip(view_proj.to_cols_array().into_iter())
        {
            *dst = src.to_bits();
        }
        Self {
            viewport_size,
            view_proj_bits,
            index_ranges: index_ranges.map(<[Range<u32>]>::to_vec),
        }
    }
}

pub(crate) type SceneStore = SceneResources;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_cache_keeps_primary_when_mirror_is_cached() {
        let viewport_size = [1280, 720];
        let primary_key = SceneDepthCacheKey::new(viewport_size, Mat4::IDENTITY, None);
        let mirror_key = SceneDepthCacheKey::new(
            viewport_size,
            Mat4::from_scale(glam::vec3(-1.0, 1.0, 1.0)),
            None,
        );
        let mut cache = SceneDepthCache::default();

        cache.insert(SceneDepthSlot::Main, primary_key.clone());
        cache.insert(
            SceneDepthSlot::SurfaceProjection(SurfaceProjectionId::MirrorX),
            mirror_key.clone(),
        );

        assert!(cache.contains(SceneDepthSlot::Main, &primary_key));
        assert!(cache.contains(
            SceneDepthSlot::SurfaceProjection(SurfaceProjectionId::MirrorX),
            &mirror_key,
        ));
    }

    #[test]
    fn primary_surface_projection_reuses_main_depth_slot() {
        assert_eq!(
            SceneDepthSlot::for_surface_projection(SurfaceProjectionId::Primary),
            SceneDepthSlot::Main
        );
        assert_eq!(
            SceneDepthSlot::for_surface_projection(SurfaceProjectionId::MirrorX),
            SceneDepthSlot::SurfaceProjection(SurfaceProjectionId::MirrorX)
        );
    }

    #[test]
    fn decal_depth_cache_is_independent_from_main_depth() {
        let mut cache = SceneDepthCache::default();
        let main_key = SceneDepthCacheKey::new([64, 64], Mat4::IDENTITY, None);
        let decal_key =
            SceneDepthCacheKey::new([64, 64], Mat4::from_translation(glam::Vec3::X), None);
        cache.insert(SceneDepthSlot::Main, main_key.clone());
        cache.insert(SceneDepthSlot::Decal, decal_key.clone());

        assert!(cache.contains(SceneDepthSlot::Main, &main_key));
        assert!(cache.contains(SceneDepthSlot::Decal, &decal_key));
        assert!(!cache.contains(SceneDepthSlot::Decal, &main_key));
    }

    #[test]
    fn removing_main_depth_cache_keeps_decal_depth_cache() {
        let mut cache = SceneDepthCache::default();
        let main_key = SceneDepthCacheKey::new([64, 64], Mat4::IDENTITY, None);
        let decal_key = SceneDepthCacheKey::new([128, 64], Mat4::IDENTITY, None);
        cache.insert(SceneDepthSlot::Main, main_key.clone());
        cache.insert(SceneDepthSlot::Decal, decal_key.clone());

        cache.remove(SceneDepthSlot::Main);

        assert!(!cache.contains(SceneDepthSlot::Main, &main_key));
        assert!(cache.contains(SceneDepthSlot::Decal, &decal_key));
    }

    #[test]
    fn depth_cache_distinguishes_index_range_scopes() {
        let mut cache = SceneDepthCache::default();
        let first_ranges = [0..3, 9..12];
        let second_ranges = [0..6];
        let first = SceneDepthCacheKey::new([64, 64], Mat4::IDENTITY, Some(&first_ranges));
        let second = SceneDepthCacheKey::new([64, 64], Mat4::IDENTITY, Some(&second_ranges));

        cache.insert(SceneDepthSlot::Decal, first.clone());

        assert!(cache.contains(SceneDepthSlot::Decal, &first));
        assert!(!cache.contains(SceneDepthSlot::Decal, &second));
    }
}
