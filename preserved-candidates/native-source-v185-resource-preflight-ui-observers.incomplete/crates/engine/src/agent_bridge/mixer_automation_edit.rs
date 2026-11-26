//! Exact authored gestures through the SAME original human idle mixer services.
use super::{mixer_automation, parse};
use crate::{EngineError, Result, commands::bad, mixer::automation};
use filmcraft_project::{Param, ParamValue, Sequence};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
const SAFE: i64 = 9_007_199_254_740_991;
fn error(m: &str) -> EngineError {
    bad(super::tracks::COMMAND, m)
}
fn clock(t: i64) -> bool {
    (-SAFE..=SAFE).contains(&t)
}
fn time(t: i64) -> bool {
    (0..=SAFE).contains(&t)
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Gesture {
    SetValue { time: i64, value: f64 },
    SetKeyframe { time: i64, value: f64 },
    DeleteKeyframe { time: i64 },
    MoveKeyframe { time: i64, new_time: Option<i64>, value: Option<f64> },
    ClearLane,
    WriteAutomation { points: Vec<(i64, f64)>, tolerance: f64 },
}
fn gesture(v: &Value) -> Result<Gesture> {
    if !serde_json::to_vec(v).is_ok_and(|b| b.len() <= 192 * 1024) || v.get("points").and_then(Value::as_array).is_some_and(|a| a.len() > 4096) {
        return Err(error("bounded explicit authored automation gesture required"));
    }
    parse(super::tracks::COMMAND, v)
}
pub(super) fn valid(v: &Value) -> bool {
    let Ok(g) = gesture(v) else { return false };
    match g {
        Gesture::SetValue { time: t, value } | Gesture::SetKeyframe { time: t, value } => time(t) && value.is_finite(),
        Gesture::DeleteKeyframe { time: t } => clock(t),
        Gesture::MoveKeyframe { time: t, new_time, value } => {
            clock(t)
                && (new_time.is_some() || value.is_some())
                && new_time.is_none_or(time)
                && value.is_none_or(f64::is_finite)
                && ["new_time", "value"].iter().all(|k| v.get(*k).is_none_or(|v| !v.is_null()))
        }
        Gesture::ClearLane => v.as_object().is_some_and(|o| o.len() == 1),
        Gesture::WriteAutomation { points, tolerance } => {
            !points.is_empty()
                && points.len() <= 4096
                && tolerance.is_finite()
                && (0.0..=1_000_000.0).contains(&tolerance)
                && points.iter().all(|(t, v)| time(*t) && v.is_finite())
                && points.windows(2).all(|w| w[0].0 < w[1].0)
        }
    }
}
fn exact_value(t: &mixer_automation::EditTarget, v: f64) -> Result<()> {
    let (min, max, hold) = t.range;
    if !v.is_finite() || v < min || v > max || hold && v != 0.0 && v != 1.0 {
        return Err(error("exact native lane value required; no range clamping, fractional mute or type conversion"));
    }
    Ok(())
}
fn after_param(q: &Sequence, t: &mixer_automation::EditTarget) -> Result<(Param, f64, bool)> {
    let tr = q.strip(t.strip).ok_or_else(|| error("expected automation strip disappeared"))?;
    let stat = tr.lane_static(&t.key).ok_or_else(|| error("expected lane static disappeared"))?;
    let p = tr.lane(&t.key);
    Ok((p.cloned().unwrap_or_else(|| Param::new(ParamValue::Float(stat))), stat, p.is_some()))
}
pub(super) fn expected(
    q: &mut Sequence,
    strip: Option<u64>,
    lane: usize,
    observed_clock: i64,
    actual_clock: Tick,
    v: &Value,
) -> Result<(&'static str, Value, Value)> {
    if !valid(v) || !clock(observed_clock) || observed_clock != actual_clock.0 {
        return Err(error("exact authored gesture and unchanged observed native playhead required"));
    }
    let target = mixer_automation::edit_target(q, strip, lane)?;
    let (_, before_static, had_lane) = after_param(q, &target)?;
    let mut args = json!({"strip":strip.map_or_else(||json!("Mix"),|id|json!(id)),"lane":target.key});
    let mut input_points = None;
    let mut kept_points = None;
    let mut tolerance = None;
    let (command, action) = match gesture(v)? {
        Gesture::SetValue { time: t, value } => {
            exact_value(&target, value)?;
            args["time"] = json!(t);
            args["value"] = json!(value);
            automation::set_value(q, target.strip, &target.key, value, Tick(t))?;
            ("mixer.setValue", "set_value")
        }
        Gesture::SetKeyframe { time: t, value } => {
            exact_value(&target, value)?;
            args["time"] = json!(t);
            args["value"] = json!(value);
            automation::set_keyframe(q, target.strip, &target.key, Tick(t), value)?;
            ("mixer.setKeyframe", "set_keyframe")
        }
        Gesture::DeleteKeyframe { time: t } => {
            args["time"] = json!(t);
            automation::delete_keyframe(q, target.strip, &target.key, Tick(t))?;
            ("mixer.deleteKeyframe", "delete_keyframe")
        }
        Gesture::MoveKeyframe { time: t, new_time, value } => {
            let i = target.param.keyframes.binary_search_by_key(&Tick(t), |k| k.time).map_err(|_| error("read the exact existing key before moving it"))?;
            if let Some(nt) = new_time {
                let lo = i.checked_sub(1).and_then(|j| target.param.keyframes.get(j)).map(|k| k.time + Tick(1)).unwrap_or(Tick::ZERO);
                let hi = target.param.keyframes.get(i.saturating_add(1)).map(|k| k.time - Tick(1)).unwrap_or(Tick(i64::MAX));
                if lo > hi || Tick(nt) < lo || Tick(nt) > hi {
                    return Err(error("keyframe move would be clamped between its neighbors"));
                }
                args["newTime"] = json!(nt);
            }
            if let Some(value) = value {
                exact_value(&target, value)?;
                args["value"] = json!(value);
            }
            args["time"] = json!(t);
            automation::move_keyframe(q, target.strip, &target.key, Tick(t), new_time.map(Tick), value)?;
            ("mixer.moveKeyframe", "move_keyframe")
        }
        Gesture::ClearLane => {
            let tr = q.strip(target.strip).ok_or_else(|| error("missing clear target"))?;
            exact_value(&target, tr.lane_value(&target.key, actual_clock))?;
            automation::clear_lane(q, target.strip, &target.key, actual_clock)?;
            ("mixer.clearLane", "clear_lane")
        }
        Gesture::WriteAutomation { points, tolerance: tol } => {
            for (_, value) in &points {
                exact_value(&target, *value)?;
            }
            let native: Vec<_> = points.iter().map(|(t, v)| (Tick(*t), *v)).collect();
            let change = automation::prepare_write(q, target.strip, &target.key, &native, tol)?;
            input_points = Some(points.len());
            kept_points = Some(change.kept_points());
            tolerance = Some(tol);
            args["points"] = json!(points);
            args["tolerance"] = json!(tol);
            change.apply(q)?;
            ("mixer.writeAutomation", "write_automation")
        }
    };
    mixer_automation::validate_after(q, strip)?;
    let (after, after_static, has_lane) = after_param(q, &target)?;
    let before_keys: BTreeMap<_, _> = target.param.keyframes.iter().map(|k| (k.time, k)).collect();
    let after_keys: BTreeMap<_, _> = after.keyframes.iter().map(|k| (k.time, k)).collect();
    let removed = before_keys.keys().filter(|t| !after_keys.contains_key(t)).count();
    let inserted = after_keys.keys().filter(|t| !before_keys.contains_key(t)).count();
    let changed = after.keyframes.iter().filter(|k| before_keys.get(&k.time).is_some_and(|old| *old != *k)).count();
    let summary = json!({"action":action,"lane":lane,"key":target.key,"storage":target.storage,"timeDomain":"native_sequence_ticks", "beforeKeyframes":target.param.keyframes.len(),"afterKeyframes":after.keyframes.len(),"removedKeyframes":removed,"insertedKeyframes":inserted,"changedKeyframes":changed,
        "storedValueChanged":target.param.value!=after.value,"staticValueChanged":before_static!=after_static,"storedLaneRemoved":had_lane&&!has_lane,"inputPoints":input_points,"keptPoints":kept_points,"tolerance":tolerance});
    Ok((command, args, summary))
}

pub(super) fn live_basis(s: &crate::Session, op: &super::Operation) -> Result<()> {
    if op.command != super::tracks::COMMAND || op.params.get("edit").and_then(|e| e.get("type")).and_then(Value::as_str) != Some("automation") {
        return Ok(());
    }
    if s.previews.live.is_active() || s.mixrec.active() {
        return Err(error("finish the live mixer gesture before an authored automation edit"));
    }
    let clock = op.params.get("edit").and_then(|e| e.get("clock")).and_then(Value::as_i64).ok_or_else(|| error("observed native playhead required"))?;
    if clock != s.playhead().0 {
        return Err(error("native playhead changed before automation publication"));
    }
    Ok(())
}
