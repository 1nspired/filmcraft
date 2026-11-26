use std::sync::Arc;

use filmcraft_project::{ClipId, Project};
use serde_json::{Value, json};

use crate::Session;

pub(super) fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}

#[test]
fn native_effect_tracking_is_explicit_and_undoable() {
    let mut s = setup();
    let before = s.project.clone();
    let n = s.history.undo.len();
    let r = s.execute("poema.trackEffects", json!({})).unwrap();
    assert_eq!(r["trackedEffects"], true);
    assert_eq!(s.history.undo.len(), n + 1);
    s.undo().unwrap();
    assert_eq!(s.project, before);
}
fn first(s: &Session) -> ClipId {
    s.active_sequence().unwrap().video_tracks[0].items[0].id
}
fn clip(s: &Session, c: ClipId) -> filmcraft_project::TrackItem {
    s.active_sequence().unwrap().find_item(c).unwrap().1.clone()
}
fn identity(s: &Session, c: ClipId) -> filmcraft_project::ClipIdentity {
    *clip(s, c).identity.unwrap()
}
fn snapshot(s: &mut Session, n: u64) -> Value {
    s.execute("agent.snapshot", json!({"token": format!("persistent-identity-test-{n:032}")})).unwrap()
}
fn trim(c: ClipId, frames: i64, mode: &str) -> Value {
    json!({"command": "timeline.trim", "params": {"clip": c.0, "edge": "out", "mode": mode, "deltaFrames": frames}})
}
fn apply(s: &mut Session, read: &Value, ops: Vec<Value>) -> crate::Result<Value> {
    s.execute("agent.apply", json!({"token": read["token"], "label": "Identity test", "operations": ops}))
}
fn protect(s: &mut Session, c: ClipId) {
    s.execute("poema.protectClips", json!({"clips": [c.0], "reason": "Keep my chosen moment"})).unwrap();
}
fn track_at(s: &Session, c: ClipId) -> filmcraft_project::TrackId {
    s.active_sequence().unwrap().find_item(c).unwrap().0
}

#[test]
fn opt_in_assigns_unique_ids_and_existing_legacy_project_remains_untracked() {
    let mut legacy = Session::default();
    legacy.execute("file.openDemoProject", json!({})).unwrap();
    assert!(clip(&legacy, first(&legacy)).identity.is_none());
    let mut s = setup();
    let all: Vec<_> = s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).map(|c| &c.identity.as_ref().unwrap().id).collect();
    assert_eq!(all.len(), all.iter().collect::<std::collections::BTreeSet<_>>().len());
    let saved = s.project.clone();
    s.execute("poema.trackSequence", json!({})).unwrap();
    assert_eq!(s.project, saved);
}

#[test]
fn split_and_real_copy_fork_notes_and_undo_branch_never_reuses_identity() {
    let mut s = setup();
    let c = first(&s);
    s.execute("poema.annotateClips", json!({"clips": [c.0], "annotations": {"intent": "Keep the reaction", "selectId": "sel_test"}})).unwrap();
    let original = identity(&s, c);
    let cut = clip(&s, c).start + s.sequence_rate().tick_of(12);
    let read = snapshot(&mut s, 1);
    apply(&mut s, &read, vec![json!({"command": "timeline.razor", "params": {"clip": c.0, "time": cut.0}})]).unwrap();
    let right = s.active_sequence().unwrap().track(track_at(&s, c)).unwrap().items.iter().find(|i| i.start == cut).unwrap().id;
    let branch_a = identity(&s, right);
    assert_eq!(identity(&s, c), original);
    assert_ne!(branch_a.id, original.id);
    assert_eq!(branch_a.parent.as_deref(), Some(original.id.as_str()));
    assert_eq!(branch_a.annotations, original.annotations);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("timeline.razor", json!({"clip": c.0, "time": cut.0})).unwrap();
    let branch_b = identity(&s, right); // same numeric handle reused on another edit branch
    assert_ne!(branch_a.id, branch_b.id);
    assert_eq!(branch_b.parent, branch_a.parent);
    let destination = s.active_sequence().unwrap().duration() + s.sequence_rate().tick_of(24);
    let copied = s.execute("timeline.move", json!({"moves": [{"clip": c.0, "track": "V1", "time": destination.0}], "copy": true, "insert": false})).unwrap();
    let ids = copied["copied"].as_array().unwrap();
    // Copy result may include linked audio; find the copy on V1 using its timeline position.
    assert!(!ids.is_empty());
    let new = s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.start == destination).unwrap().id;
    let duplicate = identity(&s, new);
    assert_eq!(duplicate.parent.as_deref(), Some(original.id.as_str()));
    assert_eq!(duplicate.annotations, original.annotations);
    assert_ne!(duplicate.id, original.id);
    let saved = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, saved);
}

