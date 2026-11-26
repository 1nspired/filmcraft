use super::tests::{exact_batch, read, token};
use serde_json::{Value, json};
use std::sync::Arc;

fn trim(s: &crate::Session) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    json!({"command":"timeline.trim","params":{"clip":c.id.0,"edge":"out","mode":"regular","deltaFrames":-1}})
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"revision":s.revision,"saved":s.saved_revision,"editor":s.state,
        "undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal,"events":s.events.len()})
}
fn applied() -> crate::Session {
    let mut s = super::condense_tests::setup();
    read(&mut s, 26101);
    let op = trim(&s);
    s.execute("agent.apply", exact_batch(26101, vec![op])).unwrap();
    s
}
fn undo(s: &mut crate::Session, n: usize, commit: usize) {
    read(s, n);
    s.execute("agent.undo", json!({"token":token(n),"commit":token(commit)})).unwrap();
}
fn redo_request(n: usize, commit: usize, undo: usize) -> Value {
    json!({"token":token(n),"commit":token(commit),"undo":token(undo)})
}

fn history_request(s: &crate::Session, n: usize, commit: usize, undo: Option<usize>) -> Value {
    json!({"token":token(n),"commit":token(commit),"undo":undo.map(token),
        "scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0}})
}

fn unchanged_history_read(s: &mut crate::Session, request: Value) -> crate::Result<Value> {
    let before = state(s);
    let project = s.project.clone();
    let tickets = s.agent_bridge.tickets.len();
    let result = s.execute("agent.readHistory", request);
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    result
}

#[test]
fn guarded_history_read_shares_actual_undo_redo_guards_without_consuming_ownership() {
    let mut s = applied();
    read(&mut s, 26701);
    let request = history_request(&s, 26701, 26101, None);
    let result = unchanged_history_read(&mut s, request.clone()).unwrap();
    assert_eq!(result["undoAvailable"], true);
    assert_eq!(result["redoAvailable"], false);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["ownership"], "live-native-session");
    assert_eq!(unchanged_history_read(&mut s, request).unwrap(), result);
    // The read ticket is still unused by a write.
    s.execute("agent.undo", json!({"token":token(26701),"commit":token(26101)})).unwrap();
    read(&mut s, 26702);
    let request = history_request(&s, 26702, 26101, Some(26701));
    let result = unchanged_history_read(&mut s, request).unwrap();
    assert_eq!(result["undoAvailable"], false);
    assert_eq!(result["redoAvailable"], true);
    s.execute("agent.redo", redo_request(26702, 26101, 26701)).unwrap();
    read(&mut s, 26703);
    let request = history_request(&s, 26703, 26101, Some(26701));
    let result = unchanged_history_read(&mut s, request).unwrap();
    assert_eq!(result["undoAvailable"], true);
    assert_eq!(result["redoAvailable"], false);
    undo(&mut s, 26704, 26101);
    read(&mut s, 26705);
    let request = history_request(&s, 26705, 26101, Some(26701));
    assert_eq!(unchanged_history_read(&mut s, request).unwrap()["redoAvailable"], false, "used Undo cannot own another cycle");
    let request = history_request(&s, 26705, 26101, Some(26704));
    assert_eq!(unchanged_history_read(&mut s, request).unwrap()["redoAvailable"], true);
}

#[test]
fn projected_history_uses_exact_live_context_and_never_adopts_or_consumes_history() {
    let mut s = applied();
    read(&mut s, 29701);
    let mut request = history_request(&s, 29701, 26101, None);
    let mut expected = unchanged_history_read(&mut s, request.clone()).unwrap();
    expected["requiresSourceValidation"] = json!(true);
    request["projection"] = json!(true);
    assert_eq!(unchanged_history_read(&mut s, request.clone()).unwrap(), expected);
    for invalid in [Value::Null, json!("true"), json!(1), json!({})] {
        let mut hostile = request.clone();
        hostile["projection"] = invalid;
        assert!(unchanged_history_read(&mut s, hostile).is_err());
    }
    s.state.linked_selection = !s.state.linked_selection;
    assert!(unchanged_history_read(&mut s, request.clone()).is_err(), "pure ownership reads must still check editor context");
    s.state.linked_selection = !s.state.linked_selection;
    s.execute("agent.undo", json!({"token":token(29701),"commit":token(26101)})).unwrap();
    read(&mut s, 29702);
    let mut request = history_request(&s, 29702, 26101, Some(29701));
    request["projection"] = json!(true);
    assert_eq!(unchanged_history_read(&mut s, request.clone()).unwrap()["redoAvailable"], true);
    s.project = Arc::new((*s.project).clone());
    assert!(unchanged_history_read(&mut s, request).is_err(), "byte-identical document replacement cannot renew a projection");
}

#[test]
fn projected_history_is_provisional_when_original_changes_and_final_undo_still_refuses() {
    let mut s = super::condense_tests::setup();
    let root = std::env::temp_dir()
        .join(format!("filmcraft-history-projection-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _scratch = Scratch(root.clone());
    std::fs::create_dir(&root).unwrap();
    let file = root.join("original.bin");
    std::fs::write(&file, b"original input").unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: file.to_string_lossy().into_owned() };
    read(&mut s, 29710);
    let op = trim(&s);
    s.execute("agent.apply", exact_batch(29710, vec![op])).unwrap();
    read(&mut s, 29711);
    let legacy = history_request(&s, 29711, 29710, None);
    let mut projection = legacy.clone();
    projection["projection"] = json!(true);
    let expected = unchanged_history_read(&mut s, projection.clone()).unwrap();
    assert_eq!(expected["undoAvailable"], true);
    assert_eq!(expected["requiresSourceValidation"], true);
    std::fs::write(&file, b"replaced input").unwrap();
    assert_eq!(unchanged_history_read(&mut s, projection).unwrap(), expected, "metadata is provisional until background qualification");
    assert!(unchanged_history_read(&mut s, legacy).is_err());
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(29711)})).is_err());
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(29711),"commit":token(29710)})).is_err());
    assert_eq!(state(&s), before, "a history projection cannot authorize Undo against changed originals");
}

