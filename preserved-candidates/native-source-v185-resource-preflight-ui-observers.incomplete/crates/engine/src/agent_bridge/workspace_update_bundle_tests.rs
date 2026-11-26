use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const UPDATE: &str = "22222222-2222-4222-8222-222222222222";
const CHECKPOINT: &str = "33333333-3333-4333-8333-333333333333";
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = crate::temp_dir().join(format!("workspace-bundle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        Self(fs::canonicalize(dir).unwrap())
    }
    fn bundle(&self) -> PathBuf {
        self.0.join(format!(".poema-update-{WORKSPACE}-{UPDATE}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Scratch, Session) {
    let dir = Scratch::new();
    let mut s = super::workspace_update_tests::setup();
    s.path = Some(dir.0.join("generated.fcproj").to_str().unwrap().into());
    fs::write(s.path.as_ref().unwrap(), filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false)).unwrap();
    s.saved_revision = s.revision;
    (dir, s)
}
fn scope(s: &Session) -> Value {
    json!({"projectId":"p_update","anchorSequence":s.state.active_sequence.unwrap().0})
}
fn basis(s: &mut Session, n: usize) -> String {
    let token = super::tests::token(n);
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope(s)})).unwrap();
    token
}
fn trim(s: &mut Session, n: usize) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    super::tests::read(s, n);
    s.execute(
        "agent.apply",
        super::tests::exact_batch(n, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
}
fn pause(s: &mut Session) -> Value {
    let token = basis(s, 100_000);
    let r = json!({"token":token,"scope":scope(s),"workspace":WORKSPACE,"update":UPDATE,"nativeSession":s.agent_bridge.epoch});
    assert_eq!(s.execute("agent.beginWorkspaceUpdate", r.clone()).unwrap()["status"], "paused");
    r
}
fn save(s: &mut Session) -> Value {
    let token = basis(s, 100_001);
    s.execute("agent.saveHistoryCheckpoint", json!({"token":token,"scope":scope(s),"workspace":WORKSPACE,"checkpoint":CHECKPOINT,"metadataBasis":true}))
        .unwrap()
}
fn stage_request(s: &mut Session, paused: &Value, saved: &Value) -> Value {
    let token = basis(s, 100_002);
    json!({"token":token,"scope":scope(s),"workspace":WORKSPACE,"update":UPDATE,"pauseToken":paused["token"],
        "checkpoint":CHECKPOINT,"checkpointSha256":saved["checkpoint"]["sha256"]})
}
fn staged(s: &mut Session) -> (Value, Value, Value) {
    let paused = pause(s);
    let saved = save(s);
    let request = stage_request(s, &paused, &saved);
    let receipt = s.execute("agent.stageWorkspaceUpdate", request.clone()).unwrap();
    assert_eq!(receipt["status"], "staged");
    (paused, request, receipt)
}
fn startup(receipt: &Value) -> Value {
    json!({"projectPath":receipt["projectPath"],"scope":receipt["scope"],"workspace":WORKSPACE,"update":UPDATE,"sha256":receipt["sha256"]})
}
fn activation(s: &Session, request: &Value) -> Value {
    json!({"token":request["token"],"scope":request["scope"],"workspace":WORKSPACE,"update":UPDATE,"nativeSession":s.agent_bridge.epoch})
}
fn history(s: &Session) -> Value {
    json!({"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key})
}
fn editor(s: &Session) -> Value {
    json!({"state":s.state,"clipboard":s.state.clipboard,"clipboardMarkers":s.state.clipboard_markers})
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn workspace_update_retained_history_validation_preserves_revision_and_graph_across_repeated_host_adoption() {
    let (_dir, mut owner) = setup();
    trim(&mut owner, 102_000);
    let (_, stage_request, staged) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&staged)).unwrap();
    candidate.agent_operation_control().enable_transport();
    candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &stage_request)).unwrap();
    // The same opaque bundle can be used for rollback on a new native epoch.
    // A delayed activation/Cancel for the old process must never reach it.
    let mut later = Session::default();
    later.open_workspace_update(&startup(&staged)).unwrap();
    later.agent_operation_control().enable_transport();
    let delayed = activation(&candidate, &stage_request);
    assert!(later.execute("agent.activateWorkspaceUpdate", delayed.clone()).is_err());
    assert!(later.execute("agent.cancelWorkspaceUpdate", delayed).is_err());
    assert_eq!(later.workspace_update_status().unwrap()["status"], "candidate_ready");
    later.execute("agent.activateWorkspaceUpdate", activation(&later, &stage_request)).unwrap();
    let project = candidate.project.clone();
    let old_history = history(&candidate);
    let revision = candidate.revision;
    for n in 102_001..102_004 {
        super::tests::read(&mut candidate, n);
        let r = candidate
            .execute(
                "agent.validateRetainedHistory",
                json!({"token":super::tests::token(n),"scope":scope(&candidate),
            "workspace":WORKSPACE,"checkpoint":CHECKPOINT,"sha256":staged["checkpointSha256"]}),
            )
            .unwrap();
        assert_eq!(r["status"], "retained");
        assert_eq!(candidate.revision, revision);
        assert!(Arc::ptr_eq(&candidate.project, &project));
        assert_eq!(history(&candidate), old_history);
    }
    let token = basis(&mut candidate, 102_004);
    assert!(
        candidate
            .execute(
                "agent.validateRetainedHistory",
                json!({"token":token,"scope":scope(&candidate),"workspace":WORKSPACE,
        "checkpoint":CHECKPOINT,"sha256":staged["checkpointSha256"],"metadataBasis":true})
            )
            .is_err()
    );
    trim(&mut candidate, 102_005);
    super::tests::read(&mut candidate, 102_006);
    let changed = candidate.project.clone();
    let revision = candidate.revision;
    assert!(
        candidate
            .execute(
                "agent.validateRetainedHistory",
                json!({"token":super::tests::token(102_006),"scope":scope(&candidate),
        "workspace":WORKSPACE,"checkpoint":CHECKPOINT,"sha256":staged["checkpointSha256"]})
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(&candidate.project, &changed));
    assert_eq!(candidate.revision, revision);
}

#[test]
fn retirement_requires_the_exact_staged_owner_and_keeps_dirty_state_and_persistence_paused_until_exit() {
    let (_dir, mut owner) = setup();
    trim(&mut owner, 101_000);
    let bytes = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let (pause, _, stage) = staged(&mut owner);
    let project = owner.project.clone();
    let old_history = history(&owner);
    let token = basis(&mut owner, 101_001);
    let request = json!({"token":token,"scope":scope(&owner),"workspace":WORKSPACE,"update":UPDATE,
        "pauseToken":pause["token"],"stageSha256":stage["sha256"]});
    for key in ["workspace", "update", "pauseToken", "stageSha256", "token"] {
        let mut invalid = request.clone();
        invalid[key] = json!("invalid");
        assert!(owner.execute("agent.retireWorkspaceUpdate", invalid).is_err());
        assert_eq!(owner.workspace_update_status().unwrap()["status"], "paused");
    }
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&stage)).unwrap();
    candidate.agent_operation_control().enable_transport();
    assert!(candidate.execute("agent.retireWorkspaceUpdate", request.clone()).is_err());
    let retired = owner.execute("agent.retireWorkspaceUpdate", request).unwrap();
    assert_eq!(retired["status"], "retiring");
    assert_eq!(retired["nativePid"], std::process::id());
    assert!(owner.cancel_workspace_update().is_err());
    assert!(owner.execute("file.save", json!({})).is_err());
    assert!(owner.undo().is_none());
    assert!(Arc::ptr_eq(&owner.project, &project));
    assert_eq!(history(&owner), old_history);
    owner.shutdown();
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), bytes);
}

