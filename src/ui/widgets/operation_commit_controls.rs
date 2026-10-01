use eframe::egui;

use crate::{localization::Localization, ui::icons::UiIconRegistry};

const BUTTON_SIZE: f32 = 32.0;
const ICON_SIZE: f32 = 24.0;
const BUTTON_SPACING: f32 = 4.0;
const TARGET_GAP: f32 = 8.0;
const VIEWPORT_MARGIN: f32 = 6.0;
const CONTROLS_WIDTH: f32 = BUTTON_SIZE * 2.0 + BUTTON_SPACING;
const CONTROLS_HEIGHT: f32 = BUTTON_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationCommitAction {
    Apply,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
pub struct OperationCommitControlsLayout {
    pub rect: egui::Rect,
}

impl OperationCommitControlsLayout {
    pub fn pointer_over(self, ctx: &egui::Context) -> bool {
        ctx.pointer_hover_pos()
            .is_some_and(|pointer| self.rect.contains(pointer))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OperationCommitControlsInteraction {
    pub rect: egui::Rect,
    pub action: Option<OperationCommitAction>,
}

impl OperationCommitControlsInteraction {
    pub fn pointer_over(self, ctx: &egui::Context) -> bool {
        ctx.pointer_hover_pos()
            .is_some_and(|pointer| self.rect.contains(pointer))
    }
}

pub fn operation_commit_controls_layout(
    viewport_rect: egui::Rect,
    target_corners: [egui::Pos2; 4],
) -> Option<OperationCommitControlsLayout> {
    if viewport_rect.width() < CONTROLS_WIDTH + VIEWPORT_MARGIN * 2.0
        || viewport_rect.height() < CONTROLS_HEIGHT + VIEWPORT_MARGIN * 2.0
        || target_corners
            .iter()
            .any(|corner| !corner.x.is_finite() || !corner.y.is_finite())
    {
        return None;
    }

    let mut target_min = target_corners[0];
    let mut target_max = target_corners[0];
    for corner in target_corners.into_iter().skip(1) {
        target_min.x = target_min.x.min(corner.x);
        target_min.y = target_min.y.min(corner.y);
        target_max.x = target_max.x.max(corner.x);
        target_max.y = target_max.y.max(corner.y);
    }
    let target_rect = egui::Rect::from_min_max(target_min, target_max);
    if target_rect.right() < viewport_rect.left()
        || target_rect.left() > viewport_rect.right()
        || target_rect.bottom() < viewport_rect.top()
        || target_rect.top() > viewport_rect.bottom()
    {
        return None;
    }

    let min_x = viewport_rect.left() + VIEWPORT_MARGIN;
    let max_x = viewport_rect.right() - VIEWPORT_MARGIN - CONTROLS_WIDTH;
    let x = (target_rect.center().x - CONTROLS_WIDTH * 0.5).clamp(min_x, max_x);

    let min_y = viewport_rect.top() + VIEWPORT_MARGIN;
    let max_y = viewport_rect.bottom() - VIEWPORT_MARGIN - CONTROLS_HEIGHT;
    let below_y = target_rect.bottom() + TARGET_GAP;
    let above_y = target_rect.top() - TARGET_GAP - CONTROLS_HEIGHT;
    let y = if below_y <= max_y {
        below_y.max(min_y)
    } else if above_y >= min_y {
        above_y.min(max_y)
    } else {
        below_y.clamp(min_y, max_y)
    };

    Some(OperationCommitControlsLayout {
        rect: egui::Rect::from_min_size(
            egui::pos2(x, y),
            egui::vec2(CONTROLS_WIDTH, CONTROLS_HEIGHT),
        ),
    })
}

pub fn draw_operation_commit_controls(
    ctx: &egui::Context,
    id: egui::Id,
    layout: OperationCommitControlsLayout,
    icons: &UiIconRegistry,
    l10n: &Localization,
    enabled: bool,
) -> OperationCommitControlsInteraction {
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(layout.rect.min)
        .movable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(BUTTON_SPACING, 0.0);
            ui.set_min_size(layout.rect.size());
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let cancel = icon_button(
                    ui,
                    icons,
                    "builtin.icon.cancel",
                    enabled,
                    &l10n.text("operation-cancel-escape"),
                );
                let apply = icon_button(
                    ui,
                    icons,
                    "builtin.icon.check_circle",
                    enabled,
                    &l10n.text("operation-apply-enter"),
                );
                if enabled && cancel.clicked() {
                    Some(OperationCommitAction::Cancel)
                } else if enabled && apply.clicked() {
                    Some(OperationCommitAction::Apply)
                } else {
                    None
                }
            })
            .inner
        });

    OperationCommitControlsInteraction {
        rect: area.response.rect,
        action: area.inner,
    }
}

