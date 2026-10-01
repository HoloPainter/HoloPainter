use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, ensure};
use eframe::egui;

use crate::{
    application::{Command, create_new_project_command},
    core::{
        document::MaterialSpec,
        material::{MaterialRenderMode, MaterialRenderSettings},
        texture_size::{DEFAULT_TEXTURE_SIZE, TEXTURE_SIZE_CHOICES},
    },
    import::{ImportedAsset, ImportedTexture, ModelFormat, load_base_color_textures, load_model},
    localization::Localization,
};

#[derive(Debug, Clone, PartialEq)]
pub struct NewProjectMaterialSettings {
    pub name: String,
    pub texture_size: [u32; 2],
    pub render_settings: MaterialRenderSettings,
}

pub struct NewProjectDraft {
    source_path: Option<PathBuf>,
    imported: Option<ImportedAsset>,
    pub materials: Vec<NewProjectMaterialSettings>,
    bulk_texture_size: u32,
    import_base_color_textures: bool,
    pub error: Option<String>,
}

impl Default for NewProjectDraft {
    fn default() -> Self {
        Self {
            source_path: None,
            imported: None,
            materials: Vec::new(),
            bulk_texture_size: DEFAULT_TEXTURE_SIZE,
            import_base_color_textures: false,
            error: None,
        }
    }
}

impl NewProjectDraft {
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn source_name_hint(&self) -> Option<String> {
        self.source_path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .map(str::to_owned)
    }

