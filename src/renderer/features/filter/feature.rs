use anyhow::{Result, anyhow};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::document::MeshData,
    renderer::{
        document::{
            materials::MaterialRegistry, selection::SelectionStore, surfaces::SurfaceRepository,
        },
        engine::gpu_state::RendererGpuState,
        gpu::frame::GpuFrame,
        report::CommandResult,
    },
};

use super::{
    command::FilterCommand,
    pipelines::FilterPipelines,
    shared::{GpuSurfaceTriangle, INVALID_TRIANGLE, SurfaceFilterScene, TriangleMapVertex},
};

pub(crate) struct FilterFeature {
    pub(super) pipelines: FilterPipelines,
    pub(super) scene: Option<SurfaceFilterScene>,
    pub(super) adjustment_preview: Option<super::adjustment::AdjustmentPreviewSession>,
}

pub(crate) struct FilterFeatureDeps<'a> {
    pub(crate) gpu: &'a mut RendererGpuState,
    pub(crate) surfaces: &'a mut SurfaceRepository,
    pub(crate) materials: &'a MaterialRegistry,
    pub(crate) selections: &'a SelectionStore,
}

impl FilterFeature {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: FilterPipelines::new(device),
            scene: None,
            adjustment_preview: None,
        }
    }

    pub(crate) fn upload_scene(
        &mut self,
        device: &wgpu::Device,
        frame: &mut GpuFrame,
        mesh: &MeshData,
    ) -> Result<()> {
        self.clear_scene_caches();
        let topology = mesh.surface_filter_topology();
        if topology.triangles.is_empty() {
            return Ok(());
        }

        let mut gpu_triangles = Vec::with_capacity(topology.triangles.len());
        let mut triangle_material_indices = Vec::with_capacity(topology.triangles.len());
        let material_bucket_count = topology
            .triangles
            .iter()
            .map(|triangle| triangle.material_index)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let mut vertices_by_material = vec![Vec::new(); material_bucket_count];

        for (triangle_index, triangle) in topology.triangles.iter().enumerate() {
            let packed_triangle_id = u32::try_from(triangle_index)
                .map_err(|_| anyhow!("surface filter triangle count exceeds u32"))?
                .checked_add(1)
                .ok_or_else(|| anyhow!("surface filter triangle id overflow"))?;
            let material_index = u32::try_from(triangle.material_index)
                .map_err(|_| anyhow!("surface filter material index exceeds u32"))?;
            let mesh_id = u32::try_from(triangle.mesh_id.0)
                .map_err(|_| anyhow!("surface filter mesh id exceeds u32"))?;
            let normal_vector = (triangle.positions[1] - triangle.positions[0])
                .cross(triangle.positions[2] - triangle.positions[0]);
            let double_area = normal_vector.length();
            let normal = normal_vector / double_area;
            let world_area = double_area * 0.5;
            let mut neighbors = [INVALID_TRIANGLE; 4];
            let mut neighbor_edges = [0u32; 4];
            for edge_index in 0..3 {
                let edge = triangle.edges[edge_index];
                if let Some(neighbor) = edge.neighbor_triangle {
                    neighbors[edge_index] = u32::try_from(neighbor)
                        .map_err(|_| anyhow!("surface filter neighbor index exceeds u32"))?;
                    neighbor_edges[edge_index] = u32::try_from(edge.neighbor_edge)
                        .map_err(|_| anyhow!("surface filter neighbor edge exceeds u32"))?;
                }
            }
            gpu_triangles.push(GpuSurfaceTriangle {
                p0: triangle.positions[0].extend(normal.x).to_array(),
                p1: triangle.positions[1].extend(normal.y).to_array(),
                p2: triangle.positions[2].extend(normal.z).to_array(),
                uv01: [
                    triangle.uvs[0].x,
                    triangle.uvs[0].y,
                    triangle.uvs[1].x,
                    triangle.uvs[1].y,
                ],
                uv2_pad: [triangle.uvs[2].x, triangle.uvs[2].y, world_area, 0.0],
                metadata: [material_index, mesh_id, 0, 0],
                neighbors,
                neighbor_edges,
            });
            triangle_material_indices.push(triangle.material_index);
            vertices_by_material[triangle.material_index].extend(triangle.uvs.iter().map(|uv| {
                TriangleMapVertex {
                    uv: uv.to_array(),
                    triangle_id: packed_triangle_id,
                    _pad: 0,
                }
            }));
        }

        let mut material_vertex_ranges = Vec::with_capacity(vertices_by_material.len());
        let mut triangle_map_vertices = Vec::new();
        for vertices in vertices_by_material {
            let start = u32::try_from(triangle_map_vertices.len())
                .map_err(|_| anyhow!("surface filter vertex count exceeds u32"))?;
            triangle_map_vertices.extend(vertices);
            let end = u32::try_from(triangle_map_vertices.len())
                .map_err(|_| anyhow!("surface filter vertex count exceeds u32"))?;
            material_vertex_ranges.push(start..end);
        }

        let triangles = frame.create_buffer_from_slice(
            device,
            "surface_filter_triangles",
            wgpu::BufferUsages::STORAGE,
            &gpu_triangles,
        );
        let triangle_map_vertices = frame.create_buffer_from_slice(
            device,
            "surface_filter_triangle_map_vertices",
            wgpu::BufferUsages::VERTEX,
            &triangle_map_vertices,
        );
        let triangle_count = u32::try_from(gpu_triangles.len())
            .map_err(|_| anyhow!("surface filter triangle count exceeds u32"))?;
        let scene_bounds_min = mesh
            .scene_bounds()
            .map(|(minimum, _)| minimum.to_array())
            .unwrap_or([0.0; 3]);
        self.scene = Some(SurfaceFilterScene {
            triangles,
            triangle_map_vertices,
            material_vertex_ranges,
            triangle_material_indices,
            triangle_count,
            scene_bounds_min,
        });
        Ok(())
    }

    pub(crate) fn clear_scene_caches(&mut self) {
        self.scene = None;
        self.adjustment_preview = None;
    }

    pub(crate) fn execute(
        &mut self,
        frame: &mut GpuFrame,
        deps: &mut FilterFeatureDeps<'_>,
        command: FilterCommand,
    ) -> Result<CommandResult> {
        if self.adjustment_preview.is_some()
            && !matches!(
                &command,
                FilterCommand::AdjustmentPreviewUpdate { .. }
                    | FilterCommand::AdjustmentPreviewCommit { .. }
                    | FilterCommand::AdjustmentPreviewCancel
            )
        {
            return Err(anyhow!(
                "finish or cancel the active adjustment filter preview before running another filter"
            ));
        }
        match command {
            FilterCommand::SurfaceBlur {
                source_surfaces,
                affected_surfaces,
                params,
                active_selection,
            } => super::surface_blur::execute(
                self,
                frame,
                deps,
                &source_surfaces,
                &affected_surfaces,
                params,
                &active_selection,
            ),
            FilterCommand::SpatialBlur {
                source_surfaces,
                affected_surfaces,
                params,
                active_selection,
            } => super::spatial_blur::execute(
                self,
                frame,
                deps,
                &source_surfaces,
                &affected_surfaces,
                params,
                &active_selection,
            ),
            FilterCommand::Adjustment {
                surfaces,
                adjustment,
                active_selection,
            } => super::adjustment::execute(
                self,
                frame,
                deps,
                &surfaces,
                adjustment,
                &active_selection,
            ),
            FilterCommand::AdjustmentPreviewBegin {
                surfaces,
                adjustment,
                active_selection,
            } => super::adjustment::begin_preview(
                self,
                frame,
                deps,
                &surfaces,
                adjustment,
                &active_selection,
            ),
            FilterCommand::AdjustmentPreviewUpdate { adjustment } => {
                super::adjustment::update_preview(self, frame, deps, adjustment)
            }
            FilterCommand::AdjustmentPreviewCommit { adjustment } => {
                super::adjustment::commit_preview(self, frame, deps, adjustment)
            }
            FilterCommand::AdjustmentPreviewCancel => {
                super::adjustment::cancel_preview(self, frame, deps)
            }
        }
    }
}
