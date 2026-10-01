use eframe::egui;

use crate::{
    application::{AppState, PaintEditBlockReason, PaintEditDecision},
    core::{
        stroke::StrokeSpace,
        tool::{ColorSampleSource, ToolBehavior, ToolDefinition},
    },
    localization::Localization,
    renderer::{ColorSamplePreview, ColorSampleView},
};

const BLOCK_HINT_MAX_WIDTH: f32 = 320.0;
const BLOCK_HINT_VIEW_MARGIN: f32 = 8.0;
const BLOCK_HINT_HORIZONTAL_OFFSET: f32 = 16.0;
const BLOCK_HINT_VERTICAL_OFFSET: f32 = 20.0;
const BLOCK_HINT_HORIZONTAL_PADDING: i8 = 8;
const BLOCK_HINT_VERTICAL_PADDING: i8 = 6;
const COLOR_PREVIEW_SIZE: f32 = 28.0;
const COLOR_PREVIEW_VIEW_MARGIN: f32 = 8.0;
const COLOR_PREVIEW_HORIZONTAL_OFFSET: f32 = 18.0;
const COLOR_PREVIEW_VERTICAL_OFFSET: f32 = 20.0;
const COLOR_PREVIEW_MAX_STALE_DISTANCE: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolCursorAppearance {
    Default,
    HiddenBrushOverlay,
    Crosshair,
    NotAllowed(PaintEditBlockReason),
}

