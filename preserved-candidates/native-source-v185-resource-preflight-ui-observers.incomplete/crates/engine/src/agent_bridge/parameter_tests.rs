use super::tests::{read, source_clock_fixture, token};
use filmcraft_project::ParamValue;
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;
fn setup() -> crate::Session {
    let mut s = source_clock_fixture();
    let c = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    for _ in 0..2 {
        s.execute("effects.apply", json!({"clips":[c.0],"effect":"gaussian_blur"})).unwrap();
    }
    s.execute("masks.add", json!({"clip":c.0,"effect":0,"shape":"rectangle"})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s
}
fn request(s: &crate::Session, n: usize) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    json!({"token":token(n),"label":"Adjust exact native parameter","exact":true,"operations":[{"command":"agent.setParameter","params":{
        "clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"effectId":c.effects[1].instance_id,"maskId":null,
        "param":"blurriness","value":{"Float":27},"frame":5,"keyframe":false,
        "scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0}}}]})
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn project(s: &crate::Session) -> Value {
    serde_json::to_value(&s.project).unwrap()
}
#[test]
fn parameter_edit_targets_second_duplicate_and_preserves_human_state_history_reopen_and_guarded_undo() {
    let mut s = setup();
    read(&mut s, 24000);
    let req = request(&s, 24000);
    let before = project(&s);
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let journal = s.journal.len();
    let revision = s.revision;
    let r = s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(r["action"], "parameter_edit");
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.journal.len(), journal + 1);
    assert_eq!(s.revision, revision + 1);
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    assert_eq!(c.effects[0].f64_at("blurriness", Tick::ZERO), 0.0);
    assert_eq!(c.effects[1].f64_at("blurriness", Tick::ZERO), 27.0);
    assert_eq!(r["results"][0]["effect"]["instance_id"], req["operations"][0]["params"]["effectId"]);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.execute("agent.apply", req).unwrap(), r);
    assert_eq!(s.revision, revision + 1);
    let after = project(&s);
    let saved = filmcraft_format::encode(&s.project, false);
    assert_eq!(serde_json::to_value(filmcraft_format::decode(&saved).unwrap().project).unwrap(), after);
    read(&mut s, 24001);
    s.execute("agent.undo", json!({"token":token(24001),"commit":token(24000)})).unwrap();
    assert_eq!(project(&s), before);
    s.redo().unwrap();
    assert_eq!(project(&s), after);
}
#[test]
fn parameter_gesture_matches_human_control_with_animated_retime_reverse_and_both_hold_filter_modes() {
    for (speed, reverse, hold, hold_filters) in
        [(1.0, false, false, false), (0.5, false, false, false), (2.0, true, false, false), (1.0, false, true, false), (1.0, false, true, true)]
    {
        let mut s = setup();
        let sid = s.state.active_sequence.unwrap();
        let rate = s.sequence_rate();
        let c = &mut Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0];
        c.speed = speed;
        c.reverse = reverse;
        c.frame_hold = hold.then(|| rate.tick_of(300));
        c.hold_filters = hold_filters;
        let prm = c.effects[1].param_mut("blurriness").unwrap();
        prm.put_keyframe(rate.tick_of(0), ParamValue::Float(4.0));
        prm.put_keyframe(rate.tick_of(500), ParamValue::Float(80.0));
        let outside = prm.keyframes.clone();
        let mt = c.effect_time_at(rate.tick_of(5));
        let id = c.id;
        let mut human = crate::Session::new(s.services.clone());
        human.project = s.project.clone();
        human.state = s.state.clone();
        human.media = s.media.clone();
        human
            .execute(
                "effects.setParam",
                json!({"clip":id.0,"effect":1,"param":"blurriness","value":27,"time":rate.tick_of(5).0,"keyframe":false,"merge":false,"begin":false}),
            )
            .unwrap();
        read(&mut s, 24002);
        let req = request(&s, 24002);
        let r = s.execute("agent.apply", req).unwrap();
        assert_eq!(project(&s), project(&human));
        assert_eq!(r["results"][0]["sourceTime"], mt.0);
        let p = s.active_sequence().unwrap().find_item(id).unwrap().1.effects[1].param("blurriness").unwrap();
        assert!(p.keyframes.iter().any(|k| k.time == mt && k.value == ParamValue::Float(27.0)));
        for key in &outside {
            assert!(p.keyframes.contains(key));
        }
        if hold {
            assert_eq!(mt == rate.tick_of(300), hold_filters);
        }
    }
}
#[test]
fn parameter_mask_identity_and_tagged_types_use_native_definitions_and_preserve_siblings() {
    let mut s = setup();
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let id = c.id;
    let effect = c.effects[0].instance_id.clone();
    let mask = c.effects[0].masks[0].instance_id.clone();
    for (n, param, value) in [
        (24003, "opacity", json!({"Float":65})),
        (24004, "path", json!({"Path":{"closed":true,"vertices":[{"p":{"x":1,"y":2}},{"p":{"x":3,"y":4}},{"p":{"x":5,"y":6}}]}})),
    ] {
        read(&mut s, n);
        let mut req = request(&s, n);
        let p = &mut req["operations"][0]["params"];
        p["effectId"] = json!(effect);
        p["maskId"] = json!(mask);
        p["param"] = json!(param);
        p["value"] = value;
        let r = s.execute("agent.apply", req).unwrap();
        assert_eq!(r["results"][0]["maskId"], json!(mask));
    }
    let m = &s.active_sequence().unwrap().find_item(id).unwrap().1.effects[0].masks[0];
    assert_eq!(m.opacity.value, ParamValue::Float(65.0));
    for (n, param, value) in [(24005, "dimensions", json!({"Choice":2})), (24006, "repeat_edge", json!({"Bool":true}))] {
        read(&mut s, n);
        let mut req = request(&s, n);
        req["operations"][0]["params"]["param"] = json!(param);
        req["operations"][0]["params"]["value"] = value;
        s.execute("agent.apply", req).unwrap();
    }
}
#[test]
fn parameter_rejects_bad_identity_scope_literal_clamping_and_mixed_batches_without_consuming_redo() {
    let mut s = setup();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("effects.setParam", json!({"clip":clip.0,"effect":1,"param":"blurriness","value":7})).unwrap();
    s.undo().unwrap();
    read(&mut s, 24007);
    let req = request(&s, 24007);
    let before = state(&s);
    let ptr = s.project.clone();
    let changes: Vec<(&str, Value)> = vec![
        ("identity", json!("f".repeat(32))),
        ("effectId", json!("f".repeat(32))),
        ("maskId", json!("f".repeat(32))),
        ("clip", json!(0)),
        ("frame", json!(20)),
        ("frame", json!(-1)),
        ("frame", json!(i64::MAX)),
        ("param", json!("not_defined")),
        ("value", json!({"Float":-1})),
        ("value", json!({"Float":3001})),
        ("value", json!({"Bool":true})),
        ("value", json!({"Text":"/not/a/resource"})),
        ("value", json!({"Vec2":{"x":1,"y":2,"extra":3}})),
        ("keyframe", Value::Null),
        ("scope", json!({"projectId":"foreign","anchorSequence":s.state.active_sequence.unwrap().0})),
        ("extra", json!(true)),
    ];
    for (key, value) in changes {
        let mut bad = req.clone();
        bad["operations"][0]["params"][key] = value;
        assert!(s.execute("agent.apply", bad).is_err(), "{key}");
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&ptr, &s.project));
    }
    for key in ["maskId", "effectId", "frame", "keyframe", "scope"] {
        let mut bad = req.clone();
        bad["operations"][0]["params"].as_object_mut().unwrap().remove(key);
        assert!(s.execute("agent.apply", bad).is_err());
        assert_eq!(state(&s), before);
    }
    for (param, value, keyframe) in
        [("blurriness", json!({"Float":0}), false), ("dimensions", json!({"Choice":99}), false), ("dimensions", json!({"Choice":1}), true)]
    {
        let mut bad = req.clone();
        let p = &mut bad["operations"][0]["params"];
        p["param"] = json!(param);
        p["value"] = value;
        p["keyframe"] = json!(keyframe);
        assert!(s.execute("agent.apply", bad).is_err());
        assert_eq!(state(&s), before);
    }
    let mut mixed = req.clone();
    mixed["operations"].as_array_mut().unwrap().push(json!({"command":"timeline.razor","params":{"clip":clip.0,"time":s.sequence_rate().tick_of(10).0}}));
    assert!(s.execute("agent.apply", mixed).is_err());
    assert_eq!(state(&s), before);
    s.redo().unwrap();
}
#[test]
fn parameter_refuses_stale_replaced_untracked_locked_and_protected_native_targets() {
    for mode in ["stale", "replace", "tracking", "locked", "protected", "rate", "clock", "offline"] {
        let mut s = setup();
        let sid = s.state.active_sequence.unwrap();
        if matches!(mode, "tracking" | "locked" | "protected" | "rate" | "clock" | "offline") {
            let p = Arc::make_mut(&mut s.project);
            if mode == "tracking" {
                p.items.get_mut(&sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
            }
            let q = p.sequence_mut(sid).unwrap();
            let source = q.video_tracks[0].items[0].item;
            if mode == "rate" {
                q.settings.frame_rate = filmcraft_time::FrameRate::new(37, 1);
            }
            if mode == "clock" {
                q.video_tracks[0].items[0].speed = f64::MAX;
            }
            if mode == "locked" {
                q.video_tracks[0].locked = true;
            }
            if mode == "protected" {
                q.video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("Human finish".into());
            }
            if mode == "offline" {
                p.item_mut(source).unwrap().as_media_mut().unwrap().offline = true;
            }
        }
        read(&mut s, 24008);
        let req = request(&s, 24008);
        if mode == "replace" {
            s.project = Arc::new((*s.project).clone());
        }
        if mode == "stale" {
            s.execute("effects.setParam", json!({"clip":req["operations"][0]["params"]["clip"],"effect":1,"param":"blurriness","value":9})).unwrap();
        }
        let before = state(&s);
        let ptr = s.project.clone();
        assert!(s.execute("agent.apply", req).is_err(), "{mode}");
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&ptr, &s.project));
    }
}

