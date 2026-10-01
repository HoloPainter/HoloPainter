use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::geometry::RectU32,
    renderer::gpu::{buffer::create_uniform_buffer, frame::GpuFrame},
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct TransformUniform {
    pub(super) inverse_row0: [f32; 4],
    pub(super) inverse_row1: [f32; 4],
    pub(super) source_min: [f32; 2],
    pub(super) source_max: [f32; 2],
    pub(super) selection_enabled: u32,
    pub(super) clear_original: u32,
    pub(super) _pad0: [u32; 2],
}

pub(super) struct TransformPipelines {
    uniform: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    surface_pipeline: wgpu::RenderPipeline,
    selection_pipeline: wgpu::RenderPipeline,
}

impl TransformPipelines {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uv_transform_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/uv_transform.wgsl").into(),
            ),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uv_transform_bgl"),
            entries: &[
                texture_binding(0),
                texture_binding(1),
                texture_binding(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
            label: Some("uv_transform_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let surface_pipeline = create_pipeline(
            device,
            &layout,
            &shader,
            "fs_surface",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::ColorWrites::ALL,
            "uv_transform_surface_pipeline",
        );
        let selection_pipeline = create_pipeline(
            device,
            &layout,
            &shader,
            "fs_selection",
            wgpu::TextureFormat::R8Unorm,
            wgpu::ColorWrites::RED,
            "uv_transform_selection_pipeline",
        );
        Self {
            uniform: create_uniform_buffer::<TransformUniform>(device, "uv_transform_uniform"),
            bind_group_layout,
            surface_pipeline,
            selection_pipeline,
        }
    }

    pub(super) fn write_uniform(
        &self,
        frame: &mut GpuFrame,
        device: &wgpu::Device,
        uniform: &TransformUniform,
    ) {
        frame.write_buffer_pod(device, &self.uniform, 0, uniform);
    }

    pub(super) fn bind_group(
        &self,
        device: &wgpu::Device,
        base_surface: &wgpu::TextureView,
        transform_source: &wgpu::TextureView,
        source_selection: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_transform_bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(base_surface),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(transform_source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(source_selection),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        })
    }

    pub(super) fn record_surface_pass(
        &self,
        frame: &mut GpuFrame,
        bind_group: &wgpu::BindGroup,
        target_view: &wgpu::TextureView,
        damage: RectU32,
    ) {
        record_pass(
            frame,
            &self.surface_pipeline,
            bind_group,
            target_view,
            damage,
            "uv_transform_surface_pass",
        );
    }

    pub(super) fn record_selection_pass(
        &self,
        frame: &mut GpuFrame,
        bind_group: &wgpu::BindGroup,
        target_view: &wgpu::TextureView,
        damage: RectU32,
    ) {
        record_pass(
            frame,
            &self.selection_pipeline,
            bind_group,
            target_view,
            damage,
            "uv_transform_selection_pass",
        );
    }
}

fn texture_binding(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
    write_mask: wgpu::ColorWrites,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
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
                format,
                blend: None,
                write_mask,
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

fn record_pass(
    frame: &mut GpuFrame,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target_view: &wgpu::TextureView,
    damage: RectU32,
    label: &'static str,
) {
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
    pass.set_bind_group(0, bind_group, &[]);
    pass.set_scissor_rect(
        damage.origin[0],
        damage.origin[1],
        damage.size[0],
        damage.size[1],
    );
    pass.draw(0..3, 0..1);
}
