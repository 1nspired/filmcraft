use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
const W: &str = "11111111-1111-4111-8111-111111111111";
const U: &str = "22222222-2222-4222-8222-222222222222";
const C: &str = "33333333-3333-4333-8333-333333333333";
fn token(n: usize) -> String {
    format!("{n:064x}")
}
fn scope() -> Value {
    json!({"projectId":"p_complete_snapshot","workspaceId":W})
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Scratch, Session) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = crate::temp_dir().join(format!("poema-complete-root-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Root","workspace":scope()})).unwrap();
    let mut cfg = crate::autosave::AutosaveConfig::new(root.join("source-state"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    s.start_autosave(cfg).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj").to_str().unwrap()})).unwrap();
    install(&mut s);
    (Scratch(root), s)
}
fn install(s: &mut Session) {
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(s).unwrap();
    s.agent_operation_control().enable_transport();
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute("agent.readProject", json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn basis(s: &mut Session, n: usize) {
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope()})).unwrap();
}
fn bin(s: &mut Session, n: usize) {
    let r = read(s, n);
    s.execute("agent.apply",json!({"token":token(n),"label":"Metadata bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":r["binState"]["sha256"],"edit":{"type":"create","name":format!("Bin{n}"),"parent":null}}}]})).unwrap();
}
fn first(s: &mut Session, n: usize) -> u64 {
    let r = read(s, n);
    s.execute("agent.apply",json!({"token":token(n),"label":"First sequence","exact":true,"authority":"document_state","operations":[{"command":"agent.createInitialSequence","params":{"scope":scope(),"beforeSha256":r["projectState"]["sha256"],"sequence":{"name":"First","width":3840,"height":2160,"frameRate":{"num":48001,"den":2000},"sampleRate":96000,"mix":"Mono","trackType":"Mono","video":2,"audio":4}}}]})).unwrap()["results"][0]["sequence"].as_u64().unwrap()
}
fn stage(s: &mut Session) -> Value {
    basis(s, 20);
    let saved = s.execute("agent.saveHistoryCheckpoint", json!({"token":token(20),"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true})).unwrap();
    basis(s, 30);
    let pause = s.execute("agent.beginWorkspaceUpdate", json!({"token":token(30),"scope":scope(),"workspace":W,"update":U})).unwrap();
    assert_eq!(pause["version"], 2);
    assert_eq!(pause["kind"], "native_project_recovery_fence");
    basis(s, 40);
    s.execute("agent.stageProjectRecovery",json!({"token":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":saved["checkpoint"]["sha256"]})).unwrap()
}
fn retire(s: &mut Session, stage: &Value) {
    basis(s, 50);
    let r = s
        .execute(
            "agent.retireWorkspaceUpdate",
            json!({"token":token(50),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"stageSha256":stage["sha256"]}),
        )
        .unwrap();
    assert_eq!(r["status"], "retiring");
    s.shutdown();
}
fn startup(s: &Session, stage: &Value) -> Value {
    json!({"projectPath":s.path,"scope":scope(),"workspace":W,"update":U,"sha256":stage["sha256"],"activationToken":token(100)})
}
fn activate(s: &mut Session) {
    install(s);
    let h = s.workspace_update_status().unwrap();
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]}),
    )
    .unwrap();
}
fn undo(s: &mut Session, n: usize, commit: usize) {
    read(s, n);
    assert_eq!(s.execute("agent.readHistory", json!({"token":token(n),"scope":scope(),"commit":token(commit),"undo":null})).unwrap()["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(n),"commit":token(commit)})).unwrap();
}
fn redo(s: &mut Session, n: usize, commit: usize, u: usize) {
    read(s, n);
    s.execute("agent.redo", json!({"token":token(n),"commit":token(commit),"undo":token(u)})).unwrap();
}
#[test]
fn complete_root_snapshot_restores_before_any_sequence_with_exact_editor_preferences_original_receipts_and_owned_undo_redo() {
    let (dir, mut source) = setup();
    bin(&mut source, 11);
    source.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    source.state.clipboard_markers.push(filmcraft_project::Marker {
        id: filmcraft_project::MarkerId(7),
        start: filmcraft_time::Tick(9_007_199_254_740_997),
        duration: filmcraft_time::Tick(0),
        name: "Native clipboard".into(),
        comment: "Exact ticks".into(),
        kind: filmcraft_project::MarkerKind::Comment,
        color: filmcraft_project::Label::Violet,
    });
    source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let graph = source.project.clone();
    let editor = serde_json::to_value(&source.state).unwrap();
    let clipboard = serde_json::to_value(&source.state.clipboard_markers).unwrap();
    let original = fs::read(source.path.as_ref().unwrap()).unwrap();
    let staged = stage(&mut source);
    let raw = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let request = startup(&source, &staged);
    let mut fresh = Session::default();
    assert!(fresh.open_project_recovery(&request).is_err(), "live source persistence owner cannot be ignored");
    assert!(fresh.path.is_none());
    retire(&mut source, &staged);
    let fence = fresh.open_project_recovery(&request).unwrap();
    assert_eq!(fence["version"], 2);
    assert!(fresh.is_display_only());
    assert!(fresh.state.active_sequence.is_none());
    assert!(fresh.project.sequences().next().is_none());
    assert_eq!(*fresh.project, *graph);
    assert_eq!(serde_json::to_value(&fresh.state).unwrap(), editor);
    assert_eq!(serde_json::to_value(&fresh.state.clipboard_markers).unwrap(), clipboard);
    assert_eq!(fresh.prefs.auto_save.max_versions, 7);
    assert!(fresh.execute("file.save", json!({})).is_err());
    fresh.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("candidate-state"))).unwrap();
    assert!(!dir.0.join("candidate-state/preferences.json").exists());
    activate(&mut fresh);
    assert!(!fresh.is_display_only());
    assert!(fresh.is_dirty());
    undo(&mut fresh, 101, 11);
    assert!(fresh.project.root.children.is_empty());
    redo(&mut fresh, 102, 11, 101);
    assert_eq!(*fresh.project, *graph);
    assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), original);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), raw);
    fresh.shutdown();
}
#[test]
fn complete_root_snapshot_preserves_real_first_sequence_closed_views_and_redo_to_absence_without_an_anchor_alias() {
    for mode in ["first", "closed", "undone"] {
        let (_dir, mut source) = setup();
        let q = first(&mut source, 11);
        if mode == "closed" {
            source.execute("sequence.close", json!({"item":q})).unwrap();
        }
        if mode == "undone" {
            undo(&mut source, 12, 11);
        }
        let graph = source.project.clone();
        let active = source.state.active_sequence;
        let staged = stage(&mut source);
        let request = startup(&source, &staged);
        retire(&mut source, &staged);
        let mut f = Session::default();
        f.open_project_recovery(&request).unwrap();
        assert_eq!(*f.project, *graph);
        assert_eq!(f.state.active_sequence, active);
        activate(&mut f);
        if mode == "first" {
            undo(&mut f, 101, 11);
            assert!(f.project.sequences().next().is_none());
            redo(&mut f, 102, 11, 101);
        } else if mode == "undone" {
            redo(&mut f, 101, 11, 12);
            assert!(f.project.sequences().next().is_some());
        } else {
            assert!(f.state.active_sequence.is_none());
            assert!(f.project.sequence(filmcraft_project::ItemId(q)).is_some());
        }
        f.shutdown();
    }
}
#[test]
fn complete_root_snapshot_reconciles_native_pause_and_clean_exit_flushes_without_losing_newer_owned_history() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    s.persistence.as_ref().unwrap().flush();
    let journal = s.persistence.as_ref().unwrap().session_dir.clone();
    let old = fs::read(journal.join("snapshot.fcproj")).unwrap();
    bin(&mut s, 12);
    let staged = stage(&mut s);
    let graph = s.project.clone();
    let request = startup(&s, &staged);
    retire(&mut s, &staged);
    let flushed = fs::read(journal.join("snapshot.fcproj")).unwrap();
    assert_ne!(flushed, old, "pause acknowledges the current source graph before staging");
    assert_eq!(filmcraft_format::decode(&flushed).unwrap().project, *graph);
    let meta: Value = serde_json::from_slice(&fs::read(journal.join("session.json")).unwrap()).unwrap();
    assert_eq!(meta["cleanExit"], true);
    let mut f = Session::default();
    f.open_project_recovery(&request).unwrap();
    assert_eq!(*f.project, *graph);
    assert_eq!(f.history.undo.len(), 2);
    assert!(f.is_display_only());
}
#[test]
fn complete_root_snapshot_refuses_later_saved_bytes_or_preferences_and_never_replaces_the_fresh_or_original_editor() {
    for mode in ["saved", "preferences"] {
        let (dir, mut s) = setup();
        bin(&mut s, 11);
        let staged = stage(&mut s);
        let request = startup(&s, &staged);
        retire(&mut s, &staged);
        if mode == "saved" {
            fs::write(s.path.as_ref().unwrap(), b"later human saved data").unwrap();
        } else {
            fs::write(dir.0.join("source-state/preferences.json"), b"later human preferences").unwrap();
        }
        let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
        let mut f = Session::default();
        let project = f.project.clone();
        assert!(f.open_project_recovery(&request).is_err());
        assert!(std::sync::Arc::ptr_eq(&project, &f.project));
        assert!(f.path.is_none());
        assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    }
}
#[test]
fn complete_root_snapshot_refuses_damaged_scope_checksums_existing_editors_and_reused_activation_tokens_without_replaying_history() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    let staged = stage(&mut s);
    let request = startup(&s, &staged);
    retire(&mut s, &staged);
    for mode in ["scope", "hash", "token", "live_editor"] {
        let mut r = request.clone();
        let mut f = Session::default();
        match mode {
            "scope" => r["scope"]["projectId"] = json!("p_foreign"),
            "hash" => r["sha256"] = json!("0".repeat(64)),
            "token" => r["activationToken"] = json!(token(40)),
            "live_editor" => {
                f.execute("file.newProject", json!({"name":"Keep this human project"})).unwrap();
            }
            _ => unreachable!(),
        }
        let graph = f.project.clone();
        assert!(f.open_project_recovery(&r).is_err(), "{mode}");
        assert!(std::sync::Arc::ptr_eq(&graph, &f.project));
    }
    let path = staged["path"].as_str().unwrap();
    let mut damaged = fs::read(path).unwrap();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    fs::write(path, &damaged).unwrap();
    assert!(Session::default().open_project_recovery(&request).is_err());
    assert_eq!(fs::read(path).unwrap(), damaged);
}
#[test]
fn complete_root_snapshot_stage_reconciles_one_attempt_and_retirement_refuses_a_changed_artifact_while_cancel_preserves_the_source() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    let staged = stage(&mut s);
    let request =
        json!({"token":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":staged["checkpointSha256"]});
    let original = s.project.clone();
    assert_eq!(s.execute("agent.stageProjectRecovery", request).unwrap(), staged);
    let path = staged["path"].as_str().unwrap();
    let raw = fs::read(path).unwrap();
    fs::write(path, b"incomplete or corrupt snapshot").unwrap();
    basis(&mut s, 50);
    assert!(
        s.execute(
            "agent.retireWorkspaceUpdate",
            json!({"token":token(50),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"stageSha256":staged["sha256"]})
        )
        .is_err()
    );
    assert!(s.is_display_only());
    let h = s.workspace_update_status().unwrap();
    s.execute("agent.cancelWorkspaceUpdate", json!({"token":h["token"],"scope":h["scope"],"workspace":W,"update":U,"nativeSession":h["nativeSession"]}))
        .unwrap();
    assert!(!s.is_display_only());
    assert!(std::sync::Arc::ptr_eq(&original, &s.project));
    assert_eq!(fs::read(path).unwrap(), b"incomplete or corrupt snapshot");
    assert_ne!(fs::read(path).unwrap(), raw);
    s.shutdown();
}

#[test]
fn complete_root_snapshot_stop_before_staging_publishes_no_artifact_and_never_strands_the_source_owner() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    basis(&mut s, 20);
    let c = s.execute("agent.saveHistoryCheckpoint", json!({"token":token(20),"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true})).unwrap();
    basis(&mut s, 30);
    s.execute("agent.beginWorkspaceUpdate", json!({"token":token(30),"scope":scope(),"workspace":W,"update":U})).unwrap();
    basis(&mut s, 40);
    let result = s.agent_operation_control().cancel(&json!({"token":token(40)})).unwrap();
    assert_eq!(result["cancellationAccepted"], true);
    let graph = s.project.clone();
    let path = std::path::Path::new(s.path.as_ref().unwrap()).parent().unwrap().join(format!(".poema-project-recovery-{W}-{U}.json"));
    assert!(s.execute("agent.stageProjectRecovery",json!({"token":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":c["checkpoint"]["sha256"]})).is_err());
    assert!(!path.exists());
    assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
    let pause = s.workspace_update_status().unwrap();
    s.execute(
        "agent.cancelWorkspaceUpdate",
        json!({"token":pause["token"],"scope":pause["scope"],"workspace":W,"update":U,"nativeSession":pause["nativeSession"]}),
    )
    .unwrap();
    assert!(!s.is_display_only());
    s.shutdown();
}
#[test]
fn complete_root_snapshot_rechecks_stop_and_original_human_changes_before_activation_without_starting_target_persistence() {
    for mode in ["stop", "preferences", "saved"] {
        let (dir, mut source) = setup();
        bin(&mut source, 11);
        let staged = stage(&mut source);
        let request = startup(&source, &staged);
        retire(&mut source, &staged);
        let mut f = Session::default();
        f.open_project_recovery(&request).unwrap();
        let data = dir.0.join("candidate-state");
        f.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(&data)).unwrap();
        install(&mut f);
        let graph = f.project.clone();
        match mode {
            "stop" => {
                let result = f.agent_operation_control().cancel(&json!({"token":token(100)})).unwrap();
                assert_eq!(result["cancellationAccepted"], true);
            }
            "preferences" => fs::write(dir.0.join("source-state/preferences.json"), b"later human preferences").unwrap(),
            "saved" => fs::write(source.path.as_ref().unwrap(), b"later human saved work").unwrap(),
            _ => unreachable!(),
        }
        let pause = f.workspace_update_status().unwrap();
        assert!(
            f.execute(
                "agent.activateWorkspaceUpdate",
                json!({"token":pause["token"],"scope":pause["scope"],"workspace":W,"update":U,"nativeSession":pause["nativeSession"]})
            )
            .is_err(),
            "{mode}"
        );
        assert!(f.is_display_only());
        assert!(f.persistence.is_none());
        assert!(!data.join("preferences.json").exists());
        assert!(std::sync::Arc::ptr_eq(&graph, &f.project));
    }
}

fn prepared(s: &mut Session) -> (Value, Value) {
    basis(s, 20);
    let c = s.execute("agent.saveHistoryCheckpoint", json!({"token":token(20),"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true})).unwrap();
    basis(s, 30);
    s.execute("agent.beginWorkspaceUpdate", json!({"token":token(30),"scope":scope(),"workspace":W,"update":U})).unwrap();
    basis(s, 40);
    basis(s, 60);
    let prep=s.execute("agent.prepareProjectRecovery",json!({"token":token(60),"stageToken":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":c["checkpoint"]["sha256"]})).unwrap();
    let publish = json!({"token":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":c["checkpoint"]["sha256"],"expectedSha256":prep["sha256"]});
    (prep, publish)
}
#[test]
fn root_preparation_is_unpublished_exact_and_recovers_from_the_previously_recorded_hash() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    let (prep, publish) = prepared(&mut s);
    assert_eq!(prep["status"], "prepared");
    assert_eq!(prep["token"], token(60));
    assert_eq!(prep["stageToken"], token(40));
    assert!(!std::path::Path::new(prep["path"].as_str().unwrap()).exists());
    assert_eq!(s.workspace_update_status().unwrap()["stage"], Value::Null);
    assert!(s.execute("agent.validateProjectRead", json!({"token":token(60),"scope":scope()})).is_err());
    let staged = s.execute("agent.stageProjectRecovery", publish.clone()).unwrap();
    assert_eq!(staged["sha256"], prep["sha256"]);
    assert_eq!(staged["bytes"], prep["bytes"]);
    assert_eq!(staged["token"], prep["stageToken"]);
    assert_eq!(s.execute("agent.stageProjectRecovery", publish).unwrap(), staged);
    // A lost staging reply does not force a replay: cold native validation can
    // use the exact prepublication digest after the original owner has exited.
    let startup = json!({"projectPath":s.path,"scope":scope(),"workspace":W,"update":U,"sha256":prep["sha256"],"activationToken":token(100)});
    retire(&mut s, &staged);
    let mut cold = Session::default();
    cold.open_project_recovery(&startup).unwrap();
    activate(&mut cold);
    undo(&mut cold, 101, 11);
}
#[test]
fn root_prepared_hash_mismatch_never_reserves_or_publishes_a_stage() {
    let (_dir, mut s) = setup();
    let (prep, mut publish) = prepared(&mut s);
    publish["expectedSha256"] = json!("f".repeat(64));
    assert!(s.execute("agent.stageProjectRecovery", publish).is_err());
    assert!(!std::path::Path::new(prep["path"].as_str().unwrap()).exists());
    assert_eq!(s.workspace_update_status().unwrap()["stage"], Value::Null);
    assert_eq!(
        s.execute("agent.cancelWorkspaceUpdate", json!({"token":token(30),"scope":scope(),"workspace":W,"update":U,"nativeSession":prep["nativeSession"]}))
            .unwrap()["status"],
        "cancelled"
    );
    bin(&mut s, 71);
}
#[test]
fn root_preparation_rejects_crossed_or_stopped_tokens_without_consuming_the_stage_basis() {
    let (_dir, mut s) = setup();
    let (prep, publish) = prepared(&mut s);
    for (offset, stage_token) in [token(30), token(62), token(999)].into_iter().enumerate() {
        let current = 61 + offset;
        basis(&mut s, current);
        let request = json!({"token":token(current),"stageToken":stage_token,"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":prep["checkpointSha256"]});
        assert!(s.execute("agent.prepareProjectRecovery", request).is_err());
        // Use a distinct fresh basis for each hostile request.
    }
    basis(&mut s, 69);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(69)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute("agent.prepareProjectRecovery",json!({"token":token(69),"stageToken":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":prep["checkpointSha256"]})).is_err());
    assert!(!std::path::Path::new(prep["path"].as_str().unwrap()).exists());
    assert!(s.execute("agent.stageProjectRecovery", publish).is_ok());
}

#[test]
fn recovery_scheduling_fingerprint_tracks_editor_preferences_history_and_save_without_source_or_edit_authority() {
    let (_dir, mut s) = setup();
    let inspect = |s: &mut Session, n| s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope()})).unwrap();
    let before = inspect(&mut s, 300);
    assert_eq!(before["capabilities"]["projectRecoveryScheduling"], true);
    assert_eq!(before["recoveryState"]["ready"], true);
    assert_eq!(before["recoveryState"], inspect(&mut s, 301)["recoveryState"]);
    let rev = s.revision;
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_997);
    let seek = inspect(&mut s, 302);
    assert_eq!(s.revision, rev);
    assert_ne!(seek["recoveryState"]["sha256"], before["recoveryState"]["sha256"]);
    s.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let prefs = inspect(&mut s, 303);
    assert_eq!(s.revision, rev);
    assert_ne!(prefs["recoveryState"]["sha256"], seek["recoveryState"]["sha256"]);
    bin(&mut s, 11);
    let edited = inspect(&mut s, 304);
    assert_ne!(edited["recoveryState"]["sha256"], prefs["recoveryState"]["sha256"]);
    s.execute("file.save", json!({})).unwrap();
    let saved = inspect(&mut s, 305);
    assert_ne!(saved["recoveryState"]["sha256"], edited["recoveryState"]["sha256"]);
    assert!(s.execute("agent.validateProjectRead", json!({"token":token(305),"scope":scope()})).is_err());
    s.execute("agent.beginWorkspaceUpdate", json!({"token":token(305),"scope":scope(),"workspace":W,"update":U})).unwrap();
    assert_eq!(inspect(&mut s, 306)["recoveryState"]["ready"], false);
}

#[cfg(unix)]
#[test]
fn recovery_hint_refuses_non_utf8_native_data_directory_without_panicking_or_editing() {
    use std::os::unix::ffi::OsStringExt;
    let (_dir, mut s) = setup();
    let original = s.persistence.as_ref().unwrap().data_dir.clone();
    s.persistence.as_mut().unwrap().data_dir = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 255]));
    let value = super::native_editor::recovery_state(&s);
    assert_eq!(value["ready"], false);
    assert_eq!(value["dataDirectory"], Value::Null);
    s.persistence.as_mut().unwrap().data_dir = original;
}

#[test]
fn cancelled_activation_refresh_keeps_one_paused_root_candidate_and_original_history_without_replaying_activation() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    let staged = stage(&mut source);
    let raw = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let file = fs::read(source.path.as_ref().unwrap()).unwrap();
    let startup = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut candidate = Session::default();
    candidate.open_project_recovery(&startup).unwrap();
    install(&mut candidate);
    basis(&mut candidate, 200);
    basis(&mut candidate, 201);
    let before = candidate.workspace_update_status().unwrap();
    let request = json!({"token":token(200),"scope":scope(),"workspace":W,"update":U,"nativeSession":before["nativeSession"],"previousToken":token(100),"activationToken":token(201)});
    assert!(candidate.execute("agent.refreshWorkspaceActivation", request.clone()).is_err());
    assert_eq!(candidate.workspace_update_status().unwrap()["token"], token(100));
    assert_eq!(candidate.agent_operation_control().cancel(&json!({"token":token(100)})).unwrap()["cancellationAccepted"], true);
    let receipt = candidate.execute("agent.refreshWorkspaceActivation", request.clone()).unwrap();
    assert_eq!(receipt["publication"], "not-started");
    assert_eq!(candidate.execute("agent.refreshWorkspaceActivation", request).unwrap(), receipt);
    let ready = candidate.workspace_update_status().unwrap();
    assert_eq!(ready["token"], token(201));
    assert_eq!(ready["activationRefresh"], receipt);
    assert!(candidate.is_display_only());
    assert!(
        candidate
            .execute(
                "agent.activateWorkspaceUpdate",
                json!({"token":token(100),"scope":scope(),"workspace":W,"update":U,"nativeSession":ready["nativeSession"]})
            )
            .is_err()
    );
    candidate
        .execute("agent.activateWorkspaceUpdate", json!({"token":token(201),"scope":scope(),"workspace":W,"update":U,"nativeSession":ready["nativeSession"]}))
        .unwrap();
    undo(&mut candidate, 301, 11);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), raw);
    assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), file);
}
#[test]
fn activation_refresh_refuses_foreign_epoch_stopped_future_basis_and_cancelled_candidate_without_changing_the_hold() {
    for case in 0..3 {
        let (_dir, mut source) = setup();
        let staged = stage(&mut source);
        let startup = startup(&source, &staged);
        retire(&mut source, &staged);
        let mut s = Session::default();
        s.open_project_recovery(&startup).unwrap();
        install(&mut s);
        basis(&mut s, 200);
        basis(&mut s, 201);
        let before = s.workspace_update_status().unwrap();
        s.agent_operation_control().cancel(&json!({"token":token(100)})).unwrap();
        let mut request = json!({"token":token(200),"scope":scope(),"workspace":W,"update":U,"nativeSession":before["nativeSession"],"previousToken":token(100),"activationToken":token(201)});
        if case == 0 {
            request["nativeSession"] = json!("f".repeat(32));
        } else if case == 1 {
            s.agent_operation_control().cancel(&json!({"token":token(201)})).unwrap();
        } else {
            s.cancel_workspace_update().unwrap();
        }
        assert!(s.execute("agent.refreshWorkspaceActivation", request).is_err());
        assert_eq!(s.workspace_update_status().unwrap()["token"], token(100));
        assert!(s.is_display_only());
    }
}

fn newer_journal_source(human: bool) -> (Scratch, Session, Value, Value) {
    newer_journal_source_with_preferences(human, false)
}
fn newer_journal_source_with_preferences(human: bool, preferences: bool) -> (Scratch, Session, Value, Value) {
    let (dir, mut s) = setup();
    bin(&mut s, 11);
    if preferences {
        s.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    }
    let saved = stage(&mut s);
    let mut request = startup(&s, &saved);
    request["latestJournal"] = json!(true);
    s.cancel_workspace_update().unwrap();
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_991);
    if human {
        s.execute("file.newBin", json!({"name":"Later real human bin"})).unwrap();
    } else {
        bin(&mut s, 12);
    }
    s.sync_persistence();
    s.persistence.take().unwrap().simulate_crash();
    (dir, s, saved, request)
}
#[test]
fn newer_journal_cold_start_restores_human_history_exact_native_editor_and_paused_receipt_without_borrowing_assistant_undo() {
    let (_d, source, saved, request) = newer_journal_source(true);
    let old = fs::read(saved["path"].as_str().unwrap()).unwrap();
    let original = fs::read(source.path.as_ref().unwrap()).unwrap();
    let mut no_optin = request.clone();
    no_optin.as_object_mut().unwrap().remove("latestJournal");
    let mut fresh = Session::default();
    assert!(fresh.open_project_recovery(&no_optin).is_err());
    assert!(fresh.path.is_none());
    let fence = fresh.open_project_recovery(&request).unwrap();
    assert_eq!(fence["capabilities"]["projectRecoveryJournal"], true);
    assert_eq!(fence["journalRecovery"]["sourceSnapshotRevision"], saved["revision"]);
    assert_eq!(fence["journalRecovery"]["revision"], source.revision);
    assert_eq!(fence["journalRecovery"]["mediaEvidence"], false);
    assert_eq!(fence["journalRecovery"]["writeAuthority"], false);
    assert!(fresh.is_display_only());
    assert_eq!(*fresh.project, *source.project);
    assert_eq!(fresh.state.source_playhead, source.state.source_playhead);
    assert!(fresh.state.active_sequence.is_none());
    assert_eq!(fresh.history.undo.len(), source.history.undo.len());
    activate(&mut fresh);
    read(&mut fresh, 301);
    assert_eq!(fresh.execute("agent.readHistory", json!({"token":token(301),"scope":scope(),"commit":token(11),"undo":null})).unwrap()["undoAvailable"], false);
    fresh.execute("edit.undo", json!({})).unwrap();
    assert_ne!(*fresh.project, *source.project);
    fresh.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*fresh.project, *source.project);
    assert_eq!(fs::read(saved["path"].as_str().unwrap()).unwrap(), old);
    assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), original);
}
#[test]
fn newer_journal_cold_start_retains_the_actual_latest_assistant_receipt_and_owned_undo_redo_without_replaying_a_recipe() {
    let (_d, source, _saved, request) = newer_journal_source(false);
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    activate(&mut fresh);
    assert_eq!(*fresh.project, *source.project);
    undo(&mut fresh, 301, 12);
    assert_ne!(*fresh.project, *source.project);
    redo(&mut fresh, 302, 12, 301);
    assert_eq!(*fresh.project, *source.project);
}
#[test]
fn newer_journal_cold_start_refuses_damaged_companion_wrong_source_pid_and_changed_saved_file_without_fallback_or_source_writes() {
    for case in 0..4 {
        let (_d, source, saved, request) = newer_journal_source(true);
        let basis: Value = serde_json::from_slice::<Value>(&fs::read(saved["path"].as_str().unwrap()).unwrap()).unwrap()["payload"]["basis"].clone();
        let dir = PathBuf::from(basis["data_directory"].as_str().unwrap()).join("Recovery").join(basis["session_directory"].as_str().unwrap());
        if case == 0 {
            let mut raw = fs::read(dir.join("native-state.json")).unwrap();
            raw[0] = b'!';
            fs::write(dir.join("native-state.json"), raw).unwrap();
        } else if case == 1 {
            let file = dir.join("session.json");
            let mut meta: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            meta["pid"] = json!(std::process::id().saturating_add(1));
            fs::write(file, serde_json::to_vec(&meta).unwrap()).unwrap();
        } else if case == 2 {
            let file = source.path.as_ref().unwrap();
            let raw = fs::read(file).unwrap();
            let replacement = dir.join("identical-saved.fcproj");
            fs::write(&replacement, raw).unwrap();
            fs::rename(replacement, file).unwrap();
        } else {
            fs::remove_file(dir.join("native-state.json")).unwrap();
        }
        let file = source.path.as_ref().unwrap();
        let before = fs::read(file).unwrap();
        let snapshot = fs::read(saved["path"].as_str().unwrap()).unwrap();
        let mut fresh = Session::default();
        assert!(fresh.open_project_recovery(&request).is_err(), "case{case}");
        assert!(fresh.path.is_none());
        assert!(fresh.history.undo.is_empty());
        assert_eq!(fs::read(file).unwrap(), before);
        assert_eq!(fs::read(saved["path"].as_str().unwrap()).unwrap(), snapshot);
    }
}
#[test]
fn newer_journal_cold_start_rechecks_the_accepted_companion_before_activation_and_preserves_the_paused_candidate_on_change() {
    let (_d, _source, saved, request) = newer_journal_source(true);
    let basis: Value = serde_json::from_slice::<Value>(&fs::read(saved["path"].as_str().unwrap()).unwrap()).unwrap()["payload"]["basis"].clone();
    let file =
        PathBuf::from(basis["data_directory"].as_str().unwrap()).join("Recovery").join(basis["session_directory"].as_str().unwrap()).join("native-state.json");
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    let project = fresh.project.clone();
    let mut raw = fs::read(&file).unwrap();
    raw[0] = b'!';
    fs::write(&file, &raw).unwrap();
    install(&mut fresh);
    let h = fresh.workspace_update_status().unwrap();
    assert!(
        fresh
            .execute(
                "agent.activateWorkspaceUpdate",
                json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]})
            )
            .is_err()
    );
    assert!(fresh.is_display_only());
    assert_eq!(*fresh.project, *project);
    assert_eq!(fs::read(&file).unwrap(), raw);
}
#[test]
fn newer_journal_cold_start_rejects_a_live_source_owner_and_later_preferences_even_with_a_complete_companion() {
    for live in [true, false] {
        let (_d, mut source) = setup();
        bin(&mut source, 11);
        let saved = stage(&mut source);
        source.cancel_workspace_update().unwrap();
        source.execute("file.newBin", json!({"name":"New human work"})).unwrap();
        source.persistence.as_ref().unwrap().flush();
        let mut request = startup(&source, &saved);
        request["latestJournal"] = json!(true);
        if !live {
            source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
            source.persistence.take().unwrap().simulate_crash();
        }
        let mut fresh = Session::default();
        assert!(fresh.open_project_recovery(&request).is_err());
        assert!(fresh.path.is_none());
        source.shutdown();
    }
}
#[test]
fn newer_journal_cold_start_restores_a_real_new_sequence_without_manufacturing_the_previous_absent_view() {
    let (_d, mut source) = setup();
    bin(&mut source, 11);
    let saved = stage(&mut source);
    source.cancel_workspace_update().unwrap();
    let sequence = first(&mut source, 12);
    source.sync_persistence();
    source.persistence.take().unwrap().simulate_crash();
    let mut request = startup(&source, &saved);
    request["latestJournal"] = json!(true);
    let mut fresh = Session::default();
    let fence = fresh.open_project_recovery(&request).unwrap();
    assert_eq!(fence["journalRecovery"]["sequence"], sequence);
    assert_eq!(fresh.state.active_sequence.map(|v| v.0), Some(sequence));
    activate(&mut fresh);
    undo(&mut fresh, 301, 12);
    assert!(fresh.state.active_sequence.is_none());
    redo(&mut fresh, 302, 12, 301);
    assert_eq!(fresh.state.active_sequence.map(|v| v.0), Some(sequence));
}

