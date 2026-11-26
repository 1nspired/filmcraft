use super::*;
use crate::{
    Session,
    autosave::{AutoSavePrefs, AutosaveConfig, session_reservation::ColdJournalReservation},
};
use serde_json::{Value, json};
use std::{cell::Cell, fs, os::unix::fs::DirBuilderExt, path::PathBuf, sync::atomic::AtomicU64, time::Duration};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(recovery: bool) -> (Scratch, Session) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("original-current-input-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    let scope = json!({"projectId":"p_checkpoint","workspaceId":"11111111-1111-4111-8111-111111111111"});
    s.execute("file.newProject", json!({"name":"Original FIFO current image","workspace":scope})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    let token = "a".repeat(64);
    let read = s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path})).unwrap();
    s.execute("agent.apply",json!({"token":token,"label":"Owned before original FIFO","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope,"beforeSha256":read["binState"]["sha256"],"edit":{"type":"create","name":"Owned bin","parent":null}}}]})).unwrap();
    s.execute("file.newBin", json!({"name":"Human before original FIFO"})).unwrap();
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_991);
    let data = root.join("native");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let mut cfg = AutosaveConfig::new(data);
    cfg.journal_debounce = Duration::from_secs(600);
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    let mut prefs = AutoSavePrefs::default();
    prefs.recovery_journal = recovery;
    s.persistence = Some(Persistence::start_cold_reserved(&cfg, prefs, reservation, &|| Ok(())).unwrap());
    // The normal capture supplies the actual epoch and original COW. With no
    // active writer its original auxiliary capture must be complete as well.
    crate::agent_bridge::capture_native_journal(&mut s).unwrap().unwrap();
    (Scratch(root), s)
}
fn capture(s: &mut Session) -> crate::agent_bridge::NativeJournalPending {
    crate::agent_bridge::capture_native_journal(s).unwrap().unwrap()
}
fn release(s: &Session) -> CurrentImageRelease {
    s.persistence.as_ref().unwrap().release_current_image(|_| Ok(()), || Ok(())).unwrap()
}
fn stop(s: &mut Session) {
    s.persistence.as_mut().unwrap().stop_worker();
}
fn no_files(s: &Session) {
    let p = &s.persistence.as_ref().unwrap().session_dir;
    for n in [crate::autosave::SNAPSHOT, crate::autosave::META, crate::agent_bridge::NATIVE_JOURNAL_FILE] {
        assert!(!p.join(n).exists());
    }
}

#[test]
fn checked_current_input_uses_original_fifo_and_complete_owned_human_history_and_auxiliary() {
    let (_d, mut s) = fixture(true);
    let native = capture(&mut s);
    let guard = crate::project_save::guard(&s).unwrap();
    let before = fs::read(s.path.as_ref().unwrap()).unwrap();
    let p = s.persistence.as_ref().unwrap();
    let ack = release(&s);
    let checks = Cell::new(0);
    let input = p
        .enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || {
            checks.set(checks.get() + 1);
            guard.current(&s).map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(checks.get(), 3);
    assert!(p.last_sent.is_none());
    p.assert_original_current_image_input(&ack, &input, &s.project, s.revision, s.path.as_deref()).unwrap();
    let flush = p.flush_current_image_pending_journal(&ack, || guard.current(&s).map_err(|e| e.to_string())).unwrap();
    assert_eq!(flush.written_revision(), Some(s.revision));
    let snapshot = fs::read(p.session_dir.join(crate::autosave::SNAPSHOT)).unwrap();
    assert_eq!(snapshot, filmcraft_format::encode(&s.project, false));
    let meta: Value = serde_json::from_slice(&fs::read(p.session_dir.join(crate::autosave::META)).unwrap()).unwrap();
    assert_eq!(meta["revision"], s.revision);
    assert_eq!(meta["projectPath"], json!(s.path));
    let native: Value = serde_json::from_slice(&fs::read(p.session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE)).unwrap()).unwrap();
    let payload = &native["payload"];
    assert_eq!(payload["history"]["revision"], s.revision);
    assert_eq!(payload["history"]["undo"].as_array().unwrap().len(), 2);
    assert_eq!(payload["history"]["tickets"].as_array().unwrap().len(), 1);
    assert!(payload["editor"]["auxiliary"].is_object());
    assert_eq!(payload["editor"]["editor"]["source_playhead"], serde_json::to_value(s.state.source_playhead).unwrap());
    assert_eq!(payload["project_file_sha256"], crate::saved_file::sha256(&snapshot));
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), before);
    guard.current(&s).unwrap();
    stop(&mut s);
}

#[test]
fn checked_current_input_refuses_equal_bytes_new_arc_wrong_revision_path_or_other_native_capture_before_send() {
    for kind in 0..4 {
        let (_d, mut s) = fixture(true);
        let (_other, mut other) = fixture(true);
        let native = if kind == 3 { capture(&mut other) } else { capture(&mut s) };
        let mut graph = s.project.clone();
        let mut revision = s.revision;
        let mut path = s.path.clone();
        match kind {
            0 => graph = Arc::new((*graph).clone()),
            1 => revision += 1,
            2 => path = Some("/generated/foreign.fcproj".into()),
            _ => {}
        }
        let ack = release(&s);
        let p = s.persistence.as_ref().unwrap();
        assert!(p.enqueue_current_image_input(&ack, graph, revision, path, native, || Ok(())).is_err());
        assert!(!ack.input_attempted.load(Ordering::Acquire));
        no_files(&s);
        stop(&mut other);
        stop(&mut s);
    }
}

