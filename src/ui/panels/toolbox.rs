use crate::ui::widgets::interaction_gate::InteractionGate;
use eframe::egui;
use glam::Vec2;

use crate::{
    application::{AppState, Command, EditorAction, EditorActionContext},
    core::{
        stroke::StrokeDab,
        stroke_preset::{StrokeOp, StrokeStrategy, StrokeToolPreset},
        stroke_sampling::{
            min_uv_spacing_for_texture_size, normalized_stroke_strategy,
            sample_segment_with_radius_and_min_spacing_into,
        },
        tool::{ToolBehavior, ToolEntry, ToolId},
        tool_operation::ToolOperation,
    },
    localization::Localization,
    renderer::{
        RendererStrokeOperation, RendererStrokeStyle, ToolBrushPreview, ToolPreviewItem,
        ToolPreviewKind, ToolPreviewRequest,
    },
    ui::{
        icons::UiIconRegistry,
        input::shortcut_profile::{ShortcutKey, ShortcutProfile},
        localized,
        render_resources::UiRenderResources,
        view_output::ViewOutput,
    },
};

const TOOL_GROUP_BUTTON_SIZE: f32 = 32.0;
const TOOL_GROUP_ICON_SIZE: f32 = 24.0;
const TOOL_PREVIEW_SIZE: [u32; 2] = [256, 72];
const TOOL_PREVIEW_CARD_HEIGHT: f32 = 52.0;
const TOOL_PREVIEW_ICON_SIZE: f32 = 24.0;
const TOOL_LIST_FOOTER_HEIGHT: f32 = 26.0;
const TOOL_LIST_FOOTER_BUTTON_SIZE: f32 = 22.0;
const TOOL_LIST_FOOTER_ICON_SIZE: f32 = 16.0;
//const TOOL_PREVIEW_CARD_SPACING: f32 = 6.0;
const TOOL_PREVIEW_MAX_RADIUS_WORLD: f32 = 0.1;
const TOOL_PREVIEW_SCENE_DIAGONAL: f32 = 2.0;
const TOOL_DRAG_AUTOSCROLL_EDGE: f32 = 28.0;
const TOOL_DRAG_AUTOSCROLL_SPEED: f32 = 10.0;

const PREVIEW_POINT_COUNT: usize = 96;
const PREVIEW_SPAN_X_RATIO: f32 = 0.90;
const PREVIEW_CENTER_X_RATIO: f32 = 0.50;
const PREVIEW_CENTER_Y_RATIO: f32 = 0.478;
const PREVIEW_TILT_RATIO: f32 = 0.134;
const PREVIEW_BEND_RATIO: f32 = 0.234;
const PREVIEW_PRESSURE_GAMMA: f32 = 1.62;
const PREVIEW_CONTINUOUS_DURATION_S: f32 = 0.75;
const PREVIEW_CONTINUOUS_DAB_LIMIT: usize = 128;

