use crate::{
    application::ReducerOutput,
    core::{
        camera::OrbitCamera, document::MeshId, uv_view::UvViewTransform,
        viewport_shading::ViewportShading, wireframe::WireframeStyle,
    },
};

use super::state::{AppState, ToolState, ViewState};

pub fn set_viewport_gizmo_visible(view: &mut ViewState, visible: bool) -> ReducerOutput {
    view.gizmo_visible = visible;
    ReducerOutput::default()
}

pub fn set_viewport_shading(view: &mut ViewState, shading: ViewportShading) -> ReducerOutput {
    view.shading = shading;
    ReducerOutput::default()
}

pub fn set_viewport_wireframe_visible(view: &mut ViewState, visible: bool) -> ReducerOutput {
    view.viewport_wireframe_visible = visible;
    ReducerOutput::default()
}

pub fn set_viewport_wireframe_style(view: &mut ViewState, style: WireframeStyle) -> ReducerOutput {
    view.viewport_wireframe = style.normalized();
    ReducerOutput::default()
}

pub fn set_viewport_background_color(view: &mut ViewState, color: [f32; 3]) -> ReducerOutput {
    view.background_color = color.map(|component| component.clamp(0.0, 1.0));
    ReducerOutput::default()
}

pub fn set_viewport_mesh_visible(
    state: &mut AppState,
    mesh_id: MeshId,
    visible: bool,
) -> ReducerOutput {
    let exists = state.document().is_some_and(|document| {
        document
            .mesh
            .mesh_objects
            .iter()
            .any(|mesh| mesh.id == mesh_id)
    });
    if exists {
        state
            .view_mut()
            .scene_visibility
            .set_mesh_visible(mesh_id, visible);
    }
    ReducerOutput::default()
}

pub fn set_viewport_material_visible(
    state: &mut AppState,
    material_index: usize,
    visible: bool,
) -> ReducerOutput {
    let exists = state
        .document()
        .is_some_and(|document| material_index < document.materials.len());
    if exists {
        state
            .view_mut()
            .scene_visibility
            .set_material_visible(material_index.into(), visible);
    }
    ReducerOutput::default()
}

pub fn set_uv_wireframe_visible(view: &mut ViewState, visible: bool) -> ReducerOutput {
    view.uv_wireframe_visible = visible;
    ReducerOutput::default()
}

pub fn set_uv_wireframe_style(view: &mut ViewState, style: WireframeStyle) -> ReducerOutput {
    view.uv_wireframe = style.normalized();
    ReducerOutput::default()
}

pub fn set_uv_view_background_color(view: &mut ViewState, color: [f32; 3]) -> ReducerOutput {
    view.uv_background_color = color.map(|component| component.clamp(0.0, 1.0));
    ReducerOutput::default()
}

pub fn set_current_color(tool: &mut ToolState, color: [f32; 3]) -> ReducerOutput {
    tool.current_color = color.map(|component| component.clamp(0.0, 1.0));
    ReducerOutput::default()
}

pub fn set_camera(view: &mut ViewState, camera: OrbitCamera) -> ReducerOutput {
    view.camera = camera;
    ReducerOutput::default()
}

pub fn set_uv_view_transform(view: &mut ViewState, transform: UvViewTransform) -> ReducerOutput {
    view.uv_view_transform = transform.normalized();
    view.uv_view_transform_initialized = true;
    ReducerOutput::default()
}

pub fn reset_uv_view_transform(view: &mut ViewState) {
    view.uv_view_transform = UvViewTransform::default();
    view.uv_view_transform_initialized = false;
}

#[cfg(test)]
mod tests {
    use super::{
        set_uv_view_background_color, set_uv_wireframe_style, set_viewport_background_color,
        set_viewport_gizmo_visible, set_viewport_shading, set_viewport_wireframe_style,
    };
    use crate::{
        application::AppState,
        core::{viewport_shading::ViewportShading, wireframe::WireframeStyle},
    };

    #[test]
    fn viewport_shading_defaults_to_shade_and_can_be_changed() {
        let mut state = AppState::default();
        assert_eq!(state.viewport_shading(), ViewportShading::Shade);

        set_viewport_shading(state.view_mut(), ViewportShading::Unlit);
        assert_eq!(state.viewport_shading(), ViewportShading::Unlit);
    }

    #[test]
    fn viewport_gizmo_is_visible_by_default_and_can_be_toggled() {
        let mut state = AppState::default();
        let camera = state.camera().clone();
        let wireframe_visible = state.viewport_wireframe_visible();
        let background_color = state.viewport_background_color();

        assert!(state.viewport_gizmo_visible());

        set_viewport_gizmo_visible(state.view_mut(), false);
        assert!(!state.viewport_gizmo_visible());
        assert_eq!(state.camera(), &camera);
        assert_eq!(state.viewport_wireframe_visible(), wireframe_visible);
        assert_eq!(state.viewport_background_color(), background_color);

        set_viewport_gizmo_visible(state.view_mut(), true);
        assert!(state.viewport_gizmo_visible());
    }

    #[test]
    fn viewport_background_colors_default_and_update_independently() {
        let mut state = AppState::default();
        assert_eq!(state.viewport_background_color(), [0.0, 0.0, 0.0]);
        assert_eq!(state.uv_view_background_color(), [0.055, 0.060, 0.070]);

        set_viewport_background_color(state.view_mut(), [-0.5, 0.4, 1.5]);
        assert_eq!(state.viewport_background_color(), [0.0, 0.4, 1.0]);
        assert_eq!(state.uv_view_background_color(), [0.055, 0.060, 0.070]);

        set_uv_view_background_color(state.view_mut(), [0.8, -1.0, 0.25]);
        assert_eq!(state.viewport_background_color(), [0.0, 0.4, 1.0]);
        assert_eq!(state.uv_view_background_color(), [0.8, 0.0, 0.25]);
    }

    #[test]
    fn wireframe_styles_default_and_update_independently() {
        let mut state = AppState::default();
        assert_eq!(
            state.viewport_wireframe_style(),
            WireframeStyle::new([30.0 / 255.0, 230.0 / 255.0, 1.0], 1.0)
        );
        assert_eq!(
            state.uv_wireframe_style(),
            WireframeStyle::new([0.05, 0.90, 1.0], 0.85)
        );

        set_viewport_wireframe_style(state.view_mut(), WireframeStyle::new([-0.5, 0.4, 1.5], 2.0));
        assert_eq!(
            state.viewport_wireframe_style(),
            WireframeStyle::new([0.0, 0.4, 1.0], 1.0)
        );
        assert_eq!(
            state.uv_wireframe_style(),
            WireframeStyle::new([0.05, 0.90, 1.0], 0.85)
        );

        set_uv_wireframe_style(
            state.view_mut(),
            WireframeStyle::new([0.8, -1.0, 0.25], -0.5),
        );
        assert_eq!(
            state.viewport_wireframe_style(),
            WireframeStyle::new([0.0, 0.4, 1.0], 1.0)
        );
        assert_eq!(
            state.uv_wireframe_style(),
            WireframeStyle::new([0.8, 0.0, 0.25], 0.0)
        );
    }
}
