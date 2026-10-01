use super::command::SelectionEditCommand;

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        composite::SelectionCompositeMode,
        mask::MaskSource,
        render_report::RenderMetrics,
        selection::{ActiveSelection, SelectionTilePayload},
    },
    renderer::{
        document::{
            GpuDocument,
            materials::MaterialRegistry,
            scene::SceneResources,
            selection::{SelectionEditContext, SelectionMasks, SelectionUploadStats},
        },
        engine::gpu_state::RendererGpuState,
        features::{
            apply::pipelines::PaintApplyPipelines,
            selection::apply::apply_gpu_projection_to_selection_mask,
        },
        gpu::{buffer::create_uniform_buffer, frame::GpuFrame},
        mutation::MutationLog,
        report::CommandResult,
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
    },
};

const SELECTION_MASK_COMPOSITE_SHADER: &str =
    include_str!("../../../shaders/selection_mask_composite.wgsl");

/// Target vertical boundary for selection renderer work.
pub(crate) struct SelectionFeature {
    pipelines: SelectionPipelines,
}

pub(crate) struct SelectionFeatureDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) selections: &'a mut SelectionMasks,
    pub(crate) materials: &'a MaterialRegistry,
    pub(crate) scene: &'a mut SceneResources,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
    pub(crate) projection_pipelines: &'a PaintApplyPipelines,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SelectionCompositeUniform {
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

pub(crate) struct SelectionPipelines {
    pub(crate) composite_uniform: wgpu::Buffer,
    pub(crate) composite_bgl: wgpu::BindGroupLayout,
    pub(crate) composite_pipeline: wgpu::RenderPipeline,
}

impl SelectionFeature {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: SelectionPipelines::new(device),
        }
    }

    pub(crate) fn upload_tiles(
        &self,
        frame: &mut GpuFrame,
        device: &wgpu::Device,
        document: &mut GpuDocument,
        active_selection: ActiveSelection,
        tiles: Vec<SelectionTilePayload>,
    ) -> anyhow::Result<CommandResult> {
        let mut mutations = MutationLog::default();
        let mut selection_sync = crate::renderer::document::selection::SelectionSyncContext::new(
            &mut document.selections,
            &mut mutations,
        );

        let mut metrics = RenderMetrics::default();
        record_selection_upload_metrics(
            &mut metrics,
            selection_sync.upload_tiles(
                device,
                frame,
                &document.materials,
                &active_selection,
                &tiles,
            )?,
        );

        Ok(CommandResult::new(mutations, metrics))
    }

    pub(crate) fn execute_edit_command(
        &self,
        frame: &mut GpuFrame,
        deps: &mut SelectionFeatureDeps<'_>,
        command: SelectionEditCommand,
    ) -> anyhow::Result<CommandResult> {
        let SelectionEditCommand::Update { mask, .. } = &command;
        crate::renderer::mask::support::ensure_supported(
            crate::renderer::mask::support::MaskConsumer::Selection,
            mask,
            None,
        )?;
        let mut mutations = MutationLog::default();
        let mut metrics = RenderMetrics::default();
        match command {
            SelectionEditCommand::Update {
                mask,
                params,
                active_selection,
            } => {
                let stats = match &mask {
                    MaskSource::Projection(projection) => {
                        let stats = apply_gpu_projection_to_selection_mask(
                            frame,
                            deps,
                            &self.pipelines,
                            projection,
                            params,
                            &active_selection,
                        );
                        mutations.selections.mask_changed(active_selection.clone());
                        stats
                    }
                    MaskSource::Full(_)
                    | MaskSource::Shape(
                        crate::core::mask::ShapeMaskSource::Rectangle(_)
                        | crate::core::mask::ShapeMaskSource::Polygon(_),
                    ) => {
                        let mut selection_edit =
                            SelectionEditContext::new(deps.selections, &mut mutations);
                        selection_edit.update_mask(
                            deps.gpu.device(),
                            frame,
                            deps.scene,
                            deps.materials,
                            &mask,
                            params,
                            &active_selection,
                        )?
                    }
                    MaskSource::Brush(_)
                    | MaskSource::Geometry(_)
                    | MaskSource::Flood(_)
                    | MaskSource::ExistingSelection => {
                        unreachable!("unsupported masks fail preflight")
                    }
                };
                record_selection_upload_metrics(&mut metrics, stats);
            }
        }

        Ok(CommandResult::new(mutations, metrics))
    }
}

impl SelectionPipelines {
    fn new(device: &wgpu::Device) -> Self {
        let composite_uniform = create_uniform_buffer::<SelectionCompositeUniform>(
            device,
            "selection_composite_uniform",
        );
        let composite_bgl = create_selection_composite_bgl(device);
        let composite_pipeline = create_selection_composite_pipeline(device, &composite_bgl);
        Self {
            composite_uniform,
            composite_bgl,
            composite_pipeline,
        }
    }
}

pub(crate) fn selection_composite_mode_index(mode: SelectionCompositeMode) -> u32 {
    match mode {
        SelectionCompositeMode::Replace => 0,
        SelectionCompositeMode::Add => 1,
        SelectionCompositeMode::Subtract => 2,
        SelectionCompositeMode::Intersect => 3,
        SelectionCompositeMode::Difference => 4,
        SelectionCompositeMode::Clear => 5,
        SelectionCompositeMode::Invert => 6,
    }
}

pub(crate) fn render_selection_composite(
    frame: &mut GpuFrame,
    device: &wgpu::Device,
    pipelines: &SelectionPipelines,
    target_view: &wgpu::TextureView,
    base_view: &wgpu::TextureView,
    overlay_view: &wgpu::TextureView,
    composite: SelectionCompositeMode,
) {
    let uniform = SelectionCompositeUniform {
        mode: selection_composite_mode_index(composite),
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    frame.write_buffer_pod(device, &pipelines.composite_uniform, 0, &uniform);
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("selection_mask_composite_bg"),
        layout: &pipelines.composite_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(base_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(overlay_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pipelines.composite_uniform.as_entire_binding(),
            },
        ],
    });

    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("selection_mask_composite_pass"),
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
    pass.set_pipeline(&pipelines.composite_pipeline);
    pass.set_bind_group(0, &bg, &[]);
    pass.draw(0..3, 0..1);
}

fn create_selection_composite_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("selection_mask_composite_bgl"),
        entries: &[
            bind_tex(0),
            bind_tex(1),
            bind_uniform(2, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}

fn create_selection_composite_pipeline(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("selection_mask_composite_shader"),
        source: wgpu::ShaderSource::Wgsl(SELECTION_MASK_COMPOSITE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("selection_mask_composite_layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("selection_mask_composite_pipeline"),
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
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::RED,
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

fn bind_tex(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
        },
        count: None,
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

fn record_selection_upload_metrics(metrics: &mut RenderMetrics, stats: SelectionUploadStats) {
    metrics.selection_upload_calls = metrics
        .selection_upload_calls
        .saturating_add(stats.upload_count);
    metrics.selection_upload_bytes = metrics
        .selection_upload_bytes
        .saturating_add(stats.uploaded_bytes);
    metrics.selection_full_upload_calls = metrics
        .selection_full_upload_calls
        .saturating_add(stats.full_upload_count);
    metrics.selection_rect_upload_calls = metrics
        .selection_rect_upload_calls
        .saturating_add(stats.rect_upload_count);
}
