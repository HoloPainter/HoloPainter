use crate::ui::widgets::interaction_gate::InteractionGate;
use eframe::egui;

use super::material_swatch::paint_material_swatch;
use crate::{
    application::{AppState, Command},
    core::{
        material::{MaterialData, MaterialRenderMode},
        texture_size::TEXTURE_SIZE_CHOICES,
    },
    localization::Localization,
    ui::{
        icons::UiIconRegistry, view_output::ViewOutput,
        widgets::visibility_toggle::draw_visibility_toggle,
    },
};

const MATERIAL_PANEL_CONTENT_HORIZONTAL_PADDING: i8 = 8;
const MATERIAL_ROW_HEIGHT: f32 = 24.0;
const MATERIAL_SWATCH_SIZE: f32 = 12.0;
const MATERIAL_ROW_CONTENT_GAP: f32 = 4.0;
const MATERIAL_SIZE_COLUMN_WIDTH: f32 = 96.0;
const MATERIAL_ROW_CORNER_RADIUS: f32 = 0.0;

pub fn draw_materials_textures_panel(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    focused_texture_size: Option<[usize; 2]>,
    max_texture_dimension_2d: Option<u32>,
    icons: &UiIconRegistry,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some(document) = state.document() else {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(
                MATERIAL_PANEL_CONTENT_HORIZONTAL_PADDING,
                0,
            ))
            .show(ui, |ui| {
                ui.label(l10n.text("panel-no-mesh-loaded"));
                if focused_texture_size.is_none() {
                    ui.label(l10n.text("panel-texture-data-unavailable"));
                }
            });
        return output;
    };

    if document.has_materials() {
        let mut focused = state.focused_material_index();
        let interacting = state.is_document_edit_interacting();
        let stroking = state.is_stroking();
        if let Some(material) = document.materials.get(focused) {
            ui.separator();
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(
                    MATERIAL_PANEL_CONTENT_HORIZONTAL_PADDING,
                    0,
                ))
                .show(ui, |ui| {
                    output.extend(draw_material_properties(
                        ui,
                        l10n,
                        focused,
                        material,
                        interacting,
                        stroking,
                    ));
                });
        }
        ui.availability_ui(!interacting || stroking, stroking, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("workspace_materials_list_view")
                .max_height(240.0)
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, _viewport| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, material) in document.materials.iter().enumerate() {
                        let selected = index == focused;
                        let visible = state.viewport_material_visible(index);
                        ui.push_id(index, |ui| {
                            let row_response = draw_material_row(
                                ui, l10n, icons, index, material, selected, visible,
                            );
                            if row_response.visibility.clicked() {
                                output.push(Command::SetViewportMaterialVisible {
                                    material_index: index,
                                    visible: !visible,
                                });
                                output.request_repaint();
                            }
                            if row_response.selection.clicked() {
                                focused = index;
                            }
                            if row_response.texture_size.clicked() {
                                focused = index;
                            }
                            let popup_id = ui.id().with("material_texture_size_popup");
                            if interacting {
                                egui::Popup::close_id(ui.ctx(), popup_id);
                            }
                            egui::Popup::menu(&row_response.texture_size)
                                .id(popup_id)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                    ui.set_min_width(MATERIAL_SIZE_COLUMN_WIDTH);
                                    for size in TEXTURE_SIZE_CHOICES {
                                        let supported = max_texture_dimension_2d
                                            .is_some_and(|maximum| size <= maximum);
                                        let selected = material.texture_size == [size; 2];
                                        let response = ui
                                            .add_available_ui(supported, |ui| {
                                                ui.selectable_label(
                                                    selected,
                                                    format!("{size} x {size}"),
                                                )
                                            })
                                            .inner;
                                        let response = if supported {
                                            response
                                        } else {
                                            response.on_disabled_hover_text(
                                                l10n.text("panel-texture-size-gpu-unsupported"),
                                            )
                                        };
                                        if response.clicked() {
                                            egui::Popup::close_id(ui.ctx(), popup_id);
                                            if !selected {
                                                output.push(Command::ResizeMaterialTexture {
                                                    material_index: index,
                                                    texture_size: [size; 2],
                                                });
                                                output.request_repaint();
                                            }
                                        }
                                    }
                                });
                        });
                    }
                });
        });

        if interacting {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(
                    MATERIAL_PANEL_CONTENT_HORIZONTAL_PADDING,
                    0,
                ))
                .show(ui, |ui| {
                    ui.label(l10n.text("panel-material-interaction-blocked"));
                });
        }
        if focused != state.focused_material_index() {
            output.push(Command::SetFocusedMaterial(focused));
            output.request_repaint();
        }
    } else {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(
                MATERIAL_PANEL_CONTENT_HORIZONTAL_PADDING,
                0,
            ))
            .show(ui, |ui| {
                ui.label(l10n.text("panel-no-materials"));
            });
    }

    output
}

