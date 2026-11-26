use super::*;
use crate::Session;
use filmcraft_project::{AutomationMode, Interpolation, Project};
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::{Value, json};
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    for name in ["Bus", "Return"] {
        s.execute("mixer.addSubmix", json!({"name":name})).unwrap();
    }
    for strip in ["A1", "S1", "Mix"] {
        s.execute("mixer.addInsert", json!({"strip":strip,"effect":"amplify"})).unwrap();
    }
    for strip in ["A1", "S1"] {
        s.execute("mixer.addSend", json!({"strip":strip,"target":"S2"})).unwrap();
    }
    s
}
fn id(s: &Session, name: &str) -> TrackId {
    super::super::strip_ref(s.active_sequence().unwrap(), &json!(name)).unwrap()
}
fn tick(s: i64) -> Tick {
    Tick(s * TICKS_PER_SECOND)
}
fn strip(s: &Session, id: TrackId) -> filmcraft_project::Track {
    s.active_sequence().unwrap().strip(id).unwrap().into_owned()
}
fn step(s: &mut Session, command: &str, args: Value, expected: impl FnOnce(&mut Sequence)) -> Value {
    let before = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let n = s.history.undo.len();
    let mut wanted = (*before).clone();
    expected(wanted.sequence_mut(s.state.active_sequence.unwrap()).unwrap());
    let result = s.execute(command, args).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&wanted).unwrap());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.history.undo.len(), n + 1);
    let after = s.project.clone();
    let round: Project = serde_json::from_slice(&serde_json::to_vec(&*after).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(round).unwrap(), serde_json::to_value(&*after).unwrap());
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*before).unwrap());
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*after).unwrap());
    result
}
fn key(s: &mut Session, name: &str, lane: &str, time: Tick, value: f64) {
    s.execute("mixer.setKeyframe", json!({"strip":name,"lane":lane,"time":time.0,"value":value})).unwrap();
}
#[test]
fn shared_lane_control_retains_native_static_animated_mode_and_master_boundaries() {
    for name in ["A1", "S1", "Mix"] {
        let mut s = demo();
        let who = id(&s, name);
        s.execute("mixer.setStrip", json!({"strip":name,"volumeDb":-3})).unwrap();
        key(&mut s, name, "volume", tick(0), -6.0);
        key(&mut s, name, "volume", tick(4), -8.0);
        step(&mut s, "mixer.setValue", json!({"strip":name,"time":tick(2).0,"value":-12}), |q| set_value(q, who, "volume", -12.0, tick(2)).unwrap());
        let tr = strip(&s, who);
        assert_eq!(tr.volume_db, -3.0);
        assert_eq!(tr.lane_keyframes("volume").len(), 3);
        assert_eq!(tr.lane_value("volume", tick(2)), -12.0);
        s.execute("mixer.setStrip", json!({"strip":name,"mode":"Off"})).unwrap();
        let keys = strip(&s, who).lane_keyframes("volume").to_vec();
        step(&mut s, "mixer.setValue", json!({"strip":name,"time":tick(1).0,"value":-15}), |q| set_value(q, who, "volume", -15.0, tick(1)).unwrap());
        let tr = strip(&s, who);
        assert_eq!(tr.volume_db, -15.0);
        assert_eq!(tr.lane_keyframes("volume"), keys);
        assert_eq!(tr.mixer.mode, AutomationMode::Off);
    }
}
#[test]
fn shared_lane_keyframe_creation_replacement_hold_and_static_base_keep_complete_payloads() {
    for (name, lane) in [("A1", "volume"), ("S1", "send.0.level"), ("Mix", "fx.0.gain")] {
        let mut s = demo();
        let who = id(&s, name);
        let base = strip(&s, who).lane_static(lane).unwrap();
        step(&mut s, "mixer.setKeyframe", json!({"strip":name,"lane":lane,"time":10,"value":-6}), |q| set_keyframe(q, who, lane, Tick(10), -6.0).unwrap());
        assert_eq!(strip(&s, who).lane(lane).unwrap().value, ParamValue::Float(base));
        s.edit_sequence("Seed original authored key handles", |q, _, _| {
            q.with_strip_mut(who, |t| {
                let k = &mut t.lane_mut(lane).unwrap().keyframes[0];
                k.interp = Interpolation::Bezier;
                k.in_influence = 0.17;
                k.out_influence = 0.83;
            })
            .unwrap();
            Ok(())
        })
        .unwrap();
        step(&mut s, "mixer.setKeyframe", json!({"strip":name,"lane":lane,"time":10,"value":-9}), |q| set_keyframe(q, who, lane, Tick(10), -9.0).unwrap());
        let tr = strip(&s, who);
        let k = &tr.lane_keyframes(lane)[0];
        assert_eq!(k.interp, Interpolation::Bezier);
        assert_eq!(k.in_influence, 0.17);
        assert_eq!(k.out_influence, 0.83);
        assert_eq!(k.value, ParamValue::Float(-9.0));
    }
    let mut s = demo();
    let who = id(&s, "A1");
    key(&mut s, "A1", "mute", Tick(10), 0.0);
    step(&mut s, "mixer.setKeyframe", json!({"strip":"A1","lane":"mute","time":20,"value":true}), |q| set_keyframe(q, who, "mute", Tick(20), 1.0).unwrap());
    assert!(strip(&s, who).lane_keyframes("mute").iter().all(|k| k.interp == Interpolation::Hold));
    assert_eq!(strip(&s, who).lane_keyframes("mute")[1].value, ParamValue::Float(1.0));
}
#[test]
fn shared_lane_key_moves_retain_original_handles_and_human_neighbor_clamps() {
    for name in ["A1", "S1", "Mix"] {
        let mut s = demo();
        let who = id(&s, name);
        for (t, v) in [(10, -10.0), (20, -20.0), (30, -30.0)] {
            key(&mut s, name, "volume", Tick(t), v);
        }
        s.edit_sequence("Seed authored move payload", |q, _, _| {
            q.with_strip_mut(who, |tr| {
                let k = &mut tr.lane_mut("volume").unwrap().keyframes[1];
                k.interp = Interpolation::ContinuousBezier;
                k.in_influence = 0.2;
                k.out_influence = 0.7;
            })
            .unwrap();
            Ok(())
        })
        .unwrap();
        step(&mut s, "mixer.moveKeyframe", json!({"strip":name,"time":20,"newTime":1000,"value":-25}), |q| {
            move_keyframe(q, who, "volume", Tick(20), Some(Tick(1000)), Some(-25.0)).unwrap()
        });
        let tr = strip(&s, who);
        let k = &tr.lane_keyframes("volume")[1];
        assert_eq!(k.time, Tick(29));
        assert_eq!(k.value, ParamValue::Float(-25.0));
        assert_eq!(k.interp, Interpolation::ContinuousBezier);
        assert_eq!((k.in_influence, k.out_influence), (0.2, 0.7));
        step(&mut s, "mixer.moveKeyframe", json!({"strip":name,"time":29,"newTime":0}), |q| {
            move_keyframe(q, who, "volume", Tick(29), Some(Tick(0)), None).unwrap()
        });
        assert_eq!(strip(&s, who).lane_keyframes("volume")[1].time, Tick(11));
    }
}
#[test]
fn shared_lane_last_key_deletion_keeps_native_removed_value_and_complete_history() {
    for (name, lane) in [("A1", "pan"), ("S1", "send.0.level"), ("Mix", "fx.0.gain")] {
        let mut s = demo();
        let who = id(&s, name);
        key(&mut s, name, lane, Tick(10), -6.0);
        step(&mut s, "mixer.deleteKeyframe", json!({"strip":name,"lane":lane,"time":10}), |q| delete_keyframe(q, who, lane, Tick(10)).unwrap());
        let tr = strip(&s, who);
        let p = tr.lane(lane).unwrap();
        assert!(p.keyframes.is_empty());
        assert_eq!(p.value, ParamValue::Float(-6.0));
        if lane.starts_with("fx.") {
            assert_eq!(tr.lane_static(lane), Some(-6.0));
        }
    }
}
#[test]
fn shared_lane_clear_freezes_playhead_value_and_keeps_effect_versus_strip_ownership() {
    for (name, lane) in [("A1", "volume"), ("S1", "send.0.level"), ("Mix", "fx.0.gain")] {
        let mut s = demo();
        let who = id(&s, name);
        key(&mut s, name, lane, tick(0), -4.0);
        key(&mut s, name, lane, tick(4), -12.0);
        s.execute("playhead.set", json!({"seconds":2})).unwrap();
        let at = s.playhead();
        let value = strip(&s, who).lane_value(lane, at);
        // Use the actual native frame-aligned Tick for this seeded linear lane.
        let expected_level = -4.0 - 8.0 * (at.0 as f64 / tick(4).0 as f64);
        assert!((value - expected_level).abs() < 1e-9);
        step(&mut s, "mixer.clearLane", json!({"strip":name,"lane":lane}), |q| clear_lane(q, who, lane, at).unwrap());
        let tr = strip(&s, who);
        assert!(tr.lane_static(lane).is_some_and(|v| (v - expected_level).abs() < 1e-9));
        assert!(tr.lane_keyframes(lane).is_empty());
        if lane.starts_with("fx.") {
            assert!(tr.lane(lane).is_some());
            assert_eq!(tr.effects.len(), 1);
        } else {
            assert!(!tr.mixer.lanes.contains_key(lane));
        }
    }
}
#[test]
fn shared_lane_write_preparation_is_readonly_and_preserves_sample_boundaries_and_native_thinning() {
    for rate in [1, 44_100, 48_000, 192_000, 384_000] {
        let mut s = demo();
        let who = id(&s, "A1");
        s.edit_sequence("Seed native sample rate", |q, _, _| {
            q.settings.sample_rate = rate;
            Ok(())
        })
        .unwrap();
        key(&mut s, "A1", "volume", tick(0), -10.0);
        key(&mut s, "A1", "volume", tick(8), -10.0);
        let points = [(tick(2), -3.0), (tick(3), -5.0), (tick(4), -7.0)];
        let q = s.active_sequence().unwrap();
        let before = serde_json::to_value(q).unwrap();
        let prepared = prepare_write(q, who, "volume", &points, 0.01).unwrap();
        assert_eq!(prepared.kept_points(), 2);
        assert_eq!(serde_json::to_value(q).unwrap(), before);
        let result = step(
            &mut s,
            "mixer.writeAutomation",
            json!({"strip":"A1","points":points.iter().map(|p|json!([p.0.0,p.1])).collect::<Vec<_>>(),"tolerance":0.01}),
            |q| prepared.apply(q).unwrap(),
        );
        assert_eq!(result, json!({"keyframes":2,"points":3}));
        let eps = Tick::from_units(1, rate.max(1) as i64).max(Tick(1));
        let tr = strip(&s, who);
        let keys = tr.lane_keyframes("volume");
        assert!(keys.iter().any(|k| k.time == tick(2) - eps && k.value == ParamValue::Float(-10.0)));
        assert!(keys.iter().any(|k| k.time == tick(4) + eps && k.value == ParamValue::Float(-10.0)));
        assert_eq!(tr.lane_value("volume", tick(3)), -5.0);
        assert_eq!(tr.lane_value("volume", tick(7)), -10.0);
    }
    // Session rejects a zero-rate project. Pure defensive preparation is read-only.
    let mut s = demo();
    let who = id(&s, "A1");
    let before = s.project.clone();
    let n = s.history.undo.len();
    assert!(
        s.edit_sequence("Reject invalid sample rate", |q, _, _| {
            q.settings.sample_rate = 0;
            Ok(())
        })
        .is_err()
    );
    assert!(std::sync::Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.history.undo.len(), n);
    let mut q = s.active_sequence().unwrap().clone();
    q.settings.sample_rate = 0;
    let bytes = serde_json::to_vec(&q).unwrap();
    let prepared = prepare_write(&q, who, "volume", &[(tick(2), -3.0), (tick(4), -7.0)], 0.01).unwrap();
    assert_eq!(prepared.kept_points(), 2);
    assert_eq!(serde_json::to_vec(&q).unwrap(), bytes);
}

