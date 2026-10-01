use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        damage::UV_ISLAND_BLEED_RADIUS_PX,
        geometry::RectU32,
        render_report::{GpuTextureMetrics, RenderMetrics},
    },
    renderer::{
        document::scene::SceneResources,
        engine::gpu_state::RendererGpuState,
        gpu::{
            clear_rgba_target, copy_rects_a_to_b, create_viewport_stroke_target, frame::GpuFrame,
            mesh::MeshVertex,
        },
    },
};

use super::resources::{
    EDIT_MASK_BLEED_WORKGROUP_SIZE, EditMaskBoundsGpu, SurfaceEditResources, UvIslandBleedParams,
};

const UV_ISLAND_COVERAGE_SHADER: &str = include_str!("../../shaders/uv_island_coverage.wgsl");
const EDIT_MASK_CLEAR_SHADER: &str = include_str!("../../shaders/edit_mask_clear.wgsl");
const EDIT_MASK_BOUNDS_SHADER: &str = include_str!("../../shaders/edit_mask_bounds.wgsl");
const EDIT_MASK_DISPATCH_SHADER: &str = include_str!("../../shaders/edit_mask_dispatch.wgsl");
const UV_ISLAND_BLEED_SHADER: &str = include_str!("../../shaders/uv_island_bleed.wgsl");

pub(crate) struct SurfaceEditTarget<'a> {
    pub(crate) material_index: usize,
    pub(crate) texture_size: [u32; 2],
    pub(crate) write_texture: &'a wgpu::Texture,
    pub(crate) write_view: &'a wgpu::TextureView,
    pub(crate) read_texture: &'a wgpu::Texture,
    pub(crate) read_view: &'a wgpu::TextureView,
}

pub(crate) struct EditMask<'a> {
    pub(crate) view: &'a wgpu::TextureView,
}

pub(crate) struct SurfaceEditPipelines {
    pub(crate) island_mask_bgl: wgpu::BindGroupLayout,
    pub(crate) edit_mask_bounds_bgl: wgpu::BindGroupLayout,
    pub(crate) edit_mask_dispatch_bgl: wgpu::BindGroupLayout,
    pub(crate) uv_island_bleed_bgl: wgpu::BindGroupLayout,

    pub(crate) island_mask_pipeline: wgpu::RenderPipeline,
    pub(crate) edit_mask_clear_pipeline: wgpu::RenderPipeline,
    pub(crate) edit_mask_bounds_pipeline: wgpu::ComputePipeline,
    pub(crate) edit_mask_dispatch_pipeline: wgpu::ComputePipeline,
    pub(crate) uv_island_bleed_pipeline: wgpu::ComputePipeline,
}

impl SurfaceEditPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let island_mask_bgl = create_island_mask_bgl(device);
        let edit_mask_bounds_bgl = create_edit_mask_bounds_bgl(device);
        let edit_mask_dispatch_bgl = create_edit_mask_dispatch_bgl(device);
        let uv_island_bleed_bgl = create_uv_island_bleed_bgl(device);

        let island_mask_pipeline = create_island_mask_pipeline(device, &island_mask_bgl);
        let edit_mask_clear_pipeline = create_edit_mask_clear_pipeline(device);
        let edit_mask_bounds_pipeline =
            create_edit_mask_bounds_pipeline(device, &edit_mask_bounds_bgl);
        let edit_mask_dispatch_pipeline =
            create_edit_mask_dispatch_pipeline(device, &edit_mask_dispatch_bgl);
        let uv_island_bleed_pipeline =
            create_uv_island_bleed_pipeline(device, &uv_island_bleed_bgl);

        Self {
            island_mask_bgl,
            edit_mask_bounds_bgl,
            edit_mask_dispatch_bgl,
            uv_island_bleed_bgl,
            island_mask_pipeline,
            edit_mask_clear_pipeline,
            edit_mask_bounds_pipeline,
            edit_mask_dispatch_pipeline,
            uv_island_bleed_pipeline,
        }
    }

    pub(crate) fn clear_edit_mask_rects(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        edit_mask_view: &wgpu::TextureView,
        texture_size: [u32; 2],
        rects: &[RectU32],
        label: &str,
    ) {
        if rects.is_empty() {
            return;
        }
        debug_assert!(rects_match_texture(rects, texture_size));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: edit_mask_view,
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
        pass.set_pipeline(&self.edit_mask_clear_pipeline);
        for rect in rects {
            pass.set_scissor_rect(rect.origin[0], rect.origin[1], rect.size[0], rect.size[1]);
            pass.draw(0..3, 0..1);
        }
    }
}

