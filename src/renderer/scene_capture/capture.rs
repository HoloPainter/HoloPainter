use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        geometry::RectU32, material::MaterialRenderMode, math::gl_to_wgpu_depth,
        render_report::GpuTextureMetrics, viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility,
    },
    renderer::{
        document::{
            materials::MaterialRegistry,
            scene::{SceneDepthSlot, SceneResources},
        },
        engine::gpu_state::RendererGpuState,
        features::composite::outputs::CompositeOutputViews,
        features::view::{
            targets::ViewTargets,
            types::{SelectionOverlayUniform, ViewportUniform, WireframeUniform},
        },
        gpu::{
            buffer::create_uniform_buffer,
            clear_rgba_target,
            frame::GpuFrame,
            texture::{create_color_target, create_depth_target_only, create_depth_targets},
        },
        scene_capture::SceneCapturePipelines,
    },
};

struct SceneDepthTargets {
    size: [u32; 2],
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _z: wgpu::Texture,
    z_view: wgpu::TextureView,
}

impl SceneDepthTargets {
    fn new(device: &wgpu::Device, viewport_size: [u32; 2]) -> Self {
        let size = [viewport_size[0].max(1), viewport_size[1].max(1)];
        let (color, color_view, z, z_view) = create_depth_targets(device, size);
        Self {
            size,
            _color: color,
            color_view,
            _z: z,
            z_view,
        }
    }
}

pub(crate) struct SceneCapture {
    viewport_size: [u32; 2],
    main_depth: SceneDepthTargets,
    decal_depth: Option<SceneDepthTargets>,
    decal_depth_generation: u64,
    surface_projection_depths: HashMap<crate::core::stroke::SurfaceProjectionId, SceneDepthTargets>,
    viewport_source_color: wgpu::Texture,
    viewport_source_color_view: wgpu::TextureView,
    _presentation_depth: wgpu::Texture,
    presentation_depth_view: wgpu::TextureView,
    viewport_uniform: wgpu::Buffer,
    selection_overlay_uniform: wgpu::Buffer,
    wireframe_uniform: wgpu::Buffer,
}

impl SceneCapture {
    pub fn new(device: &wgpu::Device) -> Self {
        let viewport_size = [512, 512];
        let main_depth = SceneDepthTargets::new(device, viewport_size);
        let (viewport_source_color, viewport_source_color_view) =
            create_color_target(device, viewport_size, "viewport_source_color");
        let (presentation_depth, presentation_depth_view) =
            create_depth_target_only(device, viewport_size);

        let viewport_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport_uniform"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let selection_overlay_uniform = create_uniform_buffer::<SelectionOverlayUniform>(
            device,
            "viewport_selection_overlay_uniform",
        );
        let wireframe_uniform =
            create_uniform_buffer::<WireframeUniform>(device, "viewport_wireframe_uniform");