pub(super) fn apply_tool_cursor(
    ui: &egui::Ui,
    l10n: &Localization,
    state: &AppState,
    view_rect: egui::Rect,
    space: StrokeSpace,
    brush_overlay_visible: bool,
    paint_edit: PaintEditDecision<()>,
) {
    match tool_cursor_appearance(state, space, brush_overlay_visible, paint_edit) {
        ToolCursorAppearance::Default => {}
        ToolCursorAppearance::HiddenBrushOverlay => {
            ui.ctx().set_cursor_image(None);
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
        ToolCursorAppearance::Crosshair => {
            ui.ctx().set_cursor_image(None);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        ToolCursorAppearance::NotAllowed(reason) => {
            ui.ctx().set_cursor_image(None);
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
            draw_paint_edit_block_hint(ui, l10n, view_rect, reason);
        }
    }
}

fn tool_cursor_appearance(
    state: &AppState,
    space: StrokeSpace,
    brush_overlay_visible: bool,
    paint_edit: PaintEditDecision<()>,
) -> ToolCursorAppearance {
    let Some(tool) = state.effective_tool_definition() else {
        return ToolCursorAppearance::Default;
    };
    tool_cursor_appearance_for_tool(tool, paint_edit, space, brush_overlay_visible)
}

fn tool_cursor_appearance_for_tool(
    tool: &ToolDefinition,
    paint_edit: PaintEditDecision<()>,
    space: StrokeSpace,
    brush_overlay_visible: bool,
) -> ToolCursorAppearance {
    if let PaintEditDecision::Blocked(reason) = paint_edit
        && tool.edits_paint_target_in_space(space)
    {
        return ToolCursorAppearance::NotAllowed(reason);
    }

    match &tool.behavior {
        ToolBehavior::Stroke { .. } if tool.supports_space(space) && brush_overlay_visible => {
            ToolCursorAppearance::HiddenBrushOverlay
        }
        ToolBehavior::Fill { .. } | ToolBehavior::Decal { .. } => ToolCursorAppearance::Crosshair,
        ToolBehavior::Shape { .. }
        | ToolBehavior::Selection { .. }
        | ToolBehavior::Transform
        | ToolBehavior::ColorPicker => ToolCursorAppearance::Crosshair,
        ToolBehavior::NoOp | ToolBehavior::Stroke { .. } => ToolCursorAppearance::Default,
    }
}

pub(super) fn color_sample_position(
    pointer: egui::Pos2,
    view_rect: egui::Rect,
    view_size: [u32; 2],
) -> Option<[u32; 2]> {
    if !view_rect.contains(pointer) {
        return None;
    }
    let local = pointer - view_rect.min;
    let x = local.x.floor();
    let y = local.y.floor();
    if x < 0.0 || y < 0.0 || x >= view_size[0] as f32 || y >= view_size[1] as f32 {
        return None;
    }
    Some([x as u32, y as u32])
}

pub(super) fn draw_color_sample_preview(
    ui: &egui::Ui,
    view_rect: egui::Rect,
    pointer: egui::Pos2,
    view: ColorSampleView,
    source: ColorSampleSource,
    document_generation: u64,
    preview: Option<ColorSamplePreview>,
) {
    let Some(preview) = preview.filter(|preview| {
        preview.source == source
            && preview.anchor.view == view
            && preview.document_generation == document_generation
    }) else {
        return;
    };
    let sampled_center = view_rect.min
        + egui::vec2(
            preview.anchor.position[0] as f32 + 0.5,
            preview.anchor.position[1] as f32 + 0.5,
        );
    if sampled_center.distance(pointer) > COLOR_PREVIEW_MAX_STALE_DISTANCE {
        return;
    }

    let preview_rect = color_sample_preview_rect(pointer, view_rect);
    let bounds = non_negative_shrunk_rect(view_rect, COLOR_PREVIEW_VIEW_MARGIN);
    let layer_id = egui::LayerId::new(egui::Order::Tooltip, ui.id().with("color_sample_preview"));
    let painter = ui.ctx().layer_painter(layer_id).with_clip_rect(bounds);
    painter.rect_filled(
        preview_rect,
        0.0,
        egui::Color32::from_rgb(preview.rgb[0], preview.rgb[1], preview.rgb[2]),
    );
    painter.rect_stroke(
        preview_rect,
        0.0,
        egui::Stroke::new(2.0, egui::Color32::from_black_alpha(180)),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        preview_rect.shrink(2.0),
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
        egui::StrokeKind::Inside,
    );
}

fn color_sample_preview_rect(pointer: egui::Pos2, view_rect: egui::Rect) -> egui::Rect {
    let size = egui::vec2(COLOR_PREVIEW_SIZE, COLOR_PREVIEW_SIZE);
    let bounds = non_negative_shrunk_rect(view_rect, COLOR_PREVIEW_VIEW_MARGIN);
    let place_left = pointer.x > view_rect.center().x;
    let place_above = pointer.y > view_rect.center().y;
    let desired_min = egui::pos2(
        if place_left {
            pointer.x - COLOR_PREVIEW_HORIZONTAL_OFFSET - size.x
        } else {
            pointer.x + COLOR_PREVIEW_HORIZONTAL_OFFSET
        },
        if place_above {
            pointer.y - COLOR_PREVIEW_VERTICAL_OFFSET - size.y
        } else {
            pointer.y + COLOR_PREVIEW_VERTICAL_OFFSET
        },
    );
    let max_min = egui::pos2(
        (bounds.right() - size.x).max(bounds.left()),
        (bounds.bottom() - size.y).max(bounds.top()),
    );
    let min = egui::pos2(
        desired_min.x.clamp(bounds.left(), max_min.x),
        desired_min.y.clamp(bounds.top(), max_min.y),
    );
    egui::Rect::from_min_size(min, size)
}

fn paint_edit_block_message_keys(reason: PaintEditBlockReason) -> (&'static str, &'static str) {
    let body = match reason {
        PaintEditBlockReason::ActiveLayerIsGroup => "paint-block-layer-group",
        PaintEditBlockReason::ActiveLayerLocked => "paint-block-layer-locked",
        PaintEditBlockReason::ActiveLayerHidden => "paint-block-layer-hidden",
        PaintEditBlockReason::MaterialExcluded => "paint-block-material-excluded",
        PaintEditBlockReason::ActiveTargetNotEditable => "paint-block-target-not-editable",
    };
    ("paint-block-title", body)
}

fn draw_paint_edit_block_hint(
    ui: &egui::Ui,
    l10n: &Localization,
    view_rect: egui::Rect,
    reason: PaintEditBlockReason,
) {
    let Some(pointer) = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|pointer| view_rect.contains(*pointer))
    else {
        return;
    };

    let bounds = non_negative_shrunk_rect(view_rect, BLOCK_HINT_VIEW_MARGIN);
    let frame = egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(
        BLOCK_HINT_HORIZONTAL_PADDING,
        BLOCK_HINT_VERTICAL_PADDING,
    ));
    let frame_margin = frame.total_margin();
    let content_width = (BLOCK_HINT_MAX_WIDTH.min(bounds.width()) - frame_margin.sum().x).max(0.0);
    let (title, body) = paint_edit_block_message_keys(reason);
    let title = l10n.text(title);
    let body = l10n.text(body);
    let title_galley = egui::WidgetText::from(egui::RichText::new(title).strong()).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        content_width,
        egui::TextStyle::Body,
    );
    let body_galley = egui::WidgetText::from(body).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        content_width,
        egui::TextStyle::Body,
    );
    let spacing = ui.spacing().item_spacing.y;
    let content_size = egui::vec2(
        title_galley.size().x.max(body_galley.size().x),
        title_galley.size().y + spacing + body_galley.size().y,
    );
    let bubble_size = content_size + frame_margin.sum();
    let bubble_rect = paint_edit_block_hint_rect(pointer, view_rect, bubble_size);
    let content_rect = bubble_rect - frame_margin;
    let layer_id = egui::LayerId::new(egui::Order::Tooltip, ui.id().with("paint_edit_block_hint"));
    let painter = ui.ctx().layer_painter(layer_id).with_clip_rect(bounds);
    painter.add(frame.paint(content_rect));
    let text_color = ui.visuals().text_color();
    painter.galley(content_rect.min, title_galley, text_color);
    painter.galley(
        content_rect.min + egui::vec2(0.0, content_size.y - body_galley.size().y),
        body_galley,
        text_color,
    );
}

