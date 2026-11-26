use super::*;
use crate::agent_bridge::condense_tests::{operation, setup};
use crate::agent_bridge::tests::{read, token};

fn request(s: &Session, n: usize) -> Value {
    json!({"token":token(n),"candidate":token(n+1000),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "label":"Generated prepared dialogue","operation":operation(s)})
}
fn before(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"dirty":s.is_dirty(),
        "undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal,"events":s.events.len()})
}
fn prepared(s: &mut Session, n: usize) -> (Value, Value) {
    read(s, n);
    let r = request(s, n);
    let result = s.execute(PREPARE, r.clone()).unwrap();
    (r, result)
}
fn wait(s: &mut Session, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let r = s.execute("agent.inspection", json!({"job":id})).unwrap();
        if r["status"] != "running" {
            return r;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn restoration_request(s: &mut Session, n: usize) -> Value {
    let op = operation(s);
    read(s, n);
    s.execute("agent.apply", crate::agent_bridge::tests::exact_batch(n, vec![op.clone()])).unwrap();
    s.edit_sequence("Human finishing after condense", |q, _, _| {
        q.video_tracks[0].items[0].name = "Current human picture".into();
        q.audio_tracks[0].items[1].gain_db = -7.0;
        q.video_tracks[0].items[2].name = "Unrelated human work".into();
        Ok(())
    })
    .unwrap();
    read(s, n + 1);
    json!({"token":token(n+1),"candidate":token(n+1000),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "label":"Inspect original native span before restoring", "operation":{"command":"agent.restoreCondense",
        "params":{"projectId":"p_test","identity":op["params"]["passages"][0]["identity"],"receiptSha256":op["params"]["receiptSha256"]}}})
}

#[test]
fn restore_candidate_inspects_and_commits_the_same_preserved_human_work_in_one_history_step() {
    let mut s = setup();
    let r = restoration_request(&mut s, 7000);
    let human = s.project.clone();
    let state = before(&s);
    let summary = s.execute(PREPARE, r.clone()).unwrap();
    assert_eq!(before(&s), state);
    assert!(Arc::ptr_eq(&s.project, &human));
    assert_eq!(summary["operation"], "agent.restoreCondense");
    assert_eq!(summary["outputQualified"], false);
    assert_eq!(summary["result"]["restored"], "original_native_span");
    assert_eq!(summary["result"]["canonicalVersionSelected"], false);
    let rate = s.sequence_rate();
    assert_eq!(summary["result"]["restorationRange"], json!({"startTicks":rate.tick_of(8).0,"endTicks":rate.tick_of(32).0}));
    assert!(summary["result"]["boundaryTicks"].as_array().unwrap().len() >= 4);
    let scope: sequences::Scope = serde_json::from_value(r["scope"].clone()).unwrap();
    let candidate = for_read(&s, r["candidate"].as_str().unwrap(), &scope).unwrap();
    assert_eq!(s.execute(PREPARE, r.clone()).unwrap(), summary);
    let q = candidate.sequence(s.state.active_sequence.unwrap()).unwrap();
    assert_eq!(q.video_tracks[0].items[0].name, "Current human picture");
    assert_eq!(q.audio_tracks[0].items[1].gain_db, -7.0);
    assert_eq!(q.video_tracks[0].items[2].name, "Unrelated human work");
    read(&mut s, 7002);
    let command = json!({"token":token(7002),"candidate":r["candidate"]});
    let receipt = s.execute(COMMIT, command.clone()).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*candidate).unwrap());
    assert_eq!(receipt["results"][0], summary["result"]);
    assert_eq!(s.history.undo.len(), state["undo"].as_u64().unwrap() as usize + 1);
    assert_eq!(s.journal.len(), state["journal"].as_array().unwrap().len() + 1);
    read(&mut s, 7003);
    s.execute("agent.undo", json!({"token":token(7003),"commit":token(7002)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &human));
    assert_eq!(s.execute(COMMIT, command).unwrap(), receipt, "historical receipt cannot reapply after Undo");
    assert!(Arc::ptr_eq(&s.project, &human));
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*candidate).unwrap());
}

