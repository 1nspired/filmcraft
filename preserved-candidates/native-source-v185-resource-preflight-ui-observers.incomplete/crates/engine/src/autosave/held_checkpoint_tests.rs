use super::*;
use crate::{
    Session,
    autosave::{AutoSavePrefs, AutosaveConfig, Sent, session_reservation::ColdJournalReservation},
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    sync::{Mutex, atomic::AtomicU64},
};
const W: &str = "11111111-1111-4111-8111-111111111111";
pub(super) struct Scratch(pub(super) PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(super) fn fixture(dirty: bool) -> (Scratch, Session) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = crate::temp_dir().join(format!("held-checkpoint-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Held original checkpoint","workspace":{"projectId":"p_checkpoint","workspaceId":W}})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    if dirty {
        let worker = crate::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut s).unwrap();
        s.agent_operation_control().enable_transport();
        let token = "a".repeat(64);
        let scope = json!({"projectId":"p_checkpoint","workspaceId":W});
        let read = s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path})).unwrap();
        s.execute("agent.apply",json!({"token":token,"label":"Owned before hold","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope,"beforeSha256":read["binState"]["sha256"],"edit":{"type":"create","name":"Owned bin","parent":null}}}]})).unwrap();
        s.execute("file.newBin", json!({"name":"Human before hold"})).unwrap();
    }
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_991);
    let data = root.join("native");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let mut cfg = AutosaveConfig::new(data);
    cfg.journal_debounce = Duration::from_secs(600);
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    s.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    (Scratch(root), s)
}
pub(super) fn args(s: &mut Session) -> (Arc<Project>, u64, Option<String>, crate::agent_bridge::NativeJournalPending) {
    let native = crate::agent_bridge::capture_native_journal(s).unwrap().unwrap();
    (s.project.clone(), s.revision, s.path.clone(), native)
}
pub(super) fn encode(s: &mut Session) -> Encoded {
    let (g, r, p, n) = args(s);
    s.persistence.as_ref().unwrap().encode_held_current_checkpoint(g, r, p, n, || Ok(())).unwrap()
}
pub(super) fn no_checkpoint_files(worker: &Persistence) {
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    assert_eq!(fs::read_dir(&worker.session_dir).unwrap().count(), 1);
    for name in [crate::autosave::SNAPSHOT, crate::autosave::META, crate::agent_bridge::NATIVE_JOURNAL_FILE] {
        assert!(!worker.session_dir.join(name).exists());
    }
}
pub(super) fn stop(s: &mut Session) {
    s.persistence.as_mut().unwrap().stop_worker();
}
#[test]
fn held_checkpoint_encodes_original_complete_owned_and_human_cow_on_the_same_actual_worker_without_writes() {
    use std::cell::Cell;
    let (_d, mut s) = fixture(true);
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let (g, r, path, native) = args(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let caller = std::thread::current().id();
    let checks = Cell::new(0);
    let encoded = worker
        .encode_held_current_checkpoint(g, r, path, native, || {
            assert_eq!(std::thread::current().id(), caller);
            checks.set(checks.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_ne!(encoded.thread, caller);
    assert_eq!(encoded.thread, worker.thread.as_ref().unwrap().thread().id());
    assert!(checks.get() >= 4);
    let files = encoded.files(worker).unwrap();
    assert_eq!(files[0].1, filmcraft_format::encode(&s.project, false));
    let meta: Value = serde_json::from_slice(files[1].1).unwrap();
    assert_eq!(meta["pid"], std::process::id());
    assert_eq!(meta["revision"], s.revision);
    assert_eq!(meta["projectPath"], json!(s.path));
    assert_eq!(meta["bytes"], files[0].1.len());
    let payload: Value = serde_json::from_slice(files[2].1).unwrap();
    let p = &payload["payload"];
    assert_eq!(p["history"]["revision"], s.revision);
    assert_eq!(p["history"]["undo"].as_array().unwrap().len(), 2);
    assert_eq!(p["history"]["tickets"].as_array().unwrap().len(), 1);
    assert!(p["editor"]["auxiliary"].is_object());
    assert_eq!(p["saved_file"]["path"], json!(s.path));
    let description = encoded.description(worker).unwrap();
    assert_eq!(description["sameOriginalWorker"], true);
    assert_eq!(description["serialized"], true);
    assert_eq!(description["filesWritten"], false);
    no_checkpoint_files(worker);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    stop(&mut s);
}
#[test]
fn held_checkpoint_keeps_clean_saved_state_complete_editor_and_original_metadata_without_ordinary_clean_or_dirty() {
    let (_d, mut s) = fixture(false);
    assert_eq!(s.saved_revision, s.revision);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let payload: Value = serde_json::from_slice(&encoded.native).unwrap();
    assert_eq!(payload["payload"]["editor"]["saved_revision"], s.saved_revision);
    assert_eq!(payload["payload"]["editor"]["editor"]["source_playhead"], serde_json::to_value(filmcraft_time::Tick(9_007_199_254_740_991)).unwrap());
    assert!(payload["payload"]["editor"]["auxiliary"].is_object());
    let meta: super::super::JournalMeta = serde_json::from_slice(&encoded.metadata).unwrap();
    assert_eq!(meta.pid, std::process::id());
    assert_eq!(meta.revision, s.revision);
    assert_eq!(meta.project_path, s.path);
    assert!(!meta.clean_exit);
    no_checkpoint_files(worker);
    stop(&mut s);
}
#[test]
fn held_checkpoint_encoded_data_cannot_cross_another_actual_worker_or_remain_current_after_original_release_or_stop() {
    let (_d, mut first) = fixture(true);
    let encoded = encode(&mut first);
    let (_other, mut second) = fixture(false);
    assert!(encoded.current(second.persistence.as_ref().unwrap()).is_err());
    stop(&mut second);
    let worker = first.persistence.as_ref().unwrap();
    let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    worker.assert_current_image_release(&release).unwrap();
    assert!(encoded.current(worker).is_err());
    stop(&mut first);
    assert!(encoded.current(first.persistence.as_ref().unwrap()).is_err());
}
#[test]
fn held_checkpoint_wrong_actual_graph_revision_path_or_complete_auxiliary_refuses_without_files_or_release() {
    for kind in 0..4 {
        let (_d, mut s) = fixture(true);
        if kind == 3 {
            s.jobs.push(crate::Job {
                id: 999,
                label: "Generated live writer".into(),
                progress: Arc::new(filmcraft_export::Progress::default()),
                result: Arc::new(Mutex::new(None)),
            });
        }
        let (mut graph, mut revision, mut path, native) = args(&mut s);
        s.jobs.clear();
        match kind {
            0 => graph = Arc::new((*graph).clone()),
            1 => revision += 1,
            2 => path = Some("/generated/foreign.fcproj".into()),
            _ => {}
        }
        let worker = s.persistence.as_ref().unwrap();
        assert!(worker.encode_held_current_checkpoint(graph, revision, path, native, || Ok(())).is_err());
        assert!(worker.startup_checkpoint_attempted.load(Ordering::Acquire));
        assert!(!worker.startup_checkpoint_ready.load(Ordering::Acquire));
        assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
        assert!(!worker.startup_attempted.load(Ordering::Acquire));
        no_checkpoint_files(worker);
        stop(&mut s);
    }
}
#[test]
fn held_checkpoint_original_native_capture_cannot_be_serialized_by_a_different_workers_directory() {
    let (_a, mut first) = fixture(true);
    let (g, r, p, n) = args(&mut first);
    let (_b, mut second) = fixture(true);
    let worker = second.persistence.as_ref().unwrap();
    assert!(worker.encode_held_current_checkpoint(g, r, p, n, || Ok(())).is_err());
    no_checkpoint_files(worker);
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    stop(&mut second);
    stop(&mut first);
}
#[test]
fn held_checkpoint_initial_source_refusal_sends_no_request_and_keeps_the_original_gate_unattempted() {
    let (_d, mut s) = fixture(true);
    let (g, r, p, n) = args(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    assert!(worker.encode_held_current_checkpoint(g, r, p, n, || Err("controlled initial source loss".into())).is_err());
    assert!(!worker.startup_checkpoint_attempted.load(Ordering::Acquire));
    no_checkpoint_files(worker);
    stop(&mut s);
}
#[test]
fn held_checkpoint_source_loss_after_send_retains_unknown_preparation_and_cannot_replay_or_release() {
    use std::cell::Cell;
    let (_d, mut s) = fixture(true);
    let (g, r, p, n) = args(&mut s);
    let calls = Cell::new(0);
    let worker = s.persistence.as_ref().unwrap();
    assert!(
        worker
            .encode_held_current_checkpoint(g, r, p, n, || {
                calls.set(calls.get() + 1);
                if calls.get() >= 3 { Err("controlled loss after enqueue".into()) } else { Ok(()) }
            })
            .is_err()
    );
    assert!(worker.startup_checkpoint_attempted.load(Ordering::Acquire));
    assert!(!worker.startup_checkpoint_ready.load(Ordering::Acquire));
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    let (g, r, p, n) = args(&mut s);
    assert!(s.persistence.as_ref().unwrap().encode_held_current_checkpoint(g, r, p, n, || Ok(())).err().unwrap().contains("already attempted"));
    no_checkpoint_files(s.persistence.as_ref().unwrap());
    stop(&mut s);
}
#[test]
fn held_checkpoint_success_is_once_only_and_never_replaces_the_original_prepared_bytes() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let bytes = encoded.native.clone();
    let (g, r, p, n) = args(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    assert!(worker.encode_held_current_checkpoint(g, r, p, n, || Ok(())).err().unwrap().contains("already attempted"));
    encoded.current(worker).unwrap();
    assert_eq!(encoded.native, bytes);
    no_checkpoint_files(worker);
    stop(&mut s);
}
#[test]
fn held_checkpoint_keeps_the_original_ordinary_fifo_unconsumed_until_its_existing_resume_exchange() {
    let (_d, mut s) = fixture(true);
    let worker = s.persistence.as_ref().unwrap();
    let mut first = Project::default();
    first.name = "Queued first".into();
    let mut second = Project::default();
    second.name = "Queued second".into();
    worker.send_dirty_native(Arc::new(first), 41, None, Ok(None));
    worker.send_clean();
    worker.send_dirty_native(Arc::new(second), 42, None, Ok(None));
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    no_checkpoint_files(worker);
    let prepared = filmcraft_format::decode(&encoded.snapshot).unwrap();
    assert_eq!(prepared.project.name, s.project.name);
    let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    let flush = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
    assert_eq!(flush.written_revision(), Some(42));
    assert_eq!(filmcraft_format::decode(&fs::read(worker.session_dir.join(crate::autosave::SNAPSHOT)).unwrap()).unwrap().project.name, "Queued second");
    assert!(encoded.current(worker).is_err());
    stop(&mut s);
}
#[test]
fn held_checkpoint_refuses_ordinary_stopped_and_already_queued_session_owners_before_attempting_serialization() {
    for kind in 0..3 {
        let (d, mut s) = fixture(true);
        if kind == 0 {
            stop(&mut s);
            s.persistence.take();
            let path = d.0.join("ordinary");
            fs::create_dir(&path).unwrap();
            s.persistence = Some(Persistence::start(&AutosaveConfig::new(path), AutoSavePrefs::default()).unwrap());
        }
        let (g, r, p, n) = args(&mut s);
        if kind == 1 {
            stop(&mut s);
        } else if kind == 2 {
            s.persistence.as_mut().unwrap().last_sent = Some(Sent { revision: s.revision, saved_revision: s.saved_revision, path: s.path.clone() });
        }
        let worker = s.persistence.as_ref().unwrap();
        assert!(worker.encode_held_current_checkpoint(g, r, p, n, || Ok(())).is_err());
        assert!(!worker.startup_checkpoint_attempted.load(Ordering::Acquire));
        stop(&mut s);
    }
}
