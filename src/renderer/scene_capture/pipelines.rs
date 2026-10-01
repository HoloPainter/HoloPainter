use eframe::egui_wgpu::wgpu;

use crate::core::material::MaterialRenderSettings;
use crate::renderer::gpu::mesh::MeshVertex;

const SCENE_CAPTURE_MESH_SHADER: &str = include_str!("../../shaders/scene_capture_mesh.wgsl");
const VIEWPORT_WIREFRAME_SHADER: &str = include_str!("../../shaders/viewport_wireframe.wgsl");
const SCENE_CAPTURE_DEPTH_SHADER: &str = include_str!("../../shaders/scene_capture_depth.wgsl");

pub(crate) struct MaterialPipelineVariants {
    single_sided: wgpu::RenderPipeline,
    double_sided: wgpu::RenderPipeline,
}

impl MaterialPipelineVariants {
    pub(crate) fn new(
        single_sided: wgpu::RenderPipeline,
        double_sided: wgpu::RenderPipeline,
    ) -> Self {
        Self {
            single_sided,
            double_sided,
        }
    }

    pub(crate) fn select(&self, settings: MaterialRenderSettings) -> &wgpu::RenderPipeline {
        if settings.double_sided {
            &self.double_sided
        } else {
            &self.single_sided
        }
    }
}

pub(crate) struct SceneCapturePipelines {
    pub(crate) viewport_prepass_bgl: wgpu::BindGroupLayout,
    pub(crate) viewport_wireframe_bgl: wgpu::BindGroupLayout,
    pub(crate) presentation_bgl: wgpu::BindGroupLayout,
    pub(crate) surface_source_bgl: wgpu::BindGroupLayout,
    pub(crate) viewport_prepass_pipeline: MaterialPipelineVariants,
    pub(crate) presentation_pipelines: PresentationPipelines,
    pub(crate) surface_source_pipeline: MaterialPipelineVariants,
    pub(crate) viewport_wireframe_pipeline: wgpu::RenderPipeline,
}

pub(crate) struct PresentationPipelines {
    pub(crate) opaque: MaterialPipelineVariants,
    pub(crate) cutoff: MaterialPipelineVariants,
    pub(crate) blend_single_sided: wgpu::RenderPipeline,
    pub(crate) blend_double_sided_draw_back_faces: wgpu::RenderPipeline,
    pub(crate) blend_double_sided_draw_front_faces: wgpu::RenderPipeline,
}

impl SceneCapturePipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let viewport_prepass_bgl = create_viewport_prepass_bgl(device);
        let viewport_wireframe_bgl = create_viewport_wireframe_bgl(device);
        let presentation_bgl = create_material_bgl(device, "scene_capture_presentation_bgl", true);
        let surface_source_bgl =
            create_material_bgl(device, "scene_capture_surface_source_bgl", false);
        let viewport_prepass_pipeline =
            create_viewport_prepass_pipeline(device, &viewport_prepass_bgl);
        let presentation_pipelines = create_presentation_pipelines(device, &presentation_bgl);
        let surface_source_pipeline = create_surface_source_pipeline(device, &surface_source_bgl);
        let viewport_wireframe_pipeline =
            create_viewport_wireframe_pipeline(device, &viewport_wireframe_bgl);
        Self {
            viewport_prepass_bgl,
            viewport_wireframe_bgl,
            presentation_bgl,
            surface_source_bgl,
            viewport_prepass_pipeline,
            presentation_pipelines,
            surface_source_pipeline,
            viewport_wireframe_pipeline,
        }
    }
}

fn create_viewport_wireframe_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("scene_capture_wireframe_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX),
            bind_uniform(1, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

fn create_viewport_prepass_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("scene_capture_prepass_bgl"),
        entries: &[bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
    })
}

fn create_material_bgl(
    device: &wgpu::Device,
    label: &'static str,
    presentation: bool,
) -> wgpu::BindGroupLayout {
    let mut entries = vec![
        bind_uniform(0, wgpu::ShaderStages::VERTEX),
        bind_tex(1, true),
        bind_sampler(2, true),
        bind_uniform(3, wgpu::ShaderStages::FRAGMENT),
    ];
    if presentation {
        entries.push(bind_uniform(4, wgpu::ShaderStages::FRAGMENT));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

fn create_viewport_prepass_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> MaterialPipelineVariants {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene_capture_prepass_shader"),
        source: wgpu::ShaderSource::Wgsl(SCENE_CAPTURE_DEPTH_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene_capture_prepass_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    let create = |label, cull_mode| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
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
                    format: wgpu::TextureFormat::R32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::RED,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    MaterialPipelineVariants {
        single_sided: create(
            "scene_capture_prepass_single_sided_pipeline",
            Some(wgpu::Face::Back),
        ),
        double_sided: create("scene_capture_prepass_double_sided_pipeline", None),
    }
}

fn create_surface_source_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> MaterialPipelineVariants {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene_capture_surface_source_shader"),
        source: wgpu::ShaderSource::Wgsl(SCENE_CAPTURE_MESH_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene_capture_surface_source_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    let create = |label, cull_mode| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(mesh_vtx_layout_full())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_surface_source"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    MaterialPipelineVariants {
        single_sided: create(
            "scene_capture_surface_source_single_sided_pipeline",
            Some(wgpu::Face::Back),
        ),
        double_sided: create("scene_capture_surface_source_double_sided_pipeline", None),
    }
}

fn create_presentation_pipelines(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> PresentationPipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene_capture_presentation_shader"),
        source: wgpu::ShaderSource::Wgsl(SCENE_CAPTURE_MESH_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene_capture_presentation_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    let premultiplied_blend = wgpu::BlendState {
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
    };
    let create = |label, fragment_entry, cull_mode, blend, depth_write| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(mesh_vtx_layout_full())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let opaque = MaterialPipelineVariants::new(
        create(
            "scene_capture_presentation_opaque_single_sided",
            "fs_presentation_opaque",
            Some(wgpu::Face::Back),
            None,
            true,
        ),
        create(
            "scene_capture_presentation_opaque_double_sided",
            "fs_presentation_opaque",
            None,
            None,
            true,
        ),
    );
    let cutoff = MaterialPipelineVariants::new(
        create(
            "scene_capture_presentation_cutoff_single_sided",
            "fs_presentation_cutoff",
            Some(wgpu::Face::Back),
            None,
            true,
        ),
        create(
            "scene_capture_presentation_cutoff_double_sided",
            "fs_presentation_cutoff",
            None,
            None,
            true,
        ),
    );
    PresentationPipelines {
        opaque,
        cutoff,
        blend_single_sided: create(
            "scene_capture_presentation_blend_single_sided",
            "fs_presentation_blend",
            Some(wgpu::Face::Back),
            Some(premultiplied_blend),
            false,
        ),
        blend_double_sided_draw_back_faces: create(
            "scene_capture_presentation_blend_double_sided_draw_back_faces",
            "fs_presentation_blend",
            Some(wgpu::Face::Front),
            Some(premultiplied_blend),
            false,
        ),
        blend_double_sided_draw_front_faces: create(
            "scene_capture_presentation_blend_double_sided_draw_front_faces",
            "fs_presentation_blend",
            Some(wgpu::Face::Back),
            Some(premultiplied_blend),
            false,
        ),
    }
}

fn create_viewport_wireframe_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene_capture_wireframe_shader"),
        source: wgpu::ShaderSource::Wgsl(VIEWPORT_WIREFRAME_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene_capture_wireframe_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene_capture_wireframe_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(mesh_vtx_layout_position())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn mesh_vtx_layout_position() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x3,
        }],
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