#[test]
fn parameter_point_color_wheel_and_curve_literals_use_the_shared_native_converter() {
    use filmcraft_project::ParamKind as K;
    for (n, which, value) in [
        (24010, 0, json!({"Vec2":{"x":15,"y":25}})),
        (24011, 1, json!({"Color":[0.3,0.4,0.5,1]})),
        (24012, 2, json!({"Vec2":{"x":0.1,"y":0.2}})),
        (24013, 3, json!({"Curve":[[0,0],[0.5,0.7],[1,1]]})),
    ] {
        let mut s = setup();
        let (def, pd) = filmcraft_project::effect::effect_defs()
            .iter()
            .find_map(|d| {
                d.params
                    .iter()
                    .find(|pd| match which {
                        0 => matches!(&pd.kind, K::Point),
                        1 => matches!(&pd.kind, K::Color),
                        2 => matches!(&pd.kind, K::Wheel),
                        _ => matches!(&pd.kind, K::Curve { .. }),
                    })
                    .map(|pd| (d, pd))
            })
            .unwrap();
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        s.edit_sequence("Generated human effect", |q, _, _| {
            q.find_item_mut(c).unwrap().1.effects.push(def.instance());
            Ok(())
        })
        .unwrap();
        let id = s.active_sequence().unwrap().find_item(c).unwrap().1.effects.last().unwrap().instance_id.clone();
        read(&mut s, n);
        let mut req = request(&s, n);
        let p = &mut req["operations"][0]["params"];
        p["effectId"] = json!(id);
        p["param"] = json!(pd.id);
        p["value"] = value;
        let r = s.execute("agent.apply", req).unwrap();
        assert_eq!(r["results"][0]["effect"]["instance_id"], json!(id));
    }
}

