use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::{
    core::render_report::RenderMetrics,
    renderer::{
        gpu::frame::GpuFrame,
        mutation::PresentDirty,
        presentation::{MaterialTextureOutput, OutputRequest, OutputRequests, TextureOutputs},
    },
};

use super::core::RenderEngine;

impl RenderEngine {
    fn texture_size(&self, material_index: usize) -> [usize; 2] {
        self.state
            .document
            .materials
            .texture_size_usize(material_index)
    }

    fn material_count(&self) -> usize {
        self.state.document.materials.material_count()
    }

    fn paint_texture_view(&self, material_index: usize) -> &wgpu::TextureView {
        self.features
            .composite
            .texture_view(material_index)
            .expect("composite texture must be allocated before material presentation")
    }

    fn viewport_texture_view(&self) -> &wgpu::TextureView {
        self.features.view.viewport_texture_view()
    }

    fn uv_view_texture_view(&self) -> &wgpu::TextureView {
        self.features.view.uv_view_texture_view()
    }

    fn tool_preview_texture_view(&self) -> &wgpu::TextureView {
        self.features.tool_preview.texture_view()
    }

    pub(super) fn prepare_required_outputs(
        &mut self,
        frame: &mut GpuFrame,
        dirty: PresentDirty,
        requests: OutputRequests,
    ) -> Result<RenderMetrics> {
        let mut metrics = RenderMetrics::default();
        if requests.is_empty() {
            return Ok(metrics);
        }

        for request in requests.into_requests() {
            match request {
                OutputRequest::MaterialComposite(request) => {
                    debug_assert!(dirty.material_textures());
                    metrics.merge(
                        self.prepare_material_composite_output(frame, request.material_index)?,
                    );
                }
                OutputRequest::Viewport(request) => {
                    debug_assert!(dirty.viewport());
                    self.prepare_viewport_output(
                        frame,
                        request.camera,
                        request.view_proj,
                        request.camera_world,
                        request.viewport_size,
                        request.brush_overlay_request,
                        request.selection_overlay_request,
                        request.mirror_plane_overlay_request,
                        request.decal_overlay_request,
                        request.scene_visibility,
                        request.show_wireframe,
                        request.wireframe_style,
                        request.background_color,
                        request.shading,
                    );
                }
                OutputRequest::UvView(request) => {
                    debug_assert!(dirty.uv_view());
                    self.prepare_uv_view_output(
                        frame,
                        request.material_index,
                        request.uv_view_size,
                        request.transform,
                        request.brush_overlay_request,
                        request.selection_overlay_request,
                        request.show_wireframe,
                        request.wireframe_style,
                        request.background_color,
                    );
                }
                OutputRequest::ToolPreview(request) => {
                    debug_assert!(dirty.tool_preview());
                    self.prepare_tool_preview_output(frame, request)?;
                }
            }
        }
        Ok(metrics)
    }

    pub fn texture_outputs(&self) -> TextureOutputs<'_> {
        let materials = (0..self.material_count())
            .map(|material_index| {
                MaterialTextureOutput::new(
                    material_index,
                    self.paint_texture_view(material_index),
                    self.texture_size(material_index),
                )
            })
            .collect();

        self.state.presentation.texture_outputs(
            materials,
            self.viewport_texture_view(),
            self.uv_view_texture_view(),
            self.tool_preview_texture_view(),
        )
    }
}
