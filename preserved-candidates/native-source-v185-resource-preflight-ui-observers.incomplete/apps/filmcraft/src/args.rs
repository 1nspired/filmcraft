//! The desktop app's command line (`filmcraft --help`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `filmcraft --help`.
pub const USAGE: &str = "\
Usage: filmcraft [options] [project.fcproj | media files...]

Options:
  --control <port>   start the localhost JSON-lines control server
                     (also: FILMCRAFT_CONTROL_PORT)
  --control-input-only  accept widget input only from the control server
                     (requires --control or FILMCRAFT_CONTROL_PORT)
  --demo             open the demo project
  --empty            start with an empty project
  --recover          recover the newest unsaved changes without asking
  --no-recover       start without asking about unsaved changes
  --data-dir <dir>   folder for auto-save, crash recovery and logs
                     (also: FILMCRAFT_DATA_DIR)
  --poema-composer <json>  embed the bound shared Poema composer (macOS)
  --poema-save-root <path>  retain this original manager for native Save startup
  --cold-save-recovery <json>  trusted cold Save on parent-owned pipes, held
                     (explicit --data-dir; no control, UI or startup fallback)
  --workspace-update <json>  managed native update startup, initially paused
                     (requires --control and an explicit --data-dir; no files)
  --project-create <json>  create a new empty Root project, initially paused\n                     (requires --control and explicit new --data-dir; no files)\n  --project-recovery <json>  restore an exact complete Root snapshot, paused
                     (requires --control and explicit --data-dir; no files)
  --identity-recovery <json>  restore an exact native identity decision, paused
                     (requires --control and an explicit --data-dir; no files)
  --graphics <mode>  start with this graphics mode and keep it: default,
                     dx12 (Windows), software (Windows, Linux);
                     `--graphics default` undoes an automatic fallback
  --version          print the version and exit
  -h, --help         print this help and exit
";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Cli {
    Run(Box<Launch>),
    Help,
    Version,
}

/// Options of a normal start.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub control_port: Option<u16>,
    pub control_input_only: bool,
    /// A project and / or media files to open.
    pub files: Vec<String>,
    pub demo: bool,
    /// `--demo` / `--empty` given: skip Settings ▸ General ▸ At Startup.
    pub startup_flag: bool,
    pub recover: Option<bool>,
    pub data_dir: Option<std::path::PathBuf>,
    /// `--graphics <mode>`: start with this graphics mode and keep it (see `graphics.rs`).
    pub graphics: Option<crate::graphics::Mode>,
    /// Private managed startup descriptor, never an automatic project fallback.
    pub workspace_update: Option<String>,
    pub identity_recovery: Option<String>,
    pub project_recovery: Option<String>,
    pub project_create: Option<String>,
    /// Exclusive trusted parent-owned pipe startup, never ordinary UI admission.
    pub cold_save_recovery: Option<String>,
    pub poema_composer: Option<filmcraft_ui_egui::composer_panel::Binding>,
    pub poema_save_root: Option<PathBuf>,
}

