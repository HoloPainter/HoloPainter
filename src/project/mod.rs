mod reader;
mod v1;
mod validate;
mod writer;

use crate::core::{
    camera::OrbitCamera, document::ActiveLayerPart, surface::LayerId, uv_view::UvViewTransform,
    viewport_shading::ViewportShading, viewport_visibility::ViewportSceneVisibility,
    wireframe::WireframeStyle,
};

use std::sync::LazyLock;

use glam::{Quat, Vec2, Vec3};

const DEFAULT_VIEW_RESOURCE: &str = "project/default.view.ron";

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectEditorState {
    pub document_focus: DocumentFocusState,
    pub view: EditorViewState,
    pub layer_tree: LayerTreeEditorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentFocusState {
    pub active_layer_id: LayerId,
    pub active_layer_part: ActiveLayerPart,
    pub focused_material_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorViewState {
    pub camera: OrbitCamera,
    pub shading: ViewportShading,
    pub gizmo_visible: bool,
    pub background_color: [f32; 3],
    pub viewport_wireframe_visible: bool,
    pub viewport_wireframe: WireframeStyle,
    pub scene_visibility: ViewportSceneVisibility,
    pub surface_mirror_x_enabled: bool,
    pub surface_mirror_x_plane: f32,
    pub surface_mirror_x_plane_visible: bool,
    pub uv_view_transform: UvViewTransform,
    pub uv_background_color: [f32; 3],
    pub uv_wireframe_visible: bool,
    pub uv_wireframe: WireframeStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerTreeEditorState {
    pub collapsed_layer_group_ids: Vec<LayerId>,
}

static DEFAULT_VIEW: LazyLock<Result<EditorViewState, String>> = LazyLock::new(|| {
    let view: EditorViewStateV1 = crate::persistence::parse_ron(
        crate::embedded_resources::text(DEFAULT_VIEW_RESOURCE)?,
        "embedded default project view",
    )?;
    validate::validate_editor_view_state_v1(&view).map_err(|error| error.to_string())?;
    Ok(decode_editor_view_state(&view))
});

pub(crate) fn default_editor_view_state() -> EditorViewState {
    DEFAULT_VIEW
        .as_ref()
        .expect("embedded default project view must be valid")
        .clone()
}

pub(crate) fn decode_editor_view_state(view: &EditorViewStateV1) -> EditorViewState {
    let camera = view.viewport_3d.camera;
    let camera_defaults = OrbitCamera::default();
    EditorViewState {
        camera: OrbitCamera {
            target: Vec3::from_array(camera.target),
            orientation: Quat::from_array(camera.orientation),
            distance: camera.distance,
            projection: match camera.projection {
                CameraProjectionV1::Perspective => {
                    crate::core::camera::CameraProjection::Perspective
                }
                CameraProjectionV1::Orthographic => {
                    crate::core::camera::CameraProjection::Orthographic
                }
            },
            fov_y_radians: camera.fov_y_radians,
            orthographic_height: camera.orthographic_height,
            near: camera_defaults.near,
            far: camera_defaults.far,
        },
        shading: match view.viewport_3d.shading {
            ViewportShadingV1::Unlit => ViewportShading::Unlit,
            ViewportShadingV1::Shade => ViewportShading::Shade,
        },
        gizmo_visible: view.viewport_3d.gizmo_visible,
        background_color: view.viewport_3d.background_color,
        viewport_wireframe_visible: view.viewport_3d.wireframe.visible,
        viewport_wireframe: WireframeStyle::new(
            view.viewport_3d.wireframe.color,
            view.viewport_3d.wireframe.opacity,
        ),
        scene_visibility: ViewportSceneVisibility::default(),
        surface_mirror_x_enabled: view.viewport_3d.surface_mirror.x_enabled,
        surface_mirror_x_plane: view.viewport_3d.surface_mirror.x_plane,
        surface_mirror_x_plane_visible: view.viewport_3d.surface_mirror.x_plane_visible,
        uv_view_transform: UvViewTransform {
            center_uv: Vec2::from_array(view.uv.transform.center_uv),
            zoom: view.uv.transform.zoom,
            rotation_radians: view.uv.transform.rotation_radians,
        },
        uv_background_color: view.uv.background_color,
        uv_wireframe_visible: view.uv.wireframe.visible,
        uv_wireframe: WireframeStyle::new(view.uv.wireframe.color, view.uv.wireframe.opacity),
    }
}

impl ProjectEditorState {
    #[cfg(test)]
    pub(crate) fn for_document(
        _document: &crate::core::document::Document,
        active_layer_id: LayerId,
    ) -> Self {
        Self {
            document_focus: DocumentFocusState {
                active_layer_id,
                active_layer_part: ActiveLayerPart::Content,
                focused_material_index: 0,
            },
            view: default_editor_view_state(),
            layer_tree: LayerTreeEditorState {
                collapsed_layer_group_ids: Vec::new(),
            },
        }
    }
}

pub use reader::{LoadedProject, LoadedSurface, OpenProjectError, open_project};
pub use v1::*;
pub use validate::{ProjectValidationError, validate_project_v1};
pub use writer::{SaveProjectError, save_project};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_view_is_valid_and_roundtrips() {
        let view: EditorViewStateV1 =
            ron::from_str(crate::embedded_resources::text(DEFAULT_VIEW_RESOURCE).unwrap())
                .expect("default view parses");
        validate::validate_editor_view_state_v1(&view).expect("default view validates");
        let encoded = ron::ser::to_string(&view).expect("default view serializes");
        let decoded: EditorViewStateV1 = ron::from_str(&encoded).expect("default view roundtrips");
        assert_eq!(decoded, view);
        assert_eq!(default_editor_view_state(), decode_editor_view_state(&view));
    }

    #[test]
    fn view_validation_rejects_invalid_color_opacity_mirror_and_transform() {
        let baseline: EditorViewStateV1 =
            ron::from_str(crate::embedded_resources::text(DEFAULT_VIEW_RESOURCE).unwrap()).unwrap();

        let mut view = baseline.clone();
        view.viewport_3d.background_color[0] = f32::NAN;
        assert!(validate::validate_editor_view_state_v1(&view).is_err());

        let mut view = baseline.clone();
        view.uv.wireframe.opacity = 1.01;
        assert!(validate::validate_editor_view_state_v1(&view).is_err());

        let mut view = baseline.clone();
        view.viewport_3d.surface_mirror.x_plane = f32::INFINITY;
        assert!(validate::validate_editor_view_state_v1(&view).is_err());

        let mut view = baseline;
        view.uv.transform.zoom = 0.0;
        assert!(validate::validate_editor_view_state_v1(&view).is_err());
    }
}
