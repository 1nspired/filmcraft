use super::*;
use crate::Session;
use serde_json::{Value, json};
const DEFAULT: &str = "effects.setDefaultTransition";
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    for name in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":name,"syncLock":false})).unwrap();
    }
    s.history = History { limit: 128, ..History::default() };
    s
}
fn select(s: &mut Session, kind: &str) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
}
fn dynamic(kind: &str, moved: bool) -> Session {
    let mut s = demo();
    select(&mut s, kind);
    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
    if moved {
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    }
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,
        "undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,
        "journal":s.journal,"trim":format!("{:?}",s.trim_play)})
}
fn capture(s: &Session) -> Capture {
    super::super::prepare(s, DEFAULT).unwrap().capture_history().unwrap()
}
fn recipe(c: &Capture) -> Value {
    serde_json::from_slice(&c.evidence().encode().unwrap()).unwrap()
}
fn decoded(value: &Value, c: &Capture) -> Result<Historical> {
    Evidence::decode(&serde_json::to_vec(value).unwrap())?.validate(c.projects(), DEFAULT)
}
fn assert_original(s: &mut Session, c: &Capture) {
    let before = state(s);
    let roundtrip = Evidence::decode(&c.evidence().encode().unwrap()).unwrap().validate(c.projects(), DEFAULT).unwrap();
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(roundtrip.before_project(), &s.project));
    s.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
    assert!(Arc::ptr_eq(roundtrip.after_project(), &s.project));
    assert!(same_history(roundtrip.after_history(), &s.history));
    assert_eq!(roundtrip.trim_shift_after(), s.state.trim_shift);
    assert_eq!(roundtrip.summary(), c.historical().summary());
}
#[test]
fn historical_projection_matches_actual_human_default_for_every_trim_kind_offset_and_limit() {
    for kind in ["trim", "ripple", "roll"] {
        for moved in [false, true] {
            for limit in [0, 1, 128] {
                let mut s = dynamic(kind, moved);
                s.history.limit = limit;
                s.history.merge_key = Some("previous continuous human control".into());
                s.history.redo.push(("Retained earlier Redo".into(), Arc::new((*s.project).clone())));
                let before = state(&s);
                let c = capture(&s);
                assert_eq!(state(&s), before);
                assert_eq!(c.historical().summary().dynamic_committed, moved);
                assert_original(&mut s, &c);
            }
        }
    }
}
#[test]
fn no_trim_and_loop_only_history_projections_match_the_original_without_new_steps() {
    for looping in [false, true] {
        let mut s = demo();
        select(&mut s, "roll");
        if looping {
            s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
        }
        let c = capture(&s);
        assert_eq!(c.historical().summary().loop_stopped, looping);
        assert!(!c.historical().summary().dynamic_finished);
        assert_original(&mut s, &c);
        assert!(s.history.undo.is_empty());
    }
}
#[test]
fn zero_offset_retains_and_restores_the_exact_original_arc_even_when_its_bytes_match() {
    let mut s = dynamic("roll", false);
    let original = s.trim_play.dynamic.as_ref().unwrap().before.clone();
    s.project = Arc::new((*original).clone());
    s.state.trim_shift.out_shift = Tick(42);
    let c = capture(&s);
    let r = recipe(&c);
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&original).unwrap());
    assert_ne!(r["before_project"], r["after_project"]);
    assert_eq!(c.projects().len(), 2);
    assert!(Arc::ptr_eq(c.historical().after_project(), &original));
    assert!(c.historical().summary().restored_before_project);
    assert_original(&mut s, &c);
}
#[test]
fn zero_limit_keeps_required_trim_before_even_when_human_undo_keeps_nothing() {
    let mut s = dynamic("roll", true);
    s.history.limit = 0;
    let original = s.trim_play.dynamic.as_ref().unwrap().before.clone();
    let weak = Arc::downgrade(&original);
    let c = capture(&s);
    drop(original);
    assert!(c.projects().iter().any(|p| weak.ptr_eq(&Arc::downgrade(p))));
    assert!(!c.historical().summary().undo_entry_retained);
    assert_original(&mut s, &c);
    assert!(s.history.undo.is_empty());
    assert!(weak.upgrade().is_some());
    drop(c);
    assert!(weak.upgrade().is_none());
}
#[test]
fn zero_limit_with_existing_undo_preserves_the_original_remove_one_behavior() {
    let mut s = dynamic("roll", true);
    s.history.limit = 0;
    for i in 0..3 {
        s.history.undo.push((format!("Earlier {i}"), Arc::new((*s.project).clone())));
    }
    let c = capture(&s);
    assert!(c.historical().summary().undo_entry_retained);
    assert_eq!(c.historical().after_history().undo.len(), 3);
    assert_eq!(c.historical().after_history().undo[0].0, "Earlier 1");
    assert_original(&mut s, &c);
}
#[test]
fn discarded_redo_and_the_original_before_project_remain_in_the_readonly_snapshot_closure() {
    let mut s = dynamic("roll", true);
    let old = Arc::new((*s.project).clone());
    let weak = Arc::downgrade(&old);
    s.history.redo.push(("Older discarded Redo".into(), old));
    let c = capture(&s);
    assert!(c.historical().summary().redo_discarded);
    assert_eq!(c.historical().before_history().redo.len(), 1);
    assert_original(&mut s, &c);
    assert!(s.history.redo.is_empty());
    assert!(weak.upgrade().is_some());
    drop(c);
    assert!(weak.upgrade().is_none());
}
#[test]
fn evidence_retains_native_i64_ticks_exactly_without_exporting_live_owner_or_project_payloads() {
    let mut s = dynamic("roll", false);
    s.state.trim_shift.in_shift = Tick(i64::MIN + 11);
    s.trim_play.dynamic.as_mut().unwrap().shift_before.out_shift = Tick(i64::MAX - 17);
    let c = capture(&s);
    let raw = c.evidence().encode().unwrap();
    let text = std::str::from_utf8(&raw).unwrap();
    assert!(text.contains(&(i64::MIN + 11).to_string()));
    assert!(text.contains(&(i64::MAX - 17).to_string()));
    let value: Value = serde_json::from_slice(&raw).unwrap();
    for key in ["owner", "clock", "anchor_clock", "speed", "clipboard", "path", "project"] {
        assert!(value.get(key).is_none());
    }
    assert_eq!(value.as_object().unwrap().len(), 13);
    let roundtrip = Evidence::decode(&raw).unwrap().validate(c.projects(), DEFAULT).unwrap();
    assert_eq!(roundtrip.trim_shift_after().out_shift, Tick(i64::MAX - 17));
}
#[test]
fn historical_capture_stays_descriptive_after_the_prepared_live_owner_expires() {
    let mut s = dynamic("roll", false);
    let p = super::super::prepare(&s, DEFAULT).unwrap();
    let c = p.capture_history().unwrap();
    s.execute("trim.shuttleStop", json!({})).unwrap();
    s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
    assert!(p.validate(&s).is_err());
    let before = state(&s);
    let h = Evidence::decode(&c.evidence().encode().unwrap()).unwrap().validate(c.projects(), DEFAULT).unwrap();
    assert!(h.summary().dynamic_finished);
    assert_eq!(state(&s), before);
    assert!(s.trim_play.dynamic.is_some());
}
#[test]
fn mapped_evidence_preserves_snapshot_relations_with_reordered_capsule_indices() {
    let mut s = dynamic("roll", false);
    s.project = Arc::new((*s.project).clone());
    let c = capture(&s);
    assert_eq!(c.projects().len(), 2);
    let pool = vec![c.projects()[1].clone(), Arc::new(Project::default()), c.projects()[0].clone()];
    let h = c.evidence().validate_mapped(&pool, &[2, 0], DEFAULT).unwrap();
    assert!(Arc::ptr_eq(h.before_project(), &pool[2]));
    assert!(Arc::ptr_eq(h.after_project(), &pool[0]));
    for bad in [vec![0, 0], vec![0], vec![0, 99], vec![0, 1, 2]] {
        assert!(c.evidence().validate_mapped(&pool, &bad, DEFAULT).is_err());
    }
    assert!(c.evidence().validate_mapped(&pool, &[2, 0], "trim.forward").is_err());
}
#[test]
fn distinct_equal_byte_projects_cannot_be_collapsed_by_mapping_or_arc_aliases() {
    let mut s = dynamic("roll", false);
    s.project = Arc::new((*s.project).clone());
    let c = capture(&s);
    let pool = vec![c.projects()[0].clone(), c.projects()[0].clone()];
    assert!(c.evidence().validate(&pool, DEFAULT).is_err());
    assert!(c.evidence().validate_mapped(c.projects(), &[0, 0], DEFAULT).is_err());
    let mut r = recipe(&c);
    r["after_project"] = json!(0);
    assert!(decoded(&r, &c).is_err());
}
#[test]
fn forged_history_outcomes_labels_limits_merge_keys_and_references_are_rejected() {
    let mut s = dynamic("roll", true);
    s.history.redo.push(("Must clear Redo".into(), Arc::new((*s.project).clone())));
    s.history.merge_key = Some("preserved human gesture".into());
    let c = capture(&s);
    for i in 0..9 {
        let mut r = recipe(&c);
        match i {
            0 => r["after_history"]["undo"][0]["label"] = json!("Invented agent trim"),
            1 => r["after_history"]["redo"] = r["before_history"]["redo"].clone(),
            2 => r["after_history"]["merge_key"] = Value::Null,
            3 => r["after_history"]["limit"] = json!(1),
            4 => r["after_history"]["undo"][0]["project"] = json!(0),
            5 => r["after_shift"]["outShift"] = json!(999),
            6 => r["dynamic"]["offset"] = json!(0),
            7 => r["after_project"] = r["dynamic"]["before"].clone(),
            _ => r["sequence"] = json!(u64::MAX),
        }
        assert!(decoded(&r, &c).is_err(), "forgery {i}");
    }
}
#[test]
fn unknown_versions_commands_fields_detached_snapshots_and_unbounded_input_refuse() {
    let s = dynamic("roll", true);
    let c = capture(&s);
    for i in 0..9 {
        let mut r = recipe(&c);
        match i {
            0 => r["version"] = json!(2),
            1 => r["command"] = json!("made.up"),
            2 => r["command"] = json!("project.inspect"),
            3 => r["owner"] = json!("restored live owner"),
            4 => r["snapshots"] = json!(1000),
            5 => r["snapshots"] = json!(r["snapshots"].as_u64().unwrap() + 1),
            6 => r["before_project"] = json!(1),
            7 => r["before_history"]["undo"] = json!([{"label":"bad","project":999}]),
            _ => r["dynamic"]["speed"] = json!(1.0),
        }
        assert!(decoded(&r, &c).is_err(), "shape {i}");
    }
    for key in ["before_shift", "after_shift"] {
        let mut r = recipe(&c);
        r[key]["unknown"] = json!(true);
        assert!(decoded(&r, &c).is_err());
    }
    let mut r = recipe(&c);
    r["points"][0]["unknown"] = json!(true);
    assert!(decoded(&r, &c).is_err());
    let mut r = recipe(&c);
    r["dynamic"]["shift_before"]["unknown"] = json!(true);
    assert!(decoded(&r, &c).is_err());
    assert!(Evidence::decode(&vec![b' '; MAX_BYTES + 1]).is_err());
    let mut r = recipe(&c);
    r["points"] = json!(vec![r["points"][0].clone(); MAX_POINTS + 1]);
    assert!(decoded(&r, &c).is_err());
}
#[test]
fn malformed_and_mutated_evidence_never_panics_or_mutates_the_live_session() {
    let s = dynamic("roll", true);
    let c = capture(&s);
    let raw = c.evidence().encode().unwrap();
    let before = state(&s);
    for n in [0, 1, 2, raw.len() / 2, raw.len() - 1] {
        let result = std::panic::catch_unwind(|| Evidence::decode(&raw[..n]));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
    for n in (0..raw.len()).step_by(17) {
        let mut input = raw.clone();
        input[n] ^= 0xff;
        assert!(
            std::panic::catch_unwind(|| {
                if let Ok(e) = Evidence::decode(&input) {
                    let _ = e.validate(c.projects(), DEFAULT);
                }
            })
            .is_ok()
        );
    }
    assert_eq!(state(&s), before);
}
#[test]
fn capture_bounds_refuse_without_dropping_snapshots_or_changing_original_history() {
    for case in 0..5 {
        let mut s = dynamic("roll", true);
        match case {
            0 => s.history.limit = 1001,
            1 => s.history.merge_key = Some("x".repeat(2049)),
            2 => s.history.undo.push(("x".repeat(2049), s.project.clone())),
            3 => {
                for i in 0..MAX_STEPS {
                    s.history.undo.push((format!("Human {i}"), Arc::new((*s.project).clone())));
                }
            }
            _ => s.state.edit_points = vec![s.state.edit_points[0].clone(); MAX_POINTS + 1],
        }
        let before = state(&s);
        assert!(super::super::prepare(&s, DEFAULT).unwrap().capture_history().is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn original_live_trim_commands_project_no_settlement_or_history_change() {
    let s = dynamic("roll", true);
    for command in super::super::LIVE {
        let before = state(&s);
        if !crate::commands::find(command).unwrap().journal {
            assert!(super::super::prepare(&s, command).is_err());
            assert_eq!(state(&s), before);
            continue;
        }
        let p = super::super::prepare(&s, command).unwrap();
        let c = p.capture_history().unwrap();
        assert_eq!(c.historical().summary(), &Summary::default());
        assert!(Arc::ptr_eq(c.historical().after_project(), &s.project));
        assert!(same_history(c.historical().after_history(), &s.history));
        assert_eq!(c.historical().trim_shift_after(), s.state.trim_shift);
    }
}
