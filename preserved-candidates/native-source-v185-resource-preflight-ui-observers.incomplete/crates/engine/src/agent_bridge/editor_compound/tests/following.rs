use super::*;
fn follow(s: &mut Session, n: usize) -> Value {
    s.execute(editor_control::PREPARE, json!({"token":token(n),"scope":scope(),"projectPath":s.path,"clockPolicy":"original_human_trim"})).unwrap()
}
fn operation(d: &Value, setting: Value) -> Value {
    let mut p = request_for(d, setting);
    p["version"] = json!(3);
    p
}
fn run_pair(s: &mut Session, doc: &Value, setting: Value) -> (Value, Value) {
    let p = operation(doc, setting);
    let mut human = twin(s);
    let typed: Request = parse(COMMAND, &p).unwrap();
    let (cmd, args) = editor_setting::human(&typed.setting);
    human.execute(cmd, args).unwrap();
    let current = project_plane::basis(s).unwrap();
    let editor = project_clear::editor_hash(&s.state).unwrap();
    let result = s.execute(COMMAND, p.clone()).unwrap();
    assert_eq!(state(s), state(&human));
    assert_eq!(result["version"], 3);
    assert_eq!(result["originalRead"], *doc);
    assert_eq!(result["publicationRead"]["projectState"]["sha256"], current);
    assert_eq!(result["publicationRead"]["editorStateSha256"], editor);
    assert_eq!(result["publication"]["beforeProjectSha256"], current);
    assert_eq!(result["publication"]["beforeEditorSha256"], editor);
    assert_eq!(result["publication"]["version"], 2);
    assert_eq!(result["agentTimelineCommits"], 0);
    assert_eq!(result["agentUndoOwned"], false);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["mediaEvidence"], false);
    assert!(project_audit::value_bytes(&result).unwrap().len() <= LIMIT);
    (p, result)
}
#[test]
fn following_defaults_use_the_current_original_human_trim_and_history_for_all_kinds_and_limits() {
    for kind in ["trim", "ripple", "roll"] {
        for limit in [0, 1, 128] {
            let mut s = setup(true);
            s.history.limit = limit;
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 192000 + limit);
            s.execute("trim.tick", json!({"clock":0.125})).unwrap();
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
            assert_ne!(d["projectState"]["sha256"], project_plane::basis(&s).unwrap());
            let (_, r) = run_pair(&mut s, &d, default());
            assert_eq!(r["publication"]["trim"]["dynamicCommitted"], true);
            assert_eq!(r["publication"]["trim"]["undoEntryRetained"], limit > 0);
            assert!(!s.trim_play.active());
            assert_eq!(s.history.undo.len(), usize::from(limit > 0));
        }
    }
}
#[test]
fn following_modes_share_the_original_human_command_and_preserve_all_live_trim_kinds_for_all_five_modes() {
    for kind in ["trim", "ripple", "roll"] {
        for mode in filmcraft_project::AutomationMode::ALL {
            let mut s = setup(true);
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 192200);
            s.execute("trim.tick", json!({"clock":0.25})).unwrap();
            let setting = super::mode(&s, mode.label());
            let (_, r) = run_pair(&mut s, &d, setting);
            assert!(s.trim_play.active());
            assert_eq!(r["publication"]["trimOwnerPreserved"], true);
            assert_eq!(r["publication"]["historyUnchanged"], true);
            assert!(s.history.undo.is_empty());
        }
    }
}
#[test]
fn following_default_stops_only_the_actual_original_loop_and_adds_no_timeline_commit() {
    let mut s = setup(true);
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let d = follow(&mut s, 192300);
    for time in [0.125, 0.25, 0.5] {
        s.execute("trim.tick", json!({"clock":time})).unwrap();
    }
    let (_, r) = run_pair(&mut s, &d, default());
    assert_eq!(r["publication"]["trim"]["loopStopped"], true);
    assert_eq!(r["publication"]["trim"]["dynamicCommitted"], false);
    assert!(s.history.undo.is_empty());
    assert!(!s.trim_play.active());
}
#[test]
fn following_zero_offset_and_repeated_ticks_keep_original_no_commit_semantics() {
    for kind in ["trim", "ripple", "roll"] {
        let mut s = setup(true);
        dynamic(&mut s, kind, false);
        let d = follow(&mut s, 192400);
        s.execute("trim.tick", json!({"clock":0.0})).unwrap();
        let (_, r) = run_pair(&mut s, &d, default());
        assert_eq!(r["publication"]["trim"]["dynamicCommitted"], false);
        assert!(s.history.undo.is_empty());
    }
}
#[test]
fn following_publication_rejects_unrelated_editor_clipboard_history_preference_and_owner_changes_without_replanning() {
    for case in 0..12 {
        let mut s = setup(true);
        s.state.selection_follows_playhead = true;
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 192500 + case);
        let p = operation(&d, default());
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        match case {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.history.limit = 1,
            2 => s.saved_revision += 1,
            3 => s.project = Arc::new((*s.project).clone()),
            4 => s.history.redo.push(("Later human".into(), s.project.clone())),
            5 => s.prefs.audio.automatch_time += 1.0,
            6 => s.state.selection.push(filmcraft_project::ClipId(u64::MAX)),
            7 => {
                let id = s.state.active_sequence.unwrap();
                s.state.playheads.insert(id, Tick(9007199254740993));
            }
            8 => s.trim_play.dynamic.as_mut().unwrap().speed = 2.0,
            9 => s.path = Some("/generated/foreign.fcproj".into()),
            10 => s.state.clipboard_markers.push(
                serde_json::from_value(json!({"id":1,"start":9007199254740993_i64,"duration":0,"name":"private","comment":"","color":"Iris","kind":"Comment"}))
                    .unwrap(),
            ),
            _ => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "case{case}");
        assert_eq!(state(&s), before);
    }
}

