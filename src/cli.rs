use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
};

use crate::import::ModelFormat;

pub const HELP_TEXT: &str = concat!(
    "HoloPainter ",
    env!("CARGO_PKG_VERSION"),
    "\n",
    "3D texture painting application\n",
    "\n",
    "Usage:\n",
    "  holopainter [OPTIONS] [FILE]\n",
    "\n",
    "Arguments:\n",
    "  [FILE]    Project or 3D model to open\n",
    "            Supported: .holopaint, .gltf, .glb, .fbx\n",
    "\n",
    "Options:\n",
    "  -h, --help       Print help\n",
    "  -V, --version    Print version\n",
);

pub const VERSION_TEXT: &str = concat!("HoloPainter ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRequest {
    None,
    OpenProject(PathBuf),
    NewProjectFromModel(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Run(StartupRequest),
    PrintHelp,
    PrintVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub fn parse_env() -> Result<CliAction, CliError> {
    parse(std::env::args_os().skip(1))
}

pub fn parse<I, T>(args: I) -> Result<CliAction, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.len() == 1 {
        if args[0] == OsStr::new("-h") || args[0] == OsStr::new("--help") {
            return Ok(CliAction::PrintHelp);
        }
        if args[0] == OsStr::new("-V") || args[0] == OsStr::new("--version") {
            return Ok(CliAction::PrintVersion);
        }
    }

    let mut file = None;
    let mut options_enabled = true;
    for arg in args {
        if options_enabled && arg == OsStr::new("--") {
            options_enabled = false;
            continue;
        }
        if options_enabled && is_option(&arg) {
            if matches!(arg.to_str(), Some("-h" | "--help" | "-V" | "--version")) {
                return Err(CliError::new(format!(
                    "option {} can only be used alone",
                    Path::new(&arg).display()
                )));
            }
            return Err(CliError::new(format!(
                "unknown option: {}",
                Path::new(&arg).display()
            )));
        }
        if file.replace(arg).is_some() {
            return Err(CliError::new("only one startup file can be specified"));
        }
    }

    let Some(file) = file else {
        return Ok(CliAction::Run(StartupRequest::None));
    };
    classify_file(PathBuf::from(file)).map(CliAction::Run)
}

fn is_option(value: &OsStr) -> bool {
    value.as_encoded_bytes().first() == Some(&b'-')
}

fn classify_file(path: PathBuf) -> Result<StartupRequest, CliError> {
    let request = if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("holopaint"))
    {
        StartupRequest::OpenProject
    } else if ModelFormat::from_path(&path).is_ok() {
        StartupRequest::NewProjectFromModel
    } else {
        return Err(CliError::new(format!(
            "unsupported startup file: {}",
            path.display()
        )));
    };
    let path = std::path::absolute(&path).map_err(|error| {
        CliError::new(format!(
            "failed to make startup path absolute ({}): {error}",
            path.display()
        ))
    })?;
    Ok(request(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_runs_without_a_startup_request() {
        assert_eq!(
            parse(Vec::<OsString>::new()).unwrap(),
            CliAction::Run(StartupRequest::None)
        );
    }

    #[test]
    fn project_extensions_are_case_insensitive_and_paths_are_absolute() {
        for path in ["project.holopaint", "PROJECT.HOLOPAINT"] {
            let action = parse([path]).unwrap();
            let CliAction::Run(StartupRequest::OpenProject(parsed)) = action else {
                panic!("expected an open-project request");
            };
            assert!(parsed.is_absolute());
            assert!(parsed.ends_with(path));
        }
    }

    #[test]
    fn model_extensions_are_case_insensitive() {
        for path in ["model.gltf", "model.GLB", "model.FBX"] {
            let action = parse([path]).unwrap();
            let CliAction::Run(StartupRequest::NewProjectFromModel(parsed)) = action else {
                panic!("expected a new-project-from-model request");
            };
            assert!(parsed.is_absolute());
            assert!(parsed.ends_with(path));
        }
    }

    #[test]
    fn paths_with_spaces_are_preserved() {
        let action = parse([OsString::from("models/My Character.glb")]).unwrap();
        let CliAction::Run(StartupRequest::NewProjectFromModel(path)) = action else {
            panic!("expected a new-project-from-model request");
        };
        assert!(path.ends_with("models/My Character.glb"));
    }

    #[cfg(windows)]
    #[test]
    fn non_utf8_windows_paths_are_preserved() {
        use std::os::windows::ffi::OsStringExt;

        let path = OsString::from_wide(&[
            b'm' as u16,
            0xd800,
            std::path::MAIN_SEPARATOR as u16,
            b'p' as u16,
            b'r' as u16,
            b'o' as u16,
            b'j' as u16,
            b'e' as u16,
            b'c' as u16,
            b't' as u16,
            b'.' as u16,
            b'h' as u16,
            b'o' as u16,
            b'l' as u16,
            b'o' as u16,
            b'p' as u16,
            b'a' as u16,
            b'i' as u16,
            b'n' as u16,
            b't' as u16,
        ]);
        let action = parse([path]).unwrap();
        let CliAction::Run(StartupRequest::OpenProject(parsed)) = action else {
            panic!("expected an open-project request");
        };
        assert!(parsed.is_absolute());
        assert!(
            parsed
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("holopaint"))
        );
    }

    #[test]
    fn supported_extensions_do_not_require_an_existing_file() {
        let action = parse(["this-file-does-not-exist.holopaint"]).unwrap();
        assert!(matches!(
            action,
            CliAction::Run(StartupRequest::OpenProject(_))
        ));
    }

    #[test]
    fn unsupported_files_are_rejected() {
        for path in ["image.png", "brush.holopack", "extensionless"] {
            assert!(
                parse([path])
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported startup file")
            );
        }
    }

    #[test]
    fn multiple_files_are_rejected() {
        assert_eq!(
            parse(["a.holopaint", "b.holopaint"])
                .unwrap_err()
                .to_string(),
            "only one startup file can be specified"
        );
    }

    #[test]
    fn unknown_options_are_rejected() {
        assert_eq!(
            parse(["--unknown"]).unwrap_err().to_string(),
            "unknown option: --unknown"
        );
    }

    #[test]
    fn help_and_version_actions_accept_short_and_long_options() {
        assert_eq!(parse(["-h"]).unwrap(), CliAction::PrintHelp);
        assert_eq!(parse(["--help"]).unwrap(), CliAction::PrintHelp);
        assert_eq!(parse(["-V"]).unwrap(), CliAction::PrintVersion);
        assert_eq!(parse(["--version"]).unwrap(), CliAction::PrintVersion);
    }

    #[test]
    fn help_and_version_options_cannot_be_combined() {
        assert!(parse(["--help", "model.fbx"]).is_err());
        assert!(parse(["--version", "model.fbx"]).is_err());
    }

    #[test]
    fn option_terminator_allows_a_dash_prefixed_file() {
        let action = parse(["--", "-model.fbx"]).unwrap();
        let CliAction::Run(StartupRequest::NewProjectFromModel(path)) = action else {
            panic!("expected a new-project-from-model request");
        };
        assert!(path.ends_with("-model.fbx"));
    }
}