const LASSO_PREVIEW_POINTS: &[[f32; 2]] = &[
    [0.12, 0.35],
    [0.27, 0.17],
    [0.49, 0.23],
    [0.69, 0.13],
    [0.88, 0.34],
    [0.72, 0.49],
    [0.84, 0.72],
    [0.57, 0.84],
    [0.38, 0.68],
    [0.16, 0.79],
    [0.21, 0.53],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolEntryPreviewStyle {
    Rendered,
    Shape {
        geometry: ToolPreviewGeometry,
        semantic: ToolPreviewSemantic,
    },
    Icon {
        icon_id: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPreviewGeometry {
    Rectangle,
    Lasso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPreviewSemantic {
    Selection,
    Paint,
}

#[derive(Debug, Default)]
pub struct ToolboxUiState {
    drag: Option<ToolboxDragState>,
    reveal_selection: bool,
}

#[derive(Debug, Clone)]
enum ToolboxDragState {
    Group {
        from_index: usize,
        target: Option<ToolDropTarget>,
    },
    Entry {
        group_id: String,
        from_index: usize,
        target: Option<ToolDropTarget>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolDropTarget {
    index: usize,
    placement: ToolDropPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolDropPlacement {
    Before,
    After,
}

pub fn draw_tool_groups(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    icons: &UiIconRegistry,
    shortcut_profile: &ShortcutProfile,
    ui_state: &mut ToolboxUiState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let shelf = state.tool_shelf();
    let panel_tool_id = state.panel_tool_id();
    let tools_enabled =
        state.active_layer_target() != crate::core::document::ActiveLayerTarget::EmbeddedImage;
    let reorder_enabled = !state.is_document_edit_interacting() && !actions.editing_blocked();
    let Some(active_group_index) = shelf.group_containing(panel_tool_id) else {
        return output;
    };

    if let Some(ToolboxDragState::Group { target, .. }) = ui_state.drag.as_mut() {
        *target = None;
    }

    for (index, group) in shelf.groups.iter().enumerate() {
        let group_name = localized::tool_group_name(l10n, group);
        let shortcut_key = shortcut_profile.first_key_for_tool_group(&group.id);
        let tooltip = tool_group_tooltip(&group_name, shortcut_key);
        let selected = index == active_group_index;
        let Some(representative_tool_id) = state.representative_tool_id(index) else {
            continue;
        };
        let action = EditorAction::ActivateTool(representative_tool_id);
        let response = match group
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|icon| !icon.is_empty())
        {
            Some(icon_id) => {
                let texture = icons.texture(icon_id).unwrap_or_else(|| {
                    panic!("tool group references unknown builtin icon id: {icon_id}")
                });
                tool_group_icon_button(ui, texture, selected, reorder_enabled)
            }
            None => ui.add(
                egui::Button::selectable(selected, short_group_label(shortcut_key, &group_name))
                    .sense(if reorder_enabled {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::click()
                    }),
            ),
        }
        .on_hover_text(tooltip);
        let response = if reorder_enabled {
            response.on_hover_cursor(egui::CursorIcon::Grab)
        } else {
            response
        };

        update_group_drag_state(ui, &response, index, shelf.groups.len(), ui_state);
        let is_drag_source = matches!(
            ui_state.drag,
            Some(ToolboxDragState::Group { from_index, .. }) if from_index == index
        );
        if is_drag_source {
            paint_drag_source(ui, response.rect);
        }
        if let Some(ToolboxDragState::Group {
            target: Some(target),
            ..
        }) = ui_state.drag.as_ref()
            && target.index == index
        {
            paint_drop_marker(ui, response.rect, target.placement);
        }

        if response.clicked()
            && tools_enabled
            && actions.is_enabled(action)
            && representative_tool_id != panel_tool_id
        {
            output.push_action(action);
        }
    }

    finish_group_drag(ui, ui_state, shelf.groups.len(), &mut output);

    output
}

pub fn draw_tool_list(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    render_resources: &UiRenderResources,
    icons: &UiIconRegistry,
    ui_state: &mut ToolboxUiState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let shelf = state.tool_shelf();
    let panel_tool_id = state.panel_tool_id();
    let tools_enabled =
        state.active_layer_target() != crate::core::document::ActiveLayerTarget::EmbeddedImage;
    let reorder_enabled = !state.is_document_edit_interacting() && !actions.editing_blocked();
    let Some(active_group_index) = shelf.group_containing(panel_tool_id) else {
        ui.heading(l10n.text("workspace-tool-list"));
        ui.label(l10n.text("toolbox-no-tools"));
        return output;
    };
    let group = &shelf.groups[active_group_index];

    let preview_styles: Vec<_> = group
        .entries
        .iter()
        .map(|entry| tool_entry_preview_style(entry.tool_id))
        .collect();
    let rendered_row_count = preview_styles
        .iter()
        .filter(|style| matches!(style, ToolEntryPreviewStyle::Rendered))
        .count();
    if render_resources.renderer_available
        && let Some(request) = build_tool_preview_request(state, &group.entries, &preview_styles)
    {
        output.request_tool_preview_render(request);
    }

    if let Some(ToolboxDragState::Entry {
        group_id, target, ..
    }) = ui_state.drag.as_mut()
        && group_id == &group.id
    {
        *target = None;
    }

    egui::ScrollArea::vertical()
        .id_salt("tool_entry_list_scroll")
        .auto_shrink([false, false])
        .max_height((ui.available_height() - TOOL_LIST_FOOTER_HEIGHT).max(0.0))
        .show(ui, |ui| {
            if group.entries.is_empty() {
                ui.label(l10n.text("toolbox-no-tools"));
            }
            let mut rendered_row = 0;
            for (index, entry) in group.entries.iter().enumerate() {
                let preview_style = preview_styles[index];
                let entry_rendered_row = matches!(preview_style, ToolEntryPreviewStyle::Rendered)
                    .then(|| {
                        let row = rendered_row;
                        rendered_row += 1;
                        row
                    });
                let selected = entry.tool_id == panel_tool_id;
                let entry_name = state
                    .tool_catalog()
                    .iter()
                    .find(|tool| tool.id == entry.tool_id)
                    .map(|tool| localized::tool_name(l10n, state, tool))
                    .unwrap_or_else(|| entry.name.clone());
                let action = EditorAction::ActivateTool(entry.tool_id);
                let response = tool_entry_preview_card(
                    ui,
                    entry,
                    &entry_name,
                    preview_style,
                    selected,
                    render_resources.tool_preview_texture_id,
                    entry_rendered_row,
                    rendered_row_count,
                    icons,
                    reorder_enabled,
                );
                let response = if reorder_enabled {
                    response.on_hover_cursor(egui::CursorIcon::Grab)
                } else {
                    response
                };
                update_entry_drag_state(
                    ui,
                    &response,
                    &group.id,
                    index,
                    group.entries.len(),
                    ui_state,
                );
                if selected && ui_state.reveal_selection {
                    response.scroll_to_me(Some(egui::Align::Center));
                    ui_state.reveal_selection = false;
                }
                let is_drag_source = matches!(
                    ui_state.drag.as_ref(),
                    Some(ToolboxDragState::Entry { group_id, from_index, .. })
                        if group_id == &group.id && *from_index == index
                );
                if is_drag_source {
                    paint_drag_source(ui, response.rect);
                }
                if let Some(ToolboxDragState::Entry {
                    group_id,
                    target: Some(target),
                    ..
                }) = ui_state.drag.as_ref()
                    && group_id == &group.id
                    && target.index == index
                {
                    paint_drop_marker(ui, response.rect, target.placement);
                }
                if response.clicked() && tools_enabled && actions.is_enabled(action) && !selected {
                    output.push_action(action);
                }
            }
            let scroll_rect = ui.clip_rect();
            update_drag_auto_scroll(ui, scroll_rect, ui_state, false);
        });
    finish_entry_drag(ui, ui_state, &group.id, group.entries.len(), &mut output);

    let active_is_brush = matches!(panel_tool_id, ToolId::BrushPreset(_));
    let can_add = tools_enabled && active_is_brush;
    let can_delete = tools_enabled && active_is_brush;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let add = ui
            .add_available_ui(can_add, |ui| {
                tool_list_icon_button(ui, icons, "builtin.icon.add")
            })
            .inner
            .on_hover_text(l10n.text("toolbox-add-brush-preset"));
        if add.clicked() {
            if let Some(tool) = state.panel_tool_definition()
                && let ToolBehavior::Stroke { preset_index } = tool.behavior
                && let Some(source) = state.stroke_tool_preset(preset_index)
            {
                let mut preset = source.clone();
                let base = format!("{} Copy", source.name);
                preset.name = base.clone();
                let mut suffix = 2;
                while state
                    .brush_presets()
                    .definitions()
                    .any(|item| item.display_name() == preset.name)
                {
                    preset.name = format!("{base} {suffix}");
                    suffix += 1;
                }
                output.push(Command::CreateBrushPreset {
                    group_id: group.id.clone(),
                    source_preset_id: tool.config_id.clone(),
                    new_preset_id: format!("user.brush.{}", uuid::Uuid::new_v4()),
                    preset,
                });
                ui_state.reveal_selection = true;
            }
        }

        let delete = ui
            .add_available_ui(can_delete, |ui| {
                tool_list_icon_button(ui, icons, "builtin.icon.delete")
            })
            .inner
            .on_hover_text(l10n.text("toolbox-delete-brush-preset"));
        if delete.clicked()
            && let Some(tool) = state
                .tool_catalog()
                .iter()
                .find(|tool| tool.id == panel_tool_id)
        {
            output.push(Command::DeleteBrushPreset {
                preset_id: tool.config_id.clone(),
            });
        }
    });

    output
}

pub fn draw_toolbox(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    icons: &UiIconRegistry,
    toolbox_state: &mut ToolboxUiState,
    shortcut_profile: &ShortcutProfile,
) -> ViewOutput {
    let mut output = draw_tool_groups(
        ui,
        l10n,
        state,
        actions,
        icons,
        shortcut_profile,
        toolbox_state,
    );
    ui.separator();
    output.extend(draw_tool_list(
        ui,
        l10n,
        state,
        actions,
        &UiRenderResources::default(),
        icons,
        toolbox_state,
    ));
    output
}

fn update_group_drag_state(
    ui: &egui::Ui,
    response: &egui::Response,
    index: usize,
    len: usize,
    ui_state: &mut ToolboxUiState,
) {
    if response.drag_started() && ui_state.drag.is_none() {
        ui_state.drag = Some(ToolboxDragState::Group {
            from_index: index,
            target: None,
        });
    }
    let Some(ToolboxDragState::Group { from_index, target }) = ui_state.drag.as_mut() else {
        return;
    };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let Some(pointer_pos) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    if !response.rect.contains(pointer_pos) {
        return;
    }
    let placement = drop_placement_for_pointer(response.rect, pointer_pos);
    *target = reorder_destination_index(*from_index, index, placement, len)
        .map(|_| ToolDropTarget { index, placement });
}

fn update_entry_drag_state(
    ui: &egui::Ui,
    response: &egui::Response,
    group_id: &str,
    index: usize,
    len: usize,
    ui_state: &mut ToolboxUiState,
) {
    if response.drag_started() && ui_state.drag.is_none() {
        ui_state.drag = Some(ToolboxDragState::Entry {
            group_id: group_id.to_owned(),
            from_index: index,
            target: None,
        });
    }
    let Some(ToolboxDragState::Entry {
        group_id: drag_group_id,
        from_index,
        target,
    }) = ui_state.drag.as_mut()
    else {
        return;
    };
    if drag_group_id != group_id {
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let Some(pointer_pos) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    if !response.rect.contains(pointer_pos) {
        return;
    }
    let placement = drop_placement_for_pointer(response.rect, pointer_pos);
    *target = reorder_destination_index(*from_index, index, placement, len)
        .map(|_| ToolDropTarget { index, placement });
}

fn finish_group_drag(
    ui: &egui::Ui,
    ui_state: &mut ToolboxUiState,
    len: usize,
    output: &mut ViewOutput,
) {
    if !ui.input(|input| input.pointer.any_released())
        || !matches!(ui_state.drag, Some(ToolboxDragState::Group { .. }))
    {
        return;
    }
    let Some(ToolboxDragState::Group { from_index, target }) = ui_state.drag.take() else {
        return;
    };
    if let Some(target) = target
        && let Some(to_index) =
            reorder_destination_index(from_index, target.index, target.placement, len)
    {
        output.push(Command::MoveToolGroup {
            from_index,
            to_index,
        });
    }
}

fn finish_entry_drag(
    ui: &egui::Ui,
    ui_state: &mut ToolboxUiState,
    visible_group_id: &str,
    len: usize,
    output: &mut ViewOutput,
) {
    if !ui.input(|input| input.pointer.any_released())
        || !matches!(ui_state.drag, Some(ToolboxDragState::Entry { .. }))
    {
        return;
    }
    let Some(ToolboxDragState::Entry {
        group_id,
        from_index,
        target,
    }) = ui_state.drag.take()
    else {
        return;
    };
    if group_id != visible_group_id {
        return;
    }
    if let Some(target) = target
        && let Some(to_index) =
            reorder_destination_index(from_index, target.index, target.placement, len)
    {
        output.push(Command::MoveToolEntry {
            group_id,
            from_index,
            to_index,
        });
    }
}

fn drop_placement_for_pointer(rect: egui::Rect, pointer_pos: egui::Pos2) -> ToolDropPlacement {
    if pointer_pos.y < rect.center().y {
        ToolDropPlacement::Before
    } else {
        ToolDropPlacement::After
    }
}

fn reorder_destination_index(
    source_index: usize,
    target_index: usize,
    placement: ToolDropPlacement,
    len: usize,
) -> Option<usize> {
    if source_index >= len || target_index >= len || source_index == target_index {
        return None;
    }
    let mut insertion_index =
        target_index + usize::from(matches!(placement, ToolDropPlacement::After));
    if source_index < insertion_index {
        insertion_index -= 1;
    }
    (insertion_index != source_index).then_some(insertion_index)
}

fn paint_drop_marker(ui: &egui::Ui, rect: egui::Rect, placement: ToolDropPlacement) {
    let y = match placement {
        ToolDropPlacement::Before => rect.top(),
        ToolDropPlacement::After => rect.bottom(),
    };
    let color = ui.visuals().selection.stroke.color;
    ui.painter().hline(
        rect.left() + 4.0..=rect.right() - 4.0,
        y,
        egui::Stroke::new(3.0, color),
    );
}

fn paint_drag_source(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn update_group_drag_auto_scroll(
    ui: &mut egui::Ui,
    scroll_rect: egui::Rect,
    ui_state: &ToolboxUiState,
) {
    if !matches!(ui_state.drag, Some(ToolboxDragState::Group { .. })) {
        return;
    }
    update_drag_auto_scroll(ui, scroll_rect, ui_state, true);
}

fn update_drag_auto_scroll(
    ui: &mut egui::Ui,
    scroll_rect: egui::Rect,
    ui_state: &ToolboxUiState,
    group_drag: bool,
) {
    let correct_drag = matches!(
        (&ui_state.drag, group_drag),
        (Some(ToolboxDragState::Group { .. }), true)
            | (Some(ToolboxDragState::Entry { .. }), false)
    );
    if !correct_drag {
        return;
    }
    let Some(pointer_pos) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    if !scroll_rect
        .expand(TOOL_DRAG_AUTOSCROLL_EDGE)
        .contains(pointer_pos)
    {
        return;
    }
    let delta_y = if pointer_pos.y < scroll_rect.top() + TOOL_DRAG_AUTOSCROLL_EDGE {
        TOOL_DRAG_AUTOSCROLL_SPEED
    } else if pointer_pos.y > scroll_rect.bottom() - TOOL_DRAG_AUTOSCROLL_EDGE {
        -TOOL_DRAG_AUTOSCROLL_SPEED
    } else {
        0.0
    };
    if delta_y != 0.0 {
        ui.scroll_with_delta(egui::vec2(0.0, delta_y));
        ui.ctx().request_repaint();
    }
}

fn tool_list_icon_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
) -> egui::Response {
    let button_size = egui::Vec2::splat(TOOL_LIST_FOOTER_BUTTON_SIZE);
    let icon_size = egui::Vec2::splat(TOOL_LIST_FOOTER_ICON_SIZE);
    let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.highlighted() || response.is_pointer_button_down_on() {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
        let texture = icons
            .texture(icon_id)
            .unwrap_or_else(|| panic!("unknown builtin icon id: {icon_id}"));
        let tint = if crate::ui::widgets::interaction_gate::visually_available(ui) {
            visuals.fg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        };
        ui.painter().image(
            texture.id(),
            egui::Rect::from_center_size(rect.center(), icon_size),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            tint,
        );
    }
    response
}

fn tool_entry_preview_card(
    ui: &mut egui::Ui,
    entry: &ToolEntry,
    entry_name: &str,
    preview_style: ToolEntryPreviewStyle,
    selected: bool,
    texture_id: Option<egui::TextureId>,
    rendered_row: Option<usize>,
    rendered_row_count: usize,
    icons: &UiIconRegistry,
    drag_enabled: bool,
) -> egui::Response {
    let width = ui.available_width().max(64.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, TOOL_PREVIEW_CARD_HEIGHT),
        if drag_enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        },
    );

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let visuals = ui.style().interact_selectable(&response, selected);
        let rounding = egui::CornerRadius::ZERO;
        painter.rect_filled(rect, rounding, ui.visuals().extreme_bg_color);

        match preview_style {
            ToolEntryPreviewStyle::Rendered => {
                if let (Some(texture_id), Some(rendered_row)) = (texture_id, rendered_row) {
                    let rows = rendered_row_count.max(1) as f32;
                    let uv_min_y = rendered_row as f32 / rows;
                    let uv_max_y = (rendered_row + 1) as f32 / rows;
                    painter.image(
                        texture_id,
                        rect,
                        egui::Rect::from_min_max(
                            egui::pos2(0.0, uv_min_y),
                            egui::pos2(1.0, uv_max_y),
                        ),
                        egui::Color32::WHITE,
                    );
                } else {
                    paint_placeholder_preview(painter, rect, dummy_preview_kind(entry.tool_id));
                }
            }
            ToolEntryPreviewStyle::Shape { geometry, semantic } => {
                paint_preview_background(painter, rect);
                paint_shape_preview(painter, rect, geometry, semantic);
            }
            ToolEntryPreviewStyle::Icon { icon_id } => {
                paint_preview_background(painter, rect);
                paint_icon_preview(ui, rect, icons, icon_id);
            }
        }

        if response.hovered() || response.highlighted() || response.is_pointer_button_down_on() {
            painter.rect_filled(rect, rounding, egui::Color32::from_black_alpha(34));
        }
        if selected {
            painter.rect_stroke(
                rect,
                rounding,
                egui::Stroke::new(2.0, visuals.fg_stroke.color),
                egui::StrokeKind::Inside,
            );
        } else {
            painter.rect_stroke(
                rect,
                rounding,
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }

        let label_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 4.0, rect.bottom() - 20.0),
            egui::vec2((rect.width() - 8.0).max(0.0), 18.0),
        );
        painter.rect_filled(
            label_rect.expand2(egui::vec2(4.0, 1.0)),
            egui::CornerRadius::ZERO,
            egui::Color32::from_white_alpha(100),
        );
        painter.text(
            label_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            entry_name,
            egui::TextStyle::Button.resolve(ui.style()),
            egui::Color32::BLACK,
        );
    }
    response
}

fn build_tool_preview_request(
    state: &AppState,
    entries: &[ToolEntry],
    preview_styles: &[ToolEntryPreviewStyle],
) -> Option<ToolPreviewRequest> {
    let items: Vec<_> = rendered_tool_ids(entries, preview_styles)
        .into_iter()
        .map(|tool_id| ToolPreviewItem {
            tool_id,
            kind: tool_preview_kind(state, tool_id),
        })
        .collect();
    (!items.is_empty()).then_some(ToolPreviewRequest {
        item_size: TOOL_PREVIEW_SIZE,
        items,
    })
}

fn rendered_tool_ids(
    entries: &[ToolEntry],
    preview_styles: &[ToolEntryPreviewStyle],
) -> Vec<ToolId> {
    debug_assert_eq!(entries.len(), preview_styles.len());
    entries
        .iter()
        .zip(preview_styles)
        .filter_map(|(entry, style)| {
            matches!(style, ToolEntryPreviewStyle::Rendered).then_some(entry.tool_id)
        })
        .collect()
}

fn tool_entry_preview_style(tool_id: ToolId) -> ToolEntryPreviewStyle {
    match tool_id {
        ToolId::RectangleSelection => ToolEntryPreviewStyle::Shape {
            geometry: ToolPreviewGeometry::Rectangle,
            semantic: ToolPreviewSemantic::Selection,
        },
        ToolId::LassoSelection => ToolEntryPreviewStyle::Shape {
            geometry: ToolPreviewGeometry::Lasso,
            semantic: ToolPreviewSemantic::Selection,
        },
        ToolId::RectanglePaint => ToolEntryPreviewStyle::Shape {
            geometry: ToolPreviewGeometry::Rectangle,
            semantic: ToolPreviewSemantic::Paint,
        },
        ToolId::LassoPaint => ToolEntryPreviewStyle::Shape {
            geometry: ToolPreviewGeometry::Lasso,
            semantic: ToolPreviewSemantic::Paint,
        },
        ToolId::FillMaterial | ToolId::FillMesh | ToolId::FillPolygon => {
            ToolEntryPreviewStyle::Icon {
                icon_id: "builtin.icon.colors",
            }
        }
        ToolId::SurfaceDecal | ToolId::ViewProjectionDecal => ToolEntryPreviewStyle::Icon {
            icon_id: "builtin.icon.sticker",
        },
        ToolId::Transform => ToolEntryPreviewStyle::Icon {
            icon_id: "builtin.icon.transform",
        },
        ToolId::ColorPicker => ToolEntryPreviewStyle::Icon {
            icon_id: "builtin.icon.colorize",
        },
        ToolId::EmptyGroup(_)
        | ToolId::RectangleErase
        | ToolId::LassoErase
        | ToolId::BrushPreset(_) => ToolEntryPreviewStyle::Rendered,
    }
}

fn tool_preview_kind(state: &AppState, tool_id: ToolId) -> ToolPreviewKind {
    match tool_id {
        ToolId::EmptyGroup(_) => ToolPreviewKind::Empty,
        ToolId::BrushPreset(_) => {
            brush_preview_kind(state, tool_id).unwrap_or(ToolPreviewKind::Empty)
        }
        ToolId::RectangleErase => ToolPreviewKind::RectangleErase,
        ToolId::LassoErase => ToolPreviewKind::LassoErase,
        ToolId::SurfaceDecal
        | ToolId::ViewProjectionDecal
        | ToolId::FillMaterial
        | ToolId::FillMesh
        | ToolId::FillPolygon
        | ToolId::RectanglePaint
        | ToolId::RectangleSelection
        | ToolId::LassoPaint
        | ToolId::LassoSelection
        | ToolId::Transform
        | ToolId::ColorPicker => {
            unreachable!("non-rendered tool preview was included in the renderer request")
        }
    }
}

fn brush_preview_kind(state: &AppState, tool_id: ToolId) -> Option<ToolPreviewKind> {
    let tool = state
        .tool_catalog()
        .iter()
        .find(|tool| tool.id == tool_id)?;
    let ToolBehavior::Stroke { preset_index } = &tool.behavior else {
        return None;
    };
    let preset = state.stroke_tool_preset(*preset_index)?;
    let preview_stroke_op =
        preview_stroke_op(preset.stroke_op.resolve(TOOL_PREVIEW_SCENE_DIAGONAL));
    let style = renderer_stroke_style(&preview_stroke_op, state.current_color());
    let dabs = preview_dabs(preset, preview_stroke_op.radius_world(), TOOL_PREVIEW_SIZE);
    Some(ToolPreviewKind::Brush(ToolBrushPreview { style, dabs }))
}

fn preview_stroke_op(mut stroke_op: StrokeOp) -> StrokeOp {
    match &mut stroke_op {
        StrokeOp::BrushEngine { radius_world, .. } => {
            *radius_world = (*radius_world).min(TOOL_PREVIEW_MAX_RADIUS_WORLD);
        }
    }
    stroke_op
}

fn renderer_stroke_style(stroke_op: &StrokeOp, current_color: [f32; 3]) -> RendererStrokeStyle {
    let operation = ToolOperation::from_stroke_op_and_color(stroke_op, current_color);
    let ToolOperation::BrushEngine(operation) = operation else {
        unreachable!("stroke previews only support brush engine operations")
    };
    RendererStrokeStyle {
        stroke_op: stroke_op.clone(),
        operation: RendererStrokeOperation::BrushEngine {
            color: operation.color,
            params: operation.params,
        },
    }
}

fn preview_dabs(preset: &StrokeToolPreset, radius_uv: f32, size: [u32; 2]) -> Vec<StrokeDab> {
    let mut out = Vec::new();
    let strategy = normalized_stroke_strategy(&preset.stroke_strategy);
    let radius_uv = radius_uv.max(1e-6);
    let min_spacing_uv = min_uv_spacing_for_texture_size(size);
    let mut last = None;
    for sample_index in 0..PREVIEW_POINT_COUNT {
        let t = preview_sample_t(sample_index, PREVIEW_POINT_COUNT);
        let dab = preview_dab(preset, t, size);
        let before_len = out.len();
        sample_segment_with_radius_and_min_spacing_into(
            last,
            dab,
            &strategy,
            radius_uv,
            min_spacing_uv,
            &mut out,
        );
        if out.len() > before_len {
            last = out.last().copied();
        }
    }
    append_continuous_preview_dabs(preset, &strategy, size, &mut out);
    out
}

fn append_continuous_preview_dabs(
    preset: &StrokeToolPreset,
    strategy: &StrokeStrategy,
    size: [u32; 2],
    out: &mut Vec<StrokeDab>,
) {
    let StrokeStrategy::ContinuousDab { rate_hz, .. } = strategy else {
        return;
    };
    let dab_count = (*rate_hz * PREVIEW_CONTINUOUS_DURATION_S).round().max(1.0) as usize;
    let dab_count = dab_count.min(PREVIEW_CONTINUOUS_DAB_LIMIT);
    for sample_index in 0..dab_count {
        let t = preview_sample_t(sample_index, dab_count);
        out.push(preview_dab(preset, t, size));
    }
}

fn preview_sample_t(sample_index: usize, sample_count: usize) -> f32 {
    if sample_count <= 1 {
        0.0
    } else {
        sample_index as f32 / (sample_count - 1) as f32
    }
}

fn preview_dab(preset: &StrokeToolPreset, t: f32, size: [u32; 2]) -> StrokeDab {
    let position = preview_point_uv(t, size);
    let raw_pressure = (std::f32::consts::PI * t)
        .sin()
        .max(0.0)
        .powf(PREVIEW_PRESSURE_GAMMA);
    let pressure = preset.corrected_pressure(raw_pressure);
    let radius_scale = preset.stroke_op.radius_scale(pressure);
    StrokeDab::with_scales(position, pressure, radius_scale)
}

fn preview_point_uv(t: f32, size: [u32; 2]) -> Vec2 {
    let width = size[0].max(1) as f32;
    let height = size[1].max(1) as f32;
    let span_x = width * PREVIEW_SPAN_X_RATIO;
    let start_x = width * PREVIEW_CENTER_X_RATIO - span_x * 0.5;
    let centered = t - 0.5;
    let x = start_x + span_x * t;
    let center_y = height * PREVIEW_CENTER_Y_RATIO;
    let tilt = height * PREVIEW_TILT_RATIO * centered;
    let bend = height * PREVIEW_BEND_RATIO * ((centered * std::f32::consts::PI).sin());
    let y = center_y - tilt + bend;
    Vec2::new((x / width).clamp(0.0, 1.0), (y / height).clamp(0.0, 1.0))
}

fn dummy_preview_kind(tool_id: ToolId) -> ToolPreviewKind {
    match tool_id {
        ToolId::EmptyGroup(_) => ToolPreviewKind::Empty,
        ToolId::RectangleErase => ToolPreviewKind::RectangleErase,
        ToolId::LassoErase => ToolPreviewKind::LassoErase,
        ToolId::BrushPreset(_) => ToolPreviewKind::Empty,
        ToolId::SurfaceDecal
        | ToolId::ViewProjectionDecal
        | ToolId::FillMaterial
        | ToolId::FillMesh
        | ToolId::FillPolygon
        | ToolId::RectanglePaint
        | ToolId::RectangleSelection
        | ToolId::LassoPaint
        | ToolId::LassoSelection
        | ToolId::Transform
        | ToolId::ColorPicker => {
            unreachable!("non-rendered tool preview requested a rendered placeholder")
        }
    }
}

fn paint_placeholder_preview(painter: &egui::Painter, rect: egui::Rect, kind: ToolPreviewKind) {
    paint_preview_background(painter, rect);

    let dummy = rect.shrink2(egui::vec2(rect.width() * 0.18, rect.height() * 0.22));
    match kind {
        ToolPreviewKind::RectangleErase | ToolPreviewKind::LassoErase => {
            painter.rect_filled(
                dummy,
                egui::CornerRadius::same(3),
                egui::Color32::from_gray(235),
            );
            painter.rect_stroke(
                dummy,
                egui::CornerRadius::same(3),
                egui::Stroke::new(2.0, egui::Color32::from_gray(70)),
                egui::StrokeKind::Inside,
            );
        }
        ToolPreviewKind::Brush(_) | ToolPreviewKind::Empty => {}
    }
}

fn paint_preview_background(painter: &egui::Painter, rect: egui::Rect) {
    let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.max.y));
    let right = egui::Rect::from_min_max(egui::pos2(rect.center().x, rect.min.y), rect.max);
    painter.rect_filled(left, egui::CornerRadius::ZERO, egui::Color32::WHITE);
    painter.rect_filled(
        right,
        egui::CornerRadius::ZERO,
        egui::Color32::from_gray(50),
    );
}

