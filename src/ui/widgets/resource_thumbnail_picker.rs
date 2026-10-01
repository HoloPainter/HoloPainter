use std::collections::HashMap;

use eframe::egui;

use crate::core::{
    brush_engine::ResourceRefValue,
    texture::{TextureCatalog, TextureResourceDefinition},
};
use crate::localization::Localization;
use crate::ui::localized;

const SELECTOR_WIDTH: f32 = 190.0;
const SELECTOR_HEIGHT: f32 = 30.0;
const SELECTOR_PREVIEW_SIZE: f32 = 22.0;
const POPUP_WIDTH: f32 = 320.0;
const POPUP_MAX_HEIGHT: f32 = 300.0;
const TILE_SIZE: f32 = 52.0;
const TILE_PREVIEW_SIZE: f32 = 42.0;
const TILE_GAP: f32 = 6.0;
const PREVIEW_BACKGROUND: u8 = 48;

#[derive(Default)]
pub(crate) struct ResourceThumbnailCache {
    textures: HashMap<String, egui::TextureHandle>,
}

#[derive(Debug, Default)]
pub(crate) struct ResourceThumbnailPickerOutput {
    pub(crate) changed: bool,
    pub(crate) import_requested: bool,
    pub(crate) library_requested: bool,
}

impl std::fmt::Debug for ResourceThumbnailCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceThumbnailCache")
            .field("texture_count", &self.textures.len())
            .finish()
    }
}

impl ResourceThumbnailCache {
    fn texture<'a>(
        &'a mut self,
        ctx: &egui::Context,
        texture: &TextureResourceDefinition,
    ) -> &'a egui::TextureHandle {
        self.textures.entry(texture.id.clone()).or_insert_with(|| {
            ctx.load_texture(
                format!("resource_thumbnail:{}", texture.id),
                mask_preview_image(texture),
                egui::TextureOptions::LINEAR,
            )
        })
    }
}

