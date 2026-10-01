use eframe::egui;
use glam::Vec2;

use crate::core::uv_view::UvViewTransform;

pub fn uv_to_screen(
    uv: Vec2,
    rect: egui::Rect,
    canvas_size: [u32; 2],
    transform: UvViewTransform,
) -> egui::Pos2 {
    let local = transform.uv_to_view_px(
        uv,
        [rect.width().round() as u32, rect.height().round() as u32],
        canvas_size,
    );
    rect.min + egui::vec2(local.x, local.y)
}

pub fn screen_to_uv(
    pointer: egui::Pos2,
    rect: egui::Rect,
    canvas_size: [u32; 2],
    transform: UvViewTransform,
) -> Vec2 {
    let local = pointer - rect.min;
    transform.view_px_to_uv(
        Vec2::new(local.x, local.y),
        [rect.width().round() as u32, rect.height().round() as u32],
        canvas_size,
    )
}

pub fn uv_rect_to_screen_quad(
    start_uv: Vec2,
    end_uv: Vec2,
    rect: egui::Rect,
    canvas_size: [u32; 2],
    transform: UvViewTransform,
) -> [egui::Pos2; 4] {
    let min_uv = start_uv.min(end_uv);
    let max_uv = start_uv.max(end_uv);
    [
        min_uv,
        Vec2::new(max_uv.x, min_uv.y),
        max_uv,
        Vec2::new(min_uv.x, max_uv.y),
    ]
    .map(|uv| uv_to_screen(uv, rect, canvas_size, transform))
}
