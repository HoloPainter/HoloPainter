use anyhow::{Result, anyhow, ensure};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        adjustment::{Adjustment, AdjustmentKind},
        damage::{DamageMap, UV_ISLAND_BLEED_RADIUS_PX},
        geometry::RectU32,
        render_report::RenderMetrics,
        selection::ActiveSelection,
        surface::PaintSurfaceId,
        surface_filter::SurfaceFilterDomain,
    },
    renderer::{
        adjustment_gpu::{AdjustmentLutUniform, adjustment_gpu_params},
        document::surfaces::SurfaceEditContext,
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
    },
};

use super::{
    feature::{FilterFeature, FilterFeatureDeps},
    packing::validate_adjustment_preview_working_set,
    shared::{
        FilterGutterUniform, GpuMaterialInfo, OwnedTexture2D, ValidatedFilterTargets,
        create_r32_uint_texture, create_rgba_texture, record_triangle_map,
        resolve_filter_selection_view, validate_filter_targets,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AdjustmentFilterUniform {
    adjustment_kind: u32,
    selection_enabled: u32,
    target_domain: u32,
    _pad: u32,
    adjustment_params: [[f32; 4]; 8],
}

pub(super) struct AdjustmentPreviewSession {
    kind: AdjustmentKind,
    domain: SurfaceFilterDomain,
    surfaces: Vec<PaintSurfaceId>,
    active_selection: ActiveSelection,
    base_surfaces: Vec<AdjustmentPreviewSurface>,
    resources: AdjustmentRenderResources,
}

struct AdjustmentPreviewSurface {
    surface: PaintSurfaceId,
    texture_size: [u32; 2],
    base: OwnedTexture2D,
}

struct AdjustmentRenderResources {
    triangle_map: OwnedTexture2D,
    filter_output: OwnedTexture2D,
    material_buffer: wgpu::Buffer,
    material_sizes: Vec<[u32; 2]>,
    material_count_u32: u32,
}

pub(super) fn execute(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    surfaces: &[PaintSurfaceId],
    adjustment: Adjustment,
    active_selection: &ActiveSelection,
) -> Result<CommandResult> {
    ensure!(
        feature.adjustment_preview.is_none(),
        "cannot apply an adjustment filter while a preview session is active"
    );
    let targets = validate_filter_targets(deps, surfaces, "adjustment filter")?;
    let adjustment = normalize_adjustment(adjustment, targets.domain, false)?;
    let resources = create_render_resources(frame, deps, surfaces, &targets)?;
    render_adjustment_surfaces(
        feature,
        frame,
        deps,
        surfaces,
        adjustment,
        active_selection,
        &resources,
        targets.domain,
        None,
        true,
    )
}

pub(super) fn begin_preview(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    surfaces: &[PaintSurfaceId],
    adjustment: Adjustment,
    active_selection: &ActiveSelection,
) -> Result<CommandResult> {
    ensure!(
        feature.adjustment_preview.is_none(),
        "an adjustment filter preview session is already active"
    );
    let targets = validate_filter_targets(deps, surfaces, "adjustment filter preview")?;
    let adjustment = normalize_adjustment(adjustment, targets.domain, true)?;
    validate_adjustment_preview_working_set(&targets.material_sizes, surfaces)?;

    let resources = create_render_resources(frame, deps, surfaces, &targets)?;
    let mut metrics = RenderMetrics::default();
    let mut base_surfaces = Vec::with_capacity(surfaces.len());
    for surface in surfaces {
        let prepare = deps
            .surfaces
            .prepare_edit_surface_for_frame(deps.gpu, frame, *surface)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let target = deps.surfaces.edit_surface_target(*surface).ok_or_else(|| {
            anyhow!("adjustment filter preview target is not GPU resident: {surface:?}")
        })?;
        let base = create_rgba_texture(
            deps.gpu.device(),
            target.texture_size,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            "adjustment_filter_preview_base",
        );
        copy_texture(
            frame.encoder(),
            target.texture,
            &base.texture,
            target.texture_size,
        );
        base_surfaces.push(AdjustmentPreviewSurface {
            surface: *surface,
            texture_size: target.texture_size,
            base,
        });
    }

    let session = AdjustmentPreviewSession {
        kind: adjustment.kind(),
        domain: targets.domain,
        surfaces: surfaces.to_vec(),
        active_selection: active_selection.clone(),
        base_surfaces,
        resources,
    };
    let mut result = render_adjustment_surfaces(
        feature,
        frame,
        deps,
        &session.surfaces,
        adjustment,
        &session.active_selection,
        &session.resources,
        session.domain,
        Some(&session.base_surfaces),
        false,
    )?;
    result.metrics.merge(metrics);
    feature.adjustment_preview = Some(session);
    Ok(result)
}

pub(super) fn update_preview(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    adjustment: Option<Adjustment>,
) -> Result<CommandResult> {
    let session = feature
        .adjustment_preview
        .take()
        .ok_or_else(|| anyhow!("adjustment filter preview has no active session"))?;
    let result = (|| match adjustment {
        Some(adjustment) => {
            let adjustment = normalize_adjustment(adjustment, session.domain, true)?;
            ensure!(
                adjustment.kind() == session.kind,
                "adjustment filter preview kind changed during the session"
            );
            render_adjustment_surfaces(
                feature,
                frame,
                deps,
                &session.surfaces,
                adjustment,
                &session.active_selection,
                &session.resources,
                session.domain,
                Some(&session.base_surfaces),
                false,
            )
        }
        None => restore_preview_surfaces(frame, deps, &session, false),
    })();
    feature.adjustment_preview = Some(session);
    result
}

pub(super) fn commit_preview(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    adjustment: Adjustment,
) -> Result<CommandResult> {
    let session = feature
        .adjustment_preview
        .take()
        .ok_or_else(|| anyhow!("adjustment filter preview has no active session"))?;
    let result = (|| {
        let adjustment = normalize_adjustment(adjustment, session.domain, true)?;
        ensure!(
            adjustment.kind() == session.kind,
            "adjustment filter preview commit kind changed during the session"
        );
        render_adjustment_surfaces(
            feature,
            frame,
            deps,
            &session.surfaces,
            adjustment,
            &session.active_selection,
            &session.resources,
            session.domain,
            Some(&session.base_surfaces),
            true,
        )
    })();
    if result.is_ok() {
        retain_preview_session(frame, session);
    } else {
        feature.adjustment_preview = Some(session);
    }
    result
}

pub(super) fn cancel_preview(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
) -> Result<CommandResult> {
    let Some(session) = feature.adjustment_preview.take() else {
        return Ok(CommandResult::default());
    };
    let result = restore_preview_surfaces(frame, deps, &session, true);
    retain_preview_session(frame, session);
    result
}

fn render_adjustment_surfaces(
    feature: &FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    surfaces: &[PaintSurfaceId],
    adjustment: Adjustment,
    active_selection: &ActiveSelection,
    resources: &AdjustmentRenderResources,
    domain: SurfaceFilterDomain,
    base_surfaces: Option<&[AdjustmentPreviewSurface]>,
    committed: bool,
) -> Result<CommandResult> {
    let scene = feature
        .scene
        .as_ref()
        .ok_or_else(|| anyhow!("adjustment filter requires an uploaded mesh"))?;
    let gpu_params = adjustment_gpu_params(&adjustment);
    let adjustment_lut = AdjustmentLutUniform::for_adjustment(&adjustment)
        .unwrap_or_else(AdjustmentLutUniform::zeroed);
    let adjustment_lut_buffer = frame.create_buffer_from_slice(
        deps.gpu.device(),
        "adjustment_filter_lut",
        wgpu::BufferUsages::UNIFORM,
        &[adjustment_lut],
    );

    let mut mutations = MutationLog::default();
    let mut metrics = RenderMetrics::default();
    let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
    for surface in surfaces {
        let stats = surface_edit.prepare_edit_surface_for_frame(deps.gpu, frame, *surface)?;
        record_surface_prepare_metrics(&mut metrics, &stats);
    }

    for surface in surfaces {
        let material_index = surface.material_index().as_usize();
        let target = surface_edit
            .edit_surface_target(*surface)
            .ok_or_else(|| anyhow!("adjustment filter target is not GPU resident: {surface:?}"))?;
        let size = target.texture_size;
        ensure!(
            size == resources.material_sizes[material_index],
            "adjustment filter target size mismatch for material {material_index}"
        );
        let source_view = match base_surfaces {
            Some(base_surfaces) => {
                let base = base_surfaces
                    .iter()
                    .find(|base| base.surface == *surface)
                    .ok_or_else(|| anyhow!("adjustment filter preview base surface is missing"))?;
                ensure!(
                    base.texture_size == size,
                    "adjustment filter preview target size changed during the session"
                );
                &base.base.view
            }
            None => target.view,
        };
        let (selection_view, selection_enabled) = resolve_filter_selection_view(
            deps.selections,
            active_selection,
            material_index,
            source_view,
        )?;
        record_triangle_map(
            frame,
            &feature.pipelines,
            scene,
            material_index,
            size,
            &resources.triangle_map.view,
        );

        let uniform = AdjustmentFilterUniform {
            adjustment_kind: gpu_params.kind,
            selection_enabled,
            target_domain: match domain {
                SurfaceFilterDomain::Color => 0,
                SurfaceFilterDomain::Scalar => 1,
            },
            _pad: 0,
            adjustment_params: gpu_params.params,
        };
        let uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "adjustment_filter_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[uniform],
        );
        let apply_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("adjustment_filter_bg"),
                layout: &feature.pipelines.adjustment.apply_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(selection_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&resources.filter_output.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: adjustment_lut_buffer.as_entire_binding(),
                    },
                ],
            });
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("adjustment_filter_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.adjustment.apply);
            pass.set_bind_group(0, &apply_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        copy_texture(
            frame.encoder(),
            &resources.filter_output.texture,
            target.texture,
            size,
        );

        let gutter_uniform = FilterGutterUniform {
            radius_world: 0.0,
            sigma_world: 0.0,
            material_count: resources.material_count_u32,
            current_material: u32::try_from(material_index)
                .map_err(|_| anyhow!("adjustment filter material index exceeds u32"))?,
            gutter_radius: UV_ISLAND_BLEED_RADIUS_PX,
            _pad: [0; 3],
        };
        let gutter_uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "adjustment_filter_gutter_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[gutter_uniform],
        );
        let gutter_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("adjustment_filter_gutter_bg"),
                layout: &feature.pipelines.shared.gutter_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&resources.filter_output.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&resources.triangle_map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: resources.material_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(target.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: gutter_uniform_buffer.as_entire_binding(),
                    },
                ],
            });
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("adjustment_filter_gutter_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.shared.gutter);
            pass.set_bind_group(0, &gutter_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
    }
    drop(surface_edit);

    deps.surfaces.resolve_mask_edit_proxies_into_frame(
        deps.gpu,
        frame,
        surfaces.iter().copied(),
    )?;

    for surface in surfaces {
        if committed {
            mutations.surfaces.full(*surface);
            mutations.surface_commits.full(*surface);
        } else {
            let mut damage = DamageMap::default();
            damage.add_rect(
                *surface,
                RectU32::full(resources.material_sizes[surface.material_index().as_usize()]),
            );
            mutations.surfaces.transient_damage(*surface, damage);
        }
    }
    Ok(CommandResult::new(mutations, metrics))
}

