use super::*;
const W: &str = "11111111-1111-4111-8111-111111111111";
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Scratch, Session) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let directory = crate::temp_dir().join(format!("poema-full-journal-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    fs::create_dir(&directory).unwrap();
    let directory = fs::canonicalize(directory).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Native journal","workspace":{"projectId":"p_journal","workspaceId":W}})).unwrap();
    let mut cfg = crate::autosave::AutosaveConfig::new(directory.join("native"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    s.start_autosave(cfg).unwrap();
    s.execute("file.saveAs", json!({"path":directory.join("Root.fcproj").to_str().unwrap()})).unwrap();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    (Scratch(directory), s)
}
fn assistant(s: &mut Session) {
    let token = "a".repeat(64);
    let scope = json!({"projectId":"p_journal","workspaceId":W});
    let read = s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path})).unwrap();
    s.execute("agent.apply",json!({"token":token,"label":"Actual owned bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope,"beforeSha256":read["binState"]["sha256"],"edit":{"type":"create","name":"Assistant bin","parent":null}}}]})).unwrap();
}
fn flush(s: &mut Session) -> PathBuf {
    s.sync_persistence();
    let dir = s.persistence.as_ref().unwrap().session_dir.clone();
    s.persistence.as_ref().unwrap().flush();
    s.apply_persistence_events();
    dir
}
#[test]
fn full_native_journal_captures_immutable_graph_and_exact_editor_on_editor_but_serializes_independently_of_later_work() {
    let (_d, mut s) = setup();
    assistant(&mut s);
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    let pending = capture(&mut s).unwrap().unwrap();
    let project = filmcraft_format::encode(&s.project, false);
    let rev = s.revision;
    s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    let raw = pending.encode(&project).unwrap();
    let e: Envelope = serde_json::from_slice(&raw).unwrap();
    assert_eq!(e.payload.history.revision, rev);
    assert_eq!(e.payload.editor.editor.source_playhead.0, 9_007_199_254_740_999);
    assert_eq!(e.payload.history.undo.len(), 1);
    assert_eq!(e.payload.history.tickets.len(), 1);
    assert!(e.payload.history.owned_head);
    validate(&raw, &project).unwrap();
    assert!(validate(&raw, &filmcraft_format::encode(&s.project, false)).is_err());
    s.shutdown();
}
#[test]
fn full_native_journal_worker_keeps_complete_owned_and_human_history_without_assigning_later_human_work_to_assistant() {
    let (_d, mut s) = setup();
    assistant(&mut s);
    let before = fs::read(s.path.as_ref().unwrap()).unwrap();
    let dir = flush(&mut s);
    let original: Envelope = serde_json::from_slice(&fs::read(dir.join(FILE)).unwrap()).unwrap();
    assert!(original.payload.history.owned_head);
    s.execute("file.newBin", json!({"name":"Human bin"})).unwrap();
    flush(&mut s);
    let raw = fs::read(dir.join(FILE)).unwrap();
    let current: Envelope = serde_json::from_slice(&raw).unwrap();
    assert_eq!(current.payload.history.undo.len(), 2);
    assert_eq!(current.payload.history.tickets.len(), 1);
    assert!(!current.payload.history.owned_head);
    assert_eq!(current.payload.history.revision, s.revision);
    validate(&raw, &fs::read(dir.join("snapshot.fcproj")).unwrap()).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    flush(&mut s);
    let undone: Envelope = serde_json::from_slice(&fs::read(dir.join(FILE)).unwrap()).unwrap();
    assert_eq!(undone.payload.history.redo.len(), 1);
    assert_eq!(undone.payload.history.tickets.len(), 1);
    assert!(!undone.payload.history.owned_head);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o077, 0);
    }
    s.shutdown();
}
#[test]
fn full_native_journal_rejects_crossed_and_damaged_exact_graph_editor_scope_receipts_and_byte_bounds() {
    let (_d, mut s) = setup();
    assistant(&mut s);
    let pending = capture(&mut s).unwrap().unwrap();
    let project = filmcraft_format::encode(&s.project, false);
    let raw = pending.encode(&project).unwrap();
    let mut damaged = raw.clone();
    damaged[0] = b'!';
    assert!(validate(&damaged, &project).is_err());
    for case in 0..8 {
        let mut e: Envelope = serde_json::from_slice(&raw).unwrap();
        match case {
            0 => e.payload.saved_file.path.push_str("-foreign"),
            1 => e.payload.history.undo[0].project = 999,
            2 => e.payload.editor.saved_revision = u64::MAX,
            3 => e.payload.history.tickets[0].result["authority"] = json!("media"),
            4 => e.payload.history.owned_head = true,
            5 => {
                e.payload.history.scope =
                    CheckpointScope::Project(crate::project_workspace::ProjectScope { project_id: "p_foreign".into(), workspace_id: W.into() })
            }
            6 => e.payload.history.revision = 0,
            _ => e.payload.schema = u32::MAX,
        }
        if case == 4 {
            e.payload.history.tickets[0].after = 999;
        }
        e.payload.history_sha256 = digest(&bytes(&e.payload.history, MAX_BYTES).unwrap());
        e.sha256 = digest(&bytes(&e.payload, LIMIT).unwrap());
        assert!(validate(&bytes(&e, LIMIT).unwrap(), &project).is_err(), "case{case}");
    }
    assert!(validate(&vec![0; LIMIT + 1], &project).is_err());
    s.shutdown();
}
#[test]
fn full_native_journal_write_refuses_another_owner_nonregular_or_nonprivate_companion_and_preserves_existing_bytes() {
    let (d, mut s) = setup();
    assistant(&mut s);
    let pending = capture(&mut s).unwrap().unwrap();
    let project = filmcraft_format::encode(&s.project, false);
    let dir = flush(&mut s);
    let raw = fs::read(dir.join(FILE)).unwrap();
    assert!(pending.write(&d.0, &project).is_err());
    assert_eq!(fs::read(dir.join(FILE)).unwrap(), raw);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join(FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(pending.write(&dir, &project).is_err());
        assert_eq!(fs::read(dir.join(FILE)).unwrap(), raw);
        fs::set_permissions(dir.join(FILE), fs::Permissions::from_mode(0o600)).unwrap();
    }
    s.shutdown();
}
#[test]
fn full_native_journal_worker_reports_companion_failure_and_keeps_the_legacy_project_without_silent_success() {
    let (_d, mut s) = setup();
    assistant(&mut s);
    let dir = s.persistence.as_ref().unwrap().session_dir.clone();
    fs::create_dir(dir.join(FILE)).unwrap();
    flush(&mut s);
    let error = s.persistence.as_ref().unwrap().status.last_error.as_deref().unwrap();
    assert!(error.contains("complete native journal"), "{error}");
    assert_eq!(s.persistence.as_ref().unwrap().status.journaled_revision, 0);
    let loaded = filmcraft_format::decode(&fs::read(dir.join("snapshot.fcproj")).unwrap()).unwrap();
    assert_eq!(project_bytes(&loaded.project, MAX_BYTES).unwrap(), project_bytes(&s.project, MAX_BYTES).unwrap());
    assert!(dir.join(FILE).is_dir());
    s.shutdown();
}
#[test]
fn full_native_journal_reuses_one_companion_and_clean_save_removes_only_the_current_sessions_files() {
    let (_d, mut s) = setup();
    assistant(&mut s);
    let dir = flush(&mut s);
    let previous = fs::read(dir.join(FILE)).unwrap();
    s.execute("file.newBin", json!({"name":"Next human"})).unwrap();
    flush(&mut s);
    assert_ne!(fs::read(dir.join(FILE)).unwrap(), previous);
    assert!(!fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".native-state-")));
    s.execute("file.save", json!({})).unwrap();
    flush(&mut s);
    assert!(!dir.join(FILE).exists());
    assert!(!dir.join("snapshot.fcproj").exists());
    s.shutdown();
}
