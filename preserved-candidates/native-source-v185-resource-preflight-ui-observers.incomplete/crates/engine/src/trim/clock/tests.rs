use super::*;
use crate::trim::command_settlement::prepare;
use serde_json::json;
const DEFAULT: &str = "effects.setDefaultTransition";
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    for name in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":name,"syncLock":false})).unwrap();
    }
    s.history = crate::History { limit: 128, ..crate::History::default() };
    s
}
fn select(s: &mut Session, kind: &str) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
}
fn dynamic(kind: &str) -> Session {
    let mut s = demo();
    select(&mut s, kind);
    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
    s
}
fn twin(s: &Session) -> Session {
    let mut t = Session::default();
    t.project = s.project.clone();
    t.state = s.state.clone();
    t.history = s.history.clone();
    t.revision = s.revision;
    t.saved_revision = s.saved_revision;
    t.path = s.path.clone();
    t.trim_play = s.trim_play.clone();
    t.prefs = s.prefs.clone();
    t.journal = s.journal.clone();
    t
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"markers":s.state.clipboard_markers,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,"journal":s.journal,"trim":format!("{:?}",s.trim_play)})
}
fn rejected(p: &Following, s: &Session) {
    let before = state(s);
    assert!(p.prepare_current(s).is_err());
    assert_eq!(state(s), before);
}
#[test]
fn following_capture_is_readonly_and_normal_playhead_motion_grants_only_a_current_default_preparation() {
    let mut s = demo();
    let before = state(&s);
    let p = Following::capture(&s).unwrap();
    assert_eq!(state(&s), before);
    let project = s.project.clone();
    s.set_playhead(s.sequence_rate().tick_of(7));
    let ready = p.prepare_current(&s).unwrap();
    ready.validate(&s).unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.history.undo.len(), 0);
}
#[test]
fn all_original_dynamic_kinds_follow_real_clock_advances_and_settle_exactly_like_the_current_human_command() {
    for kind in ["trim", "ripple", "roll"] {
        for limit in [0, 1, 128] {
            let mut s = dynamic(kind);
            s.history.limit = limit;
            let old = prepare(&s, DEFAULT).unwrap();
            let following = Following::capture(&s).unwrap();
            for time in [0.125, 0.25, 0.5] {
                s.execute("trim.tick", json!({"clock":time})).unwrap();
                assert!(old.validate(&s).is_err());
                let before = state(&s);
                let current = following.prepare_current(&s).unwrap();
                assert_eq!(state(&s), before);
                current.validate(&s).unwrap();
            }
            let mut human = twin(&s);
            let ready = following.prepare_current(&s).unwrap();
            let summary = ready.summary().clone();
            human.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
            ready.apply(&mut s).unwrap();
            s.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
            assert_eq!(state(&s), state(&human));
            assert!(summary.dynamic_committed);
            assert_eq!(summary.undo_entry_retained, limit > 0);
            assert_eq!(
                summary.label,
                Some(match kind {
                    "roll" => "Dynamic Rolling Edit",
                    "ripple" => "Dynamic Ripple Trim",
                    _ => "Dynamic Trim",
                })
            );
        }
    }
}
#[test]
fn following_loops_accept_actual_loop_ticks_and_refuse_the_ended_or_restarted_owner() {
    let mut s = demo();
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let p = Following::capture(&s).unwrap();
    for time in [0.1, 0.25, 0.5, 5.0] {
        s.execute("trim.tick", json!({"clock":time})).unwrap();
        p.prepare_current(&s).unwrap().validate(&s).unwrap();
    }
    s.execute("trim.playAround", json!({"toggle":true})).unwrap();
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    rejected(&p, &s);
    let mut s = demo();
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0,"loop":false})).unwrap();
    let p = Following::capture(&s).unwrap();
    s.execute("trim.tick", json!({"clock":1000.0})).unwrap();
    assert!(!s.trim_play.active());
    rejected(&p, &s);
}
#[test]
fn neither_noop_nor_real_ticks_bless_a_foreign_project_or_revision_after_the_original_attestation() {
    for looping in [false, true] {
        for noop in [false, true] {
            for revision in [false, true] {
                let mut s = if looping {
                    let mut s = demo();
                    select(&mut s, "roll");
                    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
                    s
                } else {
                    dynamic("roll")
                };
                let p = Following::capture(&s).unwrap();
                s.execute("trim.tick", json!({"clock":0.25})).unwrap();
                if revision {
                    s.revision += 1;
                } else {
                    s.project = Arc::new((*s.project).clone());
                }
                rejected(&p, &s);
                s.execute("trim.tick", json!({"clock":if noop{0.25}else{0.5}})).unwrap();
                rejected(&p, &s);
                assert!(Following::capture(&s).is_err());
            }
        }
    }
}
#[test]
fn following_preserves_all_nonclock_editor_clipboard_preference_and_saved_path_dependencies() {
    for change in 0..13 {
        let mut s = dynamic("roll");
        s.path = Some("/generated/same.fcproj".into());
        let p = Following::capture(&s).unwrap();
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        match change {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.state.source_playhead = Tick(1),
            2 => {
                s.state.playheads.insert(ItemId(u64::MAX), Tick(9007199254740993));
            }
            3 => s.state.default_audio_transition = "constant_gain".into(),
            4 => s.state.edit_points[0].out = false,
            5 => s.state.trim_shift.out_shift += Tick(1),
            6 => s.saved_revision += 1,
            7 => s.path = Some("/generated/other.fcproj".into()),
            8 => s.state.selection.push(filmcraft_project::ClipId(u64::MAX)),
            9 => s.prefs.audio.automatch_time += 1.0,
            10 => s.state.clipboard_markers.push(
                serde_json::from_value(json!({"id":1,"start":9007199254740993_i64,"duration":0,"name":"private","comment":"","color":"Iris","kind":"Comment"}))
                    .unwrap(),
            ),
            11 => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
                s.state.clipboard.push((filmcraft_project::TrackKind::Video, 0, clip));
            }
            _ => s.state.linked_selection = !s.state.linked_selection,
        }
        rejected(&p, &s);
    }
}
#[test]
fn following_keeps_exact_history_stack_identity_labels_limits_and_merge_keys_even_after_further_real_ticks() {
    for change in 0..6 {
        let mut s = dynamic("roll");
        s.history.undo.push(("Earlier undo".into(), s.project.clone()));
        s.history.redo.push(("Earlier redo".into(), s.project.clone()));
        let p = Following::capture(&s).unwrap();
        match change {
            0 => s.history.undo[0].0.push_str(" changed"),
            1 => s.history.redo[0].0.push_str(" changed"),
            2 => s.history.undo[0].1 = Arc::new((*s.history.undo[0].1).clone()),
            3 => s.history.redo[0].1 = Arc::new((*s.history.redo[0].1).clone()),
            4 => s.history.limit = 1,
            _ => s.history.merge_key = Some("later".into()),
        }
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        rejected(&p, &s);
    }
}
#[test]
fn following_refuses_private_clock_forgery_restarts_and_new_owners_without_weakening_original_prepared_checks() {
    for change in 0..7 {
        let mut s = dynamic("roll");
        let p = Following::capture(&s).unwrap();
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        match change {
            0 => s.trim_play.dynamic.as_mut().unwrap().speed = 2.0,
            1 => s.trim_play.dynamic.as_mut().unwrap().anchor_clock = -0.0,
            2 => s.trim_play.dynamic.as_mut().unwrap().offset += Tick(1),
            3 => s.trim_play.dynamic.as_mut().unwrap().at_limit = true,
            4 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
            5 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
            }
            _ => s.state.active_sequence = None,
        }
        rejected(&p, &s);
    }
    let mut s = demo();
    select(&mut s, "roll");
    let p = Following::capture(&s).unwrap();
    s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
    rejected(&p, &s);
}
#[test]
fn original_clock_attests_shuttle_changes_limits_and_only_clock_derived_selection() {
    for kind in ["trim", "roll"] {
        let mut s = demo();
        s.state.selection_follows_playhead = true;
        select(&mut s, kind);
        s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
        let p = Following::capture(&s).unwrap();
        for time in [0.125, 0.5, 1000.0] {
            s.execute("trim.tick", json!({"clock":time})).unwrap();
            p.prepare_current(&s).unwrap().validate(&s).unwrap();
        }
        s.execute("trim.shuttle", json!({"clock":1000.0,"direction":"reverse"})).unwrap();
        s.execute("trim.tick", json!({"clock":1000.25})).unwrap();
        p.prepare_current(&s).unwrap();
        s.state.selection.push(filmcraft_project::ClipId(u64::MAX));
        rejected(&p, &s);
    }
}