fn paint_edit_block_hint_rect(
    pointer: egui::Pos2,
    view_rect: egui::Rect,
    bubble_size: egui::Vec2,
) -> egui::Rect {
    let bounds = non_negative_shrunk_rect(view_rect, BLOCK_HINT_VIEW_MARGIN);
    let place_left = pointer.x > view_rect.center().x;
    let place_above = pointer.y > view_rect.center().y;
    let desired_min = egui::pos2(
        if place_left {
            pointer.x - BLOCK_HINT_HORIZONTAL_OFFSET - bubble_size.x
        } else {
            pointer.x + BLOCK_HINT_HORIZONTAL_OFFSET
        },
        if place_above {
            pointer.y - BLOCK_HINT_VERTICAL_OFFSET - bubble_size.y
        } else {
            pointer.y + BLOCK_HINT_VERTICAL_OFFSET
        },
    );
    let max_min = egui::pos2(
        (bounds.right() - bubble_size.x).max(bounds.left()),
        (bounds.bottom() - bubble_size.y).max(bounds.top()),
    );
    let min = egui::pos2(
        desired_min.x.clamp(bounds.left(), max_min.x),
        desired_min.y.clamp(bounds.top(), max_min.y),
    );
    egui::Rect::from_min_size(min, bubble_size.max(egui::Vec2::ZERO))
}