#[test]
fn native_trim_keeps_id_and_annotations_and_roundtrip_preserves_protection() {
    let mut s = setup();
    let c = first(&s);
    s.execute("poema.annotateClips", json!({"clips": [c.0], "annotations": {"intent": "Keep"}})).unwrap();
    protect(&mut s, c);
    let original = identity(&s, c);
    let read = snapshot(&mut s, 1);
    assert!(apply(&mut s, &read, vec![trim(c, -2, "regular")]).unwrap_err().to_string().contains("protected clip"));
    // The editor still has authority to trim their own protected decision manually.
    s.execute("timeline.trim", trim(c, -2, "regular")["params"].clone()).unwrap();
    assert_eq!(identity(&s, c), original);
    let loaded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
    assert_eq!(loaded.project, *s.project);
    assert_eq!(loaded.schema_version, filmcraft_format::SCHEMA_VERSION);
    s.project = Arc::new(loaded.project);
    let read = snapshot(&mut s, 2);
    assert!(apply(&mut s, &read, vec![trim(c, -1, "regular")]).is_err());
    s.execute("poema.protectClips", json!({"clips": [c.0], "reason": null})).unwrap();
    let read = snapshot(&mut s, 3);
    apply(&mut s, &read, vec![trim(c, -1, "regular")]).unwrap();
    assert_eq!(identity(&s, c).id, original.id);
    s.execute("edit.undo", json!({})).unwrap(); // trim
    s.execute("edit.undo", json!({})).unwrap(); // unlock
    assert!(identity(&s, c).protection.is_some());
}

