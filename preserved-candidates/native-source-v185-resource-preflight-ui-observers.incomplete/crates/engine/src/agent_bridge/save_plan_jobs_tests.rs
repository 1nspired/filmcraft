use super::super::{
    project_font_replacement_tests::{scope, setup as owner, state},
    project_plane, snapshot_jobs,
    tests::token,
};
use super::*;
use std::sync::{atomic::AtomicUsize, mpsc};
fn setup(n: usize, intent: Value) -> (Session, Value) {
    let mut s = owner();
    let doc = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    let query = json!({"token":token(n),"scope":scope(),"beforeProjectSha256":doc["projectState"]["sha256"],"intent":intent});
    (s, json!({"job":token(n+10000),"query":query}))
}
fn job<'a>(s: &'a Session, p: &Value) -> &'a Job {
    s.agent_bridge.save_plans.iter().find(|j| j.id == p["job"].as_str().unwrap()).unwrap()
}
fn drained(n: usize) {
    let until = Instant::now() + Duration::from_secs(3);
    while snapshot_jobs::worker_count() != n {
        assert!(Instant::now() < until, "worker permit did not drain");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn done(s: &Session, p: &Value) {
    let until = Instant::now() + Duration::from_secs(3);
    while matches!(*job(s, p).state.lock().unwrap_or_else(|e| e.into_inner()), State::Running) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn delivered(s: &mut Session, p: &Value) -> Value {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let r = s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap();
        match r["status"].as_str().unwrap() {
            "running" => {}
            "delivered" => return r,
            _ => panic!("{r}"),
        };
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn gated(s: &mut Session, p: &Value) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (enter_tx, enter_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::channel();
    let (end_tx, end_rx) = mpsc::channel();
    start_with(s, p, None, move |c, check| {
        enter_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let out = c.compute_owned(&|| check.check());
        end_tx.send(()).unwrap();
        out
    })
    .unwrap();
    enter_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    (release_tx, end_rx)
}
#[test]
fn background_save_plans_match_original_complete_sync_results_for_every_intent_without_consuming_root_or_recomputing_cached_delivery() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for intent in [
        json!({"kind":"save"}),
        json!({"kind":"save_as","path":"/generated/New save.fcproj"}),
        json!({"kind":"save_copy","path":"relative copy.fcproj"}),
        json!({"kind":"save_all"}),
        json!({"kind":"save_copy"}),
    ] {
        let (mut s, p) = setup(99500, intent);
        let expected = s.execute(project_save_plan::PLAN, p["query"].clone()).unwrap();
        let before = state(&s);
        let tickets = s.agent_bridge.tickets.len();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        start_with(&mut s, &p, None, move |input, check| {
            c.fetch_add(1, Ordering::SeqCst);
            input.compute_owned(&|| check.check())
        })
        .unwrap();
        let result = delivered(&mut s, &p);
        assert_eq!(result["result"], expected);
        assert_eq!(s.execute(START, p.clone()).unwrap(), result);
        assert_eq!(s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap(), result);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(state(&s), before);
        assert_eq!(s.agent_bridge.tickets.len(), tickets);
        assert_eq!(result["deadlineMs"], 45000);
        assert_eq!(result["deliveryMs"], 250);
        assert_eq!(result["writeAuthority"], false);
        drained(0);
    }
}
#[test]
fn background_save_stop_before_queue_tombstones_exact_id_without_worker_or_root_revocation() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(99510, json!({"kind":"save"}));
    let before = state(&s);
    s.execute("agent.cancelProjectSavePlan", json!({"job":p["job"]})).unwrap();
    let result = start_with(&mut s, &p, None, |_, _| panic!("cancelled job must not start")).unwrap();
    assert_eq!(result["status"], "cancelled");
    assert_eq!(result["ticketRevoked"], false);
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.save_plans.is_empty());
    s.execute(project_plane::VALIDATE, json!({"token":p["query"]["token"],"scope":scope()})).unwrap();
    drained(0);
}
#[test]
fn background_save_stop_during_compute_keeps_shared_worker_permit_until_drain_and_never_delivers_or_restarts() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(99520, json!({"kind":"save_copy","path":"/generated/No file.fcproj"}));
    let before = state(&s);
    let (release, ended) = gated(&mut s, &p);
    assert_eq!(snapshot_jobs::worker_count(), 1);
    s.execute("agent.cancelProjectSavePlan", json!({"job":p["job"]})).unwrap();
    assert_eq!(s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap()["status"], "cancelled");
    assert_eq!(snapshot_jobs::worker_count(), 1);
    let other = Worker::acquire().unwrap();
    assert!(Worker::acquire().is_err());
    release.send(()).unwrap();
    ended.recv_timeout(Duration::from_secs(3)).unwrap();
    drop(other);
    drained(0);
    assert_eq!(state(&s), before);
    assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
}
#[test]
fn background_save_delivery_refuses_later_document_editor_saved_preferences_preview_and_media_context() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for mutation in 0..7 {
        let (mut s, p) = setup(99530, json!({"kind":"save"}));
        let (release, ended) = gated(&mut s, &p);
        match mutation {
            0 => {
                s.execute("file.newBin", json!({"name":"Later human bin"})).unwrap();
            }
            1 => s.state.source_playhead = filmcraft_time::Tick(7),
            2 => s.saved_revision = s.saved_revision.saturating_sub(1),
            3 => s.prefs.general.recent_projects.push("later preference".into()),
            4 => {
                s.previews.generation.fetch_add(1, Ordering::Relaxed);
            }
            5 => s.media.set_use_proxies(!s.media.use_proxies()),
            _ => s.path = Some("/generated/Later path.fcproj".into()),
        };
        let later = state(&s);
        release.send(()).unwrap();
        ended.recv_timeout(Duration::from_secs(3)).unwrap();
        done(&s, &p);
        assert!(s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).is_err());
        assert_eq!(state(&s), later);
        drained(0);
    }
}
#[test]
fn background_save_original_operation_and_delivery_deadlines_refuse_late_results_without_renewing_jobs() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(99540, json!({"kind":"save"}));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    start_with(&mut s, &p, Some(Instant::now() - Duration::from_secs(46)), move |c, check| {
        count.fetch_add(1, Ordering::SeqCst);
        c.compute_owned(&|| check.check())
    })
    .unwrap();
    done(&s, &p);
    let cancelled = s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled.get("result").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.execute(project_plane::VALIDATE, json!({"token":p["query"]["token"],"scope":scope()})).unwrap();
    drained(0);
    let (mut s, p) = setup(99541, json!({"kind":"save"}));
    start(&mut s, &p).unwrap();
    done(&s, &p);
    {
        let mut state = job(&s, &p).state.lock().unwrap_or_else(|e| e.into_inner());
        if let State::Done(Ok(_), at) = &mut *state {
            *at = Instant::now() - Duration::from_millis(251);
        } else {
            panic!("worker did not complete")
        }
    }
    assert!(s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).is_err());
    assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
    drained(0);
}
#[test]
fn background_save_failed_and_panicked_workers_report_failure_release_permits_and_preserve_native_state() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for panic_worker in [false, true] {
        let (mut s, p) = setup(99550, json!({"kind":"save"}));
        let before = state(&s);
        start_with(&mut s, &p, None, move |_, _| if panic_worker { panic!("generated save worker failure") } else { Err(error("generated save read error")) })
            .unwrap();
        done(&s, &p);
        let result = s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap();
        assert_eq!(result["status"], "failed");
        assert!(result.get("result").is_none());
        assert_eq!(state(&s), before);
        drained(0);
    }
}
#[test]
fn background_save_bounded_cache_tombstones_evictions_and_refuses_changed_request_for_same_job() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, first) = setup(99560, json!({"kind":"save"}));
    for index in 0..5 {
        let mut p = first.clone();
        p["job"] = json!(token(109560 + index));
        start(&mut s, &p).unwrap();
        delivered(&mut s, &p);
        drained(0);
        assert!(s.agent_bridge.save_plans.len() <= 4);
    }
    let mut evicted = first.clone();
    evicted["job"] = json!(token(109560));
    assert_eq!(s.execute(START, evicted).unwrap()["status"], "cancelled");
    let mut changed = first;
    changed["job"] = json!(token(109564));
    changed["query"]["intent"] = json!({"kind":"save_all"});
    assert!(s.execute(START, changed).is_err());
    for i in 0..300 {
        s.execute("agent.cancelProjectSavePlan", json!({"job":token(120000+i)})).unwrap();
    }
    assert_eq!(s.agent_bridge.stopped_save_plans.len(), super::super::TICKET_LIMIT);
}
#[test]
fn background_save_uses_original_two_worker_pool_and_dropped_owner_cancels_without_freeing_busy_permit() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(99570, json!({"kind":"save"}));
    let (release, ended) = gated(&mut s, &p);
    let second = Worker::acquire().unwrap();
    let mut another = p.clone();
    another["job"] = json!(token(109571));
    let count = s.agent_bridge.save_plans.len();
    assert!(start(&mut s, &another).is_err());
    assert_eq!(s.agent_bridge.save_plans.len(), count);
    s.agent_bridge.save_plans.clear();
    assert_eq!(snapshot_jobs::worker_count(), 2);
    release.send(()).unwrap();
    ended.recv_timeout(Duration::from_secs(3)).unwrap();
    drop(second);
    drained(0);
}
