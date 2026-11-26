//! Shared exact marker gestures. Native UI and guarded adapters use these commands;
//! marker identity is preserved when moving, and deletion never targets a time.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{Label, Marker, MarkerId, MarkerKind};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

pub(crate) const MAX_MARKERS: usize = 100_000;

fn text<'a>(p: &'a Value, key: &str, max: usize, cmd: &str) -> Result<Option<&'a str>> {
    p.get(key).map(|v| v.as_str().filter(|s| s.len() <= max && !s.contains('\0')).ok_or_else(|| bad(cmd, format!("`{key}` must be bounded text")))).transpose()
}

fn frame_tick(value: &Value, rate: FrameRate, cmd: &str) -> Result<Tick> {
    let frames = value.as_i64().filter(|n| *n >= 0).ok_or_else(|| bad(cmd, "frame values must be nonnegative integers"))?;
    if rate.num <= 0 || rate.den <= 0 {
        return Err(bad(cmd, "marker frame rate must be positive"));
    }
    let scale = i128::from(TICKS_PER_SECOND) * i128::from(rate.den);
    let product = i128::from(frames).checked_mul(scale).ok_or_else(|| bad(cmd, "marker frame conversion overflow"))?;
    let denominator = i128::from(rate.num);
    let ticks = product / denominator + i128::from(product % denominator != 0);
    // Guard saturation first, then use the existing native clock. Exotic rates
    // use ceiling frame boundaries; truncating here would move a marker back
    // into the preceding frame.
    i64::try_from(ticks).map(|_| rate.tick_of(frames)).map_err(|_| bad(cmd, "marker frame exceeds exact native time"))
}

/// Zero duration denotes a point. The caller of Add Range additionally requires positive duration.
pub(crate) fn duration(p: &Value, rate: FrameRate, fallback: Tick, cmd: &str) -> Result<Tick> {
    let d = match (p.get("duration"), p.get("durationFrames")) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give duration or durationFrames, not both")),
        (Some(v), None) => Tick(v.as_i64().filter(|n| *n >= 0).ok_or_else(|| bad(cmd, "duration must be nonnegative integer ticks"))?),
        (None, Some(v)) => frame_tick(v, rate, cmd)?,
        (None, None) => fallback,
    };
    if d < Tick::ZERO {
        return Err(bad(cmd, "marker duration must not be negative"));
    }
    Ok(d)
}

pub(crate) fn position(p: &Value, rate: FrameRate, cmd: &str) -> Result<Option<Tick>> {
    match (p.get("time"), p.get("frame")) {
        (Some(_), Some(_)) => Err(bad(cmd, "give time or frame, not both")),
        (Some(v), None) => v.as_i64().filter(|n| *n >= 0).map(|n| Some(Tick(n))).ok_or_else(|| bad(cmd, "time must be nonnegative integer ticks")),
        (None, Some(v)) => frame_tick(v, rate, cmd).map(Some),
        (None, None) => Ok(None),
    }
}

pub(crate) fn valid_span(start: Tick, duration: Tick, cmd: &str) -> Result<()> {
    if start < Tick::ZERO || duration < Tick::ZERO || start.0.checked_add(duration.0).is_none() {
        return Err(bad(cmd, "marker start/duration exceeds the nonnegative native time range"));
    }
    Ok(())
}

pub(crate) fn index(markers: &[Marker], id: MarkerId, cmd: &str) -> Result<usize> {
    if id.0 == 0 || markers.len() > MAX_MARKERS {
        return Err(bad(cmd, "invalid marker id or oversized inventory"));
    }
    let mut found = markers.iter().enumerate().filter(|(_, m)| m.id == id);
    let (index, _) = found.next().ok_or_else(|| bad(cmd, "no such marker in this target"))?;
    if found.next().is_some() {
        return Err(bad(cmd, "marker identity is duplicated in this target"));
    }
    Ok(index)
}

/// Shared insertion used by comment, range, chapter and flash-cue commands.
pub(crate) fn add_sequence(
    s: &mut Session,
    cmd: &str,
    label: &str,
    start: Tick,
    duration: Tick,
    kind: MarkerKind,
    name: String,
    p: &Value,
) -> Result<MarkerId> {
    valid_span(start, duration, cmd)?;
    text(p, "name", 1024, cmd)?;
    if name.len() > 1024 || name.contains('\0') {
        return Err(bad(cmd, "marker name exceeds bounded text"));
    }
    let comment = text(p, "comment", 65536, cmd)?.unwrap_or_default().to_string();
    let color = crate::commands::marker_color_p(p, cmd)?.unwrap_or(Label::Green);
    // Imported/demo numeric IDs can be above next_id. Reuse the bounded allocator,
    // rather than collide with an existing chapter/caption/clip identity.
    let next = crate::project_ids::allocation_start(&s.project, 1)?;
    s.edit_sequence(label, |q, ctx, _| {
        if q.markers.len() >= MAX_MARKERS {
            return Err(bad(cmd, "marker inventory exceeds 100000"));
        }
        *ctx.next_id = next;
        let id = MarkerId(ctx.alloc());
        q.markers.push(Marker { id, start, duration, name, comment, kind, color });
        q.markers.sort_by_key(|m| m.start);
        Ok(id)
    })
}