#[test]
fn parameter_journal_replays_from_a_saved_tracked_baseline_without_a_ticket_and_keeps_one_undo_step() {
    let mut s = setup();
    let saved = filmcraft_format::encode(&s.project, false);
    let sid = s.state.active_sequence;
    let initial = project(&s);
    let start = s.journal.len();
    read(&mut s, 24014);
    let req = request(&s, 24014);
    s.execute("agent.apply", req).unwrap();
    let after = project(&s);
    let entry = s.journal[start..].to_vec();
    assert_eq!(entry.len(), 1);
    assert_eq!(entry[0].0, "agent.replayBatch");
    let mut recovered = crate::Session::new(s.services.clone());
    recovered.project = Arc::new(filmcraft_format::decode(&saved).unwrap().project);
    recovered.state.active_sequence = sid;
    recovered.execute(&entry[0].0, entry[0].1.clone()).unwrap();
    assert_eq!(project(&recovered), after);
    assert_eq!(recovered.history.undo.len(), 1);
    recovered.undo().unwrap();
    assert_eq!(project(&recovered), initial);
    // Missing/reallocated identity is an explicit recovery failure, never an
    // ordinal fallback that might modify a different effect.
    let sid = sid.unwrap();
    Arc::make_mut(&mut recovered.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].effects[1].instance_id = Some("f".repeat(32));
    let before = state(&recovered);
    assert!(recovered.execute(&entry[0].0, entry[0].1.clone()).is_err());
    assert_eq!(state(&recovered), before);
}

