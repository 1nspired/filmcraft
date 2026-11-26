//! Preserve tracked pieces only inside the SAME original merged recording pass.
//! The ordinary reconciliation guard remains unchanged and checks the candidate afterwards.
use crate::{EngineError, Result, commands::bad};
use filmcraft_project::{ClipId, ItemId, Project, TrackId, TrackItem};
use std::collections::{BTreeMap, BTreeSet};
type Key = (u64, i64, i64, u64, bool, Option<i64>, String);
fn error(message: &str) -> EngineError {
    bad("multicam.cut", message)
}
fn key(track: TrackId, clip: &TrackItem, identity: &str) -> Key {
    (track.0, clip.start.0, clip.source_in.0, clip.speed.to_bits(), clip.reverse, clip.frame_hold.map(|t| t.0), identity.to_owned())
}
pub(super) fn tracked(project: &Project, sequence: ItemId, tracks: &[TrackId], source: ItemId) -> bool {
    project.sequence(sequence).is_some_and(|q| {
        q.all_tracks().any(|t| tracks.contains(&t.id) && t.items.iter().any(|c| c.item == source && c.multicam.is_some() && c.identity.is_some()))
    })
}
pub(super) fn reserve(current: &Project, candidate: &mut Project, cuts: usize, tracks: usize) -> Result<()> {
    if cuts > 100_000 || tracks > 100_000 {
        return Err(error("the original tracked recording exceeds its bounded edit census"));
    }
    let needed = cuts
        .checked_add(1)
        .and_then(|n| n.checked_mul(tracks))
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| error("recording allocation bound overflow"))?;
    let first = current.next_id.max(candidate.next_id);
    first.checked_add(needed).filter(|n| *n < u64::MAX).ok_or_else(|| error("recording allocation identity exhausted"))?;
    candidate.next_id = first;
    Ok(())
}
pub(super) fn preserve(current: &Project, candidate: &mut Project, sequence: ItemId, tracks: &[TrackId], source: ItemId) -> Result<()> {
    filmcraft_project::clip_identity::validate_project(current).map_err(|e| bad("multicam.cut", e))?;
    let before = current.sequence(sequence).ok_or_else(|| error("the actual recording sequence disappeared"))?;
    let mut locations: BTreeMap<Key, Option<&TrackItem>> = BTreeMap::new();
    let mut count = 0usize;
    for track in before.all_tracks().filter(|t| tracks.contains(&t.id)) {
        for clip in track.items.iter().filter(|c| c.item == source && c.multicam.is_some()) {
            count = count.checked_add(1).ok_or_else(|| error("recording piece count overflow"))?;
            if count > 100_000 {
                return Err(error("the tracked recording has too many current pieces"));
            }
            if let Some(identity) = &clip.identity {
                for id in std::iter::once(&identity.id).chain(identity.parent.iter()) {
                    let slot = locations.entry(key(track.id, clip, id)).or_insert(Some(clip));
                    if slot.is_some_and(|old| !std::ptr::eq(old, clip)) {
                        *slot = None;
                    }
                }
            }
        }
    }
    let after = candidate.sequence_mut(sequence).ok_or_else(|| error("the recording candidate lost its sequence"))?;
    let mut ids = BTreeMap::<(u64, u64), u64>::new();
    let mut links = BTreeMap::<u64, u64>::new();
    let mut assigned = BTreeSet::new();
    for track in after.all_tracks_mut().filter(|t| tracks.contains(&t.id)) {
        for clip in track.items.iter_mut().filter(|c| c.item == source && c.multicam.is_some()) {
            let Some(identity) = &clip.identity else { continue };
            let Some(found) = locations.get(&key(track.id, clip, &identity.id)) else { continue };
            let old = found.ok_or_else(|| error("recording piece lineage is ambiguous; current work was retained"))?;
            if !assigned.insert(old.id.0) {
                return Err(error("two recording pieces cannot borrow one current identity"));
            }
            if ids.insert((track.id.0, clip.id.0), old.id.0).is_some() {
                return Err(error("duplicate recording candidate clip identity"));
            }
            match (clip.link, old.link) {
                (Some(new), Some(prior)) => {
                    if links.insert(new, prior).is_some_and(|previous| previous != prior) {
                        return Err(error("recording linked-piece correspondence is ambiguous"));
                    }
                }
                (None, None) => {}
                _ => return Err(error("recording linked-piece correspondence changed")),
            }
            clip.id = old.id;
            clip.identity = old.identity.clone();
        }
    }
    for track in after.all_tracks_mut().filter(|t| tracks.contains(&t.id)) {
        for clip in track.items.iter_mut().filter(|c| c.item == source && c.multicam.is_some()) {
            if let Some(link) = clip.link.and_then(|l| links.get(&l)) {
                clip.link = Some(*link);
            }
        }
        for transition in &mut track.transitions {
            if let Some(id) = transition.from.and_then(|id| ids.get(&(track.id.0, id.0))) {
                transition.from = Some(ClipId(*id));
            }
            if let Some(id) = transition.to.and_then(|id| ids.get(&(track.id.0, id.0))) {
                transition.to = Some(ClipId(*id));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ClipIdentity, Label, SequenceSettings};
    use serde_json::json;
    fn fixture() -> (Project, ItemId, TrackId, ItemId) {
        let mut p = Project::new("Recording continuity");
        let sequence = p.new_sequence("Edit", SequenceSettings::default(), 1, 0, None);
        let source = p.new_sequence("Source", SequenceSettings::default(), 1, 1, None);
        let track = p.sequence(sequence).unwrap().video_tracks[0].id;
        let mut c:TrackItem=serde_json::from_value(json!({"id":100,"item":source.0,"name":"Piece","label":Label::Iris,"start":10,"duration":20,"source_in":10,"speed":1.0,"enabled":true,"effects":[],"multicam":{"enabled":true,"angle":1},"link":40})).unwrap();
        c.identity = Some(Box::new(ClipIdentity {
            id: "b".repeat(32),
            parent: Some("a".repeat(32)),
            annotations: std::collections::BTreeMap::from([("note".into(), "same live piece".into())]),
            protection: None,
        }));
        p.sequence_mut(sequence).unwrap().video_tracks[0].items.push(c);
        p.item_mut(sequence).unwrap().metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
        p.next_id = 1000;
        (p, sequence, track, source)
    }
    fn candidate(before: &Project, sequence: ItemId) -> Project {
        let mut p = before.clone();
        let clip = &mut p.sequence_mut(sequence).unwrap().video_tracks[0].items[0];
        clip.id = ClipId(500);
        clip.link = Some(50);
        clip.identity.as_mut().unwrap().id = "a".repeat(32);
        clip.identity.as_mut().unwrap().parent = None;
        clip.identity.as_mut().unwrap().annotations.clear();
        p
    }
    #[test]
    fn original_live_piece_keeps_current_ids_notes_and_link_before_unchanged_ordinary_reconciliation() {
        let (before, sequence, track, source) = fixture();
        let snapshot = before.clone();
        let mut after = candidate(&before, sequence);
        preserve(&before, &mut after, sequence, &[track], source).unwrap();
        assert_eq!(after.sequence(sequence).unwrap().video_tracks[0].items, before.sequence(sequence).unwrap().video_tracks[0].items);
        crate::clip_identity::reconcile(&before, &mut after).unwrap();
        assert_eq!(before, snapshot);
        assert_eq!(after, before);
    }
    #[test]
    fn identical_coordinates_without_the_original_identity_lineage_cannot_borrow_an_existing_piece() {
        let (before, sequence, track, source) = fixture();
        let mut after = candidate(&before, sequence);
        after.sequence_mut(sequence).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().id = "d".repeat(32);
        preserve(&before, &mut after, sequence, &[track], source).unwrap();
        assert_eq!(after.sequence(sequence).unwrap().video_tracks[0].items[0].id, ClipId(500));
        assert_ne!(after.sequence(sequence).unwrap().video_tracks[0].items[0].identity, before.sequence(sequence).unwrap().video_tracks[0].items[0].identity);
    }
    #[test]
    fn ambiguous_live_piece_lineage_refuses_before_publication_without_changing_the_current_project() {
        let (mut before, sequence, track, source) = fixture();
        let mut duplicate = before.sequence(sequence).unwrap().video_tracks[0].items[0].clone();
        duplicate.id = ClipId(101);
        duplicate.identity.as_mut().unwrap().id = "c".repeat(32);
        before.sequence_mut(sequence).unwrap().video_tracks[0].items.push(duplicate);
        let snapshot = before.clone();
        let mut after = candidate(&before, sequence);
        after.sequence_mut(sequence).unwrap().video_tracks[0].items.truncate(1);
        assert!(preserve(&before, &mut after, sequence, &[track], source).is_err());
        assert_eq!(before, snapshot);
    }
    #[test]
    fn tracked_recording_allocation_exhaustion_and_oversized_census_refuse_without_partial_counter_changes() {
        let (mut before, sequence, _, _) = fixture();
        let mut after = candidate(&before, sequence);
        let old = after.next_id;
        before.next_id = u64::MAX - 1;
        assert!(reserve(&before, &mut after, 2, 2).is_err());
        assert_eq!(after.next_id, old);
        before.next_id = 1000;
        assert!(reserve(&before, &mut after, 100001, 1).is_err());
        assert_eq!(after.next_id, old);
        reserve(&before, &mut after, 2, 2).unwrap();
        assert_eq!(after.next_id, 1000);
    }
    #[test]
    fn retained_recording_link_translation_never_changes_an_unselected_track() {
        let (mut before, sequence, track, source) = fixture();
        let mut other = filmcraft_project::Track::new(TrackId(1001), filmcraft_project::TrackKind::Video, "Unselected".into());
        let mut clip = before.sequence(sequence).unwrap().video_tracks[0].items[0].clone();
        clip.id = ClipId(101);
        clip.identity.as_mut().unwrap().id = "c".repeat(32);
        clip.identity.as_mut().unwrap().parent = None;
        clip.link = Some(50);
        other.items.push(clip);
        before.sequence_mut(sequence).unwrap().video_tracks.push(other);
        before.next_id = 2000;
        let untouched = before.sequence(sequence).unwrap().video_tracks[1].clone();
        let mut after = candidate(&before, sequence);
        preserve(&before, &mut after, sequence, &[track], source).unwrap();
        assert_eq!(after.sequence(sequence).unwrap().video_tracks[1], untouched);
        crate::clip_identity::reconcile(&before, &mut after).unwrap();
        assert_eq!(after.sequence(sequence).unwrap().video_tracks[1], untouched);
    }
    #[test]
    fn ordinary_instance_reassignment_guard_is_still_required_and_rejects_a_changed_live_id() {
        let (before, sequence, _, _) = fixture();
        let mut after = before.clone();
        after.sequence_mut(sequence).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().id = "d".repeat(32);
        assert!(crate::clip_identity::reconcile(&before, &mut after).is_err());
    }
}
