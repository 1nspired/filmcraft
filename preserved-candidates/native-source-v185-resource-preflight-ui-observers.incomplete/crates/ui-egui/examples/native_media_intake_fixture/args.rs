// Generated intake fixture layouts only; no editor/startup authority.
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FixtureArgs {
    pub(crate) persistent: bool,
    pub(crate) isolated_project: bool,
    pub(crate) creating: bool,
    pub(crate) recovering: bool,
    pub(crate) resume: bool,
    pub(crate) root: Option<String>,
}

// args_os avoids the standard Unicode-conversion panic before validation.
pub(crate) fn parse_os(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<FixtureArgs, &'static str> {
    let mut values = Vec::new();
    for arg in args {
        if values.len() >= 6 || arg.len() > 4096 {
            return Err("bounded generated fixture arguments required");
        }
        values.push(arg.into_string().map_err(|_| "generated fixture arguments must be valid Unicode")?);
    }
    parse(values)
}

pub(crate) fn parse(mut args: Vec<String>) -> Result<FixtureArgs, &'static str> {
    if args.len() > 6 || args.iter().any(|arg| arg.len() > 4096 || arg.chars().any(char::is_control)) {
        return Err("bounded generated fixture arguments required");
    }
    let persistent = args.last().is_some_and(|v| v == "--persistent");
    if persistent {
        args.pop();
    }
    let isolated_project = args.last().is_some_and(|v| v == "--isolated-project");
    if isolated_project {
        args.pop();
    }
    let recovering = args == ["--project-recovery"];
    let creating = args == ["--project-create"];
    let resume = args.len() == 3 && args.get(2).is_some_and(|v| v == "--resume");
    if !creating && !recovering && (!(args.len() == 2 || resume) || args.first().is_none_or(|v| v != "--fixture-root")) {
        return Err("choose generated --fixture-root, --project-create or --project-recovery mode");
    }
    if creating && isolated_project {
        return Err("project creation retains its original generated Root layout");
    }
    Ok(FixtureArgs { persistent, isolated_project, creating, recovering, resume, root: if creating || recovering { None } else { args.get(1).cloned() } })
}

pub(crate) fn project_path(root: &Path, creating: bool, isolated_project: bool) -> PathBuf {
    root.join(if creating {
        "generated.fcproj"
    } else if isolated_project {
        "generated-project/generated-intake.fcproj"
    } else {
        "generated-intake.fcproj"
    })
}

fn canonical_directory(directory: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || std::fs::canonicalize(directory)? != directory {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "generated project parent is redirected"));
    }
    Ok(())
}

pub(crate) fn prepare_isolated_directory(root: &Path, resume: bool) -> io::Result<()> {
    canonical_directory(root)?;
    let directory = root.join("generated-project");
    if !resume {
        // Fresh creation never adopts an existing folder, even if it is empty.
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
    }
    canonical_directory(&directory)
}

