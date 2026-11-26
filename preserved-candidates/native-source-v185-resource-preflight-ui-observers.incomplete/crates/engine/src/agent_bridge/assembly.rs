//! Audited in-memory assembly operations inside a guarded batch, not raw command dispatch.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_edit as edit;
use filmcraft_project::{ClipId, ItemId, TrackId, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{BINDING_KEY, Binding, Operation, parse};
use crate::commands::bad;
use crate::{Result, Session};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Insert {
    item: u64,
    video_track: Option<u64>,
    audio_track: Option<u64>,
    frame: i64,
    source_in_frame: i64,
    duration_frames: i64,
    audio_channels: Vec<u16>,
    #[serde(default)]
    caption_basis: Option<super::caption_ripple::Basis>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Move {
    clip: u64,
    frame: i64,
    mode: String,
    #[serde(default)]
    caption_basis: Option<super::caption_ripple::Basis>,
    #[serde(default)]
    source_start_ticks: Option<i64>,
    #[serde(default)]
    duration_ticks: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Delete {
    clip: u64,
    #[serde(default)]
    caption_basis: Option<super::caption_ripple::Basis>,
}

fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.apply", message)
}
pub(super) fn is_assembly(command: &str) -> bool {
    matches!(command, "agent.insert" | "agent.move" | "agent.rippleDelete")
}
fn id(id: u64) -> bool {
    (1..=9_007_199_254_740_991).contains(&id)
}
fn frame(frame: i64) -> bool {
    (0..=1_000_000).contains(&frame)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let valid = match op.command.as_str() {
        "agent.insert" => {
            let p: Insert = parse(&op.command, &op.params)?;
            id(p.item)
                && (p.video_track.is_some() || p.audio_track.is_some())
                && p.video_track.into_iter().chain(p.audio_track).all(id)
                && frame(p.frame)
                && frame(p.source_in_frame)
                && (1..=1_000_000).contains(&p.duration_frames)
                && p.audio_channels.len() <= 32
                && p.audio_channels.iter().copied().collect::<BTreeSet<_>>().len() == p.audio_channels.len()
                && (p.audio_track.is_some() != p.audio_channels.is_empty())
                && p.caption_basis.as_ref().is_none_or(super::caption_ripple::Basis::valid)
        }
        "agent.move" => {
            let p: Move = parse(&op.command, &op.params)?;
            id(p.clip)
                && frame(p.frame)
                && matches!(p.mode.as_str(), "gap" | "reorder")
                && p.caption_basis.as_ref().is_none_or(super::caption_ripple::Basis::valid)
                && if p.caption_basis.is_some() {
                    p.source_start_ticks.is_some_and(|n| (0..=9_007_199_254_740_991).contains(&n))
                        && p.duration_ticks.is_some_and(|n| (1..=9_007_199_254_740_991).contains(&n))
                } else {
                    p.source_start_ticks.is_none() && p.duration_ticks.is_none()
                }
        }
        "agent.rippleDelete" => {
            let p: Delete = parse(&op.command, &op.params)?;
            id(p.clip) && p.caption_basis.as_ref().is_none_or(super::caption_ripple::Basis::valid)
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(error("invalid explicit assembly operation/range/channels")) }
}
fn tick(s: &Session, frame: i64) -> Result<Tick> {
    let rate = s.sequence_rate();
    if rate.num <= 0 || rate.den <= 0 || rate.frame_duration() <= Tick::ZERO {
        return Err(error("assembly requires a positive exact sequence frame rate"));
    }
    rate.frame_duration()
        .0
        .checked_mul(frame)
        .filter(|t| (0..=9_007_199_254_740_991).contains(t))
        .map(Tick)
        .ok_or_else(|| error("assembly time exceeds the exact supported range"))
}
pub(super) fn binding(s: &Session, item: ItemId) -> Result<Binding> {
    let source = s.project.item(item).ok_or_else(|| error("missing assembly source"))?;
    if source.as_media().is_none() {
        return Err(error("assembly supports imported media only, not nests/subclips/graphics"));
    }
    let raw = source.metadata.get(BINDING_KEY).ok_or_else(|| error("assembly source is not explicitly bound to Poema"))?;
    let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    if b.project_id.is_empty() || b.asset_id.is_empty() || b.media_id.is_empty() || b.role != "original" {
        return Err(error("assembly requires an original-media binding"));
    }
    Ok(b)
}
fn members(s: &Session, clip: ClipId) -> Result<Vec<ClipId>> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (_, target) = q.find_item(clip).ok_or_else(|| error("missing explicit assembly clip"))?;
    // Assembly always keeps a link together, independent of the UI's Alt/Linked Selection state.
    let ids: Vec<_> = q.all_tracks().flat_map(|t| &t.items).filter(|c| c.id == clip || target.link.is_some() && c.link == target.link).map(|c| c.id).collect();
    if ids.is_empty() || ids.len() > 32 {
        return Err(error("assembly needs a bounded linked group (1–32 members)"));
    }
    for id in &ids {
        let (track, c) = q.find_item(*id).ok_or_else(|| error("missing linked assembly clip"))?;
        if c.identity.is_none() || q.track(track).is_none_or(|t| t.locked) {
            return Err(error("assembly cannot skip an untracked or locked linked partner"));
        }
    }
    Ok(ids)
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let before = s.project.clone();
    let project = if op.command == "agent.insert" {
        binding(s, ItemId(parse::<Insert>(&op.command, &op.params)?.item))?.project_id
    } else {
        let clip = ClipId(op.params.get("clip").and_then(Value::as_u64).ok_or_else(|| error("missing assembly clip"))?);
        let item = s.active_sequence().and_then(|q| q.find_item(clip)).ok_or_else(|| error("missing assembly clip"))?.1.item;
        binding(s, item)?.project_id
    };
    let result = match op.command.as_str() {
        "agent.insert" => insert(s, parse(&op.command, &op.params)?),
        "agent.move" => move_clip(s, parse(&op.command, &op.params)?),
        "agent.rippleDelete" => delete(s, parse(&op.command, &op.params)?),
        _ => Err(error("unsupported assembly operation")),
    }?;
    // Validate *collateral* ripple effects too, including clips outside a bounded model preview.
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let old = before.sequence(seq).ok_or(crate::EngineError::NoSequence)?;
    validate_collateral(s, old, &project)?;
    Ok(result)
}
pub(super) fn validate_collateral(s: &Session, old: &filmcraft_project::Sequence, project: &str) -> Result<()> {
    let new = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut links: BTreeMap<u64, Vec<Option<(i64, i64)>>> = BTreeMap::new();
    for tr in old.all_tracks() {
        for c in &tr.items {
            let after = new.find_item(c.id);
            if after.is_none_or(|(t, a)| t != tr.id || a != c) {
                if c.identity.is_none() || binding(s, c.item)?.project_id != project {
                    return Err(error("assembly ripple would change untracked/unbound/foreign material"));
                }
                if tr.locked {
                    return Err(error("assembly would change a locked track"));
                }
            }
            if let Some(link) = c.link {
                let delta = after.map(|(_, a)| (a.start.0.checked_sub(c.start.0), a.duration.0.checked_sub(c.duration.0)));
                let delta = match delta {
                    None => None,
                    Some((Some(a), Some(b))) => Some((a, b)),
                    _ => return Err(error("linked assembly delta overflow")),
                };
                links.entry(link).or_default().push(delta);
            }
        }
    }
    if links.values().any(|v| v.first().is_some_and(|first| v.iter().any(|p| p != first))) {
        return Err(error("assembly would desynchronize a linked group (locked/sync-lock mismatch or split edit)"));
    }
    Ok(())
}

fn insert(s: &mut Session, p: Insert) -> Result<Value> {
    let item = ItemId(p.item);
    let media = s.project.item(item).and_then(|i| i.as_media()).ok_or_else(|| error("insert needs imported media"))?;
    let (at, sin, dur) = (tick(s, p.frame)?, tick(s, p.source_in_frame)?, tick(s, p.duration_frames)?);
    let end = sin.0.checked_add(dur.0).ok_or_else(|| error("source range overflow"))?;
    if media.offline || end > 9_007_199_254_740_991 || end > media.info.duration.0 || p.video_track.is_some() && !media.info.has_video() {
        return Err(error("source is offline, missing picture, or the requested range exceeds its duration"));
    }
    if p.audio_track.is_some() && media.info.audio().is_none_or(|a| p.audio_channels.iter().any(|c| u32::from(*c) >= a.channels)) {
        return Err(error("requested channels are not in the source's primary audio stream"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    for (track, kind) in [(p.video_track, TrackKind::Video), (p.audio_track, TrackKind::Audio)] {
        if track.is_some_and(|id| q.track(TrackId(id)).is_none_or(|t| t.kind != kind || t.locked)) {
            return Err(error("insert destination is missing, locked or the wrong kind"));
        }
    }
    let caption_witness = super::caption_ripple::prepare(s, p.caption_basis, &binding(s, item)?.project_id)?;
    at.0.checked_add(dur.0).filter(|v| *v <= 9_007_199_254_740_991).ok_or_else(|| error("insert end exceeds supported range"))?;
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let rate = s.sequence_rate();
    let pool = s.media.clone();
    let placed = s.edit("Agent Insert", |project, st| {
        let link = if p.video_track.is_some() && p.audio_track.is_some() { Some(project.alloc_id()) } else { None };
        let mut placements = Vec::new();
        for (track, kind) in [(p.video_track, TrackKind::Video), (p.audio_track, TrackKind::Audio)] {
            let Some(track) = track else { continue };
            let mut c = project.make_track_item(item, kind, at, TimeRange::new(sin, dur), rate).ok_or_else(|| error("cannot build source instance"))?;
            c.link = link;
            if kind == TrackKind::Audio {
                c.audio_stream = 0;
                c.source_channels = p.audio_channels.clone();
            }
            placements.push((TrackId(track), c));
        }
        let snapshot = project.clone();
        let durations = |id| crate::media_duration(&snapshot, &pool, id);
        let starts = |id| crate::media_start(&snapshot, id);
        let mut next = project.next_id;
        let q = project.sequence_mut(seq).ok_or(crate::EngineError::NoSequence)?;
        let mut ctx = edit::EditCtx { next_id: &mut next, media_duration: &durations, media_start: &starts, min_duration: rate.frame_duration() };
        let ids = edit::insert(q, placements, &mut ctx)?;
        if st.ripple_sequence_markers {
            crate::sequence_tools::ripple_markers(&mut q.markers, at, dur);
        }
        project.next_id = next;
        st.selection = ids.clone();
        Ok(ids)
    })?;
    if p.video_track.is_some() {
        let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
        let videos: Vec<_> = placed.iter().filter(|id| q.find_item(**id).is_some_and(|(t, _)| Some(t.0) == p.video_track)).map(|c| c.0).collect();
        s.execute("clip.fitToFrame", json!({"clips": videos}))?;
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if placed.len() != usize::from(p.video_track.is_some()) + usize::from(p.audio_track.is_some())
        || placed.iter().any(|id| q.find_item(*id).is_none_or(|(_, c)| c.start != at || c.duration != dur || c.source_in != sin || c.speed != 1.0))
    {
        return Err(error("insert did not match the exact requested range"));
    }
    let mut result = json!({"inserted": placed.iter().map(|c| c.0).collect::<Vec<_>>()});
    if let Some(proof) = super::caption_ripple::finish_insert(s, caption_witness, at, dur)? {
        result["captionRipple"] = proof;
    }
    Ok(result)
}

fn move_clip(s: &mut Session, p: Move) -> Result<Value> {
    let ids = members(s, ClipId(p.clip))?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (_, target) = q.find_item(ClipId(p.clip)).ok_or_else(|| error("missing move target"))?;
    let (at, start, dur) = (tick(s, p.frame)?, target.start, target.duration);
    if at == start {
        return Err(error("move is a no-op"));
    }
    if p.caption_basis.is_some() && (p.source_start_ticks != Some(start.0) || p.duration_ticks != Some(dur.0)) {
        return Err(error("caption move source span differs from the observed native instance"));
    }
    let mut placements = Vec::new();
    let mut tracks = BTreeSet::new();
    for id in &ids {
        let (track, c) = q.find_item(*id).ok_or_else(|| error("missing linked move target"))?;
        if q.track(track).is_some_and(|t| t.transitions.iter().any(|x| x.from == Some(*id) || x.to == Some(*id))) {
            return Err(error("moving clips with attached transitions is not supported yet"));
        }
        let mut c = c.clone();
        let offset = c.start.0.checked_sub(start.0).ok_or_else(|| error("move offset overflow"))?;
        c.start = Tick(at.0.checked_add(offset).filter(|v| *v >= 0).ok_or_else(|| error("move would clamp a linked partner before zero"))?);
        if p.mode == "reorder" && (offset != 0 || c.duration != dur || !tracks.insert(track.0)) {
            return Err(error("reorder requires aligned linked members, one per track"));
        }
        let end = c.start.0.checked_add(c.duration.0).ok_or_else(|| error("move range overflow"))?;
        if p.mode == "gap"
            && q.track(track).is_some_and(|t| t.items.iter().any(|other| !ids.contains(&other.id) && other.start.0 < end && other.end() > c.start))
        {
            return Err(error("destination is occupied; gap moves never overwrite other material"));
        }
        placements.push((track, c));
    }
    if p.mode == "reorder" && (at > q.duration() || at >= start && at <= start + dur) {
        return Err(error("reorder destination must be outside the moved clip and within the original timeline"));
    }
    let destination = if p.mode == "reorder" && at > start { at - dur } else { at };
    destination.0.checked_add(dur.0).filter(|n| *n <= 9_007_199_254_740_991).ok_or_else(|| error("move exceeds exact supported time"))?;
    let project = binding(s, target.item)?.project_id;
    let caption_witness = super::caption_ripple::prepare(s, p.caption_basis, &project)?;
    s.edit_sequence("Agent Move", |q, ctx, st| {
        if p.mode == "reorder" {
            let spans = edit::ripple_delete_items(q, &ids)?;
            if spans != vec![TimeRange::new(start, dur)] {
                return Err(error("reorder could not close exactly the source range"));
            }
            if st.ripple_sequence_markers {
                crate::sequence_tools::ripple_markers(&mut q.markers, start + dur, -dur);
            }
            for (_, c) in &mut placements {
                c.start = destination;
            }
            edit::insert(q, placements.clone(), ctx)?;
            if st.ripple_sequence_markers {
                crate::sequence_tools::ripple_markers(&mut q.markers, destination, dur);
            }
        } else {
            let moves: Vec<_> = placements.iter().map(|(t, c)| (c.id, *t, c.start)).collect();
            edit::move_items(q, &moves, false, ctx)?;
        }
        st.selection = ids.clone();
        Ok(())
    })?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if placements.iter().any(|(t, c)| {
        q.find_item(c.id).is_none_or(|(track, after)| track != *t || after.start != c.start || after.duration != c.duration || after.identity != c.identity)
    }) {
        return Err(error("move did not preserve the requested positions/durations/identities"));
    }
    let mut result = json!({"moved": ids.iter().map(|c| c.0).collect::<Vec<_>>(), "frame": s.sequence_rate().frame_at(destination)});
    if let Some(proof) = super::caption_ripple::finish_move(s, caption_witness, &p.mode, start, dur, at, destination)? {
        result["captionRipple"] = proof;
    }
    Ok(result)
}

fn delete(s: &mut Session, p: Delete) -> Result<Value> {
    let ids = members(s, ClipId(p.clip))?;
    let item = s.active_sequence().and_then(|q| q.find_item(ClipId(p.clip))).ok_or_else(|| error("missing ripple target"))?.1.item;
    let witness = super::caption_ripple::prepare(s, p.caption_basis, &binding(s, item)?.project_id)?;
    let mut closed = Vec::new();
    s.edit_sequence("Agent Ripple Delete", |q, _, st| {
        let spans = edit::ripple_delete_items(q, &ids)?;
        if st.ripple_sequence_markers {
            for span in spans.iter().rev() {
                crate::sequence_tools::ripple_markers(&mut q.markers, span.end(), -span.duration);
            }
        }
        closed = spans;
        st.selection.retain(|c| !ids.contains(c));
        Ok(())
    })?;
    if s.active_sequence().is_none_or(|q| ids.iter().any(|id| q.find_item(*id).is_some())) {
        return Err(error("ripple delete did not remove every linked member"));
    }
    let mut result = json!({"removed": ids.iter().map(|c| c.0).collect::<Vec<_>>()});
    if let Some(proof) = super::caption_ripple::finish(s, witness, &closed)? {
        result["captionRipple"] = proof;
    }
    Ok(result)
}