#[test]
fn restore_candidate_picture_and_mix_match_the_exact_committed_graph_without_live_seek() {
    let _guard = inspection::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = setup();
    let r = restoration_request(&mut s, 7100);
    let state = before(&s);
    let summary = s.execute(PREPARE, r.clone()).unwrap();
    let id = r["candidate"].as_str().unwrap();
    let mut outputs = Vec::new();
    for (n, request) in [(7102, json!({"kind":"frame","frame":14,"maxSide":64})), (7103, json!({"kind":"audio","startSample":24000,"samples":2400}))] {
        read(&mut s, n);
        let job = token(n + 1000);
        s.execute("agent.inspect", json!({"token":token(n),"job":job,"scope":r["scope"],"request":request,"candidate":id})).unwrap();
        let output = wait(&mut s, &job);
        assert_eq!(output["status"], "complete", "{output}");
        assert_eq!(output["candidateOperation"], "agent.restoreCondense");
        assert_eq!(output["view"], "condense_candidate");
        assert_eq!(output["revision"], summary["baseRevision"]);
        outputs.push((request, output["result"]["data"].clone()));
        assert_eq!(before(&s), state);
        s.execute("agent.cancelInspection", json!({"job":job})).unwrap();
    }
    read(&mut s, 7104);
    s.execute(COMMIT, json!({"token":token(7104),"candidate":id})).unwrap();
    for (n, (request, bytes)) in outputs.into_iter().enumerate() {
        read(&mut s, 7110 + n);
        let job = token(8110 + n);
        s.execute("agent.inspect", json!({"token":token(7110+n),"job":job,"scope":r["scope"],"request":request})).unwrap();
        let output = wait(&mut s, &job);
        assert_eq!(output["status"], "complete", "{output}");
        assert_eq!(output["result"]["data"], bytes);
        assert!(output.get("candidateOperation").is_none());
        s.execute("agent.cancelInspection", json!({"job":job})).unwrap();
    }
}

#[test]
fn restore_candidate_refuses_foreign_already_full_and_malformed_requests_without_history_or_cache_loss() {
    let mut s = setup();
    let r = restoration_request(&mut s, 7200);
    // Rejected preparations must leave a pre-existing human Redo branch too.
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("retained human gesture".into());
    read(&mut s, 7202);
    let mut r = r;
    r["token"] = json!(token(7202));
    let state = before(&s);
    for n in 0..7 {
        let mut request = r.clone();
        match n {
            0 => request["scope"]["projectId"] = json!("foreign"),
            1 => request["operation"]["params"]["receiptSha256"] = json!("e".repeat(64)),
            2 => request["operation"]["params"]["identity"] = json!("e".repeat(32)),
            3 => request["operation"]["params"]["sourceIn"] = json!(0),
            4 => request["operation"]["command"] = json!("edit.undo"),
            5 => request["operation"]["params"]["projectId"] = json!("foreign"),
            _ => request["operation"]["params"]["receiptSha256"] = json!(false),
        }
        assert!(s.execute(PREPARE, request).is_err());
        assert_eq!(before(&s), state);
        assert!(s.agent_bridge.candidates.is_empty());
    }
    s.execute(PREPARE, r.clone()).unwrap();
    read(&mut s, 7203);
    s.execute(COMMIT, json!({"token":token(7203),"candidate":r["candidate"]})).unwrap();
    read(&mut s, 7204);
    r["token"] = json!(token(7204));
    r["candidate"] = json!(token(8204));
    let full = before(&s);
    assert!(s.execute(PREPARE, r).is_err());
    assert_eq!(before(&s), full);
}

#[test]
fn restore_candidate_discard_and_intervening_human_work_invalidate_evidence_without_undoing_anything() {
    for discard in [false, true] {
        let mut s = setup();
        let r = restoration_request(&mut s, 7300);
        s.execute(PREPARE, r.clone()).unwrap();
        if discard {
            s.execute(DISCARD, json!({"token":r["token"],"candidate":r["candidate"]})).unwrap();
        } else {
            s.edit_sequence("Later human work", |q, _, _| {
                q.video_tracks[0].items[0].name = "New human choice".into();
                Ok(())
            })
            .unwrap();
        }
        let state = before(&s);
        read(&mut s, 7302);
        assert!(s.execute(COMMIT, json!({"token":token(7302),"candidate":r["candidate"]})).is_err());
        let scope: sequences::Scope = serde_json::from_value(r["scope"].clone()).unwrap();
        assert!(for_read(&s, r["candidate"].as_str().unwrap(), &scope).is_err());
        assert_eq!(before(&s), state);
    }
}