#[test]
fn workspace_bundle_transfers_dirty_cut_exact_native_state_history_and_clipboard_without_saving_or_activating() {
    let (dir, mut owner) = setup();
    let saved_bytes = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let audio = owner.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    owner.execute("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-7})).unwrap();
    trim(&mut owner, 99_000);
    let clip = owner.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    owner.execute("timeline.select", json!({"clips":[clip]})).unwrap();
    owner.execute("edit.copy", json!({})).unwrap();
    assert!(!owner.state.clipboard.is_empty());
    // Exact native ticks above JavaScript's safe integer limit remain opaque.
    owner.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_993);
    let before = owner.project.clone();
    let before_history = history(&owner);
    let before_editor = editor(&owner);
    let revision = owner.revision;
    let saved_revision = owner.saved_revision;
    let (_, request, receipt) = staged(&mut owner);
    let raw = fs::read(dir.bundle()).unwrap();
    assert_eq!(hash(&raw), receipt["sha256"]);
    assert_eq!(owner.execute("agent.stageWorkspaceUpdate", request.clone()).unwrap(), receipt);
    assert!(Arc::ptr_eq(&before, &owner.project));
    assert_eq!(history(&owner), before_history);
    assert_eq!(editor(&owner), before_editor);
    let mut candidate = Session::default();
    let ready = candidate.open_workspace_update(&startup(&receipt)).unwrap();
    assert_eq!(ready["status"], "candidate_ready");
    assert_eq!(ready["role"], "candidate");
    assert_eq!(ready["writeAuthority"], false);
    assert_ne!(ready["nativeSession"], receipt["nativeSession"]);
    assert!(candidate.is_display_only());
    assert_eq!(serde_json::to_value(&candidate.project).unwrap(), serde_json::to_value(&before).unwrap());
    assert_eq!(candidate.path, owner.path);
    assert_eq!(candidate.revision, revision);
    assert_eq!(candidate.saved_revision, saved_revision);
    assert!(candidate.is_dirty());
    assert_eq!(history(&candidate), before_history);
    assert_eq!(editor(&candidate), before_editor);
    assert!(candidate.execute("edit.undo", json!({})).is_err());
    assert!(candidate.undo().is_none());
    assert!(candidate.execute("agent.snapshot", json!({"token":super::tests::token(100_100)})).is_err());
    assert!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).is_err(), "no independent transport yet");
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut candidate).unwrap();
    candidate.agent_operation_control().enable_transport();
    assert_eq!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).unwrap()["status"], "activated");
    assert!(!candidate.is_display_only());
    assert_eq!(history(&candidate), before_history);
    assert_eq!(editor(&candidate), before_editor);
    assert!(candidate.execute("agent.apply", json!({"token":request["token"],"label":"Metadata is not an edit ticket","operations":[]})).is_err());
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), saved_bytes);
    assert!(owner.is_display_only(), "candidate activation does not retire the previous owner; coordinator must do so");
}

