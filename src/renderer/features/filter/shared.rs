use std::ops::Range;

use anyhow::{Result, anyhow, ensure};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        selection::ActiveSelection,
        surface::{PaintSurfaceId, PaintSurfaceRole},
        surface_filter::SurfaceFilterDomain,
    },
    renderer::{
        document::{selection::SelectionStore, surfaces::SurfaceEditContext},
        gpu::frame::GpuFrame,
    },
};

use super::{feature::FilterFeatureDeps, packing::AtlasPlacement, pipelines::FilterPipelines};

pub(super) const INVALID_TRIANGLE: u32 = u32::MAX;

pub(super) struct SurfaceFilterScene {
    pub(super) triangles: wgpu::Buffer,
    pub(super) triangle_map_vertices: wgpu::Buffer,
    pub(super) material_vertex_ranges: Vec<Range<u32>>,
    pub(super) triangle_material_indices: Vec<usize>,
    pub(super) triangle_count: u32,
    pub(super) scene_bounds_min: [f32; 3],
}

pub(super) struct OwnedTexture2D {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

#[derive(Debug)]
pub(super) struct ValidatedFilterTargets {
    pub(super) domain: SurfaceFilterDomain,
    pub(super) active_materials: Vec<bool>,
    pub(super) material_sizes: Vec<[u32; 2]>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GpuSurfaceTriangle {
    pub(super) p0: [f32; 4],
    pub(super) p1: [f32; 4],
    pub(super) p2: [f32; 4],
    pub(super) uv01: [f32; 4],
    pub(super) uv2_pad: [f32; 4],
    pub(super) metadata: [u32; 4],
    pub(super) neighbors: [u32; 4],
    pub(super) neighbor_edges: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct TriangleMapVertex {
    pub(super) uv: [f32; 2],
    pub(super) triangle_id: u32,
    pub(super) _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GpuMaterialInfo {
    pub(super) origin: [u32; 2],
    pub(super) size: [u32; 2],
    pub(super) enabled: u32,
    pub(super) _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct FilterGutterUniform {
    pub(super) radius_world: f32,
    pub(super) sigma_world: f32,
    pub(super) material_count: u32,
    pub(super) current_material: u32,
    pub(super) gutter_radius: u32,
    pub(super) _pad: [u32; 3],
}

pub(super) fn validate_filter_targets(
    deps: &FilterFeatureDeps<'_>,
    surfaces: &[PaintSurfaceId],
    filter_name: &str,
) -> Result<ValidatedFilterTargets> {
    let material_count = deps.materials.material_count();
    let (role, active_materials) =
        validate_filter_surface_ids(surfaces, material_count, filter_name)?;

    let material_sizes = (0..material_count)
        .map(|material_index| {
            deps.materials
                .texture_size(material_index)
                .ok_or_else(|| anyhow!("{filter_name} material {material_index} is missing"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ValidatedFilterTargets {
        domain: role.into(),
        active_materials,
        material_sizes,
    })
}

fn validate_filter_surface_ids(
    surfaces: &[PaintSurfaceId],
    material_count: usize,
    filter_name: &str,
) -> Result<(PaintSurfaceRole, Vec<bool>)> {
    ensure!(!surfaces.is_empty(), "{filter_name} has no target surfaces");
    ensure!(material_count > 0, "{filter_name} has no materials");

    let layer_id = surfaces[0].layer_id;
    let role = surfaces[0].role;
    ensure!(
        matches!(role, PaintSurfaceRole::Raster | PaintSurfaceRole::LayerMask),
        "{filter_name} has an unsupported target role"
    );
    let mut active_materials = vec![false; material_count];
    for surface in surfaces {
        ensure!(
            surface.role == role,
            "{filter_name} targets must have one surface role"
        );
        ensure!(
            surface.layer_id == layer_id,
            "{filter_name} targets must belong to one layer"
        );
        ensure!(
            surface.material_index().as_usize() < material_count,
            "{filter_name} material {} is out of range",
            surface.material_index
        );
        ensure!(
            !active_materials[surface.material_index().as_usize()],
            "{filter_name} contains duplicate material {}",
            surface.material_index
        );
        active_materials[surface.material_index().as_usize()] = true;
    }

    Ok((role, active_materials))
}

pub(super) fn validate_affected_filter_targets(
    source_surfaces: &[PaintSurfaceId],
    affected_surfaces: &[PaintSurfaceId],
    filter_name: &str,
) -> Result<()> {
    ensure!(
        !affected_surfaces.is_empty(),
        "{filter_name} has no affected surfaces"
    );
    for (index, surface) in affected_surfaces.iter().enumerate() {
        ensure!(
            source_surfaces.contains(surface),
            "{filter_name} affected surfaces must be a subset of source surfaces"
        );
        ensure!(
            !affected_surfaces[..index].contains(surface),
            "{filter_name} contains duplicate affected surface {surface:?}"
        );
    }
    Ok(())
}

pub(super) fn resolve_filter_selection_view<'a>(
    selections: &'a SelectionStore,
    active_selection: &ActiveSelection,
    material_index: usize,
    full_selection_fallback: &'a wgpu::TextureView,
) -> Result<(&'a wgpu::TextureView, u32)> {
    if active_selection.is_effectively_full() {
        return Ok((full_selection_fallback, 0));
    }
    let material_mask = active_selection
        .material_mask(material_index.into())
        .ok_or_else(|| {
            anyhow!("active selection has no mask metadata for material {material_index}")
        })?;
    let mask_id = material_mask
        .mask_id
        .ok_or_else(|| anyhow!("active selection has no GPU mask for material {material_index}"))?;
    let selection_view = selections.view(mask_id).ok_or_else(|| {
        anyhow!("ActiveSelection references a missing GPU selection texture: {mask_id:?}")
    })?;
    Ok((selection_view, 1))
}

pub(super) fn copy_surfaces_to_source_atlas(
    frame: &mut GpuFrame,
    surface_edit: &SurfaceEditContext<'_>,
    surfaces: &[PaintSurfaceId],
    material_count: usize,
    placements: &[AtlasPlacement],
    source_atlas: &wgpu::Texture,
    filter_name: &str,
) -> Result<Vec<Option<AtlasPlacement>>> {
    let mut placements_by_material = vec![None; material_count];
    for placement in placements {
        placements_by_material[placement.material_index] = Some(*placement);
    }
    for surface in surfaces {
        let target = surface_edit
            .edit_surface_target(*surface)
            .ok_or_else(|| anyhow!("{filter_name} target is not GPU resident: {surface:?}"))?;
        let placement = placements_by_material[surface.material_index().as_usize()]
            .ok_or_else(|| anyhow!("{filter_name} material has no atlas placement"))?;
        ensure!(
            target.texture_size == placement.size,
            "{filter_name} target size mismatch for material {}",
            surface.material_index
        );
        copy_surface_to_atlas(frame.encoder(), target.texture, source_atlas, placement);
    }
    Ok(placements_by_material)
}

pub(super) fn create_material_buffer(
    frame: &mut GpuFrame,
    device: &wgpu::Device,
    material_count: usize,
    placements: &[AtlasPlacement],
    label: &'static str,
) -> wgpu::Buffer {
    let mut material_infos = vec![GpuMaterialInfo::zeroed(); material_count];
    for placement in placements {
        material_infos[placement.material_index] = GpuMaterialInfo {
            origin: placement.origin,
            size: placement.size,
            enabled: 1,
            _pad: [0; 3],
        };
    }
    frame.create_buffer_from_slice(device, label, wgpu::BufferUsages::STORAGE, &material_infos)
}

pub(super) fn record_triangle_map(
    frame: &mut GpuFrame,
    pipelines: &FilterPipelines,
    scene: &SurfaceFilterScene,
    material_index: usize,
    size: [u32; 2],
    view: &wgpu::TextureView,
) {
    let mut pass = frame
        .encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("filter_triangle_map_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(&pipelines.shared.triangle_map);
    pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
    pass.set_vertex_buffer(0, scene.triangle_map_vertices.slice(..));
    if let Some(range) = scene.material_vertex_ranges.get(material_index) {
        pass.draw(range.clone(), 0..1);
    }
}

pub(super) fn create_rgba_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    usage: wgpu::TextureUsages,
    label: &'static str,
) -> OwnedTexture2D {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    OwnedTexture2D { texture, view }
}

pub(super) fn create_r32_uint_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    usage: wgpu::TextureUsages,
    label: &'static str,
) -> OwnedTexture2D {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    OwnedTexture2D { texture, view }
}

fn copy_surface_to_atlas(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    placement: AtlasPlacement,
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
            origin: wgpu::Origin3d {
                x: placement.origin[0],
                y: placement.origin[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: placement.size[0],
            height: placement.size[1],
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use crate::core::{material::MaterialIndex, surface::LayerId};
    use slotmap::KeyData;

    use super::*;

    fn layer(value: u64) -> LayerId {
        LayerId::from(KeyData::from_ffi(value))
    }

    #[test]
    fn filter_surface_validation_derives_domain_and_rejects_mixed_or_invalid_sets() {
        let raster = PaintSurfaceId::raster(MaterialIndex(0), layer(1));
        let mask = PaintSurfaceId::layer_mask(MaterialIndex(0), layer(1));
        assert_eq!(
            validate_filter_surface_ids(&[raster], 2, "filter")
                .unwrap()
                .0,
            PaintSurfaceRole::Raster
        );
        assert_eq!(
            validate_filter_surface_ids(&[mask], 2, "filter").unwrap().0,
            PaintSurfaceRole::LayerMask
        );
        assert!(validate_filter_surface_ids(&[raster, mask], 2, "filter").is_err());
        assert!(
            validate_filter_surface_ids(
                &[raster, PaintSurfaceId::raster(MaterialIndex(1), layer(2)),],
                2,
                "filter",
            )
            .is_err()
        );
        assert!(validate_filter_surface_ids(&[raster, raster], 2, "filter").is_err());
    }
}
