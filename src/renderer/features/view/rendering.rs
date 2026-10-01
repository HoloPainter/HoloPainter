use eframe::egui_wgpu::wgpu;
use glam::Mat4;

use crate::{
    core::{
        camera::OrbitCamera, math::gl_to_wgpu_depth, selection::ActiveSelection,
        uv_view::UvViewTransform, viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility, wireframe::WireframeStyle,
    },
    renderer::{
        decal_image::DecalImageCache,
        document::{
            GpuDocument,
            scene::{SceneDepthSlot, SceneResources},
            selection::SelectionMasks,
        },
        engine::gpu_state::RendererGpuState,
        features::{
            composite::CompositeViewResources,
            view::{
                BrushOverlayRequest, DecalOverlayRequest, MirrorPlaneOverlayRequest,
                SelectionOverlayRequest, SurfaceBrushOverlayRequest, UvBrushOverlayRequest,
                types::{
                    DecalOverlayUniform, MirrorPlaneOverlayUniform, SelectionOverlayUniform,
                    SurfaceBrushOverlayUniform, UvBrushOverlayUniform, UvSelectionOverlayUniform,
                    UvViewTransformUniform, WireframeUniform,
                },
            },
        },
        gpu::{clear_rgba_target, frame::GpuFrame},
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
    },
};

use super::deps::ViewStrokeDeps;
use super::resources::DecalDepthSource;
use super::{ViewFeature, transient as view_transient};

impl ViewFeature {
    pub(crate) fn render_viewport(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &mut RendererGpuState,
        scratch: &mut TransientTextures,
        scene_capture: &mut SceneCapture,
        scene_capture_pipelines: &SceneCapturePipelines,
        document: &mut GpuDocument,
        composite_resources: CompositeViewResources<'_>,
        camera: OrbitCamera,
        view_proj: Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        brush_overlay_request: Option<BrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        mirror_plane_overlay_request: Option<MirrorPlaneOverlayRequest>,
        decal_overlay_request: Option<DecalOverlayRequest>,
        decal_images: &mut DecalImageCache,
        scene_visibility: ViewportSceneVisibility,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
        shading: ViewportShading,
    ) {
        let Self {
            pipelines,
            targets,
            decal_resources,
        } = self;
        let mut view_deps = ViewStrokeDeps {
            gpu: gpu_state,
            view_pipelines: pipelines,
            scene_capture_pipelines,
            composite_outputs: composite_resources.outputs(),
            materials: &document.materials,
            scene: &mut document.scene,
            scratch,
            view_targets: targets,
            scene_capture,
        };

        let visible_index_ranges = view_deps
            .scene
            .mesh()
            .map(|mesh| mesh.visible_index_ranges(&scene_visibility))
            .unwrap_or_default();
        view_deps.scene.ensure_scene_depth_for_index_ranges(
            view_deps.gpu,
            view_deps.scene_capture,
            &view_deps.scene_capture_pipelines.viewport_prepass_bgl,
            &view_deps.scene_capture_pipelines.viewport_prepass_pipeline,
            frame,
            SceneDepthSlot::Main,
            viewport_size,
            gl_to_wgpu_depth() * view_proj,
            &visible_index_ranges,
        );
        let brush_overlay_ready = match brush_overlay_request.as_ref() {
            Some(BrushOverlayRequest::Surface(request)) => {
                render_surface_brush_overlay(frame, &mut view_deps, request)
            }
            None => false,
        };
        view_deps.scene_capture.render_viewport_3d(
            view_deps.gpu,
            view_deps.scene_capture_pipelines,
            view_deps.scene,
            view_deps.materials,
            view_deps.view_targets,
            view_deps.composite_outputs,
            frame,
            view_proj,
            camera_world,
            viewport_size,
            &camera,
            &scene_visibility,
            background_color,
            shading,
        );
        if show_wireframe {
            view_deps.scene_capture.render_viewport_wireframe(
                view_deps.gpu,
                view_deps.scene_capture_pipelines,
                view_deps.scene,
                view_deps.view_targets,
                frame,
                &scene_visibility,
                wireframe_style,
            );
        }
        if let Some(request) = decal_overlay_request {
            render_decal_image_overlay(
                frame,
                view_deps.gpu,
                view_deps.scene,
                view_deps.view_pipelines,
                decal_resources,
                decal_images,
                view_deps.view_targets,
                view_deps.scene_capture,
                view_deps.scene_capture_pipelines,
                view_proj,
                viewport_size,
                &scene_visibility,
                request,
            );
        }
        if view_deps.scene.has_mesh()
            && let Some(request) = mirror_plane_overlay_request
        {
            render_mirror_plane_overlay(
                frame,
                view_deps.gpu,
                view_deps.view_pipelines,
                view_deps.view_targets,
                view_deps.scene_capture,
                view_proj,
                request,
            );
        }
        if let Some(request) = selection_overlay_request.as_ref() {
            render_viewport_selection_overlay(
                frame,
                view_deps.gpu,
                view_deps.scene,
                &document.selections,
                view_deps.view_pipelines,
                view_deps.view_targets,
                view_deps.scene_capture,
                &request.active_selection,
                viewport_size,
                request.phase,
                &scene_visibility,
            );
        }
        if brush_overlay_ready {
            let overlay_view = view_transient::viewport_brush_overlay_view(view_deps.scratch)
                .expect("viewport brush overlay texture is initialized");
            composite_viewport_brush_overlay(
                frame,
                view_deps.gpu,
                view_deps.view_pipelines,
                view_deps.view_targets,
                overlay_view,
            );
        }
    }

