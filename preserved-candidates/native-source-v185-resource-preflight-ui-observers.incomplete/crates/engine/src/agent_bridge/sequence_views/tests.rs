use super::*;
use crate::agent_bridge::{editor_control, project_navigation, tests::token};
use crate::{History, project_workspace};
use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::Tick;
const W: &str = "42f72fd5-bade-4f51-883a-d325db93ff9b";
fn scope() -> Value {
    json!({"projectId":"p_sequence_views","workspaceId":W})
}
fn setup(demo: bool) -> Session {
    let mut s = Session::default();
    if demo {
        s.execute("file.openDemoProject", json!({})).unwrap();
        let root = serde_json::from_value(scope()).unwrap();
        let anchor = s.state.active_sequence.unwrap();
        s.project = Arc::new(project_workspace::legacy_identity_candidate(&s.project, &root, anchor).unwrap());
        s.history = History { limit: 128, ..History::default() };
        for track in ["V2", "V3", "A2", "A3"] {
            s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
        }
    } else {
        s.execute("file.newProject", json!({"name":"Generated view preparation","workspace":scope()})).unwrap();
    }
    s.path = Some(crate::temp_dir().join("generated-sequence-views.fcproj").to_string_lossy().into_owned());
    s.agent_operation_control().enable_transport();
    s
}
fn args(s: &Session, n: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"projectPath":s.path})
}
fn capture(s: &mut Session, n: usize) -> Value {
    let p = args(s, n);
    s.execute(READ, p).unwrap()
}
fn observation(d: &Value) -> Value {
    json!({"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"clipboardMarkers":s.state.clipboard_markers,
        "revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,
        "journal":s.journal,"prefs":s.prefs,"trim":format!("{:?}",s.trim_play),"recording":s.mixrec.active(),"preview":s.previews.live.is_active()})
}
fn dynamic(s: &mut Session, kind: &str, moved: bool) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
    if moved {
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    }
}
fn select_loop(s: &mut Session) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":"roll"})).unwrap();
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
}
fn reject(s: &mut Session, p: Value) {
    let before = state(s);
    assert!(s.execute(OBSERVE, p).is_err());
    assert_eq!(state(s), before);
}
fn matches_current(s: &Session, d: &Value, r: &Value) {
    assert_eq!(r["original"], *d);
    assert_eq!(r["current"]["projectState"]["sha256"], project_plane::basis(s).unwrap());
    assert_eq!(r["current"]["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
    assert_eq!(r["current"]["historyState"], crate::agent_bridge::editor_compound::history_state(&s.project, &s.history).unwrap());
    assert_eq!(r["current"]["capabilities"]["sequenceViewPublication"], false);
    assert_eq!(r["current"]["sourceValidation"], "not-performed");
    assert_eq!(r["current"]["mediaEvidence"], false);
    assert_eq!(r["current"]["writeAuthority"], false);
}
#[test]
fn sequence_view_read_keeps_genuine_empty_root_nullable_scope_and_no_effects() {
    let mut s = setup(false);
    let before = state(&s);
    let project = s.project.clone();
    let d = capture(&mut s, 210000);
    assert_eq!(d["kind"], "native_sequence_view_preparation");
    assert_eq!(d["readPurpose"], "sequence_views_only");
    assert_eq!(d["sequence"], Value::Null);
    assert_eq!(d["sequenceScope"], Value::Null);
    assert_eq!(d["openTabs"], json!([]));
    assert_eq!(d["sequences"], json!([]));
    assert_eq!(d["humanCommands"], json!(["sequence.open", "sequence.close", "sequence.closeOthers", "sequence.moveTab"]));
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&s.project, &project));
    let r = s.execute(OBSERVE, observation(&d)).unwrap();
    matches_current(&s, &d, &r);
    assert_eq!(state(&s), before);
}
#[test]
fn sequence_view_read_retains_complete_open_order_duplicates_closed_targets_and_foreign_realm_membership() {
    let mut s = setup(false);
    let mut ids = Vec::new();
    for name in ["Own anchor", "Own target", "Foreign view"] {
        let v = s.execute("file.newSequence", json!({"name":name,"video":0,"audio":0})).unwrap();
        ids.push(ItemId(v["sequence"].as_u64().unwrap()));
    }
    Arc::make_mut(&mut s.project).item_mut(ids[2]).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
    s.state.open_sequences = vec![ids[2], ids[0], ids[2]];
    s.state.active_sequence = Some(ids[2]);
    let before = state(&s);
    let d = capture(&mut s, 210010);
    assert_eq!(state(&s), before);
    assert_eq!(
        d["openTabs"],
        json!([{"index":0,"item":ids[2].0,"owned":false},{"index":1,"item":ids[0].0,"owned":true},{"index":2,"item":ids[2].0,"owned":false}])
    );
    assert_eq!(d["sequences"].as_array().unwrap().len(), 3);
    assert!(d["sequences"].as_array().unwrap().iter().any(|r| r["item"] == ids[1].0 && r["owned"] == true));
    assert_eq!(d["sequenceScope"]["anchorSequence"], ids[0].0);
}
#[test]
fn sequence_view_read_all_real_dynamic_kinds_zero_and_nonzero_are_readonly() {
    for (k, kind) in ["trim", "ripple", "roll"].into_iter().enumerate() {
        for moved in [false, true] {
            let mut s = setup(true);
            dynamic(&mut s, kind, moved);
            let before = state(&s);
            let d = capture(&mut s, 210020 + k * 2 + usize::from(moved));
            assert_eq!(state(&s), before);
            assert!(s.trim_play.active());
            assert_eq!(d["trim"]["settlement"]["dynamicCommitted"], moved, "{kind}");
            assert_eq!(d["trim"]["settlement"]["offsetTicks"], command_settlement::prepare(&s, "sequence.open").unwrap().summary().offset.0.to_string());
        }
    }
}
#[test]
fn sequence_view_observation_follows_all_three_actual_human_clocks_without_replanning_or_settlement() {
    for (k, kind) in ["trim", "ripple", "roll"].into_iter().enumerate() {
        let mut s = setup(true);
        dynamic(&mut s, kind, false);
        let d = capture(&mut s, 210040 + k);
        for clock in [0.25, 0.5, 0.5] {
            s.execute("trim.tick", json!({"clock":clock})).unwrap();
            let before = state(&s);
            let r = s.execute(OBSERVE, observation(&d)).unwrap();
            matches_current(&s, &d, &r);
            assert_eq!(state(&s), before);
            assert!(s.trim_play.active());
        }
        assert_ne!(d["projectState"]["sha256"], project_plane::basis(&s).unwrap());
    }
}
#[test]
fn sequence_view_finite_shared_clock_preparations_match_each_original_human_prelude() {
    for kind in ["trim", "ripple", "roll"] {
        let mut s = setup(true);
        dynamic(&mut s, kind, false);
        let d = capture(&mut s, 210050);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let before = state(&s);
        let t = s.agent_bridge.tickets.iter().find(|t| t.token == d["token"].as_str().unwrap()).unwrap();
        let owner = t.sequence_views.as_ref().unwrap();
        for command in SequenceViewCommand::ALL {
            let p = owner.current(&s, command).unwrap();
            let native = command_settlement::prepare(&s, command.id()).unwrap();
            assert_eq!(p.summary(), native.summary());
            assert!(Arc::ptr_eq(p.project_after(), native.project_after()));
            assert_eq!(p.trim_shift_after(), native.trim_shift_after());
            p.validate(&s).unwrap();
        }
        assert_eq!(state(&s), before);
    }
}
#[test]
fn sequence_view_default_clock_preparations_cannot_borrow_finite_view_purpose() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let default = Following::capture(&s).unwrap();
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    for command in SequenceViewCommand::ALL {
        assert!(default.prepare_current_sequence_view(&s, command).is_err());
    }
    default.prepare_current(&s).unwrap();
}
#[test]
fn sequence_view_loop_observation_retains_loop_and_refuses_equivalent_restart() {
    let mut s = setup(true);
    select_loop(&mut s);
    let d = capture(&mut s, 210060);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let before = state(&s);
    let r = s.execute(OBSERVE, observation(&d)).unwrap();
    matches_current(&s, &d, &r);
    assert_eq!(state(&s), before);
    assert!(s.trim_play.active());
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    assert!(s.trim_play.active());
    reject(&mut s, observation(&d));
}
#[test]
fn sequence_view_equivalent_zero_dynamic_restart_expires_the_private_owner() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = capture(&mut s, 210070);
    let project = s.project.clone();
    s.execute("trim.shuttleStop", json!({})).unwrap();
    s.execute("trim.shuttle", json!({"direction":"forward","clock":0.0})).unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    reject(&mut s, observation(&d));
}
#[test]
fn sequence_view_nonclock_editor_clipboard_preference_path_and_history_changes_stay_foreign_after_ticks() {
    for change in 0..10 {
        let mut s = setup(true);
        s.state.selection_follows_playhead = false;
        dynamic(&mut s, "roll", false);
        let d = capture(&mut s, 210080 + change);
        match change {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.state.source_playhead = Tick(9_007_199_254_740_993),
            2 => s.state.playheads.insert(ItemId(u64::MAX), Tick(9_007_199_254_740_993)).map(|_| ()).unwrap_or(()),
            3 => s.state.selection.push(ClipId(u64::MAX)),
            4 => s.state.open_sequences.push(s.state.active_sequence.unwrap()),
            5 => s.history.limit = 1,
            6 => s.history.merge_key = Some("foreign gesture".into()),
            7 => s.prefs.audio.automatch_time += 1.0,
            8 => s.path = Some(crate::temp_dir().join("foreign-root.fcproj").to_string_lossy().into_owned()),
            _ => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
                s.state.clipboard.push((filmcraft_project::TrackKind::Video, 0, clip));
            }
        }
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        reject(&mut s, observation(&d));
    }
}
#[test]
fn sequence_view_original_digests_and_native_epoch_are_bound_even_after_real_motion() {
    for change in 0..5 {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = capture(&mut s, 210100 + change);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let mut p = observation(&d);
        match change {
            0 => p["beforeSha256"] = json!(project_plane::basis(&s).unwrap()),
            1 => p["beforeEditorSha256"] = json!(project_clear::editor_hash(&s.state).unwrap()),
            2 => p["scope"]["projectId"] = json!("foreign"),
            3 => p["scope"]["workspaceId"] = json!("a8028c26-a2e1-4d92-bb60-6b974b0929ee"),
            _ => s.agent_bridge.epoch = Some("dd980a7d-dbe7-4916-b88b-679c11101a73".into()),
        };
        reject(&mut s, p);
    }
}
#[test]
fn sequence_view_hostile_read_and_observation_shapes_never_register_a_partial_owner() {
    for change in 0..8 {
        let mut s = setup(true);
        let mut p = args(&s, 210120 + change);
        match change {
            0 => p["scope"]["projectId"] = json!("foreign"),
            1 => p["projectPath"] = json!("relative.fcproj"),
            2 => p["projectPath"] = json!(null),
            3 => p["projectPath"] = json!("/foreign.fcproj"),
            4 => p["token"] = json!(true),
            5 => p["clockPolicy"] = json!("original_human_trim"),
            6 => p["sourceValidation"] = json!(true),
            _ => p["scope"]["anchorSequence"] = json!(1),
        }
        let before = state(&s);
        let count = s.agent_bridge.tickets.len();
        assert!(s.execute(READ, p).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(s.agent_bridge.tickets.len(), count);
    }
    let mut s = setup(true);
    let d = capture(&mut s, 210140);
    for key in ["beforeSha256", "beforeEditorSha256"] {
        for value in [json!(null), json!(false), json!("x"), json!("F".repeat(64))] {
            let mut p = observation(&d);
            p[key] = value;
            reject(&mut s, p);
        }
    }
    let mut p = observation(&d);
    p["command"] = json!("edit.delete");
    reject(&mut s, p);
}
#[test]
fn sequence_view_old_root_sequence_and_editor_control_purposes_cannot_promote_to_view_owner() {
    for (i, command) in [project_plane::READ, editor_control::READ, editor_control::PREPARE].into_iter().enumerate() {
        let mut s = setup(true);
        let p = args(&s, 210150 + i);
        let d = s.execute(command, p).unwrap();
        reject(&mut s, observation(&d));
    }
    let mut s = setup(true);
    let d = s.execute("agent.snapshot", json!({"token":token(210160)})).unwrap();
    let old = s.agent_bridge.tickets.iter().find(|t| t.token == token(210160)).unwrap();
    assert!(old.project_scope.is_none() && old.editor_control.is_none() && old.sequence_views.is_none());
    assert_eq!(old.sequence, s.state.active_sequence);
    let p = json!({"token":d["token"],"scope":scope(),"beforeSha256":"0".repeat(64),"beforeEditorSha256":"0".repeat(64)});
    reject(&mut s, p);
}
#[test]
fn sequence_view_ticket_cannot_pay_old_settings_source_timeline_history_navigation_or_persistence() {
    let mut s = setup(true);
    let d = capture(&mut s, 210170);
    for command in [
        "agent.readEditorSettings",
        "agent.readEditorTransitionCatalog",
        "agent.configureEditor",
        "agent.configureEditorControl",
        "agent.apply",
        "agent.readSourceCatalog",
        "agent.snapshot",
        "agent.sourceFrames",
        "agent.readHistory",
        "agent.undo",
        "agent.redo",
        "agent.saveHistoryCheckpoint",
        "agent.restoreHistoryCheckpoint",
        project_navigation::COMMAND,
    ] {
        assert!(crate::commands::find(command).is_some(), "test must exercise registered command {command}");
        assert!(super::super::project_plane::command_scope(&s, command, &json!({"token":d["token"]})).is_err());
        let before = state(&s);
        let p = json!({"token":d["token"],"scope":scope()});
        assert!(s.execute(command, p).is_err(), "{command}");
        assert_eq!(state(&s), before);
    }
    assert!(super::super::check_edit_ticket_basis(&s, d["token"].as_str().unwrap()).is_err());
    let mut p = args(&s, 210170);
    p["extra"] = json!(true);
    assert!(s.execute(READ, p).is_err());
}
#[test]
fn sequence_view_original_stop_and_ticket_eviction_expire_observation_without_a_human_effect() {
    let mut s = setup(true);
    let d = capture(&mut s, 210180);
    let stop = s.agent_operation_control().cancel(&json!({"token":d["token"]})).unwrap();
    assert_eq!(stop["cancellationAccepted"], true);
    reject(&mut s, observation(&d));
    let mut s = setup(false);
    let d = capture(&mut s, 210181);
    for i in 0..=super::super::TICKET_LIMIT {
        capture(&mut s, 211000 + i);
    }
    assert!(!s.agent_bridge.tickets.iter().any(|t| t.token == d["token"].as_str().unwrap()));
    reject(&mut s, observation(&d));
}
#[test]
fn sequence_view_missing_stop_or_display_only_session_never_acquires_preparation() {
    let mut s = setup(false);
    s.agent_bridge.operations = Default::default();
    let p = args(&s, 210190);
    let before = state(&s);
    assert!(read(&mut s, &p).is_err());
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
    let mut s = setup(false);
    s.view_only = true;
    let p = args(&s, 210191);
    let before = state(&s);
    assert!(read(&mut s, &p).is_err());
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
}
#[test]
fn sequence_view_complete_inventory_refuses_dangling_oversized_and_nonrepresentable_tabs_atomically() {
    for change in 0..3 {
        let mut s = setup(true);
        let id = s.state.active_sequence.unwrap();
        match change {
            0 => s.state.open_sequences.push(ItemId(u64::MAX)),
            1 => s.state.open_sequences = vec![id; ROW_LIMIT + 1],
            _ => Arc::make_mut(&mut s.project).item_mut(id).unwrap().name = "x".repeat(1025),
        }
        let before = state(&s);
        let p = args(&s, 210200 + change);
        assert!(s.execute(READ, p).is_err());
        assert_eq!(state(&s), before);
        assert!(s.agent_bridge.tickets.is_empty());
    }
}
