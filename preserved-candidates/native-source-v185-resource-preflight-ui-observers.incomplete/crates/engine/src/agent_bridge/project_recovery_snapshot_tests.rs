use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
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

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod managed_save_startup_tests {
    use super::*;
    use std::os::unix::fs::OpenOptionsExt;
    fn control(path: &std::path::Path, value: &Value) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
        f.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
        f.sync_all().unwrap();
    }
    fn candidate() -> (Scratch, Session, PathBuf) {
        let (dir, mut source) = setup();
        let staged = stage(&mut source);
        let request = startup(&source, &staged);
        retire(&mut source, &staged);
        let mut fresh = Session::default();
        fresh.open_project_recovery(&request).unwrap();
        fresh.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("save-candidate-state"))).unwrap();
        let manifest = dir.0.join("workspace.json");
        control(&manifest, &json!({"kind":"poema-saved-native-workspace","version":3,"id":W,"projectId":"p_complete_snapshot","projectPath":fresh.path}));
        control(&dir.0.join("workspace.json.lock"), &json!({"pid":std::process::id(),"identity":U}));
        (dir, fresh, manifest)
    }
    #[test]
    fn original_paused_candidate_retains_same_startup_manager_without_save_or_persistence_until_real_activation() {
        let (dir, mut s, manifest) = candidate();
        let original = fs::read(s.path.as_ref().unwrap()).unwrap();
        assert!(s.install_managed_save_root(&manifest).is_err());
        s.retain_managed_save_startup_root(&manifest).unwrap();
        let retained = s.agent_bridge.manager_save_startup.clone().unwrap();
        assert!(s.agent_bridge.manager_save_origin.is_none());
        assert!(!s.managed_save_dispatch_available());
        assert!(s.is_display_only());
        assert!(s.execute("file.save", json!({})).is_err());
        assert!(!dir.0.join("save-candidate-state/preferences.json").exists());
        activate(&mut s);
        assert!(s.managed_save_dispatch_available());
        assert!(!s.is_display_only());
        assert!(s.agent_bridge.manager_save_startup.is_none());
        assert!(std::sync::Arc::ptr_eq(&retained, s.agent_bridge.manager_save_origin.as_ref().unwrap()));
        assert!(s.retain_managed_save_startup_root(&manifest).is_err());
        assert!(s.install_managed_save_root(&manifest).is_err());
        assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), original);
        s.shutdown();
    }
    #[test]
    fn replaced_original_startup_manager_refuses_activation_before_native_persistence_or_worker_resume() {
        let (dir, mut s, manifest) = candidate();
        s.retain_managed_save_startup_root(&manifest).unwrap();
        assert!(s.retain_managed_save_startup_root(&manifest).is_err());
        let file = dir.0.join("workspace.json.lock");
        let raw = fs::read(&file).unwrap();
        let replacement = dir.0.join("same-byte-lease");
        control(&replacement, &serde_json::from_slice(&raw).unwrap());
        fs::rename(replacement, file).unwrap();
        install(&mut s);
        let fence = s.workspace_update_status().unwrap();
        assert!(s.execute("agent.activateWorkspaceUpdate",json!({"token":fence["token"],"scope":fence["scope"],"workspace":fence["workspace"],"update":fence["update"],"nativeSession":fence["nativeSession"]})).is_err());
        assert!(s.is_display_only());
        assert!(!s.managed_save_dispatch_available());
        assert!(s.agent_bridge.manager_save_origin.is_none());
        assert!(!dir.0.join("save-candidate-state/preferences.json").exists());
        s.shutdown();
    }
    #[test]
    fn startup_manager_cannot_be_installed_by_control_or_ordinary_candidate_description_and_missing_controls_never_create_files() {
        let (dir, mut s, manifest) = candidate();
        let missing = dir.0.join("missing.json");
        assert!(s.retain_managed_save_startup_root(&missing).is_err());
        assert!(!missing.exists());
        assert!(s.agent_bridge.manager_save_startup.is_none());
        assert!(s.execute("agent.installManagedSaveRoot", json!({"manifestPath":manifest})).is_err());
        activate(&mut s);
        assert!(!s.managed_save_dispatch_available());
        assert!(s.retain_managed_save_startup_root(&manifest).is_err());
        s.shutdown();
    }
}

fn snapshot_inspection_candidate() -> (Scratch, Session, Value, Value) {
    let (dir, mut source) = setup();
    bin(&mut source, 11);
    source.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_997);
    let staged = stage(&mut source);
    let request = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut candidate = Session::default();
    let fence = candidate.open_project_recovery(&request).unwrap();
    install(&mut candidate);
    (dir, candidate, staged, fence)
}
fn snapshot_inspection_request(staged: &Value, fence: &Value, n: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"workspace":W,"update":U,"projectPath":staged["projectPath"],
        "activationToken":fence["token"],"nativeSession":fence["nativeSession"],"revision":fence["revision"],
        "sequence":staged["sequence"],"checkpoint":staged["checkpoint"],"sha256":staged["checkpointSha256"],
        "snapshotSha256":staged["sha256"]})
}
#[test]
fn paused_snapshot_inspection_compares_complete_original_history_and_editor_without_activation_or_retirement_authority() {
    let (_dir, mut candidate, staged, fence) = snapshot_inspection_candidate();
    let graph = candidate.project.clone();
    let snapshot = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let proof = candidate.execute("agent.inspectPausedWorkspaceHistory", snapshot_inspection_request(&staged, &fence, 200)).unwrap();
    assert_eq!(proof["snapshot"]["sha256"], staged["sha256"]);
    assert_eq!(proof["snapshot"]["bytes"], staged["bytes"]);
    assert_eq!(proof["snapshot"]["editorUnchanged"], true);
    assert_eq!(proof["snapshot"]["retirementAuthority"], false);
    assert!(std::sync::Arc::ptr_eq(&candidate.project, &graph));
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), snapshot);
    assert!(!candidate.save_operation_held());
    candidate.shutdown();
}
#[test]
fn paused_snapshot_inspection_rejects_changed_editor_even_when_the_project_graph_revision_and_history_are_unchanged() {
    let (_dir, mut candidate, staged, fence) = snapshot_inspection_candidate();
    let graph = candidate.project.clone();
    candidate.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    assert!(candidate.execute("agent.inspectPausedWorkspaceHistory", snapshot_inspection_request(&staged, &fence, 200)).is_err());
    assert!(std::sync::Arc::ptr_eq(&candidate.project, &graph));
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    candidate.shutdown();
}
#[test]
fn paused_snapshot_inspection_rejects_changed_complete_snapshot_and_crossed_original_hashes_without_retiring() {
    for changed_file in [false, true] {
        let (_dir, mut candidate, staged, fence) = snapshot_inspection_candidate();
        let mut request = snapshot_inspection_request(&staged, &fence, 200);
        if changed_file {
            fs::write(staged["path"].as_str().unwrap(), b"{}").unwrap();
        } else {
            request["sha256"] = json!("f".repeat(64));
        }
        assert!(candidate.execute("agent.inspectPausedWorkspaceHistory", request).is_err());
        assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
        assert!(!candidate.save_operation_held());
        candidate.shutdown();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_image_candidate() -> (Scratch, Session, Value, Value) {
    let (dir, mut candidate, staged, fence) = snapshot_inspection_candidate();
    candidate.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("current-paused-state"))).unwrap();
    (dir, candidate, staged, fence)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_image_request(fence: &Value, path: &Value, n: usize, action: &str) -> Value {
    json!({"token":token(n),"scope":scope(),"workspace":W,"update":U,"projectPath":path,
        "activationToken":fence["token"],"nativeSession":fence["nativeSession"],"capture":"44444444-4444-4444-8444-444444444444","action":action})
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_image_retains_actual_changed_editor_history_and_deferred_resources_without_starting_persistence() {
    let (dir, mut candidate, staged, fence) = current_image_candidate();
    candidate.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    let resources = dir.0.join("current-paused-state/Graphics Templates");
    fs::create_dir(&resources).unwrap();
    fs::write(resources.join("current.json"), b"current resource bytes").unwrap();
    assert!(candidate.execute("agent.inspectPausedWorkspaceHistory", snapshot_inspection_request(&staged, &fence, 200)).is_err());
    let graph = candidate.project.clone();
    let saved = fs::read(staged["projectPath"].as_str().unwrap()).unwrap();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut candidate).unwrap();
    assert!(crate::session_worker::supports("agent.preparePausedProjectRecovery"));
    let mut pending = worker
        .start(&mut candidate, "agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 201, "prepare"), None)
        .unwrap();
    assert!(candidate.view_only);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let done = loop {
        if let Some(done) = pending.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let proof = done.reply.result.unwrap();
    candidate = done.session;
    assert_eq!(proof["status"], "prepared");
    assert_eq!(proof["resourceNodes"], 2);
    assert_eq!(proof["retirementAuthority"], false);
    assert_eq!(proof["startupAuthority"], false);
    let again = candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 202, "read")).unwrap();
    assert_eq!(again["sha256"], proof["sha256"]);
    assert!(std::sync::Arc::ptr_eq(&candidate.project, &graph));
    assert!(candidate.persistence.is_none());
    assert!(!candidate.save_operation_held());
    assert_eq!(fs::read(staged["projectPath"].as_str().unwrap()).unwrap(), saved);
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    candidate.state.source_playhead = filmcraft_time::Tick(9_007_199_254_741_001);
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 203, "read")).is_err());
    candidate.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_image_refuses_same_byte_replaced_resources_and_namespace_additions_without_recapture() {
    for replace in [true, false] {
        let (dir, mut candidate, staged, fence) = current_image_candidate();
        let file = dir.0.join("current-paused-state/effect-presets.json");
        fs::write(&file, b"same bytes").unwrap();
        candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 200, "prepare")).unwrap();
        if replace {
            let other = file.with_extension("replacement");
            fs::write(&other, b"same bytes").unwrap();
            fs::rename(other, &file).unwrap();
        } else {
            fs::write(dir.0.join("current-paused-state/new-resource.json"), b"new resource").unwrap();
        }
        assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 201, "read")).is_err());
        assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 202, "prepare")).is_err());
        assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
        candidate.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_image_refuses_replaced_saved_file_and_metadata_without_the_actual_live_image() {
    let (_dir, mut candidate, staged, fence) = current_image_candidate();
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 200, "read")).is_err());
    candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 201, "prepare")).unwrap();
    let path = PathBuf::from(staged["projectPath"].as_str().unwrap());
    let copy = path.with_extension("replacement");
    fs::write(&copy, fs::read(&path).unwrap()).unwrap();
    fs::rename(copy, path).unwrap();
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 202, "read")).is_err());
    assert!(candidate.persistence.is_none());
    candidate.shutdown();
    // Equal project bytes do not replace the actual admitted native graph.
    // Nor may a current-view capture silently adopt a changed Undo contract.
    for replace_graph in [true, false] {
        let (_dir, mut candidate, staged, fence) = current_image_candidate();
        if replace_graph {
            candidate.project = std::sync::Arc::new((*candidate.project).clone());
        } else {
            candidate.history.limit = candidate.history.limit.saturating_add(1);
        }
        assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 204, "prepare")).is_err());
        assert!(candidate.persistence.is_none());
        assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
        candidate.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_image_refuses_symlinks_and_cancelled_original_operations_before_memory_publication() {
    let (dir, mut candidate, staged, fence) = current_image_candidate();
    let target = dir.0.join("external.json");
    fs::write(&target, b"external").unwrap();
    std::os::unix::fs::symlink(&target, dir.0.join("current-paused-state/linked.json")).unwrap();
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 200, "prepare")).is_err());
    fs::remove_file(dir.0.join("current-paused-state/linked.json")).unwrap();
    let stopped_request = current_image_request(&fence, &staged["projectPath"], 201, "prepare");
    workspace_update::prepare_paused_image_operation(&mut candidate, &stopped_request).unwrap();
    assert_eq!(candidate.agent_operation_control().cancel(&json!({"token":token(201)})).unwrap()["cancellationAccepted"], true);
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 201, "prepare")).is_err());
    assert!(candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 202, "read")).is_err());
    assert!(candidate.persistence.is_none());
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    candidate.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
struct PausedPublicationManager {
    process: std::process::Child,
    path: PathBuf,
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl Drop for PausedPublicationManager {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn paused_publication_private(path: &std::path::Path, value: &Value) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn paused_publication_manager(dir: &Scratch, staged: &Value) -> PausedPublicationManager {
    let process = std::process::Command::new("/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let root = dir.0.join("publication-manager");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join(format!("workspace-state-{W}"))).unwrap();
    let path = root.join("workspace.json");
    paused_publication_private(
        &path,
        &json!({"kind":"poema-saved-native-workspace","id":W,"projectId":scope()["projectId"],"projectPath":staged["projectPath"]}),
    );
    paused_publication_private(&PathBuf::from(format!("{}.lock", path.display())), &json!({"pid":process.id(),"identity":C}));
    PausedPublicationManager { process, path }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn paused_publication_request(staged: &Value, fence: &Value, manager: &PausedPublicationManager, n: usize, action: &str) -> Value {
    let mut r = current_image_request(fence, &staged["projectPath"], n, action);
    r["manifestPath"] = json!(manager.path);
    r["reserveBytes"] = json!(1024 * 1024);
    r
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_publication_uses_actual_current_image_and_durable_original_manager_intent_without_starting_persistence() {
    let (dir, mut s, staged, fence) = current_image_candidate();
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    let manager = paused_publication_manager(&dir, &staged);
    let image = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 300, "prepare")).unwrap();
    let graph = s.project.clone();
    let saved = fs::read(staged["projectPath"].as_str().unwrap()).unwrap();
    let prepare = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 301, "prepare")).unwrap();
    let target = PathBuf::from(prepare["record"]["receipt"]["intent"]["destination"].as_str().unwrap());
    assert!(!target.exists());
    assert_eq!(prepare["frameSha256"], image["sha256"]);
    assert_eq!(prepare["frameBytes"], image["bytes"]);
    assert!(!s.save_operation_held());
    assert!(s.persistence.is_none());
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 302, "publish")).is_err());
    assert!(!target.exists());
    paused_publication_private(std::path::Path::new(prepare["intentPath"].as_str().unwrap()), &prepare["intent"]);
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    assert!(crate::session_worker::supports(workspace_update::PAUSED_RECOVERY_RECORD));
    let mut pending =
        worker.start(&mut s, workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 303, "publish"), None).unwrap();
    assert!(s.view_only);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let done = loop {
        if let Some(done) = pending.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let published = done.reply.result.unwrap();
    s = done.session;
    assert_eq!(published["record"]["matches_published"], true);
    assert_eq!(published["record"]["receipt"]["phase"], "published");
    let raw = fs::read(&target).unwrap();
    assert_eq!(raw.len() as u64, image["bytes"].as_u64().unwrap());
    assert_eq!(history_checkpoint::digest(&raw), image["sha256"].as_str().unwrap());
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["kind"], "native-paused-current-image");
    assert_eq!(body["files"]["source"]["kind"], "native-original-resource-frames");
    assert_eq!(body["files"]["source"]["persistence"]["kind"], "native_project_recovery_persistence");
    let original_nodes = body["files"]["source"]["nodes"].as_object().unwrap();
    for name in ["Templates", "Label Presets"] {
        assert!(original_nodes.contains_key(dir.0.join("source-state").join(name).to_str().unwrap()));
    }
    assert_eq!(body["editor"]["editor"], serde_json::to_value(&s.state).unwrap());
    assert_eq!(published["startupAuthority"], false);
    assert_eq!(published["retirementAuthority"], false);
    assert_eq!(published["workersMayResume"], false);
    assert!(std::sync::Arc::ptr_eq(&s.project, &graph));
    assert!(s.persistence.is_none());
    assert_eq!(fs::read(staged["projectPath"].as_str().unwrap()).unwrap(), saved);
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert_eq!(
        s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 304, "read")).unwrap()["record"]["matches_published"],
        true
    );
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_publication_refuses_same_byte_replaced_original_manager_controls_before_file_io_and_still_observes_the_original_record() {
    for lease in [true, false] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let manager = paused_publication_manager(&dir, &staged);
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 300, "prepare")).unwrap();
        let prepared = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 301, "prepare")).unwrap();
        paused_publication_private(std::path::Path::new(prepared["intentPath"].as_str().unwrap()), &prepared["intent"]);
        let file = if lease { PathBuf::from(format!("{}.lock", manager.path.display())) } else { manager.path.clone() };
        let replacement = file.with_extension("replacement");
        let value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        paused_publication_private(&replacement, &value);
        fs::rename(replacement, &file).unwrap();
        assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 302, "publish")).is_err());
        let read = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 303, "read")).unwrap();
        assert_eq!(read["record"]["receipt"]["call_attempted"], false);
        assert!(!std::path::Path::new(read["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).exists());
        assert_eq!(read["current"], false);
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_publication_refuses_crossed_journal_changed_editor_foreign_manager_and_unknown_parameters_without_replacing_image() {
    for change in ["journal", "editor", "manager", "unknown", "reserve"] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let manager = paused_publication_manager(&dir, &staged);
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 300, "prepare")).unwrap();
        let prepared = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 301, "prepare")).unwrap();
        let mut intent = prepared["intent"].clone();
        if change == "journal" {
            intent["intent"]["reserve_bytes"] = json!(17);
        }
        paused_publication_private(std::path::Path::new(prepared["intentPath"].as_str().unwrap()), &intent);
        let mut request = paused_publication_request(&staged, &fence, &manager, 302, "publish");
        match change {
            "editor" => s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_741_001),
            "manager" => request["manifestPath"] = json!(dir.0.join("foreign.json")),
            "unknown" => request["path"] = json!(dir.0.join("arbitrary.json")),
            "reserve" => request["reserveBytes"] = json!(0),
            _ => {}
        }
        assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, request).is_err(), "{change}");
        let read = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 303, "read")).unwrap();
        assert_eq!(read["record"]["receipt"]["call_attempted"], false);
        assert_eq!(read["startupAuthority"], false);
        assert!(!std::path::Path::new(read["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).exists());
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_publication_observes_same_byte_artifact_replacement_without_republishing_or_claiming_currentness() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mut s, staged, fence) = current_image_candidate();
    let manager = paused_publication_manager(&dir, &staged);
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 300, "prepare")).unwrap();
    let prepared = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 301, "prepare")).unwrap();
    paused_publication_private(std::path::Path::new(prepared["intentPath"].as_str().unwrap()), &prepared["intent"]);
    s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 302, "publish")).unwrap();
    let target = PathBuf::from(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap());
    let replacement = target.with_extension("replacement");
    let bytes = fs::read(&target).unwrap();
    fs::write(&replacement, &bytes).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(replacement, &target).unwrap();
    let observed = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 303, "read")).unwrap();
    assert_eq!(observed["record"]["matches_published"], false);
    assert_eq!(observed["record"]["receipt"]["phase"], "published");
    assert_eq!(observed["current"], false);
    assert_eq!(observed["retirementAuthority"], false);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 304, "publish")).is_err());
    assert_eq!(fs::read(target).unwrap(), bytes);
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_publication_requires_original_image_and_honors_cancelled_original_operation_before_memory_publication() {
    let (dir, mut s, staged, fence) = current_image_candidate();
    let manager = paused_publication_manager(&dir, &staged);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 300, "prepare")).is_err());
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 301, "prepare")).unwrap();
    let request = paused_publication_request(&staged, &fence, &manager, 302, "prepare");
    workspace_update::prepare_paused_record_operation(&mut s, &request).unwrap();
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(302)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, request).is_err());
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 303, "read")).is_err());
    assert!(s.persistence.is_none());
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    s.shutdown();
}

