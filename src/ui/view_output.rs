#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEyedropperTarget {
    CurrentColor,
    SolidFill {
        layer_id: crate::core::surface::LayerId,
        edit_session: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiRequest {
    SelectDecalAsset {
        asset_id: String,
    },
    ImportDecalImage,
    ImportTextureResource {
        required_tags: Vec<String>,
    },
    OpenImageAssetLibrary,
    SampleColor(crate::renderer::ColorSampleUiRequest),
    StartScreenEyedropper(ScreenEyedropperTarget),
    OpenAdjustmentEditor {
        layer_id: crate::core::surface::LayerId,
    },
}

use crate::{
    application::{Command, EditorAction},
    renderer::view::{ToolPreviewRequest, UvViewRequest, ViewportViewRequest},
    renderer::{ViewCommand, ViewRequest},
};

#[derive(Debug, Default)]
pub struct ViewOutput {
    pub commands: Vec<Command>,
    pub actions: Vec<EditorAction>,
    pub view_requests: Vec<ViewRequest>,
    pub ui_requests: Vec<UiRequest>,
    pub needs_repaint: bool,
}

impl ViewOutput {
    pub fn command(command: Command) -> Self {
        Self {
            commands: vec![command],
            actions: Vec::new(),
            view_requests: Vec::new(),
            ui_requests: Vec::new(),
            needs_repaint: false,
        }
    }

    pub fn repaint() -> Self {
        Self {
            commands: Vec::new(),
            actions: Vec::new(),
            view_requests: Vec::new(),
            ui_requests: Vec::new(),
            needs_repaint: true,
        }
    }

    pub fn push(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn push_action(&mut self, action: EditorAction) {
        self.actions.push(action);
    }

    pub fn request_viewport_render(&mut self, request: ViewportViewRequest) {
        merge_view_request(
            &mut self.view_requests,
            ViewCommand::RenderViewport {
                camera: request.camera,
                view_proj: request.view_proj,
                camera_world: request.camera_world,
                viewport_size: request.viewport_size,
                brush_overlay_request: request.brush_overlay_request,
                selection_overlay_request: request.selection_overlay_request,
                mirror_plane_overlay_request: request.mirror_plane_overlay_request,
                decal_overlay_request: request.decal_overlay_request,
                scene_visibility: request.scene_visibility,
                show_wireframe: request.show_wireframe,
                wireframe_style: request.wireframe_style,
                background_color: request.background_color,
                shading: request.shading,
            },
        );
    }

    pub fn request_uv_view_render(&mut self, request: UvViewRequest) {
        merge_view_request(
            &mut self.view_requests,
            ViewCommand::RenderUvView {
                material_index: request.material_index,
                uv_view_size: request.uv_view_size,
                transform: request.transform,
                brush_overlay_request: request.brush_overlay_request,
                selection_overlay_request: request.selection_overlay_request,
                show_wireframe: request.show_wireframe,
                wireframe_style: request.wireframe_style,
                background_color: request.background_color,
            },
        );
    }

    pub fn request_tool_preview_render(&mut self, request: ToolPreviewRequest) {
        merge_view_request(
            &mut self.view_requests,
            ViewCommand::RenderToolPreviews(request),
        );
    }

    pub fn request_ui(&mut self, request: UiRequest) {
        self.ui_requests.push(request);
    }

    pub fn request_repaint(&mut self) {
        self.needs_repaint = true;
    }

    pub fn extend(&mut self, other: ViewOutput) {
        self.commands.extend(other.commands);
        self.actions.extend(other.actions);
        merge_view_requests(&mut self.view_requests, other.view_requests);
        self.ui_requests.extend(other.ui_requests);
        self.needs_repaint |= other.needs_repaint;
    }
}

pub(crate) fn merge_view_requests(target: &mut Vec<ViewRequest>, requests: Vec<ViewRequest>) {
    for request in requests {
        merge_view_request(target, request);
    }
}

fn merge_view_request(target: &mut Vec<ViewRequest>, request: ViewRequest) {
    if let Some(existing) = target
        .iter_mut()
        .find(|existing| same_view_request_kind(existing, &request))
    {
        *existing = request;
    } else {
        target.push(request);
    }
}

fn same_view_request_kind(a: &ViewRequest, b: &ViewRequest) -> bool {
    matches!(
        (a, b),
        (
            ViewCommand::RenderViewport { .. },
            ViewCommand::RenderViewport { .. }
        ) | (
            ViewCommand::RenderUvView { .. },
            ViewCommand::RenderUvView { .. }
        ) | (
            ViewCommand::RenderToolPreviews(_),
            ViewCommand::RenderToolPreviews(_),
        )
    )
}
