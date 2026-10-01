use anyhow::Result;
use glam::Mat4;

use crate::{
    core::{
        camera::OrbitCamera, render_report::RenderMetrics, viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility, wireframe::WireframeStyle,
    },
    renderer::{
        ColorSampleRequest, ColorSampleTarget, ColorSampleView,
        gpu::frame::GpuFrame,
        view::{
            BrushOverlayRequest, DecalOverlayRequest, MirrorPlaneOverlayRequest,
            SelectionOverlayRequest, ToolPreviewRequest, UvBrushOverlayRequest,
        },
    },
};

use super::core::RenderEngine;

fn uv_to_texel(uv: [f32; 2], size: [u32; 2]) -> Option<[u32; 2]> {
    if size[0] == 0
        || size[1] == 0
        || !uv[0].is_finite()
        || !uv[1].is_finite()
        || !(0.0..=1.0).contains(&uv[0])
        || !(0.0..=1.0).contains(&uv[1])
    {
        return None;
    }
    Some([
        ((uv[0] * size[0] as f32).floor() as u32).min(size[0] - 1),
        ((uv[1] * size[1] as f32).floor() as u32).min(size[1] - 1),
    ])
}

impl RenderEngine {
    pub(super) fn prepare_material_composite_output(
        &mut self,
        frame: &mut GpuFrame,
        material_index: usize,
    ) -> Result<RenderMetrics> {
        self.features.composite.prepare_material_composite(
            frame,
            &self.state.gpu,
            &mut self.state.document,
            &mut self.state.transient,
            material_index,
        )
    }

    pub(super) fn prepare_viewport_output(
        &mut self,
        frame: &mut GpuFrame,
        camera: OrbitCamera,
        view_proj: Mat4,
        camera_world: [f32; 3],
        viewport_size: [u32; 2],
        brush_overlay_request: Option<BrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        mirror_plane_overlay_request: Option<MirrorPlaneOverlayRequest>,
        decal_overlay_request: Option<DecalOverlayRequest>,
        scene_visibility: ViewportSceneVisibility,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
        shading: ViewportShading,
    ) {
        let composite_resources = self.features.composite.view_resources();
        self.features.view.render_viewport(
            frame,
            &mut self.state.gpu,
            &mut self.state.transient,
            &mut self.scene_capture,
            &self.features.scene_capture_pipelines,
            &mut self.state.document,
            composite_resources,
            camera,
            view_proj,
            camera_world,
            viewport_size,
            brush_overlay_request,
            selection_overlay_request,
            mirror_plane_overlay_request,
            decal_overlay_request,
            &mut self.features.decal_images,
            scene_visibility,
            show_wireframe,
            wireframe_style,
            background_color,
            shading,
        );
    }

    pub(super) fn prepare_uv_view_output(
        &mut self,
        frame: &mut GpuFrame,
        material_index: usize,
        uv_view_size: [u32; 2],
        transform: crate::core::uv_view::UvViewTransform,
        brush_overlay_request: Option<UvBrushOverlayRequest>,
        selection_overlay_request: Option<SelectionOverlayRequest>,
        show_wireframe: bool,
        wireframe_style: WireframeStyle,
        background_color: [f32; 3],
    ) {
        let composite_resources = self.features.composite.view_resources();
        self.features.view.render_uv_view(
            frame,
            &mut self.state.gpu,
            &mut self.state.transient,
            composite_resources,
            &mut self.state.document,
            material_index,
            uv_view_size,
            transform,
            brush_overlay_request,
            selection_overlay_request,
            show_wireframe,
            wireframe_style,
            background_color,
        );
    }

    pub(super) fn enqueue_color_sample(
        &mut self,
        frame: &mut GpuFrame,
        request: ColorSampleRequest,
    ) {
        match request.target {
            ColorSampleTarget::ViewOutput { view, position } => {
                let features = &mut self.features;
                let (texture, size) = match view {
                    ColorSampleView::Viewport3d => (
                        features.view.viewport_texture(),
                        features.view.viewport_size(),
                    ),
                    ColorSampleView::Uv => (
                        features.view.uv_view_texture(),
                        features.view.uv_view_size(),
                    ),
                };
                features
                    .color_sampler
                    .enqueue(frame, texture, size, position, request);
            }
            ColorSampleTarget::CompositeTexture { material_index, uv } => {
                let features = &mut self.features;
                let Some(size) = features.composite.texture_size(material_index) else {
                    features.color_sampler.complete_with_error(
                        request,
                        format!("composite texture is unavailable for material {material_index}"),
                    );
                    return;
                };
                let Some(position) = uv_to_texel(uv, size) else {
                    features.color_sampler.complete_with_error(
                        request,
                        "color sample UV is outside the composite texture".to_owned(),
                    );
                    return;
                };
                let Some(texture) = features.composite.texture(material_index) else {
                    features.color_sampler.complete_with_error(
                        request,
                        format!("composite texture is unavailable for material {material_index}"),
                    );
                    return;
                };
                features
                    .color_sampler
                    .enqueue(frame, texture, size, position, request);
            }
            ColorSampleTarget::SurfaceTexture { surface, uv } => {
                if let Err(error) = self.state.document.surfaces.prepare_read_surface_for_frame(
                    &self.state.gpu,
                    frame,
                    surface,
                ) {
                    self.features.color_sampler.complete_with_error(
                        request,
                        format!("preparing current layer texture: {error:#}"),
                    );
                    return;
                }
                let Some(size) = self.state.document.surfaces.surface_texture_size(surface) else {
                    self.features.color_sampler.complete_with_error(
                        request,
                        "current layer texture is unavailable".to_owned(),
                    );
                    return;
                };
                let Some(position) = uv_to_texel(uv, size) else {
                    self.features.color_sampler.complete_with_error(
                        request,
                        "color sample UV is outside the current layer texture".to_owned(),
                    );
                    return;
                };
                let Some(texture) = self.state.document.surfaces.surface_texture(surface) else {
                    self.features.color_sampler.complete_with_error(
                        request,
                        "current layer texture is unavailable".to_owned(),
                    );
                    return;
                };
                self.features
                    .color_sampler
                    .enqueue(frame, texture, size, position, request);
            }
        }
    }

    pub(super) fn prepare_tool_preview_output(
        &mut self,
        frame: &mut GpuFrame,
        request: ToolPreviewRequest,
    ) -> Result<()> {
        self.features.tool_preview.render(
            frame,
            &mut self.state.gpu,
            &mut self.state.transient,
            &mut self.scene_capture,
            &self.features.scene_capture_pipelines,
            request,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::uv_to_texel;

    #[test]
    fn uv_to_texel_clamps_unit_edges() {
        assert_eq!(uv_to_texel([0.0, 0.0], [4, 8]), Some([0, 0]));
        assert_eq!(uv_to_texel([1.0, 1.0], [4, 8]), Some([3, 7]));
        assert_eq!(uv_to_texel([0.5, 0.5], [4, 8]), Some([2, 4]));
    }

    #[test]
    fn uv_to_texel_rejects_invalid_coordinates() {
        assert_eq!(uv_to_texel([-0.01, 0.5], [4, 8]), None);
        assert_eq!(uv_to_texel([0.5, 1.01], [4, 8]), None);
        assert_eq!(uv_to_texel([f32::NAN, 0.5], [4, 8]), None);
        assert_eq!(uv_to_texel([0.5, f32::INFINITY], [4, 8]), None);
        assert_eq!(uv_to_texel([0.5, 0.5], [0, 8]), None);
    }
}