#[test]
fn checked_current_input_refuses_incomplete_auxiliary_capture_without_burning_the_unused_send() {
    let (_d, mut s) = fixture(true);
    s.jobs.push(crate::Job {
        id: 999,
        label: "Generated active writer".into(),
        progress: Arc::new(filmcraft_export::Progress::default()),
        result: Arc::new(std::sync::Mutex::new(None)),
    });
    let native = capture(&mut s);
    s.jobs.clear();
    let ack = release(&s);
    let p = s.persistence.as_ref().unwrap();
    assert!(p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).is_err());
    assert!(!ack.input_attempted.load(Ordering::Acquire));
    no_files(&s);
    stop(&mut s);
}

#[test]
fn checked_current_input_source_refusal_before_send_keeps_unused_exchange_but_late_refusal_keeps_unknown_fifo_outcome() {
    for late in [false, true] {
        let (_d, mut s) = fixture(true);
        let native = capture(&mut s);
        let ack = release(&s);
        let checks = Cell::new(0);
        let p = s.persistence.as_ref().unwrap();
        assert!(
            p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || {
                checks.set(checks.get() + 1);
                if !late || checks.get() == 3 { Err("controlled actual source refusal".into()) } else { Ok(()) }
            })
            .is_err()
        );
        assert_eq!(ack.input_attempted.load(Ordering::Acquire), late);
        if late {
            // Observe the actual already-enqueued generated effect through the
            // original barrier. This supplies no returned input receipt.
            p.flush();
            assert!(p.session_dir.join(crate::autosave::SNAPSHOT).is_file());
        } else {
            no_files(&s);
        }
        let fresh = capture(&mut s);
        let p = s.persistence.as_ref().unwrap();
        let retry = p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), fresh, || Ok(()));
        assert_eq!(retry.is_ok(), !late);
        stop(&mut s);
    }
}

#[test]
fn checked_current_input_cannot_replay_or_cross_release_worker_cow_or_stopped_owner_receipts() {
    let (_d, mut s) = fixture(true);
    let native = capture(&mut s);
    let second = capture(&mut s);
    let ack = release(&s);
    let p = s.persistence.as_ref().unwrap();
    let input = p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).unwrap();
    assert!(p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), second, || Ok(())).unwrap_err().contains("no replay"));
    assert!(p.assert_original_current_image_input(&ack, &input, &Arc::new((*s.project).clone()), s.revision, s.path.as_deref()).is_err());
    assert!(p.assert_original_current_image_input(&ack, &input, &s.project, s.revision + 1, s.path.as_deref()).is_err());
    assert!(p.assert_original_current_image_input(&ack, &input, &s.project, s.revision, Some("/generated/foreign.fcproj")).is_err());
    let (_other, mut other) = fixture(true);
    let other_ack = release(&other);
    let other_p = other.persistence.as_ref().unwrap();
    assert!(other_p.assert_original_current_image_input(&ack, &input, &s.project, s.revision, s.path.as_deref()).is_err());
    assert!(p.assert_original_current_image_input(&other_ack, &input, &s.project, s.revision, s.path.as_deref()).is_err());
    stop(&mut other);
    stop(&mut s);
    assert!(s.persistence.as_ref().unwrap().assert_original_current_image_input(&ack, &input, &s.project, s.revision, s.path.as_deref()).is_err());
}

#[test]
fn checked_current_input_reports_closed_fifo_send_without_returning_or_replaying_a_receipt() {
    let (_d, mut s) = fixture(true);
    let native = capture(&mut s);
    let ack = release(&s);
    let (closed, receiver) = std::sync::mpsc::channel();
    drop(receiver);
    // Keep the original live worker's real sender retained while injecting an
    // actually closed channel at the send boundary. Restore it for Stop/join.
    let p = s.persistence.as_mut().unwrap();
    let original = std::mem::replace(&mut p.tx, closed);
    let error = p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).unwrap_err();
    assert!(error.contains("before current-image input enqueue; no replay"));
    assert!(ack.input_attempted.load(Ordering::Acquire));
    p.tx = original;
    no_files(&s);
    let fresh = capture(&mut s);
    assert!(
        s.persistence
            .as_ref()
            .unwrap()
            .enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), fresh, || Ok(()))
            .unwrap_err()
            .contains("no replay")
    );
    no_files(&s);
    stop(&mut s);
}

#[test]
fn checked_current_input_preserves_disabled_recovery_and_cannot_interpret_enqueue_or_none_flush_as_completed_image() {
    let (_d, mut s) = fixture(false);
    let native = capture(&mut s);
    let ack = release(&s);
    let p = s.persistence.as_ref().unwrap();
    let input = p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).unwrap();
    let flush = p.flush_current_image_pending_journal(&ack, || Ok(())).unwrap();
    assert_eq!(flush.written_revision(), None);
    p.assert_original_current_image_input(&ack, &input, &s.project, s.revision, s.path.as_deref()).unwrap();
    no_files(&s);
    stop(&mut s);
}

#[test]
fn checked_current_input_cannot_follow_an_already_attempted_checked_flush_or_ordinary_sync() {
    for ordinary in [false, true] {
        let (_d, mut s) = fixture(true);
        let native = capture(&mut s);
        let ack = release(&s);
        if ordinary {
            s.sync_persistence();
        } else {
            s.persistence.as_ref().unwrap().flush_current_image_pending_journal(&ack, || Ok(())).unwrap();
        }
        let p = s.persistence.as_ref().unwrap();
        assert!(p.enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || Ok(())).is_err());
        assert!(!ack.input_attempted.load(Ordering::Acquire));
        stop(&mut s);
    }
}