fn restore_preview_surfaces(
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    session: &AdjustmentPreviewSession,
    cancelled: bool,
) -> Result<CommandResult> {
    let mut mutations = MutationLog::default();
    let mut metrics = RenderMetrics::default();
    for base in &session.base_surfaces {
        let prepare =
            deps.surfaces
                .prepare_edit_surface_for_frame(deps.gpu, frame, base.surface)?;
        record_surface_prepare_metrics(&mut metrics, &prepare);
        let target = deps
            .surfaces
            .edit_surface_target(base.surface)
            .ok_or_else(|| {
                anyhow!(
                    "adjustment filter preview target is not GPU resident: {:?}",
                    base.surface
                )
            })?;
        ensure!(
            target.texture_size == base.texture_size,
            "adjustment filter preview target size changed during the session"
        );
        copy_texture(
            frame.encoder(),
            &base.base.texture,
            target.texture,
            base.texture_size,
        );
        deps.surfaces
            .resolve_mask_edit_proxy_into_frame(deps.gpu, frame, base.surface)?;
        if cancelled {
            mutations.surfaces.cancelled(base.surface);
        } else {
            let mut damage = DamageMap::default();
            damage.add_rect(base.surface, RectU32::full(base.texture_size));
            mutations.surfaces.transient_damage(base.surface, damage);
        }
    }
    Ok(CommandResult::new(mutations, metrics))
}