pub(crate) fn validate_recovery_path(root: &Path, saved: &str, isolated_project: bool) -> io::Result<()> {
    let selected = Path::new(saved);
    if saved.len() > 4096 || saved.chars().any(char::is_control) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bounded generated recovery filename required"));
    }
    let isolated = project_path(root, false, true);
    let direct = [project_path(root, true, false), project_path(root, false, false)];
    let exact = |candidate: &Path| candidate.to_str() == Some(saved);
    if !exact(&isolated) && (!direct.iter().any(|p| exact(p)) || isolated_project) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "recovery must use the exact marked generated document"));
    }
    canonical_directory(root)?;
    let parent = selected.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "generated recovery parent required"))?;
    canonical_directory(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }
    fn root() -> Result<PathBuf, Box<dyn std::error::Error>> {
        // The parent driver must declare scratch explicitly; never use temp_dir.
        let parent = PathBuf::from(std::env::var("POEMA_INTAKE_ARGS_TEST_ROOT")?);
        canonical_directory(&parent)?;
        let root = parent.join(format!("intake-layout-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&root)?;
        Ok(root)
    }
    #[test]
    fn original_root_resume_persistence_and_startup_modes_are_preserved() {
        for persistent in [false, true] {
            for mode in ["--project-create", "--project-recovery"] {
                let mut input = args(&[mode]);
                if persistent {
                    input.push("--persistent".into());
                }
                let value = parse(input).unwrap();
                assert_eq!(value.persistent, persistent);
                assert_eq!(value.creating, mode == "--project-create");
                assert_eq!(value.recovering, mode == "--project-recovery");
                assert_eq!(value.root, None);
            }
            for resume in [false, true] {
                let mut input = args(&["--fixture-root", "/generated/root"]);
                if resume {
                    input.push("--resume".into());
                }
                if persistent {
                    input.push("--persistent".into());
                }
                let value = parse(input).unwrap();
                assert_eq!(value.resume, resume);
                assert_eq!(value.persistent, persistent);
                assert_eq!(value.root.as_deref(), Some("/generated/root"));
                assert!(!value.isolated_project);
            }
        }
    }
    #[test]
    fn declared_isolated_root_and_resume_flags_match_host_argument_order() {
        for resume in [false, true] {
            for persistent in [false, true] {
                let mut input = args(&["--fixture-root", "/generated/root"]);
                if resume {
                    input.push("--resume".into());
                }
                input.push("--isolated-project".into());
                if persistent {
                    input.push("--persistent".into());
                }
                let value = parse(input).unwrap();
                assert!(value.isolated_project);
                assert_eq!(value.resume, resume);
                assert_eq!(value.persistent, persistent);
            }
        }
        assert!(parse(args(&["--project-create", "--isolated-project"])).is_err());
        assert!(parse(args(&["--project-recovery", "--isolated-project", "--persistent"])).unwrap().recovering);
    }
    #[test]
    fn malformed_duplicate_reordered_and_oversized_arguments_refuse_without_panicking() {
        let bad = vec![
            vec![],
            args(&["--fixture-root"]),
            args(&["--fixture-root", "/root", "--unknown"]),
            args(&["--fixture-root", "/root", "--persistent", "--isolated-project"]),
            args(&["--fixture-root", "/root", "--isolated-project", "--isolated-project"]),
            args(&["--fixture-root", "/root", "--resume", "--resume"]),
            args(&["--project-create", "--resume"]),
            args(&["--project-recovery", "extra"]),
            args(&["--fixture-root", "bad\nroot"]),
            vec!["--fixture-root".into(), "x".repeat(4097)],
            vec!["--persistent".into(); 7],
        ];
        for input in bad {
            let result = std::panic::catch_unwind(|| parse(input));
            assert!(result.is_ok());
            assert!(result.unwrap().is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn non_unicode_and_unbounded_os_arguments_refuse_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
        let result = std::panic::catch_unwind(|| parse_os(["--fixture-root".into(), invalid]));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        assert!(parse_os(std::iter::repeat_with(|| "--persistent".into())).is_err());
        assert_eq!(parse_os(["--project-create".into()]).unwrap(), parse(vec!["--project-create".into()]).unwrap());
    }
    #[test]
    fn project_paths_preserve_default_and_only_add_the_named_child_layout() {
        let root = Path::new("/generated/root");
        assert_eq!(project_path(root, true, false), root.join("generated.fcproj"));
        assert_eq!(project_path(root, false, false), root.join("generated-intake.fcproj"));
        assert_eq!(project_path(root, false, true), root.join("generated-project/generated-intake.fcproj"));
    }
    #[test]
    fn isolated_creation_is_private_and_never_overwrites_an_existing_folder() -> Result<(), Box<dyn std::error::Error>> {
        let root = root()?;
        prepare_isolated_directory(&root, false)?;
        let file = root.join("generated-project/later-work.keep");
        std::fs::write(&file, b"generated later work")?;
        assert!(prepare_isolated_directory(&root, false).is_err());
        prepare_isolated_directory(&root, true)?;
        assert_eq!(std::fs::read(&file)?, b"generated later work");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(root.join("generated-project"))?.permissions().mode() & 0o777, 0o700);
        }
        Ok(())
    }
    #[test]
    fn default_and_isolated_recovery_require_exact_existing_canonical_parents() -> Result<(), Box<dyn std::error::Error>> {
        let root = root()?;
        for name in ["generated.fcproj", "generated-intake.fcproj"] {
            validate_recovery_path(&root, root.join(name).to_str().ok_or("path")?, false)?;
        }
        assert!(validate_recovery_path(&root, root.join("generated-project/generated-intake.fcproj").to_str().ok_or("path")?, false).is_err());
        prepare_isolated_directory(&root, false)?;
        let nested = root.join("generated-project/generated-intake.fcproj");
        validate_recovery_path(&root, nested.to_str().ok_or("path")?, false)?;
        validate_recovery_path(&root, nested.to_str().ok_or("path")?, true)?;
        assert!(validate_recovery_path(&root, root.join("generated-intake.fcproj").to_str().ok_or("path")?, true).is_err());
        for bad in [root.join("other.fcproj"), root.join("generated-project/../generated-intake.fcproj"), root.join("other/generated-intake.fcproj")] {
            assert!(validate_recovery_path(&root, bad.to_str().ok_or("path")?, false).is_err());
        }
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn symlinked_or_file_project_parents_refuse_without_touching_later_work() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        let first = root()?;
        let target = root()?;
        let sentinel = target.join("later.keep");
        std::fs::write(&sentinel, b"preserve generated bytes")?;
        symlink(&target, first.join("generated-project"))?;
        assert!(prepare_isolated_directory(&first, false).is_err());
        assert!(prepare_isolated_directory(&first, true).is_err());
        assert!(validate_recovery_path(&first, first.join("generated-project/generated-intake.fcproj").to_str().ok_or("path")?, false).is_err());
        assert_eq!(std::fs::read(&sentinel)?, b"preserve generated bytes");
        let second = root()?;
        std::fs::write(second.join("generated-project"), b"retain generated file")?;
        assert!(prepare_isolated_directory(&second, true).is_err());
        assert_eq!(std::fs::read(second.join("generated-project"))?, b"retain generated file");
        Ok(())
    }
}