#[test]
fn auxiliary_recovery_root_cold_start_and_activation_preserve_live_libraries() {
    let (dir, mut source) = setup();
    super::native_auxiliary_tests::populate(&mut source);
    bin(&mut source, 11);
    let expected = super::native_auxiliary::Snapshot::capture(&source).unwrap();
    let staged = stage(&mut source);
    let request = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut cold = Session::default();
    cold.open_project_recovery(&request).unwrap();
    assert!(expected.matches(&cold).unwrap());
    let destination = dir.0.join("auxiliary-candidate-state");
    cold.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(destination.clone())).unwrap();
    // Match the original manager's missing-only resource transfer before
    // activation; the native publisher must never adopt an unproven copy.
    let copied = destination.join("Keyboard Shortcuts/active.json");
    fs::create_dir_all(copied.parent().unwrap()).unwrap();
    fs::copy(dir.0.join("source-state/Keyboard Shortcuts/active.json"), &copied).unwrap();
    activate(&mut cold);
    assert_eq!(cold.presets.user.len(), source.presets.user.len());
    assert_eq!(cold.export_presets.user.len(), source.export_presets.user.len());
    assert_eq!(cold.export_presets.favorites, source.export_presets.favorites);
    assert!(cold.shortcuts.can_redo());
    assert_eq!(serde_json::to_value(cold.presets.recovery()).unwrap()["directory"], json!(destination));
    assert_eq!(cold.export_presets.dir(), Some(destination.as_path()));
    cold.shutdown();
}