struct IslandMaskResources {
    tex_size: [u32; 2],
    _island_mask: wgpu::Texture,
    island_mask_view: wgpu::TextureView,
}

#[derive(Default)]
pub(crate) struct UvIslandBleed {
    island_masks: HashMap<usize, IslandMaskResources>,
    generation_count: usize,
}

impl UvIslandBleed {
    pub(crate) fn clear(&mut self) {
        self.island_masks.clear();
    }

    pub(crate) fn retain_material_count(&mut self, material_count: usize) {
        self.island_masks
            .retain(|material_index, _| *material_index < material_count);
    }

    pub(crate) fn prewarm_materials(
        &mut self,
        gpu: &RendererGpuState,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        encoder: &mut wgpu::CommandEncoder,
        material_sizes: impl IntoIterator<Item = (usize, [u32; 2])>,
    ) {
        for (material_index, texture_size) in material_sizes {
            self.ensure_island_mask_for_material(
                gpu,
                scene,
                pipelines,
                encoder,
                material_index,
                texture_size,
            );
        }
    }

    pub(crate) fn take_metrics(&mut self) -> RenderMetrics {
        RenderMetrics {
            uv_island_mask_generation_count: std::mem::take(&mut self.generation_count),
            ..RenderMetrics::default()
        }
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        for mask in self.island_masks.values() {
            let bytes = (mask.tex_size[0] as usize)
                .saturating_mul(mask.tex_size[1] as usize)
                .saturating_mul(4);
            metrics.add_total_bytes(bytes);
            metrics.scratch_bytes = metrics.scratch_bytes.saturating_add(bytes);
            metrics.scratch_texture_count = metrics.scratch_texture_count.saturating_add(1);
        }
        metrics
    }

    pub(crate) fn record(
        &mut self,
        gpu: &RendererGpuState,
        resources: &SurfaceEditResources,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        frame: &mut GpuFrame,
        target: SurfaceEditTarget<'_>,
        edit_mask: EditMask<'_>,
    ) {
        let full_rect = RectU32::full(target.texture_size);
        self.record_regions(
            gpu,
            resources,
            scene,
            pipelines,
            frame,
            target,
            edit_mask,
            std::slice::from_ref(&full_rect),
            std::slice::from_ref(&full_rect),
        );
    }

    pub(crate) fn record_bounded(
        &mut self,
        gpu: &RendererGpuState,
        resources: &SurfaceEditResources,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        frame: &mut GpuFrame,
        target: SurfaceEditTarget<'_>,
        edit_mask: EditMask<'_>,
        scan_rects: &[RectU32],
        damage_rects: &[RectU32],
    ) {
        if scan_rects.is_empty() || damage_rects.is_empty() {
            return;
        }
        debug_assert!(rects_match_texture(scan_rects, target.texture_size));
        debug_assert!(rects_match_texture(damage_rects, target.texture_size));
        self.record_regions(
            gpu,
            resources,
            scene,
            pipelines,
            frame,
            target,
            edit_mask,
            scan_rects,
            damage_rects,
        );
    }