#[test]
fn newer_journal_cold_start_does_not_resurrect_a_cached_dirty_edit_after_human_revert_clears_its_original_journal() {
    for latest in [false, true] {
        let (_d, mut source) = setup();
        bin(&mut source, 11);
        let saved = stage(&mut source);
        source.cancel_workspace_update().unwrap();
        source.execute("file.revert", json!({})).unwrap();
        source.sync_persistence();
        source.persistence.take().unwrap().simulate_crash();
        let mut request = startup(&source, &saved);
        request["latestJournal"] = json!(latest);
        let original = fs::read(source.path.as_ref().unwrap()).unwrap();
        let snapshot = fs::read(saved["path"].as_str().unwrap()).unwrap();
        let mut fresh = Session::default();
        let error = fresh.open_project_recovery(&request).unwrap_err().to_string();
        assert!(error.contains("journal disappeared"), "{error}");
        assert!(fresh.path.is_none());
        assert!(fresh.history.undo.is_empty());
        assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), original);
        assert_eq!(fs::read(saved["path"].as_str().unwrap()).unwrap(), snapshot);
    }
}

#[test]
fn newer_journal_checkpoint_retention_proves_the_exact_current_composer_checkpoint_from_the_original_native_journal_graph() {
    let (_d, mut source, _cached, request) = newer_journal_source(false);
    let checkpoint = "66666666-6666-4666-8666-666666666666";
    basis(&mut source, 70);
    let saved = source
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(70),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    activate(&mut fresh);
    let before = fresh.project.clone();
    read(&mut fresh, 301);
    let retained = fresh
        .execute(
            "agent.validateRetainedHistory",
            json!({"token":token(301),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"sha256":saved["checkpoint"]["sha256"]}),
        )
        .unwrap();
    assert_eq!(retained["status"], "retained");
    assert_eq!(*fresh.project, *before);
    undo(&mut fresh, 302, 12);
    redo(&mut fresh, 303, 12, 302);
    assert_eq!(*fresh.project, *source.project);
}
#[test]
fn newer_journal_checkpoint_retention_refuses_later_human_work_or_an_equal_value_graph_without_verified_native_bootstrap() {
    let (_d, mut source, _cached, request) = newer_journal_source(false);
    let checkpoint = "66666666-6666-4666-8666-666666666666";
    basis(&mut source, 70);
    let saved = source
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(70),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    for bootstrap in [true, false] {
        let mut fresh = Session::default();
        if bootstrap {
            fresh.open_project_recovery(&request).unwrap();
            activate(&mut fresh);
            fresh.execute("file.newBin", json!({"name":"Later human"})).unwrap();
        } else {
            crate::commands::install_project(&mut fresh, (*source.project).clone(), source.path.clone(), true);
            fresh.revision = source.revision;
            fresh.saved_revision = source.revision;
            install(&mut fresh);
        }
        let before = fresh.project.clone();
        read(&mut fresh, 301);
        assert!(
            fresh
                .execute(
                    "agent.validateRetainedHistory",
                    json!({"token":token(301),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"sha256":saved["checkpoint"]["sha256"]})
                )
                .is_err()
        );
        assert_eq!(*fresh.project, *before);
    }
}