#[test]
fn auxiliary_recovery_shared_persistence_rebinding_preserves_memory_over_stale_library_files() {
    let (dir, mut source) = setup();
    super::native_auxiliary_tests::populate(&mut source);
    let snapshot = super::native_auxiliary::Snapshot::capture(&source).unwrap();
    source.shutdown();
    let destination = dir.0.join("stale-library-state");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("effect-presets.json"), br#"{"format":"filmcraft.effect-presets","version":1,"presets":[]}"#).unwrap();
    fs::write(destination.join("export-presets.json"), br#"{"format":"filmcraft.export-presets","version":1,"presets":[],"favorites":[]}"#).unwrap();
    let mut stale = crate::shortcuts::Shortcuts::new();
    stale.set_dir(&destination);
    stale.set("file.save", "Cmd+Alt+P", None, false, true).unwrap();
    let mut restored = Session::default();
    snapshot.install(&mut restored).unwrap();
    restored.start_workspace_persistence(crate::autosave::AutosaveConfig::new(destination.clone())).unwrap();
    assert_eq!(restored.shortcuts.primary("test.auxiliary"), source.shortcuts.primary("test.auxiliary"));
    assert!(restored.shortcuts.can_redo());
    assert_eq!(restored.presets.user, source.presets.user);
    assert_eq!(restored.export_presets.favorites, source.export_presets.favorites);
    assert_eq!(restored.export_presets.user.len(), source.export_presets.user.len());
    assert!(restored.export_presets.user[0].builtin);
    assert_eq!(serde_json::to_value(restored.presets.recovery()).unwrap()["directory"], json!(destination));
    restored.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_resource_frames_refuse_identical_replacement_in_every_original_authored_library() {
    for relative in [
        "Keyboard Shortcuts/active.json",
        "effect-presets.json",
        "export-presets.json",
        "Graphics Templates/look.fcgt",
        "Templates/project.fcproj",
        "Label Presets/color.json",
    ] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let file = dir.0.join("source-state").join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"generated original authored resource").unwrap();
        let graph = s.project.clone();
        let bytes = fs::read(&file).unwrap();
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 400, "prepare")).unwrap();
        replace_identical(&file);
        assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 401, "read")).is_err(), "{relative}");
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert!(std::sync::Arc::ptr_eq(&s.project, &graph));
        assert!(s.persistence.is_none());
        assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_resource_frames_retain_absence_of_all_authored_source_names() {
    for relative in ["Keyboard Shortcuts", "effect-presets.json", "export-presets.json", "Graphics Templates", "Templates", "Label Presets"] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let path = dir.0.join("source-state").join(relative);
        assert!(!path.exists());
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 410, "prepare")).unwrap();
        fs::write(&path, b"later external authored choice").unwrap();
        assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 411, "read")).is_err(), "{relative}");
        assert_eq!(fs::read(&path).unwrap(), b"later external authored choice");
        assert!(s.persistence.is_none());
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_resource_frames_refuse_redirected_empty_excessive_and_deep_source_libraries_before_memory_publication() {
    use std::os::unix::fs::symlink;
    for mode in ["redirected", "empty", "large", "deep"] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let root = dir.0.join("source-state/Templates");
        if mode == "redirected" {
            symlink(&dir.0, &root).unwrap();
        } else {
            fs::create_dir(&root).unwrap();
            let file = root.join("generated.fcproj");
            if mode == "deep" {
                let mut path = root;
                for _ in 0..18 {
                    path = path.join("nested");
                    fs::create_dir(&path).unwrap();
                }
            } else {
                let file = fs::File::create(file).unwrap();
                if mode == "large" {
                    file.set_len(64 * 1024 * 1024 + 1).unwrap();
                }
            }
        }
        assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 420, "prepare")).is_err(), "{mode}");
        assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 421, "read")).is_err());
        assert!(s.persistence.is_none());
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_resource_frames_refuse_publication_intent_inside_the_original_authored_namespace() {
    let (dir, mut s, staged, fence) = current_image_candidate();
    let mut manager = paused_publication_manager(&dir, &staged);
    let root = dir.0.join("source-state/Templates");
    fs::create_dir(&root).unwrap();
    let target = root.join("Manager");
    fs::rename(manager.path.parent().unwrap(), &target).unwrap();
    manager.path = target.join("workspace.json");
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 430, "prepare")).unwrap();
    let error = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 431, "prepare")).unwrap_err();
    assert!(error.to_string().contains("outside the complete retained resource namespace"));
    assert!(fs::read_dir(target.join(format!("workspace-state-{W}"))).unwrap().next().is_none());
    assert!(s.persistence.is_none());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_resource_frames_preserve_original_persistence_metadata_exclusions_and_allow_unrelated_root_names() {
    let (dir, mut s, staged, fence) = current_image_candidate();
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 440, "prepare")).unwrap();
    fs::write(dir.0.join("source-state/unrelated-cache.tmp"), b"derived cache metadata").unwrap();
    fs::write(crate::autosave::recovery_root(&dir.0.join("source-state")).join(".DS_Store"), b"derived Finder metadata").unwrap();
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 441, "read")).unwrap();
    assert!(s.persistence.is_none());
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn library_durability_source() -> (Scratch, Session, Vec<(PathBuf, Vec<u8>)>) {
    let (dir, mut source) = setup();
    super::native_auxiliary_tests::populate(&mut source);
    for p in &mut source.presets.user {
        p.builtin = false;
        p.name = "Authored recovery look".into();
    }
    for p in &mut source.export_presets.user {
        p.builtin = false;
        p.name = "Authored recovery delivery".into();
    }
    source.export_presets.favorites = vec!["Authored recovery delivery".into()];
    let root = dir.0.join("source-state");
    let old = vec![
        (root.join("Keyboard Shortcuts/active.json"), crate::shortcuts::Shortcuts::new().recovery_file_bytes().unwrap()),
        (root.join("effect-presets.json"), crate::presets::PresetLibrary::default().recovery_file_bytes().unwrap()),
        (root.join("export-presets.json"), crate::export_tools::ExportPresetLibrary::default().recovery_file_bytes().unwrap()),
    ];
    for (path, bytes) in &old {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    bin(&mut source, 11);
    (dir, source, old)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn library_durability_candidate(dir: &Scratch, source: &mut Session, original: &[(PathBuf, Vec<u8>)]) -> (Session, PathBuf) {
    let staged = stage(source);
    let request = startup(source, &staged);
    retire(source, &staged);
    let mut cold = Session::default();
    cold.open_project_recovery(&request).unwrap();
    let target = dir.0.join("durable-library-candidate");
    cold.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(target.clone())).unwrap();
    for (path, bytes) in original {
        let relative = path.strip_prefix(dir.0.join("source-state")).unwrap();
        let to = target.join(relative);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(to, bytes).unwrap();
    }
    (cold, target)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_durability_activation_publishes_actual_native_state_and_plain_cold_library_load_reads_it() {
    let (dir, mut source, original) = library_durability_source();
    let (mut cold, target) = library_durability_candidate(&dir, &mut source, &original);
    let graph = cold.project.clone();
    activate(&mut cold);
    assert!(cold.persistence.is_some());
    assert!(std::sync::Arc::ptr_eq(&cold.project, &graph));
    let mut fresh = Session::default();
    fresh.shortcuts.set_dir(&target);
    fresh.presets.set_dir(&target);
    fresh.export_presets.set_dir(&target);
    assert_eq!(fresh.shortcuts.primary("test.auxiliary"), cold.shortcuts.primary("test.auxiliary"));
    assert_eq!(fresh.presets.user, cold.presets.user);
    assert_eq!(fresh.export_presets.favorites, cold.export_presets.favorites);
    assert_eq!(serde_json::to_value(&fresh.export_presets.user).unwrap(), serde_json::to_value(&cold.export_presets.user).unwrap());
    assert!(cold.shortcuts.can_redo());
    for (path, bytes) in &original {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
    let mut preserved = 0;
    for parent in [&target, &target.join("Keyboard Shortcuts")] {
        for entry in fs::read_dir(parent).unwrap() {
            let path = entry.unwrap().path();
            if !path.file_name().unwrap().to_string_lossy().starts_with(".poema-save-") {
                continue;
            }
            let observation = filmcraft_format::retained::inspect(&path, &|| Ok(())).unwrap();
            assert_eq!(observation.current, filmcraft_format::retained::CurrentState::Published);
            let receipt = observation.recorded_receipt.unwrap();
            if let Some(displaced) = receipt.displaced {
                assert!(original.iter().any(|(_, bytes)| crate::saved_file::sha256(bytes) == displaced.sha256));
                preserved += 1;
            }
        }
    }
    assert_eq!(preserved, 3);
    cold.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_durability_conflicting_candidate_file_refuses_before_any_canonical_publication_or_worker_start() {
    let (dir, mut source, original) = library_durability_source();
    let (mut cold, target) = library_durability_candidate(&dir, &mut source, &original);
    fs::write(target.join("export-presets.json"), b"later human library data").unwrap();
    let before = fs::read(target.join("effect-presets.json")).unwrap();
    install(&mut cold);
    let h = cold.workspace_update_status().unwrap();
    let reply = cold.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]}),
    );
    assert!(reply.is_err());
    assert!(cold.persistence.is_none());
    assert_eq!(cold.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert_eq!(fs::read(target.join("effect-presets.json")).unwrap(), before);
    assert_eq!(fs::read(target.join("export-presets.json")).unwrap(), b"later human library data");
    assert!(!target.join("preferences.json").exists());
    cold.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_durability_configured_directory_replacement_cannot_be_adopted_from_identical_path_and_bytes() {
    let (dir, mut source, original) = library_durability_source();
    let (mut cold, target) = library_durability_candidate(&dir, &mut source, &original);
    fs::rename(&target, target.with_extension("original-directory")).unwrap();
    fs::create_dir(&target).unwrap();
    for (path, bytes) in &original {
        let to = target.join(path.strip_prefix(dir.0.join("source-state")).unwrap());
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(to, bytes).unwrap();
    }
    install(&mut cold);
    let h = cold.workspace_update_status().unwrap();
    assert!(
        cold.execute(
            "agent.activateWorkspaceUpdate",
            json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]})
        )
        .is_err()
    );
    assert!(cold.persistence.is_none());
    assert!(!target.join("preferences.json").exists());
    cold.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn library_effects_stopped_candidate() -> (Scratch, Session, Value, Value, PathBuf, Vec<(PathBuf, Vec<u8>)>) {
    let (dir, mut source, original) = library_durability_source();
    let (mut cold, target) = library_durability_candidate(&dir, &mut source, &original);
    fs::write(target.join("Recovery"), b"controlled refusal before persistence worker start").unwrap();
    install(&mut cold);
    let h = cold.workspace_update_status().unwrap();
    assert!(
        cold.execute(
            "agent.activateWorkspaceUpdate",
            json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]})
        )
        .is_err()
    );
    assert!(cold.persistence.is_none());
    assert_eq!(cold.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(cold.shortcuts.can_redo());
    let staged = json!({"projectPath":cold.path});
    (dir, cold, staged, h, target, original)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_effects_current_image_publishes_original_intents_receipts_and_displaced_bytes_without_startup_authority() {
    let (dir, mut s, staged, fence, target, original) = library_effects_stopped_candidate();
    let graph = s.project.clone();
    let image = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 501, "prepare")).unwrap();
    let manager = paused_publication_manager(&dir, &staged);
    let prepared = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 502, "prepare")).unwrap();
    paused_publication_private(std::path::Path::new(prepared["intentPath"].as_str().unwrap()), &prepared["intent"]);
    let published = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 503, "publish")).unwrap();
    assert_eq!(published["record"]["matches_published"], true);
    let raw = fs::read(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).unwrap();
    assert_eq!(history_checkpoint::digest(&raw), image["sha256"].as_str().unwrap());
    let body: Value = serde_json::from_slice(&raw).unwrap();
    let effects = &body["files"]["libraryPublication"];
    assert_eq!(effects["kind"], "native-original-library-publication-effects");
    assert_eq!(effects["activationToken"], fence["token"]);
    assert_eq!(effects["directory"]["path"], json!(target));
    assert!(effects["stopped"].is_null()); // Libraries completed; persistence itself refused.
    let entries = effects["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for row in entries {
        assert_eq!(row["phase"], "published");
        assert_eq!(row["originalReceiptDurable"], true);
        assert_eq!(row["observation"]["current"], "published");
        assert_eq!(row["observation"]["recorded_receipt"], row["originalReceipt"]);
        assert!(original.iter().any(|(_, bytes)| row["originalReceipt"]["displaced"]["sha256"] == crate::saved_file::sha256(bytes)));
        assert_eq!(row["observation"]["write_authority"], false);
        assert_eq!(row["observation"]["startup_authority"], false);
    }
    for key in ["writeAuthority", "startupAuthority", "retirementAuthority", "workersMayResume", "replayAllowed"] {
        assert_eq!(effects[key], false);
    }
    for (path, bytes) in &original {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
    assert!(std::sync::Arc::ptr_eq(&s.project, &graph));
    assert!(s.persistence.is_none());
    assert!(s.shortcuts.can_redo());
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_effects_current_image_refuses_same_byte_original_source_replacement_after_persistence_rebinding() {
    let (_dir, mut s, staged, fence, _target, original) = library_effects_stopped_candidate();
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 511, "prepare")).unwrap();
    let (path, bytes) = &original[0];
    let replacement = path.with_extension("same-bytes");
    fs::write(&replacement, bytes).unwrap();
    fs::rename(&replacement, path).unwrap();
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 512, "read")).is_err());
    assert_eq!(fs::read(path).unwrap(), *bytes);
    assert!(s.persistence.is_none());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_effects_current_image_refuses_altered_retained_intent_before_following_its_foreign_parent() {
    let (_dir, mut s, staged, fence, target, _original) = library_effects_stopped_candidate();
    let operation =
        fs::read_dir(&target).unwrap().map(|v| v.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".poema-save-")).unwrap();
    let path = operation.join("intent.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["destination"] = json!(target.join("unowned-parent/foreign-library"));
    value["parent"]["path"] = json!(target.join("unowned-parent"));
    paused_publication_private(&path, &value);
    let error = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 521, "prepare")).unwrap_err();
    assert!(error.to_string().contains("retained immutable intent differs"), "{error}");
    assert!(!target.join("unowned-parent").exists());
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 522, "read")).is_err());
    assert!(s.persistence.is_none());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn library_effects_current_image_refuses_same_byte_receipt_replacement_without_recapturing_or_republishing() {
    let (_dir, mut s, staged, fence, target, _original) = library_effects_stopped_candidate();
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 531, "prepare")).unwrap();
    let operation =
        fs::read_dir(&target).unwrap().map(|v| v.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".poema-save-")).unwrap();
    let receipt = operation.join("receipt.json");
    let bytes = fs::read(&receipt).unwrap();
    let replacement = operation.join("same-receipt.json");
    fs::write(&replacement, &bytes).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&replacement, &receipt).unwrap();
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 532, "read")).is_err());
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 533, "prepare")).is_err());
    assert_eq!(fs::read(receipt).unwrap(), bytes);
    assert!(s.persistence.is_none());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn emit_paused_native_image_fixture() {
    let Some(boot) = std::env::var_os("POEMA_CURRENT_IMAGE_TEST_BOOT") else { return };
    let boot = PathBuf::from(boot);
    let (dir, mut candidate, staged, fence) = current_image_candidate();
    candidate.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    candidate.prefs.auto_save.max_versions = 17;
    candidate.browser.dir = dir.0.join("current browser choice").to_string_lossy().into_owned();
    candidate.state.clipboard_markers.push(filmcraft_project::Marker {
        id: filmcraft_project::MarkerId(77),
        start: filmcraft_time::Tick(9_007_199_254_740_995),
        duration: filmcraft_time::Tick(0),
        name: "Current clipboard".into(),
        comment: "exact current image".into(),
        kind: filmcraft_project::MarkerKind::Comment,
        color: filmcraft_project::Label::Violet,
    });
    let resource = dir.0.join("current-paused-state/Graphics Templates");
    fs::create_dir(&resource).unwrap();
    fs::write(resource.join("current.json"), b"complete current resource").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(resource.join("current.json"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::create_dir(resource.join("Empty")).unwrap();
        fs::set_permissions(resource.join("Empty"), fs::Permissions::from_mode(0o750)).unwrap();
        fs::create_dir(resource.join("Locked")).unwrap();
        fs::write(resource.join("Locked/current.json"), b"readonly authored directory contents").unwrap();
        fs::set_permissions(resource.join("Locked/current.json"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(resource.join("Locked"), fs::Permissions::from_mode(0o550)).unwrap();
        if std::env::var_os("POEMA_CURRENT_DEEP_AUTHORED_TEST").is_some() {
            let mut deepest = resource.clone();
            for n in 0..16 {
                deepest.push(format!("level-{n}"));
                fs::create_dir(&deepest).unwrap();
                fs::set_permissions(&deepest, fs::Permissions::from_mode(0o750)).unwrap();
            }
            fs::write(deepest.join("boundary.json"), b"original depth-sixteen authored resource").unwrap();
            fs::set_permissions(deepest.join("boundary.json"), fs::Permissions::from_mode(0o640)).unwrap();
        }
        for (relative, payload) in [
            ("Keyboard Shortcuts/Studio.json", b"generated additional keyboard data".as_slice()),
            ("Templates/project.fcproj", b"generated authored project template".as_slice()),
            ("Label Presets/Team.json", b"generated authored label data".as_slice()),
        ] {
            let file = dir.0.join("current-paused-state").join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, payload).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        }
    }
    let manager = paused_publication_manager(&dir, &staged);
    if let Ok(pid) = std::env::var("POEMA_CURRENT_IMAGE_TEST_MANAGER_PID") {
        let pid = pid.parse::<u32>().unwrap();
        assert!(pid > 0 && pid != std::process::id());
        paused_publication_private(&PathBuf::from(format!("{}.lock", manager.path.display())), &json!({"pid":pid,"identity":C}));
    }
    let image = candidate.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 801, "prepare")).unwrap();
    let record = candidate.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 802, "prepare")).unwrap();
    paused_publication_private(std::path::Path::new(record["intentPath"].as_str().unwrap()), &record["intent"]);
    let record = candidate.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 803, "publish")).unwrap();
    assert_eq!(record["record"]["matches_published"], true);
    let path = PathBuf::from(record["record"]["receipt"]["intent"]["destination"].as_str().unwrap());
    let raw = fs::read(&path).unwrap();
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["version"], 2);
    assert!(body["basis"].is_object());
    let root = filmcraft_format::retained::preview_context::DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap().record();
    let mut start = startup(&candidate, &staged);
    start["activationToken"] = json!(token(900));
    start["currentImage"] = json!({"capture":image["capture"],"sha256":image["sha256"],"bytes":image["bytes"],
        "nativePid":std::process::id(),"nativeSession":image["nativeSession"],"activationToken":image["activationToken"],
        "observation":record["record"]["output"],"storageRoots":[root],"reserveBytes":1024*1024});
    paused_publication_private(
        &boot.with_extension("pending"),
        &json!({"root":dir.0,"path":path,"startup":start,"body":body,"original":staged,
        "saved":staged["projectPath"],"resource":resource.join("current.json"),"managerPath":manager.path}),
    );
    fs::rename(boot.with_extension("pending"), &boot).unwrap();
    std::mem::forget(dir);
    if std::env::var_os("POEMA_CURRENT_IMAGE_TEST_HOLD").is_some() {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !boot.with_extension("release").exists() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    candidate.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
struct CurrentReaderFixture {
    child: std::process::Child,
    _source: Scratch,
    parent: Scratch,
    boot: Value,
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl CurrentReaderFixture {
    fn release(&mut self) {
        fs::write(self.parent.0.join("boot.release"), b"finish owned fixture").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl Drop for CurrentReaderFixture {
    fn drop(&mut self) {
        let _ = fs::write(self.parent.0.join("boot.release"), b"finish owned fixture");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_reader_fixture_owned(hold: bool, managed: bool) -> CurrentReaderFixture {
    current_reader_fixture_options(hold, managed, false)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_reader_fixture_options(hold: bool, managed: bool, deep: bool) -> CurrentReaderFixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let parent = crate::temp_dir().join(format!("poema-current-reader-parent-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&parent).unwrap();
    let parent = Scratch(fs::canonicalize(parent).unwrap());
    let boot_path = parent.0.join("boot.json");
    let log = fs::File::create(parent.0.join("child.log")).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "agent_bridge::project_recovery_snapshot_tests::emit_paused_native_image_fixture", "--nocapture"])
        .env("POEMA_CURRENT_IMAGE_TEST_BOOT", &boot_path)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    if deep {
        command.env("POEMA_CURRENT_DEEP_AUTHORED_TEST", "1");
    } else {
        command.env_remove("POEMA_CURRENT_DEEP_AUTHORED_TEST");
    }
    if managed {
        command.env("POEMA_CURRENT_IMAGE_TEST_MANAGER_PID", std::process::id().to_string());
    } else {
        command.env_remove("POEMA_CURRENT_IMAGE_TEST_MANAGER_PID");
    }
    if hold {
        command.env("POEMA_CURRENT_IMAGE_TEST_HOLD", "1");
    } else {
        command.env_remove("POEMA_CURRENT_IMAGE_TEST_HOLD");
    }
    let mut child = command.spawn().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !boot_path.exists() {
        assert!(std::time::Instant::now() < until, "{}", fs::read_to_string(parent.0.join("child.log")).unwrap());
        assert!(child.try_wait().unwrap().is_none(), "{}", fs::read_to_string(parent.0.join("child.log")).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let boot: Value = serde_json::from_slice(&fs::read(&boot_path).unwrap()).unwrap();
    let root = PathBuf::from(boot["root"].as_str().unwrap());
    assert!(root.starts_with(crate::temp_dir()) && root.file_name().unwrap().to_str().unwrap().starts_with("poema-complete-root-"));
    let mut fixture = CurrentReaderFixture { child, _source: Scratch(root), parent, boot };
    if !hold {
        fixture.release();
    }
    fixture
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_reader_fixture(hold: bool) -> CurrentReaderFixture {
    current_reader_fixture_owned(hold, false)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn current_reader_refresh_descriptor(start: &mut Value, file: &std::path::Path) {
    use filmcraft_format::retained::preview_context::DirectoryFrame;
    let parent = DirectoryFrame::capture(file.parent().unwrap(), &|| Ok(())).unwrap();
    let node = parent.node(file.file_name().unwrap().to_str().unwrap(), &|| Ok(())).unwrap();
    let filmcraft_format::retained::preview_context::NodeState::File(observation) = node.state() else { panic!("owned fixture file required") };
    start["currentImage"]["observation"] = serde_json::to_value(observation).unwrap();
    start["currentImage"]["sha256"] = json!(observation.sha256);
    start["currentImage"]["bytes"] = json!(observation.bytes);
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_restores_actual_current_editor_history_and_auxiliary_through_original_fresh_constructor() {
    let f = current_reader_fixture(false);
    let original = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    let mut fresh = Session::default();
    let fence = fresh.open_project_recovery(&f.boot["startup"]).unwrap();
    assert_eq!(fence["status"], "candidate_ready");
    assert!(fresh.is_display_only());
    assert!(fresh.persistence.is_none());
    assert!(!fresh.worker_available);
    assert_eq!(fresh.state.source_playhead, filmcraft_time::Tick(9_007_199_254_740_999));
    assert_eq!(fresh.prefs.auto_save.max_versions, 17);
    assert_eq!(serde_json::to_value(super::native_editor::Snapshot::capture_complete(&fresh).unwrap()).unwrap(), f.boot["body"]["editor"]);
    assert_eq!(fresh.history.undo.len(), f.boot["body"]["history"]["undo"].as_array().unwrap().len());
    assert_eq!(fresh.history.redo.len(), f.boot["body"]["history"]["redo"].as_array().unwrap().len());
    if f.boot["body"]["history"]["owned_head"] == true {
        let head = fresh.agent_bridge.head.as_ref().unwrap().1.upgrade().unwrap();
        assert!(std::sync::Arc::ptr_eq(&head, &fresh.project));
    }
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), original);
    assert!(fresh.execute("file.save", json!({})).is_err());
    assert!(fresh.execute("file.newBin", json!({"name":"No edit from observed image"})).is_err());
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_requires_actual_prior_process_exit_before_loading_current_image() {
    let mut f = current_reader_fixture(true);
    let mut fresh = Session::default();
    let graph = fresh.project.clone();
    assert!(fresh.open_project_recovery(&f.boot["startup"]).is_err());
    assert!(std::sync::Arc::ptr_eq(&fresh.project, &graph));
    assert!(fresh.path.is_none());
    f.release();
    assert_eq!(fresh.open_project_recovery(&f.boot["startup"]).unwrap()["status"], "candidate_ready");
    assert!(fresh.persistence.is_none());
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_refuses_original_file_directory_and_absence_replacements_without_saved_file_fallback() {
    for change in ["image", "saved", "resource", "directory", "absence"] {
        let f = current_reader_fixture(false);
        if change == "directory" {
            let dir = f._source.0.join("current-paused-state");
            fs::rename(&dir, dir.with_extension("original")).unwrap();
            fs::create_dir(&dir).unwrap();
        } else if change == "absence" {
            fs::create_dir(f._source.0.join("source-state/Label Presets")).unwrap();
        } else {
            let file = PathBuf::from(
                f.boot[if change == "image" {
                    "path"
                } else if change == "saved" {
                    "saved"
                } else {
                    "resource"
                }]
                .as_str()
                .unwrap(),
            );
            let replacement = file.with_extension("same");
            fs::write(&replacement, fs::read(&file).unwrap()).unwrap();
            fs::set_permissions(&replacement, fs::metadata(&file).unwrap().permissions()).unwrap();
            fs::rename(replacement, file).unwrap();
        }
        let mut fresh = Session::default();
        let graph = fresh.project.clone();
        assert!(fresh.open_project_recovery(&f.boot["startup"]).is_err(), "{change}");
        assert!(std::sync::Arc::ptr_eq(&fresh.project, &graph));
        assert!(fresh.path.is_none());
        assert!(fresh.persistence.is_none());
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_refuses_incomplete_legacy_corrupt_and_truncated_bodies_without_panics_or_partial_installation() {
    let f = current_reader_fixture(false);
    let path = std::path::Path::new(f.boot["path"].as_str().unwrap());
    let original = fs::read(path).unwrap();
    let text = std::str::from_utf8(&original).unwrap();
    let mut variants = vec![text.replacen("\"version\":2", "\"version\":1", 1).into_bytes()];
    for size in [0, 1, original.len() / 2, original.len() - 1] {
        variants.push(original.get(..size).unwrap().to_vec());
    }
    let mut flipped = original.clone();
    let middle = flipped.len() / 2;
    *flipped.get_mut(middle).unwrap() ^= 0xff;
    variants.push(flipped);
    let mut missing: Value = serde_json::from_slice(&original).unwrap();
    missing.as_object_mut().unwrap().remove("basis");
    variants.push(serde_json::to_vec(&missing).unwrap());
    for bytes in variants {
        fs::write(path, bytes).unwrap();
        let mut start = f.boot["startup"].clone();
        current_reader_refresh_descriptor(&mut start, path);
        let mut fresh = Session::default();
        let graph = fresh.project.clone();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fresh.open_project_recovery(&start)));
        assert!(outcome.is_ok());
        assert!(outcome.unwrap().is_err());
        assert!(std::sync::Arc::ptr_eq(&fresh.project, &graph));
        assert!(fresh.path.is_none());
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_rejects_foreign_roots_metadata_overlay_and_nonfresh_sessions_before_installation() {
    let f = current_reader_fixture(false);
    for change in ["roots", "root_identity", "pid", "epoch", "capture", "reserve", "overlay", "restoration", "nonfresh"] {
        let mut start = f.boot["startup"].clone();
        let mut fresh = Session::default();
        match change {
            "roots" => start["currentImage"]["storageRoots"] = json!([]),
            "root_identity" => start["currentImage"]["storageRoots"][0]["anchor"]["identity"]["inode"] = json!("0"),
            "pid" => start["currentImage"]["nativePid"] = json!(std::process::id()),
            "epoch" => start["currentImage"]["nativeSession"] = json!("f".repeat(32)),
            "capture" => start["currentImage"]["capture"] = json!(U),
            "reserve" => start["currentImage"]["reserveBytes"] = json!(u64::MAX),
            "overlay" => start["latestJournal"] = json!(true),
            "restoration" => start["restoration"] = json!({}),
            "nonfresh" => fresh.path = Some("keep existing human session".into()),
            _ => unreachable!(),
        }
        let graph = fresh.project.clone();
        let path = fresh.path.clone();
        assert!(fresh.open_project_recovery(&start).is_err(), "{change}");
        assert!(std::sync::Arc::ptr_eq(&fresh.project, &graph));
        assert_eq!(fresh.path, path);
        assert!(fresh.persistence.is_none());
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_retains_actual_file_frames_and_never_mints_activation_or_persistence_permission() {
    let f = current_reader_fixture(false);
    let mut fresh = Session::default();
    fresh.open_project_recovery(&f.boot["startup"]).unwrap();
    install(&mut fresh);
    let fence = fresh.workspace_update_status().unwrap();
    let activate =
        json!({"token":fence["token"],"scope":fence["scope"],"workspace":fence["workspace"],"update":fence["update"],"nativeSession":fence["nativeSession"]});
    assert!(fresh.execute("agent.activateWorkspaceUpdate", activate.clone()).is_err());
    let resource = std::path::Path::new(f.boot["resource"].as_str().unwrap());
    fs::write(resource, b"later actual library work").unwrap();
    assert!(fresh.execute("agent.activateWorkspaceUpdate", activate).is_err());
    assert_eq!(fresh.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(fresh.persistence.is_none());
    assert!(!fresh.save_operation_held());
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_retains_original_checkpoint_through_original_paused_history_inspection() {
    let f = current_reader_fixture(false);
    let mut fresh = Session::default();
    let fence = fresh.open_project_recovery(&f.boot["startup"]).unwrap();
    fresh.agent_operation_control().enable_transport();
    let mut request = snapshot_inspection_request(&f.boot["original"], &fence, 991);
    request.as_object_mut().unwrap().remove("snapshotSha256");
    let proof = fresh.execute("agent.inspectPausedWorkspaceHistory", request).unwrap();
    assert_eq!(proof["checkpoint"]["checkpoint"], f.boot["original"]["checkpoint"]);
    assert_eq!(proof["checkpoint"]["sha256"], f.boot["original"]["checkpointSha256"]);
    assert_eq!(proof["editingPaused"], true);
    assert_eq!(proof["projectUnchanged"], true);
    assert_eq!(proof["historyUnchanged"], true);
    assert_eq!(fresh.state.source_playhead, filmcraft_time::Tick(9_007_199_254_740_999));
    assert_eq!(fresh.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(fresh.persistence.is_none());
    assert!(!fresh.worker_available);
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_original_checkpoint_rebind_refuses_different_graph_before_retaining_identity() {
    let f = current_reader_fixture(false);
    let mut fresh = Session::default();
    fresh.open_project_recovery(&f.boot["startup"]).unwrap();
    let envelope: Value = serde_json::from_slice(&fs::read(f.boot["original"]["path"].as_str().unwrap()).unwrap()).unwrap();
    let mut value = envelope["payload"]["history"].clone();
    value["projects"][0] = json!(format!("{} ", value["projects"][0].as_str().unwrap()));
    let original: history_checkpoint::Capsule = serde_json::from_value(value).unwrap();
    let hash = history_checkpoint::digest(&history_checkpoint::bytes(&original, 32 * 1024 * 1024).unwrap());
    let count = fresh.agent_bridge.history_checkpoints.len();
    let graph = fresh.project.clone();
    let check = media_guard::Check::new(web_time::Instant::now());
    assert!(history_checkpoint::retain_current_image_original_checkpoint(&mut fresh, &original, &hash, &check).is_err());
    assert_eq!(fresh.agent_bridge.history_checkpoints.len(), count);
    assert!(std::sync::Arc::ptr_eq(&graph, &fresh.project));
    assert!(fresh.is_display_only());
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn current_reader_original_checkpoint_inspection_refuses_later_same_byte_graph_replacement() {
    let f = current_reader_fixture(false);
    let mut fresh = Session::default();
    let fence = fresh.open_project_recovery(&f.boot["startup"]).unwrap();
    fresh.agent_operation_control().enable_transport();
    fresh.project = std::sync::Arc::new((*fresh.project).clone());
    let mut request = snapshot_inspection_request(&f.boot["original"], &fence, 992);
    request.as_object_mut().unwrap().remove("snapshotSha256");
    assert!(fresh.execute("agent.inspectPausedWorkspaceHistory", request).is_err());
    assert_eq!(fresh.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(fresh.persistence.is_none());
    assert!(!fresh.worker_available);
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn paused_retirement_fixture(publish: bool) -> (Scratch, Session, Value, Value, PausedPublicationManager, Value) {
    let (dir, mut s, staged, fence) = current_image_candidate();
    fs::write(dir.0.join("current-paused-state/retained-resource.json"), b"original resource").unwrap();
    let manager = paused_publication_manager(&dir, &staged);
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1101, "prepare")).unwrap();
    let prepared = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1102, "prepare")).unwrap();
    paused_publication_private(std::path::Path::new(prepared["intentPath"].as_str().unwrap()), &prepared["intent"]);
    if publish {
        let result = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1103, "publish")).unwrap();
        assert_eq!(result["record"]["matches_published"], true);
    }
    (dir, s, staged, fence, manager, prepared)
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_retirement_same_live_record_fences_the_real_worker_owner_without_save_or_edit_release() {
    let (_dir, mut s, staged, fence, manager, prepared) = paused_retirement_fixture(true);
    let saved = fs::read(staged["projectPath"].as_str().unwrap()).unwrap();
    let image = fs::read(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).unwrap();
    let graph = s.project.clone();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let mut pending =
        worker.start(&mut s, workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1104, "retire"), None).unwrap();
    assert!(s.is_display_only());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let done = loop {
        if let Some(done) = pending.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    s = done.session;
    let result = done.reply.result.unwrap();
    assert_eq!(result["status"], "retiring");
    assert_eq!(result["role"], "candidate");
    assert_eq!(result["currentImageRetirement"]["token"], token(1104));
    assert_eq!(result["currentImageRetirement"]["frameSha256"], prepared["frameSha256"]);
    assert_eq!(result["currentImageRetirement"]["workersMayResume"], false);
    assert_eq!(s.workspace_update_status().unwrap(), result);
    assert!(std::sync::Arc::ptr_eq(&s.project, &graph));
    assert!(s.persistence.is_none());
    assert!(s.execute("file.save", json!({})).is_err());
    assert!(s.execute("file.newBin", json!({"name":"Cannot edit a retiring producer"})).is_err());
    assert!(s.execute("agent.cancelWorkspaceUpdate", json!({"token":fence["token"],"scope":scope(),"workspace":W,"update":U})).is_err());
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1105, "read")).is_err());
    assert_eq!(fs::read(staged["projectPath"].as_str().unwrap()).unwrap(), saved);
    assert_eq!(fs::read(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).unwrap(), image);
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_retirement_requires_this_original_fully_synced_publication_before_fencing_the_candidate() {
    let (_dir, mut s, staged, fence, manager, prepared) = paused_retirement_fixture(false);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1110, "retire")).is_err());
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(s.workspace_update_status().unwrap().get("currentImageRetirement").is_none());
    assert!(!PathBuf::from(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap()).exists());
    let seen = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1111, "read")).unwrap();
    assert_eq!(seen["record"]["receipt"]["call_attempted"], false);
    assert!(s.persistence.is_none());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_retirement_refuses_same_byte_controls_intent_and_file_replacements_or_changed_editor_and_graph() {
    for change in ["manifest", "lease", "intent", "output", "saved", "resource", "editor", "graph"] {
        let (dir, mut s, staged, fence, manager, prepared) = paused_retirement_fixture(true);
        if change == "editor" {
            s.state.source_playhead = filmcraft_time::Tick(s.state.source_playhead.0.saturating_add(1));
        } else if change == "graph" {
            s.project = std::sync::Arc::new((*s.project).clone());
        } else {
            let file = match change {
                "manifest" => manager.path.clone(),
                "lease" => PathBuf::from(format!("{}.lock", manager.path.display())),
                "intent" => PathBuf::from(prepared["intentPath"].as_str().unwrap()),
                "output" => PathBuf::from(prepared["record"]["receipt"]["intent"]["destination"].as_str().unwrap()),
                "saved" => PathBuf::from(staged["projectPath"].as_str().unwrap()),
                _ => dir.0.join("current-paused-state/retained-resource.json"),
            };
            let replacement = file.with_extension("retirement-replacement");
            fs::write(&replacement, fs::read(&file).unwrap()).unwrap();
            fs::rename(replacement, file).unwrap();
        }
        assert!(
            s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1120, "retire")).is_err(),
            "{change}"
        );
        assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready", "{change}");
        assert!(s.workspace_update_status().unwrap().get("currentImageRetirement").is_none());
        let seen = s.execute(workspace_update::PAUSED_RECOVERY_RECORD, paused_publication_request(&staged, &fence, &manager, 1121, "read")).unwrap();
        assert_eq!(seen["record"]["receipt"]["directory_synced"], true);
        assert!(s.persistence.is_none());
        s.shutdown();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_retirement_rejects_cancelled_and_unknown_purposes_before_entering_the_native_close_fence() {
    let (_dir, mut s, staged, fence, manager, _prepared) = paused_retirement_fixture(true);
    let request = paused_publication_request(&staged, &fence, &manager, 1130, "retire");
    workspace_update::prepare_paused_record_operation(&mut s, &request).unwrap();
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(1130)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, request).is_err());
    let mut unknown = paused_publication_request(&staged, &fence, &manager, 1131, "retire");
    unknown["activate"] = json!(true);
    assert!(s.execute(workspace_update::PAUSED_RECOVERY_RECORD, unknown).is_err());
    assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert!(s.workspace_update_status().unwrap().get("currentImageRetirement").is_none());
    assert!(s.persistence.is_none());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn stopped_current_candidate(bind: bool) -> (CurrentReaderFixture, Session, PathBuf, Value) {
    let f = current_reader_fixture_owned(false, true);
    let root = PathBuf::from(f.boot["root"].as_str().unwrap());
    let mut s = Session::default();
    s.open_project_recovery(&f.boot["startup"]).unwrap();
    let data = root.join("actual-stopped-activation-state");
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(data.clone())).unwrap();
    let manifest = PathBuf::from(f.boot["managerPath"].as_str().unwrap());
    if bind {
        s.retain_managed_save_startup_root(&manifest).unwrap();
    }
    install(&mut s);
    let ready = s.workspace_update_status().unwrap();
    let c = &f.boot["body"]["context"];
    let image = &f.boot["startup"]["currentImage"];
    let phase = json!({"kind":"poema-current-image-activation","version":1,"stage":"requested","capture":image["capture"],
        "activationToken":ready["token"],"nativePid":std::process::id(),"nativeSession":ready["nativeSession"],"dataDirectory":data});
    let row = json!({"kind":"poema-native-workspace-update","version":4,"phase":"current_image_activating","workspace":W,"update":U,
        "scope":scope(),"original":{"projectPath":s.path},"preparedSnapshot":{"sha256":f.boot["startup"]["sha256"]},
        "candidate":{"pid":c["native_pid"],"nativeSession":c["native_session"],"dataDir":c["data_directory"]},
        "currentImageRetirement":{"kind":"poema-current-image-retirement","version":1,"stage":"exited","capture":image["capture"],"sha256":image["sha256"],"bytes":image["bytes"],
            "nativePid":c["native_pid"],"nativeSession":c["native_session"],"activationToken":c["activation_token"],
            "exit":{"pid":c["native_pid"],"code":0,"signal":null}},
        "currentImageRestart":{"kind":"poema-current-image-restart","version":1,"stage":"ready","capture":image["capture"],
            "activationToken":ready["token"],"revision":s.revision,"dirty":s.is_dirty(),
            "candidate":{"pid":std::process::id(),"nativeSession":ready["nativeSession"],"dataDir":data}},
        "currentImageActivation":phase});
    let directory = manifest.parent().unwrap().join(format!("workspace-state-{W}"));
    paused_publication_private(&directory.join("activity.json"), &json!({"status":"active","action":"workspace-update","operationId":U}));
    paused_publication_private(&directory.join("workspace-update.json"), &row);
    (f, s, data, row)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn stopped_current_activate(s: &mut Session) -> Result<Value> {
    let h = s.workspace_update_status().unwrap();
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]}),
    )
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn stopped_current_activation_uses_actual_startup_owner_and_original_writers_before_native_edit_release() {
    let (f, mut s, data, _row) = stopped_current_candidate(true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    let graph = s.project.clone();
    let origin = s.agent_bridge.manager_save_startup.clone().unwrap();
    let input = workspace_update::current_activation_input_test(&s).unwrap();
    // The retained input is observational; only actual explicit native startup
    // controls plus the fresh-directory owner can cross the publication fence.
    assert!(s.is_display_only());
    assert!(s.persistence.is_none());
    assert_eq!(stopped_current_activate(&mut s).unwrap()["status"], "activated");
    assert!(!s.is_display_only());
    assert!(s.persistence.is_some());
    assert!(s.managed_save_dispatch_available());
    assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
    assert!(std::sync::Arc::ptr_eq(&origin, s.agent_bridge.manager_save_origin.as_ref().unwrap()));
    assert_eq!(s.state.source_playhead, filmcraft_time::Tick(9_007_199_254_740_999));
    assert_eq!(s.prefs.auto_save.max_versions, 17);
    assert_eq!(crate::autosave::Preferences::load(&data.join("preferences.json")).auto_save.max_versions, 17);
    let operation = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(format!("{}:native-library:preferences.json", token(900)).as_bytes()));
    let seen = filmcraft_format::retained::inspect(&data.join(format!(".poema-save-{operation}.incomplete")), &|| Ok(())).unwrap();
    assert_eq!(seen.current, filmcraft_format::retained::CurrentState::Published, "startup must not rewrite the already published native preferences");
    input.current(&media_guard::Check::new(web_time::Instant::now())).unwrap();
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    undo(&mut s, 1501, 11);
    redo(&mut s, 1502, 11, 1501);
    s.execute("file.newBin", json!({"name":"Human after genuine current-image activation"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.is_dirty());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn stopped_current_activation_requires_original_explicit_intent_and_actual_native_startup_binding() {
    for mode in ["unbound", "ready_only", "foreign_manager", "foreign_candidate", "extra_owner"] {
        let (f, mut s, data, mut row) = stopped_current_candidate(mode != "unbound");
        let manager = PathBuf::from(f.boot["managerPath"].as_str().unwrap());
        let directory = manager.parent().unwrap().join(format!("workspace-state-{W}"));
        if mode == "ready_only" {
            row["phase"] = json!("current_image_ready");
        }
        if mode == "foreign_candidate" {
            row["currentImageRestart"]["candidate"]["pid"] = json!(std::process::id() + 1);
        }
        paused_publication_private(&directory.join("workspace-update.json"), &row);
        if mode == "foreign_manager" {
            let intent = directory.join(format!("paused-recovery-{}.json", f.boot["startup"]["currentImage"]["capture"].as_str().unwrap()));
            let mut value: Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
            value["managerPid"] = json!(std::process::id() + 1);
            paused_publication_private(&intent, &value);
        }
        if mode == "extra_owner" {
            let h = s.workspace_update_status().unwrap();
            assert!(s.execute("agent.activateWorkspaceUpdate",json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"],"owner":row})).is_err());
        } else {
            assert!(stopped_current_activate(&mut s).is_err(), "{mode}");
        }
        assert!(s.is_display_only());
        assert!(s.persistence.is_none());
        assert_eq!(fs::read_dir(data).unwrap().count(), 0, "{mode}");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn stopped_current_activation_refuses_bound_generation_and_original_or_fresh_namespace_drift_before_writes() {
    for mode in ["journal", "lease", "intent", "saved", "resource", "image", "foreign_target", "graph"] {
        let (f, mut s, data, _row) = stopped_current_candidate(true);
        workspace_update::prepare_current_activation_test(&mut s, false).unwrap();
        let manager = PathBuf::from(f.boot["managerPath"].as_str().unwrap());
        let directory = manager.parent().unwrap().join(format!("workspace-state-{W}"));
        let file = match mode {
            "journal" => Some(directory.join("workspace-update.json")),
            "lease" => Some(PathBuf::from(format!("{}.lock", manager.display()))),
            "intent" => Some(directory.join(format!("paused-recovery-{}.json", f.boot["startup"]["currentImage"]["capture"].as_str().unwrap()))),
            "saved" => Some(PathBuf::from(f.boot["saved"].as_str().unwrap())),
            "resource" => Some(PathBuf::from(f.boot["resource"].as_str().unwrap())),
            "image" => Some(PathBuf::from(f.boot["path"].as_str().unwrap())),
            _ => None,
        };
        if let Some(file) = file {
            let temporary = file.with_extension("same-activation");
            fs::write(&temporary, fs::read(&file).unwrap()).unwrap();
            fs::set_permissions(&temporary, fs::metadata(&file).unwrap().permissions()).unwrap();
            fs::rename(temporary, file).unwrap();
        }
        if mode == "foreign_target" {
            fs::write(data.join("preferences.json"), b"later human file").unwrap();
        }
        if mode == "graph" {
            s.project = std::sync::Arc::new((*s.project).clone());
        }
        assert!(stopped_current_activate(&mut s).is_err(), "{mode}");
        assert!(s.is_display_only());
        assert!(s.persistence.is_none());
        assert_eq!(fs::read_dir(&data).unwrap().count(), usize::from(mode == "foreign_target"), "{mode}");
        if mode == "foreign_target" {
            assert_eq!(fs::read(data.join("preferences.json")).unwrap(), b"later human file");
        }
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn stopped_current_activation_keeps_original_live_partial_publication_and_never_replays_after_stop() {
    let (f, mut s, data, _row) = stopped_current_candidate(true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    workspace_update::prepare_current_activation_test(&mut s, true).unwrap();
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(data.join("Keyboard Shortcuts").is_dir());
    let names = fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>();
    assert!(!data.join("preferences.json").exists());
    assert!(s.is_display_only());
    assert!(s.persistence.is_none());
    assert!(stopped_current_activate(&mut s).is_err());
    assert_eq!(fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), names);
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    assert!(s.execute("file.newBin", json!({"name":"No release from a partial activation"})).is_err());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn stopped_current_activation_keeps_actual_new_worker_gated_after_post_creation_stop() {
    let (f, mut s, data, _row) = stopped_current_candidate(true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    workspace_update::prepare_current_activation_worker_stop_test(&mut s).unwrap();
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(s.is_display_only());
    let persistence = s.persistence.as_ref().unwrap();
    assert!(persistence.startup_pending());
    assert!(persistence.auto_save_now().is_err());
    let before = fs::read(data.join("preferences.json")).unwrap();
    persistence.flush();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(fs::read(data.join("preferences.json")).unwrap(), before);
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.execute("file.newBin", json!({"name":"No editing from an unreleased worker"})).is_err());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_dependencies_reuse_original_enumerator_for_retained_current_undo_redo_and_auxiliary_frames() {
    let (dir, mut source) = setup();
    native_only_file(&mut source, "old.MXF", Some("old-proxy.mov"), false);
    source.history.undo.push(("Earlier human source".into(), source.project.clone()));
    native_only_file(&mut source, "current.MXF", Some("current-proxy.mov"), false);
    let current = source.project.clone();
    native_only_file(&mut source, "redo.MXF", None, false);
    source.history.redo.push(("Later human source".into(), source.project.clone()));
    source.project = current;
    source.media.add_stored_paths(["old.MXF", "old-proxy.mov", "current.MXF", "current-proxy.mov", "redo.MXF"].map(String::from), source.path.as_deref());
    let staged = stage(&mut source);
    let expected = source.execute(DEPENDENCIES, dependency_request(&staged)).unwrap();
    let request = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut s = Session::default();
    let fence = s.open_project_recovery(&request).unwrap();
    install(&mut s);
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("current-paused-state"))).unwrap();
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
    let file = dir.0.join("current-paused-state/effect-presets.json");
    fs::write(&file, b"actual paused resource").unwrap();
    let graph = s.project.clone();
    let before = (s.revision, s.history.undo.len(), s.history.redo.len(), s.agent_bridge.tickets.len(), s.agent_bridge.history_bases.len());
    let saved = fs::read(staged["projectPath"].as_str().unwrap()).unwrap();
    let image = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1300, "prepare")).unwrap();
    assert_eq!(fence["capabilities"]["pausedCurrentRecoveryDependencies"], true);
    let value = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1301, "dependencies")).unwrap();
    assert_eq!(value["kind"], "native_paused_current_recovery_dependencies");
    assert_eq!(value["status"], "metadata");
    for name in ["sha256", "bytes", "capture", "nativeSession", "nativePid", "revision", "sequence"] {
        assert_eq!(value[name], image[name]);
    }
    for name in ["projects", "files", "references"] {
        assert_eq!(value["mediaInventory"][name], expected[name]);
    }
    assert_eq!(value["mediaInventory"]["files"].as_array().unwrap().len(), 5);
    assert_eq!(value["fileFrames"]["kind"], "native_paused_current_file_frames");
    let frame_json = value["fileFrames"]["json"].as_str().unwrap();
    assert_eq!(frame_json.len(), value["fileFrames"]["bytes"].as_u64().unwrap() as usize);
    assert_eq!(format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(frame_json.as_bytes())), value["fileFrames"]["sha256"]);
    let frames: Value = serde_json::from_str(frame_json).unwrap();
    assert_eq!(frames["savedPath"], staged["projectPath"]);
    assert!(frames["nodes"].as_object().unwrap().contains_key(file.to_str().unwrap()));
    assert_eq!(frames["source"]["kind"], "native-original-resource-frames");
    for name in ["writeAuthority", "retirementAuthority", "startupAuthority", "mediaEvidence"] {
        assert_eq!(value[name], false);
    }
    assert_eq!(value["sourceValidation"], "not-performed");
    let repeated = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1302, "dependencies")).unwrap();
    assert_eq!(value["mediaInventory"], repeated["mediaInventory"]);
    assert_eq!(value["fileFrames"], repeated["fileFrames"]);
    assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
    assert_eq!(before, (s.revision, s.history.undo.len(), s.history.redo.len(), s.agent_bridge.tickets.len(), s.agent_bridge.history_bases.len()));
    assert_eq!(s.state.source_playhead.0, 9_007_199_254_740_999);
    assert!(s.persistence.is_none());
    assert!(!s.save_operation_held());
    assert_eq!(fs::read(staged["projectPath"].as_str().unwrap()).unwrap(), saved);
    assert_eq!(fs::read(file).unwrap(), b"actual paused resource");
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_dependencies_never_capture_missing_images_and_refuse_crossed_or_stopped_purposes() {
    let (_dir, mut s, staged, fence) = current_image_candidate();
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1310, "dependencies")).is_err());
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1335, "read")).is_err());
    let image = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1311, "prepare")).unwrap();
    for field in ["capture", "nativeSession", "activationToken", "projectPath"] {
        let mut r = current_image_request(&fence, &staged["projectPath"], 1312, "dependencies");
        r[field] = json!(if field == "capture" { "55555555-5555-4555-8555-555555555555" } else { "foreign" });
        assert!(s.execute("agent.preparePausedProjectRecovery", r).is_err());
    }
    let stopped = current_image_request(&fence, &staged["projectPath"], 1313, "dependencies");
    workspace_update::prepare_paused_image_operation(&mut s, &stopped).unwrap();
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(1313)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute("agent.preparePausedProjectRecovery", stopped).is_err());
    assert_eq!(
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1314, "read")).unwrap()["sha256"],
        image["sha256"]
    );
    assert!(s.persistence.is_none());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_dependencies_refuse_same_byte_resource_replacement_namespace_editor_and_native_graph_drift() {
    for mode in ["resource", "namespace", "editor", "graph", "saved"] {
        let (dir, mut s, staged, fence) = current_image_candidate();
        let file = dir.0.join("current-paused-state/unknown-partial-effect");
        fs::write(&file, b"retained").unwrap();
        s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1320, "prepare")).unwrap();
        match mode {
            "resource" => {
                let replacement = file.with_extension("new");
                fs::write(&replacement, b"retained").unwrap();
                fs::rename(replacement, &file).unwrap();
            }
            "namespace" => {
                fs::write(dir.0.join("current-paused-state/new-member"), b"new").unwrap();
            }
            "editor" => {
                s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_999);
            }
            "graph" => {
                s.project = std::sync::Arc::new((*s.project).clone());
            }
            _ => {
                let path = PathBuf::from(staged["projectPath"].as_str().unwrap());
                let replacement = path.with_extension("new");
                fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
                fs::rename(replacement, path).unwrap();
            }
        }
        assert!(
            s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1321, "dependencies")).is_err(),
            "{mode}"
        );
        assert!(s.persistence.is_none());
        assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_dependencies_share_native_numbered_sequence_gap_policy_without_media_decode() {
    let (dir, mut source) = setup();
    let first = dir.0.join("retained_0001.png");
    let third = dir.0.join("retained_0003.png");
    fs::write(&first, b"not decoded").unwrap();
    fs::write(&third, b"not decoded either").unwrap();
    native_only_file(&mut source, first.to_str().unwrap(), None, true);
    let staged = stage(&mut source);
    let request = startup(&source, &staged);
    retire(&mut source, &staged);
    let mut s = Session::default();
    let fence = s.open_project_recovery(&request).unwrap();
    install(&mut s);
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("current-paused-state"))).unwrap();
    s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1330, "prepare")).unwrap();
    let value = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1331, "dependencies")).unwrap();
    let files = value["mediaInventory"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(value["mediaInventory"]["references"], 2);
    assert!(files.iter().all(|r| r["kind"] == "sequence-frame"));
    assert!(files.iter().any(|r| r["path"] == first.to_str().unwrap()));
    assert!(files.iter().any(|r| r["path"] == third.to_str().unwrap()));
    assert_eq!(value["mediaEvidence"], false);
    assert!(s.persistence.is_none());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn paused_current_archive_inventory_preserves_captured_files_empty_directories_absences_and_exact_modes() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mut s, staged, fence) = current_image_candidate();
    let empty = dir.0.join("current-paused-state/unknown-partial/empty");
    fs::create_dir_all(&empty).unwrap();
    fs::set_permissions(&empty, fs::Permissions::from_mode(0o750)).unwrap();
    let file = empty.parent().unwrap().join("retained.part");
    fs::write(&file, b"actual incomplete native staging bytes").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    let graph = s.project.clone();
    let image = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1400, "prepare")).unwrap();
    let result = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1401, "dependencies")).unwrap();
    assert_eq!(fence["capabilities"]["pausedCurrentRecoveryResources"], true);
    assert_eq!(result["version"], 2);
    assert_eq!(result["sha256"], image["sha256"]);
    let inventory = &result["resourceInventory"];
    assert_eq!(inventory["kind"], "native_paused_current_resource_inventory");
    let file_row = inventory["files"].as_array().unwrap().iter().find(|r| r["path"] == file.to_str().unwrap()).unwrap();
    assert_eq!(file_row["bytes"], fs::metadata(&file).unwrap().len().to_string());
    assert_eq!(file_row["mode"], 0o640);
    assert!(inventory["files"].as_array().unwrap().iter().any(|r| r["path"] == staged["projectPath"]));
    assert!(inventory["directories"].as_array().unwrap().iter().any(|r| r["path"] == empty.to_str().unwrap() && r["mode"] == 0o750));
    assert!(!inventory["absent"].as_array().unwrap().is_empty());
    let again = s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1402, "dependencies")).unwrap();
    assert_eq!(again["resourceInventory"], *inventory);
    assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
    assert!(s.persistence.is_none());
    assert!(!s.save_operation_held());
    fs::write(&file, b"later partial bytes").unwrap();
    assert!(s.execute("agent.preparePausedProjectRecovery", current_image_request(&fence, &staged["projectPath"], 1403, "dependencies")).is_err());
    s.shutdown();
}

