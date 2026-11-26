//! Read-only process-directory custody. Absolute historical Save rows may
//! explicitly omit an old directory; that does not authorize an internal-drive
//! default. The fresh process must still be launched in an explicit scoped
//! directory. A recorded original directory, when present, must match exactly.
use super::*;

pub(super) struct NativeDirectory {
    frame: Arc<DirectoryFrame>,
    recorded: Option<PathBuf>,
}
impl NativeDirectory {
    pub(super) fn capture(recorded: &Value, roots: &[Root], check: &media_guard::Check) -> Result<Self> {
        let actual = std::env::current_dir().map_err(error)?;
        let result = Self::capture_at(recorded, roots, &actual, check)?;
        result.current(check)?;
        Ok(result)
    }
    fn capture_at(recorded: &Value, roots: &[Root], actual: &Path, check: &media_guard::Check) -> Result<Self> {
        check.check()?;
        if roots.is_empty() || roots.len() > 16 || !literal(actual) || !roots.iter().any(|root| actual.starts_with(&root.path)) {
            return Err(error("native process directory escapes explicit working roots; no default or fallback"));
        }
        let recorded = match recorded {
            Value::Null => None,
            Value::String(value) if literal(Path::new(value)) => Some(PathBuf::from(value)),
            _ => return Err(error("explicit literal or absent original native directory required")),
        };
        if recorded.as_ref().is_some_and(|path| path.as_os_str() != actual.as_os_str()) {
            return Err(error("native process directory differs from the exact original Save directory"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let frame = DirectoryFrame::capture(actual, &io).map_err(error)?;
        if !frame.record().missing.is_empty() || frame.record().path.as_os_str() != actual.as_os_str() {
            return Err(error("actual canonical existing native process directory required"));
        }
        let result = Self { frame, recorded };
        result.current_at(actual, check)?;
        Ok(result)
    }
    fn current_at(&self, actual: &Path, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        if actual.as_os_str() != self.frame.record().path.as_os_str() {
            return Err(error("native process directory changed while the recovered editor is held"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.frame.current(&io).map_err(error)?;
        check.check()
    }
    pub(super) fn current(&self, check: &media_guard::Check) -> Result<()> {
        let before = std::env::current_dir().map_err(error)?;
        self.current_at(&before, check)?;
        let after = std::env::current_dir().map_err(error)?;
        if before.as_os_str() != after.as_os_str() {
            return Err(error("native process directory changed during its current observation"));
        }
        self.current_at(&after, check)
    }
    pub(super) fn path(&self) -> PathBuf {
        self.frame.record().path
    }
    pub(super) fn recorded(&self) -> Option<&Path> {
        self.recorded.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    fn temp() -> PathBuf {
        let path =
            crate::temp_dir().join(format!("poema-native-cold-directory-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        path
    }
    fn check() -> media_guard::Check {
        media_guard::Check::new(web_time::Instant::now())
    }
    fn root(path: &Path) -> Root {
        let frame = DirectoryFrame::capture(path, &|| Ok(())).unwrap();
        let pin = frame.record().anchor.identity;
        Root { path: path.to_owned(), physical: DirectoryPhysical { dev: pin.device, ino: pin.inode, mode: pin.mode.to_string(), uid: pin.uid.to_string() } }
    }
    #[test]
    fn native_cold_directory_keeps_exact_current_process_context_and_explicit_unknown_original_cwd_without_changing_it() {
        let actual = std::env::current_dir().unwrap();
        let roots = [root(&actual)];
        for original in [Value::Null, serde_json::to_value(&actual).unwrap()] {
            let held = NativeDirectory::capture(&original, &roots, &check()).unwrap();
            held.current(&check()).unwrap();
            assert_eq!(held.path(), actual);
            assert_eq!(held.recorded().is_none(), original.is_null());
            assert_eq!(std::env::current_dir().unwrap(), actual);
        }
    }
    #[test]
    fn native_cold_directory_refuses_unscoped_nonliteral_aliased_and_different_contexts_without_internal_fallback() {
        let actual = std::env::current_dir().unwrap();
        let roots = [root(&actual)];
        let temp = temp();
        for original in [
            json!(true),
            json!({}),
            json!([]),
            json!("relative"),
            json!(format!("{}/.", actual.display())),
            json!(temp.as_path()),
            json!("/missing/original-native-directory"),
        ] {
            assert!(NativeDirectory::capture(&original, &roots, &check()).is_err());
            assert_eq!(std::env::current_dir().unwrap(), actual);
        }
        assert!(NativeDirectory::capture(&Value::Null, &[], &check()).is_err());
        assert!(NativeDirectory::capture(&Value::Null, &[root(temp.as_path())], &check()).is_err());
        assert_eq!(std::env::current_dir().unwrap(), actual);
    }
    #[test]
    fn native_cold_directory_refuses_replaced_and_redirected_generations_while_preserving_both_original_and_later_work() {
        let temp = temp();
        let actual = temp.as_path().join("native");
        let old = temp.as_path().join("retained-original");
        fs::create_dir(&actual).unwrap();
        fs::write(actual.join("keep"), b"original generated work").unwrap();
        let held = NativeDirectory::capture_at(&Value::Null, &[root(temp.as_path())], &actual, &check()).unwrap();
        held.current_at(&actual, &check()).unwrap();
        fs::rename(&actual, &old).unwrap();
        fs::create_dir(&actual).unwrap();
        fs::write(actual.join("keep"), b"later generated work").unwrap();
        assert!(held.current_at(&actual, &check()).is_err());
        assert_eq!(fs::read(old.join("keep")).unwrap(), b"original generated work");
        assert_eq!(fs::read(actual.join("keep")).unwrap(), b"later generated work");
        let other = temp.as_path().join("redirected");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&old, &other).unwrap();
            assert!(NativeDirectory::capture_at(&Value::Null, &[root(temp.as_path())], &other, &check()).is_err());
            assert_eq!(fs::read(old.join("keep")).unwrap(), b"original generated work");
        }
    }
}
