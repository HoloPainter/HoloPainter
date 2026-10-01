use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        composite::{ApplyParams, SelectionCompositeMode},
        mask::{FullMaskSource, MaskSource, ProjectionMaskSource, ShapeMaskSource},
        selection::ActiveSelection,
    },
    renderer::{
        document::{
            materials::MaterialRegistry,
            scene::SceneResources,
            selection::{SelectionStore, SelectionUploadStats},
        },
        features::{
            brush::transient as stroke_transient,
            selection::{
                feature::{SelectionFeatureDeps, SelectionPipelines, render_selection_composite},
                transient as selection_transient,
            },
        },
        gpu::{clear_rgba_target, frame::GpuFrame},
        mask::support::{MaskConsumer, ensure_supported},
        mask::{
            projection::{
                build_viewport_polygon_projection_depth, build_viewport_projection_depth,
                viewport_polygon_projection_mask_r8_with_depth,
                viewport_polygon_projection_mask_rect_r8_with_depth,
                viewport_rect_projection_mask_r8_with_depth,
                viewport_rect_projection_mask_rect_r8_with_depth,
            },
            projection_gpu::{
                ViewportProjectionMaskGpuDeps, prepare_viewport_polygon_coverage,
                write_viewport_projection_mask_to_stroke_uv,
            },
            shape::{
                polygon_mask_r8, polygon_mask_rect_r8, rectangle_mask_r8, rectangle_mask_rect_r8,
            },
        },
    },
};

