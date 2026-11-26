//! Shared native roll/slip/slide requests. Resolve before entering history;
//! prepared requests cannot silently acquire new linked-selection dependencies.
use crate::commands::{bad, clip_p, linked_roll_pairs, slip_together, with_links};
use crate::{EngineError, Result, Session};
use filmcraft_edit as edit;
use filmcraft_project::{ClipId, ItemId, Project, Sequence};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Roll,
    Slip,
    Slide,
}
impl Kind {
    fn command(self) -> &'static str {
        match self {
            Self::Roll => "timeline.roll",
            Self::Slip => "timeline.slip",
            Self::Slide => "timeline.slide",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Roll => "Rolling Edit",
            Self::Slip => "Slip",
            Self::Slide => "Slide",
        }
    }
}

pub(crate) enum Targets {
    Roll(Vec<(ClipId, ClipId)>),
    Slip(Vec<ClipId>),
    Slide(Vec<ClipId>),
}
pub(crate) struct TrimPlan {
    basis: Arc<Project>,
    sequence: ItemId,
    linked_basis: Option<bool>,
    kind: Kind,
    action: Targets,
    delta: Tick,
}

fn delta(s: &Session, p: &Value, cmd: &str) -> Result<Tick> {
    let raw = match (p.get("delta"), p.get("deltaFrames")) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give exactly one of delta or deltaFrames, not both")),
        (None, None) => return Err(bad(cmd, "need exactly one of delta or deltaFrames")),
        (Some(v), None) => i128::from(v.as_i64().ok_or_else(|| bad(cmd, "delta must be integer ticks"))?),
        (None, Some(v)) => {
            let frames = v.as_i64().ok_or_else(|| bad(cmd, "deltaFrames must be an integer"))?;
            let rate = s.sequence_rate();
            if rate.num <= 0 || rate.den <= 0 || rate.frame_duration() <= Tick::ZERO {
                return Err(bad(cmd, "frame timing requires a positive representable sequence rate"));
            }
            let product = i128::from(frames)
                .checked_mul(i128::from(TICKS_PER_SECOND))
                .and_then(|n| n.checked_mul(i128::from(rate.den)))
                .ok_or_else(|| bad(cmd, "frame delta exceeds exact native time"))?;
            let denominator = i128::from(rate.num);
            // The shared native clock uses ceiling boundaries for exotic rates,
            // including negative frames. Check before its saturating conversion.
            product.div_euclid(denominator) + i128::from(product.rem_euclid(denominator) != 0)
        }
    };
    if raw == 0 || raw < i128::from(Tick::MIN.0) || raw > i128::from(Tick::MAX.0) {
        return Err(bad(cmd, "delta must be nonzero and within supported native time"));
    }
    i64::try_from(raw).map(Tick).map_err(|_| bad(cmd, "delta exceeds exact native time"))
}

pub(crate) fn run(s: &mut Session, p: &Value, kind: Kind) -> Result<Value> {
    prepare(s, p, kind)?.execute(s)
}

pub(crate) fn prepare(s: &Session, p: &Value, kind: Kind) -> Result<TrimPlan> {
    let cmd = kind.command();
    let obj = p.as_object().ok_or_else(|| bad(cmd, "need a trim request object"))?;
    let fields: &[&str] = match kind {
        Kind::Roll => &["left", "right", "delta", "deltaFrames", "followLinks"],
        Kind::Slip | Kind::Slide => &["clip", "delta", "deltaFrames"],
    };
    if obj.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(bad(cmd, "unknown trim field"));
    }
    let d = delta(s, p, cmd)?;
    let follow = p.get("followLinks").map(|v| v.as_bool().ok_or_else(|| bad(cmd, "followLinks must be boolean"))).transpose()?.unwrap_or(true);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let inventory = q.all_tracks().map(|t| t.items.len().saturating_add(t.transitions.len())).fold(0usize, usize::saturating_add);
    if inventory > 100_000 || q.all_tracks().count() > 4096 {
        return Err(bad(cmd, "trim inventory exceeds 100000 objects or 4096 tracks"));
    }
    let target = |key| -> Result<ClipId> {
        let id = clip_p(p, key)?.ok_or_else(|| bad(cmd, format!("need explicit {key}")))?;
        q.find_item(id).ok_or(edit::EditError::NoItem(id))?;
        Ok(id)
    };
    let action = match kind {
        Kind::Roll => {
            let (left, right) = (target("left")?, target("right")?);
            // edit::roll enforces the same-track, distinct and adjacent rule
            // below every caller, including Trim mode and keyboard extension.
            let pairs = linked_roll_pairs(q, left, right, s.state.linked_selection && follow);
            if pairs.len() > 4096 {
                return Err(bad(cmd, "roll exceeds 4096 resolved pairs"));
            }
            Targets::Roll(pairs)
        }
        Kind::Slip | Kind::Slide => {
            let clips = with_links(s, &[target("clip")?]);
            if clips.len() > 4096 {
                return Err(bad(cmd, "trim exceeds 4096 resolved clips"));
            }
            if matches!(kind, Kind::Slip) { Targets::Slip(clips) } else { Targets::Slide(clips) }
        }
    };
    Ok(TrimPlan {
        basis: s.project.clone(),
        sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        linked_basis: (!matches!(kind, Kind::Roll) || follow).then_some(s.state.linked_selection),
        kind,
        action,
        delta: d,
    })
}

