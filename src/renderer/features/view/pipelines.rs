use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    features::view::types::{
        DecalOverlayUniform, MirrorPlaneOverlayUniform, SurfaceBrushOverlayUniform,
        UvBrushOverlayUniform, UvSelectionOverlayUniform, UvViewTransformUniform, WireframeUniform,
    },
    gpu::{buffer::create_uniform_buffer, mesh::MeshVertex},
    scene_capture::pipelines::MaterialPipelineVariants,
};

const UV_VIEW_PRESENT_SHADER: &str = include_str!("../../../shaders/uv_view_present.wgsl");
const UV_WIREFRAME_SHADER: &str = include_str!("../../../shaders/uv_wireframe.wgsl");
const BRUSH_OVERLAY_VIEWPORT_COMPOSITE_SHADER: &str =
    include_str!("../../../shaders/brush_overlay_viewport_composite.wgsl");
const BRUSH_OVERLAY_UV_VIEW_SHADER: &str =
    include_str!("../../../shaders/brush_overlay_uv_view.wgsl");
const BRUSH_OVERLAY_SURFACE_VIEW_SHADER: &str =
    include_str!("../../../shaders/brush_overlay_surface_view.wgsl");
const UV_SELECTION_OVERLAY_SHADER: &str =
    include_str!("../../../shaders/uv_selection_overlay.wgsl");
const VIEWPORT_SELECTION_OVERLAY_SHADER: &str =
    include_str!("../../../shaders/viewport_selection_overlay.wgsl");
const MIRROR_PLANE_OVERLAY_SHADER: &str =
    include_str!("../../../shaders/mirror_plane_overlay.wgsl");
const DECAL_IMAGE_OVERLAY_SHADER: &str = include_str!("../../../shaders/decal_image_overlay.wgsl");

pub(crate) struct ViewPipelines {
    pub(crate) uv_view_transform_uniform: wgpu::Buffer,
    pub(crate) uv_wireframe_uniform: wgpu::Buffer,
    pub(crate) uv_wireframe_bg: wgpu::BindGroup,
    pub(crate) uv_view_present_bgl: wgpu::BindGroupLayout,
    pub(crate) uv_selection_overlay_uniform: wgpu::Buffer,
    pub(crate) brush_overlay_uv_view_uniform: wgpu::Buffer,
    pub(crate) brush_overlay_uv_view_bg: wgpu::BindGroup,
    pub(crate) mirror_plane_overlay_uniform: wgpu::Buffer,
    pub(crate) mirror_plane_overlay_bg: wgpu::BindGroup,
    pub(crate) decal_overlay_uniform: wgpu::Buffer,
    pub(crate) decal_overlay_bgl: wgpu::BindGroupLayout,
    pub(crate) uv_selection_overlay_bgl: wgpu::BindGroupLayout,
    pub(crate) brush_overlay_surface_view_uniform: wgpu::Buffer,
    pub(crate) brush_overlay_surface_view_bg: wgpu::BindGroup,

    pub(crate) brush_overlay_viewport_composite_bgl: wgpu::BindGroupLayout,
    pub(crate) viewport_selection_overlay_bgl: wgpu::BindGroupLayout,

    pub(crate) brush_overlay_viewport_composite_pipeline: wgpu::RenderPipeline,
    pub(crate) viewport_selection_overlay_pipeline: MaterialPipelineVariants,
    pub(crate) mirror_plane_face_pipeline: wgpu::RenderPipeline,
    pub(crate) mirror_plane_line_visible_pipeline: wgpu::RenderPipeline,
    pub(crate) mirror_plane_line_occluded_pipeline: wgpu::RenderPipeline,
    pub(crate) decal_overlay_pipeline: MaterialPipelineVariants,
    pub(crate) uv_view_present_pipeline: wgpu::RenderPipeline,
    pub(crate) uv_wireframe_pipeline: wgpu::RenderPipeline,
    pub(crate) brush_overlay_uv_view_pipeline: wgpu::RenderPipeline,
    pub(crate) uv_selection_overlay_pipeline: wgpu::RenderPipeline,
    pub(crate) brush_overlay_surface_view_pipeline: wgpu::RenderPipeline,
}

