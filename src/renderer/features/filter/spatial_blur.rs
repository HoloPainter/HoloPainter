use anyhow::{Result, anyhow, ensure};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        damage::UV_ISLAND_BLEED_RADIUS_PX, render_report::RenderMetrics,
        selection::ActiveSelection, stroke::PaintSurfaceSet, surface::PaintSurfaceId,
        surface_filter::SpatialBlurOrientation,
    },
    renderer::{
        document::surfaces::SurfaceEditContext,
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        report::{CommandResult, DiagnosticReadbackRequest, record_surface_prepare_metrics},
    },
};

use super::{
    command::SpatialBlurParams,
    feature::{FilterFeature, FilterFeatureDeps},
    packing::{FilterAtlasLayout, pack_filter_materials, validate_filter_working_set},
    shared::{
        FilterGutterUniform, SurfaceFilterScene, copy_surfaces_to_source_atlas,
        create_material_buffer, create_r32_uint_texture, create_rgba_texture, record_triangle_map,
        resolve_filter_selection_view, validate_affected_filter_targets, validate_filter_targets,
    },
};

pub(super) const MAX_SPATIAL_BLUR_SAMPLES: u32 = 1_048_576;
const MAX_VISITED_SAMPLES_PER_TEXEL: u32 = 4096;
const SPATIAL_SAMPLE_SIZE: u64 = 32;
const SPATIAL_COUNTERS_SIZE: u64 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpatialBlurUniform {
    grid_origin: [f32; 4],
    radius_world: f32,
    sigma_world: f32,
    normal_threshold_cos: f32,
    reference_radius_px: f32,
    material_count: u32,
    current_material: u32,
    stride_px: u32,
    sample_capacity: u32,
    bucket_mask: u32,
    cross_meshes: u32,
    orientation_mode: u32,
    gutter_radius: u32,
    current_size: [u32; 2],
    triangle_count: u32,
    selection_enabled: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpatialBlurPlan {
    target_stride_px: u32,
    actual_stride_px: u32,
    sample_capacity: u32,
    bucket_count: u32,
    active_triangle_count: u32,
    additional_working_set_bytes: u64,
}

pub(super) fn execute(
    feature: &mut FilterFeature,
    frame: &mut GpuFrame,
    deps: &mut FilterFeatureDeps<'_>,
    source_surfaces: &[PaintSurfaceId],
    affected_surfaces: &[PaintSurfaceId],
    params: SpatialBlurParams,
    active_selection: &ActiveSelection,
) -> Result<CommandResult> {
    ensure!(
        params.radius_world.is_finite() && params.radius_world > 0.0,
        "spatial blur radius must be positive and finite"
    );
    ensure!(
        params.reference_radius_px.is_finite()
            && (0.5..=64.0).contains(&params.reference_radius_px),
        "spatial blur reference radius must be between 0.5 and 64.0 average texels"
    );
    ensure!(
        params.normal_threshold_cos.is_finite()
            && (-1.0..=1.0).contains(&params.normal_threshold_cos),
        "spatial blur normal threshold must be finite and between -1 and 1"
    );
    let scene = feature
        .scene
        .as_ref()
        .ok_or_else(|| anyhow!("spatial blur requires an uploaded mesh"))?;
    let targets = validate_filter_targets(deps, source_surfaces, "spatial blur")?;
    validate_affected_filter_targets(source_surfaces, affected_surfaces, "spatial blur")?;
    let material_count = targets.material_sizes.len();
    let layout = pack_filter_materials(
        &targets.material_sizes,
        &targets.active_materials,
        deps.gpu.device().limits().max_texture_dimension_2d,
    )?;
    let plan = plan_spatial_blur(
        scene,
        &targets.material_sizes,
        &targets.active_materials,
        params.reference_radius_px,
    )?;
    validate_spatial_buffer_limits(deps.gpu.device().limits(), scene, &layout, &plan)?;
    validate_filter_working_set(
        "Spatial Blur",
        layout.base_working_set_bytes,
        plan.additional_working_set_bytes,
    )?;

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
        "spatial_blur_source_atlas",
    );
    let triangle_map = create_r32_uint_texture(
        deps.gpu.device(),
        layout.max_material_size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        "spatial_blur_triangle_map",
    );
    let blur_output = create_rgba_texture(
        deps.gpu.device(),
        layout.max_material_size,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        "spatial_blur_output",
    );
    copy_surfaces_to_source_atlas(
        frame,
        &surface_edit,
        source_surfaces,
        material_count,
        &layout.placements,
        &source_atlas.texture,
        "spatial blur",
    )?;
    let material_buffer = create_material_buffer(
        frame,
        deps.gpu.device(),
        material_count,
        &layout.placements,
        "spatial_blur_materials",
    );

    let sample_buffer = create_storage_buffer(
        deps.gpu.device(),
        u64::from(plan.sample_capacity) * SPATIAL_SAMPLE_SIZE,
        "spatial_blur_samples",
        false,
    );
    let bucket_heads = create_storage_buffer(
        deps.gpu.device(),
        u64::from(plan.bucket_count) * 4,
        "spatial_blur_bucket_heads",
        true,
    );
    let triangle_sample_counts = create_storage_buffer(
        deps.gpu.device(),
        u64::from(scene.triangle_count) * 4,
        "spatial_blur_triangle_sample_counts",
        true,
    );
    let counters = create_storage_buffer(
        deps.gpu.device(),
        SPATIAL_COUNTERS_SIZE,
        "spatial_blur_counters",
        true,
    );
    frame.encoder().clear_buffer(&bucket_heads, 0, None);
    frame
        .encoder()
        .clear_buffer(&triangle_sample_counts, 0, None);
    frame.encoder().clear_buffer(&counters, 0, None);

    let material_count_u32 = u32::try_from(material_count)
        .map_err(|_| anyhow!("spatial blur material count exceeds u32"))?;
    let grid_origin = [
        scene.scene_bounds_min[0] - params.radius_world,
        scene.scene_bounds_min[1] - params.radius_world,
        scene.scene_bounds_min[2] - params.radius_world,
        0.0,
    ];

    for surface in source_surfaces {
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
        let uniform = spatial_uniform(
            params,
            plan,
            grid_origin,
            material_count_u32,
            scene.triangle_count,
            material_index,
            size,
            0,
        )?;
        let uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "spatial_blur_build_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[uniform],
        );
        let count_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("spatial_blur_count_bg"),
                layout: &feature.pipelines.spatial_blur.count_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&triangle_map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: triangle_sample_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                ],
            });
        let grid_size = [
            size[0].div_ceil(plan.actual_stride_px),
            size[1].div_ceil(plan.actual_stride_px),
        ];
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("spatial_blur_count_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.spatial_blur.count);
            pass.set_bind_group(0, &count_bind_group, &[]);
            pass.dispatch_workgroups(grid_size[0].div_ceil(8), grid_size[1].div_ceil(8), 1);
        }

        let build_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("spatial_blur_build_bg"),
                layout: &feature.pipelines.spatial_blur.build_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&triangle_map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: scene.triangles.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: triangle_sample_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: sample_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: bucket_heads.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: counters.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                ],
            });
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("spatial_blur_build_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.spatial_blur.build);
            pass.set_bind_group(0, &build_bind_group, &[]);
            pass.dispatch_workgroups(grid_size[0].div_ceil(8), grid_size[1].div_ceil(8), 1);
        }
    }

    let fallback_uniform = spatial_uniform(
        params,
        plan,
        grid_origin,
        material_count_u32,
        scene.triangle_count,
        0,
        [1, 1],
        0,
    )?;
    let fallback_uniform_buffer = frame.create_buffer_from_slice(
        deps.gpu.device(),
        "spatial_blur_fallback_uniform",
        wgpu::BufferUsages::UNIFORM,
        &[fallback_uniform],
    );
    let fallback_bind_group = deps
        .gpu
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spatial_blur_fallback_bg"),
            layout: &feature.pipelines.spatial_blur.fallback_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scene.triangles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: triangle_sample_counts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sample_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: bucket_heads.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: counters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: fallback_uniform_buffer.as_entire_binding(),
                },
            ],
        });
    {
        let mut pass = frame
            .encoder()
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("spatial_blur_fallback_pass"),
                timestamp_writes: None,
            });
        pass.set_pipeline(&feature.pipelines.spatial_blur.fallback);
        pass.set_bind_group(0, &fallback_bind_group, &[]);
        pass.dispatch_workgroups(scene.triangle_count.div_ceil(64), 1, 1);
    }

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
        let uniform = spatial_uniform(
            params,
            plan,
            grid_origin,
            material_count_u32,
            scene.triangle_count,
            material_index,
            size,
            selection_enabled,
        )?;
        let uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "spatial_blur_gather_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[uniform],
        );
        let gather_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("spatial_blur_gather_bg"),
                layout: &feature.pipelines.spatial_blur.gather_bgl,
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
                        resource: sample_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: bucket_heads.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: counters.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&blur_output.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(selection_view),
                    },
                ],
            });
        {
            let mut pass = frame
                .encoder()
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("spatial_blur_gather_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.spatial_blur.gather);
            pass.set_bind_group(0, &gather_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }

        let target = surface_edit
            .edit_surface_target(*surface)
            .ok_or_else(|| anyhow!("spatial blur target is not GPU resident: {surface:?}"))?;
        let gutter_uniform = FilterGutterUniform {
            radius_world: params.radius_world,
            sigma_world: params.radius_world / 3.0,
            material_count: material_count_u32,
            current_material: u32::try_from(material_index)
                .map_err(|_| anyhow!("spatial blur material index exceeds u32"))?,
            gutter_radius: UV_ISLAND_BLEED_RADIUS_PX,
            _pad: [0; 3],
        };
        let gutter_uniform_buffer = frame.create_buffer_from_slice(
            deps.gpu.device(),
            "spatial_blur_gutter_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[gutter_uniform],
        );
        let gutter_bind_group = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("spatial_blur_gutter_bg"),
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
                    label: Some("spatial_blur_gutter_pass"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&feature.pipelines.shared.gutter);
            pass.set_bind_group(0, &gutter_bind_group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
    }

    let diagnostic_buffer = deps.gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("spatial_blur_diagnostic_readback"),
        size: SPATIAL_COUNTERS_SIZE,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    frame.encoder().copy_buffer_to_buffer(
        &counters,
        0,
        &diagnostic_buffer,
        0,
        SPATIAL_COUNTERS_SIZE,
    );

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
    Ok(
        CommandResult::new(mutations, metrics).with_diagnostic_readback(
            DiagnosticReadbackRequest::SpatialBlur {
                buffer: diagnostic_buffer,
                target_stride_px: plan.target_stride_px,
                actual_stride_px: plan.actual_stride_px,
            },
        ),
    )
}

fn spatial_uniform(
    params: SpatialBlurParams,
    plan: SpatialBlurPlan,
    grid_origin: [f32; 4],
    material_count: u32,
    triangle_count: u32,
    material_index: usize,
    current_size: [u32; 2],
    selection_enabled: u32,
) -> Result<SpatialBlurUniform> {
    Ok(SpatialBlurUniform {
        grid_origin,
        radius_world: params.radius_world,
        sigma_world: params.radius_world / 3.0,
        normal_threshold_cos: params.normal_threshold_cos,
        reference_radius_px: params.reference_radius_px,
        material_count,
        current_material: u32::try_from(material_index)
            .map_err(|_| anyhow!("spatial blur material index exceeds u32"))?,
        stride_px: plan.actual_stride_px,
        sample_capacity: plan.sample_capacity,
        bucket_mask: plan.bucket_count - 1,
        cross_meshes: u32::from(params.cross_meshes),
        orientation_mode: match params.orientation {
            SpatialBlurOrientation::Ignore => 0,
            SpatialBlurOrientation::SimilarNormals => 1,
        },
        gutter_radius: UV_ISLAND_BLEED_RADIUS_PX,
        current_size,
        triangle_count,
        selection_enabled,
    })
}

fn plan_spatial_blur(
    scene: &SurfaceFilterScene,
    material_sizes: &[[u32; 2]],
    active_materials: &[bool],
    reference_radius_px: f32,
) -> Result<SpatialBlurPlan> {
    let active_triangle_count = u32::try_from(
        scene
            .triangle_material_indices
            .iter()
            .filter(|material_index| {
                active_materials
                    .get(**material_index)
                    .copied()
                    .unwrap_or(false)
            })
            .count(),
    )
    .map_err(|_| anyhow!("spatial blur active triangle count exceeds u32"))?;
    ensure!(
        active_triangle_count > 0,
        "spatial blur has no active surface triangles"
    );
    ensure!(
        active_triangle_count <= MAX_SPATIAL_BLUR_SAMPLES,
        "spatial blur active triangle count {} exceeds the sample limit {}",
        active_triangle_count,
        MAX_SPATIAL_BLUR_SAMPLES
    );
    let target_stride_px = (reference_radius_px / 3.0).floor().max(1.0) as u32;
    let (actual_stride_px, sample_capacity) = choose_sample_stride(
        material_sizes,
        active_materials,
        active_triangle_count,
        target_stride_px,
    )?;
    let bucket_count = sample_capacity
        .checked_mul(4)
        .ok_or_else(|| anyhow!("spatial blur hash bucket count overflow"))?
        .next_power_of_two();
    let sample_bytes = u64::from(sample_capacity) * SPATIAL_SAMPLE_SIZE;
    let bucket_bytes = u64::from(bucket_count) * 4;
    let triangle_counter_bytes = u64::from(scene.triangle_count) * 4;
    let additional_working_set_bytes = sample_bytes
        .checked_add(bucket_bytes)
        .and_then(|bytes| bytes.checked_add(triangle_counter_bytes))
        .and_then(|bytes| bytes.checked_add(SPATIAL_COUNTERS_SIZE * 2))
        .ok_or_else(|| anyhow!("spatial blur working-set estimate overflow"))?;
    Ok(SpatialBlurPlan {
        target_stride_px,
        actual_stride_px,
        sample_capacity,
        bucket_count,
        active_triangle_count,
        additional_working_set_bytes,
    })
}

fn choose_sample_stride(
    material_sizes: &[[u32; 2]],
    active_materials: &[bool],
    active_triangle_count: u32,
    target_stride_px: u32,
) -> Result<(u32, u32)> {
    ensure!(
        target_stride_px > 0,
        "spatial blur sample stride must be positive"
    );
    let maximum_material_dimension = material_sizes
        .iter()
        .zip(active_materials.iter().copied())
        .filter(|(_, active)| *active)
        .map(|(size, _)| size[0].max(size[1]))
        .max()
        .unwrap_or(1);
    let mut lower = target_stride_px;
    let mut upper = target_stride_px.max(maximum_material_dimension);
    let minimum_capacity = candidate_sample_capacity(
        material_sizes,
        active_materials,
        active_triangle_count,
        upper,
    )?;
    ensure!(
        minimum_capacity <= MAX_SPATIAL_BLUR_SAMPLES,
        "spatial blur requires at least {minimum_capacity} samples, exceeding the sample limit {MAX_SPATIAL_BLUR_SAMPLES}"
    );

    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let candidate = candidate_sample_capacity(
            material_sizes,
            active_materials,
            active_triangle_count,
            middle,
        )?;
        if candidate <= MAX_SPATIAL_BLUR_SAMPLES {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    let capacity = candidate_sample_capacity(
        material_sizes,
        active_materials,
        active_triangle_count,
        lower,
    )?;
    Ok((lower, capacity))
}

fn candidate_sample_capacity(
    material_sizes: &[[u32; 2]],
    active_materials: &[bool],
    active_triangle_count: u32,
    stride_px: u32,
) -> Result<u32> {
    let grid_samples = material_sizes
        .iter()
        .zip(active_materials.iter().copied())
        .filter(|(_, active)| *active)
        .try_fold(0u64, |total, (size, _)| {
            let material_samples = u64::from(size[0].div_ceil(stride_px))
                .checked_mul(u64::from(size[1].div_ceil(stride_px)))
                .ok_or_else(|| anyhow!("spatial blur candidate sample count overflow"))?;
            total
                .checked_add(material_samples)
                .ok_or_else(|| anyhow!("spatial blur candidate sample count overflow"))
        })?;
    let total = grid_samples
        .checked_add(u64::from(active_triangle_count))
        .ok_or_else(|| anyhow!("spatial blur candidate sample count overflow"))?;
    u32::try_from(total).map_err(|_| anyhow!("spatial blur candidate sample count exceeds u32"))
}

fn validate_spatial_buffer_limits(
    limits: wgpu::Limits,
    scene: &SurfaceFilterScene,
    layout: &FilterAtlasLayout,
    plan: &SpatialBlurPlan,
) -> Result<()> {
    let maximum_storage_binding = u64::from(limits.max_storage_buffer_binding_size);
    let buffers = [
        (
            "sample",
            u64::from(plan.sample_capacity) * SPATIAL_SAMPLE_SIZE,
        ),
        ("hash bucket", u64::from(plan.bucket_count) * 4),
        ("triangle counter", u64::from(scene.triangle_count) * 4),
        ("counter", SPATIAL_COUNTERS_SIZE),
    ];
    for (name, size) in buffers {
        ensure!(
            size <= limits.max_buffer_size,
            "spatial blur {name} buffer requires {size} bytes, exceeding the device buffer limit {}",
            limits.max_buffer_size
        );
        ensure!(
            size <= maximum_storage_binding,
            "spatial blur {name} buffer requires {size} bytes, exceeding the device storage binding limit {maximum_storage_binding}"
        );
    }
    ensure!(
        layout.max_material_size[0].div_ceil(8) <= limits.max_compute_workgroups_per_dimension
            && layout.max_material_size[1].div_ceil(8)
                <= limits.max_compute_workgroups_per_dimension,
        "spatial blur material dimensions exceed the device compute dispatch limit"
    );
    ensure!(
        scene.triangle_count.div_ceil(64) <= limits.max_compute_workgroups_per_dimension,
        "spatial blur triangle count exceeds the device compute dispatch limit"
    );
    ensure!(
        MAX_VISITED_SAMPLES_PER_TEXEL > 0,
        "spatial blur traversal limit must be positive"
    );
    Ok(())
}

fn create_storage_buffer(
    device: &wgpu::Device,
    size: u64,
    label: &'static str,
    clearable: bool,
) -> wgpu::Buffer {
    let mut usage = wgpu::BufferUsages::STORAGE;
    if clearable {
        usage |= wgpu::BufferUsages::COPY_DST;
    }
    if label == "spatial_blur_counters" {
        usage |= wgpu::BufferUsages::COPY_SRC;
    }
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(wgpu::COPY_BUFFER_ALIGNMENT),
        usage,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use eframe::egui_wgpu::wgpu;

    use super::{
        MAX_SPATIAL_BLUR_SAMPLES, SpatialBlurPlan, candidate_sample_capacity, choose_sample_stride,
    };

    #[test]
    fn candidate_capacity_includes_triangle_fallback_reserve() {
        let capacity = candidate_sample_capacity(&[[16, 16]], &[true], 3, 4).unwrap();
        assert_eq!(capacity, 19);
    }

    #[test]
    fn sample_stride_increases_to_fit_the_sample_limit() {
        let (stride, capacity) = choose_sample_stride(&[[4096, 4096]], &[true], 1, 1).unwrap();

        assert!(stride > 1);
        assert!(capacity <= MAX_SPATIAL_BLUR_SAMPLES);
        assert!(
            candidate_sample_capacity(&[[4096, 4096]], &[true], 1, stride - 1).unwrap()
                > MAX_SPATIAL_BLUR_SAMPLES
        );
    }

    #[test]
    fn sample_stride_rejects_an_impossible_fallback_reserve() {
        let error =
            choose_sample_stride(&[[1, 1]], &[true], MAX_SPATIAL_BLUR_SAMPLES, 1).unwrap_err();

        assert!(error.to_string().contains("exceeding the sample limit"));
    }

    #[test]
    fn constants_match_initial_spatial_blur_contract() {
        assert_eq!(MAX_SPATIAL_BLUR_SAMPLES, 1_048_576);
        let _ = std::mem::size_of::<SpatialBlurPlan>();
        let _ = wgpu::COPY_BUFFER_ALIGNMENT;
    }
}