#[test]
fn workspace_bundle_adopted_checkpoint_reconciles_existing_host_recovery_and_owned_undo_without_replaying() {
    let (_dir, mut owner) = setup();
    let canonical = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let audio = owner.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    owner.execute("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-7})).unwrap();
    let human = owner.project.clone();
    trim(&mut owner, 99_010);
    let (_, request, receipt) = staged(&mut owner);
    let before_history = history(&owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).unwrap();
    let before = candidate.project.clone();
    super::tests::read(&mut candidate, 100_200);
    let restored = candidate
        .execute(
            "agent.restoreHistoryCheckpoint",
            json!({"token":super::tests::token(100_200),"scope":scope(&candidate),
        "workspace":WORKSPACE,"checkpoint":CHECKPOINT,"sha256":request["checkpointSha256"]}),
        )
        .unwrap();
    assert_eq!(restored["action"], "history_restore");
    assert!(Arc::ptr_eq(&before, &candidate.project));
    assert_eq!(history(&candidate), before_history);
    assert!(candidate.is_dirty());
    super::tests::read(&mut candidate, 100_201);
    candidate.execute("agent.undo", json!({"token":super::tests::token(100_201),"commit":super::tests::token(99_010)})).unwrap();
    assert_eq!(serde_json::to_value(&candidate.project).unwrap(), serde_json::to_value(&human).unwrap());
    candidate.execute("edit.undo", json!({})).unwrap();
    assert_eq!(candidate.active_sequence().unwrap().audio_tracks[0].items[0].gain_db, 0.0);
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), canonical);
}

#[test]
fn workspace_bundle_cancelled_candidate_cannot_become_a_second_writer_and_old_owner_resumes_unchanged() {
    let (_dir, mut owner) = setup();
    trim(&mut owner, 99_020);
    let project = owner.project.clone();
    let before_history = history(&owner);
    let (paused, request, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    let cancelled = candidate.cancel_workspace_update().unwrap().unwrap();
    assert_eq!(cancelled["status"], "candidate_cancelled");
    assert_eq!(cancelled["editingPaused"], true);
    assert!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).is_err());
    assert!(candidate.execute("file.save", json!({})).is_err());
    assert!(candidate.execute("edit.undo", json!({})).is_err());
    assert!(candidate.is_display_only());
    owner.execute("agent.cancelWorkspaceUpdate", paused).unwrap();
    assert!(!owner.is_display_only());
    assert!(Arc::ptr_eq(&owner.project, &project));
    assert_eq!(history(&owner), before_history);
    assert!(owner.execute("agent.stageWorkspaceUpdate", request).is_err());
}

#[test]
fn workspace_bundle_corrupt_foreign_redirected_and_changed_saved_files_leave_fresh_candidates_untouched() {
    let (dir, mut owner) = setup();
    let (_, request, receipt) = staged(&mut owner);
    let raw = fs::read(dir.bundle()).unwrap();
    for key in ["workspace", "update", "sha256"] {
        let mut args = startup(&receipt);
        args[key] = json!(if key == "sha256" { "f".repeat(64) } else { "44444444-4444-4444-8444-444444444444".into() });
        let mut candidate = Session::default();
        let before = candidate.project.clone();
        assert!(candidate.open_workspace_update(&args).is_err());
        assert!(Arc::ptr_eq(&candidate.project, &before));
        assert!(candidate.path.is_none());
    }
    let mut damaged = raw.clone();
    *damaged.last_mut().unwrap() ^= 1;
    fs::write(dir.bundle(), &damaged).unwrap();
    let mut candidate = Session::default();
    assert!(candidate.open_workspace_update(&startup(&receipt)).is_err());
    assert!(candidate.path.is_none());
    fs::write(dir.bundle(), &raw).unwrap();
    #[cfg(unix)]
    {
        let actual = dir.0.join("retained-bundle.json");
        fs::rename(dir.bundle(), &actual).unwrap();
        std::os::unix::fs::symlink(&actual, dir.bundle()).unwrap();
        assert!(Session::default().open_workspace_update(&startup(&receipt)).is_err());
        fs::remove_file(dir.bundle()).unwrap();
        fs::rename(actual, dir.bundle()).unwrap();
    }
    let canonical = fs::read(owner.path.as_ref().unwrap()).unwrap();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    fs::write(owner.path.as_ref().unwrap(), b"external saved-project change").unwrap();
    assert!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).is_err());
    assert!(candidate.is_display_only());
    assert!(Session::default().open_workspace_update(&startup(&receipt)).is_err());
    fs::write(owner.path.as_ref().unwrap(), canonical).unwrap();
    assert_eq!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).unwrap()["status"], "activated");
}

