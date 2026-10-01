use std::{collections::HashSet, path::PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use eframe::egui;

use crate::{
    core::material::{MaterialData, MaterialId},
    localization::Localization,
};

#[derive(Debug)]
pub struct ExportImageDraft {
    pub document_generation: u64,
    pub output_dir: Option<PathBuf>,
    pub files: Vec<ExportImageFileDraft>,
    pub error: Option<String>,
    pub overwrite_confirmation: Option<OverwriteConfirmation>,
}

#[derive(Debug, Clone)]
pub struct ExportImageFileDraft {
    pub material_id: MaterialId,
    pub material_index: usize,
    pub material_name: String,
    pub file_name: String,
}

#[derive(Debug, Clone)]
pub struct OverwriteConfirmation {
    pub plan: ExportImagePlan,
    pub existing_file_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExportImagePlan {
    pub document_generation: u64,
    pub output_dir: PathBuf,
    pub files: Vec<ExportImagePlanEntry>,
}

#[derive(Debug, Clone)]
pub struct ExportImagePlanEntry {
    pub material_id: MaterialId,
    pub material_index: usize,
    pub file_name: String,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportImageDialogAction {
    None,
    Browse,
    Cancel,
    PrepareExport,
    CancelOverwrite,
    OverwriteAll,
}

impl ExportImageDraft {
    pub fn new(
        materials: &[MaterialData],
        document_generation: u64,
        output_dir: Option<PathBuf>,
    ) -> Self {
        let mut reserved = materials
            .iter()
            .filter_map(|material| material.export_image_file_name.as_deref())
            .map(reserved_name_key)
            .collect::<HashSet<_>>();
        let files = materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| {
                let file_name = material
                    .export_image_file_name
                    .clone()
                    .unwrap_or_else(|| unique_default_file_name(&material.name, &mut reserved));
                ExportImageFileDraft {
                    material_id: material.id,
                    material_index,
                    material_name: material.name.clone(),
                    file_name,
                }
            })
            .collect();
        Self {
            document_generation,
            output_dir,
            files,
            error: None,
            overwrite_confirmation: None,
        }
    }

    pub fn prepare_plan(&mut self) -> Result<ExportImagePlan> {
        let output_dir = self
            .output_dir
            .clone()
            .ok_or_else(|| anyhow!("Select an output folder."))?;
        ensure!(output_dir.exists(), "Output folder does not exist.");
        ensure!(output_dir.is_dir(), "Output folder is not a directory.");
        ensure!(!self.files.is_empty(), "There are no materials to export.");

        let mut names = HashSet::with_capacity(self.files.len());
        let mut files = Vec::with_capacity(self.files.len());
        for draft in &mut self.files {
            let normalized = normalize_and_validate_file_name(&draft.file_name)?;
            let key = normalized.to_lowercase();
            if !names.insert(key) {
                bail!("Duplicate file name: {normalized}");
            }
            draft.file_name.clone_from(&normalized);
            files.push(ExportImagePlanEntry {
                material_id: draft.material_id,
                material_index: draft.material_index,
                output_path: output_dir.join(&normalized),
                file_name: normalized,
            });
        }
        Ok(ExportImagePlan {
            document_generation: self.document_generation,
            output_dir,
            files,
        })
    }
}

pub fn draw_export_image_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    draft: &mut ExportImageDraft,
) -> ExportImageDialogAction {
    if draft.overwrite_confirmation.is_some() {
        return draw_overwrite_confirmation(
            ctx,
            l10n,
            draft
                .overwrite_confirmation
                .as_ref()
                .expect("overwrite confirmation was checked"),
        );
    }
    let mut action = ExportImageDialogAction::None;
    let response = egui::Modal::new(egui::Id::new("export_image_dialog")).show(ctx, |ui| {
        ui.set_min_width(620.0);
        ui.heading(l10n.text("export-image-title"));
        ui.separator();
        ui.label(l10n.text("export-output-folder"));
        ui.horizontal(|ui| {
            let folder = draft
                .output_dir
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| l10n.text("export-no-output-folder"));
            ui.add_sized([480.0, 20.0], egui::Label::new(folder).truncate());
            if ui.button(l10n.text("action-browse")).clicked() {
                action = ExportImageDialogAction::Browse;
            }
        });
        ui.add_space(10.0);
        ui.heading(l10n.text("export-files"));
        egui::ScrollArea::vertical()
            .id_salt("export_image_files")
            .max_height(360.0)
            .show(ui, |ui| {
                egui::Grid::new("export_image_file_grid")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong(l10n.text("material-title"));
                        ui.strong(l10n.text("export-file-name"));
                        ui.end_row();
                        for file in &mut draft.files {
                            ui.add_sized(
                                [220.0, 20.0],
                                egui::Label::new(&file.material_name).truncate(),
                            );
                            ui.add_sized(
                                [340.0, 24.0],
                                egui::TextEdit::singleline(&mut file.file_name),
                            );
                            ui.end_row();
                        }
                    });
            });
        if let Some(error) = &draft.error {
            let mut args = fluent::FluentArgs::new();
            args.set("error", error);
            ui.colored_label(
                ui.visuals().error_fg_color,
                l10n.format("dialog-error-detail", Some(&args)),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(l10n.text("dialog-cancel")).clicked() {
                action = ExportImageDialogAction::Cancel;
            }
            if ui
                .add_enabled(
                    draft.output_dir.is_some() && !draft.files.is_empty(),
                    egui::Button::new(l10n.text("action-export")),
                )
                .clicked()
            {
                action = ExportImageDialogAction::PrepareExport;
            }
        });
    });
    if response.should_close() && action == ExportImageDialogAction::None {
        ExportImageDialogAction::Cancel
    } else {
        action
    }
}

