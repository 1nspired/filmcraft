//! Crash-safe file replacement.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The temp file a save of `path` goes through (same directory, so the rename never crosses file
/// systems). Hidden, and unique per process and call.
fn temp_path(path: &Path) -> PathBuf {
    let dir = parent_dir(path);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{name}.{}-{n}.tmp", std::process::id()))
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Replace `path` with `data` atomically and durably:
/// write `data` to a temp file in the same directory, `fsync` it, rename it over `path`, then
/// `fsync` the directory so the rename itself survives a power cut. If anything fails the temp file
/// is removed and `path` is untouched.
pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);
    let r = (|| {
        {
            // closed before the rename
            let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            write_staged_bytes(&mut f, data, &|| Ok(()))?;
        }
        fs::rename(&tmp, path)
    })();
    if let Err(e) = r {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_dir(&parent_dir(path));
    Ok(())
}

/// Shared original complete-byte staging and file fsync. Cancellation can
/// interrupt a retained stage without touching its destination. Ordinary human
/// atomic_write supplies its original unconditional policy.
pub(crate) fn write_staged_bytes(file: &mut File, data: &[u8], check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    check()?;
    for chunk in data.chunks(65536) {
        let mut offset = 0;
        while offset < chunk.len() {
            check()?;
            let rest = chunk.get(offset..).ok_or_else(|| io::Error::other("invalid staged write offset"))?;
            match file.write(rest) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "staged file stopped accepting bytes")),
                Ok(count) => offset = offset.checked_add(count).ok_or_else(|| io::Error::other("staged write count overflow"))?,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
    check()?;
    file.sync_all()?;
    check()
}

/// Best-effort directory fsync (persists the rename on POSIX; not possible on Windows, where
/// `MoveFileEx` with write-through semantics is already durable enough).
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("filmcraft-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn replaces_and_leaves_no_temp_files() {
        let d = tmpdir("replace");
        let p = d.join("a.fcproj");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn failed_write_keeps_old_file() {
        let d = tmpdir("fail");
        let p = d.join("a.fcproj");
        atomic_write(&p, b"good").unwrap();
        // Renaming a file over a non-empty directory fails: the original target must survive.
        let dir_target = d.join("sub");
        fs::create_dir_all(dir_target.join("x")).unwrap();
        assert!(atomic_write(&dir_target, b"bad").is_err());
        assert_eq!(fs::read(&p).unwrap(), b"good");
        let tmp_left = fs::read_dir(&d).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(tmp_left, 0);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn missing_directory_is_an_error() {
        let d = tmpdir("missing");
        assert!(atomic_write(&d.join("nope/a.fcproj"), b"x").is_err());
        fs::remove_dir_all(&d).unwrap();
    }
}
