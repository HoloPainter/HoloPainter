use eframe::egui_wgpu::wgpu;

const TRIANGLE_MAP_SHADER: &str = include_str!("../../../shaders/surface_blur_triangle_map.wgsl");
const ADJUSTMENT_FILTER_SHADER: &str = include_str!("../../../shaders/adjustment_filter.wgsl");
const SURFACE_BLUR_SHADER: &str = include_str!("../../../shaders/surface_blur_gather.wgsl");
const FILTER_GUTTER_SHADER: &str = include_str!("../../../shaders/filter_gutter.wgsl");
const SPATIAL_BLUR_COUNT_SHADER: &str = include_str!("../../../shaders/spatial_blur_count.wgsl");
const SPATIAL_BLUR_BUILD_SHADER: &str =
    include_str!("../../../shaders/spatial_blur_build_samples.wgsl");
const SPATIAL_BLUR_FALLBACK_SHADER: &str =
    include_str!("../../../shaders/spatial_blur_fallback_samples.wgsl");
const SPATIAL_BLUR_GATHER_SHADER: &str = include_str!("../../../shaders/spatial_blur_gather.wgsl");

pub(super) struct FilterPipelines {
    pub(super) shared: SharedFilterPipelines,
    pub(super) adjustment: AdjustmentFilterPipelines,
    pub(super) surface_blur: SurfaceBlurPipelines,
    pub(super) spatial_blur: SpatialBlurPipelines,
}

pub(super) struct SharedFilterPipelines {
    pub(super) triangle_map: wgpu::RenderPipeline,
    pub(super) gutter_bgl: wgpu::BindGroupLayout,
    pub(super) gutter: wgpu::ComputePipeline,
}

pub(super) struct AdjustmentFilterPipelines {
    pub(super) apply_bgl: wgpu::BindGroupLayout,
    pub(super) apply: wgpu::ComputePipeline,
}

pub(super) struct SurfaceBlurPipelines {
    pub(super) gather_bgl: wgpu::BindGroupLayout,
    pub(super) gather: wgpu::ComputePipeline,
}

pub(super) struct SpatialBlurPipelines {
    pub(super) count_bgl: wgpu::BindGroupLayout,
    pub(super) count: wgpu::ComputePipeline,
    pub(super) build_bgl: wgpu::BindGroupLayout,
    pub(super) build: wgpu::ComputePipeline,
    pub(super) fallback_bgl: wgpu::BindGroupLayout,
    pub(super) fallback: wgpu::ComputePipeline,
    pub(super) gather_bgl: wgpu::BindGroupLayout,
    pub(super) gather: wgpu::ComputePipeline,
}

impl FilterPipelines {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let triangle_map = create_triangle_map_pipeline(device);
        let gutter_bgl = create_gutter_bgl(device);
        let gutter =
            create_compute_pipeline(device, "filter_gutter", FILTER_GUTTER_SHADER, &gutter_bgl);
        let adjustment_apply_bgl = create_adjustment_filter_bgl(device);
        let adjustment_apply = create_compute_pipeline(
            device,
            "adjustment_filter",
            ADJUSTMENT_FILTER_SHADER,
            &adjustment_apply_bgl,
        );
        let surface_gather_bgl = create_surface_blur_gather_bgl(device);
        let surface_gather = create_compute_pipeline(
            device,
            "surface_blur_gather",
            SURFACE_BLUR_SHADER,
            &surface_gather_bgl,
        );
        let count_bgl = create_spatial_count_bgl(device);
        let count = create_compute_pipeline(
            device,
            "spatial_blur_count",
            SPATIAL_BLUR_COUNT_SHADER,
            &count_bgl,
        );
        let build_bgl = create_spatial_build_bgl(device);
        let build = create_compute_pipeline(
            device,
            "spatial_blur_build_samples",
            SPATIAL_BLUR_BUILD_SHADER,
            &build_bgl,
        );
        let fallback_bgl = create_spatial_fallback_bgl(device);
        let fallback = create_compute_pipeline(
            device,
            "spatial_blur_fallback_samples",
            SPATIAL_BLUR_FALLBACK_SHADER,
            &fallback_bgl,
        );
        let spatial_gather_bgl = create_spatial_gather_bgl(device);
        let spatial_gather = create_compute_pipeline(
            device,
            "spatial_blur_gather",
            SPATIAL_BLUR_GATHER_SHADER,
            &spatial_gather_bgl,
        );
        Self {
            shared: SharedFilterPipelines {
                triangle_map,
                gutter_bgl,
                gutter,
            },
            adjustment: AdjustmentFilterPipelines {
                apply_bgl: adjustment_apply_bgl,
                apply: adjustment_apply,
            },
            surface_blur: SurfaceBlurPipelines {
                gather_bgl: surface_gather_bgl,
                gather: surface_gather,
            },
            spatial_blur: SpatialBlurPipelines {
                count_bgl,
                count,
                build_bgl,
                build,
                fallback_bgl,
                fallback,
                gather_bgl: spatial_gather_bgl,
                gather: spatial_gather,
            },
        }
    }
}

