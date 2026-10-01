use crate::ui::widgets::interaction_gate::InteractionGate;
use std::path::PathBuf;

use eframe::egui;

use crate::{
    application::{AppState, Command, EditorAction, EditorActionContext},
    core::{
        adjustment::AdjustmentKind,
        surface::{LayerContent, LayerMaskInitMode},
    },
    localization::Localization,
    settings::ProjectHistoryFileV1,
    ui::{
        view_output::ViewOutput,
        workspace::{WorkspacePane, WorkspaceState},
    },
};

#[derive(Debug, Default)]
pub struct MenuBarOutput {
    pub view: ViewOutput,
    pub new_project_requested: bool,
    pub open_project_requested: bool,
    pub save_requested: bool,
    pub save_as_requested: bool,
    pub open_recent_project_requested: Option<PathBuf>,
    pub reload_mesh_requested: bool,
    pub export_image_requested: bool,
    pub export_psd_requested: bool,
    pub quit_requested: bool,
    pub adjustment_filter_requested: Option<AdjustmentKind>,
    pub blur_requested: bool,
    pub cut_image_requested: bool,
    pub copy_image_requested: bool,
    pub paste_image_requested: bool,
    pub settings_requested: bool,
    pub system_information_requested: bool,
    pub about_requested: bool,
    pub reset_layout_requested: bool,
    pub pane_visibility: Vec<(WorkspacePane, bool)>,
}

pub fn draw_menu_bar(
    root_ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    workspace: &WorkspaceState,
    project_history: &ProjectHistoryFileV1,
    renderer_available: bool,
    actions: EditorActionContext,
    has_single_copy_target: bool,
    has_cuttable_copy_target: bool,
) -> MenuBarOutput {
    let mut output = MenuBarOutput::default();
    egui::Panel::top("main_menu_bar").show(root_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            draw_file_menu(
                ui,
                l10n,
                actions,
                state.is_stroking(),
                renderer_available,
                project_history,
                state
                    .document()
                    .is_some_and(|document| document.has_materials()),
                &mut output,
            );
            draw_edit_menu(
                ui,
                l10n,
                actions,
                state.is_stroking(),
                has_single_copy_target,
                has_cuttable_copy_target,
                &mut output,
            );
            draw_select_menu(ui, l10n, state, actions, &mut output);
            draw_layer_menu(ui, l10n, state, actions, &mut output);
            draw_filter_menu(ui, l10n, state, actions, &mut output);
            draw_window_menu(ui, l10n, workspace, &mut output);
            draw_help_menu(ui, l10n, &mut output);
        });
    });
    output
}

fn draw_file_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    actions: EditorActionContext,
    stroking: bool,
    renderer_available: bool,
    project_history: &ProjectHistoryFileV1,
    document_has_materials: bool,
    output: &mut MenuBarOutput,
) {
    let input_blocked = actions.transient_ui_input_blocked(stroking);
    let actions = actions.presentation_context(stroking);
    ui.menu_button(l10n.text("menu-file"), |ui| {
        ui.availability_ui(true, input_blocked, |ui| {
            let document_edit_enabled = !actions.tool_interacting && !actions.editing_blocked();
            if ui
                .add_available(
                    document_edit_enabled,
                    egui::Button::new(l10n.text("menu-file-new-project")),
                )
                .clicked()
            {
                output.new_project_requested = true;
                ui.close();
            }
            if ui
                .add_available(
                    document_edit_enabled,
                    egui::Button::new(l10n.text("menu-file-open-project")),
                )
                .clicked()
            {
                output.open_project_requested = true;
                ui.close();
            }
            ui.add_available_ui(document_edit_enabled, |ui| {
                ui.menu_button(l10n.text("menu-file-recent-projects"), |ui| {
                    if project_history.entries.is_empty() {
                        disabled_menu_item(ui, l10n.text("menu-file-no-recent-projects"));
                    } else {
                        for entry in &project_history.entries {
                            let label = entry
                                .path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .filter(|name| !name.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| entry.path.display().to_string());
                            if ui
                                .button(label)
                                .on_hover_text(entry.path.display().to_string())
                                .clicked()
                            {
                                output.open_recent_project_requested = Some(entry.path.clone());
                                ui.close();
                            }
                        }
                    }
                });
            });
            ui.separator();
            let reload_enabled = actions.has_document
                && renderer_available
                && !actions.tool_interacting
                && !actions.editing_blocked();
            if ui
                .add_available(
                    reload_enabled,
                    egui::Button::new(l10n.text("menu-file-reload-mesh")),
                )
                .clicked()
            {
                output.reload_mesh_requested = true;
                ui.close();
            }
            ui.separator();
            let save_enabled = actions.can_save_project();
            if ui
                .add_available(
                    save_enabled,
                    egui::Button::new(l10n.text("menu-file-save-project")),
                )
                .clicked()
            {
                output.save_requested = true;
                ui.close();
            }
            if ui
                .add_available(
                    save_enabled,
                    egui::Button::new(l10n.text("menu-file-save-project-as")),
                )
                .clicked()
            {
                output.save_as_requested = true;
                ui.close();
            }
            ui.separator();
            let export_enabled = actions.has_document
                && document_has_materials
                && renderer_available
                && !actions.tool_interacting
                && !actions.editing_blocked();
            if ui
                .add_available(
                    export_enabled,
                    egui::Button::new(l10n.text("menu-file-export-image")),
                )
                .clicked()
            {
                output.export_image_requested = true;
                ui.close();
            }
            if ui
                .add_available(
                    export_enabled,
                    egui::Button::new(l10n.text("menu-file-export-psd")),
                )
                .clicked()
            {
                output.export_psd_requested = true;
                ui.close();
            }
            ui.separator();
            if ui.button(l10n.text("menu-file-quit")).clicked() {
                output.quit_requested = true;
                ui.close();
            }
        });
    });
}

