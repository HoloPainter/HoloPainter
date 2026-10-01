//! Presentation boundary between renderer outputs and UI texture bindings.
//!
//! `TextureOutputs` exposes the texture views the presenter needs without
//! letting the presenter inspect `RenderEngine`, feature workspaces, or GPU
//! stores individually.

use std::collections::BTreeSet;

use eframe::egui_wgpu::wgpu;

use crate::{
    renderer::mutation::PresentDirty,
    renderer::view::{ToolPreviewRequest, UvViewRequest, ViewportViewRequest},
};

#[derive(Debug, Clone, Copy)]
pub struct MaterialTextureOutput<'a> {
    material_index: usize,
    view: &'a wgpu::TextureView,
    size: [usize; 2],
}

impl<'a> MaterialTextureOutput<'a> {
    pub fn new(material_index: usize, view: &'a wgpu::TextureView, size: [usize; 2]) -> Self {
        Self {
            material_index,
            view,
            size,
        }
    }

    pub fn material_index(&self) -> usize {
        self.material_index
    }

    pub fn view(&self) -> &'a wgpu::TextureView {
        self.view
    }

    pub fn size(&self) -> [usize; 2] {
        self.size
    }
}

#[derive(Debug)]
pub struct TextureOutputs<'a> {
    materials: Vec<MaterialTextureOutput<'a>>,
    viewport: &'a wgpu::TextureView,
    uv_view: &'a wgpu::TextureView,
    tool_preview: &'a wgpu::TextureView,
}

impl<'a> TextureOutputs<'a> {
    pub fn new(
        materials: Vec<MaterialTextureOutput<'a>>,
        viewport: &'a wgpu::TextureView,
        uv_view: &'a wgpu::TextureView,
        tool_preview: &'a wgpu::TextureView,
    ) -> Self {
        Self {
            materials,
            viewport,
            uv_view,
            tool_preview,
        }
    }

    pub fn materials(&self) -> &[MaterialTextureOutput<'a>] {
        &self.materials
    }

    pub fn viewport(&self) -> &'a wgpu::TextureView {
        self.viewport
    }

    pub fn uv_view(&self) -> &'a wgpu::TextureView {
        self.uv_view
    }

    pub fn tool_preview(&self) -> &'a wgpu::TextureView {
        self.tool_preview
    }

    pub fn material_count(&self) -> usize {
        self.materials.len()
    }
}

/// Engine-owned presentation state.
///
/// Presentation state exposes the texture outputs consumed by the UI. Per-frame
/// output requests are assembled at the render-engine boundary from explicit
/// view requests and the aggregate mutation log instead of being queued by
/// individual features.
#[derive(Debug, Default)]
pub(crate) struct PresentationState;

impl PresentationState {
    pub(crate) fn texture_outputs<'a>(
        &self,
        materials: Vec<MaterialTextureOutput<'a>>,
        viewport: &'a wgpu::TextureView,
        uv_view: &'a wgpu::TextureView,
        tool_preview: &'a wgpu::TextureView,
    ) -> TextureOutputs<'a> {
        TextureOutputs::new(materials, viewport, uv_view, tool_preview)
    }
}

/// Renderer-owned presentation work requested by commands but prepared by the
/// engine boundary.
#[derive(Debug, Default)]
pub(crate) struct OutputRequests {
    material_composites: BTreeSet<usize>,
    viewport: Option<ViewportOutputRequest>,
    uv_view: Option<UvViewOutputRequest>,
    tool_preview: Option<ToolPreviewOutputRequest>,
}

impl OutputRequests {
    pub(crate) fn is_empty(&self) -> bool {
        self.material_composites.is_empty()
            && self.viewport.is_none()
            && self.uv_view.is_none()
            && self.tool_preview.is_none()
    }

    pub(crate) fn push_material_composite(&mut self, material_index: usize) {
        self.material_composites.insert(material_index);
    }

    pub(crate) fn push_viewport(&mut self, request: ViewportOutputRequest) {
        self.viewport = Some(request);
    }

    pub(crate) fn push_uv_view(&mut self, request: UvViewOutputRequest) {
        self.uv_view = Some(request);
    }

    pub(crate) fn push_tool_preview(&mut self, request: ToolPreviewOutputRequest) {
        self.tool_preview = Some(request);
    }

    pub(crate) fn present_dirty(&self) -> PresentDirty {
        let mut dirty = PresentDirty::default();
        if !self.material_composites.is_empty() {
            dirty.request_material_textures();
        }
        if self.viewport.is_some() {
            dirty.request_viewport();
        }
        if self.uv_view.is_some() {
            dirty.request_uv_view();
        }
        if self.tool_preview.is_some() {
            dirty.request_tool_preview();
        }
        dirty
    }

    pub(crate) fn into_requests(self) -> Vec<OutputRequest> {
        let material_count = self.material_composites.len();
        let mut requests = Vec::with_capacity(
            material_count
                + usize::from(self.viewport.is_some())
                + usize::from(self.uv_view.is_some())
                + usize::from(self.tool_preview.is_some()),
        );
        requests.extend(self.material_composites.into_iter().map(|material_index| {
            OutputRequest::MaterialComposite(MaterialCompositeRequest { material_index })
        }));
        if let Some(request) = self.viewport {
            requests.push(OutputRequest::Viewport(request));
        }
        if let Some(request) = self.uv_view {
            requests.push(OutputRequest::UvView(request));
        }
        if let Some(request) = self.tool_preview {
            requests.push(OutputRequest::ToolPreview(request));
        }
        requests
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum OutputRequest {
    MaterialComposite(MaterialCompositeRequest),
    Viewport(ViewportOutputRequest),
    UvView(UvViewOutputRequest),
    ToolPreview(ToolPreviewOutputRequest),
}

#[derive(Debug)]
pub(crate) struct MaterialCompositeRequest {
    pub(crate) material_index: usize,
}

pub(crate) type ViewportOutputRequest = ViewportViewRequest;
pub(crate) type UvViewOutputRequest = UvViewRequest;
pub(crate) type ToolPreviewOutputRequest = ToolPreviewRequest;
