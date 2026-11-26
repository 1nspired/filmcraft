use super::*;
const OUTCOME: &str = crate::agent_bridge::editor_control::OUTCOME;
fn args(p: &Value, r: &Value) -> Value {
    json!({"version":1,"token":p["token"],"scope":scope(),"receiptSha256":history_checkpoint::digest(&project_audit::value_bytes(r).unwrap())})
}
fn read(s: &mut Session, p: &Value, r: &Value) -> Value {
    let before = state(s);
    let count = s.agent_bridge.tickets.len();
    let v = s.execute(OUTCOME, args(p, r)).unwrap();
    assert_eq!(state(s), before);
    assert_eq!(s.agent_bridge.tickets.len(), count);
    assert_eq!(v["current"]["version"], 1);
    assert_eq!(v["current"]["projectState"]["sha256"], project_plane::basis(s).unwrap());
    assert_eq!(v["current"]["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
    assert_eq!(v["currentHistory"], super::super::super::history_state(&s.project, &s.history).unwrap());
    assert_eq!(v["newReadTicket"], false);
    assert_eq!(v["writeAuthority"], false);
    assert_eq!(v["agentUndoOwned"], false);
    assert_eq!(v["agentTimelineCommits"], 0);
    assert_eq!(v["mediaEvidence"], false);
    v
}
#[test]
fn warm_mode_outcomes_follow_all_three_original_trim_clocks_and_all_five_modes_across_real_frames() {
    for kind in ["trim", "ripple", "roll"] {
        for m in filmcraft_project::AutomationMode::ALL {
            let mut s = setup(true);
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 195000);
            s.execute("trim.tick", json!({"clock":0.25})).unwrap();
            let setting = super::super::mode(&s, m.label());
            let (p, r) = run_pair(&mut s, &d, setting);
            for time in [0.5, 0.75, 1.0] {
                s.execute("trim.tick", json!({"clock":time})).unwrap();
                let v = read(&mut s, &p, &r);
                assert_eq!(v["verification"], "same_clock_lineage");
                assert_eq!(v["target"]["mode"], m.label());
                assert_eq!(v["target"]["matchesRequested"], true);
                assert_ne!(v["currentHistory"]["sha256"], r["publication"]["afterHistory"]["sha256"]);
            }
        }
    }
}
#[test]
fn warm_defaults_project_the_original_settled_history_for_each_kind_and_undo_limit() {
    for kind in ["trim", "ripple", "roll"] {
        for limit in [0, 1, 128] {
            let mut s = setup(true);
            s.history.limit = limit;
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 195100);
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
            let (p, r) = run_pair(&mut s, &d, default());
            let v = read(&mut s, &p, &r);
            assert_eq!(v["verification"], "same_current_basis");
            assert_eq!(v["currentHistory"], r["publication"]["afterHistory"]);
            assert_eq!(v["target"]["definition"], "constant_gain");
            assert!(!s.trim_play.active());
        }
    }
}
#[test]
fn warm_loop_modes_follow_the_actual_loop_while_defaults_leave_no_loop_owner() {
    for preserve in [false, true] {
        let mut s = setup(true);
        select(&mut s, "roll");
        s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
        let d = follow(&mut s, 195200 + usize::from(preserve));
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let setting = if preserve { super::super::mode(&s, "Touch") } else { default() };
        let (p, r) = run_pair(&mut s, &d, setting);
        if preserve {
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
        }
        let v = read(&mut s, &p, &r);
        assert_eq!(v["verification"], if preserve { "same_clock_lineage" } else { "same_current_basis" });
        assert_eq!(s.trim_play.active(), preserve);
    }
}
#[test]
fn warm_readback_discloses_nonclock_editor_history_preferences_saved_revision_and_project_replacement_changes() {
    for case in 0..7 {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 195300 + case);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let setting = super::super::mode(&s, "Write");
        let (p, r) = run_pair(&mut s, &d, setting);
        match case {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.prefs.audio.automatch_time += 1.0,
            2 => s.history.limit = 1,
            3 => s.saved_revision += 1,
            4 => s.project = Arc::new((*s.project).clone()),
            5 => s.history.redo.push(("later human".into(), s.project.clone())),
            _ => {
                let track = s.active_sequence().unwrap().audio_tracks[0].id;
                s.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Read"})).unwrap();
            }
        }
        let v = read(&mut s, &p, &r);
        assert_eq!(v["verification"], "changed_since_setting");
    }
}
#[test]
fn warm_readback_refuses_restarted_ended_or_reidentified_native_clock_ownership_even_when_target_matches() {
    for case in 0..3 {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 195400 + case);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let setting = super::super::mode(&s, "Touch");
        let (p, r) = run_pair(&mut s, &d, setting);
        if case == 2 {
            s.agent_bridge.epoch = Some("f".repeat(32));
        } else {
            s.execute("trim.shuttleStop", json!({})).unwrap();
            if case == 1 {
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
        }
        let v = read(&mut s, &p, &r);
        assert_eq!(v["verification"], "changed_since_setting");
        assert_eq!(v["target"]["matchesRequested"], true);
    }
}
#[test]
fn outcome_reads_remain_documentary_and_cannot_pay_a_new_writer_source_or_undo_hand() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 195500);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    let v = read(&mut s, &p, &r);
    let before = state(&s);
    let mut changed = operation(&v["current"], json!({"kind":"default_transition","definition":"cross_dissolve"}));
    changed["version"] = json!(3);
    assert!(s.execute(COMMAND, changed).is_err());
    for cmd in ["agent.apply", "agent.undo", "agent.readSourceView", "agent.readTracks"] {
        assert!(s.execute(cmd, json!({"token":p["token"]})).is_err());
    }
    assert_eq!(state(&s), before);
}