#[test]
fn condense_candidate_prepare_is_read_only_and_idempotent_then_publishes_the_exact_inspected_identities() {
    let mut s = setup();
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("human gesture".into());
    let original = s.project.clone();
    let state = before(&s);
    let (r, summary) = prepared(&mut s, 100);
    assert_eq!(before(&s), state);
    assert!(Arc::ptr_eq(&original, &s.project));
    assert_eq!(summary["status"], "prepared");
    assert_eq!(summary["outputQualified"], false);
    assert_eq!(summary["result"]["joinTicks"].as_array().unwrap().len(), 3);
    let scope: sequences::Scope = serde_json::from_value(r["scope"].clone()).unwrap();
    let id = r["candidate"].as_str().unwrap();
    let candidate = for_read(&s, id, &scope).unwrap();
    assert_eq!(s.execute(PREPARE, r.clone()).unwrap(), summary);
    assert!(Arc::ptr_eq(&candidate, &for_read(&s, id, &scope).unwrap()));
    // Many evidence reads can evict the original read ticket, not the candidate.
    for n in 2000..2260 {
        read(&mut s, n);
    }
    read(&mut s, 101);
    let commit = json!({"token":token(101),"candidate":id});
    let receipt = s.execute(COMMIT, commit.clone()).unwrap();
    assert_eq!(receipt["candidate"], id);
    assert_eq!(receipt["revision"], state["revision"].as_u64().unwrap() + 1);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*candidate).unwrap(), "commit must not rebuild new split identities");
    assert_eq!(s.history.undo.len(), state["undo"].as_u64().unwrap() as usize + 1);
    assert!(s.history.redo.is_empty());
    assert_eq!(s.journal.len(), state["journal"].as_array().unwrap().len() + 1);
    assert_eq!(s.execute(COMMIT, commit.clone()).unwrap(), receipt);
    read(&mut s, 102);
    s.execute("agent.undo", json!({"token":token(102),"commit":token(101)})).unwrap();
    assert!(Arc::ptr_eq(&original, &s.project));
    assert_eq!(s.execute(COMMIT, commit).unwrap(), receipt, "lost reply is historical, not a redo");
    assert!(Arc::ptr_eq(&original, &s.project));
    read(&mut s, 103);
    assert!(s.execute(COMMIT, json!({"token":token(103),"candidate":id})).is_err());
}

#[test]
fn condense_candidate_frame_and_mix_reads_use_the_prepared_graph_without_touching_the_live_cut() {
    let _guard = inspection::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = setup();
    let original = before(&s);
    let (r, summary) = prepared(&mut s, 110);
    let id = r["candidate"].as_str().unwrap();
    for (n, request) in [(111, json!({"kind":"frame","frame":11,"maxSide":64})), (112, json!({"kind":"audio","startSample":24000,"samples":480}))] {
        read(&mut s, n);
        let job = token(n + 5000);
        s.execute("agent.inspect", json!({"token":token(n),"job":job,"scope":r["scope"],"request":request,"candidate":id})).unwrap();
        let output = wait(&mut s, &job);
        assert_eq!(output["status"], "complete", "{output}");
        assert_eq!(output["candidate"], id);
        assert_eq!(output["view"], "condense_candidate");
        assert_eq!(output["revision"], summary["baseRevision"]);
        assert!(!output["result"]["data"].as_str().unwrap().is_empty());
        assert_eq!(before(&s), original);
        s.execute("agent.cancelInspection", json!({"job":job})).unwrap();
    }
    read(&mut s, 113);
    let job = token(5113);
    s.execute("agent.inspect", json!({"token":token(113),"job":job,"scope":r["scope"],"request":{"kind":"frame","frame":11,"maxSide":64}})).unwrap();
    let live = wait(&mut s, &job);
    assert!(live.get("candidate").is_none());
    assert!(live.get("view").is_none());
    assert_eq!(before(&s), original);
}

