//! Shared native track identity, settings, exact removal and ordering services.
//! These are human commands, not an unguarded model executor.
use std::collections::BTreeSet;

use crate::{EditorState, EngineError, Result, Session, commands::bad};
use filmcraft_project::{AudioChannels, ItemId, Label, Sequence, Track, TrackId};
use serde_json::{Value, json};

pub(crate) fn fields<'a>(p: &'a Value, allowed: &[&str], cmd: &str) -> Result<&'a serde_json::Map<String, Value>> {
    p.as_object().filter(|o| o.keys().all(|k| allowed.contains(&k.as_str()))).ok_or_else(|| bad(cmd, "explicit known track fields required"))
}
pub(crate) fn text(value: &str, cmd: &str) -> Result<()> {
    if value.len() > 1024 || value.contains('\0') {
        return Err(bad(cmd, "track name exceeds 1024 UTF-8 bytes or contains NUL"));
    }
    Ok(())
}
pub(crate) fn channels(p: &Value, key: &str, default: AudioChannels, cmd: &str) -> Result<AudioChannels> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::String(s)) => crate::mixer::channels_from(s).ok_or_else(|| bad(cmd, format!("unknown {key} channel format"))),
        Some(_) => Err(bad(cmd, format!("{key} must name a channel format"))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Video,
    Audio,
    Submix,
    Caption,
}
pub(crate) fn locate(q: &Sequence, id: TrackId, cmd: &str) -> Result<(Kind, usize)> {
    let count = q.video_tracks.len().saturating_add(q.audio_tracks.len()).saturating_add(q.submix_tracks.len()).saturating_add(q.caption_tracks.len());
    if count > 4096 {
        return Err(bad(cmd, "active track inventory exceeds 4096 entries"));
    }
    if id.0 == 0 {
        return Err(bad(cmd, "a positive track identity is required"));
    }
    let mut found = Vec::new();
    for (kind, rows) in [(Kind::Video, &q.video_tracks), (Kind::Audio, &q.audio_tracks), (Kind::Submix, &q.submix_tracks)] {
        found.extend(rows.iter().enumerate().filter(|(_, t)| t.id == id).map(|(i, _)| (kind, i)));
    }
    found.extend(q.caption_tracks.iter().enumerate().filter(|(_, t)| t.id == id).map(|(i, _)| (Kind::Caption, i)));
    match found.as_slice() {
        [one] => Ok(*one),
        _ => Err(bad(cmd, "track identity is missing or duplicated in the active sequence")),
    }
}
pub(crate) fn resolve(q: &Sequence, v: &Value, cmd: &str) -> Result<TrackId> {
    let id = if let Some(id) = v.as_u64() {
        TrackId(id)
    } else {
        let name = v.as_str().ok_or_else(|| bad(cmd, "track must be an ID or V/A/S/C reference"))?;
        let mut chars = name.chars();
        let prefix = chars.next().ok_or_else(|| bad(cmd, "empty track reference"))?;
        let index = chars.as_str().parse::<usize>().ok().and_then(|n| n.checked_sub(1)).ok_or_else(|| bad(cmd, "invalid track reference"))?;
        let id = match prefix.to_ascii_uppercase() {
            'V' => q.video_tracks.get(index).map(|t| t.id),
            'A' => q.audio_tracks.get(index).map(|t| t.id),
            'S' => q.submix_tracks.get(index).map(|t| t.id),
            'C' => q.caption_tracks.get(index).map(|t| t.id),
            _ => None,
        };
        id.ok_or_else(|| bad(cmd, "track reference is outside the active sequence"))?
    };
    locate(q, id, cmd)?;
    Ok(id)
}

