use eframe::egui;

use crate::{
    application::{HoloPackConflictKind, HoloPackImportPlan},
    localization::Localization,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoloPackImportDialogAction {
    Cancel,
    Import,
}

pub(crate) fn draw_holopack_import_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    plan: &HoloPackImportPlan,
) -> Option<HoloPackImportDialogAction> {
    let mut action = None;
    egui::Modal::new(egui::Id::new("holopack_import_preview")).show(ctx, |ui| {
        ui.heading(l10n.text("holopack-import-title"));
        ui.add_space(6.0);
        ui.strong(plan.manifest.name.as_str());
        let mut args = fluent::FluentArgs::new();
        args.set("version", plan.manifest.version.to_string());
        ui.label(l10n.format("holopack-import-version", Some(&args)));
        if let Some(author) = plan.manifest.author.as_deref() {
            let mut args = fluent::FluentArgs::new();
            args.set("author", author);
            ui.label(l10n.format("holopack-import-author", Some(&args)));
        }
        if let Some(description) = plan.manifest.description.as_deref() {
            ui.add_space(4.0);
            ui.label(description);
        }
        ui.add_space(8.0);
        for (key, count) in [
            ("holopack-import-brush-engines", plan.brush_engines.len()),
            ("holopack-import-brush-presets", plan.brush_presets.len()),
            ("holopack-import-textures", plan.textures.len()),
        ] {
            let mut args = fluent::FluentArgs::new();
            args.set("count", count as i64);
            ui.label(l10n.format(key, Some(&args)));
        }

        if !plan.conflicts.is_empty() {
            ui.separator();
            let mut args = fluent::FluentArgs::new();
            args.set("count", plan.conflicts.len() as i64);
            ui.label(l10n.format("holopack-import-conflicts", Some(&args)));
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    for conflict in &plan.conflicts {
                        let kind = match conflict.kind {
                            HoloPackConflictKind::BrushEngine => l10n.text("resource-brush-engine"),
                            HoloPackConflictKind::BrushPreset => l10n.text("resource-brush-preset"),
                        };
                        ui.label(format!(
                            "- {kind}: {} ({})",
                            conflict.display_name, conflict.id
                        ));
                    }
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(l10n.text("dialog-cancel")).clicked() {
                action = Some(HoloPackImportDialogAction::Cancel);
            }
            let label = if plan.conflicts.is_empty() {
                l10n.text("action-import")
            } else {
                l10n.text("action-import-replace")
            };
            if ui.button(label).clicked() {
                action = Some(HoloPackImportDialogAction::Import);
            }
        });
    });
    action
}
