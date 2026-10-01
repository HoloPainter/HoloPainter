use std::path::Path;

use crate::persistence::load_ron;

use super::model::SettingsFileV1;

pub(crate) fn load_effective_settings(path: Option<&Path>) -> SettingsFileV1 {
    let builtin = SettingsFileV1::builtin().expect("embedded default settings must be valid");
    let Some(path) = path else {
        return builtin;
    };
    match load_file(path) {
        Ok(Some(settings)) => settings,
        Ok(None) => builtin,
        Err(error) => {
            eprintln!("Ignoring invalid settings file {}: {error}", path.display());
            builtin
        }
    }
}

fn load_file(path: &Path) -> Result<Option<SettingsFileV1>, String> {
    let Some(settings) =
        load_ron::<SettingsFileV1>(path).map_err(|error| format!("loading settings: {error}"))?
    else {
        return Ok(None);
    };
    settings.validate()?;
    Ok(Some(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_file_uses_builtin_without_creating_anything() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("settings.ron");

        let loaded = load_effective_settings(Some(&path));

        assert_eq!(loaded, SettingsFileV1::builtin().expect("builtin settings"));
        assert!(!path.exists());
        assert!(!path.parent().expect("parent").exists());
    }
}
