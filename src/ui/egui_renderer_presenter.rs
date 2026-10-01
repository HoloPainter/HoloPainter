use std::sync::Arc;

use eframe::{
    egui::{self, TextureId},
    egui_wgpu::{RenderState, wgpu},
};

use crate::renderer::{mutation::PresentDirty, presentation::TextureOutputs};

pub struct EguiRendererPresenter {
    renderer: Arc<egui::mutex::RwLock<eframe::egui_wgpu::Renderer>>,
    device: wgpu::Device,
    paint_texture_ids: Vec<TextureId>,
    viewport_texture_id: TextureId,
    uv_view_texture_id: TextureId,
    tool_preview_texture_id: TextureId,
}

impl EguiRendererPresenter {
    pub(crate) fn new(rs: &RenderState, outputs: TextureOutputs<'_>) -> Self {
        let renderer = rs.renderer.clone();
        let device = rs.device.clone();
        let paint_texture_ids = outputs
            .materials()
            .iter()
            .map(|material| {
                renderer.write().register_native_texture(
                    &device,
                    material.view(),
                    wgpu::FilterMode::Linear,
                )
            })
            .collect();
        let viewport_texture_id = renderer.write().register_native_texture(
            &device,
            outputs.viewport(),
            wgpu::FilterMode::Linear,
        );
        let uv_view_texture_id = renderer.write().register_native_texture(
            &device,
            outputs.uv_view(),
            wgpu::FilterMode::Linear,
        );
        let tool_preview_texture_id = renderer.write().register_native_texture(
            &device,
            outputs.tool_preview(),
            wgpu::FilterMode::Linear,
        );

        Self {
            renderer,
            device,
            paint_texture_ids,
            viewport_texture_id,
            uv_view_texture_id,
            tool_preview_texture_id,
        }
    }

    pub(crate) fn sync(&mut self, outputs: TextureOutputs<'_>, dirty: PresentDirty) {
        if dirty.material_textures() {
            self.sync_material_texture_bindings(&outputs);
        }
        if dirty.viewport() {
            self.sync_viewport_texture_binding(outputs.viewport());
        }
        if dirty.uv_view() {
            self.sync_uv_view_texture_binding(outputs.uv_view());
        }
        if dirty.tool_preview() {
            self.sync_tool_preview_texture_binding(outputs.tool_preview());
        }
        let _ = dirty.selection();
    }

    pub(crate) fn sync_material_texture_bindings(&mut self, outputs: &TextureOutputs<'_>) {
        if self.paint_texture_ids.len() > outputs.material_count() {
            self.paint_texture_ids.truncate(outputs.material_count());
        }
        for material in outputs.materials() {
            let idx = material.material_index();
            let _ = material.size();
            if idx >= self.paint_texture_ids.len() {
                self.paint_texture_ids
                    .push(self.renderer.write().register_native_texture(
                        &self.device,
                        material.view(),
                        wgpu::FilterMode::Linear,
                    ));
            } else {
                self.renderer.write().update_egui_texture_from_wgpu_texture(
                    &self.device,
                    material.view(),
                    wgpu::FilterMode::Linear,
                    self.paint_texture_ids[idx],
                );
            }
        }
    }

    fn sync_viewport_texture_binding(&mut self, viewport: &wgpu::TextureView) {
        self.renderer.write().update_egui_texture_from_wgpu_texture(
            &self.device,
            viewport,
            wgpu::FilterMode::Linear,
            self.viewport_texture_id,
        );
    }

    fn sync_uv_view_texture_binding(&mut self, uv_view: &wgpu::TextureView) {
        self.renderer.write().update_egui_texture_from_wgpu_texture(
            &self.device,
            uv_view,
            wgpu::FilterMode::Linear,
            self.uv_view_texture_id,
        );
    }

    fn sync_tool_preview_texture_binding(&mut self, tool_preview: &wgpu::TextureView) {
        self.renderer.write().update_egui_texture_from_wgpu_texture(
            &self.device,
            tool_preview,
            wgpu::FilterMode::Linear,
            self.tool_preview_texture_id,
        );
    }

    pub fn texture_id(&self, material_index: usize) -> TextureId {
        self.paint_texture_ids[material_index]
    }

    pub fn viewport_texture_id(&self) -> TextureId {
        self.viewport_texture_id
    }

    pub fn uv_view_texture_id(&self) -> TextureId {
        self.uv_view_texture_id
    }

    pub fn tool_preview_texture_id(&self) -> TextureId {
        self.tool_preview_texture_id
    }
}
