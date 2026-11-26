use super::*;
use crate::session_worker::{Completion, Pending, SessionWorker};
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn world(worker: &SessionWorker) -> Session {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = crate::temp_dir().join(format!("poema-resource-worker-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let mut s = Session::default();
    s.start_autosave(crate::autosave::AutosaveConfig::new(root)).unwrap();
    s.agent_operation_control().enable_transport();
    worker.install(&mut s).unwrap();
    s
}
fn address_of(s: &mut Session, letter: char) -> Value {
    let owner = s.execute(OWNER, json!({})).unwrap();
    json!({"request":letter.to_string().repeat(64),"nativePid":owner["nativePid"],"nativeSession":owner["nativeSession"],"dataDirectory":owner["dataDirectory"]})
}
fn finish(mut pending: Pending) -> Completion {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(c) = pending.try_finish() {
            return c;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn run(worker: &SessionWorker, s: &mut Session, command: &str, p: &Value) -> Value {
    let completed = finish(worker.start(s, command, p.clone(), None).unwrap());
    let result = completed.reply.result.unwrap();
    *s = completed.session;
    result
}

#[test]
fn resource_acquire_and_release_use_original_session_worker_without_a_read_token_or_second_editor() {
    let worker = SessionWorker::new().unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'a');
    let project = s.project.clone();
    let control = s.agent_operation_control();
    let pending = worker.start(&mut s, BEGIN, r.clone(), None).unwrap();
    let same_check = control.check().unwrap();
    assert!(s.is_display_only());
    assert!(!s.resource_preflight_pending());
    assert!(s.execute(STATUS, r.clone()).is_err());
    let completed = finish(pending);
    assert_eq!(completed.reply.result.unwrap()["namespaceHeld"], true);
    s = completed.session;
    assert!(s.resource_preflight_pending());
    assert!(Arc::ptr_eq(&same_check, &control.check().unwrap()));
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    assert_eq!(run(&worker, &mut s, RELEASE, &r)["status"], "released");
    assert!(!s.resource_preflight_pending());
    assert!(!s.is_display_only());
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
#[test]
fn stop_can_overtake_prepared_begin_on_the_same_original_worker_before_any_pause_is_acquired() {
    let (entered, ready) = mpsc::channel();
    let (proceed, gate) = mpsc::channel();
    let worker = SessionWorker::test_with_executor(move |s, id, p| {
        if id == BEGIN {
            entered.send(()).unwrap();
            gate.recv().unwrap();
        }
        s.execute(id, p)
    })
    .unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'b');
    let control = s.agent_operation_control();
    let pending = worker.start(&mut s, BEGIN, r.clone(), None).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.cancel(&json!({"token":r["request"]})).unwrap()["cancellationAccepted"], true);
    proceed.send(()).unwrap();
    let completed = finish(pending);
    assert!(completed.reply.result.is_err());
    s = completed.session;
    assert!(!s.resource_preflight_pending());
    assert_eq!(s.execute(STATUS, r).unwrap()["status"], "failed");
    s.persistence.as_ref().unwrap().pause_resource_preflight(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
#[test]
fn rejected_bounded_worker_submission_retires_only_its_actual_prepared_request_and_restores_session() {
    let (entered, ready) = mpsc::channel();
    let (proceed, gate) = mpsc::channel();
    let first = std::sync::atomic::AtomicBool::new(true);
    let worker = SessionWorker::test_with_executor(move |s, id, p| {
        if first.swap(false, Ordering::AcqRel) {
            entered.send(()).unwrap();
            gate.recv().unwrap();
        }
        s.execute(id, p)
    })
    .unwrap();
    let mut a = world(&worker);
    let ra = address_of(&mut a, 'c');
    let pa = worker.start(&mut a, BEGIN, ra.clone(), None).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut b = world(&worker);
    let rb = address_of(&mut b, 'd');
    let pb = worker.start(&mut b, BEGIN, rb.clone(), None).unwrap();
    let mut c = world(&worker);
    let rc = address_of(&mut c, 'e');
    let original = c.project.clone();
    assert!(worker.start(&mut c, BEGIN, rc.clone(), None).is_err());
    assert!(!c.is_display_only());
    assert!(!c.resource_preflight_pending());
    assert!(Arc::ptr_eq(&original, &c.project));
    assert_eq!(c.execute(STATUS, rc.clone()).unwrap()["status"], "failed");
    assert!(c.execute(BEGIN, rc).is_err());
    c.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    proceed.send(()).unwrap();
    let ca = finish(pa);
    assert_eq!(ca.reply.result.unwrap()["namespaceHeld"], true);
    a = ca.session;
    let cb = finish(pb);
    assert_eq!(cb.reply.result.unwrap()["namespaceHeld"], true);
    b = cb.session;
    assert_eq!(a.execute(RELEASE, ra).unwrap()["status"], "released");
    assert_eq!(b.execute(RELEASE, rb).unwrap()["status"], "released");
}
#[test]
fn worker_panic_before_begin_keeps_non_ready_preparation_until_exact_release_without_a_resume_marker() {
    let worker = SessionWorker::test_with_executor(|s, id, p| {
        if id == BEGIN {
            panic!("generated pre-Begin worker failure");
        }
        s.execute(id, p)
    })
    .unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'f');
    let completed = finish(worker.start(&mut s, BEGIN, r.clone(), None).unwrap());
    assert!(completed.reply.uncertain);
    assert!(completed.reply.result.is_err());
    s = completed.session;
    assert!(s.resource_preflight_pending());
    let status = s.execute(STATUS, r.clone()).unwrap();
    assert_eq!(status["status"], "preparing");
    assert_eq!(status["namespaceHeld"], false);
    assert!(worker.start(&mut s, BEGIN, r.clone(), None).is_err());
    assert_eq!(run(&worker, &mut s, RELEASE, &r)["status"], "released");
    assert!(!s.resource_preflight_pending());
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
#[test]
fn worker_duplicate_begin_and_release_keep_same_original_hold_and_cannot_release_a_later_owner() {
    let worker = SessionWorker::new().unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'g');
    assert_eq!(run(&worker, &mut s, BEGIN, &r)["status"], "held");
    assert_eq!(run(&worker, &mut s, BEGIN, &r)["status"], "held");
    assert_eq!(run(&worker, &mut s, RELEASE, &r)["status"], "released");
    let later = s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap();
    assert_eq!(run(&worker, &mut s, RELEASE, &r)["status"], "released");
    later.current(s.persistence.as_ref().unwrap()).unwrap();
    later.finish(|| Ok(())).unwrap();
}
#[test]
fn malformed_worker_resource_address_never_consumes_session_or_prepares_a_request() {
    let worker = SessionWorker::new().unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'h');
    for (key, value) in [
        ("nativePid", json!(0)),
        ("request", json!("short")),
        ("nativeSession", json!("x".repeat(32))),
        ("dataDirectory", json!("/wrong/generated/root")),
        ("extraGrant", json!(true)),
    ] {
        let mut bad = r.clone();
        bad[key] = value;
        assert!(worker.start(&mut s, BEGIN, bad, None).is_err());
        assert!(!s.is_display_only());
        assert!(!s.resource_preflight_pending());
    }
    assert_eq!(run(&worker, &mut s, BEGIN, &r)["namespaceHeld"], true);
    assert_eq!(run(&worker, &mut s, RELEASE, &r)["status"], "released");
}
#[test]
fn shutdown_during_original_resource_hold_preserves_recovery_files_instead_of_cleaning_or_resuming() {
    let worker = SessionWorker::new().unwrap();
    let mut s = world(&worker);
    s.set_prefs(s.prefs.clone()).unwrap();
    let root = s.persistence.as_ref().unwrap().session_dir.clone();
    let sentinel = root.join("generated-preserve-on-exit.keep");
    fs::write(&sentinel, b"GENERATED_RECOVERY_MUST_SURVIVE").unwrap();
    let r = address_of(&mut s, 'i');
    assert_eq!(run(&worker, &mut s, BEGIN, &r)["status"], "held");
    s.shutdown();
    assert!(s.persistence.is_none());
    assert!(s.resource_preflight_pending());
    assert_eq!(fs::read(sentinel).unwrap(), b"GENERATED_RECOVERY_MUST_SURVIVE");
    assert!(s.execute(STATUS, r).is_err());
}

#[test]
fn physical_resource_directory_replacement_while_queued_is_refused_before_original_journal_io_without_rebasing() {
    let (entered, ready) = mpsc::channel();
    let (proceed, gate) = mpsc::channel();
    let worker = SessionWorker::test_with_executor(move |s, id, p| {
        if id == BEGIN {
            entered.send(()).unwrap();
            gate.recv().unwrap();
        }
        s.execute(id, p)
    })
    .unwrap();
    let mut s = world(&worker);
    let r = address_of(&mut s, 'j');
    let root = s.persistence.as_ref().unwrap().data_dir.clone();
    let original = root.with_extension("original-held");
    let replacement = root.with_extension("replacement-preserved");
    let pending = worker.start(&mut s, BEGIN, r.clone(), None).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    fs::rename(&root, &original).unwrap();
    fs::create_dir(&root).unwrap();
    proceed.send(()).unwrap();
    let completed = finish(pending);
    assert!(completed.reply.result.is_err());
    s = completed.session;
    assert!(!s.resource_preflight_pending());
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "failed");
    assert!(s.execute(BEGIN, r).is_err());
    fs::rename(&root, replacement).unwrap();
    fs::rename(original, root).unwrap();
    s.persistence.as_ref().unwrap().pause_resource_preflight(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
