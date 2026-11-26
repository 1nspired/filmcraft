//! Explicit no-edit completion, original live fences and passive recovery only.
use super::*;
fn ready(method: &str, id: usize) -> (Fixture, Value, Value) {
    let mut f = fixture();
    let (seq, ids) = timeline(&mut f);
    let clips = f.s.project.sequence(ItemId(seq)).unwrap().video_tracks.iter().map(|t| t.items[0].id.0).collect::<Vec<_>>();
    f.s.execute("clip.synchronize", json!({"clips":clips,"method":method,"marker":"Clap","ignoreHours":true})).unwrap();
    inputs(&mut f, id, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, id, json!({"gesture":"synchronize","options":{"method":method,"marker":"Clap","ignoreHours":true}})).unwrap();
    assert_eq!(doc["result"]["moved"], 0);
    let request = commit_request(id, &doc);
    (f, doc, request)
}
fn head(s: &Session) -> Option<(u64, usize)> {
    s.agent_bridge.head.as_ref().map(|(revision, p)| (*revision, p.as_ptr() as usize))
}
fn history(s: &Session) -> Value {
    json!({"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key})
}
fn ack(f: &mut Fixture, request: Value) -> Value {
    let before = state(&f.s);
    let graph = f.s.project.clone();
    let owner = head(&f.s);
    let stack = history(&f.s);
    let result = worker_command(f, "agent.apply", request).unwrap();
    assert_eq!(result["status"], "committed");
    assert_eq!(result["action"], "multicam_production");
    assert_eq!(result["outcome"], "unchanged");
    assert_eq!(result["timelineCommits"], 0);
    assert_eq!(result["undoOwned"], false);
    assert_eq!(result["revision"], result["beforeRevision"]);
    assert!(Arc::ptr_eq(&f.s.project, &graph));
    assert_eq!(state(&f.s), before);
    assert_eq!(head(&f.s), owner);
    assert_eq!(history(&f.s), stack);
    result
}
#[test]
fn all_five_original_methods_acknowledge_no_change_without_native_commit_head_journal_or_reanalysis_and_retry_after_human_work() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for method in ["in", "out", "timecode", "marker", "audio"] {
        let (mut f, _, request) = ready(method, 500);
        let result = ack(&mut f, request.clone());
        let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(500)).unwrap();
        assert!(t.multicam_production.is_none());
        assert!(t.multicam_history.is_some());
        assert_eq!(t.project.as_ptr(), t.after.as_ref().unwrap().as_ptr());
        f.s.execute("file.newBin", json!({"name":"Later independent human work"})).unwrap();
        let before = state(&f.s);
        assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), result);
        assert_eq!(f.s.execute("agent.receipt", json!({"token":token(500)})).unwrap()["result"], result);
        assert_eq!(state(&f.s), before);
        eprintln!("native unchanged {method}: exact result, zero timeline/history/journal effects, retained retry after human work");
    }
}
#[test]
fn unchanged_intent_preserves_previous_owned_source_undo_and_never_borrows_an_edit_of_its_own() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, ids) = timeline(&mut f);
    let original = f.s.project.clone();
    inputs(&mut f, 510, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 510, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    worker_command(&mut f, "agent.apply", commit_request(510, &doc)).unwrap();
    let changed = f.s.project.clone();
    inputs(&mut f, 511, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 511, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    ack(&mut f, commit_request(511, &doc));
    inputs(&mut f, 512, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    for (commit, available) in [(510, true), (511, false)] {
        assert_eq!(
            f.s.execute("agent.readHistory", json!({"token":token(512),"scope":scope(),"commit":token(commit),"undo":null})).unwrap()["undoAvailable"],
            available
        );
    }
    let before = state(&f.s);
    assert!(worker_command(&mut f, "agent.undo", json!({"token":token(512),"commit":token(511)})).is_err());
    assert_eq!(state(&f.s), before);
    worker_command(&mut f, "agent.undo", json!({"token":token(512),"commit":token(510)})).unwrap();
    assert!(Arc::ptr_eq(&f.s.project, &original));
    inputs(&mut f, 513, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    worker_command(&mut f, "agent.redo", json!({"token":token(513),"commit":token(510),"undo":token(512)})).unwrap();
    assert!(Arc::ptr_eq(&f.s.project, &changed));
}
#[test]
fn capsule_twelve_keeps_exact_unchanged_receipt_at_zero_or_normal_history_without_recreating_qualified_inputs() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for limit in [0, 128] {
        let (mut f, _, _) = ready("in", 520);
        release(&mut f, 520);
        f.s.history.limit = limit;
        if limit == 0 {
            f.s.history.undo.clear();
            f.s.history.redo.clear();
        }
        let seq = f.s.state.active_sequence.unwrap();
        let ids =
            f.s.project
                .sequence(seq)
                .unwrap()
                .video_tracks
                .iter()
                .flat_map(|t| &t.items)
                .filter_map(|c| c.identity.as_ref().map(|i| i.id.clone()))
                .collect::<Vec<_>>();
        inputs(&mut f, 521, json!({"kind":"timeline","sequence":seq.0,"identities":ids}));
        let doc = plan(&mut f, 521, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
        let request = commit_request(521, &doc);
        let result = ack(&mut f, request.clone());
        let stack = history(&f.s);
        let (saved, path) = saved_source(&mut f, 522);
        let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(raw["version"], 12);
        let t = raw["tickets"].as_array().unwrap().iter().find(|t| t["token"] == token(521)).unwrap();
        assert_eq!(t["project"], t["after"]);
        assert_eq!(t["multicam_history"]["before"], t["multicam_history"]["after"]);
        reopen_source(&mut f, 523, &saved);
        assert_eq!(history(&f.s), stack);
        let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(521)).unwrap();
        assert!(t.multicam_production.is_none());
        assert!(t.multicam_history.is_some());
        assert_eq!(t.media.scope_json()["libraryVerified"], false);
        let before = state(&f.s);
        assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), result);
        assert_eq!(state(&f.s), before);
        inputs(&mut f, 524, json!({"kind":"timeline","sequence":seq.0,"identities":ids}));
        assert_eq!(
            f.s.execute("agent.readHistory", json!({"token":token(524),"scope":scope(),"commit":token(521),"undo":null})).unwrap()["undoAvailable"],
            false
        );
    }
}
#[test]
fn older_capsules_and_rehashed_fake_history_revision_or_effect_claims_cannot_misrepresent_an_unchanged_intent() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut f, _, request) = ready("in", 530);
    ack(&mut f, request);
    let (saved, path) = saved_source(&mut f, 531);
    let original = fs::read(&path).unwrap();
    let raw: Value = serde_json::from_slice(&original).unwrap();
    let index = raw["tickets"].as_array().unwrap().iter().position(|t| t["token"] == token(530)).unwrap();
    let mut attacks = Vec::new();
    for version in 1..=11 {
        let mut v = raw.clone();
        v["version"] = json!(version);
        attacks.push(v);
    }
    for kind in ["effect", "undo", "revision", "after", "missing", "edge", "redo"] {
        let mut v = raw.clone();
        let t = &mut v["tickets"][index];
        match kind {
            "effect" => t["result"]["timelineCommits"] = json!(1),
            "undo" => t["result"]["undoOwned"] = json!(true),
            "revision" => t["result"]["revision"] = json!(t["revision"].as_u64().unwrap() + 1),
            "after" => t["after"] = json!(999999),
            "missing" => {
                t["result"].as_object_mut().unwrap().remove("outcome");
            }
            "edge" => t["multicam_history"]["after"] = json!(999999),
            "redo" => t["redo_consumed"] = json!(true),
            _ => unreachable!(),
        };
        attacks.push(v);
    }
    for v in attacks {
        let bytes = serde_json::to_vec(&v).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut fresh = Session::default();
        fresh.execute("file.open", json!({"path":f.s.path})).unwrap();
        fresh.execute("agent.readProject", json!({"token":token(532),"scope":scope(),"projectPath":fresh.path})).unwrap();
        let before = state(&fresh);
        assert!(fresh.execute("agent.restoreHistoryCheckpoint",json!({"token":token(532),"scope":scope(),"workspace":WORKSPACE,"checkpoint":saved["checkpoint"]["checkpoint"],"sha256":format!("{:x}",Sha256::digest(&bytes))})).is_err());
        assert_eq!(state(&fresh), before);
    }
}
#[test]
fn no_change_acknowledgment_still_requires_original_record_bytes_scope_current_editor_and_independent_stop() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in ["record", "bytes", "scope", "editor", "stop", "worker"] {
        let (mut f, _, mut request) = ready("in", 540);
        match mode {
            "record" => request["operations"][0]["params"]["recordSha256"] = json!("0".repeat(64)),
            "bytes" => {
                use std::io::Write;
                fs::OpenOptions::new().append(true).open(&f.files[0]).unwrap().write_all(b"changed source before unchanged completion").unwrap();
            }
            "scope" => request["operations"][0]["params"]["scope"]["projectId"] = json!("p_foreign"),
            "editor" => f.s.state.linked_selection = false,
            "stop" => {
                assert_eq!(f.s.agent_operation_control().cancel(&json!({"token":token(540)})).unwrap()["cancellationAccepted"], true);
            }
            "worker" => f.s.worker_available = false,
            _ => unreachable!(),
        };
        let before = state(&f.s);
        assert!(worker_command(&mut f, "agent.apply", request).is_err(), "{mode}");
        assert_eq!(state(&f.s), before);
    }
}
#[test]
fn original_physical_journal_preserves_unchanged_outcome_and_later_human_history_through_paused_cold_start() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut f, startup, _, baseline) = super::mixed::journal_source("synchronize", false);
    let seq = f.s.state.active_sequence.unwrap();
    let ids =
        f.s.project
            .sequence(seq)
            .unwrap()
            .video_tracks
            .iter()
            .flat_map(|t| &t.items)
            .filter_map(|c| c.identity.as_ref().map(|i| i.id.clone()))
            .collect::<Vec<_>>();
    inputs(&mut f, 550, json!({"kind":"timeline","sequence":seq.0,"identities":ids}));
    let doc = plan(&mut f, 550, json!({"gesture":"synchronize","options":{"method":"in","offset":2}})).unwrap();
    assert_eq!(doc["result"]["moved"], 0);
    let request = commit_request(550, &doc);
    let result = ack(&mut f, request.clone());
    f.s.execute("file.newBin", json!({"name":"Human work after unchanged intent"})).unwrap();
    f.s.sync_persistence();
    f.s.persistence.as_ref().unwrap().flush();
    f.s.apply_persistence_events();
    assert!(f.s.persistence.as_ref().unwrap().status.last_error.is_none());
    let dir = f.s.persistence.as_ref().unwrap().session_dir.clone();
    let raw = fs::read(dir.join("native-state.json")).unwrap();
    let envelope: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(envelope["payload"]["history"]["version"], 12);
    let project = f.s.project.clone();
    let stack = history(&f.s);
    let journal = f.s.journal.clone();
    f.s.persistence.take().unwrap().simulate_crash();
    let mut cold = Session::default();
    cold.open_project_recovery(&startup).unwrap();
    assert!(cold.is_display_only());
    assert_eq!(project_audit::bytes(&cold.project).unwrap(), project_audit::bytes(&project).unwrap());
    assert_eq!(history(&cold), stack);
    assert_eq!(cold.journal, journal);
    f.s = cold;
    super::mixed::activate(&mut f);
    let before = state(&f.s);
    assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), result);
    assert_eq!(state(&f.s), before);
    inputs(&mut f, 551, json!({"kind":"timeline","sequence":seq.0,"identities":ids}));
    assert_eq!(f.s.execute("agent.readHistory", json!({"token":token(551),"scope":scope(),"commit":token(550),"undo":null})).unwrap()["undoAvailable"], false);
    f.s.execute("edit.undo", json!({})).unwrap();
    f.s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&project).unwrap());
    assert_eq!(fs::read(f.s.path.as_ref().unwrap()).unwrap(), baseline);
    assert_eq!(fs::read(dir.join("native-state.json")).unwrap(), raw);
    f.s.shutdown();
}