    fn record_regions(
        &mut self,
        gpu: &RendererGpuState,
        resources: &SurfaceEditResources,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        frame: &mut GpuFrame,
        target: SurfaceEditTarget<'_>,
        edit_mask: EditMask<'_>,
        scan_rects: &[RectU32],
        damage_rects: &[RectU32],
    ) {
        if !self.ensure_island_mask_for_material(
            gpu,
            scene,
            pipelines,
            frame.encoder(),
            target.material_index,
            target.texture_size,
        ) {
            copy_rects_a_to_b(
                frame.encoder(),
                target.write_texture,
                target.read_texture,
                target.texture_size,
                damage_rects,
            );
            return;
        }

        let Some(mask) = self.island_masks.get(&target.material_index) else {
            copy_rects_a_to_b(
                frame.encoder(),
                target.write_texture,
                target.read_texture,
                target.texture_size,
                damage_rects,
            );
            return;
        };
        Self::record_for_textures(
            gpu,
            resources,
            pipelines,
            frame,
            target,
            &mask.island_mask_view,
            edit_mask.view,
            scan_rects,
            damage_rects,
        );
    }

    fn record_for_textures(
        gpu: &RendererGpuState,
        resources: &SurfaceEditResources,
        pipelines: &SurfaceEditPipelines,
        frame: &mut GpuFrame,
        target: SurfaceEditTarget<'_>,
        island_mask_view: &wgpu::TextureView,
        edit_mask_view: &wgpu::TextureView,
        scan_rects: &[RectU32],
        damage_rects: &[RectU32],
    ) {
        let bounds_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edit_mask_bounds_bg"),
            layout: &pipelines.edit_mask_bounds_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(edit_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: resources.bounds_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: resources.bleed_uniform().as_entire_binding(),
                },
            ],
        });
        let dispatch_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edit_mask_dispatch_bg"),
            layout: &pipelines.edit_mask_dispatch_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: resources.bounds_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: resources.dispatch_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: resources.bleed_uniform().as_entire_binding(),
                },
            ],
        });
        let bleed_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_island_bleed_bg"),
            layout: &pipelines.uv_island_bleed_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(target.write_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(edit_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(island_mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(target.read_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: resources.bounds_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: resources.bleed_uniform().as_entire_binding(),
                },
            ],
        });

        copy_rects_a_to_b(
            frame.encoder(),
            target.write_texture,
            target.read_texture,
            target.texture_size,
            damage_rects,
        );
        for scan_rect in scan_rects {
            let bleed_params = UvIslandBleedParams {
                tex_size: target.texture_size,
                dilation_radius: UV_ISLAND_BLEED_RADIUS_PX,
                _pad0: 0,
                scan_origin: scan_rect.origin,
                scan_size: scan_rect.size,
            };
            frame.write_buffer_pod(gpu.device(), resources.bleed_uniform(), 0, &bleed_params);
            frame.encoder().copy_buffer_to_buffer(
                resources.bounds_reset_buffer(),
                0,
                resources.bounds_buffer(),
                0,
                std::mem::size_of::<EditMaskBoundsGpu>() as u64,
            );
            frame.encoder().copy_buffer_to_buffer(
                resources.dispatch_reset_buffer(),
                0,
                resources.dispatch_buffer(),
                0,
                std::mem::size_of::<[u32; 3]>() as u64,
            );
            {
                let mut pass = frame
                    .encoder()
                    .begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("edit_mask_bounds_pass"),
                        timestamp_writes: None,
                    });
                pass.set_pipeline(&pipelines.edit_mask_bounds_pipeline);
                pass.set_bind_group(0, &bounds_bg, &[]);
                let gx = scan_rect.size[0].div_ceil(EDIT_MASK_BLEED_WORKGROUP_SIZE);
                let gy = scan_rect.size[1].div_ceil(EDIT_MASK_BLEED_WORKGROUP_SIZE);
                pass.dispatch_workgroups(gx, gy, 1);
            }
            {
                let mut pass = frame
                    .encoder()
                    .begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("edit_mask_dispatch_prepare_pass"),
                        timestamp_writes: None,
                    });
                pass.set_pipeline(&pipelines.edit_mask_dispatch_pipeline);
                pass.set_bind_group(0, &dispatch_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            {
                let mut pass = frame
                    .encoder()
                    .begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("uv_island_bleed_pass"),
                        timestamp_writes: None,
                    });
                pass.set_pipeline(&pipelines.uv_island_bleed_pipeline);
                pass.set_bind_group(0, &bleed_bg, &[]);
                pass.dispatch_workgroups_indirect(resources.dispatch_buffer(), 0);
            }
        }
        copy_rects_a_to_b(
            frame.encoder(),
            target.read_texture,
            target.write_texture,
            target.texture_size,
            damage_rects,
        );
        copy_rects_a_to_b(
            frame.encoder(),
            target.write_texture,
            target.read_texture,
            target.texture_size,
            damage_rects,
        );
        pipelines.clear_edit_mask_rects(
            frame.encoder(),
            edit_mask_view,
            target.texture_size,
            scan_rects,
            "clear_edit_mask_after_uv_island_bleed",
        );
    }

    fn ensure_island_mask_for_material(
        &mut self,
        gpu: &RendererGpuState,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        encoder: &mut wgpu::CommandEncoder,
        material_index: usize,
        tex_size: [u32; 2],
    ) -> bool {
        if !scene.has_mesh() {
            return false;
        }

        let needs_recreate = self
            .island_masks
            .get(&material_index)
            .map(|r| r.tex_size != tex_size)
            .unwrap_or(true);
        if needs_recreate {
            let (island_mask, island_mask_view) =
                create_viewport_stroke_target(gpu.device(), tex_size, "uv_island_coverage_tex");
            clear_rgba_target(
                encoder,
                &island_mask_view,
                [0.0, 0.0, 0.0, 0.0],
                "clear_uv_island_coverage",
            );

            self.island_masks.insert(
                material_index,
                IslandMaskResources {
                    tex_size,
                    _island_mask: island_mask,
                    island_mask_view,
                },
            );
            self.record_island_mask(gpu, scene, pipelines, encoder, material_index);
            self.generation_count = self.generation_count.saturating_add(1);
        }

        true
    }

    fn record_island_mask(
        &self,
        gpu: &RendererGpuState,
        scene: &SceneResources,
        pipelines: &SurfaceEditPipelines,
        encoder: &mut wgpu::CommandEncoder,
        material_index: usize,
    ) {
        let Some(mesh) = scene.mesh() else {
            return;
        };
        let Some(mask) = self.island_masks.get(&material_index) else {
            return;
        };

        let mask_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_island_coverage_bg"),
            layout: &pipelines.island_mask_bgl,
            entries: &[],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("uv_island_coverage_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &mask.island_mask_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipelines.island_mask_pipeline);
        pass.set_bind_group(0, &mask_bg, &[]);
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
        for sm in &mesh.sub_meshes {
            if sm.material_index == material_index {
                pass.draw_indexed(sm.index_start..(sm.index_start + sm.index_count), 0, 0..1);
            }
        }
    }
}