/// One exact deletion policy shared by the old singular/bulk and submix commands.
/// Linked clips on other tracks remain; deleting a submix reroutes its inputs to Mix
/// and removes its sends with their exact native automation-lane reindexing.
pub(crate) fn remove(s: &mut Session, ids: &[TrackId], label: &str, cmd: &str) -> Result<()> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if ids.is_empty() || ids.len() > 4096 {
        return Err(bad(cmd, "explicit nonempty bounded track identities required"));
    }
    let mut doomed = BTreeSet::new();
    for id in ids {
        locate(q, *id, cmd)?;
        if !doomed.insert(*id) {
            return Err(bad(cmd, "duplicate track removal"));
        }
    }
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit_sequence(label, move |q, _, st| {
        remove_inventory(q, st, sequence, &doomed);
        Ok(())
    })
}
/// Pure domain mutation shared by the human transaction and guarded exact plan.
pub(crate) fn remove_inventory(q: &mut Sequence, st: &mut EditorState, sequence: ItemId, doomed: &BTreeSet<TrackId>) {
    let removed_clips: BTreeSet<_> = q.all_tracks().filter(|t| doomed.contains(&t.id)).flat_map(|t| t.items.iter().map(|c| c.id)).collect();
    let removed_transitions: BTreeSet<_> = q.all_tracks().filter(|t| doomed.contains(&t.id)).flat_map(|t| t.transitions.iter().map(|c| c.id)).collect();
    q.video_tracks.retain(|t| !doomed.contains(&t.id));
    q.audio_tracks.retain(|t| !doomed.contains(&t.id));
    q.submix_tracks.retain(|t| !doomed.contains(&t.id));
    q.caption_tracks.retain(|t| !doomed.contains(&t.id));
    for strip in q.strip_ids() {
        q.with_strip_mut(strip, |t| {
            if t.mixer.output.is_some_and(|id| doomed.contains(&id)) {
                t.mixer.output = None;
            }
            while let Some(index) = t.mixer.sends.iter().position(|send| doomed.contains(&send.target)) {
                t.remove_send(index);
            }
        });
    }
    st.transition_selection.retain(|id| !removed_transitions.contains(id));
    if st.gap_selection.is_some_and(|gap| doomed.contains(&gap.track)) {
        st.gap_selection = None;
    }
    repair(st, q, sequence, doomed, &removed_clips);
}
fn repair(st: &mut EditorState, q: &Sequence, sequence: ItemId, doomed: &BTreeSet<TrackId>, removed_clips: &BTreeSet<filmcraft_project::ClipId>) {
    st.selection.retain(|c| q.find_item(*c).is_some());
    st.caption_selection.retain(|c| q.find_caption(*c).is_some());
    if st.selected_effect.as_ref().is_some_and(|e| removed_clips.contains(&e.clip)) {
        st.selected_effect = None;
    }
    if st.selected_mask.is_some_and(|m| removed_clips.contains(&m.clip)) {
        st.selected_mask = None;
    }
    if let Some(targets) = st.targeting.get_mut(&sequence) {
        targets.targeted.retain(|id| !doomed.contains(id));
        if targets.video_dest.is_some_and(|id| doomed.contains(&id)) {
            targets.video_dest = q.video_tracks.first().map(|t| t.id);
        }
        if targets.audio_dest.is_some_and(|id| doomed.contains(&id)) {
            targets.audio_dest = q.audio_tracks.first().map(|t| t.id);
        }
    }
}
pub(crate) fn remove_command(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.removeTracks";
    fields(p, &["tracks"], CMD)?;
    let rows = p
        .get("tracks")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 4096)
        .ok_or_else(|| bad(CMD, "explicit nonempty track list required"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let ids = rows.iter().map(|v| resolve(q, v, CMD)).collect::<Result<Vec<_>>>()?;
    remove(s, &ids, "Remove Tracks", CMD)?;
    Ok(json!({"deleted":ids.len()}))
}

fn reorder_rows<T>(rows: &mut Vec<T>, from: usize, after: Option<usize>, cmd: &str) -> Result<()> {
    let destination = after.map_or(0, |index| if index < from { index + 1 } else { index });
    if from == destination {
        return Err(bad(cmd, "track is already at that position"));
    }
    if from >= rows.len() || destination >= rows.len() {
        return Err(bad(cmd, "track order is out of range"));
    }
    let track = rows.remove(from);
    rows.insert(destination, track);
    Ok(())
}
/// Reorder by persistent ID; `after:null` means first in the same collection.
/// Track payloads, clip clocks, links, captions and mixer routes retain identity.
pub(crate) fn reorder(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.reorderTrack";
    let object = fields(p, &["track", "after"], CMD)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let id = resolve(q, object.get("track").ok_or_else(|| bad(CMD, "explicit track required"))?, CMD)?;
    let after = match object.get("after").ok_or_else(|| bad(CMD, "explicit after identity or null required"))? {
        Value::Null => None,
        v => Some(resolve(q, v, CMD)?),
    };
    s.edit_sequence("Reorder Track", move |q, _, _| reorder_inventory(q, id, after, CMD))?;
    Ok(json!({"track":id.0}))
}
pub(crate) fn reorder_inventory(q: &mut Sequence, id: TrackId, after: Option<TrackId>, cmd: &str) -> Result<()> {
    let (kind, index) = locate(q, id, cmd)?;
    let after = after
        .map(|anchor| {
            let (other, position) = locate(q, anchor, cmd)?;
            if anchor == id || other != kind {
                return Err(bad(cmd, "ordering requires a different track of the same kind"));
            }
            Ok(position)
        })
        .transpose()?;
    if kind == Kind::Submix {
        let mut order: Vec<_> = q.submix_tracks.iter().map(|t| t.id).collect();
        reorder_rows(&mut order, index, after, cmd)?;
        for (i, identity) in order.iter().enumerate() {
            let track = q.submix_tracks.iter().find(|t| t.id == *identity).ok_or_else(|| bad(cmd, "missing submix identity"))?;
            for target in track.mixer.output.into_iter().chain(track.mixer.sends.iter().map(|send| send.target)) {
                if order.iter().position(|id| *id == target).is_none_or(|target| target <= i) {
                    return Err(bad(cmd, "submix order would invalidate an existing output/send route"));
                }
            }
        }
    }
    let (rows, prefix) = match kind {
        Kind::Video => (&mut q.video_tracks, "Video"),
        Kind::Audio => (&mut q.audio_tracks, "Audio"),
        Kind::Submix => (&mut q.submix_tracks, "Submix"),
        Kind::Caption => {
            reorder_rows(&mut q.caption_tracks, index, after, cmd)?;
            return Ok(());
        }
    };
    reorder_rows(rows, index, after, cmd)?;
    crate::sequence_tools::renumber(rows, prefix);
    Ok(())
}

/// Strict Track Settings parsing before the native transaction. The human pan
/// clamp is retained; malformed types never silently act as omitted fields.
pub(crate) fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "timeline.setTrack";
    let object = fields(p, &["track", "locked", "syncLock", "enabled", "muted", "solo", "name", "volumeDb", "pan", "label"], CMD)?;
    let id = crate::commands::track_p(s, p, "track", CMD)?.ok_or_else(|| bad(CMD, "explicit track required"))?;
    locate(s.active_sequence().ok_or(EngineError::NoSequence)?, id, CMD)?;
    if object.len() < 2 {
        return Err(bad(CMD, "explicit track setting required"));
    }
    for key in ["locked", "syncLock", "enabled", "muted", "solo"] {
        if p.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(bad(CMD, format!("{key} must be boolean")));
        }
    }
    if let Some(value) = p.get("name") {
        text(value.as_str().ok_or_else(|| bad(CMD, "name must be text"))?, CMD)?;
    }
    for key in ["volumeDb", "pan"] {
        if let Some(value) = p.get(key) {
            let n = value.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad(CMD, format!("{key} must be finite")))?;
            if key == "volumeDb" && !(filmcraft_project::mixer::FADER_MIN_DB..=filmcraft_project::mixer::FADER_MAX_DB).contains(&n) {
                return Err(bad(CMD, "track fader exceeds its native range"));
            }
        }
    }
    let label = match p.get("label") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(v)) if v.is_empty() || v.eq_ignore_ascii_case("none") => Some(None),
        Some(Value::String(v)) => Some(Some(Label::from_name(v).ok_or_else(|| bad(CMD, "unknown track label"))?)),
        Some(_) => return Err(bad(CMD, "label must be a native color or null")),
    };
    let p = p.clone();
    s.edit_sequence("Track Settings", move |q, _, _| {
        let t = q.track_mut(id).ok_or(filmcraft_edit::EditError::NoTrack(id))?;
        for (key, slot) in
            [("locked", &mut t.locked), ("syncLock", &mut t.sync_lock), ("enabled", &mut t.enabled), ("muted", &mut t.muted), ("solo", &mut t.solo)]
        {
            if let Some(v) = p.get(key).and_then(Value::as_bool) {
                *slot = v;
            }
        }
        if let Some(v) = p.get("name").and_then(Value::as_str) {
            t.name = v.into();
        }
        if let Some(v) = p.get("volumeDb").and_then(Value::as_f64) {
            t.volume_db = v;
        }
        if let Some(v) = p.get("pan").and_then(Value::as_f64) {
            t.pan = v.clamp(-100.0, 100.0);
        }
        if let Some(v) = label {
            t.label = v;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(crate) fn add_submix(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mixer.addSubmix";
    fields(p, &["name", "channels"], CMD)?;
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = v.as_str().ok_or_else(|| bad(CMD, "name must be text"))?;
            text(n, CMD)?;
            Some(n.to_string())
        }
    };
    let channels = channels(p, "channels", AudioChannels::Stereo, CMD)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let count = s.active_sequence().ok_or(EngineError::NoSequence)?.submix_tracks.len();
    if count >= crate::sequence_tools::MAX_TRACKS {
        return Err(bad(CMD, "submix track limit reached"));
    }
    let start = crate::project_ids::allocation_start(&s.project, 1)?;
    let id = s.edit("Add Submix Track", move |pr, _| {
        pr.next_id = start;
        let id = TrackId(pr.alloc_id());
        let q = pr.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
        let mut track = Track::new(id, filmcraft_project::TrackKind::Audio, name.unwrap_or_else(|| format!("Submix {}", count + 1)));
        track.channels = channels;
        q.submix_tracks.push(track);
        q.check().map_err(EngineError::Other)?;
        Ok(id)
    })?;
    Ok(json!({"id":id.0,"ref":format!("S{}",count+1)}))
}
