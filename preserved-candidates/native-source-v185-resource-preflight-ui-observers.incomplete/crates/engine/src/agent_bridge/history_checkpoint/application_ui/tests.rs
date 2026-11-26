use super::*;
use crate::{ApplicationSnapshotCapture, History};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
const W: &str = "6cd868fc-9e1c-485b-87aa-9c60e13b00bf";
const C: &str = "901c432f-8c98-4f7e-806a-e53fc4745ee1";
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"passive-ui-engine-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn setup() -> (Root, Session) {
    let root = std::env::temp_dir().join(format!("native-passive-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("generated-root.txt"), b"passive-ui-engine-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Passive UI","workspace":{"projectId":"p_passive_ui","workspaceId":W}})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    s.history = History { limit: 128, ..History::default() };
    s.agent_operation_control().enable_transport();
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":format!("{:064x}",1),"scope":{"projectId":"p_passive_ui","workspaceId":W}})).unwrap();
    (Root(root), s)
}
fn proof() -> Value {
    let data = serde_json::to_string(&json!({"kind":"generated-engine-only-passive-ui","version":1})).unwrap();
    json!({"kind":"native_passive_application_ui_proof","version":1,"bytes":data.len(),"sha256":digest(data.as_bytes()),"data":data})
}
fn request() -> super::super::Request {
    serde_json::from_value(json!({"token":format!("{:064x}",2),"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"checkpoint":C})).unwrap()
}
#[test]
fn original_capture_attachment_keeps_ui_on_same_physical_session_and_pool() {
    let (_root, mut s) = setup();
    let p = proof();
    let capture = ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, p.clone()).unwrap();
    capture.install(&mut s).unwrap();
    assert_eq!(s.saved_application_ui().unwrap(), Some(p));
    let (pool, _, _) = super::super::graph(&s).unwrap();
    assert!(Arc::ptr_eq(&pool[0], &s.project));
    let c = super::super::pending_capsule(&s, &request(), None).unwrap().encode().unwrap();
    assert_eq!(c.version, 10);
    assert!(c.application.is_empty());
    assert!(c.application_ui.is_some());
}
#[test]
fn readonly_decode_cannot_turn_equal_byte_distinct_project_into_current_ui() {
    let (_root, s) = setup();
    let record = Record::capture(&s, proof()).unwrap();
    let mut pool = vec![s.project.clone()];
    record.add_to_pool(&mut pool).unwrap();
    let saved = record.encode(&pool).unwrap();
    let cold = Arc::new((*s.project).clone());
    let decoded = Record::decode(&saved, &[cold.clone()], &mut Default::default()).unwrap();
    assert!(!decoded.current(&s).unwrap());
    assert!(Arc::ptr_eq(&decoded.project, &cold));
}
#[test]
fn later_native_work_keeps_old_ui_data_but_refuses_current_adoption() {
    let (_root, mut s) = setup();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let old = s.agent_bridge.application_ui.as_ref().unwrap().clone();
    s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    assert!(s.saved_application_ui().is_err());
    assert!(!old.current(&s).unwrap());
    let c = super::super::pending_capsule(&s, &request(), None).unwrap().encode().unwrap();
    assert_eq!(c.version, 10);
    assert!(c.projects.len() > 1);
    assert!(c.application_ui.is_some());
}
#[test]
fn malformed_or_oversized_passive_proof_refuses_without_native_publication() {
    let (_root, s) = setup();
    let project = s.project.clone();
    let original = proof();
    for field in ["kind", "version", "bytes", "sha256", "data", "extra"] {
        let mut p = original.clone();
        p[field] = match field {
            "version" | "bytes" => json!(0),
            "extra" => json!(true),
            _ => json!("wrong"),
        };
        assert!(Record::capture(&s, p).is_err());
    }
    let mut p = original;
    p["data"] = json!("x".repeat(UI_BYTES + 1));
    assert!(Record::capture(&s, p).is_err());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert!(s.agent_bridge.application_ui.is_none());
}
#[test]
fn capsule10_data_cannot_be_downgraded_or_omitted_during_restore() {
    let (_root, mut s) = setup();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let c = super::super::pending_capsule(&s, &request(), None).unwrap().encode().unwrap();
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.saved_revision = cold.revision;
    for version in [1, 2, 3, 4, 5, 6, 7, 8, 9, 11] {
        let mut bad = c.clone();
        bad.version = version;
        assert!(super::super::prepare_restore(&cold, &request(), &bad, "", None).is_err());
    }
    let mut bad = c.clone();
    bad.application_ui = None;
    assert!(super::super::prepare_restore(&cold, &request(), &bad, "", None).is_err());
}
#[test]
fn original_prepared_history_restores_readonly_ui_data_without_live_grants() {
    let (_root, mut s) = setup();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let c = super::super::pending_capsule(&s, &request(), None).unwrap().encode().unwrap();
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.saved_revision = cold.revision;
    let ready = super::super::prepare_restore(&cold, &request(), &c, "", None).unwrap();
    assert!(ready.application_ui.is_some());
    assert!(ready.tickets.is_empty());
    assert!(ready.application.is_empty());
    assert!(cold.agent_bridge.application_ui.is_none());
    assert!(cold.agent_bridge.tickets.is_empty());
    assert!(!cold.worker_available);
}

#[test]
fn real_paused_session_can_read_bound_passive_data_without_releasing_edit_fence() {
    let (_root, mut s) = setup();
    let expected = proof();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, expected.clone()).unwrap().install(&mut s).unwrap();
    let project = s.project.clone();
    let scope = serde_json::from_value(json!({"projectId":"p_passive_ui","workspaceId":W})).unwrap();
    crate::agent_bridge::workspace_update::hold_scoped_candidate(
        &mut s,
        scope,
        W.into(),
        "22222222-2222-4222-8222-222222222222".into(),
        format!("{:064x}", 200),
        "a".repeat(64),
    )
    .unwrap();
    assert!(s.is_display_only());
    let before = s.workspace_update_status();
    assert_eq!(s.saved_application_ui().unwrap(), Some(expected));
    assert!(s.execute("file.newBin", json!({"name":"Must remain held"})).is_err());
    assert!(ApplicationSnapshotCapture::capture(&s, &[]).is_err());
    assert_eq!(s.workspace_update_status(), before);
    assert!(Arc::ptr_eq(&project, &s.project));
}
#[test]
fn display_projection_cannot_adopt_even_pointer_identical_passive_data() {
    let (_root, mut s) = setup();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let project = s.project.clone();
    s.view_only = true;
    assert!(s.saved_application_ui().is_err());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert!(ApplicationSnapshotCapture::capture(&s, &[]).is_err());
}

const SECOND: &str = "44444444-4444-4444-8444-444444444444";
fn basis_for(s: &mut Session, token: u64) {
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":format!("{token:064x}"),"scope":{"projectId":"p_passive_ui","workspaceId":W}})).unwrap();
}
fn worker_checkpoint(s: &mut Session, token: u64, checkpoint: &str, issue_basis: bool, malformed: bool) -> Result<Value> {
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(s).unwrap();
    if issue_basis {
        basis_for(s, token);
    }
    let capture = ApplicationSnapshotCapture::capture(s, &[]).unwrap().with_application_ui(s, proof()).unwrap();
    let mut params = json!({"token":format!("{token:064x}"),"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"checkpoint":checkpoint,"metadataBasis":true});
    if malformed {
        params["unrecognized"] = json!(true);
    }
    let mut pending = worker.start_with_application_capture(s, "agent.saveHistoryCheckpoint", params, None, capture).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(done) = pending.try_finish() {
            *s = done.session;
            assert!(!done.reply.uncertain);
            return done.reply.result;
        }
        assert!(std::time::Instant::now() < until, "bounded original worker reply");
        std::thread::yield_now();
    }
}
fn generated_wav() -> Vec<u8> {
    let samples = 480_u32;
    let size = samples * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + size).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16_u32.to_le_bytes());
    b.extend_from_slice(&1_u16.to_le_bytes());
    b.extend_from_slice(&1_u16.to_le_bytes());
    b.extend_from_slice(&48000_u32.to_le_bytes());
    b.extend_from_slice(&96000_u32.to_le_bytes());
    b.extend_from_slice(&2_u16.to_le_bytes());
    b.extend_from_slice(&16_u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&size.to_le_bytes());
    b.resize(44 + size as usize, 0);
    b
}
#[test]
fn same_worker_refresh_after_history_eviction_keeps_current_ui_and_exact_media_origins() {
    let (root, mut s) = setup();
    let mut cfg = crate::autosave::AutosaveConfig::new(root.0.join("generated-state"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    s.start_autosave(cfg).unwrap();
    s.history.limit = 1;
    let sound = root.0.join("generated-silence.wav");
    std::fs::write(&sound, generated_wav()).unwrap();
    let imported = s.execute("file.import", json!({"paths":[sound]})).unwrap();
    let item = imported["items"][0].as_u64().unwrap();
    worker_checkpoint(&mut s, 101, C, true, false).unwrap();
    assert!(s.saved_application_ui().unwrap().is_some());
    let first = s.agent_bridge.application_ui.as_ref().unwrap().clone();
    assert!(serde_json::to_value(first.native.auxiliary.as_ref().unwrap()).unwrap()["origins"].as_object().unwrap().contains_key(sound.to_str().unwrap()));
    s.execute("project.delete", json!({"items":[item]})).unwrap();
    s.execute("file.newBin", json!({"name":"Evict the imported project"})).unwrap();
    assert!(s.saved_application_ui().is_err());
    let project = s.project.clone();
    let history = s.history.clone();
    let result = worker_checkpoint(&mut s, 102, SECOND, true, false).unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    assert!(crate::trim::command_settlement::historical::same_history(&history, &s.history));
    assert_eq!(s.saved_application_ui().unwrap(), Some(proof()));
    assert!(s.agent_bridge.pending_application_capture.is_none());
    let current = s.agent_bridge.application_ui.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&first, current));
    assert!(!serde_json::to_value(current.native.auxiliary.as_ref().unwrap()).unwrap()["origins"].as_object().unwrap().contains_key(sound.to_str().unwrap()));
    assert_eq!(
        serde_json::to_value(current.native.auxiliary.as_ref().unwrap()).unwrap()["origins"],
        serde_json::to_value(crate::agent_bridge::native_auxiliary::Snapshot::capture(&s).unwrap()).unwrap()["origins"]
    );
    let raw = std::fs::read(root.0.join(format!(".poema-history-{W}-{SECOND}.json"))).unwrap();
    let saved: Value = serde_json::from_slice(&raw).unwrap();
    basis_for(&mut s, 103);
    s.execute("agent.beginWorkspaceUpdate", json!({"token":format!("{:064x}",103),"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"update":"22222222-2222-4222-8222-222222222222"})).unwrap();
    basis_for(&mut s, 104);
    s.execute("agent.stageProjectRecovery", json!({"token":format!("{:064x}",104),"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"update":"22222222-2222-4222-8222-222222222222","pauseToken":format!("{:064x}",103),"checkpoint":SECOND,"checkpointSha256":result["checkpoint"]["sha256"]})).unwrap();
    let staged: Value =
        serde_json::from_slice(&std::fs::read(root.0.join(format!(".poema-project-recovery-{W}-22222222-2222-4222-8222-222222222222.json"))).unwrap()).unwrap();
    assert_eq!(saved["application_ui"]["native"]["auxiliary"]["origins"], staged["payload"]["editor"]["auxiliary"]["origins"]);
    s.shutdown();
}
#[test]
fn physical_attachment_never_refreshes_stale_project_context_or_history_basis() {
    for change in ["project", "context", "history"] {
        let (root, mut s) = setup();
        ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
        let previous = s.agent_bridge.application_ui.as_ref().unwrap().clone();
        basis_for(&mut s, 101);
        match change {
            "project" => {
                s.execute("file.newBin", json!({"name":"Later human edit"})).unwrap();
            }
            "context" => s.state.source_playhead = filmcraft_time::Tick(17),
            "history" => s.history.limit = 1,
            _ => unreachable!(),
        }
        assert!(worker_checkpoint(&mut s, 101, SECOND, false, false).is_err());
        assert!(Arc::ptr_eq(&previous, s.agent_bridge.application_ui.as_ref().unwrap()));
        assert!(s.agent_bridge.pending_application_capture.is_none());
        assert!(!root.0.join(format!(".poema-history-{W}-{SECOND}.json")).exists());
    }
}
#[test]
fn malformed_original_save_discards_unconsumed_attachment_without_replacing_prior_ui() {
    let (root, mut s) = setup();
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let previous = s.agent_bridge.application_ui.as_ref().unwrap().clone();
    assert!(worker_checkpoint(&mut s, 101, C, true, true).is_err());
    assert!(Arc::ptr_eq(&previous, s.agent_bridge.application_ui.as_ref().unwrap()));
    assert!(s.agent_bridge.pending_application_capture.is_none());
    assert!(!root.0.join(format!(".poema-history-{W}-{C}.json")).exists());
    worker_checkpoint(&mut s, 102, SECOND, true, false).unwrap();
    assert!(s.saved_application_ui().unwrap().is_some());
    assert!(s.agent_bridge.pending_application_capture.is_none());
}

const PAUSE: &str = "22222222-2222-4222-8222-222222222222";
fn pause_on_worker(s: &mut Session, token: u64, issue_basis: bool, malformed: bool) -> Result<Value> {
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(s).unwrap();
    if issue_basis {
        basis_for(s, token);
    }
    let capture = ApplicationSnapshotCapture::capture(s, &[]).unwrap().with_application_ui(s, proof()).unwrap();
    let mut params = json!({"token":format!("{token:064x}"),"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"update":PAUSE});
    if malformed {
        params["unrecognized"] = json!(true);
    }
    let mut pending = worker.start_with_application_capture(s, "agent.beginWorkspaceUpdate", params, None, capture).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(done) = pending.try_finish() {
            *s = done.session;
            assert!(!done.reply.uncertain);
            return done.reply.result;
        }
        assert!(std::time::Instant::now() < until, "bounded original pause worker");
        std::thread::yield_now();
    }
}
fn start_original_persistence(root: &Root, s: &mut Session) {
    let mut cfg = crate::autosave::AutosaveConfig::new(root.0.join("original-state"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    s.start_autosave(cfg).unwrap();
}
fn checked_pause_status(s: &mut Session) -> Value {
    s.execute("agent.workspaceUpdateStatus", json!({"scope":{"projectId":"p_passive_ui","workspaceId":W},"workspace":W,"update":PAUSE})).unwrap()
}
#[test]
fn same_original_pause_captures_current_clean_ui_after_human_save_without_edit_or_stale_graph() {
    let (root, mut s) = setup();
    start_original_persistence(&root, &mut s);
    worker_checkpoint(&mut s, 201, C, true, false).unwrap();
    let old = s.agent_bridge.application_ui.as_ref().unwrap().clone();
    s.history.limit = 1;
    s.execute("file.newBin", json!({"name":"First human change"})).unwrap();
    s.execute("file.newBin", json!({"name":"Evict earlier UI-only root"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.is_dirty());
    assert!(s.saved_application_ui().is_err());
    let saved = std::fs::read(s.path.as_ref().unwrap()).unwrap();
    let project = s.project.clone();
    let history = s.history.clone();
    let revision = s.revision;
    let result = pause_on_worker(&mut s, 202, true, false).unwrap();
    assert_eq!(result["status"], "paused");
    assert_eq!(checked_pause_status(&mut s)["status"], "paused");
    assert!(s.is_display_only());
    assert!(!s.is_dirty());
    assert_eq!(s.saved_application_ui().unwrap(), Some(proof()));
    assert!(!Arc::ptr_eq(&old, s.agent_bridge.application_ui.as_ref().unwrap()));
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    assert!(crate::trim::command_settlement::historical::same_history(&history, &s.history));
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert!(s.agent_bridge.pending_application_capture.is_none());
    assert_eq!(s.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
    assert!(!s.is_display_only());
    assert!(!s.is_dirty());
    assert_eq!(s.persistence.as_ref().unwrap().auto_save_now().unwrap(), None);
    s.shutdown();
}
#[test]
fn original_pause_settles_actual_saved_project_notification_before_binding_current_ui() {
    let (root, mut s) = setup();
    start_original_persistence(&root, &mut s);
    s.prefs.auto_save.save_current_project = true;
    s.persistence.as_ref().unwrap().set_prefs(s.prefs.auto_save.clone());
    s.execute("file.newBin", json!({"name":"Actual autosave notification"})).unwrap();
    let before_saved = s.saved_revision;
    assert!(before_saved < s.revision);
    assert!(s.persistence.as_ref().unwrap().auto_save_now().unwrap().is_some());
    assert_eq!(s.saved_revision, before_saved, "notification must still await original Session delivery");
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), filmcraft_format::encode(&s.project, false));
    let project = s.project.clone();
    let revision = s.revision;
    let history = s.history.clone();
    pause_on_worker(&mut s, 203, true, false).unwrap();
    assert_eq!(s.saved_revision, s.revision);
    assert!(!s.is_dirty());
    assert_eq!(s.agent_bridge.application_ui.as_ref().unwrap().native.saved_revision, s.saved_revision);
    assert_eq!(s.saved_application_ui().unwrap(), Some(proof()));
    assert_eq!(checked_pause_status(&mut s)["status"], "paused");
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    assert!(crate::trim::command_settlement::historical::same_history(&history, &s.history));
    assert_eq!(s.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
    s.shutdown();
}
#[test]
fn failed_original_pause_refuses_stale_or_malformed_capture_and_leaves_persistence_usable() {
    for malformed in [false, true] {
        let (root, mut s) = setup();
        start_original_persistence(&root, &mut s);
        ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
        let previous = s.agent_bridge.application_ui.as_ref().unwrap().clone();
        s.execute("file.newBin", json!({"name":"Dirty source"})).unwrap();
        basis_for(&mut s, 204);
        if !malformed {
            s.state.source_playhead = filmcraft_time::Tick(17);
        }
        assert!(pause_on_worker(&mut s, 204, false, malformed).is_err());
        assert!(s.workspace_update_status().is_none());
        assert!(!s.is_display_only());
        assert!(Arc::ptr_eq(&previous, s.agent_bridge.application_ui.as_ref().unwrap()));
        assert!(s.agent_bridge.pending_application_capture.is_none());
        assert!(s.persistence.as_ref().unwrap().auto_save_now().unwrap().is_some());
        // The finite attachment still cannot enter an arbitrary agent command.
        let worker = crate::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut s).unwrap();
        let cap = ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap();
        assert!(worker.start_with_application_capture(&mut s, "agent.apply", json!({}), None, cap).is_err());
        s.shutdown();
    }
}
#[test]
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn original_gated_worker_encodes_clean_ui_and_history_without_ordinary_dirty_or_clean_input() {
    let (root, mut s) = setup();
    s.execute("file.newBin", json!({"name":"Human history retained by Save"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.is_dirty());
    assert!(!s.history.undo.is_empty());
    let original = std::fs::read(s.path.as_ref().unwrap()).unwrap();
    use std::os::unix::fs::DirBuilderExt;
    let data = root.0.join("gated-current-state");
    std::fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let cfg = crate::autosave::AutosaveConfig::new(data);
    let reservation = crate::autosave::session_reservation::ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    s.persistence = Some(crate::autosave::Persistence::start_cold_reserved(&cfg, s.prefs.auto_save.clone(), reservation, &|| Ok(())).unwrap());
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().with_application_ui(&s, proof()).unwrap().install(&mut s).unwrap();
    let native = super::super::journal::capture_retained(&s).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let encoded = worker.encode_held_current_checkpoint(s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).unwrap();
    let files = encoded.files(worker).unwrap();
    let payload: Value = serde_json::from_slice(files[2].1).unwrap();
    assert_eq!(payload["payload"]["history"]["application_ui"]["proof"], proof());
    assert_eq!(payload["payload"]["editor"]["saved_revision"], s.revision);
    assert_eq!(payload["payload"]["history"]["undo"].as_array().unwrap().len(), s.history.undo.len());
    super::super::journal::decode(files[2].1, files[0].1).unwrap();
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    assert_eq!(std::fs::read_dir(&worker.session_dir).unwrap().count(), 1);
    for name in [crate::autosave::SNAPSHOT, crate::autosave::META, crate::agent_bridge::NATIVE_JOURNAL_FILE] {
        assert!(!worker.session_dir.join(name).exists());
    }
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), original);
    assert!(!s.is_dirty());
    assert_eq!(s.saved_application_ui().unwrap(), Some(proof()));
    s.persistence.as_mut().unwrap().stop_worker();
}
