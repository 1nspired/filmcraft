use super::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
const W: &str = "11111111-1111-4111-8111-111111111111";
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Scratch, Session, Value) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!("poema-root-recovery-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let scope = json!({"projectId":"p_recovery","workspaceId":W});
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Root","workspace":scope})).unwrap();
    let mut cfg = crate::autosave::AutosaveConfig::new(root.join("native"));
    cfg.journal_debounce = std::time::Duration::from_millis(20);
    s.start_autosave(cfg).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj").to_str().unwrap()})).unwrap();
    (Scratch(root), s, scope)
}
fn bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, p: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            let m = fs::symlink_metadata(e.path()).unwrap();
            if m.is_dir() {
                walk(root, &e.path(), out);
            } else if m.is_file() {
                out.insert(e.path().strip_prefix(root).unwrap().to_path_buf(), fs::read(e.path()).unwrap());
            } else if m.file_type().is_symlink() {
                out.insert(e.path().strip_prefix(root).unwrap().to_path_buf(), fs::read_link(e.path()).unwrap().to_string_lossy().as_bytes().to_vec());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn crashed(s: &mut Session) {
    s.sync_persistence();
    s.persistence.take().unwrap().simulate_crash();
}
#[test]
fn native_saved_byte_baseline_follows_real_open_save_and_revert_while_failed_writes_copies_and_new_projects_keep_exact_semantics() {
    let (dir, mut s, scope) = fixture();
    let path = s.path.clone().unwrap();
    let baseline = s.saved_file.clone().unwrap();
    assert_eq!(baseline.sha256, crate::saved_file::sha256(&fs::read(&path).unwrap()));
    s.execute("file.newSequence", json!({"name":"First"})).unwrap();
    let before = s.saved_file.clone();
    s.execute("file.saveCopy", json!({"path":dir.0.join("copy.fcproj").to_str().unwrap()})).unwrap();
    assert_eq!(s.saved_file, before);
    assert!(s.is_dirty());
    assert!(s.execute("file.saveAs", json!({"path":dir.0.to_str().unwrap()})).is_err());
    assert_eq!(s.saved_file, before);
    assert_eq!(s.path.as_deref(), Some(path.as_str()));
    s.execute("file.revert", json!({})).unwrap();
    assert_eq!(s.saved_file, Some(baseline));
    assert!(s.state.active_sequence.is_none());
    s.execute("file.saveCopy", json!({"path":path})).unwrap();
    assert!(s.saved_file.is_none());
    s.execute("file.save", json!({})).unwrap();
    assert!(s.saved_file.is_some());
    s.execute("file.newProject", json!({"name":"New","workspace":scope})).unwrap();
    assert!(s.saved_file.is_none());
    assert!(s.path.is_none());
    s.shutdown();
}
#[test]
fn native_autosave_current_project_provides_the_actual_written_byte_hash_and_stale_notifications_cannot_certify_later_saved_work() {
    let (_dir, mut s, _) = fixture();
    s.prefs.auto_save.save_current_project = true;
    s.persistence.as_ref().unwrap().set_prefs(s.prefs.auto_save.clone());
    s.execute("file.newSequence", json!({"name":"First"})).unwrap();
    let r = s.revision;
    s.persistence.as_ref().unwrap().auto_save_now().unwrap();
    s.apply_persistence_events();
    assert_eq!(s.saved_revision, r);
    let baseline = s.saved_file.clone().unwrap();
    assert_eq!(baseline.sha256, crate::saved_file::sha256(&fs::read(&baseline.path).unwrap()));
    s.execute("file.newSequence", json!({"name":"Second"})).unwrap();
    s.persistence.as_ref().unwrap().auto_save_now().unwrap();
    s.execute("file.newSequence", json!({"name":"Third"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    let saved = s.saved_revision;
    s.apply_persistence_events();
    assert_eq!(s.saved_revision, saved);
    assert!(s.saved_file.is_none(), "late worker write does not certify the current baseline");
    s.execute("file.save", json!({})).unwrap();
    assert!(s.saved_file.is_some());
    s.shutdown();
}
#[test]
fn native_recovery_basis_preserves_genuine_sequence_absence_and_refuses_a_live_original_even_without_a_journal() {
    let (dir, mut s, scope) = fixture();
    let project = s.project.clone();
    let revision = s.revision;
    let state = serde_json::to_value(&s.state).unwrap();
    let files = bytes(&dir.0);
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    assert!(basis["sequence"].is_null());
    assert!(basis["journals"].as_array().unwrap().is_empty());
    assert!(std::sync::Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(bytes(&dir.0), files);
    assert!(Session::validate_project_recovery_basis(&basis).is_err());
    s.shutdown();
    let after = bytes(&dir.0);
    Session::validate_project_recovery_basis(&basis).unwrap();
    assert_eq!(bytes(&dir.0), after);
}
#[test]
fn native_recovery_basis_accepts_only_the_source_journal_that_flushes_the_same_captured_graph_after_a_crash() {
    let (dir, mut s, scope) = fixture();
    s.execute("file.newSequence", json!({"name":"First"})).unwrap();
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    let project = s.project.clone();
    let source = s.persistence.as_ref().unwrap().session_dir.clone();
    crashed(&mut s);
    assert!(source.join("snapshot.fcproj").exists());
    let before = bytes(&dir.0);
    Session::validate_project_recovery_basis(&basis).unwrap();
    assert_eq!(bytes(&dir.0), before);
    assert_eq!(*s.project, *project);
    assert!(s.is_dirty());
}
#[test]
fn native_recovery_basis_refuses_later_human_unsaved_work_and_preserves_every_original_journal_byte() {
    let (dir, mut s, scope) = fixture();
    s.execute("file.newSequence", json!({"name":"First"})).unwrap();
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    s.execute("file.newSequence", json!({"name":"Later human"})).unwrap();
    crashed(&mut s);
    let before = bytes(&dir.0);
    let error = Session::validate_project_recovery_basis(&basis).unwrap_err().to_string();
    assert!(error.contains("later or unknown"), "{error}");
    assert_eq!(bytes(&dir.0), before);
}
#[test]
fn native_recovery_basis_refuses_changed_saved_bytes_and_identical_byte_replacements_without_adopting_a_disk_hash() {
    for identical in [false, true] {
        let (dir, mut s, scope) = fixture();
        let basis = s.capture_project_recovery_basis(&scope).unwrap();
        s.shutdown();
        let path = s.path.as_ref().unwrap();
        let original = fs::read(path).unwrap();
        let replacement = if identical { original.clone() } else { b"later human saved bytes".to_vec() };
        let tmp = dir.0.join("replacement");
        fs::write(&tmp, &replacement).unwrap();
        fs::rename(&tmp, path).unwrap();
        let before = bytes(&dir.0);
        assert!(Session::validate_project_recovery_basis(&basis).is_err());
        assert_eq!(bytes(&dir.0), before);
        if !identical {
            assert!(s.capture_project_recovery_basis(&scope).is_err());
        }
    }
}
#[test]
fn native_recovery_basis_refuses_another_live_owner_and_new_foreign_journals_without_cleaning_empty_directories() {
    let (dir, mut s, scope) = fixture();
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    let data = s.persistence.as_ref().unwrap().data_dir.clone();
    s.shutdown();
    let empty = crate::autosave::recovery_root(&data).join("1-2-3");
    fs::create_dir(&empty).unwrap();
    let before = bytes(&dir.0);
    Session::validate_project_recovery_basis(&basis).unwrap();
    assert!(empty.is_dir());
    assert_eq!(bytes(&dir.0), before);
    let mut other = Session::default();
    other.start_autosave(crate::autosave::AutosaveConfig::new(&data)).unwrap();
    let active = bytes(&dir.0);
    assert!(Session::validate_project_recovery_basis(&basis).is_err());
    assert_eq!(bytes(&dir.0), active);
    other.execute("file.newProject", json!({"name":"Other","workspace":{"projectId":"p_other","workspaceId":W}})).unwrap();
    other.execute("file.newSequence", json!({"name":"Other edit"})).unwrap();
    crashed(&mut other);
    let later = bytes(&dir.0);
    assert!(Session::validate_project_recovery_basis(&basis).is_err());
    assert_eq!(bytes(&dir.0), later);
}
#[test]
fn native_recovery_basis_refuses_incomplete_corrupt_and_oversized_recovery_inputs_without_repair_or_deletion() {
    for mode in ["missing_meta", "corrupt_meta", "oversized_meta", "changed_snapshot"] {
        let (dir, mut s, scope) = fixture();
        s.execute("file.newSequence", json!({"name":"First"})).unwrap();
        let basis = s.capture_project_recovery_basis(&scope).unwrap();
        let journal = s.persistence.as_ref().unwrap().session_dir.clone();
        crashed(&mut s);
        match mode {
            "missing_meta" => fs::remove_file(journal.join("session.json")).unwrap(),
            "corrupt_meta" => fs::write(journal.join("session.json"), b"{").unwrap(),
            "oversized_meta" => fs::write(journal.join("session.json"), vec![b' '; 65537]).unwrap(),
            "changed_snapshot" => fs::write(journal.join("snapshot.fcproj"), b"damaged snapshot").unwrap(),
            _ => unreachable!(),
        }
        let before = bytes(&dir.0);
        assert!(Session::validate_project_recovery_basis(&basis).is_err(), "{mode}");
        assert_eq!(bytes(&dir.0), before, "{mode}");
    }
}
#[cfg(unix)]
#[test]
fn native_recovery_basis_refuses_redirected_saved_files_journal_snapshots_and_persistence_roots_without_following_them() {
    use std::os::unix::fs::symlink;
    for mode in ["saved", "snapshot", "data"] {
        let (dir, mut s, scope) = fixture();
        s.execute("file.newSequence", json!({"name":"First"})).unwrap();
        let basis = s.capture_project_recovery_basis(&scope).unwrap();
        let journal = s.persistence.as_ref().unwrap().session_dir.clone();
        let data = s.persistence.as_ref().unwrap().data_dir.clone();
        crashed(&mut s);
        let path = match mode {
            "saved" => PathBuf::from(s.path.as_ref().unwrap()),
            "snapshot" => journal.join("snapshot.fcproj"),
            "data" => data,
            _ => unreachable!(),
        };
        let target = dir.0.join("preserved-target");
        fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        let before = bytes(&dir.0);
        assert!(Session::validate_project_recovery_basis(&basis).is_err());
        assert_eq!(bytes(&dir.0), before);
    }
}
#[test]
fn native_recovery_basis_capture_error_resumes_the_same_worker_and_refuses_invented_identity_paths_and_extra_authority_fields() {
    let (dir, mut s, scope) = fixture();
    let original = fs::read(s.path.as_ref().unwrap()).unwrap();
    fs::write(s.path.as_ref().unwrap(), b"external changed bytes").unwrap();
    assert!(s.capture_project_recovery_basis(&scope).is_err());
    s.execute("file.newSequence", json!({"name":"After refusal"})).unwrap();
    s.persistence.as_ref().unwrap().flush();
    assert!(s.persistence.as_ref().unwrap().session_dir.join("snapshot.fcproj").is_file(), "error must resume source persistence");
    fs::write(s.path.as_ref().unwrap(), &original).unwrap();
    let mut foreign = scope.clone();
    foreign["projectId"] = json!("p_foreign");
    assert!(s.capture_project_recovery_basis(&foreign).is_err());
    let mut extra = scope.clone();
    extra["writeAuthority"] = json!(true);
    assert!(s.capture_project_recovery_basis(&extra).is_err());
    let before = s.saved_file.take();
    assert!(s.capture_project_recovery_basis(&scope).is_err());
    s.saved_file = before;
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    crashed(&mut s);
    for change in ["version", "unknown", "epoch", "duplicate"] {
        let mut bad = basis.clone();
        match change {
            "version" => bad["version"] = json!(2),
            "unknown" => bad["restore"] = json!(true),
            "epoch" => bad["native_session"] = json!("invented"),
            "duplicate" => {
                let rows = bad["journals"].as_array_mut().unwrap();
                if let Some(row) = rows.first().cloned() {
                    rows.push(row);
                } else {
                    bad["revision"] = json!(0);
                }
            }
            _ => unreachable!(),
        }
        let all = bytes(&dir.0);
        assert!(Session::validate_project_recovery_basis(&bad).is_err(), "{change}");
        assert_eq!(bytes(&dir.0), all);
    }
}

#[test]
fn native_recovery_basis_retains_unrelated_existing_journals_but_refuses_their_later_replacement_and_ignores_only_regular_finder_metadata() {
    let (dir, mut s, scope) = fixture();
    let data = s.persistence.as_ref().unwrap().data_dir.clone();
    let recovery = crate::autosave::recovery_root(&data);
    let foreign = recovery.join("1-2-3");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("lock"), b"").unwrap();
    let project = filmcraft_project::Project::new("Existing unrelated recovery");
    let snapshot = filmcraft_format::encode(&project, false);
    fs::write(foreign.join("snapshot.fcproj"), &snapshot).unwrap();
    let mut meta = crate::autosave::JournalMeta { revision: 1, bytes: snapshot.len(), project_name: project.name.clone(), ..Default::default() };
    fs::write(foreign.join("session.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
    fs::write(recovery.join(".DS_Store"), b"Finder metadata").unwrap();
    let basis = s.capture_project_recovery_basis(&scope).unwrap();
    s.shutdown();
    let before = bytes(&dir.0);
    Session::validate_project_recovery_basis(&basis).unwrap();
    assert_eq!(bytes(&dir.0), before);
    assert!(foreign.join("snapshot.fcproj").is_file());
    meta.revision = 2;
    fs::write(foreign.join("session.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
    let changed = bytes(&dir.0);
    assert!(Session::validate_project_recovery_basis(&basis).is_err());
    assert_eq!(bytes(&dir.0), changed);
}
