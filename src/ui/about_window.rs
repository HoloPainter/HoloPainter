use super::icons::UiIconRegistry;
use crate::localization::Localization;
use eframe::egui;

pub(crate) fn draw_about_window(
    ctx: &egui::Context,
    open: &mut bool,
    l10n: &Localization,
    icons: &UiIconRegistry,
) {
    if !*open {
        return;
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        *open = false;
        return;
    }
    let mut close_requested = false;
    egui::Window::new(l10n.text("about-title"))
        .id(egui::Id::new("about_holopainter_window"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .default_width(320.0)
        .default_pos(ctx.content_rect().center() - egui::vec2(160.0, 150.0))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                if let Some(icon) = icons.texture("builtin.icon.app") {
                    ui.image((icon.id(), egui::vec2(80.0, 80.0)));
                }
                ui.add_space(12.0);
                ui.heading(l10n.text("app-name"));
                let mut args = fluent::FluentArgs::new();
                args.set("version", env!("CARGO_PKG_VERSION"));
                ui.label(l10n.format("about-version", Some(&args)));
                ui.add_space(20.0);
                for (label, url) in [
                    ("GitHub ↗", "https://github.com/HoloPainter/HoloPainter"),
                    (
                        "Microsoft Store ↗",
                        "https://apps.microsoft.com/detail/9P8X0BQCLJBH",
                    ),
                ] {
                    ui.hyperlink_to(label, url).on_hover_text(url);
                    ui.add_space(6.0);
                }
                ui.add_space(14.0);
                close_requested = ui.button(l10n.text("about-close")).clicked();
                ui.add_space(8.0);
            });
        });
    if close_requested {
        *open = false;
    }
}
