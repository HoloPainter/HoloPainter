use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::wgpu;
use glam::Vec3;

use crate::{
    core::{
        document::{MeshData, MeshId},
        material::MaterialRenderSettings,
        viewport_visibility::ViewportSceneVisibility,
    },
    renderer::gpu::frame::GpuFrame,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
}

pub(crate) struct GpuMesh {
    pub(crate) vertex: wgpu::Buffer,
    pub(crate) index: wgpu::Buffer,
    pub(crate) wire_index: wgpu::Buffer,
    pub(crate) sub_meshes: Vec<GpuSubMesh>,
    material_sub_meshes: Vec<Vec<usize>>,
    pub(crate) uv_linear_scale: f32,
    pub(crate) source_mesh: MeshData,
}

impl GpuMesh {
    pub(crate) fn sub_meshes_for_material(
        &self,
        material_index: usize,
    ) -> impl Iterator<Item = &GpuSubMesh> {
        self.material_sub_meshes
            .get(material_index)
            .into_iter()
            .flat_map(|indices| indices.iter())
            .map(|&index| &self.sub_meshes[index])
    }

    pub(crate) fn visible_sub_meshes<'a>(
        &'a self,
        visibility: &'a ViewportSceneVisibility,
    ) -> impl Iterator<Item = &'a GpuSubMesh> {
        self.sub_meshes
            .iter()
            .filter(|sub_mesh| sub_mesh.is_visible(visibility))
    }

    pub(crate) fn visible_index_ranges(
        &self,
        visibility: &ViewportSceneVisibility,
    ) -> Vec<Range<u32>> {
        self.visible_draw_ranges(visibility)
            .into_iter()
            .map(|draw| draw.index_range)
            .collect()
    }

    pub(crate) fn visible_draw_ranges(
        &self,
        visibility: &ViewportSceneVisibility,
    ) -> Vec<GpuDrawRange> {
        self.visible_sub_meshes(visibility)
            .map(|sub_mesh| GpuDrawRange {
                material_index: sub_mesh.material_index,
                index_range: sub_mesh.index_range(),
                render_settings: sub_mesh.render_settings,
            })
            .collect()
    }

    pub(crate) fn visible_index_range_intersections(
        &self,
        ranges: &[Range<u32>],
        visibility: &ViewportSceneVisibility,
    ) -> Vec<Range<u32>> {
        intersect_index_ranges(
            self.visible_sub_meshes(visibility)
                .map(GpuSubMesh::index_range),
            ranges,
        )
    }

    pub(crate) fn set_material_render_settings(
        &mut self,
        material_index: usize,
        settings: MaterialRenderSettings,
    ) {
        for sub_mesh in &mut self.sub_meshes {
            if sub_mesh.material_index == material_index {
                sub_mesh.render_settings = settings;
            }
        }
    }
}

pub(crate) struct GpuSubMesh {
    pub(crate) mesh_id: MeshId,
    pub(crate) material_index: usize,
    pub(crate) index_start: u32,
    pub(crate) index_count: u32,
    pub(crate) wire_start: u32,
    pub(crate) wire_count: u32,
    pub(crate) render_settings: MaterialRenderSettings,
    pub(crate) bounds_center: Vec3,
}

impl GpuSubMesh {
    pub(crate) fn is_visible(&self, visibility: &ViewportSceneVisibility) -> bool {
        visibility.geometry_visible(self.mesh_id, self.material_index.into())
    }

    pub(crate) fn index_range(&self) -> Range<u32> {
        self.index_start..(self.index_start + self.index_count)
    }

    pub(crate) fn wire_range(&self) -> Range<u32> {
        self.wire_start..(self.wire_start + self.wire_count)
    }
}