fn mesh_vtx_layout_full() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 20,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x3,
            },
        ],
    }
}

fn rects_match_texture(rects: &[RectU32], texture_size: [u32; 2]) -> bool {
    rects.iter().all(|rect| {
        rect.size[0] > 0
            && rect.size[1] > 0
            && rect.origin[0].saturating_add(rect.size[0]) <= texture_size[0]
            && rect.origin[1].saturating_add(rect.size[1]) <= texture_size[1]
    })
}

fn create_island_mask_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uv_island_coverage_bgl"),
        entries: &[],
    })
}

fn create_edit_mask_bounds_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("edit_mask_bounds_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
            bind_storage_buffer(1, wgpu::ShaderStages::COMPUTE, false),
            bind_uniform(2, wgpu::ShaderStages::COMPUTE),
        ],
    })
}

fn create_edit_mask_dispatch_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("edit_mask_dispatch_bgl"),
        entries: &[
            bind_storage_buffer(0, wgpu::ShaderStages::COMPUTE, false),
            bind_storage_buffer(1, wgpu::ShaderStages::COMPUTE, false),
            bind_uniform(2, wgpu::ShaderStages::COMPUTE),
        ],
    })
}

fn create_uv_island_bleed_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uv_island_bleed_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
            bind_storage_texture_write(3, wgpu::TextureFormat::Rgba8Unorm),
            bind_storage_buffer(4, wgpu::ShaderStages::COMPUTE, true),
            bind_uniform(5, wgpu::ShaderStages::COMPUTE),
        ],
    })
}

