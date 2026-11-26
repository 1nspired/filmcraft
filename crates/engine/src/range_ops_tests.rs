use crate::{
    Session,
    range_ops::{self, Kind},
};
use filmcraft_project::{ClipId, ItemId, MarkerId};
use filmcraft_time::{Tick, TimeRange};
use serde_json::json;
use std::sync::Arc;
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
fn fork(s: &Session) -> Session {
    let mut n = Session::new(s.services.clone());
    n.project = s.project.clone();
    n.media = s.media.clone();
    n.state = s.state.clone();
    n.prefs = s.prefs.clone();
    n.revision = s.revision;
    n.history.undo = s.history.undo.clone();
    n.history.redo = s.history.redo.clone();
    n
}

#[test]
fn typed_range_matches_human_lift_and_extract_with_exact_saved_native_history() {
    for kind in [Kind::Lift, Kind::Extract] {
        let mut human = demo();
        let seq = human.state.active_sequence.unwrap();
        let unit = human.sequence_rate().frame_duration();
        let range = TimeRange::new(Tick(unit.0 * 3), Tick(unit.0 * 7));
        let q = Arc::make_mut(&mut human.project).sequence_mut(seq).unwrap();
        q.mark_in = Some(range.start);
        q.mark_out = Some(range.end() - unit);
        let mut explicit = fork(&human);
        let before = human.project.to_json();
        let targets = human.targeting().targeted;
        range_ops::prepare(&explicit, targets, range, kind, explicit.state.ripple_sequence_markers).unwrap().execute(&mut explicit).unwrap();
        human
            .execute(
                match kind {
                    Kind::Lift => "sequence.lift",
                    Kind::Extract => "sequence.extract",
                },
                json!({}),
            )
            .unwrap();
        assert_eq!(explicit.project.to_json(), human.project.to_json());
        assert_eq!(serde_json::to_value(&explicit.state).unwrap(), serde_json::to_value(&human.state).unwrap());
        let after = explicit.project.to_json();
        explicit.undo().unwrap();
        assert_eq!(explicit.project.to_json(), before);
        explicit.redo().unwrap();
        assert_eq!(explicit.project.to_json(), after);
        assert_eq!(filmcraft_project::Project::from_json(&after).unwrap().to_json(), after);
    }
}