struct MaterialRowResponse {
    selection: egui::Response,
    visibility: egui::Response,
    texture_size: egui::Response,
}

struct MaterialContentResponse {
    selection: egui::Response,
    texture_size: egui::Response,
}

fn draw_material_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    icons: &UiIconRegistry,
    index: usize,
    material: &MaterialData,
    selected: bool,
    visible: bool,
) -> MaterialRowResponse {
    let mut content_response = None;
    let mut visibility_response = None;
    let inner = egui::Frame::NONE
        .fill(material_row_background_fill(ui, selected))
        .show(ui, |ui| {
            ui.set_min_height(MATERIAL_ROW_HEIGHT);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = MATERIAL_ROW_CONTENT_GAP;

                let response = draw_visibility_toggle(ui, icons, visible);
                visibility_response = Some(response.on_hover_text(if visible {
                    l10n.text("panel-hide-in-3d-view")
                } else {
                    l10n.text("panel-show-in-3d-view")
                }));

                content_response = Some(draw_material_content(
                    ui, l10n, index, material, selected, visible,
                ));
            });
        });

    let pointer_over_row = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pointer_pos| inner.response.rect.contains(pointer_pos));
    paint_material_row_interaction_state(ui, inner.response.rect, pointer_over_row);

    let content = content_response.expect("material rows must expose content responses");
    MaterialRowResponse {
        selection: content.selection,
        visibility: visibility_response.expect("material rows must expose a visibility response"),
        texture_size: content
            .texture_size
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(l10n.text("panel-change-texture-size")),
    }
}