fn draw_edit_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    actions: EditorActionContext,
    stroking: bool,
    has_single_copy_target: bool,
    has_cuttable_copy_target: bool,
    output: &mut MenuBarOutput,
) {
    let input_blocked = actions.transient_ui_input_blocked(stroking);
    let actions = actions.presentation_context(stroking);
    ui.menu_button(l10n.text("menu-edit"), |ui| {
        ui.availability_ui(true, input_blocked, |ui| {
            if ui
                .add_available(
                    actions.is_enabled(EditorAction::Undo),
                    egui::Button::new(l10n.text("menu-edit-undo")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::Undo);
                ui.close();
            }
            if ui
                .add_available(
                    actions.is_enabled(EditorAction::Redo),
                    egui::Button::new(l10n.text("menu-edit-redo")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::Redo);
                ui.close();
            }
            ui.separator();
            if ui
                .add_available(
                    actions.can_cut_image(has_cuttable_copy_target),
                    egui::Button::new(l10n.text("menu-edit-cut")),
                )
                .clicked()
            {
                output.cut_image_requested = true;
                ui.close();
            }
            if ui
                .add_available(
                    actions.can_copy_image(has_single_copy_target),
                    egui::Button::new(l10n.text("menu-edit-copy")),
                )
                .clicked()
            {
                output.copy_image_requested = true;
                ui.close();
            }
            if ui
                .add_available(
                    actions.can_paste_image(),
                    egui::Button::new(l10n.text("menu-edit-paste")),
                )
                .clicked()
            {
                output.paste_image_requested = true;
                ui.close();
            }
            ui.separator();
            if ui.button(l10n.text("menu-edit-settings")).clicked() {
                output.settings_requested = true;
                ui.close();
            }
        });
    });
}

fn draw_select_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    output: &mut MenuBarOutput,
) {
    let stroking = state.is_stroking();
    let input_blocked = actions.transient_ui_input_blocked(stroking);
    let actions = actions.presentation_context(stroking);
    ui.menu_button(l10n.text("menu-select"), |ui| {
        ui.availability_ui(true, input_blocked, |ui| {
            if ui
                .add_available(
                    actions.is_enabled(EditorAction::SelectAll),
                    egui::Button::new(l10n.text("menu-select-all")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::SelectAll);
                output.view.request_repaint();
                ui.close();
            }
            if ui
                .add_available(
                    actions.is_enabled(EditorAction::Deselect),
                    egui::Button::new(l10n.text("menu-select-deselect")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::Deselect);
                output.view.request_repaint();
                ui.close();
            }
            if ui
                .add_available(
                    actions.is_enabled(EditorAction::InvertSelection),
                    egui::Button::new(l10n.text("menu-select-invert")),
                )
                .clicked()
            {
                output.view.push_action(EditorAction::InvertSelection);
                output.view.request_repaint();
                ui.close();
            }
            ui.separator();
            let active_layer = state.active_layer_id();
            let can_select_from_layer = state.document().is_some_and(|document| {
                document
                    .layer_tree
                    .get(active_layer)
                    .is_some_and(|node| matches!(node.content, LayerContent::Raster))
            }) && !actions.tool_interacting
                && !actions.editing_blocked();
            if ui
                .add_available(
                    can_select_from_layer,
                    egui::Button::new(l10n.text("menu-select-from-layer-transparency")),
                )
                .clicked()
            {
                output.view.push(Command::SelectFromLayerTransparency {
                    layer_id: active_layer,
                });
                output.view.request_repaint();
                ui.close();
            }
        });
    });
}

