use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        mask::{
            ProjectionMaskSource, ViewportPolygonProjectionMaskSource, ViewportProjection,
            ViewportRectProjectionMaskSource,
        },
        math::gl_to_wgpu_depth,
        viewport_visibility::ViewportSceneVisibility,
    },
    renderer::{
        engine::gpu_state::RendererGpuState,
        features::{
            apply::pipelines::{
                PaintApplyPipelines, ViewportPolygonProjectionMaskUniform,
                ViewportRectProjectionMaskUniform,
            },
            brush::transient as stroke_transient,
        },
        gpu::{clear_rgba_target, frame::GpuFrame, texture::create_mask_texture},
        mask::shape::rasterize_polygon_rect_r8,
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
    },
};

use crate::renderer::document::scene::{SceneDepthSlot, SceneResources};

pub(crate) struct ViewportProjectionMaskGpuDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) pipelines: &'a PaintApplyPipelines,
    pub(crate) scene: &'a mut SceneResources,
    pub(crate) scratch: &'a mut TransientTextures,
    pub(crate) scene_capture: &'a mut SceneCapture,
    pub(crate) scene_capture_pipelines: &'a SceneCapturePipelines,
}

pub(crate) struct PreparedViewportPolygonCoverage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl PreparedViewportPolygonCoverage {
    pub(crate) fn retain_until_submit(self, frame: &mut GpuFrame) {
        frame.retain_texture_until_submit(self.texture, self.view);
    }
}

pub(crate) fn prepare_viewport_polygon_coverage(
    frame: &mut GpuFrame,
    device: &wgpu::Device,
    polygon: &ViewportPolygonProjectionMaskSource,
) -> Option<PreparedViewportPolygonCoverage> {
    let size = [
        polygon.viewport_size[0].max(1),
        polygon.viewport_size[1].max(1),
    ];
    let (rect, r8) = rasterize_polygon_rect_r8(&polygon.points_px, size)?;
    let (texture, view) = create_mask_texture(device, size, "viewport_polygon_coverage");
    clear_rgba_target(
        frame.encoder(),
        &view,
        [0.0, 0.0, 0.0, 0.0],
        "clear_viewport_polygon_coverage",
    );
    frame.write_texture_r8(device, &texture, rect.origin, rect.size, &r8);
    Some(PreparedViewportPolygonCoverage {
        texture,
        view,
        size,
    })
}

pub(crate) fn write_viewport_projection_mask_to_stroke_uv(
    frame: &mut GpuFrame,
    deps: &mut ViewportProjectionMaskGpuDeps<'_>,
    material_index: usize,
    projection: &ProjectionMaskSource,
    texture_size: [u32; 2],
    polygon_coverage: Option<&PreparedViewportPolygonCoverage>,
) -> Option<()> {
    match projection {
        ProjectionMaskSource::ViewportRect(rect) => {
            write_viewport_rect_projection_mask_to_stroke_uv(
                frame,
                deps,
                material_index,
                rect,
                texture_size,
            )
        }
        ProjectionMaskSource::ViewportPolygon(polygon) => {
            write_viewport_polygon_projection_mask_to_stroke_uv(
                frame,
                deps,
                material_index,
                polygon,
                texture_size,
                polygon_coverage?,
            )
        }
    }
}

pub(crate) fn write_viewport_rect_projection_mask_to_stroke_uv(
    frame: &mut GpuFrame,
    deps: &mut ViewportProjectionMaskGpuDeps<'_>,
    material_index: usize,
    rect: &ViewportRectProjectionMaskSource,
    texture_size: [u32; 2],
) -> Option<()> {
    if rect
        .material_index
        .is_some_and(|index| index != material_index)
        || rect.projections.is_empty()
    {
        return None;
    }

    prepare_projection_mask_target(
        frame,
        deps,
        material_index,
        texture_size,
        "clear_viewport_rect_projection_mask",
    )?;

    for projection in &rect.projections {
        let depth_slot = prepare_projection_depth(
            frame,
            deps,
            rect.viewport_size,
            projection,
            &rect.scene_visibility,
        );
        let uniform = ViewportRectProjectionMaskUniform {
            view_proj: (gl_to_wgpu_depth() * projection.view_proj).to_cols_array_2d(),
            rect_min_max: [rect.min_px.x, rect.min_px.y, rect.max_px.x, rect.max_px.y],
            viewport_texture_size: [
                rect.viewport_size[0] as f32,
                rect.viewport_size[1] as f32,
                texture_size[0] as f32,
                texture_size[1] as f32,
            ],
        };
        frame.write_buffer_pod(
            deps.gpu.device(),
            &deps.pipelines.viewport_rect_projection_mask_uniform,
            0,
            &uniform,
        );
        let bg = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewport_rect_projection_mask_bg"),
                layout: &deps.pipelines.viewport_rect_projection_mask_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: deps
                            .pipelines
                            .viewport_rect_projection_mask_uniform
                            .as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            deps.scene_capture.depth_color_view(depth_slot),
                        ),
                    },
                ],
            });
        draw_projection_mask(
            frame,
            deps,
            material_index,
            &deps.pipelines.viewport_rect_projection_mask_pipeline,
            &bg,
            "viewport_rect_projection_mask_pass",
            &rect.scene_visibility,
        )?;
    }
    Some(())
}

