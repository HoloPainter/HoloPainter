use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::persistence::{load_ron, save_ron_atomic};

pub const PROJECT_HISTORY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHistoryFileV1 {
    pub schema_version: u32,
    pub entries: Vec<ProjectHistoryEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHistoryEntryV1 {
    pub path: PathBuf,
    pub export_image_directory: Option<PathBuf>,
    pub export_psd_directory: Option<PathBuf>,
}

impl Default for ProjectHistoryFileV1 {
    fn default() -> Self {
        Self {
            schema_version: PROJECT_HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl ProjectHistoryFileV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PROJECT_HISTORY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported project history schema version {}",
                self.schema_version
            ));
        }
        let mut paths = HashSet::new();
        for entry in &self.entries {
            if !entry.path.is_absolute() {
                return Err(format!(
                    "project history path must be absolute: {}",
                    entry.path.display()
                ));
            }
            if !paths.insert(&entry.path) {
                return Err(format!(
                    "project history contains duplicate path: {}",
                    entry.path.display()
                ));
            }
            for directory in [
                entry.export_image_directory.as_deref(),
                entry.export_psd_directory.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if !directory.is_absolute() {
                    return Err(format!(
                        "project export directory must be absolute: {}",
                        directory.display()
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn entry(&self, path: &Path) -> Option<&ProjectHistoryEntryV1> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    pub fn touch(&mut self, path: PathBuf) -> Result<(), String> {
        ensure_absolute(&path, "project path")?;
        let existing = self
            .entries
            .iter()
            .position(|entry| entry.path == path)
            .map(|index| self.entries.remove(index));
        self.entries.insert(
            0,
            existing.unwrap_or(ProjectHistoryEntryV1 {
                path,
                export_image_directory: None,
                export_psd_directory: None,
            }),
        );
        Ok(())
    }

    pub fn set_export_image_directory(
        &mut self,
        path: PathBuf,
        directory: PathBuf,
    ) -> Result<(), String> {
        ensure_absolute(&directory, "image export directory")?;
        self.touch(path.clone())?;
        self.entries[0].export_image_directory = Some(directory);
        Ok(())
    }

    pub fn set_export_psd_directory(
        &mut self,
        path: PathBuf,
        directory: PathBuf,
    ) -> Result<(), String> {
        ensure_absolute(&directory, "PSD export directory")?;
        self.touch(path.clone())?;
        self.entries[0].export_psd_directory = Some(directory);
        Ok(())
    }
}

pub(crate) fn load_project_history(path: Option<&Path>) -> ProjectHistoryFileV1 {
    let Some(path) = path else {
        return ProjectHistoryFileV1::default();
    };
    match load_ron::<ProjectHistoryFileV1>(path) {
        Ok(Some(history)) => match history.validate() {
            Ok(()) => history,
            Err(error) => {
                eprintln!(
                    "Ignoring invalid project history {}: {error}",
                    path.display()
                );
                ProjectHistoryFileV1::default()
            }
        },
        Ok(None) => ProjectHistoryFileV1::default(),
        Err(error) => {
            eprintln!(
                "Ignoring invalid project history {}: {error}",
                path.display()
            );
            ProjectHistoryFileV1::default()
        }
    }
}

pub(crate) fn save_project_history(
    path: Option<&Path>,
    history: &ProjectHistoryFileV1,
) -> Result<(), String> {
    history.validate()?;
    if let Some(path) = path {
        save_ron_atomic(path, history)?;
    }
    Ok(())
}

fn ensure_absolute(path: &Path, label: &str) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!("{label} must be absolute: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_moves_existing_entry_to_front_without_losing_directories() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.holopaint");
        let second = temp.path().join("second.holopaint");
        let image_dir = temp.path().join("images");
        let mut history = ProjectHistoryFileV1::default();

        history
            .set_export_image_directory(first.clone(), image_dir.clone())
            .unwrap();
        history.touch(second.clone()).unwrap();
        history.touch(first.clone()).unwrap();

        assert_eq!(history.entries[0].path, first);
        assert_eq!(history.entries[0].export_image_directory, Some(image_dir));
        assert_eq!(history.entries[1].path, second);
    }

    #[test]
    fn image_and_psd_directories_are_independent() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project.holopaint");
        let image_dir = temp.path().join("images");
        let psd_dir = temp.path().join("psd");
        let mut history = ProjectHistoryFileV1::default();

        history
            .set_export_image_directory(project.clone(), image_dir.clone())
            .unwrap();
        history
            .set_export_psd_directory(project.clone(), psd_dir.clone())
            .unwrap();

        let entry = history.entry(&project).unwrap();
        assert_eq!(entry.export_image_directory.as_ref(), Some(&image_dir));
        assert_eq!(entry.export_psd_directory.as_ref(), Some(&psd_dir));
    }

    #[test]
    fn round_trip_preserves_mru_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("project_history.ron");
        let mut history = ProjectHistoryFileV1::default();
        history.touch(temp.path().join("a.holopaint")).unwrap();
        history.touch(temp.path().join("b.holopaint")).unwrap();

        save_project_history(Some(&path), &history).unwrap();

        assert_eq!(load_project_history(Some(&path)), history);
    }

    #[test]
    fn relative_paths_are_rejected() {
        let mut history = ProjectHistoryFileV1::default();
        assert!(history.touch(PathBuf::from("relative.holopaint")).is_err());
    }
}
