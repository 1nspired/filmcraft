use super::*;
use crate::Session;
use filmcraft_project::{Interpolation, Keyframe, Project};
use serde_json::{Value, json};
use std::sync::Arc;
fn demo() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let c = &mut q.audio_tracks[0].items[0];
    c.start = Tick(1000);
    c.duration = Tick(100);
    c.source_in = Tick(10000);
    c.speed = 1.0;
    c.reverse = false;
    c.frame_hold = None;
    c.hold_filters = false;
    for (effect, param) in [("volume", "level"), ("panner", "balance")] {
        let p = c.effect_mut(effect).unwrap().param_mut(param).unwrap();
        p.value = ParamValue::Float(0.0);
        p.keyframes.clear();
    }
    let id = c.id;
    s.history.undo.clear();
    s.history.redo.clear();
    s.history.merge_key = None;
    s.journal.clear();
    (s, id)
}
fn qmut(s: &mut Session) -> &mut Sequence {
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap()
}
fn parameter<'a>(s: &'a Session, id: ClipId, e: &str, p: &str) -> &'a filmcraft_project::Param {
    s.active_sequence().unwrap().find_item(id).unwrap().1.effect(e).unwrap().param(p).unwrap()
}
fn paired(s: &mut Session, id: ClipId, e: &str, p: &str, v: f64, time: Tick, kf: bool) {
    let before = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let mut expected = (*before).clone();
    let sid = s.state.active_sequence.unwrap();
    let q = expected.sequence_mut(sid).unwrap();
    prepare(q, id, e, p, v, time, kf).unwrap().apply(q).unwrap();
    s.execute("clipMixer.set", json!({"clip":id.0,"effect":e,"param":p,"value":v,"time":time.0,"keyframe":kf,"begin":true})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&expected).unwrap());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = s.project.clone();
    let round: Project = filmcraft_format::decode(&filmcraft_format::encode(&after, false)).unwrap().project;
    assert_eq!(serde_json::to_value(round).unwrap(), serde_json::to_value(&*after).unwrap());
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*before).unwrap());
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*after).unwrap());
}
fn preserve(s: &mut Session) {
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    assert!(!s.history.redo.is_empty());
    s.history.merge_key = Some("existing original human drag".into());
}
fn refused(s: &mut Session, args: Value) {
    let project = s.project.clone();
    let bytes = serde_json::to_vec(&*project).unwrap();
    let editor = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let merge = s.history.merge_key.clone();
    let journal = s.journal.clone();
    assert!(s.execute("clipMixer.set", args.clone()).is_err(), "accepted {args}");
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(serde_json::to_vec(&*s.project).unwrap(), bytes);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.merge_key, merge);
    assert_eq!(s.journal, journal);
    for (a, b) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b).all(|((x, p), (y, q))| x == y && Arc::ptr_eq(p, q)));
    }
}
#[test]
fn shared_clip_controls_retain_original_defaults_clamps_static_bases_history_and_saved_format() {
    for (e, p, v, want) in
        [("volume", "level", 25.0, 15.0), ("volume", "level", -120.0, -96.0), ("panner", "balance", 125.0, 100.0), ("panner", "balance", -125.0, -100.0)]
    {
        let (mut s, id) = demo();
        paired(&mut s, id, e, p, v, Tick(1020), false);
        let got = parameter(&s, id, e, p);
        assert_eq!(got.value, ParamValue::Float(want));
        assert!(got.keyframes.is_empty());
    }
    let (mut s, id) = demo();
    s.execute("clipMixer.set", json!({"clip":id.0,"value":-8})).unwrap();
    assert_eq!(parameter(&s, id, "volume", "level").value, ParamValue::Float(-8.0));
}
#[test]
fn shared_clip_controls_use_trimmed_reverse_speed_and_hold_source_time_with_original_outside_clamps() {
    for (speed, reverse, hold, filters) in [
        (1.0, false, false, false),
        (0.5, false, false, false),
        (2.0, false, false, false),
        (2.0, true, false, false),
        (-2.0, false, false, false),
        (1.0, false, true, false),
        (1.0, false, true, true),
        (2.0, true, true, false),
    ] {
        for (time, relative) in [(900, 0), (1000, 0), (1020, 20), (1100, 100), (1200, 100)] {
            let (mut s, id) = demo();
            let c = qmut(&mut s).find_item_mut(id).unwrap().1;
            c.speed = speed;
            c.reverse = reverse;
            c.frame_hold = hold.then_some(Tick(77777));
            c.hold_filters = filters;
            let expected = if hold {
                77777
            } else if reverse {
                10000 + (100.0 * speed.abs()).round() as i64 - (relative as f64 * speed.abs()).round() as i64 - 1
            } else {
                10000 + (relative as f64 * speed.abs()).round() as i64
            };
            if hold && !filters {
                assert_ne!(c.effect_time_at(Tick(time)), Tick(expected));
            }
            paired(&mut s, id, "volume", "level", -6.0, Tick(time), true);
            let got = parameter(&s, id, "volume", "level");
            assert_eq!(got.keyframes.len(), 1);
            assert_eq!(got.keyframes[0].time, Tick(expected));
            assert_eq!(got.keyframes[0].value, ParamValue::Float(-6.0));
        }
    }
}
#[test]
fn shared_clip_controls_follow_explicit_or_already_animated_keyframe_mode_without_replacing_outside_keys() {
    for (animated, explicit) in [(false, false), (false, true), (true, false), (true, true)] {
        let (mut s, id) = demo();
        let p = qmut(&mut s).find_item_mut(id).unwrap().1.effect_mut("volume").unwrap().param_mut("level").unwrap();
        if animated {
            p.put_keyframe(Tick(9990), ParamValue::Float(-12.0));
            p.put_keyframe(Tick(10300), ParamValue::Float(0.0));
        }
        let original = p.keyframes.clone();
        paired(&mut s, id, "volume", "level", -4.0, Tick(1020), explicit);
        let got = parameter(&s, id, "volume", "level");
        if animated || explicit {
            assert_eq!(got.value, ParamValue::Float(0.0));
            assert!(got.keyframes.iter().any(|k| k.time == Tick(10020) && k.value == ParamValue::Float(-4.0)));
            for key in &original {
                assert!(got.keyframes.contains(key));
            }
        } else {
            assert_eq!(got.value, ParamValue::Float(-4.0));
            assert!(got.keyframes.is_empty());
        }
    }
}
#[test]
fn shared_clip_controls_keep_key_interpolation_influences_and_only_the_first_original_effect_target() {
    let (mut s, id) = demo();
    let c = qmut(&mut s).find_item_mut(id).unwrap().1;
    let mut duplicate = c.effect("volume").unwrap().clone();
    duplicate.param_mut("level").unwrap().value = ParamValue::Float(-30.0);
    c.effects.push(duplicate.clone());
    let p = c.effect_mut("volume").unwrap().param_mut("level").unwrap();
    let mut key = Keyframe::new(Tick(10020), ParamValue::Float(-9.0));
    key.interp = Interpolation::ContinuousBezier;
    key.in_influence = 0.21;
    key.out_influence = 0.79;
    p.keyframes.push(key.clone());
    paired(&mut s, id, "volume", "level", -3.0, Tick(1020), false);
    let got = parameter(&s, id, "volume", "level");
    key.value = ParamValue::Float(-3.0);
    assert_eq!(got.keyframes, vec![key]);
    let c = s.active_sequence().unwrap().find_item(id).unwrap().1;
    assert_eq!(serde_json::to_value(c.effects.last().unwrap()).unwrap(), serde_json::to_value(duplicate).unwrap());
}
#[test]
fn original_clip_control_begin_and_continuation_keep_human_drag_merge_boundaries_and_undo_redo() {
    let (mut s, id) = demo();
    let before = s.project.clone();
    let args = |v, begin| json!({"clip":id.0,"value":v,"time":1020,"begin":begin});
    s.execute("clipMixer.set", args(-3, true)).unwrap();
    assert_eq!(s.history.undo.len(), 1);
    let key = s.history.merge_key.clone();
    s.execute("clipMixer.set", args(-6, false)).unwrap();
    assert_eq!(s.history.undo.len(), 1);
    assert_eq!(s.history.merge_key, key);
    let merged = s.project.clone();
    s.execute("clipMixer.set", args(-9, true)).unwrap();
    assert_eq!(s.history.undo.len(), 2);
    let second = s.project.clone();
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*merged).unwrap());
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*before).unwrap());
    s.redo().unwrap();
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*second).unwrap());
}
#[test]
fn original_clip_control_bad_targets_and_values_preserve_human_redo_and_existing_merge_ownership() {
    let (mut s, id) = demo();
    preserve(&mut s);
    for args in [
        json!({"clip":0,"value":-6,"begin":true}),
        json!({"clip":id.0,"effect":"missing","value":-6,"begin":true}),
        json!({"clip":id.0,"param":"missing","value":-6,"begin":true}),
        json!({"clip":id.0,"value":null,"begin":true}),
        json!({"clip":id.0,"value":true,"begin":true}),
        json!({"value":-6,"begin":true}),
    ] {
        refused(&mut s, args);
    }
    let c = qmut(&mut s).find_item_mut(id).unwrap().1;
    c.duration = Tick(-1);
    refused(&mut s, json!({"clip":id.0,"value":-6,"time":1020,"begin":true}));
}
#[test]
fn original_clip_control_post_preparation_commit_failure_restores_the_original_merge_boundary() {
    let (mut s, id) = demo();
    preserve(&mut s);
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "unsupported".into());
    assert!(prepare(s.active_sequence().unwrap(), id, "volume", "level", -6.0, Tick(1020), false).is_ok());
    refused(&mut s, json!({"clip":id.0,"value":-6,"time":1020,"begin":true}));
}
#[test]
fn shared_clip_control_resolves_the_current_candidate_clock_and_animation_and_refuses_nonfinite_or_missing_targets() {
    let (mut s, id) = demo();
    let change = prepare(s.active_sequence().unwrap(), id, "volume", "level", -6.0, Tick(1020), false).unwrap();
    let q = qmut(&mut s);
    let c = q.find_item_mut(id).unwrap().1;
    c.source_in = Tick(20000);
    c.effect_mut("volume").unwrap().param_mut("level").unwrap().put_keyframe(Tick(20000), ParamValue::Float(-12.0));
    change.apply(q).unwrap();
    let got = parameter(&s, id, "volume", "level");
    assert_eq!(got.value, ParamValue::Float(0.0));
    assert_eq!(got.keyframes.len(), 2);
    assert!(got.keyframes.iter().any(|k| k.time == Tick(20020) && k.value == ParamValue::Float(-6.0)));
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(prepare(s.active_sequence().unwrap(), id, "volume", "level", value, Tick(1020), false).is_err());
    }
    let q = qmut(&mut s);
    let change = prepare(q, id, "volume", "level", -6.0, Tick(1020), true).unwrap();
    q.find_item_mut(id).unwrap().1.effects.clear();
    let before = serde_json::to_value(&*q).unwrap();
    assert!(change.apply(q).is_err());
    assert_eq!(serde_json::to_value(&*q).unwrap(), before);
}
