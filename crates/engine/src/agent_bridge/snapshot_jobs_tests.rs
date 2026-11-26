use super::*;
use crate::agent_bridge::{
    assembly_tests,
    tests::{exact_batch, token},
};
use filmcraft_project::MediaRef;
use std::{fs, path::PathBuf, sync::mpsc, time::Duration};

use super::TEST_SERIAL as SERIAL;
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, Scratch) {
    let (mut s, _) = assembly_tests::fixture();
    let dir = Scratch(std::env::temp_dir().join(format!(
        "filmcraft-snapshot-job-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    fs::create_dir(&dir.0).unwrap();
    let file = dir.0.join("original.bin");
    fs::write(&file, b"generated original bytes").unwrap();
    let id = s.project.items.values().find(|i| i.metadata.contains_key(super::super::BINDING_KEY)).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    (s, dir)
}
fn before(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal,"events":s.events.len()})
}
fn wait_until(mut f: impl FnMut() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while !f() {
        assert!(std::time::Instant::now() < end, "generated worker did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn ready(s: &Session, n: usize) {
    wait_until(|| s.agent_bridge.snapshot_jobs.iter().find(|j| j.token == token(n)).is_some_and(|j| !matches!(*j.state.lock().unwrap(), State::Running)));
}
fn workers_done() {
    wait_until(|| WORKERS.load(Ordering::SeqCst) == 0);
}
fn blocked(s: &mut Session, n: usize) -> mpsc::Sender<()> {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    start_with(s, token(n), Instant::now(), move |plan, check| {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        plan.capture(check)
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    release_tx
}
fn poll_job(s: &mut Session, n: usize) -> Result<Value> {
    s.execute("agent.snapshotStatus", json!({"job":token(n)}))
}
fn cancel_job(s: &mut Session, n: usize) -> Value {
    s.execute("agent.cancelSnapshot", json!({"job":token(n)})).unwrap()
}
fn split(s: &Session, n: usize) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    exact_batch(n, vec![json!({"command":"timeline.razor","params":{"clip":c.id.0,"time":c.start.0+s.sequence_rate().tick_of(1).0}})])
}

#[test]
fn background_read_has_a_separate_bounded_lifetime_without_extending_publication() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    let start = Instant::now() - Duration::from_secs(46);
    let running = start_with(&mut s, token(70100), start, |p, c| p.capture(c)).unwrap();
    ready(&s, 70100);
    let delivered = poll_job(&mut s, 70100).unwrap();
    assert_eq!(delivered["status"], "complete", "library reads must outlive the publication budget");
    let policy = json!({"version":1,"deadline_ms":300000,"publication_deadline_ms":45000});
    assert_eq!(running["verification_policy"], policy);
    assert_eq!(delivered["verification_policy"], policy);
    assert_eq!(delivered["result"]["capabilities"]["backgroundSnapshotDeadlineMs"], 300000);
    assert_eq!(delivered["result"]["capabilities"]["guardedOperationDeadlineMs"], 45000);
    assert!(Check::new(start).check().is_err(), "publication remains bounded to 45 seconds");
    assert_eq!(before(&s), unchanged);
    workers_done();
}

#[test]
fn large_snapshot_size_hint_extends_only_the_read_and_delivery_keeps_its_fixed_policy() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, dir) = fixture();
    let id = s.project.items.values().find(|i| i.metadata.contains_key(super::super::BINDING_KEY)).unwrap().id;
    // Deliberately overstate the generated file. A scheduling hint must never
    // become a byte witness or extend subsequent publication authority.
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().info.file_size = Some(super::super::media_guard::LARGE_SOURCE_BYTES);
    s.worker_available = true;
    s.agent_bridge.operations.enable_transport();
    let unchanged = before(&s);
    let start = Instant::now() - Duration::from_secs(301);
    let running = start_with(&mut s, token(70101), start, |p, c| p.capture(c)).unwrap();
    ready(&s, 70101);
    let delivered = poll_job(&mut s, 70101).unwrap();
    assert_eq!(delivered["status"], "complete");
    let policy = json!({"version":1,"deadline_ms":600000,"publication_deadline_ms":45000});
    assert_eq!(running["verification_policy"], policy);
    assert_eq!(delivered["verification_policy"], policy);
    assert_eq!(delivered["result"]["capabilities"]["backgroundSnapshotDeadlineMs"], 600000);
    assert_eq!(delivered["result"]["capabilities"]["largeSourceVerification"], false);
    let bytes = fs::metadata(dir.0.join("original.bin")).unwrap().len();
    assert_eq!(delivered["progress"]["completed_checks"], 2);
    // Precise identity may avoid content I/O; coarse identity hashes the tiny
    // real file twice. Neither path treats the size hint as observed bytes.
    assert!(delivered["progress"]["bytes_read"].as_str().unwrap().parse::<u64>().unwrap() <= bytes * 2);
    assert_eq!(s.agent_bridge.tickets.back().unwrap().media.source_bytes(), bytes);
    assert_eq!(before(&s), unchanged);
    workers_done();
}

#[test]
fn snapshot_deadlines_remain_bounded_for_ordinary_and_large_hints() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (n, seconds, hint) in [(70102, 301, 0), (70103, 601, u64::MAX)] {
        let (mut s, _dir) = fixture();
        let id = s.project.items.values().find(|i| i.metadata.contains_key(super::super::BINDING_KEY)).unwrap().id;
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().info.file_size = Some(hint);
        let unchanged = before(&s);
        start_with(&mut s, token(n), Instant::now() - Duration::from_secs(seconds), |p, c| p.capture(c)).unwrap();
        ready(&s, n);
        assert_eq!(poll_job(&mut s, n).unwrap()["status"], "cancelled");
        assert!(s.agent_bridge.tickets.is_empty());
        assert_eq!(before(&s), unchanged);
        workers_done();
    }
}

