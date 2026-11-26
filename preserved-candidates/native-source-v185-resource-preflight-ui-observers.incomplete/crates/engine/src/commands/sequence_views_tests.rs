//! Original sequence-view state services and human-command equivalence.
use super::{close_other_sequence_states, close_sequence_state, move_sequence_tab_state, open_sequence_state};
use crate::{EditorState, Event, Session};
use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> EditorState {
    let mut s = EditorState::default();
    s.active_sequence = Some(ItemId(2));
    s.open_sequences = vec![ItemId(1), ItemId(2), ItemId(3)];
    s.selection = vec![ClipId(19), ClipId(29)];
    s.project_selection = vec![ItemId(71)];
    s.source_item = Some(ItemId(88));
    s.source_playhead = Tick(9_007_199_254_740_993);
    s.playheads.insert(ItemId(2), Tick(9_007_199_254_740_995));
    s
}
fn collateral(before: &EditorState, after: &EditorState, changed: &[&str]) {
    let mut a = serde_json::to_value(before).unwrap();
    let mut b = serde_json::to_value(after).unwrap();
    for key in changed {
        assert!(a.as_object_mut().unwrap().remove(*key).is_some());
        assert!(b.as_object_mut().unwrap().remove(*key).is_some());
    }
    assert_eq!(a, b, "unrelated serialized native editor payload changed");
}
fn session() -> (Session, Vec<ItemId>, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let first = s.state.active_sequence.unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    let mut ids = vec![first];
    for name in ["Second native view", "Third native view"] {
        let r = s.execute("file.newSequence", json!({"name":name,"video":0,"audio":0})).unwrap();
        ids.push(ItemId(r["sequence"].as_u64().unwrap()));
    }
    s.execute("sequence.open", json!({"item":first.0})).unwrap();
    s.state.open_sequences = ids.clone();
    s.state.selection = vec![clip];
    s.state.project_selection = ids.clone();
    s.state.source_playhead = Tick(9_007_199_254_740_993);
    s.events.clear();
    s.journal.clear();
    s.history.merge_key = Some("preserved original view gesture".into());
    (s, ids, clip)
}
fn same_stack(a: &[(String, Arc<filmcraft_project::Project>)], b: &[(String, Arc<filmcraft_project::Project>)]) {
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(b).all(|((x, p), (y, q))| x == y && Arc::ptr_eq(p, q)));
}

#[test]
fn shared_sequence_view_close_preserves_original_order_fallback_and_all_other_state() {
    for (target, active, want, next) in [
        (1, Some(2), vec![2, 3], Some(2)),
        (2, Some(2), vec![1, 3], Some(3)),
        (3, Some(3), vec![1, 2], Some(2)),
        (99, Some(2), vec![1, 2, 3], Some(2)),
        (2, None, vec![1, 3], None),
    ] {
        let mut s = state();
        s.active_sequence = active.map(ItemId);
        let before = s.clone();
        close_sequence_state(&mut s, ItemId(target));
        assert_eq!(s.open_sequences, want.into_iter().map(ItemId).collect::<Vec<_>>());
        assert_eq!(s.active_sequence, next.map(ItemId));
        collateral(&before, &s, &["open_sequences", "active_sequence"]);
    }
    let mut s = state();
    s.active_sequence = Some(ItemId(2));
    s.open_sequences = vec![ItemId(2), ItemId(2)];
    close_sequence_state(&mut s, ItemId(2));
    assert!(s.open_sequences.is_empty());
    assert_eq!(s.active_sequence, None);
}

