use std::path::PathBuf;

pub(crate) fn user_settings_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|base| base.join("HoloPainter"))
}

pub(crate) fn current_user_settings_path() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("settings.ron"))
}

pub(crate) fn current_user_project_history_path() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("project_history.ron"))
}

pub(crate) fn current_user_shortcut_profile_path() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("shortcuts").join("default.shortcut_profile.ron"))
}

pub(crate) fn current_user_image_asset_dir() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("images"))
}

pub(crate) fn current_user_brush_preset_dir() -> Option<PathBuf> {
    user_settings_root().map(|root| root.join("brushes").join("presets"))
}

pub(crate) fn current_user_tool_layout_path() -> Option<PathBuf> {
    user_settings_root().map(|root| {
        root.join("tools")
            .join("layouts")
            .join("default.tool_layout.ron")
    })
}
