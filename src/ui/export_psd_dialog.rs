use std::{collections::HashSet, path::PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use eframe::egui;

use crate::{
    core::material::{MaterialData, MaterialId},
    localization::Localization,
};

#[derive(Debug)]
pub struct ExportPsdDraft {
    pub document_generation: u64,
    pub output_dir: Option<PathBuf>,
    pub files: Vec<ExportPsdFileDraft>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub overwrite_confirmation: Option<PsdOverwriteConfirmation>,
}

#[derive(Debug, Clone)]
pub struct ExportPsdFileDraft {
    pub material_id: MaterialId,
    pub material_index: usize,
    pub material_name: String,
    pub file_name: String,
}

#[derive(Debug, Clone)]
pub struct PsdOverwriteConfirmation {
    pub plan: ExportPsdPlan,
    pub existing_file_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExportPsdPlan {
    pub document_generation: u64,
    pub output_dir: PathBuf,
    pub files: Vec<ExportPsdPlanEntry>,
}

#[derive(Debug, Clone)]
pub struct ExportPsdPlanEntry {
    pub material_id: MaterialId,
    pub material_index: usize,
    pub file_name: String,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportPsdDialogAction {
    None,
    Browse,
    Cancel,
    PrepareExport,
    CancelOverwrite,
    OverwriteAll,
}

impl ExportPsdDraft {
    pub fn new(
        materials: &[MaterialData],
        document_generation: u64,
        output_dir: Option<PathBuf>,
        warnings: Vec<String>,
    ) -> Self {
        let preferred = materials
            .iter()
            .map(preferred_file_name)
            .collect::<Vec<_>>();
        let mut reserved = preferred
            .iter()
            .flatten()
            .map(|name| reserved_name_key(name))
            .collect::<HashSet<_>>();
        let files = materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| ExportPsdFileDraft {
                material_id: material.id,
                material_index,
                material_name: material.name.clone(),
                file_name: preferred[material_index]
                    .clone()
                    .unwrap_or_else(|| unique_default_file_name(&material.name, &mut reserved)),
            })
            .collect();
        Self {
            document_generation,
            output_dir,
            files,
            warnings,
            error: None,
            overwrite_confirmation: None,
        }
    }

    pub fn prepare_plan(&mut self) -> Result<ExportPsdPlan> {
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
            if !names.insert(normalized.to_lowercase()) {
                bail!("Duplicate file name: {normalized}");
            }
            draft.file_name.clone_from(&normalized);
            files.push(ExportPsdPlanEntry {
                material_id: draft.material_id,
                material_index: draft.material_index,
                file_name: normalized.clone(),
                output_path: output_dir.join(normalized),
            });
        }
        Ok(ExportPsdPlan {
            document_generation: self.document_generation,
            output_dir,
            files,
        })
    }
}

pub fn draw_export_psd_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    draft: &mut ExportPsdDraft,
) -> ExportPsdDialogAction {
    if let Some(confirmation) = &draft.overwrite_confirmation {
        return draw_overwrite_confirmation(ctx, l10n, confirmation);
    }
    let mut action = ExportPsdDialogAction::None;
    let response = egui::Modal::new(egui::Id::new("export_psd_dialog")).show(ctx, |ui| {
        ui.set_min_width(620.0);
        ui.heading(l10n.text("export-psd-title"));
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
                action = ExportPsdDialogAction::Browse;
            }
        });
        ui.add_space(10.0);
        ui.heading(l10n.text("export-files"));
        egui::ScrollArea::vertical()
            .id_salt("export_psd_files")
            .max_height(300.0)
            .show(ui, |ui| {
                egui::Grid::new("export_psd_file_grid")
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
        if !draft.warnings.is_empty() {
            ui.add_space(8.0);
            let mut args = fluent::FluentArgs::new();
            args.set("count", draft.warnings.len() as i64);
            ui.collapsing(l10n.format("export-warnings", Some(&args)), |ui| {
                for warning in &draft.warnings {
                    ui.label(format!("• {warning}"));
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
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(l10n.text("dialog-cancel")).clicked() {
                action = ExportPsdDialogAction::Cancel;
            }
            if ui
                .add_enabled(
                    draft.output_dir.is_some() && !draft.files.is_empty(),
                    egui::Button::new(l10n.text("action-export")),
                )
                .clicked()
            {
                action = ExportPsdDialogAction::PrepareExport;
            }
        });
    });
    if response.should_close() && action == ExportPsdDialogAction::None {
        ExportPsdDialogAction::Cancel
    } else {
        action
    }
}

fn draw_overwrite_confirmation(
    ctx: &egui::Context,
    l10n: &Localization,
    confirmation: &PsdOverwriteConfirmation,
) -> ExportPsdDialogAction {
    let mut action = ExportPsdDialogAction::None;
    let response =
        egui::Modal::new(egui::Id::new("export_psd_overwrite_confirmation")).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.heading(l10n.text("export-psd-overwrite-title"));
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
                    action = ExportPsdDialogAction::CancelOverwrite;
                }
                if ui.button(l10n.text("export-overwrite-all")).clicked() {
                    action = ExportPsdDialogAction::OverwriteAll;
                }
            });
        });
    if response.should_close() && action == ExportPsdDialogAction::None {
        ExportPsdDialogAction::CancelOverwrite
    } else {
        action
    }
}

