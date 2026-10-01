use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::{
        decal::DecalNormalMode,
        image_asset::{IMAGE_ASSET_DECAL_TAG, ImageAssetCatalog},
        tool::ToolId,
    },
    localization::Localization,
    ui::{
        input::shortcut_profile::{ShortcutAction, ShortcutProfile},
        render_resources::UiRenderResources,
        tool_options::widgets::labeled_slider,
        view_output::{UiRequest, ViewOutput},
        widgets::image_asset_picker::{ImageAssetThumbnailCache, draw_image_asset_picker},
    },
};

const THUMBNAIL_MAX_SIZE: f32 = 160.0;
const CHECKER_SIZE: f32 = 12.0;

pub(crate) fn draw_decal_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    render_resources: &UiRenderResources,
    image_assets: &ImageAssetCatalog,
    selected_decal_asset_id: Option<&str>,
    asset_thumbnails: &mut ImageAssetThumbnailCache,
    shortcut_profile: &ShortcutProfile,
) -> ViewOutput {
    let mut output = ViewOutput::default();

    let mut options = image_assets
        .assets()
        .filter(|asset| {
            asset.rgba8_representation().is_some_and(|representation| {
                representation
                    .tags
                    .iter()
                    .any(|tag| tag == IMAGE_ASSET_DECAL_TAG)
            })
        })
        .collect::<Vec<_>>();
    options.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let picker = draw_image_asset_picker(
        ui,
        l10n,
        egui::Id::new("decal_image_asset_picker"),
        selected_decal_asset_id,
        &options,
        image_assets,
        asset_thumbnails,
    );
    if let Some(asset_id) = picker.selected_asset_id {
        output.request_ui(UiRequest::SelectDecalAsset { asset_id });
    }
    if picker.import_requested {
        output.request_ui(UiRequest::ImportDecalImage);
    }
    if picker.library_requested {
        output.request_ui(UiRequest::OpenImageAssetLibrary);
    }

    ui.add_space(4.0);
    draw_thumbnail(ui, l10n, state, render_resources);

    if let Some(image) = state.decal_image() {
        ui.label(image.file_name.as_str());
        ui.label(format!("{} x {}", image.size[0], image.size[1]));
    } else {
        ui.label(l10n.text("tool-decal-no-image-selected"));
    }

    ui.add_space(4.0);
    let mut options = state.decal_options().clone();
    if labeled_slider(
        ui,
        l10n.text("field-opacity"),
        egui::Slider::new(&mut options.opacity, 0.0..=1.0),
    )
    .changed()
    {
        output.push(Command::UpdateDecalToolOptions(options.clone()));
    }

    if state.effective_tool_id() == ToolId::SurfaceDecal {
        let previous_mode = options.normal_mode;
        egui::ComboBox::from_label(l10n.text("tool-decal-normal"))
            .selected_text(match options.normal_mode {
                DecalNormalMode::Face => l10n.text("tool-decal-normal-face"),
                DecalNormalMode::Smooth => l10n.text("tool-decal-normal-smooth"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut options.normal_mode,
                    DecalNormalMode::Face,
                    l10n.text("tool-decal-normal-face"),
                );
                ui.selectable_value(
                    &mut options.normal_mode,
                    DecalNormalMode::Smooth,
                    l10n.text("tool-decal-normal-smooth"),
                );
            });
        if options.normal_mode != previous_mode {
            output.push(Command::UpdateDecalToolOptions(options.clone()));
        }

        let angle_response =
            ui.add_enabled_ui(options.normal_mode == DecalNormalMode::Smooth, |ui| {
                labeled_slider(
                    ui,
                    l10n.text("tool-decal-smooth-angle"),
                    egui::Slider::new(&mut options.smooth_angle_degrees, 0.0..=180.0).suffix("°"),
                )
            });
        if angle_response.inner.changed() {
            output.push(Command::UpdateDecalToolOptions(options));
        }
    }

    ui.add_space(4.0);
    if state.has_active_decal_session() {
        ui.horizontal(|ui| {
            if ui.button(l10n.text("action-apply")).clicked() {
                output.push(Command::ApplyActiveDecal);
            }
            if ui.button(l10n.text("action-cancel")).clicked() {
                output.push(Command::CancelActiveDecal);
            }
        });
        ui.label(l10n.text("tool-decal-drag-inside"));
        ui.label(l10n.text("tool-decal-drag-handles"));
        ui.label(l10n.text("tool-decal-modifier-help"));
        let apply = shortcut_profile
            .first_chord_for_action(ShortcutAction::ApplyActiveOperation)
            .map(|chord| chord.display())
            .unwrap_or_else(|| l10n.text("shortcut-unassigned"));
        let cancel = shortcut_profile
            .first_chord_for_action(ShortcutAction::CancelActiveOperation)
            .map(|chord| chord.display())
            .unwrap_or_else(|| l10n.text("shortcut-unassigned"));
        let mut args = fluent::FluentArgs::new();
        args.set("shortcut", apply);
        ui.label(l10n.format("tool-decal-apply-shortcut", Some(&args)));
        let mut args = fluent::FluentArgs::new();
        args.set("shortcut", cancel);
        ui.label(l10n.format("tool-decal-cancel-shortcut", Some(&args)));
    } else if state.decal_image().is_some()
        && state.effective_tool_id() == ToolId::ViewProjectionDecal
    {
        if ui.button(l10n.text("tool-decal-start-new")).clicked() {
            output.push(Command::RequestViewProjectionDecal);
        }
        ui.label(l10n.text("tool-decal-view-projection-help"));
    } else if state.decal_image().is_some() {
        ui.label(l10n.text("tool-decal-place-help"));
    } else {
        //ui.label(l10n.text("tool-decal-select-image-help"));
    }

    output
}

fn draw_thumbnail(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    render_resources: &UiRenderResources,
) {
    let side = ui.available_width().min(THUMBNAIL_MAX_SIZE).max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    paint_checkerboard(ui.painter(), rect);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(3),
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let Some(image) = state.decal_image() else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            l10n.text("tool-decal-no-image"),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        return;
    };
    let Some(texture_id) = render_resources.decal_thumbnail_texture_id else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            l10n.text("tool-decal-preview-unavailable"),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        return;
    };

    let image_size = egui::vec2(image.size[0] as f32, image.size[1] as f32);
    let scale = (rect.width() / image_size.x)
        .min(rect.height() / image_size.y)
        .min(1.0);
    let display_size = image_size * scale;
    let image_rect = egui::Rect::from_center_size(rect.center(), display_size);
    ui.painter().image(
        texture_id,
        image_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn paint_checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(3),
        egui::Color32::from_gray(92),
    );
    let light = egui::Color32::from_gray(126);
    let columns = (rect.width() / CHECKER_SIZE).ceil() as usize;
    let rows = (rect.height() / CHECKER_SIZE).ceil() as usize;
    for row in 0..rows {
        for column in 0..columns {
            if (row + column) % 2 != 0 {
                continue;
            }
            let min = egui::pos2(
                rect.left() + column as f32 * CHECKER_SIZE,
                rect.top() + row as f32 * CHECKER_SIZE,
            );
            let max = egui::pos2(
                (min.x + CHECKER_SIZE).min(rect.right()),
                (min.y + CHECKER_SIZE).min(rect.bottom()),
            );
            painter.rect_filled(
                egui::Rect::from_min_max(min, max),
                egui::CornerRadius::ZERO,
                light,
            );
        }
    }
}
