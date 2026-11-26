use super::super::{
    assembly_tests,
    snapshot_jobs::{TEST_SERIAL, worker_count},
    tests::{read, token},
};
use super::*;
use std::{fs, path::PathBuf, sync::mpsc};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, Scratch) {
    let (mut s, _) = assembly_tests::fixture();
    let root = Scratch(std::env::temp_dir().join(format!(
        "filmcraft-read-job-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    fs::create_dir(&root.0).unwrap();
    let file = root.0.join("original.bin");
    fs::write(&file, b"original file witness").unwrap();
    let id = s.project.items.values().find(|i| i.metadata.contains_key(super::super::BINDING_KEY)).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: file.to_string_lossy().into_owned() };
    (s, root)
}
fn wait(mut f: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(3);
    while !f() {
        assert!(Instant::now() < limit);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn settled(s: &Session, id: &str) {
    wait(|| s.agent_bridge.read_jobs.iter().find(|j| j.id == id).is_some_and(|j| !matches!(*j.state.lock().unwrap(), State::Running)));
}
fn workers_done() {
    wait(|| worker_count() == 0);
}
fn before(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn request(n: usize) -> Value {
    json!({"token":token(n),"job":token(n+1)})
}
fn apply_request(s: &Session, n: usize) -> Value {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    let batch =
        super::super::tests::exact_batch(n, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]);
    json!({"job":token(n+1),"batch":batch})
}
struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
fn blocked_apply(s: &mut Session, request: &Value) -> Release {
    let (entered, entry) = mpsc::channel();
    let (release, held) = mpsc::channel();
    prepare_apply_with(s, request, move |plan, expected, check| {
        entered.send(()).unwrap();
        held.recv().unwrap();
        plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), check)
    })
    .unwrap();
    entry.recv_timeout(Duration::from_secs(2)).unwrap();
    Release(Some(release))
}
fn delivered_apply(s: &mut Session, request: &Value) -> Value {
    prepare_apply(s, request).unwrap();
    let id = request["job"].as_str().unwrap();
    settled(s, id);
    let result = poll(s, &json!({"job":id})).unwrap();
    assert_eq!(result["status"], "complete");
    result
}
#[test]
fn apply_preparation_leaves_engine_available_then_publishes_one_exact_native_step() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 30000);
    let request = apply_request(&s, 30000);
    let initial = before(&s);
    let tickets = s.agent_bridge.tickets.len();
    let release = blocked_apply(&mut s, &request);
    assert_eq!(poll(&mut s, &json!({"job":request["job"]})).unwrap()["kind"], "native_apply_preparation");
    assert!(s.execute("sequence.inspect", json!({})).is_ok());
    assert_eq!(before(&s), initial);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    drop(release);
    settled(&s, request["job"].as_str().unwrap());
    let ready = poll(&mut s, &json!({"job":request["job"]})).unwrap();
    assert_eq!(ready["result"]["status"], "prepared");
    assert_eq!(ready["result"]["writeAuthority"], false);
    assert_eq!(ready["result"]["requiresFinalValidation"], true);
    let result = s.execute("agent.apply", request["batch"].clone()).unwrap();
    assert_eq!(result["status"], "committed");
    assert_eq!(s.revision, initial["revision"].as_u64().unwrap() + 1);
    let changed = before(&s);
    assert_eq!(s.execute("agent.apply", request["batch"].clone()).unwrap(), result);
    assert_eq!(before(&s), changed);
    let historical = prepare_apply(&mut s, &json!({"job":token(30002),"batch":request["batch"]})).unwrap();
    assert_eq!(historical["result"]["historical"], true);
    assert_eq!(before(&s), changed);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), initial["project"]);
    workers_done();
}
#[test]
fn apply_preparation_does_not_replace_final_file_guard_or_allow_ordinary_proof_substitution() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for ordinary in [false, true] {
        let (mut s, root) = fixture();
        read(&mut s, 30100);
        let request = apply_request(&s, 30100);
        if ordinary {
            let p = super::tests::request(30100);
            let r: ValidateRead = parse(COMMAND, &p).unwrap();
            start(&mut s, &p, &r).unwrap();
            settled(&s, &token(30101));
            poll(&mut s, &json!({"job":token(30101)})).unwrap();
        } else {
            delivered_apply(&mut s, &request);
        }
        let initial = before(&s);
        let file = root.0.join("original.bin");
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        fs::write(&file, b"replaced file witness").unwrap();
        fs::File::options().write(true).open(&file).unwrap().set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
        assert!(s.execute("agent.apply", request["batch"].clone()).is_err(), "ordinary={ordinary}");
        assert_eq!(before(&s), initial);
        workers_done();
    }
}
#[test]
fn apply_preparation_refuses_wrong_batch_expiry_eviction_and_changed_editor_basis_without_fallback() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["batch", "expired", "evicted", "context", "cow", "path"] {
        let (mut s, _root) = fixture();
        read(&mut s, 30200);
        let mut request = apply_request(&s, 30200);
        delivered_apply(&mut s, &request);
        match mode {
            "batch" => request["batch"]["label"] = json!("another requested edit"),
            "expired" => {
                *s.agent_bridge.read_jobs.back().unwrap().state.lock().unwrap() = State::Delivered(Instant::now() - DELIVERY - Duration::from_millis(1))
            }
            "evicted" => s.agent_bridge.read_jobs.clear(),
            "context" => s.state.linked_selection = !s.state.linked_selection,
            "cow" => s.project = Arc::new((*s.project).clone()),
            _ => s.path = Some("/generated/another-project.fcproj".into()),
        }
        let initial = before(&s);
        assert!(s.execute("agent.apply", request["batch"].clone()).is_err(), "{mode}");
        assert_eq!(before(&s), initial);
        if mode == "evicted" {
            assert!(prepare_apply(&mut s, &request).is_err(), "an evicted preparation is not restarted");
        }
        workers_done();
    }
}
#[test]
fn stopped_apply_preparation_retains_ticket_fence_and_worker_permit_until_exit() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for before_start in [false, true] {
        let (mut s, _root) = fixture();
        read(&mut s, 30300);
        let request = apply_request(&s, 30300);
        let initial = before(&s);
        let release = if before_start {
            cancel(&mut s, &json!({"job":request["job"]})).unwrap();
            assert_eq!(prepare_apply(&mut s, &request).unwrap()["status"], "cancelled");
            None
        } else {
            let release = blocked_apply(&mut s, &request);
            cancel(&mut s, &json!({"job":request["job"]})).unwrap();
            assert_eq!(worker_count(), 1);
            Some(release)
        };
        assert!(s.execute("agent.apply", request["batch"].clone()).is_err());
        assert_eq!(before(&s), initial);
        let mut another = request.clone();
        another["job"] = json!(token(30302));
        assert!(prepare_apply(&mut s, &another).is_err());
        drop(release);
        workers_done();
    }
}
#[test]
fn failed_and_consumed_apply_preparations_cannot_restart_or_replay_staging() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for failed in [false, true] {
        let (mut s, _root) = fixture();
        read(&mut s, 30400);
        let mut request = apply_request(&s, 30400);
        if failed {
            prepare_apply_with(&mut s, &request, |_, _, _| Err(error("generated complete-file verification failure"))).unwrap();
            settled(&s, &token(30401));
            assert_eq!(poll(&mut s, &json!({"job":token(30401)})).unwrap()["status"], "failed");
        } else {
            request["batch"]["operations"][0]["params"]["clip"] = json!(999999);
            delivered_apply(&mut s, &request);
        }
        let initial = before(&s);
        assert!(s.execute("agent.apply", request["batch"].clone()).is_err());
        assert!(s.execute("agent.apply", request["batch"].clone()).is_err());
        assert_eq!(before(&s), initial);
        workers_done();
    }
}
#[test]
fn hostile_apply_preparation_requests_refuse_before_workers_or_project_changes() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["job", "alias", "extra", "batch-extra", "empty", "operation", "unknown-token"] {
        let (mut s, _root) = fixture();
        read(&mut s, 30500);
        let mut p = apply_request(&s, 30500);
        match mode {
            "job" => p["job"] = Value::Null,
            "alias" => p["job"] = p["batch"]["token"].clone(),
            "extra" => p["publish"] = json!(true),
            "batch-extra" => p["batch"]["force"] = json!(true),
            "empty" => p["batch"]["operations"] = json!([]),
            "operation" => p["batch"]["operations"][0]["command"] = json!("file.save"),
            _ => p["batch"]["token"] = json!(token(99999)),
        }
        let initial = before(&s);
        assert!(prepare_apply(&mut s, &p).is_err(), "{mode}");
        assert_eq!(before(&s), initial);
        assert_eq!(worker_count(), 0);
    }
}
fn blocked(s: &mut Session, n: usize) -> mpsc::Sender<()> {
    let (entered, entry) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let p = request(n);
    let r: ValidateRead = parse(COMMAND, &p).unwrap();
    start_with(s, &p, &r, move |plan, expected, check| {
        entered.send(()).unwrap();
        held.recv().unwrap();
        plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), check)
    })
    .unwrap();
    entry.recv_timeout(Duration::from_secs(2)).unwrap();
    release
}
#[test]
fn read_validation_workers_leave_engine_available_and_never_mint_history_or_tickets() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 28000);
    let initial = before(&s);
    let tickets = s.agent_bridge.tickets.len();
    let release = blocked(&mut s, 28000);
    assert_eq!(poll(&mut s, &json!({"job":token(28001)})).unwrap()["status"], "running");
    assert!(s.execute("sequence.inspect", json!({})).is_ok());
    assert_eq!(before(&s), initial);
    release.send(()).unwrap();
    settled(&s, &token(28001));
    let out = poll(&mut s, &json!({"job":token(28001)})).unwrap();
    assert_eq!(out["status"], "complete");
    assert_eq!(out["result"]["token"], token(28000));
    assert_eq!(out["progress"]["completed_checks"], 1);
    assert_eq!(out["progress"]["total_checks"], 1);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    assert_eq!(before(&s), initial);
    assert_eq!(poll(&mut s, &json!({"job":token(28001)})).unwrap()["status"], "delivered");
    workers_done();
}
#[test]
fn replaced_originals_fail_complete_worker_verification_without_changing_the_cut() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, root) = fixture();
    read(&mut s, 28100);
    let initial = before(&s);
    let release = blocked(&mut s, 28100);
    fs::write(root.0.join("original.bin"), b"replaced file witness").unwrap();
    release.send(()).unwrap();
    settled(&s, &token(28101));
    let out = poll(&mut s, &json!({"job":token(28101)})).unwrap();
    assert_eq!(out["status"], "failed");
    assert!(out.get("result").is_none());
    assert_eq!(before(&s), initial);
    workers_done();
}
#[test]
fn read_validation_refuses_cow_path_and_ticket_changes_at_delivery() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["cow", "path", "ticket"] {
        let (mut s, _root) = fixture();
        read(&mut s, 28200);
        let release = blocked(&mut s, 28200);
        match mode {
            "cow" => s.project = Arc::new((*s.project).clone()),
            "path" => s.path = Some("/different/generated.fcproj".into()),
            _ => s.agent_bridge.tickets.clear(),
        };
        let initial = before(&s);
        assert!(poll(&mut s, &json!({"job":token(28201)})).is_err());
        assert_eq!(before(&s), initial);
        release.send(()).unwrap();
        workers_done();
    }
}
#[test]
fn read_validation_cancellation_is_tombstoned_and_does_not_revoke_the_original_ticket() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 28300);
    let p = request(28300);
    let release = blocked(&mut s, 28300);
    assert_eq!(cancel(&mut s, &json!({"job":token(28301)})).unwrap()["ticketRevoked"], false);
    assert!(super::super::validate_read_basis(&s, &token(28300)).is_ok());
    assert_eq!(start(&mut s, &p, &parse(COMMAND, &p).unwrap()).unwrap()["status"], "cancelled");
    release.send(()).unwrap();
    workers_done();
    cancel(&mut s, &json!({"job":token(28303)})).unwrap();
    let p = json!({"token":token(28300),"job":token(28303)});
    assert_eq!(start(&mut s, &p, &parse(COMMAND, &p).unwrap()).unwrap()["status"], "cancelled");
    assert_eq!(worker_count(), 0);
}
#[test]
fn read_validation_shares_snapshot_worker_bounds_even_after_cancellation() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 28400);
    let first = blocked(&mut s, 28400);
    let p = json!({"token":token(28400),"job":token(28403)});
    let (entered, entry) = mpsc::channel();
    let (second, held) = mpsc::channel();
    start_with(&mut s, &p, &parse(COMMAND, &p).unwrap(), move |_, _, _| {
        entered.send(()).unwrap();
        held.recv().unwrap();
        Ok(())
    })
    .unwrap();
    entry.recv().unwrap();
    cancel(&mut s, &json!({"job":token(28401)})).unwrap();
    cancel(&mut s, &json!({"job":token(28403)})).unwrap();
    let p = json!({"token":token(28400),"job":token(28405)});
    assert!(start(&mut s, &p, &parse(COMMAND, &p).unwrap()).is_err());
    assert!(s.execute("agent.snapshot", json!({"token":token(28406),"background":true})).is_err());
    first.send(()).unwrap();
    second.send(()).unwrap();
    workers_done();
}
#[test]
fn completed_verification_expires_instead_of_becoming_a_reusable_old_proof() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 28500);
    let p = request(28500);
    start(&mut s, &p, &parse(COMMAND, &p).unwrap()).unwrap();
    settled(&s, &token(28501));
    workers_done();
    let j = s.agent_bridge.read_jobs.back().unwrap();
    *j.state.lock().unwrap() = State::Done(Ok(()), Instant::now() - Duration::from_secs(1));
    let out = poll(&mut s, &json!({"job":token(28501)})).unwrap();
    assert_eq!(out["status"], "expired");
    assert!(out.get("result").is_none());
}
#[test]
fn postflight_requires_the_same_completed_inspection_ticket_and_view() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // This test also uses the bounded evidence-worker pool. Share its test
    // lock so a blocked-decoder limit test cannot consume both permits here.
    let _inspection = super::super::inspection::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _) = assembly_tests::fixture();
    read(&mut s, 28600);
    let sid = s.state.active_sequence.unwrap().0;
    s.execute(
        "agent.inspect",
        json!({"token":token(28600),"job":token(28602),"scope":{"projectId":"p_test","anchorSequence":sid},"request":{"kind":"frame","frame":0,"maxSide":64}}),
    )
    .unwrap();
    wait(|| s.execute("agent.inspection", json!({"job":token(28602)})).unwrap()["status"] == "complete");
    let p = json!({"token":token(28600),"job":token(28601),"inspection":token(28602)});
    start(&mut s, &p, &parse(COMMAND, &p).unwrap()).unwrap();
    settled(&s, &token(28601));
    assert_eq!(poll(&mut s, &json!({"job":token(28601)})).unwrap()["result"]["inspection"], token(28602));
    let p = json!({"token":token(28600),"job":token(28603),"inspection":token(28602)});
    start(&mut s, &p, &parse(COMMAND, &p).unwrap()).unwrap();
    settled(&s, &token(28603));
    s.execute("agent.cancelInspection", json!({"job":token(28602)})).unwrap();
    assert!(poll(&mut s, &json!({"job":token(28603)})).is_err());
    workers_done();
}
#[test]
fn invalid_jobs_requests_and_worker_panics_cannot_return_current_evidence() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _root) = fixture();
    read(&mut s, 28700);
    let initial = before(&s);
    for p in [
        json!({"token":token(28700),"job":null}),
        json!({"token":token(28700),"job":false}),
        json!({"token":token(28700),"job":token(28701),"inspection":null}),
        json!({"token":token(28700),"job":token(28701),"candidate":"unknown"}),
        json!({"token":token(28700),"job":token(28701),"extra":true}),
    ] {
        assert!(s.execute(COMMAND, p).is_err());
    }
    let p = request(28700);
    start_with(&mut s, &p, &parse(COMMAND, &p).unwrap(), |_, _, _| panic!("generated verifier failure")).unwrap();
    settled(&s, &token(28701));
    assert_eq!(poll(&mut s, &json!({"job":token(28701)})).unwrap()["status"], "failed");
    assert_eq!(before(&s), initial);
    workers_done();
}