#[test]
fn following_request_must_keep_original_digests_scope_and_canonical_setting_even_after_real_frames() {
    for case in 0..10 {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 192600 + case);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let mut p = operation(&d, default());
        match case {
            0 => p["beforeSha256"] = json!(project_plane::basis(&s).unwrap()),
            1 => p["beforeEditorSha256"] = json!(project_clear::editor_hash(&s.state).unwrap()),
            2 => p["scope"]["workspaceId"] = json!(C),
            3 => p["setting"]["definition"] = json!("Constant Gain"),
            4 => p["setting"]["execute"] = json!("edit.undo"),
            5 => p["execute"] = json!("file.open"),
            6 => p["version"] = json!(2),
            7 => p["writeAuthority"] = json!(true),
            8 => p["token"] = json!(token(192699)),
            _ => p["setting"]["kind"] = json!("command"),
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "hostile{case}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn following_write_never_borrows_ordinary_strict_readonly_or_source_ticket_purposes() {
    for case in 0..4 {
        let mut s = setup(true);
        let d = match case {
            0 => root(&mut s, 192700),
            1 => prepare_read(&mut s, 192701),
            2 => s.execute(editor_control::READ, json!({"token":token(192702),"scope":scope(),"projectPath":s.path})).unwrap(),
            _ => {
                let d = follow(&mut s, 192703);
                s.agent_bridge.tickets.back_mut().unwrap().source_library = Some(vec![1]);
                d
            }
        };
        let p = operation(&d, default());
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn following_original_outcome_survives_lost_reply_and_later_human_work_without_any_replay() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 192800);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    s.execute("effects.setDefaultTransition", json!({"effect":"cross_dissolve"})).unwrap();
    let later = state(&s);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(state(&s), later);
    let mut foreign = p.clone();
    foreign["setting"]["definition"] = json!("cross_dissolve");
    assert!(s.execute(COMMAND, foreign).is_err());
    assert_eq!(state(&s), later);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], false);
}
#[test]
fn following_independent_stop_and_missing_transport_refuse_before_the_shared_publication_fence() {
    for missing in [false, true] {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 192900 + usize::from(missing));
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let p = operation(&d, default());
        if missing {
            s.agent_bridge.operations = crate::agent_bridge::operation_control::AgentOperationControl::default();
        } else {
            assert_eq!(s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], true);
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn following_outcome_is_included_in_complete_original_and_publication_capsule() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 193000);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (_, result) = run_pair(&mut s, &d, default());
    let saved = save(&mut s, 193001);
    let raw: Value = serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 7);
    let tickets = raw["tickets"].as_array().unwrap();
    let t = tickets.iter().find(|t| t["token"] == d["token"]).unwrap();
    assert_eq!(t["result"], result);
    assert_eq!(t["editor_control"]["version"], 2);
    assert!(t["editor_control"]["original_snapshots"].as_array().is_some());
    assert!(saved["checkpoint"]["sha256"].is_string());
}
#[test]
fn following_receipt_and_ticket_keep_strong_original_and_publication_history_roots_and_exact_native_editor_proofs() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let original = s.project.clone();
    let original_editor = project_clear::editor_hash(&s.state).unwrap();
    let d = follow(&mut s, 193100);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let publication = s.project.clone();
    let (_, r) = run_pair(&mut s, &d, default());
    assert!(!Arc::ptr_eq(&original, &publication));
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == d["token"]).unwrap();
    assert!(t.project.ptr_eq(&Arc::downgrade(&publication)));
    let retained = super::super::retained(t).unwrap();
    let initial = retained.original.as_ref().unwrap();
    let initial_history = initial.evidence.validate(&initial.projects, "effects.setDefaultTransition").unwrap();
    assert!(Arc::ptr_eq(initial_history.before_project(), &original));
    assert!(retained.projects.iter().any(|p| Arc::ptr_eq(p, &publication)));
    let editor = editor_proof::decode(&r["originalEditorProof"], "native_following_editor_basis", LIMIT, COMMAND).unwrap();
    assert_eq!(project_clear::editor_hash_with(&editor.before, &editor.clipboard_sha256).unwrap(), original_editor);
    let current_editor = editor_proof::decode(&r["publication"]["editorProof"], EDITOR_KIND, LIMIT, COMMAND).unwrap();
    editor_proof::require_after(&editor.after_editor, &current_editor.before, COMMAND).unwrap();
    assert_eq!(initial.proof_command("effects.setDefaultTransition", None).unwrap(), r["originalHistoryProof"]);
}

mod history;

mod outcome;
