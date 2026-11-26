//! Shared original idle track-mixer automation, without a Session or history owner.
//! Human command parsing/defaults stay in mixer.rs. Exact external callers must
//! separately validate observed targets, field types, clocks and no-clamp intent.
//! This module neither runs nor authorizes a live automation recorder.
use super::{MASTER_STRIP, PAN51_LANES, bad, lane_info, parse_fx_lane, parse_send_lane, underlying};
use crate::{EngineError, Result};
use filmcraft_project::mixer::{LANE_MUTE, LANE_PAN, thin_points, write_lane};
use filmcraft_project::{Param, ParamValue, Sequence, TrackId};
use filmcraft_time::Tick;

pub(crate) fn validate_lane(q: &Sequence, strip: TrackId, lane: &str, cmd: &str) -> Result<()> {
    let tr = q.strip(strip).ok_or_else(|| bad(cmd, "no such strip"))?;
    let ok = match parse_fx_lane(lane) {
        Some((slot, param)) => tr.effects.get(slot).and_then(|e| e.param(param)).is_some_and(|p| p.value.as_f64().is_some()),
        None => lane_info(lane).is_some() && parse_send_lane(lane).is_none_or(|i| i < tr.mixer.sends.len()),
    };
    if !ok {
        return Err(bad(cmd, format!("no lane `{lane}` on this strip (volume, pan, mute, send.<i>.level, fx.<slot>.<param>)")));
    }
    if strip == MASTER_STRIP && (lane == LANE_PAN || lane == LANE_MUTE || PAN51_LANES.contains(&lane) || parse_send_lane(lane).is_some()) {
        return Err(bad(cmd, "the Mix track has volume and effect lanes only"));
    }
    Ok(())
}
fn missing_strip() -> EngineError {
    EngineError::Other("no such strip".into())
}
fn put_keyframe(p: &mut Param, t: Tick, value: f64, hold: bool) {
    p.put_keyframe(t, ParamValue::Float(value));
    if hold {
        for k in &mut p.keyframes {
            k.interp = filmcraft_project::Interpolation::Hold;
        }
    }
}
pub(crate) fn set_value(q: &mut Sequence, strip: TrackId, lane: &str, value: f64, time: Tick) -> Result<()> {
    validate_lane(q, strip, lane, "mixer.setValue")?;
    let hold = lane_info(lane).is_some_and(|i| i.hold);
    q.with_strip_mut(strip, |tr| {
        let animated = tr.lane(lane).is_some_and(Param::is_animated) && tr.mixer.mode.reads();
        if animated {
            if let Some(p) = tr.lane_mut(lane) {
                put_keyframe(p, time, value, hold);
            }
        } else {
            tr.set_lane_static(lane, value);
        }
    })
    .ok_or_else(missing_strip)
}
pub(crate) fn set_keyframe(q: &mut Sequence, strip: TrackId, lane: &str, time: Tick, value: f64) -> Result<()> {
    validate_lane(q, strip, lane, "mixer.setKeyframe")?;
    let hold = lane_info(lane).is_some_and(|i| i.hold);
    q.with_strip_mut(strip, |tr| {
        if let Some(par) = tr.lane_mut(lane) {
            if par.keyframes.is_empty() {
                let cur = par.value.as_f64().unwrap_or(value);
                par.value = ParamValue::Float(cur);
            }
            put_keyframe(par, time, value, hold);
        }
    })
    .ok_or_else(missing_strip)
}
fn key_exists(q: &Sequence, strip: TrackId, lane: &str, time: Tick, cmd: &str) -> Result<()> {
    validate_lane(q, strip, lane, cmd)?;
    if !q.strip(strip).and_then(|tr| tr.lane(lane).map(|p| p.keyframes.binary_search_by_key(&time, |k| k.time).is_ok())).unwrap_or(false) {
        return Err(bad(cmd, "no keyframe at that time"));
    }
    Ok(())
}
pub(crate) fn delete_keyframe(q: &mut Sequence, strip: TrackId, lane: &str, time: Tick) -> Result<()> {
    key_exists(q, strip, lane, time, "mixer.deleteKeyframe")?;
    if !q.with_strip_mut(strip, |tr| tr.lane_mut(lane).is_some_and(|p| p.remove_keyframe_at(time))).unwrap_or(false) {
        return Err(bad("mixer.deleteKeyframe", "no keyframe at that time"));
    }
    Ok(())
}
pub(crate) fn move_keyframe(q: &mut Sequence, strip: TrackId, lane: &str, time: Tick, new_time: Option<Tick>, value: Option<f64>) -> Result<()> {
    key_exists(q, strip, lane, time, "mixer.moveKeyframe")?;
    q.with_strip_mut(strip, |tr| -> Result<()> {
        let par = tr.lane_mut(lane).ok_or_else(|| bad("mixer.moveKeyframe", "no such lane"))?;
        let i = par.keyframes.binary_search_by_key(&time, |k| k.time).map_err(|_| bad("mixer.moveKeyframe", "no keyframe at that time"))?;
        let mut k = par.keyframes.remove(i);
        if let Some(nt) = new_time {
            // Keep the original human clamp and the whole original key payload.
            // Tick arithmetic and clamp are total even on malformed time bounds.
            let lo = i.checked_sub(1).and_then(|j| par.keyframes.get(j)).map(|k| k.time + Tick(1)).unwrap_or(Tick::ZERO);
            let hi = par.keyframes.get(i).map(|k| k.time - Tick(1)).unwrap_or(Tick(i64::MAX));
            k.time = nt.clamp(lo, hi);
        }
        if let Some(v) = value {
            k.value = ParamValue::Float(v);
        }
        let j = par.keyframes.partition_point(|x| x.time < k.time);
        par.keyframes.insert(j, k);
        Ok(())
    })
    .ok_or_else(missing_strip)?
}
pub(crate) fn clear_lane(q: &mut Sequence, strip: TrackId, lane: &str, time: Tick) -> Result<()> {
    validate_lane(q, strip, lane, "mixer.clearLane")?;
    q.with_strip_mut(strip, |tr| {
        let v = tr.lane_value(lane, time);
        if let Some(par) = tr.lane_mut(lane) {
            par.keyframes.clear();
        }
        tr.set_lane_static(lane, v);
        if parse_fx_lane(lane).is_none() {
            tr.mixer.lanes.remove(lane);
        }
    })
    .ok_or_else(missing_strip)
}
pub(crate) struct WriteChange {
    strip: TrackId,
    lane: String,
    points: Vec<(Tick, f64)>,
    eps: Tick,
    hold: bool,
}
pub(crate) fn prepare_write(q: &Sequence, strip: TrackId, lane: &str, points: &[(Tick, f64)], tolerance: f64) -> Result<WriteChange> {
    validate_lane(q, strip, lane, "mixer.writeAutomation")?;
    if points.is_empty() {
        return Err(bad("mixer.writeAutomation", "no points"));
    }
    let hold = lane_info(lane).is_some_and(|i| i.hold);
    let points = thin_points(points, tolerance, Tick::ZERO, hold);
    if points.is_empty() {
        return Err(bad("mixer.writeAutomation", "no points after thinning"));
    }
    let rate = q.settings.sample_rate.max(1) as i64;
    let eps = Tick::from_units(1, rate).max(Tick(1));
    Ok(WriteChange { strip, lane: lane.to_string(), points, eps, hold })
}
impl WriteChange {
    pub(crate) fn kept_points(&self) -> usize {
        self.points.len()
    }
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<()> {
        let Self { strip, lane, points, eps, hold } = self;
        validate_lane(q, strip, &lane, "mixer.writeAutomation")?;
        let (Some(first), Some(last)) = (points.first(), points.last()) else { return Err(bad("mixer.writeAutomation", "no points")) };
        let (t0, t1) = (first.0, last.0);
        q.with_strip_mut(strip, |tr| {
            let before = (t0.0 > 0).then(|| underlying(tr, &lane, t0 - eps));
            let after = tr.lane(&lane).filter(|p| p.keyframes.iter().any(|k| k.time > t1)).map(|_| underlying(tr, &lane, t1 + eps));
            if let Some(par) = tr.lane_mut(&lane) {
                write_lane(par, t0, t1, &points, before, after, eps, hold);
            }
        })
        .ok_or_else(missing_strip)
    }
}
#[cfg(test)]
mod tests;
