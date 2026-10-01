use std::cell::Cell;

use eframe::egui_wgpu::wgpu;

use crate::renderer::{adjustment_gpu::AdjustmentLutUniform, gpu::buffer::create_uniform_buffer};

use super::types::LayerCompositeUniform;

const LAYER_COMPOSITE_SHADER: &str = include_str!("../../../shaders/layer_composite.wgsl");
const LAYER_BLEND_SHADER: &str = include_str!("../../../shaders/layer_blend.wgsl");
const PASS_THROUGH_GATE_SHADER: &str = include_str!("../../../shaders/pass_through_gate.wgsl");
const CLEAR_RECT_SHADER: &str = include_str!("../../../shaders/clear_rect.wgsl");

pub(crate) struct CompositePipelines {
    pub(crate) layer_composite_bgl: wgpu::BindGroupLayout,
    pub(crate) layer_blend_bgl: wgpu::BindGroupLayout,
    pub(crate) layer_composite_uniform: wgpu::Buffer,
    pub(crate) adjustment_lut_uniform: wgpu::Buffer,
    _solid_fill_dummy_texture: wgpu::Texture,
    pub(crate) solid_fill_dummy_view: wgpu::TextureView,

    pub(crate) layer_composite_pipeline: wgpu::RenderPipeline,
    pub(crate) layer_blend_pipeline: wgpu::RenderPipeline,
    pub(crate) pass_through_gate_pipeline: wgpu::RenderPipeline,
    pub(crate) clear_rect_pipeline: wgpu::RenderPipeline,
    composite_draw_call_count: Cell<usize>,
}

impl CompositePipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layer_composite_bgl = create_layer_composite_bgl(device);
        let layer_blend_bgl = create_layer_blend_bgl(device);
        let layer_composite_uniform =
            create_uniform_buffer::<LayerCompositeUniform>(device, "layer_composite_uniform");
        let adjustment_lut_uniform =
            create_uniform_buffer::<AdjustmentLutUniform>(device, "adjustment_lut_uniform");
        let solid_fill_dummy_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("solid_fill_dummy_texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let solid_fill_dummy_view =
            solid_fill_dummy_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let layer_composite_pipeline =
            create_layer_composite_pipeline(device, &layer_composite_bgl);
        let layer_blend_pipeline = create_layer_blend_pipeline(device, &layer_blend_bgl);
        let pass_through_gate_pipeline =
            create_pass_through_gate_pipeline(device, &layer_blend_bgl);
        let clear_rect_pipeline = create_clear_rect_pipeline(device);

        Self {
            layer_composite_bgl,
            layer_blend_bgl,
            layer_composite_uniform,
            adjustment_lut_uniform,
            _solid_fill_dummy_texture: solid_fill_dummy_texture,
            solid_fill_dummy_view,
            layer_composite_pipeline,
            layer_blend_pipeline,
            pass_through_gate_pipeline,
            clear_rect_pipeline,
            composite_draw_call_count: Cell::new(0),
        }
    }

    pub(crate) fn record_composite_draw_call(&self) {
        self.composite_draw_call_count
            .set(self.composite_draw_call_count.get().saturating_add(1));
    }

    pub(crate) fn composite_draw_call_count(&self) -> usize {
        self.composite_draw_call_count.get()
    }
}

fn create_pass_through_gate_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pass_through_gate_shader"),
        source: wgpu::ShaderSource::Wgsl(PASS_THROUGH_GATE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pass_through_gate_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pass_through_gate_pipeline"),
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
    })
}

fn create_layer_composite_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("layer_composite_bgl"),
        entries: &[
            bind_tex(0, true),
            bind_sampler(1, true),
            bind_uniform(2, wgpu::ShaderStages::FRAGMENT),
            bind_tex(3, true),
        ],
    })
}

fn create_layer_blend_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("layer_blend_bgl"),
        entries: &[
            bind_tex(0, true),
            bind_tex(1, true),
            bind_sampler(2, true),
            bind_uniform(3, wgpu::ShaderStages::FRAGMENT),
            bind_tex(4, true),
            bind_uniform(5, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

fn create_layer_composite_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("layer_composite_shader"),
        source: wgpu::ShaderSource::Wgsl(LAYER_COMPOSITE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("layer_composite_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("layer_composite_pipeline"),
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
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
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

fn create_layer_blend_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("layer_blend_shader"),
        source: wgpu::ShaderSource::Wgsl(LAYER_BLEND_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("layer_blend_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("layer_blend_pipeline"),
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
    })
}

fn create_clear_rect_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("clear_rect_shader"),
        source: wgpu::ShaderSource::Wgsl(CLEAR_RECT_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("clear_rect_layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clear_rect_pipeline"),
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

fn bind_tex(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable },
        },
        count: None,
    }
}

fn bind_sampler(binding: u32, filtering: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(if filtering {
            wgpu::SamplerBindingType::Filtering
        } else {
            wgpu::SamplerBindingType::NonFiltering
        }),
        count: None,
    }
}