#[test]
fn workspace_bundle_collision_is_unknown_and_cannot_overwrite_or_repeat_the_native_stage() {
    let (dir, mut owner) = setup();
    let before = owner.project.clone();
    let canonical = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let paused = pause(&mut owner);
    let saved = save(&mut owner);
    let request = stage_request(&mut owner, &paused, &saved);
    fs::write(dir.bundle(), b"existing owned or foreign artifact").unwrap();
    assert!(owner.execute("agent.stageWorkspaceUpdate", request.clone()).is_err());
    assert_eq!(owner.execute("agent.stageWorkspaceUpdate", request).unwrap()["status"], "unknown");
    assert_eq!(fs::read(dir.bundle()).unwrap(), b"existing owned or foreign artifact");
    assert!(Arc::ptr_eq(&owner.project, &before));
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), canonical);
}

#[test]
fn workspace_update_waits_for_real_pending_native_work_without_stopping_or_replaying_it() {
    let (_dir, mut owner) = setup();
    let token = basis(&mut owner, 100_300);
    let result = Arc::new(Mutex::new(None));
    owner.jobs.push(crate::Job {
        id: 1,
        label: "Pending native export".into(),
        progress: Arc::new(filmcraft_export::Progress::default()),
        result: result.clone(),
    });
    let request = json!({"token":token,"scope":scope(&owner),"workspace":WORKSPACE,"update":UPDATE});
    assert!(owner.execute("agent.beginWorkspaceUpdate", request.clone()).is_err());
    assert!(!owner.is_display_only());
    assert!(!owner.jobs[0].progress.cancel.load(Ordering::Relaxed));
    *result.lock().unwrap() = Some(Err("generated export settled".into()));
    assert_eq!(owner.execute("agent.beginWorkspaceUpdate", request).unwrap()["status"], "paused");
}

#[test]
fn workspace_bundle_keeps_native_relative_media_resolution_for_the_preceding_human_history() {
    let (dir, mut owner) = setup();
    let item = owner.active_sequence().unwrap().video_tracks[0].items[0].item;
    for name in ["prior.wav", "current.wav"] {
        fs::write(dir.0.join(name), filmcraft_media::wav::write_wav16(&vec![0.25; 48000], 1, 48000)).unwrap();
    }
    for name in ["prior.wav", "current.wav"] {
        owner
            .edit("Human relink fixture", |project, _| {
                project.item_mut(item).unwrap().as_media_mut().unwrap().media = filmcraft_project::MediaRef::File { path: name.into() };
                Ok(())
            })
            .unwrap();
    }
    let (_, request, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).unwrap();
    candidate.execute("edit.undo", json!({})).unwrap();
    assert_eq!(candidate.project.item(item).unwrap().as_media().unwrap().media, filmcraft_project::MediaRef::File { path: "prior.wav".into() });
    let source = candidate.source(item).unwrap();
    assert_eq!(candidate.media.offline_status(item), None);
    assert_eq!(source.info().audio().as_ref().unwrap().sample_rate, 48000);
}

#[test]
fn workspace_update_fences_real_autosaving_and_candidate_persistence_starts_only_on_activation_with_original_preferences() {
    let (dir, mut owner) = setup();
    let original = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let old_data = dir.0.join("old-native-state");
    owner.start_autosave(crate::autosave::AutosaveConfig::new(&old_data)).unwrap();
    owner.execute("prefs.set", json!({"values":{"autoSave.saveCurrentProject":true,"trim.largeTrimOffset":12,"media.enableProxies":false}})).unwrap();
    trim(&mut owner, 99_030);
    let preferences = owner.prefs.clone();
    let (_, request, receipt) = staged(&mut owner);
    assert!(owner.persistence.as_ref().unwrap().auto_save_now().unwrap_err().contains("paused"));
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), original);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    assert_eq!(candidate.prefs, preferences);
    assert!(!candidate.media.use_proxies());
    assert!(candidate.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(&old_data)).is_err());
    let new_data = dir.0.join("new-native-state");
    candidate.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(&new_data)).unwrap();
    assert!(candidate.persistence.is_none());
    assert!(!new_data.join("preferences.json").exists());
    assert!(candidate.start_autosave(crate::autosave::AutosaveConfig::new(&new_data)).is_err());
    candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).unwrap();
    assert!(candidate.persistence.is_some());
    assert_eq!(candidate.prefs, preferences);
    assert_eq!(crate::autosave::Preferences::load(&new_data.join("preferences.json")), preferences);
    let clip = candidate.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    candidate.execute("timeline.trim", json!({"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1})).unwrap();
    candidate.sync_persistence();
    assert!(candidate.persistence.as_ref().unwrap().auto_save_now().unwrap().is_some());
    let current = fs::read(candidate.path.as_ref().unwrap()).unwrap();
    assert_ne!(current, original);
    owner.shutdown();
    assert_eq!(
        fs::read(candidate.path.as_ref().unwrap()).unwrap(),
        current,
        "old paused shutdown may flush private recovery, never overwrite the active candidate"
    );
    candidate.shutdown();
}

#[test]
fn workspace_update_cancellation_resumes_autosave_and_late_pause_cancellation_cannot_strand_the_worker() {
    let (dir, mut owner) = setup();
    let original = fs::read(owner.path.as_ref().unwrap()).unwrap();
    owner.start_autosave(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).unwrap();
    owner.execute("prefs.set", json!({"key":"autoSave.saveCurrentProject","value":true})).unwrap();
    trim(&mut owner, 99_040);
    let paused = pause(&mut owner);
    assert!(owner.persistence.as_ref().unwrap().auto_save_now().unwrap_err().contains("paused"));
    owner.execute("agent.cancelWorkspaceUpdate", paused).unwrap();
    assert!(owner.persistence.as_ref().unwrap().auto_save_now().unwrap().is_some());
    assert_ne!(fs::read(owner.path.as_ref().unwrap()).unwrap(), original);
    let mut calls = 0;
    assert!(
        owner
            .persistence
            .as_ref()
            .unwrap()
            .pause_workspace(|| {
                calls += 1;
                if calls >= 2 { Err("stopped after enqueue".into()) } else { Ok(()) }
            })
            .is_err()
    );
    assert!(owner.persistence.as_ref().unwrap().auto_save_now().is_ok(), "queued Resume must follow a late Pause acknowledgement");
    owner.shutdown();
}

#[test]
fn workspace_candidate_failed_persistence_start_keeps_the_dirty_native_graph_paused_and_does_not_save_the_project() {
    let (dir, mut owner) = setup();
    trim(&mut owner, 99_050);
    let canonical = fs::read(owner.path.as_ref().unwrap()).unwrap();
    let (_, request, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.agent_operation_control().enable_transport();
    let project = candidate.project.clone();
    let old_history = history(&candidate);
    let data = dir.0.join("candidate-native-state");
    candidate.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(&data)).unwrap();
    fs::remove_dir(&data).unwrap();
    fs::write(&data, b"generated persistence collision").unwrap();
    assert!(candidate.execute("agent.activateWorkspaceUpdate", activation(&candidate, &request)).is_err());
    assert!(candidate.is_display_only());
    assert!(candidate.persistence.is_none());
    assert!(Arc::ptr_eq(&candidate.project, &project));
    assert_eq!(history(&candidate), old_history);
    assert_eq!(candidate.workspace_update_status().unwrap()["status"], "candidate_ready");
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), canonical);
    assert!(candidate.execute("file.save", json!({})).is_err());
}

