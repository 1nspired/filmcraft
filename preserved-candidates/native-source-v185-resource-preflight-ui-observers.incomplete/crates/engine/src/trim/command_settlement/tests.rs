use super::*;
use serde_json::{Value, json};

const DEFAULT: &str = "effects.setDefaultTransition";
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    for name in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":name,"syncLock":false})).unwrap();
    }
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
fn twin(s: &Session) -> Session {
    let mut t = Session::default();
    t.project = s.project.clone();
    t.state = s.state.clone();
    t.history = s.history.clone();
    t.revision = s.revision;
    t.saved_revision = s.saved_revision;
    t.trim_play = s.trim_play.clone();
    t.journal = s.journal.clone();
    t.prefs = s.prefs.clone();
    t.path = s.path.clone();
    t
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,
        "undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,
        "journal":s.journal,"trim":format!("{:?}",s.trim_play)})
}
fn assert_history(a: &Session, b: &Session) {
    assert_eq!(a.history.limit, b.history.limit);
    assert_eq!(a.history.merge_key, b.history.merge_key);
    assert!(same_stack(&a.history.undo, &b.history.undo));
    assert!(same_stack(&a.history.redo, &b.history.redo));
}
fn reject(plan: Prepared, s: &mut Session) {
    let before = state(s);
    let project = s.project.clone();
    assert!(plan.apply(s).is_err());
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&project, &s.project));
}

