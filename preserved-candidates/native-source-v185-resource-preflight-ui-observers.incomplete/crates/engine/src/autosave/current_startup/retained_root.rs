//! Read-only custody of existing recovery members. Existing directories never
//! become OwnedDirectory values and unknown/partial data is never cleaned up.
use super::*;
use filmcraft_format::retained::preview_context::NodeState;
use std::collections::BTreeSet;

pub(in crate::autosave) enum PreparedRoot {
    Missing(Arc<DirectoryFrame>),
    Existing(Arc<RetainedRoot>),
}
pub(in crate::autosave) struct RetainedRoot {
    path: PathBuf,
    root: Arc<DirectoryFrame>,
    directories: Vec<(Arc<DirectoryFrame>, Vec<String>)>,
    nodes: Vec<NodeFrame>,
    sessions: Vec<PathBuf>,
    reserved: String,
    absent_session: NodeFrame,
}
fn entries(path: &Path, limit: usize, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<String>> {
    check()?;
    let mut result = Vec::new();
    for entry in fs::read_dir(path)? {
        check()?;
        let value = entry?.file_name().into_string().map_err(|_| io::Error::other("literal retained journal member required"))?;
        if result.len() >= limit {
            return Err(io::Error::other("retained journal namespace exceeds its original bound"));
        }
        result.push(value);
    }
    result.sort();
    Ok(result)
}
impl PreparedRoot {
    pub(in crate::autosave) fn current_before(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        match self {
            Self::Missing(frame) => frame.current(check),
            Self::Existing(root) => {
                root.current(check)?;
                root.absent_session.current(check)
            }
        }
    }
    pub(in crate::autosave) fn description(&self) -> Value {
        match self {
            Self::Missing(frame) => json!({"path":frame.record().path,"exists":false,"physical":null,"sessionAbsent":true}),
            Self::Existing(root) => {
                let pin = root.root.record();
                json!({"path":pin.path,"exists":true,"physical":{"dev":pin.anchor.identity.device,"ino":pin.anchor.identity.inode,"mode":pin.anchor.identity.mode.to_string(),"uid":pin.anchor.identity.uid.to_string()},"sessionAbsent":true})
            }
        }
    }
    pub(in crate::autosave) fn retained(&self) -> Option<Arc<RetainedRoot>> {
        match self {
            Self::Existing(root) => Some(root.clone()),
            _ => None,
        }
    }
    pub(in crate::autosave) fn capture(cfg: &AutosaveConfig, reserved: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<Self> {
        check()?;
        let path = recovery_root(&cfg.data_dir);
        let root = DirectoryFrame::capture(&path, check)?;
        if !root.exists() {
            if root.record().missing.len() != 1 {
                return Err(io::Error::other("actual configured native data directory required"));
            }
            return Ok(Self::Missing(root));
        }
        let before = crate::agent_bridge::native_recovery_startup_inventory(&cfg.data_dir, reserved).map_err(io::Error::other)?;
        let dirs: Vec<PathBuf> = serde_json::from_value(before["directories"].clone()).map_err(io::Error::other)?;
        if dirs.len() > 257 || dirs.first() != Some(&path) {
            return Err(io::Error::other("bounded original recovery directories required"));
        }
        let mut directories = Vec::new();
        let mut nodes = Vec::new();
        let mut observed = BTreeSet::new();
        let mut sessions = Vec::new();
        for dir in dirs {
            check()?;
            let frame = DirectoryFrame::capture(&dir, check)?;
            if !frame.exists() || dir != path && dir.parent() != Some(path.as_path()) {
                return Err(io::Error::other("retained directory escaped original recovery root"));
            }
            let names = entries(&dir, if dir == path { 257 } else { 5 }, check)?;
            if dir == path && names.iter().any(|n| n == reserved) {
                return Err(io::Error::other("reserved native journal name is already occupied"));
            }
            if dir != path {
                sessions.push(dir.clone());
            }
            if names.iter().any(|n| n == ".DS_Store") {
                let node = frame.node_bounded(".DS_Store", 64 * 1024, check)?;
                if !matches!(node.state(), NodeState::File(_)) {
                    return Err(io::Error::other("retained Finder metadata changed type"));
                }
                observed.insert(dir.join(".DS_Store"));
                nodes.push(node);
            }
            directories.push((frame, names));
        }
        let files = before["files"].as_array().ok_or_else(|| io::Error::other("original retained file inventory required"))?;
        if files.len() > 1025 {
            return Err(io::Error::other("original retained file inventory exceeds its bound"));
        }
        for file in files {
            check()?;
            let name = file["path"].as_str().ok_or_else(|| io::Error::other("original retained file path required"))?;
            let file_path = PathBuf::from(name);
            if !observed.insert(file_path.clone()) {
                continue;
            }
            let parent = file_path.parent().ok_or_else(|| io::Error::other("retained file parent missing"))?;
            let leaf = file_path.file_name().and_then(|v| v.to_str()).ok_or_else(|| io::Error::other("literal retained file required"))?;
            let bytes =
                file["bytes"].as_u64().filter(|n| *n <= 80 * 1024 * 1024).ok_or_else(|| io::Error::other("bounded original retained bytes required"))?;
            let frame = DirectoryFrame::capture(parent, check)?.node_bounded(leaf, bytes, check)?;
            let NodeState::File(value) = frame.state() else {
                return Err(io::Error::other("retained native file changed type"));
            };
            if value.bytes != bytes || file["sha256"] != value.sha256 {
                return Err(io::Error::other("retained native file changed during capture"));
            }
            nodes.push(frame);
        }
        let absent: Vec<PathBuf> = serde_json::from_value(before["absent"].clone()).map_err(io::Error::other)?;
        if absent.len() > 1025 {
            return Err(io::Error::other("original native absence inventory exceeds its bound"));
        }
        for file in absent {
            check()?;
            let parent = file.parent().ok_or_else(|| io::Error::other("retained absence parent missing"))?;
            let name = file.file_name().and_then(|v| v.to_str()).ok_or_else(|| io::Error::other("literal retained absence required"))?;
            let frame = DirectoryFrame::capture(parent, check)?.node_bounded(name, 0, check)?;
            if !matches!(frame.state(), NodeState::Absent) {
                return Err(io::Error::other("retained native absence was occupied"));
            }
            nodes.push(frame);
        }
        if crate::agent_bridge::native_recovery_startup_inventory(&cfg.data_dir, reserved).map_err(io::Error::other)? != before {
            return Err(io::Error::other("original native persistence inventory changed during capture"));
        }
        let absent_session = root.node_bounded(reserved, 0, check)?;
        if !matches!(absent_session.state(), NodeState::Absent) {
            return Err(io::Error::other("reserved native session appeared during capture"));
        }
        let retained = Arc::new(RetainedRoot { path, root, directories, nodes, sessions, reserved: reserved.into(), absent_session });
        retained.current(check)?;
        Ok(Self::Existing(retained))
    }
}
impl RetainedRoot {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(in crate::autosave) fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.current_with_preference(None, check)
    }
    pub(in crate::autosave) fn current_with_preference(
        &self,
        preference: Option<&crate::agent_bridge::ColdPreferenceContinuation>,
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        check()?;
        self.root.current(check)?;
        for (directory, before) in &self.directories {
            directory.current(check)?;
            let mut actual = entries(&directory.record().path, if directory.record().path == self.root.record().path { 258 } else { 5 }, check)?;
            if directory.record().path == self.root.record().path {
                actual.retain(|name| name != &self.reserved);
            }
            if &actual != before {
                return Err(io::Error::other("original recovery membership changed; retained state was preserved"));
            }
        }
        for node in &self.nodes {
            // Only the actual original Commit can continue its exact original
            // preference frame. Every other retained node stays unchanged.
            if preference.map(|p| p.current_node(node, check)).transpose().map_err(|e| io::Error::other(e.to_string()))?.unwrap_or(false) {
                continue;
            }
            node.current(check)?;
        }
        for session in &self.sessions {
            if !lock_is_stale(session) {
                return Err(io::Error::other("an original recovery journal has a live writer; cold startup remains held"));
            }
        }
        check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autosave::{AutoSavePrefs, Persistence, session_reservation::ColdJournalReservation};
    fn cfg(label: &str) -> AutosaveConfig {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let data = crate::temp_dir().join(format!("poema-cold-retained-{label}-{}-{n}", std::process::id()));
        fs::create_dir(&data).unwrap();
        AutosaveConfig::new(data)
    }
    fn old(cfg: &AutosaveConfig, name: &str) -> PathBuf {
        let p = recovery_root(&cfg.data_dir).join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn original_reserved_constructor_preserves_empty_partial_and_native_only_old_sessions_and_keeps_new_worker_gated() {
        let cfg = cfg("preserve");
        let empty = old(&cfg, "100-1");
        let partial = old(&cfg, "101-1");
        fs::write(partial.join(META), b"unfinished original metadata {").unwrap();
        fs::write(partial.join(crate::agent_bridge::NATIVE_JOURNAL_FILE), b"original native companion retained without a legacy snapshot").unwrap();
        let before = fs::metadata(recovery_root(&cfg.data_dir)).unwrap();
        let reserved = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let path: PathBuf = serde_json::from_value(reserved.description(&cfg).unwrap()["path"].clone()).unwrap();
        let mut p = Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reserved, &|| Ok(())).unwrap();
        assert_eq!(p.session_dir, path);
        assert!(p.startup_pending());
        assert!(p.last_sent.is_none());
        assert!(!p.thread.as_ref().unwrap().is_finished());
        let journal = p.current_image_journal(&|| Ok(())).unwrap();
        journal.current(&|| Ok(())).unwrap();
        assert!(empty.is_dir());
        assert_eq!(fs::read(partial.join(META)).unwrap(), b"unfinished original metadata {");
        assert_eq!(fs::read(partial.join(crate::agent_bridge::NATIVE_JOURNAL_FILE)).unwrap(), b"original native companion retained without a legacy snapshot");
        assert!(!partial.join(SNAPSHOT).exists());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 3);
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(recovery_root(&cfg.data_dir)).unwrap().ino(), before.ino());
        let second = File::options().read(true).write(true).open(path.join(LOCK)).unwrap();
        assert!(matches!(second.try_lock(), Err(fs::TryLockError::WouldBlock)));
        assert_eq!(fs::metadata(path.join(LOCK)).unwrap().mode() & 0o777, 0o600);
        p.stop_worker();
        assert!(journal.current(&|| Ok(())).is_err());
        drop(p);
        assert!(path.join(LOCK).is_file());
        assert_eq!(fs::read(path.join(LOCK)).unwrap(), b"");
        assert!(!path.join(SNAPSHOT).exists());
        second.try_lock().unwrap();
    }
    #[test]
    fn original_reserved_constructor_reuses_fresh_root_writer_without_cleanup_or_snapshot() {
        let cfg = cfg("missing");
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        assert_eq!(reservation.root_description()["exists"], false);
        let path: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let p = Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap();
        assert!(p.startup_pending());
        assert!(p.last_sent.is_none());
        p.current_image_journal(&|| Ok(())).unwrap();
        drop(p);
        assert!(path.join(LOCK).exists());
        assert!(!path.join(SNAPSHOT).exists());
    }
    #[test]
    fn original_recovery_live_lock_refuses_reserved_startup_and_leaves_every_old_member() {
        let cfg = cfg("live");
        let dir = old(&cfg, "100-1");
        let lock = File::options().create_new(true).read(true).write(true).open(dir.join(LOCK)).unwrap();
        lock.try_lock().unwrap();
        assert!(ColdJournalReservation::prepare(&cfg, &|| Ok(())).is_err());
        assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 1);
        assert_eq!(fs::read(dir.join(LOCK)).unwrap(), b"");
        drop(lock);
        assert!(ColdJournalReservation::prepare(&cfg, &|| Ok(())).is_ok());
    }
    #[test]
    fn original_same_byte_replacement_cannot_renew_retained_recovery_custody() {
        let cfg = cfg("replace");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(SNAPSHOT), b"original snapshot").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let held = cfg.data_dir.join("held-original");
        fs::rename(dir.join(SNAPSHOT), &held).unwrap();
        fs::write(dir.join(SNAPSHOT), b"original snapshot").unwrap();
        assert!(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(held).unwrap(), b"original snapshot");
    }
    #[test]
    fn a_recovery_root_appearing_after_reservation_is_never_adopted_as_original() {
        let cfg = cfg("appeared");
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let root = recovery_root(&cfg.data_dir);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("foreign"), b"later work").unwrap();
        assert!(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(root.join("foreign")).unwrap(), b"later work");
    }
    #[test]
    fn stop_after_empty_lock_creation_retains_partial_effects_and_every_original_session() {
        let cfg = cfg("stop");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(META), b"old metadata").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let stop = || {
            if target.join(LOCK).exists() { Err(io::Error::other("Stop after actual native lock creation")) } else { Ok(()) }
        };
        assert!(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &stop).is_err());
        assert!(target.join(LOCK).is_file());
        assert!(!target.join(SNAPSHOT).exists());
        assert_eq!(fs::read(dir.join(META)).unwrap(), b"old metadata");
    }
    #[test]
    fn unknown_partial_members_are_retained_without_creating_a_new_native_session() {
        let cfg = cfg("unknown");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join("unfinished.tmp"), b"unknown interrupted work").unwrap();
        assert!(ColdJournalReservation::prepare(&cfg, &|| Ok(())).is_err());
        assert_eq!(fs::read(dir.join("unfinished.tmp")).unwrap(), b"unknown interrupted work");
        assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 1);
    }

    #[test]
    fn bounded_retained_inspection_reduces_global_source_checks_and_keeps_the_same_original_constructor_gated() {
        use std::cell::Cell;
        fn construct(observed: bool) -> usize {
            let cfg = cfg(if observed { "observed-count" } else { "original-count" });
            let dir = old(&cfg, "100-1");
            fs::write(dir.join(META), b"original partial metadata").unwrap();
            let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
            let checks = Cell::new(0_usize);
            let source = || {
                checks.set(checks.get() + 1);
                Ok(())
            };
            let mut worker = if observed {
                Persistence::start_cold_reserved_with_inspection(&cfg, AutoSavePrefs::default(), reservation, &source, &|| Ok(())).unwrap()
            } else {
                Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &source).unwrap()
            };
            let count = checks.get();
            assert!(worker.startup_pending());
            assert!(worker.last_sent.is_none());
            assert_eq!(fs::read(dir.join(META)).unwrap(), b"original partial metadata");
            assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 2);
            assert_eq!(fs::read_dir(&worker.session_dir).unwrap().count(), 1);
            let journal = worker.current_image_journal(&|| Ok(())).unwrap();
            journal.current(&|| Ok(())).unwrap();
            worker.stop_worker();
            assert!(worker.session_dir.join(LOCK).is_file());
            assert!(!worker.session_dir.join(SNAPSHOT).exists());
            count
        }
        let original = construct(false);
        let observed = construct(true);
        assert!(observed > 0 && observed < original, "source guards: original={original}, observed={observed}");
    }

    #[test]
    fn source_loss_inside_a_pure_retained_read_refuses_before_any_new_session_effect() {
        use std::cell::Cell;
        let cfg = cfg("observed-source-loss");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(META), b"original metadata").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let current = Cell::new(true);
        let source = || if current.get() { Ok(()) } else { Err(io::Error::other("original source owner lost")) };
        let inspect = || {
            current.set(false);
            Ok(())
        };
        let error = Persistence::start_cold_reserved_with_inspection(&cfg, AutoSavePrefs::default(), reservation, &source, &inspect).err().unwrap();
        assert!(error.to_string().contains("original source owner lost"));
        assert!(!target.exists());
        assert_eq!(fs::read(dir.join(META)).unwrap(), b"original metadata");
        assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 1);
    }

    #[test]
    fn stopped_retained_inspection_keeps_original_data_without_starting_a_worker() {
        let cfg = cfg("observed-read-stop");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(META), b"original metadata").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let error = Persistence::start_cold_reserved_with_inspection(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(()), &|| {
            Err(io::Error::other("original bounded inspection stopped"))
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("original bounded inspection stopped"));
        assert!(!target.exists());
        assert_eq!(fs::read(dir.join(META)).unwrap(), b"original metadata");
    }

    #[test]
    fn source_stop_during_retained_read_after_lock_creation_preserves_the_actual_partial_effect() {
        use std::cell::Cell;
        let cfg = cfg("observed-late-stop");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(META), b"original metadata").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let current = Cell::new(true);
        let source = || if current.get() { Ok(()) } else { Err(io::Error::other("source stopped after lock publication")) };
        let inspect = || {
            if target.join(LOCK).exists() {
                current.set(false);
            }
            Ok(())
        };
        assert!(Persistence::start_cold_reserved_with_inspection(&cfg, AutoSavePrefs::default(), reservation, &source, &inspect).is_err());
        assert!(target.join(LOCK).is_file());
        assert!(!target.join(SNAPSHOT).exists());
        assert!(!target.join(META).exists());
        assert_eq!(fs::read(dir.join(META)).unwrap(), b"original metadata");
    }

    #[test]
    fn bounded_retained_inspection_still_refuses_a_same_byte_old_snapshot_replacement() {
        use std::cell::Cell;
        let cfg = cfg("observed-inode-drift");
        let dir = old(&cfg, "100-1");
        fs::write(dir.join(SNAPSHOT), b"original snapshot").unwrap();
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        let target: PathBuf = serde_json::from_value(reservation.description(&cfg).unwrap()["path"].clone()).unwrap();
        let held = cfg.data_dir.join("held-original-snapshot");
        let changed = Cell::new(false);
        let inspect = || {
            if !changed.replace(true) {
                fs::rename(dir.join(SNAPSHOT), &held)?;
                fs::write(dir.join(SNAPSHOT), b"original snapshot")?;
            }
            Ok(())
        };
        assert!(Persistence::start_cold_reserved_with_inspection(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(()), &inspect).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(&held).unwrap(), b"original snapshot");
        assert_eq!(fs::read(dir.join(SNAPSHOT)).unwrap(), b"original snapshot");
    }
}