fn draw_material_content(
    ui: &mut egui::Ui,
    l10n: &Localization,
    index: usize,
    material: &MaterialData,
    selected: bool,
    visible: bool,
) -> MaterialContentResponse {
    let desired_size = egui::vec2(ui.available_width(), MATERIAL_ROW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let swatch_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + MATERIAL_SWATCH_SIZE * 0.5, rect.center().y),
        egui::Vec2::splat(MATERIAL_SWATCH_SIZE),
    );
    let size_rect = egui::Rect::from_min_max(
        egui::pos2(
            (rect.right() - MATERIAL_SIZE_COLUMN_WIDTH)
                .max(swatch_rect.right() + MATERIAL_ROW_CONTENT_GAP),
            rect.top(),
        ),
        egui::pos2(rect.right(), rect.bottom()),
    );
    let selection_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            (size_rect.left() - MATERIAL_ROW_CONTENT_GAP).max(rect.left()),
            rect.bottom(),
        ),
    );
    let selection = ui.interact(
        selection_rect,
        ui.id().with(("material_selection", index)),
        egui::Sense::click(),
    );
    let texture_size = ui.interact(
        size_rect,
        ui.id().with(("material_texture_size", index)),
        egui::Sense::click(),
    );

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&selection, selected);
        paint_material_swatch(ui, swatch_rect, material.ui_color);
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(swatch_rect.right() + MATERIAL_ROW_CONTENT_GAP, rect.top()),
            egui::pos2(
                selection_rect
                    .right()
                    .max(swatch_rect.right() + MATERIAL_ROW_CONTENT_GAP),
                rect.bottom(),
            ),
        );
        let text_color = if visible {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        let font_id = egui::FontId::proportional(14.0);

        if name_rect.width() > 0.0 {
            ui.painter().with_clip_rect(name_rect).text(
                egui::pos2(name_rect.left(), name_rect.center().y),
                egui::Align2::LEFT_CENTER,
                material_list_name(l10n, index, material),
                font_id.clone(),
                text_color,
            );
        }
        if size_rect.width() > 0.0 {
            if texture_size.hovered() {
                ui.painter().rect_filled(
                    size_rect.shrink(1.0),
                    MATERIAL_ROW_CORNER_RADIUS,
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            ui.painter().with_clip_rect(size_rect).text(
                size_rect.center(),
                egui::Align2::CENTER_CENTER,
                material_list_size(material),
                font_id,
                text_color,
            );
        }
    }

    MaterialContentResponse {
        selection,
        texture_size,
    }
}

fn material_row_background_fill(ui: &egui::Ui, selected: bool) -> egui::Color32 {
    if selected {
        translucent(ui.visuals().selection.bg_fill, 96)
    } else {
        egui::Color32::TRANSPARENT
    }
}

fn paint_material_row_interaction_state(ui: &egui::Ui, rect: egui::Rect, pointer_over_row: bool) {
    if !pointer_over_row || !ui.is_rect_visible(rect) {
        return;
    }

    ui.painter().rect_stroke(
        rect.shrink(0.5),
        MATERIAL_ROW_CORNER_RADIUS,
        egui::Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

fn material_list_name(l10n: &Localization, index: usize, material: &MaterialData) -> String {
    if material.name.is_empty() {
        let mut args = fluent::FluentArgs::new();
        args.set("index", index as i64);
        l10n.format("material-fallback-name", Some(&args))
    } else {
        material.name.clone()
    }
}

fn material_list_size(material: &MaterialData) -> String {
    format!(
        "{} x {}",
        material.texture_size[0], material.texture_size[1]
    )
}

fn translucent(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn draw_material_properties(
    ui: &mut egui::Ui,
    l10n: &Localization,
    material_index: usize,
    material: &MaterialData,
    interacting: bool,
    stroking: bool,
) -> ViewOutput {
    let mut output = ViewOutput::default();

    let mut settings = material.render_settings;
    ui.availability_ui(!interacting || stroking, stroking, |ui| {
        egui::ComboBox::from_id_salt("material_rendering_mode")
            .selected_text(match settings.render_mode {
                MaterialRenderMode::Opaque => l10n.text("material-rendering-opaque"),
                MaterialRenderMode::Cutoff => l10n.text("material-rendering-cutoff"),
                MaterialRenderMode::Blend => l10n.text("material-rendering-blend"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut settings.render_mode,
                    MaterialRenderMode::Opaque,
                    l10n.text("material-rendering-opaque"),
                );
                ui.selectable_value(
                    &mut settings.render_mode,
                    MaterialRenderMode::Cutoff,
                    l10n.text("material-rendering-cutoff"),
                );
                ui.selectable_value(
                    &mut settings.render_mode,
                    MaterialRenderMode::Blend,
                    l10n.text("material-rendering-blend"),
                );
            });
    });
    ui.availability_ui(!interacting || stroking, stroking, |ui| {
        ui.add(egui::Checkbox::new(
            &mut settings.double_sided,
            l10n.text("material-double-sided"),
        ))
    })
    .inner
    .on_hover_text(if settings.double_sided {
        l10n.text("material-double-sided-enabled-help")
    } else {
        l10n.text("material-double-sided-disabled-help")
    });
    if settings.render_mode == MaterialRenderMode::Cutoff {
        ui.availability_ui(!interacting || stroking, stroking, |ui| {
            ui.add(
                egui::Slider::new(&mut settings.alpha_cutoff, 0..=255)
                    .text(l10n.text("material-alpha-cutoff")),
            )
        });
    }

    if settings != material.render_settings {
        output.push(Command::SetMaterialRenderSettings {
            material_index,
            settings,
        });
        output.request_repaint();
    }

    output
}