fn icon_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    enabled: bool,
    tooltip: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(BUTTON_SIZE), sense);

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let visual_rect = rect.expand(visuals.expansion);
        ui.painter()
            .rect_filled(visual_rect, visuals.corner_radius, visuals.weak_bg_fill);
        ui.painter().rect_stroke(
            visual_rect,
            visuals.corner_radius,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );

        if let Some(texture) = icons.texture(icon_id) {
            let tint = if enabled {
                visuals.fg_stroke.color
            } else {
                ui.visuals().widgets.noninteractive.fg_stroke.color
            };
            let icon_rect =
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(ICON_SIZE));
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
    }

    response.on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corners(min: egui::Pos2, max: egui::Pos2) -> [egui::Pos2; 4] {
        [min, egui::pos2(max.x, min.y), max, egui::pos2(min.x, max.y)]
    }

    #[test]
    fn layout_places_controls_below_target_center() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));
        let layout = operation_commit_controls_layout(
            viewport,
            corners(egui::pos2(50.0, 50.0), egui::pos2(150.0, 100.0)),
        )
        .unwrap();

        assert_eq!(layout.rect.min, egui::pos2(66.0, 108.0));
        assert_eq!(
            layout.rect.size(),
            egui::vec2(CONTROLS_WIDTH, CONTROLS_HEIGHT)
        );
    }

    #[test]
    fn layout_stays_horizontal_for_rotated_target() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));
        let layout = operation_commit_controls_layout(
            viewport,
            [
                egui::pos2(100.0, 40.0),
                egui::pos2(140.0, 80.0),
                egui::pos2(100.0, 120.0),
                egui::pos2(60.0, 80.0),
            ],
        )
        .unwrap();

        assert_eq!(layout.rect.min, egui::pos2(66.0, 128.0));
        assert_eq!(layout.rect.height(), BUTTON_SIZE);
    }

    #[test]
    fn layout_moves_above_target_near_bottom_edge() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));
        let layout = operation_commit_controls_layout(
            viewport,
            corners(egui::pos2(50.0, 160.0), egui::pos2(150.0, 190.0)),
        )
        .unwrap();

        assert_eq!(layout.rect.min, egui::pos2(66.0, 120.0));
    }

    #[test]
    fn layout_clamps_to_viewport_horizontal_margins() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));
        let left = operation_commit_controls_layout(
            viewport,
            corners(egui::pos2(-10.0, 50.0), egui::pos2(30.0, 100.0)),
        )
        .unwrap();
        let right = operation_commit_controls_layout(
            viewport,
            corners(egui::pos2(170.0, 50.0), egui::pos2(210.0, 100.0)),
        )
        .unwrap();

        assert_eq!(left.rect.left(), 6.0);
        assert_eq!(right.rect.right(), 194.0);
    }

    #[test]
    fn layout_keeps_controls_inside_viewport_for_oversized_target() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));
        let layout = operation_commit_controls_layout(
            viewport,
            corners(egui::pos2(-50.0, -50.0), egui::pos2(250.0, 250.0)),
        )
        .unwrap();

        assert!(viewport.contains(layout.rect.min));
        assert!(viewport.contains(layout.rect.max));
    }

    #[test]
    fn layout_hides_controls_for_offscreen_target() {
        let viewport = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 200.0));

        assert!(
            operation_commit_controls_layout(
                viewport,
                corners(egui::pos2(220.0, 50.0), egui::pos2(260.0, 100.0)),
            )
            .is_none()
        );
    }
}
