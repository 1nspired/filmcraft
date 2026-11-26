use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::Condvar,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn scope() -> Value {
    json!({"projectId":"p_save_worker","workspaceId":"2dca7108-935c-42c7-a048-e11caa03915c"})
}
fn token(n: usize) -> String {
    format!("save-worker-{n:032}")
}
fn fixture(worker: &SessionWorker, n: usize, kind: &str) -> (Session, Value, PathBuf) {
    fixture_before_read(worker, n, kind, |_, _| {})
}
fn fixture_before_read(worker: &SessionWorker, n: usize, kind: &str, before_read: impl FnOnce(&mut Session, &std::path::Path)) -> (Session, Value, PathBuf) {
    let root = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir).join(format!(
        "save-worker-{}-{n}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Save worker","workspace":scope()})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    s.start_autosave(crate::autosave::AutosaveConfig::new(root.join("runtime"))).unwrap();
    s.sync_persistence();
    s.persistence.as_ref().unwrap().flush();
    s.apply_persistence_events();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    before_read(&mut s, &root);
    let doc = s.execute("agent.readProject", json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    let intent = if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) };
    let job = token(n + 10000);
    s.execute(
        "agent.startProjectSavePlan",
        json!({"job":job,"query":{"token":token(n),"scope":scope(),"beforeProjectSha256":doc["projectState"]["sha256"],"intent":intent}}),
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let r = s.execute("agent.projectSavePlanStatus", json!({"job":job})).unwrap();
        if r["status"] == "delivered" {
            break;
        }
        assert_eq!(r["status"], "running");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    (s, json!({"job":job,"operation":token(n+20000)}), root)
}
fn finish(p: &mut Pending) -> Completion {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(c) = p.try_finish() {
            return c;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn cancel(worker: &SessionWorker, s: &mut Session, p: &Value) {
    let mut work = worker.start(s, "agent.cancelProjectSaveOperation", p.clone(), None).unwrap();
    let done = finish(&mut work);
    *s = done.session;
    assert_eq!(done.reply.result.unwrap()["status"], "cancelled");
    assert!(s.pending_save_operation_request().is_none());
}
#[test]
fn original_save_worker_moves_one_real_Session_and_key_across_all_four_intents_without_edit_tokens() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, root) = fixture(&worker, 240000 + i, kind);
        let project = s.project.clone();
        let bytes = fs::read(root.join("Original.fcproj")).unwrap();
        let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
        assert!(s.is_display_only());
        assert!(s.pending_save_operation_request().is_none());
        assert!(s.execute("file.save", json!({})).is_err());
        let done = finish(&mut work);
        s = done.session;
        assert_eq!(done.reply.result.unwrap()["status"], "held");
        assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
        assert!(Arc::ptr_eq(&s.project, &project));
        let mut status = worker.start(&mut s, "agent.projectSaveOperationStatus", p.clone(), None).unwrap();
        let done = finish(&mut status);
        s = done.session;
        assert_eq!(done.reply.result.unwrap()["pending"]["phase"], "Prepared");
        cancel(&worker, &mut s, &p);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
        assert!(!root.join("Chosen.fcproj").exists());
        s.shutdown();
    }
}
#[test]
fn independent_save_wait_stop_before_execution_cannot_mint_a_key_or_claim_native_cancellation() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        tx.send(()).unwrap();
        let mut open = held.0.lock().unwrap();
        while !*open {
            open = held.1.wait(open).unwrap();
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, root) = fixture(&worker, 240010, "save_as");
    let control = s.agent_operation_control();
    let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let wrong = json!({"job":p["job"],"operation":token(290010)});
    assert_eq!(control.cancel_save_wait(&wrong).unwrap()["waitStopAccepted"], false);
    let result = control.cancel_save_wait(&p).unwrap();
    assert_eq!(result["waitStopAccepted"], true);
    assert_eq!(result["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut work);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert!(s.pending_save_operation_request().is_none());
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(control.cancel_save_wait(&p).unwrap()["waitStopAccepted"], false);
    s.execute("agent.cancelProjectSaveOperation", p).unwrap();
    s.shutdown();
}
#[test]
fn independent_save_wait_stop_after_begin_preserves_actual_native_key_until_exact_cancellation() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        let result = s.execute(c, p);
        if c == "agent.beginProjectSaveOperation" {
            tx.send(()).unwrap();
            let mut open = held.0.lock().unwrap();
            while !*open {
                open = held.1.wait(open).unwrap();
            }
        }
        result
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 240020, "save_copy");
    let control = s.agent_operation_control();
    let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.cancel_save_wait(&p).unwrap()["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut work);
    s = done.session;
    assert_eq!(done.reply.result.unwrap()["status"], "held");
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn original_save_worker_panic_after_begin_returns_same_native_owner_and_clears_only_task_wait() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::spawn(|s, c, p| {
        let result = s.execute(c, p);
        if c == "agent.beginProjectSaveOperation" {
            panic!("generated worker failure after native begin");
        }
        result
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 240030, "save");
    let control = s.agent_operation_control();
    let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    let done = finish(&mut work);
    s = done.session;
    assert!(done.reply.uncertain);
    assert!(done.reply.result.is_err());
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert_eq!(control.cancel_save_wait(&p).unwrap()["waitStopAccepted"], false);
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn save_wait_transport_refuses_malformed_requests_without_changing_an_existing_native_owner() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, _) = fixture(&worker, 240040, "save");
    for bad in
        [Value::Null, json!({}), json!({"job":"bad","operation":p["operation"]}), json!({"job":p["job"],"operation":p["operation"],"nativeComplete":true})]
    {
        assert!(s.agent_operation_control().cancel_save_wait(&bad).is_err());
        assert!(worker.start(&mut s, "agent.beginProjectSaveOperation", bad, None).is_err());
        assert!(!s.is_display_only());
    }
    s.execute("agent.cancelProjectSaveOperation", p).unwrap();
    s.shutdown();
}