#[test]
fn guarded_history_read_reports_human_changed_expired_and_unknown_heads_without_adoption() {
    for mode in ["human-edit", "human-redo", "human-redo-undo", "replacement", "path", "expired", "unknown", "overflow", "fresh-session"] {
        let mut s = applied();
        undo(&mut s, 26801, 26101);
        match mode {
            "human-edit" => {
                s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
            }
            "human-redo" => {
                s.redo().unwrap();
            }
            "human-redo-undo" => {
                s.redo().unwrap();
                s.undo().unwrap();
            }
            "replacement" => {
                s.project = Arc::new((*s.project).clone());
            }
            "path" => {
                s.path = Some("/generated/other.fcproj".into());
            }
            "expired" => {
                for n in 0..super::TICKET_LIMIT + 1 {
                    read(&mut s, 26900 + n);
                }
            }
            "overflow" => {
                s.revision = u64::MAX - 1;
                s.agent_bridge.head = Some((s.revision, Arc::downgrade(&s.project)));
            }
            "fresh-session" => {
                s.agent_bridge = Default::default();
            }
            _ => {}
        }
        read(&mut s, 26802);
        let request = history_request(&s, 26802, if mode == "unknown" { 99999 } else { 26101 }, Some(26801));
        let result = unchanged_history_read(&mut s, request).unwrap();
        assert_eq!(result["undoAvailable"], false, "{mode}");
        assert_eq!(result["redoAvailable"], false, "{mode}");
    }
}