        Self {
            viewport_size,
            main_depth,
            decal_depth: None,
            decal_depth_generation: 0,
            surface_projection_depths: HashMap::new(),
            viewport_source_color,
            viewport_source_color_view,
            _presentation_depth: presentation_depth,
            presentation_depth_view,
            viewport_uniform,
            selection_overlay_uniform,
            wireframe_uniform,
        }
    }

    pub(crate) fn viewport_size(&self) -> [u32; 2] {
        self.viewport_size
    }

    pub(crate) fn ensure_viewport_size(
        &mut self,
        device: &wgpu::Device,
        viewport: [u32; 2],
    ) -> bool {
        if viewport == self.viewport_size {
            return false;
        }

        self.viewport_size = viewport;
        self.main_depth = SceneDepthTargets::new(device, viewport);
        let (viewport_source_color, viewport_source_color_view) =
            create_color_target(device, viewport, "viewport_source_color");
        self.viewport_source_color = viewport_source_color;
        self.viewport_source_color_view = viewport_source_color_view;
        let (presentation_depth, presentation_depth_view) =
            create_depth_target_only(device, viewport);
        self._presentation_depth = presentation_depth;
        self.presentation_depth_view = presentation_depth_view;
        true
    }

    pub(crate) fn ensure_depth_slot(
        &mut self,
        device: &wgpu::Device,
        slot: SceneDepthSlot,
        requested_size: [u32; 2],
    ) -> bool {
        let requested_size = [requested_size[0].max(1), requested_size[1].max(1)];
        match slot {
            SceneDepthSlot::Main => self.ensure_viewport_size(device, requested_size),
            SceneDepthSlot::Decal => {
                if self
                    .decal_depth
                    .as_ref()
                    .is_some_and(|target| target.size == requested_size)
                {
                    return false;
                }
                self.decal_depth = Some(SceneDepthTargets::new(device, requested_size));
                self.decal_depth_generation = self.decal_depth_generation.wrapping_add(1);
                true
            }
            SceneDepthSlot::SurfaceProjection(projection_id) => {
                if self
                    .surface_projection_depths
                    .get(&projection_id)
                    .is_some_and(|target| target.size == requested_size)
                {
                    return false;
                }
                self.surface_projection_depths.insert(
                    projection_id,
                    SceneDepthTargets::new(device, requested_size),
                );
                true
            }
        }
    }

    pub(crate) fn decal_depth_generation(&self) -> u64 {
        self.decal_depth_generation
    }

    pub(crate) fn depth_color_view(&self, slot: SceneDepthSlot) -> &wgpu::TextureView {
        &self.depth_targets(slot).color_view
    }

    pub(crate) fn depth_z_view(&self, slot: SceneDepthSlot) -> &wgpu::TextureView {
        &self.depth_targets(slot).z_view
    }

    fn depth_targets(&self, slot: SceneDepthSlot) -> &SceneDepthTargets {
        match slot {
            SceneDepthSlot::Main => &self.main_depth,
            SceneDepthSlot::Decal => self
                .decal_depth
                .as_ref()
                .expect("decal depth slot must be ensured before use"),
            SceneDepthSlot::SurfaceProjection(projection_id) => self
                .surface_projection_depths
                .get(&projection_id)
                .expect("surface projection depth slot must be ensured before use"),
        }
    }

    pub(crate) fn viewport_source_color_view(&self) -> &wgpu::TextureView {
        &self.viewport_source_color_view
    }

    pub(crate) fn presentation_depth_view(&self) -> &wgpu::TextureView {
        &self.presentation_depth_view
    }

    pub(crate) fn selection_overlay_uniform(&self) -> &wgpu::Buffer {
        &self.selection_overlay_uniform
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let decal_texture_count = if self.decal_depth.is_some() { 2 } else { 0 };
        let texture_count = 4usize
            .saturating_add(decal_texture_count)
            .saturating_add(self.surface_projection_depths.len().saturating_mul(2));
        let mut bytes = texture_bytes_rgba32_equivalent(self.viewport_size).saturating_mul(2);
        bytes = bytes.saturating_add(
            texture_bytes_rgba32_equivalent(self.main_depth.size).saturating_mul(2),
        );
        if let Some(decal_depth) = &self.decal_depth {
            bytes = bytes.saturating_add(
                texture_bytes_rgba32_equivalent(decal_depth.size).saturating_mul(2),
            );
        }
        for depth in self.surface_projection_depths.values() {
            bytes =
                bytes.saturating_add(texture_bytes_rgba32_equivalent(depth.size).saturating_mul(2));
        }
        let mut metrics = GpuTextureMetrics::default();
        metrics.add_total_bytes(bytes);
        metrics.view_bytes = bytes;
        metrics.view_texture_count = texture_count;
        metrics
    }

    pub(crate) fn copy_viewport_source_color_to_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        destination: &wgpu::Texture,
        viewport_size: [u32; 2],
    ) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.viewport_source_color,
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
                width: viewport_size[0].max(1),
                height: viewport_size[1].max(1),
                depth_or_array_layers: 1,
            },
        );
    }

    pub(crate) fn copy_texture_to_viewport_source_color(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        viewport_size: [u32; 2],
    ) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.viewport_source_color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: viewport_size[0].max(1),
                height: viewport_size[1].max(1),
                depth_or_array_layers: 1,
            },
        );
    }

    pub(crate) fn render_viewport_3d(
        &mut self,
        gpu: &mut RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &mut SceneResources,
        materials: &MaterialRegistry,
        targets: &mut ViewTargets,
        composite_outputs: CompositeOutputViews<'_>,
        frame: &mut GpuFrame,
        viewport_view_proj_gl: glam::Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        camera: &crate::core::camera::OrbitCamera,
        scene_visibility: &ViewportSceneVisibility,
        background_color: [f32; 3],
        shading: ViewportShading,
    ) {
        if viewport_size[0] == 0 || viewport_size[1] == 0 {
            return;
        }
        if !scene.has_mesh() {
            targets.ensure_viewport_size(gpu.device(), viewport_size);
            clear_rgba_target(
                frame.encoder(),
                &targets.viewport_color_view,
                [
                    background_color[0],
                    background_color[1],
                    background_color[2],
                    1.0,
                ],
                "clear_viewport_without_mesh",
            );
            return;
        }

        self.render_viewport_presentation(
            gpu,
            pipelines,
            scene,
            materials,
            targets,
            Some(composite_outputs),
            frame,
            viewport_view_proj_gl,
            camera_world,
            viewport_size,
            camera,
            scene_visibility,
            background_color,
            shading,
        );
    }

    pub(crate) fn render_viewport_wireframe(
        &self,
        gpu: &RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &SceneResources,
        targets: &ViewTargets,
        frame: &mut GpuFrame,
        scene_visibility: &ViewportSceneVisibility,
        style: crate::core::wireframe::WireframeStyle,
    ) {
        let Some(mesh) = scene.mesh() else {
            return;
        };
        let uniform = WireframeUniform {
            color_opacity: [
                style.color[0],
                style.color[1],
                style.color[2],
                style.opacity,
            ],
        };
        frame.write_buffer_pod(gpu.device(), &self.wireframe_uniform, 0, &uniform);
        let viewport_wireframe_bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_wireframe_bg"),
            layout: &pipelines.viewport_wireframe_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scene.view_proj_uniform().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.wireframe_uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport_wireframe_overlay_pass"),
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
                    view: self.depth_z_view(SceneDepthSlot::Main),
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
        pass.set_pipeline(&pipelines.viewport_wireframe_pipeline);
        pass.set_bind_group(0, &viewport_wireframe_bg, &[]);
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.wire_index.slice(..), wgpu::IndexFormat::Uint32);
        for sub_mesh in mesh.visible_sub_meshes(scene_visibility) {
            if sub_mesh.wire_count == 0 {
                continue;
            }
            pass.draw_indexed(sub_mesh.wire_range(), 0, 0..1);
        }
    }

    pub(crate) fn create_viewport_material_bind_group(
        &self,
        gpu: &RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &SceneResources,
        current_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv_filter_active_layer_viewport_material_bg"),
            layout: &pipelines.surface_source_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scene.view_proj_uniform().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(current_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(gpu.paint_sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.viewport_uniform.as_entire_binding(),
                },
            ],
        })
    }

    pub(crate) fn record_filter_backup_scene_color_from_material_bind_groups(
        &mut self,
        gpu: &RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &mut SceneResources,
        frame: &mut GpuFrame,
        viewport_view_proj_gl: glam::Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        material_bind_groups: &[(usize, &wgpu::BindGroup)],
        capture_rect: Option<RectU32>,
        scene_visibility: &ViewportSceneVisibility,
    ) {
        if !scene.has_mesh() || viewport_size[0] == 0 || viewport_size[1] == 0 {
            return;
        }

        self.render_viewport_source_from_material_bind_groups(
            gpu,
            pipelines,
            scene,
            frame,
            viewport_view_proj_gl,
            camera_world,
            viewport_size,
            material_bind_groups,
            capture_rect,
            scene_visibility,
            "uv_filter_active_layer_scene_color_pass",
        );
    }

    fn render_viewport_presentation(
        &mut self,
        gpu: &mut RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &mut SceneResources,
        materials: &MaterialRegistry,
        targets: &mut ViewTargets,
        composite_outputs: Option<CompositeOutputViews<'_>>,
        frame: &mut GpuFrame,
        viewport_view_proj_gl: glam::Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        camera: &crate::core::camera::OrbitCamera,
        scene_visibility: &ViewportSceneVisibility,
        background_color: [f32; 3],
        shading: ViewportShading,
    ) {
        self.ensure_viewport_size(gpu.device(), viewport_size);
        targets.ensure_viewport_size(gpu.device(), viewport_size);
        scene.write_view_proj(
            gpu.device(),
            frame,
            gl_to_wgpu_depth() * viewport_view_proj_gl,
        );

        let viewport_uniform = ViewportUniform {
            camera_world: [camera_world[0], camera_world[1], camera_world[2], 0.0],
            shading: [
                if shading == ViewportShading::Shade {
                    1.0
                } else {
                    0.0
                },
                0.0,
                0.0,
                0.0,
            ],
        };
        frame.write_buffer_pod(gpu.device(), &self.viewport_uniform, 0, &viewport_uniform);

        let material_count = composite_outputs
            .map(|outputs| outputs.material_count())
            .unwrap_or_default();
        let mut viewport_bgs = Vec::with_capacity(material_count);
        viewport_bgs.resize_with(material_count, || None);
        for mi in 0..material_count {
            let Some(current_view) = composite_outputs.and_then(|outputs| outputs.texture_view(mi))
            else {
                continue;
            };
            let bg = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewport_presentation_material_bg"),
                layout: &pipelines.presentation_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: scene.view_proj_uniform().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(current_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(gpu.paint_sampler()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.viewport_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: materials
                            .presentation_uniform(mi)
                            .expect("composite material has presentation uniform")
                            .as_entire_binding(),
                    },
                ],
            });
            viewport_bgs[mi] = Some(bg);
        }

        let mesh = scene.mesh().expect("mesh checked");
        {
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport_presentation_opaque_cutoff_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets.viewport_color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: background_color[0] as f64,
                                g: background_color[1] as f64,
                                b: background_color[2] as f64,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: self.presentation_depth_view(),
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            for sm in mesh.visible_sub_meshes(scene_visibility) {
                let pipeline = match sm.render_settings.render_mode {
                    MaterialRenderMode::Opaque => pipelines
                        .presentation_pipelines
                        .opaque
                        .select(sm.render_settings),
                    MaterialRenderMode::Cutoff => pipelines
                        .presentation_pipelines
                        .cutoff
                        .select(sm.render_settings),
                    MaterialRenderMode::Blend => continue,
                };
                let Some(Some(bg)) = viewport_bgs.get(sm.material_index) else {
                    continue;
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw_indexed(sm.index_range(), 0, 0..1);
            }
        }

        let camera_position = camera.position();
        let camera_forward = camera.forward();
        let mut blend_draws = mesh
            .sub_meshes
            .iter()
            .enumerate()
            .filter(|(_, sm)| {
                sm.is_visible(scene_visibility)
                    && sm.render_settings.render_mode == MaterialRenderMode::Blend
                    && viewport_bgs
                        .get(sm.material_index)
                        .is_some_and(Option::is_some)
            })
            .map(|(sub_mesh_index, sm)| {
                (
                    sub_mesh_index,
                    (sm.bounds_center - camera_position).dot(camera_forward),
                )
            })
            .collect::<Vec<_>>();
        blend_draws.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        if !blend_draws.is_empty() {
            let mut pass = frame
                .encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport_presentation_blend_pass"),
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
                        view: self.presentation_depth_view(),
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
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            for (sub_mesh_index, _) in blend_draws {
                let sm = &mesh.sub_meshes[sub_mesh_index];
                let bg = viewport_bgs[sm.material_index]
                    .as_ref()
                    .expect("blend draw checked material bind group");
                pass.set_bind_group(0, bg, &[]);
                if sm.render_settings.double_sided {
                    pass.set_pipeline(
                        &pipelines
                            .presentation_pipelines
                            .blend_double_sided_draw_back_faces,
                    );
                    pass.draw_indexed(sm.index_range(), 0, 0..1);
                    pass.set_pipeline(
                        &pipelines
                            .presentation_pipelines
                            .blend_double_sided_draw_front_faces,
                    );
                    pass.draw_indexed(sm.index_range(), 0, 0..1);
                } else {
                    pass.set_pipeline(&pipelines.presentation_pipelines.blend_single_sided);
                    pass.draw_indexed(sm.index_range(), 0, 0..1);
                }
            }
        }
    }

    fn render_viewport_source_from_material_bind_groups(
        &mut self,
        gpu: &RendererGpuState,
        pipelines: &SceneCapturePipelines,
        scene: &mut SceneResources,
        frame: &mut GpuFrame,
        viewport_view_proj_gl: glam::Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        material_bind_groups: &[(usize, &wgpu::BindGroup)],
        capture_rect: Option<RectU32>,
        scene_visibility: &ViewportSceneVisibility,
        pass_label: &'static str,
    ) {
        self.ensure_viewport_size(gpu.device(), viewport_size);
        scene.write_view_proj(
            gpu.device(),
            frame,
            gl_to_wgpu_depth() * viewport_view_proj_gl,
        );

        let viewport_uniform = ViewportUniform {
            camera_world: [camera_world[0], camera_world[1], camera_world[2], 0.0],
            shading: [0.0; 4],
        };
        frame.write_buffer_pod(gpu.device(), &self.viewport_uniform, 0, &viewport_uniform);

        let bind_group_count = material_bind_groups
            .iter()
            .map(|(material_index, _)| *material_index)
            .max()
            .map_or(0, |material_index| material_index + 1);
        let mut viewport_bgs = vec![None; bind_group_count];
        for (material_index, bind_group) in material_bind_groups {
            viewport_bgs[*material_index] = Some(*bind_group);
        }

        let mut pass = frame
            .encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(pass_label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.viewport_source_color_view(),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.presentation_depth_view(),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

        let mesh = scene.mesh().expect("mesh checked");
        pass.set_vertex_buffer(0, mesh.vertex.slice(..));
        pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
        if let Some(rect) = capture_rect {
            pass.set_scissor_rect(rect.origin[0], rect.origin[1], rect.size[0], rect.size[1]);
        }
        for sm in mesh.visible_sub_meshes(scene_visibility) {
            let Some(Some(bg)) = viewport_bgs.get(sm.material_index) else {
                continue;
            };
            pass.set_pipeline(pipelines.surface_source_pipeline.select(sm.render_settings));
            pass.set_bind_group(0, *bg, &[]);
            pass.draw_indexed(sm.index_start..(sm.index_start + sm.index_count), 0, 0..1);
        }
    }
}

fn texture_bytes_rgba32_equivalent(size: [u32; 2]) -> usize {
    size[0].max(1).saturating_mul(size[1].max(1)) as usize * 4
}
