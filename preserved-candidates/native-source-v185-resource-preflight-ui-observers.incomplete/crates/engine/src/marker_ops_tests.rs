use super::*;
use filmcraft_project::{ItemId, ItemKind};
use std::sync::Arc;

fn fixture() -> (Session, ItemId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Marker test"})).unwrap();
    let source = s.execute("file.newBarsAndTone", json!({"seconds":20})).unwrap()["item"].as_u64().unwrap();
    s.execute("source.open", json!({"item":source})).unwrap();
    (s, ItemId(source))
}

fn marker(s: &mut Session, command: &str, params: Value) -> MarkerId {
    MarkerId(s.execute(command, params).unwrap()["marker"].as_u64().unwrap())
}

fn markers(s: &Session) -> Vec<Marker> {
    s.active_sequence().unwrap().markers.clone()
}

fn assert_refused(s: &mut Session, command: &str, params: Value) {
    let project = s.project.to_json();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(command, params.clone())));
    assert!(result.is_ok(), "{command} {params} panicked");
    assert!(result.unwrap().is_err(), "{command} {params} unexpectedly succeeded");
    assert_eq!(s.project.to_json(), project);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.undo, undo);
    assert_eq!(s.history.redo, redo);
}

#[test]
fn exact_marker_move_preserves_identity_kind_metadata_and_coincident_neighbors_with_owned_native_history() {
    let (mut s, _) = fixture();
    let first = marker(&mut s, "markers.add", json!({"time":13,"name":"Review","comment":"Human note","color":"Teal"}));
    let chapter = marker(&mut s, "markers.addChapter", json!({"time":13,"name":"Proposal","comment":"Keep chapter identity","color":"Rose"}));
    let other = marker(&mut s, "markers.addRange", json!({"time":29,"duration":17,"name":"Other"}));
    let before = s.project.to_json();
    let state = serde_json::to_value(&s.state).unwrap();
    let mut expected = markers(&s);
    let moving = expected.iter_mut().find(|m| m.id == chapter).unwrap();
    moving.start = Tick(29);
    expected.sort_by_key(|m| m.start);
    let n = s.history.undo.len();
    s.execute("markers.edit", json!({"target":"program","marker":chapter.0,"time":29})).unwrap();
    assert_eq!(s.history.undo.len(), n + 1);
    assert_eq!(markers(&s), expected);
    assert_eq!(markers(&s).iter().find(|m| m.id == first).unwrap().start, Tick(13));
    assert_eq!(markers(&s).iter().find(|m| m.id == other).unwrap().duration, Tick(17));
    let moved = markers(&s).into_iter().find(|m| m.id == chapter).unwrap();
    assert_eq!(moved.kind, MarkerKind::Chapter);
    assert_eq!((moved.name.as_str(), moved.comment.as_str(), moved.color), ("Proposal", "Keep chapter identity", Label::Rose));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state, "moving a marker does not seek or select");
    let after = s.project.to_json();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), after);
    assert_eq!(filmcraft_project::Project::from_json(&after).unwrap().to_json(), after);
}

#[test]
fn exact_marker_removal_keeps_coincident_markers_and_the_source_inventory() {
    let (mut s, source) = fixture();
    let a = marker(&mut s, "markers.add", json!({"time":13,"name":"A"}));
    let b = marker(&mut s, "markers.addChapter", json!({"time":13,"name":"B"}));
    let source_marker = marker(&mut s, "markers.add", json!({"target":"source","time":13,"name":"Source"}));
    let source_before = s.project.item(source).unwrap().clone();
    let before = s.project.to_json();
    let state = serde_json::to_value(&s.state).unwrap();
    let result = s.execute("markers.remove", json!({"target":"program","marker":a.0})).unwrap();
    assert_eq!(result, json!({"removed":a.0}));
    assert_eq!(markers(&s).iter().map(|m| m.id).collect::<Vec<_>>(), vec![b]);
    assert_eq!(s.project.item(source).unwrap(), &source_before);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    let after = s.project.to_json();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), after);
    assert_refused(&mut s, "markers.remove", json!({"marker":source_marker.0}));
    assert_refused(&mut s, "markers.remove", json!({"target":"source","marker":b.0}));
}