#[cfg(unix)]
#[test]
fn current_basis_restoration_preserves_original_baseline_and_admits_saved_preferences_and_journal_stamps_only() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let staged = stage(&mut source);
    let raw = fs::read(staged["path"].as_str().unwrap()).unwrap();
    let body: Value = serde_json::from_slice(&raw).unwrap();
    let mut original = body["payload"]["basis"].clone();
    retire(&mut source, &staged);
    let data = PathBuf::from(original["data_directory"].as_str().unwrap());
    let session = original["session_directory"].as_str().unwrap();
    let companion = crate::autosave::recovery_root(&data).join(session).join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    if !companion.exists() {
        paused_publication_private(&companion, &json!({"generated":"current companion stamp fixture"}));
    }
    let before = restored_file(&companion, "journal-companion");
    let physical = &before["physical"];
    let ns = physical["ctimeNs"].as_str().unwrap().parse::<u128>().unwrap();
    original["journal_companion"] = json!({"path":companion,"sha256":before["sha256"],"stamp":{"length":before["bytes"],"modified":physical["mtimeNs"],"device":physical["dev"],"inode":physical["ino"],"changed":format!("{}:{}",ns/1_000_000_000,ns%1_000_000_000)}});
    let saved = PathBuf::from(source.path.as_ref().unwrap());
    let pref = source.prefs_path.as_ref().unwrap();
    for p in [&saved, pref, &companion] {
        replace_identical(p);
    }
    assert!(project_recovery::validate(&original).is_err());
    let description = json!({"kind":"native_current_project_recovery_restoration","version":1,"snapshotSha256":staged["sha256"],"activationToken":token(100),
        "files":[restored_file(&saved,"saved-project"),restored_file(pref,"preferences"),restored_file(&companion,"journal-companion")]});
    let restored = project_recovery::restore_current_basis(&original, &description, staged["sha256"].as_str().unwrap(), &token(100)).unwrap();
    project_recovery::validate(&restored).unwrap();
    for key in ["saved_file", "native_session", "revision", "saved_revision", "sequence", "project_sha256", "data_directory", "session_directory", "journals"] {
        assert_eq!(restored[key], original[key], "{key}");
    }
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), raw);
    assert!(project_recovery::restore_basis(&original, &description, staged["sha256"].as_str().unwrap(), &token(100)).is_err());
}
#[cfg(unix)]
#[test]
fn current_basis_restoration_rejects_foreign_roles_hashes_duplicate_roles_and_later_same_byte_files() {
    let (_dir, mut source) = setup();
    source.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":7})).unwrap();
    let staged = stage(&mut source);
    let body: Value = serde_json::from_slice(&fs::read(staged["path"].as_str().unwrap()).unwrap()).unwrap();
    let original = &body["payload"]["basis"];
    retire(&mut source, &staged);
    let saved = PathBuf::from(source.path.as_ref().unwrap());
    replace_identical(&saved);
    let good = json!({"kind":"native_current_project_recovery_restoration","version":1,"snapshotSha256":staged["sha256"],"activationToken":token(100),"files":[restored_file(&saved,"saved-project")]});
    for fault in ["hash", "role", "activation", "duplicate", "snapshot"] {
        let mut bad = good.clone();
        match fault {
            "hash" => bad["files"][0]["sha256"] = json!("0".repeat(64)),
            "role" => bad["files"][0]["role"] = json!("journal-companion"),
            "activation" => bad["activationToken"] = json!(token(101)),
            "snapshot" => bad["snapshotSha256"] = json!("0".repeat(64)),
            "duplicate" => bad["files"].as_array_mut().unwrap().push(good["files"][0].clone()),
            _ => panic!("known fault"),
        }
        assert!(project_recovery::restore_current_basis(original, &bad, staged["sha256"].as_str().unwrap(), &token(100)).is_err(), "{fault}");
    }
    replace_identical(&saved);
    assert!(project_recovery::restore_current_basis(original, &good, staged["sha256"].as_str().unwrap(), &token(100)).is_err());
}