fn preferred_file_name(material: &MaterialData) -> Option<String> {
    material
        .export_psd_file_name
        .as_deref()
        .or(material.export_image_file_name.as_deref())
        .map(with_psd_extension)
}

fn unique_default_file_name(material_name: &str, reserved: &mut HashSet<String>) -> String {
    let base = sanitize_default_base_name(material_name);
    let mut suffix = 1;
    loop {
        let candidate = if suffix == 1 {
            format!("{base}.psd")
        } else {
            format!("{base}_{suffix}.psd")
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
    let normalized = with_psd_extension(file_name);
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

fn with_psd_extension(file_name: &str) -> String {
    let mut path = PathBuf::from(file_name);
    path.set_extension("psd");
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name)
        .to_owned()
}

fn reserved_name_key(file_name: &str) -> String {
    with_psd_extension(file_name).to_lowercase()
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

    fn material(id: u64, name: &str, image: Option<&str>, psd: Option<&str>) -> MaterialData {
        MaterialData {
            id: MaterialId(id),
            name: name.to_owned(),
            texture_size: [8, 8],
            ui_color: MaterialUiColor { rgb: [0; 3] },
            render_settings: MaterialRenderSettings::default(),
            export_image_file_name: image.map(str::to_owned),
            export_psd_file_name: psd.map(str::to_owned),
        }
    }

    #[test]
    fn initial_names_use_psd_then_image_then_material_name() {
        let draft = ExportPsdDraft::new(
            &[
                material(1, "First", Some("image.png"), Some("saved.PSD")),
                material(2, "Second", Some("render.jpg"), None),
                material(3, "Body/Cloth", None, None),
            ],
            3,
            None,
            Vec::new(),
        );
        assert_eq!(draft.files[0].file_name, "saved.psd");
        assert_eq!(draft.files[1].file_name, "render.psd");
        assert_eq!(draft.files[2].file_name, "Body_Cloth.psd");
    }

    #[test]
    fn saved_names_reserve_colliding_material_defaults() {
        let draft = ExportPsdDraft::new(
            &[
                material(1, "A", None, Some("Body.psd")),
                material(2, "Body", None, None),
            ],
            3,
            None,
            Vec::new(),
        );
        assert_eq!(draft.files[1].file_name, "Body_2.psd");
    }

    #[test]
    fn plan_normalizes_extension_and_rejects_case_insensitive_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let mut draft = ExportPsdDraft::new(
            &[material(1, "A", None, None), material(2, "B", None, None)],
            3,
            Some(directory.path().to_path_buf()),
            Vec::new(),
        );
        draft.files[0].file_name = "Body.png".to_owned();
        draft.files[1].file_name = "body.PSD".to_owned();
        assert!(
            draft
                .prepare_plan()
                .unwrap_err()
                .to_string()
                .contains("Duplicate")
        );
        assert_eq!(draft.files[0].file_name, "Body.psd");
    }

    #[test]
    fn invalid_names_are_rejected() {
        for name in ["", " ", ".", "..", "a/b", "bad*name", "CON.psd", "name."] {
            assert!(normalize_and_validate_file_name(name).is_err(), "{name:?}");
        }
    }
}
