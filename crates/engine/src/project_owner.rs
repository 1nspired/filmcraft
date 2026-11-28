//! Cooperative local-file ownership for Poema-managed native projects.
//! The persistent sidecar is never unlinked; the OS lock is released by Drop/crash.
//! It is not authentication or protection against uncooperative external writers.
use crate::{EngineError, Result, Session};
use filmcraft_project::Project;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(unix)]
use std::fs::{File, OpenOptions};

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("managed project ownership: {message}"))
}
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(std::io::Error::other("unresolved managed project symlink"));
            }
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = path.file_name().ok_or_else(|| std::io::Error::other("project filename required"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}
fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".poema-owner.lock");
    PathBuf::from(name)
}
fn marked(project: &Project) -> bool {
    project.items.values().any(|item| item.metadata.contains_key("poema.editorial.v1"))
}
fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}
pub(crate) fn required(project: &Project, path: &str) -> Result<bool> {
    if marked(project) {
        return Ok(true);
    }
    match canonical(Path::new(path)) {
        Ok(path) => present(&sidecar(&path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}
#[derive(Debug)]
struct LeaseWrites {
    epoch: u64,
    freeze: Option<String>,
}
#[derive(Debug)]
pub(crate) struct ProjectLease {
    id: String,
    path: PathBuf,
    #[cfg(unix)]
    lock_path: PathBuf,
    #[cfg(unix)]
    file: File,
    project_present: AtomicBool,
    writes: Mutex<LeaseWrites>,
}
impl ProjectLease {
    fn acquire(path: &str) -> Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(error("managed file leases are not proved on this platform"))
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let path = canonical(Path::new(path)).map_err(error)?;
            match fs::metadata(&path) {
                Ok(meta) if !meta.is_file() || meta.nlink() != 1 => return Err(error("managed project must be a regular file without hardlink aliases")),
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(error(e)),
                _ => {}
            }
            let lock_path = sidecar(&path);
            if present(&lock_path)? && !fs::symlink_metadata(&lock_path).map_err(error)?.file_type().is_file() {
                return Err(error("owner sidecar must be a regular file, never a symlink"));
            }
            let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path).map_err(error)?;
            file.try_lock().map_err(|e| error(format!("another owner holds this project, or locking failed: {e}")))?;
            let project_present = AtomicBool::new(path.exists());
            let lease = Self {
                id: crate::agent_bridge::fresh_owner(),
                path,
                lock_path,
                file,
                project_present,
                writes: Mutex::new(LeaseWrites { epoch: 0, freeze: None }),
            };
            lease.validate()?;
            Ok(lease)
        }
    }
    pub(crate) fn path(&self) -> Result<&str> {
        self.path.to_str().ok_or_else(|| error("managed filename is not UTF-8"))
    }
    fn matches(&self, path: &str) -> Result<bool> {
        Ok(canonical(Path::new(path)).map_err(error)? == self.path)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        #[cfg(not(unix))]
        {
            Err(error("managed file leases are not proved on this platform"))
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = self.file.metadata().map_err(error)?;
            let current = fs::symlink_metadata(&self.lock_path).map_err(error)?;
            if !current.is_file() || current.dev() != held.dev() || current.ino() != held.ino() || held.nlink() != 1 {
                return Err(error("owner sidecar disappeared, was replaced, or has aliases; stop and reconcile"));
            }
            if canonical(&self.path).map_err(error)? != self.path {
                return Err(error("project path changed"));
            }
            match fs::metadata(&self.path) {
                Ok(meta) if meta.nlink() != 1 => return Err(error("hardlinked managed projects are refused; aliases cannot have separate owners")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !self.project_present.load(Ordering::Relaxed) => {}
                Err(e) => return Err(error(e)),
                _ => {}
            }
            Ok(())
        }
    }
    pub(crate) fn save(&self, bytes: &[u8]) -> Result<()> {
        self.save_at(bytes, None, None)
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.writes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).epoch
    }
    pub(crate) fn frozen(&self) -> bool {
        self.writes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).freeze.is_some()
    }
    pub(crate) fn save_background(&self, bytes: &[u8], epoch: u64) -> Result<()> {
        self.save_at(bytes, Some(epoch), None)
    }
    fn save_at(&self, bytes: &[u8], expected: Option<u64>, freeze: Option<String>) -> Result<()> {
        // Serialize the file write and freeze transition with in-flight autosaves.
        let mut writes = self.writes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if writes.freeze.is_some() || expected.is_some_and(|epoch| epoch != writes.epoch) {
            return Err(error("checkpoint is frozen or queued autosave basis expired"));
        }
        let next = writes.epoch.checked_add(1).ok_or_else(|| error("persistence epoch exhausted"))?;
        self.validate()?;
        filmcraft_format::atomic_write(&self.path, bytes).map_err(error)?;
        self.project_present.store(true, Ordering::Relaxed);
        if expected.is_none() {
            writes.epoch = next;
        }
        writes.freeze = freeze;
        Ok(())
    }
    fn release(&self, freeze: &str) -> Result<()> {
        self.validate()?;
        let mut writes = self.writes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if writes.freeze.as_deref() != Some(freeze) {
            return Err(error("foreign, expired or already released checkpoint freeze"));
        }
        writes.epoch = writes.epoch.checked_add(1).ok_or_else(|| error("persistence epoch exhausted"))?;
        writes.freeze = None;
        Ok(())
    }
}
pub(crate) fn target(s: &Session, project: &Project, path: &str, inherit: bool) -> Result<Option<Arc<ProjectLease>>> {
    let needed = if inherit && s.project_lease.is_some() { true } else { required(project, path)? };
    if !needed {
        return Ok(None);
    }
    if let Some(lease) = &s.project_lease {
        lease.validate()?;
        if lease.matches(path)? {
            return Ok(Some(lease.clone()));
        }
    }
    Ok(Some(Arc::new(ProjectLease::acquire(path)?)))
}
pub(crate) fn current(s: &Session) -> Result<()> {
    if let Some(lease) = &s.project_lease {
        lease.validate()?;
        let path = s.path.as_deref().ok_or_else(|| error("session/file binding is missing"))?;
        if !lease.matches(path)? {
            return Err(error("session/file binding changed"));
        }
    } else if let Some(path) = &s.path
        && required(&s.project, path)?
    {
        return Err(error("this session has no file lease; reopen through the owning project door"));
    }
    Ok(())
}