#[test]
fn dropped_queued_save_task_clears_only_its_original_wait_registry_entry() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let is_first = first.clone();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if is_first.swap(false, Ordering::SeqCst) {
            tx.send(()).unwrap();
            let mut open = held.0.lock().unwrap();
            while !*open {
                open = held.1.wait(open).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut one, p1, _) = fixture(&worker, 240050, "save");
    let mut active = worker.start(&mut one, "agent.beginProjectSaveOperation", p1.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (mut two, p2, _) = fixture(&worker, 240051, "save");
    let registry = two.agent_operation_control();
    let queued = worker.start(&mut two, "agent.beginProjectSaveOperation", p2.clone(), None).unwrap();
    drop(queued);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut active);
    one = done.session;
    let end = Instant::now() + Duration::from_secs(3);
    while registry.cancel_save_wait(&p2).unwrap()["waitStopAccepted"] == true {
        assert!(Instant::now() < end, "dropped queued task left a stale wait");
        std::thread::sleep(Duration::from_millis(1));
    }
    if one.pending_save_operation_request().is_some() {
        cancel(&worker, &mut one, &p1);
    }
    one.shutdown();
}

#[test]
fn original_Save_worker_prepares_complete_pending_recovery_on_same_Session_without_another_owner_or_resume() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture(&worker, 340000, "save_copy");
    let project = s.project.clone();
    let before = fs::read(root.join("Original.fcproj")).unwrap();
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let mut read = worker.start(&mut s, "agent.prepareProjectSaveRecovery", p.clone(), None).unwrap();
    assert!(s.is_display_only());
    let done = finish(&mut read);
    s = done.session;
    let reply = done.reply.result.unwrap();
    assert_eq!(reply["kind"], "native_Save_recovery_read");
    assert_eq!(reply["phase"], "Prepared");
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), before);
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn independent_Stop_of_queued_Save_recovery_read_keeps_original_native_key_and_requires_separate_cancellation_ack() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if c == "agent.prepareProjectSaveRecovery" {
            tx.send(()).unwrap();
            let mut ready = held.0.lock().unwrap();
            while !*ready {
                ready = held.1.wait(ready).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 340010, "save");
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let control = s.agent_operation_control();
    let mut read = worker.start(&mut s, "agent.prepareProjectSaveRecovery", p.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let stopped = control.cancel_save_wait(&p).unwrap();
    assert_eq!(stopped["nativeOperationCancelled"], false);
    assert_eq!(stopped["waitStopAccepted"], true);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut read);
    assert!(done.reply.result.is_err());
    s = done.session;
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}