fn create_island_mask_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uv_island_coverage_shader"),
        source: wgpu::ShaderSource::Wgsl(UV_ISLAND_COVERAGE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("uv_island_coverage_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("uv_island_coverage_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(mesh_vtx_layout_full())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::RED,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_edit_mask_clear_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("edit_mask_clear_shader"),
        source: wgpu::ShaderSource::Wgsl(EDIT_MASK_CLEAR_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("edit_mask_clear_layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("edit_mask_clear_pipeline"),
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
                format: wgpu::TextureFormat::R8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::RED,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_edit_mask_bounds_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("edit_mask_bounds_shader"),
        source: wgpu::ShaderSource::Wgsl(EDIT_MASK_BOUNDS_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("edit_mask_bounds_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("edit_mask_bounds_pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn create_edit_mask_dispatch_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("edit_mask_dispatch_shader"),
        source: wgpu::ShaderSource::Wgsl(EDIT_MASK_DISPATCH_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("edit_mask_dispatch_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("edit_mask_dispatch_pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn create_uv_island_bleed_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uv_island_bleed_shader"),
        source: wgpu::ShaderSource::Wgsl(UV_ISLAND_BLEED_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("uv_island_bleed_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("uv_island_bleed_pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

fn bind_uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bind_storage_buffer(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bind_storage_texture_write(
    binding: u32,
    format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{
        engine::readback::blocking_read_texture_rect_rgba8_with_device_queue,
        gpu::{
            buffer::create_initialized_buffer,
            frame::GpuFrame,
            texture::{create_mask_texture, create_paint_texture},
        },
    };

    #[test]
    fn bounded_regions_must_stay_inside_their_texture() {
        let texture_size = [128, 64];
        assert!(rects_match_texture(
            &[RectU32 {
                origin: [8, 4],
                size: [32, 16],
            }],
            texture_size,
        ));
        assert!(!rects_match_texture(
            &[RectU32 {
                origin: [120, 4],
                size: [16, 16],
            }],
            texture_size,
        ));
        assert!(!rects_match_texture(
            &[RectU32 {
                origin: [0, 0],
                size: [0, 8],
            }],
            texture_size,
        ));
    }

    #[test]
    fn uv_island_bleed_extends_outward_without_tangential_edit_dilation() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
            .ok()
        else {
            eprintln!("skipping GPU regression test: no wgpu adapter available");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("skipping GPU regression test: no wgpu device available");
            return;
        };

        let texture_size = [48, 48];
        let boundary_x = 20_u32;
        let edited_y = 20_u32;
        let corner_source = [32_u32, 32_u32];
        let (base_texture, base_view) =
            create_paint_texture(&device, texture_size, "uv_bleed_test_base");
        let (stroke_texture, stroke_view) =
            create_mask_texture(&device, texture_size, "uv_bleed_test_stroke");
        let (island_texture, island_view) =
            create_mask_texture(&device, texture_size, "uv_bleed_test_island");
        let (destination_texture, destination_view) =
            create_paint_texture(&device, texture_size, "uv_bleed_test_destination");

        let pixel_count = (texture_size[0] * texture_size[1]) as usize;
        let mut base = vec![0_u8; pixel_count * 4];
        let mut stroke = vec![0_u8; pixel_count];
        let mut island = vec![0_u8; pixel_count];
        for y in 0..texture_size[1] {
            for x in 0..=boundary_x {
                island[(y * texture_size[0] + x) as usize] = 255;
            }
        }
        let edited_index = (edited_y * texture_size[0] + boundary_x) as usize;
        base[edited_index * 4..edited_index * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        stroke[edited_index] = 255;
        let corner_source_index = (corner_source[1] * texture_size[0] + corner_source[0]) as usize;
        island[corner_source_index] = 255;
        base[corner_source_index * 4..corner_source_index * 4 + 4]
            .copy_from_slice(&[255, 0, 0, 255]);
        stroke[corner_source_index] = 255;

        let params = UvIslandBleedParams {
            tex_size: texture_size,
            dilation_radius: UV_ISLAND_BLEED_RADIUS_PX,
            _pad0: 0,
            scan_origin: [0, 0],
            scan_size: texture_size,
        };
        let bounds = EditMaskBoundsGpu {
            min_xy: [
                boundary_x - UV_ISLAND_BLEED_RADIUS_PX,
                edited_y - UV_ISLAND_BLEED_RADIUS_PX,
            ],
            max_xy: [
                corner_source[0] + UV_ISLAND_BLEED_RADIUS_PX,
                corner_source[1] + UV_ISLAND_BLEED_RADIUS_PX,
            ],
            stroke_count: 2,
            _pad0: [0; 3],
        };
        let params_buffer = create_initialized_buffer(
            &device,
            "uv_bleed_test_params",
            wgpu::BufferUsages::UNIFORM,
            std::slice::from_ref(&params),
        );
        let bounds_buffer = create_initialized_buffer(
            &device,
            "uv_bleed_test_bounds",
            wgpu::BufferUsages::STORAGE,
            std::slice::from_ref(&bounds),
        );
        let bind_group_layout = create_uv_island_bleed_bgl(&device);
        let pipeline = create_uv_island_bleed_pipeline(&device, &bind_group_layout);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_bleed_test_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&stroke_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&island_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&destination_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: bounds_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut frame = GpuFrame::new(&device, "uv_bleed_nearest_island_regression");
        frame.write_texture_rgba8(&device, &base_texture, [0, 0], texture_size, &base);
        frame.write_texture_r8(&device, &stroke_texture, [0, 0], texture_size, &stroke);
        frame.write_texture_r8(&device, &island_texture, [0, 0], texture_size, &island);
        frame.write_texture_rgba8(
            &device,
            &destination_texture,
            [0, 0],
            texture_size,
            &vec![0_u8; pixel_count * 4],
        );
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("uv_bleed_nearest_island_regression_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let extent_x = bounds.max_xy[0] - bounds.min_xy[0] + 1;
            let extent_y = bounds.max_xy[1] - bounds.min_xy[1] + 1;
            pass.dispatch_workgroups(
                extent_x.div_ceil(EDIT_MASK_BLEED_WORKGROUP_SIZE),
                extent_y.div_ceil(EDIT_MASK_BLEED_WORKGROUP_SIZE),
                1,
            );
        }
        frame.finish().submit(&queue);

        let output = blocking_read_texture_rect_rgba8_with_device_queue(
            &device,
            &queue,
            &destination_texture,
            texture_size,
            [0, 0],
            texture_size,
            "uv_bleed_nearest_island_regression_readback",
        )
        .expect("UV bleed regression texture should be readable");
        let pixel = |x: u32, y: u32| {
            let offset = ((y * texture_size[0] + x) * 4) as usize;
            <[u8; 4]>::try_from(&output.rgba8[offset..offset + 4]).unwrap()
        };

        assert_eq!(pixel(boundary_x, edited_y), [255, 0, 0, 255]);
        assert_eq!(pixel(boundary_x + 1, edited_y), [255, 0, 0, 255]);
        assert_eq!(
            pixel(boundary_x + UV_ISLAND_BLEED_RADIUS_PX, edited_y),
            [255, 0, 0, 255]
        );
        assert_eq!(
            pixel(boundary_x + 1, edited_y + 1),
            [0, 0, 0, 0],
            "an unedited nearest boundary texel must block tangential dilation"
        );
        assert_eq!(
            pixel(
                corner_source[0] + UV_ISLAND_BLEED_RADIUS_PX,
                corner_source[1] + UV_ISLAND_BLEED_RADIUS_PX,
            ),
            [0, 0, 0, 0],
            "the square search corner lies outside the Euclidean radius"
        );
    }
}