pub(crate) fn write_viewport_polygon_projection_mask_to_stroke_uv(
    frame: &mut GpuFrame,
    deps: &mut ViewportProjectionMaskGpuDeps<'_>,
    material_index: usize,
    polygon: &ViewportPolygonProjectionMaskSource,
    texture_size: [u32; 2],
    coverage: &PreparedViewportPolygonCoverage,
) -> Option<()> {
    if polygon
        .material_index
        .is_some_and(|index| index != material_index)
        || polygon.points_px.len() < 3
        || polygon.projections.is_empty()
    {
        return None;
    }

    prepare_projection_mask_target(
        frame,
        deps,
        material_index,
        texture_size,
        "clear_viewport_polygon_projection_mask",
    )?;

    let coverage_size = coverage.size;
    debug_assert_eq!(
        coverage_size,
        [
            polygon.viewport_size[0].max(1),
            polygon.viewport_size[1].max(1),
        ]
    );

    for projection in &polygon.projections {
        let depth_slot = prepare_projection_depth(
            frame,
            deps,
            polygon.viewport_size,
            projection,
            &polygon.scene_visibility,
        );
        let uniform = ViewportPolygonProjectionMaskUniform {
            view_proj: (gl_to_wgpu_depth() * projection.view_proj).to_cols_array_2d(),
            viewport_texture_size: [
                coverage_size[0] as f32,
                coverage_size[1] as f32,
                texture_size[0] as f32,
                texture_size[1] as f32,
            ],
        };
        frame.write_buffer_pod(
            deps.gpu.device(),
            &deps.pipelines.viewport_polygon_projection_mask_uniform,
            0,
            &uniform,
        );
        let bg = deps
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewport_polygon_projection_mask_bg"),
                layout: &deps.pipelines.viewport_polygon_projection_mask_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: deps
                            .pipelines
                            .viewport_polygon_projection_mask_uniform
                            .as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            deps.scene_capture.depth_color_view(depth_slot),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&coverage.view),
                    },
                ],
            });
        draw_projection_mask(
            frame,
            deps,
            material_index,
            &deps.pipelines.viewport_polygon_projection_mask_pipeline,
            &bg,
            "viewport_polygon_projection_mask_pass",
            &polygon.scene_visibility,
        )?;
    }
    Some(())
}

fn prepare_projection_mask_target(
    frame: &mut GpuFrame,
    deps: &mut ViewportProjectionMaskGpuDeps<'_>,
    material_index: usize,
    texture_size: [u32; 2],
    clear_label: &'static str,
) -> Option<()> {
    stroke_transient::ensure_material_stroke_uv(
        deps.scratch,
        deps.gpu.device(),
        frame.encoder(),
        material_index,
        texture_size,
    );
    let stroke_view = stroke_transient::material_stroke_uv_view(deps.scratch, material_index)?;
    clear_rgba_target(
        frame.encoder(),
        stroke_view,
        [0.0, 0.0, 0.0, 0.0],
        clear_label,
    );
    Some(())
}

fn prepare_projection_depth(
    frame: &mut GpuFrame,
    deps: &mut ViewportProjectionMaskGpuDeps<'_>,
    viewport_size: [u32; 2],
    projection: &ViewportProjection,
    scene_visibility: &ViewportSceneVisibility,
) -> SceneDepthSlot {
    let depth_slot = SceneDepthSlot::for_surface_projection(projection.id);
    let visible_index_ranges = deps
        .scene
        .mesh()
        .map(|mesh| mesh.visible_index_ranges(scene_visibility))
        .unwrap_or_default();
    deps.scene.ensure_scene_depth_for_index_ranges(
        deps.gpu,
        deps.scene_capture,
        &deps.scene_capture_pipelines.viewport_prepass_bgl,
        &deps.scene_capture_pipelines.viewport_prepass_pipeline,
        frame,
        depth_slot,
        viewport_size,
        gl_to_wgpu_depth() * projection.view_proj,
        &visible_index_ranges,
    );
    depth_slot
}

fn draw_projection_mask(
    frame: &mut GpuFrame,
    deps: &ViewportProjectionMaskGpuDeps<'_>,
    material_index: usize,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    pass_label: &'static str,
    scene_visibility: &ViewportSceneVisibility,
) -> Option<()> {
    let stroke_view = stroke_transient::material_stroke_uv_view(deps.scratch, material_index)?;
    let mesh = deps.scene.mesh()?;
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(pass_label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: stroke_view,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.set_vertex_buffer(0, mesh.vertex.slice(..));
    pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
    for sm in mesh
        .sub_meshes_for_material(material_index)
        .filter(|sm| sm.is_visible(scene_visibility))
    {
        pass.draw_indexed(sm.index_start..(sm.index_start + sm.index_count), 0, 0..1);
    }
    Some(())
}