fn create_triangle_map_pipeline(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("filter_triangle_map_shader"),
        source: wgpu::ShaderSource::Wgsl(TRIANGLE_MAP_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("filter_triangle_map_layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("filter_triangle_map_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32,
                        offset: 8,
                        shader_location: 1,
                    },
                ],
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R32Uint,
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

fn create_adjustment_filter_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("adjustment_filter_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_binding(1, wgpu::TextureSampleType::Float { filterable: false }),
            storage_texture_binding(2),
            uniform_buffer_binding(3),
            uniform_buffer_binding(4),
        ],
    })
}

fn create_surface_blur_gather_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("surface_blur_gather_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_binding(1, wgpu::TextureSampleType::Uint),
            storage_buffer_binding(2, true),
            storage_buffer_binding(3, true),
            storage_texture_binding(4),
            uniform_buffer_binding(5),
            texture_binding(6, wgpu::TextureSampleType::Float { filterable: false }),
        ],
    })
}

fn create_gutter_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("filter_gutter_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_binding(1, wgpu::TextureSampleType::Uint),
            storage_buffer_binding(2, true),
            storage_texture_binding(3),
            uniform_buffer_binding(4),
        ],
    })
}

fn create_spatial_count_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spatial_blur_count_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Uint),
            storage_buffer_binding(1, false),
            uniform_buffer_binding(2),
        ],
    })
}

fn create_spatial_build_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spatial_blur_build_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Uint),
            storage_buffer_binding(1, true),
            storage_buffer_binding(2, true),
            storage_buffer_binding(3, false),
            storage_buffer_binding(4, false),
            storage_buffer_binding(5, false),
            uniform_buffer_binding(6),
        ],
    })
}

fn create_spatial_fallback_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spatial_blur_fallback_bgl"),
        entries: &[
            storage_buffer_binding(0, true),
            storage_buffer_binding(1, true),
            storage_buffer_binding(2, true),
            storage_buffer_binding(3, false),
            storage_buffer_binding(4, false),
            storage_buffer_binding(5, false),
            uniform_buffer_binding(6),
        ],
    })
}

fn create_spatial_gather_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spatial_blur_gather_bgl"),
        entries: &[
            texture_binding(0, wgpu::TextureSampleType::Float { filterable: false }),
            texture_binding(1, wgpu::TextureSampleType::Uint),
            storage_buffer_binding(2, true),
            storage_buffer_binding(3, true),
            storage_buffer_binding(4, true),
            storage_buffer_binding(5, false),
            storage_buffer_binding(6, false),
            storage_texture_binding(7),
            uniform_buffer_binding(8),
            texture_binding(9, wgpu::TextureSampleType::Float { filterable: false }),
        ],
    })
}

fn texture_binding(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_buffer_binding(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_buffer_binding(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_texture_binding(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    source: &'static str,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}
