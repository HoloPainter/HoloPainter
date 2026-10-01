mod color_picker;
mod tool_cursor;
pub mod uv_view;
pub mod viewport_3d;

use eframe::egui;
use glam::{Vec2, Vec3, Vec4};

use crate::{
    application::{AppState, Command},
    ui::{
        input::view_navigation::{ViewNavigationAction, ViewNavigationFrame, dominant_drag_delta},
        view_output::ViewOutput,
    },
};

#[derive(Debug, Clone, Copy, Default)]
struct BrushSizeDragState {
    active: Option<ActiveBrushSizeDrag>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveBrushSizeDrag {
    preset_index: usize,
    start_size_scene_ratio: f32,
    start_size_screen_points: f32,
    overlay_anchor_screen: egui::Pos2,
}

fn update_brush_size_drag(
    ctx: &egui::Context,
    id: egui::Id,
    navigation: ViewNavigationFrame,
    start_size_screen_points: Option<f32>,
    state: &AppState,
    output: &mut ViewOutput,
) -> Option<egui::Pos2> {
    let mut drag = ctx
        .data_mut(|data| data.get_persisted::<BrushSizeDragState>(id))
        .unwrap_or_default();

    if navigation.started() && navigation.action() == Some(ViewNavigationAction::BrushSize) {
        drag.active = start_size_screen_points
            .filter(|size| size.is_finite() && *size > f32::EPSILON)
            .and_then(|start_size_screen_points| {
                state.active_stroke_preset_index().and_then(|preset_index| {
                    state
                        .stroke_tool_preset(preset_index)
                        .map(|preset| ActiveBrushSizeDrag {
                            preset_index,
                            start_size_scene_ratio: preset.stroke_op.size_scene_ratio(),
                            start_size_screen_points,
                            overlay_anchor_screen: navigation.start_position(),
                        })
                })
            });
    }

    let active_drag = (navigation.action() == Some(ViewNavigationAction::BrushSize))
        .then_some(drag.active)
        .flatten();
    if let Some(brush_drag) = active_drag
        && let Some(mut preset) = state.stroke_tool_preset(brush_drag.preset_index).cloned()
    {
        let target = brush_size_from_drag(
            brush_drag.start_size_scene_ratio,
            brush_drag.start_size_screen_points,
            navigation.total_delta(),
        );
        if preset.stroke_op.set_size_scene_ratio(target) {
            output.push(Command::UpdateRuntimeStrokePreset {
                preset_index: brush_drag.preset_index,
                preset,
            });
            output.request_repaint();
        }
    }

    if navigation.ended() || navigation.action() != Some(ViewNavigationAction::BrushSize) {
        drag.active = None;
    }
    ctx.data_mut(|data| data.insert_persisted(id, drag));
    active_drag.map(|drag| drag.overlay_anchor_screen)
}

fn brush_overlay_screen_position(
    live_pointer: Option<egui::Pos2>,
    view_rect: egui::Rect,
    normal_hover_allowed: bool,
    brush_size_anchor: Option<egui::Pos2>,
) -> Option<egui::Pos2> {
    brush_size_anchor
        .or_else(|| normal_hover_allowed.then_some(live_pointer).flatten())
        .filter(|pointer| view_rect.contains(*pointer))
}

fn brush_size_from_drag(
    start_size_scene_ratio: f32,
    start_size_screen_points: f32,
    total_delta: egui::Vec2,
) -> f32 {
    if !start_size_scene_ratio.is_finite()
        || !start_size_screen_points.is_finite()
        || start_size_screen_points <= f32::EPSILON
    {
        return start_size_scene_ratio;
    }
    let target_size_screen_points =
        (start_size_screen_points + dominant_drag_delta(total_delta)).max(0.0);
    if !target_size_screen_points.is_finite() {
        return start_size_scene_ratio;
    }
    start_size_scene_ratio * target_size_screen_points / start_size_screen_points
}

fn surface_brush_overlay_size_points(
    request: &crate::renderer::SurfaceBrushOverlayRequest,
) -> Option<f32> {
    projected_surface_size_points(
        request.viewport_view_proj_gl,
        request.viewport_size,
        request.dab.world_pos,
        request.dab.tangent_x,
        request.dab.tangent_y,
        request.stroke_op.radius_world() * request.dab.radius_scale,
    )
}

fn projected_surface_size_points(
    view_proj: glam::Mat4,
    viewport_size: [u32; 2],
    center_world: Vec3,
    tangent_x: Vec3,
    tangent_y: Vec3,
    radius_world: f32,
) -> Option<f32> {
    if !radius_world.is_finite() || radius_world <= f32::EPSILON {
        return None;
    }
    let center = world_to_viewport_points(view_proj, center_world, viewport_size)?;
    let offsets = [
        tangent_x * radius_world,
        -tangent_x * radius_world,
        tangent_y * radius_world,
        -tangent_y * radius_world,
    ];
    let mut radius_points = 0.0_f32;
    for offset in offsets {
        let edge = world_to_viewport_points(view_proj, center_world + offset, viewport_size)?;
        radius_points = radius_points.max((edge - center).length());
    }
    let size_points = radius_points * 2.0;
    (size_points.is_finite() && size_points > f32::EPSILON).then_some(size_points)
}

fn world_to_viewport_points(
    view_proj: glam::Mat4,
    world: Vec3,
    viewport_size: [u32; 2],
) -> Option<Vec2> {
    let clip = view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w.abs() <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0].max(1) as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1].max(1) as f32,
    ))
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use glam::{Mat4, Vec3};

    use super::{
        brush_overlay_screen_position, brush_size_from_drag, projected_surface_size_points,
    };

    #[test]
    fn brush_overlay_prefers_the_brush_size_drag_anchor() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));

        assert_eq!(
            brush_overlay_screen_position(
                Some(egui::pos2(300.0, 200.0)),
                rect,
                false,
                Some(egui::pos2(100.0, 100.0)),
            ),
            Some(egui::pos2(100.0, 100.0))
        );
    }

    #[test]
    fn brush_overlay_uses_live_pointer_for_normal_hover() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));

        assert_eq!(
            brush_overlay_screen_position(Some(egui::pos2(300.0, 200.0)), rect, true, None,),
            Some(egui::pos2(300.0, 200.0))
        );
        assert_eq!(
            brush_overlay_screen_position(Some(egui::pos2(300.0, 200.0)), rect, false, None,),
            None
        );
    }

    #[test]
    fn brush_size_drag_changes_screen_size_linearly_across_both_axes() {
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(20.0, 0.0)) - 0.015).abs() < 1.0e-6);
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(-20.0, 0.0)) - 0.005).abs() < 1.0e-6);
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(0.0, 20.0)) - 0.015).abs() < 1.0e-6);
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(0.0, -20.0)) - 0.005).abs() < 1.0e-6);
    }

    #[test]
    fn brush_size_drag_uses_the_dominant_axis_without_cancellation() {
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(20.0, -18.0)) - 0.015).abs() < 1.0e-6);
        assert!((brush_size_from_drag(0.01, 40.0, egui::vec2(18.0, -20.0)) - 0.005).abs() < 1.0e-6);
    }

    #[test]
    fn brush_size_drag_does_not_return_a_negative_ratio() {
        assert_eq!(
            brush_size_from_drag(0.01, 40.0, egui::vec2(-100.0, 0.0)),
            0.0
        );
    }

    #[test]
    fn projected_surface_size_scales_with_world_radius() {
        let small = projected_surface_size_points(
            Mat4::IDENTITY,
            [200, 100],
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            0.1,
        )
        .expect("identity projection should produce a size");
        let large = projected_surface_size_points(
            Mat4::IDENTITY,
            [200, 100],
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            0.2,
        )
        .expect("identity projection should produce a size");

        assert!((small - 20.0).abs() < 1.0e-4);
        assert!((large - 40.0).abs() < 1.0e-4);
    }
}
