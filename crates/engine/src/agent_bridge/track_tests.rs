use super::tests::{read, token};
use crate::Session;
use filmcraft_project::{Param, ParamValue, TrackId, TrackSend};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup() -> Session {
    let mut s = super::tests::setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn page(s: &mut Session, n: usize, offset: usize, limit: usize) -> Value {
    s.execute(
        "agent.readTracks",
        json!({"token":token(n),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"offset":offset,"limit":limit}),
    )
    .unwrap()
}
fn request(s: &mut Session, n: usize, edit: Value) -> Value {
    let rows = page(s, n, 0, 1);
    json!({"token":token(n),"label":"Exact shared track gesture","exact":true,"operations":[{"command":"agent.track","params":{"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"beforeSha256":rows["sha256"],"edit":edit}}]})
}
fn step(s: &mut Session, n: usize, edit: Value, command: &str, params: Value) -> Value {
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute(command, params).unwrap();
    let before = s.project.to_json();
    let undo = s.history.undo.len();
    read(s, n);
    let batch = request(s, n, edit);
    let receipt = s.execute("agent.apply", batch.clone()).unwrap();
    assert_eq!(receipt["action"], "track_edit");
    assert_eq!(receipt["results"][0]["humanCommand"], command);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.project.to_json(), human.project.to_json());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
    assert_eq!(s.execute("agent.apply", batch.clone()).unwrap(), receipt);
    let after = s.project.to_json();
    read(s, n + 1);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert_eq!(s.project.to_json(), before);
    let unchanged = state(s);
    assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt);
    assert_eq!(state(s), unchanged);
    read(s, n + 2);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(s.project.to_json(), after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), after);
    receipt["results"][0].clone()
}
#[test]
fn authored_track_pages_cover_all_kinds_and_master_without_mutation_and_digest_hidden_payloads() {
    let mut s = setup();
    s.execute("mixer.addSubmix", json!({"name":"Human bus","channels":"5.1"})).unwrap();
    s.execute("captions.newTrack", json!({"format":"CEA-608","language":"ja"})).unwrap();
    read(&mut s, 60000);
    let before = state(&s);
    let first = page(&mut s, 60000, 0, 1);
    assert_eq!(state(&s), before);
    assert_eq!(first["master"]["kind"], "master");
    let all = page(&mut s, 60000, 0, 32);
    assert_eq!(all["tracks"].as_array().unwrap().len(), first["total"].as_u64().unwrap() as usize);
    for kind in ["video", "audio", "submix", "caption"] {
        assert!(all["tracks"].as_array().unwrap().iter().any(|t| t["kind"] == kind));
    }
    assert_eq!(all["tracks"].as_array().unwrap().last().unwrap()["language"], "ja");
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].mixer.lanes.insert("volume".into(), Param::new(ParamValue::Float(-7.0)));
    read(&mut s, 60001);
    assert_ne!(page(&mut s, 60001, 0, 1)["sha256"], first["sha256"], "an unpaged track's authored automation is part of whole-state proof");
}
#[test]
fn exact_track_creation_uses_native_channel_defaults_insertion_names_and_global_id_reservation() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    let other = s.project.sequences().find(|i| i.id != sid).unwrap().id;
    Arc::make_mut(&mut s.project).sequence_mut(other).unwrap().video_tracks[0].id = TrackId(8000000);
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].id.0;
    let a = q.audio_tracks.last().unwrap().id.0;
    let r = step(
        &mut s,
        60100,
        json!({"type":"add","video":2,"audio":1,"submix":1,"video_after":v,"audio_after":a,"submix_after":null,"audio_channels":"Mono","submix_channels":"5.1"}),
        "sequence.addTracks",
        json!({"video":2,"audio":1,"submix":1,"videoAfter":1,"audioAfter":3,"submixAfter":0,"audioType":"Mono","submixType":"5.1"}),
    );
    assert_eq!(r["tracks"], json!([8000001, 8000002, 8000003, 8000004]));
}
#[test]
fn exact_track_settings_match_shared_human_video_audio_submix_and_caption_gestures() {
    let mut s = setup();
    let bus = s.execute("mixer.addSubmix", json!({"channels":"Mono"})).unwrap()["id"].as_u64().unwrap();
    s.execute("captions.newTrack", json!({"format":"Subtitle"})).unwrap();
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].id.0;
    let a = q.audio_tracks[0].id.0;
    let c = q.caption_tracks[0].id.0;
    for (n, id, changes, command, key) in [
        (60200, v, json!({"name":"Human picture","enabled":false,"syncLock":false,"label":"Rose"}), "timeline.setTrack", "track"),
        (60210, a, json!({"name":"Original sound","volumeDb":-7.0,"pan":31.0,"muted":true,"solo":false}), "timeline.setTrack", "track"),
        (60220, bus, json!({"name":"Music bus","volumeDb":-6.0,"pan":-25.0,"solo":true}), "mixer.setStrip", "strip"),
        (60230, c, json!({"name":"Captions 原文","language":"ja","format":"CEA-608","syncLock":false}), "captions.setTrack", "track"),
    ] {
        let mut params = changes.clone();
        params[key] = json!(id);
        step(&mut s, n, json!({"type":"set","track":id,"changes":changes}), command, params);
    }
}
#[test]
fn exact_track_removal_retains_linked_survivors_and_repairs_native_selection_and_inspector_targets() {
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let id = q.video_tracks[0].id.0;
    let clip = q.video_tracks[0].items[0].id;
    let audio = serde_json::to_value(&q.audio_tracks).unwrap();
    s.state.selection = vec![clip];
    s.state.selected_effect = Some(crate::EffectSel { clip, effect: "opacity".into(), instance: 0, instance_id: None });
    let r = step(&mut s, 60300, json!({"type":"remove","tracks":[id]}), "sequence.removeTracks", json!({"tracks":[id]}));
    assert!(r["collateral"]["removedClips"].as_u64().unwrap() > 0);
    assert_eq!(serde_json::to_value(&s.active_sequence().unwrap().audio_tracks).unwrap(), audio);
    assert!(s.state.selected_effect.is_none());
}
#[test]
fn exact_submix_removal_shares_routing_send_automation_and_native_history() {
    let mut s = setup();
    let bus = s.execute("mixer.addSubmix", json!({})).unwrap()["id"].as_u64().unwrap();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let t = &mut q.audio_tracks[0];
    t.mixer.output = Some(TrackId(bus));
    t.mixer.sends.push(TrackSend::new(TrackId(bus)));
    t.mixer.lanes.insert("send.0.level".into(), Param::new(ParamValue::Float(-3.0)));
    let r = step(&mut s, 60400, json!({"type":"remove","tracks":[bus]}), "sequence.removeTracks", json!({"tracks":[bus]}));
    assert_eq!(r["collateral"]["reroutedOutputs"], 1);
    assert_eq!(r["collateral"]["removedSends"], 1);
    let t = &s.active_sequence().unwrap().audio_tracks[0];
    assert!(t.mixer.output.is_none());
    assert!(t.mixer.sends.is_empty());
    assert!(!t.mixer.lanes.contains_key("send.0.level"));
}
#[test]
fn exact_reorders_match_all_four_human_collections_preserving_track_payloads() {
    let mut s = setup();
    s.execute("sequence.addTracks", json!({"video":0,"submix":2})).unwrap();
    s.execute("captions.newTrack", json!({"format":"Subtitle"})).unwrap();
    s.execute("captions.newTrack", json!({"format":"CEA-608"})).unwrap();
    for (i, (track, anchor)) in [("V1", "V3"), ("A1", "A3"), ("S1", "S2"), ("C1", "C2")].into_iter().enumerate() {
        let q = s.active_sequence().unwrap();
        let id = crate::track_ops::resolve(q, &json!(track), "test").unwrap().0;
        let after = crate::track_ops::resolve(q, &json!(anchor), "test").unwrap().0;
        step(&mut s, 60500 + i * 10, json!({"type":"reorder","track":id,"after":after}), "sequence.reorderTrack", json!({"track":id,"after":after}));
    }
}
#[test]
fn track_hostile_foreign_locked_noop_and_clamping_requests_preserve_project_context_and_history() {
    let mut s = setup();
    s.execute("sequence.addTracks", json!({"video":1})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].id.0;
    let a = q.audio_tracks[0].id.0;
    let sid = s.state.active_sequence.unwrap();
    let foreign = s.project.sequences().find(|i| i.id != sid).unwrap().as_sequence().unwrap().video_tracks[0].id.0;
    for (i, edit) in [
        json!({"type":"remove","tracks":[v,v]}),
        json!({"type":"remove","tracks":[foreign]}),
        json!({"type":"reorder","track":v,"after":a}),
        json!({"type":"reorder","track":v,"after":null}),
        json!({"type":"set","track":v,"changes":{"pan":101.0}}),
        json!({"type":"set","track":v,"changes":{"enabled":"false"}}),
        json!({"type":"set","track":v,"changes":{"name":"x","output":null}}),
        json!({"type":"set","track":v,"changes":{}}),
        json!({"type":"set","track":v,"changes":{"name":"x".repeat(1025)}}),
        json!({"type":"reorder","track":v}),
        json!({"type":"remove","tracks":[]}),
    ]
    .into_iter()
    .enumerate()
    {
        read(&mut s, 60600 + i);
        let batch = request(&mut s, 60600 + i, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    s.execute("timeline.setTrack", json!({"track":v,"locked":true})).unwrap();
    for (i, edit) in [
        json!({"type":"remove","tracks":[v]}),
        json!({"type":"reorder","track":v,"after":s.active_sequence().unwrap().video_tracks[1].id.0}),
        json!({"type":"set","track":v,"changes":{"locked":false,"name":"Implicit unlock"}}),
    ]
    .into_iter()
    .enumerate()
    {
        read(&mut s, 60700 + i);
        let batch = request(&mut s, 60700 + i, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    step(&mut s, 60710, json!({"type":"set","track":v,"changes":{"locked":false}}), "timeline.setTrack", json!({"track":v,"locked":false}));
}
#[test]
fn track_digest_scope_stale_ticket_and_mixed_nonexact_batches_cannot_publish() {
    for mode in 0..5 {
        let mut s = setup();
        read(&mut s, 60800);
        let v = s.active_sequence().unwrap().video_tracks[0].id.0;
        let mut batch = request(&mut s, 60800, json!({"type":"set","track":v,"changes":{"name":"Exact"}}));
        match mode {
            0 => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
            1 => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(999999),
            2 => batch["exact"] = json!(false),
            3 => {
                let second = batch["operations"][0].clone();
                batch["operations"].as_array_mut().unwrap().push(second);
            }
            _ => {
                s.execute("timeline.setTrack", json!({"track":v,"name":"Later human"})).unwrap();
            }
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn protected_track_contents_and_exhausted_id_space_refuse_without_any_partial_structural_change() {
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].id.0;
    let clip = q.video_tracks[0].items[0].id.0;
    s.execute("poema.protectClips", json!({"clips":[clip],"reason":"Human decision"})).unwrap();
    read(&mut s, 60900);
    let batch = request(&mut s, 60900, json!({"type":"remove","tracks":[v]}));
    let before = state(&s);
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    read(&mut s, 60901);
    let batch = request(
        &mut s,
        60901,
        json!({"type":"add","video":1,"audio":0,"submix":0,"video_after":null,"audio_after":null,"submix_after":null,"audio_channels":"Stereo","submix_channels":"Stereo"}),
    );
    let before = state(&s);
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn native_human_submix_reorder_refuses_to_drop_existing_output_or_send_routes() {
    for send in [false, true] {
        let mut s = setup();
        s.execute("sequence.addTracks", json!({"video":0,"submix":2})).unwrap();
        let sid = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
        let target = q.submix_tracks[1].id;
        if send {
            q.submix_tracks[0].mixer.sends.push(TrackSend::new(target));
        } else {
            q.submix_tracks[0].mixer.output = Some(target);
        }
        let id = q.submix_tracks[0].id.0;
        let before = state(&s);
        assert!(s.execute("sequence.reorderTrack", json!({"track":id,"after":target.0})).is_err());
        assert_eq!(state(&s), before);
        read(&mut s, 61000);
        let batch = request(&mut s, 61000, json!({"type":"reorder","track":id,"after":target.0}));
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn native_track_deletion_repairs_transition_selection_without_deleting_neighbors() {
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let track = q.video_tracks.iter().find(|t| !t.transitions.is_empty()).unwrap();
    let id = track.id.0;
    let transition = track.transitions[0].id;
    s.state.transition_selection = vec![transition];
    let r = step(&mut s, 61100, json!({"type":"remove","tracks":[id]}), "sequence.removeTracks", json!({"tracks":[id]}));
    assert!(s.state.transition_selection.is_empty());
    assert!(r["collateral"]["removedTransitions"].as_u64().unwrap() > 0);
}
