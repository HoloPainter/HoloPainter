use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::document::MeshObject,
    localization::Localization,
    ui::{
        icons::UiIconRegistry, view_output::ViewOutput,
        widgets::visibility_toggle::draw_visibility_toggle,
    },
};

const MESH_ICON: &str = "builtin.icon.mesh";
const MESH_ROW_HEIGHT: f32 = 24.0;
const MESH_ICON_SIZE: f32 = 18.0;
const MESH_ROW_HORIZONTAL_PADDING: f32 = 4.0;
const MESH_ROW_CONTENT_GAP: f32 = 6.0;

pub fn draw_mesh_panel(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    icons: &UiIconRegistry,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some(document) = state.document() else {
        ui.label(l10n.text("panel-no-mesh-loaded"));
        return output;
    };

    if document.mesh.mesh_objects.is_empty() {
        ui.label(l10n.text("panel-no-mesh-objects"));
        return output;
    }

    let interacting = state.is_document_edit_interacting();
    ui.add_enabled_ui(!interacting, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("workspace_meshes_list_view")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, mesh) in document.mesh.mesh_objects.iter().enumerate() {
                    let visible = state.viewport_mesh_visible(mesh.id);
                    ui.push_id(mesh.id.0, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = MESH_ROW_CONTENT_GAP;
                            let visibility_response = draw_visibility_toggle(ui, icons, visible);
                            let visibility_clicked = visibility_response.clicked();
                            visibility_response.on_hover_text(if visible {
                                l10n.text("panel-hide-in-3d-view")
                            } else {
                                l10n.text("panel-show-in-3d-view")
                            });

                            let row_response = draw_mesh_row(ui, l10n, index, mesh, visible, icons);
                            let mut args = fluent::FluentArgs::new();
                            args.set("id", mesh.id.0 as i64);
                            row_response.on_hover_text(l10n.format("panel-mesh-id", Some(&args)));

                            if visibility_clicked {
                                output.push(Command::SetViewportMeshVisible {
                                    mesh_id: mesh.id,
                                    visible: !visible,
                                });
                                output.request_repaint();
                            }
                        });
                    });
                }
            });
    });

    if interacting {
        ui.label(l10n.text("panel-mesh-interaction-blocked"));
    }

    output
}

fn draw_mesh_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    index: usize,
    mesh: &MeshObject,
    visible: bool,
    icons: &UiIconRegistry,
) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), MESH_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        if response.hovered() || response.highlighted() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }

        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(
                rect.left() + MESH_ROW_HORIZONTAL_PADDING + MESH_ICON_SIZE * 0.5,
                rect.center().y,
            ),
            egui::Vec2::splat(MESH_ICON_SIZE),
        );
        if let Some(texture) = icons.texture(MESH_ICON) {
            let tint = if visible {
                visuals.fg_stroke.color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                tint,
            );
        }

        let text_pos = egui::pos2(icon_rect.right() + MESH_ROW_CONTENT_GAP, rect.center().y);
        let text_color = if visible {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            mesh_list_label(l10n, index, mesh),
            egui::FontId::proportional(14.0),
            text_color,
        );
    }

    response
}

fn mesh_list_label(l10n: &Localization, index: usize, mesh: &MeshObject) -> String {
    if mesh.name.trim().is_empty() {
        let mut args = fluent::FluentArgs::new();
        args.set("index", index as i64);
        l10n.format("panel-mesh-fallback-name", Some(&args))
    } else {
        mesh.name.clone()
    }
}