#[test]
fn guarded_history_read_rejects_stale_foreign_and_hostile_requests_before_any_change() {
    for mode in ["stale", "foreign-film", "foreign-sequence", "missing-undo", "bad-undo", "alias", "extra", "scope-extra", "consumed", "unknown-read"] {
        let mut s = applied();
        read(&mut s, 27001);
        let mut request = history_request(&s, 27001, 26101, None);
        match mode {
            "stale" => {
                s.state.linked_selection = !s.state.linked_selection;
            }
            "foreign-film" => {
                request["scope"]["projectId"] = json!("p_other");
            }
            "foreign-sequence" => {
                request["scope"]["anchorSequence"] = json!(99999);
            }
            "missing-undo" => {
                request.as_object_mut().unwrap().remove("undo");
            }
            "bad-undo" => {
                request["undo"] = json!(7);
            }
            "alias" => {
                request["commit"] = request["token"].clone();
            }
            "extra" => {
                request["adopt"] = json!(true);
            }
            "scope-extra" => {
                request["scope"]["recover"] = json!(true);
            }
            "consumed" => {
                let op = trim(&s);
                s.execute("agent.apply", exact_batch(27001, vec![op])).unwrap();
            }
            "unknown-read" => {
                request["token"] = json!(token(99999));
            }
            _ => {}
        }
        assert!(unchanged_history_read(&mut s, request).is_err(), "{mode}");
    }
}

#[test]
fn guarded_redo_restores_exact_native_snapshot_receipts_once_and_supports_undo_again() {
    let mut s = applied();
    let edited = s.project.clone();
    undo(&mut s, 26102, 26101);
    let before = s.project.clone();
    read(&mut s, 26103);
    let revision = s.revision;
    let request = redo_request(26103, 26101, 26102);
    let r = s.execute("agent.redo", request.clone()).unwrap();
    assert_eq!(r["action"], "redo");
    assert_eq!(r["results"][0]["redone"], token(26101));
    assert_eq!(r["results"][0]["undo"], token(26102));
    assert_eq!(s.revision, revision + 1);
    assert!(Arc::ptr_eq(&s.project, &edited));
    assert!(s.history.redo.is_empty());
    let done = state(&s);
    assert_eq!(s.execute("agent.redo", request).unwrap(), r);
    assert_eq!(state(&s), done);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(26103)})).unwrap()["result"], r);
    undo(&mut s, 26104, 26101);
    assert!(Arc::ptr_eq(&s.project, &before));
    read(&mut s, 26105);
    let previous = state(&s);
    assert!(s.execute("agent.redo", redo_request(26105, 26101, 26102)).is_err(), "old already-consumed Undo cannot own a new Redo");
    assert_eq!(state(&s), previous);
    s.execute("agent.redo", redo_request(26105, 26101, 26104)).unwrap();
    assert!(Arc::ptr_eq(&s.project, &edited));
}

#[test]
fn guarded_redo_follows_multiple_owned_undos_without_replaying_edits() {
    let mut s = applied();
    let first = s.project.clone();
    read(&mut s, 26201);
    let op = trim(&s);
    s.execute("agent.apply", exact_batch(26201, vec![op])).unwrap();
    let second = s.project.clone();
    undo(&mut s, 26202, 26201);
    undo(&mut s, 26203, 26101);
    read(&mut s, 26204);
    s.execute("agent.redo", redo_request(26204, 26101, 26203)).unwrap();
    assert!(Arc::ptr_eq(&s.project, &first));
    read(&mut s, 26205);
    s.execute("agent.redo", redo_request(26205, 26201, 26202)).unwrap();
    assert!(Arc::ptr_eq(&s.project, &second));
    undo(&mut s, 26206, 26201);
    assert!(Arc::ptr_eq(&s.project, &first));
}

