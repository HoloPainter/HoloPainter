use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, ensure};
use eframe::egui;

use crate::{
    application::{
        Command, MeshReloadMatchConfidence, MeshReloadMaterialBinding, MeshReloadMaterialTarget,
        build_mesh_reload_matches, create_mesh_reload_command,
    },
    core::{
        document::Document,
        material::{MaterialId, MaterialSpec},
        texture_size::{DEFAULT_TEXTURE_SIZE, TEXTURE_SIZE_CHOICES},
    },
    import::{ImportedAsset, load_model},
    localization::Localization,
};

use super::new_project_dialog::{render_mode_combo, texture_size_combo};

#[derive(Debug, Clone, PartialEq)]
pub struct MeshReloadNewMaterialSettings {
    pub name: String,
    pub texture_size: [u32; 2],
    pub render_settings: crate::core::material::MaterialRenderSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshReloadMaterialRow {
    pub imported_material_index: usize,
    pub target: MeshReloadMaterialTarget,
    pub confidence: MeshReloadMatchConfidence,
    pub reason: String,
    pub manually_changed: bool,
    pub new_material: MeshReloadNewMaterialSettings,
}

pub struct MeshReloadDraft {
    source_path: Option<PathBuf>,
    imported: Option<ImportedAsset>,
    pub materials: Vec<MeshReloadMaterialRow>,
    pub error: Option<String>,
}

impl Default for MeshReloadDraft {
    fn default() -> Self {
        Self {
            source_path: None,
            imported: None,
            materials: Vec::new(),
            error: None,
        }
    }
}

impl MeshReloadDraft {
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn load_source(&mut self, document: &Document, path: PathBuf) {
        match load_model(&path) {
            Ok(imported) => self.set_imported_source(document, path, imported),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn set_imported_source(&mut self, document: &Document, path: PathBuf, imported: ImportedAsset) {
        let matches = build_mesh_reload_matches(document, &imported);
        self.materials = matches
            .into_iter()
            .map(|matched| {
                let imported_material = &imported.materials[matched.imported_material_index];
                MeshReloadMaterialRow {
                    imported_material_index: matched.imported_material_index,
                    target: matched.target,
                    confidence: matched.confidence,
                    reason: matched.reason,
                    manually_changed: false,
                    new_material: MeshReloadNewMaterialSettings {
                        name: imported_material.name.clone(),
                        texture_size: [DEFAULT_TEXTURE_SIZE; 2],
                        render_settings: imported_material.render_settings,
                    },
                }
            })
            .collect();
        self.source_path = Some(path);
        self.imported = Some(imported);
        self.error = None;
    }

    pub fn summary_counts(&self) -> MeshReloadSummaryCounts {
        let mut summary = MeshReloadSummaryCounts::default();
        for row in &self.materials {
            match row.target {
                MeshReloadMaterialTarget::CreateNew => summary.new += 1,
                MeshReloadMaterialTarget::Existing(_) if row.manually_changed => {
                    summary.matched += 1;
                }
                MeshReloadMaterialTarget::Existing(_) => match row.confidence {
                    MeshReloadMatchConfidence::High => summary.matched += 1,
                    MeshReloadMatchConfidence::Medium | MeshReloadMatchConfidence::Low => {
                        summary.needs_review += 1;
                    }
                    MeshReloadMatchConfidence::New => summary.needs_review += 1,
                },
            }
        }
        summary
    }

    pub fn unused_project_material_ids(&self, document: &Document) -> Vec<MaterialId> {
        let used = self
            .materials
            .iter()
            .filter_map(|row| match row.target {
                MeshReloadMaterialTarget::Existing(material_id) => Some(material_id),
                MeshReloadMaterialTarget::CreateNew => None,
            })
            .collect::<HashSet<_>>();
        document
            .materials
            .iter()
            .filter_map(|material| (!used.contains(&material.id)).then_some(material.id))
            .collect()
    }

    pub fn create_command(
        &self,
        document: &Document,
        max_texture_dimension_2d: Option<u32>,
    ) -> Result<Command> {
        let imported = self
            .imported
            .as_ref()
            .ok_or_else(|| anyhow!("select a source mesh first"))?;
        let maximum =
            max_texture_dimension_2d.ok_or_else(|| anyhow!("GPU renderer is unavailable"))?;
        ensure!(
            self.materials.len() == imported.materials.len(),
            "mesh reload material mapping is incomplete"
        );

        let bindings = self
            .materials
            .iter()
            .map(|row| {
                let new_material = match row.target {
                    MeshReloadMaterialTarget::Existing(_) => None,
                    MeshReloadMaterialTarget::CreateNew => {
                        ensure!(
                            TEXTURE_SIZE_CHOICES.contains(&row.new_material.texture_size[0])
                                && row.new_material.texture_size[0]
                                    == row.new_material.texture_size[1],
                            "{} has an unsupported texture size {}x{}",
                            row.new_material.name,
                            row.new_material.texture_size[0],
                            row.new_material.texture_size[1]
                        );
                        Some(
                            MaterialSpec::new(
                                &row.new_material.name,
                                row.new_material.texture_size,
                            )
                            .with_render_settings(row.new_material.render_settings),
                        )
                    }
                };
                Ok(MeshReloadMaterialBinding {
                    target: row.target,
                    new_material,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        create_mesh_reload_command(document, imported, &bindings, maximum)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshReloadSummaryCounts {
    pub matched: usize,
    pub needs_review: usize,
    pub new: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshReloadDialogAction {
    None,
    SelectMesh,
    Reload,
    Cancel,
}

pub fn draw_mesh_reload_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    draft: &mut MeshReloadDraft,
    document: &Document,
    max_texture_dimension_2d: Option<u32>,
) -> MeshReloadDialogAction {
    let mut action = MeshReloadDialogAction::None;
    let response = egui::Modal::new(egui::Id::new("mesh_reload_dialog")).show(ctx, |ui| {
        ui.set_min_width(720.0);
        ui.heading(l10n.text("mesh-reload-title"));
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
                action = MeshReloadDialogAction::SelectMesh;
            }
        });
        ui.small(l10n.text("dialog-mesh-supported-formats"));

        if !draft.materials.is_empty() {
            let summary = draft.summary_counts();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (key, count) in [
                    ("mesh-reload-summary-matched", summary.matched),
                    ("mesh-reload-summary-review", summary.needs_review),
                    ("mesh-reload-summary-new", summary.new),
                ] {
                    let mut args = fluent::FluentArgs::new();
                    args.set("count", count as i64);
                    ui.label(l10n.format(key, Some(&args)));
                }
            });

            ui.add_space(10.0);
            ui.heading(l10n.text("mesh-reload-material-mapping"));
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("mesh_reload_material_mapping")
                .max_height(410.0)
                .show(ui, |ui| {
                    for row_index in 0..draft.materials.len() {
                        draw_material_row(ui, l10n, row_index, draft, document);
                    }
                });

            let unused = draft.unused_project_material_ids(document);
            if !unused.is_empty() {
                ui.add_space(8.0);
                ui.strong(l10n.text("mesh-reload-unused-materials"));
                ui.small(l10n.text("mesh-reload-unused-materials-help"));
                let labels = unused
                    .iter()
                    .filter_map(|&material_id| project_material_label(l10n, document, material_id))
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(labels);
            }
        } else {
            ui.add_space(10.0);
            ui.label(l10n.text("mesh-reload-select-valid-mesh"));
        }

        if let Some(error) = &draft.error {
            ui.add_space(8.0);
            let mut args = fluent::FluentArgs::new();
            args.set("error", error);
            ui.colored_label(
                ui.visuals().error_fg_color,
                l10n.format("dialog-error-detail", Some(&args)),
            );
        }
        if max_texture_dimension_2d.is_none() {
            ui.add_space(8.0);
            ui.colored_label(
                ui.visuals().error_fg_color,
                l10n.text("mesh-reload-gpu-required"),
            );
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(l10n.text("dialog-cancel")).clicked() {
                action = MeshReloadDialogAction::Cancel;
            }
            let can_reload = draft.imported.is_some()
                && !draft.materials.is_empty()
                && max_texture_dimension_2d.is_some();
            if ui
                .add_enabled(can_reload, egui::Button::new(l10n.text("action-reload")))
                .clicked()
            {
                action = MeshReloadDialogAction::Reload;
            }
        });
    });

    if response.should_close() && action == MeshReloadDialogAction::None {
        MeshReloadDialogAction::Cancel
    } else {
        action
    }
}

fn draw_material_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    row_index: usize,
    draft: &mut MeshReloadDraft,
    document: &Document,
) {
    let imported_material_index = draft.materials[row_index].imported_material_index;
    let imported_name = draft
        .imported
        .as_ref()
        .and_then(|imported| imported.materials.get(imported_material_index))
        .map(|material| material.name.as_str())
        .unwrap_or("");
    let row = &mut draft.materials[row_index];

    ui.push_id(row_index, |ui| {
        egui::Grid::new("mesh_reload_material_row")
            .num_columns(4)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                ui.strong(if imported_name.trim().is_empty() {
                    l10n.text("material-unnamed")
                } else {
                    imported_name.to_owned()
                });

                let previous_target = row.target;
                egui::ComboBox::from_id_salt("project_material")
                    .width(230.0)
                    .selected_text(target_label(l10n, document, row.target))
                    .show_ui(ui, |ui| {
                        for material in &document.materials {
                            ui.selectable_value(
                                &mut row.target,
                                MeshReloadMaterialTarget::Existing(material.id),
                                material_label(l10n, material.name.as_str(), material.id),
                            );
                        }
                        ui.separator();
                        ui.selectable_value(
                            &mut row.target,
                            MeshReloadMaterialTarget::CreateNew,
                            l10n.text("mesh-reload-create-material"),
                        );
                    });
                if row.target != previous_target {
                    row.manually_changed = true;
                    row.reason = "Manual mapping".to_owned();
                }

                ui.label(if row.manually_changed {
                    l10n.text("mesh-reload-confidence-manual")
                } else {
                    confidence_label(l10n, row.confidence)
                });
                ui.small(match_reason_label(l10n, &row.reason));
                ui.end_row();
            });

        if matches!(row.target, MeshReloadMaterialTarget::CreateNew) {
            ui.indent("mesh_reload_new_material_settings", |ui| {
                ui.horizontal(|ui| {
                    ui.label(l10n.text("material-name"));
                    ui.text_edit_singleline(&mut row.new_material.name);
                    //ui.label(l10n.text("material-texture-size"));
                    texture_size_combo(
                        ui,
                        "mesh_reload_texture_size",
                        &mut row.new_material.texture_size[0],
                    );
                    row.new_material.texture_size[1] = row.new_material.texture_size[0];
                });
                ui.horizontal(|ui| {
                    render_mode_combo(ui, l10n, &mut row.new_material.render_settings.render_mode);
                    ui.checkbox(
                        &mut row.new_material.render_settings.double_sided,
                        l10n.text("material-double-sided"),
                    );
                });
            });
        }
        ui.separator();
    });
}

