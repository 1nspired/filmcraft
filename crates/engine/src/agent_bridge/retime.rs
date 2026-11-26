//! Exact retiming via the existing human `clip.speedDuration` command. No
//! second editing algorithm, live selection defaults or partial linked edits.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ClipId, TimeInterpolation};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Operation, assembly, caption_ripple, parse};
use crate::commands::bad;
use crate::{Result, Session};

pub(super) const COMMAND: &str = "agent.retime";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Retime {
    clips: Vec<u64>,
    project_id: String,
    speed_percent: f64,
    reverse: bool,
    ripple: bool,
    interpolation: String,
    #[serde(default)]
    caption_basis: Option<caption_ripple::Basis>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Retime = parse(COMMAND, &op.params)?;
    if p.clips.is_empty()
        || p.clips.len() > 16
        || p.clips.iter().any(|id| *id == 0 || *id > SAFE as u64)
        || p.clips.iter().collect::<BTreeSet<_>>().len() != p.clips.len()
        || p.project_id.is_empty()
        || p.project_id.len() > 256
        || p.project_id.chars().any(char::is_control)
        || !p.speed_percent.is_finite()
        || p.speed_percent <= 0.0
        || p.speed_percent / 100.0 <= 0.0
        || p.caption_basis.as_ref().is_some_and(|basis| !p.ripple || !basis.valid())
        || !matches!(p.interpolation.as_str(), "frameSampling" | "frameBlending" | "opticalFlow")
    {
        return Err(error("need 1–16 distinct explicit clips, original project scope, positive speed percent, reverse, ripple and interpolation"));
    }
    Ok(())
}

pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Retime = parse(COMMAND, &op.params)?;
    let interpolation = TimeInterpolation::from_name(&p.interpolation).ok_or_else(|| error("unknown interpolation"))?;
    let speed = p.speed_percent / 100.0;
    let before = s.project.clone();
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let old = before.sequence(seq).ok_or(crate::EngineError::NoSequence)?;
    let item_count = old.all_tracks().flat_map(|t| &t.items).take(100_001).count();
    if item_count > 100_000 || old.all_tracks().flat_map(|t| &t.transitions).take(100_001).count() > 100_000 {
        return Err(error("retime exceeds the bounded instance audit"));
    }
    if old.settings.frame_rate.num <= 0 || old.settings.frame_rate.den <= 0 || old.settings.frame_rate.frame_duration() <= Tick::ZERO {
        return Err(error("retime needs a positive native sequence frame rate"));
    }
    let captions = if p.ripple { caption_ripple::prepare(s, p.caption_basis, &p.project_id)? } else { None };
    let mut members: Vec<ClipId> = p.clips.iter().copied().map(ClipId).collect();
    let mut links = BTreeSet::new();
    for id in &members {
        let (_, c) = old.find_item(*id).ok_or_else(|| error("retime clip is absent from the active sequence"))?;
        if let Some(link) = c.link {
            links.insert(link);
        }
    }
    // Same expansion order as with_links, but explicit even when the human has
    // Linked Selection off. The guarded request always means the whole group.
    for c in old.all_tracks().flat_map(|t| &t.items) {
        if c.link.is_some_and(|l| links.contains(&l)) && !members.contains(&c.id) {
            members.push(c.id);
        }
    }
    if members.len() > 128 {
        return Err(error("retime linked expansion exceeds 128 members"));
    }
    let targets: BTreeSet<_> = members.iter().copied().collect();
    let mut durations = BTreeMap::new();
    for id in &members {
        let (track, c) = old.find_item(*id).ok_or_else(|| error("missing linked target"))?;
        let media = before.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("retime needs explicitly bound original media"))?;
        if old.track(track).is_none_or(|t| t.locked)
            || c.identity.is_none()
            || media.offline
            || s.media.offline_status(c.item).is_some()
            || assembly::binding(s, c.item)?.project_id != p.project_id
        {
            return Err(error("retime cannot skip locked, untracked, offline or foreign linked material"));
        }
        if !(c.speed.is_finite()
            && c.speed > 0.0
            && (-SAFE..=SAFE).contains(&c.start.0)
            && (1..=SAFE).contains(&c.duration.0)
            && (0..=SAFE).contains(&c.source_in.0))
        {
            return Err(error("existing retime source mapping is outside the exact supported range"));
        }
        let source_length = c.source_out().0.checked_sub(c.source_in.0).filter(|n| (1..=SAFE).contains(n)).ok_or_else(|| error("invalid source span"))?;
        let desired = filmcraft_edit::duration_at_speed(Tick(source_length), speed)?;
        if desired < old.settings.frame_rate.frame_duration()
            || !(1..=SAFE).contains(&desired.0)
            || c.start.0.checked_add(desired.0).is_none_or(|n| n > SAFE)
            || c.source_out() > media.info.duration
        {
            return Err(error("requested speed exceeds source, minimum-duration or exact tick bounds"));
        }
        durations.insert(*id, desired);
    }
    let steps = crate::speed_duration::run(
        s,
        &json!({"clips":members.iter().map(|id| id.0).collect::<Vec<_>>(),"speed":p.speed_percent,
        "reverse":p.reverse,"ripple":p.ripple,"interpolation":p.interpolation}),
    )?;
    let new = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    // A single index keeps a large collateral audit O(n log n), rather than
    // repeatedly scanning the whole sequence once for each old instance.
    let after_items: BTreeMap<_, _> = new.all_tracks().flat_map(|t| t.items.iter().map(move |c| (c.id, (t.id, c)))).take(100_001).collect();
    if after_items.len() != item_count {
        return Err(error("retime unexpectedly added/duplicated an instance"));
    }
    let mut shifts: BTreeMap<u64, i64> = BTreeMap::new();
    for track in old.all_tracks() {
        let after_track = new.track(track.id).ok_or_else(|| error("retime removed a track"))?;
        let after_transitions: BTreeMap<_, _> = after_track.transitions.iter().map(|t| (t.id, t)).collect();
        if after_transitions.len() != track.transitions.len() {
            return Err(error("retime unexpectedly added/removed/duplicated a transition"));
        }
        if track.locked && track.transitions != after_track.transitions {
            return Err(error("retime would change transitions on a locked track"));
        }
        let protected: BTreeSet<_> = track.items.iter().filter(|c| c.identity.as_ref().is_some_and(|i| i.protection.is_some())).map(|c| c.id).collect();
        for transition in &track.transitions {
            if (transition.from.is_some_and(|c| protected.contains(&c)) || transition.to.is_some_and(|c| protected.contains(&c)))
                && after_transitions.get(&transition.id).copied() != Some(transition)
            {
                return Err(error("retime would change a transition involving a protected clip"));
            }
        }
        for c in &track.items {
            let (tid, after) = *after_items.get(&c.id).ok_or_else(|| error("retime unexpectedly removed an instance"))?;
            let mut expected = c.clone();
            if targets.contains(&c.id) {
                expected.duration = *durations.get(&c.id).ok_or_else(|| error("missing exact duration"))?;
                expected.speed = speed;
                expected.reverse = p.reverse;
                expected.time_interpolation = interpolation;
                let media = before.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("missing original"))?;
                if after.source_out() > media.info.duration || after.source_out().0 > SAFE {
                    return Err(error("native retiming rounds beyond the original source bound"));
                }
            }
            if p.ripple {
                expected.start = after.start;
            }
            if tid != track.id || &expected != after {
                return Err(error("retime was clamped/incomplete or changed source, identity, routing or clip effects"));
            }
            if after != c && (track.locked || c.identity.is_none() || assembly::binding(s, c.item)?.project_id != p.project_id) {
                return Err(error("retime ripple would change locked, untracked or foreign collateral"));
            }
            if after != c
                && (!(-SAFE..=SAFE).contains(&after.start.0) || after.start.0.checked_add(after.duration.0).is_none_or(|n| !(-SAFE..=SAFE).contains(&n)))
            {
                return Err(error("retime ripple exceeded the exact supported timeline"));
            }
            if let Some(link) = c.link.filter(|_| !targets.contains(&c.id)) {
                let shift = after.start.0.checked_sub(c.start.0).ok_or_else(|| error("linked shift overflow"))?;
                if shifts.insert(link, shift).is_some_and(|old| old != shift) {
                    return Err(error("retime would desynchronize a linked group through locked/sync-lock collateral"));
                }
            }
        }
    }
    if !p.ripple && new.caption_tracks != old.caption_tracks {
        return Err(error("non-ripple retiming must preserve caption state"));
    }
    let mut result = json!({"retimed":members.len(),"speedPercent":p.speed_percent,"reverse":p.reverse,"ripple":p.ripple,"interpolation":p.interpolation});
    if let Some(proof) = caption_ripple::finish_retime(s, captions, &steps)? {
        result["captionRipple"] = proof;
    }
    Ok(result)
}