// Recovery metadata reads reuse the complete staged native decoder, never a
// Source/Timeline read ticket or original-file qualification.
const DEPENDENCIES: &str = "agent.readProjectRecoveryDependencies";
fn dependency_request(stage: &Value) -> Value {
    json!({"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"nativeSession":stage["nativeSession"],"stageSha256":stage["sha256"]})
}
fn native_only_file(s: &mut Session, path: &str, proxy: Option<&str>, sequence: bool) {
    let (source, id) = assembly_tests::fixture();
    let mut item = source.project.item(ItemId(id)).unwrap().clone();
    item.name = "Human native-only source".into();
    item.metadata.clear();
    let media = item.as_media_mut().unwrap();
    media.media = filmcraft_project::MediaRef::File { path: path.into() };
    media.proxy = proxy.map(|p| filmcraft_project::MediaRef::File { path: p.into() });
    if sequence {
        media.info.kind = filmcraft_media::MediaKind::ImageSequence;
    }
    let project = std::sync::Arc::make_mut(&mut s.project);
    let existing = project.items.values().find(|i| i.name == item.name).map(|i| i.id);
    if let Some(id) = existing {
        project.items.get_mut(&id).unwrap().kind = item.kind;
    } else {
        project.add_item(&item.name, item.label, item.kind, None);
    }
    s.revision = s.revision.checked_add(1).unwrap();
}
#[test]
fn staged_recovery_dependencies_include_unbound_originals_proxies_and_both_history_branches_without_tickets_or_file_proof() {
    let (dir, mut s) = setup();
    native_only_file(&mut s, "old.MXF", Some("old-proxy.mov"), false);
    s.history.undo.push(("Earlier human source".into(), s.project.clone()));
    native_only_file(&mut s, "current.MXF", Some("current-proxy.mov"), false);
    let current = s.project.clone();
    native_only_file(&mut s, "redo.MXF", None, false);
    s.history.redo.push(("Later human source".into(), s.project.clone()));
    s.project = current;
    s.media.add_stored_paths(["old.MXF", "old-proxy.mov", "current.MXF", "current-proxy.mov", "redo.MXF"].map(String::from), s.path.as_deref());
    let staged = stage(&mut s);
    let request = dependency_request(&staged);
    let before = (s.project.clone(), s.agent_bridge.tickets.len(), s.agent_bridge.history_bases.len(), s.history.undo.len(), s.history.redo.len(), s.revision);
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let frozen = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let value = s.execute(DEPENDENCIES, request.clone()).unwrap();
    assert_eq!(value["projects"], 3);
    assert_eq!(value["files"].as_array().unwrap().len(), 5);
    let paths: std::collections::BTreeSet<_> = value["files"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    for name in ["old.MXF", "old-proxy.mov", "current.MXF", "current-proxy.mov", "redo.MXF"] {
        assert!(paths.contains(dir.0.join(name).to_str().unwrap()));
    }
    assert_eq!(value["mediaEvidence"], false);
    assert_eq!(value["writeAuthority"], false);
    assert_eq!(value["sourceValidation"], "not-performed");
    assert_eq!(value["savedProject"]["sha256"], staged["savedFileSha256"]);
    assert_eq!(s.execute(DEPENDENCIES, request).unwrap(), value);
    assert!(std::sync::Arc::ptr_eq(&before.0, &s.project));
    assert_eq!(
        (s.agent_bridge.tickets.len(), s.agent_bridge.history_bases.len(), s.history.undo.len(), s.history.redo.len(), s.revision),
        (before.1, before.2, before.3, before.4, before.5)
    );
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), frozen);
    s.shutdown();
}
#[test]
fn staged_recovery_dependencies_follow_original_numbered_image_sequence_policy_without_decoding_or_qualifying_frames() {
    let (dir, mut s) = setup();
    let first = dir.0.join("native_0001.png");
    let third = dir.0.join("native_0003.png");
    fs::write(&first, b"not decoded by a recovery dependency read").unwrap();
    fs::write(&third, b"not decoded either").unwrap();
    native_only_file(&mut s, first.to_str().unwrap(), None, true);
    let staged = stage(&mut s);
    let value = s.execute(DEPENDENCIES, dependency_request(&staged)).unwrap();
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|r| r["kind"] == "sequence-frame"));
    assert!(files.iter().any(|r| r["path"] == first.to_str().unwrap()));
    assert!(files.iter().any(|r| r["path"] == third.to_str().unwrap()));
    assert_eq!(value["references"], 2);
    assert_eq!(value["mediaEvidence"], false);
    s.shutdown();
}
#[test]
fn staged_recovery_dependencies_refuse_foreign_or_unstaged_addresses_and_preserve_the_original_pause_and_snapshot() {
    let (_dir, mut s) = setup();
    let empty = json!({"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"nativeSession":token(99),"stageSha256":"0".repeat(64)});
    assert!(s.execute(DEPENDENCIES, empty).is_err());
    let staged = stage(&mut s);
    let good = dependency_request(&staged);
    let bytes = fs::read(staged["path"].as_str().unwrap()).unwrap();
    for (key, value) in [
        ("workspace", json!(C)),
        ("update", json!(C)),
        ("pauseToken", json!(token(99))),
        ("nativeSession", json!(token(99))),
        ("stageSha256", json!("0".repeat(64))),
        ("path", json!("/foreign/snapshot.json")),
        ("scope", json!({"projectId":"foreign","workspaceId":W})),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(s.execute(DEPENDENCIES, bad).is_err());
        assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), bytes);
        assert_eq!(s.workspace_update_status().unwrap()["status"], "paused");
    }
    assert!(s.execute(DEPENDENCIES, good).is_ok());
    s.shutdown();
}
#[test]
fn staged_recovery_dependencies_refuse_corrupt_snapshot_and_later_saved_work_without_repair_or_recapture() {
    for mode in ["snapshot", "saved"] {
        let (_dir, mut s) = setup();
        let staged = stage(&mut s);
        let target = if mode == "snapshot" { staged["path"].as_str().unwrap() } else { s.path.as_deref().unwrap() }.to_string();
        fs::write(&target, b"later or corrupt bytes must be retained").unwrap();
        let before = s.project.clone();
        assert!(s.execute(DEPENDENCIES, dependency_request(&staged)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"later or corrupt bytes must be retained");
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        s.shutdown();
    }
}
#[test]
fn staged_recovery_dependencies_never_read_for_a_candidate_retired_or_cancelled_owner_or_guess_a_relative_import_origin() {
    let (_dir, mut source) = setup();
    let staged = stage(&mut source);
    let request = dependency_request(&staged);
    let startup = startup(&source, &staged);
    retire(&mut source, &staged);
    assert!(source.execute(DEPENDENCIES, request.clone()).is_err());
    let mut fresh = Session::default();
    fresh.open_project_recovery(&startup).unwrap();
    assert!(fresh.execute(DEPENDENCIES, request).is_err());
    fresh.shutdown();
    source.shutdown();
    let (_dir, mut s) = setup();
    native_only_file(&mut s, "picked-relative.MXF", None, false);
    let staged = stage(&mut s);
    assert!(s.execute(DEPENDENCIES, dependency_request(&staged)).is_err());
    let h = s.workspace_update_status().unwrap();
    s.execute("agent.cancelWorkspaceUpdate", json!({"token":h["token"],"scope":h["scope"],"workspace":W,"update":U,"nativeSession":h["nativeSession"]}))
        .unwrap();
    assert!(s.execute(DEPENDENCIES, dependency_request(&staged)).is_err());
    s.shutdown();
}

const ARCHIVE_DEPENDENCIES: &str = "agent.readArchivedRecoveryDependencies";
const ARCHIVED_C: &str = "44444444-4444-4444-8444-444444444444";
fn save_archived_dependencies(s: &mut Session) -> Value {
    basis(s, 10);
    let saved = s
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(10),"scope":scope(),"workspace":W,"checkpoint":ARCHIVED_C,"metadataBasis":true}))
        .unwrap();
    json!({"checkpoint":ARCHIVED_C,"sha256":saved["checkpoint"]["sha256"],"basis":{"nativeSession":saved["checkpoint"]["nativeSession"],"revision":saved["checkpoint"]["nativeRevision"],"sequence":saved["sequence"]}})
}
fn archived_request(stage: &Value, checkpoint: &Value) -> Value {
    json!({"owner":dependency_request(stage),"checkpoint":checkpoint})
}
#[test]
fn archived_recovery_dependencies_retain_old_only_originals_proxies_and_document_origins_without_current_tickets_or_history() {
    let (dir, mut s) = setup();
    let original = s.project.clone();
    native_only_file(&mut s, "forgotten.MXF", Some("forgotten-proxy.mov"), false);
    let checkpoint = save_archived_dependencies(&mut s);
    s.project = original;
    s.revision += 1;
    s.media.allow("forgotten.MXF");
    let staged = stage(&mut s);
    let request = archived_request(&staged, &checkpoint);
    let current = s.execute(DEPENDENCIES, dependency_request(&staged)).unwrap();
    assert!(current["files"].as_array().unwrap().is_empty());
    let before = (
        s.project.clone(),
        s.revision,
        s.agent_bridge.tickets.len(),
        s.agent_bridge.history_bases.len(),
        s.agent_bridge.history_checkpoints.len(),
        s.history.undo.len(),
        s.history.redo.len(),
    );
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let snapshot = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let value = s.execute(ARCHIVE_DEPENDENCIES, request.clone()).unwrap();
    assert_eq!(value["kind"], "native_archived_recovery_dependencies");
    assert_eq!(value["stageSha256"], staged["sha256"]);
    assert_eq!(value["checkpoint"]["sha256"], checkpoint["sha256"]);
    assert_eq!(value["basis"], checkpoint["basis"]);
    assert_eq!(value["historyValidation"], "validated_archive_only");
    assert_eq!(value["mediaEvidence"], false);
    assert_eq!(value["writeAuthority"], false);
    assert!(value.get("savedProject").is_none());
    assert!(value.get("token").is_none());
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|r| r["kind"] == "original" && r["path"] == dir.0.join("forgotten.MXF").to_str().unwrap()));
    assert!(files.iter().any(|r| r["kind"] == "proxy"));
    assert!(files.iter().all(|r| r["origin"]["kind"] == "document" && r["origin"]["document"] == s.path.clone().unwrap()));
    assert_eq!(s.execute(ARCHIVE_DEPENDENCIES, request).unwrap(), value);
    assert!(std::sync::Arc::ptr_eq(&before.0, &s.project));
    assert_eq!(
        (
            s.revision,
            s.agent_bridge.tickets.len(),
            s.agent_bridge.history_bases.len(),
            s.agent_bridge.history_checkpoints.len(),
            s.history.undo.len(),
            s.history.redo.len()
        ),
        (before.1, before.2, before.3, before.4, before.5, before.6)
    );
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), snapshot);
    s.shutdown();
}
#[test]
fn archived_recovery_dependencies_share_original_numbered_sequence_gap_policy_without_live_media_admission() {
    let (dir, mut s) = setup();
    let original = s.project.clone();
    let first = dir.0.join("archive_0001.png");
    let third = dir.0.join("archive_0003.png");
    fs::write(&first, b"generated frame bytes, never decoded").unwrap();
    fs::write(&third, b"generated third frame").unwrap();
    native_only_file(&mut s, "archive_0001.png", None, true);
    let checkpoint = save_archived_dependencies(&mut s);
    s.project = original;
    s.revision += 1;
    let staged = stage(&mut s);
    let request = archived_request(&staged, &checkpoint);
    let value = s.execute(ARCHIVE_DEPENDENCIES, request.clone()).unwrap();
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|r| r["kind"] == "sequence-frame"));
    assert!(files.iter().any(|r| r["path"] == third.to_str().unwrap()));
    assert_eq!(value["references"], 2);
    fs::write(dir.0.join("archive_0002.png"), b"generated newly present frame").unwrap();
    assert_eq!(s.execute(ARCHIVE_DEPENDENCIES, request).unwrap()["files"].as_array().unwrap().len(), 3);
    s.shutdown();
}
#[test]
fn archived_recovery_dependencies_refuse_crossed_basis_hash_scope_unknown_paths_rehashed_corruption_and_symlinks_without_repair() {
    let (dir, mut s) = setup();
    let checkpoint = save_archived_dependencies(&mut s);
    let staged = stage(&mut s);
    let request = archived_request(&staged, &checkpoint);
    let file = dir.0.join(format!(".poema-history-{W}-{ARCHIVED_C}.json"));
    let raw = fs::read(&file).unwrap();
    let project = s.project.clone();
    for field in ["basis", "digest", "scope", "epoch", "stage", "extra", "checkpoint"] {
        let mut r = request.clone();
        match field {
            "basis" => r["checkpoint"]["basis"]["revision"] = json!(999999),
            "digest" => r["checkpoint"]["sha256"] = json!("f".repeat(64)),
            "scope" => r["owner"]["scope"]["projectId"] = json!("p_foreign"),
            "epoch" => r["owner"]["nativeSession"] = json!("f".repeat(32)),
            "stage" => r["owner"]["stageSha256"] = json!("f".repeat(64)),
            "extra" => r["checkpoint"]["path"] = json!(file),
            _ => r["checkpoint"]["checkpoint"] = json!("not-a-uuid"),
        };
        assert!(s.execute(ARCHIVE_DEPENDENCIES, r).is_err(), "{field}");
        assert!(std::sync::Arc::ptr_eq(&project, &s.project));
        assert_eq!(fs::read(&file).unwrap(), raw);
    }
    let mut malformed: Value = serde_json::from_slice(&raw).unwrap();
    malformed["kind"] = json!("foreign-history");
    let malformed = serde_json::to_vec(&malformed).unwrap();
    fs::write(&file, &malformed).unwrap();
    let mut rehashed = request.clone();
    rehashed["checkpoint"]["sha256"] = json!(super::history_checkpoint::digest(&malformed));
    assert!(s.execute(ARCHIVE_DEPENDENCIES, rehashed).is_err());
    assert_eq!(fs::read(&file).unwrap(), malformed);
    fs::write(&file, &raw).unwrap();
    #[cfg(unix)]
    {
        let held = file.with_extension("held");
        fs::rename(&file, &held).unwrap();
        std::os::unix::fs::symlink(&held, &file).unwrap();
        assert!(s.execute(ARCHIVE_DEPENDENCIES, request).is_err());
        assert!(fs::symlink_metadata(&file).unwrap().is_symlink());
    }
    s.shutdown();
}
#[test]
fn archived_recovery_dependencies_cannot_borrow_current_user_picked_permissions_for_a_refused_stored_network_path() {
    let (_dir, mut s) = setup();
    let original = s.project.clone();
    let stored = "file://192.0.2.1/share/generated.MXF";
    native_only_file(&mut s, stored, None, false);
    s.media.allow(stored);
    let checkpoint = save_archived_dependencies(&mut s);
    s.project = original;
    s.revision += 1;
    let staged = stage(&mut s);
    assert!(s.execute(ARCHIVE_DEPENDENCIES, archived_request(&staged, &checkpoint)).is_err());
    assert_eq!(s.media.follow(stored).unwrap(), stored);
    assert_eq!(s.media.path_origin(stored), None);
    s.shutdown();
}
#[test]
fn archived_recovery_dependencies_require_actual_staged_owner_and_never_read_for_candidate_retirement_cancellation_or_changed_saved_basis() {
    let (_dir, mut s) = setup();
    let checkpoint = save_archived_dependencies(&mut s);
    let staged = stage(&mut s);
    let request = archived_request(&staged, &checkpoint);
    let bootstrap = startup(&s, &staged);
    retire(&mut s, &staged);
    assert!(s.execute(ARCHIVE_DEPENDENCIES, request.clone()).is_err());
    let mut candidate = Session::default();
    candidate.open_project_recovery(&bootstrap).unwrap();
    assert!(candidate.execute(ARCHIVE_DEPENDENCIES, request).is_err());
    candidate.shutdown();
    s.shutdown();
    for cancelled in [true, false] {
        let (_dir, mut s) = setup();
        let checkpoint = save_archived_dependencies(&mut s);
        let staged = stage(&mut s);
        let request = archived_request(&staged, &checkpoint);
        if cancelled {
            let h = s.workspace_update_status().unwrap();
            s.execute(
                "agent.cancelWorkspaceUpdate",
                json!({"token":h["token"],"scope":h["scope"],"workspace":W,"update":U,"nativeSession":h["nativeSession"]}),
            )
            .unwrap();
        } else {
            fs::write(s.path.as_ref().unwrap(), b"later human native file must remain").unwrap();
        }
        assert!(s.execute(ARCHIVE_DEPENDENCIES, request).is_err());
        s.shutdown();
    }
}