#[test]
fn background_start_and_poll_leave_the_engine_responsive_and_issue_no_ticket_until_delivery() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    let release = blocked(&mut s, 70001);
    assert!(!s.execute("command.list", json!({})).unwrap().as_array().unwrap().is_empty());
    let running = poll_job(&mut s, 70001).unwrap();
    assert_eq!(running["status"], "running");
    assert_eq!(running["progress"]["total_checks"], 2);
    assert!(s.agent_bridge.tickets.is_empty());
    assert_eq!(before(&s), unchanged);
    assert!(s.execute("agent.snapshot", json!({"token":token(70001)})).is_err());
    release.send(()).unwrap();
    ready(&s, 70001);
    assert!(s.agent_bridge.tickets.is_empty());
    let out = poll_job(&mut s, 70001).unwrap();
    assert_eq!(out["status"], "complete");
    assert_eq!(out["result"]["token"], token(70001));
    assert_eq!(out["result"]["capabilities"]["backgroundSnapshots"], true);
    assert_eq!(out["progress"]["completed_checks"], 2);
    assert_eq!(before(&s), unchanged);
    assert_eq!(poll_job(&mut s, 70001).unwrap()["status"], "delivered");
    s.execute("agent.apply", split(&s, 70001)).unwrap();
    workers_done();
}

#[test]
fn stop_while_a_worker_is_blocked_prevents_late_ticket_and_preserves_native_history() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    let release = blocked(&mut s, 70002);
    assert_eq!(cancel_job(&mut s, 70002)["ticketRevoked"], false);
    assert_eq!(poll_job(&mut s, 70002).unwrap()["status"], "cancelled");
    release.send(()).unwrap();
    workers_done();
    assert_eq!(poll_job(&mut s, 70002).unwrap()["status"], "cancelled");
    assert!(s.agent_bridge.tickets.is_empty());
    assert_eq!(before(&s), unchanged);
    assert!(s.execute("agent.snapshot", json!({"token":token(70002),"background":true})).is_err());
    assert!(s.execute("agent.apply", split(&s, 70002)).is_err());
}

#[test]
fn cancelling_after_worker_completion_or_delivery_cannot_leave_an_unused_edit_ticket() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    for (n, deliver) in [(70003, false), (70004, true)] {
        s.execute("agent.snapshot", json!({"token":token(n),"background":true})).unwrap();
        ready(&s, n);
        if deliver {
            assert_eq!(poll_job(&mut s, n).unwrap()["status"], "complete");
        }
        assert_eq!(cancel_job(&mut s, n)["ticketRevoked"], deliver);
        assert!(s.execute("agent.apply", split(&s, n)).is_err());
        assert_eq!(before(&s), unchanged);
    }
    workers_done();
}