fn paint_shape_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    geometry: ToolPreviewGeometry,
    semantic: ToolPreviewSemantic,
) {
    let points = preview_geometry_points(rect, geometry);
    let fill = match semantic {
        ToolPreviewSemantic::Selection => egui::Color32::from_rgba_unmultiplied(120, 180, 240, 70),
        ToolPreviewSemantic::Paint => egui::Color32::from_rgba_unmultiplied(110, 145, 215, 90),
    };
    fill_preview_polygon(painter, &points, fill);

    match semantic {
        ToolPreviewSemantic::Selection => paint_dashed_closed_path(
            painter,
            &points,
            8.0,
            0.55,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
        ),
        ToolPreviewSemantic::Paint => {
            painter.add(egui::Shape::closed_line(
                points,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 120, 210)),
            ));
        }
    }
}

fn preview_geometry_points(rect: egui::Rect, geometry: ToolPreviewGeometry) -> Vec<egui::Pos2> {
    match geometry {
        ToolPreviewGeometry::Rectangle => {
            let shape_rect = rect.shrink2(egui::vec2(rect.width() * 0.18, rect.height() * 0.22));
            vec![
                shape_rect.left_top(),
                shape_rect.right_top(),
                shape_rect.right_bottom(),
                shape_rect.left_bottom(),
            ]
        }
        ToolPreviewGeometry::Lasso => LASSO_PREVIEW_POINTS
            .iter()
            .map(|point| {
                egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), point[0]),
                    egui::lerp(rect.top()..=rect.bottom(), point[1]),
                )
            })
            .collect(),
    }
}