    pub(crate) fn render_uv_view(
        &mut self,
        frame: &mut GpuFrame,
        gpu_state: &mut RendererGpuState,
        _scratch: &mut TransientTextures,
        composite_resources: CompositeViewResources<'_>,
        document: &mut GpuDocument,
        material_index: usize,
        uv_view_size: [u32; 2],
        transform: UvViewTransform,
        brush_overlay_request: Option<UvBrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
    ) {
        let gpu = gpu_state;
        let Self {
            pipelines,
            targets: view_targets,
            ..
        } = self;
        view_targets.ensure_uv_view_size(gpu.device(), uv_view_size);
        let transform = transform.normalized();
        let canvas_size = document
            .materials
            .texture_size(material_index)
            .unwrap_or([1, 1]);
        let transform_uniform =
            uv_view_transform_uniform(uv_view_size, canvas_size, transform, background_color);
        frame.write_buffer_pod(
            gpu.device(),
            &pipelines.uv_view_transform_uniform,
            0,
            &transform_uniform,
        );
        let composite_view = (material_index < document.materials.material_count())
            .then(|| composite_resources.outputs().output_view(material_index))
            .flatten();
        let Some(composite_view) = composite_view else {
            clear_rgba_target(
                frame.encoder(),
                &view_targets.uv_view_output_view,
                rgba_background(background_color),
                "clear_uv_view_without_composite",
            );
            return;
        };
        let present_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_view_present_bg"),
            layout: &pipelines.uv_view_present_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pipelines.uv_view_transform_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(composite_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(gpu.paint_sampler()),
                },
            ],
        });
        {
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("uv_view_present_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_targets.uv_view_output_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu_background(background_color)),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            pass.set_pipeline(&pipelines.uv_view_present_pipeline);
            pass.set_bind_group(0, &present_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        if let Some(request) = selection_overlay_request.as_ref() {
            render_uv_selection_overlay(
                gpu,
                frame,
                pipelines,
                &document.selections,
                &view_targets.uv_view_output_view,
                material_index,
                uv_view_size,
                transform,
                request,
            );
        }

        let Some(mesh) = document.scene.mesh() else {
            return;
        };
        if show_wireframe {
            let uniform = WireframeUniform {
                color_opacity: [
                    wireframe_style.color[0],
                    wireframe_style.color[1],
                    wireframe_style.color[2],
                    wireframe_style.opacity,
                ],
            };
            frame.write_buffer_pod(gpu.device(), &pipelines.uv_wireframe_uniform, 0, &uniform);
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("uv_wireframe_overlay_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_targets.uv_view_output_view,
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
            pass.set_pipeline(&pipelines.uv_wireframe_pipeline);
            pass.set_bind_group(0, &pipelines.uv_wireframe_bg, &[]);
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.wire_index.slice(..), wgpu::IndexFormat::Uint32);
            for sm in &mesh.sub_meshes {
                if sm.material_index != material_index || sm.wire_count == 0 {
                    continue;
                }
                pass.draw_indexed(sm.wire_start..(sm.wire_start + sm.wire_count), 0, 0..1);
            }
        }

        if let Some(request) = brush_overlay_request {
            let uniform = UvBrushOverlayUniform {
                view_size: [request.view_size[0] as f32, request.view_size[1] as f32],
                canvas_size: [canvas_size[0] as f32, canvas_size[1] as f32],
                center_px: request.center_px,
                _pad0: [0.0; 2],
                brush_params: [request.radius_px.max(0.5), 1.5, 0.95, transform.zoom],
                center_transform: [
                    transform.center_uv.x,
                    transform.center_uv.y,
                    transform.rotation_radians.sin(),
                    transform.rotation_radians.cos(),
                ],
            };
            frame.write_buffer_pod(
                gpu.device(),
                &pipelines.brush_overlay_uv_view_uniform,
                0,
                &uniform,
            );
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("brush_overlay_uv_view_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_targets.uv_view_output_view,
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
            pass.set_pipeline(&pipelines.brush_overlay_uv_view_pipeline);
            pass.set_bind_group(0, &pipelines.brush_overlay_uv_view_bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn render_decal_image_overlay(
    frame: &mut GpuFrame,
    gpu: &mut RendererGpuState,
    scene: &mut SceneResources,
    pipelines: &crate::renderer::features::view::pipelines::ViewPipelines,
    decal_resources: &mut crate::renderer::features::view::resources::DecalTextureResources,
    decal_images: &mut DecalImageCache,
    targets: &crate::renderer::features::view::targets::ViewTargets,
    scene_capture: &mut SceneCapture,
    scene_capture_pipelines: &SceneCapturePipelines,
    view_proj: Mat4,
    viewport_size: [u32; 2],
    viewport_scene_visibility: &ViewportSceneVisibility,
    request: DecalOverlayRequest,
) {
    if !scene.has_mesh() || !request.projection.is_valid() || !request.opacity.is_finite() {
        return;
    }
    let opacity = request.opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    let Some(projector_view_proj) = request.projection.projector_view_projection() else {
        return;
    };
    let Some(visibility_view_proj) = request.projection.visibility_view_projection() else {
        return;
    };
    let mesh_generation = scene.mesh_generation();
    let Some(mesh) = scene.mesh() else {
        return;
    };
    let Some(preview_geometry) = decal_resources.preview_geometry(
        &mesh.source_mesh,
        mesh_generation,
        request.projection,
        view_proj,
        viewport_size,
    ) else {
        return;
    };
    let visible_index_ranges = mesh.visible_index_range_intersections(
        &preview_geometry.index_ranges,
        &request.scene_visibility,
    );
    if visible_index_ranges.is_empty() {
        return;
    }
    let projector_view_proj = gl_to_wgpu_depth() * projector_view_proj;
    let visibility_view_proj = gl_to_wgpu_depth() * visibility_view_proj;
    // View Projection uses the viewport camera for visibility, so the Main depth
    // generated immediately before this overlay is already the exact occlusion
    // source. Surface decals still need projector-space depth.
    let reuse_main_depth = request.projection.is_view_projection()
        && request.scene_visibility == *viewport_scene_visibility;
    let (depth_slot, depth_source, depth_generation, sample_visibility_depth) = if reuse_main_depth
    {
        (SceneDepthSlot::Main, DecalDepthSource::Main, 0, true)
    } else {
        let depth_view_proj = if request.projection.is_view_projection() {
            visibility_view_proj
        } else {
            projector_view_proj
        };
        scene.ensure_scene_depth_for_index_ranges(
            gpu,
            scene_capture,
            &scene_capture_pipelines.viewport_prepass_bgl,
            &scene_capture_pipelines.viewport_prepass_pipeline,
            frame,
            SceneDepthSlot::Decal,
            viewport_size,
            depth_view_proj,
            &visible_index_ranges,
        );
        (
            SceneDepthSlot::Decal,
            DecalDepthSource::Decal,
            scene_capture.decal_depth_generation(),
            request.projection.is_view_projection(),
        )
    };
    let depth_size = viewport_size;

    let transform = match request.projection {
        crate::core::decal::DecalProjection::Surface(transform) => Some(transform),
        crate::core::decal::DecalProjection::ViewProjection { .. } => None,
    };
    let surface_transform = transform.unwrap_or(crate::core::decal::DecalTransform {
        center_world: glam::Vec3::ZERO,
        axis_x_world: glam::Vec3::X,
        axis_y_world: glam::Vec3::Y,
        normal_world: glam::Vec3::Z,
        size_world: glam::Vec2::ONE,
        projection_depth_world: 1.0,
    });
    let half_size = surface_transform.size_world * 0.5;
    let front_projection_depth = surface_transform.front_projection_depth_world();
    let uniform = DecalOverlayUniform {
        view_proj: (gl_to_wgpu_depth() * view_proj).to_cols_array_2d(),
        projector_view_proj: projector_view_proj.to_cols_array_2d(),
        visibility_view_proj: visibility_view_proj.to_cols_array_2d(),
        center_depth: [
            surface_transform.center_world.x,
            surface_transform.center_world.y,
            surface_transform.center_world.z,
            surface_transform.projection_depth_world,
        ],
        axis_x_half_width: [
            surface_transform.axis_x_world.x,
            surface_transform.axis_x_world.y,
            surface_transform.axis_x_world.z,
            half_size.x,
        ],
        axis_y_half_height: [
            surface_transform.axis_y_world.x,
            surface_transform.axis_y_world.y,
            surface_transform.axis_y_world.z,
            half_size.y,
        ],
        normal_opacity: [
            surface_transform.normal_world.x,
            surface_transform.normal_world.y,
            surface_transform.normal_world.z,
            opacity,
        ],
        params: [
            if transform.is_some() { 0.1 } else { -2.0 },
            front_projection_depth,
            depth_size[0].max(1) as f32,
            depth_size[1].max(1) as f32,
        ],
        flags: [
            u32::from(request.projection.is_view_projection()),
            u32::from(sample_visibility_depth),
            0,
            0,
        ],
    };
    frame.write_buffer_pod(gpu.device(), &pipelines.decal_overlay_uniform, 0, &uniform);
    let Some(image) = decal_images.ensure(frame, gpu.device(), &request.image) else {
        return;
    };
    let decal_bind_group = decal_resources.ensure_bind_group(
        gpu.device(),
        &pipelines.decal_overlay_bgl,
        &pipelines.decal_overlay_uniform,
        image.key,
        image.view,
        scene_capture.depth_color_view(depth_slot),
        depth_source,
        depth_size,
        depth_generation,
    );

    let mesh = scene.mesh().expect("mesh checked");
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("decal_image_overlay_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.viewport_color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: scene_capture.depth_z_view(SceneDepthSlot::Main),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_bind_group(0, decal_bind_group, &[]);
    pass.set_vertex_buffer(0, mesh.vertex.slice(..));
    pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
    if let Some(scissor) = preview_geometry.screen_scissor {
        pass.set_scissor_rect(
            scissor.origin[0],
            scissor.origin[1],
            scissor.size[0],
            scissor.size[1],
        );
    }
    for sub_mesh in &mesh.sub_meshes {
        let sub_mesh_range = sub_mesh.index_range();
        for range in &visible_index_ranges {
            let start = sub_mesh_range.start.max(range.start);
            let end = sub_mesh_range.end.min(range.end);
            if start < end {
                pass.set_pipeline(
                    pipelines
                        .decal_overlay_pipeline
                        .select(sub_mesh.render_settings),
                );
                pass.draw_indexed(start..end, 0, 0..1);
            }
        }
    }
}

fn render_mirror_plane_overlay(
    frame: &mut GpuFrame,
    gpu: &mut RendererGpuState,
    pipelines: &crate::renderer::features::view::pipelines::ViewPipelines,
    targets: &crate::renderer::features::view::targets::ViewTargets,
    scene_capture: &SceneCapture,
    view_proj: Mat4,
    request: MirrorPlaneOverlayRequest,
) {
    if !request.plane_x.is_finite()
        || !request.center_yz.into_iter().all(f32::is_finite)
        || !request.half_extent_yz.into_iter().all(f32::is_finite)
        || request
            .half_extent_yz
            .into_iter()
            .any(|extent| extent <= 0.0)
    {
        return;
    }

    let uniform = MirrorPlaneOverlayUniform {
        view_proj: (gl_to_wgpu_depth() * view_proj).to_cols_array_2d(),
        center_half_y: [
            request.plane_x,
            request.center_yz[0],
            request.center_yz[1],
            request.half_extent_yz[0],
        ],
        half_z_opacity: [request.half_extent_yz[1], 0.10, 0.72, 0.22],
        color: [0.18, 0.82, 1.0, 1.0],
    };
    frame.write_buffer_pod(
        gpu.device(),
        &pipelines.mirror_plane_overlay_uniform,
        0,
        &uniform,
    );

    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mirror_plane_overlay_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.viewport_color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: scene_capture.depth_z_view(SceneDepthSlot::Main),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_bind_group(0, &pipelines.mirror_plane_overlay_bg, &[]);
    pass.set_pipeline(&pipelines.mirror_plane_face_pipeline);
    pass.draw(0..4, 0..1);
    pass.set_pipeline(&pipelines.mirror_plane_line_occluded_pipeline);
    pass.draw(0..12, 0..1);
    pass.set_pipeline(&pipelines.mirror_plane_line_visible_pipeline);
    pass.draw(0..12, 0..1);
}

fn render_viewport_selection_overlay(
    frame: &mut GpuFrame,
    gpu: &mut RendererGpuState,
    scene: &mut SceneResources,
    selections: &SelectionMasks,
    pipelines: &crate::renderer::features::view::pipelines::ViewPipelines,
    targets: &crate::renderer::features::view::targets::ViewTargets,
    scene_capture: &SceneCapture,
    active_selection: &ActiveSelection,
    viewport_size: [u32; 2],
    phase: f32,
    scene_visibility: &ViewportSceneVisibility,
) {
    if !active_selection.is_visible_active()
        || !scene.has_mesh()
        || viewport_size[0] == 0
        || viewport_size[1] == 0
    {
        return;
    }

    let mesh = scene.mesh().expect("mesh checked");
    for material_mask in active_selection
        .masks
        .iter()
        .filter(|mask| mask.mask_id.is_some())
    {
        let Some(mask_id) = material_mask.mask_id else {
            continue;
        };
        let Some(selection_view) = selections.view(mask_id) else {
            continue;
        };
        let Some(texture_size) = selections.texture_size(mask_id) else {
            continue;
        };
        let uniform = selection_overlay_uniform(viewport_size, texture_size, phase, 0.0, 0.9);
        frame.write_buffer_pod(
            gpu.device(),
            scene_capture.selection_overlay_uniform(),
            0,
            &uniform,
        );
        let bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_selection_overlay_bg"),
            layout: &pipelines.viewport_selection_overlay_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scene.view_proj_uniform().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(selection_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: scene_capture
                        .selection_overlay_uniform()
                        .as_entire_binding(),
                },
            ],
        });
        let mut pass = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport_selection_overlay_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.viewport_color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: scene_capture.depth_z_view(SceneDepthSlot::Main),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_bind_group(0, &bg, &[]);
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
        for sm in mesh.visible_sub_meshes(scene_visibility) {
            if sm.material_index != material_mask.material_index.as_usize() {
                continue;
            }
            pass.set_pipeline(
                pipelines
                    .viewport_selection_overlay_pipeline
                    .select(sm.render_settings),
            );
            pass.draw_indexed(sm.index_start..(sm.index_start + sm.index_count), 0, 0..1);
        }
    }
}

fn composite_viewport_brush_overlay(
    frame: &mut GpuFrame,
    gpu: &RendererGpuState,
    pipelines: &crate::renderer::features::view::pipelines::ViewPipelines,
    targets: &crate::renderer::features::view::targets::ViewTargets,
    overlay_view: &wgpu::TextureView,
) {
    let bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("brush_overlay_viewport_composite_bg"),
        layout: &pipelines.brush_overlay_viewport_composite_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(overlay_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(gpu.stroke_sampler()),
            },
        ],
    });
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brush_overlay_viewport_composite_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.viewport_color_view,
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
    pass.set_pipeline(&pipelines.brush_overlay_viewport_composite_pipeline);
    pass.set_bind_group(0, &bg, &[]);
    pass.draw(0..3, 0..1);
}

const SURFACE_OVERLAY_RING_WIDTH_RATIO: f32 = 0.04;
const SURFACE_OVERLAY_FILL_ALPHA: f32 = 0.0;
const SURFACE_OVERLAY_VIEW_OFFSET_RATIO: f32 = 0.8;

fn render_surface_brush_overlay(
    frame: &mut GpuFrame,
    txn: &mut ViewStrokeDeps<'_>,
    request: &SurfaceBrushOverlayRequest,
) -> bool {
    let radius_world = request.stroke_op.radius_world();
    if !txn.scene.has_mesh() || request.viewport_size[0] == 0 || request.viewport_size[1] == 0 {
        return false;
    }
    let normal = request.dab.world_normal.normalize_or_zero();
    if normal.length_squared() <= f32::EPSILON {
        return false;
    }

    view_transient::ensure_viewport_brush_overlay(
        txn.scratch,
        txn.gpu.device(),
        frame.encoder(),
        request.viewport_size,
    );
    clear_rgba_target(
        frame.encoder(),
        view_transient::viewport_brush_overlay_view(txn.scratch)
            .expect("viewport brush overlay texture is initialized"),
        [0.0, 0.0, 0.0, 0.0],
        "clear_surface_brush_overlay",
    );

    let uniform = SurfaceBrushOverlayUniform {
        view_proj: (gl_to_wgpu_depth() * request.viewport_view_proj_gl).to_cols_array_2d(),
        center_radius: [
            request.dab.world_pos.x,
            request.dab.world_pos.y,
            request.dab.world_pos.z,
            (radius_world * request.dab.radius_scale).max(1e-6),
        ],
        normal_params: [normal.x, normal.y, normal.z, 0.0],
        view_direction: [
            request.view_direction_world[0],
            request.view_direction_world[1],
            request.view_direction_world[2],
            SURFACE_OVERLAY_VIEW_OFFSET_RATIO,
        ],
        params: [
            0.0,
            SURFACE_OVERLAY_RING_WIDTH_RATIO,
            SURFACE_OVERLAY_FILL_ALPHA,
            0.0,
        ],
    };
    frame.write_buffer_pod(
        txn.gpu.device(),
        &txn.view_pipelines.brush_overlay_surface_view_uniform,
        0,
        &uniform,
    );

    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brush_overlay_surface_view_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: view_transient::viewport_brush_overlay_view(txn.scratch)
                    .expect("viewport brush overlay texture is initialized"),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: txn.scene_capture.depth_z_view(SceneDepthSlot::Main),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(&txn.view_pipelines.brush_overlay_surface_view_pipeline);
    pass.set_bind_group(0, &txn.view_pipelines.brush_overlay_surface_view_bg, &[]);
    pass.draw(0..6, 0..1);
    true
}

fn render_uv_selection_overlay(
    gpu: &RendererGpuState,
    frame: &mut GpuFrame,
    pipelines: &crate::renderer::features::view::pipelines::ViewPipelines,
    selections: &crate::renderer::document::selection::SelectionStore,
    target_view: &wgpu::TextureView,
    material_index: usize,
    uv_view_size: [u32; 2],
    transform: UvViewTransform,
    request: &SelectionOverlayRequest,
) {
    let Some(material_mask) = request
        .active_selection
        .material_mask(material_index.into())
    else {
        return;
    };
    let Some(mask_id) = material_mask.mask_id else {
        return;
    };
    let Some(selection_view) = selections.view(mask_id) else {
        return;
    };
    let Some(texture_size) = selections.texture_size(mask_id) else {
        return;
    };

    let uniform = uv_selection_overlay_uniform(
        uv_view_size,
        texture_size,
        request.phase,
        0.0,
        0.95,
        transform,
    );
    frame.write_buffer_pod(
        gpu.device(),
        &pipelines.uv_selection_overlay_uniform,
        0,
        &uniform,
    );
    let bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uv_selection_overlay_bg"),
        layout: &pipelines.uv_selection_overlay_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: pipelines.uv_selection_overlay_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(selection_view),
            },
        ],
    });
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("uv_selection_overlay_pass"),
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
    pass.set_pipeline(&pipelines.uv_selection_overlay_pipeline);
    pass.set_bind_group(0, &bg, &[]);
    pass.draw(0..3, 0..1);
}