#[test]
fn outcome_queries_bind_complete_original_receipt_scope_and_strict_request_shape() {
    for case in 0..6 {
        let mut s = setup(true);
        let d = follow(&mut s, 195600 + case);
        let (p, r) = run_pair(&mut s, &d, default());
        let mut a = args(&p, &r);
        match case {
            0 => a["receiptSha256"] = json!("0".repeat(64)),
            1 => a["scope"]["workspaceId"] = json!(C),
            2 => a["version"] = json!(2),
            3 => a["execute"] = json!("edit.undo"),
            4 => a["token"] = json!(token(195699)),
            _ => a["receiptSha256"] = json!("bad"),
        };
        let before = state(&s);
        assert!(s.execute(OUTCOME, a).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn old_or_uncommitted_control_purposes_cannot_acquire_following_outcome_authority() {
    for case in 0..3 {
        let mut s = setup(true);
        let token_id = 195700 + case;
        let d = if case == 0 {
            prepare_read(&mut s, token_id)
        } else if case == 1 {
            follow(&mut s, token_id)
        } else {
            root(&mut s, token_id)
        };
        let receipt = if case == 0 { s.execute(COMMAND, request_for(&d, default())).unwrap() } else { json!({}) };
        let p = json!({"token":d["token"]});
        let before = state(&s);
        assert!(s.execute(OUTCOME, args(&p, &receipt)).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn readonly_outcome_queries_never_replay_or_replace_the_retained_setting_or_its_stop_state() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 195800);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let setting = super::super::mode(&s, "Touch");
    let (p, r) = run_pair(&mut s, &d, setting);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    read(&mut s, &p, &r);
    read(&mut s, &p, &r);
    let before = state(&s);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(state(&s), before);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], false);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
    assert_eq!(state(&s), before);
}
#[test]
fn cold_readback_can_describe_an_exact_idle_match_but_never_recreates_warm_clock_lineage() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 195900);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    let editor = s.state.clone();
    let saved = save(&mut s, 195901);
    let mut cold = reopened(&s);
    restore(&mut cold, 195902, &saved).unwrap();
    // An explicit fixture current editor matches the past after-state. It creates
    // no private playback owner, and is not a normal-constructor acceptance claim.
    cold.state = editor;
    let v = read(&mut cold, &p, &r);
    assert_eq!(v["warmBinding"], false);
    assert_eq!(v["verification"], "exact_idle_current");
    cold.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
    let v = read(&mut cold, &p, &r);
    assert_eq!(v["warmBinding"], false);
    assert_eq!(v["verification"], "historical_only");
    assert_eq!(v["target"]["matchesRequested"], true);
}
#[test]
fn cold_mode_outcome_after_later_human_trim_commit_is_historical_even_when_current_target_matches() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 196000);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let setting = super::super::mode(&s, "Touch");
    let (p, r) = run_pair(&mut s, &d, setting);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    s.execute("trim.shuttleStop", json!({})).unwrap();
    let saved = save(&mut s, 196001);
    let editor = s.state.clone();
    let mut cold = reopened(&s);
    restore(&mut cold, 196002, &saved).unwrap();
    cold.state = editor;
    let v = read(&mut cold, &p, &r);
    assert_eq!(v["verification"], "historical_only");
    assert_eq!(v["warmBinding"], false);
    assert_eq!(v["target"]["matchesRequested"], true);
}
#[test]
fn warm_metadata_readback_preserves_the_current_human_recording_and_preview_without_claiming_media_output() {
    for recording in [false, true] {
        let mut s = setup(true);
        s.set_playhead(Tick::ZERO);
        let track = s.active_sequence().unwrap().audio_tracks[0].id;
        if recording {
            s.execute("mixer.recordStart", json!({"time":0})).unwrap();
        } else {
            s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":0})).unwrap();
        }
        let d = follow(&mut s, 196100 + usize::from(recording));
        let p = operation(&d, super::super::mode(&s, "Touch"));
        let r = s.execute(COMMAND, p.clone()).unwrap();
        let before = state(&s);
        let v = read(&mut s, &p, &r);
        assert_eq!(v["verification"], "same_current_basis");
        assert_eq!(state(&s), before);
        assert_eq!(s.mixrec.active(), recording);
        assert_eq!(s.previews.live.is_active(), !recording);
        assert_eq!(v["sourceValidation"], "not-performed");
    }
}
