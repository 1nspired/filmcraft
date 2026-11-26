//! One original reaction trace through the shared human Razor/Enable services.
//! Runs only on agent.apply's private stage; no editorial selection or I/O.
use super::{Operation, assembly, captions, markers, parse, project_audit, sequences, source_edit, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, MediaRef, TrackId, TrackItem, Transition};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const COMMAND: &str = "agent.revealReactionPass";
const SAFE: i64 = 9_007_199_254_740_991;
const LIMIT: usize = 4096;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha(v: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    slot: usize,
    clip: u64,
    identity: String,
    sha256: String,
    asset_id: String,
    media_id: String,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Split {
        target: usize,
        second: usize,
        #[serde(rename = "timeTicks")]
        time: i64,
    },
    Enable {
        target: usize,
        marker: u64,
        kind: String,
        #[serde(rename = "atTicks")]
        at: i64,
        strength: f64,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_project_sha256: String,
    before_state_sha256: String,
    before_sha256: String,
    before_caption_sha256: String,
    before_marker_sha256: String,
    plan_sha256: String,
    decisions_sha256: String,
    audio_policy: String,
    caption_policy: String,
    transition_policy: String,
    targets: Vec<Target>,
    steps: Vec<Step>,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if project_audit::value_bytes(&op.params)?.len() > 4 * 1024 * 1024 - 8192
        || !id(p.scope.anchor_sequence)
        || p.scope.project_id.is_empty()
        || p.scope.project_id.len() > 256
        || p.scope.project_id.chars().any(char::is_control)
        || [
            &p.before_project_sha256,
            &p.before_state_sha256,
            &p.before_sha256,
            &p.before_caption_sha256,
            &p.before_marker_sha256,
            &p.plan_sha256,
            &p.decisions_sha256,
        ]
        .iter()
        .any(|s| !hash(s))
        || p.audio_policy != "preserve-authored"
        || p.caption_policy != "preserve-authored"
        || !["preserve-authored", "native-razor"].contains(&p.transition_policy.as_str())
        || p.targets.is_empty()
        || p.targets.len() > LIMIT
        || p.steps.is_empty()
        || p.steps.len() > LIMIT * 2
        || p.targets.iter().enumerate().any(|(i, t)| {
            t.slot != i
                || !id(t.clip)
                || !filmcraft_project::clip_identity::ClipIdentity::valid_id(&t.identity)
                || !hash(&t.sha256)
                || [&t.asset_id, &t.media_id].iter().any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        })
        || p.targets.iter().map(|t| t.clip).collect::<BTreeSet<_>>().len() != p.targets.len()
        || p.targets.iter().map(|t| &t.identity).collect::<BTreeSet<_>>().len() != p.targets.len()
    {
        return Err(error("one complete bounded reaction trace, original plan and exact current targets required"));
    }
    let mut roots: Vec<usize> = (0..p.targets.len()).collect();
    let mut used = BTreeSet::new();
    for step in &p.steps {
        let target = match step {
            Step::Split { target, .. } | Step::Enable { target, .. } => *target,
        };
        let root = *roots.get(target).ok_or_else(|| error("reaction trace refers to a future or missing handle"))?;
        used.insert(root);
        match step {
            Step::Split { second, time, .. } => {
                if *second != roots.len() || roots.len() >= LIMIT || !(0..=SAFE).contains(time) {
                    return Err(error("reaction split needs one new bounded ordered handle and exact time"));
                }
                roots.push(root);
            }
            Step::Enable { marker, kind, at, strength, .. } => {
                if !id(*marker)
                    || !["laugh", "tears", "cheer", "applause"].contains(&kind.as_str())
                    || !(0..=SAFE).contains(at)
                    || !strength.is_finite()
                    || !(0.0..=1.0).contains(strength)
                {
                    return Err(error("reaction enable needs its exact authored cue and finite strength"));
                }
            }
        }
    }
    if used.len() != p.targets.len() {
        return Err(error("reaction trace contains unused initial targets"));
    }
    Ok(())
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if sha(&*s.project)? != p.before_project_sha256
        || source_edit::digest(s)? != p.before_state_sha256
        || tracks::digest(q)? != p.before_sha256
        || captions::digest(&q.caption_tracks)? != p.before_caption_sha256
        || markers::digest(&q.markers)? != p.before_marker_sha256
    {
        return Err(error("reaction project, source clocks, processing, tracks, captions or cues changed"));
    }
    Ok(())
}
fn safe(c: &TrackItem) -> bool {
    if !id(c.id.0)
        || !id(c.item.0)
        || !(0..=SAFE).contains(&c.start.0)
        || c.duration.0 <= 0
        || !(0..=SAFE).contains(&c.source_in.0)
        || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
        || !c.speed.is_finite()
        || c.speed <= 0.0
    {
        return false;
    }
    // The native source clock owns speed rounding and saturates hostile input.
    // Bounds checking here never reconstructs source time with float edit math.
    let out = c.source_out().0;
    out > c.source_in.0 && out <= SAFE
}
fn discard(s: &mut Session) {
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s.events.clear();
}
fn transitions(before: &[Transition], after: &[Transition], old: ClipId, new: ClipId, time: Tick, policy: &str, step: usize) -> Result<Vec<Value>> {
    let mut expected = Vec::with_capacity(before.len());
    let mut changes = Vec::new();
    for t in before {
        let removed = t.start < time && time < t.end() && (t.from == Some(old) || t.to == Some(old)) && !(t.from.is_some() && t.to.is_some());
        let mut next = t.clone();
        if next.start >= time && next.from == Some(old) {
            next.from = Some(new);
        }
        if removed && policy == "preserve-authored" {
            return Err(error("reaction razor would remove an authored transition; nothing published"));
        }
        if removed || next.from != t.from {
            if !id(t.id.0) {
                return Err(error("reaction transition exceeds exact output identity bounds"));
            }
            changes
                .push(json!({"step":step,"transition":t.id.0,"removed":removed,"beforeSha256":sha(t)?,"afterSha256":if removed{None}else{Some(sha(&next)?)}}));
        }
        if !removed {
            expected.push(next);
        }
    }
    if project_audit::value_bytes(&expected)? != project_audit::value_bytes(&after)? {
        return Err(error("reaction razor changed transitions outside its declared native policy"));
    }
    Ok(changes)
}
fn row(slot: usize, root: usize, track: TrackId, c: &TrackItem) -> Result<Value> {
    if !safe(c) || !id(track.0) {
        return Err(error("reaction output exceeds exact identity/clock transport"));
    }
    let i = c.identity.as_ref().ok_or_else(|| error("reaction output lost native tracking"))?;
    Ok(json!({"slot":slot,"originSlot":root,"clip":c.id.0,"identity":i.id,"parent":i.parent,"track":track.0,"item":c.item.0,
        "startTicks":c.start.0,"durationTicks":c.duration.0,"sourceInTicks":c.source_in.0,"sourceOutTicks":c.source_out().0,"enabled":c.enabled,"sha256":sha(c)?}))
}
pub(super) fn run(s: &mut Session, op: &Operation, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
    checkpoint()?;
    validate(op)?;
    validate_live_basis(s, op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)? {
        return Err(error("reaction trace is outside the adopted workspace"));
    }
    let before = s.project.clone();
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let state = s.state.clone();
    let unit = s.sequence_rate().frame_duration().0;
    if unit <= 0 || s.revision.checked_add((p.steps.len() as u64).saturating_mul(3).saturating_add(4)).is_none() {
        return Err(error("invalid reaction frame rate or exhausted private revision counter"));
    }
    let mut handles = Vec::with_capacity(LIMIT);
    let mut roots = Vec::with_capacity(LIMIT);
    let mut identities = Vec::with_capacity(LIMIT);
    let mut originals = BTreeMap::new();
    for t in &p.targets {
        checkpoint()?;
        let (track, c) = old.find_item(ClipId(t.clip)).ok_or_else(|| error("current reaction instance disappeared"))?;
        if !safe(c)
            || c.reverse
            || c.frame_hold.is_some()
            || c.link.is_some()
            || !old.video_tracks.iter().any(|v| v.id == track && !v.locked)
            || c.identity.as_ref().is_none_or(|i| i.id != t.identity || i.protection.is_some())
            || sha(c)? != t.sha256
        {
            return Err(error("reaction targets must retain exact tracked unlocked forward picture and processing, without linked sound"));
        }
        let binding = assembly::binding(s, c.item)?;
        let media = s.project.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("reaction source is not direct original media"))?;
        if binding.project_id != p.scope.project_id
            || binding.asset_id != t.asset_id
            || binding.media_id != t.media_id
            || binding.role != "original"
            || media.offline
            || !matches!(media.media, MediaRef::File { .. })
            || !media.info.has_video()
        {
            return Err(error("reaction original binding or availability changed"));
        }
        identities.push(c.identity.clone().ok_or_else(|| error("reaction tracking missing"))?);
        handles.push(c.id);
        roots.push(t.slot);
        originals.insert(t.slot, (track, c));
    }
    let mut razor_count = 0usize;
    let mut enable_count = 0usize;
    let mut annotation_count = 0usize;
    let mut transition_changes = Vec::new();
    for (index, step) in p.steps.iter().enumerate() {
        checkpoint()?;
        discard(s);
        let target = match step {
            Step::Split { target, .. } | Step::Enable { target, .. } => *target,
        };
        let clip = *handles.get(target).ok_or_else(|| error("missing staged reaction handle"))?;
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let (track, c) = q.find_item(clip).ok_or_else(|| error("staged reaction fragment disappeared"))?;
        match step {
            Step::Split { second, time, .. } => {
                let at = Tick(*time);
                let edge = q.video_tracks.iter().flat_map(|t| &t.items).any(|c| c.start == at || c.end() == at);
                if (!edge && time % unit != 0) || time.checked_sub(c.start.0).is_none_or(|n| n < unit) || c.end().0.checked_sub(*time).is_none_or(|n| n < unit)
                {
                    return Err(error("original reaction razor needs exact record points with at least one native frame on each side"));
                }
                let old_ids = q.video_tracks.iter().flat_map(|t| t.items.iter().map(|c| c.id)).collect::<BTreeSet<_>>();
                let old_transitions = q.track(track).ok_or_else(|| error("reaction track missing"))?.transitions.clone();
                s.state.linked_selection = false;
                let result = s.execute("timeline.razor", json!({"clip":clip.0,"time":time}))?;
                s.state.linked_selection = state.linked_selection;
                let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
                let created = after.video_tracks.iter().flat_map(|t| &t.items).filter(|c| !old_ids.contains(&c.id)).collect::<Vec<_>>();
                let right = created.first().copied().ok_or_else(|| error("native human razor created no right fragment"))?;
                if result.get("cuts").and_then(Value::as_u64) != Some(1) || created.len() != 1 || right.start != at || *second != handles.len() {
                    return Err(error("native human razor did not match the original split trace"));
                }
                transition_changes.extend(transitions(
                    &old_transitions,
                    &after.track(track).ok_or_else(|| error("reaction track missing"))?.transitions,
                    clip,
                    right.id,
                    at,
                    &p.transition_policy,
                    index,
                )?);
                if transition_changes.len() > LIMIT {
                    return Err(error("complete reaction transition collateral exceeds its receipt bound"));
                }
                let identity = right.identity.as_ref().ok_or_else(|| error("human razor lost reaction tracking"))?;
                let parent = identities.get(target).ok_or_else(|| error("reaction parent missing"))?;
                if identity.id == parent.id
                    || identity.parent.as_deref() != Some(parent.id.as_str())
                    || identity.annotations != parent.annotations
                    || identity.protection != parent.protection
                {
                    return Err(error("human razor lost reaction identity or documentary lineage"));
                }
                identities.push(identity.clone());
                handles.push(right.id);
                roots.push(*roots.get(target).ok_or_else(|| error("reaction lineage missing"))?);
                razor_count += 1;
            }
            Step::Enable { marker, kind, at, strength, .. } => {
                if !q.markers.iter().any(|m| m.id.0 == *marker && m.start.0 == *at) {
                    return Err(error("reaction cue moved or disappeared"));
                }
                let enable = !c.enabled;
                if enable {
                    s.state.linked_selection = false;
                    s.execute("clip.enable", json!({"clips":[clip.0]}))?;
                    s.state.linked_selection = state.linked_selection;
                    enable_count += 1;
                }
                discard(s);
                checkpoint()?;
                let i = identities.get_mut(target).ok_or_else(|| error("reaction annotation identity missing"))?;
                for (k, v) in [
                    ("poema.reaction.plan", p.plan_sha256.clone()),
                    ("poema.reaction.decisions", p.decisions_sha256.clone()),
                    ("poema.reaction.marker", marker.to_string()),
                    ("poema.reaction.kind", kind.clone()),
                    ("poema.reaction.at", at.to_string()),
                    ("poema.reaction.strength", strength.to_string()),
                ] {
                    i.annotations.insert(k.into(), v);
                }
                i.validate().map_err(error)?;
                let annotations = i.annotations.clone();
                s.execute("poema.annotateClips", json!({"clips":[clip.0],"annotations":annotations}))?;
                annotation_count += 1;
            }
        }
    }
    discard(s);
    checkpoint()?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut outputs = Vec::with_capacity(handles.len());
    let mut targeted = BTreeMap::<TrackId, BTreeSet<ClipId>>::new();
    for (slot, cid) in handles.iter().enumerate() {
        let root = *roots.get(slot).ok_or_else(|| error("reaction root missing"))?;
        let (old_track, original) = originals.get(&root).ok_or_else(|| error("reaction original missing"))?;
        let (track, c) = after.find_item(*cid).ok_or_else(|| error("reaction output disappeared"))?;
        if track != *old_track || !safe(c) {
            return Err(error("reaction fragment moved tracks or lost exact clocks"));
        }
        if c.identity.as_ref() != identities.get(slot) {
            return Err(error("reaction trace altered native identity, notes or protection"));
        }
        let mut payload = c.clone();
        payload.id = original.id;
        payload.start = original.start;
        payload.duration = original.duration;
        payload.source_in = original.source_in;
        payload.identity = original.identity.clone();
        payload.enabled = original.enabled;
        if project_audit::value_bytes(&payload)? != project_audit::value_bytes(original)? {
            return Err(error("reaction razor altered authored processing, channels, labels or media"));
        }
        outputs.push(row(slot, root, track, c)?);
        targeted.entry(track).or_default().insert(*cid);
    }
    for (root, (track, original)) in &originals {
        let mut parts = handles
            .iter()
            .zip(&roots)
            .filter(|(_, r)| *r == root)
            .map(|(id, _)| after.find_item(*id).map(|(_, c)| c).ok_or_else(|| error("reaction partition lost a piece")))
            .collect::<Result<Vec<_>>>()?;
        parts.sort_by_key(|c| c.start);
        if parts.first().is_none_or(|c| c.start != original.start)
            || parts.last().is_none_or(|c| c.end() != original.end())
            || parts.windows(2).any(|w| w.first().zip(w.get(1)).is_none_or(|(a, b)| a.end() != b.start))
            || after.track(*track).is_none()
        {
            return Err(error("reaction fragments do not preserve the complete original record interval"));
        }
    }
    let mut projection = (*before).clone();
    projection.next_id = s.project.next_id;
    let projected = projection.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    for (track, outputs) in &targeted {
        let a = after.track(*track).ok_or_else(|| error("reaction track missing"))?;
        let b = old.track(*track).ok_or_else(|| error("old reaction track missing"))?;
        let initial = p.targets.iter().filter(|t| originals.get(&t.slot).is_some_and(|(tr, _)| tr == track)).map(|t| ClipId(t.clip)).collect::<BTreeSet<_>>();
        if a.items.len()
            != b.items.len().checked_sub(initial.len()).and_then(|n| n.checked_add(outputs.len())).ok_or_else(|| error("reaction inventory overflow"))?
        {
            return Err(error("reaction trace changed undeclared clip inventory"));
        }
        for c in b.items.iter().filter(|c| !initial.contains(&c.id)) {
            let now = a.items.iter().find(|v| v.id == c.id).ok_or_else(|| error("reaction trace removed unrelated picture"))?;
            if project_audit::value_bytes(now)? != project_audit::value_bytes(c)? {
                return Err(error("reaction trace changed unrelated picture"));
            }
        }
        let t = projected.track_mut(*track).ok_or_else(|| error("projected reaction track missing"))?;
        t.items = a.items.clone();
        t.transitions = a.transitions.clone();
    }
    if project_audit::bytes(&projection)? != project_audit::bytes(&s.project)? || project_audit::value_bytes(&state)? != project_audit::value_bytes(&s.state)? {
        return Err(error("reaction trace changed unrelated project, audio, captions, markers or editor context"));
    }
    let after_project = sha(&*s.project)?;
    if after_project == p.before_project_sha256 {
        return Err(error("reaction trace makes no native change"));
    }
    Ok(
        json!({"beforeProjectSha256":p.before_project_sha256,"afterProjectSha256":after_project,"beforeStateSha256":p.before_state_sha256,"afterStateSha256":source_edit::digest(s)?,
        "beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":captions::digest(&after.caption_tracks)?,
        "beforeMarkerSha256":p.before_marker_sha256,"afterMarkerSha256":markers::digest(&after.markers)?,"beforeAudioSha256":sha(&old.audio_tracks)?,"afterAudioSha256":sha(&after.audio_tracks)?,
        "planSha256":p.plan_sha256,"decisionsSha256":p.decisions_sha256,"audioPolicy":p.audio_policy,"captionPolicy":p.caption_policy,"transitionPolicy":p.transition_policy,
        "sourceClockPolicy":"shared-human-razor","transitionChanges":transition_changes,"outcomes":outputs,"humanCommands":{"timeline.razor":razor_count,"clip.enable":enable_count,"poema.annotateClips":annotation_count}}),
    )
}
