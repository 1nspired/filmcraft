use super::super::{
    project_font_replacement_tests::{scope, state},
    project_font_write_tests::{batch, fixture, read},
    snapshot_jobs,
    tests::token,
};
use super::*;
use crate::Session;
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
fn setup(n: usize, readback: bool) -> (Session, Value) {
    let mut s = fixture();
    let q = if readback {
        let p = batch(&mut s, n);
        s.execute("agent.apply", p).unwrap();
        read(&mut s, n + 1);
        json!({"token":token(n+1),"scope":scope(),"commit":token(n),"side":"after","offset":0,"limit":1})
    } else {
        let d = read(&mut s, n);
        json!({"token":token(n),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":{"from":{"family":"Noto Serif"},"to":{"family":"JetBrains Mono","style":"Bold"}},"offset":0,"limit":1})
    };
    (s, json!({"job":token(n+10000),"kind":if readback{"operation_readback"}else{"replacement_plan"},"query":q}))
}
fn job<'a>(s: &'a Session, p: &Value) -> &'a Job {
    s.agent_bridge.font_resources.iter().find(|j| j.id == p["job"].as_str().unwrap()).unwrap()
}
fn drained(expected: usize) {
    let until = Instant::now() + Duration::from_secs(3);
    while snapshot_jobs::worker_count() != expected {
        assert!(Instant::now() < until, "workers did not drain");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn done(s: &Session, p: &Value) {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if !matches!(*job(s, p).state.lock().unwrap_or_else(|e| e.into_inner()), State::Running) {
            return;
        }
        assert!(Instant::now() < until, "font worker did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn delivered(s: &mut Session, p: &Value) -> Value {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let r = s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap();
        match r["status"].as_str().unwrap() {
            "running" => {}
            "delivered" => return r,
            _ => panic!("unexpected job result {r}"),
        };
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn gated(s: &mut Session, p: &Value) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (entered_tx, entered_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::channel();
    let (end_tx, end_rx) = mpsc::channel();
    start_with(s, p, None, move |prepared, check| {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let r = prepared.compute(&|| check.check());
        end_tx.send(()).unwrap();
        r
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    (release_tx, end_rx)
}
#[test]
fn background_font_plan_and_current_readback_match_original_complete_sync_queries_and_retry_delivery_without_recomputation() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for readback in [false, true] {
        let (mut s, p) = setup(91000, readback);
        let expected = s.execute(if readback { project_font_resources::READ } else { project_font_resources::PLAN }, p["query"].clone()).unwrap();
        let before = state(&s);
        let tickets = s.agent_bridge.tickets.len();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let started = start_with(&mut s, &p, None, move |prepared, check| {
            count.fetch_add(1, Ordering::SeqCst);
            prepared.compute(&|| check.check())
        })
        .unwrap();
        assert_eq!(started["status"], "running");
        assert!(started.get("result").is_none());
        let r = delivered(&mut s, &p);
        assert_eq!(r["result"], expected);
        assert_eq!(s.execute(START, p.clone()).unwrap(), r);
        assert_eq!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap(), r);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(s.agent_bridge.tickets.len(), tickets);
        assert_eq!(state(&s), before);
        assert_eq!(r["deadlineMs"], 45000);
        assert_eq!(r["deliveryMs"], 250);
        drained(0);
    }
}
#[test]
fn background_font_stop_before_queue_during_work_and_after_completion_cannot_deliver_or_restart_or_block_native_reads() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for mode in 0..3 {
        let (mut s, p) = setup(91010 + mode, false);
        let before = state(&s);
        let gate = if mode == 1 { Some(gated(&mut s, &p)) } else { None };
        if mode == 2 {
            s.execute(START, p.clone()).unwrap();
            done(&s, &p);
            drained(0);
        }
        let stopped = s.execute("agent.cancelFontResources", json!({"job":p["job"]})).unwrap();
        assert_eq!(stopped["ticketRevoked"], false);
        if let Some((release, end)) = gate {
            assert_eq!(s.execute("agent.validateProjectRead", json!({"token":p["query"]["token"],"scope":scope()})).unwrap()["status"], "current");
            assert_eq!(snapshot_jobs::worker_count(), 1);
            release.send(()).unwrap();
            end.recv_timeout(Duration::from_secs(3)).unwrap();
            drained(0);
        }
        assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
        if mode > 0 {
            assert_eq!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap()["status"], "cancelled");
        }
        assert_eq!(state(&s), before);
        assert_eq!(snapshot_jobs::worker_count(), 0);
    }
}
#[test]
fn background_font_delivery_rechecks_late_editor_project_path_and_exact_retained_operation_and_refuses_malformed_queries() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for mode in 0..4 {
        let (mut s, p) = setup(91020 + mode, true);
        let (release, end) = gated(&mut s, &p);
        match mode {
            0 => s.set_playhead(filmcraft_time::Tick(1)),
            1 => Arc::make_mut(&mut s.project).name = "Later human change".into(),
            2 => s.path = Some("/generated/different.fcproj".into()),
            _ => s.agent_bridge.tickets.iter_mut().find(|t| t.token == p["query"]["commit"].as_str().unwrap()).unwrap().after = None,
        };
        let before = state(&s);
        release.send(()).unwrap();
        end.recv_timeout(Duration::from_secs(3)).unwrap();
        drained(0);
        assert!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).is_err());
        assert_eq!(state(&s), before);
    }
    let (mut s, p) = setup(91029, false);
    let before = state(&s);
    for mode in 0..7 {
        let mut bad = p.clone();
        match mode {
            0 => bad["kind"] = json!("agent.apply"),
            1 => bad["query"]["scope"]["projectId"] = json!("foreign"),
            2 => bad["query"]["limit"] = json!(17),
            3 => bad["query"]["recipe"] = json!({}),
            4 => bad["job"] = bad["query"]["token"].clone(),
            5 => bad["command"] = json!("file.replaceFonts"),
            _ => bad["query"]["token"] = json!(token(999999)),
        };
        assert!(s.execute(START, bad).is_err(), "case{mode}");
        assert_eq!(state(&s), before);
        assert!(s.agent_bridge.font_resources.is_empty());
    }
    assert_eq!(snapshot_jobs::worker_count(), 0);
}
struct Face {
    family: String,
}
impl FontSource for Face {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Bold".into(),
            weight: 700,
            italic: false,
            index: 0,
            data: FaceData::Static(fonts::INTER_BOLD),
            origin: "generated-test",
        }]
    }
}
#[test]
fn background_font_registry_changes_after_completion_refuse_cached_delivery_and_fresh_jobs_recompute_without_authority() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(91030, false);
    let expected = s.execute(project_font_resources::PLAN, p["query"].clone()).unwrap();
    s.execute(START, p.clone()).unwrap();
    done(&s, &p);
    drained(0);
    assert_eq!(fonts::add_source(&Face { family: format!("Async unrelated {}", std::process::id()) }), 1);
    assert!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).is_err());
    let mut fresh = p.clone();
    fresh["job"] = json!(token(101031));
    s.execute(START, fresh.clone()).unwrap();
    let r = delivered(&mut s, &fresh);
    assert_eq!(r["result"], expected);
    assert_eq!(r["writeAuthority"], false);
    drained(0);
}
#[test]
fn background_font_original_deadline_and_delivery_expiry_refuse_results_without_rerunning_or_consuming_tickets() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(91040, false);
    let before = state(&s);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    start_with(&mut s, &p, Some(Instant::now() - Duration::from_millis(45001)), move |prepared, check| {
        count.fetch_add(1, Ordering::SeqCst);
        prepared.compute(&|| check.check())
    })
    .unwrap();
    done(&s, &p);
    drained(0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap()["status"], "cancelled");
    assert_eq!(state(&s), before);
    for delivered_first in [false, true] {
        let (mut s, p) = setup(91041 + usize::from(delivered_first), false);
        s.execute(START, p.clone()).unwrap();
        done(&s, &p);
        drained(0);
        if delivered_first {
            delivered(&mut s, &p);
        }
        let j = job(&s, &p);
        let mut state = j.state.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *state {
            State::Done(_, at) | State::Delivered(_, at) => *at = Instant::now() - Duration::from_millis(251),
            _ => panic!("completed result missing"),
        };
        drop(state);
        let before = super::super::project_font_replacement_tests::state(&s);
        assert!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).is_err());
        assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
        assert_eq!(super::super::project_font_replacement_tests::state(&s), before);
    }
}
#[test]
fn background_font_uses_original_shared_worker_limit_and_waits_for_cancelled_worker_drain_without_evicting_running_reads() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    let (mut s, p) = setup(91050, false);
    let source_permit = Worker::acquire().unwrap();
    let (release, end) = gated(&mut s, &p);
    let mut second = p.clone();
    second["job"] = json!(token(101051));
    assert_eq!(snapshot_jobs::worker_count(), 2);
    assert!(s.execute(START, second.clone()).is_err());
    assert_eq!(s.agent_bridge.font_resources.len(), 1);
    s.execute("agent.cancelFontResources", json!({"job":p["job"]})).unwrap();
    assert!(s.execute(START, second.clone()).is_err());
    release.send(()).unwrap();
    end.recv_timeout(Duration::from_secs(3)).unwrap();
    drained(1);
    drop(source_permit);
    drained(0);
    s.execute(START, second.clone()).unwrap();
    delivered(&mut s, &second);
    drained(0);
    for index in 0..4 {
        let mut q = p.clone();
        q["job"] = json!(token(101060 + index));
        s.execute(START, q.clone()).unwrap();
        done(&s, &q);
        drained(0);
        s.execute("agent.cancelFontResources", json!({"job":q["job"]})).unwrap();
        assert!(s.agent_bridge.font_resources.len() <= 4);
    }
    assert!(s.agent_bridge.stopped_font_resources.iter().any(|id| id == p["job"].as_str().unwrap()));
    assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
}
#[test]
fn background_font_worker_errors_and_panics_fail_as_reads_release_shared_permits_and_never_publish() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for panics in [false, true] {
        let (mut s, p) = setup(91070 + usize::from(panics), false);
        let before = state(&s);
        start_with(&mut s, &p, None, move |_, _| {
            if panics {
                panic!("generated font worker failure")
            }
            Err(error("generated read failure"))
        })
        .unwrap();
        done(&s, &p);
        drained(0);
        let r = s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap();
        assert_eq!(r["status"], "failed");
        assert!(r.get("result").is_none());
        assert_eq!(state(&s), before);
        assert_eq!(s.execute(START, p.clone()).unwrap(), r);
    }
}
#[test]
fn background_font_original_full_target_traversal_observes_shared_check_before_completion_for_plan_and_current_readback() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained(0);
    for readback in [false, true] {
        let (mut s, p) = setup(91080, readback);
        let prepared = project_font_resources::prepare(&s, p["kind"].as_str().unwrap(), &p["query"]).unwrap();
        let full = AtomicUsize::new(0);
        let original = prepared
            .compute(&|| {
                full.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        assert_eq!(original.value["total"].as_u64(), Some(4));
        let limit = full.load(Ordering::SeqCst) - 2;
        assert!(limit > 6);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let before = state(&s);
        start_with(&mut s, &p, None, move |prepared, check| {
            prepared.compute(&|| {
                if counter.fetch_add(1, Ordering::SeqCst) + 1 == limit {
                    check.stop.store(true, Ordering::Release);
                }
                check.check()
            })
        })
        .unwrap();
        done(&s, &p);
        drained(0);
        assert_eq!(calls.load(Ordering::SeqCst), limit);
        assert_eq!(s.execute("agent.fontResourcesStatus", json!({"job":p["job"]})).unwrap()["status"], "cancelled");
        assert_eq!(state(&s), before);
    }
}
