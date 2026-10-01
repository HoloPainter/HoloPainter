use anyhow::{Result, anyhow, ensure};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        damage::UV_ISLAND_BLEED_RADIUS_PX, render_report::RenderMetrics,
        selection::ActiveSelection, stroke::PaintSurfaceSet, surface::PaintSurfaceId,
    },
    renderer::{
        document::surfaces::SurfaceEditContext,
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
    },
};

use super::{
    command::SurfaceBlurParams,
    feature::{FilterFeature, FilterFeatureDeps},
    packing::{pack_filter_materials, validate_filter_working_set},
    shared::{
        FilterGutterUniform, copy_surfaces_to_source_atlas, create_material_buffer,
        create_r32_uint_texture, create_rgba_texture, record_triangle_map,
        resolve_filter_selection_view, validate_affected_filter_targets, validate_filter_targets,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SurfaceBlurUniform {
    radius_world: f32,
    sigma_world: f32,
    material_count: u32,
    current_material: u32,
    gutter_radius: u32,
    selection_enabled: u32,
    _pad: [u32; 2],
}

pub(super) fn execute(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    source_surfaces: &[PaintSurfaceId],
    affected_surfaces: &[PaintSurfaceId],
    params: SurfaceBlurParams,
    active_selection: &ActiveSelection,
) -> Result<CommandResult> {
    ensure!(
        params.radius_world.is_finite() && params.radius_world > 0.0,
        "surface blur radius must be positive and finite"
    );
    ensure!(
        feature.scene.is_some(),
        "surface blur requires an uploaded mesh"
    );
    let targets = validate_filter_targets(deps, source_surfaces, "surface blur")?;
    validate_affected_filter_targets(source_surfaces, affected_surfaces, "surface blur")?;
    let material_count = targets.material_sizes.len();
    let layout = pack_filter_materials(
        &targets.material_sizes,
        &targets.active_materials,
        deps.gpu.device().limits().max_texture_dimension_2d,
    )?;
    validate_filter_working_set("Surface Blur", layout.base_working_set_bytes, 0)?;

    let mut mutations = MutationLog::default();
    let mut metrics = RenderMetrics::default();
    let mut surface_edit = SurfaceEditContext::new(&mut *deps.surfaces, &mut mutations);
    for surface in source_surfaces {
        let stats = surface_edit.prepare_edit_surface_for_frame(deps.gpu, frame, *surface)?;
        record_surface_prepare_metrics(&mut metrics, &stats);
    }

    let source_atlas = create_rgba_texture(
        deps.gpu.device(),
        layout.size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "surface_blur_source_atlas",
    );
    let triangle_map = create_r32_uint_texture(
        deps.gpu.device(),
        layout.max_material_size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        "surface_blur_triangle_map",
    );
    let blur_output = create_rgba_texture(
        deps.gpu.device(),
        layout.max_material_size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        "surface_blur_output",
    );
    copy_surfaces_to_source_atlas(
        frame,
        &surface_edit,
        source_surfaces,
        material_count,
        &layout.placements,
        &source_atlas.texture,
        "surface blur",
    )?;
    let material_buffer = create_material_buffer(
        frame,
        deps.gpu.device(),
        material_count,
        &layout.placements,
        "surface_blur_materials",
    );
    let material_count_u32 = u32::try_from(material_count)
        .map_err(|_| anyhow!("surface blur material count exceeds u32"))?;
    let scene = feature
        .scene
        .as_ref()
        .expect("surface blur scene must exist after validation");

    for surface in affected_surfaces {
        let material_index = surface.material_index().as_usize();
        let size = targets.material_sizes[material_index];
        record_triangle_map(
            frame,
            &feature.pipelines,
            scene,
            material_index,
            size,
            &triangle_map.view,
        );

        let (selection_view, selection_enabled) = resolve_filter_selection_view(
            deps.selections,
            active_selection,
            material_index,
            &source_atlas.view,
        )?;
        let uniform = SurfaceBlurUniform {
            radius_world: params.radius_world,
            sigma_world: params.radius_world / 3.0,
            material_count: material_count_u32,
            current_material: u32::try_from(material_index)
                .map_err(|_| anyhow!("surface blur material index exceeds u32"))?,
            gutter_radius: UV_ISLAND_BLEED_RADIUS_PX,
            selection_enabled,
            _pad: [0; 2],
        };
        let uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "surface_blur_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[uniform],
        );
        let blur_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("surface_blur_bg"),
                layout: &feature.pipelines.surface_blur.gather_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_atlas.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&triangle_map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: scene.triangles.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: material_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&blur_output.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(selection_view),
                    },
                ],
            });
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("surface_blur_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.surface_blur.gather);
            pass.set_bind_group(0, &blur_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }

        let target = surface_edit
            .edit_surface_target(*surface)
            .ok_or_else(|| anyhow!("surface blur target is not GPU resident: {surface:?}"))?;
        let gutter_uniform = FilterGutterUniform {
            radius_world: params.radius_world,
            sigma_world: params.radius_world / 3.0,
            material_count: material_count_u32,
            current_material: uniform.current_material,
            gutter_radius: UV_ISLAND_BLEED_RADIUS_PX,
            _pad: [0; 3],
        };
        let gutter_uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "surface_blur_gutter_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[gutter_uniform],
        );
        let gutter_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("surface_blur_gutter_bg"),
                layout: &feature.pipelines.shared.gutter_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&blur_output.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&triangle_map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: material_buffer.as_entire_binding(),
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
                    label: Some("surface_blur_gutter_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.shared.gutter);
            pass.set_bind_group(0, &gutter_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
    }

    surface_edit.resolve_mask_edit_proxies_into_frame(
        deps.gpu,
        frame,
        affected_surfaces.iter().copied(),
    )?;
    surface_edit.record_committed_surface_damage(
        &PaintSurfaceSet::from_vec(affected_surfaces.to_vec()),
        None,
    );
    drop(surface_edit);
    Ok(CommandResult::new(mutations, metrics))
}