pub(crate) fn create_gpu_mesh_from_document_mesh(
    device: &wgpu::Device,
    frame: &mut GpuFrame,
    mesh: &MeshData,
) -> GpuMesh {
    let vertices: Vec<MeshVertex> = mesh
        .positions
        .iter()
        .zip(mesh.uvs.iter())
        .zip(mesh.normals.iter())
        .map(|((p, uv), n)| MeshVertex {
            position: [p.x, p.y, p.z],
            uv: [uv.x, uv.y],
            normal: [n.x, n.y, n.z],
        })
        .collect();
    let mut indices = Vec::<u32>::with_capacity(mesh.indices.len() * 3);
    for tri in mesh.indices.iter() {
        indices.extend_from_slice(tri);
    }
    let mut wire_indices = Vec::<u32>::new();
    let sub_meshes: Vec<GpuSubMesh> = mesh
        .sub_meshes
        .iter()
        .map(|sm| {
            let wire_start = wire_indices.len() as u32;
            for [a, b] in &sm.wireframe_edges {
                wire_indices.push(*a);
                wire_indices.push(*b);
            }
            GpuSubMesh {
                mesh_id: sm.mesh_id,
                material_index: sm.material_index,
                index_start: sm.start_index,
                index_count: sm.index_count,
                wire_start,
                wire_count: wire_indices.len() as u32 - wire_start,
                render_settings: MaterialRenderSettings::default(),
                bounds_center: sub_mesh_bounds_center(mesh, sm.start_index, sm.index_count),
            }
        })
        .collect();

    let mut material_sub_meshes = Vec::<Vec<usize>>::new();
    for (sub_mesh_index, sub_mesh) in sub_meshes.iter().enumerate() {
        if sub_mesh.material_index >= material_sub_meshes.len() {
            material_sub_meshes.resize_with(sub_mesh.material_index + 1, Vec::new);
        }
        material_sub_meshes[sub_mesh.material_index].push(sub_mesh_index);
    }

    let mesh_buffer_usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX;
    let vertex =
        frame.create_buffer_from_slice(device, "mesh_vertex", mesh_buffer_usage, &vertices);
    let index = frame.create_buffer_from_slice(device, "mesh_index", mesh_buffer_usage, &indices);
    let wire_index =
        frame.create_buffer_from_slice(device, "mesh_wire_index", mesh_buffer_usage, &wire_indices);

    GpuMesh {
        vertex,
        index,
        wire_index,
        sub_meshes,
        material_sub_meshes,
        uv_linear_scale: mesh.uv_linear_scale,
        source_mesh: mesh.clone(),
    }
}

fn sub_mesh_bounds_center(mesh: &MeshData, index_start: u32, index_count: u32) -> Vec3 {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;
    for vertex_index in mesh
        .indices
        .iter()
        .flatten()
        .skip(index_start as usize)
        .take(index_count as usize)
    {
        let position = mesh.positions[*vertex_index as usize];
        min = min.min(position);
        max = max.max(position);
        found = true;
    }
    if found { (min + max) * 0.5 } else { Vec3::ZERO }
}

fn intersect_index_ranges(
    visible_ranges: impl IntoIterator<Item = Range<u32>>,
    requested_ranges: &[Range<u32>],
) -> Vec<Range<u32>> {
    let mut intersections = Vec::new();
    for visible in visible_ranges {
        for requested in requested_ranges {
            let start = visible.start.max(requested.start);
            let end = visible.end.min(requested.end);
            if start < end {
                intersections.push(start..end);
            }
        }
    }
    intersections.sort_unstable_by_key(|range| (range.start, range.end));

    let mut merged: Vec<Range<u32>> = Vec::with_capacity(intersections.len());
    for range in intersections {
        if let Some(previous) = merged.last_mut()
            && previous.end >= range.start
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use crate::core::{
        document::MeshId, material::MaterialRenderSettings,
        viewport_visibility::ViewportSceneVisibility,
    };

    use super::{GpuSubMesh, intersect_index_ranges};

    #[test]
    fn sub_mesh_visibility_combines_mesh_and_material() {
        let sub_mesh = GpuSubMesh {
            mesh_id: MeshId(2),
            material_index: 5,
            index_start: 0,
            index_count: 3,
            wire_start: 0,
            wire_count: 6,
            render_settings: MaterialRenderSettings::default(),
            bounds_center: Vec3::ZERO,
        };
        let mut visibility = ViewportSceneVisibility::default();

        assert!(sub_mesh.is_visible(&visibility));
        visibility.set_mesh_visible(MeshId(2), false);
        assert!(!sub_mesh.is_visible(&visibility));
        visibility.set_mesh_visible(MeshId(2), true);
        visibility.set_material_visible(5.into(), false);
        assert!(!sub_mesh.is_visible(&visibility));
    }

    #[test]
    fn visible_index_intersections_are_clipped_and_merged() {
        let intersections = intersect_index_ranges([0..12, 18..30], &[6..21, 24..27, 27..33]);

        assert_eq!(intersections, vec![6..12, 18..21, 24..30]);
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuDrawRange {
    #[allow(dead_code)]
    pub(crate) material_index: usize,
    pub(crate) index_range: Range<u32>,
    #[allow(dead_code)]
    pub(crate) render_settings: MaterialRenderSettings,
}