pub(crate) fn draw_texture_resource_thumbnail_picker(
    ui: &mut egui::Ui,
    l10n: &Localization,
    id: egui::Id,
    value: &mut ResourceRefValue,
    options: &[&TextureResourceDefinition],
    catalog: &TextureCatalog,
    cache: &mut ResourceThumbnailCache,
) -> ResourceThumbnailPickerOutput {
    let mut output = ResourceThumbnailPickerOutput::default();
    let before = value.clone();
    let selected = match value {
        ResourceRefValue::None => None,
        ResourceRefValue::Resource(resource_id) => catalog.get(resource_id),
    };
    let selected_text = match value {
        ResourceRefValue::None => l10n.text("value-none"),
        ResourceRefValue::Resource(resource_id) => selected.map_or_else(
            || {
                let mut args = fluent::FluentArgs::new();
                args.set("id", resource_id.as_str());
                l10n.format("value-missing-id", Some(&args))
            },
            |texture| localized::texture_resource_name(l10n, texture),
        ),
    };

    let selector_width = SELECTOR_WIDTH.min(ui.available_width());
    let response = if let Some(texture) = selected {
        let handle = cache.texture(ui.ctx(), texture);
        let image =
            egui::Image::new(handle).fit_to_exact_size(egui::Vec2::splat(SELECTOR_PREVIEW_SIZE));
        ui.add_sized(
            egui::vec2(selector_width, SELECTOR_HEIGHT),
            egui::Button::image_and_text(image, format!("{selected_text}  ▾")),
        )
    } else {
        ui.add_sized(
            egui::vec2(selector_width, SELECTOR_HEIGHT),
            egui::Button::new(format!("?  {selected_text}  ▾")),
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
                    let columns = thumbnail_grid_column_count(ui.available_width());
                    egui::Grid::new(id.with("grid"))
                        .num_columns(columns)
                        .min_col_width(TILE_SIZE)
                        .spacing([TILE_GAP, TILE_GAP])
                        .show(ui, |ui| {
                            for (index, texture) in options.iter().enumerate() {
                                let handle = cache.texture(ui.ctx(), texture);
                                let image = egui::Image::new(handle)
                                    .fit_to_exact_size(egui::Vec2::splat(TILE_PREVIEW_SIZE));
                                let is_selected = matches!(
                                    &*value,
                                    ResourceRefValue::Resource(resource_id)
                                        if resource_id == &texture.id
                                );
                                let response = ui
                                    .add_sized(
                                        egui::Vec2::splat(TILE_SIZE),
                                        egui::Button::image(image).selected(is_selected),
                                    )
                                    .on_hover_ui(|ui| {
                                        ui.label(localized::texture_resource_name(l10n, texture));
                                        ui.monospace(texture.id.as_str());
                                        ui.label(format!(
                                            "{} × {}",
                                            texture.size[0], texture.size[1]
                                        ));
                                    });
                                if response.clicked() {
                                    *value = ResourceRefValue::Resource(texture.id.clone());
                                    egui::Popup::close_id(ui.ctx(), popup_id);
                                }
                                if (index + 1) % columns == 0 {
                                    ui.end_row();
                                }
                            }
                        });
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

    output.changed = *value != before;
    output
}

fn thumbnail_grid_column_count(available_width: f32) -> usize {
    (((available_width + TILE_GAP) / (TILE_SIZE + TILE_GAP)).floor() as usize).max(1)
}

fn mask_preview_image(texture: &TextureResourceDefinition) -> egui::ColorImage {
    let width = texture.size[0] as usize;
    let height = texture.size[1] as usize;
    let side = width.max(height);
    let offset_x = (side - width) / 2;
    let offset_y = (side - height) / 2;
    let mut rgba = vec![0_u8; side * side * 4];

    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[
            PREVIEW_BACKGROUND,
            PREVIEW_BACKGROUND,
            PREVIEW_BACKGROUND,
            255,
        ]);
    }
    for y in 0..height {
        for x in 0..width {
            let mask = texture.r8[y * width + x] as u16;
            let gray =
                PREVIEW_BACKGROUND as u16 + ((255 - PREVIEW_BACKGROUND as u16) * mask + 127) / 255;
            let dst = ((y + offset_y) * side + x + offset_x) * 4;
            let gray = gray as u8;
            rgba[dst..dst + 4].copy_from_slice(&[gray, gray, gray, 255]);
        }
    }

    egui::ColorImage::from_rgba_unmultiplied([side, side], &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{brush_engine::TextureResourceFormat, image_asset::ImageAssetOrigin};

    #[test]
    fn thumbnail_grid_columns_never_drop_below_one() {
        assert_eq!(thumbnail_grid_column_count(0.0), 1);
        assert_eq!(thumbnail_grid_column_count(TILE_SIZE), 1);
        assert_eq!(thumbnail_grid_column_count(TILE_SIZE * 2.0 + TILE_GAP), 2);
    }

    #[test]
    fn mask_preview_centers_rectangular_texture_on_dark_square() {
        let texture = TextureResourceDefinition {
            id: "texture.test".to_owned(),
            display_name: "Test".to_owned(),
            source_asset_id: "image.test".to_owned(),
            source_asset_origin: ImageAssetOrigin::User,
            format: TextureResourceFormat::R8Unorm,
            size: [2, 1],
            r8: vec![0, 255],
            mipmaps: false,
            tags: Vec::new(),
        };

        let image = mask_preview_image(&texture);

        assert_eq!(image.size, [2, 2]);
        assert_eq!(
            image.pixels[0],
            egui::Color32::from_gray(PREVIEW_BACKGROUND)
        );
        assert_eq!(image.pixels[1], egui::Color32::WHITE);
        assert_eq!(
            image.pixels[2],
            egui::Color32::from_gray(PREVIEW_BACKGROUND)
        );
        assert_eq!(
            image.pixels[3],
            egui::Color32::from_gray(PREVIEW_BACKGROUND)
        );
    }
}