fn target_label(
    l10n: &Localization,
    document: &Document,
    target: MeshReloadMaterialTarget,
) -> String {
    match target {
        MeshReloadMaterialTarget::Existing(material_id) => {
            project_material_label(l10n, document, material_id).unwrap_or_else(|| {
                let mut args = fluent::FluentArgs::new();
                args.set("id", material_id.0 as i64);
                l10n.format("mesh-reload-missing-material", Some(&args))
            })
        }
        MeshReloadMaterialTarget::CreateNew => l10n.text("mesh-reload-create-material"),
    }
}

fn project_material_label(
    l10n: &Localization,
    document: &Document,
    material_id: MaterialId,
) -> Option<String> {
    document
        .materials
        .iter()
        .find(|material| material.id == material_id)
        .map(|material| material_label(l10n, &material.name, material.id))
}

fn material_label(l10n: &Localization, name: &str, material_id: MaterialId) -> String {
    let name = if name.trim().is_empty() {
        l10n.text("material-unnamed")
    } else {
        name.to_owned()
    };
    format!("{name} (ID {})", material_id.0)
}

fn confidence_label(l10n: &Localization, confidence: MeshReloadMatchConfidence) -> String {
    l10n.text(match confidence {
        MeshReloadMatchConfidence::High => "mesh-reload-confidence-high",
        MeshReloadMatchConfidence::Medium => "mesh-reload-confidence-medium",
        MeshReloadMatchConfidence::Low => "mesh-reload-confidence-low",
        MeshReloadMatchConfidence::New => "mesh-reload-confidence-new",
    })
}

