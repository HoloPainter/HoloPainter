use std::collections::HashMap;

use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::{
    core::brush_engine::{
        BrushEngineDefinition, BrushEngineRegistry, BrushPassDomain, BrushResourceDefinition,
        BrushSpace, BuiltinPassInput, ParamDynamicsBinding, ParamType, ParamValue, PassBinding,
        PassBlend, PassClearValue, PassOutputLoad, PipelineDefinition, RasterMode, SamplerKind,
        SurfaceTextureSource, SurfaceTextureSync, TextureResourceExtent, TextureResourceFormat,
        TextureResourceLifetime, TextureSampleType, TextureViewDimension, UniformKind,
        resource_format,
    },
    renderer::{
        features::brush::types::BrushDabInstance,
        gpu::{buffer::create_uniform_buffer, mesh::MeshVertex},
    },
};

use super::engine_types::{BakeUniform, BrushEngineUniform};

pub(crate) struct BrushEnginePipelines {
    pub(crate) brush_uniform: wgpu::Buffer,
    pub(crate) bake_uniform: wgpu::Buffer,
    runtimes: HashMap<String, BrushEngineRuntime>,
}

pub(crate) struct BrushEngineRuntime {
    param_layout: BrushParamLayout,
    resources: Vec<CompiledBrushResource>,
    uv: Vec<BrushPassRuntime>,
    surface_viewport: Vec<BrushPassRuntime>,
    surface: Vec<BrushPassRuntime>,
    uv_usage: BrushSpaceResourceUsage,
    surface_viewport_usage: BrushSpaceResourceUsage,
    surface_usage: BrushSpaceResourceUsage,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BrushParamLayout {
    fields: Vec<BrushParamField>,
    dynamic_fields: Vec<BrushParamField>,
    resource_fields: Vec<BrushResourceParamField>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BrushParamField {
    pub(crate) name: String,
    pub(crate) ty: ParamType,
    pub(crate) default: ParamValue,
    pub(crate) slot: BrushParamSlot,
    pub(crate) dynamics: Vec<ParamDynamicsBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BrushResourceParamField {
    pub(crate) name: String,
    pub(crate) default: ParamValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrushParamSlot {
    F32(usize),
    U32(usize),
    PerDabF32(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompiledBrushResource {
    pub(crate) name: String,
    pub(crate) definition: BrushResourceDefinition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompiledResourceRef(usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompiledPassBinding {
    SampledTexture {
        resource: CompiledResourceRef,
        sample_type: TextureSampleType,
        view_dimension: TextureViewDimension,
    },
    Builtin(BuiltinPassInput),
    Sampler(SamplerKind),
    Uniform(UniformKind),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompiledPassAttachment {
    pub(crate) name: String,
    pub(crate) resource: CompiledResourceRef,
    pub(crate) load: PassOutputLoad,
    pub(crate) clear_value: Option<PassClearValue>,
    pub(crate) blend: PassBlend,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BrushScratchResource {
    pub(crate) name: String,
    pub(crate) format: TextureResourceFormat,
    pub(crate) extent: TextureResourceExtent,
    pub(crate) lifetime: TextureResourceLifetime,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BrushSpaceResourceUsage {
    pub(crate) needs_stroke_snapshot: bool,
    pub(crate) needs_batch_source: bool,
    pub(crate) needs_current_source: bool,
    pub(crate) scratch_resources: Vec<BrushScratchResource>,
    pub(crate) needs_viewport_source_color: bool,
    pub(crate) needs_viewport_depth: bool,
    pub(crate) reads_current_dab_batch: bool,
    pub(crate) uses_target_mesh_uv: bool,
    pub(crate) uses_uv_dab_quad_instances: bool,
    pub(crate) uses_viewport_dab_quad_instances: bool,
    pub(crate) reads_selection_mask: bool,
}

impl BrushSpaceResourceUsage {
    fn from_passes(passes: &[BrushPassRuntime]) -> Self {
        let mut scratch_resources = passes
            .iter()
            .flat_map(|pass| pass.scratch_resources.iter().cloned())
            .collect::<Vec<_>>();
        scratch_resources.sort_by(|left, right| {
            scratch_resource_sort_key(left).cmp(&scratch_resource_sort_key(right))
        });
        scratch_resources.dedup_by(|left, right| {
            scratch_resource_sort_key(left) == scratch_resource_sort_key(right)
        });
        Self {
            needs_stroke_snapshot: passes.iter().any(|pass| pass.reads_stroke_snapshot),
            needs_batch_source: passes.iter().any(|pass| pass.reads_batch_source),
            needs_current_source: passes.iter().any(|pass| pass.reads_current_source),
            scratch_resources,
            needs_viewport_source_color: passes.iter().any(|pass| pass.reads_viewport_source_color),
            needs_viewport_depth: passes.iter().any(|pass| pass.reads_viewport_depth),
            reads_current_dab_batch: passes.iter().any(|pass| pass.reads_current_dab_batch),
            uses_target_mesh_uv: passes
                .iter()
                .any(|pass| pass.raster == RasterMode::TargetMeshUv),
            uses_uv_dab_quad_instances: passes.iter().any(|pass| pass.uses_uv_dab_quad_instances),
            uses_viewport_dab_quad_instances: passes
                .iter()
                .any(|pass| pass.uses_viewport_dab_quad_instances),
            reads_selection_mask: passes.iter().any(|pass| pass.reads_selection_mask),
        }
    }

    pub(crate) fn needs_material_source_texture(&self) -> bool {
        // StrokeSurfaceRecordTarget uses the material source texture as its
        // read side. CurrentSource therefore needs the same backing texture as
        // StrokeSnapshot, even though its contents are refreshed before each
        // stamp instead of captured only at stroke begin.
        self.needs_stroke_snapshot || self.needs_current_source
    }

    pub(crate) fn needs_surface_dabs(&self) -> bool {
        self.reads_current_dab_batch
    }

    pub(crate) fn needs_surface_viewport_inputs(&self) -> bool {
        self.needs_viewport_source_color || self.needs_viewport_depth
    }

    pub(crate) fn requires_scene_mesh(&self) -> bool {
        // TargetMeshUv rasterization binds the uploaded mesh directly. Viewport
        // source/depth inputs are generated by scene capture, which also needs
        // the scene mesh. Other UV texture-space passes can run without one.
        self.uses_target_mesh_uv || self.needs_surface_viewport_inputs()
    }
}

pub(crate) struct BrushPassRuntime {
    pub(crate) name: String,
    pub(crate) raster: RasterMode,
    pub(crate) inputs: Vec<PassInputRuntime>,
    pub(crate) outputs: Vec<CompiledPassAttachment>,
    pub(crate) bind_group_layout: wgpu::BindGroupLayout,
    scratch_resources: Vec<BrushScratchResource>,
    reads_stroke_snapshot: bool,
    writes_canvas: bool,
    pub(crate) reads_batch_source: bool,
    pub(crate) reads_current_source: bool,
    reads_viewport_source_color: bool,
    reads_viewport_depth: bool,
    pub(crate) reads_current_dab_batch: bool,
    pub(crate) uses_uv_dab_quad_instances: bool,
    pub(crate) uses_viewport_dab_quad_instances: bool,
    reads_selection_mask: bool,
    pub(crate) resource_params: Vec<BrushResourceParamField>,
    pub(crate) pipeline: wgpu::RenderPipeline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PassInputRuntime {
    pub(crate) name: String,
    pub(crate) binding: CompiledPassBinding,
    pub(crate) binding_index: u32,
}

impl BrushEnginePipelines {
    pub(crate) fn new(device: &wgpu::Device, registry: &BrushEngineRegistry) -> Result<Self> {
        let brush_uniform = create_uniform_buffer::<BrushEngineUniform>(device, "brush_uniform");
        let bake_uniform = create_uniform_buffer::<BakeUniform>(device, "brush_bake_uniform");
        let mut runtimes = HashMap::new();
        for engine in registry.engines() {
            runtimes.insert(engine.id.clone(), BrushEngineRuntime::new(device, engine)?);
        }

        Ok(Self {
            brush_uniform,
            bake_uniform,
            runtimes,
        })
    }

    pub(crate) fn runtime(&self, engine_id: &str) -> Option<&BrushEngineRuntime> {
        self.runtimes.get(engine_id)
    }

    pub(crate) fn register_engine(
        &mut self,
        device: &wgpu::Device,
        engine: &BrushEngineDefinition,
    ) -> Result<()> {
        let runtime = BrushEngineRuntime::new(device, engine)?;
        self.runtimes.insert(engine.id.clone(), runtime);
        Ok(())
    }

    pub(crate) fn unregister_engine(&mut self, engine_id: &str) {
        self.runtimes.remove(engine_id);
    }
}

impl BrushEngineRuntime {
    fn new(device: &wgpu::Device, engine: &BrushEngineDefinition) -> Result<Self> {
        let param_layout = BrushParamLayout::from_engine(engine);
        let resources = compile_resources(engine);
        let resource_lookup = resource_lookup(&resources);
        let uv = compile_passes(
            device,
            engine,
            &param_layout,
            &resources,
            &resource_lookup,
            BrushSpace::Uv,
            BrushPassDomain::UvTexture,
        )?;
        let surface_viewport = compile_passes(
            device,
            engine,
            &param_layout,
            &resources,
            &resource_lookup,
            BrushSpace::Surface,
            BrushPassDomain::Viewport,
        )?;
        let surface = compile_passes(
            device,
            engine,
            &param_layout,
            &resources,
            &resource_lookup,
            BrushSpace::Surface,
            BrushPassDomain::SurfaceProjection,
        )?;
        let uv_usage = BrushSpaceResourceUsage::from_passes(&uv);
        let surface_viewport_usage = BrushSpaceResourceUsage::from_passes(&surface_viewport);
        let surface_usage = BrushSpaceResourceUsage::from_passes(&surface);
        Ok(Self {
            param_layout,
            resources,
            uv,
            surface_viewport,
            surface,
            uv_usage,
            surface_viewport_usage,
            surface_usage,
        })
    }

    pub(crate) fn param_layout(&self) -> &BrushParamLayout {
        &self.param_layout
    }

    pub(crate) fn resources(&self) -> &[CompiledBrushResource] {
        &self.resources
    }

    pub(crate) fn uv_passes(&self) -> &[BrushPassRuntime] {
        &self.uv
    }

    pub(crate) fn surface_viewport_passes(&self) -> &[BrushPassRuntime] {
        &self.surface_viewport
    }

    pub(crate) fn surface_passes(&self) -> &[BrushPassRuntime] {
        &self.surface
    }

    pub(crate) fn uv_usage(&self) -> &BrushSpaceResourceUsage {
        &self.uv_usage
    }

    pub(crate) fn surface_viewport_usage(&self) -> &BrushSpaceResourceUsage {
        &self.surface_viewport_usage
    }

    pub(crate) fn surface_usage(&self) -> &BrushSpaceResourceUsage {
        &self.surface_usage
    }

    pub(crate) fn surface_dilation_mask(&self) -> Option<BrushScratchResource> {
        self.surface
            .iter()
            .flat_map(|pass| pass.outputs.iter())
            .find_map(|output| {
                let resource = brush_scratch_resource(&self.resources, output.resource)?;
                let is_stroke_mask = resource.format == TextureResourceFormat::R8Unorm
                    && matches!(resource.extent, TextureResourceExtent::PaintSurface)
                    && resource.lifetime == TextureResourceLifetime::Stroke
                    && output.load == PassOutputLoad::ClearOnStrokeBegin;
                is_stroke_mask.then_some(resource)
            })
    }
}

fn compile_passes(
    device: &wgpu::Device,
    engine: &BrushEngineDefinition,
    param_layout: &BrushParamLayout,
    resources: &[CompiledBrushResource],
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
    space: BrushSpace,
    domain: BrushPassDomain,
) -> Result<Vec<BrushPassRuntime>> {
    engine
        .passes
        .get(&space)
        .into_iter()
        .flatten()
        .filter(|instance| instance.domain == domain)
        .map(|instance| {
            let pipeline = engine
                .pipelines
                .get(&instance.pipeline)
                .expect("validated pipeline exists");
            BrushPassRuntime::new(
                device,
                &engine.id,
                param_layout,
                resources,
                resource_lookup,
                &instance.pipeline,
                pipeline,
            )
        })
        .collect()
}

impl BrushParamLayout {
    fn from_engine(engine: &BrushEngineDefinition) -> Self {
        let mut next_f32 = 0usize;
        let mut next_u32 = 0usize;
        let mut fields = Vec::new();
        let mut dynamic_fields = Vec::new();
        let mut resource_fields = Vec::new();
        let bindings = engine.param_dynamics_bindings();
        for param in &engine.params {
            match &param.ty {
                ParamType::F32 => {
                    let dynamics = bindings
                        .iter()
                        .filter(|binding| binding.target == param.name)
                        .cloned()
                        .collect::<Vec<_>>();
                    if dynamics.is_empty() {
                        let slot = BrushParamSlot::F32(next_f32);
                        next_f32 += 1;
                        fields.push(BrushParamField {
                            name: param.name.clone(),
                            ty: param.ty.clone(),
                            default: param.default.clone(),
                            slot,
                            dynamics,
                        });
                    } else {
                        let slot = BrushParamSlot::PerDabF32(dynamic_fields.len());
                        dynamic_fields.push(BrushParamField {
                            name: param.name.clone(),
                            ty: param.ty.clone(),
                            default: param.default.clone(),
                            slot,
                            dynamics,
                        });
                    }
                }
                ParamType::Dynamics { .. } => {}
                ParamType::Bool | ParamType::U32 | ParamType::Enum { .. } => {
                    let slot = BrushParamSlot::U32(next_u32);
                    next_u32 += 1;
                    fields.push(BrushParamField {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                        default: param.default.clone(),
                        slot,
                        dynamics: Vec::new(),
                    });
                }
                ParamType::ResourceRef { .. } => {
                    resource_fields.push(BrushResourceParamField {
                        name: param.name.clone(),
                        default: param.default.clone(),
                    });
                }
            }
        }
        Self {
            fields,
            dynamic_fields,
            resource_fields,
        }
    }

    pub(crate) fn fields(&self) -> &[BrushParamField] {
        &self.fields
    }

    pub(crate) fn resource_fields(&self) -> &[BrushResourceParamField] {
        &self.resource_fields
    }

    pub(crate) fn dynamic_fields(&self) -> &[BrushParamField] {
        &self.dynamic_fields
    }

    fn fields_in_uniform_order(&self) -> impl Iterator<Item = &BrushParamField> {
        self.fields
            .iter()
            .filter(|field| matches!(field.slot, BrushParamSlot::F32(_)))
            .chain(
                self.fields
                    .iter()
                    .filter(|field| matches!(field.slot, BrushParamSlot::U32(_))),
            )
    }
}

fn compile_resources(engine: &BrushEngineDefinition) -> Vec<CompiledBrushResource> {
    let mut resources = engine
        .resources
        .iter()
        .map(|(name, definition)| CompiledBrushResource {
            name: name.clone(),
            definition: definition.clone(),
        })
        .collect::<Vec<_>>();
    resources.sort_by(|left, right| left.name.cmp(&right.name));
    resources
}

fn resource_lookup(resources: &[CompiledBrushResource]) -> HashMap<&str, CompiledResourceRef> {
    resources
        .iter()
        .enumerate()
        .map(|(index, resource)| (resource.name.as_str(), CompiledResourceRef(index)))
        .collect()
}

pub(crate) fn resource_definition(
    resources: &[CompiledBrushResource],
    resource: CompiledResourceRef,
) -> &BrushResourceDefinition {
    &resources[resource.0].definition
}

pub(crate) fn resource_name(
    resources: &[CompiledBrushResource],
    resource: CompiledResourceRef,
) -> &str {
    &resources[resource.0].name
}

impl BrushPassRuntime {
    fn new(
        device: &wgpu::Device,
        engine_id: &str,
        param_layout: &BrushParamLayout,
        resources: &[CompiledBrushResource],
        resource_lookup: &HashMap<&str, CompiledResourceRef>,
        name: &str,
        pipeline_definition: &PipelineDefinition,
    ) -> Result<Self> {
        let raster = pipeline_definition.raster;
        let inputs = pass_inputs(pipeline_definition, resource_lookup);
        let outputs = pass_outputs(pipeline_definition, resource_lookup);
        let bind_group_layout = create_pass_bind_group_layout(device, raster, &inputs, resources);
        let source = shader_source(
            raster,
            param_layout,
            &inputs,
            pipeline_definition,
            resources,
            resource_lookup,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("brush_{engine_id}_{name}_shader")),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("brush_{engine_id}_{name}_layout")),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let mesh_layout = [Some(mesh_vtx_layout_full())];
        let dab_quad_layouts = [Some(quad_vtx_layout()), Some(dab_instance_layout())];
        let vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>] = match raster {
            RasterMode::FullscreenTriangle => &[],
            RasterMode::TargetMeshUv => &mesh_layout,
            RasterMode::UVDabQuadInstances | RasterMode::ViewportDabQuadInstances => {
                &dab_quad_layouts
            }
        };
        let color_targets = color_targets(&outputs, resources);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&format!("brush_{engine_id}_{name}_pipeline")),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: vertex_buffers,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &color_targets,
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
        });
        let scratch_resources = scratch_resources(&inputs, &outputs, resources);
        let reads_stroke_snapshot = pass_reads_surface_texture(
            &inputs,
            resources,
            SurfaceTextureSource::Canvas,
            SurfaceTextureSync::StrokeBeginSnapshot,
        );
        let writes_canvas = pass_writes_surface_texture(
            &outputs,
            resources,
            SurfaceTextureSource::Canvas,
            SurfaceTextureSync::Live,
        );
        let reads_batch_source = pass_reads_surface_texture(
            &inputs,
            resources,
            SurfaceTextureSource::Canvas,
            SurfaceTextureSync::BeforeEachBatch,
        );
        let reads_current_source = pass_reads_surface_texture(
            &inputs,
            resources,
            SurfaceTextureSource::Canvas,
            SurfaceTextureSync::BeforeEachPass,
        );
        let reads_viewport_source_color = pass_reads_surface_texture_source(
            &inputs,
            resources,
            SurfaceTextureSource::ViewportColor,
        );
        let reads_viewport_depth = pass_reads_surface_texture_source(
            &inputs,
            resources,
            SurfaceTextureSource::ViewportDepth,
        );
        let reads_current_dab_batch =
            pass_reads_builtin_input(&inputs, BuiltinPassInput::CurrentDabBatch);
        let uses_uv_dab_quad_instances = raster == RasterMode::UVDabQuadInstances;
        let uses_viewport_dab_quad_instances = raster == RasterMode::ViewportDabQuadInstances;
        let reads_selection_mask =
            pass_reads_builtin_input(&inputs, BuiltinPassInput::SelectionMask);
        Ok(Self {
            name: name.to_owned(),
            raster,
            inputs,
            outputs,
            bind_group_layout,
            scratch_resources,
            reads_stroke_snapshot,
            writes_canvas,
            reads_batch_source,
            reads_current_source,
            reads_viewport_source_color,
            reads_viewport_depth,
            reads_current_dab_batch,
            uses_uv_dab_quad_instances,
            uses_viewport_dab_quad_instances,
            reads_selection_mask,
            resource_params: param_layout.resource_fields().to_vec(),
            pipeline,
        })
    }

    pub(crate) fn writes_canvas(&self) -> bool {
        self.writes_canvas
    }

    pub(crate) fn reads_current_source(&self) -> bool {
        self.reads_current_source
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

fn quad_vtx_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        }],
    }
}

fn dab_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<BrushDabInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 4,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 24,
                shader_location: 5,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 32,
                shader_location: 6,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 48,
                shader_location: 7,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 64,
                shader_location: 8,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 80,
                shader_location: 9,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    }
}

fn shader_source(
    raster: RasterMode,
    param_layout: &BrushParamLayout,
    inputs: &[PassInputRuntime],
    pipeline: &PipelineDefinition,
    resources: &[CompiledBrushResource],
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n@fragment\nfn fs_main(in: {}) -> PassOutput {{\n{}{}\n}}\n",
        common_wgsl(param_layout, inputs),
        builtin_input_wgsl(raster, inputs, param_layout),
        input_declarations(inputs),
        output_declaration(pipeline, resources, resource_lookup),
        match raster {
            RasterMode::FullscreenTriangle => FULLSCREEN_VERTEX_WGSL,
            RasterMode::TargetMeshUv => TARGET_MESH_VERTEX_WGSL,
            RasterMode::UVDabQuadInstances => UV_DAB_QUAD_VERTEX_WGSL,
            RasterMode::ViewportDabQuadInstances => VIEWPORT_DAB_QUAD_VERTEX_WGSL,
        },
        brush_dab_helpers_wgsl(raster, inputs, param_layout),
        match raster {
            RasterMode::FullscreenTriangle => "FullscreenVertexOutput",
            RasterMode::TargetMeshUv => "TargetMeshUvFragmentInput",
            RasterMode::UVDabQuadInstances => "UVDabQuadFragmentInput",
            RasterMode::ViewportDabQuadInstances => "ViewportDabQuadFragmentInput",
        },
        brush_dab_fragment_prologue(raster),
        pipeline.fragment
    )
}

fn output_declaration(
    pipeline: &PipelineDefinition,
    resources: &[CompiledBrushResource],
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
) -> String {
    let mut declaration = String::from("struct PassOutput {\n");
    for (location, output) in pipeline.color_targets.iter().enumerate() {
        let resource = *resource_lookup
            .get(output.resource())
            .expect("validated output resource exists");
        let ty = match resource_format(resource_definition(resources, resource)) {
            TextureResourceFormat::R8Unorm => "f32",
            TextureResourceFormat::Rgba8Unorm | TextureResourceFormat::Rgba16Float => "vec4<f32>",
        };
        declaration.push_str(&format!(
            "    @location({}) {}: {},\n",
            location,
            output.name(),
            ty
        ));
    }
    declaration.push_str("};\n");
    declaration
}

fn brush_dab_fragment_prologue(raster: RasterMode) -> &'static str {
    match raster {
        RasterMode::UVDabQuadInstances | RasterMode::ViewportDabQuadInstances => {
            "let dab = brush_dab_from_input(in);\n"
        }
        RasterMode::FullscreenTriangle | RasterMode::TargetMeshUv => "",
    }
}

fn brush_dab_helpers_wgsl(
    raster: RasterMode,
    inputs: &[PassInputRuntime],
    param_layout: &BrushParamLayout,
) -> String {
    let mut source = String::new();
    let has_current_dabs = inputs.iter().any(|input| {
        matches!(
            input.binding,
            CompiledPassBinding::Builtin(BuiltinPassInput::CurrentDabBatch)
        )
    });
    match raster {
        RasterMode::FullscreenTriangle if has_current_dabs => {
            source.push_str(
                r#"
fn brush_dab_from_current(current: CurrentDab) -> BrushDab {
    var dab: BrushDab;
    dab.radius_px = current.radius_px;
    dab.radius_world = 0.0;
    dab.pressure = current.pressure;
    dab.spacing_alpha_scale = current.spacing_alpha_scale;
    dab.direction = current.dir;
"#,
            );
            source.push_str(&dynamic_dab_field_initializers_wgsl(
                param_layout,
                "current",
                "    ",
            ));
            source.push_str("    return dab;\n}\n");
        }
        RasterMode::TargetMeshUv if has_current_dabs => {
            source.push_str(
                r#"
fn brush_dab_from_current(current: CurrentDab) -> BrushDab {
    var dab: BrushDab;
    dab.radius_px = 0.0;
    dab.radius_world = current.center_radius.w;
    dab.pressure = current.normal_pressure.w;
    dab.spacing_alpha_scale = 1.0;
    dab.direction = vec2<f32>(current.tangent_x_pad.w, current.tangent_y_pad.w);
"#,
            );
            source.push_str(&dynamic_dab_field_initializers_wgsl(
                param_layout,
                "current",
                "    ",
            ));
            source.push_str("    return dab;\n}\n");
        }
        RasterMode::FullscreenTriangle | RasterMode::TargetMeshUv => {}
        RasterMode::UVDabQuadInstances => {
            source.push_str(
                r#"
fn brush_dab_from_input(input: UVDabQuadFragmentInput) -> BrushDab {
    var dab: BrushDab;
    dab.radius_px = input.radius_px;
    dab.radius_world = 0.0;
    dab.pressure = input.pressure;
    dab.spacing_alpha_scale = input.spacing_alpha_scale;
    dab.direction = input.dir;
"#,
            );
            source.push_str(&dynamic_dab_field_initializers_wgsl(
                param_layout,
                "input",
                "    ",
            ));
            source.push_str("    return dab;\n}\n");
        }
        RasterMode::ViewportDabQuadInstances => {
            source.push_str(
                r#"
fn brush_dab_from_input(input: ViewportDabQuadFragmentInput) -> BrushDab {
    var dab: BrushDab;
    dab.radius_px = input.radius_px;
    dab.radius_world = 0.0;
    dab.pressure = input.pressure;
    dab.spacing_alpha_scale = input.spacing_alpha_scale;
    dab.direction = input.dir;
"#,
            );
            source.push_str(&dynamic_dab_field_initializers_wgsl(
                param_layout,
                "input",
                "    ",
            ));
            source.push_str("    return dab;\n}\n");
        }
    }
    source
}

fn color_targets(
    outputs: &[CompiledPassAttachment],
    resources: &[CompiledBrushResource],
) -> Vec<Option<wgpu::ColorTargetState>> {
    outputs
        .iter()
        .map(|output| {
            let resource = resource_definition(resources, output.resource);
            let format = texture_format(resource_format(resource));
            let write_mask = match resource_format(resource) {
                TextureResourceFormat::R8Unorm => wgpu::ColorWrites::RED,
                TextureResourceFormat::Rgba8Unorm | TextureResourceFormat::Rgba16Float => {
                    wgpu::ColorWrites::ALL
                }
            };
            Some(wgpu::ColorTargetState {
                format,
                blend: blend_state(output.blend),
                write_mask,
            })
        })
        .collect()
}

fn texture_format(format: TextureResourceFormat) -> wgpu::TextureFormat {
    match format {
        TextureResourceFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureResourceFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        TextureResourceFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
    }
}

fn blend_state(blend: PassBlend) -> Option<wgpu::BlendState> {
    let color = match blend {
        PassBlend::Replace => return None,
        PassBlend::AddClamp => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        PassBlend::Max => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        },
        PassBlend::AlphaAccumulate => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    Some(wgpu::BlendState {
        color,
        alpha: color,
    })
}

fn input_declarations(inputs: &[PassInputRuntime]) -> String {
    let mut declarations = String::new();
    for input in inputs {
        declarations.push_str(&input_declaration(input));
        declarations.push('\n');
    }
    declarations
}

fn builtin_input_wgsl(
    raster: RasterMode,
    inputs: &[PassInputRuntime],
    _param_layout: &BrushParamLayout,
) -> String {
    let has_current_dabs = inputs.iter().any(|input| {
        matches!(
            input.binding,
            CompiledPassBinding::Builtin(BuiltinPassInput::CurrentDabBatch)
        )
    });
    if !has_current_dabs {
        return String::new();
    }
    let dynamic_storage_fields = r#"
    dynamic0: vec4<f32>,
    dynamic1: vec4<f32>,
    dynamic2: vec4<f32>,
    dynamic3: vec4<f32>,
"#;
    match raster {
        RasterMode::FullscreenTriangle => format!(
            r#"
struct CurrentDab {{
    center_px: vec2<f32>,
    radius_px: f32,
    pressure: f32,
    dir: vec2<f32>,
    spacing_alpha_scale: f32,
    _pad0: f32,
{dynamic_storage_fields}}};

struct CurrentDabBatch {{
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    items: array<CurrentDab>,
}};
"#
        ),
        RasterMode::TargetMeshUv => format!(
            r#"
struct CurrentDab {{
    center_radius: vec4<f32>,
    normal_pressure: vec4<f32>,
    tangent_x_pad: vec4<f32>,
    tangent_y_pad: vec4<f32>,
{dynamic_storage_fields}}};

struct CurrentDabBatch {{
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    items: array<CurrentDab>,
}};
"#
        ),
        RasterMode::UVDabQuadInstances | RasterMode::ViewportDabQuadInstances => String::new(),
    }
}

fn dynamic_dab_fields_wgsl(param_layout: &BrushParamLayout) -> String {
    let mut source = String::new();
    for field in param_layout.dynamic_fields() {
        source.push_str(&format!("    {}: f32,\n", field.name));
    }
    source
}

fn dynamic_dab_field_initializers_wgsl(
    param_layout: &BrushParamLayout,
    prefix: &str,
    indent: &str,
) -> String {
    let mut source = String::new();
    for field in param_layout.dynamic_fields() {
        let BrushParamSlot::PerDabF32(index) = field.slot else {
            continue;
        };
        let vector = index / 4;
        let lane = ["x", "y", "z", "w"][index % 4];
        source.push_str(&format!(
            "{indent}dab.{} = {prefix}.dynamic{vector}.{lane};\n",
            field.name
        ));
    }
    source
}

fn input_declaration(input: &PassInputRuntime) -> String {
    match &input.binding {
        CompiledPassBinding::SampledTexture { .. } => {
            format!(
                "@group(0) @binding({}) var {}: texture_2d<f32>;",
                input.binding_index, input.name
            )
        }
        CompiledPassBinding::Builtin(BuiltinPassInput::CurrentDabBatch) => {
            format!(
                "@group(0) @binding({}) var<storage, read> {}: CurrentDabBatch;",
                input.binding_index, input.name
            )
        }
        CompiledPassBinding::Builtin(BuiltinPassInput::SelectionMask) => {
            format!(
                "@group(0) @binding({}) var {}: texture_2d<f32>;",
                input.binding_index, input.name
            )
        }
        CompiledPassBinding::Builtin(BuiltinPassInput::StrokeDabs) => {
            unreachable!("StrokeDabs is rejected during validation")
        }
        CompiledPassBinding::Sampler(_) => {
            format!(
                "@group(0) @binding({}) var {}: sampler;",
                input.binding_index, input.name
            )
        }
        CompiledPassBinding::Uniform(uniform) => {
            let ty = match uniform {
                UniformKind::BrushParams => "BrushParams",
                UniformKind::ViewProj => "ViewProj",
                UniformKind::BakeParams => "BakeParams",
            };
            format!(
                "@group(0) @binding({}) var<uniform> {}: {};",
                input.binding_index, input.name, ty
            )
        }
    }
}

fn pass_inputs(
    pipeline: &PipelineDefinition,
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
) -> Vec<PassInputRuntime> {
    let mut inputs = pipeline
        .inputs
        .iter()
        .map(|(name, binding)| (name.clone(), binding.clone()))
        .collect::<Vec<_>>();
    inputs.sort_by(|(left_name, left_binding), (right_name, right_binding)| {
        binding_sort_key(left_binding)
            .cmp(&binding_sort_key(right_binding))
            .then_with(|| left_name.cmp(right_name))
    });
    inputs
        .into_iter()
        .flat_map(|(name, binding)| compile_binding_entries(name, binding, resource_lookup))
        .enumerate()
        .map(|(index, (name, binding))| PassInputRuntime {
            name,
            binding,
            binding_index: index as u32,
        })
        .collect()
}

fn pass_outputs(
    pipeline: &PipelineDefinition,
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
) -> Vec<CompiledPassAttachment> {
    pipeline
        .color_targets
        .iter()
        .map(|output| CompiledPassAttachment {
            name: output.name().to_owned(),
            resource: *resource_lookup
                .get(output.resource())
                .expect("validated output resource exists"),
            load: output.load,
            clear_value: output.clear_value.clone(),
            blend: output.blend,
        })
        .collect()
}

fn compile_binding_entries(
    name: String,
    binding: PassBinding,
    resource_lookup: &HashMap<&str, CompiledResourceRef>,
) -> Vec<(String, CompiledPassBinding)> {
    match binding {
        PassBinding::SampledTexture {
            resource,
            sample_type,
            view_dimension,
            sampler,
        } => {
            let resource = *resource_lookup
                .get(resource.as_str())
                .expect("validated input resource exists");
            vec![
                (
                    name.clone(),
                    CompiledPassBinding::SampledTexture {
                        resource,
                        sample_type,
                        view_dimension,
                    },
                ),
                (
                    format!("{name}_sampler"),
                    CompiledPassBinding::Sampler(sampler),
                ),
            ]
        }
        PassBinding::Builtin(input) => vec![(name, CompiledPassBinding::Builtin(input))],
        PassBinding::Sampler(sampler) => vec![(name, CompiledPassBinding::Sampler(sampler))],
        PassBinding::Uniform(uniform) => vec![(name, CompiledPassBinding::Uniform(uniform))],
        PassBinding::StorageTexture { .. }
        | PassBinding::DepthTexture { .. }
        | PassBinding::ExternalTexture { .. } => {
            unreachable!("unsupported texture bindings are rejected during validation")
        }
    }
}

fn binding_sort_key(input: &PassBinding) -> (u8, String) {
    match input {
        PassBinding::SampledTexture { resource, .. } => (0, resource.clone()),
        PassBinding::StorageTexture { resource, .. } => (1, resource.clone()),
        PassBinding::DepthTexture { resource, .. } => (2, resource.clone()),
        PassBinding::ExternalTexture { resource } => (3, resource.clone()),
        PassBinding::Builtin(BuiltinPassInput::CurrentDabBatch) => (4, "dabs".to_owned()),
        PassBinding::Builtin(BuiltinPassInput::SelectionMask) => (5, "selection".to_owned()),
        PassBinding::Builtin(BuiltinPassInput::StrokeDabs) => (6, "stroke_dabs".to_owned()),
        PassBinding::Sampler(sampler) => (7, format!("sampler_{sampler:?}")),
        PassBinding::Uniform(UniformKind::BrushParams) => (8, "brush".to_owned()),
        PassBinding::Uniform(UniformKind::ViewProj) => (9, "view".to_owned()),
        PassBinding::Uniform(UniformKind::BakeParams) => (10, "bake".to_owned()),
    }
}

fn create_pass_bind_group_layout(
    device: &wgpu::Device,
    raster: RasterMode,
    inputs: &[PassInputRuntime],
    resources: &[CompiledBrushResource],
) -> wgpu::BindGroupLayout {
    let entries = inputs
        .iter()
        .map(|input| match &input.binding {
            CompiledPassBinding::SampledTexture {
                resource,
                sample_type,
                view_dimension,
            } => bind_tex(
                input.binding_index,
                *sample_type,
                *view_dimension,
                texture_binding_visibility(raster, *resource, resources),
            ),
            CompiledPassBinding::Builtin(BuiltinPassInput::CurrentDabBatch) => {
                bind_readonly_storage(input.binding_index)
            }
            CompiledPassBinding::Builtin(BuiltinPassInput::SelectionMask) => bind_tex(
                input.binding_index,
                TextureSampleType::Float { filterable: true },
                TextureViewDimension::D2,
                wgpu::ShaderStages::FRAGMENT,
            ),
            CompiledPassBinding::Builtin(BuiltinPassInput::StrokeDabs) => {
                unreachable!("StrokeDabs is rejected during validation")
            }
            CompiledPassBinding::Sampler(sampler) => {
                bind_sampler(input.binding_index, sampler.is_filtering())
            }
            CompiledPassBinding::Uniform(UniformKind::BrushParams) => {
                bind_uniform(input.binding_index, wgpu::ShaderStages::VERTEX_FRAGMENT)
            }
            CompiledPassBinding::Uniform(UniformKind::ViewProj) => {
                bind_uniform(input.binding_index, wgpu::ShaderStages::VERTEX_FRAGMENT)
            }
            CompiledPassBinding::Uniform(UniformKind::BakeParams) => {
                let visibility = if raster == RasterMode::ViewportDabQuadInstances {
                    wgpu::ShaderStages::VERTEX_FRAGMENT
                } else {
                    wgpu::ShaderStages::FRAGMENT
                };
                bind_uniform(input.binding_index, visibility)
            }
        })
        .collect::<Vec<_>>();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("brush_pass_bgl"),
        entries: &entries,
    })
}

fn bind_readonly_storage(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_binding_visibility(
    raster: RasterMode,
    resource: CompiledResourceRef,
    resources: &[CompiledBrushResource],
) -> wgpu::ShaderStages {
    if raster == RasterMode::ViewportDabQuadInstances
        && matches!(
            resource_definition(resources, resource),
            BrushResourceDefinition::SurfaceTexture {
                source: SurfaceTextureSource::ViewportDepth,
                ..
            }
        )
    {
        wgpu::ShaderStages::VERTEX_FRAGMENT
    } else {
        wgpu::ShaderStages::FRAGMENT
    }
}

impl SamplerKind {
    fn is_filtering(self) -> bool {
        matches!(self, SamplerKind::LinearClamp | SamplerKind::LinearRepeat)
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

fn bind_tex(
    binding: u32,
    sample_type: TextureSampleType,
    view_dimension: TextureViewDimension,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    let sample_type = match sample_type {
        TextureSampleType::Float { filterable } => wgpu::TextureSampleType::Float { filterable },
    };
    let view_dimension = match view_dimension {
        TextureViewDimension::D2 => wgpu::TextureViewDimension::D2,
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled: false,
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

fn scratch_resources(
    inputs: &[PassInputRuntime],
    outputs: &[CompiledPassAttachment],
    resources: &[CompiledBrushResource],
) -> Vec<BrushScratchResource> {
    let mut scratch = Vec::new();
    for input in inputs {
        if let CompiledPassBinding::SampledTexture { resource, .. } = &input.binding {
            if let Some(resource) = brush_scratch_resource(resources, *resource) {
                scratch.push(resource);
            }
        }
    }
    for output in outputs {
        if let Some(resource) = brush_scratch_resource(resources, output.resource) {
            scratch.push(resource);
        }
    }
    scratch.sort_by(|left, right| {
        scratch_resource_sort_key(left).cmp(&scratch_resource_sort_key(right))
    });
    scratch.dedup_by(|left, right| {
        scratch_resource_sort_key(left) == scratch_resource_sort_key(right)
    });
    scratch
}

fn brush_scratch_resource(
    resources: &[CompiledBrushResource],
    resource: CompiledResourceRef,
) -> Option<BrushScratchResource> {
    let name = resource_name(resources, resource);
    let BrushResourceDefinition::Texture {
        format,
        extent,
        lifetime,
        ..
    } = resource_definition(resources, resource)
    else {
        return None;
    };
    Some(BrushScratchResource {
        name: name.to_owned(),
        format: *format,
        extent: extent.clone(),
        lifetime: *lifetime,
    })
}

fn scratch_resource_sort_key(
    resource: &BrushScratchResource,
) -> (&str, TextureResourceFormat, u8, TextureResourceLifetime) {
    (
        resource.name.as_str(),
        resource.format,
        extent_sort_key(&resource.extent),
        resource.lifetime,
    )
}

fn extent_sort_key(extent: &TextureResourceExtent) -> u8 {
    match extent {
        TextureResourceExtent::PaintSurface => 0,
        TextureResourceExtent::Viewport => 1,
        TextureResourceExtent::Fixed { .. } => 2,
        TextureResourceExtent::MatchResource(_) => 3,
        TextureResourceExtent::ScaleOf { .. } => 4,
    }
}

fn pass_reads_surface_texture(
    inputs: &[PassInputRuntime],
    resources: &[CompiledBrushResource],
    source: SurfaceTextureSource,
    sync: SurfaceTextureSync,
) -> bool {
    inputs.iter().any(|input| {
        let CompiledPassBinding::SampledTexture { resource, .. } = &input.binding else {
            return false;
        };
        matches!(
            resource_definition(resources, *resource),
            BrushResourceDefinition::SurfaceTexture {
                source: candidate_source,
                sync: candidate_sync,
                ..
            } if *candidate_source == source && *candidate_sync == sync
        )
    })
}

fn pass_reads_surface_texture_source(
    inputs: &[PassInputRuntime],
    resources: &[CompiledBrushResource],
    source: SurfaceTextureSource,
) -> bool {
    inputs.iter().any(|input| {
        let CompiledPassBinding::SampledTexture { resource, .. } = &input.binding else {
            return false;
        };
        matches!(
            resource_definition(resources, *resource),
            BrushResourceDefinition::SurfaceTexture {
                source: candidate_source,
                ..
            } if *candidate_source == source
        )
    })
}

fn pass_reads_builtin_input(inputs: &[PassInputRuntime], builtin: BuiltinPassInput) -> bool {
    inputs.iter().any(|input| {
        matches!(input.binding, CompiledPassBinding::Builtin(candidate) if candidate == builtin)
    })
}

fn pass_writes_surface_texture(
    outputs: &[CompiledPassAttachment],
    resources: &[CompiledBrushResource],
    source: SurfaceTextureSource,
    sync: SurfaceTextureSync,
) -> bool {
    outputs.iter().any(|output| {
        matches!(
            resource_definition(resources, output.resource),
            BrushResourceDefinition::SurfaceTexture {
                source: candidate_source,
                sync: candidate_sync,
                ..
            } if *candidate_source == source && *candidate_sync == sync
        )
    })
}

fn common_wgsl(param_layout: &BrushParamLayout, inputs: &[PassInputRuntime]) -> String {
    let mut source = String::new();
    for field in &param_layout.fields {
        if let ParamType::Enum { variants } = &field.ty {
            for variant in variants {
                source.push_str(&format!(
                    "const {}_{}: u32 = {}u;\n",
                    field.name, variant.name, variant.value
                ));
            }
        }
    }
    source.push_str(
        r#"
struct BrushParams {
    color_rgb_pad: vec4<f32>,
    base_radius_world: f32,
    base_radius_px: f32,
    tex_size: vec2<f32>,
"#,
    );
    let mut f32_names: Vec<Option<&str>> = vec![None; 16];
    let mut u32_names: Vec<Option<&str>> = vec![None; 16];
    for field in param_layout.fields_in_uniform_order() {
        match field.slot {
            BrushParamSlot::F32(index) => f32_names[index] = Some(field.name.as_str()),
            BrushParamSlot::U32(index) => u32_names[index] = Some(field.name.as_str()),
            BrushParamSlot::PerDabF32(_) => {}
        }
    }
    for (index, name) in f32_names.into_iter().enumerate() {
        let name = name
            .map(str::to_owned)
            .unwrap_or_else(|| format!("ext_f32_pad{index}"));
        source.push_str(&format!("    {name}: f32,\n"));
    }
    for (index, name) in u32_names.into_iter().enumerate() {
        let name = name
            .map(str::to_owned)
            .unwrap_or_else(|| format!("ext_u32_pad{index}"));
        source.push_str(&format!("    {name}: u32,\n"));
    }
    source.push_str(
        r#"};

struct ViewProj {
    view_proj: mat4x4<f32>,
};

struct BakeParams {
    camera_world: vec4<f32>,
    paint_color: vec4<f32>,
    viewport_depth: vec4<f32>,
    viewport_metrics: vec4<f32>,
    depth_params: vec4<f32>,
};
"#,
    );
    source.push_str(
        r#"
struct BrushDab {
    radius_px: f32,
    radius_world: f32,
    pressure: f32,
    spacing_alpha_scale: f32,
    direction: vec2<f32>,
"#,
    );
    source.push_str(&dynamic_dab_fields_wgsl(param_layout));
    source.push_str("};\n");
    let has_brush_params = inputs.iter().any(|input| {
        input.name == "brush"
            && matches!(
                input.binding,
                CompiledPassBinding::Uniform(UniformKind::BrushParams)
            )
    });
    let has_bake_params = inputs.iter().any(|input| {
        input.name == "bake"
            && matches!(
                input.binding,
                CompiledPassBinding::Uniform(UniformKind::BakeParams)
            )
    });
    if has_brush_params {
        source.push_str(
            r#"
fn brush_radius_texture_px() -> f32 {
    return brush.base_radius_px;
}

fn brush_relative_texture_px(ratio: f32) -> f32 {
    return brush_radius_texture_px() * ratio;
}
"#,
        );
    }
    if has_brush_params && has_bake_params {
        source.push_str(
            r#"
fn brush_depth01_to_view_depth(depth01: f32) -> f32 {
    let near = bake.depth_params.x;
    let far = bake.depth_params.y;
    if (bake.depth_params.z >= 0.5) {
        return max(near + depth01 * (far - near), 1e-4);
    }
    let depth_denom = max(far - depth01 * (far - near), 1e-6);
    return max((near * far) / depth_denom, 1e-4);
}

fn brush_radius_viewport_px(depth01: f32) -> f32 {
    if (bake.depth_params.z >= 0.5) {
        return brush.base_radius_world * bake.viewport_metrics.w;
    }
    let view_depth = brush_depth01_to_view_depth(depth01);
    let focal_px = 0.5 * max(bake.viewport_metrics.y, 1.0) * bake.viewport_metrics.z;
    return brush.base_radius_world * focal_px / view_depth;
}

fn brush_relative_viewport_px(depth01: f32, ratio: f32) -> f32 {
    return brush_radius_viewport_px(depth01) * ratio;
}
"#,
        );
    }
    source.push_str(
        r#"

fn sample_blur(tex: texture_2d<f32>, smp: sampler, uv: vec2<f32>, dir: vec2<f32>, radius_px: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let texel = 1.0 / max(dims, vec2<f32>(1.0, 1.0));
    let radius = min(i32(ceil(max(radius_px, 0.0))), 32);
    if (radius <= 0) {
        return textureSampleLevel(tex, smp, uv, 0.0);
    }
    let axis = select(vec2<f32>(1.0, 0.0), normalize(dir), dot(dir, dir) > 0.0);
    let sigma = max(radius_px * 0.5, 0.5);
    let two_sigma2 = 2.0 * sigma * sigma;
    var sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var i = -radius; i <= radius; i = i + 1) {
        let dist = f32(i);
        let w = exp(-(dist * dist) / two_sigma2);
        sum += textureSampleLevel(tex, smp, uv + axis * dist * texel, 0.0) * w;
        weight_sum += w;
    }
    if (weight_sum <= 1e-5) {
        return textureSampleLevel(tex, smp, uv, 0.0);
    }
    return sum / weight_sum;
}

fn brush_falloff(dist: f32, radius: f32, hardness: f32, aa_floor: f32) -> f32 {
    if (radius <= 1e-6) {
        return 0.0;
    }
    let aa = max(fwidth(dist), aa_floor);
    if (dist > radius + aa) {
        return 0.0;
    }
    let base_soft = radius * (1.0 - hardness);
    let soft = max(base_soft, aa);
    if (soft <= 1e-6) {
        return select(0.0, 1.0, dist <= radius);
    }
    let inner = max(radius - soft, 0.0);
    if (dist <= inner) {
        return 1.0;
    }
    return 1.0 - smoothstep(inner, radius + aa, dist);
}

fn brush_paint_alpha(dist: f32, radius: f32, hardness: f32, anti_aliasing: u32, aa_floor: f32) -> f32 {
    if (radius <= 1e-6) {
        return 0.0;
    }
    let aa = select(0.0, max(fwidth(dist), aa_floor), anti_aliasing != 0u);
    if (dist > radius + aa) {
        return 0.0;
    }
    let base_soft = radius * (1.0 - hardness);
    let soft = select(base_soft, max(base_soft, aa), anti_aliasing != 0u);
    if (soft <= 1e-6) {
        return select(0.0, 1.0, dist <= radius);
    }
    let inner = max(radius - soft, 0.0);
    if (dist <= inner) {
        return 1.0;
    }
    return 1.0 - smoothstep(inner, radius + aa, dist);
}

fn brush_subpixel_coverage_gain(radius_px: f32) -> f32 {
    // When the brush footprint is smaller than one pixel, the analytic AA ramp
    // still covers roughly a pixel-sized region. Scale the mask by projected
    // area so subpixel dabs do not behave like full 1px stamps.
    let radius = max(radius_px, 0.0);
    return clamp(radius * radius, 0.0, 1.0);
}

fn brush_spacing_alpha_compensate(alpha: f32, spacing_alpha_scale: f32) -> f32 {
    let a = clamp(alpha, 0.0, 1.0);
    let scale = max(spacing_alpha_scale, 1.0);
    if (scale <= 1.0) {
        return a;
    }
    return 1.0 - pow(1.0 - a, scale);
}

fn sample_local_alpha_texture(
    tip_tex: texture_2d<f32>,
    tip_smp: sampler,
    local: vec2<f32>,
    radius: f32,
    anti_aliasing: u32,
    aa_floor: f32,
) -> f32 {
    let size = max(radius * 2.0, 1e-6);
    let tip_uv = local / size + vec2<f32>(0.5, 0.5);
    if (anti_aliasing == 0u) {
        if (tip_uv.x < 0.0 || tip_uv.x > 1.0 || tip_uv.y < 0.0 || tip_uv.y > 1.0) {
            return 0.0;
        }
        return textureSampleLevel(tip_tex, tip_smp, tip_uv, 0.0).r;
    }
    let uv_dx = max(fwidth(tip_uv.x), aa_floor);
    let uv_dy = max(fwidth(tip_uv.y), aa_floor);
    let edge_x = smoothstep(0.0, uv_dx, tip_uv.x) *
        (1.0 - smoothstep(1.0 - uv_dx, 1.0, tip_uv.x));
    let edge_y = smoothstep(0.0, uv_dy, tip_uv.y) *
        (1.0 - smoothstep(1.0 - uv_dy, 1.0, tip_uv.y));
    let edge = edge_x * edge_y;
    if (edge <= 0.0) {
        return 0.0;
    }
    let tip_uv_dx = dpdx(tip_uv);
    let tip_uv_dy = dpdy(tip_uv);
    return textureSampleGrad(tip_tex, tip_smp, tip_uv, tip_uv_dx, tip_uv_dy).r * edge;
}

"#,
    );
    source
}

const FULLSCREEN_VERTEX_WGSL: &str = r#"
struct FullscreenVertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> FullscreenVertexOutput {
    var tri = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    var out: FullscreenVertexOutput;
    out.clip_pos = vec4<f32>(tri[i], 0.0, 1.0);
    out.uv = vec2<f32>(tri[i].x * 0.5 + 0.5, 1.0 - (tri[i].y * 0.5 + 0.5));
    return out;
}
"#;

const TARGET_MESH_VERTEX_WGSL: &str = r#"
struct TargetMeshUvFragmentInput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> TargetMeshUvFragmentInput {
    var out: TargetMeshUvFragmentInput;
    out.clip_pos = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    out.uv = uv;
    out.world_pos = position;
    out.normal = normal;
    return out;
}
"#;

const UV_DAB_QUAD_VERTEX_WGSL: &str = r#"
struct UVDabQuadFragmentInput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) center_uv: vec2<f32>,
    @location(2) local_px: vec2<f32>,
    @location(3) radius_px: f32,
    @location(4) pressure: f32,
    @location(5) spacing_alpha_scale: f32,
    @location(6) dir: vec2<f32>,
    @location(7) dynamic0: vec4<f32>,
    @location(8) dynamic1: vec4<f32>,
    @location(9) dynamic2: vec4<f32>,
    @location(10) dynamic3: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) quad_pos: vec2<f32>,
    @location(1) center_uv: vec2<f32>,
    @location(2) radius_scale: f32,
    @location(3) pressure: f32,
    @location(4) spacing_alpha_scale: f32,
    @location(5) dir: vec2<f32>,
    @location(6) dynamic0: vec4<f32>,
    @location(7) dynamic1: vec4<f32>,
    @location(8) dynamic2: vec4<f32>,
    @location(9) dynamic3: vec4<f32>,
) -> UVDabQuadFragmentInput {
    let radius_px = max(brush.base_radius_px * radius_scale, 1e-6);
    let quad_radius_px = max(radius_px, 1.0);
    let padded_radius_px = quad_radius_px + 1.0;
    let local_px = quad_pos * padded_radius_px;
    let uv = center_uv + local_px / max(brush.tex_size, vec2<f32>(1.0, 1.0));

    var out: UVDabQuadFragmentInput;
    out.clip_pos = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, 0.0, 1.0);
    out.uv = uv;
    out.center_uv = center_uv;
    out.local_px = local_px;
    out.radius_px = radius_px;
    out.pressure = pressure;
    out.spacing_alpha_scale = spacing_alpha_scale;
    out.dir = dir;
    out.dynamic0 = dynamic0;
    out.dynamic1 = dynamic1;
    out.dynamic2 = dynamic2;
    out.dynamic3 = dynamic3;
    return out;
}
"#;

const VIEWPORT_DAB_QUAD_VERTEX_WGSL: &str = r#"
const VIEWPORT_DAB_AA_PAD_PX: f32 = 1.0;

struct ViewportDabQuadFragmentInput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) viewport_uv: vec2<f32>,
    @location(1) frag_px: vec2<f32>,
    @location(2) center_px: vec2<f32>,
    @location(3) local_px: vec2<f32>,
    @location(4) radius_px: f32,
    @location(5) pressure: f32,
    @location(6) spacing_alpha_scale: f32,
    @location(7) dir: vec2<f32>,
    @location(8) dynamic0: vec4<f32>,
    @location(9) dynamic1: vec4<f32>,
    @location(10) dynamic2: vec4<f32>,
    @location(11) dynamic3: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) quad_pos: vec2<f32>,
    @location(1) center_px: vec2<f32>,
    @location(2) radius_scale: f32,
    @location(3) pressure: f32,
    @location(4) spacing_alpha_scale: f32,
    @location(5) dir: vec2<f32>,
    @location(6) dynamic0: vec4<f32>,
    @location(7) dynamic1: vec4<f32>,
    @location(8) dynamic2: vec4<f32>,
    @location(9) dynamic3: vec4<f32>,
) -> ViewportDabQuadFragmentInput {
    let viewport_size = max(bake.viewport_depth.xy, vec2<f32>(1.0, 1.0));
    let center_ix = vec2<i32>(
        clamp(i32(center_px.x), 0, i32(viewport_size.x) - 1),
        clamp(i32(center_px.y), 0, i32(viewport_size.y) - 1)
    );
    let center_depth = textureLoad(viewport_depth_tex, center_ix, 0).r;
    var radius_px = 0.0;
    if (center_depth < 0.99999) {
        radius_px = max(brush_radius_viewport_px(center_depth) * radius_scale, 1e-6);
    }
    let quad_radius_px = max(radius_px, 1.0);
    let padded_radius_px = quad_radius_px + VIEWPORT_DAB_AA_PAD_PX;
    let local_px = quad_pos * padded_radius_px;
    let frag_px = center_px + local_px;
    let ndc = vec2<f32>(
        (frag_px.x / viewport_size.x) * 2.0 - 1.0,
        1.0 - (frag_px.y / viewport_size.y) * 2.0
    );

    var out: ViewportDabQuadFragmentInput;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.viewport_uv = frag_px / viewport_size;
    out.frag_px = frag_px;
    out.center_px = center_px;
    out.local_px = local_px;
    out.radius_px = radius_px;
    out.pressure = pressure;
    out.spacing_alpha_scale = spacing_alpha_scale;
    out.dir = dir;
    out.dynamic0 = dynamic0;
    out.dynamic1 = dynamic1;
    out.dynamic2 = dynamic2;
    out.dynamic3 = dynamic3;
    return out;
}
"#;

#[cfg(test)]
mod tests {
    use crate::core::brush_engine::BrushEngineDefinition;

    use super::{
        BrushParamLayout, BrushParamSlot, UV_DAB_QUAD_VERTEX_WGSL, VIEWPORT_DAB_QUAD_VERTEX_WGSL,
        common_wgsl,
    };

    #[test]
    fn separate_dynamics_param_uses_no_gpu_slot_and_makes_target_per_dab() {
        let engine: BrushEngineDefinition =
            ron::from_str(crate::embedded_resources::text("brushes/engines/paint.ron").unwrap())
                .unwrap();
        let layout = BrushParamLayout::from_engine(&engine);

        assert!(
            layout
                .fields()
                .iter()
                .all(|field| field.name != "flow_pressure")
        );
        assert!(
            layout
                .dynamic_fields()
                .iter()
                .all(|field| field.name != "flow_pressure")
        );
        let flow = layout
            .dynamic_fields()
            .iter()
            .find(|field| field.name == "flow")
            .expect("flow should be resolved per dab");
        assert!(matches!(flow.slot, BrushParamSlot::PerDabF32(_)));
        assert_eq!(flow.dynamics.len(), 1);
        assert_eq!(flow.dynamics[0].parameter, "flow_pressure");

        assert!(common_wgsl(&layout, &[]).contains("direction: vec2<f32>"));
    }

    #[test]
    fn uv_dab_quad_keeps_subpixel_mask_radius() {
        assert!(
            UV_DAB_QUAD_VERTEX_WGSL
                .contains("let radius_px = max(brush.base_radius_px * radius_scale, 1e-6);")
        );
        assert!(UV_DAB_QUAD_VERTEX_WGSL.contains("let quad_radius_px = max(radius_px, 1.0);"));
        assert!(UV_DAB_QUAD_VERTEX_WGSL.contains("out.radius_px = radius_px;"));
        assert!(UV_DAB_QUAD_VERTEX_WGSL.contains("out.spacing_alpha_scale = spacing_alpha_scale;"));
    }

    #[test]
    fn viewport_dab_quad_keeps_subpixel_mask_radius() {
        assert!(VIEWPORT_DAB_QUAD_VERTEX_WGSL.contains(
            "radius_px = max(brush_radius_viewport_px(center_depth) * radius_scale, 1e-6);"
        ));
        assert!(
            VIEWPORT_DAB_QUAD_VERTEX_WGSL.contains("let quad_radius_px = max(radius_px, 1.0);")
        );
        assert!(VIEWPORT_DAB_QUAD_VERTEX_WGSL.contains("out.radius_px = radius_px;"));
        assert!(
            VIEWPORT_DAB_QUAD_VERTEX_WGSL
                .contains("out.spacing_alpha_scale = spacing_alpha_scale;")
        );
    }
}