#[test]
fn shared_sequence_view_close_others_switch_event_and_selection_match_original_semantics() {
    for active in [None, Some(ItemId(1)), Some(ItemId(2)), Some(ItemId(99))] {
        let mut s = state();
        s.active_sequence = active;
        let before = s.clone();
        let (closed, opened) = close_other_sequence_states(&mut s, ItemId(2)).unwrap();
        assert_eq!(closed, 2);
        assert_eq!(opened, active != Some(ItemId(2)));
        assert_eq!(s.open_sequences, vec![ItemId(2)]);
        assert_eq!(s.active_sequence, Some(ItemId(2)));
        assert_eq!(s.selection.is_empty(), opened);
        collateral(&before, &s, &["open_sequences", "active_sequence", "selection"]);
    }
    let mut s = state();
    let before = serde_json::to_value(&s).unwrap();
    assert!(close_other_sequence_states(&mut s, ItemId(99)).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

#[test]
fn shared_sequence_view_tab_move_retains_end_clamp_order_active_selection_and_missing_target_rejection() {
    for (id, index, want, actual) in [
        (1, 0, vec![1, 2, 3], 0),
        (1, 1, vec![2, 1, 3], 1),
        (1, 2, vec![2, 3, 1], 2),
        (1, u64::MAX, vec![2, 3, 1], 2),
        (3, 0, vec![3, 1, 2], 0),
        (2, u64::MAX, vec![1, 3, 2], 2),
    ] {
        let mut s = state();
        let before = s.clone();
        assert_eq!(move_sequence_tab_state(&mut s, ItemId(id), index).unwrap(), actual);
        assert_eq!(s.open_sequences, want.into_iter().map(ItemId).collect::<Vec<_>>());
        collateral(&before, &s, &["open_sequences"]);
    }
    let mut s = state();
    let before = serde_json::to_value(&s).unwrap();
    assert!(move_sequence_tab_state(&mut s, ItemId(99), u64::MAX).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

#[test]
fn shared_sequence_view_open_keeps_existing_validation_append_and_selection_behavior() {
    let (s, ids, _) = session();
    for open in [Vec::new(), ids.clone(), vec![ids[1], ids[1], ids[2]]] {
        let mut current = s.state.clone();
        current.open_sequences = open.clone();
        let before = current.clone();
        open_sequence_state(&s.project, &mut current, ids[0]).unwrap();
        let mut want = open;
        if !want.contains(&ids[0]) {
            want.push(ids[0]);
        }
        assert_eq!(current.open_sequences, want);
        assert_eq!(current.active_sequence, Some(ids[0]));
        assert!(current.selection.is_empty());
        collateral(&before, &current, &["open_sequences", "active_sequence", "selection"]);
    }
    let mut current = s.state.clone();
    let before = serde_json::to_value(&current).unwrap();
    assert!(open_sequence_state(&s.project, &mut current, ItemId(u64::MAX)).is_err());
    assert_eq!(serde_json::to_value(&current).unwrap(), before);
}

#[test]
fn shared_sequence_view_idle_human_commands_match_shared_state_events_journal_and_whole_history() {
    for command in ["sequence.open", "sequence.close", "sequence.closeOthers", "sequence.moveTab"] {
        let (mut s, ids, _) = session();
        let mut expected = s.state.clone();
        let target = ids[1];
        let (expected_value, event) = match command {
            "sequence.open" => {
                open_sequence_state(&s.project, &mut expected, target).unwrap();
                (Value::Null, true)
            }
            "sequence.close" => {
                close_sequence_state(&mut expected, target);
                (Value::Null, false)
            }
            "sequence.closeOthers" => {
                let (closed, opened) = close_other_sequence_states(&mut expected, target).unwrap();
                (json!({"closed":closed}), opened)
            }
            _ => {
                let index = move_sequence_tab_state(&mut expected, target, u64::MAX).unwrap();
                (json!({"index":index}), false)
            }
        };
        let args = if command == "sequence.moveTab" { json!({"item":target.0,"index":u64::MAX}) } else { json!({"item":target.0}) };
        let project = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.clone();
        let redo = s.history.redo.clone();
        let merge = s.history.merge_key.clone();
        assert_eq!(s.execute(command, args.clone()).unwrap(), expected_value);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&expected).unwrap());
        assert!(Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.revision, revision);
        same_stack(&undo, &s.history.undo);
        same_stack(&redo, &s.history.redo);
        assert_eq!(s.history.merge_key, merge);
        assert_eq!(s.journal, vec![(command.to_string(), args)]);
        let views: Vec<_> = s.events.iter().filter_map(|e| if let Event::OpenSequence(id) = e { Some(*id) } else { None }).collect();
        assert_eq!(views, if event { vec![target] } else { Vec::new() });
        let roundtrip: EditorState = serde_json::from_value(serde_json::to_value(&s.state).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&roundtrip).unwrap(), serde_json::to_value(&s.state).unwrap());
    }
}

#[test]
fn shared_sequence_view_hostile_human_targets_and_indices_keep_original_rejection_without_partial_state_or_history() {
    let (mut s, _, clip) = session();
    s.execute("clip.rename", json!({"clip":clip.0,"name":"Preserve the human redo branch"})).unwrap();
    s.undo().unwrap();
    assert!(!s.history.redo.is_empty());
    s.events.clear();
    for command in ["sequence.open", "sequence.close", "sequence.closeOthers", "sequence.moveTab"] {
        for value in [json!(-1), json!(0.25), json!(true), json!("bad"), json!([]), json!({})] {
            let args = json!({"item":value,"index":0});
            let project = s.project.clone();
            let state = serde_json::to_value(&s.state).unwrap();
            let undo = s.history.undo.clone();
            let redo = s.history.redo.clone();
            let merge = s.history.merge_key.clone();
            let revision = s.revision;
            let journal = s.journal.clone();
            assert!(s.execute(command, args).is_err());
            assert!(Arc::ptr_eq(&s.project, &project));
            assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
            assert_eq!(s.revision, revision);
            same_stack(&undo, &s.history.undo);
            same_stack(&redo, &s.history.redo);
            assert_eq!(s.history.merge_key, merge);
            assert_eq!(s.journal, journal);
        }
    }
    for value in [json!(-1), json!(0.25), json!(true), json!("bad"), json!([]), json!({}), json!(null)] {
        let before = serde_json::to_value(&s.state).unwrap();
        assert!(s.execute("sequence.moveTab", json!({"index":value})).is_err());
        assert_eq!(serde_json::to_value(&s.state).unwrap(), before);
    }
}

#[test]
fn shared_sequence_view_human_live_trim_and_loop_keep_original_settlement_and_no_agent_history() {
    for kind in ["trim", "ripple", "roll"] {
        for moved in [false, true] {
            for command in ["sequence.open", "sequence.close", "sequence.closeOthers", "sequence.moveTab"] {
                let (mut s, ids, clip) = session();
                s.history.merge_key = None;
                // Use the same movable synthetic cut as the original trim tests.
                // Default sync-locked overlay/music tracks can legitimately clamp ripple.
                for track in ["V2", "V3", "A2", "A3"] {
                    s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
                }
                s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
                s.execute("trim.shuttle", json!({"direction":if kind=="roll" {"forward"}else{"reverse"},"clock":0})).unwrap();
                if moved {
                    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
                }
                let prepared = crate::trim::command_settlement::prepare(&s, command).unwrap();
                assert_eq!(prepared.summary().dynamic_committed, moved, "{kind} moved={moved} before {command}");
                let project = prepared.project_after().clone();
                let undo = s.history.undo.len();
                let target = if command == "sequence.close" { ids[0] } else { ids[1] };
                let args = if command == "sequence.moveTab" { json!({"item":target.0,"index":u64::MAX}) } else { json!({"item":target.0}) };
                s.execute(command, args).unwrap();
                assert!(!s.trim_play.active());
                assert!(Arc::ptr_eq(&s.project, &project));
                assert_eq!(s.history.undo.len(), undo + usize::from(moved));
                if moved {
                    assert_eq!(s.history.undo.last().unwrap().0, prepared.summary().label.unwrap());
                }
            }
        }
    }
    for command in ["sequence.open", "sequence.close", "sequence.closeOthers", "sequence.moveTab"] {
        let (mut s, ids, clip) = session();
        s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":"roll"})).unwrap();
        s.execute("trim.playAround", json!({"clock":0})).unwrap();
        let undo = s.history.undo.clone();
        let args = if command == "sequence.moveTab" { json!({"item":ids[1].0,"index":u64::MAX}) } else { json!({"item":ids[1].0}) };
        s.execute(command, args).unwrap();
        assert!(!s.trim_play.active());
        same_stack(&undo, &s.history.undo);
    }
}

#[test]
fn shared_sequence_view_missing_tab_close_remains_original_successful_noop_and_no_active_view_stays_disabled() {
    let (mut s, _, _) = session();
    let before = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.clone();
    assert_eq!(s.execute("sequence.close", json!({"item":u64::MAX})).unwrap(), Value::Null);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before);
    same_stack(&undo, &s.history.undo);
    assert_eq!(s.journal.last().unwrap().0, "sequence.close");
    for _ in 0..s.state.open_sequences.len() {
        if s.state.active_sequence.is_none() {
            break;
        }
        s.execute("sequence.close", json!({})).unwrap();
    }
    assert!(s.state.active_sequence.is_none());
    for command in ["sequence.close", "sequence.closeOthers", "sequence.moveTab"] {
        let before = serde_json::to_value(&s.state).unwrap();
        let journal = s.journal.clone();
        assert!(s.execute(command, json!({"index":0})).is_err());
        assert_eq!(serde_json::to_value(&s.state).unwrap(), before);
        assert_eq!(s.journal, journal);
    }
}