fn persistence_read(s: &mut Session, staged: &Value) -> Value {
    let h = s.workspace_update_status().unwrap();
    s.execute(
        "agent.readProjectRecoveryDependencies",
        json!({"scope":scope(),"workspace":W,"update":U,
        "pauseToken":h["token"],"nativeSession":h["nativeSession"],"stageSha256":staged["sha256"]}),
    )
    .unwrap()
}
#[test]
fn native_persistence_dependencies_include_exact_preferences_locks_journals_and_companions_without_tickets_or_writes() {
    let (_dir, mut s) = setup();
    bin(&mut s, 11);
    s.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let staged = stage(&mut s);
    let raw = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let graph = s.project.clone();
    let revision = s.revision;
    let before = s.workspace_update_status().unwrap();
    let value = persistence_read(&mut s, &staged);
    assert_eq!(value["version"], 2);
    assert_eq!(value["persistence"]["kind"], "native_project_recovery_persistence");
    let files = value["persistence"]["files"].as_array().unwrap();
    for role in ["preferences", "journal-lock", "journal-snapshot", "journal-metadata", "journal-companion"] {
        assert!(files.iter().any(|f| f["role"] == role), "{role}");
    }
    for file in files {
        let raw = fs::read(file["path"].as_str().unwrap()).unwrap();
        assert_eq!(file["bytes"], raw.len());
        assert_eq!(file["sha256"], crate::saved_file::sha256(&raw));
    }
    assert!(files.iter().any(|f| f["role"] == "journal-lock" && f["bytes"] == 0));
    assert_eq!(persistence_read(&mut s, &staged), value);
    assert_eq!(s.workspace_update_status().unwrap(), before);
    assert_eq!(*s.project, *graph);
    assert_eq!(s.revision, revision);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), raw);
    assert!(value.get("token").is_none());
    s.shutdown();
}
#[test]
fn native_persistence_dependencies_refuse_unknown_or_redirected_members_without_skipping_them() {
    for name in ["credentials.json", "partial-native-state.tmp", "foreign.bin"] {
        let (_dir, mut s) = setup();
        bin(&mut s, 11);
        let staged = stage(&mut s);
        let dir = s.persistence.as_ref().unwrap().session_dir.clone();
        fs::write(dir.join(name), b"generated foreign state").unwrap();
        let h = s.workspace_update_status().unwrap();
        let r = s.execute(
            "agent.readProjectRecoveryDependencies",
            json!({"scope":scope(),"workspace":W,"update":U,
            "pauseToken":h["token"],"nativeSession":h["nativeSession"],"stageSha256":staged["sha256"]}),
        );
        assert!(r.is_err());
        assert_eq!(fs::read(dir.join(name)).unwrap(), b"generated foreign state");
        assert_eq!(s.workspace_update_status().unwrap(), h);
        s.shutdown();
    }
}
#[test]
fn native_persistence_dependencies_preserve_absent_preferences_and_empty_journal_membership() {
    let (_dir, mut s) = setup();
    let staged = stage(&mut s);
    let value = persistence_read(&mut s, &staged);
    let p = &value["persistence"];
    let pref = PathBuf::from(p["dataDirectory"].as_str().unwrap()).join("preferences.json");
    if !pref.exists() {
        assert!(p["absent"].as_array().unwrap().iter().any(|v| v == pref.to_str().unwrap()));
    }
    assert!(p["directories"].as_array().unwrap().len() >= 2);
    for file in p["absent"].as_array().unwrap() {
        assert!(!PathBuf::from(file.as_str().unwrap()).exists());
    }
    s.shutdown();
}

