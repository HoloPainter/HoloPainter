use std::path::{Path, PathBuf};

use crate::{persistence::load_ron, settings::user_settings_root};

use super::WorkspaceFileV1;

pub struct StartupWorkspace {
    pub(crate) path: Option<PathBuf>,
    pub(crate) file: WorkspaceFileV1,
}

impl StartupWorkspace {
    pub fn apply_to_viewport_builder(
        &self,
        mut builder: eframe::egui::ViewportBuilder,
    ) -> eframe::egui::ViewportBuilder {
        if let Some(size) = self.file.window.inner_size {
            builder = builder.with_inner_size([size.width, size.height]);
        }
        if let Some(position) = self.file.window.position {
            builder = builder.with_position([position.x, position.y]);
        }
        if let Some(maximized) = self.file.window.maximized {
            builder = builder.with_maximized(maximized);
        }
        builder
    }

    pub(crate) fn into_parts(self) -> (Option<PathBuf>, WorkspaceFileV1) {
        (self.path, self.file)
    }
}

pub fn load_startup_workspace() -> StartupWorkspace {
    let path = current_user_workspace_path();
    let file = load_workspace_file(path.as_deref()).unwrap_or_else(|| {
        WorkspaceFileV1::builtin().expect("embedded default workspace must be valid")
    });
    StartupWorkspace { path, file }
}

pub(crate) fn current_user_workspace_path() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("workspace.ron"))
}

pub(crate) fn load_workspace_file(path: Option<&Path>) -> Option<WorkspaceFileV1> {
    let path = path?;
    match load_file(path) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!(
                "Ignoring invalid workspace file {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn load_file(path: &Path) -> Result<Option<WorkspaceFileV1>, String> {
    let Some(workspace) =
        load_ron::<WorkspaceFileV1>(path).map_err(|error| format!("loading workspace: {error}"))?
    else {
        return Ok(None);
    };
    workspace.validate()?;
    Ok(Some(workspace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_startup_workspace_applies_only_explicit_window_defaults() {
        let startup = StartupWorkspace {
            path: None,
            file: WorkspaceFileV1::builtin().expect("builtin workspace"),
        };

        let builder = startup.apply_to_viewport_builder(eframe::egui::ViewportBuilder::default());

        assert_eq!(builder.inner_size, Some(eframe::egui::vec2(1200.0, 720.0)));
        assert_eq!(builder.position, None);
        assert_eq!(builder.maximized, None);
    }

    #[test]
    fn missing_workspace_file_does_not_create_anything() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("workspace.ron");

        assert!(load_workspace_file(Some(&path)).is_none());
        assert!(!path.exists());
        assert!(!path.parent().expect("parent").exists());
    }
}
