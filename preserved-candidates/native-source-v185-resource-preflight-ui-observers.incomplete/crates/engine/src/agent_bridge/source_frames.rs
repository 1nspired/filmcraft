//! Read-only original-source coordinates from the existing native clip clock.
//! No decode, seek, annotation, file read/write or history ownership is created.
use std::collections::BTreeMap;

use filmcraft_project::MediaRef;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{BINDING_KEY, Binding, parse, sequences};
use crate::{Result, Session, commands::bad};

const COMMAND: &str = "agent.sourceFrames";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    scope: sequences::Scope,
    frames: Vec<i64>,
    #[serde(default)]
    projection: bool,
}

pub(super) fn map(s: &mut Session, params: &Value) -> Result<Value> {
    let r: Request = parse(COMMAND, params)?;
    if r.frames.is_empty() || r.frames.len() > 12 || r.frames.iter().any(|f| !(0..=1_000_000).contains(f)) || r.frames.windows(2).any(|w| w[0] >= w[1]) {
        return Err(bad(COMMAND, "need 1–12 distinct ordered sequence frames"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &r.scope)? {
        return Err(bad(COMMAND, "sequence is outside this explicit workspace"));
    }
    let q = s.project.sequence(sequence).ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(|e| bad(COMMAND, e))?;
    if s.project.items.len() > 100_000 {
        return Err(bad(COMMAND, "source inventory exceeds the bounded mapping scan"));
    }
    let mut bindings = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for (id, item) in &s.project.items {
        if item.as_media().is_none() {
            continue;
        }
        if let Some(raw) = item.metadata.get(BINDING_KEY) {
            let binding: Binding = serde_json::from_str(raw).map_err(|e| bad(COMMAND, e.to_string()))?;
            if binding.project_id == r.scope.project_id && binding.role == "original" {
                *counts.entry(binding.asset_id.clone()).or_insert(0usize) += 1;
            }
            bindings.insert(*id, binding);
        }
    }
    let mut result = Vec::with_capacity(r.frames.len());
    for frame in &r.frames {
        let numerator = i128::from(*frame) * i128::from(TICKS_PER_SECOND) * i128::from(q.settings.frame_rate.den);
        let denominator = i128::from(q.settings.frame_rate.num);
        if numerator % denominator != 0 || numerator / denominator > i128::from(SAFE) {
            return Err(bad(COMMAND, "sequence frame is not exactly representable by this transport"));
        }
        let t = Tick(i64::try_from(numerator / denominator).map_err(|_| bad(COMMAND, "frame overflow"))?);
        let mut candidate = None;
        let mut budget = 100_000usize;
        for track in &q.video_tracks {
            for clip in &track.items {
                budget = budget.checked_sub(1).ok_or_else(|| bad(COMMAND, "clip inventory exceeds the bounded mapping scan"))?;
                let end = clip.start.0.checked_add(clip.duration.0).ok_or_else(|| bad(COMMAND, "clip range overflow"))?;
                if !track.enabled || !clip.enabled || clip.duration.0 <= 0 || t.0 < clip.start.0 || t.0 >= end {
                    continue;
                }
                if candidate.replace(clip).is_some() {
                    return Err(bad(COMMAND, "overlapping video needs native Program inspection, not a guessed original"));
                }
            }
        }
        let clip = candidate.ok_or_else(|| bad(COMMAND, "sample lies in a gap; no original frame corresponds"))?;
        if clip.frame_hold.is_none() && (clip.speed != 1.0 || clip.reverse)
            || clip.effects.iter().any(|e| {
                e.enabled
                    && e.effect == "time_remap"
                    && e.params.values().any(|p| !p.keyframes.is_empty() || p.value != filmcraft_project::ParamValue::Float(100.0))
            })
        {
            return Err(bad(COMMAND, "this original-source read needs a held or forward 1x clock; use Program inspection for other retiming"));
        }
        let media = s.project.item(clip.item).and_then(|i| i.as_media()).ok_or_else(|| bad(COMMAND, "nested/graphic source needs Program inspection"))?;
        let binding = bindings.get(&clip.item).ok_or_else(|| bad(COMMAND, "source has no explicit original binding"))?;
        if binding.project_id != r.scope.project_id
            || binding.role != "original"
            || binding.asset_id.is_empty()
            || binding.media_id.is_empty()
            || counts.get(&binding.asset_id) != Some(&1)
            || media.offline
            || !media.info.has_video()
            || !matches!(&media.media, MediaRef::File { path } if std::path::Path::new(path).is_absolute())
            || !(1..=SAFE).contains(&media.info.duration.0)
            || !(0..=SAFE).contains(&clip.source_in.0)
            || clip.start.0.abs_diff(t.0) > SAFE as u64
        {
            return Err(bad(COMMAND, "choose one online bound original with supported exact coordinates"));
        }
        // The human monitor/renderer use this same clock, including native frame holds.
        // Bounds above keep forward 1x arithmetic within i64 and exact f64 integer range.
        let source = clip.source_time_at(t);
        if !(0..media.info.duration.0).contains(&source.0) || source.0 > SAFE {
            return Err(bad(COMMAND, "mapped source time is outside this original"));
        }
        result.push(json!({"frame":frame,"timelineTicks":t.0,"clip":clip.id.0,"item":clip.item.0,
            "identity":clip.identity.as_ref().map(|i|i.id.as_str()),"sourceTicks":source.0,
            "mapping":if clip.frame_hold.is_some(){"held"}else{"forward"}}));
    }
    let mut result = json!({"status":"mapped","token":r.token,"revision":s.revision,"sequence":sequence.0,"scope":r.scope,"frames":result});
    if r.projection {
        result["requiresSourceValidation"] = json!(true);
    }
    Ok(result)
}
