use super::*;
use crate::agent_bridge::{
    assembly_tests,
    tests::{exact_batch, read, token},
};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> Session {
    assembly_tests::fixture().0
}
fn split(s: &Session, n: usize) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    exact_batch(n, vec![json!({"command":"timeline.razor","params":{"clip":c.id.0,"time":c.start.0+s.sequence_rate().tick_of(1).0}})])
}
fn basis(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"merge":s.history.merge_key,"events":s.events.len()})
}

#[test]
fn cancellation_before_a_queued_write_cannot_publish_or_be_restarted() {
    let mut s = fixture();
    read(&mut s, 71001);
    let before = basis(&s);
    let p = split(&s, 71001);
    let control = s.agent_operation_control();
    let r = control.cancel(&json!({"token":token(71001)})).unwrap();
    assert_eq!(r["cancellationAccepted"], true);
    assert_eq!(r["status"], "cancelled");
    assert!(s.execute("agent.apply", p.clone()).is_err());
    assert!(s.execute("agent.apply", p).is_err());
    assert_eq!(basis(&s), before);
    read(&mut s, 71002);
    s.execute("agent.apply", split(&s, 71002)).unwrap();
}

#[test]
fn independent_cancellation_during_staging_blocks_the_same_final_native_publication() {
    let mut s = fixture();
    read(&mut s, 71003);
    let before = basis(&s);
    let p = split(&s, 71003);
    let control = s.agent_operation_control();
    let (tx, rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let r = control.cancel(&json!({"token":token(71003)})).unwrap();
        done_tx.send(r).unwrap();
    });
    let result = run(&mut s, "agent.apply", &p, |s, p| {
        let batch: crate::agent_bridge::Batch = serde_json::from_value(p.clone()).unwrap();
        crate::agent_bridge::check_edit_ticket(s, &batch.token)?;
        let stage = crate::agent_bridge::stage_batch(s, &batch)?;
        tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(3)).unwrap()["cancellationAccepted"], true);
        crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
    });
    worker.join().unwrap();
    assert!(result.is_err());
    assert_eq!(basis(&s), before);
    assert!(s.agent_bridge.operations.check().is_none());
}

#[test]
fn publication_fence_makes_late_cancel_explicit_and_preserves_historical_receipts() {
    let mut s = fixture();
    read(&mut s, 71004);
    let p = split(&s, 71004);
    let receipt = s.execute("agent.apply", p.clone()).unwrap();
    let after = basis(&s);
    let r = s.agent_operation_control().cancel(&json!({"token":token(71004)})).unwrap();
    assert_eq!(r["status"], "committed");
    assert_eq!(r["cancellationAccepted"], false);
    assert_eq!(s.execute("agent.apply", p).unwrap(), receipt);
    assert_eq!(basis(&s), after);
    read(&mut s, 71005);
    let control = s.agent_operation_control();
    let operation = control.begin(&token(71005)).unwrap();
    control.enter_publication(&token(71005)).unwrap();
    let r = control.cancel(&json!({"token":token(71005)})).unwrap();
    assert_eq!(r["status"], "publication_started");
    assert_eq!(r["cancellationAccepted"], false);
    drop(operation);
}

#[test]
fn expired_foreign_and_hostile_tokens_do_not_create_authority_or_evict_an_active_operation() {
    let control = AgentOperationControl::default();
    let unknown = control.cancel(&json!({"token":token(71999)})).unwrap();
    assert_eq!(unknown["status"], "unknown");
    assert!(control.inner.lock().unwrap().rows.is_empty());
    for p in [json!(null), json!({"token":"short"}), json!({"token":token(71999),"all":true}), json!({"token":42})] {
        assert!(control.cancel(&p).is_err());
    }
    control.register(&token(71006)).unwrap();
    let operation = control.begin(&token(71006)).unwrap();
    for n in 72000..72300 {
        control.register(&token(n)).unwrap();
    }
    assert_eq!(control.inner.lock().unwrap().rows.len(), super::super::TICKET_LIMIT);
    assert!(control.inner.lock().unwrap().rows.iter().any(|(id, _)| id == &token(71006)));
    drop(operation);
    assert!(control.begin(&token(72000)).is_err());
}

#[test]
fn panic_cleanup_releases_the_active_scope_without_reviving_the_consumed_ticket() {
    let mut s = fixture();
    read(&mut s, 71007);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut s, "agent.apply", &json!({"token":token(71007)}), |_, _| panic!("generated command failure"))
    }));
    assert!(result.is_err());
    assert!(s.agent_bridge.operations.check().is_none());
    assert!(s.agent_bridge.operations.begin(&token(71007)).is_err());
    read(&mut s, 71008);
    s.execute("agent.apply", split(&s, 71008)).unwrap();
}

