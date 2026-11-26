//! Caption collateral proof for the shared native media Ripple Delete service.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{CaptionTrack, ClipId, ItemId};
use filmcraft_time::{Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{captions, sequences};
use crate::{Result, Session, commands::bad};

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Basis {
    version: u8,
    before_sha256: String,
    scope: sequences::Scope,
}
impl Basis {
    pub(super) fn valid(&self) -> bool {
        self.version == 1 && self.before_sha256.len() == 64 && self.before_sha256.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }
    pub(super) fn scope(&self) -> sequences::Scope {
        self.scope.clone()
    }
}
pub(super) fn current_basis(s: &Session, scope: &sequences::Scope) -> Result<Option<Basis>> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.caption_tracks.is_empty() {
        return Ok(None);
    }
    Ok(Some(Basis { version: 1, before_sha256: captions::digest(&q.caption_tracks)?, scope: scope.clone() }))
}
pub(super) struct Witness {
    basis: Basis,
    sequence: ItemId,
    before: Vec<CaptionTrack>,
    next_id: u64,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.rippleDelete", message)
}
pub(super) fn prepare(s: &Session, basis: Option<Basis>, project: &str) -> Result<Option<Witness>> {
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if basis.is_none() && q.caption_tracks.is_empty() {
        return Ok(None);
    }
    let basis = basis.ok_or_else(|| error("native caption ripple requires an exact caption basis and supported timing policy"))?;
    if !basis.valid() || basis.scope.project_id != project || !sequences::belongs(&s.project, sequence, &basis.scope)? {
        return Err(error("caption ripple basis belongs to a different policy or workspace"));
    }
    if captions::digest(&q.caption_tracks)? != basis.before_sha256 {
        return Err(error("caption state changed before ripple; read and plan again"));
    }
    Ok(Some(Witness { basis, sequence, before: q.caption_tracks.clone(), next_id: s.project.next_id }))
}