fn create_render_resources(
    frame: &mut GpuFrame,
    deps: &FilterFeatureDeps<'_>,
    surfaces: &[PaintSurfaceId],
    targets: &ValidatedFilterTargets,
) -> Result<AdjustmentRenderResources> {
    let material_count_u32 = u32::try_from(targets.material_sizes.len())
        .map_err(|_| anyhow!("adjustment filter material count exceeds u32"))?;
    let max_size = surfaces.iter().fold([1, 1], |maximum, surface| {
        let size = targets.material_sizes[surface.material_index().as_usize()];
        [maximum[0].max(size[0]), maximum[1].max(size[1])]
    });
    let triangle_map = create_r32_uint_texture(
        deps.gpu.device(),
        max_size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        "adjustment_filter_triangle_map",
    );
    let filter_output = create_rgba_texture(
        deps.gpu.device(),
        max_size,
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        "adjustment_filter_output",
    );
    let material_buffer = create_adjustment_material_buffer(
        frame,
        deps.gpu.device(),
        &targets.material_sizes,
        &targets.active_materials,
    );
    Ok(AdjustmentRenderResources {
        triangle_map,
        filter_output,
        material_buffer,
        material_sizes: targets.material_sizes.clone(),
        material_count_u32,
    })
}

fn normalize_adjustment(
    adjustment: Adjustment,
    domain: SurfaceFilterDomain,
    preview: bool,
) -> Result<Adjustment> {
    let kind = adjustment.kind();
    let supported = if preview {
        kind.supports_destructive_filter_preview(domain)
    } else {
        kind.supports_destructive_filter(domain)
    };
    ensure!(
        supported,
        "{} is not available as a destructive {}filter",
        kind.id(),
        if preview { "preview " } else { "" }
    );
    let adjustment = adjustment
        .try_normalized_for_domain(domain)
        .ok_or_else(|| anyhow!("adjustment filter parameters must be finite"))?;
    ensure!(
        !adjustment.is_identity(),
        "{} adjustment filter must not be an identity operation",
        kind.id()
    );
    Ok(adjustment)
}

fn create_adjustment_material_buffer(
    frame: &mut GpuFrame,
    device: &wgpu::Device,
    material_sizes: &[[u32; 2]],
    active_materials: &[bool],
) -> wgpu::Buffer {
    let materials = material_sizes
        .iter()
        .copied()
        .zip(active_materials.iter().copied())
        .map(|(size, active)| GpuMaterialInfo {
            origin: [0, 0],
            size,
            enabled: u32::from(active),
            _pad: [0; 3],
        })
        .collect::<Vec<_>>();
    frame.create_buffer_from_slice(
        device,
        "adjustment_filter_materials",
        wgpu::BufferUsages::STORAGE,
        &materials,
    )
}

fn copy_texture(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    size: [u32; 2],
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
}

fn retain_preview_session(frame: &mut GpuFrame, session: AdjustmentPreviewSession) {
    for base in session.base_surfaces {
        frame.retain_texture_until_submit(base.base.texture, base.base.view);
    }
    frame.retain_texture_until_submit(
        session.resources.triangle_map.texture,
        session.resources.triangle_map.view,
    );
    frame.retain_texture_until_submit(
        session.resources.filter_output.texture,
        session.resources.filter_output.view,
    );
}