// Native boundary coverage only. The full Poema fixture separately proves the
// original service protocol; this owned helper supplies its actual separate PID.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn emit_cold_current_source_fixture() {
    let Some(root) = std::env::var_os("POEMA_COLD_CURRENT_SOURCE_TEST_ROOT") else { return };
    let root = PathBuf::from(root);
    fs::write(root.join("ready"), b"owned separate native-test source").unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !root.join("release").exists() {
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
struct ColdCurrentSourceFixture {
    child: std::process::Child,
    root: Scratch,
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl ColdCurrentSourceFixture {
    fn start() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = crate::temp_dir().join(format!("poema-native-cold-source-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let root = Scratch(fs::canonicalize(root).unwrap());
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "agent_bridge::project_recovery_snapshot_tests::emit_cold_current_source_fixture", "--nocapture"])
            .env("POEMA_COLD_CURRENT_SOURCE_TEST_ROOT", &root.0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !root.0.join("ready").exists() {
            assert!(std::time::Instant::now() < until);
            assert!(child.try_wait().unwrap().is_none());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Self { child, root }
    }
    fn exit(&mut self) {
        fs::write(self.root.0.join("release"), b"owned fixture release").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.child.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl Drop for ColdCurrentSourceFixture {
    fn drop(&mut self) {
        let _ = fs::write(self.root.0.join("release"), b"owned fixture cleanup");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn cold_current_control_paths(f: &CurrentReaderFixture) -> (PathBuf, PathBuf) {
    let manifest = PathBuf::from(f.boot["managerPath"].as_str().unwrap());
    let directory = manifest.parent().unwrap().join(format!("workspace-state-{W}"));
    (manifest, directory)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn cold_current_write_controls(f: &CurrentReaderFixture, row: &mut Value, completion: &Value) {
    let file = PathBuf::from(row["coldCurrentActivation"]["sourceCompletion"]["file"].as_str().unwrap());
    paused_publication_private(&file, completion);
    row["coldCurrentActivation"]["sourceCompletion"]["sha256"] = json!(format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(fs::read(&file).unwrap())));
    let (_, directory) = cold_current_control_paths(f);
    paused_publication_private(&directory.join("workspace-update.json"), row);
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn cold_current_candidate(restored: bool, bind: bool) -> (CurrentReaderFixture, ColdCurrentSourceFixture, Session, PathBuf, Value, Value) {
    cold_current_candidate_options(restored, bind, false)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn cold_current_candidate_options(restored: bool, bind: bool, deep: bool) -> (CurrentReaderFixture, ColdCurrentSourceFixture, Session, PathBuf, Value, Value) {
    use filmcraft_format::retained::preview_context::{DirectoryFrame, NodeState};
    let f = current_reader_fixture_options(false, false, deep);
    let (manifest, directory) = cold_current_control_paths(&f);
    // The archived publisher's actual manager was its now-exited child. The
    // new native constructor explicitly binds this separate current manager.
    let old_intent = directory.join(format!("paused-recovery-{}.json", f.boot["startup"]["currentImage"]["capture"].as_str().unwrap()));
    let intent: Value = serde_json::from_slice(&fs::read(old_intent).unwrap()).unwrap();
    assert_ne!(intent["managerPid"], std::process::id());
    paused_publication_private(&PathBuf::from(format!("{}.lock", manifest.display())), &json!({"pid":std::process::id(),"identity":C}));
    let mut start = f.boot["startup"].clone();
    if restored {
        let input = PathBuf::from(f.boot["root"].as_str().unwrap()).join("actual-cold-current-admission.json");
        paused_publication_private(
            &input,
            &json!({"kind":"native_current_image_restoration","version":1,"capture":start["currentImage"]["capture"],
            "frameSha256":start["currentImage"]["sha256"],"snapshotSha256":start["sha256"],"activationToken":start["activationToken"],
            "image":start["currentImage"]["observation"],"files":[]}),
        );
        let frame =
            DirectoryFrame::capture(input.parent().unwrap(), &|| Ok(())).unwrap().node(input.file_name().unwrap().to_str().unwrap(), &|| Ok(())).unwrap();
        let NodeState::File(observation) = frame.state() else { panic!("actual private admission required") };
        start["currentImage"]["restoration"] = json!({"path":input,"observation":observation});
    }
    let mut s = Session::default();
    s.open_project_recovery(&start).unwrap();
    let attempt = "36363636-3636-4636-8636-363636363636";
    let root = directory.join(format!("update-{U}"));
    fs::create_dir_all(&root).unwrap();
    let data = root.join(format!("recovery-{attempt}-state"));
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(data.clone())).unwrap();
    if bind {
        s.retain_managed_save_startup_root(&manifest).unwrap();
    }
    install(&mut s);
    let source = ColdCurrentSourceFixture::start();
    let ready = s.workspace_update_status().unwrap();
    let c = &f.boot["body"]["context"];
    let image = &start["currentImage"];
    let candidate = json!({"pid":std::process::id(),"nativePort":46111,"nativeSession":ready["nativeSession"],"dataDir":data});
    let completion_file = directory.join("application-cold-source-45454545-4545-4545-8545-454545454545-0001-completion.json");
    let completion = json!({"kind":"poema-application-cold-source-completion","version":1,"ordinal":1,
        "application":{"manifestPath":manifest,"workspaceId":W,"projectId":scope()["projectId"]},
        "native":{"update":U,"snapshotSha256":start["sha256"],"activationSha256":format!("{:x}",<sha2::Sha256 as sha2::Digest>::digest(start["activationToken"].as_str().unwrap().as_bytes())),"candidate":candidate},
        "handoff":{"record":{"phase":"complete","replacement":{"pid":source.child.id()}}}});
    let mut row = json!({"kind":"poema-native-workspace-update","version":7,"phase":"activating","workspace":W,"update":U,"root":root,
        "recoveryAttempt":attempt,"scope":scope(),"original":{"projectPath":s.path},"preparedSnapshot":{"sha256":start["sha256"]},
        "candidateBuild":"next","activationToken":ready["token"],"candidate":candidate,
        "activation":{"pid":std::process::id(),"nativeSession":ready["nativeSession"],"finish":"complete"},
        "coldCurrentActivation":{"kind":"poema-cold-current-activation","version":1,"capture":image["capture"],"frameSha256":image["sha256"],"frameBytes":image["bytes"],
            "sourceSnapshot":start["sha256"],"producer":{"pid":c["native_pid"],"nativeSession":c["native_session"],"activationToken":c["activation_token"],"dataDir":c["data_directory"]},
            "candidate":{"pid":std::process::id(),"nativeSession":ready["nativeSession"],"activationToken":ready["token"],"dataDir":data,"revision":s.revision},
            "sourceCompletion":{"file":completion_file,"sha256":""}}});
    paused_publication_private(&directory.join("activity.json"), &json!({"status":"active","action":"workspace-update","operationId":U}));
    cold_current_write_controls(&f, &mut row, &completion);
    (f, source, s, data, row, completion)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_uses_actual_new_manager_restored_input_and_original_native_writers_with_human_save() {
    use std::os::unix::fs::PermissionsExt;
    let (f, _source, mut s, data, _row, _completion) = cold_current_candidate_options(true, true, true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    let graph = s.project.clone();
    assert!(s.is_display_only());
    assert_eq!(stopped_current_activate(&mut s).unwrap()["status"], "activated");
    assert!(!s.is_display_only());
    assert!(s.persistence.is_some());
    assert!(s.managed_save_dispatch_available());
    assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
    assert_eq!(s.state.source_playhead, filmcraft_time::Tick(9_007_199_254_740_999));
    assert_eq!(crate::autosave::Preferences::load(&data.join("preferences.json")).auto_save.max_versions, 17);
    assert_eq!(crate::graphic_templates::user_templates_dir(&s), Some(data.join("Graphics Templates")));
    for (relative, payload) in [
        ("Graphics Templates/current.json", b"complete current resource".as_slice()),
        ("Graphics Templates/Locked/current.json", b"readonly authored directory contents".as_slice()),
        ("Keyboard Shortcuts/Studio.json", b"generated additional keyboard data".as_slice()),
        ("Templates/project.fcproj", b"generated authored project template".as_slice()),
        ("Label Presets/Team.json", b"generated authored label data".as_slice()),
    ] {
        assert_eq!(fs::read(data.join(relative)).unwrap(), payload, "{relative}");
        assert_eq!(fs::metadata(data.join(relative)).unwrap().permissions().mode() & 0o7777, 0o640, "{relative}");
    }
    assert_eq!(fs::read_dir(data.join("Graphics Templates/Empty")).unwrap().count(), 0);
    assert_eq!(fs::metadata(data.join("Graphics Templates/Empty")).unwrap().permissions().mode() & 0o7777, 0o750);
    assert_eq!(fs::metadata(data.join("Graphics Templates/Locked")).unwrap().permissions().mode() & 0o7777, 0o550);
    let mut deepest = data.join("Graphics Templates");
    for n in 0..16 {
        deepest.push(format!("level-{n}"));
        assert_eq!(fs::metadata(&deepest).unwrap().permissions().mode() & 0o7777, 0o750);
    }
    assert_eq!(fs::read(deepest.join("boundary.json")).unwrap(), b"original depth-sixteen authored resource");
    assert_eq!(fs::metadata(deepest.join("boundary.json")).unwrap().permissions().mode() & 0o7777, 0o640);
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    undo(&mut s, 1601, 11);
    redo(&mut s, 1602, 11, 1601);
    s.execute("file.newBin", json!({"name":"Human after genuine cold current activation"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.is_dirty());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_refuses_unrestored_unbound_crossed_and_descriptive_only_intents_before_writes() {
    for fault in [
        "unrestored",
        "unbound",
        "ready",
        "capture",
        "producer",
        "candidate",
        "snapshot",
        "revision",
        "old_retirement",
        "extra_owner",
        "completion_digest",
        "completion_path",
    ] {
        let (f, _source, mut s, data, mut row, _completion) = cold_current_candidate(fault != "unrestored", fault != "unbound");
        match fault {
            "unrestored" | "unbound" => {}
            "ready" => row["phase"] = json!("candidate_ready"),
            "capture" => row["coldCurrentActivation"]["capture"] = json!(C),
            "producer" => row["coldCurrentActivation"]["producer"]["pid"] = json!(std::process::id()),
            "candidate" => row["coldCurrentActivation"]["candidate"]["nativeSession"] = json!(token(7)),
            "snapshot" => row["coldCurrentActivation"]["sourceSnapshot"] = json!("f".repeat(64)),
            "revision" => row["coldCurrentActivation"]["candidate"]["revision"] = json!(s.revision + 1),
            "old_retirement" => row["currentImageRetirement"] = json!({}),
            "extra_owner" => row["coldCurrentActivation"]["owner"] = json!({}),
            "completion_digest" => row["coldCurrentActivation"]["sourceCompletion"]["sha256"] = json!("f".repeat(64)),
            "completion_path" => row["coldCurrentActivation"]["sourceCompletion"]["file"] = json!(data.join("foreign-completion.json")),
            _ => panic!("known native control fault"),
        }
        let (_, directory) = cold_current_control_paths(&f);
        paused_publication_private(&directory.join("workspace-update.json"), &row);
        assert!(stopped_current_activate(&mut s).is_err(), "{fault}");
        assert!(s.is_display_only(), "{fault}");
        assert!(s.persistence.is_none(), "{fault}");
        assert_eq!(fs::read_dir(data).unwrap().count(), 0, "{fault}");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_pins_actual_completion_journal_manager_lease_and_live_source_before_effects() {
    for fault in ["completion", "journal", "lease", "source_exit"] {
        let (f, mut source, mut s, data, row, _completion) = cold_current_candidate(true, true);
        workspace_update::prepare_current_activation_test(&mut s, false).unwrap();
        let (manifest, directory) = cold_current_control_paths(&f);
        match fault {
            "completion" => replace_identical(Path::new(row["coldCurrentActivation"]["sourceCompletion"]["file"].as_str().unwrap())),
            "journal" => replace_identical(&directory.join("workspace-update.json")),
            "lease" => replace_identical(&PathBuf::from(format!("{}.lock", manifest.display()))),
            "source_exit" => source.exit(),
            _ => panic!("known fresh control fault"),
        }
        assert!(stopped_current_activate(&mut s).is_err(), "{fault}");
        assert!(s.is_display_only(), "{fault}");
        assert!(s.persistence.is_none(), "{fault}");
        assert_eq!(fs::read_dir(data).unwrap().count(), 0, "{fault}");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_requires_matching_original_source_completion_scope_and_candidate() {
    for fault in ["workspace", "project", "manifest", "update", "snapshot", "activation", "candidate", "phase", "source_pid", "ordinal"] {
        let (f, _source, mut s, data, mut row, mut completion) = cold_current_candidate(true, true);
        match fault {
            "workspace" => completion["application"]["workspaceId"] = json!(C),
            "project" => completion["application"]["projectId"] = json!("foreign-project"),
            "manifest" => completion["application"]["manifestPath"] = json!(data.join("foreign-workspace.json")),
            "update" => completion["native"]["update"] = json!(C),
            "snapshot" => completion["native"]["snapshotSha256"] = json!("f".repeat(64)),
            "activation" => completion["native"]["activationSha256"] = json!("f".repeat(64)),
            "candidate" => completion["native"]["candidate"]["pid"] = json!(std::process::id() + 1),
            "phase" => completion["handoff"]["record"]["phase"] = json!("stopped"),
            "source_pid" => completion["handoff"]["record"]["replacement"]["pid"] = json!(std::process::id()),
            "ordinal" => completion["ordinal"] = json!(2),
            _ => panic!("known source completion fault"),
        }
        cold_current_write_controls(&f, &mut row, &completion);
        assert!(stopped_current_activate(&mut s).is_err(), "{fault}");
        assert!(s.is_display_only(), "{fault}");
        assert!(s.persistence.is_none(), "{fault}");
        assert_eq!(fs::read_dir(data).unwrap().count(), 0, "{fault}");
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_retains_original_partial_publication_without_replay_or_edit_release() {
    let (f, _source, mut s, data, _row, _completion) = cold_current_candidate(true, true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    workspace_update::prepare_current_activation_test(&mut s, true).unwrap();
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(data.join("Keyboard Shortcuts").is_dir());
    assert!(!data.join("preferences.json").exists());
    let names = fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>();
    assert!(stopped_current_activate(&mut s).is_err());
    assert_eq!(fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), names);
    assert!(s.is_display_only());
    assert!(s.persistence.is_none());
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    assert!(s.execute("file.newBin", json!({"name":"No edit after cold partial effect"})).is_err());
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_keeps_original_new_worker_gated_after_creation_stop() {
    let (f, _source, mut s, data, _row, _completion) = cold_current_candidate(true, true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    workspace_update::prepare_current_activation_worker_stop_test(&mut s).unwrap();
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(s.is_display_only());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().auto_save_now().is_err());
    let bytes = fs::read(data.join("preferences.json")).unwrap();
    s.persistence.as_ref().unwrap().flush();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(fs::read(data.join("preferences.json")).unwrap(), bytes);
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    assert!(stopped_current_activate(&mut s).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_authored_source_generation_mode_and_namespace_refuse_before_effects() {
    use std::os::unix::fs::PermissionsExt;
    for fault in ["same_bytes", "source_mode", "source_namespace", "foreign_target", "foreign_recovery"] {
        let (f, _source, mut s, data, _, _) = cold_current_candidate(true, true);
        workspace_update::prepare_current_activation_test(&mut s, false).unwrap();
        let source = f._source.0.join("current-paused-state/Graphics Templates/current.json");
        match fault {
            "same_bytes" => replace_identical(&source),
            "source_mode" => fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap(),
            "source_namespace" => fs::write(source.parent().unwrap().join("new.json"), b"new authored member").unwrap(),
            "foreign_target" => fs::write(data.join("foreign-human.json"), b"later human data").unwrap(),
            "foreign_recovery" => {
                fs::create_dir(data.join("Recovery")).unwrap();
                fs::write(data.join("Recovery/foreign-human.json"), b"later human data").unwrap();
            }
            _ => panic!("known authored fault"),
        }
        assert!(stopped_current_activate(&mut s).is_err(), "{fault}");
        assert!(s.is_display_only(), "{fault}");
        assert!(s.persistence.is_none(), "{fault}");
        if fault == "foreign_recovery" {
            assert_eq!(fs::read(data.join("Recovery/foreign-human.json")).unwrap(), b"later human data");
            assert_eq!(fs::read_dir(&data).unwrap().count(), 1);
        } else if fault == "foreign_target" {
            assert_eq!(fs::read(data.join("foreign-human.json")).unwrap(), b"later human data");
            assert_eq!(fs::read_dir(&data).unwrap().count(), 1);
        } else {
            assert_eq!(fs::read_dir(&data).unwrap().count(), 0, "{fault}");
        }
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_authored_partial_copy_and_directory_seal_never_replay_or_release_worker() {
    use std::os::unix::fs::PermissionsExt;
    fn listing(path: &Path) -> Vec<PathBuf> {
        let mut pending = vec![path.to_path_buf()];
        let mut seen = Vec::new();
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path.clone());
                }
                seen.push(path);
                assert!(seen.len() < 4096);
            }
        }
        seen.sort();
        seen
    }
    for phase in [1, 2] {
        let (f, _source, mut s, data, _, _) = cold_current_candidate(true, true);
        let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
        workspace_update::prepare_current_activation_authored_stop_test(&mut s, phase).unwrap();
        let error = stopped_current_activate(&mut s).unwrap_err().to_string();
        assert!(error.contains("controlled Stop after actual authored publication effect"), "{error}");
        assert_eq!(fs::read(data.join("Graphics Templates/current.json")).unwrap(), b"complete current resource");
        if phase == 2 {
            assert_eq!(fs::metadata(data.join("Graphics Templates/Locked")).unwrap().permissions().mode() & 0o7777, 0o550);
        }
        assert!(s.is_display_only());
        assert!(s.persistence.is_none());
        let effects = listing(&data);
        assert!(effects.iter().any(|p| p.file_name().unwrap().to_string_lossy().starts_with(".poema-copy-")));
        assert!(stopped_current_activate(&mut s).is_err());
        assert_eq!(listing(&data), effects);
        assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
        assert!(s.execute("file.newBin", json!({"name":"No edit after authored partial effect"})).is_err());
        s.shutdown();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_original_journal_generation_and_prepared_worker_fences_prevent_release() {
    for fault in [1, 2, 3, 4, 5, 6] {
        let (f, _source, mut s, data, _, _) = cold_current_candidate(true, true);
        let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
        workspace_update::prepare_current_activation_journal_fault_test(&mut s, fault).unwrap();
        assert!(stopped_current_activate(&mut s).is_err(), "journal fault {fault}");
        assert!(s.is_display_only(), "journal fault {fault}");
        let p = s.persistence.as_ref().unwrap();
        assert!(p.startup_pending(), "journal fault {fault}");
        assert!(p.auto_save_now().is_err());
        assert!(data.join("preferences.json").is_file());
        assert!(data.join("Recovery").is_dir());
        let preferences = fs::read(data.join("preferences.json")).unwrap();
        p.flush();
        assert_eq!(fs::read(data.join("preferences.json")).unwrap(), preferences);
        assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
        assert!(stopped_current_activate(&mut s).is_err());
        assert!(s.execute("file.newBin", json!({"name":"No edit after native journal fault"})).is_err());
        s.shutdown();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_current_activation_authored_namespace_after_copy_retains_foreign_file_and_stops_before_worker() {
    let (f, _source, mut s, data, _, _) = cold_current_candidate(true, true);
    let saved = fs::read(f.boot["saved"].as_str().unwrap()).unwrap();
    workspace_update::prepare_current_activation_authored_stop_test(&mut s, 3).unwrap();
    let error = stopped_current_activate(&mut s).unwrap_err().to_string();
    assert!(error.contains("foreign native authored or canonical namespace member"), "{error}");
    assert_eq!(fs::read(data.join("foreign-after-copy.json")).unwrap(), b"later human data");
    assert_eq!(fs::read(data.join("Graphics Templates/current.json")).unwrap(), b"complete current resource");
    assert!(s.is_display_only());
    assert!(s.persistence.is_none());
    assert!(!data.join("Recovery").exists());
    assert!(stopped_current_activate(&mut s).is_err());
    assert_eq!(fs::read(data.join("foreign-after-copy.json")).unwrap(), b"later human data");
    assert_eq!(fs::read(f.boot["saved"].as_str().unwrap()).unwrap(), saved);
    assert!(s.execute("file.newBin", json!({"name":"No edit after foreign authored namespace"})).is_err());
    s.shutdown();
}

#[test]
fn complete_cold_start_preserves_existing_native_command_journal_without_replay() {
    let (_dir, source, saved, request) = newer_journal_source(true);
    let source_journal = source.journal.clone();
    assert!(!source_journal.is_empty());
    let saved_before = fs::read(source.path.as_ref().unwrap()).unwrap();
    let snapshot_before = fs::read(saved["path"].as_str().unwrap()).unwrap();

    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    assert!(fresh.is_display_only());
    assert!(fresh.persistence.is_none());
    assert_eq!(*fresh.project, *source.project);
    assert_eq!(fresh.revision, source.revision);
    assert_eq!(fresh.history.undo.len(), source.history.undo.len());
    assert_eq!(
        fresh.journal, source_journal,
        "cold startup must preserve the passive native command journal"
    );
    assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), saved_before);
    assert_eq!(fs::read(saved["path"].as_str().unwrap()).unwrap(), snapshot_before);
}

#[test]
fn complete_cold_start_keeps_the_latest_successful_direct_command_without_an_extra_sync() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    let saved = stage(&mut source);
    let mut request = startup(&source, &saved);
    request["latestJournal"] = json!(true);
    source.cancel_workspace_update().unwrap();
    source.execute("file.newBin", json!({"name":"Final successful direct command"})).unwrap();
    let expected = source.journal.clone();
    assert_eq!(expected.last().unwrap().0, "file.newBin");
    source.persistence.take().unwrap().simulate_crash();
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    assert_eq!(fresh.journal, expected);
    assert_eq!(*fresh.project, *source.project);
    assert_eq!(fresh.revision, source.revision);
    assert_eq!(fresh.history.undo.len(), source.history.undo.len());
    assert!(fresh.is_display_only());
    assert!(fresh.persistence.is_none());
}
#[test]
fn complete_cold_start_keeps_grouped_agent_metadata_and_original_owned_undo_redo() {
    let (_dir, source, _saved, request) = newer_journal_source(false);
    assert!(source.journal.iter().any(|(id, value)| id == "agent.replayBatch" && value["label"] == "Metadata bin"));
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    assert_eq!(fresh.journal, source.journal);
    assert_eq!(*fresh.project, *source.project);
    assert_eq!(fresh.revision, source.revision);
    activate(&mut fresh);
    undo(&mut fresh, 301, 12);
    assert_ne!(*fresh.project, *source.project);
    redo(&mut fresh, 302, 12, 301);
    assert_eq!(*fresh.project, *source.project);
}
#[test]
fn complete_cold_start_base_bundle_keeps_command_metadata_and_later_human_history() {
    let (_dir, mut source) = setup();
    bin(&mut source, 11);
    source.execute("file.newBin", json!({"name":"Human before staged snapshot"})).unwrap();
    let saved = stage(&mut source);
    let expected = source.journal.clone();
    retire(&mut source, &saved);
    let mut fresh = Session::default();
    fresh.open_project_recovery(&startup(&source, &saved)).unwrap();
    assert_eq!(fresh.journal, expected);
    assert_eq!(*fresh.project, *source.project);
    activate(&mut fresh);
    fresh.execute("edit.undo", json!({})).unwrap();
    assert_ne!(*fresh.project, *source.project);
    fresh.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*fresh.project, *source.project);
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn complete_cold_current_image_keeps_passive_command_log_across_original_activation() {
    let (fixture, _source, mut candidate, _data, _, _) = cold_current_candidate(true, true);
    let expected: Vec<(String, Value)> = serde_json::from_value(fixture.boot["body"]["editor"]["command_journal"].clone()).unwrap();
    assert!(!expected.is_empty());
    assert_eq!(candidate.journal, expected);
    let graph = candidate.project.clone();
    assert_eq!(stopped_current_activate(&mut candidate).unwrap()["status"], "activated");
    assert_eq!(candidate.journal, expected);
    assert!(std::sync::Arc::ptr_eq(&graph, &candidate.project));
    undo(&mut candidate, 1601, 11);
    redo(&mut candidate, 1602, 11, 1601);
    assert_eq!(*candidate.project, *graph);
    candidate.shutdown();
}

#[test]
fn paused_cold_start_can_read_saved_preferences_without_activation_or_state_changes() {
    let (_dir, source, saved, request) = newer_journal_source_with_preferences(true, true);
    let mut fresh = Session::default();
    fresh.open_project_recovery(&request).unwrap();
    assert!(fresh.is_display_only());
    assert!(fresh.persistence.is_none());
    let project = fresh.project.clone();
    let history = fresh.history.clone();
    let journal = fresh.journal.clone();
    let state = serde_json::to_value(&fresh.state).unwrap();
    let preferences = fresh.prefs.to_value();
    let revision = fresh.revision;
    let saved_file = fs::read(source.path.as_ref().unwrap()).unwrap();
    let snapshot_file = fs::read(saved["path"].as_str().unwrap()).unwrap();
    let value = fresh.execute("prefs.get", json!({"key":"autoSave.maxVersions"}))
        .expect("paused cold startup must permit the existing read-only preference query");
    assert_eq!(value, json!(7));
    assert!(fresh.workspace_update_allows("prefs.get"));
    assert_eq!(fresh.execute("prefs.get", json!({})).unwrap()["values"], preferences);
    assert!(std::sync::Arc::ptr_eq(&fresh.project, &project));
    assert!(crate::trim::command_settlement::historical::same_history(&fresh.history, &history));
    assert_eq!(fresh.journal, journal);
    assert_eq!(serde_json::to_value(&fresh.state).unwrap(), state);
    assert_eq!(fresh.prefs.to_value(), preferences);
    assert_eq!(fresh.revision, revision);
    assert!(fresh.is_display_only());
    assert!(fresh.persistence.is_none());
    assert_eq!(fresh.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert_eq!(fs::read(source.path.as_ref().unwrap()).unwrap(), saved_file);
    assert_eq!(fs::read(saved["path"].as_str().unwrap()).unwrap(), snapshot_file);
}

#[test]
fn paused_preference_read_owner_keeps_writes_replay_and_unsupported_queries_gated() {
    let (_dir, mut owner) = setup();
    owner.execute("prefs.set", json!({"key":"autoSave.maxVersions","value":17})).unwrap();
    let staged = stage(&mut owner);
    let project = owner.project.clone();
    let history = owner.history.clone();
    let journal = owner.journal.clone();
    let preferences = owner.prefs.to_value();
    let state = serde_json::to_value(&owner.state).unwrap();
    let revision = owner.revision;
    let saved_before = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let prefs_path = owner.persistence.as_ref().unwrap().data_dir.join("preferences.json");
    let prefs_before = fs::read(&prefs_path).unwrap();
    let snapshot_before = fs::read(staged["path"].as_str().unwrap()).unwrap();
    assert!(owner.workspace_update_allows("prefs.get"));
    assert_eq!(owner.execute("prefs.get", json!({"key":"autoSave.maxVersions"})).unwrap(), json!(17));
    assert_eq!(owner.execute("prefs.get", json!({})).unwrap()["values"], preferences);
    let error = owner.execute("prefs.get", json!({"key":"not.a.native.preference"})).unwrap_err().to_string();
    assert!(error.contains("unknown preference"), "{error}");
    for (command, params) in [
        ("prefs.set", json!({"key":"autoSave.maxVersions","value":99})),
        ("prefs.reset", json!({})),
        ("prefs.schema", json!({})),
        ("file.newBin", json!({"name":"Blocked during update"})),
        ("file.save", json!({})),
        ("agent.replayBatch", json!({"label":"No replay","operations":[]})),
    ] {
        assert!(!owner.workspace_update_allows(command), "{command}");
        assert!(owner.execute(command, params).is_err(), "{command}");
    }
    assert!(owner.is_display_only());
    assert!(std::sync::Arc::ptr_eq(&owner.project, &project));
    assert!(crate::trim::command_settlement::historical::same_history(&owner.history, &history));
    assert_eq!(owner.journal, journal);
    assert_eq!(owner.prefs.to_value(), preferences);
    assert_eq!(serde_json::to_value(&owner.state).unwrap(), state);
    assert_eq!(owner.revision, revision);
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), saved_before);
    assert_eq!(fs::read(prefs_path).unwrap(), prefs_before);
    assert_eq!(fs::read(staged["path"].as_str().unwrap()).unwrap(), snapshot_before);
    owner.shutdown();
}
#[test]
fn paused_preference_read_does_not_override_a_real_display_projection() {
    let (_dir, mut owner) = setup();
    let _staged = stage(&mut owner);
    let mut display = owner.display_projection();
    assert!(display.is_display_only());
    let journal = display.journal.clone();
    let error = display.execute("prefs.get", json!({"key":"autoSave.maxVersions"})).unwrap_err().to_string();
    assert!(error.contains("display projection cannot execute"), "{error}");
    assert_eq!(display.journal, journal);
    assert!(owner.is_display_only());
    assert_eq!(owner.execute("prefs.get", json!({"key":"autoSave.maxVersions"})).unwrap(), json!(owner.prefs.auto_save.max_versions));
    owner.shutdown();
}
#[test]
fn paused_preference_read_does_not_override_original_retirement() {
    let (_dir, mut owner) = setup();
    let staged = stage(&mut owner);
    retire(&mut owner, &staged);
    let journal = owner.journal.clone();
    assert!(!owner.workspace_update_allows("prefs.get"));
    let error = owner.execute("prefs.get", json!({"key":"autoSave.maxVersions"})).unwrap_err().to_string();
    assert!(error.contains("retiring"), "{error}");
    assert_eq!(owner.journal, journal);
    assert!(owner.is_display_only());
}