impl ViewPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let uv_view_transform_uniform =
            create_uniform_buffer::<UvViewTransformUniform>(device, "uv_view_transform_uniform");
        let uv_wireframe_uniform =
            create_uniform_buffer::<WireframeUniform>(device, "uv_wireframe_uniform");
        let uv_selection_overlay_uniform = create_uniform_buffer::<UvSelectionOverlayUniform>(
            device,
            "uv_selection_overlay_uniform",
        );
        let brush_overlay_uv_view_uniform =
            create_uniform_buffer::<UvBrushOverlayUniform>(device, "brush_overlay_uv_view_uniform");
        let brush_overlay_surface_view_uniform = create_uniform_buffer::<SurfaceBrushOverlayUniform>(
            device,
            "brush_overlay_surface_view_uniform",
        );
        let mirror_plane_overlay_uniform = create_uniform_buffer::<MirrorPlaneOverlayUniform>(
            device,
            "mirror_plane_overlay_uniform",
        );
        let decal_overlay_uniform =
            create_uniform_buffer::<DecalOverlayUniform>(device, "decal_overlay_uniform");
        let uv_wireframe_bgl = create_uv_wireframe_bgl(device);
        let uv_view_present_bgl = create_uv_view_present_bgl(device);
        let brush_overlay_uv_view_bgl = create_brush_overlay_uv_view_bgl(device);
        let uv_selection_overlay_bgl = create_uv_selection_overlay_bgl(device);
        let brush_overlay_surface_view_bgl = create_brush_overlay_surface_view_bgl(device);
        let brush_overlay_viewport_composite_bgl =
            create_brush_overlay_viewport_composite_bgl(device);
        let viewport_selection_overlay_bgl = create_viewport_selection_overlay_bgl(device);
        let mirror_plane_overlay_bgl = create_mirror_plane_overlay_bgl(device);
        let decal_overlay_bgl = create_decal_overlay_bgl(device);

        let uv_wireframe_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_wireframe_bg"),
            layout: &uv_wireframe_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uv_view_transform_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uv_wireframe_uniform.as_entire_binding(),
                },
            ],
        });
        let brush_overlay_uv_view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush_overlay_uv_view_bg"),
            layout: &brush_overlay_uv_view_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: brush_overlay_uv_view_uniform.as_entire_binding(),
            }],
        });
        let mirror_plane_overlay_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mirror_plane_overlay_bg"),
            layout: &mirror_plane_overlay_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: mirror_plane_overlay_uniform.as_entire_binding(),
            }],
        });
        let brush_overlay_surface_view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush_overlay_surface_view_bg"),
            layout: &brush_overlay_surface_view_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: brush_overlay_surface_view_uniform.as_entire_binding(),
            }],
        });

        let brush_overlay_viewport_composite_pipeline =
            create_brush_overlay_viewport_composite_pipeline(
                device,
                &brush_overlay_viewport_composite_bgl,
            );
        let viewport_selection_overlay_pipeline =
            create_viewport_selection_overlay_pipeline(device, &viewport_selection_overlay_bgl);
        let (
            mirror_plane_face_pipeline,
            mirror_plane_line_visible_pipeline,
            mirror_plane_line_occluded_pipeline,
        ) = create_mirror_plane_overlay_pipelines(device, &mirror_plane_overlay_bgl);
        let decal_overlay_pipeline = create_decal_overlay_pipeline(device, &decal_overlay_bgl);
        let uv_view_present_pipeline =
            create_uv_view_present_pipeline(device, &uv_view_present_bgl);
        let uv_wireframe_pipeline = create_uv_wireframe_pipeline(device, &uv_wireframe_bgl);
        let brush_overlay_uv_view_pipeline =
            create_brush_overlay_uv_view_pipeline(device, &brush_overlay_uv_view_bgl);
        let uv_selection_overlay_pipeline =
            create_uv_selection_overlay_pipeline(device, &uv_selection_overlay_bgl);
        let brush_overlay_surface_view_pipeline =
            create_brush_overlay_surface_view_pipeline(device, &brush_overlay_surface_view_bgl);

        Self {
            uv_view_transform_uniform,
            uv_wireframe_uniform,
            uv_wireframe_bg,
            uv_view_present_bgl,
            uv_selection_overlay_uniform,
            brush_overlay_uv_view_uniform,
            brush_overlay_uv_view_bg,
            mirror_plane_overlay_uniform,
            mirror_plane_overlay_bg,
            decal_overlay_uniform,
            decal_overlay_bgl,
            uv_selection_overlay_bgl,
            brush_overlay_surface_view_uniform,
            brush_overlay_surface_view_bg,
            brush_overlay_viewport_composite_bgl,
            viewport_selection_overlay_bgl,
            brush_overlay_viewport_composite_pipeline,
            viewport_selection_overlay_pipeline,
            mirror_plane_face_pipeline,
            mirror_plane_line_visible_pipeline,
            mirror_plane_line_occluded_pipeline,
            decal_overlay_pipeline,
            uv_view_present_pipeline,
            uv_wireframe_pipeline,
            brush_overlay_uv_view_pipeline,
            uv_selection_overlay_pipeline,
            brush_overlay_surface_view_pipeline,
        }
    }
}