pub(super) struct TrimWitness {
    captions: Witness,
    edge: filmcraft_edit::Edge,
    cut: Tick,
    delta: Tick,
    before_sequence: filmcraft_project::Sequence,
}
pub(super) fn prepare_trim(s: &Session, op: &super::Operation) -> Result<Option<TrimWitness>> {
    let clip = ClipId(op.params.get("clip").and_then(Value::as_u64).ok_or_else(|| error("missing caption trim target"))?);
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.caption_tracks.is_empty() && op.params.get("captionBasis").is_none() {
        return Ok(None);
    }
    let target = q.find_item(clip).ok_or_else(|| error("missing caption trim instance"))?.1;
    let basis = op.params.get("captionBasis").map(|v| serde_json::from_value(v.clone()).map_err(|e| error(e.to_string()))).transpose()?;
    let project = super::assembly::binding(s, target.item)?.project_id;
    let Some(captions) = prepare(s, basis, &project)? else { return Ok(None) };
    let edge = if op.params.get("edge").and_then(Value::as_str) == Some("in") { filmcraft_edit::Edge::In } else { filmcraft_edit::Edge::Out };
    let cut = if edge == filmcraft_edit::Edge::In {
        crate::commands::with_links(s, &[clip])
            .iter()
            .filter_map(|id| q.find_item(*id).map(|(_, item)| item.start))
            .max()
            .ok_or_else(|| error("missing linked caption cut"))?
    } else {
        target.end()
    };
    let frames = op.params.get("deltaFrames").and_then(Value::as_i64).ok_or_else(|| error("missing exact caption trim frames"))?;
    let delta = s
        .sequence_rate()
        .frame_duration()
        .0
        .checked_mul(frames)
        .filter(|n| *n != 0 && n.unsigned_abs() <= 9_007_199_254_740_991)
        .map(Tick)
        .ok_or_else(|| error("caption trim exceeds exact supported time"))?;
    Ok(Some(TrimWitness { captions, edge, cut, delta, before_sequence: q.clone() }))
}
pub(super) fn finish_trim(s: &Session, witness: Option<TrimWitness>) -> Result<Option<Value>> {
    let Some(t) = witness else { return Ok(None) };
    let w = t.captions;
    if s.state.active_sequence != Some(w.sequence) {
        return Err(error("caption trim changed its owning sequence"));
    }
    let mut expected = w.before.clone();
    let mut next = w.next_id;
    let mut ctx = filmcraft_edit::EditCtx {
        next_id: &mut next,
        media_duration: &|_| None,
        media_start: &|_| Tick::ZERO,
        min_duration: s.sequence_rate().frame_duration(),
    };
    filmcraft_edit::captions::ripple_trim(&mut expected, t.edge, t.cut, t.delta, &mut ctx)?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    super::assembly::validate_collateral(s, &t.before_sequence, &w.basis.scope.project_id)?;
    if q.caption_tracks != expected || s.project.next_id != next {
        return Err(error("caption trim changed collateral metadata, native identities or exact timing"));
    }
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let after: BTreeMap<_, _> = q.caption_tracks.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let removed = before.keys().filter(|id| !after.contains_key(id)).count();
    let created = after.keys().filter(|id| !before.contains_key(id)).count();
    let retimed = after.values().filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration)).count();
    let extension = (t.edge == filmcraft_edit::Edge::Out) == (t.delta > Tick::ZERO);
    let magnitude = t.delta.0.checked_abs().ok_or_else(|| error("caption trim delta overflow"))?;
    let start = if !extension && t.edge == filmcraft_edit::Edge::Out { t.cut.0.checked_sub(magnitude) } else { Some(t.cut.0) }
        .filter(|n| *n >= 0)
        .ok_or_else(|| error("caption trim precedes the sequence"))?;
    if start.checked_add(magnitude).is_none_or(|n| n > 9_007_199_254_740_991) {
        return Err(error("caption trim range exceeds exact supported time"));
    }
    let span = json!({"startTicks":start,"durationTicks":magnitude});
    Ok(Some(json!({"version":1,"policy":"ripple_trim_v1","scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),
        "beforeCues":before.len(),"afterCues":after.len(),"removedCues":removed,"retimedCues":retimed,"createdCues":created,
        "edge":if t.edge==filmcraft_edit::Edge::In {"in"} else {"out"},"cutTicks":t.cut.0,"deltaTicks":t.delta.0,
        "closedSpans":if extension {vec![]} else {vec![span.clone()]},"openedSpans":if extension {vec![span]} else {vec![]}})))
}
pub(super) fn finish(s: &Session, witness: Option<Witness>, spans: &[TimeRange]) -> Result<Option<Value>> {
    let Some(w) = witness else { return Ok(None) };
    if s.state.active_sequence != Some(w.sequence) || spans.is_empty() || spans.len() > 64 {
        return Err(error("caption ripple requires the same sequence and bounded complete closed spans"));
    }
    let mut expected = w.before.clone();
    for span in spans.iter().rev() {
        if span.start.0 < 0 || span.duration.0 <= 0 || span.start.0.checked_add(span.duration.0).is_none_or(|n| n > 9_007_199_254_740_991) {
            return Err(error("caption ripple time is outside exact supported bounds"));
        }
        for track in expected.iter_mut().filter(|track| !track.locked && track.sync_lock) {
            filmcraft_edit::captions::extract_range(track, *span);
        }
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if expected != q.caption_tracks {
        return Err(error("caption ripple changed collateral metadata or did not match the shared exact timing policy"));
    }
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| t.captions.iter()).map(|c| (c.id, c)).collect();
    let after_count = q.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>();
    let removed = before.len().checked_sub(after_count).ok_or_else(|| error("caption ripple unexpectedly created cues"))?;
    let retimed = q
        .caption_tracks
        .iter()
        .flat_map(|t| t.captions.iter())
        .filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration))
        .count();
    Ok(Some(json!({"version":1,"policy":"extract_closed_spans_v1","scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),
        "beforeCues":before.len(),"afterCues":after_count,"removedCues":removed,"retimedCues":retimed,
        "closedSpans":spans.iter().map(|r|json!({"startTicks":r.start.0,"durationTicks":r.duration.0})).collect::<Vec<_>>() })))
}

