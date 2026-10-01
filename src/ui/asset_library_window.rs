use eframe::egui;

use crate::{
    core::image_asset::{
        IMAGE_ASSET_BRUSH_TIP_TAG, IMAGE_ASSET_DECAL_TAG, ImageAssetCatalog, ImageAssetOrigin,
    },
    localization::Localization,
    ui::widgets::image_asset_picker::ImageAssetThumbnailCache,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AssetFilter {
    #[default]
    All,
    BrushTips,
    Decals,
}

#[derive(Debug, Default)]
pub(crate) struct AssetLibraryUiState {
    filter: AssetFilter,
    search: String,
    selected_tag: Option<String>,
    selected_asset_id: Option<String>,
    edit_asset_id: Option<String>,
    rename_text: String,
    tags_text: String,
    thumbnails: ImageAssetThumbnailCache,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssetLibraryAction {
    ImportImage,
    Rename {
        asset_id: String,
        display_name: String,
    },
    SetTags {
        asset_id: String,
        tags: Vec<String>,
    },
    SetDecalEnabled {
        asset_id: String,
        enabled: bool,
    },
    SetBrushTipEnabled {
        asset_id: String,
        enabled: bool,
    },
    Delete {
        asset_id: String,
    },
}

pub(crate) fn draw_asset_library_window(
    ctx: &egui::Context,
    l10n: &Localization,
    open: &mut bool,
    catalog: &ImageAssetCatalog,
    state: &mut AssetLibraryUiState,
) -> Vec<AssetLibraryAction> {
    let mut actions = Vec::new();
    if state
        .selected_asset_id
        .as_deref()
        .is_some_and(|asset_id| catalog.get(asset_id).is_none())
    {
        state.selected_asset_id = None;
        state.edit_asset_id = None;
    }
    egui::Window::new(l10n.text("asset-library-title"))
        .open(open)
        .default_size([760.0, 520.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(l10n.text("asset-library-search"));
                ui.add(egui::TextEdit::singleline(&mut state.search).desired_width(220.0));
                if ui.button(l10n.text("action-import-plus")).clicked() {
                    actions.push(AssetLibraryAction::ImportImage);
                }
            });
            ui.separator();

            egui::Panel::left("asset_library_filters")
                .resizable(true)
                .default_size(108.0)
                .size_range(84.0..=180.0)
                .show(ui, |ui| {
                    ui.selectable_value(
                        &mut state.filter,
                        AssetFilter::All,
                        l10n.text("asset-library-filter-all"),
                    );
                    ui.selectable_value(
                        &mut state.filter,
                        AssetFilter::BrushTips,
                        l10n.text("asset-library-filter-brush-tips"),
                    );
                    ui.selectable_value(
                        &mut state.filter,
                        AssetFilter::Decals,
                        l10n.text("asset-library-filter-decals"),
                    );
                    ui.separator();
                    ui.label(l10n.text("asset-library-tags"));
                    if ui
                        .selectable_label(
                            state.selected_tag.is_none(),
                            l10n.text("asset-library-all-tags"),
                        )
                        .clicked()
                    {
                        state.selected_tag = None;
                    }
                    let mut tags = catalog
                        .assets()
                        .flat_map(|asset| asset.tags.iter().cloned())
                        .collect::<Vec<_>>();
                    tags.sort();
                    tags.dedup();
                    egui::ScrollArea::vertical()
                        .id_salt("asset_library_tag_scroll")
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for tag in tags {
                                let selected = state.selected_tag.as_deref() == Some(tag.as_str());
                                if ui.selectable_label(selected, tag.as_str()).clicked() {
                                    state.selected_tag = Some(tag);
                                }
                            }
                        });
                });

            egui::Panel::right("asset_library_inspector")
                .resizable(true)
                .default_size(200.0)
                .size_range(160.0..=320.0)
                .show(ui, |ui| {
                    if let Some(asset_id) = state.selected_asset_id.clone() {
                        if let Some(asset) = catalog.get(&asset_id) {
                            if state.edit_asset_id.as_deref() != Some(asset_id.as_str()) {
                                state.edit_asset_id = Some(asset_id.clone());
                                state.rename_text = asset.display_name.clone();
                                state.tags_text = asset.tags.join(", ");
                            }
                            let origin = catalog.origin(&asset_id);
                            let editable = origin == Some(ImageAssetOrigin::User);
                            let display_name = if origin == Some(ImageAssetOrigin::Builtin) {
                                l10n.builtin_name("image", &asset.id, &asset.display_name)
                            } else {
                                asset.display_name.clone()
                            };
                            ui.heading(display_name);
                            ui.small(match origin {
                                Some(ImageAssetOrigin::Builtin) => {
                                    l10n.text("resource-origin-builtin")
                                }
                                Some(ImageAssetOrigin::UserOverride) => {
                                    l10n.text("resource-origin-user-override")
                                }
                                Some(ImageAssetOrigin::User) => l10n.text("resource-origin-user"),
                                None => l10n.text("resource-origin-unknown"),
                            });
                            ui.monospace(asset.id.as_str());
                            ui.separator();

                            ui.label(l10n.text("asset-library-name"));
                            ui.add_enabled(
                                editable,
                                egui::TextEdit::singleline(&mut state.rename_text),
                            );
                            if ui
                                .add_enabled(
                                    editable,
                                    egui::Button::new(l10n.text("asset-library-apply-name")),
                                )
                                .clicked()
                            {
                                actions.push(AssetLibraryAction::Rename {
                                    asset_id: asset_id.clone(),
                                    display_name: state.rename_text.clone(),
                                });
                            }

                            ui.add_space(8.0);
                            ui.label(l10n.text("asset-library-tags-comma-separated"));
                            ui.add_enabled(
                                editable,
                                egui::TextEdit::singleline(&mut state.tags_text),
                            );
                            if ui
                                .add_enabled(
                                    editable,
                                    egui::Button::new(l10n.text("asset-library-apply-tags")),
                                )
                                .clicked()
                            {
                                let tags = state
                                    .tags_text
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|tag| !tag.is_empty())
                                    .map(str::to_owned)
                                    .collect();
                                actions.push(AssetLibraryAction::SetTags {
                                    asset_id: asset_id.clone(),
                                    tags,
                                });
                            }

                            ui.separator();
                            ui.label(l10n.text("asset-library-usable-as"));
                            if let Some(rgba) = asset.rgba8_representation() {
                                let mut decal =
                                    rgba.tags.iter().any(|tag| tag == IMAGE_ASSET_DECAL_TAG);
                                if ui
                                    .add_enabled(
                                        editable,
                                        egui::Checkbox::new(
                                            &mut decal,
                                            l10n.text("asset-library-decal"),
                                        ),
                                    )
                                    .changed()
                                {
                                    actions.push(AssetLibraryAction::SetDecalEnabled {
                                        asset_id: asset_id.clone(),
                                        enabled: decal,
                                    });
                                }
                            } else {
                                let mut decal = false;
                                ui.add_enabled(
                                    false,
                                    egui::Checkbox::new(
                                        &mut decal,
                                        l10n.text("asset-library-decal"),
                                    ),
                                );
                            }

                            let mut brush_tip = asset
                                .representation_with_tag(IMAGE_ASSET_BRUSH_TIP_TAG)
                                .is_some();
                            if ui
                                .add_enabled(
                                    editable && asset.rgba8_representation().is_some(),
                                    egui::Checkbox::new(
                                        &mut brush_tip,
                                        l10n.text("asset-library-brush-tip"),
                                    ),
                                )
                                .changed()
                            {
                                actions.push(AssetLibraryAction::SetBrushTipEnabled {
                                    asset_id: asset_id.clone(),
                                    enabled: brush_tip,
                                });
                            }

                            ui.separator();
                            if ui
                                .add_enabled(
                                    editable,
                                    egui::Button::new(l10n.text("asset-library-delete")),
                                )
                                .clicked()
                            {
                                actions.push(AssetLibraryAction::Delete {
                                    asset_id: asset_id.clone(),
                                });
                            }
                        }
                    } else {
                        ui.label(l10n.text("asset-library-select-help"));
                    }
                });

            egui::CentralPanel::default().show(ui, |ui| {
                let search = state.search.trim().to_ascii_lowercase();
                let selected_tag = state.selected_tag.as_deref();
                let mut assets = catalog
                    .assets()
                    .filter(|asset| match state.filter {
                        AssetFilter::All => true,
                        AssetFilter::BrushTips => asset
                            .representation_with_tag(IMAGE_ASSET_BRUSH_TIP_TAG)
                            .is_some(),
                        AssetFilter::Decals => {
                            asset.rgba8_representation().is_some_and(|representation| {
                                representation
                                    .tags
                                    .iter()
                                    .any(|tag| tag == IMAGE_ASSET_DECAL_TAG)
                            })
                        }
                    })
                    .filter(|asset| {
                        selected_tag.map_or(true, |tag| {
                            asset.tags.iter().any(|asset_tag| asset_tag == tag)
                        })
                    })
                    .filter(|asset| {
                        search.is_empty()
                            || asset.display_name.to_ascii_lowercase().contains(&search)
                            || asset
                                .tags
                                .iter()
                                .any(|tag| tag.to_ascii_lowercase().contains(&search))
                    })
                    .collect::<Vec<_>>();
                assets.sort_by(|left, right| {
                    left.display_name
                        .cmp(&right.display_name)
                        .then_with(|| left.id.cmp(&right.id))
                });

                egui::ScrollArea::vertical()
                    .id_salt("asset_library_grid_scroll")
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for asset in assets {
                                ui.vertical(|ui| {
                                    let selected = state.selected_asset_id.as_deref()
                                        == Some(asset.id.as_str());
                                    let response = if let Some(texture) =
                                        state.thumbnails.texture(ui.ctx(), asset)
                                    {
                                        let image = egui::Image::new(texture)
                                            .fit_to_exact_size(egui::vec2(72.0, 72.0));
                                        ui.add(egui::Button::image(image).selected(selected))
                                    } else {
                                        ui.add_sized([72.0, 72.0], egui::Button::new("?"))
                                    };
                                    if response.clicked() {
                                        state.selected_asset_id = Some(asset.id.clone());
                                    }
                                    ui.add_sized(
                                        [88.0, 18.0],
                                        egui::Label::new(
                                            egui::RichText::new(
                                                if catalog.origin(&asset.id)
                                                    == Some(ImageAssetOrigin::Builtin)
                                                {
                                                    l10n.builtin_name(
                                                        "image",
                                                        &asset.id,
                                                        &asset.display_name,
                                                    )
                                                } else {
                                                    asset.display_name.clone()
                                                },
                                            )
                                            .small(),
                                        )
                                        .truncate(),
                                    );
                                });
                            }
                        });
                    });
            });
        });
    actions
}