fn non_negative_shrunk_rect(rect: egui::Rect, margin: f32) -> egui::Rect {
    let min = rect.min + egui::vec2(margin, margin);
    let max = rect.max - egui::vec2(margin, margin);
    egui::Rect::from_min_max(min, max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::{
        FillScope, SelectionShapeKind, ShapePaintMode, ShapeToolKind, ToolId, ToolSection,
    };

    #[test]
    fn stroke_tool_hides_os_cursor_when_brush_overlay_is_visible() {
        let tool = tool(ToolBehavior::Stroke { preset_index: 0 }, &[StrokeSpace::Uv]);

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Allowed(()),
                StrokeSpace::Uv,
                true
            ),
            ToolCursorAppearance::HiddenBrushOverlay
        );
    }

    #[test]
    fn stroke_tool_uses_default_cursor_without_brush_overlay() {
        let tool = tool(
            ToolBehavior::Stroke { preset_index: 0 },
            &[StrokeSpace::Surface],
        );

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Allowed(()),
                StrokeSpace::Surface,
                false
            ),
            ToolCursorAppearance::Default
        );
    }

    #[test]
    fn no_op_tool_always_uses_the_default_cursor() {
        let tool = tool(ToolBehavior::NoOp, &[]);

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Unavailable,
                StrokeSpace::Uv,
                false,
            ),
            ToolCursorAppearance::Default
        );
    }

    #[test]
    fn paint_edit_block_takes_priority_for_stroke_and_fill_tools() {
        let stroke = tool(ToolBehavior::Stroke { preset_index: 0 }, &[StrokeSpace::Uv]);
        let fill = tool(
            ToolBehavior::Fill {
                scope: FillScope::Material,
            },
            &[StrokeSpace::Uv, StrokeSpace::Surface],
        );

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &stroke,
                PaintEditDecision::Blocked(PaintEditBlockReason::ActiveLayerHidden),
                StrokeSpace::Uv,
                true
            ),
            ToolCursorAppearance::NotAllowed(PaintEditBlockReason::ActiveLayerHidden)
        );
        assert_eq!(
            tool_cursor_appearance_for_tool(
                &fill,
                PaintEditDecision::Blocked(PaintEditBlockReason::MaterialExcluded),
                StrokeSpace::Surface,
                false
            ),
            ToolCursorAppearance::NotAllowed(PaintEditBlockReason::MaterialExcluded)
        );
    }

    #[test]
    fn fill_tool_uses_crosshair_when_paint_edit_is_allowed() {
        let tool = tool(
            ToolBehavior::Fill {
                scope: FillScope::Material,
            },
            &[StrokeSpace::Uv, StrokeSpace::Surface],
        );

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Allowed(()),
                StrokeSpace::Surface,
                false
            ),
            ToolCursorAppearance::Crosshair
        );
    }

    #[test]
    fn selection_tool_uses_crosshair_even_when_paint_edit_is_blocked() {
        let tool = tool(
            ToolBehavior::Selection {
                shape: SelectionShapeKind::Rectangle,
            },
            &[StrokeSpace::Uv],
        );

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Blocked(PaintEditBlockReason::ActiveLayerHidden),
                StrokeSpace::Uv,
                false
            ),
            ToolCursorAppearance::Crosshair
        );
    }

    #[test]
    fn shape_tool_uses_crosshair_when_paint_edit_is_allowed_and_not_allowed_when_disallowed() {
        let tool = tool(
            ToolBehavior::Shape {
                shape: ShapeToolKind::Rectangle,
                paint_mode: ShapePaintMode::Paint,
            },
            &[StrokeSpace::Uv],
        );

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Allowed(()),
                StrokeSpace::Uv,
                false
            ),
            ToolCursorAppearance::Crosshair
        );
        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Blocked(PaintEditBlockReason::ActiveTargetNotEditable),
                StrokeSpace::Uv,
                false
            ),
            ToolCursorAppearance::NotAllowed(PaintEditBlockReason::ActiveTargetNotEditable)
        );
    }

    #[test]
    fn unsupported_space_does_not_use_not_allowed_cursor() {
        let tool = tool(ToolBehavior::Stroke { preset_index: 0 }, &[StrokeSpace::Uv]);

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Blocked(PaintEditBlockReason::ActiveLayerHidden),
                StrokeSpace::Surface,
                true
            ),
            ToolCursorAppearance::Default
        );
    }

    #[test]
    fn allowed_and_unavailable_do_not_use_not_allowed_cursor() {
        let tool = tool(ToolBehavior::Stroke { preset_index: 0 }, &[StrokeSpace::Uv]);

        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Allowed(()),
                StrokeSpace::Uv,
                false
            ),
            ToolCursorAppearance::Default
        );
        assert_eq!(
            tool_cursor_appearance_for_tool(
                &tool,
                PaintEditDecision::Unavailable,
                StrokeSpace::Uv,
                false
            ),
            ToolCursorAppearance::Default
        );
    }

    #[test]
    fn block_hint_is_placed_opposite_each_view_quadrant() {
        let view = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));
        let size = egui::vec2(100.0, 50.0);

        let top_left = paint_edit_block_hint_rect(egui::pos2(100.0, 75.0), view, size);
        assert!(top_left.left() > 100.0 && top_left.top() > 75.0);

        let top_right = paint_edit_block_hint_rect(egui::pos2(300.0, 75.0), view, size);
        assert!(top_right.right() < 300.0 && top_right.top() > 75.0);

        let bottom_left = paint_edit_block_hint_rect(egui::pos2(100.0, 225.0), view, size);
        assert!(bottom_left.left() > 100.0 && bottom_left.bottom() < 225.0);

        let bottom_right = paint_edit_block_hint_rect(egui::pos2(300.0, 225.0), view, size);
        assert!(bottom_right.right() < 300.0 && bottom_right.bottom() < 225.0);
    }

    #[test]
    fn block_hint_is_clamped_to_the_inset_view_bounds() {
        let view = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 120.0));
        let hint =
            paint_edit_block_hint_rect(egui::pos2(199.0, 119.0), view, egui::vec2(120.0, 80.0));
        let bounds = view.shrink(BLOCK_HINT_VIEW_MARGIN);

        assert!(bounds.contains_rect(hint));
    }

    #[test]
    fn block_hint_handles_views_smaller_than_the_margin() {
        let view = egui::Rect::from_min_max(egui::pos2(3.0, 4.0), egui::pos2(5.0, 6.0));
        let hint = paint_edit_block_hint_rect(egui::pos2(4.0, 5.0), view, egui::vec2(100.0, 60.0));

        assert!(hint.min.is_finite());
        assert!(hint.max.is_finite());
        assert!(hint.width() >= 0.0);
        assert!(hint.height() >= 0.0);
    }

    fn tool(behavior: ToolBehavior, supported_spaces: &[StrokeSpace]) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::BrushPreset(0),
            config_id: "test.tool".to_owned(),
            name: "Tool".to_owned(),
            section: ToolSection::Paint,
            behavior,
            supported_spaces: supported_spaces.to_vec(),
        }
    }
}