#[test]
fn actual_Save_worker_prepares_and_reads_full_physical_intent_on_the_same_original_Session_without_file_writes() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture(&worker, 740000, "save_copy");
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let mut prepare = worker.start(&mut s, "agent.prepareProjectSaveRecovery", p.clone(), None).unwrap();
    let done = finish(&mut prepare);
    s = done.session;
    let frame = done.reply.result.unwrap();
    let request = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576});
    let mut record = worker.start(&mut s, "agent.prepareProjectSaveRecoveryRecord", request, None).unwrap();
    let done = finish(&mut record);
    s = done.session;
    let first = done.reply.result.unwrap();
    let mut read = worker
        .start(&mut s, "agent.readProjectSaveRecoveryRecord", json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"]}), None)
        .unwrap();
    let done = finish(&mut read);
    s = done.session;
    assert_eq!(done.reply.result.unwrap(), first);
    assert_eq!(first["record"]["receipt"]["call_attempted"], false);
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn original_independent_Stop_of_queued_physical_intent_preparation_never_cancels_native_Save_or_publishes_a_file() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if c == "agent.prepareProjectSaveRecoveryRecord" {
            tx.send(()).unwrap();
            let mut ready = held.0.lock().unwrap();
            while !*ready {
                ready = held.1.wait(ready).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 740010, "save");
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let mut frame = worker.start(&mut s, "agent.prepareProjectSaveRecovery", p.clone(), None).unwrap();
    let done = finish(&mut frame);
    s = done.session;
    let frame = done.reply.result.unwrap();
    let request = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576});
    let control = s.agent_operation_control();
    let mut record = worker.start(&mut s, "agent.prepareProjectSaveRecoveryRecord", request, None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.cancel_save_wait(&p).unwrap()["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut record);
    assert!(done.reply.result.is_err());
    s = done.session;
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}

#[test]
fn original_Save_worker_publishes_full_private_record_without_native_completion_or_worker_resume() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, _) = fixture(&worker, 940000, "save_all");
    let mut run = |s: &mut Session, c: &str, v: Value| {
        let mut task = worker.start(s, c, v, None).unwrap();
        let done = finish(&mut task);
        *s = done.session;
        done.reply.result.unwrap()
    };
    run(&mut s, "agent.beginProjectSaveOperation", p.clone());
    let frame = run(&mut s, "agent.prepareProjectSaveRecovery", p.clone());
    let ready = run(
        &mut s,
        "agent.prepareProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576}),
    );
    let published = run(
        &mut s,
        "agent.publishProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"intent":ready["record"]["receipt"]["intent"]}),
    );
    assert_eq!(published["record"]["matches_published"], true);
    assert_eq!(published["nativeComplete"], false);
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn independent_Stop_of_queued_original_record_publication_preserves_original_key_and_absent_output() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if c == "agent.publishProjectSaveRecoveryRecord" {
            tx.send(()).unwrap();
            let mut open = held.0.lock().unwrap();
            while !*open {
                open = held.1.wait(open).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 940010, "save");
    let run = |s: &mut Session, c: &str, v: Value| {
        let mut task = worker.start(s, c, v, None).unwrap();
        let done = finish(&mut task);
        *s = done.session;
        done.reply.result.unwrap()
    };
    run(&mut s, "agent.beginProjectSaveOperation", p.clone());
    let frame = run(&mut s, "agent.prepareProjectSaveRecovery", p.clone());
    let ready = run(
        &mut s,
        "agent.prepareProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576}),
    );
    let i = &ready["record"]["receipt"]["intent"];
    let control = s.agent_operation_control();
    let mut task = worker
        .start(&mut s, "agent.publishProjectSaveRecoveryRecord", json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"intent":i}), None)
        .unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.cancel_save_wait(&p).unwrap()["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut task);
    assert!(done.reply.result.is_err());
    s = done.session;
    assert!(!PathBuf::from(i["destination"].as_str().unwrap()).exists());
    assert!(!PathBuf::from(i["candidate"].as_str().unwrap()).exists());
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    cancel(&worker, &mut s, &p);
    s.shutdown();
}