fn uv_view_transform_uniform(
    view_size: [u32; 2],
    canvas_size: [u32; 2],
    transform: UvViewTransform,
    background_color: [f32; 3],
) -> UvViewTransformUniform {
    UvViewTransformUniform {
        view_size: [view_size[0] as f32, view_size[1] as f32],
        canvas_size: [canvas_size[0] as f32, canvas_size[1] as f32],
        center_transform: [
            transform.center_uv.x,
            transform.center_uv.y,
            transform.zoom,
            transform.rotation_radians.sin(),
        ],
        rotation: [transform.rotation_radians.cos(), 0.0, 0.0, 0.0],
        background_color: rgba_background(background_color),
    }
}

fn rgba_background(color: [f32; 3]) -> [f32; 4] {
    [color[0], color[1], color[2], 1.0]
}

fn wgpu_background(color: [f32; 3]) -> wgpu::Color {
    wgpu::Color {
        r: color[0] as f64,
        g: color[1] as f64,
        b: color[2] as f64,
        a: 1.0,
    }
}

fn uv_selection_overlay_uniform(
    view_size: [u32; 2],
    texture_size: [u32; 2],
    phase: f32,
    fill_opacity: f32,
    edge_opacity: f32,
    transform: UvViewTransform,
) -> UvSelectionOverlayUniform {
    UvSelectionOverlayUniform {
        view_size: [view_size[0] as f32, view_size[1] as f32],
        texture_size: [texture_size[0] as f32, texture_size[1] as f32],
        params: [phase, fill_opacity, edge_opacity, 1.5],
        color: [0.0, 0.0, 0.0, 1.0],
        center_transform: [
            transform.center_uv.x,
            transform.center_uv.y,
            transform.zoom,
            transform.rotation_radians.sin(),
        ],
        rotation: [transform.rotation_radians.cos(), 0.0, 0.0, 0.0],
    }
}

fn selection_overlay_uniform(
    view_size: [u32; 2],
    texture_size: [u32; 2],
    phase: f32,
    fill_opacity: f32,
    edge_opacity: f32,
) -> SelectionOverlayUniform {
    SelectionOverlayUniform {
        view_size: [view_size[0] as f32, view_size[1] as f32],
        texture_size: [texture_size[0] as f32, texture_size[1] as f32],
        params: [phase, fill_opacity, edge_opacity, 1.5],
        color: [0.0, 0.0, 0.0, 1.0],
    }
}