fn create_decal_overlay_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("decal_overlay_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            bind_tex(1, true),
            bind_sampler(2, true),
            bind_tex_nonfilter(3),
        ],
    })
}

fn create_mirror_plane_overlay_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mirror_plane_overlay_bgl"),
        entries: &[bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
    })
}

fn create_brush_overlay_viewport_composite_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("brush_overlay_viewport_composite_bgl"),
        entries: &[bind_tex(0, true), bind_sampler(1, true)],
    })
}

fn create_uv_wireframe_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uv_wireframe_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX),
            bind_uniform(1, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

fn create_uv_view_present_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uv_view_present_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::FRAGMENT),
            bind_tex(1, true),
            bind_sampler(2, true),
        ],
    })
}

fn create_brush_overlay_uv_view_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("brush_overlay_uv_view_bgl"),
        entries: &[bind_uniform(0, wgpu::ShaderStages::FRAGMENT)],
    })
}

fn create_uv_selection_overlay_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uv_selection_overlay_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::FRAGMENT),
            bind_tex_nonfilter(1),
        ],
    })
}

fn create_brush_overlay_surface_view_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("brush_overlay_surface_view_bgl"),
        entries: &[bind_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
    })
}

fn create_viewport_selection_overlay_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport_selection_overlay_bgl"),
        entries: &[
            bind_uniform(0, wgpu::ShaderStages::VERTEX),
            bind_tex_nonfilter(1),
            bind_uniform(2, wgpu::ShaderStages::VERTEX_FRAGMENT),
        ],
    })
}