#[test]
fn complete_effect_read_moves_same_original_Session_through_worker_and_retains_both_pauses() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture(&worker, 420000, "save_copy");
    let before = fs::read(root.join("Original.fcproj")).unwrap();
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let mut work = worker.start(&mut s, "agent.readProjectSaveEffects", p.clone(), None).unwrap();
    assert!(s.is_display_only());
    assert!(s.execute("file.save", json!({})).is_err());
    let done = finish(&mut work);
    s = done.session;
    let value = done.reply.result.unwrap();
    assert_eq!(value["observation"]["phase"], "Prepared");
    assert_eq!(value["workersMayResume"], false);
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), before);
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[test]
fn independent_Stop_of_queued_complete_effect_read_cannot_cancel_original_Save_or_reacquire_workers() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let hold = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if c == "agent.readProjectSaveEffects" {
            tx.send(()).unwrap();
            let mut open = hold.0.lock().unwrap();
            while !*open {
                open = hold.1.wait(open).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, _) = fixture(&worker, 420010, "save");
    let mut begin = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    s = finish(&mut begin).session;
    let control = s.agent_operation_control();
    let mut read = worker.start(&mut s, "agent.readProjectSaveEffects", p.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let stop = control.cancel_save_wait(&p).unwrap();
    assert_eq!(stop["waitStopAccepted"], true);
    assert_eq!(stop["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut read);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    let mut fresh = worker.start(&mut s, "agent.readProjectSaveEffects", p.clone(), None).unwrap();
    let done = finish(&mut fresh);
    s = done.session;
    assert_eq!(done.reply.result.unwrap()["observation"]["phase"], "Prepared");
    cancel(&worker, &mut s, &p);
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn original_Save_worker_retains_complete_file_admission_on_same_actual_Session_and_key() {
    use std::os::unix::fs::MetadataExt;
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture(&worker, 1140000, "save_copy");
    let project = s.project.clone();
    let run = |s: &mut Session, c: &str, v: Value| {
        let mut task = worker.start(s, c, v, None).unwrap();
        let done = finish(&mut task);
        *s = done.session;
        done.reply.result.unwrap()
    };
    run(&mut s, "agent.beginProjectSaveOperation", p.clone());
    let frame = run(&mut s, "agent.prepareProjectSaveRecovery", p.clone());
    let ready = run(
        &mut s,
        "agent.prepareProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576}),
    );
    run(
        &mut s,
        "agent.publishProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"intent":ready["record"]["receipt"]["intent"]}),
    );
    let q = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"volumes":[{"root":root,"device":fs::metadata(&root).unwrap().dev().to_string(),"minimumFree":1048576}]});
    let first = run(&mut s, "agent.prepareProjectSaveAdmission", q.clone());
    let read = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"]});
    assert_eq!(run(&mut s, "agent.readProjectSaveAdmission", read), first);
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert!(Arc::ptr_eq(&s.project, &project));
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    cancel(&worker, &mut s, &p);
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn independent_Stop_of_admission_read_keeps_original_key_without_native_cancellation_or_recapture() {
    use std::os::unix::fs::MetadataExt;
    let _serial = crate::agent_bridge::native_read_test_serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, c, p| {
        if c == "agent.readProjectSaveAdmission" {
            tx.send(()).unwrap();
            let mut ready = held.0.lock().unwrap();
            while !*ready {
                ready = held.1.wait(ready).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let (mut s, p, root) = fixture(&worker, 1140010, "save_copy");
    let run = |s: &mut Session, c: &str, v: Value| {
        let mut task = worker.start(s, c, v, None).unwrap();
        let done = finish(&mut task);
        *s = done.session;
        done.reply.result.unwrap()
    };
    run(&mut s, "agent.beginProjectSaveOperation", p.clone());
    let frame = run(&mut s, "agent.prepareProjectSaveRecovery", p.clone());
    let ready = run(
        &mut s,
        "agent.prepareProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"reserveBytes":1048576}),
    );
    run(
        &mut s,
        "agent.publishProjectSaveRecoveryRecord",
        json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"intent":ready["record"]["receipt"]["intent"]}),
    );
    let q = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"],"volumes":[{"root":root,"device":fs::metadata(&root).unwrap().dev().to_string(),"minimumFree":1048576}]});
    let first = run(&mut s, "agent.prepareProjectSaveAdmission", q);
    let read = json!({"job":p["job"],"operation":p["operation"],"capture":frame["capture"]});
    let control = s.agent_operation_control();
    let mut task = worker.start(&mut s, "agent.readProjectSaveAdmission", read.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.cancel_save_wait(&p).unwrap()["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut task);
    assert!(done.reply.result.is_err());
    s = done.session;
    assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
    assert_eq!(s.execute("agent.readProjectSaveAdmission", read).unwrap(), first);
    cancel(&worker, &mut s, &p);
    s.shutdown();
}

#[test]
fn original_save_worker_settles_only_completed_media_scans_before_same_delivery_for_all_four_intents() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, root) = fixture(&worker, 245000 + i, kind);
        let project = s.project.clone();
        let revision = s.revision;
        let bytes = fs::read(root.join("Original.fcproj")).unwrap();
        let scan: crate::media_watch::MediaScan = Arc::default();
        *scan.lock().unwrap() = Some(Vec::new());
        s.media_scan = Some(scan.clone());
        let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
        assert!(s.is_display_only());
        let done = finish(&mut work);
        s = done.session;
        assert_eq!(done.reply.result.unwrap()["status"], "held");
        assert!(s.media_scan.is_none());
        assert!(scan.lock().unwrap().is_none());
        assert_eq!(s.pending_save_operation_request(), Some(p.clone()));
        assert!(Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.revision, revision);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
        assert!(!root.join("Chosen.fcproj").exists());
        cancel(&worker, &mut s, &p);
        s.shutdown();
    }
}
#[test]
fn original_save_worker_keeps_running_media_scan_on_same_owner_and_refuses_without_wait_retry_or_files() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture(&worker, 245010, "save_as");
    let project = s.project.clone();
    let bytes = fs::read(root.join("Original.fcproj")).unwrap();
    let scan: crate::media_watch::MediaScan = Arc::default();
    s.media_scan = Some(scan.clone());
    let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    let done = finish(&mut work);
    s = done.session;
    let refusal = done.reply.result.err().unwrap().to_string();
    assert!(refusal.contains("mediaScan=true"), "{refusal}");
    assert!(Arc::ptr_eq(s.media_scan.as_ref().unwrap(), &scan));
    assert!(scan.lock().unwrap().is_none());
    assert!(s.pending_save_operation_request().is_none());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
    assert!(!root.join("Chosen.fcproj").exists());
    s.shutdown();
}
#[test]
fn original_save_worker_never_saves_stale_payload_when_completed_scan_observes_changed_real_media() {
    let _serial = crate::agent_bridge::native_read_test_serial();
    let worker = SessionWorker::new().unwrap();
    let (mut s, p, root) = fixture_before_read(&worker, 245020, "save_as", |s, root| {
        let file = root.join("original-media.mov");
        crate::media_test_util::make_movie(&file, filmcraft_media::DemoScene::Aurora, 64, 48, 2);
        let imported = s.execute("file.import", json!({"paths":[file]})).unwrap();
        assert_eq!(imported["errors"], json!([]));
        s.execute("media.refreshChanged", json!({})).unwrap();
        s.execute("file.save", json!({})).unwrap();
    });
    let bytes = fs::read(root.join("Original.fcproj")).unwrap();
    let file = root.join("original-media.mov");
    crate::media_test_util::make_movie(&file, filmcraft_media::DemoScene::CityNight, 64, 48, 4);
    let m = fs::metadata(&file).unwrap();
    let stamp = (m.len(), m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos());
    let scan: crate::media_watch::MediaScan = Arc::default();
    *scan.lock().unwrap() = Some(vec![(file.to_str().unwrap().into(), Some(stamp))]);
    s.media_scan = Some(scan);
    let mut work = worker.start(&mut s, "agent.beginProjectSaveOperation", p.clone(), None).unwrap();
    let done = finish(&mut work);
    s = done.session;
    assert!(done.reply.result.is_err(), "Changed media cannot validate an already encoded old Save");
    assert!(s.pending_save_operation_request().is_none());
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
    assert!(!root.join("Chosen.fcproj").exists());
    s.shutdown();
}
