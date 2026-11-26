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

fn insert_fixture() -> (Session, u64, u64) {
    let mut s = setup();
    let anchor = s.state.active_sequence.unwrap().0;
    s.execute(
        "project.associateEditorialOwner",
        json!({"scope":{"projectId":"p_tracks","anchorSequence":anchor},"workspaceId":"00000000-0000-4000-8000-000000000001"}),
    )
    .unwrap();
    let audio = s.active_sequence().unwrap().audio_tracks[0].id.0;
    let bus = s.execute("mixer.addSubmix", json!({"name":"Insert test bus"})).unwrap()["id"].as_u64().unwrap();
    for strip in [json!(audio), json!(bus), json!("Mix")] {
        for _ in 0..2 {
            s.execute("mixer.addInsert", json!({"strip":strip,"effect":"amplify"})).unwrap();
        }
    }
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    for id in [TrackId(audio), TrackId(bus), filmcraft_project::mixer::MASTER_STRIP] {
        q.with_strip_mut(id, |t| {
            let p = t.effects[1].params.get_mut("gain").unwrap();
            p.keyframes.push(filmcraft_project::Keyframe::new(filmcraft_time::Tick(254016000000), ParamValue::Float(4.0)));
            p.keyframes.push(filmcraft_project::Keyframe::new(filmcraft_time::Tick(508032000000), ParamValue::Float(8.0)));
        })
        .unwrap();
    }
    (s, audio, bus)
}
fn insert_page(s: &mut Session, n: usize, strip: Value, offset: usize, limit: usize) -> Value {
    s.execute("agent.readTrackInserts", json!({"token":token(n),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"strip":strip,"offset":offset,"limit":limit})).unwrap()
}
#[test]
fn track_insert_pages_cover_audio_submix_and_explicit_master_with_exact_sequence_keys() {
    let (mut s, audio, bus) = insert_fixture();
    let doc = read(&mut s, 71000);
    assert_eq!(doc["capabilities"]["trackInsertTools"], true);
    let before = state(&s);
    let track_hash = page(&mut s, 71000, 0, 1)["sha256"].clone();
    for strip in [json!(audio), json!(bus), Value::Null] {
        let first = insert_page(&mut s, 71000, strip.clone(), 0, 1);
        let second = insert_page(&mut s, 71000, strip.clone(), 1, 1);
        assert_eq!(first["strip"], strip);
        assert_eq!(first["total"], 2);
        assert_eq!(first["nextOffset"], 1);
        assert_eq!(first["sha256"], track_hash);
        assert_eq!(second["nextOffset"], Value::Null);
        assert_eq!(second["effects"][0]["ordinal"], 2);
        let param = &second["effects"][0]["params"]["gain"];
        assert_eq!(param["definition"]["default"], json!({"Float":0.0}));
        assert_eq!(param["keyframes"][0]["time"], 254016000000_i64);
        assert_eq!(param["keyframes"][1]["time"], 508032000000_i64);
        assert_eq!(second["timeDomain"], "native_sequence_ticks");
        assert_eq!(second["referenceScope"], "exact_read_ticket_only");
        assert_eq!(insert_page(&mut s, 71000, strip, 2, 1)["effects"], json!([]));
    }
    assert_eq!(state(&s), before);
}
#[test]
fn track_insert_edits_match_full_human_audio_submix_master_state_and_owned_history() {
    let (mut s, audio, bus) = insert_fixture();
    for (index, strip) in [Some(audio), Some(bus), None].into_iter().enumerate() {
        let original = s.active_sequence().unwrap().clone();
        let human_strip = strip.map_or_else(|| json!("Mix"), |id| json!(id));
        let result = step(
            &mut s,
            71100 + index * 10,
            json!({"type":"set_insert","strip":strip,"slot":1,"changes":{"enabled":false,"postFader":true,"params":{"gain":{"Float":6.0}}}}),
            "mixer.setInsert",
            json!({"strip":human_strip,"slot":1,"enabled":false,"postFader":true,"params":{"gain":6.0}}),
        );
        assert_eq!(result["tracks"], strip.map_or_else(|| json!([]), |id| json!([id])));
        assert_eq!(result["collateral"], json!({"removedClips":0,"removedCues":0,"removedTransitions":0,"removedSends":0,"reroutedOutputs":0}));
        let q = s.active_sequence().unwrap();
        let effects = |q: &filmcraft_project::Sequence| -> Vec<filmcraft_project::EffectInstance> {
            strip.map_or_else(|| q.master_effects.clone(), |id| q.mix_track(TrackId(id)).unwrap().effects.clone())
        };
        let before = effects(&original);
        let after = effects(q);
        assert_eq!(after[0], before[0]);
        assert_eq!(after[1].params["gain"].keyframes, before[1].params["gain"].keyframes);
        assert_eq!(after[1].params["gain"].value, ParamValue::Float(6.0));
        read(&mut s, 71103 + index * 10);
        let readback = insert_page(&mut s, 71103 + index * 10, json!(strip), 1, 1);
        assert_eq!(readback["sha256"], result["afterSha256"]);
    }
}
#[test]
fn track_insert_hostile_reads_refuse_without_touching_project_editor_or_history() {
    let (mut s, audio, _) = insert_fixture();
    read(&mut s, 71200);
    let base =
        json!({"token":token(71200),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"strip":audio,"offset":0,"limit":1});
    let video = s.active_sequence().unwrap().video_tracks[0].id.0;
    let mut bads = Vec::new();
    for (key, value) in [
        ("strip", json!(0)),
        ("strip", json!(video)),
        ("strip", json!(u64::MAX)),
        ("strip", json!("Mix")),
        ("offset", json!(3)),
        ("offset", json!(-1)),
        ("limit", json!(0)),
        ("limit", json!(5)),
        ("projection", json!("true")),
        ("extra", json!(true)),
    ] {
        let mut p = base.clone();
        p[key] = value;
        bads.push(p);
    }
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("strip");
    bads.push(missing);
    let mut foreign = base.clone();
    foreign["scope"]["projectId"] = json!("p_other");
    bads.push(foreign);
    for p in bads {
        let before = state(&s);
        let diagnostic = p.to_string();
        assert!(s.execute("agent.readTrackInserts", p).is_err(), "{diagnostic}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn track_insert_hostile_changes_preserve_human_redo_and_all_collateral() {
    let (mut s, audio, _) = insert_fixture();
    s.execute("timeline.setTrack", json!({"track":audio,"name":"Human redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let video = s.active_sequence().unwrap().video_tracks[0].id.0;
    let base = json!({"type":"set_insert","strip":audio,"slot":1,"changes":{"params":{"gain":{"Float":6.0}}}});
    let mut bads = Vec::new();
    for changes in [
        json!({}),
        json!({"enabled":"false"}),
        json!({"postFader":0}),
        json!({"params":{}}),
        json!({"params":{"gain":{"Float":25.0}}}),
        json!({"params":{"gain":{"Bool":true}}}),
        json!({"params":{"unknown":{"Float":1.0}}}),
        json!({"params":{"gain":{"Text":"/file"}}}),
        json!({"params":{"gain":{"Vec2":{"x":null,"y":0.0}}}}),
        json!({"extra":true}),
    ] {
        let mut e = base.clone();
        e["changes"] = changes;
        bads.push(e);
    }
    for (key, value) in [
        ("strip", json!(video)),
        ("strip", json!(0)),
        ("strip", json!(u64::MAX)),
        ("slot", json!(2)),
        ("slot", json!(4096)),
        ("slot", json!(-1)),
        ("extra", json!(true)),
    ] {
        let mut e = base.clone();
        e[key] = value;
        bads.push(e);
    }
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("strip");
    bads.push(missing);
    for (i, e) in bads.into_iter().enumerate() {
        let n = 71300 + i;
        read(&mut s, n);
        let batch = request(&mut s, n, e);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(!s.history.redo.is_empty());
}
#[test]
fn track_insert_stale_digest_ticket_and_locked_strip_never_publish() {
    for mode in ["digest", "ticket", "locked", "scope", "mixed", "nonexact"] {
        let (mut s, audio, _) = insert_fixture();
        if mode == "locked" {
            s.execute("timeline.setTrack", json!({"track":audio,"locked":true})).unwrap();
        }
        read(&mut s, 71400);
        let mut batch = request(&mut s, 71400, json!({"type":"set_insert","strip":audio,"slot":1,"changes":{"enabled":false}}));
        match mode {
            "digest" => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
            "ticket" => {
                s.execute("mixer.setInsert", json!({"strip":"Mix","slot":0,"enabled":false})).unwrap();
            }
            "scope" => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(999999),
            "mixed" => {
                let next = batch["operations"][0].clone();
                batch["operations"].as_array_mut().unwrap().push(next);
            }
            "nonexact" => batch["exact"] = json!(false),
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
    }
}
#[test]
fn track_insert_noop_and_unread_master_sentinel_refuse_without_history_steps() {
    let (mut s, audio, _) = insert_fixture();
    for (i, edit) in [
        json!({"type":"set_insert","strip":audio,"slot":0,"changes":{"enabled":true}}),
        json!({"type":"set_insert","strip":filmcraft_project::mixer::MASTER_STRIP.0,"slot":0,"changes":{"enabled":false}}),
    ]
    .into_iter()
    .enumerate()
    {
        read(&mut s, 71500 + i);
        let batch = request(&mut s, 71500 + i, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn track_insert_projection_is_readonly_and_changed_unpaged_master_invalidates_old_ticket() {
    let (mut s, audio, _) = insert_fixture();
    read(&mut s, 71600);
    let p = json!({"token":token(71600),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"strip":audio,"offset":0,"limit":1,"projection":true});
    let before = state(&s);
    let r = s.execute("agent.readTrackInserts", p.clone()).unwrap();
    assert_eq!(r["requiresSourceValidation"], true);
    assert_eq!(state(&s), before);
    s.execute("mixer.setInsert", json!({"strip":"Mix","slot":1,"postFader":true})).unwrap();
    let after = state(&s);
    assert!(s.execute("agent.readTrackInserts", p).is_err());
    assert_eq!(state(&s), after);
    read(&mut s, 71601);
    assert_ne!(insert_page(&mut s, 71601, json!(audio), 0, 1)["sha256"], r["sha256"]);
}

fn owned_insert_stack(s: &Session, strip: Option<u64>) -> Vec<filmcraft_project::EffectInstance> {
    let q = s.active_sequence().unwrap();
    strip.map_or_else(|| q.master_effects.clone(), |id| q.mix_track(TrackId(id)).unwrap().effects.clone())
}
#[test]
fn track_insert_add_matches_shared_human_defaults_positions_audio_submix_master_and_history() {
    for which in 0..3 {
        for slot in 0..=2 {
            let (mut s, audio, bus) = insert_fixture();
            let strip = [Some(audio), Some(bus), None][which];
            let before = owned_insert_stack(&s, strip);
            let native_strip = strip.map_or_else(|| json!("Mix"), |id| json!(id));
            let receipt = step(
                &mut s,
                72000 + which * 100 + slot * 10,
                json!({"type":"add_insert","strip":strip,"slot":slot,"definition":"amplify","postFader":true}),
                "mixer.addInsert",
                json!({"strip":native_strip,"slot":slot,"effect":"amplify","postFader":true}),
            );
            let after = owned_insert_stack(&s, strip);
            assert_eq!(after.len(), before.len() + 1);
            let mut expected = before;
            let mut effect = filmcraft_project::find_effect("amplify").unwrap().instance();
            effect.post_fader = true;
            expected.insert(slot, effect);
            assert_eq!(after, expected);
            assert_eq!(receipt["tracks"], strip.map_or_else(|| json!([]), |id| json!([id])));
            assert!(receipt["collateral"].as_object().unwrap().values().all(|v| v.as_u64() == Some(0)));
        }
    }
}
#[test]
fn track_insert_removal_matches_shared_human_complete_effect_animation_loss_and_owned_history() {
    for which in 0..3 {
        for slot in 0..2 {
            let (mut s, audio, bus) = insert_fixture();
            let strip = [Some(audio), Some(bus), None][which];
            let before = owned_insert_stack(&s, strip);
            let native_strip = strip.map_or_else(|| json!("Mix"), |id| json!(id));
            let receipt = step(
                &mut s,
                72400 + which * 100 + slot * 10,
                json!({"type":"remove_insert","strip":strip,"slot":slot}),
                "mixer.removeInsert",
                json!({"strip":native_strip,"slot":slot}),
            );
            let after = owned_insert_stack(&s, strip);
            let mut expected = before;
            expected.remove(slot);
            assert_eq!(after, expected);
            assert_eq!(receipt["tracks"], strip.map_or_else(|| json!([]), |id| json!([id])));
            assert!(receipt["collateral"].as_object().unwrap().values().all(|v| v.as_u64() == Some(0)));
        }
    }
}
#[test]
fn track_insert_structure_rejects_clamping_wrong_types_unknown_definitions_and_implicit_master() {
    let (mut s, audio, _) = insert_fixture();
    let video = s.active_sequence().unwrap().video_tracks[0].id.0;
    s.execute("timeline.setTrack", json!({"track":audio,"name":"Human redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let add = json!({"type":"add_insert","strip":audio,"slot":1,"definition":"amplify","postFader":false});
    let remove = json!({"type":"remove_insert","strip":audio,"slot":1});
    let mut bads = Vec::new();
    for (key, value) in [
        ("definition", json!("unknown_audio_definition")),
        ("definition", json!("gaussian_blur")),
        ("definition", json!("")),
        ("definition", json!("x".repeat(257))),
        ("postFader", json!("false")),
        ("slot", json!(3)),
        ("slot", json!(4096)),
        ("slot", json!(-1)),
        ("strip", json!(video)),
        ("strip", json!(u64::MAX)),
        ("params", json!({})),
    ] {
        let mut e = add.clone();
        e[key] = value;
        bads.push(e);
    }
    for (key, value) in
        [("slot", json!(2)), ("slot", json!(4096)), ("strip", json!(0)), ("strip", json!(video)), ("strip", json!(u64::MAX)), ("extra", json!(true))]
    {
        let mut e = remove.clone();
        e[key] = value;
        bads.push(e);
    }
    for mut e in [add, remove] {
        e.as_object_mut().unwrap().remove("strip");
        bads.push(e);
    }
    for (i, e) in bads.into_iter().enumerate() {
        let n = 72800 + i;
        read(&mut s, n);
        let batch = request(&mut s, n, e);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(!s.history.redo.is_empty());
}
#[test]
fn track_insert_structure_checks_real_native_capacity_empty_stacks_and_locked_strips() {
    for mode in ["full", "empty", "locked"] {
        let (mut s, audio, _) = insert_fixture();
        if mode == "full" {
            while owned_insert_stack(&s, Some(audio)).len() < filmcraft_project::mixer::MAX_INSERTS {
                s.execute("mixer.addInsert", json!({"strip":audio,"effect":"amplify"})).unwrap();
            }
        }
        if mode == "empty" {
            while !owned_insert_stack(&s, Some(audio)).is_empty() {
                s.execute("mixer.removeInsert", json!({"strip":audio,"slot":0})).unwrap();
            }
        }
        if mode == "locked" {
            s.execute("timeline.setTrack", json!({"track":audio,"locked":true})).unwrap();
        }
        let edit = if mode == "empty" {
            json!({"type":"remove_insert","strip":audio,"slot":0})
        } else {
            json!({"type":"add_insert","strip":audio,"slot":0,"definition":"amplify","postFader":false})
        };
        read(&mut s, 73000);
        let batch = request(&mut s, 73000, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
        if mode == "locked" {
            read(&mut s, 73001);
            let batch = request(&mut s, 73001, json!({"type":"remove_insert","strip":audio,"slot":1}));
            let before = state(&s);
            assert!(s.execute("agent.apply", batch).is_err());
            assert_eq!(state(&s), before);
        }
        if mode == "empty" {
            step(
                &mut s,
                73010,
                json!({"type":"add_insert","strip":audio,"slot":0,"definition":"amplify","postFader":false}),
                "mixer.addInsert",
                json!({"strip":audio,"slot":0,"effect":"amplify","postFader":false}),
            );
        }
    }
}
#[test]
fn track_insert_structure_stale_scope_and_nonexact_batches_preserve_complete_state() {
    for which in ["add_insert", "remove_insert"] {
        for mode in ["digest", "ticket", "scope", "project", "mixed", "nonexact"] {
            let (mut s, audio, _) = insert_fixture();
            read(&mut s, 73100);
            let mut e = json!({"type":which,"strip":audio,"slot":1});
            if which == "add_insert" {
                e["definition"] = json!("amplify");
                e["postFader"] = json!(false);
            }
            let mut batch = request(&mut s, 73100, e);
            match mode {
                "digest" => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
                "ticket" => {
                    s.execute("mixer.setInsert", json!({"strip":"Mix","slot":0,"enabled":false})).unwrap();
                }
                "scope" => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(999999),
                "project" => batch["operations"][0]["params"]["scope"]["projectId"] = json!("p_other"),
                "mixed" => {
                    let second = batch["operations"][0].clone();
                    batch["operations"].as_array_mut().unwrap().push(second);
                }
                _ => batch["exact"] = json!(false),
            }
            let before = state(&s);
            assert!(s.execute("agent.apply", batch).is_err(), "{which}/{mode}");
            assert_eq!(state(&s), before, "{which}/{mode}");
        }
    }
}

#[test]
fn track_insert_creation_covers_every_current_human_dsp_audio_catalog_default() {
    let definitions = filmcraft_project::effect_defs()
        .iter()
        .filter(|d| d.kind == filmcraft_project::EffectKind::Audio && !d.intrinsic && filmcraft_render::audio_fx::supported(d.id))
        .collect::<Vec<_>>();
    assert!(!definitions.is_empty());
    for (i, definition) in definitions.iter().enumerate() {
        assert!(super::effect_stack::reason(definition).is_none(), "DSP menu resource default requires its own admission: {}", definition.id);
        let (mut s, audio, _) = insert_fixture();
        let before = owned_insert_stack(&s, Some(audio));
        step(
            &mut s,
            74000 + i * 10,
            json!({"type":"add_insert","strip":audio,"slot":1,"definition":definition.id,"postFader":false}),
            "mixer.addInsert",
            json!({"strip":audio,"slot":1,"effect":definition.id,"postFader":false}),
        );
        let after = owned_insert_stack(&s, Some(audio));
        assert_eq!(after[0], before[0]);
        assert_eq!(after[2], before[1]);
        assert_eq!(after[1], definition.instance());
    }
    println!("qualified actual human DSP-backed insert defaults: {}", definitions.len());
}

fn mixer_route_fixture() -> (Session, u64, u64, u64) {
    let (mut s, a, one) = insert_fixture();
    let two = s.execute("mixer.addSubmix", json!({"name":"Later bus"})).unwrap()["id"].as_u64().unwrap();
    (s, a, one, two)
}
#[test]
fn mixer_route_settings_match_human_audio_submix_master_fields_history_and_current_capability() {
    for which in 0..3 {
        let (mut s, a, one, two) = mixer_route_fixture();
        let doc = read(&mut s, 76000);
        assert_eq!(doc["capabilities"]["mixerRoutingTools"], true);
        let strip = [Some(a), Some(one), None][which];
        let changes = if strip.is_none() {
            json!({"volumeDb":-8.0,"mode":"Touch"})
        } else {
            json!({"name":"Original sound mix","volumeDb":-7.0,"pan":25.0,"muted":true,"solo":true,"recordArm":true,"soloSafe":true,"mode":"Touch","inputMap":"Left","channels":"Mono","output":two})
        };
        let mut args = changes.clone();
        args["strip"] = strip.map_or_else(|| json!("Mix"), |id| json!(id));
        let result = step(&mut s, 76010, json!({"type":"set_mixer","strip":strip,"changes":changes}), "mixer.setStrip", args);
        assert_eq!(result["tracks"], strip.map_or_else(|| json!([]), |id| json!([id])));
        assert_eq!(result["collateral"]["removedSends"], 0);
    }
}
#[test]
fn mixer_route_output_changes_report_actual_rerouting_and_preserve_the_remaining_graph() {
    let (mut s, a, one, two) = mixer_route_fixture();
    s.execute("mixer.setStrip", json!({"strip":a,"output":one})).unwrap();
    for (i, target) in [Some(two), None].into_iter().enumerate() {
        let result = step(
            &mut s,
            76100 + i * 10,
            json!({"type":"set_mixer","strip":a,"changes":{"output":target}}),
            "mixer.setStrip",
            json!({"strip":a,"output":target}),
        );
        assert_eq!(result["collateral"]["reroutedOutputs"], 1);
        assert_eq!(s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().mixer.output, target.map(TrackId));
    }
}
#[test]
fn mixer_route_send_creation_and_settings_share_human_values_and_keep_existing_animation() {
    let (mut s, a, one, two) = mixer_route_fixture();
    let result = step(
        &mut s,
        76200,
        json!({"type":"add_send","strip":a,"target":one,"levelDb":-6.0,"pan":31.0,"preFader":true}),
        "mixer.addSend",
        json!({"strip":a,"target":one,"levelDb":-6.0,"pan":31.0,"preFader":true}),
    );
    assert_eq!(result["collateral"]["removedSends"], 0);
    let slot = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().mixer.sends.len() - 1;
    let lane = format!("send.{slot}.level");
    for (time, value) in [(0, -10.0), (254016000000_i64, -12.0)] {
        s.execute("mixer.setKeyframe", json!({"strip":a,"lane":lane,"time":time,"value":value})).unwrap();
    }
    let before = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().mixer.lanes.clone();
    let result = step(
        &mut s,
        76210,
        json!({"type":"set_send","strip":a,"slot":slot,"changes":{"target":two,"levelDb":-3.0,"pan":-25.0,"preFader":false,"muted":true}}),
        "mixer.setSend",
        json!({"strip":a,"send":slot,"target":two,"levelDb":-3.0,"pan":-25.0,"preFader":false,"muted":true}),
    );
    let t = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap();
    assert_eq!(t.mixer.lanes, before);
    assert_eq!(t.mixer.sends[slot].target, TrackId(two));
    assert_eq!(t.mixer.sends[slot].level_db, -3.0);
    assert!(t.mixer.sends[slot].muted);
    assert_eq!(result["collateral"]["removedSends"], 0);
}
#[test]
fn mixer_route_send_removal_reports_owned_loss_and_remaps_later_keys_with_native_history() {
    let (mut s, a, one, two) = mixer_route_fixture();
    for target in [one, two] {
        s.execute("mixer.addSend", json!({"strip":a,"target":target})).unwrap();
    }
    for (time, value) in [(0, -9.0), (254016000000_i64, -11.0)] {
        s.execute("mixer.setKeyframe", json!({"strip":a,"lane":"send.1.level","time":time,"value":value})).unwrap();
    }
    let before = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().clone();
    let result = step(&mut s, 76300, json!({"type":"remove_send","strip":a,"slot":0}), "mixer.removeSend", json!({"strip":a,"send":0}));
    let after = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap();
    assert_eq!(after.mixer.lanes["send.0.level"], before.mixer.lanes["send.1.level"]);
    assert!(!after.mixer.lanes.contains_key("send.1.level"));
    assert_eq!(result["collateral"], json!({"removedClips":0,"removedCues":0,"removedTransitions":0,"removedSends":1,"reroutedOutputs":0}));
}
#[test]
fn mixer_route_bad_values_master_ignored_fields_aliases_and_graphs_preserve_human_redo() {
    let (mut s, a, one, two) = mixer_route_fixture();
    s.execute("mixer.addSend", json!({"strip":a,"target":one})).unwrap();
    s.execute("mixer.setStrip", json!({"strip":a,"name":"Human redo"})).unwrap();
    s.undo().unwrap();
    let video = s.active_sequence().unwrap().video_tracks[0].id.0;
    let mut bads = Vec::new();
    for changes in [
        json!({}),
        json!({"volumeDb":15.001}),
        json!({"pan":-100.001}),
        json!({"recordArm":0}),
        json!({"name":"x".repeat(1025)}),
        json!({"name":"bad\0name"}),
        json!({"mode":"unknown"}),
        json!({"inputMap":"unknown"}),
        json!({"channels":"unknown"}),
        json!({"output":"Mix"}),
        json!({"output":video}),
        json!({"output":a}),
    ] {
        bads.push(json!({"type":"set_mixer","strip":a,"changes":changes}));
    }
    for changes in [json!({"name":"Ignored"}), json!({"pan":25.0,"volumeDb":-5.0}), json!({"muted":true}), json!({"channels":"Mono"})] {
        bads.push(json!({"type":"set_mixer","strip":null,"changes":changes}));
    }
    for edit in [
        json!({"type":"set_mixer","strip":two,"changes":{"output":one}}),
        json!({"type":"add_send","strip":a,"target":video,"levelDb":0,"pan":0,"preFader":false}),
        json!({"type":"add_send","strip":two,"target":one,"levelDb":0,"pan":0,"preFader":false}),
        json!({"type":"add_send","strip":a,"target":one,"levelDb":16,"pan":0,"preFader":false}),
        json!({"type":"add_send","strip":null,"target":one,"levelDb":0,"pan":0,"preFader":false}),
        json!({"type":"set_send","strip":a,"slot":0,"changes":{"levelDb":-97}}),
        json!({"type":"set_send","strip":a,"slot":0,"changes":{"target":null}}),
        json!({"type":"set_send","strip":a,"slot":0,"changes":{"muted":0}}),
        json!({"type":"set_send","strip":a,"slot":0,"changes":{}}),
        json!({"type":"remove_send","strip":a,"slot":256}),
        json!({"type":"remove_send","strip":u64::MAX,"slot":0}),
    ] {
        bads.push(edit);
    }
    for (i, e) in bads.into_iter().enumerate() {
        let n = 76400 + i;
        read(&mut s, n);
        let batch = request(&mut s, n, e);
        let before = state(&s);
        let redo = s.history.redo.clone();
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(s.history.redo.len(), redo.len());
        assert!(s.history.redo.iter().zip(&redo).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
    }
}
#[test]
fn mixer_route_stale_locked_full_nonexact_and_foreign_requests_never_publish() {
    for kind in ["set_mixer", "add_send", "set_send", "remove_send"] {
        for mode in ["digest", "ticket", "scope", "foreign", "locked", "nonexact", "mixed", "capacity"] {
            if mode == "capacity" && kind != "add_send" {
                continue;
            }
            let (mut s, a, one, _) = mixer_route_fixture();
            s.execute("mixer.addSend", json!({"strip":a,"target":one})).unwrap();
            if mode == "locked" {
                s.execute("timeline.setTrack", json!({"track":a,"locked":true})).unwrap();
            }
            if mode == "capacity" {
                while s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().mixer.sends.len() < filmcraft_project::mixer::MAX_SENDS {
                    s.execute("mixer.addSend", json!({"strip":a,"target":one})).unwrap();
                }
            }
            let edit = match kind {
                "set_mixer" => json!({"type":kind,"strip":a,"changes":{"volumeDb":-7.0}}),
                "add_send" => json!({"type":kind,"strip":a,"target":one,"levelDb":-6.0,"pan":0.0,"preFader":false}),
                "set_send" => json!({"type":kind,"strip":a,"slot":0,"changes":{"muted":true}}),
                _ => json!({"type":kind,"strip":a,"slot":0}),
            };
            read(&mut s, 76600);
            let mut batch = request(&mut s, 76600, edit);
            match mode {
                "digest" => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
                "ticket" => {
                    s.execute("mixer.setStrip", json!({"strip":a,"name":"Later human"})).unwrap();
                }
                "scope" => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(999999),
                "foreign" => batch["operations"][0]["params"]["scope"]["projectId"] = json!("p_other"),
                "nonexact" => batch["exact"] = json!(false),
                "mixed" => {
                    let second = batch["operations"][0].clone();
                    batch["operations"].as_array_mut().unwrap().push(second);
                }
                _ => {}
            }
            let before = state(&s);
            assert!(s.execute("agent.apply", batch).is_err(), "{kind}/{mode}");
            assert_eq!(state(&s), before, "{kind}/{mode}");
        }
    }
}

fn automation_lane_index(s: &mut Session, n: usize, strip: Option<u64>, key: &str) -> usize {
    read(s, n);
    let mut offset = 0;
    loop {
        let r=s.execute("agent.readMixerAutomation",json!({"token":token(n),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"strip":strip,"lane":null,"offset":offset,"limit":16})).unwrap();
        if let Some(row) = r["lanes"].as_array().unwrap().iter().find(|r| r["key"] == key && r["nativeControlAvailable"] == true) {
            return row["position"].as_u64().unwrap() as usize - 1;
        }
        match r["nextOffset"].as_u64() {
            Some(v) => offset = v as usize,
            None => panic!("missing native lane {key}"),
        }
    }
}
fn automation_edit(s: &Session, strip: Option<u64>, lane: usize, gesture: Value) -> Value {
    json!({"type":"automation","strip":strip,"lane":lane,"clock":s.playhead().0,"gesture":gesture})
}
fn automation_refused(s: &mut Session, n: usize, edit: Value) {
    read(s, n);
    let batch = request(s, n, edit);
    let before = state(s);
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(s), before);
}
#[test]
fn mixer_automation_exact_control_matches_human_static_and_read_mode_for_every_strip_kind() {
    for which in 0..3 {
        let (mut s, a, b) = insert_fixture();
        let strip = [Some(a), Some(b), None][which];
        let native = strip.map_or_else(|| json!("Mix"), |v| json!(v));
        for (time, value) in [(10, -6), (20, -8)] {
            s.execute("mixer.setKeyframe", json!({"strip":native,"time":time,"value":value})).unwrap();
        }
        let n = 100000 + which * 100;
        let lane = automation_lane_index(&mut s, n, strip, "volume");
        let edit = automation_edit(&s, strip, lane, json!({"action":"set_value","time":15,"value":-12}));
        let result = step(&mut s, n + 1, edit, "mixer.setValue", json!({"strip":native,"time":15,"value":-12}));
        assert_eq!(result["automation"]["insertedKeyframes"], 1);
        assert_eq!(result["automation"]["staticValueChanged"], false);
        s.execute("mixer.setStrip", json!({"strip":native,"mode":"Off"})).unwrap();
        let lane = automation_lane_index(&mut s, n + 10, strip, "volume");
        let edit = automation_edit(&s, strip, lane, json!({"action":"set_value","time":16,"value":-15}));
        let result = step(&mut s, n + 11, edit, "mixer.setValue", json!({"strip":native,"time":16,"value":-15}));
        assert_eq!(result["automation"]["staticValueChanged"], true);
        assert_eq!(result["automation"]["beforeKeyframes"], result["automation"]["afterKeyframes"]);
    }
}
#[test]
fn mixer_automation_exact_key_set_preserves_native_bases_and_mute_hold_semantics() {
    let (mut s, a, b) = insert_fixture();
    let doc = read(&mut s, 100400);
    assert_eq!(doc["capabilities"]["mixerAutomationTools"], true);
    for (i, strip) in [Some(a), Some(b), None].into_iter().enumerate() {
        let n = 100410 + i * 10;
        let lane = automation_lane_index(&mut s, n, strip, "volume");
        let edit = automation_edit(&s, strip, lane, json!({"action":"set_keyframe","time":33,"value":-6}));
        let result = step(&mut s, n + 1, edit, "mixer.setKeyframe", json!({"strip":strip.map_or_else(||json!("Mix"),|v|json!(v)),"time":33,"value":-6}));
        assert_eq!(result["automation"]["beforeKeyframes"], 0);
        assert_eq!(result["automation"]["afterKeyframes"], 1);
        assert_eq!(result["automation"]["storedValueChanged"], false);
    }
    let lane = automation_lane_index(&mut s, 100450, Some(a), "mute");
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":20,"value":1}));
    step(&mut s, 100451, edit, "mixer.setKeyframe", json!({"strip":a,"lane":"mute","time":20,"value":true}));
    assert!(
        s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().lane_keyframes("mute").iter().all(|k| k.interp == filmcraft_project::Interpolation::Hold)
    );
}
#[test]
fn mixer_automation_exact_key_move_preserves_original_interpolation_influences_and_history() {
    let (mut s, a, _) = insert_fixture();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let p = q.mix_track_mut(TrackId(a)).unwrap().effects[1].params.get_mut("gain").unwrap();
    p.keyframes[0].interp = filmcraft_project::Interpolation::ContinuousBezier;
    p.keyframes[0].in_influence = 0.11;
    p.keyframes[0].out_influence = 0.79;
    let lane = automation_lane_index(&mut s, 100500, Some(a), "fx.1.gain");
    let t = 254016000000_i64;
    let nt = 381024000000_i64;
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"move_keyframe","time":t,"new_time":nt,"value":7}));
    let result = step(&mut s, 100501, edit, "mixer.moveKeyframe", json!({"strip":a,"lane":"fx.1.gain","time":t,"newTime":nt,"value":7}));
    assert_eq!(result["automation"]["removedKeyframes"], 1);
    assert_eq!(result["automation"]["insertedKeyframes"], 1);
    let k = &s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().effects[1].params["gain"].keyframes[0];
    assert_eq!(k.interp, filmcraft_project::Interpolation::ContinuousBezier);
    assert_eq!((k.in_influence, k.out_influence), (0.11, 0.79));
}
#[test]
fn mixer_automation_exact_delete_and_clear_disclose_actual_keys_values_and_lane_removal() {
    let (mut s, a, b) = insert_fixture();
    s.execute("mixer.setKeyframe", json!({"strip":a,"time":17,"value":-6})).unwrap();
    let lane = automation_lane_index(&mut s, 100600, Some(a), "volume");
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"delete_keyframe","time":17}));
    let result = step(&mut s, 100601, edit, "mixer.deleteKeyframe", json!({"strip":a,"time":17}));
    assert_eq!(result["automation"]["removedKeyframes"], 1);
    assert_eq!(result["automation"]["storedValueChanged"], true);
    assert_eq!(result["automation"]["staticValueChanged"], false);
    for (i, strip) in [Some(a), Some(b), None].into_iter().enumerate() {
        let native = strip.map_or_else(|| json!("Mix"), |v| json!(v));
        for (t, v) in [(0, -4), (1016064000000_i64, -12)] {
            s.execute("mixer.setKeyframe", json!({"strip":native,"time":t,"value":v})).unwrap();
        }
        s.execute("playhead.set", json!({"seconds":2})).unwrap();
        let n = 100620 + i * 10;
        let lane = automation_lane_index(&mut s, n, strip, "volume");
        let edit = automation_edit(&s, strip, lane, json!({"action":"clear_lane"}));
        let result = step(&mut s, n + 1, edit, "mixer.clearLane", json!({"strip":native}));
        assert_eq!(result["automation"]["afterKeyframes"], 0);
        assert_eq!(result["automation"]["removedKeyframes"], 2);
        assert_eq!(result["automation"]["storedLaneRemoved"], true);
    }
}
#[test]
fn mixer_automation_exact_point_write_uses_native_thinning_sample_boundaries_and_loss_receipt() {
    let (mut s, a, _) = insert_fixture();
    let second = 254016000000_i64;
    for (t, v) in [(0, -10), (second * 3, -17), (second * 8, -10)] {
        s.execute("mixer.setKeyframe", json!({"strip":a,"time":t,"value":v})).unwrap();
    }
    let lane = automation_lane_index(&mut s, 100700, Some(a), "volume");
    let points = json!([[second * 2, -3], [second * 3, -5], [second * 4, -7]]);
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"write_automation","points":points,"tolerance":0.01}));
    let result = step(&mut s, 100701, edit, "mixer.writeAutomation", json!({"strip":a,"points":points,"tolerance":0.01}));
    let x = &result["automation"];
    assert_eq!(x["inputPoints"], 3);
    assert_eq!(x["keptPoints"], 2);
    assert_eq!(x["removedKeyframes"], 1);
    assert_eq!(x["insertedKeyframes"], 4);
    assert_eq!(x["beforeKeyframes"], 3);
    assert_eq!(x["afterKeyframes"], 6);
}
#[test]
fn mixer_automation_exact_refuses_clamps_malformed_actions_noops_and_locked_strips_without_redo_loss() {
    let (mut s, a, _) = insert_fixture();
    s.execute("mixer.setStrip", json!({"strip":a,"pan":-30})).unwrap();
    s.undo().unwrap();
    let lane = automation_lane_index(&mut s, 100800, Some(a), "volume");
    let unchanged = s.active_sequence().unwrap().mix_track(TrackId(a)).unwrap().volume_db;
    for (i, gesture) in [
        json!({"action":"set_value","time":0,"value":16}),
        json!({"action":"set_keyframe","time":-1,"value":0}),
        json!({"action":"set_value","time":0,"value":unchanged}),
        json!({"action":"move_keyframe","time":7,"new_time":8}),
        json!({"action":"move_keyframe","time":7,"new_time":null}),
        json!({"action":"clear_lane","ignored":true}),
        json!({"action":"write_automation","points":[[2,0],[1,-1]],"tolerance":0}),
        json!({"action":"write_automation","points":[[1,0],[1,-1]],"tolerance":0}),
        json!({"action":"write_automation","points":[],"tolerance":0}),
    ]
    .into_iter()
    .enumerate()
    {
        let edit = automation_edit(&s, Some(a), lane, gesture);
        automation_refused(&mut s, 100810 + i, edit);
    }
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().mix_track_mut(TrackId(a)).unwrap().locked = true;
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":3,"value":-6}));
    automation_refused(&mut s, 100830, edit);
}
#[test]
fn mixer_automation_exact_checks_clock_scope_digest_ticket_and_single_gesture_before_publication() {
    for mode in 0..5 {
        let (mut s, a, _) = insert_fixture();
        let n = 100900 + mode * 10;
        let lane = automation_lane_index(&mut s, n, Some(a), "fx.1.gain");
        let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":5,"value":7}));
        read(&mut s, n + 1);
        let mut batch = request(&mut s, n + 1, edit);
        match mode {
            0 => batch["operations"][0]["params"]["edit"]["clock"] = json!(s.playhead().0 + 1),
            1 => batch["operations"][0]["params"]["scope"]["projectId"] = json!("foreign"),
            2 => batch["operations"][0]["params"]["beforeSha256"] = json!("f".repeat(64)),
            3 => batch["exact"] = json!(false),
            _ => {
                batch["operations"].as_array_mut().unwrap().push(json!({"command":"timeline.select","params":{"clips":[]}}));
            }
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    let (mut s, a, _) = insert_fixture();
    s.execute("mixer.setKeyframe", json!({"strip":a,"time":10,"value":-1})).unwrap();
    s.execute("mixer.setKeyframe", json!({"strip":a,"time":20,"value":-2})).unwrap();
    let lane = automation_lane_index(&mut s, 100970, Some(a), "volume");
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"move_keyframe","time":10,"new_time":20}));
    automation_refused(&mut s, 100971, edit);
}
#[test]
fn mixer_automation_exact_follows_actual_human_float_widget_definitions_and_keeps_typed_insert_controls_separate() {
    let mut floats = 0;
    let mut typed = 0;
    for (i, def) in filmcraft_project::effect_defs()
        .iter()
        .filter(|d| d.kind == filmcraft_project::EffectKind::Audio && !d.intrinsic && filmcraft_render::audio_fx::supported(d.id))
        .enumerate()
    {
        let (mut s, a, _) = insert_fixture();
        let sid = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().mix_track_mut(TrackId(a)).unwrap().effects = vec![def.instance()];
        if let Some(pd) = def.params.iter().find(|p| p.animatable && matches!(p.kind, filmcraft_project::ParamKind::Float { .. })) {
            let value = match pd.kind {
                filmcraft_project::ParamKind::Float { min, .. } => min,
                _ => unreachable!(),
            };
            let key = format!("fx.0.{}", pd.id);
            let n = 101000 + i * 20;
            let lane = automation_lane_index(&mut s, n, Some(a), &key);
            let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":17,"value":value}));
            step(&mut s, n + 1, edit, "mixer.setKeyframe", json!({"strip":a,"lane":key,"time":17,"value":value}));
            floats += 1;
        }
        if let Some(pd) = def.params.iter().find(|p| matches!(p.kind, filmcraft_project::ParamKind::Bool | filmcraft_project::ParamKind::Choice(_))) {
            let key = format!("fx.0.{}", pd.id);
            let n = 101010 + i * 20;
            let lane = automation_lane_index(&mut s, n, Some(a), &key);
            let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":17,"value":0}));
            automation_refused(&mut s, n + 1, edit);
            typed += 1;
        }
    }
    assert!(floats > 0 && typed > 0);
}

#[test]
fn mixer_automation_exact_result_must_remain_inside_the_complete_authored_read_budget() {
    let (mut s, a, _) = insert_fixture();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    q.mix_track_mut(TrackId(a)).unwrap().lane_mut("volume").unwrap().keyframes =
        (0..20000).map(|i| filmcraft_project::Keyframe::new(filmcraft_time::Tick(i), ParamValue::Float(0.0))).collect();
    let lane = automation_lane_index(&mut s, 103000, Some(a), "volume");
    let points: Vec<_> = (0..2048).map(|i| json!([20001 + i, if i % 2 == 0 { -4.0 } else { -5.0 }])).collect();
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"write_automation","points":points,"tolerance":0}));
    automation_refused(&mut s, 103001, edit);
}
#[test]
fn mixer_automation_exact_live_touch_blocks_stage_and_late_publication_without_borrowing_the_human_gesture() {
    let (mut s, a, _) = insert_fixture();
    let lane = automation_lane_index(&mut s, 103100, Some(a), "volume");
    let edit = automation_edit(&s, Some(a), lane, json!({"action":"set_keyframe","time":17,"value":-6}));
    read(&mut s, 103101);
    let req = request(&mut s, 103101, edit);
    let batch: super::Batch = super::parse("agent.apply", &req).unwrap();
    let stage = super::stage_batch(&s, &batch).unwrap();
    s.execute("mixer.touch", json!({"strip":a,"lane":"volume","time":0,"value":-9})).unwrap();
    assert!(s.previews.live.is_active());
    let before = state(&s);
    assert!(super::stage_batch(&s, &batch).is_err());
    assert_eq!(state(&s), before);
    assert!(s.previews.live.is_active());
    assert!(super::publish_batch(&mut s, &batch, stage, &req, None).is_err());
    assert_eq!(state(&s), before);
    assert!(s.previews.live.is_active());
    s.execute("mixer.release", json!({"strip":a,"lane":"volume","time":0})).unwrap();
    assert!(!s.previews.live.is_active());
}