fn identity_candidate_request(s: &mut Session, n: usize) -> Value {
    s.worker_available = true;
    let inspect = basis(s, n);
    let r = s.execute("agent.inspectIdentityMigration", json!({"token":inspect,"scope":scope(s),"workspace":WORKSPACE})).unwrap();
    let token = basis(s, n + 1);
    json!({"token":token,"scope":scope(s),"workspace":WORKSPACE,"migration":UPDATE,"expected":{"sourceGraphSha256":r["sourceGraph"]["sha256"],"candidateGraphSha256":r["candidateGraph"]["sha256"],"savedFileSha256":hash(&fs::read(s.path.as_ref().unwrap()).unwrap())}})
}
fn identity_record(s: &mut Session, n: usize, p: &Value) -> Value {
    let token = super::tests::token(n);
    let root = json!({"projectId":"p_update","workspaceId":WORKSPACE});
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":root})).unwrap();
    s.execute("agent.readIdentityPublication", json!({"token":token,"scope":root,"workspace":WORKSPACE,"migration":UPDATE,"expected":p["expected"]})).unwrap()
}
fn promoted_activation(s: &Session) -> Value {
    let h = s.workspace_update_status().unwrap();
    json!({"token":h["token"],"scope":h["scope"],"workspace":WORKSPACE,"update":UPDATE,"nativeSession":h["nativeSession"]})
}
#[test]
fn candidate_identity_promotion_preserves_real_source_autosave_pause_and_defers_target_persistence_until_rotated_activation() {
    let (dir, mut owner) = setup();
    let original = fs::read(owner.path.as_ref().unwrap()).unwrap();
    owner.start_autosave(crate::autosave::AutosaveConfig::new(dir.0.join("old-state"))).unwrap();
    trim(&mut owner, 120000);
    let owner_graph = owner.project.clone();
    let (_, old_activation, receipt) = staged(&mut owner);
    let bundle = fs::read(dir.bundle()).unwrap();
    let companion = dir.0.join(format!(".poema-history-{WORKSPACE}-{CHECKPOINT}.json"));
    let old_history = fs::read(&companion).unwrap();
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.worker_available = true;
    candidate.agent_operation_control().enable_transport();
    let target = dir.0.join("target-state");
    candidate.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(&target)).unwrap();
    let old = activation(&candidate, &old_activation);
    let before = candidate.workspace_update_status().unwrap();
    let p = identity_candidate_request(&mut candidate, 120001);
    let publication = candidate.execute("agent.publishIdentityMigration", p.clone()).unwrap();
    let after = candidate.workspace_update_status().unwrap();
    assert_eq!(after["status"], "candidate_ready");
    assert_eq!(after["revision"], before["revision"].as_u64().unwrap() + 1);
    assert_ne!(after["token"], before["token"]);
    assert!(candidate.persistence.is_none());
    assert!(!target.join("preferences.json").exists());
    assert!(candidate.is_display_only());
    assert_eq!(identity_record(&mut candidate, 120003, &p)["liveState"], "applied");
    assert!(candidate.execute("agent.activateWorkspaceUpdate", old.clone()).is_err());
    assert!(candidate.execute("agent.cancelWorkspaceUpdate", old).is_err());
    assert!(owner.persistence.as_ref().unwrap().auto_save_now().unwrap_err().contains("paused"));
    assert!(Arc::ptr_eq(&owner.project, &owner_graph));
    assert!(crate::project_workspace::identity(&owner.project).unwrap().is_none());
    assert_eq!(fs::read(&companion).unwrap(), old_history);
    assert_eq!(fs::read(dir.bundle()).unwrap(), bundle);
    assert_eq!(fs::read(owner.path.as_ref().unwrap()).unwrap(), original);
    owner.shutdown();
    assert_eq!(publication["execution"], "snapshot-published");
    let activate = promoted_activation(&candidate);
    candidate.execute("agent.activateWorkspaceUpdate", activate).unwrap();
    assert!(candidate.persistence.is_some());
    assert!(target.join("preferences.json").is_file());
    assert!(crate::project_workspace::identity(&candidate.project).unwrap().is_some());
    candidate.shutdown();
}
#[test]
fn candidate_identity_promotion_refuses_original_owners_foreign_updates_cancelled_candidates_and_stopped_operations_without_mutation() {
    for mode in ["owner", "workspace", "update", "cancelled", "stop", "disk"] {
        let (dir, mut owner) = setup();
        let (_, request, receipt) = staged(&mut owner);
        let mut candidate = Session::default();
        candidate.open_workspace_update(&startup(&receipt)).unwrap();
        candidate.worker_available = true;
        candidate.agent_operation_control().enable_transport();
        let s = if mode == "owner" { &mut owner } else { &mut candidate };
        let mut p = identity_candidate_request(s, 120100);
        if mode == "workspace" {
            p["workspace"] = json!(CHECKPOINT);
        } else if mode == "update" {
            p["migration"] = json!(CHECKPOINT);
        } else if mode == "cancelled" {
            candidate_cancel(s, &request);
        } else if mode == "stop" {
            s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap();
        } else if mode == "disk" {
            let file = s.path.as_ref().unwrap();
            let mut raw = fs::read(file).unwrap();
            raw.push(b'\n');
            fs::write(file, &raw).unwrap();
            p["expected"]["savedFileSha256"] = json!(hash(&raw));
        }
        let before = history(s);
        let project = s.project.clone();
        let fence = s.workspace_update_status().unwrap();
        assert!(s.execute("agent.publishIdentityMigration", p).is_err(), "{mode}");
        assert!(Arc::ptr_eq(&s.project, &project));
        assert_eq!(history(s), before);
        assert_eq!(s.workspace_update_status().unwrap(), fence);
        assert!(!dir.0.join(format!(".poema-identity-{WORKSPACE}-{UPDATE}.json")).exists());
    }
}
fn candidate_cancel(s: &mut Session, request: &Value) {
    s.execute("agent.cancelWorkspaceUpdate", activation(s, request)).unwrap();
}
#[test]
fn candidate_identity_promotion_keeps_the_rotated_pause_after_a_committed_stop_duplicate_or_candidate_cancel() {
    let (dir, mut owner) = setup();
    let (_, request, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.worker_available = true;
    candidate.agent_operation_control().enable_transport();
    let p = identity_candidate_request(&mut candidate, 120200);
    candidate.execute("agent.publishIdentityMigration", p.clone()).unwrap();
    let file = dir.0.join(format!(".poema-identity-{WORKSPACE}-{UPDATE}.json"));
    let bytes = fs::read(&file).unwrap();
    let after = candidate.workspace_update_status().unwrap();
    assert!(candidate.execute("agent.publishIdentityMigration", p.clone()).is_err());
    assert_eq!(candidate.workspace_update_status().unwrap(), after);
    assert_eq!(candidate.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], false);
    assert!(candidate.is_display_only());
    assert_eq!(identity_record(&mut candidate, 120202, &p)["liveState"], "applied");
    assert!(candidate.execute("agent.cancelWorkspaceUpdate", activation(&candidate, &request)).is_err());
    let current = promoted_activation(&candidate);
    candidate.execute("agent.cancelWorkspaceUpdate", current.clone()).unwrap();
    assert!(candidate.execute("agent.activateWorkspaceUpdate", current).is_err());
    assert!(candidate.is_display_only());
    assert_eq!(fs::read(&file).unwrap(), bytes);
}
#[test]
fn candidate_identity_promotion_retains_its_prepared_lifecycle_basis_beyond_metadata_eviction_and_preserves_owned_undo_redo() {
    let (_dir, mut owner) = setup();
    trim(&mut owner, 120300);
    let original = owner.execute("agent.receipt", json!({"token":super::tests::token(120300)})).unwrap();
    let (_, _, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.worker_available = true;
    candidate.agent_operation_control().enable_transport();
    let p = identity_candidate_request(&mut candidate, 120301);
    candidate.execute("agent.publishIdentityMigration", p).unwrap();
    for n in 120303..120325 {
        basis(&mut candidate, n);
    }
    let activate = promoted_activation(&candidate);
    owner.shutdown();
    candidate.execute("agent.activateWorkspaceUpdate", activate).unwrap();
    assert_eq!(candidate.execute("agent.receipt", json!({"token":super::tests::token(120300)})).unwrap(), original);
    super::tests::read(&mut candidate, 120326);
    candidate.execute("agent.undo", json!({"token":super::tests::token(120326),"commit":super::tests::token(120300)})).unwrap();
    assert!(crate::project_workspace::identity(&candidate.project).unwrap().is_some());
    super::tests::read(&mut candidate, 120327);
    candidate
        .execute("agent.redo", json!({"token":super::tests::token(120327),"commit":super::tests::token(120300),"undo":super::tests::token(120326)}))
        .unwrap();
}
#[test]
fn candidate_identity_promotion_restarts_from_its_exact_record_without_changing_the_original_staged_bundle() {
    let (dir, mut owner) = setup();
    trim(&mut owner, 120400);
    let (_, _, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.worker_available = true;
    candidate.agent_operation_control().enable_transport();
    let p = identity_candidate_request(&mut candidate, 120401);
    let r = candidate.execute("agent.publishIdentityMigration", p.clone()).unwrap();
    let bundle = fs::read(dir.bundle()).unwrap();
    let startup = json!({"projectPath":candidate.path,"scope":scope(&candidate),"workspace":WORKSPACE,"migration":UPDATE,"expected":p["expected"],"sha256":r["publication"]["sha256"],"activationToken":super::tests::token(120403)});
    let graph = serde_json::to_value(&candidate.project).unwrap();
    owner.shutdown();
    candidate.shutdown();
    let mut cold = Session::default();
    cold.open_identity_publication(&startup).unwrap();
    cold.worker_available = true;
    cold.agent_operation_control().enable_transport();
    assert_eq!(serde_json::to_value(&cold.project).unwrap(), graph);
    assert_eq!(identity_record(&mut cold, 120404, &p)["liveState"], "recovered");
    assert_eq!(fs::read(dir.bundle()).unwrap(), bundle);
    let activate = promoted_activation(&cold);
    cold.execute("agent.activateWorkspaceUpdate", activate).unwrap();
}

// A target checkpoint can be durable even when its reply was lost. The next
// process starts from the exact identity decision, never the old saved file.
fn cold_identity_checkpoint() -> (Scratch, Session, Value, Value) {
    let (dir, mut owner) = setup();
    trim(&mut owner, 121000);
    let (_, _, receipt) = staged(&mut owner);
    let mut candidate = Session::default();
    candidate.open_workspace_update(&startup(&receipt)).unwrap();
    candidate.worker_available = true;
    candidate.agent_operation_control().enable_transport();
    let request = identity_candidate_request(&mut candidate, 121001);
    let publication = candidate.execute("agent.publishIdentityMigration", request.clone()).unwrap();
    let root = json!({"projectId":"p_update","workspaceId":WORKSPACE});
    let token = super::tests::token(121003);
    candidate.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":root})).unwrap();
    let saved = candidate
        .execute("agent.saveHistoryCheckpoint", json!({"token":token,"scope":root,"workspace":WORKSPACE,"checkpoint":CHECKPOINT,"metadataBasis":true}));
    // The original checkpoint ID already exists for the legacy source. A new
    // target ID is required; never repeat a save that has an unknown outcome.
    assert!(saved.is_err());
    let checkpoint = "44444444-4444-4444-8444-444444444444";
    let token = super::tests::token(121004);
    candidate.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":root})).unwrap();
    let saved = candidate
        .execute("agent.saveHistoryCheckpoint", json!({"token":token,"scope":root,"workspace":WORKSPACE,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    let startup = json!({"projectPath":candidate.path,"scope":scope(&candidate),"workspace":WORKSPACE,"migration":UPDATE,"expected":request["expected"],"sha256":publication["publication"]["sha256"],"activationToken":super::tests::token(121005)});
    owner.shutdown();
    candidate.shutdown();
    let mut cold = Session::default();
    cold.open_identity_publication(&startup).unwrap();
    cold.worker_available = true;
    cold.agent_operation_control().enable_transport();
    let activate = promoted_activation(&cold);
    cold.execute("agent.activateWorkspaceUpdate", activate).unwrap();
    let inspection = json!({"scope":root,"workspace":WORKSPACE,"checkpoint":checkpoint,"basis":{"nativeSession":saved["checkpoint"]["nativeSession"],"revision":saved["checkpoint"]["nativeRevision"],"sequence":saved["sequence"]}});
    (dir, cold, inspection, saved)
}
fn inspect_cold_checkpoint(s: &mut Session, request: &Value, n: usize) -> Result<Value> {
    let token = super::tests::token(n);
    s.execute("agent.readProject", json!({"token":token,"scope":request["scope"],"projectPath":s.path}))?;
    let mut request = request.clone();
    request["token"] = json!(token);
    s.execute("agent.inspectHistoryCheckpoint", request)
}
#[test]
fn cold_identity_origin_inspects_lost_target_save_without_mutation_or_borrowed_restore_ownership() {
    let (dir, mut cold, request, saved) = cold_identity_checkpoint();
    let path = dir.0.join(format!(".poema-history-{}-{}.json", WORKSPACE, request["checkpoint"].as_str().unwrap()));
    let raw = fs::read(&path).unwrap();
    let project = cold.project.clone();
    let before = (history(&cold), editor(&cold), cold.revision, cold.agent_bridge.history_checkpoints.len());
    let result = inspect_cold_checkpoint(&mut cold, &request, 121010).unwrap();
    assert_eq!(result["checkpoint"], saved["checkpoint"]);
    assert_eq!(result["execution"], "not-applied");
    assert_ne!(result["nativeSession"], request["basis"]["nativeSession"]);
    assert!(Arc::ptr_eq(&cold.project, &project));
    assert_eq!((history(&cold), editor(&cold), cold.revision, cold.agent_bridge.history_checkpoints.len()), before);
    assert_eq!(fs::read(&path).unwrap(), raw);
    // Successful inspection cannot be reused as a retention guard or restore.
    for (i, command) in ["agent.validateRetainedHistory", "agent.restoreHistoryCheckpoint"].into_iter().enumerate() {
        let token = super::tests::token(121011 + i);
        cold.execute("agent.readProject", json!({"token":token,"scope":request["scope"],"projectPath":cold.path})).unwrap();
        assert!(
            cold.execute(
                command,
                json!({"token":token,"scope":request["scope"],"workspace":WORKSPACE,"checkpoint":request["checkpoint"],"sha256":saved["checkpoint"]["sha256"]})
            )
            .is_err()
        );
        assert!(Arc::ptr_eq(&cold.project, &project));
    }
    // A NEW checkpoint of the actual verified runtime graph grants its ordinary
    // current-process retention identity; it does not rewrite the old companion.
    let token = super::tests::token(121020);
    cold.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":request["scope"]})).unwrap();
    let next = "55555555-5555-4555-8555-555555555555";
    let fresh = cold
        .execute("agent.saveHistoryCheckpoint", json!({"token":token,"scope":request["scope"],"workspace":WORKSPACE,"checkpoint":next,"metadataBasis":true}))
        .unwrap();
    let token = super::tests::token(121021);
    cold.execute("agent.readProject", json!({"token":token,"scope":request["scope"],"projectPath":cold.path})).unwrap();
    cold.execute(
        "agent.validateRetainedHistory",
        json!({"token":token,"scope":request["scope"],"workspace":WORKSPACE,"checkpoint":next,"sha256":fresh["checkpoint"]["sha256"]}),
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), raw);
}
#[test]
fn cold_identity_origin_refuses_equal_by_value_replacements_and_changed_or_missing_origin_guards() {
    for mode in ["current_arc", "history_arc", "merge_key", "missing_origin", "foreign_origin"] {
        let (_dir, mut cold, request, _) = cold_identity_checkpoint();
        match mode {
            "current_arc" => cold.project = Arc::new((*cold.project).clone()),
            "history_arc" => cold.history.undo.last_mut().unwrap().1 = Arc::new((*cold.history.undo.last().unwrap().1).clone()),
            "merge_key" => cold.history.merge_key = Some("later-human-merge".into()),
            "missing_origin" => cold.agent_bridge.history_checkpoints.clear(),
            "foreign_origin" => {
                let (_other_dir, mut other, _, _) = cold_identity_checkpoint();
                std::mem::swap(&mut cold.agent_bridge.history_checkpoints, &mut other.agent_bridge.history_checkpoints);
            }
            _ => unreachable!(),
        }
        let project = cold.project.clone();
        let before = history(&cold);
        assert!(inspect_cold_checkpoint(&mut cold, &request, 121100).is_err(), "{mode}");
        assert!(Arc::ptr_eq(&cold.project, &project));
        assert_eq!(history(&cold), before);
    }
}
#[test]
fn cold_identity_origin_refuses_changed_companion_basis_graph_receipts_and_later_human_work() {
    for mode in ["revision", "epoch", "sequence", "label", "project", "receipt", "human"] {
        let (dir, mut cold, request, _) = cold_identity_checkpoint();
        let path = dir.0.join(format!(".poema-history-{}-{}.json", WORKSPACE, request["checkpoint"].as_str().unwrap()));
        let mut c: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mode {
            "revision" => c["revision"] = json!(c["revision"].as_u64().unwrap() + 1),
            "epoch" => c["epoch"] = json!("a".repeat(32)),
            "sequence" => c["sequence"] = Value::Null,
            "label" => c["undo"][0]["label"] = json!("unrelated label"),
            "project" => c["projects"][0] = json!("{}"),
            "receipt" => c["tickets"][0]["result"]["revision"] = json!(900000),
            "human" => {
                cold.execute("file.newBin", json!({"name":"Keep later human edit"})).unwrap();
            }
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&c).unwrap()).unwrap();
        let project = cold.project.clone();
        let before = history(&cold);
        assert!(inspect_cold_checkpoint(&mut cold, &request, 121200).is_err(), "{mode}");
        assert!(Arc::ptr_eq(&cold.project, &project));
        assert_eq!(history(&cold), before);
    }
}

#[test]
fn auxiliary_recovery_sequence_bundle_restores_the_same_passive_editor_payload() {
    let (_dir, mut source) = setup();
    super::native_auxiliary_tests::populate(&mut source);
    let expected = super::native_auxiliary::Snapshot::capture(&source).unwrap();
    let (_, _, staged) = staged(&mut source);
    let mut cold = Session::default();
    cold.open_workspace_update(&startup(&staged)).unwrap();
    assert!(cold.is_display_only());
    assert!(expected.matches(&cold).unwrap());
    assert!(!cold.worker_available);
}