#[test]
fn cancel_and_publication_have_exactly_one_winner() {
    for n in 73000..73064 {
        let control = AgentOperationControl::default();
        let id = token(n);
        control.register(&id).unwrap();
        let operation = control.begin(&id).unwrap();
        let start = Arc::new(std::sync::Barrier::new(2));
        let other = start.clone();
        let cancel = control.clone();
        let request = json!({"token":id});
        let worker = std::thread::spawn(move || {
            other.wait();
            cancel.cancel(&request).unwrap()
        });
        start.wait();
        let publication = control.enter_publication(&id);
        let reply = worker.join().unwrap();
        assert_ne!(publication.is_ok(), reply["cancellationAccepted"] == true);
        drop(operation);
    }
}

#[test]
fn cancelled_undo_and_redo_preserve_the_owned_history_branch() {
    let mut s = fixture();
    read(&mut s, 74000);
    s.execute("agent.apply", split(&s, 74000)).unwrap();
    read(&mut s, 74001);
    let before = basis(&s);
    s.agent_operation_control().cancel(&json!({"token":token(74001)})).unwrap();
    assert!(s.execute("agent.undo", json!({"token":token(74001),"commit":token(74000)})).is_err());
    assert_eq!(basis(&s), before);
    read(&mut s, 74002);
    s.execute("agent.undo", json!({"token":token(74002),"commit":token(74000)})).unwrap();
    read(&mut s, 74003);
    let before = basis(&s);
    s.agent_operation_control().cancel(&json!({"token":token(74003)})).unwrap();
    assert!(s.execute("agent.redo", json!({"token":token(74003),"commit":token(74000),"undo":token(74002)})).is_err());
    assert_eq!(basis(&s), before);
    read(&mut s, 74004);
    s.execute("agent.redo", json!({"token":token(74004),"commit":token(74000),"undo":token(74002)})).unwrap();
}

#[test]
fn cancelling_candidate_commit_does_not_publish_or_destroy_the_inspected_candidate() {
    let mut s = crate::agent_bridge::condense_tests::setup();
    read(&mut s, 74005);
    let request = json!({"token":token(74005),"candidate":token(75005),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "label":"Generated cancel candidate","operation":crate::agent_bridge::condense_tests::operation(&s)});
    s.execute("agent.prepareCondense", request).unwrap();
    read(&mut s, 74006);
    let before = basis(&s);
    s.agent_operation_control().cancel(&json!({"token":token(74006)})).unwrap();
    assert!(s.execute("agent.commitCondense", json!({"token":token(74006),"candidate":token(75005)})).is_err());
    assert_eq!(basis(&s), before);
    read(&mut s, 74007);
    assert_eq!(s.execute("agent.commitCondense", json!({"token":token(74007),"candidate":token(75005)})).unwrap()["status"], "committed");
}

#[test]
fn large_source_clock_and_transport_hint_do_not_change_stop_or_ordinary_operation_authority() {
    let c = AgentOperationControl::default();
    let id = token(79001);
    c.register(&id).unwrap();
    let work = c.begin_sources(&id, false, crate::agent_bridge::media_guard::LARGE_SOURCE_BYTES).unwrap();
    assert_eq!(work.flag.check.deadline_ms(), 600000);
    assert_eq!(c.cancel(&json!({"token":id})).unwrap()["cancellationAccepted"], true);
    assert!(c.enter_publication(&id).is_err());
    drop(work);
    let id = token(79002);
    c.register(&id).unwrap();
    let ordinary = c.begin_sources(&id, false, 0).unwrap();
    assert_eq!(ordinary.flag.check.deadline_ms(), 45000);
    let request = json!({"command":"agent.undo","params":{"token":id,"commit":token(79003)},"verificationPolicy":"large_originals_v1"});
    assert_eq!(crate::session_worker::control_reply_wait_ms("engine.execute", &request), 602000);
    assert_eq!(ordinary.flag.check.deadline_ms(), 45000, "reply hints never select the actual native clock");
    assert_eq!(crate::session_worker::control_reply_wait_ms("state.inspect", &request), 60000);
    let mut wrong = request;
    wrong["verificationPolicy"] = json!("caller-deadline");
    assert_eq!(crate::session_worker::control_reply_wait_ms("engine.execute", &wrong), 60000);
}
