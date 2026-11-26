//! One original picture pass through the human track/clear/place services.
//! Called only on agent.apply's private in-memory staging session. No owner
//! networking, file writes, editorial decisions or separate live publications.
use super::{Operation, assembly, captions, parse, project_audit, sequences, source_edit, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, ItemId, TrackId, TrackItem};
use filmcraft_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.placePicturePass";
const SAFE: i64 = 9_007_199_254_740_991;
const LIMIT: usize = 10_000;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn sha(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Window {
    index: usize,
    item: u64,
    asset_id: String,
    media_id: String,
    start_ticks: i64,
    source_in_ticks: i64,
    duration_ticks: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    clip: u64,
    identity: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_state_sha256: String,
    before_sha256: String,
    before_caption_sha256: String,
    placement_defaults_sha256: String,
    plan_sha256: String,
    placement_sha256: String,
    target_track: Option<u64>,
    picture_only: bool,
    ripple: bool,
    caption_policy: String,
    windows: Vec<Window>,
    replacements: Vec<Replacement>,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if project_audit::value_bytes(&op.params)?.len() > 4 * 1024 * 1024 - 8192
        || !id(p.scope.anchor_sequence)
        || p.scope.project_id.is_empty()
        || p.scope.project_id.len() > 256
        || p.scope.project_id.chars().any(char::is_control)
        || [&p.before_state_sha256, &p.before_sha256, &p.before_caption_sha256, &p.placement_defaults_sha256, &p.plan_sha256, &p.placement_sha256]
            .iter()
            .any(|s| !hash(s))
        || !p.picture_only
        || p.ripple
        || p.caption_policy != "preserve-authored"
        || p.windows.is_empty()
        || p.windows.len() > LIMIT
        || p.replacements.len() > LIMIT
        || p.target_track.is_some_and(|n| !id(n))
        || p.target_track.is_none() && !p.replacements.is_empty()
        || p.windows.iter().enumerate().any(|(i, w)| {
            w.index != i
                || !id(w.item)
                || [&w.asset_id, &w.media_id].iter().any(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
                || w.start_ticks < 0
                || w.source_in_ticks < 0
                || w.duration_ticks <= 0
                || w.start_ticks.checked_add(w.duration_ticks).is_none_or(|n| n > SAFE)
                || w.source_in_ticks.checked_add(w.duration_ticks).is_none_or(|n| n > SAFE)
        })
        || p.replacements.iter().any(|r| !id(r.clip) || !filmcraft_project::clip_identity::ClipIdentity::valid_id(&r.identity) || !hash(&r.sha256))
        || p.replacements.iter().map(|r| r.clip).collect::<BTreeSet<_>>().len() != p.replacements.len()
    {
        return Err(error("need one complete bounded picture-only pass with current state, original plan and exact distinct targets"));
    }
    Ok(())
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if source_edit::digest(s)? != p.before_state_sha256
        || tracks::digest(q)? != p.before_sha256
        || captions::digest(&q.caption_tracks)? != p.before_caption_sha256
        || source_edit::placement_defaults(s)? != p.placement_defaults_sha256
    {
        return Err(error("picture destination, captions or native placement defaults changed"));
    }
    Ok(())
}
fn row(track: TrackId, c: &TrackItem) -> Result<Value> {
    if !id(track.0) || !id(c.id.0) || !id(c.item.0) {
        return Err(error("picture outcome exceeds exact transport identity bounds"));
    }
    let identity = c.identity.as_ref().ok_or_else(|| error("picture instance lost its persistent identity"))?;
    Ok(
        json!({"clip":c.id.0,"track":track.0,"identity":identity.id,"item":c.item.0,"startTicks":c.start.0,"durationTicks":c.duration.0,"sourceInTicks":c.source_in.0,"sha256":sha(c)?}),
    )
}
// Bound memory independently of the number of human service calls. These are
// PRIVATE staging accumulators; the live history/journal is never reachable.
fn discard_private_steps(s: &mut Session) {
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s.events.clear();
}
pub(super) fn run(s: &mut Session, op: &Operation, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
    checkpoint()?;
    validate(op)?;
    validate_live_basis(s, op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)? {
        return Err(error("picture pass is outside its adopted workspace"));
    }
    let before = s.project.clone();
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let old_state = s.state.clone();
    let rate = s.sequence_rate();
    let unit = rate.frame_duration().0;
    if unit <= 0 || s.revision.checked_add(p.windows.len() as u64 + 8).is_none() {
        return Err(error("invalid frame rate or exhausted private revision counter"));
    }
    let target = if let Some(id) = p.target_track {
        let t = old.video_tracks.iter().find(|t| t.id.0 == id).ok_or_else(|| error("missing picture destination"))?;
        if t.locked || old.video_tracks.first().is_some_and(|first| first.id == t.id) {
            return Err(error("picture pass requires an unlocked overlay destination"));
        }
        Some(t)
    } else {
        if old.video_tracks.len() != 1 {
            return Err(error("choose the existing overlay destination; no implicit extra layer"));
        }
        None
    };
    let removed: BTreeSet<_> = p.replacements.iter().map(|r| ClipId(r.clip)).collect();
    let mut removed_rows = Vec::with_capacity(p.replacements.len());
    for r in &p.replacements {
        checkpoint()?;
        let track = target.ok_or_else(|| error("replacement requires its current overlay"))?;
        let c = track.items.iter().find(|c| c.id.0 == r.clip).ok_or_else(|| error("replacement no longer identifies this overlay"))?;
        if c.identity.as_ref().is_none_or(|i| i.id != r.identity || i.protection.is_some()) || sha(c)? != r.sha256 {
            return Err(error("replacement identity, processing or human protection changed"));
        }
        removed_rows.push(row(track.id, c)?);
    }
    let mut intervals = Vec::with_capacity(p.windows.len());
    for w in &p.windows {
        checkpoint()?;
        let end = w.start_ticks.checked_add(w.duration_ticks).ok_or_else(|| error("picture end overflow"))?;
        let source_end = w.source_in_ticks.checked_add(w.duration_ticks).ok_or_else(|| error("source end overflow"))?;
        if w.duration_ticks % unit != 0 || end > old.duration().0 {
            return Err(error("original picture pass exceeds the current sequence or whole-frame duration"));
        }
        let binding = assembly::binding(s, ItemId(w.item))?;
        if binding.project_id != p.scope.project_id || binding.asset_id != w.asset_id || binding.media_id != w.media_id || binding.role != "original" {
            return Err(error("picture source differs from its bound original"));
        }
        let media = s.project.item(ItemId(w.item)).and_then(|i| i.as_media()).ok_or_else(|| error("picture pass requires direct media originals"))?;
        if media.offline
            || !s.project.source_streams(ItemId(w.item)).0
            || media.info.duration.0 > 0 && source_end.checked_sub(unit).is_none_or(|last| last >= media.info.duration.0)
        {
            return Err(error("picture source is unavailable or outside its last native frame"));
        }
        intervals.push((w.start_ticks, end));
    }
    intervals.sort_unstable();
    if intervals.windows(2).any(|w| w.first().zip(w.get(1)).is_none_or(|(a, b)| a.1 > b.0)) {
        return Err(error("original placed windows overlap after frame conversion"));
    }
    let mut occupied: Vec<_> = target.into_iter().flat_map(|t| &t.items).filter(|c| !removed.contains(&c.id)).map(|c| (c.start.0, c.end().0)).collect();
    occupied.sort_unstable();
    let mut at = 0;
    for (start, end) in &occupied {
        while intervals.get(at).is_some_and(|w| w.1 <= *start) {
            at += 1;
        }
        if intervals.get(at).is_some_and(|w| w.0 < *end && w.1 > *start) {
            return Err(error("original frame placement overlaps retained picture; no overwrite or partial pass"));
        }
    }
    let before_project_sha256 = sha(&*before)?;
    let before_audio_sha256 = sha(&old.audio_tracks)?;
    checkpoint()?;
    discard_private_steps(s);
    let destination = if let Some(t) = target {
        t.id
    } else {
        s.execute("sequence.addTracks", json!({"video":1,"audio":0,"submix":0}))?;
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        if q.video_tracks.len() != 2 {
            return Err(error("native track service did not create exactly one overlay"));
        }
        q.video_tracks.last().map(|t| t.id).ok_or_else(|| error("created picture track missing"))?
    };
    if !removed.is_empty() {
        discard_private_steps(s);
        s.state.linked_selection = false;
        s.execute("edit.clear", json!({"clips":p.replacements.iter().map(|r|r.clip).collect::<Vec<_>>()}))?;
        s.state.linked_selection = old_state.linked_selection;
    }
    let mut created = Vec::with_capacity(p.windows.len());
    for w in &p.windows {
        checkpoint()?;
        discard_private_steps(s);
        let end = w.source_in_ticks.checked_add(w.duration_ticks).ok_or_else(|| error("source end overflow"))?;
        let placed = crate::commands::place_item_with_marker_policy(
            s,
            ItemId(w.item),
            TimeRange::from_bounds(Tick(w.source_in_ticks), Tick(end)),
            Tick(w.start_ticks),
            Some(destination),
            None,
            false,
            "Original Poema Picture Pass",
            None,
            Some(false),
        )?;
        if placed.len() != 1 {
            return Err(error("native human placement did not create exactly one silent picture instance"));
        }
        let id = placed.first().copied().ok_or_else(|| error("native picture placement returned no instance"))?;
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let (track, c) = q.find_item(id).ok_or_else(|| error("placed picture disappeared"))?;
        if track != destination
            || c.start.0 != w.start_ticks
            || c.duration.0 != w.duration_ticks
            || c.source_in.0 != w.source_in_ticks
            || c.item.0 != w.item
            || c.link.is_some()
        {
            return Err(error("native human placement clamped, linked or moved an original decision"));
        }
        created.push(id);
    }
    discard_private_steps(s);
    checkpoint()?;
    s.edit("Retain Original Picture Plan", |project, state| {
        let q = project.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
        let track = q.video_tracks.iter_mut().find(|t| t.id == destination).ok_or_else(|| error("picture destination disappeared"))?;
        for (index, id) in created.iter().enumerate() {
            let clip = track.items.iter_mut().find(|c| c.id == *id).ok_or_else(|| error("picture pass lost an instance"))?;
            let identity = clip.identity.as_mut().ok_or_else(|| error("picture pass requires tracked native instances"))?;
            identity.annotations.insert("poema.picture.plan".into(), p.plan_sha256.clone());
            identity.annotations.insert("poema.picture.placement".into(), p.placement_sha256.clone());
            identity.annotations.insert("poema.picture.index".into(), index.to_string());
            identity.validate().map_err(error)?;
        }
        state.selection = created.clone();
        crate::move_ops::repair_clip_state(q, state);
        Ok(())
    })?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if project_audit::value_bytes(&old.audio_tracks)? != project_audit::value_bytes(&after.audio_tracks)?
        || project_audit::value_bytes(&old.caption_tracks)? != project_audit::value_bytes(&after.caption_tracks)?
    {
        return Err(error("picture pass changed authored sound or captions"));
    }
    let final_track = after.video_tracks.iter().find(|t| t.id == destination).ok_or_else(|| error("picture result track missing"))?;
    if target.is_some_and(|t| t.items.len().checked_sub(removed.len()).and_then(|n| n.checked_add(created.len())) != Some(final_track.items.len()))
        || target.is_none() && final_track.items.len() != created.len()
    {
        return Err(error("picture pass created undeclared collateral instances"));
    }
    for c in target.into_iter().flat_map(|t| &t.items).filter(|c| !removed.contains(&c.id)) {
        let now = final_track.items.iter().find(|v| v.id == c.id).ok_or_else(|| error("retained picture disappeared"))?;
        if project_audit::value_bytes(c)? != project_audit::value_bytes(now)? {
            return Err(error("picture pass changed retained picture"));
        }
    }
    let mut projection = (*before).clone();
    projection.next_id = s.project.next_id;
    let projected = projection.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    if let Some(id) = p.target_track {
        let track = projected.video_tracks.iter_mut().find(|t| t.id.0 == id).ok_or_else(|| error("original target missing"))?;
        track.items = final_track.items.clone();
        let expected: Vec<_> = track
            .transitions
            .iter()
            .filter(|t| !t.from.is_some_and(|c| removed.contains(&c)) && !t.to.is_some_and(|c| removed.contains(&c)))
            .cloned()
            .collect();
        if project_audit::value_bytes(&expected)? != project_audit::value_bytes(&final_track.transitions)? {
            return Err(error("picture pass changed an unrelated transition"));
        }
        track.transitions = expected;
    } else {
        projected.video_tracks.push(final_track.clone());
    }
    if project_audit::bytes(&projection)? != project_audit::bytes(&s.project)? {
        return Err(error("picture pass changed unrelated project, track, marker or source state"));
    }
    let mut expected_state = old_state;
    expected_state.selection = created.clone();
    crate::move_ops::repair_clip_state(after, &mut expected_state);
    if project_audit::value_bytes(&expected_state)? != project_audit::value_bytes(&s.state)? {
        return Err(error("picture pass changed unrelated editor context"));
    }
    let outcomes = created
        .iter()
        .map(|id| final_track.items.iter().find(|c| c.id == *id).ok_or_else(|| error("missing picture outcome")).and_then(|c| row(destination, c)))
        .collect::<Result<Vec<_>>>()?;
    let mut human_commands = Vec::new();
    if target.is_none() {
        human_commands.push("sequence.addTracks");
    }
    if !removed.is_empty() {
        human_commands.push("edit.clear");
    }
    human_commands.push("timeline.place");
    Ok(
        json!({"beforeProjectSha256":before_project_sha256,"afterProjectSha256":sha(&*s.project)?,"beforeStateSha256":p.before_state_sha256,"afterStateSha256":source_edit::digest(s)?,"beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":captions::digest(&after.caption_tracks)?,"beforeAudioSha256":before_audio_sha256,"afterAudioSha256":sha(&after.audio_tracks)?,"targetTrack":destination.0,"createdTrack":target.is_none(),"planSha256":p.plan_sha256,"placementSha256":p.placement_sha256,"pictureOnly":true,"ripple":false,"captionPolicy":"preserve-authored","removed":removed_rows,"outcomes":outcomes,"humanCommands":human_commands}),
    )
}