#[cfg(unix)]
fn restored_file(file: &std::path::Path, role: &str) -> Value {
    use std::os::unix::fs::MetadataExt;
    let m = fs::symlink_metadata(file).unwrap();
    let ns = |time: std::time::SystemTime| time.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos().to_string();
    json!({"role":role,"path":file,"sha256":crate::saved_file::sha256(&fs::read(file).unwrap()),"bytes":m.len(),"mode":m.mode() & 0o777,
        "physical":{"dev":m.dev().to_string(),"ino":m.ino().to_string(),"size":m.len().to_string(),"mtimeNs":ns(m.modified().unwrap()),
        "ctimeNs":((m.ctime() as u128) * 1_000_000_000 + m.ctime_nsec() as u128).to_string(),"mode":m.mode().to_string(),"uid":m.uid().to_string()}})
}
#[cfg(unix)]
fn replace_identical(file: &std::path::Path) {
    let temporary = file.with_extension("original-native-restored-copy");
    fs::write(&temporary, fs::read(file).unwrap()).unwrap();
    fs::set_permissions(&temporary, fs::metadata(file).unwrap().permissions()).unwrap();
    fs::rename(temporary, file).unwrap();
}
#[cfg(unix)]
fn restored_request(source: &Session, staged: &Value) -> Value {
    let mut request = startup(source, staged);
    let saved = PathBuf::from(source.path.as_ref().unwrap());
    let pref = source.prefs_path.as_ref().unwrap();
    request["restoration"] = json!({"kind":"native_project_recovery_restoration","version":1,
        "snapshotSha256":staged["sha256"],"activationToken":request["activationToken"],
        "files":[restored_file(&saved,"saved-project"),restored_file(pref,"preferences")]});
    request
}
#[cfg(unix)]
#[test]
fn restored_native_files_reopen_original_snapshot_with_owned_undo_redo_and_exact_editor_state() {
    for sequence in [false, true] {
        let (_dir, mut source) = setup();
        bin(&mut source, 11);
        if sequence {
            first(&mut source, 12);
        }
        source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
        let staged = stage(&mut source);
        let raw = fs::read(staged["path"].as_str().unwrap()).unwrap();
        let graph = source.project.clone();
        let state = serde_json::to_value(&source.state).unwrap();
        retire(&mut source, &staged);
        replace_identical(std::path::Path::new(source.path.as_ref().unwrap()));
        replace_identical(source.prefs_path.as_ref().unwrap());
        let mut fresh = Session::default();
        assert!(fresh.open_project_recovery(&startup(&source, &staged)).is_err());
        assert!(fresh.path.is_none());
        let fence = fresh.open_project_recovery(&restored_request(&source, &staged)).unwrap();
        assert_eq!(fence["status"], "candidate_ready");
        assert_eq!(fence["writeAuthority"], false);
        assert!(fresh.is_display_only());
        assert_eq!(*fresh.project, *graph);
        assert_eq!(serde_json::to_value(&fresh.state).unwrap(), state);
        assert_eq!(fresh.prefs.auto_save.max_versions, 7);
        activate(&mut fresh);
        let commit = if sequence { 12 } else { 11 };
        undo(&mut fresh, 101, commit);
        assert_ne!(*fresh.project, *graph);
        redo(&mut fresh, 102, commit, 101);
        assert_eq!(*fresh.project, *graph);
        fresh.execute("edit.undo", json!({})).unwrap();
        assert_ne!(*fresh.project, *graph);
        fresh.execute("edit.redo", json!({})).unwrap();
        assert_eq!(*fresh.project, *graph);
        assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), raw);
        fresh.shutdown();
    }
}
#[cfg(unix)]
#[test]
fn restored_native_files_require_exact_original_hash_roles_current_physical_identity_and_activation() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let staged = stage(&mut source);
    retire(&mut source, &staged);
    replace_identical(std::path::Path::new(source.path.as_ref().unwrap()));
    replace_identical(source.prefs_path.as_ref().unwrap());
    let valid = restored_request(&source, &staged);
    for case in 0..17 {
        let mut r = valid.clone();
        match case {
            0 => r["restoration"]["snapshotSha256"] = json!("0".repeat(64)),
            1 => r["restoration"]["activationToken"] = json!(token(999)),
            2 => r["restoration"]["files"] = json!([]),
            3 => r["restoration"]["files"][0]["path"] = json!("/different-native.fcproj"),
            4 => r["restoration"]["files"][0]["sha256"] = json!("0".repeat(64)),
            5 => r["restoration"]["files"][0]["physical"]["ino"] = json!("0"),
            6 => r["restoration"]["files"][0]["physical"]["dev"] = json!("0"),
            7 => r["restoration"]["files"][0]["physical"]["mtimeNs"] = json!("0"),
            8 => r["restoration"]["files"][0]["physical"]["ctimeNs"] = json!("0"),
            9 => r["restoration"]["files"][0]["bytes"] = json!(0),
            10 => r["restoration"]["files"][0]["role"] = json!("source-authority"),
            11 => r["restoration"]["files"][0]["mode"] = json!(0),
            12 => r["restoration"]["files"][0]["physical"]["uid"] = json!("invalid"),
            13 => r["restoration"]["files"][0]["physical"]["mode"] = json!("invalid"),
            14 => r["restoration"]["grant"] = json!(true),
            15 => {
                let f = r["restoration"]["files"][0].clone();
                r["restoration"]["files"] = json!([f.clone(), f]);
            }
            _ => r["restoration"]["files"][1]["path"] = json!(source.path),
        }
        let mut fresh = Session::default();
        assert!(fresh.open_project_recovery(&r).is_err(), "case{case}");
        assert!(fresh.path.is_none());
        assert!(fresh.history.undo.is_empty());
    }
    let mut fresh = Session::default();
    fresh.open_project_recovery(&valid).unwrap();
    assert!(fresh.is_display_only());
    fresh.shutdown();
}
#[cfg(unix)]
#[test]
fn restored_native_files_refuse_later_identical_replacement_or_rewrite_before_candidate_publication() {
    for pref in [false, true] {
        let (_dir, mut source) = setup();
        bin(&mut source, 11);
        source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
        let staged = stage(&mut source);
        retire(&mut source, &staged);
        replace_identical(std::path::Path::new(source.path.as_ref().unwrap()));
        replace_identical(source.prefs_path.as_ref().unwrap());
        let r = restored_request(&source, &staged);
        let file = if pref { source.prefs_path.as_ref().unwrap().clone() } else { PathBuf::from(source.path.as_ref().unwrap()) };
        let raw = fs::read(&file).unwrap();
        replace_identical(&file);
        let mut fresh = Session::default();
        assert!(fresh.open_project_recovery(&r).is_err());
        assert!(fresh.path.is_none());
        assert_eq!(fs::read(file).unwrap(), raw);
    }
}
#[cfg(unix)]
#[test]
fn restored_native_files_keep_live_source_and_unknown_recovery_journal_refusals() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let staged = stage(&mut source);
    let request = restored_request(&source, &staged);
    let mut fresh = Session::default();
    assert!(fresh.open_project_recovery(&request).is_err());
    assert!(fresh.path.is_none());
    let session = source.persistence.as_ref().unwrap().session_dir.clone();
    let data = source.persistence.as_ref().unwrap().data_dir.clone();
    retire(&mut source, &staged);
    replace_identical(std::path::Path::new(source.path.as_ref().unwrap()));
    replace_identical(source.prefs_path.as_ref().unwrap());
    let request = restored_request(&source, &staged);
    let foreign = crate::autosave::recovery_root(&data).join("9999999999999-777");
    fs::create_dir(&foreign).unwrap();
    for name in ["lock", "snapshot.fcproj", "session.json"] {
        fs::copy(session.join(name), foreign.join(name)).unwrap();
    }
    let raw = fs::read(foreign.join("session.json")).unwrap();
    assert!(fresh.open_project_recovery(&request).is_err());
    assert!(fresh.path.is_none());
    assert_eq!(fs::read(foreign.join("session.json")).unwrap(), raw);
}
#[cfg(unix)]
#[test]
fn restored_native_files_latest_journal_preserves_later_human_and_owned_history_without_snapshot_replay() {
    for human in [false, true] {
        let (_dir, source, staged, mut request) = newer_journal_source_with_preferences(human, true);
        replace_identical(std::path::Path::new(source.path.as_ref().unwrap()));
        replace_identical(source.prefs_path.as_ref().unwrap());
        request["restoration"] = restored_request(&source, &staged)["restoration"].clone();
        let mut fresh = Session::default();
        let fence = fresh.open_project_recovery(&request).unwrap();
        assert_eq!(fence["capabilities"]["projectRecoveryJournal"], true);
        assert_eq!(*fresh.project, *source.project);
        activate(&mut fresh);
        if human {
            fresh.execute("edit.undo", json!({})).unwrap();
            assert_ne!(*fresh.project, *source.project);
            fresh.execute("edit.redo", json!({})).unwrap();
        } else {
            undo(&mut fresh, 301, 12);
            assert_ne!(*fresh.project, *source.project);
            redo(&mut fresh, 302, 12, 301);
        }
        assert_eq!(*fresh.project, *source.project);
        fresh.shutdown();
    }
}

