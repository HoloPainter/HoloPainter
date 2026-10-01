use std::collections::HashMap;

use eframe::egui;

use crate::{
    core::image_asset::{
        ImageAssetCatalog, ImageAssetDefinition, ImageAssetOrigin, ImageRepresentationFormat,
    },
    localization::Localization,
};

const SELECTOR_WIDTH: f32 = 190.0;
const SELECTOR_HEIGHT: f32 = 30.0;
const SELECTOR_PREVIEW_SIZE: f32 = 22.0;
const POPUP_WIDTH: f32 = 340.0;
const POPUP_MAX_HEIGHT: f32 = 320.0;
const TILE_SIZE: f32 = 58.0;
const TILE_PREVIEW_SIZE: f32 = 46.0;
const TILE_GAP: f32 = 6.0;
const PREVIEW_BACKGROUND: u8 = 48;

#[derive(Default)]
pub(crate) struct ImageAssetThumbnailCache {
    textures: HashMap<String, egui::TextureHandle>,
}

impl std::fmt::Debug for ImageAssetThumbnailCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageAssetThumbnailCache")
            .field("texture_count", &self.textures.len())
            .finish()
    }
}

impl ImageAssetThumbnailCache {
    pub(crate) fn texture<'a>(
        &'a mut self,
        ctx: &egui::Context,
        asset: &ImageAssetDefinition,
    ) -> Option<&'a egui::TextureHandle> {
        if !self.textures.contains_key(&asset.id) {
            let image = asset_preview_image(asset)?;
            let handle = ctx.load_texture(
                format!("image_asset_thumbnail:{}", asset.id),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.textures.insert(asset.id.clone(), handle);
        }
        self.textures.get(&asset.id)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ImageAssetPickerOutput {
    pub(crate) selected_asset_id: Option<String>,
    pub(crate) import_requested: bool,
    pub(crate) library_requested: bool,
}

pub(crate) fn draw_image_asset_picker(
    ui: &mut egui::Ui,
    l10n: &Localization,
    id: egui::Id,
    selected_asset_id: Option<&str>,
    options: &[&ImageAssetDefinition],
    catalog: &ImageAssetCatalog,
    cache: &mut ImageAssetThumbnailCache,
) -> ImageAssetPickerOutput {
    let mut output = ImageAssetPickerOutput::default();
    let selected = selected_asset_id.and_then(|selected_id| {
        options
            .iter()
            .copied()
            .find(|asset| asset.id == selected_id)
    });
    let asset_name = |asset: &ImageAssetDefinition| {
        if matches!(catalog.origin(&asset.id), Some(ImageAssetOrigin::Builtin)) {
            l10n.builtin_name("image", &asset.id, &asset.display_name)
        } else {
            asset.display_name.clone()
        }
    };
    let selected_text = selected
        .map(asset_name)
        .unwrap_or_else(|| l10n.text("image-picker-select-image"));
    let selector_width = SELECTOR_WIDTH.min(ui.available_width());
    let response = if let Some(asset) = selected {
        if let Some(handle) = cache.texture(ui.ctx(), asset) {
            let image = egui::Image::new(handle)
                .fit_to_exact_size(egui::Vec2::splat(SELECTOR_PREVIEW_SIZE));
            ui.add_sized(
                egui::vec2(selector_width, SELECTOR_HEIGHT),
                egui::Button::image_and_text(image, format!("{selected_text}  ▾")),
            )
        } else {
            ui.add_sized(
                egui::vec2(selector_width, SELECTOR_HEIGHT),
                egui::Button::new(format!("{selected_text}  ▾")),
            )
        }
    } else {
        ui.add_sized(
            egui::vec2(selector_width, SELECTOR_HEIGHT),
            egui::Button::new(format!("{selected_text}  ▾")),
        )
    };

    let popup_id = id.with("popup");
    egui::Popup::menu(&response)
        .id(popup_id)
        .width(POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(POPUP_WIDTH - 20.0);
            egui::ScrollArea::vertical()
                .max_height(POPUP_MAX_HEIGHT)
                .show(ui, |ui| {
                    if options.is_empty() {
                        ui.label(l10n.text("image-picker-no-matching-images"));
                    } else {
                        let columns = thumbnail_grid_column_count(ui.available_width());
                        egui::Grid::new(id.with("grid"))
                            .num_columns(columns)
                            .min_col_width(TILE_SIZE)
                            .spacing([TILE_GAP, TILE_GAP])
                            .show(ui, |ui| {
                                for (index, asset) in options.iter().enumerate() {
                                    let response = if let Some(handle) =
                                        cache.texture(ui.ctx(), asset)
                                    {
                                        let image = egui::Image::new(handle).fit_to_exact_size(
                                            egui::Vec2::splat(TILE_PREVIEW_SIZE),
                                        );
                                        ui.add_sized(
                                            egui::Vec2::splat(TILE_SIZE),
                                            egui::Button::image(image).selected(
                                                selected_asset_id == Some(asset.id.as_str()),
                                            ),
                                        )
                                    } else {
                                        ui.add_sized(
                                            egui::Vec2::splat(TILE_SIZE),
                                            egui::Button::new("?"),
                                        )
                                    }
                                    .on_hover_ui(|ui| {
                                        ui.label(asset_name(asset));
                                        ui.monospace(asset.id.as_str());
                                    });
                                    if response.clicked() {
                                        output.selected_asset_id = Some(asset.id.clone());
                                        egui::Popup::close_id(ui.ctx(), popup_id);
                                    }
                                    if (index + 1) % columns == 0 {
                                        ui.end_row();
                                    }
                                }
                            });
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(l10n.text("action-import-ellipsis")).clicked() {
                    output.import_requested = true;
                    egui::Popup::close_id(ui.ctx(), popup_id);
                }
                if ui
                    .button(l10n.text("action-open-asset-library-ellipsis"))
                    .clicked()
                {
                    output.library_requested = true;
                    egui::Popup::close_id(ui.ctx(), popup_id);
                }
            });
        });

    output
}

fn thumbnail_grid_column_count(available_width: f32) -> usize {
    (((available_width + TILE_GAP) / (TILE_SIZE + TILE_GAP)).floor() as usize).max(1)
}

fn asset_preview_image(asset: &ImageAssetDefinition) -> Option<egui::ColorImage> {
    if let Some(representation) = asset.rgba8_representation() {
        let width = representation.size[0] as usize;
        let height = representation.size[1] as usize;
        return Some(egui::ColorImage::from_rgba_unmultiplied(
            [width, height],
            representation.rgba8()?,
        ));
    }

    let representation = asset
        .representations
        .iter()
        .find(|representation| representation.format == ImageRepresentationFormat::R8Unorm)?;
    let width = representation.size[0] as usize;
    let height = representation.size[1] as usize;
    let side = width.max(height);
    let offset_x = (side - width) / 2;
    let offset_y = (side - height) / 2;
    let mut rgba = vec![PREVIEW_BACKGROUND; side * side * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    for y in 0..height {
        for x in 0..width {
            let mask = representation.r8()?[y * width + x] as u16;
            let gray =
                PREVIEW_BACKGROUND as u16 + ((255 - PREVIEW_BACKGROUND as u16) * mask + 127) / 255;
            let dst = ((y + offset_y) * side + x + offset_x) * 4;
            let gray = gray as u8;
            rgba[dst..dst + 4].copy_from_slice(&[gray, gray, gray, 255]);
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [side, side],
        &rgba,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_grid_columns_never_drop_below_one() {
        assert_eq!(thumbnail_grid_column_count(0.0), 1);
        assert_eq!(thumbnail_grid_column_count(TILE_SIZE), 1);
        assert_eq!(thumbnail_grid_column_count(TILE_SIZE * 2.0 + TILE_GAP), 2);
    }
}