fn fill_preview_polygon(painter: &egui::Painter, points: &[egui::Pos2], fill: egui::Color32) {
    let center = points
        .iter()
        .fold(egui::Vec2::ZERO, |sum, point| sum + point.to_vec2())
        / points.len().max(1) as f32;
    let center = center.to_pos2();
    for index in 0..points.len() {
        painter.add(egui::Shape::convex_polygon(
            vec![center, points[index], points[(index + 1) % points.len()]],
            fill,
            egui::Stroke::NONE,
        ));
    }
}

fn paint_dashed_closed_path(
    painter: &egui::Painter,
    points: &[egui::Pos2],
    dash_length: f32,
    painted_ratio: f32,
    stroke: egui::Stroke,
) {
    for index in 0..points.len() {
        let start = points[index];
        let end = points[(index + 1) % points.len()];
        let segment = end - start;
        let length = segment.length();
        if length <= f32::EPSILON {
            continue;
        }
        let direction = segment / length;
        let mut offset = 0.0;
        while offset < length {
            let dash_end = (offset + dash_length * painted_ratio).min(length);
            painter.line_segment(
                [start + direction * offset, start + direction * dash_end],
                stroke,
            );
            offset += dash_length;
        }
    }
}

fn paint_icon_preview(ui: &egui::Ui, rect: egui::Rect, icons: &UiIconRegistry, icon_id: &str) {
    let texture = icons
        .texture(icon_id)
        .unwrap_or_else(|| panic!("unknown builtin icon id: {icon_id}"));
    let label_top = rect.bottom() - 20.0;
    let preview_height = (label_top - rect.top()).max(TOOL_PREVIEW_ICON_SIZE);
    let icon_top = rect.top() + (preview_height - TOOL_PREVIEW_ICON_SIZE) * 0.5;
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), icon_top),
        egui::Vec2::splat(TOOL_PREVIEW_ICON_SIZE),
    );
    let tint = if crate::ui::widgets::interaction_gate::visually_available(ui) {
        egui::Color32::from_gray(40)
    } else {
        egui::Color32::from_gray(130)
    };
    ui.painter().image(
        texture.id(),
        icon_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        tint,
    );
}

