use super::*;
use crate::project_save::effects::{CallResult, DirectoryRole, Event, FileEffects};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn root() -> PathBuf {
    let d = crate::temp_dir().join(format!("preview-effect-service-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(d).unwrap()
}
fn names() -> Vec<String> {
    vec![format!("{}.mov", "a".repeat(32)), format!("{}.wav", "b".repeat(32))]
}
fn store(root: &Path) -> PreviewStore {
    std::fs::create_dir_all(root).unwrap();
    for n in names() {
        std::fs::write(root.join(&n), n.as_bytes()).unwrap();
    }
    std::fs::write(root.join("unindexed.keep"), b"not a preview").unwrap();
    let s = PreviewStore::default();
    s.set_dir(Some(root.into()));
    s
}
#[derive(Default)]
struct Trace {
    events: Vec<Event>,
    fail_rename: bool,
    fail_copy: bool,
    fail_remove: bool,
}
impl FileEffects for Trace {
    fn preview_rename(&mut self, a: &Path, b: &Path) -> std::io::Result<()> {
        if self.fail_rename {
            return Err(std::io::Error::other("generated rename failure"));
        }
        std::fs::rename(a, b)
    }
    fn preview_copy(&mut self, a: &Path, b: &Path) -> std::io::Result<u64> {
        if self.fail_copy {
            return Err(std::io::Error::other("generated copy failure"));
        }
        std::fs::copy(a, b)
    }
    fn preview_remove(&mut self, a: &Path) -> std::io::Result<()> {
        if self.fail_remove {
            return Err(std::io::Error::other("generated remove failure"));
        }
        std::fs::remove_file(a)
    }
    fn event(&mut self, e: Event) {
        self.events.push(e);
    }
}
fn contents(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    if !dir.is_dir() {
        return Default::default();
    }
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())
        })
        .collect()
}

// Frozen pre-refactor implementation, not a call to the new effects helper.
fn original_move(store: &PreviewStore, dir: PathBuf) {
    let old = store.dir();
    if old.as_deref() == Some(dir.as_path()) {
        return;
    }
    if let Some(old) = &old
        && (old.starts_with(crate::temp_dir()) || store.temp_root().is_some_and(|r| old.starts_with(r)))
    {
        let _ = std::fs::create_dir_all(&dir);
        for name in store.files.read().unwrap_or_else(|e| e.into_inner()).iter() {
            let (a, b) = (old.join(name), dir.join(name));
            if !b.exists() && std::fs::rename(&a, &b).is_err() {
                let _ = std::fs::copy(&a, &b).map(|_| std::fs::remove_file(&a));
            }
        }
    }
    store.set_dir(Some(dir));
}

#[test]
fn shared_preview_effects_match_frozen_original_moves_existing_destinations_and_ignored_directory_errors_without_losing_unindexed_files() {
    for kind in ["move", "existing", "bad-dir", "same"] {
        let d = root();
        let a = d.join("a-source");
        let b = d.join("b-source");
        let old = store(&a);
        let new = store(&b);
        let to_a = if kind == "same" { a.clone() } else { d.join("a-to") };
        let to_b = if kind == "same" { b.clone() } else { d.join("b-to") };
        if kind == "existing" {
            for p in [&to_a, &to_b] {
                std::fs::create_dir_all(p).unwrap();
                std::fs::write(p.join(&names()[0]), b"existing preview stays").unwrap();
            }
        }
        if kind == "bad-dir" {
            std::fs::write(&to_a, b"foreign destination file").unwrap();
            std::fs::write(&to_b, b"foreign destination file").unwrap();
        }
        original_move(&old, to_a.clone());
        let mut trace = Trace::default();
        new.move_to_with(to_b.clone(), &mut trace);
        assert_eq!(contents(&a), contents(&b));
        assert_eq!(contents(&to_a), contents(&to_b));
        assert_eq!(old.count(), new.count());
        assert_eq!(old.generation.load(Ordering::Acquire), new.generation.load(Ordering::Acquire));
        assert_eq!(new.dir(), Some(to_b.clone()));
        assert_eq!(std::fs::read(b.join("unindexed.keep")).unwrap(), b"not a preview");
        if kind == "bad-dir" {
            assert!(trace.events.iter().any(|e| matches!(e, Event::Directories { role: DirectoryRole::Previews, result: CallResult::ReturnedError(_), .. })));
            assert!(trace.events.iter().any(|e| matches!(e, Event::PreviewCopy { result: CallResult::ReturnedError(_), .. })));
        }
        if kind == "same" {
            assert!(trace.events.is_empty());
        }
    }
}
#[test]
fn shared_preview_effects_keep_original_rename_copy_remove_fallback_order_and_report_individually_ignored_failures() {
    for kind in ["rename", "copy", "remove"] {
        let d = root();
        let from = d.join("source");
        let to = d.join("to");
        let s = store(&from);
        let mut trace = Trace { fail_rename: true, fail_copy: kind == "copy", fail_remove: kind == "remove", ..Default::default() };
        s.move_to_with(to.clone(), &mut trace);
        assert_eq!(s.dir(), Some(to.clone()));
        let renames = trace.events.iter().filter(|e| matches!(e, Event::PreviewRename { result: CallResult::ReturnedError(_), .. })).count();
        let copies = trace.events.iter().filter(|e| matches!(e, Event::PreviewCopy { .. })).count();
        let removes = trace.events.iter().filter(|e| matches!(e, Event::PreviewRemove { .. })).count();
        assert_eq!(renames, 2);
        assert_eq!(copies, 2);
        assert_eq!(removes, if kind == "copy" { 0 } else { 2 });
        for name in names() {
            if kind == "copy" {
                assert!(from.join(name).exists());
            } else {
                assert!(to.join(&name).exists());
                assert_eq!(from.join(name).exists(), kind == "remove");
            }
        }
        if kind == "remove" {
            assert!(trace.events.iter().any(|e| matches!(e, Event::PreviewRemove { result: CallResult::ReturnedError(_), .. })));
        }
    }
}