pub(super) fn finish_insert(s: &Session, witness: Option<Witness>, at: Tick, duration: Tick) -> Result<Option<Value>> {
    let Some(w) = witness else { return Ok(None) };
    if s.state.active_sequence != Some(w.sequence) || at.0 < 0 || duration.0 <= 0 || at.0.checked_add(duration.0).is_none_or(|n| n > 9_007_199_254_740_991) {
        return Err(error("caption insertion requires its exact owning sequence and opened time"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let mut expected = w.before.clone();
    let created = expected_insert(s, &w, &mut expected, at, duration)?;
    if q.caption_tracks != expected {
        return Err(error("caption insertion differs from shared native timing or changed collateral metadata"));
    }
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    let after_count = q.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>();
    let retimed = q
        .caption_tracks
        .iter()
        .flat_map(|t| &t.captions)
        .filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration))
        .count();
    Ok(Some(json!({"version":1,"policy":"insert_span_v1","scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),
        "beforeCues":before.len(),"afterCues":after_count,"removedCues":0,"retimedCues":retimed,"createdCues":created,
        "closedSpans":[],"openedSpans":[{"startTicks":at.0,"durationTicks":duration.0}]})))
}

fn expected_insert(s: &Session, w: &Witness, expected: &mut [CaptionTrack], at: Tick, duration: Tick) -> Result<usize> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let before: BTreeSet<_> = w.before.iter().flat_map(|t| &t.captions).map(|c| c.id).collect();
    let mut created = BTreeSet::new();
    for track in expected.iter_mut().filter(|t| !t.locked && t.sync_lock) {
        let actual = q.caption_tracks.iter().find(|t| t.id == track.id).ok_or_else(|| error("caption insertion removed a track"))?;
        let new_ids: Vec<_> = actual.captions.iter().filter(|c| !before.contains(&c.id)).map(|c| c.id).collect();
        let split = track.caption_at(at).is_some_and(|c| c.start < at);
        if new_ids.len() != usize::from(split) {
            return Err(error("caption insertion created unexpected cue identities"));
        }
        // Placement/media splitting may allocate before captions. Validate the
        // actual fresh native identity instead of guessing that allocation order.
        let mut next = if let Some(id) = new_ids.first() {
            if id.0 < w.next_id || id.0 >= s.project.next_id || !created.insert(*id) {
                return Err(error("caption insertion reused or detached a native cue identity"));
            }
            id.0
        } else {
            w.next_id
        };
        let mut ctx = filmcraft_edit::EditCtx {
            next_id: &mut next,
            media_duration: &|_| None,
            media_start: &|_| Tick::ZERO,
            min_duration: s.sequence_rate().frame_duration(),
        };
        filmcraft_edit::captions::insert_gap(track, at, duration, &mut ctx);
    }
    Ok(created.len())
}

pub(super) fn finish_move(
    s: &Session,
    witness: Option<Witness>,
    mode: &str,
    start: Tick,
    duration: Tick,
    requested: Tick,
    destination: Tick,
) -> Result<Option<Value>> {
    let Some(w) = witness else { return Ok(None) };
    if s.state.active_sequence != Some(w.sequence) || start.0 < 0 || duration.0 <= 0 || destination.0 < 0 {
        return Err(error("caption move requires its exact owning sequence and observed range"));
    }
    let mut expected = w.before.clone();
    let created = match mode {
        "gap" => 0,
        "reorder" => {
            for track in expected.iter_mut().filter(|t| !t.locked && t.sync_lock) {
                filmcraft_edit::captions::extract_range(track, TimeRange::new(start, duration));
            }
            expected_insert(s, &w, &mut expected, destination, duration)?
        }
        _ => return Err(error("unknown caption move policy")),
    };
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if expected != q.caption_tracks {
        return Err(error("caption move changed collateral metadata or differed from shared sequence timing"));
    }
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let after: BTreeMap<_, _> = q.caption_tracks.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let removed = before.keys().filter(|id| !after.contains_key(id)).count();
    let retimed = after.values().filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration)).count();
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    Ok(Some(json!({"version":1,"policy":"sequence_media_move_v1","scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),
        "beforeCues":before.len(),"afterCues":after.len(),"removedCues":removed,"retimedCues":retimed,"createdCues":created,
        "mode":mode,"sourceStartTicks":start.0,"durationTicks":duration.0,"requestedDestinationTicks":requested.0,"destinationTicks":destination.0,
        "closedSpans":if mode=="gap" {vec![]} else {vec![json!({"startTicks":start.0,"durationTicks":duration.0})]},
        "openedSpans":if mode=="gap" {vec![]} else {vec![json!({"startTicks":destination.0,"durationTicks":duration.0})]}})))
}