#[test]
fn collateral_ripple_and_linked_audio_protection_roll_back_every_staged_operation() {
    let mut s = setup();
    // A minimal pair: the demo's separate full-length music bed correctly blocks ripple on
    // sync-lock grounds before protection is consulted. That is not the guard under test.
    let source = clip(&s, first(&s)).item;
    let rate = s.sequence_rate();
    s.execute("file.newSequence", json!({"name": "Ripple guard fixture", "video": 1, "audio": 1})).unwrap();
    for frame in [0, 24] {
        s.execute(
            "timeline.place",
            json!({"item": source.0, "track": "V1", "audioTrack": "A1", "frame": frame, "sourceIn": 0, "duration": rate.tick_of(24).0}),
        )
        .unwrap();
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    let c = first(&s);
    let later = s.active_sequence().unwrap().video_tracks[0].items[1].id;
    protect(&mut s, later);
    let read = snapshot(&mut s, 1);
    let before = s.project.clone();
    let revision = s.revision;
    let rejection = apply(&mut s, &read, vec![trim(c, -1, "regular"), trim(c, -1, "ripple")]).unwrap_err().to_string();
    assert!(rejection.contains("protected clip"), "unexpected rejection: {rejection}");
    assert!(Arc::ptr_eq(&s.project, &before));
    assert_eq!(s.revision, revision);
    // Protecting picture freezes its linked sound too, independently of linked-selection mode.
    protect(&mut s, c);
    let link = clip(&s, c).link.unwrap();
    let sound = s.active_sequence().unwrap().audio_tracks.iter().flat_map(|t| &t.items).find(|i| i.link == Some(link)).unwrap().id;
    assert!(identity(&s, sound).protection.is_some());
    s.state.linked_selection = false;
    let read = snapshot(&mut s, 2);
    assert!(apply(&mut s, &read, vec![trim(sound, -1, "regular")]).is_err());
}

#[test]
fn agent_cannot_unlock_annotate_or_disable_tracking_inside_a_batch() {
    let mut s = setup();
    let c = first(&s);
    protect(&mut s, c);
    let read = snapshot(&mut s, 1);
    for command in ["poema.protectClips", "poema.annotateClips", "poema.trackSequence"] {
        assert!(apply(&mut s, &read, vec![json!({"command": command, "params": {"clips": [c.0], "reason": null}})]).is_err());
    }
    assert!(identity(&s, c).protection.is_some());
}

#[test]
fn damaged_identity_duplicate_identity_and_oversized_notes_fail_closed() {
    let mut s = setup();
    let c = first(&s);
    let old = s.project.clone();
    for params in [
        json!({"clips": [], "annotations": {}}),
        json!({"clips": [c.0], "annotations": {"x": "y".repeat(1025)}}),
        json!({"clips": [999999], "annotations": {}}),
    ] {
        assert!(s.execute("poema.annotateClips", params).is_err());
    }
    assert!(Arc::ptr_eq(&old, &s.project));
    assert!(s.execute("poema.protectClips", json!({"clips": [c.0]})).is_err());
    assert!(s.execute("poema.protectClips", json!({"clips": [c.0], "reason": " "})).is_err());
    let mut damaged = (*s.project).clone();
    let sid = s.state.active_sequence.unwrap();
    let q = damaged.sequence_mut(sid).unwrap();
    q.video_tracks[0].items[1].identity = q.video_tracks[0].items[0].identity.clone();
    assert!(filmcraft_format::decode(&filmcraft_format::encode(&damaged, false)).is_err());
    q_bad(&mut damaged, sid, c);
    assert!(filmcraft_format::decode(&filmcraft_format::encode(&damaged, false)).is_err());
}

fn q_bad(p: &mut Project, sid: filmcraft_project::ItemId, c: ClipId) {
    p.sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.identity.as_mut().unwrap().id = "bad".into();
}

#[test]
fn new_placements_in_an_emptied_tracked_sequence_get_fresh_identity() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    let c = clip(&s, first(&s));
    s.edit("Empty test timeline", |p, _| {
        for t in p.sequence_mut(sid).unwrap().all_tracks_mut() {
            t.items.clear();
            t.transitions.clear();
        }
        Ok(())
    })
    .unwrap();
    s.edit("Restore a copy", |p, _| {
        p.sequence_mut(sid).unwrap().video_tracks[0].items.push(c.clone());
        Ok(())
    })
    .unwrap();
    let new = identity(&s, c.id);
    assert_ne!(new.id, c.identity.as_ref().unwrap().id);
    assert_eq!(new.parent.as_ref(), Some(&c.identity.as_ref().unwrap().id));
}

#[test]
fn protected_nested_picture_blocks_edits_inside_its_source_sequence() {
    let mut s = setup();
    let inner = s.state.active_sequence.unwrap();
    let c = first(&s);
    let rate = s.sequence_rate();
    s.execute("file.newSequence", json!({"name": "Outer", "video": 1, "audio": 1})).unwrap();
    s.execute("timeline.place", json!({"item": inner.0, "track": "V1", "audioTrack": "A1", "frame": 0, "sourceIn": 0, "duration": rate.tick_of(24).0}))
        .unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let nest = first(&s);
    protect(&mut s, nest);
    s.execute("sequence.open", json!({"item": inner.0})).unwrap();
    let read = snapshot(&mut s, 1);
    let old = s.project.clone();
    assert!(apply(&mut s, &read, vec![trim(c, -1, "regular")]).unwrap_err().to_string().contains("protected clip source/nested sequence"));
    assert!(Arc::ptr_eq(&s.project, &old));
}

#[test]
fn changing_source_mints_new_identity_clears_old_notes_and_keeps_protection() {
    let mut s = setup();
    let c = first(&s);
    let sid = s.state.active_sequence.unwrap();
    s.execute("poema.annotateClips", json!({"clips": [c.0], "annotations": {"selectId": "source_specific"}})).unwrap();
    protect(&mut s, c);
    let old = identity(&s, c);
    let other = s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.item != clip(&s, c).item).unwrap().item;
    s.edit("Manual source replacement", |p, _| {
        p.sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.item = other;
        Ok(())
    })
    .unwrap();
    let new = identity(&s, c);
    assert_ne!(old.id, new.id);
    assert_eq!(new.parent.as_ref(), Some(&old.id));
    assert!(new.annotations.is_empty());
    assert_eq!(new.protection, old.protection);
    let loaded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
    assert_eq!(*s.project, loaded.project);
}