#[test]
fn guarded_redo_rejections_preserve_current_human_state_and_both_history_branches() {
    for mode in [
        "human-edit",
        "human-redo",
        "human-redo-undo",
        "replacement",
        "stale-context",
        "path",
        "expired",
        "wrong-commit",
        "wrong-undo",
        "not-undo",
        "extra",
        "overflow",
    ] {
        let mut s = applied();
        undo(&mut s, 26301, 26101);
        match mode {
            "human-edit" => {
                s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
            }
            "human-redo" => {
                s.redo().unwrap();
            }
            "human-redo-undo" => {
                s.redo().unwrap();
                s.undo().unwrap();
            }
            "replacement" => {
                s.project = Arc::new((*s.project).clone());
            }
            "path" => {
                s.path = Some("/generated/different.fcproj".into());
            }
            "expired" => {
                for n in 0..super::TICKET_LIMIT + 1 {
                    read(&mut s, 26400 + n);
                }
            }
            "overflow" => {
                s.revision = u64::MAX - 1;
                s.agent_bridge.head = Some((s.revision, Arc::downgrade(&s.project)));
            }
            _ => {}
        }
        read(&mut s, 26302);
        if mode == "stale-context" {
            s.state.linked_selection = !s.state.linked_selection;
        }
        let mut r = redo_request(26302, 26101, 26301);
        if mode == "wrong-commit" {
            r["commit"] = json!(token(99999));
        }
        if mode == "wrong-undo" {
            r["undo"] = json!(token(99999));
        }
        if mode == "not-undo" {
            r["undo"] = json!(token(26101));
        }
        if mode == "extra" {
            r["force"] = json!(true);
        }
        let before = state(&s);
        let project = s.project.clone();
        assert!(s.execute("agent.redo", r).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
        assert!(Arc::ptr_eq(&s.project, &project));
    }
}

#[test]
fn guarded_redo_sequence_creation_restores_active_sequence_and_native_event() {
    let mut s = super::condense_tests::setup();
    let anchor = s.state.active_sequence.unwrap();
    read(&mut s, 26501);
    let op = json!({"command":"agent.createSequence","params":{"name":"Redo sequence","scope":{"projectId":"p_test","anchorSequence":anchor.0}}});
    s.execute("agent.apply", exact_batch(26501, vec![op])).unwrap();
    let created = s.state.active_sequence.unwrap();
    let project = s.project.clone();
    undo(&mut s, 26502, 26501);
    assert_eq!(s.state.active_sequence, Some(anchor));
    read(&mut s, 26503);
    s.events.clear();
    s.execute("agent.redo", redo_request(26503, 26501, 26502)).unwrap();
    assert_eq!(s.state.active_sequence, Some(created));
    assert!(Arc::ptr_eq(&s.project, &project));
    assert!(s.events.iter().any(|e| matches!(e,crate::Event::OpenSequence(id) if *id==created)));
    undo(&mut s, 26504, 26501);
    assert_eq!(s.state.active_sequence, Some(anchor));
}

#[test]
fn guarded_redo_journal_uses_shared_native_redo_but_reopen_does_not_invent_ticket_ownership() {
    let mut s = super::condense_tests::setup();
    let base = s.project.clone();
    let state = s.state.clone();
    read(&mut s, 26601);
    let op = trim(&s);
    s.execute("agent.apply", exact_batch(26601, vec![op])).unwrap();
    undo(&mut s, 26602, 26601);
    read(&mut s, 26603);
    s.execute("agent.redo", redo_request(26603, 26601, 26602)).unwrap();
    assert_eq!(s.journal.last().unwrap().0, "edit.redo");
    let mut recovered = crate::Session::new(s.services.clone());
    recovered.project = base;
    recovered.state = state;
    recovered.media = s.media.clone();
    for (command, args) in &s.journal {
        recovered.execute(command, args.clone()).unwrap();
    }
    assert_eq!(filmcraft_format::encode(&s.project, false), filmcraft_format::encode(&recovered.project, false));
    let decoded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
    let mut reopened = crate::Session::new(s.services.clone());
    reopened.project = Arc::new(decoded.project);
    reopened.state = s.state.clone();
    read(&mut reopened, 26604);
    assert!(reopened.execute("agent.redo", redo_request(26604, 26601, 26602)).is_err());
}