pub(crate) fn writable(s: &Session) -> Result<()> {
    current(s)?;
    if s.project_lease.as_ref().is_some_and(|lease| lease.frozen()) {
        return Err(error("native checkpoint is frozen for ownership publication; reconcile and explicitly release it"));
    }
    Ok(())
}

pub(crate) fn status(s: &mut Session, _: &serde_json::Value) -> Result<serde_json::Value> {
    current(s)?;
    Ok(
        serde_json::json!({"protocol":1,"owner":s.agent_bridge.owner,"path":s.path,"managed":s.project_lease.is_some(),"lease":s.project_lease.as_ref().map(|lease|&lease.id),"frozen":s.project_lease.as_ref().is_some_and(|lease|lease.frozen()),"dirty":s.is_dirty(),"revision":s.revision.to_string(),"schemaVersion":filmcraft_format::SCHEMA_VERSION}),
    )
}

/// One native command guards the exact basis, validates and saves it, then
/// returns the digest of those exact bytes. No read/save race with human edits.
pub(crate) fn checkpoint(s: &mut Session, p: &serde_json::Value) -> Result<serde_json::Value> {
    checkpoint_impl(s, p, false)
}
pub(crate) fn freeze_checkpoint(s: &mut Session, p: &serde_json::Value) -> Result<serde_json::Value> {
    checkpoint_impl(s, p, true)
}
pub(crate) fn release_checkpoint(s: &mut Session, p: &serde_json::Value) -> Result<serde_json::Value> {
    use crate::commands::bad;
    current(s)?;
    if p.as_object().is_none_or(|params| params.keys().any(|key| key != "freeze" && key != "lease")) {
        return Err(bad("poema.releaseCheckpoint", "release accepts only the original freeze and file lease"));
    }
    let lease = s.project_lease.as_ref().ok_or_else(|| bad("poema.releaseCheckpoint", "managed file owner required"))?;
    if p["lease"].as_str() != Some(lease.id.as_str()) {
        return Err(bad("poema.releaseCheckpoint", "file lease changed"));
    }
    lease.release(p["freeze"].as_str().ok_or_else(|| bad("poema.releaseCheckpoint", "freeze required"))?)?;
    let result = serde_json::json!({"protocol":1,"released":true,"owner":s.agent_bridge.owner,"lease":lease.id,"revision":s.revision.to_string()});
    crate::agent_bridge::invalidate_plans(s);
    Ok(result)
}
fn checkpoint_impl(s: &mut Session, p: &serde_json::Value, freeze: bool) -> Result<serde_json::Value> {
    use crate::commands::bad;
    use sha2::Digest;
    crate::agent_bridge::guard(s, p, "poema.checkpoint")?;
    current(s)?;
    if p.as_object().is_none_or(|params| params.keys().any(|key| key != "token" && key != "lease")) {
        return Err(bad("poema.checkpoint", "checkpoint accepts only the original token and file lease"));
    }
    let lease = s.project_lease.as_ref().ok_or_else(|| bad("poema.checkpoint", "a managed file owner is required"))?;
    if p["lease"].as_str() != Some(lease.id.as_str()) {
        return Err(bad("poema.checkpoint", "file lease changed; reconcile the owning file before checkpointing"));
    }
    crate::native_document::bounded_json(&s.project).map_err(|e| bad("poema.checkpoint", format!("checkpoint preflight: {e}")))?;
    for item in s.project.items.values() {
        if let filmcraft_project::ItemKind::Sequence(q) = &item.kind {
            q.check().map_err(|e| bad("poema.checkpoint", e))?;
        }
    }
    let view = s.project_view();
    let bytes = filmcraft_format::encode_with_view(&s.project, Some(&view), false);
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(bad("poema.checkpoint", "checkpoint exceeds 4MiB"));
    }
    let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    let freeze_id = freeze.then(crate::agent_bridge::fresh_owner);
    lease.save_at(&bytes, None, freeze_id.clone())?;
    s.saved_revision = s.revision;
    s.loaded_schema = filmcraft_format::SCHEMA_VERSION;
    let mut result = serde_json::json!({"protocol":1,"owner":s.agent_bridge.owner,"lease":lease.id,"path":lease.path()?,"revision":s.revision.to_string(),"sha256":digest,"bytes":bytes.len()});
    if let Some(id) = freeze_id {
        result["freeze"] = id.into();
        crate::agent_bridge::invalidate_plans(s);
    }
    Ok(result)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("fc-owner-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn one_owner_survives_atomic_save_and_symlink_aliases() {
        let dir = directory();
        let path = dir.join("project.fcproj");
        let lease = ProjectLease::acquire(path.to_str().unwrap()).unwrap();
        lease.save(b"first").unwrap();
        lease.save(b"second").unwrap();
        let alias = dir.join("alias.fcproj");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(ProjectLease::acquire(alias.to_str().unwrap()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"second");
        drop(lease);
        assert!(sidecar(&path).exists());
        let next = ProjectLease::acquire(alias.to_str().unwrap()).unwrap();
        next.validate().unwrap();
        drop(next);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_replaced_or_aliased_owner_and_hardlinked_project_refuse() {
        let dir = directory();
        let path = dir.join("project.fcproj");
        let lease = ProjectLease::acquire(path.to_str().unwrap()).unwrap();
        lease.save(b"original").unwrap();
        let hard = dir.join("hard.fcproj");
        fs::hard_link(&path, &hard).unwrap();
        assert!(lease.validate().is_err());
        assert!(ProjectLease::acquire(hard.to_str().unwrap()).is_err());
        fs::remove_file(&hard).unwrap();
        fs::rename(sidecar(&path), dir.join("old-lock")).unwrap();
        assert!(lease.save(b"missing lock").is_err());
        fs::write(sidecar(&path), b"").unwrap();
        assert!(lease.save(b"replaced lock").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        drop(lease);
        fs::remove_dir_all(dir).unwrap();
    }
    fn managed() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", serde_json::json!({})).unwrap();
        let sequence = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).items.get_mut(&sequence).unwrap().metadata.insert("poema.editorial.v1".into(), "{}".into());
        s
    }
    #[test]
    fn managed_lifecycle_refuses_a_second_owner_before_any_state_or_file_change() {
        let dir = directory();
        let path = dir.join("a.fcproj");
        let copy = dir.join("copy.fcproj");
        let mut a = managed();
        a.execute("file.saveAs", serde_json::json!({"path":path})).unwrap();
        let bytes = fs::read(&path).unwrap();
        let mut b = managed();
        let before = b.project.clone();
        let revision = b.revision;
        for command in ["file.open", "file.saveAs", "file.saveCopy"] {
            assert!(b.execute(command, serde_json::json!({"path":path})).is_err(), "{command}");
            assert!(Arc::ptr_eq(&before, &b.project));
            assert_eq!(b.revision, revision);
            assert!(b.path.is_none());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        a.execute("file.saveCopy", serde_json::json!({"path":copy})).unwrap();
        assert_eq!(a.path.as_deref(), path.to_str());
        b.execute("file.open", serde_json::json!({"path":copy})).unwrap();
        a.execute("file.open", serde_json::json!({"path":path})).unwrap();
        a.execute("file.newProject", serde_json::json!({})).unwrap();
        b.execute("file.open", serde_json::json!({"path":path})).unwrap();
        assert_eq!(filmcraft_format::decode(&fs::read(&path).unwrap()).unwrap().project, *b.project);
        drop(a);
        drop(b);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn lost_lease_fences_direct_edits_history_and_saves_without_touching_the_checkpoint() {
        let dir = directory();
        let path = dir.join("a.fcproj");
        let mut s = managed();
        s.execute("file.saveAs", serde_json::json!({"path":path})).unwrap();
        s.execute("sequence.addEdit", serde_json::json!({"seconds":1.5})).unwrap();
        let before = s.project.clone();
        let revision = s.revision;
        let depth = s.history.undo.len();
        let bytes = fs::read(&path).unwrap();
        fs::rename(sidecar(&path), dir.join("held-owner")).unwrap();
        assert!(s.execute("file.save", serde_json::json!({})).is_err());
        assert!(s.execute("sequence.addEdit", serde_json::json!({"seconds":2})).is_err());
        assert!(
            s.edit("direct", |p, _| {
                p.name = "lost".into();
                Ok(())
            })
            .is_err()
        );
        assert!(s.undo().is_none());
        assert!(s.redo().is_none());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(s);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn broken_project_symlink_and_symlinked_sidecar_are_not_followed_or_overwritten() {
        let dir = directory();
        let path = dir.join("a.fcproj");
        std::os::unix::fs::symlink(dir.join("missing.fcproj"), &path).unwrap();
        assert!(ProjectLease::acquire(path.to_str().unwrap()).is_err());
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        fs::remove_file(&path).unwrap();
        let outside = dir.join("untouched");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, sidecar(&path)).unwrap();
        assert!(ProjectLease::acquire(path.to_str().unwrap()).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn checkpoint_guards_revision_lease_and_parameters_and_preserves_undo_and_journal() {
        use serde_json::json;
        let dir = directory();
        let path = dir.join("checkpoint.fcproj");
        let mut s = managed();
        s.execute("file.saveAs", json!({"path":path})).unwrap();
        let old = s.execute("agent.snapshot", json!({})).unwrap();
        let owner = s.execute("poema.owner", json!({})).unwrap();
        let original = fs::read(&path).unwrap();
        s.execute("sequence.addEdit", json!({"seconds":1.5})).unwrap();
        assert!(s.execute("poema.checkpoint", json!({"token":old["token"],"lease":owner["lease"]})).is_err());
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        assert!(s.execute("poema.checkpoint", json!({"token":read["token"],"lease":"foreign"})).is_err());
        assert!(s.execute("poema.checkpoint", json!({"token":read["token"],"lease":owner["lease"],"path":dir.join("redirect.fcproj")})).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(s.is_dirty());
        let project = s.project.clone();
        let depth = s.history.undo.len();
        let revision = s.revision;
        let (_, events, _subscription) = s.command_journal.subscribe().unwrap();
        let result = s.execute("poema.checkpoint", json!({"token":read["token"],"lease":owner["lease"]})).unwrap();
        assert_eq!(result["revision"], revision.to_string());
        assert_eq!(result["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(filmcraft_format::decode(&fs::read(&path).unwrap()).unwrap().project, *project);
        assert!(Arc::ptr_eq(&project, &s.project));
        assert_eq!(s.history.undo.len(), depth);
        assert_eq!(s.revision, revision);
        assert!(!s.is_dirty());
        assert!(events.try_recv().is_err());
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.is_dirty());
        drop(s);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_file_lease_generation_changes_on_reopen_and_save_as_but_not_save_copy() {
        use serde_json::json;
        let dir = directory();
        let path = dir.join("a.fcproj");
        let mut s = managed();
        s.execute("file.saveAs", json!({"path":path})).unwrap();
        let first = s.execute("poema.owner", json!({})).unwrap();
        s.execute("file.saveCopy", json!({"path":dir.join("copy.fcproj")})).unwrap();
        assert_eq!(s.execute("poema.owner", json!({})).unwrap()["lease"], first["lease"]);
        s.execute("file.newProject", json!({})).unwrap();
        s.execute("file.open", json!({"path":path})).unwrap();
        let next = s.execute("poema.owner", json!({})).unwrap();
        assert_eq!(next["owner"], first["owner"]);
        assert_ne!(next["lease"], first["lease"]);
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        assert!(s.execute("poema.checkpoint", json!({"token":read["token"],"lease":first["lease"]})).is_err());
        s.execute("file.saveAs", json!({"path":dir.join("b.fcproj")})).unwrap();
        assert_ne!(s.execute("poema.owner", json!({})).unwrap()["lease"], next["lease"]);
        drop(s);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn frozen_checkpoint_fences_commands_direct_edits_history_and_foreign_release_without_an_edit() {
        use serde_json::json;
        let dir = directory();
        let path = dir.join("frozen.fcproj");
        let mut s = managed();
        s.execute("file.saveAs", json!({"path":path})).unwrap();
        s.execute("sequence.addEdit", json!({"seconds":1.5})).unwrap();
        let owner = s.execute("poema.owner", json!({})).unwrap();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let operations = json!([{"command":"timeline.trim","params":{"clip":basis["sequence"]["video"][0]["items"][0]["clip"],"edge":"out","deltaFrames":-1}}]);
        let old_preview = s.execute("agent.preview", json!({"token":basis["token"],"label":"Before freeze","operations":operations})).unwrap();
        let revision = s.revision;
        let project = s.project.clone();
        let depth = s.history.undo.len();
        let mark = s.undo_mark();
        let (_, events, _subscription) = s.command_journal.subscribe().unwrap();
        let frozen = s.execute("poema.freezeCheckpoint", json!({"token":basis["token"],"lease":owner["lease"]})).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(s.execute("poema.owner", json!({})).unwrap()["frozen"], true);
        assert!(s.execute("agent.document", json!({})).is_ok());
        for (command, params) in [
            ("file.newProject", json!({})),
            ("file.open", json!({"path":path})),
            ("file.save", json!({})),
            ("file.saveAs", json!({"path":dir.join("redirect.fcproj")})),
            ("file.saveCopy", json!({"path":dir.join("copy.fcproj")})),
            ("sequence.addEdit", json!({"seconds":1.0})),
            ("edit.undo", json!({})),
            ("agent.apply", json!({"token":basis["token"],"operations":[]})),
        ] {
            assert!(s.execute(command, params).is_err(), "{command}");
        }
        assert!(s.edit("Direct", |_, _| Ok(())).is_err());
        assert!(s.edit_merged("Direct", "same", |_, _| Ok(())).is_err());
        assert!(s.edit_group("Direct", |_| Ok(())).is_err());
        assert_eq!(s.undo(), None);
        assert_eq!(s.redo(), None);
        assert_eq!(s.revert_to(mark), 0);
        for params in [
            json!({"lease":owner["lease"],"freeze":"foreign"}),
            json!({"lease":"foreign","freeze":frozen["freeze"]}),
            json!({"lease":owner["lease"],"freeze":frozen["freeze"],"extra":true}),
        ] {
            assert!(s.execute("poema.releaseCheckpoint", params).is_err());
        }
        assert!(Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(events.try_recv().is_err());
        let during = s.execute("agent.snapshot", json!({})).unwrap();
        let release = json!({"lease":owner["lease"],"freeze":frozen["freeze"]});
        s.execute("poema.releaseCheckpoint", release.clone()).unwrap();
        assert!(s.execute("poema.releaseCheckpoint", release).is_err());
        for old in [basis, during] {
            assert!(crate::agent_bridge::guard(&s, &json!({"token":old["token"]}), "agent.apply").is_err());
        }
        assert_eq!(s.execute("poema.owner", json!({})).unwrap()["frozen"], false);
        let fresh = s.execute("agent.snapshot", json!({})).unwrap();
        let new_preview = s.execute("agent.preview", json!({"token":fresh["token"],"label":"After freeze","operations":operations})).unwrap();
        assert_ne!(old_preview["preview"], new_preview["preview"]);
        assert!(s.execute("agent.commitPreview", json!({"token":fresh["token"],"preview":old_preview["preview"]})).is_err());
        s.execute("agent.discardPreview", json!({"preview":new_preview["preview"]})).unwrap();
        assert!(s.execute("edit.undo", json!({})).is_ok());
        assert_eq!(events.try_recv().unwrap()["command"], "edit.undo");
        drop(s);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checkpoint_and_release_expire_queued_autosaves_even_after_the_freeze_ends() {
        let dir = directory();
        let lease = ProjectLease::acquire(dir.join("queued.fcproj").to_str().unwrap()).unwrap();
        lease.save(b"initial").unwrap();
        let before = lease.epoch();
        lease.save_at(b"frozen", None, Some("original-freeze".into())).unwrap();
        let during = lease.epoch();
        assert!(lease.save_background(b"queued before", before).is_err());
        assert!(lease.save_background(b"queued during", during).is_err());
        lease.release("original-freeze").unwrap();
        assert!(lease.save_background(b"queued before", before).is_err());
        assert!(lease.save_background(b"queued during", during).is_err());
        assert_eq!(fs::read(dir.join("queued.fcproj")).unwrap(), b"frozen");
        lease.save_background(b"fresh worker", lease.epoch()).unwrap();
        assert_eq!(fs::read(dir.join("queued.fcproj")).unwrap(), b"fresh worker");
        drop(lease);
        fs::remove_dir_all(dir).unwrap();
    }
}