/// Explicit current instances for callers which already resolved linked intent.
/// This shares the same algebra and one native history gesture as human commands;
/// it does not change the editor's Linked Selection setting.
pub(crate) fn prepare_explicit(s: &Session, targets: Targets, d: Tick) -> Result<TrimPlan> {
    let (kind, clips) = match &targets {
        Targets::Roll(pairs) => (Kind::Roll, pairs.iter().flat_map(|(l, r)| [*l, *r]).collect::<Vec<_>>()),
        Targets::Slip(clips) => (Kind::Slip, clips.clone()),
        Targets::Slide(clips) => (Kind::Slide, clips.clone()),
    };
    let cmd = kind.command();
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if d == Tick::ZERO
        || d < Tick::MIN
        || d > Tick::MAX
        || clips.is_empty()
        || clips.len() > 128
        || clips.iter().collect::<std::collections::BTreeSet<_>>().len() != clips.len()
    {
        return Err(bad(cmd, "need distinct explicit targets and a bounded nonzero delta"));
    }
    let mut tracks = std::collections::BTreeSet::new();
    for id in clips {
        let (track, _) = q.find_item(id).ok_or(edit::EditError::NoItem(id))?;
        if q.track(track).is_none_or(|t| t.locked) {
            return Err(edit::EditError::Locked.into());
        }
        if matches!(kind, Kind::Slide) && !tracks.insert(track) {
            return Err(bad(cmd, "one slide center per track is required for an exact gesture"));
        }
    }
    if q.all_tracks().count() > 4096 || q.all_tracks().map(|t| t.items.len().saturating_add(t.transitions.len())).fold(0usize, usize::saturating_add) > 100_000
    {
        return Err(bad(cmd, "trim inventory exceeds 100000 objects or 4096 tracks"));
    }
    Ok(TrimPlan {
        basis: s.project.clone(),
        sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        linked_basis: None,
        kind,
        action: targets,
        delta: d,
    })
}

impl TrimPlan {
    pub(crate) fn execute(self, s: &mut Session) -> Result<Value> {
        self.execute_mode(s, false)
    }
    pub(crate) fn execute_exact(self, s: &mut Session) -> Result<Value> {
        self.execute_mode(s, true)
    }
    fn execute_mode(self, s: &mut Session, exact: bool) -> Result<Value> {
        let cmd = self.kind.command();
        if !Arc::ptr_eq(&self.basis, &s.project)
            || s.state.active_sequence != Some(self.sequence)
            || self.linked_basis.is_some_and(|v| v != s.state.linked_selection)
        {
            return Err(bad(cmd, "prepared trim expired after document, sequence or linked-selection change"));
        }
        let d = self.delta;
        let label = self.kind.label();
        // Evaluate the shared algebra once against a private sequence. A valid
        // human gesture clamped to zero still succeeds, but must not enter native
        // history or discard Redo. Malformed input never reaches this point.
        let apply = |q: &mut Sequence, ctx: &mut edit::EditCtx<'_>| -> Result<(Value, bool)> {
            match self.action {
                Targets::Roll(pairs) => {
                    let mut effective = d;
                    for &(left, right) in &pairs {
                        let actual = edit::roll(&mut q.clone(), left, right, effective, ctx)?;
                        if actual.0.signum() != effective.0.signum() {
                            effective = Tick::ZERO;
                        } else if actual.abs() < effective.abs() {
                            effective = actual;
                        }
                    }
                    if exact && effective != d {
                        return Err(bad(cmd, "exact roll would clamp its requested delta"));
                    }
                    if effective != Tick::ZERO {
                        for &(left, right) in &pairs {
                            edit::roll(q, left, right, effective, ctx)?;
                        }
                    }
                    Ok((json!({"delta":effective.0,"pairs":pairs.len(),"linked":pairs.len()>1}), effective != Tick::ZERO))
                }
                Targets::Slip(clips) => {
                    let before: Vec<_> = clips.iter().filter_map(|id| q.find_item(*id).map(|(_, c)| (*id, c.source_in))).collect();
                    let deltas: Vec<_> = clips.iter().map(|id| (*id, d)).collect();
                    slip_together(q, ctx, &deltas)?;
                    if exact
                        && before.iter().any(|(id, source)| {
                            source.0.checked_add(d.0).is_none_or(|expected| q.find_item(*id).is_none_or(|(_, c)| c.source_in.0 != expected))
                        })
                    {
                        return Err(bad(cmd, "exact slip would clamp or skip a requested source delta"));
                    }
                    let changed = before.iter().any(|(id, source)| q.find_item(*id).is_some_and(|(_, c)| c.source_in != *source));
                    Ok((Value::Null, changed))
                }
                Targets::Slide(clips) => {
                    let unlocked: Vec<_> =
                        clips.into_iter().filter(|id| q.find_item(*id).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| !tr.locked))).collect();
                    if unlocked.is_empty() {
                        return Err(edit::EditError::Locked.into());
                    }
                    let actual = edit::slide_items(q, &unlocked, d, ctx)?;
                    if exact && actual != d {
                        return Err(bad(cmd, "exact slide would clamp its requested delta"));
                    }
                    let changed = actual != Tick::ZERO;
                    Ok((Value::Null, changed))
                }
            }
        };
        let mut candidate = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
        let media = s.media.clone();
        let durations = |id| crate::media_duration(&self.basis, &media, id);
        let starts = |id| crate::media_start(&self.basis, id);
        let mut next = self.basis.next_id;
        let mut ctx = edit::EditCtx {
            next_id: &mut next,
            media_duration: &durations,
            media_start: &starts,
            min_duration: candidate.settings.frame_rate.frame_duration(),
        };
        let (result, changed) = apply(&mut candidate, &mut ctx)?;
        candidate.check().map_err(EngineError::Other)?;
        if next != self.basis.next_id {
            return Err(bad(cmd, "roll/slip/slide unexpectedly allocated a native identity"));
        }
        if !changed {
            return Ok(result);
        }
        s.edit_sequence(label, move |q, _, _| {
            *q = candidate;
            Ok(result)
        })
    }
}
