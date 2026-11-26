use super::*;
fn prepare_follow(s: &mut Session, n: usize) -> Value {
    let mut p = args(s, n);
    p["clockPolicy"] = json!("original_human_trim");
    s.execute(PREPARE, p).unwrap()
}
fn follow_page(d: &Value) -> Value {
    let mut p = page(d);
    p["version"] = json!(2);
    p
}
fn check_basis(s: &Session, original: &Value, result: &Value) {
    assert_eq!(result["version"], 2);
    assert_eq!(result["followingBasis"]["original"], *original);
    let current = &result["followingBasis"]["current"];
    assert_eq!(current["version"], 3);
    assert_eq!(current["clockPolicy"], "original_human_trim");
    assert_eq!(current["readPurpose"], "editor_setting_clock_following");
    assert_eq!(current["projectState"]["sha256"], project_plane::basis(s).unwrap());
    assert_eq!(current["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
    assert_eq!(current["revision"], s.revision);
    assert_eq!(current["historyState"], crate::agent_bridge::editor_compound::history_state(&s.project, &s.history).unwrap());
    assert_eq!(result["editorStateSha256"], current["editorStateSha256"]);
    assert_eq!(result["revision"], current["revision"]);
    assert_eq!(current["capabilities"]["compoundEditorWrite"], true);
    assert_eq!(current["writeAuthority"], false);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["mediaEvidence"], false);
    assert!(project_audit::value_bytes(result).unwrap().len() <= 64 * 1024);
    let p = command_settlement::prepare(s, DEFAULT).unwrap();
    assert_eq!(current["trim"]["settlement"]["offsetTicks"], p.summary().offset.0.to_string());
}
#[test]
fn following_policy_is_explicit_native_preparation_only_and_keeps_old_reads_exact() {
    let mut s = setup(true);
    let before = state(&s);
    let old = capture(&mut s, 190000);
    let p = args(&s, 190001);
    let strict = s.execute(PREPARE, p).unwrap();
    assert_eq!(old["version"], 1);
    assert_eq!(strict["version"], 2);
    assert!(old.get("clockPolicy").is_none() && strict.get("clockPolicy").is_none());
    for (i, policy) in [json!(null), json!(false), json!("all_clocks"), json!({})].into_iter().enumerate() {
        let mut p = args(&s, 190010 + i);
        p["clockPolicy"] = policy;
        assert!(s.execute(PREPARE, p).is_err());
    }
    let mut p = args(&s, 190020);
    p["clockPolicy"] = json!("original_human_trim");
    assert!(s.execute(READ, p).is_err());
    let d = prepare_follow(&mut s, 190021);
    assert_eq!(d["version"], 3);
    assert_eq!(d["capabilities"]["compoundEditorWrite"], true);
    assert_eq!(state(&s), before);
    let mut missing = Session::default();
    missing.execute("file.newProject", json!({"name":"No independent transport","workspace":scope()})).unwrap();
    missing.path = s.path.clone();
    let mut p = args(&missing, 190022);
    p["clockPolicy"] = json!("original_human_trim");
    assert!(missing.execute(PREPARE, p).is_err());
}
#[test]
fn following_settings_and_catalog_observe_all_real_dynamic_kinds_without_settling_or_refreshing_the_original_read() {
    for (i, kind) in ["trim", "ripple", "roll"].into_iter().enumerate() {
        let mut s = setup(true);
        dynamic(&mut s, kind, false);
        let doc = prepare_follow(&mut s, 190100 + i);
        for clock in [0.125, 0.25, 0.5] {
            s.execute("trim.tick", json!({"clock":clock})).unwrap();
            let before = state(&s);
            let project = s.project.clone();
            for cmd in [editor_settings::READ, editor_setting::CATALOG] {
                let result = s.execute(cmd, follow_page(&doc)).unwrap();
                check_basis(&s, &doc, &result);
                assert_eq!(state(&s), before);
                assert!(Arc::ptr_eq(&project, &s.project));
                assert!(s.agent_bridge.tickets.iter().find(|t| t.token == doc["token"]).unwrap().committed.is_none());
            }
        }
    }
}
#[test]
fn following_loop_pages_keep_both_exact_bases_and_refuse_the_ended_or_restarted_owner() {
    for once in [false, true] {
        let mut s = setup(true);
        s.execute("trim.playAround", json!({"clock":0.0,"loop":!once})).unwrap();
        let d = prepare_follow(&mut s, 190200 + usize::from(once));
        for time in [0.1, 0.25, 0.25, 0.5] {
            s.execute("trim.tick", json!({"clock":time})).unwrap();
            let before = state(&s);
            let p = s.execute(editor_settings::READ, follow_page(&d)).unwrap();
            check_basis(&s, &d, &p);
            assert_eq!(state(&s), before);
        }
        if once {
            s.execute("trim.tick", json!({"clock":1000.0})).unwrap();
        } else {
            s.execute("trim.playAround", json!({"toggle":true})).unwrap();
            s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
        }
        let before = state(&s);
        assert!(s.execute(editor_settings::READ, follow_page(&d)).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn strict_and_following_metadata_purposes_never_promote_each_other_even_before_the_first_tick() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let f = prepare_follow(&mut s, 190300);
    let p = args(&s, 190301);
    let old = s.execute(PREPARE, p).unwrap();
    let before = state(&s);
    for cmd in [editor_settings::READ, editor_setting::CATALOG] {
        assert!(s.execute(cmd, page(&f)).is_err());
        assert!(s.execute(cmd, follow_page(&old)).is_err());
    }
    assert_eq!(state(&s), before);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    for cmd in [editor_settings::READ, editor_setting::CATALOG] {
        assert!(s.execute(cmd, page(&old)).is_err());
        let r = s.execute(cmd, follow_page(&f)).unwrap();
        check_basis(&s, &f, &r);
    }
}
#[test]
fn following_pages_reject_foreign_dependencies_and_do_not_bless_selection_erased_by_a_repeated_tick() {
    for change in 0..8 {
        let mut s = setup(true);
        s.state.selection_follows_playhead = true;
        dynamic(&mut s, "roll", false);
        let d = prepare_follow(&mut s, 190400 + change);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        match change {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.prefs.audio.automatch_time += 1.0,
            2 => s.saved_revision += 1,
            3 => s.history.limit = 1,
            4 => s.project = Arc::new((*s.project).clone()),
            5 => s.state.selection.push(filmcraft_project::ClipId(u64::MAX)),
            6 => {
                let id = s.state.active_sequence.unwrap();
                s.state.playheads.insert(id, Tick(9007199254740993));
            }
            _ => s.path = Some("/generated/other.fcproj".into()),
        }
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let before = state(&s);
        for cmd in [editor_settings::READ, editor_setting::CATALOG] {
            assert!(s.execute(cmd, follow_page(&d)).is_err());
            assert_eq!(state(&s), before);
        }
    }
}
#[test]
fn following_preparation_grants_no_old_setting_write_checkpoint_undo_source_or_timeline_hand() {
    let mut s = setup(true);
    let d = prepare_follow(&mut s, 190500);
    let before = state(&s);
    for cmd in [
        CONFIGURE,
        editor_setting::COMMAND,
        "agent.apply",
        "agent.undo",
        "agent.redo",
        "agent.snapshot",
        "agent.readSourceView",
        "agent.saveHistoryCheckpoint",
        "agent.restoreHistoryCheckpoint",
        project_plane::READ,
        project_plane::VALIDATE,
    ] {
        assert!(command_scope(&s, cmd, &json!({"token":d["token"]})).is_err(), "{cmd}");
        assert!(s.execute(cmd, json!({"token":d["token"]})).is_err(), "{cmd}");
    }
    let p = json!({"version":2,"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":{"kind":"default_transition","definition":"constant_gain"}});
    assert!(crate::agent_bridge::editor_compound::configure(&mut s, &p).is_err());
    let mut legacy = p;
    legacy.as_object_mut().unwrap().remove("version");
    assert!(editor_setting::configure(&mut s, &legacy).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn following_catalog_pages_cover_every_existing_definition_while_actual_frames_advance() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let doc = prepare_follow(&mut s, 190600);
    let mut offset = 0;
    let mut all = Vec::new();
    let mut clock = 0.0;
    loop {
        clock += 0.125;
        s.execute("trim.tick", json!({"clock":clock})).unwrap();
        let mut p = follow_page(&doc);
        p["offset"] = json!(offset);
        p["limit"] = json!(7);
        let before = state(&s);
        let result = s.execute(editor_setting::CATALOG, p).unwrap();
        check_basis(&s, &doc, &result);
        assert_eq!(state(&s), before);
        all.extend(result["definitions"].as_array().unwrap().iter().map(|v| v["definition"].as_str().unwrap().to_owned()));
        let Some(next) = result["nextOffset"].as_u64() else { break };
        offset = next;
    }
    let expected = filmcraft_project::effect_defs()
        .iter()
        .filter(|d| matches!(d.kind, filmcraft_project::EffectKind::AudioTransition | filmcraft_project::EffectKind::VideoTransition))
        .map(|d| d.id.to_owned())
        .collect::<Vec<_>>();
    assert_eq!(all, expected);
    assert_eq!(all.len(), 115);
}
#[test]
fn following_page_requests_bind_the_original_digest_and_reject_substituted_current_or_foreign_addresses() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = prepare_follow(&mut s, 190700);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    assert_ne!(d["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
    let before = state(&s);
    for cmd in [editor_settings::READ, editor_setting::CATALOG] {
        for change in 0..7 {
            let mut p = follow_page(&d);
            match change {
                0 => p["version"] = json!(1),
                1 => p["version"] = json!(3),
                2 => p["beforeEditorSha256"] = json!(project_clear::editor_hash(&s.state).unwrap()),
                3 => p["scope"]["workspaceId"] = json!("1ae902c3-55aa-46c4-9eae-aa3fc3359a14"),
                4 => p["token"] = json!(token(190799)),
                5 => p["limit"] = json!(33),
                _ => p["execute"] = json!("edit.undo"),
            }
            assert!(s.execute(cmd, p).is_err());
            assert_eq!(state(&s), before);
        }
    }
}

#[test]
fn following_metadata_expires_with_the_original_cancelled_ticket_or_changed_native_epoch() {
    for cancel in [false, true] {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = prepare_follow(&mut s, 190800 + usize::from(cancel));
        if cancel {
            let result = s.execute("agent.cancelSnapshot", json!({"job":d["token"]})).unwrap();
            assert_eq!(result["ticketRevoked"], true);
        } else {
            s.agent_bridge.epoch = Some("f".repeat(32));
        }
        let before = state(&s);
        for cmd in [editor_settings::READ, editor_setting::CATALOG] {
            assert!(s.execute(cmd, follow_page(&d)).is_err());
            assert_eq!(state(&s), before);
        }
    }
}