fn rejected(s: &mut Session, command: &str, args: Value) {
    let old = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let merge = s.history.merge_key.clone();
    let journal = s.journal.clone();
    assert!(s.execute(command, args.clone()).is_err(), "accepted {command} {args}");
    assert!(std::sync::Arc::ptr_eq(&old, &s.project));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.merge_key, merge);
    assert_eq!(s.journal, journal);
    for (a, b) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b).all(|((la, pa), (lb, pb))| la == lb && std::sync::Arc::ptr_eq(pa, pb)));
    }
}
#[test]
fn shared_lane_missing_keys_and_invalid_lanes_never_create_lanes_or_consume_human_redo() {
    let mut s = demo();
    let who = id(&s, "A1");
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("existing native human branch".into());
    assert!(!s.history.redo.is_empty());
    for command in ["mixer.moveKeyframe", "mixer.deleteKeyframe"] {
        rejected(&mut s, command, json!({"strip":"A1","time":123,"newTime":456}));
        rejected(&mut s, command, json!({"strip":"A1","lane":"send.50.level","time":123}));
        rejected(&mut s, command, json!({"strip":"Mix","lane":"pan","time":123}));
    }
    let mut q = s.active_sequence().unwrap().clone();
    let bytes = serde_json::to_vec(&q).unwrap();
    assert!(move_keyframe(&mut q, who, "volume", Tick(123), Some(Tick(456)), None).is_err());
    assert_eq!(serde_json::to_vec(&q).unwrap(), bytes);
    assert!(delete_keyframe(&mut q, who, "volume", Tick(123)).is_err());
    assert_eq!(serde_json::to_vec(&q).unwrap(), bytes);
    assert!(prepare_write(&q, who, "volume", &[], 0.05).is_err());
    assert_eq!(serde_json::to_vec(&q).unwrap(), bytes);
}
#[test]
fn shared_lane_human_defaults_clamps_and_live_recorder_branch_remain_original() {
    let mut s = demo();
    let who = id(&s, "A1");
    s.execute("playhead.set", json!({"seconds":2})).unwrap();
    let t = s.playhead();
    step(&mut s, "mixer.setKeyframe", json!({"strip":"A1","value":1000}), |q| set_keyframe(q, who, "volume", t, 15.0).unwrap());
    assert_eq!(strip(&s, who).lane_keyframes("volume")[0].time, t);
    let result =
        step(&mut s, "mixer.writeAutomation", json!({"strip":"A1","points":["ignored",[0,1000],[tick(1).0,-1000],[tick(2).0,-6],[true]],"tolerance":0}), |q| {
            prepare_write(q, who, "volume", &[(tick(0), 15.0), (tick(1), -96.0), (tick(2), -6.0)], 0.0).unwrap().apply(q).unwrap()
        });
    assert_eq!(result["points"], 3);
    let before = s.project.clone();
    let count = s.history.undo.len();
    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
    assert!(s.mixrec.active());
    s.execute("mixer.setValue", json!({"strip":"A1","time":tick(1).0,"value":-9})).unwrap();
    assert!(s.mixrec.active());
    assert_eq!(s.history.undo.len(), count);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*before).unwrap());
    let end = s.execute("mixer.recordStop", json!({"time":tick(2).0})).unwrap();
    assert_eq!(end["lanes"], 0);
    assert!(!s.mixrec.active());
}
