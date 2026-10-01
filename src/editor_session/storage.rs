use std::path::{Path, PathBuf};

use crate::{persistence::load_ron, settings::user_settings_root};

use super::EditorSessionFileV1;

pub(crate) fn current_user_editor_session_path() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("session.ron"))
}

pub(crate) fn load_editor_session_file(path: Option<&Path>) -> Option<EditorSessionFileV1> {
    let path = path?;
    match load_file(path) {
        Ok(session) => session,
        Err(error) => {
            eprintln!(
                "Ignoring invalid editor session file {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn load_file(path: &Path) -> Result<Option<EditorSessionFileV1>, String> {
    let Some(session) = load_ron::<EditorSessionFileV1>(path)
        .map_err(|error| format!("loading editor session: {error}"))?
    else {
        return Ok(None);
    };
    session.validate()?;
    Ok(Some(session))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::editor_session::model::{EDITOR_SESSION_SCHEMA_VERSION, ToolSelectionStateV1};

    use super::*;

    #[test]
    fn missing_editor_session_file_does_not_create_anything() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("session.ron");

        assert!(load_editor_session_file(Some(&path)).is_none());
        assert!(!path.exists());
        assert!(!path.parent().expect("parent").exists());
    }

    #[test]
    fn invalid_editor_session_file_is_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.ron");
        fs::write(&path, "this is not valid RON").expect("write invalid editor session");

        assert!(load_editor_session_file(Some(&path)).is_none());
    }

    #[test]
    fn unsupported_editor_session_file_is_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.ron");
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION + 1,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: None,
                groups: Vec::new(),
            },
        };
        fs::write(
            &path,
            ron::ser::to_string(&file).expect("serialize session"),
        )
        .expect("write unsupported editor session");

        assert!(load_editor_session_file(Some(&path)).is_none());
    }
}