#[test]
fn a_human_edit_context_replacement_or_authorization_change_refuses_worker_delivery_without_reverting_it() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in 0..4 {
        let (mut s, dir) = fixture();
        let release = blocked(&mut s, 70010 + mode);
        match mode {
            0 => {
                let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
                s.execute("timeline.razor", json!({"clip":c.id.0,"time":c.start.0+s.sequence_rate().tick_of(1).0})).unwrap();
            }
            1 => s.state.linked_selection = !s.state.linked_selection,
            2 => s.project = Arc::new((*s.project).clone()),
            _ => s.media.add_stored_paths([dir.0.join("original.bin").to_string_lossy().into_owned()], None),
        }
        let human = before(&s);
        release.send(()).unwrap();
        ready(&s, 70010 + mode);
        assert!(poll_job(&mut s, 70010 + mode).is_err());
        assert!(s.agent_bridge.tickets.is_empty());
        assert_eq!(before(&s), human);
        workers_done();
    }
}

#[test]
fn replacement_after_worker_completion_still_fails_the_existing_native_publication_guard() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, dir) = fixture();
    s.execute("agent.snapshot", json!({"token":token(70020),"background":true})).unwrap();
    ready(&s, 70020);
    fs::write(dir.0.join("original.bin"), b"replaced original bytes ").unwrap();
    assert_eq!(poll_job(&mut s, 70020).unwrap()["status"], "complete");
    let unchanged = before(&s);
    assert!(s.execute("agent.apply", split(&s, 70020)).is_err());
    assert_eq!(before(&s), unchanged);
    workers_done();
}

#[test]
fn worker_deadline_panic_and_unknown_cancel_never_issue_authority_or_restart_work() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    start_with(&mut s, token(70030), Instant::now() - Duration::from_secs(301), |p, c| p.capture(c)).unwrap();
    ready(&s, 70030);
    assert_eq!(poll_job(&mut s, 70030).unwrap()["status"], "cancelled");
    workers_done();
    start_with(&mut s, token(70031), Instant::now(), |_, _| panic!("generated worker failure")).unwrap();
    ready(&s, 70031);
    assert_eq!(poll_job(&mut s, 70031).unwrap()["status"], "failed");
    workers_done();
    cancel_job(&mut s, 70032);
    assert!(s.execute("agent.snapshot", json!({"token":token(70032),"background":true})).is_err());
    assert!(s.agent_bridge.tickets.is_empty());
    assert_eq!(before(&s), unchanged);
}

#[test]
fn cancelled_blocked_workers_keep_their_permits_and_cannot_create_unbounded_threads() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let a = blocked(&mut s, 70040);
    let b = blocked(&mut s, 70041);
    cancel_job(&mut s, 70040);
    cancel_job(&mut s, 70041);
    assert_eq!(WORKERS.load(Ordering::SeqCst), 2);
    assert!(s.execute("agent.snapshot", json!({"token":token(70042),"background":true})).is_err());
    a.send(()).unwrap();
    b.send(()).unwrap();
    workers_done();
    s.execute("agent.snapshot", json!({"token":token(70043),"background":true})).unwrap();
    ready(&s, 70043);
    cancel_job(&mut s, 70043);
    workers_done();
}

#[test]
fn cancellation_never_erases_an_already_committed_receipt_or_undoes_a_human_edit() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    s.execute("agent.snapshot", json!({"token":token(70050),"background":true})).unwrap();
    ready(&s, 70050);
    poll_job(&mut s, 70050).unwrap();
    s.execute("agent.apply", split(&s, 70050)).unwrap();
    let committed = before(&s);
    assert_eq!(cancel_job(&mut s, 70050)["ticketRevoked"], false);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(70050)})).unwrap()["status"], "committed");
    assert_eq!(before(&s), committed);
    workers_done();
}

#[test]
fn malformed_background_and_job_arguments_fail_before_work_or_ticket_changes() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _dir) = fixture();
    let unchanged = before(&s);
    for p in [
        json!(null),
        json!({"token":token(70060),"background":null}),
        json!({"token":token(70060),"background":"true"}),
        json!({"token":token(70060),"background":true,"path":"/not-admitted"}),
    ] {
        assert!(s.execute("agent.snapshot", p).is_err());
    }
    for command in ["agent.snapshotStatus", "agent.cancelSnapshot"] {
        for p in [json!({}), json!({"job":null}), json!({"job":"../foreign"}), json!({"job":token(70061),"path":"/not-admitted"})] {
            assert!(s.execute(command, p).is_err());
        }
    }
    assert!(s.execute("agent.snapshotStatus", json!({"job":token(70062)})).is_err());
    assert!(s.agent_bridge.snapshot_jobs.is_empty());
    assert!(s.agent_bridge.tickets.is_empty());
    assert_eq!(before(&s), unchanged);
    assert_eq!(WORKERS.load(Ordering::SeqCst), 0);
}
