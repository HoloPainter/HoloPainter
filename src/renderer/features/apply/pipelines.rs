use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    features::apply::types::{DecalApplyUniform, UvCompositeUniform},
    gpu::{buffer::create_uniform_buffer, mesh::MeshVertex},
};

const MASKED_APPLY_COMPOSITE_SHADER: &str =
    include_str!("../../../shaders/masked_apply_composite.wgsl");
const VIEWPORT_RECT_PROJECTION_MASK_SHADER: &str =
    include_str!("../../../shaders/viewport_rect_projection_mask.wgsl");
const VIEWPORT_POLYGON_PROJECTION_MASK_SHADER: &str =
    include_str!("../../../shaders/viewport_polygon_projection_mask.wgsl");
const DECAL_APPLY_SHADER: &str = include_str!("../../../shaders/decal_apply.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct ViewportRectProjectionMaskUniform {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) rect_min_max: [f32; 4],
    pub(crate) viewport_texture_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct ViewportPolygonProjectionMaskUniform {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) viewport_texture_size: [f32; 4],
}

/// Pipelines used to blend generated paint/apply stroke textures back into a
/// material surface. This is owned by paint/apply features instead of the
/// material compositing feature so one-shot apply and stroke finalization do
/// not depend on `CompositeFeature` internals.
pub(crate) struct PaintApplyPipelines {
    pub(crate) masked_apply_composite_uniform: wgpu::Buffer,
    pub(crate) masked_apply_composite_bgl: wgpu::BindGroupLayout,
    pub(crate) masked_apply_composite_pipeline: wgpu::RenderPipeline,
    pub(crate) viewport_rect_projection_mask_uniform: wgpu::Buffer,
    pub(crate) viewport_rect_projection_mask_bgl: wgpu::BindGroupLayout,
    pub(crate) viewport_rect_projection_mask_pipeline: wgpu::RenderPipeline,
    pub(crate) viewport_polygon_projection_mask_uniform: wgpu::Buffer,
    pub(crate) viewport_polygon_projection_mask_bgl: wgpu::BindGroupLayout,
    pub(crate) viewport_polygon_projection_mask_pipeline: wgpu::RenderPipeline,
    pub(crate) decal_apply_uniform: wgpu::Buffer,
    pub(crate) decal_apply_bgl: wgpu::BindGroupLayout,
    pub(crate) decal_apply_pipeline: wgpu::RenderPipeline,
}

impl PaintApplyPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let masked_apply_composite_uniform =
            create_uniform_buffer::<UvCompositeUniform>(device, "masked_apply_composite_uniform");
        let masked_apply_composite_bgl = create_masked_apply_composite_bgl(device);
        let masked_apply_composite_pipeline =
            create_masked_apply_composite_pipeline(device, &masked_apply_composite_bgl);
        let viewport_rect_projection_mask_uniform =
            create_uniform_buffer::<ViewportRectProjectionMaskUniform>(
                device,
                "viewport_rect_projection_mask_uniform",
            );
        let viewport_rect_projection_mask_bgl = create_viewport_rect_projection_mask_bgl(device);
        let viewport_rect_projection_mask_pipeline = create_viewport_rect_projection_mask_pipeline(
            device,
            &viewport_rect_projection_mask_bgl,
        );
        let viewport_polygon_projection_mask_uniform =
            create_uniform_buffer::<ViewportPolygonProjectionMaskUniform>(
                device,
                "viewport_polygon_projection_mask_uniform",
            );
        let viewport_polygon_projection_mask_bgl =
            create_viewport_polygon_projection_mask_bgl(device);
        let viewport_polygon_projection_mask_pipeline =
            create_viewport_polygon_projection_mask_pipeline(
                device,
                &viewport_polygon_projection_mask_bgl,
            );
        let decal_apply_uniform =
            create_uniform_buffer::<DecalApplyUniform>(device, "decal_apply_uniform");
        let decal_apply_bgl = create_decal_apply_bgl(device);
        let decal_apply_pipeline = create_decal_apply_pipeline(device, &decal_apply_bgl);

        Self {
            masked_apply_composite_uniform,
            masked_apply_composite_bgl,
            masked_apply_composite_pipeline,
            viewport_rect_projection_mask_uniform,
            viewport_rect_projection_mask_bgl,
            viewport_rect_projection_mask_pipeline,
            viewport_polygon_projection_mask_uniform,
            viewport_polygon_projection_mask_bgl,
            viewport_polygon_projection_mask_pipeline,
            decal_apply_uniform,
            decal_apply_bgl,
            decal_apply_pipeline,
        }
    }
}

fn create_decal_apply_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("decal_apply_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            bind_tex(1, true),
            bind_sampler(2, true),
            bind_tex(3, false),
            bind_tex(4, true),
            bind_tex(5, true),
        ],
    })
}

fn create_decal_apply_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("decal_apply_shader"),
        source: wgpu::ShaderSource::Wgsl(DECAL_APPLY_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("decal_apply_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("decal_apply_pipeline"),
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
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R8Unorm,
                    blend: Some(projection_mask_union_blend_state()),
                    write_mask: wgpu::ColorWrites::RED,
                }),
            ],
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

fn create_masked_apply_composite_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("masked_apply_composite_bgl"),
        entries: &[
            bind_tex(0, true),
            bind_sampler(1, true),
            bind_tex(2, true),
            bind_uniform(3, wgpu::ShaderStages::FRAGMENT),
            bind_tex(4, true),
        ],
    })
}

fn create_viewport_rect_projection_mask_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport_rect_projection_mask_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            bind_tex(1, false),
        ],
    })
}

fn create_viewport_rect_projection_mask_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport_rect_projection_mask_shader"),
        source: wgpu::ShaderSource::Wgsl(VIEWPORT_RECT_PROJECTION_MASK_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport_rect_projection_mask_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport_rect_projection_mask_pipeline"),
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
                blend: Some(projection_mask_union_blend_state()),
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

fn create_viewport_polygon_projection_mask_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport_polygon_projection_mask_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            bind_tex(1, false),
            bind_tex(2, false),
        ],
    })
}

fn create_viewport_polygon_projection_mask_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport_polygon_projection_mask_shader"),
        source: wgpu::ShaderSource::Wgsl(VIEWPORT_POLYGON_PROJECTION_MASK_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport_polygon_projection_mask_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport_polygon_projection_mask_pipeline"),
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
                blend: Some(projection_mask_union_blend_state()),
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

fn projection_mask_union_blend_state() -> wgpu::BlendState {
    let component = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    };
    wgpu::BlendState {
        color: component,
        alpha: component,
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

fn create_masked_apply_composite_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("masked_apply_composite_shader"),
        source: wgpu::ShaderSource::Wgsl(MASKED_APPLY_COMPOSITE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("masked_apply_composite_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("masked_apply_composite_pipeline"),
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