#[test]
fn source_marker_move_uses_source_frames_and_shared_id_removal_without_touching_the_cut() {
    let (mut s, source) = fixture();
    let ItemKind::Media(m) = &mut Arc::make_mut(&mut s.project).item_mut(source).unwrap().kind else { panic!("fixture source") };
    m.info.video.as_mut().unwrap().frame_rate = FrameRate::FPS_60;
    let a = marker(&mut s, "markers.add", json!({"target":"source","time":13,"duration":7,"name":"A"}));
    let b = marker(&mut s, "markers.add", json!({"target":"source","time":13,"name":"B"}));
    let cut = s.active_sequence().unwrap().clone();
    let before = s.project.to_json();
    let state = serde_json::to_value(&s.state).unwrap();
    s.execute("markers.edit", json!({"target":"source","marker":a.0,"frame":12})).unwrap();
    let list = &s.project.item(source).unwrap().as_media().unwrap().markers;
    let edited = list.iter().find(|m| m.id == a).unwrap();
    assert_eq!(edited.start, FrameRate::FPS_60.tick_of(12));
    assert_ne!(edited.start, s.sequence_rate().tick_of(12));
    assert_eq!(edited.duration, Tick(7));
    assert_eq!(s.active_sequence().unwrap(), &cut);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    let after = s.project.to_json();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), after);
    s.execute("markers.remove", json!({"target":"source","marker":b.0})).unwrap();
    assert_eq!(s.project.item(source).unwrap().as_media().unwrap().markers.iter().map(|m| m.id).collect::<Vec<_>>(), vec![a]);
    assert_eq!(s.active_sequence().unwrap(), &cut);
}

#[test]
fn source_range_relocation_refuses_truncation_while_existing_duration_gesture_clamps_at_the_source_end() {
    let (mut s, source) = fixture();
    let end = crate::clip_ops::source_view(&s, source).unwrap().end;
    let id = marker(&mut s, "markers.add", json!({"target":"source","time":0,"duration":7}));
    assert_refused(&mut s, "markers.edit", json!({"target":"source","marker":id.0,"time":end.0-3}));
    s.execute("markers.edit", json!({"target":"source","marker":id.0,"time":end.0-7})).unwrap();
    s.execute("markers.edit", json!({"target":"source","marker":id.0,"durationFrames":1})).unwrap();
    assert_eq!(s.project.item(source).unwrap().as_media().unwrap().markers[0].duration, Tick(7));
    assert_refused(&mut s, "markers.edit", json!({"target":"source","marker":id.0,"time":end.0}));
}

#[test]
fn all_marker_creation_services_reserve_ids_above_legacy_native_inventory_without_renumbering() {
    let (mut s, source) = fixture();
    marker(&mut s, "markers.add", json!({"time":13}));
    let q = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(q).unwrap().markers[0].id = MarkerId(90000);
    Arc::make_mut(&mut s.project).next_id = 1;
    let mut allocated = Vec::new();
    for (cmd, p) in [
        ("markers.add", json!({"time":13})),
        ("markers.addRange", json!({"time":13,"duration":7})),
        ("markers.addChapter", json!({"time":13})),
        ("markers.addFlashCue", json!({"time":0})),
        ("markers.add", json!({"target":"source","time":0})),
    ] {
        allocated.push(marker(&mut s, cmd, p));
    }
    assert!(allocated.iter().all(|id| id.0 > 90000));
    let mut dedup = allocated.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), allocated.len());
    assert!(markers(&s).iter().any(|m| m.id == MarkerId(90000)));
    assert_eq!(s.project.item(source).unwrap().as_media().unwrap().markers[0].id, *allocated.last().unwrap());
}

