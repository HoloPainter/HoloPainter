use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        mask::{
            FullMaskSource, GeometryMaskSource, MaskSource, ProjectionMaskSource, ShapeMaskSource,
        },
        selection::{ActiveSelection, SelectionMaskId},
        surface::PaintSurfaceId,
    },
    renderer::{
        document::selection::SelectionStore,
        engine::gpu_state::RendererGpuState,
        features::{
            apply::{
                deps::PaintApplyDeps, materialize::PreparedMask, pipelines::PaintApplyPipelines,
                types::UvCompositeUniform,
            },
            brush::transient as stroke_transient,
        },
        gpu::{copy_a_to_b, frame::GpuFrame},
        mask::support::{ApplyOperationKind, MaskConsumer, UnsupportedMaskError, ensure_supported},
    },
};

use super::{
    materialize::prepare_mask_texture_for_request,
    operation::{ApplyOperation, apply_operation_with_prepared_mask},
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OneShotMaskedApplyRequest {
    pub target: PaintSurfaceId,
    pub mask: MaskSource,
    pub operation: ApplyOperation,
    pub params: ApplyParams<TextureCompositeMode>,
    pub active_selection: ActiveSelection,
}

pub(crate) fn plan_one_shot_masked_apply_requests(
    target: PaintSurfaceId,
    surfaces: &[PaintSurfaceId],
    mask: &MaskSource,
    operation: &ApplyOperation,
    params: ApplyParams<TextureCompositeMode>,
    active_selection: &ActiveSelection,
) -> Result<Vec<OneShotMaskedApplyRequest>, UnsupportedMaskError> {
    let operation_kind = operation_kind(operation);
    ensure_supported(MaskConsumer::Paint, mask, Some(operation_kind))?;
    let requests = match mask {
        MaskSource::Full(FullMaskSource::MeshAllMaterials) => surfaces
            .iter()
            .copied()
            .map(|surface| one_shot_request(surface, mask, operation, params, active_selection))
            .collect(),
        MaskSource::Full(FullMaskSource::Material { material_index }) => surfaces
            .iter()
            .copied()
            .filter(|surface| surface.material_index.as_usize() == *material_index)
            .map(|surface| one_shot_request(surface, mask, operation, params, active_selection))
            .collect(),
        MaskSource::Shape(ShapeMaskSource::Rectangle(rect)) => {
            let shape_target = rect
                .material_index
                .and_then(|material_index| {
                    surfaces
                        .iter()
                        .copied()
                        .find(|surface| surface.material_index.as_usize() == material_index)
                })
                .unwrap_or(target);
            vec![one_shot_request(
                shape_target,
                mask,
                operation,
                params,
                active_selection,
            )]
        }
        MaskSource::Shape(ShapeMaskSource::Polygon(polygon)) => {
            let shape_target = polygon
                .material_index
                .and_then(|material_index| {
                    surfaces
                        .iter()
                        .copied()
                        .find(|surface| surface.material_index.as_usize() == material_index)
                })
                .unwrap_or(target);
            vec![one_shot_request(
                shape_target,
                mask,
                operation,
                params,
                active_selection,
            )]
        }
        MaskSource::Geometry(GeometryMaskSource::Mesh(mesh)) => surfaces
            .iter()
            .copied()
            .filter(|surface| {
                mesh.triangles.iter().any(|tri| {
                    tri.mesh_id == mesh.mesh_id
                        && tri.material_index == surface.material_index.as_usize()
                })
            })
            .map(|surface| one_shot_request(surface, mask, operation, params, active_selection))
            .collect(),
        MaskSource::Projection(projection) => {
            let material_index = match projection {
                ProjectionMaskSource::ViewportRect(rect) => rect.material_index,
                ProjectionMaskSource::ViewportPolygon(polygon) => polygon.material_index,
            };
            surfaces
                .iter()
                .copied()
                .filter(|surface| {
                    material_index.is_none_or(|material_index| {
                        material_index == surface.material_index.as_usize()
                    })
                })
                .map(|surface| one_shot_request(surface, mask, operation, params, active_selection))
                .collect()
        }
        MaskSource::Brush(_) | MaskSource::Flood(_) | MaskSource::ExistingSelection => {
            return Err(UnsupportedMaskError::new(
                MaskConsumer::Paint,
                mask,
                Some(operation_kind),
            ));
        }
    };
    Ok(requests)
}

fn one_shot_request(
    target: PaintSurfaceId,
    mask: &MaskSource,
    operation: &ApplyOperation,
    params: ApplyParams<TextureCompositeMode>,
    active_selection: &ActiveSelection,
) -> OneShotMaskedApplyRequest {
    OneShotMaskedApplyRequest {
        target,
        mask: mask.clone(),
        operation: operation.clone(),
        params,
        active_selection: active_selection.clone(),
    }
}

pub(crate) fn record_one_shot_masked_apply(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    request: &OneShotMaskedApplyRequest,
) -> anyhow::Result<Option<OneShotMaskedApplyResult>> {
    preflight_one_shot_request(request)?;
    let Some(mask) = prepare_mask_texture_for_request(frame, txn, request)? else {
        return Ok(None);
    };
    Ok(
        apply_operation_with_prepared_mask(frame, txn, request, &mask)
            .then(|| OneShotMaskedApplyResult::from_request_and_mask(request, mask)),
    )
}

pub(crate) fn preflight_one_shot_requests(
    requests: &[OneShotMaskedApplyRequest],
) -> Result<(), UnsupportedMaskError> {
    for request in requests {
        preflight_one_shot_request(request)?;
    }
    Ok(())
}

fn preflight_one_shot_request(
    request: &OneShotMaskedApplyRequest,
) -> Result<(), UnsupportedMaskError> {
    let operation_kind = operation_kind(&request.operation);
    ensure_supported(MaskConsumer::Paint, &request.mask, Some(operation_kind))
}

pub(crate) fn operation_kind(operation: &ApplyOperation) -> ApplyOperationKind {
    match operation {
        ApplyOperation::SolidColorPaint { .. } => ApplyOperationKind::SolidColorPaint,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OneShotMaskedApplyResult {
    pub(crate) target: PaintSurfaceId,
    pub(crate) material_index: usize,
    pub(crate) texture_size: [u32; 2],
    pub(crate) needs_geometry_dilation: bool,
}

impl OneShotMaskedApplyResult {
    fn from_request_and_mask(request: &OneShotMaskedApplyRequest, mask: PreparedMask) -> Self {
        Self {
            target: mask.target,
            material_index: mask.material_index,
            texture_size: mask.texture_size,
            needs_geometry_dilation: mask_needs_geometry_dilation(&request.mask),
        }
    }
}

fn mask_needs_geometry_dilation(mask: &MaskSource) -> bool {
    match mask {
        MaskSource::Geometry(GeometryMaskSource::Mesh(_))
        | MaskSource::Projection(ProjectionMaskSource::ViewportRect(_))
        | MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(_)) => true,
        MaskSource::Brush(_)
        | MaskSource::Shape(ShapeMaskSource::Rectangle(_))
        | MaskSource::Shape(ShapeMaskSource::Polygon(_))
        | MaskSource::Full(_)
        | MaskSource::Flood(_)
        | MaskSource::ExistingSelection => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScratchGeneratedMask {
    pub size: [u32; 2],
    pub material_index: Option<usize>,
}

pub(crate) fn record_masked_paint_apply_with_uniform(
    frame: &mut GpuFrame,
    txn: &mut PaintApplyDeps<'_, '_>,
    target: PaintSurfaceId,
    comp_u: &UvCompositeUniform,
    active_selection: Option<&ActiveSelection>,
) {
    let material_index = target.material_index().as_usize();
    let Some(mask_view) = stroke_transient::material_stroke_uv_view(txn.scratch, material_index)
    else {
        return;
    };
    let Some(layer) = txn.surfaces.stroke_surface_target(target, txn.scratch) else {
        return;
    };
    let mask = ScratchGeneratedMask {
        size: layer.texture_size,
        material_index: Some(material_index),
    };
    debug_assert_eq!(mask.material_index, Some(material_index));
    record_masked_paint_apply_with_resources(
        frame,
        &*txn.gpu,
        txn.pipelines,
        &*txn.selections,
        material_index,
        mask_view,
        layer.read_texture,
        layer.read_view,
        layer.write_texture,
        layer.write_view,
        layer.texture_size,
        comp_u,
        active_selection,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_masked_paint_apply_with_resources(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    pipelines: &PaintApplyPipelines,
    selections: &SelectionStore,
    material_index: usize,
    mask_view: &wgpu::TextureView,
    source_texture: &wgpu::Texture,
    source_view: &wgpu::TextureView,
    write_texture: &wgpu::Texture,
    write_view: &wgpu::TextureView,
    texture_size: [u32; 2],
    comp_u: &UvCompositeUniform,
    active_selection: Option<&ActiveSelection>,
) -> bool {
    let selection = EffectiveSelection::from_active_selection(active_selection, material_index);
    let (selection_view, selection_enabled) = match selection {
        EffectiveSelection::Full => (mask_view, 0),
        EffectiveSelection::Empty => return false,
        EffectiveSelection::GpuMask(mask_id) => {
            let Some(selection_view) = selections.view(mask_id) else {
                debug_assert!(
                    false,
                    "ActiveSelection references a missing GPU selection texture: {:?}",
                    mask_id
                );
                return false;
            };
            (selection_view, 1)
        }
    };
    let mut comp_u = *comp_u;
    comp_u.selection_enabled = selection_enabled;

    frame.write_buffer_pod(
        gpu.device(),
        &pipelines.masked_apply_composite_uniform,
        0,
        &comp_u,
    );
    let composite_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("masked_paint_apply_bg"),
        layout: &pipelines.masked_apply_composite_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(gpu.stroke_sampler()),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipelines.masked_apply_composite_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(selection_view),
            },
        ],
    });

    copy_a_to_b(frame.encoder(), source_texture, write_texture, texture_size);

    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("masked_paint_apply_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: write_view,
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
    pass.set_pipeline(&pipelines.masked_apply_composite_pipeline);
    pass.set_bind_group(0, &composite_bg, &[]);
    pass.draw(0..3, 0..1);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveSelection {
    Full,
    Empty,
    GpuMask(SelectionMaskId),
}

impl EffectiveSelection {
    fn from_active_selection(
        active_selection: Option<&ActiveSelection>,
        material_index: usize,
    ) -> Self {
        let Some(active_selection) = active_selection else {
            return Self::Full;
        };
        if active_selection.is_effectively_full() {
            return Self::Full;
        }
        if let Some(mask_id) = active_selection
            .masks
            .iter()
            .find(|mask| mask.material_index.as_usize() == material_index)
            .and_then(|mask| mask.mask_id)
        {
            Self::GpuMask(mask_id)
        } else {
            Self::Empty
        }
    }
}