    pub fn load_source(&mut self, path: PathBuf) {
        match load_model(&path) {
            Ok(imported) => {
                let format = ModelFormat::from_path(&path)
                    .expect("a successfully loaded model must have a supported format");
                let materials = imported
                    .materials
                    .iter()
                    .map(|material| NewProjectMaterialSettings {
                        name: material.name.clone(),
                        texture_size: [DEFAULT_TEXTURE_SIZE; 2],
                        render_settings: material.render_settings,
                    })
                    .collect();
                self.source_path = Some(path);
                self.imported = Some(imported);
                self.materials = materials;
                self.bulk_texture_size = DEFAULT_TEXTURE_SIZE;
                self.error = None;
                if !format.supports_base_color_texture_import() {
                    self.import_base_color_textures = false;
                }
                if self.import_base_color_textures {
                    self.apply_base_color_texture_size_defaults();
                }
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    pub fn apply_texture_size_to_all(&mut self, size: u32) {
        self.bulk_texture_size = size;
        for material in &mut self.materials {
            material.texture_size = [size; 2];
        }
    }

    fn set_import_base_color_textures(&mut self, enabled: bool) {
        if enabled && !self.supports_base_color_texture_import() {
            return;
        }
        self.import_base_color_textures = enabled;
        if enabled {
            self.apply_base_color_texture_size_defaults();
        } else if self.error.is_some() {
            self.error = None;
        }
    }

    fn supports_base_color_texture_import(&self) -> bool {
        self.source_path
            .as_deref()
            .and_then(|path| ModelFormat::from_path(path).ok())
            .is_none_or(ModelFormat::supports_base_color_texture_import)
    }

    fn apply_base_color_texture_size_defaults(&mut self) {
        let (Some(source_path), Some(imported)) =
            (self.source_path.as_deref(), self.imported.as_ref())
        else {
            return;
        };
        match load_base_color_textures(source_path, &imported.materials) {
            Ok(textures) => {
                apply_imported_texture_size_defaults(&mut self.materials, &textures);
                self.bulk_texture_size =
                    common_texture_size(&self.materials).unwrap_or(DEFAULT_TEXTURE_SIZE);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    pub fn create_command(&self, max_texture_dimension_2d: Option<u32>) -> Result<Command> {
        let imported = self
            .imported
            .as_ref()
            .ok_or_else(|| anyhow!("select a source mesh first"))?;
        let maximum =
            max_texture_dimension_2d.ok_or_else(|| anyhow!("GPU renderer is unavailable"))?;
        ensure!(
            !self.materials.is_empty(),
            "source mesh does not contain any materials"
        );
        for material in &self.materials {
            ensure!(
                TEXTURE_SIZE_CHOICES.contains(&material.texture_size[0])
                    && material.texture_size[0] == material.texture_size[1],
                "{} has an unsupported texture size {}x{}",
                material.name,
                material.texture_size[0],
                material.texture_size[1]
            );
        }
        let materials = self
            .materials
            .iter()
            .map(|material| {
                MaterialSpec::new(&material.name, material.texture_size)
                    .with_render_settings(material.render_settings)
            })
            .collect();
        let base_color_textures = if self.import_base_color_textures {
            let source_path = self
                .source_path
                .as_deref()
                .ok_or_else(|| anyhow!("select a source mesh first"))?;
            ensure!(
                ModelFormat::from_path(source_path)?.supports_base_color_texture_import(),
                "Base Color Texture import is not supported for this model format"
            );
            Some(load_base_color_textures(source_path, &imported.materials)?)
        } else {
            None
        };
        create_new_project_command(imported, materials, base_color_textures.as_deref(), maximum)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewProjectDialogAction {
    None,
    SelectMesh,
    Create,
    Cancel,
}

pub fn draw_new_project_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    draft: &mut NewProjectDraft,
    max_texture_dimension_2d: Option<u32>,
) -> NewProjectDialogAction {
    let mut action = NewProjectDialogAction::None;
    let response = egui::Modal::new(egui::Id::new("new_project_dialog")).show(ctx, |ui| {
        ui.set_min_width(560.0);
        ui.heading(l10n.text("new-project-title"));
        ui.separator();
        ui.heading(l10n.text("dialog-mesh"));
        ui.horizontal(|ui| {
            let source = draft
                .source_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| l10n.text("dialog-no-source-mesh"));
            ui.add(egui::Label::new(source).truncate());
            let label = if draft.source_path().is_some() {
                l10n.text("action-reselect")
            } else {
                l10n.text("action-select")
            };
            if ui.button(label).clicked() {
                action = NewProjectDialogAction::SelectMesh;
            }
        });
        ui.small(l10n.text("dialog-mesh-supported-formats"));
        let mut import_base_color_textures = draft.import_base_color_textures;
        let supports_texture_import = draft.supports_base_color_texture_import();
        let import_response = ui
            .add_enabled(
                supports_texture_import,
                egui::Checkbox::new(
                    &mut import_base_color_textures,
                    l10n.text("new-project-import-base-color"),
                ),
            )
            .on_hover_text(if supports_texture_import {
                l10n.text("new-project-import-base-color-help")
            } else {
                l10n.text("new-project-import-base-color-unsupported")
            });
        if import_response.changed() {
            draft.set_import_base_color_textures(import_base_color_textures);
        }

        ui.add_space(10.0);
        ui.heading(l10n.text("new-project-material-settings"));
        if draft.materials.is_empty() {
            ui.label(l10n.text("new-project-select-valid-mesh"));
        } else {
            ui.horizontal(|ui| {
                ui.label(l10n.text("new-project-all-texture-sizes"));
                let previous = draft.bulk_texture_size;
                texture_size_combo(ui, "bulk_texture_size", &mut draft.bulk_texture_size);
                if draft.bulk_texture_size != previous {
                    draft.apply_texture_size_to_all(draft.bulk_texture_size);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("new_project_materials")
                .max_height(360.0)
                .show(ui, |ui| {
                    for (index, material) in draft.materials.iter_mut().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.strong(if material.name.trim().is_empty() {
                                l10n.text("material-unnamed")
                            } else {
                                material.name.clone()
                            });
                            ui.horizontal(|ui| {
                                //ui.label(l10n.text("material-texture-size"));
                                texture_size_combo(
                                    ui,
                                    "material_texture_size",
                                    &mut material.texture_size[0],
                                );
                                material.texture_size[1] = material.texture_size[0];
                                render_mode_combo(
                                    ui,
                                    l10n,
                                    &mut material.render_settings.render_mode,
                                );
                                ui.checkbox(
                                    &mut material.render_settings.double_sided,
                                    l10n.text("material-double-sided"),
                                );
                            });

                            ui.separator();
                        });
                    }
                });
        }

        if let Some(error) = &draft.error {
            let mut args = fluent::FluentArgs::new();
            args.set("error", error);
            ui.colored_label(
                ui.visuals().error_fg_color,
                l10n.format("dialog-error-detail", Some(&args)),
            );
        }
        if max_texture_dimension_2d.is_none() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                l10n.text("new-project-gpu-required"),
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(l10n.text("dialog-cancel")).clicked() {
                action = NewProjectDialogAction::Cancel;
            }
            let can_create = draft.imported.is_some()
                && !draft.materials.is_empty()
                && max_texture_dimension_2d.is_some();
            if ui
                .add_enabled(can_create, egui::Button::new(l10n.text("action-create")))
                .clicked()
            {
                action = NewProjectDialogAction::Create;
            }
        });
    });
    if response.should_close() && action == NewProjectDialogAction::None {
        NewProjectDialogAction::Cancel
    } else {
        action
    }
}

fn apply_imported_texture_size_defaults(
    materials: &mut [NewProjectMaterialSettings],
    textures: &[Option<ImportedTexture>],
) {
    for (material, texture) in materials.iter_mut().zip(textures) {
        if let Some(texture) = texture {
            let size = nearest_texture_size(texture.size);
            material.texture_size = [size; 2];
        }
    }
}

fn nearest_texture_size(source_size: [u32; 2]) -> u32 {
    let source_extent = source_size[0].max(source_size[1]);
    TEXTURE_SIZE_CHOICES
        .into_iter()
        .min_by_key(|choice| (source_extent.abs_diff(*choice), Reverse(*choice)))
        .expect("texture size choices must not be empty")
}

fn common_texture_size(materials: &[NewProjectMaterialSettings]) -> Option<u32> {
    let first = materials.first()?.texture_size[0];
    materials
        .iter()
        .all(|material| material.texture_size == [first; 2])
        .then_some(first)
}

pub(crate) fn texture_size_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    size: &mut u32,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{size} x {size}"))
        .show_ui(ui, |ui| {
            for choice in TEXTURE_SIZE_CHOICES {
                ui.selectable_value(size, choice, format!("{choice} x {choice}"));
            }
        });
}

pub(crate) fn render_mode_combo(
    ui: &mut egui::Ui,
    l10n: &Localization,
    mode: &mut MaterialRenderMode,
) {
    egui::ComboBox::from_label(l10n.text("material-rendering-mode"))
        .selected_text(render_mode_label(l10n, *mode))
        .show_ui(ui, |ui| {
            for choice in [
                MaterialRenderMode::Opaque,
                MaterialRenderMode::Cutoff,
                MaterialRenderMode::Blend,
            ] {
                ui.selectable_value(mode, choice, render_mode_label(l10n, choice));
            }
        });
}

fn render_mode_label(l10n: &Localization, mode: MaterialRenderMode) -> String {
    l10n.text(match mode {
        MaterialRenderMode::Opaque => "material-render-mode-opaque",
        MaterialRenderMode::Cutoff => "material-render-mode-cutoff",
        MaterialRenderMode::Blend => "material-render-mode-blend",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_defaults_to_1024_and_bulk_size_updates_both_dimensions() {
        let mut draft = NewProjectDraft::default();
        assert_eq!(draft.bulk_texture_size, DEFAULT_TEXTURE_SIZE);
        assert!(!draft.import_base_color_textures);
        draft.materials = vec![
            NewProjectMaterialSettings {
                name: "A".to_owned(),
                texture_size: [DEFAULT_TEXTURE_SIZE; 2],
                render_settings: MaterialRenderSettings::default(),
            },
            NewProjectMaterialSettings {
                name: "B".to_owned(),
                texture_size: [512; 2],
                render_settings: MaterialRenderSettings::default(),
            },
        ];

        draft.apply_texture_size_to_all(2048);

        assert_eq!(draft.bulk_texture_size, 2048);
        assert!(
            draft
                .materials
                .iter()
                .all(|material| material.texture_size == [2048, 2048])
        );
        draft.materials[1].texture_size = [512, 512];
        assert_eq!(draft.materials[0].texture_size, [2048, 2048]);
        assert_eq!(draft.materials[1].texture_size, [512, 512]);
    }

    #[test]
    fn draft_keeps_only_a_name_hint_instead_of_a_project_dependency() {
        let draft = NewProjectDraft {
            source_path: Some(PathBuf::from("C:/models/character.glb")),
            ..NewProjectDraft::default()
        };
        assert_eq!(draft.source_name_hint().as_deref(), Some("character"));
        assert!(draft.create_command(Some(4096)).is_err());
    }

    #[test]
    fn mesh_reselection_failure_does_not_reset_texture_import_choice() {
        let mut draft = NewProjectDraft {
            import_base_color_textures: true,
            ..NewProjectDraft::default()
        };

        draft.load_source(PathBuf::from("missing.gltf"));

        assert!(draft.import_base_color_textures);
        assert!(draft.error.is_some());
    }

    #[test]
    fn selecting_fbx_preserves_base_color_texture_import_choice() {
        let mut draft = NewProjectDraft {
            import_base_color_textures: true,
            ..NewProjectDraft::default()
        };
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/import/triangle_ascii.fbx");

        draft.load_source(path);

        assert!(draft.imported.is_some());
        assert!(draft.import_base_color_textures);
        assert!(draft.supports_base_color_texture_import());
        assert!(draft.create_command(Some(4096)).is_ok());
    }

    #[test]
    fn nearest_texture_size_uses_source_maximum_extent_and_prefers_larger_ties() {
        assert_eq!(nearest_texture_size([700, 500]), 512);
        assert_eq!(nearest_texture_size([768, 300]), 1024);
        assert_eq!(nearest_texture_size([512, 2048]), 2048);
    }

    #[test]
    fn imported_texture_sizes_initialize_only_materials_with_base_color_textures() {
        let mut materials = vec![
            NewProjectMaterialSettings {
                name: "Body".to_owned(),
                texture_size: [DEFAULT_TEXTURE_SIZE; 2],
                render_settings: MaterialRenderSettings::default(),
            },
            NewProjectMaterialSettings {
                name: "Eyes".to_owned(),
                texture_size: [DEFAULT_TEXTURE_SIZE; 2],
                render_settings: MaterialRenderSettings::default(),
            },
        ];
        let textures = vec![
            Some(ImportedTexture {
                size: [1900, 1000],
                rgba8: Vec::new(),
            }),
            None,
        ];

        apply_imported_texture_size_defaults(&mut materials, &textures);

        assert_eq!(materials[0].texture_size, [2048, 2048]);
        assert_eq!(materials[1].texture_size, [DEFAULT_TEXTURE_SIZE; 2]);
        assert_eq!(common_texture_size(&materials), None);
    }
}