/// Validate the exact caption changes from the shared human speed service.
/// Group coordinates are sequential, not a union in the original sequence.
pub(super) fn finish_retime(s: &Session, witness: Option<Witness>, steps: &[filmcraft_edit::SpeedRipple]) -> Result<Option<Value>> {
    let Some(w) = witness else { return Ok(None) };
    if s.state.active_sequence != Some(w.sequence) || steps.len() > 128 {
        return Err(error("caption retiming needs its owning sequence and bounded native group timing"));
    }
    let mut expected = w.before.clone();
    let mut next = w.next_id;
    let mut closed = Vec::new();
    let mut opened = Vec::new();
    let mut closed_total = 0i64;
    let mut opened_total = 0i64;
    for step in steps {
        let magnitude = step
            .delta
            .0
            .checked_abs()
            .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
            .ok_or_else(|| error("caption retiming has an invalid native ripple delta"))?;
        if !(0..=9_007_199_254_740_991).contains(&step.cut.0) {
            return Err(error("caption retiming cut exceeds exact supported time"));
        }
        let start = if step.delta < Tick::ZERO { step.cut.0.checked_sub(magnitude) } else { Some(step.cut.0) }
            .filter(|n| *n >= 0)
            .ok_or_else(|| error("caption retiming precedes the sequence"))?;
        if start.checked_add(magnitude).is_none_or(|n| n > 9_007_199_254_740_991) {
            return Err(error("caption retiming span exceeds exact supported time"));
        }
        let (spans, total) = if step.delta < Tick::ZERO { (&mut closed, &mut closed_total) } else { (&mut opened, &mut opened_total) };
        *total =
            total.checked_add(magnitude).filter(|n| *n <= 9_007_199_254_740_991).ok_or_else(|| error("caption retiming total exceeds exact supported time"))?;
        spans.push(json!({"startTicks":start,"durationTicks":magnitude}));
        let mut ctx = filmcraft_edit::EditCtx {
            next_id: &mut next,
            media_duration: &|_| None,
            media_start: &|_| Tick::ZERO,
            min_duration: s.sequence_rate().frame_duration(),
        };
        filmcraft_edit::captions::ripple_trim(&mut expected, filmcraft_edit::Edge::Out, step.cut, step.delta, &mut ctx)?;
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.caption_tracks != expected || s.project.next_id != next {
        return Err(error("caption retiming changed collateral metadata, identities or shared timing"));
    }
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let after: BTreeMap<_, _> = q.caption_tracks.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let removed = before.keys().filter(|id| !after.contains_key(id)).count();
    let created = after.keys().filter(|id| !before.contains_key(id)).count();
    let retimed = after.values().filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration)).count();
    Ok(Some(json!({"version":1,"policy":"ripple_retime_v1","scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),
        "beforeCues":before.len(),"afterCues":after.len(),"removedCues":removed,"retimedCues":retimed,"createdCues":created,
        "steps":steps.iter().map(|step|json!({"cutTicks":step.cut.0,"deltaTicks":step.delta.0})).collect::<Vec<_>>(),
        "closedSpans":closed,"openedSpans":opened})))
}

pub(super) fn finish_condense(s: &Session, witness: Option<Witness>, operation: &str) -> Result<Option<Value>> {
    let Some(w) = witness else { return Ok(None) };
    if s.state.active_sequence != Some(w.sequence) || !matches!(operation, "condense" | "restore" | "choice") {
        return Err(error("caption condense changed its owning sequence or operation"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let headers = |tracks: &[CaptionTrack]| {
        tracks
            .iter()
            .cloned()
            .map(|mut t| {
                t.captions.clear();
                t
            })
            .collect::<Vec<_>>()
    };
    if headers(&q.caption_tracks) != headers(&w.before) {
        return Err(error("caption condense changed track metadata or locks"));
    }
    let before: BTreeMap<_, _> = w.before.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    let after: BTreeMap<_, _> = q.caption_tracks.iter().flat_map(|t| &t.captions).map(|c| (c.id, c)).collect();
    for cue in after.values() {
        if let Some(old) = before.get(&cue.id) {
            let mut expected = (*old).clone();
            expected.start = cue.start;
            expected.duration = cue.duration;
            if &expected != *cue {
                return Err(error("caption condense changed surviving authored words or metadata"));
            }
        }
    }
    let removed = before.keys().filter(|id| !after.contains_key(id)).count();
    let created = after.keys().filter(|id| !before.contains_key(id)).count();
    let retimed = after.values().filter(|c| before.get(&c.id).is_some_and(|old| old.start != c.start || old.duration != c.duration)).count();
    let after_sha256 = captions::digest(&q.caption_tracks)?;
    Ok(Some(json!({"version":1,"policy":"condense_caption_timing_v1","operation":operation,"scope":w.basis.scope,"sequence":w.sequence.0,
        "beforeSha256":w.basis.before_sha256,"afterSha256":after_sha256,"tracks":q.caption_tracks.len(),"beforeCues":before.len(),"afterCues":after.len(),
        "removedCues":removed,"retimedCues":retimed,"createdCues":created})))
}
