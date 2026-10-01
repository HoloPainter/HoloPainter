//! Read-only built-in bytes, addressed by `/`-separated paths relative to resources/.

pub(crate) struct EmbeddedResourceFile {
    pub(crate) path: &'static str,
    pub(crate) bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_resources.rs"));

pub(crate) fn bytes(path: &str) -> Option<&'static [u8]> {
    EMBEDDED_RESOURCE_FILES
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.bytes)
}

pub(crate) fn text(path: &str) -> Result<&'static str, String> {
    let bytes = bytes(path).ok_or_else(|| format!("embedded resource {path:?} was not found"))?;
    std::str::from_utf8(bytes)
        .map_err(|error| format!("embedded resource {path:?} is not UTF-8: {error}"))
}

pub(crate) fn files_under(
    directory: &str,
) -> impl Iterator<Item = &'static EmbeddedResourceFile> + '_ {
    EMBEDDED_RESOURCE_FILES
        .iter()
        .filter(move |file| relative_to_directory(file.path, directory).is_some())
}

pub(crate) fn files_in(
    directory: &str,
) -> impl Iterator<Item = &'static EmbeddedResourceFile> + '_ {
    files_under(directory).filter(move |file| {
        relative_to_directory(file.path, directory).is_some_and(|path| !path.contains('/'))
    })
}

fn relative_to_directory<'a>(path: &'a str, directory: &str) -> Option<&'a str> {
    let directory = directory.trim_end_matches('/');
    if directory.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(directory)?.strip_prefix('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_are_available_and_missing_keys_are_explicit() {
        assert!(!bytes("images/brush_tips/brush.r8").unwrap().is_empty());
        assert!(
            text("settings/default.settings.ron")
                .unwrap()
                .contains("schema_version")
        );
        assert!(bytes("missing.ron").is_none());
        assert!(
            text("missing.ron")
                .unwrap_err()
                .contains("embedded resource")
        );
        assert!(bytes("resources/settings/default.settings.ron").is_none());
        assert!(text("images/brush_tips/brush.r8").is_err());
    }

    #[test]
    fn enumeration_is_sorted_and_respects_directory_boundaries() {
        let paths = files_under("").map(|file| file.path).collect::<Vec<_>>();
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(paths.iter().all(|path| !path.contains('\\')));
        assert!(files_under("images").next().is_some());
        assert!(files_in("images").next().is_none());
        assert!(files_in("images/brush_tips").next().is_some());
        assert_eq!(
            files_under("images").count(),
            files_under("images/").count()
        );
        assert_eq!(files_under("image").count(), 0);
        assert_eq!(files_under("missing").count(), 0);
    }

    #[test]
    fn builtins_load_without_resources_directory() {
        const CHILD: &str = "HOLOPAINTER_EMBEDDED_RESOURCES_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let directory = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "embedded_resources::tests::builtins_load_without_resources_directory",
                    "--nocapture",
                ])
                .current_dir(directory.path())
                .env(CHILD, "1")
                .env("LOCALAPPDATA", directory.path().join("user-data"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("embedded resources loaded"));
            return;
        }

        assert!(!std::path::Path::new("resources").exists());
        crate::core::image_asset::ImageAssetCatalog::load_effective(None).unwrap();
        // Also resolves engines, presets, textures, and the built-in tool layout together.
        crate::core::tool_catalog::load_default_tools_from_resources().unwrap();
        crate::settings::SettingsFileV1::builtin().unwrap();
        crate::ui::workspace::WorkspaceFileV1::builtin().unwrap();
        crate::project::default_editor_view_state();
        let shortcut_path = crate::settings::current_user_shortcut_profile_path().unwrap();
        assert!(!shortcut_path.exists());
        let shortcuts = crate::ui::input::shortcuts::ShortcutInputRuntime::load().unwrap();
        assert_eq!(shortcuts.profile(), shortcuts.default_profile());

        // Exercise the standalone renderer when GPU tests are supported by this host.
        use eframe::egui_wgpu::wgpu;
        let instance = wgpu::Instance::default();
        if let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        {
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                    .unwrap();
            crate::renderer::RenderEngine::new(device, queue, [8, 8]).unwrap();
            println!("standalone renderer initialized");
        } else {
            eprintln!("standalone renderer check skipped: no GPU adapter");
        }
        println!("embedded resources loaded");
    }
}