const PAUSED_HISTORY: &str = "agent.inspectPausedWorkspaceHistory";
fn paused_history_request(s: &Session, saved: &Value, checkpoint: &str, n: usize) -> Value {
    let ready = s.workspace_update_status().unwrap();
    json!({"token":token(n),"scope":scope(),"workspace":W,"update":U,"projectPath":s.path,
        "activationToken":ready["token"],"nativeSession":ready["nativeSession"],"revision":ready["revision"],
        "sequence":s.state.active_sequence.map(|id|id.0),"checkpoint":checkpoint,"sha256":saved["sha256"]})
}
#[test]
fn paused_workspace_history_validates_original_checkpoint_without_editable_tickets_or_activation() {
    for sequence in [false, true] {
        let (_d, mut source) = setup();
        bin(&mut source, 11);
        if sequence {
            first(&mut source, 12);
        }
        let staged = stage(&mut source);
        let saved = source.agent_bridge.history_checkpoints.len();
        assert!(saved > 0);
        let startup = startup(&source, &staged);
        retire(&mut source, &staged);
        let mut candidate = Session::default();
        candidate.open_project_recovery(&startup).unwrap();
        install(&mut candidate);
        let checkpoint = json!({"sha256":staged["checkpointSha256"]});
        let p = paused_history_request(&candidate, &checkpoint, C, 401);
        let before = (
            candidate.project.clone(),
            serde_json::to_value(&candidate.state).unwrap(),
            candidate.revision,
            candidate.history.undo.len(),
            candidate.history.redo.len(),
            candidate.agent_bridge.tickets.len(),
            candidate.agent_bridge.history_bases.len(),
        );
        let result = candidate.execute(PAUSED_HISTORY, p.clone()).unwrap();
        assert_eq!(result["status"], "retained");
        assert_eq!(result["editingPaused"], true);
        assert_eq!(result["checkpoint"]["sha256"], checkpoint["sha256"]);
        assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
        assert!(std::sync::Arc::ptr_eq(&candidate.project, &before.0));
        assert_eq!(
            (
                serde_json::to_value(&candidate.state).unwrap(),
                candidate.revision,
                candidate.history.undo.len(),
                candidate.history.redo.len(),
                candidate.agent_bridge.tickets.len(),
                candidate.agent_bridge.history_bases.len()
            ),
            (before.1, before.2, before.3, before.4, before.5, before.6)
        );
        assert!(candidate.execute(PAUSED_HISTORY, p.clone()).is_err());
        assert!(candidate.execute("agent.apply", json!({"token":p["token"],"label":"Forbidden","exact":true,"operations":[]})).is_err());
        assert!(
            candidate
                .execute(
                    "agent.restoreHistoryCheckpoint",
                    json!({"token":p["token"],"scope":scope(),"workspace":W,"checkpoint":C,"sha256":checkpoint["sha256"]})
                )
                .is_err()
        );
        // Original persistence bases still cannot qualify the original retention operation.
        basis(&mut candidate, 402);
        assert!(
            candidate
                .execute(
                    "agent.validateRetainedHistory",
                    json!({"token":token(402),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":checkpoint["sha256"],"metadataBasis":true})
                )
                .is_err()
        );
        activate(&mut candidate);
        let mut after = p;
        after["token"] = json!(token(403));
        assert!(candidate.execute(PAUSED_HISTORY, after).is_err());
    }
}
#[test]
fn paused_workspace_history_rejects_hostile_or_foreign_candidate_metadata_without_changing_the_graph() {
    let (_d, mut source) = setup();
    bin(&mut source, 11);
    let staged = stage(&mut source);
    let startup = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut candidate = Session::default();
    candidate.open_project_recovery(&startup).unwrap();
    install(&mut candidate);
    let checkpoint = json!({"sha256":staged["checkpointSha256"]});
    let original = paused_history_request(&candidate, &checkpoint, C, 501);
    let before = candidate.project.clone();
    let revision = candidate.revision;
    for (i, (field, value)) in [
        ("token", json!("short")),
        ("scope", json!({"projectId":"foreign","workspaceId":W})),
        ("workspace", json!(U)),
        ("update", json!(C)),
        ("projectPath", json!("/foreign.fcproj")),
        ("activationToken", json!(token(999))),
        ("nativeSession", json!(token(999))),
        ("revision", json!(0)),
        ("sequence", json!(1)),
        ("checkpoint", json!("../foreign")),
        ("sha256", json!("0".repeat(64))),
        ("extra", json!(true)),
        ("metadataBasis", json!(true)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut p = original.clone();
        p["token"] = json!(token(510 + i));
        p[field] = value;
        assert!(candidate.execute(PAUSED_HISTORY, p).is_err(), "{field}");
        assert!(std::sync::Arc::ptr_eq(&candidate.project, &before));
        assert_eq!(candidate.revision, revision);
        assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    }
    let mut p = original.clone();
    p.as_object_mut().unwrap().remove("sequence");
    assert!(candidate.execute(PAUSED_HISTORY, p).is_err());
    basis(&mut candidate, 550);
    let mut p = original.clone();
    p["token"] = json!(token(550));
    assert!(candidate.execute(PAUSED_HISTORY, p).is_err());
    candidate.execute(PAUSED_HISTORY, original).unwrap();
}
#[test]
fn paused_workspace_history_retains_exact_newer_journal_graph_and_refuses_equal_value_replacement() {
    let (_d, mut source, _cached, request) = newer_journal_source(false);
    let checkpoint = "66666666-6666-4666-8666-666666666666";
    basis(&mut source, 70);
    let saved = source
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(70),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    let mut candidate = Session::default();
    candidate.open_project_recovery(&request).unwrap();
    install(&mut candidate);
    let p = paused_history_request(&candidate, &saved["checkpoint"], checkpoint, 601);
    let result = candidate.execute(PAUSED_HISTORY, p).unwrap();
    assert_eq!(result["checkpoint"], saved["checkpoint"]);
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    let mut replacement = Session::default();
    replacement.open_project_recovery(&request).unwrap();
    install(&mut replacement);
    let p = paused_history_request(&replacement, &saved["checkpoint"], checkpoint, 602);
    replacement.project = std::sync::Arc::new((*replacement.project).clone());
    assert!(replacement.execute(PAUSED_HISTORY, p).is_err());
    activate(&mut candidate);
    undo(&mut candidate, 603, 12);
    redo(&mut candidate, 604, 12, 603);
}
