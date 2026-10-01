use anyhow::{Result, ensure};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        decal::{DecalApplyPlan, DecalImageAsset, DecalProjection},
        math::gl_to_wgpu_depth,
        selection::ActiveSelection,
    },
    renderer::{
        decal_image::DecalImageCache,
        document::scene::SceneDepthSlot,
        features::{
            apply::{deps::PaintApplyDeps, types::DecalApplyUniform},
            brush::transient as stroke_transient,
        },
        gpu::{copy_rects_a_to_b, frame::GpuFrame},
        surface_edit::{
            EditMask, SurfaceEditPipelines, SurfaceEditResources, SurfaceEditTarget, UvIslandBleed,
        },
    },
};

pub(crate) struct DecalApplyResources {
    sampler: wgpu::Sampler,
}

impl DecalApplyResources {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("decal_apply_sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
        }
    }
}

pub(crate) struct DecalApplyRequest<'a> {
    pub(crate) plan: &'a DecalApplyPlan,
    pub(crate) image: &'a DecalImageAsset,
    pub(crate) projection: DecalProjection,
    pub(crate) opacity: f32,
    pub(crate) active_selection: &'a ActiveSelection,
}

pub(crate) fn record_decal_apply(
    frame: &mut GpuFrame,
    deps: &mut PaintApplyDeps<'_, '_>,
    resources: &mut DecalApplyResources,
    decal_images: &mut DecalImageCache,
    surface_edit_resources: &SurfaceEditResources,
    surface_edit_pipelines: &SurfaceEditPipelines,
    uv_island_bleed: &mut UvIslandBleed,
    request: DecalApplyRequest<'_>,
) -> Result<bool> {
    ensure!(request.projection.is_valid(), "Decal projection is invalid");
    ensure!(request.opacity.is_finite(), "Decal opacity is not finite");
    if request.opacity <= 0.0 || request.plan.is_empty() {
        return Ok(false);
    }
    let Some(projector_view_proj) = request.projection.projector_view_projection() else {
        return Ok(false);
    };
    let Some(visibility_view_proj) = request.projection.visibility_view_projection() else {
        return Ok(false);
    };
    let projector_view_proj = gl_to_wgpu_depth() * projector_view_proj;
    let visibility_view_proj = gl_to_wgpu_depth() * visibility_view_proj;
    let depth_size = decal_depth_size(request.projection.depth_size(request.image.size));
    deps.scene.ensure_scene_depth_for_index_ranges(
        deps.gpu,
        deps.scene_capture,
        &deps.scene_capture_pipelines.viewport_prepass_bgl,
        &deps.scene_capture_pipelines.viewport_prepass_pipeline,
        frame,
        SceneDepthSlot::Decal,
        depth_size,
        projector_view_proj,
        &request.plan.depth_index_ranges,
    );
    let projector_depth = deps.scene_capture.depth_color_view(SceneDepthSlot::Decal);
    let Some(image) = decal_images.ensure(frame, deps.gpu.device(), request.image) else {
        return Ok(false);
    };
    let image_view = image.view;
    let image_sampler = &resources.sampler;

    let transform = match request.projection {
        DecalProjection::Surface(transform) => Some(transform),
        DecalProjection::ViewProjection { .. } => None,
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
    let uniform_base = DecalApplyUniform {
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
            request.opacity.clamp(0.0, 1.0),
        ],
        params: [
            if transform.is_some() { 0.1 } else { -2.0 },
            surface_transform.front_projection_depth_world(),
            depth_size[0] as f32,
            depth_size[1] as f32,
        ],
        flags: [0, u32::from(request.projection.is_view_projection()), 0, 0],
    };

    let mut any_applied = false;
    for target in &request.plan.targets {
        let surface = target.surface;
        let material_index = surface.material_index().as_usize();
        let Some(texture_size) = deps.surfaces.surface_texture_size(surface) else {
            continue;
        };
        stroke_transient::ensure_material_source_uv(
            deps.scratch,
            deps.gpu.device(),
            frame.encoder(),
            material_index,
            texture_size,
        );
        stroke_transient::ensure_material_stroke_uv(
            deps.scratch,
            deps.gpu.device(),
            frame.encoder(),
            material_index,
            texture_size,
        );

        let applied = {
            let Some(layer) = deps.surfaces.stroke_surface_target(surface, deps.scratch) else {
                continue;
            };
            copy_rects_a_to_b(
                frame.encoder(),
                layer.write_texture,
                layer.read_texture,
                layer.texture_size,
                &target.damage_rects,
            );
            let Some(edit_mask_view) =
                stroke_transient::material_stroke_uv_view(deps.scratch, material_index)
            else {
                continue;
            };
            surface_edit_pipelines.clear_edit_mask_rects(
                frame.encoder(),
                edit_mask_view,
                layer.texture_size,
                &target.footprint_rects,
                "clear_decal_edit_mask_regions",
            );
            let (selection_view, selection_enabled) =
                if request.active_selection.is_effectively_full() {
                    (layer.read_view, 0)
                } else if let Some(mask_id) = request
                    .active_selection
                    .material_mask(material_index.into())
                    .and_then(|mask| mask.mask_id)
                {
                    let Some(selection_view) = deps.selections.view(mask_id) else {
                        continue;
                    };
                    (selection_view, 1)
                } else {
                    continue;
                };

            let uniform = DecalApplyUniform {
                flags: [selection_enabled, uniform_base.flags[1], 0, 0],
                ..uniform_base
            };
            frame.write_buffer_pod(
                deps.gpu.device(),
                &deps.pipelines.decal_apply_uniform,
                0,
                &uniform,
            );
            let bind_group = deps
                .gpu
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("decal_apply_bg"),
                    layout: &deps.pipelines.decal_apply_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: deps.pipelines.decal_apply_uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(image_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(image_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(projector_depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(layer.read_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(selection_view),
                        },
                    ],
                });
            let Some(mesh) = deps.scene.mesh() else {
                continue;
            };
            {
                let mut pass = frame
                    .encoder()
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("decal_apply_pass"),
                        color_attachments: &[
                            Some(wgpu::RenderPassColorAttachment {
                                view: layer.write_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: edit_mask_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            }),
                        ],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                pass.set_pipeline(&deps.pipelines.decal_apply_pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                for batch in &target.draw_batches {
                    let rect = batch.scissor;
                    pass.set_scissor_rect(
                        rect.origin[0],
                        rect.origin[1],
                        rect.size[0],
                        rect.size[1],
                    );
                    for range in &batch.index_ranges {
                        pass.draw_indexed(range.clone(), 0, 0..1);
                    }
                }
            }

            uv_island_bleed.record_bounded(
                &*deps.gpu,
                surface_edit_resources,
                &*deps.scene,
                surface_edit_pipelines,
                frame,
                SurfaceEditTarget {
                    material_index,
                    texture_size: layer.texture_size,
                    write_texture: layer.write_texture,
                    write_view: layer.write_view,
                    read_texture: layer.read_texture,
                    read_view: layer.read_view,
                },
                EditMask {
                    view: edit_mask_view,
                },
                &target.footprint_rects,
                &target.damage_rects,
            );
            true
        };

        if applied {
            deps.surfaces.resolve_mask_edit_proxy_rects_into_frame(
                &mut *deps.gpu,
                frame,
                surface,
                &target.damage_rects,
            )?;
            any_applied = true;
        }
    }

    Ok(any_applied)
}

fn decal_depth_size(image_size: [u32; 2]) -> [u32; 2] {
    [image_size[0].clamp(64, 2048), image_size[1].clamp(64, 2048)]
}

#[cfg(test)]
mod tests {
    #[test]
    fn decal_depth_size_is_bounded_but_preserves_rectangular_resolution() {
        assert_eq!(super::decal_depth_size([1, 4096]), [64, 2048]);
        assert_eq!(super::decal_depth_size([512, 256]), [512, 256]);
    }
}