fn draw_layer_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    output: &mut MenuBarOutput,
) {
    let stroking = state.is_stroking();
    let input_blocked = actions.transient_ui_input_blocked(stroking);
    let actions = actions.presentation_context(stroking);
    ui.menu_button(l10n.text("menu-layer"), |ui| {
        ui.availability_ui(true, input_blocked, |ui| {
            let layer_enabled = state.document().is_some()
                && !actions.tool_interacting
                && !actions.editing_blocked();
            if ui
                .add_available(
                    layer_enabled,
                    egui::Button::new(l10n.text("menu-layer-add")),
                )
                .clicked()
            {
                output.view.push(Command::AddLayer);
                output.view.request_repaint();
                ui.close();
            }
            if ui
                .add_available(
                    layer_enabled,
                    egui::Button::new(l10n.text("menu-layer-add-group")),
                )
                .clicked()
            {
                output.view.push(Command::AddGroup);
                output.view.request_repaint();
                ui.close();
            }

            if let Some(document) = state.document() {
                let active_layer = state.active_layer_id();
                let active_can_duplicate = active_layer != document.layer_tree.root()
                    && document.layer_tree.contains(active_layer);
                let active_has_mask = document.layer_tree.has_layer_mask(active_layer);
                let active_can_add_mask = document.layer_tree.can_add_layer_mask(active_layer);
                let active_mask_editable = !document.layer_tree.is_effectively_locked(active_layer);
                let active_can_rasterize = document.layer_tree.can_rasterize_layer(active_layer);
                let active_can_apply_mask = document.layer_tree.can_apply_layer_mask(active_layer);
                let active_removes = document
                    .layer_tree
                    .raster_layers_in_subtree(active_layer)
                    .len();
                let can_delete = active_layer != document.layer_tree.root()
                    && active_removes < document.layer_tree.ordered_raster_layers().len();

                ui.separator();
                if ui
                    .add_available(
                        layer_enabled && active_can_duplicate,
                        egui::Button::new(l10n.text("menu-layer-duplicate")),
                    )
                    .clicked()
                {
                    output.view.push(Command::DuplicateLayer {
                        layer_id: active_layer,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                if ui
                    .add_available(
                        layer_enabled && can_delete,
                        egui::Button::new(l10n.text("menu-layer-delete")),
                    )
                    .clicked()
                {
                    output.view.push(Command::DeleteLayer {
                        layer_id: active_layer,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_available(
                        layer_enabled && active_can_rasterize,
                        egui::Button::new(l10n.text("menu-layer-rasterize")),
                    )
                    .clicked()
                {
                    output.view.push(Command::RasterizeLayer {
                        layer_id: active_layer,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_available(
                        layer_enabled && active_can_add_mask,
                        egui::Button::new(l10n.text("menu-layer-mask-add")),
                    )
                    .clicked()
                {
                    output.view.push(Command::AddLayerMask {
                        layer_id: active_layer,
                        mode: LayerMaskInitMode::RevealAll,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                if ui
                    .add_available(
                        layer_enabled && active_can_apply_mask,
                        egui::Button::new(l10n.text("menu-layer-mask-apply")),
                    )
                    .clicked()
                {
                    output.view.push(Command::ApplyLayerMask {
                        layer_id: active_layer,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                if ui
                    .add_available(
                        layer_enabled && active_has_mask && active_mask_editable,
                        egui::Button::new(l10n.text("menu-layer-mask-delete")),
                    )
                    .clicked()
                {
                    output.view.push(Command::DeleteLayerMask {
                        layer_id: active_layer,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
                let mask_enabled = document
                    .layer_tree
                    .layer_mask(active_layer)
                    .map(|mask| mask.enabled)
                    .unwrap_or(false);
                let mask_label = if mask_enabled {
                    l10n.text("menu-layer-mask-disable")
                } else {
                    l10n.text("menu-layer-mask-enable")
                };
                if ui
                    .add_available(
                        layer_enabled && active_has_mask && active_mask_editable,
                        egui::Button::new(mask_label),
                    )
                    .clicked()
                {
                    output.view.push(Command::SetLayerMaskEnabled {
                        layer_id: active_layer,
                        enabled: !mask_enabled,
                    });
                    output.view.request_repaint();
                    ui.close();
                }
            } else {
                ui.separator();
                disabled_menu_item(ui, l10n.text("menu-layer-duplicate-layer"));
                disabled_menu_item(ui, l10n.text("menu-layer-delete"));
                disabled_menu_item(ui, l10n.text("menu-layer-rasterize"));
                disabled_menu_item(ui, l10n.text("menu-layer-mask-add"));
                disabled_menu_item(ui, l10n.text("menu-layer-mask-apply"));
                disabled_menu_item(ui, l10n.text("menu-layer-mask-delete"));
                disabled_menu_item(ui, l10n.text("menu-layer-mask-toggle"));
            }
        });
    });
}

fn draw_filter_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    output: &mut MenuBarOutput,
) {
    let stroking = state.is_stroking();
    let input_blocked = actions.transient_ui_input_blocked(stroking);
    let actions = actions.presentation_context(stroking);
    ui.menu_button(l10n.text("menu-filter"), |ui| {
        ui.availability_ui(true, input_blocked, |ui| {
            let filter_interaction_enabled =
                !actions.tool_interacting && !actions.editing_blocked();
            for kind in AdjustmentKind::DESTRUCTIVE_FILTERS {
                let label = if kind == AdjustmentKind::Invert {
                    adjustment_label(l10n, kind)
                } else {
                    format!("{}...", adjustment_label(l10n, kind))
                };
                let enabled = filter_interaction_enabled && state.adjustment_filter_available(kind);
                let response = ui.add_available(enabled, egui::Button::new(label));
                let response = if !enabled
                    && kind == AdjustmentKind::HueSaturation
                    && state.active_layer_target()
                        == crate::core::document::ActiveLayerTarget::LayerMask
                {
                    response.on_disabled_hover_text(l10n.text("menu-filter-hsv-mask-unavailable"))
                } else {
                    response
                };
                if response.clicked() {
                    output.adjustment_filter_requested = Some(kind);
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add_available(
                    filter_interaction_enabled && state.blur_filter_available(),
                    egui::Button::new(l10n.text("menu-filter-blur")),
                )
                .clicked()
            {
                output.blur_requested = true;
                ui.close();
            }
        });
    });
}

fn draw_window_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    workspace: &WorkspaceState,
    output: &mut MenuBarOutput,
) {
    ui.menu_button(l10n.text("menu-window"), |ui| {
        let groups: &[&[WorkspacePane]] = &[
            &[
                WorkspacePane::ToolList,
                WorkspacePane::ToolProperties,
                WorkspacePane::Color,
            ],
            &[WorkspacePane::Viewport3d, WorkspacePane::UvView],
            &[
                WorkspacePane::MaterialsTextures,
                WorkspacePane::Meshes,
                WorkspacePane::Layers,
            ],
        ];
        for (group_index, group) in groups.iter().enumerate() {
            if group_index > 0 {
                ui.separator();
            }
            for &pane in *group {
                let mut visible = workspace.pane_visible(pane);
                if ui
                    .checkbox(&mut visible, workspace_pane_label(l10n, pane))
                    .changed()
                {
                    output.pane_visibility.push((pane, visible));
                }
            }
        }
        ui.separator();
        if ui.button(l10n.text("menu-window-reset-layout")).clicked() {
            output.reset_layout_requested = true;
            ui.close();
        }
    });
}

fn draw_help_menu(ui: &mut egui::Ui, l10n: &Localization, output: &mut MenuBarOutput) {
    ui.menu_button(l10n.text("menu-help"), |ui| {
        if ui
            .button(l10n.text("menu-help-system-information"))
            .clicked()
        {
            output.system_information_requested = true;
            ui.close();
        }
        ui.separator();
        if ui.button(l10n.text("menu-help-about")).clicked() {
            output.about_requested = true;
            ui.close();
        }
    });
}

fn disabled_menu_item(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>) {
    ui.add_available(false, egui::Button::new(label));
}

fn adjustment_label(l10n: &Localization, kind: AdjustmentKind) -> String {
    l10n.text(match kind {
        AdjustmentKind::BrightnessContrast => "adjustment-brightness-contrast",
        AdjustmentKind::Levels => "adjustment-levels",
        AdjustmentKind::Curves => "adjustment-curves",
        AdjustmentKind::HueSaturation => "adjustment-hue-saturation",
        AdjustmentKind::Invert => "adjustment-invert",
        AdjustmentKind::GradientMap => "adjustment-gradient-map",
        AdjustmentKind::UvMirror => "adjustment-uv-mirror",
    })
}

fn workspace_pane_label(l10n: &Localization, pane: WorkspacePane) -> String {
    l10n.text(match pane {
        WorkspacePane::ToolList => "workspace-pane-tool-list",
        WorkspacePane::ToolProperties => "workspace-pane-tool-properties",
        WorkspacePane::Viewport3d => "workspace-pane-viewport-3d",
        WorkspacePane::UvView => "workspace-pane-uv-view",
        WorkspacePane::Color => "workspace-pane-color",
        WorkspacePane::MaterialsTextures => "workspace-pane-materials-textures",
        WorkspacePane::Meshes => "workspace-pane-meshes",
        WorkspacePane::Layers => "workspace-pane-layers",
    })
}