#[test]
fn preparation_is_readonly_and_projects_the_exact_existing_gesture() {
    for moved in [false, true] {
        let s = dynamic("roll", moved);
        let before = state(&s);
        let plan = prepare(&s, DEFAULT).unwrap();
        assert_eq!(state(&s), before);
        assert!(plan.summary().dynamic_finished);
        assert_eq!(plan.summary().dynamic_committed, moved);
        assert_eq!(plan.summary().label, moved.then_some("Dynamic Rolling Edit"));
        assert!(Arc::ptr_eq(plan.project_after(), &s.project));
        assert_eq!(plan.trim_shift_after(), if moved { s.state.trim_shift } else { s.trim_play.dynamic.as_ref().unwrap().shift_before });
    }
}
#[test]
fn prepared_default_workflow_matches_original_top_level_human_in_every_trim_kind() {
    for kind in ["trim", "ripple", "roll"] {
        for moved in [false, true] {
            let base = dynamic(kind, moved);
            let mut original = twin(&base);
            let mut prepared = twin(&base);
            let plan = prepare(&prepared, DEFAULT).unwrap();
            let expected = plan.summary().clone();
            original.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
            assert_eq!(plan.apply(&mut prepared).unwrap(), expected);
            prepared.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
            assert_eq!(state(&prepared), state(&original));
            assert_history(&prepared, &original);
            assert!(prepared.trim_play.dynamic.is_none());
            if moved {
                assert_eq!(prepared.history.undo.len(), base.history.undo.len() + 1);
                let after = serde_json::to_value(&prepared.project).unwrap();
                original.execute("edit.undo", json!({})).unwrap();
                prepared.execute("edit.undo", json!({})).unwrap();
                assert_eq!(state(&prepared), state(&original));
                original.execute("edit.redo", json!({})).unwrap();
                prepared.execute("edit.redo", json!({})).unwrap();
                assert_eq!(state(&prepared), state(&original));
                assert_eq!(serde_json::to_value(&prepared.project).unwrap(), after);
            }
        }
    }
}
#[test]
fn nonzero_dynamic_settlement_reports_original_history_limit_and_redo_effects() {
    for limit in [0, 1, 128] {
        let mut s = dynamic("roll", true);
        s.history.limit = limit;
        s.history.redo.push(("Earlier human Redo".into(), s.project.clone()));
        let plan = prepare(&s, DEFAULT).unwrap();
        let expected = plan.summary().clone();
        assert!(expected.dynamic_committed && expected.redo_discarded);
        assert_eq!(expected.undo_entry_retained, limit > 0 || !s.history.undo.is_empty());
        let mut original = twin(&s);
        original.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
        plan.apply(&mut s).unwrap();
        s.execute(DEFAULT, json!({"effect":"constant_gain"})).unwrap();
        assert_eq!(state(&s), state(&original));
        assert_history(&s, &original);
    }
}
#[test]
fn zero_offset_keeps_redo_and_restores_the_original_project_arc_and_shift() {
    let mut s = dynamic("roll", false);
    s.history.redo.push(("Keep human Redo".into(), s.project.clone()));
    let original = s.trim_play.dynamic.as_ref().unwrap().before.clone();
    let shift = s.trim_play.dynamic.as_ref().unwrap().shift_before;
    s.project = Arc::new((*s.project).clone());
    s.state.trim_shift.out_shift = Tick(42);
    let history = s.history.clone();
    let plan = prepare(&s, DEFAULT).unwrap();
    assert!(plan.summary().restored_before_project);
    assert!(!plan.summary().dynamic_committed);
    assert!(Arc::ptr_eq(plan.project_after(), &original));
    assert_eq!(plan.trim_shift_after(), shift);
    plan.apply(&mut s).unwrap();
    assert!(Arc::ptr_eq(&s.project, &original));
    assert_eq!(s.state.trim_shift, shift);
    assert!(same_stack(&s.history.undo, &history.undo) && same_stack(&s.history.redo, &history.redo));
}
#[test]
fn loop_only_default_stops_playback_without_a_project_or_history_step() {
    let mut s = demo();
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let project = s.project.clone();
    let before = twin(&s);
    let plan = prepare(&s, DEFAULT).unwrap();
    assert!(plan.summary().loop_stopped);
    assert!(!plan.summary().dynamic_finished);
    plan.apply(&mut s).unwrap();
    assert!(s.trim_play.around.is_none());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, before.revision);
    assert_eq!(s.journal, before.journal);
    assert_history(&s, &before);
}
#[test]
fn original_live_command_exceptions_and_trim_loop_policy_remain_unchanged() {
    for command in LIVE {
        let mut s = dynamic("roll", true);
        let before = state(&s);
        let project = s.project.clone();
        assert_eq!(settle_before_command(&mut s, command), Summary::default());
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
    }
    let mut s = demo();
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let before = state(&s);
    settle_before_command(&mut s, "trim.forward");
    assert_eq!(state(&s), before);
}
#[test]
fn observed_absence_expires_when_a_dynamic_or_loop_owner_starts() {
    for loop_only in [false, true] {
        let mut s = demo();
        select(&mut s, "roll");
        let plan = prepare(&s, DEFAULT).unwrap();
        assert_eq!(plan.summary(), &Summary::default());
        if loop_only {
            s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
        } else {
            s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
        }
        reject(plan, &mut s);
    }
}
#[test]
fn identical_dynamic_restart_is_a_different_live_owner_even_with_the_same_project_and_editor() {
    let mut s = dynamic("roll", false);
    let plan = prepare(&s, DEFAULT).unwrap();
    let project = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    s.execute("trim.shuttleStop", json!({})).unwrap();
    s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    reject(plan, &mut s);
}
#[test]
fn identical_loop_restart_is_a_different_live_owner() {
    let mut s = demo();
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let plan = prepare(&s, DEFAULT).unwrap();
    let editor = serde_json::to_value(&s.state).unwrap();
    let project = s.project.clone();
    let revision = s.revision;
    s.execute("trim.playAround", json!({"toggle":true})).unwrap();
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    reject(plan, &mut s);
}
#[test]
fn native_clock_owner_and_edit_dependencies_expire_before_any_settlement() {
    for change in 0..10 {
        let mut s = dynamic("roll", true);
        let plan = prepare(&s, DEFAULT).unwrap();
        match change {
            0 => s.trim_play.dynamic.as_mut().unwrap().anchor_clock = -0.0,
            1 => s.trim_play.dynamic.as_mut().unwrap().anchor_offset += Tick(1),
            2 => s.trim_play.dynamic.as_mut().unwrap().speed = 2.0,
            3 => s.trim_play.dynamic.as_mut().unwrap().at_limit = true,
            4 => s.trim_play.dynamic.as_mut().unwrap().offset += Tick(1),
            5 => s.project = Arc::new((*s.project).clone()),
            6 => s.revision += 1,
            7 => s.state.edit_points[0].out = false,
            8 => s.state.trim_shift.out_shift += Tick(1),
            _ => s.saved_revision += 1,
        }
        reject(plan, &mut s);
    }
}
#[test]
fn complete_history_identity_labels_limits_and_merge_keys_are_retained_dependencies() {
    for change in 0..6 {
        let mut s = dynamic("roll", true);
        s.history.undo.push(("Earlier Undo".into(), s.project.clone()));
        s.history.redo.push(("Earlier Redo".into(), s.project.clone()));
        let plan = prepare(&s, DEFAULT).unwrap();
        match change {
            0 => s.history.undo[0].0.push_str(" changed"),
            1 => s.history.redo[0].0.push_str(" changed"),
            2 => s.history.undo[0].1 = Arc::new((*s.history.undo[0].1).clone()),
            3 => s.history.redo[0].1 = Arc::new((*s.history.redo[0].1).clone()),
            4 => s.history.limit += 1,
            _ => s.history.merge_key = Some("Later human gesture".into()),
        }
        reject(plan, &mut s);
    }
}
#[test]
fn missing_nonjournaled_and_display_owned_commands_never_settle_a_live_trim() {
    let mut s = dynamic("roll", true);
    let before = state(&s);
    assert!(prepare(&s, "made.up.command").is_err());
    assert!(prepare(&s, "project.inspect").is_err());
    assert_eq!(state(&s), before);
    let plan = prepare(&s, DEFAULT).unwrap();
    s.view_only = true;
    assert!(prepare(&s, DEFAULT).is_err());
    reject(plan, &mut s);
}
