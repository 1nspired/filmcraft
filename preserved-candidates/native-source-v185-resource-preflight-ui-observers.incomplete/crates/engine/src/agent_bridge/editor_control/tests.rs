use super::*;
use crate::agent_bridge::{editor_setting, editor_settings, tests::token};
use crate::{History, project_workspace};
use filmcraft_time::Tick;
const W: &str = "e8c7a1b0-7ecf-4a8f-b0b3-75d4eaa077ce";
fn scope() -> Value {
    json!({"projectId":"p_live_editor","workspaceId":W})
}
fn setup(demo: bool) -> Session {
    let mut s = Session::default();
    if demo {
        s.execute("file.openDemoProject", json!({})).unwrap();
        let root: ProjectScope = serde_json::from_value(scope()).unwrap();
        let anchor = s.state.active_sequence.unwrap();
        // Generated fixture identity uses the real native anchor and existing migration helper.
        s.project = Arc::new(project_workspace::legacy_identity_candidate(&s.project, &root, anchor).unwrap());
        s.history = History { limit: 128, ..History::default() };
        for track in ["V2", "V3", "A2", "A3"] {
            s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
        }
    } else {
        s.execute("file.newProject", json!({"name":"Generated live control","workspace":scope()})).unwrap();
    }
    s.path = Some(crate::temp_dir().join("generated-live-editor-control.fcproj").to_string_lossy().into_owned());
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
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,
    "revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,
    "limit":s.history.limit,"merge":s.history.merge_key,"journal":s.journal,"trim":format!("{:?}",s.trim_play),
    "recording":s.mixrec.active(),"preview":s.previews.live.is_active()})
}
fn dynamic(s: &mut Session, kind: &str, moved: bool) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
    s.execute("trim.shuttle", json!({"clock":0.0,"direction":if kind=="roll"{"forward"}else{"reverse"}})).unwrap();
    if moved {
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    }
}
fn page(doc: &Value) -> Value {
    json!({"token":doc["token"],"scope":scope(),"beforeEditorSha256":doc["editorStateSha256"],"offset":0,"limit":32})
}
#[test]
fn live_editor_read_keeps_genuine_empty_root_and_exact_readonly_metadata() {
    let mut s = setup(false);
    let before = state(&s);
    let project = s.project.clone();
    let r = capture(&mut s, 170000);
    assert_eq!(r["kind"], "native_editor_control_read");
    assert_eq!(r["readPurpose"], "editor_controls_only");
    assert_eq!(r["sequence"], Value::Null);
    assert_eq!(r["projectState"]["sequences"], 0);
    assert_eq!(r["trim"]["active"], false);
    assert_eq!(r["capabilities"]["compoundEditorWrite"], false);
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["projectState"]["sha256"], project_plane::basis(&s).unwrap());
    assert_eq!(r["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&project, &s.project));
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token(170000)).unwrap();
    assert!(ticket.editor_control.as_ref().unwrap().settlement.is_some());
    assert!(ticket.source_library.is_none());
    assert_eq!(ticket.project_scope.as_ref(), Some(&serde_json::from_value(scope()).unwrap()));
}
#[test]
fn live_editor_read_captures_current_dynamic_regular_ripple_and_roll_without_settlement() {
    for (i, kind) in ["trim", "ripple", "roll"].into_iter().enumerate() {
        let mut s = setup(true);
        dynamic(&mut s, kind, true);
        let before = state(&s);
        let project = s.project.clone();
        let r = capture(&mut s, 170010 + i);
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
        assert!(s.trim_play.dynamic.is_some());
        assert_eq!(r["trim"]["active"], true);
        assert_eq!(r["trim"]["dynamic"], true);
        assert_eq!(r["trim"]["settlement"]["dynamicCommitted"], true);
        assert!(r["trim"]["settlement"]["offsetTicks"].is_string());
        let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token(170010 + i)).unwrap();
        ticket.editor_control.as_ref().unwrap().validate(&s).unwrap();
    }
}
#[test]
fn live_editor_read_observes_loop_without_stopping_it_or_making_a_history_step() {
    let mut s = setup(true);
    s.set_playhead(Tick::ZERO);
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let before = state(&s);
    let r = capture(&mut s, 170020);
    assert_eq!(state(&s), before);
    assert!(s.trim_play.around.is_some());
    assert_eq!(r["trim"]["loop"], true);
    assert_eq!(r["trim"]["settlement"]["loopStopped"], true);
    assert_eq!(r["trim"]["settlement"]["dynamicCommitted"], false);
}
#[test]
fn ordinary_project_and_sequence_read_idle_guards_remain_closed_during_live_trim() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let before = state(&s);
    let p = args(&s, 170030);
    let e = s.execute(project_plane::READ, p).unwrap_err();
    assert!(e.to_string().contains("finish the active trim"));
    assert!(s.execute("agent.snapshot", json!({"token":token(170031)})).is_err());
    assert_eq!(state(&s), before);
    capture(&mut s, 170032);
    assert_eq!(state(&s), before);
}
#[test]
fn live_editor_read_preserves_original_mix_recording_and_preview_owners() {
    for recording in [false, true] {
        let mut s = setup(true);
        s.set_playhead(Tick::ZERO);
        let track = s.active_sequence().unwrap().audio_tracks[0].id;
        if recording {
            s.execute("mixer.recordStart", json!({"time":0})).unwrap();
        } else {
            s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":0})).unwrap();
        }
        let before = state(&s);
        let r = capture(&mut s, 170040 + usize::from(recording));
        assert_eq!(state(&s), before);
        assert_eq!(r["capabilities"]["compoundEditorWrite"], false);
        if recording {
            assert!(s.mixrec.active());
        } else {
            assert!(s.previews.live.is_active());
        }
    }
}
#[test]
fn live_editor_ticket_supports_only_exact_existing_settings_and_transition_catalog_reads() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let r = capture(&mut s, 170050);
    let before = state(&s);
    let settings = s.execute(editor_settings::READ, page(&r)).unwrap();
    assert_eq!(settings["token"], r["token"]);
    assert_eq!(settings["modeState"], "owned_active_sequence");
    assert!(!settings["tracks"].as_array().unwrap().is_empty());
    let catalog = s.execute(editor_setting::CATALOG, page(&r)).unwrap();
    assert_eq!(catalog["total"], 115);
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.iter().find(|t| t.token == token(170050)).unwrap().committed.is_none());
}
#[test]
fn live_editor_ticket_cannot_be_borrowed_for_project_source_timeline_history_or_old_settings_mutations() {
    let mut s = setup(false);
    let r = capture(&mut s, 170060);
    let before = state(&s);
    for command in [
        project_plane::READ,
        project_plane::VALIDATE,
        project_plane::SEQUENCE_SCOPE,
        editor_setting::COMMAND,
        "agent.apply",
        "agent.undo",
        "agent.redo",
        "agent.readBins",
        "agent.readSearchBins",
        "agent.readTracks",
        "agent.readHistory",
        "agent.saveHistoryCheckpoint",
        "agent.restoreHistoryCheckpoint",
        "agent.validateRetainedHistory",
        "agent.inspectHistoryCheckpoint",
        "agent.readFinishing",
        "agent.snapshot",
        "agent.readSourceFrame",
        "agent.readSourceView",
    ] {
        assert!(command_scope(&s, command, &json!({"token":r["token"]})).is_err(), "{command}");
    }
    for command in ["agent.apply", "agent.undo", project_plane::VALIDATE, editor_setting::COMMAND, "agent.saveHistoryCheckpoint"] {
        let e = s.execute(command, json!({"token":r["token"]})).unwrap_err();
        assert!(e.to_string().contains("editor-only ticket"), "{command}: {e}");
    }
    assert!(super::super::check_edit_ticket_basis(&s, token(170060).as_str()).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn stopped_live_editor_owner_does_not_promote_its_read_into_an_idle_project_plan() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let r = capture(&mut s, 170070);
    s.execute("trim.shuttleStop", json!({})).unwrap();
    assert!(!s.trim_play.active());
    let before = state(&s);
    let e = s.execute("agent.apply", json!({"token":r["token"]})).unwrap_err();
    assert!(e.to_string().contains("editor-only ticket"));
    let normal = args(&s, 170071);
    s.execute(project_plane::READ, normal).unwrap();
    assert!(s.agent_bridge.tickets.iter().find(|t| t.token == token(170071)).unwrap().editor_control.is_none());
    assert_eq!(state(&s), before);
}
#[test]
fn identical_restart_keeps_visible_basis_but_expires_the_native_prepared_owner() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let r = capture(&mut s, 170080);
    let editor = r["editorStateSha256"].clone();
    let project = r["projectState"]["sha256"].clone();
    s.execute("trim.shuttleStop", json!({})).unwrap();
    s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
    assert_eq!(editor, project_clear::editor_hash(&s.state).unwrap());
    assert_eq!(project, project_plane::basis(&s).unwrap());
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token(170080)).unwrap();
    assert!(ticket.editor_control.as_ref().unwrap().validate(&s).is_err());
}
#[test]
fn live_editor_read_rejects_foreign_legacy_reused_cancelled_and_malformed_read_requests() {
    let mut s = setup(false);
    let r = capture(&mut s, 170090);
    let before = state(&s);
    assert!(s.execute(READ, args(&s, 170090)).is_err());
    for (i, change) in ["scope", "path", "sequence", "raw", "extra"].into_iter().enumerate() {
        let mut p = args(&s, 170091 + i);
        match change {
            "scope" => p["scope"]["projectId"] = json!("foreign"),
            "path" => p["projectPath"] = json!("/foreign.fcproj"),
            "sequence" => p["scope"] = json!({"projectId":"p_live_editor","anchorSequence":1}),
            "raw" => p["token"] = json!("bad"),
            _ => p["execute"] = json!("edit.undo"),
        }
        assert!(s.execute(READ, p).is_err());
    }
    s.agent_bridge.stopped_snapshots.push_back(token(170099));
    let p = args(&s, 170099);
    assert!(s.execute(READ, p).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute(READ, json!({"token":token(170100),"scope":r["scope"],"projectPath":null})).is_err());
    let mut legacy = Session::default();
    legacy.execute("file.openDemoProject", json!({})).unwrap();
    legacy.path = Some(crate::temp_dir().join("unowned-legacy-control.fcproj").to_string_lossy().into_owned());
    let old = state(&legacy);
    let p = args(&legacy, 170101);
    assert!(legacy.execute(READ, p).is_err());
    assert_eq!(state(&legacy), old);
}
#[test]
fn live_editor_preparations_follow_original_ticket_eviction_and_cannot_be_serialized_or_checkpointed() {
    let mut s = setup(false);
    for i in 0..=super::super::TICKET_LIMIT {
        capture(&mut s, 170200 + i);
    }
    assert_eq!(s.agent_bridge.tickets.len(), super::super::TICKET_LIMIT);
    assert!(!s.agent_bridge.tickets.iter().any(|t| t.token == token(170200)));
    let p = json!({"token":token(170200),"scope":scope(),"beforeEditorSha256":project_clear::editor_hash(&s.state).unwrap(),"offset":0,"limit":32});
    assert!(s.execute(editor_settings::READ, p).is_err());
    let token = s.agent_bridge.tickets.back().unwrap().token.clone();
    let e = s.execute("agent.saveHistoryCheckpoint", json!({"token":token})).unwrap_err();
    assert!(e.to_string().contains("editor-only ticket"));
}
#[test]
fn read_only_lifecycle_preparation_never_recreates_or_acquires_a_live_writer() {
    let mut s = setup(false);
    s.view_only = true;
    let before = state(&s);
    let owner = ReadOwner::capture(&s).unwrap();
    assert!(owner.settlement.is_none());
    assert_eq!(owner.summary(), Value::Null);
    owner.validate(&s).unwrap();
    assert_eq!(state(&s), before);
    assert!(s.execute(READ, args(&s, 170500)).is_err());
}

mod following;