fn create_mirror_plane_overlay_pipelines(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("mirror_plane_overlay_shader"),
        source: wgpu::ShaderSource::Wgsl(MIRROR_PLANE_OVERLAY_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("mirror_plane_overlay_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    let create_pipeline = |label: &'static str,
                           vertex_entry: &'static str,
                           fragment_entry: &'static str,
                           topology: wgpu::PrimitiveTopology,
                           depth_compare: wgpu::CompareFunction,
                           depth_bias: wgpu::DepthBiasState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vertex_entry),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fragment_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend_alpha()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(depth_compare),
                stencil: wgpu::StencilState::default(),
                bias: depth_bias,
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    (
        create_pipeline(
            "mirror_plane_face_pipeline",
            "vs_face",
            "fs_face",
            wgpu::PrimitiveTopology::TriangleStrip,
            wgpu::CompareFunction::LessEqual,
            wgpu::DepthBiasState {
                constant: -1,
                slope_scale: 0.0,
                clamp: 0.0,
            },
        ),
        create_pipeline(
            "mirror_plane_line_visible_pipeline",
            "vs_line",
            "fs_line_visible",
            wgpu::PrimitiveTopology::LineList,
            wgpu::CompareFunction::LessEqual,
            wgpu::DepthBiasState::default(),
        ),
        create_pipeline(
            "mirror_plane_line_occluded_pipeline",
            "vs_line",
            "fs_line_occluded",
            wgpu::PrimitiveTopology::LineList,
            wgpu::CompareFunction::Always,
            wgpu::DepthBiasState::default(),
        ),
    )
}

fn create_brush_overlay_viewport_composite_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("brush_overlay_viewport_composite_shader"),
        source: wgpu::ShaderSource::Wgsl(BRUSH_OVERLAY_VIEWPORT_COMPOSITE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("brush_overlay_viewport_composite_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("brush_overlay_viewport_composite_pipeline"),
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
                blend: Some(blend_alpha()),
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

fn create_brush_overlay_surface_view_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("brush_overlay_surface_view_shader"),
        source: wgpu::ShaderSource::Wgsl(BRUSH_OVERLAY_SURFACE_VIEW_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("brush_overlay_surface_view_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("brush_overlay_surface_view_pipeline"),
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
                blend: Some(stroke_mask_blend()),
                write_mask: wgpu::ColorWrites::RED,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
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

fn create_decal_overlay_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> MaterialPipelineVariants {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("decal_image_overlay_shader"),
        source: wgpu::ShaderSource::Wgsl(DECAL_IMAGE_OVERLAY_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("decal_overlay_layout"),
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
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend_premultiplied_alpha()),
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
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                // The preview redraws the same mesh against the viewport depth buffer.
                // A small negative slope bias prevents floating-point self-occlusion on
                // grazing faces while preserving occlusion by genuinely nearer surfaces.
                bias: wgpu::DepthBiasState {
                    constant: -1,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    MaterialPipelineVariants::new(
        create(
            "decal_overlay_single_sided_pipeline",
            Some(wgpu::Face::Back),
        ),
        create("decal_overlay_double_sided_pipeline", None),
    )
}

fn create_viewport_selection_overlay_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> MaterialPipelineVariants {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport_selection_overlay_shader"),
        source: wgpu::ShaderSource::Wgsl(VIEWPORT_SELECTION_OVERLAY_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport_selection_overlay_layout"),
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
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend_alpha()),
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
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    MaterialPipelineVariants::new(
        create(
            "viewport_selection_overlay_single_sided_pipeline",
            Some(wgpu::Face::Back),
        ),
        create("viewport_selection_overlay_double_sided_pipeline", None),
    )
}

fn create_uv_view_present_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uv_view_present_shader"),
        source: wgpu::ShaderSource::Wgsl(UV_VIEW_PRESENT_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("uv_view_present_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("uv_view_present_pipeline"),
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

fn create_uv_wireframe_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uv_wireframe_shader"),
        source: wgpu::ShaderSource::Wgsl(UV_WIREFRAME_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("uv_wireframe_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("uv_wireframe_pipeline"),
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
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(blend_alpha()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_brush_overlay_uv_view_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("brush_overlay_uv_view_shader"),
        source: wgpu::ShaderSource::Wgsl(BRUSH_OVERLAY_UV_VIEW_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("brush_overlay_uv_view_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("brush_overlay_uv_view_pipeline"),
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
                blend: Some(blend_alpha()),
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

fn create_uv_selection_overlay_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uv_selection_overlay_shader"),
        source: wgpu::ShaderSource::Wgsl(UV_SELECTION_OVERLAY_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("uv_selection_overlay_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("uv_selection_overlay_pipeline"),
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
                blend: Some(blend_alpha()),
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

fn blend_alpha() -> wgpu::BlendState {
    wgpu::BlendState::ALPHA_BLENDING
}

fn blend_premultiplied_alpha() -> wgpu::BlendState {
    wgpu::BlendState {
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
    }
}

fn stroke_mask_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
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

fn bind_tex_nonfilter(binding: u32) -> wgpu::BindGroupLayoutEntry {
    bind_tex(binding, false)
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