fn tool_group_icon_button(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    selected: bool,
    drag_enabled: bool,
) -> egui::Response {
    let button_size = egui::Vec2::splat(TOOL_GROUP_BUTTON_SIZE);
    let icon_size = egui::Vec2::splat(TOOL_GROUP_ICON_SIZE);
    let (rect, response) = ui.allocate_exact_size(
        button_size,
        if drag_enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        },
    );

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let rect = rect.expand(visuals.expansion);
        let show_frame = selected
            || response.hovered()
            || response.highlighted()
            || response.has_focus()
            || response.is_pointer_button_down_on();

        if show_frame {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            ui.painter().rect_stroke(
                rect,
                visuals.corner_radius,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }

        let tint = if crate::ui::widgets::interaction_gate::visually_available(ui) {
            visuals.fg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);
        ui.painter().image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }

    response
}

fn short_group_label(shortcut_key: Option<ShortcutKey>, name: &str) -> String {
    shortcut_key
        .map(|key| key.label().to_owned())
        .unwrap_or_else(|| name.chars().next().unwrap_or('?').to_string())
}

fn tool_group_tooltip(name: &str, shortcut_key: Option<ShortcutKey>) -> String {
    shortcut_key.map_or_else(
        || name.to_owned(),
        |key| format!("{name} ({})", key.label()),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        LASSO_PREVIEW_POINTS, TOOL_PREVIEW_SCENE_DIAGONAL, TOOL_PREVIEW_SIZE, ToolDropPlacement,
        ToolEntryPreviewStyle, ToolPreviewGeometry, ToolPreviewSemantic, preview_dabs,
        rendered_tool_ids, reorder_destination_index, short_group_label, tool_entry_preview_style,
        tool_group_tooltip,
    };
    use crate::core::{
        stroke_preset::{StrokePresetOp, StrokeStrategy, StrokeToolPreset},
        tool::{ToolEntry, ToolId},
    };
    use crate::ui::input::shortcut_profile::ShortcutKey;

    #[test]
    fn tool_group_labels_and_tooltips_include_the_assigned_shortcut() {
        assert_eq!(short_group_label(Some(ShortcutKey::B), "Brush"), "B");
        assert_eq!(
            tool_group_tooltip("Brush", Some(ShortcutKey::B)),
            "Brush (B)"
        );
    }

    #[test]
    fn tool_group_labels_and_tooltips_fall_back_when_unassigned() {
        assert_eq!(short_group_label(None, "Brush"), "B");
        assert_eq!(short_group_label(None, ""), "?");
        assert_eq!(tool_group_tooltip("Brush", None), "Brush");
    }

    #[test]
    fn selection_and_paint_tools_map_to_shape_previews() {
        assert_eq!(
            tool_entry_preview_style(ToolId::RectangleSelection),
            ToolEntryPreviewStyle::Shape {
                geometry: ToolPreviewGeometry::Rectangle,
                semantic: ToolPreviewSemantic::Selection,
            }
        );
        assert_eq!(
            tool_entry_preview_style(ToolId::LassoSelection),
            ToolEntryPreviewStyle::Shape {
                geometry: ToolPreviewGeometry::Lasso,
                semantic: ToolPreviewSemantic::Selection,
            }
        );
        assert_eq!(
            tool_entry_preview_style(ToolId::RectanglePaint),
            ToolEntryPreviewStyle::Shape {
                geometry: ToolPreviewGeometry::Rectangle,
                semantic: ToolPreviewSemantic::Paint,
            }
        );
        assert_eq!(
            tool_entry_preview_style(ToolId::LassoPaint),
            ToolEntryPreviewStyle::Shape {
                geometry: ToolPreviewGeometry::Lasso,
                semantic: ToolPreviewSemantic::Paint,
            }
        );
    }

    #[test]
    fn non_renderable_tools_map_to_existing_icons() {
        for tool_id in [ToolId::FillMaterial, ToolId::FillMesh, ToolId::FillPolygon] {
            assert_eq!(
                tool_entry_preview_style(tool_id),
                ToolEntryPreviewStyle::Icon {
                    icon_id: "builtin.icon.colors",
                }
            );
        }
        for tool_id in [ToolId::SurfaceDecal, ToolId::ViewProjectionDecal] {
            assert_eq!(
                tool_entry_preview_style(tool_id),
                ToolEntryPreviewStyle::Icon {
                    icon_id: "builtin.icon.sticker",
                }
            );
        }
        assert_eq!(
            tool_entry_preview_style(ToolId::Transform),
            ToolEntryPreviewStyle::Icon {
                icon_id: "builtin.icon.transform",
            }
        );
        assert_eq!(
            tool_entry_preview_style(ToolId::ColorPicker),
            ToolEntryPreviewStyle::Icon {
                icon_id: "builtin.icon.colorize",
            }
        );
    }

    #[test]
    fn renderer_request_rows_exclude_shape_and_icon_previews() {
        let entries = [
            ToolEntry {
                tool_id: ToolId::RectangleSelection,
                name: "Rectangle Selection".to_owned(),
            },
            ToolEntry {
                tool_id: ToolId::BrushPreset(3),
                name: "Brush".to_owned(),
            },
            ToolEntry {
                tool_id: ToolId::FillMaterial,
                name: "Fill Material".to_owned(),
            },
            ToolEntry {
                tool_id: ToolId::RectangleErase,
                name: "Rectangle Erase".to_owned(),
            },
        ];
        let styles: Vec<_> = entries
            .iter()
            .map(|entry| tool_entry_preview_style(entry.tool_id))
            .collect();

        assert_eq!(
            rendered_tool_ids(&entries, &styles),
            vec![ToolId::BrushPreset(3), ToolId::RectangleErase]
        );
        assert!(rendered_tool_ids(&entries[..1], &styles[..1]).is_empty());
        assert_eq!(
            tool_entry_preview_style(ToolId::BrushPreset(3)),
            ToolEntryPreviewStyle::Rendered
        );
    }

    #[test]
    fn lasso_preview_is_bounded_and_concave() {
        assert!(LASSO_PREVIEW_POINTS.len() >= 8);
        assert!(
            LASSO_PREVIEW_POINTS.iter().all(|point| {
                (0.1..=0.9).contains(&point[0]) && (0.1..=0.9).contains(&point[1])
            })
        );

        let mut has_positive_turn = false;
        let mut has_negative_turn = false;
        for index in 0..LASSO_PREVIEW_POINTS.len() {
            let a = LASSO_PREVIEW_POINTS[index];
            let b = LASSO_PREVIEW_POINTS[(index + 1) % LASSO_PREVIEW_POINTS.len()];
            let c = LASSO_PREVIEW_POINTS[(index + 2) % LASSO_PREVIEW_POINTS.len()];
            let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
            has_positive_turn |= cross > 1e-4;
            has_negative_turn |= cross < -1e-4;
        }
        assert!(has_positive_turn && has_negative_turn);
    }

    #[test]
    fn continuous_preview_dab_count_tracks_spray_rate() {
        let mut slow = StrokeToolPreset::default();
        slow.stroke_strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.18,
            rate_hz: 20.0,
        };
        let mut fast = slow.clone();
        fast.stroke_strategy = StrokeStrategy::ContinuousDab {
            spacing: 0.18,
            rate_hz: 80.0,
        };
        let radius_uv = slow.stroke_op.radius_world(TOOL_PREVIEW_SCENE_DIAGONAL);

        let slow_dabs = preview_dabs(&slow, radius_uv, TOOL_PREVIEW_SIZE);
        let fast_dabs = preview_dabs(&fast, radius_uv, TOOL_PREVIEW_SIZE);

        assert!(fast_dabs.len() > slow_dabs.len());
    }

    #[test]
    fn preview_dabs_apply_min_spacing_alpha_compensation() {
        let mut preset = StrokeToolPreset::default();
        match &mut preset.stroke_op {
            StrokePresetOp::BrushEngine {
                size_scene_ratio,
                size_pressure,
                ..
            } => {
                size_scene_ratio.value = 0.001;
                size_pressure.enabled = false;
            }
        }

        let radius_uv = preset.stroke_op.radius_world(TOOL_PREVIEW_SCENE_DIAGONAL);
        let dabs = preview_dabs(&preset, radius_uv, TOOL_PREVIEW_SIZE);

        assert!(
            dabs.iter().any(|dab| dab.spacing_alpha_scale > 1.0),
            "preview dabs should preserve shared sampler alpha compensation"
        );
    }

    #[test]
    fn drop_target_converts_to_final_destination_index() {
        assert_eq!(
            reorder_destination_index(1, 3, ToolDropPlacement::After, 4),
            Some(3)
        );
        assert_eq!(
            reorder_destination_index(3, 1, ToolDropPlacement::Before, 4),
            Some(1)
        );
    }

    #[test]
    fn drop_target_rejects_no_op_and_invalid_positions() {
        assert_eq!(
            reorder_destination_index(1, 2, ToolDropPlacement::Before, 4),
            None
        );
        assert_eq!(
            reorder_destination_index(1, 0, ToolDropPlacement::After, 4),
            None
        );
        assert_eq!(
            reorder_destination_index(1, 1, ToolDropPlacement::Before, 4),
            None
        );
        assert_eq!(
            reorder_destination_index(4, 0, ToolDropPlacement::Before, 4),
            None
        );
        assert_eq!(
            reorder_destination_index(0, 4, ToolDropPlacement::Before, 4),
            None
        );
    }
}
