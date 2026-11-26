//! Shared native movement planning and publication for UI/CLI and guarded adapters.
use crate::{
    EngineError, Result, Session,
    commands::{bad, bool_p, clip_p, str_p, track_p},
};
use filmcraft_edit as edit;
use filmcraft_project::{ClipId, ItemId, Project, Sequence, Track, TrackId, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

const CMD: &str = "timeline.move";
const MAX_MOVES: usize = 4096;

/// A fully resolved native request. It cannot be applied after its document or
/// active sequence changes. This type is not a model command or authorization.
pub(crate) struct MovementPlan {
    basis: Arc<Project>,
    sequence: ItemId,
    pub(crate) moves: Vec<(ClipId, TrackId, Tick)>,
    new_moves: Vec<(ClipId, TrackId)>,
    insert: bool,
    copy: bool,
    linked_basis: Option<bool>,
    budget: u64,
}
fn request(p: &Value) -> Result<&[Value]> {
    let obj = p.as_object().ok_or_else(|| bad(CMD, "need a movement object"))?;
    if obj.keys().any(|k| !["moves", "insert", "linked", "copy"].contains(&k.as_str())) {
        return Err(bad(CMD, "unknown movement field"));
    }
    for key in ["insert", "linked", "copy"] {
        if p.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(bad(CMD, format!("explicit {key} must be boolean")));
        }
    }
    let moves =
        p.get("moves").and_then(Value::as_array).filter(|v| !v.is_empty() && v.len() <= MAX_MOVES).ok_or_else(|| bad(CMD, "need 1..4096 complete moves"))?;
    if moves.iter().any(|m| m.as_object().is_none_or(|o| o.len() != 3 || !["clip", "track", "time"].iter().all(|k| o.contains_key(*k)))) {
        return Err(bad(CMD, "every move needs exactly clip, track and integer time"));
    }
    Ok(moves)
}
/// `timeline.move`: move the listed clips to (track, time). With Linked Selection on (or
/// `linked: true`) the linked partners of a listed clip that are not listed themselves follow by
/// the same time offset on their own tracks, so picture and sound stay in sync; a partner on a
/// locked track, or one whose position would not change, stays as it is. Nothing moves before the sequence start: when a clip would,
/// every clip of the call lands later by the same amount, so the spacing asked for is kept.
/// `overwritten` lists every clip the move covered without being asked to move it: its length
/// before and after (ticks) and the clips what is left of it now lives on as (none = removed; a
/// clip a moved clip lands inside is split in two). With `copy: true` (Option-drag) the clips stay
/// where they are and copies land at the destinations instead; the copies are selected and listed
/// as `copied`.
pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    prepare(s, p)?.execute(s)
}
pub(crate) fn prepare(s: &Session, p: &Value) -> Result<MovementPlan> {
    let mut listed: Vec<(ClipId, TrackId, Tick)> = Vec::new();
    let mut new_moves = Vec::new();
    let rows = request(p)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    for m in rows {
        let clip = clip_p(m, "clip")?.ok_or_else(|| bad(CMD, "every move needs a clip"))?;
        if listed.iter().any(|(id, _, _)| *id == clip) {
            return Err(bad(CMD, "duplicate clip in moves"));
        }
        let time = m.get("time").and_then(Value::as_i64).ok_or_else(|| bad(CMD, "every move needs an integer time"))?;
        let (source, _) = q.find_item(clip).ok_or(edit::EditError::NoItem(clip))?;
        let source_track = q.track(source).ok_or(edit::EditError::NoTrack(source))?;
        let track = if str_p(m, "track").is_some_and(|t| t.eq_ignore_ascii_case("new")) {
            if source_track.locked {
                return Err(edit::EditError::Locked.into());
            }
            new_moves.push((clip, source));
            source
        } else {
            let dest = track_p(s, m, "track", CMD)?.ok_or_else(|| bad(CMD, "every move needs a destination track"))?;
            if q.track(dest).is_none_or(|t| t.kind != source_track.kind) {
                return Err(bad(CMD, "destination track must match the source video/audio kind"));
            }
            dest
        };
        listed.push((clip, track, Tick(time)));
    }
    let follow = bool_p(p, "linked").unwrap_or(s.state.linked_selection);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // (clip, destination track, new start, where a partner starts now). `time` is any i64: the
    // sums are done in i128, where a few i64 values cannot overflow, and checked on the way back.
    let mut plan: Vec<(ClipId, TrackId, i128, Option<i128>)> = Vec::new();
    for &(clip, track, time) in &listed {
        plan.push((clip, track, i128::from(time.0), None));
        let Some((_, it)) = q.find_item(clip) else { continue }; // `edit::move_items` reports it
        let Some(link) = it.link.filter(|_| follow) else { continue };
        let offset = i128::from(time.0) - i128::from(it.start.0);
        for t in q.all_tracks().filter(|t| !t.locked) {
            for partner in t.items.iter().filter(|i| i.link == Some(link)) {
                if !listed.iter().any(|m| m.0 == partner.id) && !plan.iter().any(|m| m.0 == partner.id) {
                    if plan.len() >= MAX_MOVES {
                        return Err(bad(CMD, "linked movement exceeds 4096 resolved clips"));
                    }
                    plan.push((partner.id, t.id, i128::from(partner.start.0) + offset, Some(i128::from(partner.start.0))));
                }
            }
        }
    }
    let late = plan.iter().map(|m| -m.2).max().unwrap_or(0).max(0);
    let mut moves: Vec<(ClipId, TrackId, Tick)> = Vec::with_capacity(plan.len());
    for (clip, track, start, was) in plan {
        // a partner that ends up where it is does not move (its listed clip only changed track):
        // re-placing it would drop the transitions at its edges
        if was == Some(start + late) {
            continue;
        }
        let start = i64::try_from(start + late).ok().map(Tick).filter(|t| *t <= Tick::MAX).ok_or_else(|| bad(CMD, "`time` is out of range"))?;
        moves.push((clip, track, start));
    }

    let copy = bool_p(p, "copy").unwrap_or(false);
    let insert = bool_p(p, "insert").unwrap_or(false);
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let mut destinations = Vec::with_capacity(moves.len());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    for (clip, track, start) in &moves {
        let (_, item) = q.find_item(*clip).ok_or(edit::EditError::NoItem(*clip))?;
        let end = start.0.checked_add(item.duration.0).filter(|n| *n <= Tick::MAX.0).ok_or_else(|| bad(CMD, "destination end exceeds supported time"))?;
        if item.duration <= Tick::ZERO {
            return Err(bad(CMD, "source clip needs positive duration"));
        }
        let new_source = new_moves.iter().find(|m| m.0 == *clip).map(|m| m.1);
        destinations.push(((new_source.is_some(), new_source.unwrap_or(*track)), *start, Tick(end)));
    }
    destinations.sort_by_key(|r| (r.0, r.1));
    if destinations.windows(2).any(|w| w[0].0 == w[1].0 && w[0].2 > w[1].1) {
        return Err(bad(CMD, "requested destination clips overlap each other"));
    }
    if !copy
        && new_moves.is_empty()
        && moves.iter().all(|(clip, track, start)| q.find_item(*clip).is_some_and(|(old, item)| old == *track && item.start == *start))
    {
        return Err(bad(CMD, "movement makes no change"));
    }
    let inventory = q
        .all_tracks()
        .map(|t| t.items.len().saturating_add(t.transitions.len()))
        .chain(q.caption_tracks.iter().map(|t| t.captions.len()))
        .fold(0usize, usize::saturating_add);
    if inventory > 100000 {
        return Err(bad(CMD, "movement inventory exceeds 100000 objects"));
    }
    // Each placement has at most two boundary splits (clip + link); an insert
    // splits each track/cue at most once, and copies allocate clip/link/transition
    // IDs. This upper bound covers all of them before any EditCtx allocation.
    let budget = inventory
        .checked_add(moves.len())
        .and_then(|n| n.checked_add(new_moves.len()))
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_add(64))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| bad(CMD, "movement identity budget overflow"))?;
    crate::project_ids::allocation_start(&s.project, budget)?;
    let linked_basis = if p.get("linked").is_none() { Some(s.state.linked_selection) } else { None };
    Ok(MovementPlan { basis: s.project.clone(), sequence, moves, new_moves, insert, copy, linked_basis, budget })
}
impl MovementPlan {
    pub(crate) fn execute(self, s: &mut Session) -> Result<Value> {
        if !Arc::ptr_eq(&self.basis, &s.project) || s.state.active_sequence != Some(self.sequence) {
            return Err(bad(CMD, "movement plan document changed"));
        }
        if self.linked_basis.is_some_and(|value| value != s.state.linked_selection) {
            return Err(bad(CMD, "movement plan linked-selection state changed"));
        }
        let moves = &self.moves;
        let new_moves = &self.new_moves;
        let (copy, ins) = (self.copy, self.insert);
        // every clip on the sequence now: id -> (track, start, end, project item)
        let spots = |s: &Session| -> Vec<(u64, TrackId, Tick, Tick, ItemId)> {
            let q = s.active_sequence();
            q.into_iter().flat_map(|q| q.all_tracks()).flat_map(|t| t.items.iter().map(move |i| (i.id.0, t.id, i.start, i.end(), i.item))).collect()
        };
        let before = spots(s);
        // Allocate destinations in the same transaction as the move: an invalid edit leaves
        // neither empty tracks nor history entries. Each source track keeps its clips together.
        let prepare = |q: &mut filmcraft_project::Sequence, ctx: &mut edit::EditCtx<'_>| -> Result<Vec<(ClipId, TrackId, Tick)>> {
            let sources: Vec<_> = q.all_tracks().filter(|t| new_moves.iter().any(|m| m.1 == t.id)).map(|t| (t.id, t.kind, t.channels)).collect();
            for kind in [TrackKind::Video, TrackKind::Audio] {
                let added = sources.iter().filter(|s| s.1 == kind).count();
                if added > 0 && q.tracks(kind).len().saturating_add(added) > crate::sequence_tools::MAX_TRACKS {
                    return Err(bad(CMD, "new tracks exceed the sequence track limit"));
                }
            }
            let mut destinations = Vec::new();
            for (source, kind, channels) in sources {
                let tracks = if kind == TrackKind::Video { &mut q.video_tracks } else { &mut q.audio_tracks };
                let prefix = if kind == TrackKind::Video { "Video" } else { "Audio" };
                let id = TrackId(ctx.alloc());
                let mut track = Track::new(id, kind, format!("{prefix} {}", tracks.len() + 1));
                track.channels = channels;
                tracks.push(track);
                destinations.push((source, id));
            }
            Ok(moves
                .iter()
                .map(|&(clip, track, time)| {
                    let dest = new_moves.iter().find(|m| m.0 == clip).and_then(|m| destinations.iter().find(|d| d.0 == m.1)).map_or(track, |d| d.1);
                    (clip, dest, time)
                })
                .collect())
        };
        let label = if copy {
            if ins { "Copy (Insert)" } else { "Copy" }
        } else if ins {
            "Move (Insert)"
        } else {
            "Move"
        };
        let sequence = self.sequence;
        let pool = s.media.clone();
        let copied = s.edit(label, |project, st| {
            let reserved = crate::project_ids::allocation_start(project, self.budget)?;
            let old_next = project.next_id;
            let snapshot = project.clone();
            let durations = |id| crate::media_duration(&snapshot, &pool, id);
            let starts = |id| crate::media_start(&snapshot, id);
            let min = snapshot.sequence(sequence).ok_or(EngineError::NoSequence)?.settings.frame_rate.frame_duration();
            let mut next = reserved;
            let q = project.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
            let mut ctx = edit::EditCtx { next_id: &mut next, media_duration: &durations, media_start: &starts, min_duration: min };
            let destinations = prepare(q, &mut ctx)?;
            let copied = if copy {
                let ids = edit::copy_items(q, &destinations, ins, &mut ctx)?;
                st.selection = ids.clone();
                st.transition_selection.clear();
                ids
            } else {
                edit::move_items(q, &destinations, ins, &mut ctx)?;
                Vec::new()
            };
            q.check().map_err(EngineError::Other)?;
            repair_clip_state(q, st);
            if next.checked_sub(reserved).is_none_or(|used| used > self.budget) {
                return Err(bad(CMD, "movement exceeded its bounded identity reservation"));
            }
            project.next_id = if next == reserved { old_next } else { next };
            Ok(copied)
        })?;
        // An overwrite shortens, splits or removes whatever already sits where a clip lands, and a
        // shortened clip may live on under a new id: name each clip the move changed without being
        // asked to, with what is left of it, so the caller hears about it instead of finding out later.
        let after = spots(s);
        let moved = |c: u64| moves.iter().any(|m| m.0.0 == c) && !copy;
        let new = |c: u64| !before.iter().any(|b| b.0 == c) && !copied.iter().any(|k| k.0 == c);
        let mut overwritten = Vec::new();
        for &(c, track, start, end, item) in before.iter().filter(|b| !moved(b.0)) {
            let kept = after.iter().find(|a| a.0 == c);
            if ins || kept.is_some_and(|k| (k.2, k.3) == (start, end)) {
                continue; // untouched (an insert only shifts clips, it never covers one)
            }
            let pieces: Vec<_> =
                after.iter().filter(|a| a.0 == c || (new(a.0) && !moved(a.0) && a.1 == track && a.4 == item && a.2 >= start && a.3 <= end)).collect();
            let now: i64 = pieces.iter().map(|a| (a.3 - a.2).0).sum();
            overwritten.push(json!({"clip": c, "was": (end - start).0, "now": now, "pieces": pieces.iter().map(|a| a.0).collect::<Vec<_>>()}));
        }
        if copy {
            return Ok(json!({"copied": copied.iter().map(|c| c.0).collect::<Vec<_>>(), "overwritten": overwritten}));
        }
        Ok(json!({"moved": moves.iter().map(|m| m.0.0).collect::<Vec<_>>(), "overwritten": overwritten}))
    }
}

/// Repair references removed by overwrite/copy through the same human service.
pub(crate) fn repair_clip_state(q: &Sequence, st: &mut crate::EditorState) {
    st.selection.retain(|id| q.find_item(*id).is_some());
    st.transition_selection.retain(|id| q.all_tracks().any(|t| t.transitions.iter().any(|x| x.id == *id)));
    if st.selected_effect.as_ref().is_some_and(|e| q.find_item(e.clip).is_none_or(|(_, c)| e.index(&c.effects).is_none())) {
        st.selected_effect = None;
    }
    if st.selected_mask.is_some_and(|m| q.find_item(m.clip).and_then(|(_, c)| c.effects.get(m.effect)).is_none_or(|e| m.mask >= e.masks.len())) {
        st.selected_mask = None;
    }
    if st.gap_selection.is_some_and(|g| q.track(g.track).is_none_or(|t| !filmcraft_edit::through::track_gaps(t).contains(&g.range))) {
        st.gap_selection = None;
    }
}
