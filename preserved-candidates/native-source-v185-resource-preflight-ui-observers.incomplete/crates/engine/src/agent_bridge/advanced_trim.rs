//! Exact current-instance gestures over the shared native trim services.
use super::{Operation, assembly, captions, parse, project_audit, sequences, tracks};
use crate::{
    EngineError, Result, Session,
    commands::bad,
    trim_ops::{self, Targets},
};
use filmcraft_project::{ClipId, Sequence, TrackId, TrackItem};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub(super) const COMMAND: &str = "agent.advancedTrim";
const SAFE: u64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(v: u64) -> bool {
    v > 0 && v <= SAFE
}
fn hash(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    left: u64,
    right: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Slide {
    clip: u64,
    previous: Option<u64>,
    next: Option<u64>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Edit {
    Roll { pairs: Vec<Pair> },
    Slip { clips: Vec<u64> },
    Slide { clips: Vec<Slide> },
}
impl Edit {
    fn kind(&self) -> &'static str {
        match self {
            Self::Roll { .. } => "roll",
            Self::Slip { .. } => "slip",
            Self::Slide { .. } => "slide",
        }
    }
    fn selected(&self) -> Vec<u64> {
        match self {
            Self::Roll { pairs } => pairs.iter().flat_map(|p| [p.left, p.right]).collect(),
            Self::Slip { clips } => clips.clone(),
            Self::Slide { clips } => clips.iter().map(|p| p.clip).collect(),
        }
    }
    fn context(&self) -> BTreeSet<u64> {
        match self {
            Self::Slide { clips } => clips.iter().flat_map(|p| [Some(p.clip), p.previous, p.next]).flatten().collect(),
            _ => self.selected().into_iter().collect(),
        }
    }
    fn targets(&self) -> Targets {
        match self {
            Self::Roll { pairs } => Targets::Roll(pairs.iter().map(|p| (ClipId(p.left), ClipId(p.right))).collect()),
            Self::Slip { clips } => Targets::Slip(clips.iter().copied().map(ClipId).collect()),
            Self::Slide { clips } => Targets::Slide(clips.iter().map(|p| ClipId(p.clip)).collect()),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_sha256: String,
    before_caption_sha256: String,
    delta_ticks: i64,
    linked_policy: String,
    caption_policy: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    let selected = p.edit.selected();
    let context = p.edit.context();
    if matches!(p.edit, Edit::Slide { .. })
        && op
            .params
            .get("edit")
            .and_then(|v| v.get("clips"))
            .and_then(Value::as_array)
            .is_none_or(|rows| rows.iter().any(|r| r.as_object().is_none_or(|o| !o.contains_key("previous") || !o.contains_key("next"))))
    {
        return Err(error("slide requires explicit previous and next, including null edges"));
    }
    if !hash(&p.before_sha256)
        || !hash(&p.before_caption_sha256)
        || !id(p.scope.anchor_sequence)
        || p.scope.project_id.is_empty()
        || p.scope.project_id.len() > 256
        || p.scope.project_id.chars().any(char::is_control)
        || p.delta_ticks == 0
        || p.delta_ticks.unsigned_abs() > SAFE
        || !matches!(p.linked_policy.as_str(), "preserve" | "explicit")
        || p.caption_policy != "retain_sequence_timing"
        || selected.is_empty()
        || selected.len() > 128
        || selected.iter().any(|x| !id(*x))
        || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
        || context.len() > 192
        || context.iter().any(|x| !id(*x))
    {
        return Err(error("need bounded distinct current targets, exact delta, link/caption policy and complete state hashes"));
    }
    match p.edit {
        Edit::Slide { clips } if clips.len() > 64 => Err(error("at most 64 explicit slide centers")),
        _ => Ok(()),
    }
}
fn ids(q: &Sequence) -> Result<BTreeMap<u64, (TrackId, &TrackItem)>> {
    let mut all = BTreeMap::new();
    for t in q.all_tracks() {
        for c in &t.items {
            if all.insert(c.id.0, (t.id, c)).is_some() {
                return Err(error("ambiguous native clip identity"));
            }
        }
    }
    Ok(all)
}
fn clock(c: &TrackItem) -> Value {
    json!({"startTicks":c.start.0,"durationTicks":c.duration.0,"sourceInTicks":c.source_in.0})
}
fn safe(c: &TrackItem) -> bool {
    id(c.id.0)
        && c.start.0 >= 0
        && c.duration.0 > 0
        && c.source_in.0 >= 0
        && [c.start.0, c.duration.0, c.source_in.0, c.end().0].iter().all(|n| *n >= 0 && *n as u64 <= SAFE)
        && c.speed.is_finite()
        && c.speed > 0.0
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)? {
        return Err(error("advanced trim is outside this workspace"));
    }
    let before = s.project.clone();
    project_audit::bytes(&before)?;
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let old_state = s.state.clone();
    if tracks::digest(old)? != p.before_sha256 || captions::digest(&old.caption_tracks)? != p.before_caption_sha256 {
        return Err(error("complete trim state changed since its read"));
    }
    let all = ids(old)?;
    let selected = p.edit.selected().into_iter().collect::<BTreeSet<_>>();
    let context = p.edit.context();
    for cid in &context {
        let (track, c) = all.get(cid).ok_or_else(|| error("missing observed current instance or neighbor"))?;
        if !id(track.0) || !safe(c) || c.identity.is_none() {
            return Err(error("trim context needs tracked exact current clocks"));
        }
        if selected.contains(cid) {
            if old.track(*track).is_none_or(|t| t.locked) {
                return Err(error("requested trim target is locked"));
            }
            if assembly::binding(s, c.item)?.project_id != p.scope.project_id {
                return Err(error("trim targets must be bound original instances"));
            }
            if matches!(p.edit, Edit::Slip { .. }) && c.frame_hold.is_some() {
                return Err(error("slipping a held source does not change its held frame; use the native hold control"));
            }
        }
    }
    if let Edit::Slide { clips } = &p.edit {
        for row in clips {
            let (track, _) = all.get(&row.clip).ok_or_else(|| error("missing slide center"))?;
            let tr = old.track(*track).ok_or_else(|| error("missing slide track"))?;
            let index = tr.items.iter().position(|c| c.id.0 == row.clip).ok_or_else(|| error("missing slide center"))?;
            let previous = index.checked_sub(1).and_then(|i| tr.items.get(i)).map(|c| c.id.0);
            let next = index.checked_add(1).and_then(|i| tr.items.get(i)).map(|c| c.id.0);
            if row.previous != previous || row.next != next {
                return Err(error("slide neighbors differ from the explicit current read"));
            }
        }
    }
    trim_ops::prepare_explicit(s, p.edit.targets(), Tick(p.delta_ticks))?.execute_exact(s)?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let now = ids(after)?;
    if now.keys().collect::<Vec<_>>() != all.keys().collect::<Vec<_>>() {
        return Err(error("trim created or removed clip identities"));
    }
    let mut changed = BTreeSet::new();
    for (cid, (track, c)) in &all {
        let (dest, n) = now.get(cid).ok_or_else(|| error("trim lost an instance"))?;
        if project_audit::value_bytes(c)? != project_audit::value_bytes(n)? || dest != track {
            if !context.contains(cid)
                || old.track(*track).is_none_or(|t| t.locked)
                || c.identity.is_none()
                || assembly::binding(s, c.item)?.project_id != p.scope.project_id
                || !safe(n)
            {
                return Err(error("trim changed unread locked untracked or foreign collateral"));
            }
            changed.insert(*cid);
        }
    }
    if changed.is_empty() {
        return Err(error("exact trim made no change"));
    }
    if p.linked_policy == "preserve" {
        for cid in &changed {
            let c = all.get(cid).ok_or_else(|| error("missing linked origin"))?.1;
            let n = now.get(cid).ok_or_else(|| error("missing linked result"))?.1;
            if let Some(link) = c.link {
                let delta = |a: &TrackItem, b: &TrackItem| {
                    if matches!(p.edit, Edit::Slip { .. }) {
                        (0, 0, b.source_in.0 - a.source_in.0)
                    } else {
                        (b.start.0 - a.start.0, b.duration.0 - a.duration.0, 0)
                    }
                };
                let wanted = delta(c, n);
                for (peer, (_, other)) in all.iter().filter(|(_, (_, x))| x.link == Some(link)) {
                    let actual = now.get(peer).ok_or_else(|| error("lost linked peer"))?.1;
                    if !context.contains(peer) || !safe(other) || !safe(actual) || delta(other, actual) != wanted {
                        return Err(error(
                            "preserve requires every changed linked boundary or source clock to follow; use explicit only for intended independent picture/sound",
                        ));
                    }
                }
            }
        }
    }
    // Reconstruct only permitted clock changes; no effect, source, routing,
    // identity, caption, master, foreign-sequence or resource authoring is allowed.
    let mut expected = (*before).clone();
    expected.items.get_mut(&sequence).ok_or(EngineError::NoSequence)?.metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
    let q = expected.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    for cid in &changed {
        let (_, actual) = now.get(cid).ok_or_else(|| error("missing changed result"))?;
        let (_, c) = q.find_item_mut(ClipId(*cid)).ok_or_else(|| error("missing expected instance"))?;
        c.start = actual.start;
        c.duration = actual.duration;
        c.source_in = actual.source_in;
    }
    let mut transition_changes = 0usize;
    for t in q.all_tracks_mut() {
        let actual = after.track(t.id).ok_or_else(|| error("trim changed tracks"))?;
        if t.transitions.len() != actual.transitions.len() {
            return Err(error("trim created or removed transitions"));
        }
        for (old_tr, new_tr) in t.transitions.iter_mut().zip(&actual.transitions) {
            if old_tr.id != new_tr.id {
                return Err(error("trim changed transition identity/order"));
            }
            if old_tr.start != new_tr.start {
                if ![old_tr.from, old_tr.to].into_iter().flatten().any(|c| changed.contains(&c.0)) {
                    return Err(error("trim shifted an unrelated transition"));
                }
                old_tr.start = new_tr.start;
                transition_changes += 1;
            }
        }
    }
    if project_audit::bytes(&expected)? != project_audit::bytes(&s.project)? {
        return Err(error("trim changed unrelated authored state or current payload"));
    }
    let mut expected_state = old_state;
    crate::move_ops::repair_clip_state(after, &mut expected_state);
    if project_audit::value_bytes(&expected_state)? != project_audit::value_bytes(&s.state)? {
        return Err(error("trim changed unrelated editor state"));
    }
    let after_caption = captions::digest(&after.caption_tracks)?;
    if after_caption != p.before_caption_sha256 {
        return Err(error("trim changed sequence caption timing or words"));
    }
    let mut rows = Vec::new();
    for cid in &context {
        let (track, old) = all.get(cid).ok_or_else(|| error("missing observed origin"))?;
        let (new_track, c) = now.get(cid).ok_or_else(|| error("missing observed result"))?;
        let identity = c.identity.as_ref().ok_or_else(|| error("trim lost persistent identity"))?;
        if track != new_track {
            return Err(error("trim changed a clip track"));
        }
        rows.push(json!({"clip":cid,"track":track.0,"identity":identity.id,"parentIdentity":identity.parent,"before":clock(old),"after":clock(c)}));
    }
    Ok(
        json!({"beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":after_caption,"kind":p.edit.kind(),"deltaTicks":p.delta_ticks,"linkedPolicy":p.linked_policy,"captionPolicy":p.caption_policy,"clips":rows,"collateral":{"changedClips":changed.len(),"changedNeighbors":changed.iter().filter(|id|!selected.contains(id)).count(),"changedTransitions":transition_changes,"changedCues":0},"humanCommand":format!("timeline.{}",p.edit.kind())}),
    )
}