#[test]
fn typed_range_expires_on_equal_value_replacement_and_refuses_hostile_bounds_without_publication() {
    let mut s = demo();
    let tracks = s.targeting().targeted;
    let unit = s.sequence_rate().frame_duration();
    let plan = range_ops::prepare(&s, tracks.clone(), TimeRange::new(unit, unit), Kind::Lift, false).unwrap();
    s.project = Arc::new((*s.project).clone());
    let before = s.project.clone();
    assert!(plan.execute(&mut s).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    for range in [TimeRange::new(Tick(-1), unit), TimeRange::new(unit, Tick::ZERO), TimeRange::new(Tick(i64::MAX), unit), TimeRange::new(unit, Tick(i64::MAX))]
    {
        assert!(range_ops::prepare(&s, tracks.clone(), range, Kind::Extract, true).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}

#[test]
fn typed_range_no_allocation_can_lift_whole_clips_at_exhausted_counter_but_partial_splits_refuse() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    p.next_id = u64::MAX;
    let q = p.sequence_mut(seq).unwrap();
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    for t in q.all_tracks_mut() {
        t.items.truncate(1);
        t.transitions.clear();
    }
    let range = q.video_tracks[0].items[0].range();
    let tracks = q.all_tracks().map(|t| t.id).collect::<Vec<_>>();
    let before = s.project.clone();
    let unit = s.sequence_rate().frame_duration();
    assert!(range_ops::prepare(&s, tracks.clone(), TimeRange::new(unit, unit), Kind::Lift, false).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    range_ops::prepare(&s, tracks, range, Kind::Lift, false).unwrap().execute(&mut s).unwrap();
    assert!(s.active_sequence().unwrap().all_tracks().all(|t| t.items.is_empty()));
    assert_eq!(s.project.next_id, u64::MAX);
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn shared_source_placement_reserves_above_legacy_ids_and_rejects_exhausted_allocations_atomically() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    s.execute("markers.add", json!({"name":"Legacy high marker","time":0})).unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().markers.last_mut().unwrap().id = MarkerId(50_000);
    let at = s.active_sequence().unwrap().duration();
    let unit = s.sequence_rate().frame_duration();
    let r = s.execute("timeline.place", json!({"item":5,"track":"V1","time":at.0,"sourceIn":0,"duration":unit.0,"video":true,"audio":false})).unwrap();
    assert!(r["clips"][0].as_u64().unwrap() > 50_000);
    assert_eq!(s.active_sequence().unwrap().markers.last().unwrap().id, MarkerId(50_000));
    s.undo().unwrap();
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let redo = s.history.redo.len();
    let revision = s.revision;
    assert!(s.execute("timeline.place", json!({"item":5,"track":"V1","time":at.0,"sourceIn":0,"duration":unit.0,"video":true,"audio":false})).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.history.undo.len(), undo);
    assert_eq!(s.history.redo.len(), redo);
    assert_eq!(s.revision, revision);
}

#[test]
fn explicit_replacement_uses_current_slot_and_native_lineage_without_changing_monitor_or_selection() {
    let mut human = demo();
    human.execute("poema.trackSequence", json!({})).unwrap();
    let id = human.active_sequence().unwrap().video_tracks[0].items[0].id;
    let source = human.project.items.values().find(|i| i.name == "Misty_Forest.mp4").unwrap().id;
    let unit = human.sequence_rate().frame_duration();
    human.execute("project.setMarks", json!({"item":source.0,"in":unit.0*2})).unwrap();
    human.execute("source.open", json!({"item":source.0})).unwrap();
    human.execute("sequence.linkedSelection", json!({"on":false})).unwrap();
    let original = human.active_sequence().unwrap().find_item(id).unwrap().1.clone();
    let parent = original.identity.as_ref().unwrap().id.clone();
    let mut explicit = fork(&human);
    explicit.state.source_item = Some(ItemId(5));
    explicit.state.selection.clear();
    let state = serde_json::to_value(&explicit.state).unwrap();
    let before = explicit.project.to_json();
    crate::clip_ops::replace_explicit(&mut explicit, source, &[(id, Tick(unit.0 * 2))]).unwrap();
    human.execute("clip.replaceFromSource", json!({"clips":[id.0]})).unwrap();
    let a = explicit.active_sequence().unwrap().find_item(id).unwrap().1;
    let b = human.active_sequence().unwrap().find_item(id).unwrap().1;
    assert_eq!(a.duration, original.duration);
    assert_eq!(a.start, original.start);
    assert_eq!(a.effects, original.effects);
    assert_eq!(a.speed, original.speed);
    assert_eq!(a.item, source);
    for c in [a, b] {
        assert_ne!(c.identity.as_ref().unwrap().id, parent);
        assert_eq!(c.identity.as_ref().unwrap().parent.as_deref(), Some(parent.as_str()));
        assert!(c.identity.as_ref().unwrap().annotations.is_empty());
    }
    let aid = a.identity.as_ref().unwrap().id.clone();
    let bid = b.identity.as_ref().unwrap().id.clone();
    assert_eq!(explicit.project.to_json().replace(&aid, "fresh-native-source-identity"), human.project.to_json().replace(&bid, "fresh-native-source-identity"));
    assert_eq!(serde_json::to_value(&explicit.state).unwrap(), state);
    let after = explicit.project.to_json();
    explicit.undo().unwrap();
    assert_eq!(explicit.project.to_json(), before);
    explicit.redo().unwrap();
    assert_eq!(explicit.project.to_json(), after);
}

#[test]
fn explicit_replacement_rejects_partial_locks_missing_targets_and_source_clock_overflow() {
    let mut s = demo();
    let q = s.active_sequence().unwrap();
    let id = q.video_tracks[0].items[0].id;
    let source = ItemId(5);
    s.execute("timeline.setTrack", json!({"track":"A1","locked":true})).unwrap();
    let audio = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    let before = s.project.clone();
    for rows in [vec![(id, Tick::ZERO), (audio, Tick::ZERO)], vec![(id, Tick::ZERO), (ClipId(999999), Tick::ZERO)], vec![(id, Tick(i64::MAX))]] {
        assert!(crate::clip_ops::replace_explicit(&mut s, source, &rows).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
