use super::*;
use crate::agent_bridge::{project_plane, tests::token};
use filmcraft_project::AutomationMode;
use std::sync::Arc;
const W: &str = "1ae902c3-55aa-46c4-9eae-aa3fc3359a14";
fn root_scope() -> Value {
    json!({"projectId":"p_editor_settings","workspaceId":W})
}
fn setup(sequence: bool) -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated editor settings","workspace":root_scope()})).unwrap();
    if sequence {
        s.execute("file.newSequence", json!({"name":"Original settings view","fps":25,"video":1,"audio":1})).unwrap();
    }
    s.path = Some(crate::temp_dir().join("generated-editor-settings.fcproj").to_string_lossy().into_owned());
    s
}
fn sequence_scope(s: &Session) -> Value {
    serde_json::to_value(crate::editorial_workspace::association(&s.project).unwrap().unwrap().scope).unwrap()
}
fn capture(s: &mut Session, n: usize, project: bool) -> Value {
    let (scope, d) = if project {
        let scope = root_scope();
        let d = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope,"projectPath":s.path})).unwrap();
        (scope, d)
    } else {
        let scope = sequence_scope(s);
        let d = s.execute("agent.snapshot", json!({"token":token(n)})).unwrap();
        (scope, d)
    };
    assert_eq!(d["capabilities"]["editorSettingsRead"], true);
    assert_eq!(d["editorStateSha256"].as_str().unwrap().len(), 64);
    json!({"scope":scope,"beforeEditorSha256":d["editorStateSha256"]})
}
fn request(n: usize, basis: Value, offset: usize, limit: usize) -> Value {
    json!({"token":token(n),"scope":basis["scope"],"beforeEditorSha256":basis["beforeEditorSha256"],"offset":offset,"limit":limit})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"markers":s.state.clipboard_markers,
        "revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
#[test]
fn editor_settings_read_uses_original_root_and_sequence_tickets_and_actual_all_five_stored_clip_modes() {
    for (index, mode) in AutomationMode::ALL.into_iter().enumerate() {
        for project in [false, true] {
            let mut s = setup(true);
            let id = s.active_sequence().unwrap().audio_tracks[0].id;
            s.execute("clipMixer.setMode", json!({"track":id.0,"mode":mode.label()})).unwrap();
            let n = 150000 + index * 10 + usize::from(project);
            let scope = capture(&mut s, n, project);
            let before = state(&s);
            let pointer = s.project.clone();
            let result = s.execute(READ, request(n, scope.clone(), 0, 32)).unwrap();
            assert_eq!(result["scope"], scope["scope"]);
            assert_eq!(result["modeState"], "owned_active_sequence");
            assert_eq!(result["tracks"][0]["track"], id.0);
            assert_eq!(result["tracks"][0]["mode"], mode.label());
            assert_eq!(result["tracks"][0]["storedMode"], mode.label());
            assert_eq!(result["editorStateSha256"], project_clear::editor_hash(&s.state).unwrap());
            assert_eq!(result["sourceValidation"], "not-performed");
            assert_eq!(result["mediaEvidence"], false);
            assert_eq!(result["writeAuthority"], false);
            assert_eq!(state(&s), before);
            assert!(Arc::ptr_eq(&pointer, &s.project));
        }
    }
}
#[test]
fn editor_settings_read_pages_complete_current_audio_inventory_without_leaking_or_granting_orphan_mode_targets() {
    let mut s = setup(true);
    for _ in 0..39 {
        s.execute("sequence.addTracks", json!({"video":0,"audio":1})).unwrap();
    }
    s.state.clip_mixer_modes.insert(900000, AutomationMode::Write);
    let scope = capture(&mut s, 150100, true);
    let before = state(&s);
    let mut rows = Vec::new();
    let mut offset = 0;
    loop {
        let r = s.execute(READ, request(150100, scope.clone(), offset, 7)).unwrap();
        assert_eq!(r["total"], 40);
        assert_eq!(r["offset"], offset);
        rows.extend(r["tracks"].as_array().unwrap().clone());
        let Some(next) = r["nextOffset"].as_u64() else { break };
        offset = next as usize;
    }
    assert_eq!(rows.len(), 40);
    let expected = s.active_sequence().unwrap().audio_tracks.iter().map(|t| t.id.0).collect::<Vec<_>>();
    assert_eq!(rows.iter().map(|r| r["track"].as_u64().unwrap()).collect::<Vec<_>>(), expected);
    assert!(rows.iter().all(|r| r["storedMode"].is_null() && r["mode"] == "Read"));
    assert!(!rows.iter().any(|r| r["track"] == 900000));
    assert_eq!(state(&s), before);
}
#[test]
fn editor_settings_read_preserves_empty_root_and_foreign_active_sequence_purposes_without_guessing_an_anchor() {
    let mut empty = setup(false);
    let scope = capture(&mut empty, 150200, true);
    let r = empty.execute(READ, request(150200, scope, 0, 32)).unwrap();
    assert_eq!(r["sequence"], Value::Null);
    assert_eq!(r["modeState"], "no_active_sequence");
    assert_eq!(r["total"], 0);
    assert_eq!(r["tracks"], json!([]));
    let mut s = setup(true);
    s.execute("file.newSequence", json!({"name":"Foreign native view","fps":25,"video":1,"audio":1})).unwrap();
    let id = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
    let scope = capture(&mut s, 150210, true);
    let before = state(&s);
    let r = s.execute(READ, request(150210, scope, 0, 32)).unwrap();
    assert_eq!(r["sequence"], id.0);
    assert_eq!(r["modeState"], "outside_owning_sequence");
    assert_eq!(r["tracks"], json!([]));
    assert_eq!(state(&s), before);
}
#[test]
fn editor_settings_read_refuses_foreign_mixed_library_consumed_and_wrong_ticket_purposes() {
    for mode in ["wrong-root", "root-as-sequence", "sequence-as-root", "mixed", "library", "consumed", "expired"] {
        let mut s = setup(true);
        let project = mode != "sequence-as-root";
        let mut scope = capture(&mut s, 150300, project);
        match mode {
            "wrong-root" => scope["scope"]["projectId"] = json!("foreign"),
            "root-as-sequence" => scope["scope"] = sequence_scope(&s),
            "sequence-as-root" => scope["scope"] = root_scope(),
            "mixed" => scope["scope"]["anchorSequence"] = json!(s.state.active_sequence.unwrap().0),
            "library" => s.agent_bridge.tickets.back_mut().unwrap().source_library = Some(vec![1]),
            "consumed" => s.agent_bridge.tickets.back_mut().unwrap().committed = Some((json!({}), json!({}))),
            _ => {}
        }
        let mut args = request(150300, scope, 0, 32);
        if mode == "expired" {
            args["token"] = json!(token(150399));
        }
        let before = state(&s);
        assert!(s.execute(READ, args).is_err(), "accepted {mode}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn editor_settings_read_checks_full_editor_and_private_clipboard_context_when_project_revision_is_unchanged() {
    for mode in ["clip-mode", "default", "playhead", "clipboard"] {
        let mut s = setup(true);
        let scope = capture(&mut s, 150400, true);
        let revision = s.revision;
        match mode {
            "clip-mode" => {
                s.execute("clipMixer.setMode", json!({"track":"A1","mode":"Write"})).unwrap();
            }
            "default" => {
                s.execute("effects.setDefaultTransition", json!({"effect":"constant_gain"})).unwrap();
            }
            "playhead" => s.state.source_playhead = filmcraft_time::Tick(123),
            _ => s.state.clipboard_markers.push(
                serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"Private later clipboard","comment":"","color":"Iris","kind":"Comment"}))
                    .unwrap(),
            ),
        }
        assert_eq!(s.revision, revision);
        let before = state(&s);
        assert!(s.execute(READ, request(150400, scope, 0, 32)).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn editor_settings_read_keeps_implicit_defaults_explicit_modes_and_unknown_transition_storage_distinct() {
    let mut s = setup(true);
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    s.state.default_audio_transition = "cross_dissolve".into();
    s.state.default_video_transition = "old_unknown_transition".into();
    let scope = capture(&mut s, 150500, true);
    let before = s.execute(READ, request(150500, scope, 0, 32)).unwrap();
    assert_eq!(before["tracks"][0]["mode"], "Read");
    assert_eq!(before["tracks"][0]["storedMode"], Value::Null);
    assert_eq!(before["defaults"]["audio"]["definition"], "cross_dissolve");
    assert_eq!(before["defaults"]["audio"]["nativeDefinitionKnown"], false);
    assert_eq!(before["defaults"]["video"]["nativeDefinitionKnown"], false);
    s.execute("clipMixer.setMode", json!({"track":id.0,"mode":"Read"})).unwrap();
    s.execute("effects.setDefaultTransition", json!({"effect":"exponential_fade"})).unwrap();
    let scope = capture(&mut s, 150501, true);
    let after = s.execute(READ, request(150501, scope, 0, 32)).unwrap();
    assert_eq!(after["tracks"][0]["storedMode"], "Read");
    assert_eq!(after["defaults"]["audio"]["nativeDefinitionKnown"], true);
    assert_ne!(before["editorStateSha256"], after["editorStateSha256"]);
}
#[test]
fn editor_settings_read_preserves_actual_live_preview_and_recording_owners_without_relaxing_original_mutation_guards() {
    let mut s = setup(true);
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("clipMixer.touch", json!({"track":id.0,"lane":"volume","value":-9,"time":0})).unwrap();
    assert!(s.previews.live.is_active());
    let scope = capture(&mut s, 150600, true);
    let before = state(&s);
    s.execute(READ, request(150600, scope, 0, 32)).unwrap();
    assert!(s.previews.live.is_active());
    assert_eq!(state(&s), before);
    s.previews.live.clear_all();
    let scope = capture(&mut s, 150601, true);
    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
    assert!(s.mixrec.active());
    let before = state(&s);
    s.execute(READ, request(150601, scope, 0, 32)).unwrap();
    assert!(s.mixrec.active());
    assert_eq!(state(&s), before);
    assert!(crate::agent_bridge::check_edit_ticket_basis(&s, &token(150601)).is_err());
    assert!(s.execute("agent.readProject", json!({"token":token(150602),"scope":root_scope(),"projectPath":s.path})).is_err());
}
#[test]
fn editor_settings_read_refuses_hostile_pages_extra_arguments_and_malformed_native_inventory_without_granting_write_authority() {
    for mode in ["zero", "large", "offset", "negative", "extra", "hash", "long-name", "zero-id", "duplicate", "long-default"] {
        let mut s = setup(true);
        let sid = s.state.active_sequence.unwrap();
        match mode {
            "long-name" => Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].name = "x".repeat(1025),
            "zero-id" => Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].id.0 = 0,
            "duplicate" => {
                let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
                q.audio_tracks.push(q.audio_tracks[0].clone());
            }
            "long-default" => s.state.default_audio_transition = "x".repeat(257),
            _ => {}
        }
        let scope = capture(&mut s, 150700, true);
        let mut args = request(150700, scope, 0, 32);
        match mode {
            "zero" => args["limit"] = json!(0),
            "large" => args["limit"] = json!(33),
            "offset" => args["offset"] = json!(2),
            "negative" => args["offset"] = json!(-1),
            "extra" => args["require_idle"] = json!(false),
            "hash" => args["beforeEditorSha256"] = json!("a".repeat(64)),
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute(READ, args).is_err(), "accepted {mode}");
        assert_eq!(state(&s), before);
    }
}
