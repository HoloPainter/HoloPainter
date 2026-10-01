use eframe::egui;

use crate::{
    core::image_asset::{ImageAssetCatalog, ImageMaskRecipe, ImageMaskSource, build_r8_mask},
    localization::Localization,
};

const PREVIEW_SIZE: f32 = 128.0;

pub(crate) struct ImageMaskConversionDraft {
    pub(crate) asset_id: String,
    pub(crate) required_tags: Vec<String>,
    pub(crate) recipe: ImageMaskRecipe,
    source_preview: Option<egui::TextureHandle>,
    mask_preview: Option<(ImageMaskRecipe, egui::TextureHandle)>,
}

impl ImageMaskConversionDraft {
    pub(crate) fn new(asset_id: String, required_tags: Vec<String>) -> Self {
        Self {
            asset_id,
            required_tags,
            recipe: ImageMaskRecipe {
                source: ImageMaskSource::Alpha,
                invert: false,
            },
            source_preview: None,
            mask_preview: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImageMaskConversionAction {
    Create {
        asset_id: String,
        required_tags: Vec<String>,
        recipe: ImageMaskRecipe,
    },
    Cancel,
}

pub(crate) fn draw_image_mask_conversion_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    catalog: &ImageAssetCatalog,
    draft: &mut Option<ImageMaskConversionDraft>,
) -> Option<ImageMaskConversionAction> {
    let Some(state) = draft.as_mut() else {
        return None;
    };
    let Some(asset) = catalog.get(&state.asset_id) else {
        return Some(ImageMaskConversionAction::Cancel);
    };
    let Some(rgba) = asset.rgba8_representation() else {
        return Some(ImageMaskConversionAction::Cancel);
    };
    let Some(rgba8) = rgba.rgba8() else {
        return Some(ImageMaskConversionAction::Cancel);
    };

    if state.source_preview.is_none() {
        state.source_preview = Some(ctx.load_texture(
            format!("image_mask_source:{}", state.asset_id),
            egui::ColorImage::from_rgba_unmultiplied(
                [rgba.size[0] as usize, rgba.size[1] as usize],
                rgba8,
            ),
            egui::TextureOptions::LINEAR,
        ));
    }
    if state
        .mask_preview
        .as_ref()
        .map_or(true, |(recipe, _)| recipe != &state.recipe)
        && let Ok(mask) = build_r8_mask(rgba8, rgba.size, &state.recipe)
    {
        let pixels = mask
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect::<Vec<_>>();
        state.mask_preview = Some((
            state.recipe.clone(),
            ctx.load_texture(
                format!(
                    "image_mask_result:{}:{:?}:{}",
                    state.asset_id, state.recipe.source, state.recipe.invert
                ),
                egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.size[0] as usize, rgba.size[1] as usize],
                    &pixels,
                ),
                egui::TextureOptions::LINEAR,
            ),
        ));
    }

    let mut open = true;
    let mut action = None;
    egui::Window::new(l10n.text("image-mask-create-title"))
        .id(egui::Id::new("image_mask_conversion_dialog"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(asset.display_name.as_str());
            ui.label(format!("{} × {}", rgba.size[0], rgba.size[1]));
            if !state.required_tags.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(l10n.text("image-mask-required-tags"));
                    for tag in &state.required_tags {
                        ui.monospace(tag);
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(source) = &state.source_preview {
                    ui.vertical(|ui| {
                        ui.label(l10n.text("image-mask-source-preview"));
                        ui.add(
                            egui::Image::new(source)
                                .fit_to_exact_size(egui::Vec2::splat(PREVIEW_SIZE)),
                        );
                    });
                }
                if let Some((_, mask)) = &state.mask_preview {
                    ui.vertical(|ui| {
                        ui.label(l10n.text("image-mask-mask-preview"));
                        ui.add(
                            egui::Image::new(mask)
                                .fit_to_exact_size(egui::Vec2::splat(PREVIEW_SIZE)),
                        );
                    });
                }
            });
            ui.add_space(6.0);
            egui::ComboBox::from_label(l10n.text("image-mask-source"))
                .selected_text(mask_source_label(l10n, state.recipe.source))
                .show_ui(ui, |ui| {
                    for source in [
                        ImageMaskSource::Alpha,
                        ImageMaskSource::Luminance,
                        ImageMaskSource::Red,
                        ImageMaskSource::Green,
                        ImageMaskSource::Blue,
                    ] {
                        ui.selectable_value(
                            &mut state.recipe.source,
                            source,
                            mask_source_label(l10n, source),
                        );
                    }
                });
            ui.checkbox(&mut state.recipe.invert, l10n.text("adjustment-invert"));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(l10n.text("dialog-cancel")).clicked() {
                    action = Some(ImageMaskConversionAction::Cancel);
                }
                if ui.button(l10n.text("action-create")).clicked() {
                    action = Some(ImageMaskConversionAction::Create {
                        asset_id: state.asset_id.clone(),
                        required_tags: state.required_tags.clone(),
                        recipe: state.recipe.clone(),
                    });
                }
            });
        });
    if !open {
        action = Some(ImageMaskConversionAction::Cancel);
    }
    action
}

fn mask_source_label(l10n: &Localization, source: ImageMaskSource) -> String {
    l10n.text(match source {
        ImageMaskSource::Alpha => "image-mask-channel-alpha",
        ImageMaskSource::Luminance => "image-mask-channel-luminance",
        ImageMaskSource::Red => "image-mask-channel-red",
        ImageMaskSource::Green => "image-mask-channel-green",
        ImageMaskSource::Blue => "image-mask-channel-blue",
    })
}