#[test]
fn capsule_twelve_restores_prior_owned_source_head_across_no_change_and_keeps_only_the_real_native_undo_edge() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, ids) = timeline(&mut f);
    let original = project_audit::bytes(&f.s.project).unwrap();
    inputs(&mut f, 560, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 560, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    let real = worker_command(&mut f, "agent.apply", commit_request(560, &doc)).unwrap();
    let changed = project_audit::bytes(&f.s.project).unwrap();
    inputs(&mut f, 561, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 561, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    let request = commit_request(561, &doc);
    let nochange = ack(&mut f, request.clone());
    let (saved, path) = saved_source(&mut f, 562);
    let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(raw["version"], 12);
    assert_eq!(raw["owned_head"], true);
    reopen_source(&mut f, 563, &saved);
    assert_eq!(f.s.execute("agent.receipt", json!({"token":token(560)})).unwrap()["result"], real);
    let before = state(&f.s);
    assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), nochange);
    assert_eq!(state(&f.s), before);
    inputs(&mut f, 564, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    for (commit, available) in [(560, true), (561, false)] {
        assert_eq!(
            f.s.execute("agent.readHistory", json!({"token":token(564),"scope":scope(),"commit":token(commit),"undo":null})).unwrap()["undoAvailable"],
            available
        );
    }
    worker_command(&mut f, "agent.undo", json!({"token":token(564),"commit":token(560)})).unwrap();
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), original);
    inputs(&mut f, 565, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    worker_command(&mut f, "agent.redo", json!({"token":token(565),"commit":token(560),"undo":token(564)})).unwrap();
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), changed);
}