fn draw_overwrite_confirmation(
    ctx: &egui::Context,
    l10n: &Localization,
    confirmation: &OverwriteConfirmation,
) -> ExportImageDialogAction {
    let mut action = ExportImageDialogAction::None;
    let response =
        egui::Modal::new(egui::Id::new("export_image_overwrite_confirmation")).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.heading(l10n.text("export-image-overwrite-title"));
            let mut args = fluent::FluentArgs::new();
            args.set("count", confirmation.existing_file_names.len() as i64);
            ui.label(l10n.format("export-existing-files", Some(&args)));
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for file_name in &confirmation.existing_file_names {
                        ui.label(file_name);
                    }
                });
            ui.horizontal(|ui| {
                if ui.button(l10n.text("dialog-cancel")).clicked() {
                    action = ExportImageDialogAction::CancelOverwrite;
                }
                if ui.button(l10n.text("export-overwrite-all")).clicked() {
                    action = ExportImageDialogAction::OverwriteAll;
                }
            });
        });
    if response.should_close() && action == ExportImageDialogAction::None {
        ExportImageDialogAction::CancelOverwrite
    } else {
        action
    }
}

fn unique_default_file_name(material_name: &str, reserved: &mut HashSet<String>) -> String {
    let base = sanitize_default_base_name(material_name);
    let mut suffix = 1;
    loop {
        let candidate = if suffix == 1 {
            format!("{base}.png")
        } else {
            format!("{base}_{suffix}.png")
        };
        if reserved.insert(candidate.to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn sanitize_default_base_name(material_name: &str) -> String {
    let mut base = material_name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_control() || is_invalid_windows_file_character(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while base.ends_with([' ', '.']) {
        base.pop();
    }
    if base.is_empty() {
        base.push_str("Material");
    }
    if is_windows_reserved_base_name(&base) {
        base.push('_');
    }
    base
}

fn normalize_and_validate_file_name(file_name: &str) -> Result<String> {
    if file_name.is_empty() || file_name.trim().is_empty() {
        bail!("File name must not be empty.");
    }
    if file_name == "." || file_name == ".." {
        bail!("Invalid file name: {file_name}");
    }
    if file_name.ends_with([' ', '.']) {
        bail!("Invalid trailing space or period: {file_name}");
    }
    if file_name.chars().any(|character| {
        character.is_ascii_control() || is_invalid_windows_file_character(character)
    }) {
        bail!("Invalid file name: {file_name}");
    }

    let mut normalized = PathBuf::from(file_name);
    normalized.set_extension("png");
    let normalized = normalized
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("Invalid file name: {file_name}"))?
        .to_owned();
    let base = PathBuf::from(&normalized)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    if base.is_empty() || is_windows_reserved_base_name(&base) {
        bail!("Invalid file name: {file_name}");
    }
    Ok(normalized)
}

fn reserved_name_key(file_name: &str) -> String {
    let mut path = PathBuf::from(file_name);
    path.set_extension("png");
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name)
        .to_lowercase()
}

fn is_invalid_windows_file_character(character: char) -> bool {
    matches!(
        character,
        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
    )
}

fn is_windows_reserved_base_name(base: &str) -> bool {
    let upper = base.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::material::{MaterialRenderSettings, MaterialUiColor};

    fn material(id: u64, name: &str, saved: Option<&str>) -> MaterialData {
        MaterialData {
            id: MaterialId(id),
            name: name.to_owned(),
            texture_size: [8, 8],
            ui_color: MaterialUiColor { rgb: [0; 3] },
            render_settings: MaterialRenderSettings::default(),
            export_image_file_name: saved.map(str::to_owned),
            export_psd_file_name: None,
        }
    }

    #[test]
    fn defaults_are_sanitized_and_deduplicated_case_insensitively() {
        let draft = ExportImageDraft::new(
            &[
                material(1, "Body/Cloth", None),
                material(2, "body:cloth", None),
                material(3, "CON", None),
                material(4, " . ", None),
            ],
            1,
            None,
        );
        assert_eq!(draft.files[0].file_name, "Body_Cloth.png");
        assert_eq!(draft.files[1].file_name, "body_cloth_2.png");
        assert_eq!(draft.files[2].file_name, "CON_.png");
        assert_eq!(draft.files[3].file_name, "Material.png");
    }

    #[test]
    fn saved_names_win_collisions_with_new_defaults() {
        let draft = ExportImageDraft::new(
            &[
                material(1, "A", Some("Body.png")),
                material(2, "Body", None),
            ],
            1,
            None,
        );
        assert_eq!(draft.files[0].file_name, "Body.png");
        assert_eq!(draft.files[1].file_name, "Body_2.png");
    }

    #[test]
    fn custom_names_are_normalized_to_png() {
        assert_eq!(normalize_and_validate_file_name("foo").unwrap(), "foo.png");
        assert_eq!(
            normalize_and_validate_file_name("foo.jpg").unwrap(),
            "foo.png"
        );
        assert_eq!(
            normalize_and_validate_file_name("foo.PNG").unwrap(),
            "foo.png"
        );
    }

    #[test]
    fn plan_rejects_duplicates_after_png_normalization() {
        let directory = tempfile::tempdir().unwrap();
        let mut draft = ExportImageDraft::new(
            &[material(1, "A", None), material(2, "B", None)],
            7,
            Some(directory.path().to_path_buf()),
        );
        draft.files[0].file_name = "Body.jpg".to_owned();
        draft.files[1].file_name = "body.PNG".to_owned();

        let error = draft.prepare_plan().unwrap_err();

        assert!(error.to_string().contains("Duplicate file name"));
    }

    #[test]
    fn invalid_names_are_rejected() {
        for name in [
            "", " ", ".", "..", "a/b", "a\\b", "bad*name", "CON.png", "name.", "name ",
        ] {
            assert!(normalize_and_validate_file_name(name).is_err(), "{name:?}");
        }
    }
}