#[test]
fn condense_candidate_rejects_hostile_generic_operations_and_never_falls_back_to_live_program() {
    let mut s = setup();
    read(&mut s, 120);
    let r = request(&s, 120);
    let original = before(&s);
    for mutate in [0, 1, 2, 3, 4, 5] {
        let mut p = r.clone();
        match mutate {
            0 => p["operation"]["command"] = json!("file.saveAs"),
            1 => p["scope"]["projectId"] = json!("foreign"),
            2 => p["candidate"] = p["token"].clone(),
            3 => p["extra"] = json!(true),
            4 => p["operation"]["params"]["passages"][0]["identity"] = json!("a".repeat(32)),
            _ => p["label"] = json!(""),
        }
        assert!(s.execute(PREPARE, p).is_err());
        assert_eq!(before(&s), original);
    }
    let (_, result) = prepared(&mut s, 121);
    assert_eq!(result["status"], "prepared");
    for value in [Value::Null, json!("unknown"), json!(false), json!(token(9912))] {
        assert!(
            s.execute(
                "agent.inspect",
                json!({"token":token(121),"job":token(5121),"scope":r["scope"],
            "request":{"kind":"frame","frame":11,"maxSide":64},"candidate":value})
            )
            .is_err()
        );
    }
    assert_eq!(before(&s), original);
}

#[test]
fn condense_candidate_human_changes_same_path_replacement_and_context_changes_invalidate_preview_and_commit() {
    for mode in ["human", "replacement", "context"] {
        let mut s = setup();
        let (r, _) = prepared(&mut s, 130);
        let id = r["candidate"].as_str().unwrap();
        match mode {
            "human" => {
                s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
            }
            "replacement" => s.project = Arc::new((*s.project).clone()),
            _ => s.state.linked_selection = !s.state.linked_selection,
        }
        let state = before(&s);
        read(&mut s, 131);
        assert!(s.execute(COMMIT, json!({"token":token(131),"candidate":id})).is_err());
        assert!(
            s.execute(
                "agent.inspect",
                json!({"token":token(131),"job":token(5131),"scope":r["scope"],
            "request":{"kind":"frame","frame":11,"maxSide":64},"candidate":id})
            )
            .is_err()
        );
        assert_eq!(before(&s), state);
        s.execute(DISCARD, json!({"token":r["token"],"candidate":id})).unwrap();
        assert_eq!(before(&s), state);
    }
}

#[test]
fn condense_candidate_limits_expiry_and_discard_are_bounded_and_cancel_completed_evidence() {
    let _guard = inspection::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = setup();
    let (r, _) = prepared(&mut s, 140);
    let (other, _) = prepared(&mut s, 141);
    prepared(&mut s, 144);
    let state = before(&s);
    read(&mut s, 142);
    assert!(s.execute(PREPARE, request(&s, 142)).is_err());
    assert_eq!(before(&s), state);
    let id = r["candidate"].as_str().unwrap();
    let job = token(5140);
    s.execute(
        "agent.inspect",
        json!({"token":token(140),"job":job,"scope":r["scope"],
        "request":{"kind":"frame","frame":11,"maxSide":64},"candidate":id}),
    )
    .unwrap();
    assert_eq!(wait(&mut s, &job)["status"], "complete");
    s.execute(DISCARD, json!({"token":r["token"],"candidate":id})).unwrap();
    assert!(s.execute("agent.inspection", json!({"job":job})).is_err());
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(142),"candidate":id})).is_err());
    assert!(s.execute(COMMIT, json!({"token":token(142),"candidate":id})).is_err());
    assert!(s.execute(PREPARE, r).is_err());
    let id = other["candidate"].as_str().unwrap();
    s.agent_bridge.candidates.iter_mut().find(|c| c.id == id).unwrap().started -= LIFETIME;
    assert!(s.execute(COMMIT, json!({"token":token(142),"candidate":id})).is_err());
    assert_eq!(before(&s), state);
    let (_, next) = prepared(&mut s, 143);
    assert_eq!(next["status"], "prepared");
}