fn match_reason_label(l10n: &Localization, reason: &str) -> String {
    match reason {
        "Manual mapping" => l10n.text("mesh-reload-reason-manual"),
        "Normalized name" => l10n.text("mesh-reload-reason-normalized-name"),
        "No reliable existing material match" => l10n.text("mesh-reload-reason-no-match"),
        "Heuristic match" => l10n.text("mesh-reload-reason-heuristic"),
        _ => reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{document::MeshData, material::MaterialRenderSettings},
        import::ImportedMaterial,
    };

    fn document() -> Document {
        Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("Body", [1024, 1024]),
                MaterialSpec::new("Eyes", [1024, 1024]),
            ],
        )
    }

    fn imported(names: &[&str]) -> ImportedAsset {
        ImportedAsset {
            mesh: MeshData::empty(),
            materials: names
                .iter()
                .map(|name| ImportedMaterial {
                    source_material_index: None,
                    name: (*name).to_owned(),
                    render_settings: MaterialRenderSettings::default(),
                })
                .collect(),
        }
    }

    #[test]
    fn draft_uses_heuristic_matches_and_tracks_unused_materials() {
        let document = document();
        let mut draft = MeshReloadDraft::default();
        draft.set_imported_source(
            &document,
            PathBuf::from("character.glb"),
            imported(&["Body", "Metal"]),
        );

        let summary = draft.summary_counts();
        assert_eq!(summary.matched, 1);
        assert_eq!(summary.new, 1);
        assert_eq!(
            draft.unused_project_material_ids(&document),
            vec![document.materials[1].id]
        );
    }

    #[test]
    fn manual_many_to_one_mapping_is_preserved_in_reload_command() {
        let document = document();
        let mut draft = MeshReloadDraft::default();
        draft.set_imported_source(
            &document,
            PathBuf::from("character.glb"),
            imported(&["Body Upper", "Body Lower"]),
        );
        let body_id = document.materials[0].id;
        for row in &mut draft.materials {
            row.target = MeshReloadMaterialTarget::Existing(body_id);
            row.manually_changed = true;
        }

        let Command::ReloadMesh { request } = draft.create_command(&document, Some(4096)).unwrap()
        else {
            panic!("expected reload command");
        };
        assert!(request.new_materials.is_empty());
    }

    #[test]
    fn create_new_material_uses_import_render_settings_and_default_texture_size() {
        let document = document();
        let mut asset = imported(&["Metal"]);
        asset.materials[0].render_settings.double_sided = false;
        let expected_render_settings = asset.materials[0].render_settings;
        let mut draft = MeshReloadDraft::default();
        draft.set_imported_source(&document, PathBuf::from("metal.glb"), asset);
        draft.materials[0].target = MeshReloadMaterialTarget::CreateNew;

        let Command::ReloadMesh { request } = draft.create_command(&document, Some(4096)).unwrap()
        else {
            panic!("expected reload command");
        };
        assert_eq!(request.new_materials.len(), 1);
        assert_eq!(
            request.new_materials[0].texture_size,
            [DEFAULT_TEXTURE_SIZE; 2]
        );
        assert_eq!(
            request.new_materials[0].render_settings,
            expected_render_settings
        );
    }
}