pub(crate) fn apply_to_selection_mask(
    store: &mut SelectionStore,
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    scene: &SceneResources,
    materials: &MaterialRegistry,
    mask: &MaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> anyhow::Result<SelectionUploadStats> {
    ensure_supported(MaskConsumer::Selection, mask, None)?;
    let stats = match mask {
        MaskSource::Full(full) => apply_full_to_selection_mask(
            store,
            device,
            frame,
            materials,
            *full,
            params,
            active_selection,
        ),
        MaskSource::Shape(ShapeMaskSource::Rectangle(rect)) => apply_rectangle_to_selection_mask(
            store,
            device,
            frame,
            materials,
            rect,
            params,
            active_selection,
        ),
        MaskSource::Shape(ShapeMaskSource::Polygon(polygon)) => apply_polygon_to_selection_mask(
            store,
            device,
            frame,
            materials,
            polygon,
            params,
            active_selection,
        ),
        MaskSource::Projection(projection) => apply_viewport_projection_to_selection_mask(
            store,
            device,
            frame,
            scene,
            materials,
            projection,
            params,
            active_selection,
        ),
        // TODO(refactor-paint): Support brush/flood/lasso/existing selection mask apply.
        MaskSource::Brush(_)
        | MaskSource::Geometry(_)
        | MaskSource::Flood(_)
        | MaskSource::ExistingSelection => unreachable!("unsupported masks fail preflight"),
    };
    Ok(stats)
}

fn apply_full_to_selection_mask(
    store: &mut SelectionStore,
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    materials: &MaterialRegistry,
    full: FullMaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> SelectionUploadStats {
    let mut stats = SelectionUploadStats::default();
    for material_mask in &active_selection.masks {
        let material_index = material_mask.material_index.as_usize();
        if matches!(
            full,
            FullMaskSource::Material {
                material_index: target
            } if material_index != target
        ) {
            continue;
        }
        let Some(mask_id) = material_mask.mask_id else {
            continue;
        };
        let Some(size) = materials.texture_size(material_index) else {
            continue;
        };
        let selection = store.ensure_texture(device, mask_id, material_index, size);
        let overlay = vec![255; size[0] as usize * size[1] as usize];
        combine_selection_pixels(selection.pixels_mut(), &overlay, params.composite);
        stats.merge(selection.upload_full_into_frame(device, frame));
    }
    stats
}

fn apply_rectangle_to_selection_mask(
    store: &mut SelectionStore,
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    materials: &MaterialRegistry,
    rect: &crate::core::mask::RectangleMaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> SelectionUploadStats {
    let Some(material_index) = rect.material_index else {
        return SelectionUploadStats::default();
    };
    let Some(material_mask) = active_selection.material_mask(material_index.into()) else {
        return SelectionUploadStats::default();
    };
    let Some(mask_id) = material_mask.mask_id else {
        return SelectionUploadStats::default();
    };
    let Some(size) = materials.texture_size(material_index) else {
        return SelectionUploadStats::default();
    };
    let selection = store.ensure_texture(device, mask_id, material_index, size);
    if selection_operation_requires_full_upload(params.composite) {
        let overlay = rectangle_mask_r8(rect, size);
        combine_selection_pixels(selection.pixels_mut(), &overlay, params.composite);
        return selection.upload_full_into_frame(device, frame);
    }

    let Some((rect, overlay)) = rectangle_mask_rect_r8(rect, size) else {
        return SelectionUploadStats::default();
    };
    combine_selection_rect_pixels(
        selection.pixels_mut(),
        size,
        rect,
        &overlay,
        params.composite,
    );
    selection.upload_rect_into_frame(device, frame, rect)
}

fn apply_polygon_to_selection_mask(
    store: &mut SelectionStore,
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    materials: &MaterialRegistry,
    polygon: &crate::core::mask::PolygonMaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> SelectionUploadStats {
    let Some(material_index) = polygon.material_index else {
        return SelectionUploadStats::default();
    };
    let Some(material_mask) = active_selection.material_mask(material_index.into()) else {
        return SelectionUploadStats::default();
    };
    let Some(mask_id) = material_mask.mask_id else {
        return SelectionUploadStats::default();
    };
    let Some(size) = materials.texture_size(material_index) else {
        return SelectionUploadStats::default();
    };
    let selection = store.ensure_texture(device, mask_id, material_index, size);
    if selection_operation_requires_full_upload(params.composite) {
        let overlay = polygon_mask_r8(polygon, size);
        combine_selection_pixels(selection.pixels_mut(), &overlay, params.composite);
        return selection.upload_full_into_frame(device, frame);
    }

    let Some((rect, overlay)) = polygon_mask_rect_r8(polygon, size) else {
        return SelectionUploadStats::default();
    };
    combine_selection_rect_pixels(
        selection.pixels_mut(),
        size,
        rect,
        &overlay,
        params.composite,
    );
    selection.upload_rect_into_frame(device, frame, rect)
}

fn apply_viewport_projection_to_selection_mask(
    store: &mut SelectionStore,
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    scene: &SceneResources,
    materials: &MaterialRegistry,
    projection: &ProjectionMaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> SelectionUploadStats {
    let Some(mesh) = scene.mesh() else {
        return SelectionUploadStats::default();
    };
    let source_mesh = &mesh.source_mesh;
    let Some(depth) = (match projection {
        ProjectionMaskSource::ViewportRect(rect) => {
            build_viewport_projection_depth(source_mesh, rect)
        }
        ProjectionMaskSource::ViewportPolygon(polygon) => {
            build_viewport_polygon_projection_depth(source_mesh, polygon)
        }
    }) else {
        return SelectionUploadStats::default();
    };
    let projection_material_index = projection_material_index(projection);
    let mut stats = SelectionUploadStats::default();
    let needs_full_upload = selection_operation_requires_full_upload(params.composite);
    for material_mask in &active_selection.masks {
        if projection_material_index
            .is_some_and(|material_index| material_index != material_mask.material_index.as_usize())
        {
            continue;
        }
        let Some(mask_id) = material_mask.mask_id else {
            continue;
        };
        let material_index = material_mask.material_index.as_usize();
        let Some(texture_size) = materials.texture_size(material_index) else {
            continue;
        };
        let selection = store.ensure_texture(device, mask_id, material_index, texture_size);
        if needs_full_upload {
            let overlay = match projection {
                ProjectionMaskSource::ViewportRect(rect) => {
                    viewport_rect_projection_mask_r8_with_depth(
                        source_mesh,
                        material_index,
                        rect,
                        texture_size,
                        &depth,
                    )
                }
                ProjectionMaskSource::ViewportPolygon(polygon) => {
                    viewport_polygon_projection_mask_r8_with_depth(
                        source_mesh,
                        material_index,
                        polygon,
                        texture_size,
                        &depth,
                    )
                }
            };
            combine_selection_pixels(selection.pixels_mut(), &overlay, params.composite);
            stats.merge(selection.upload_full_into_frame(device, frame));
            continue;
        }

        let overlay = match projection {
            ProjectionMaskSource::ViewportRect(rect) => {
                viewport_rect_projection_mask_rect_r8_with_depth(
                    source_mesh,
                    material_index,
                    rect,
                    texture_size,
                    &depth,
                )
            }
            ProjectionMaskSource::ViewportPolygon(polygon) => {
                viewport_polygon_projection_mask_rect_r8_with_depth(
                    source_mesh,
                    material_index,
                    polygon,
                    texture_size,
                    &depth,
                )
            }
        };
        let Some(overlay) = overlay else {
            continue;
        };
        combine_selection_rect_pixels(
            selection.pixels_mut(),
            texture_size,
            overlay.rect,
            &overlay.r8,
            params.composite,
        );
        stats.merge(selection.upload_rect_into_frame(device, frame, overlay.rect));
    }
    stats
}

pub(crate) fn apply_gpu_projection_to_selection_mask(
    frame: &mut GpuFrame,
    deps: &mut SelectionFeatureDeps<'_>,
    pipelines: &SelectionPipelines,
    projection: &ProjectionMaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: &ActiveSelection,
) -> SelectionUploadStats {
    let mut stats = SelectionUploadStats::default();
    let projection_material_index = projection_material_index(projection);
    let viewport_polygon_coverage = match projection {
        ProjectionMaskSource::ViewportPolygon(polygon) => {
            prepare_viewport_polygon_coverage(frame, deps.gpu.device(), polygon)
        }
        ProjectionMaskSource::ViewportRect(_) => None,
    };
    for material_mask in &active_selection.masks {
        if projection_material_index
            .is_some_and(|material_index| material_index != material_mask.material_index.as_usize())
        {
            continue;
        }
        let Some(mask_id) = material_mask.mask_id else {
            continue;
        };
        let material_index = material_mask.material_index.as_usize();
        let Some(texture_size) = deps.materials.texture_size(material_index) else {
            continue;
        };
        {
            let selection = deps.selections.ensure_texture(
                deps.gpu.device(),
                mask_id,
                material_index,
                texture_size,
            );
            if !selection.is_initialized() {
                clear_rgba_target(
                    frame.encoder(),
                    selection.view(),
                    [0.0, 0.0, 0.0, 0.0],
                    "clear_uninitialized_selection_mask",
                );
                selection.mark_gpu_initialized();
            }
        }

        selection_transient::ensure_material_source_uv(
            deps.scratch,
            deps.gpu.device(),
            frame.encoder(),
            material_index,
            texture_size,
        );
        {
            let Some(selection_source_texture) =
                selection_transient::material_source_uv_texture(deps.scratch, material_index)
            else {
                continue;
            };
            let Some(selection_texture) = deps.selections.texture(mask_id) else {
                continue;
            };
            frame.encoder().copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: selection_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: selection_source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: texture_size[0].max(1),
                    height: texture_size[1].max(1),
                    depth_or_array_layers: 1,
                },
            );
        }

        let wrote_projection = {
            let mut projection_deps = ViewportProjectionMaskGpuDeps {
                gpu: deps.gpu,
                pipelines: deps.projection_pipelines,
                scene: deps.scene,
                scratch: deps.scratch,
                scene_capture: deps.scene_capture,
                scene_capture_pipelines: deps.scene_capture_pipelines,
            };
            write_viewport_projection_mask_to_stroke_uv(
                frame,
                &mut projection_deps,
                material_index,
                projection,
                texture_size,
                viewport_polygon_coverage.as_ref(),
            )
            .is_some()
        };
        if !wrote_projection {
            continue;
        }
        let Some(selection_source_view) =
            selection_transient::material_source_uv_view(deps.scratch, material_index)
        else {
            continue;
        };
        let Some(overlay_view) =
            stroke_transient::material_stroke_uv_view(deps.scratch, material_index)
        else {
            continue;
        };
        let Some(selection_view) = deps.selections.view(mask_id) else {
            continue;
        };
        render_selection_composite(
            frame,
            deps.gpu.device(),
            pipelines,
            selection_view,
            selection_source_view,
            overlay_view,
            params.composite,
        );
        if let Some(selection) = deps.selections.texture_mut(mask_id) {
            selection.mark_gpu_written();
        }
        selection_transient::release_material_source_uv_after_submit(
            deps.scratch,
            frame,
            material_index,
        );
        stats.record(
            texture_size,
            crate::core::geometry::RectU32::full(texture_size),
        );
    }
    if let Some(coverage) = viewport_polygon_coverage {
        coverage.retain_until_submit(frame);
    }
    stats
}

fn projection_material_index(projection: &ProjectionMaskSource) -> Option<usize> {
    match projection {
        ProjectionMaskSource::ViewportRect(rect) => rect.material_index,
        ProjectionMaskSource::ViewportPolygon(polygon) => polygon.material_index,
    }
}

pub(crate) fn combine_selection_pixels(
    base: &mut [u8],
    overlay: &[u8],
    composite: SelectionCompositeMode,
) {
    debug_assert_eq!(base.len(), overlay.len());
    for (base, overlay) in base.iter_mut().zip(overlay.iter().copied()) {
        *base = match composite {
            SelectionCompositeMode::Replace => overlay,
            SelectionCompositeMode::Add => (*base).max(overlay),
            SelectionCompositeMode::Subtract => {
                ((*base as u16 * (255 - overlay) as u16) / 255) as u8
            }
            SelectionCompositeMode::Intersect => (*base).min(overlay),
            SelectionCompositeMode::Difference => base.abs_diff(overlay),
            SelectionCompositeMode::Clear => 0,
            SelectionCompositeMode::Invert => 255 - *base,
        };
    }
}

fn selection_operation_requires_full_upload(composite: SelectionCompositeMode) -> bool {
    matches!(
        composite,
        SelectionCompositeMode::Replace
            | SelectionCompositeMode::Intersect
            | SelectionCompositeMode::Clear
            | SelectionCompositeMode::Invert
    )
}

fn combine_selection_rect_pixels(
    base: &mut [u8],
    texture_size: [u32; 2],
    rect: crate::core::geometry::RectU32,
    overlay: &[u8],
    composite: SelectionCompositeMode,
) {
    debug_assert_eq!(overlay.len(), rect.size[0] as usize * rect.size[1] as usize);
    let width = texture_size[0] as usize;
    let rect_width = rect.size[0] as usize;
    for row in 0..rect.size[1] as usize {
        let base_start = (rect.origin[1] as usize + row) * width + rect.origin[0] as usize;
        let overlay_start = row * rect_width;
        combine_selection_pixels(
            &mut base[base_start..base_start + rect_width],
            &overlay[overlay_start..overlay_start + rect_width],
            composite,
        );
    }
}