/// Parse the arguments after the program name. `env_control_port` is the value of
/// `FILMCRAFT_CONTROL_PORT`, used when `--control` is not given (an empty value counts as unset).
///
/// An option we don't know, or one without a usable value, is an error: opening a window for a
/// mistyped flag helps nobody, and starting without the control server that was asked for leaves
/// a script waiting for it. That holds for the whole line, so `--version --bogus` is an error
/// too, and for a port from the environment exactly as for one from `--control`.
///
/// The arguments come from `std::env::args_os`: not every argument is Unicode (a Linux file name
/// in Latin-1), and `std::env::args` panics on one. A file name that is not is passed on lossily
/// (its import then reports it missing); `--data-dir` keeps the folder exactly.
pub fn parse(env_control_port: Option<String>, args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    let mut l = Launch {
        control_port: None,
        control_input_only: false,
        files: Vec::new(),
        demo: true,
        startup_flag: false,
        recover: None,
        data_dir: None,
        graphics: None,
        workspace_update: None,
        identity_recovery: None,
        project_recovery: None,
        project_create: None,
        cold_save_recovery: None,
        poema_composer: None,
        poema_save_root: None,
    };
    let (mut help, mut version) = (false, false);
    let mut data_dir_count = 0usize;
    let mut args = args.into_iter();
    while let Some(raw) = args.next() {
        let a = raw.to_string_lossy();
        match a.as_ref() {
            "-h" | "--help" => help = true,
            "--version" => version = true,
            "--control" => {
                let port = args.next().ok_or("--control needs a port number")?;
                let port = port.to_string_lossy();
                l.control_port = Some(port.parse().map_err(|_| format!("--control needs a port number, got `{port}`"))?);
            }
            "--demo" => (l.demo, l.startup_flag) = (true, true),
            "--control-input-only" => l.control_input_only = true,
            "--empty" => (l.demo, l.startup_flag) = (false, true),
            "--recover" => l.recover = Some(true),
            "--no-recover" => l.recover = Some(false),
            "--data-dir" => {
                data_dir_count = data_dir_count.saturating_add(1);
                l.data_dir = Some(args.next().map(PathBuf::from).ok_or("--data-dir needs a folder")?);
            }
            "--cold-save-recovery" => {
                if l.cold_save_recovery.is_some() {
                    return Err("repeated --cold-save-recovery".into());
                }
                let raw = args.next().ok_or("--cold-save-recovery needs an original private input descriptor")?;
                let text = raw.to_str().filter(|v| !v.is_empty() && v.len() <= 16384).ok_or("bounded UTF-8 cold Save descriptor required")?;
                l.cold_save_recovery = Some(text.into());
            }
            "--poema-save-root" => {
                if l.poema_save_root.is_some() {
                    return Err("repeated --poema-save-root".into());
                }
                let value = args.next().ok_or("--poema-save-root needs an original manifest path")?;
                let text = value
                    .to_str()
                    .filter(|v| !v.is_empty() && v.len() <= 16384 && !v.chars().any(char::is_control))
                    .ok_or("bounded UTF-8 Save manager path required")?;
                let path = PathBuf::from(text);
                if !path.is_absolute() || path.components().any(|v| !matches!(v, std::path::Component::RootDir | std::path::Component::Normal(_))) {
                    return Err("literal absolute Save manager path required".into());
                }
                l.poema_save_root = Some(path);
            }
            "--poema-composer" => {
                if l.poema_composer.is_some() {
                    return Err("repeated --poema-composer".into());
                }
                let raw = args.next().ok_or("--poema-composer needs a workspace binding")?;
                let text = raw.to_str().ok_or("Poema composer binding must be UTF-8")?;
                l.poema_composer = Some(filmcraft_ui_egui::composer_panel::Binding::parse(text)?);
            }
            "--project-create" => {
                if l.project_create.is_some() {
                    return Err("repeated --project-create".into());
                }
                let raw = args.next().ok_or("--project-create needs a new project descriptor")?;
                let text = raw.to_str().filter(|v| !v.is_empty() && v.len() <= 16384).ok_or("invalid bounded project creation descriptor")?;
                l.project_create = Some(text.into());
            }
            "--project-recovery" => {
                if l.project_recovery.is_some() {
                    return Err("repeated --project-recovery".into());
                }
                let raw = args.next().ok_or("--project-recovery needs a complete native snapshot descriptor")?;
                let text = raw.to_str().filter(|v| !v.is_empty() && v.len() <= 16384).ok_or("invalid bounded project recovery descriptor")?;
                l.project_recovery = Some(text.into());
            }
            "--identity-recovery" => {
                if l.identity_recovery.is_some() {
                    return Err("repeated --identity-recovery".into());
                }
                let raw = args.next().ok_or("--identity-recovery needs a native descriptor")?;
                let text = raw.to_str().filter(|v| !v.is_empty() && v.len() <= 16384).ok_or("invalid bounded native identity descriptor")?;
                l.identity_recovery = Some(text.into());
            }
            "--workspace-update" => {
                if l.workspace_update.is_some() {
                    return Err("repeated --workspace-update".into());
                }
                let raw = args.next().ok_or("--workspace-update needs a native descriptor")?;
                let text = raw.to_str().filter(|v| !v.is_empty() && v.len() <= 16384).ok_or("invalid bounded native update descriptor")?;
                l.workspace_update = Some(text.into());
            }
            "--graphics" => {
                let name = args.next().ok_or("--graphics needs a mode: default, dx12 or software")?;
                l.graphics = Some(graphics_mode(&name.to_string_lossy())?);
            }
            // everything after `--` is a file, whatever it looks like
            "--" => l.files.extend(args.by_ref().map(|f| f.to_string_lossy().into_owned())),
            // macOS adds a process serial number when it launches an app bundle
            _ if a.starts_with("-psn_") => {}
            _ if a.len() > 1 && a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ => l.files.push(a.into_owned()),
        }
    }
    if help {
        return Ok(Cli::Help);
    }
    if version {
        return Ok(Cli::Version);
    }
    if l.control_port.is_none()
        && let Some(port) = env_control_port.as_deref().map(str::trim).filter(|p| !p.is_empty())
    {
        l.control_port = Some(port.parse().map_err(|_| format!("FILMCRAFT_CONTROL_PORT needs a port number, got `{port}`"))?);
    }
    if l.control_input_only && l.control_port.is_none() {
        return Err("--control-input-only requires --control or FILMCRAFT_CONTROL_PORT".into());
    }
    if [l.workspace_update.is_some(), l.identity_recovery.is_some(), l.project_recovery.is_some(), l.project_create.is_some(), l.cold_save_recovery.is_some()]
        .into_iter()
        .filter(|v| *v)
        .count()
        > 1
    {
        return Err("choose one explicit native startup descriptor".into());
    }
    if l.cold_save_recovery.is_some() {
        if data_dir_count != 1
            || l.data_dir
                .as_ref()
                .is_none_or(|p| !p.is_absolute() || p.components().any(|v| !matches!(v, std::path::Component::RootDir | std::path::Component::Normal(_))))
            || l.control_port.is_some()
            || l.control_input_only
            || !l.files.is_empty()
            || l.startup_flag
            || l.recover == Some(true)
            || l.graphics.is_some()
            || l.poema_composer.is_some()
            || l.poema_save_root.is_some()
        {
            return Err("cold Save requires one explicit absolute data directory and exclusive parent-owned pipes; no control, UI or startup fallback".into());
        }
        l.demo = false;
        l.startup_flag = true;
        l.recover = Some(false);
    } else if l.workspace_update.is_some() || l.identity_recovery.is_some() || l.project_recovery.is_some() || l.project_create.is_some() {
        if l.control_port.is_none()
            || l.control_input_only
            || l.data_dir.as_ref().is_none_or(|p| !p.is_absolute())
            || !l.files.is_empty()
            || l.startup_flag
            || l.recover == Some(true)
        {
            return Err("managed update requires --control and an absolute --data-dir, with no project/demo/empty/recover fallback".into());
        }
        l.demo = false;
        l.startup_flag = true;
        l.recover = Some(false);
    }
    if let Some(binding) = &l.poema_composer
        && (l.control_port.is_none()
            || l.control_input_only
            || l.recover == Some(true)
            || l.workspace_update.is_none()
                && l.identity_recovery.is_none()
                && l.project_recovery.is_none()
                && l.project_create.is_none()
                && (l.files.len() != 1 || l.files.first() != Some(&binding.project_path) || l.startup_flag))
    {
        return Err("embedded composer requires its exact explicit project and control port; no recovery/demo or control-input-only fallback".into());
    }
    if l.poema_save_root.is_some()
        && (l.control_port.is_none()
            || l.control_input_only
            || l.recover != Some(false)
            || l.data_dir.as_ref().is_none_or(|p| !p.is_absolute())
            || l.workspace_update.is_none()
                && l.identity_recovery.is_none()
                && l.project_recovery.is_none()
                && l.project_create.is_none()
                && (l.files.len() != 1 || l.files.first().is_none_or(|p| !Path::new(p).is_absolute() || !is_project(p)) || l.startup_flag))
    {
        return Err("Save manager startup requires explicit native project or managed recovery, control, data and no-recover; no fallback".into());
    }
    Ok(Cli::Run(Box::new(l)))
}

