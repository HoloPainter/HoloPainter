use eframe::egui;

use crate::{
    application::AppState,
    localization::Localization,
    ui::{localized, panels::runtime_metrics::RuntimeMetricsUiData},
};

pub fn draw_status_bar(
    root_ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    focused_texture_size: Option<[usize; 2]>,
    metrics: &RuntimeMetricsUiData,
    brush_save_error: Option<&str>,
) {
    egui::Panel::bottom("status_bar").show(root_ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(error) = brush_save_error {
                let mut args = fluent::FluentArgs::new();
                args.set("error", error);
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    l10n.format("status-brush-autosave-failed", Some(&args)),
                );
            } else {
                ui.label(state.status().format(l10n));
            }
            ui.separator();
            let mut args = fluent::FluentArgs::new();
            args.set("name", active_tool_name(l10n, state));
            ui.label(l10n.format("status-bar-tool", Some(&args)));
            ui.separator();
            let mut args = fluent::FluentArgs::new();
            args.set("name", focused_material_name(state));
            ui.label(l10n.format("status-bar-material", Some(&args)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut args = fluent::FluentArgs::new();
                args.set("version", env!("CARGO_PKG_VERSION"));
                ui.label(l10n.format("about-version", Some(&args)));
                ui.separator();
                ui.label(texture_status(l10n, focused_texture_size, metrics));
            });
        });
    });
}

fn active_tool_name(l10n: &Localization, state: &AppState) -> String {
    state
        .effective_tool_definition()
        .map(|tool| localized::tool_name(l10n, state, tool))
        .unwrap_or_default()
}

fn focused_material_name(state: &AppState) -> &str {
    state
        .document()
        .and_then(|document| document.materials.get(state.focused_material_index()))
        .map(|material| material.name.as_str())
        .unwrap_or("")
}

fn texture_status(
    l10n: &Localization,
    focused_texture_size: Option<[usize; 2]>,
    metrics: &RuntimeMetricsUiData,
) -> String {
    match focused_texture_size {
        Some(size) => {
            let mut args = fluent::FluentArgs::new();
            args.set("width", size[0] as i64);
            args.set("height", size[1] as i64);
            args.set("bytes", metrics.textures.total_bytes as i64);
            l10n.format("status-bar-texture", Some(&args))
        }
        None => l10n.text("status-bar-texture-unavailable"),
    }
}