pub(crate) struct Patch {
    pub id: MarkerId,
    start: Option<Tick>,
    duration: Option<Tick>,
    name: Option<String>,
    comment: Option<String>,
    color: Option<Label>,
}

impl Patch {
    pub(crate) fn parse(p: &Value, rate: FrameRate) -> Result<Self> {
        const CMD: &str = "markers.edit";
        let allowed = ["target", "marker", "time", "frame", "duration", "durationFrames", "name", "comment", "color"];
        if p.as_object().is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str()))) {
            return Err(bad(CMD, "unknown marker edit field"));
        }
        let id = marker_id(p, CMD)?;
        let start = position(p, rate, CMD)?;
        let duration = if p.get("duration").is_some() || p.get("durationFrames").is_some() { Some(duration(p, rate, Tick::ZERO, CMD)?) } else { None };
        let name = text(p, "name", 1024, CMD)?.map(str::to_string);
        let comment = text(p, "comment", 65536, CMD)?.map(str::to_string);
        let color = crate::commands::marker_color_p(p, CMD)?;
        if start.is_none() && duration.is_none() && name.is_none() && comment.is_none() && color.is_none() {
            return Err(bad(CMD, "give at least one marker field to edit"));
        }
        Ok(Self { id, start, duration, name, comment, color })
    }

    pub(crate) fn apply(&self, markers: &mut [Marker], source_bounds: Option<(Tick, Tick)>) -> Result<()> {
        const CMD: &str = "markers.edit";
        let i = index(markers, self.id, CMD)?;
        let old = markers.get(i).ok_or_else(|| bad(CMD, "marker disappeared"))?;
        let mut marker = old.clone();
        if let Some(start) = self.start {
            marker.start = start;
        }
        if let Some(duration) = self.duration {
            marker.duration = duration;
        }
        if let Some(name) = &self.name {
            marker.name.clone_from(name);
        }
        if let Some(comment) = &self.comment {
            marker.comment.clone_from(comment);
        }
        if let Some(color) = self.color {
            marker.color = color;
        }
        valid_span(marker.start, marker.duration, CMD)?;
        if let Some((lo, hi)) = source_bounds {
            if self.duration.is_some_and(|d| d.0 > hi.0.saturating_sub(lo.0)) {
                return Err(bad(CMD, "marker duration is outside the Source clip"));
            }
            if marker.start < lo || marker.start >= hi {
                return Err(bad(CMD, "marker time is outside the Source clip"));
            }
            // Preserve the established human Source duration gesture. Relocating
            // an existing range alone never silently shortens it.
            if self.duration.is_some() && self.start.is_none() {
                marker.duration = marker.duration.min(Tick(hi.0.saturating_sub(marker.start.0)));
            } else if marker.start.0.checked_add(marker.duration.0).is_none_or(|end| end > hi.0) {
                return Err(bad(CMD, "marker range extends outside the Source clip"));
            }
        }
        *markers.get_mut(i).ok_or_else(|| bad(CMD, "marker disappeared"))? = marker;
        markers.sort_by_key(|m| m.start);
        Ok(())
    }
}

pub(crate) fn marker_id(p: &Value, cmd: &str) -> Result<MarkerId> {
    p.get("marker").and_then(Value::as_u64).filter(|id| *id > 0).map(MarkerId).ok_or_else(|| bad(cmd, "need one positive integer marker id"))
}

pub(crate) fn edit_sequence(s: &mut Session, p: &Value) -> Result<Value> {
    let patch = Patch::parse(p, s.sequence_rate())?;
    s.edit_sequence("Edit Marker", |q, _, _| patch.apply(&mut q.markers, None))?;
    Ok(Value::Null)
}

pub(crate) fn remove_sequence(s: &mut Session, p: &Value) -> Result<Value> {
    let id = s.edit_sequence("Remove Marker", |q, _, _| remove(&mut q.markers, p))?;
    Ok(json!({"removed":id.0}))
}

pub(crate) fn remove(markers: &mut Vec<Marker>, p: &Value) -> Result<MarkerId> {
    const CMD: &str = "markers.remove";
    if p.as_object().is_none_or(|o| o.keys().any(|k| !["target", "marker"].contains(&k.as_str()))) {
        return Err(bad(CMD, "remove requires one marker id and its target"));
    }
    let id = marker_id(p, CMD)?;
    let i = index(markers, id, CMD)?;
    markers.remove(i);
    Ok(id)
}

#[cfg(test)]
#[path = "marker_ops_tests.rs"]
mod tests;
