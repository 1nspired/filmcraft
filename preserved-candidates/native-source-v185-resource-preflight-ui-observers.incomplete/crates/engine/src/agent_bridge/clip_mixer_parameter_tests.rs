use super::tests::{read, source_clock_fixture, token};
use filmcraft_project::{Interpolation, ParamValue};
use serde_json::{Value, json};
use std::sync::Arc;
fn setup() -> crate::Session {
    let mut s = source_clock_fixture();
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    q.audio_tracks.truncate(1);
    q.audio_tracks[0].items.truncate(1);
    q.audio_tracks[0].transitions.clear();
    let c = &mut q.audio_tracks[0].items[0];
    c.start = rate.tick_of(2);
    c.duration = rate.tick_of(20);
    c.source_in = rate.tick_of(100);
    c.speed = 1.0;
    c.reverse = false;
    c.frame_hold = None;
    c.hold_filters = false;
    c.enabled = true;
    c.effects = filmcraft_project::effect::intrinsic_audio();
    let item = c.item;
    let media = Arc::make_mut(&mut s.project).item_mut(item).unwrap();
    media
        .metadata
        .insert("poema.binding.v1".into(), json!({"projectId":"p_source_clock","assetId":"a_audio_clock","mediaId":"original","role":"original"}).to_string());
    let m = media.as_media_mut().unwrap();
    m.offline = false;
    m.info.duration = rate.tick_of(1000);
    s.execute("poema.trackEffects", json!({})).unwrap();
    s
}
fn request(s: &crate::Session, n: usize, effect: &str, param: &str, value: f64, keyframe: bool) -> Value {
    let c = &s.active_sequence().unwrap().audio_tracks[0].items[0];
    let e = c.effect(effect).unwrap();
    json!({"token":token(n),"label":"Adjust original Clip Mixer control","exact":true,"operations":[{"command":"agent.setParameter","params":{"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"effectId":e.instance_id,"maskId":null,"param":param,"value":{"Float":value},"frame":6,"keyframe":keyframe,"control":"clip_mixer","scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0}}}]})
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
fn qmut(s: &mut crate::Session) -> &mut filmcraft_project::Sequence {
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap()
}
fn refused(s: &mut crate::Session, req: Value) {
    let before = state(s);
    let project = s.project.clone();
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&project, &s.project));
}
fn paired(s: &mut crate::Session, n: usize, effect: &str, param: &str, value: f64, keyframe: bool) -> Value {
    let rate = s.sequence_rate();
    let c = &s.active_sequence().unwrap().audio_tracks[0].items[0];
    let id = c.id;
    let mt = c.source_time_at(rate.tick_of(6));
    let mut human = crate::Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human
        .execute("clipMixer.set", json!({"clip":id.0,"effect":effect,"param":param,"value":value,"time":rate.tick_of(6).0,"keyframe":keyframe,"begin":true}))
        .unwrap();
    read(s, n);
    let req = request(s, n, effect, param, value, keyframe);
    let before = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let receipt = s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&human.project).unwrap());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(receipt["action"], "parameter_edit");
    assert_eq!(receipt["results"][0]["control"], "clip_mixer");
    assert_eq!(receipt["results"][0]["humanCommand"], "clipMixer.set");
    assert_eq!(receipt["results"][0]["sourceTime"], mt.0);
    let after = s.project.clone();
    assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(
        serde_json::to_value(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project).unwrap(),
        serde_json::to_value(&s.project).unwrap()
    );
    read(s, n + 1);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&before).unwrap());
    read(s, n + 2);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&after).unwrap());
    receipt
}
#[test]
fn clip_mixer_parameter_alias_matches_original_volume_pan_static_and_key_controls_with_owned_history() {
    for (i, (effect, param, value, kf)) in
        [("volume", "level", -6.0, false), ("volume", "level", -8.0, true), ("panner", "balance", 25.0, false), ("panner", "balance", -35.0, true)]
            .into_iter()
            .enumerate()
    {
        let mut s = setup();
        let r = paired(&mut s, 110000 + i * 10, effect, param, value, kf);
        assert_eq!(r["results"][0]["param"], param);
    }
}
#[test]
fn clip_mixer_parameter_alias_preserves_trim_reverse_speed_and_hold_source_clock_without_changing_effect_control_time() {
    for (i, (speed, reverse, hold, filters)) in
        [(1.0, false, false, false), (0.5, false, false, false), (2.0, true, false, false), (1.0, false, true, false), (1.0, false, true, true)]
            .into_iter()
            .enumerate()
    {
        let mut s = setup();
        let rate = s.sequence_rate();
        let c = &mut qmut(&mut s).audio_tracks[0].items[0];
        c.speed = speed;
        c.reverse = reverse;
        c.frame_hold = hold.then(|| rate.tick_of(300));
        c.hold_filters = filters;
        let expected_source = c.source_time_at(rate.tick_of(6));
        let expected_effect = c.effect_time_at(rate.tick_of(6));
        let mut legacy = crate::Session::new(s.services.clone());
        legacy.project = s.project.clone();
        legacy.state = s.state.clone();
        legacy.media = s.media.clone();
        let r = paired(&mut s, 110100 + i * 10, "volume", "level", -6.0, true);
        assert_eq!(r["results"][0]["sourceTime"], expected_source.0);
        read(&mut legacy, 110200 + i * 10);
        let mut old = request(&legacy, 110200 + i * 10, "volume", "level", -6.0, true);
        old["operations"][0]["params"].as_object_mut().unwrap().remove("control");
        let old = legacy.execute("agent.apply", old).unwrap();
        assert!(old["results"][0].get("control").is_none());
        assert!(old["results"][0].get("humanCommand").is_none());
        assert_eq!(old["results"][0]["sourceTime"], expected_effect.0);
        if hold && !filters {
            assert_ne!(expected_source, expected_effect);
        }
    }
}
#[test]
fn clip_mixer_parameter_alias_uses_existing_animation_and_retains_complete_outside_key_payloads() {
    let mut s = setup();
    let rate = s.sequence_rate();
    let p = qmut(&mut s).audio_tracks[0].items[0].effect_mut("volume").unwrap().param_mut("level").unwrap();
    p.put_keyframe(rate.tick_of(50), ParamValue::Float(-12.0));
    p.put_keyframe(rate.tick_of(200), ParamValue::Float(0.0));
    p.keyframes[0].interp = Interpolation::Bezier;
    p.keyframes[0].in_influence = 0.17;
    p.keyframes[0].out_influence = 0.83;
    let keys = p.keyframes.clone();
    paired(&mut s, 110300, "volume", "level", -9.0, false);
    let got = s.active_sequence().unwrap().audio_tracks[0].items[0].effect("volume").unwrap().param("level").unwrap();
    assert_eq!(got.keyframes.len(), 3);
    for k in keys {
        assert!(got.keyframes.contains(&k));
    }
}
#[test]
fn clip_mixer_parameter_alias_refuses_unknown_discriminators_masks_wrong_controls_and_duplicate_effect_fallback() {
    let mut s = setup();
    read(&mut s, 110400);
    let base = request(&s, 110400, "volume", "level", -6.0, false);
    for (key, value) in [
        ("control", Value::Null),
        ("control", json!(false)),
        ("control", json!("effects")),
        ("param", json!("gain")),
        ("value", json!({"Bool":false})),
        ("value", json!({"Float":16})),
        ("maskId", json!("f".repeat(32))),
    ] {
        let mut req = base.clone();
        req["operations"][0]["params"][key] = value;
        refused(&mut s, req);
    }
    let mut s = setup();
    let mut duplicate = filmcraft_project::find_effect("volume").unwrap().instance();
    duplicate.instance_id = Some("e".repeat(32));
    qmut(&mut s).audio_tracks[0].items[0].effects.push(duplicate);
    let other = s.active_sequence().unwrap().audio_tracks[0].items[0].effects.last().unwrap().instance_id.clone();
    read(&mut s, 110410);
    let mut req = request(&s, 110410, "volume", "level", -6.0, false);
    req["operations"][0]["params"]["effectId"] = json!(other);
    refused(&mut s, req);
}
#[test]
fn clip_mixer_parameter_alias_refuses_missing_values_disabled_video_and_locked_targets_without_losing_redo() {
    for mode in ["missing", "disabled", "locked", "video", "noop"] {
        let mut s = setup();
        s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
        s.undo().unwrap();
        if mode == "missing" {
            qmut(&mut s).audio_tracks[0].items[0].effect_mut("volume").unwrap().params.remove("level");
        }
        if mode == "disabled" {
            qmut(&mut s).audio_tracks[0].items[0].enabled = false;
        }
        if mode == "locked" {
            qmut(&mut s).audio_tracks[0].locked = true;
        }
        if mode == "video" {
            let mut effects = filmcraft_project::effect::intrinsic_audio();
            for (i, effect) in effects.iter_mut().enumerate() {
                effect.instance_id = Some(format!("{:032x}", 0xc100 + i));
            }
            qmut(&mut s).video_tracks[0].items[0].effects = effects;
        }
        let value = if mode == "noop" { 0.0 } else { -6.0 };
        read(&mut s, 110500);
        let mut req = request(&s, 110500, "volume", "level", value, false);
        if mode == "video" {
            let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
            let p = &mut req["operations"][0]["params"];
            p["clip"] = json!(c.id.0);
            p["identity"] = json!(c.identity.as_ref().unwrap().id);
            p["effectId"] = json!(c.effect("volume").unwrap().instance_id);
        }
        refused(&mut s, req);
    }
}
#[test]
fn clip_mixer_parameter_alias_rejects_outside_frames_scope_nonexact_and_mixed_requests() {
    let mut s = setup();
    read(&mut s, 110600);
    let base = request(&s, 110600, "volume", "level", -6.0, false);
    for frame in [1, 22, -1] {
        let mut req = base.clone();
        req["operations"][0]["params"]["frame"] = json!(frame);
        refused(&mut s, req);
    }
    let mut req = base.clone();
    req["operations"][0]["params"]["scope"]["projectId"] = json!("foreign");
    refused(&mut s, req);
    let mut req = base.clone();
    req["exact"] = json!(false);
    refused(&mut s, req);
    let mut req = base.clone();
    let op = req["operations"][0].clone();
    req["operations"].as_array_mut().unwrap().push(op);
    refused(&mut s, req);
}
#[test]
fn clip_mixer_parameter_alias_guards_actual_live_mixer_before_stage_and_again_before_publication() {
    let mut s = setup();
    read(&mut s, 110700);
    let req = request(&s, 110700, "volume", "level", -6.0, false);
    let batch: super::Batch = super::parse("agent.apply", &req).unwrap();
    let stage = super::stage_batch(&s, &batch).unwrap();
    let track = s.active_sequence().unwrap().audio_tracks[0].id;
    let time = s.sequence_rate().tick_of(6);
    s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":time.0})).unwrap();
    assert!(s.previews.live.is_active());
    let before = state(&s);
    assert!(super::stage_batch(&s, &batch).is_err());
    assert!(super::publish_batch(&mut s, &batch, stage, &req, None).is_err());
    assert_eq!(state(&s), before);
    assert!(s.previews.live.is_active());
    s.previews.live.clear_all();
    let mut s = setup();
    read(&mut s, 110710);
    let req = request(&s, 110710, "volume", "level", -6.0, false);
    let batch: super::Batch = super::parse("agent.apply", &req).unwrap();
    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
    assert!(super::parameter::live_basis(&s, &batch.operations[0]).is_err());
    refused(&mut s, req);
}
#[test]
fn clip_mixer_parameter_alias_replays_only_its_receipt_and_cannot_undo_a_later_human_edit() {
    let mut s = setup();
    read(&mut s, 110800);
    let req = request(&s, 110800, "volume", "level", -6.0, false);
    let r = s.execute("agent.apply", req.clone()).unwrap();
    let revision = s.revision;
    assert_eq!(s.execute("agent.apply", req.clone()).unwrap(), r);
    assert_eq!(s.revision, revision);
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":10})).unwrap();
    read(&mut s, 110801);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(110801),"commit":token(110800)})).is_err());
    assert_eq!(state(&s), before);
    assert_eq!(s.execute("agent.apply", req).unwrap(), r);
    assert_eq!(state(&s), before);
}