// Audio editor parity uses the original parameter bridge and human command.
// These are authored-state engine fixtures: no media decode, model or GUI claim.
fn audio_parameter_fixture(effect: &str) -> (crate::Session, filmcraft_project::ClipId) {
    let mut s = source_clock_fixture();
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let item;
    let id;
    {
        let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
        q.audio_tracks.truncate(1);
        let track = &mut q.audio_tracks[0];
        track.items.truncate(2);
        track.transitions.clear();
        track.locked = false;
        let c = &mut track.items[0];
        c.start = Tick::ZERO;
        c.duration = rate.tick_of(20);
        c.source_in = rate.tick_of(100);
        c.speed = 1.0;
        c.reverse = false;
        c.frame_hold = None;
        c.enabled = true;
        c.effects.clear();
        item = c.item;
        id = c.id;
    }
    let source = Arc::make_mut(&mut s.project).item_mut(item).unwrap();
    source.metadata.insert(
        "poema.binding.v1".into(),
        json!({"projectId":"p_source_clock","assetId":"a_audio_parameter","mediaId":"original","role":"original"}).to_string(),
    );
    let media = source.as_media_mut().unwrap();
    media.media = filmcraft_project::MediaRef::File { path: "/generated/audio-parameter.wav".into() };
    media.offline = false;
    media.info.duration = rate.tick_of(1000);
    for _ in 0..2 {
        s.execute("effects.apply", json!({"clips":[id.0],"effect":effect})).unwrap();
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    (s, id)
}
fn audio_parameter_request(s: &crate::Session, id: filmcraft_project::ClipId, n: usize, param: &str, value: Value) -> Value {
    let c = s.active_sequence().unwrap().find_item(id).unwrap().1;
    json!({"token":token(n),"label":"Exact audio editor parameter","exact":true,"operations":[{"command":"agent.setParameter","params":{
        "clip":id.0,"identity":c.identity.as_ref().unwrap().id,"effectId":c.effects[1].instance_id,"maskId":null,
        "param":param,"value":value,"frame":5,"keyframe":false,
        "scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0}}}]})
}
fn audio_parameter_human(s: &crate::Session) -> crate::Session {
    let mut h = crate::Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.media = s.media.clone();
    h
}
fn audio_parameter_differential(effect: &str, param: &str, value: f64) {
    for (round, (speed, reverse, hold, hold_filters)) in
        [(1.0, false, false, false), (0.5, false, false, false), (2.0, true, false, false), (1.0, false, true, false), (1.0, false, true, true)]
            .into_iter()
            .enumerate()
    {
        let (mut s, id) = audio_parameter_fixture(effect);
        let sid = s.state.active_sequence.unwrap();
        let rate = s.sequence_rate();
        let c = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(id).unwrap().1;
        c.speed = speed;
        c.reverse = reverse;
        c.frame_hold = hold.then(|| rate.tick_of(300));
        c.hold_filters = hold_filters;
        // Animated modes retain keys well outside the trimmed source interval.
        let previous_keys = if round > 0 {
            let p = c.effects[1].param_mut(param).unwrap();
            p.put_keyframe(rate.tick_of(0), p.value.clone());
            p.put_keyframe(rate.tick_of(500), p.value.clone());
            p.keyframes.clone()
        } else {
            Vec::new()
        };
        let source_time = c.effect_time_at(rate.tick_of(5));
        let duplicate = serde_json::to_value(&c.effects[0]).unwrap();
        let before = project(&s);
        let editor = serde_json::to_value(&s.state).unwrap();
        let (revision, undo, journal) = (s.revision, s.history.undo.len(), s.journal.len());
        let mut human = audio_parameter_human(&s);
        human
            .execute(
                "effects.setParam",
                json!({"clip":id.0,"effect":1,"param":param,"value":value,"time":rate.tick_of(5).0,"keyframe":false,"merge":false,"begin":false}),
            )
            .unwrap();
        let n = 241000 + round * 4;
        read(&mut s, n);
        let request = audio_parameter_request(&s, id, n, param, json!({"Float":value}));
        let target = request["operations"][0]["params"]["effectId"].clone();
        let scope = json!({"projectId":"p_source_clock","anchorSequence":sid.0});
        let identity = request["operations"][0]["params"]["identity"].clone();
        let finishing = s.execute("agent.readFinishing", json!({"token":token(n),"scope":scope,"identity":identity,"offset":1,"limit":1})).unwrap();
        assert_eq!(finishing["effects"][0]["instance_id"], target);
        assert!(finishing["effects"][0]["params"][param]["definition"].is_object());
        let receipt = s.execute("agent.apply", request.clone()).unwrap();
        assert_eq!(project(&s), project(&human), "{effect} round {round}");
        assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
        assert_eq!((s.revision, s.history.undo.len(), s.journal.len()), (revision + 1, undo + 1, journal + 1));
        assert_eq!(receipt["action"], "parameter_edit");
        assert_eq!(receipt["results"][0]["sourceTime"], source_time.0);
        assert_eq!(receipt["results"][0]["effect"]["instance_id"], target);
        let c = s.active_sequence().unwrap().find_item(id).unwrap().1;
        assert_eq!(serde_json::to_value(&c.effects[0]).unwrap(), duplicate);
        let p = c.effects[1].param(param).unwrap();
        if round == 0 {
            assert_eq!(p.value, ParamValue::Float(value));
        } else {
            assert!(p.keyframes.iter().any(|k| k.time == source_time && k.value == ParamValue::Float(value)));
            for old in previous_keys {
                assert!(p.keyframes.contains(&old));
            }
        }
        assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
        assert_eq!((s.revision, s.history.undo.len(), s.journal.len()), (revision + 1, undo + 1, journal + 1));
        let after = project(&s);
        let bytes = filmcraft_format::encode(&s.project, false);
        assert_eq!(serde_json::to_value(filmcraft_format::decode(&bytes).unwrap().project).unwrap(), after);
        read(&mut s, n + 1);
        s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
        assert_eq!(project(&s), before);
        read(&mut s, n + 2);
        s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
        assert_eq!(project(&s), after);
    }
}
#[test]
fn audio_parameter_parametric_eq_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("parametric_eq", "mid_gain", 3.5);
}
#[test]
fn audio_parameter_graphic_eq10_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("graphic_eq", "b4", 2.5);
}
#[test]
fn audio_parameter_graphic_eq20_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("graphic_eq_20", "b12", -2.5);
}
#[test]
fn audio_parameter_graphic_eq30_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("graphic_eq_30", "b22", 4.0);
}
#[test]
fn audio_parameter_multiband_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("multiband_compressor", "b3_threshold", -24.0);
}
#[test]
fn audio_parameter_dynamics_matches_shared_human_clock_history_and_readback() {
    audio_parameter_differential("dynamics_rack", "comp_ratio", 4.0);
}
#[test]
fn audio_parameter_boolean_toggles_use_native_types_and_preserve_the_other_duplicate() {
    for (effect, param, value) in [("parametric_eq", "mid_on", false), ("multiband_compressor", "b2_solo", true), ("dynamics_rack", "gate_on", true)] {
        let (mut s, id) = audio_parameter_fixture(effect);
        let before = s.active_sequence().unwrap().find_item(id).unwrap().1.effects[0].clone();
        let mut human = audio_parameter_human(&s);
        human.execute("effects.setParam", json!({"clip":id.0,"effect":1,"param":param,"value":value,"time":s.sequence_rate().tick_of(5).0})).unwrap();
        read(&mut s, 241100);
        let req = audio_parameter_request(&s, id, 241100, param, json!({"Bool":value}));
        s.execute("agent.apply", req).unwrap();
        assert_eq!(project(&s), project(&human));
        let c = s.active_sequence().unwrap().find_item(id).unwrap().1;
        assert_eq!(c.effects[0], before);
        assert_eq!(c.effects[1].param(param).unwrap().value, ParamValue::Bool(value));
    }
}
#[test]
fn audio_parameter_bad_literals_and_stale_read_plans_preserve_human_redo_and_document() {
    for effect in ["parametric_eq", "graphic_eq", "graphic_eq_20", "graphic_eq_30", "multiband_compressor", "dynamics_rack"] {
        let (mut s, id) = audio_parameter_fixture(effect);
        let param = if effect == "parametric_eq" {
            "mid_gain"
        } else if effect.starts_with("graphic_eq") {
            "b1"
        } else if effect == "multiband_compressor" {
            "b1_gain"
        } else {
            "output"
        };
        s.execute("effects.setParam", json!({"clip":id.0,"effect":1,"param":param,"value":1})).unwrap();
        s.undo().unwrap();
        read(&mut s, 241200);
        let req = audio_parameter_request(&s, id, 241200, param, json!({"Float":2}));
        let before = state(&s);
        let ptr = s.project.clone();
        for value in [json!({"Float":1000000}), json!({"Bool":true}), json!({"Text":"/not/a/resource"})] {
            let mut bad = req.clone();
            bad["operations"][0]["params"]["value"] = value;
            assert!(s.execute("agent.apply", bad).is_err(), "{effect}");
            assert_eq!(state(&s), before);
            assert!(Arc::ptr_eq(&s.project, &ptr));
        }
        s.redo().unwrap();
        let human = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "old read after human Redo: {effect}");
        assert_eq!(state(&s), human);
    }
}