/// Whether a launch file is a FilmCraft project. The extension's case does not matter: Windows file
/// associations and the Linux MIME glob start the app for `CUT.FCPROJ` too (#850).
pub fn is_project(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("fcproj"))
}

/// `path` made absolute against the working directory, so a project or media file named relative
/// to a terminal's folder is still found once the app runs from another one (Recent Projects, the
/// media paths saved in the project). Unlike `canonicalize`, `std::path::absolute` keeps symlinks
/// and adds no Windows verbatim `\\?\` prefix. A path it cannot resolve is kept as given.
pub fn absolute(path: &str) -> String {
    std::path::absolute(path).ok().and_then(|p| p.into_os_string().into_string().ok()).unwrap_or_else(|| path.to_string())
}

/// The launch files as the project to open (the first one, see [`is_project`]) and the media files
/// to import (every file that is not a project), all made [`absolute`].
pub fn project_and_media(files: &[String]) -> (Option<String>, Vec<String>) {
    let project = files.iter().find(|f| is_project(f)).map(|f| absolute(f));
    let media = files.iter().filter(|f| !is_project(f)).map(|f| absolute(f)).collect();
    (project, media)
}

/// A `--graphics` value: one of this platform's modes.
fn graphics_mode(name: &str) -> Result<crate::graphics::Mode, String> {
    use crate::graphics::{LADDER, Mode};
    let here = || LADDER.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ");
    let mode = Mode::parse(name).ok_or_else(|| format!("--graphics needs one of {}, got `{name}`", here()))?;
    if !LADDER.contains(&mode) {
        return Err(format!("--graphics {name} is not available on this platform (use one of {})", here()));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_env(env: Option<&str>, args: &[&str]) -> Result<Cli, String> {
        parse(env.map(str::to_string), args.iter().map(OsString::from))
    }

    fn parse_args(args: &[&str]) -> Result<Cli, String> {
        parse_env(None, args)
    }

    fn launch(args: &[&str]) -> Launch {
        match parse_args(args) {
            Ok(Cli::Run(l)) => *l,
            other => panic!("{args:?} should start the app, got {other:?}"),
        }
    }

    /// `filmcraft --help` used to open the app with "--help" as a media file to import.
    #[test]
    fn help_and_version_do_not_start_the_app() {
        assert_eq!(parse_args(&["--help"]), Ok(Cli::Help));
        assert_eq!(parse_args(&["-h"]), Ok(Cli::Help));
        assert_eq!(parse_args(&["--empty", "--help"]), Ok(Cli::Help));
        assert_eq!(parse_args(&["--version"]), Ok(Cli::Version));
        assert_eq!(parse_args(&["--version", "--help"]), Ok(Cli::Help));
        assert_eq!(parse_args(&["a.mp4", "--version"]), Ok(Cli::Version));
        for flag in ["--control", "--demo", "--empty", "--recover", "--no-recover", "--data-dir", "--version", "--help"] {
            assert!(USAGE.contains(flag), "usage lists {flag}");
        }
    }

    #[test]
    fn unknown_options_are_errors() {
        assert_eq!(parse_args(&["--frobnicate"]), Err("unknown option `--frobnicate`".to_string()));
        assert!(parse_args(&["a.mp4", "-x"]).is_err());
        assert!(parse_args(&["--Help"]).is_err());
        assert!(parse_args(&["--control"]).is_err(), "missing port");
        assert!(parse_args(&["--control", "abc"]).unwrap_err().contains("abc"));
        assert!(parse_args(&["--control", "70000"]).is_err(), "not a port");
        assert!(parse_args(&["--data-dir"]).is_err(), "missing folder");
        // the whole line is checked, whatever comes first
        assert_eq!(parse_args(&["--version", "--bogus"]), Err("unknown option `--bogus`".to_string()));
        assert_eq!(parse_args(&["--bogus", "--version"]), Err("unknown option `--bogus`".to_string()));
        assert_eq!(parse_args(&["--help", "--bogus"]), Err("unknown option `--bogus`".to_string()));
        assert!(parse_args(&["--help", "--control"]).is_err());
    }

    /// A bad `FILMCRAFT_CONTROL_PORT` used to start the app without the control server, silently.
    #[test]
    fn a_bad_port_in_the_environment_is_an_error_like_a_bad_control_flag() {
        for bad in ["abc", "70000", "-1", "98 76", "9876x", "\u{fffd}"] {
            let e = parse_env(Some(bad), &[]).unwrap_err();
            assert!(e.contains("FILMCRAFT_CONTROL_PORT") && e.contains(bad), "{bad}: {e}");
            assert!(parse_env(Some(bad), &["--empty", "a.mp4"]).is_err(), "{bad}");
            // the flag wins, so the environment's value is not looked at; nor for --help / --version
            assert_eq!(parse_env(Some(bad), &["--control", "9"]).map(|c| matches!(c, Cli::Run(l) if l.control_port == Some(9))), Ok(true), "{bad}");
            assert_eq!(parse_env(Some(bad), &["--help"]), Ok(Cli::Help));
            assert_eq!(parse_env(Some(bad), &["--version"]), Ok(Cli::Version));
        }
        // unset or empty: no control server
        assert_eq!(launch(&[]).control_port, None);
        assert_eq!(parse_env(Some(""), &[]), Ok(Cli::Run(Box::new(launch(&[])))));
        assert_eq!(parse_env(Some("  "), &[]), Ok(Cli::Run(Box::new(launch(&[])))));
        // a good one is the default
        assert_eq!(parse_env(Some("1234"), &[]), Ok(Cli::Run(Box::new(Launch { control_port: Some(1234), ..launch(&[]) }))));
        assert_eq!(parse_env(Some("1234"), &["--control", "9"]).map(|c| matches!(c, Cli::Run(l) if l.control_port == Some(9))), Ok(true));
    }

    #[test]
    fn known_options_and_files_parse_as_before() {
        let none = launch(&[]);
        assert_eq!(
            none,
            Launch {
                control_port: None,
                control_input_only: false,
                files: vec![],
                demo: true,
                startup_flag: false,
                recover: None,
                data_dir: None,
                graphics: None,
                workspace_update: None,
                identity_recovery: None,
                project_recovery: None,
                project_create: None,
                cold_save_recovery: None,
                poema_composer: None,
                poema_save_root: None,
            }
        );
        let l = launch(&["--control", "9876", "--empty", "--no-recover", "--data-dir", "/tmp/fc", "cut.fcproj", "a.mp4"]);
        assert_eq!(l.control_port, Some(9876));
        assert_eq!((l.demo, l.startup_flag, l.recover), (false, true, Some(false)));
        assert_eq!(l.data_dir, Some(std::path::PathBuf::from("/tmp/fc")));
        assert_eq!(l.files, ["cut.fcproj", "a.mp4"]);
        assert_eq!((launch(&["--demo"]).demo, launch(&["--demo"]).startup_flag, launch(&["--recover"]).recover), (true, true, Some(true)));
        // `--` ends the options; a lone `-` and macOS' process serial number are not options
        assert_eq!(launch(&["--", "--odd name.mov", "-h"]).files, ["--odd name.mov", "-h"]);
        assert_eq!(launch(&["-", "-psn_0_12345"]).files, ["-"]);
    }

    #[test]
    fn managed_update_startup_has_no_project_demo_or_recovery_fallback() {
        let descriptor = r#"{"workspace":"native-only-descriptor"}"#;
        let directory = std::env::temp_dir().join("generated-native-update").to_string_lossy().into_owned();
        let l = launch(&["--control", "9876", "--data-dir", &directory, "--workspace-update", descriptor]);
        assert_eq!(l.workspace_update.as_deref(), Some(descriptor));
        assert!(!l.demo);
        assert!(l.startup_flag);
        assert_eq!(l.recover, Some(false));
        assert!(l.files.is_empty());
        for args in [
            vec!["--workspace-update", descriptor],
            vec!["--control", "9876", "--workspace-update", descriptor],
            vec!["--control", "9876", "--data-dir", "relative", "--workspace-update", descriptor],
            vec!["--control", "9876", "--data-dir", "/tmp/generated", "--workspace-update", descriptor, "cut.fcproj"],
            vec!["--control", "9876", "--data-dir", "/tmp/generated", "--workspace-update", descriptor, "--demo"],
            vec!["--control", "9876", "--data-dir", "/tmp/generated", "--workspace-update", descriptor, "--empty"],
            vec!["--control", "9876", "--data-dir", "/tmp/generated", "--workspace-update", descriptor, "--recover"],
            vec!["--control", "9876", "--data-dir", "/tmp/generated", "--workspace-update", descriptor, "--workspace-update", descriptor],
        ] {
            assert!(parse_env(None, &args).is_err(), "{args:?}");
        }
    }

    /// `std::env::args` panics on an argument that is not Unicode, so such a file name (Latin-1 on
    /// Linux) stopped the app before its window opened.
    #[cfg(unix)]
    #[test]
    fn arguments_that_are_not_unicode_still_start_the_app() {
        use std::os::unix::ffi::OsStringExt;
        let dir = OsString::from_vec(b"/tmp/d\xe9".to_vec());
        let file = OsString::from_vec(b"caf\xe9.mov".to_vec());
        let Ok(Cli::Run(l)) = parse(None, [OsString::from("--data-dir"), dir.clone(), file.clone(), OsString::from("--"), file]) else {
            panic!("should start the app");
        };
        assert_eq!(l.data_dir, Some(PathBuf::from(dir)), "the data folder is kept exactly");
        assert_eq!(l.files, ["caf\u{fffd}.mov", "caf\u{fffd}.mov"]);
    }

    /// Windows file associations and the Linux MIME glob match `CUT.FCPROJ` too; it used to be
    /// imported as a media file instead of opened (#850).
    #[test]
    fn a_project_is_recognised_whatever_the_case_of_its_extension() {
        for p in ["cut.fcproj", "CUT.FCPROJ", "Cut.FcProj", "/edits/a b.fcproj", r"C:\Edits\CUT.FCPROJ"] {
            assert!(is_project(p), "{p}");
        }
        for p in ["cut.mp4", "fcproj", ".fcproj", "a.fcproj.mov", "a.fcprojx", ""] {
            assert!(!is_project(p), "{p}");
        }
    }

    /// Relative launch paths were opened and imported as given, so the project's media paths and
    /// Recent Projects pointed nowhere once the app ran from another folder (#850).
    #[test]
    fn launch_files_are_made_absolute_and_split_case_insensitively() {
        let cwd = std::env::current_dir().unwrap();
        let nested = Path::new("clips").join("a.mp4");
        let already = cwd.join("b.wav").to_string_lossy().into_owned();
        let files = ["Cut.FCPROJ".to_string(), nested.to_string_lossy().into_owned(), "other.fcproj".to_string(), already.clone()];
        let (project, media) = project_and_media(&files);
        assert_eq!(project, Some(cwd.join("Cut.FCPROJ").to_string_lossy().into_owned()));
        assert_eq!(media, [cwd.join(&nested).to_string_lossy().into_owned(), already]);
        for p in media.iter().chain(&project) {
            assert!(Path::new(p).is_absolute() && !p.starts_with(r"\\?\"), "{p}");
        }
        assert_eq!(project_and_media(&[]), (None, vec![]));
        // nothing to resolve against: kept as given rather than dropped
        assert_eq!(absolute(""), "");
    }

    #[test]
    fn isolated_input_is_explicit_and_requires_a_control_server() {
        assert!(!launch(&["--control", "9876"]).control_input_only);
        assert!(parse_args(&["--control-input-only"]).is_err());
        assert!(launch(&["--control-input-only", "--control", "9876"]).control_input_only);
        assert!(matches!(parse_env(Some("9876"), &["--control-input-only"]), Ok(Cli::Run(l)) if l.control_input_only && l.control_port == Some(9876)));
        assert_eq!(parse_args(&["--control-input-only", "--help"]), Ok(Cli::Help));
        assert!(USAGE.contains("--control-input-only"));
    }

    #[test]
    fn graphics_picks_one_of_this_platforms_modes() {
        use crate::graphics::{LADDER, Mode};
        assert_eq!(launch(&[]).graphics, None);
        assert_eq!(launch(&["--graphics", "default"]).graphics, Some(Mode::Default));
        for mode in LADDER {
            assert_eq!(launch(&["--graphics", mode.name()]).graphics, Some(*mode));
        }
        // a mode this platform has not is refused, not ignored
        for mode in [Mode::Dx12, Mode::Software].into_iter().filter(|m| !LADDER.contains(m)) {
            assert!(parse_args(&["--graphics", mode.name()]).unwrap_err().contains("not available"));
        }
        assert!(parse_args(&["--graphics"]).is_err(), "missing mode");
        assert!(parse_args(&["--graphics", "vulkan"]).unwrap_err().contains("vulkan"));
        assert!(parse_args(&["--graphics", "DEFAULT"]).is_err());
        assert!(parse_args(&["--graphics", &"x".repeat(1 << 16)]).is_err());
        assert!(USAGE.contains("--graphics"));
    }
    #[test]
    fn embedded_composer_requires_one_exact_saved_project_and_control_port() {
        let binding = serde_json::json!({"version":1,"httpPort":55117,"workspaceId":"b2113e8d-8fe4-42e2-9561-a8f39d427d61",
            "projectId":"p_test","projectPath":"/generated/Film.fcproj","applicationRelease":"a".repeat(64)})
        .to_string();
        let parsed = launch(&["--control", "9876", "--no-recover", "--poema-composer", &binding, "/generated/Film.fcproj"]);
        assert_eq!(parsed.poema_composer.unwrap().project_id, "p_test");
        for arguments in [
            vec!["--poema-composer", &binding, "/generated/Film.fcproj"],
            vec!["--control", "9876", "--poema-composer", &binding, "/other.fcproj"],
            vec!["--control", "9876", "--poema-composer", &binding, "/generated/Film.fcproj", "more.mov"],
            vec!["--control", "9876", "--recover", "--poema-composer", &binding, "/generated/Film.fcproj"],
            vec!["--control", "9876", "--control-input-only", "--poema-composer", &binding, "/generated/Film.fcproj"],
            vec!["--control", "9876", "--poema-composer", &binding, "--poema-composer", &binding, "/generated/Film.fcproj"],
        ] {
            assert!(parse_args(&arguments).is_err());
        }
        assert!(parse_args(&["--poema-composer"]).is_err());
        assert!(parse_args(&["--control", "9876", "--poema-composer", "{bad", "/generated/Film.fcproj"]).is_err());
    }
    #[test]
    fn identity_recovery_startup_is_bounded_exclusive_and_has_no_file_or_demo_fallback() {
        let descriptor = r#"{"workspace":"native-identity-descriptor"}"#;
        let directory = std::env::temp_dir().join("generated-identity-recovery").to_string_lossy().into_owned();
        let parsed = launch(&["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor]);
        assert_eq!(parsed.identity_recovery.as_deref(), Some(descriptor));
        assert!(!parsed.demo);
        assert!(parsed.startup_flag);
        assert_eq!(parsed.recover, Some(false));
        assert!(parsed.files.is_empty());
        for args in [
            vec!["--identity-recovery", descriptor],
            vec!["--control", "9876", "--identity-recovery", descriptor],
            vec!["--control", "9876", "--data-dir", "relative", "--identity-recovery", descriptor],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "cut.fcproj"],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--empty"],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--demo"],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--recover"],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--workspace-update", descriptor],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--identity-recovery", descriptor],
            vec!["--control", "9876", "--data-dir", &directory, "--identity-recovery", descriptor, "--control-input-only"],
        ] {
            assert!(parse(None, args.into_iter().map(Into::into)).is_err());
        }
        for text in ["".into(), "x".repeat(16385)] {
            assert!(parse(None, ["--control", "9876", "--data-dir", &directory, "--identity-recovery", &text].into_iter().map(Into::into)).is_err());
        }
    }
    #[test]
    fn complete_project_recovery_cli_requires_explicit_exclusive_managed_startup_without_file_or_sequence_fallback() {
        let directory = std::env::temp_dir().join("generated-project-recovery").to_string_lossy().into_owned();
        let args = ["--control", "9876", "--data-dir", &directory, "--project-recovery", "{}"];
        let l = launch(&args);
        assert_eq!(l.project_recovery.as_deref(), Some("{}"));
        assert!(!l.demo);
        assert!(l.startup_flag);
        assert_eq!(l.recover, Some(false));
        assert!(l.files.is_empty());
        for extra in [
            vec!["--identity-recovery", "{}"],
            vec!["--workspace-update", "{}"],
            vec!["--project-recovery", "{}"],
            vec!["--empty"],
            vec!["--demo"],
            vec!["--recover"],
            vec!["film.fcproj"],
            vec!["--control-input-only"],
        ] {
            let mut bad = args.to_vec();
            bad.extend(extra);
            assert!(parse_args(&bad).is_err());
        }
        assert!(parse_args(&["--project-recovery", "{}"]).is_err());
        assert!(parse_args(&["--control", "9876", "--data-dir", "relative", "--project-recovery", "{}"]).is_err());
    }
    #[test]
    fn cold_save_cli_requires_exclusive_private_pipes_and_one_explicit_data_directory() {
        let l = launch(&["--cold-save-recovery", "{}", "--data-dir", "/generated/native"]);
        assert_eq!(l.cold_save_recovery.as_deref(), Some("{}"));
        assert!(!l.demo && l.startup_flag);
        assert_eq!(l.recover, Some(false));
        assert!(l.control_port.is_none());
        for suffix in [
            vec!["--control", "55444"],
            vec!["--control-input-only"],
            vec!["--empty"],
            vec!["--demo"],
            vec!["--recover"],
            vec!["--graphics", "default"],
            vec!["/generated/project.fcproj"],
            vec!["--poema-save-root", "/generated/root.json"],
            vec!["--data-dir", "/other"],
        ] {
            let mut args = vec!["--cold-save-recovery", "{}", "--data-dir", "/generated/native"];
            args.extend(suffix);
            assert!(parse(None, args.into_iter().map(Into::into)).is_err());
        }
        assert!(parse(Some("55444".into()), ["--cold-save-recovery", "{}", "--data-dir", "/generated/native"].into_iter().map(Into::into)).is_err());
        assert!(parse(None, ["--cold-save-recovery", "{}"].into_iter().map(Into::into)).is_err());
        assert!(parse(None, ["--cold-save-recovery", "{}", "--data-dir", "relative"].into_iter().map(Into::into)).is_err());
    }
    #[test]
    fn cold_save_cli_refuses_crossed_repeated_and_unbounded_descriptors() {
        for kind in ["--workspace-update", "--identity-recovery", "--project-recovery", "--project-create", "--cold-save-recovery"] {
            let args = ["--cold-save-recovery", "{}", "--data-dir", "/generated/native", kind, "{}"];
            assert!(parse(None, args.into_iter().map(Into::into)).is_err());
        }
        for raw in [String::new(), "x".repeat(16385)] {
            assert!(parse(None, [std::ffi::OsString::from("--cold-save-recovery"), raw.into(), "--data-dir".into(), "/generated/native".into()]).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn cold_save_cli_refuses_non_unicode_descriptor_without_panicking_or_startup_fallback() {
        use std::os::unix::ffi::OsStringExt;
        assert!(
            parse(None, ["--cold-save-recovery".into(), std::ffi::OsString::from_vec(vec![255]), "--data-dir".into(), "/generated/native".into()]).is_err()
        );
    }
}

#[cfg(test)]
mod project_creation_tests {
    use super::*;
    #[test]
    fn creation_cli_has_no_project_recovery_demo_or_existing_file_fallback() {
        let base = ["--control", "9876", "--data-dir", "/generated/new-state", "--project-create", "{}"];
        let Cli::Run(l) = parse(None, base.into_iter().map(Into::into)).unwrap() else { panic!("run expected") };
        assert_eq!(l.project_create.as_deref(), Some("{}"));
        assert!(!l.demo);
        assert!(l.startup_flag);
        assert_eq!(l.recover, Some(false));
        assert!(l.files.is_empty());
        for extra in [
            vec!["--project-create", "{}"],
            vec!["--project-recovery", "{}"],
            vec!["--identity-recovery", "{}"],
            vec!["--workspace-update", "{}"],
            vec!["old.fcproj"],
            vec!["--recover"],
            vec!["--demo"],
            vec!["--empty"],
            vec!["--control-input-only"],
        ] {
            let args = base.into_iter().chain(extra);
            assert!(parse(None, args.map(Into::into)).is_err());
        }
        assert!(parse(None, ["--project-create", "{}"].into_iter().map(Into::into)).is_err());
        assert!(parse(None, ["--control", "9876", "--data-dir", "relative", "--project-create", "{}"].into_iter().map(Into::into)).is_err());
        for raw in [String::new(), "x".repeat(16385)] {
            assert!(parse(None, ["--control", "9876", "--data-dir", "/generated/new", "--project-create", &raw].into_iter().map(Into::into)).is_err());
        }
    }
    #[test]
    fn managed_save_startup_keeps_one_explicit_absolute_original_manager_and_no_implicit_project_fallback() {
        let good =
            ["--control", "9876", "--data-dir", "/generated/state", "--no-recover", "--poema-save-root", "/generated/workspace.json", "/generated/Root.fcproj"];
        let Cli::Run(parsed) = parse(None, good.into_iter().map(Into::into)).unwrap() else { panic!("run expected") };
        assert_eq!(parsed.poema_save_root, Some(PathBuf::from("/generated/workspace.json")));
        assert_eq!(parsed.files, vec!["/generated/Root.fcproj"]);
        for invalid in [
            vec!["--poema-save-root"],
            vec!["--poema-save-root", "relative.json"],
            vec!["--control", "9876", "--no-recover", "--poema-save-root", "/generated/workspace.json", "/generated/Root.fcproj"],
            vec![
                "--control",
                "9876",
                "--data-dir",
                "/generated/state",
                "--no-recover",
                "--poema-save-root",
                "/generated/../workspace.json",
                "/generated/Root.fcproj",
            ],
            vec![
                "--control",
                "9876",
                "--data-dir",
                "/generated/state",
                "--no-recover",
                "--poema-save-root",
                "/generated/workspace.json",
                "--poema-save-root",
                "/generated/workspace.json",
                "/generated/Root.fcproj",
            ],
            vec![
                "--control",
                "9876",
                "--data-dir",
                "/generated/state",
                "--no-recover",
                "--poema-save-root",
                "/generated/workspace.json",
                "/generated/Root.mov",
            ],
        ] {
            assert!(parse(None, invalid.into_iter().map(Into::into)).is_err());
        }
    }
}