#[test]
fn following_accepts_only_the_original_human_clock_clearing_of_an_earlier_merge_marker() {
    for kind in ["trim", "ripple", "roll"] {
        let mut s = demo();
        s.history.merge_key = Some("Original earlier human drag".into());
        select(&mut s, kind);
        s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
        let following = Following::capture(&s).unwrap();
        let strict = prepare(&s, DEFAULT).unwrap();
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        assert_eq!(s.history.merge_key, None);
        assert!(strict.validate(&s).is_err());
        let ready = following.prepare_current(&s).unwrap();
        let mut human = twin(&s);
        human.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
        ready.apply(&mut s).unwrap();
        s.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
        assert_eq!(state(&s), state(&human));
    }
}

#[test]
fn following_rejects_foreign_selection_or_playhead_even_when_the_next_original_tick_would_replace_it() {
    for kind in ["trim", "ripple", "roll", "loop"] {
        for noop in [false, true] {
            for selection in [false, true] {
                let mut s = demo();
                s.state.selection_follows_playhead = true;
                select(&mut s, if kind == "loop" { "roll" } else { kind });
                if kind == "loop" {
                    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
                } else {
                    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
                }
                let p = Following::capture(&s).unwrap();
                s.execute("trim.tick", json!({"clock":0.25})).unwrap();
                p.prepare_current(&s).unwrap();
                if selection {
                    s.state.selection.push(ClipId(u64::MAX));
                } else {
                    let seq = s.state.active_sequence.unwrap();
                    s.state.playheads.insert(seq, Tick(9007199254740993));
                }
                rejected(&p, &s);
                s.execute("trim.tick", json!({"clock":if noop{0.25}else{0.5}})).unwrap();
                rejected(&p, &s);
                assert!(Following::capture(&s).is_err());
            }
        }
    }
}

#[test]
fn clock_evidence_bounds_its_extra_allocations_without_interrupting_the_original_human_gesture() {
    for selection in [false, true] {
        let mut s = demo();
        select(&mut s, "roll");
        if selection {
            s.state.selection_follows_playhead = true;
            s.state.selection = vec![ClipId(u64::MAX); 4097];
        } else {
            s.history.merge_key = Some("x".repeat(2049));
        }
        s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
        assert!(s.trim_play.active());
        assert!(!current(&s));
        prepare(&s, DEFAULT).unwrap().validate(&s).unwrap();
        assert!(Following::capture(&s).is_err());
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        assert!(s.trim_play.active());
        assert!(!current(&s));
    }
}