#[test]
fn malformed_marker_gestures_preserve_the_cut_revision_and_both_history_branches_without_panicking() {
    let (mut s, _) = fixture();
    let id = marker(&mut s, "markers.add", json!({"time":13,"duration":7}));
    marker(&mut s, "markers.add", json!({"time":29}));
    s.execute("edit.undo", json!({})).unwrap();
    for patch in [
        json!({}),
        json!({"time":-1}),
        json!({"time":1.5}),
        json!({"time":null}),
        json!({"frame":i64::MAX}),
        json!({"time":1,"frame":1}),
        json!({"time":i64::MAX,"duration":1}),
        json!({"duration":-1}),
        json!({"durationFrames":i64::MAX}),
        json!({"duration":1,"durationFrames":1}),
        json!({"name":17}),
        json!({"comment":null}),
        json!({"name":"\0"}),
        json!({"name":"x".repeat(1025)}),
        json!({"color":"Red"}),
        json!({"kind":"Chapter"}),
        json!({"unknown":true}),
    ] {
        let mut params = patch;
        params["marker"] = json!(id.0);
        assert_refused(&mut s, "markers.edit", params);
    }
    for params in [
        json!({"marker":0}),
        json!({"marker":-1}),
        json!({"marker":1.5}),
        json!({"marker":null}),
        json!({"marker":id.0,"time":13}),
        json!({"target":"other","marker":id.0}),
    ] {
        assert_refused(&mut s, "markers.remove", params);
    }
    for cmd in ["markers.add", "markers.addRange", "markers.addFlashCue"] {
        assert_refused(&mut s, cmd, json!({"time":0,"durationFrames":i64::MAX}));
        assert_refused(&mut s, cmd, json!({"time":0,"duration":-1}));
        assert_refused(&mut s, cmd, json!({"time":0,"name":17}));
    }
}

#[test]
fn duplicated_or_foreign_marker_identity_refuses_instead_of_editing_the_first_match() {
    let (mut s, _) = fixture();
    let id = marker(&mut s, "markers.add", json!({"time":13}));
    let q = s.state.active_sequence.unwrap();
    let duplicate = markers(&s)[0].clone();
    Arc::make_mut(&mut s.project).sequence_mut(q).unwrap().markers.push(duplicate);
    assert_refused(&mut s, "markers.edit", json!({"marker":id.0,"time":29}));
    assert_refused(&mut s, "markers.remove", json!({"marker":id.0}));
    s.execute("file.newSequence", json!({"name":"Other sequence"})).unwrap();
    assert_refused(&mut s, "markers.edit", json!({"marker":id.0,"time":29}));
    assert_refused(&mut s, "markers.remove", json!({"marker":id.0}));
}

#[test]
fn marker_frame_positions_and_durations_use_native_ceiling_boundaries_at_exotic_rates() {
    let (mut s, source) = fixture();
    let rate = FrameRate::new(37516, 1000);
    let q = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(q).unwrap().settings.frame_rate = rate;
    let ItemKind::Media(media) = &mut Arc::make_mut(&mut s.project).item_mut(source).unwrap().kind else { panic!("fixture source") };
    media.info.video.as_mut().unwrap().frame_rate = rate;
    for target in ["program", "source"] {
        let id = marker(&mut s, "markers.add", json!({"target":target,"time":0}));
        s.execute("markers.edit", json!({"target":target,"marker":id.0,"frame":17,"durationFrames":11})).unwrap();
        let list = if target == "program" { &s.active_sequence().unwrap().markers } else { &s.project.item(source).unwrap().as_media().unwrap().markers };
        let actual = list.iter().find(|m| m.id == id).unwrap();
        assert_eq!(rate.frame_at(actual.start), 17, "position belongs to the requested native frame");
        assert_eq!(actual.start, rate.tick_of(17));
        assert_eq!(actual.duration, rate.tick_of(11));
    }
}

#[test]
fn exhausted_native_id_space_refuses_every_marker_creation_without_losing_redo() {
    let (mut s, _) = fixture();
    marker(&mut s, "markers.add", json!({"time":13}));
    s.execute("edit.undo", json!({})).unwrap();
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    for (cmd, p) in [
        ("markers.add", json!({})),
        ("markers.addRange", json!({})),
        ("markers.addChapter", json!({})),
        ("markers.addFlashCue", json!({})),
        ("markers.add", json!({"target":"source"})),
    ] {
        assert_refused(&mut s, cmd, p);
    }
}
