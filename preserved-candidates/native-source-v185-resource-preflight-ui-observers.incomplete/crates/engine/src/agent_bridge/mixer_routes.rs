//! Exact scoped mixer settings/sends through the original human services.
use crate::{EngineError, Result, commands::bad, mixer::settings};
use filmcraft_project::{
    Sequence, TrackId,
    mixer::{FADER_MAX_DB, FADER_MIN_DB, MASTER_STRIP},
};
use serde_json::{Value, json};
fn error(message: &str) -> EngineError {
    bad(super::tracks::COMMAND, message)
}
fn id(v: u64) -> bool {
    v > 0 && v <= 9_007_199_254_740_991
}
fn text(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.len() <= 1024 && !s.contains('\0'))
}
fn number(v: &Value, min: f64, max: f64) -> bool {
    v.as_f64().is_some_and(|n| n.is_finite() && n >= min && n <= max)
}
fn choice(v: &Value, values: &[&str]) -> bool {
    v.as_str().is_some_and(|s| values.contains(&s))
}
pub(super) fn valid_strip_changes(value: &Value, master: bool) -> bool {
    let Some(o) = value.as_object().filter(|o| !o.is_empty() && o.len() <= 12) else { return false };
    o.iter().all(|(k, v)| {
        if master && !matches!(k.as_str(), "volumeDb" | "mode") {
            return false;
        }
        match k.as_str() {
            "name" => text(v),
            "volumeDb" => number(v, FADER_MIN_DB, FADER_MAX_DB),
            "pan" => number(v, -100.0, 100.0),
            "muted" | "solo" | "recordArm" | "soloSafe" => v.is_boolean(),
            "mode" => choice(v, &["Off", "Read", "Latch", "Touch", "Write"]),
            "inputMap" => choice(v, &["Stereo", "Left", "Right", "Swap", "Mono"]),
            "channels" => choice(v, &["Mono", "Stereo", "5.1", "Adaptive"]),
            "output" => v.is_null() || v.as_u64().is_some_and(id),
            _ => false,
        }
    })
}
pub(super) fn valid_send_changes(value: &Value) -> bool {
    let Some(o) = value.as_object().filter(|o| !o.is_empty() && o.len() <= 5) else { return false };
    o.iter().all(|(k, v)| match k.as_str() {
        "levelDb" => number(v, FADER_MIN_DB, FADER_MAX_DB),
        "pan" => number(v, -100.0, 100.0),
        "preFader" | "muted" => v.is_boolean(),
        "target" => v.as_u64().is_some_and(id),
        _ => false,
    })
}
fn strip(q: &Sequence, value: Option<u64>) -> Result<TrackId> {
    let Some(value) = value else { return Ok(MASTER_STRIP) };
    if !id(value) {
        return Err(error("explicit safe audio/submix strip required"));
    }
    let t = q.mix_track(TrackId(value)).ok_or_else(|| error("strip is outside the current audio/submix collection"))?;
    if t.locked {
        return Err(error("unlock the target strip explicitly before mixer edits"));
    }
    Ok(t.id)
}
fn strip_args(value: Option<u64>) -> Value {
    value.map_or_else(|| json!("Mix"), |id| json!(id))
}
pub(super) fn strip_expected(q: &mut Sequence, value: Option<u64>, changes: &Value) -> Result<Value> {
    if !valid_strip_changes(changes, value.is_none()) {
        return Err(error("explicit persisted mixer fields and exact values required; no ignored master fields or clamping"));
    }
    let id = strip(q, value)?;
    let mut args = changes.clone();
    args["strip"] = strip_args(value);
    settings::prepare_strip(q, id, &args)?.apply(q)?;
    Ok(args)
}
pub(super) fn add_send_expected(q: &mut Sequence, value: u64, target: u64, level_db: f64, pan: f64, pre_fader: bool) -> Result<Value> {
    if !id(target) || !number(&json!(level_db), FADER_MIN_DB, FADER_MAX_DB) || !number(&json!(pan), -100.0, 100.0) {
        return Err(error("exact bounded send target and values required"));
    }
    let id = strip(q, Some(value))?;
    let args = json!({"strip":value,"target":target,"levelDb":level_db,"pan":pan,"preFader":pre_fader});
    settings::prepare_add_send(q, id, &args)?.apply(q)?;
    Ok(args)
}
pub(super) fn set_send_expected(q: &mut Sequence, value: u64, slot: usize, changes: &Value) -> Result<Value> {
    if slot >= 256 || !valid_send_changes(changes) {
        return Err(error("observed bounded send and exact nonempty changes required"));
    }
    let id = strip(q, Some(value))?;
    let mut args = changes.clone();
    args["strip"] = json!(value);
    args["send"] = json!(slot);
    settings::prepare_set_send(q, id, &args)?.apply(q)?;
    Ok(args)
}
pub(super) fn remove_send_expected(q: &mut Sequence, value: u64, slot: usize) -> Result<Value> {
    if slot >= 256 {
        return Err(error("observed bounded send required"));
    }
    let id = strip(q, Some(value))?;
    let args = json!({"strip":value,"send":slot});
    settings::prepare_remove_send(id, &args)?.apply(q)?;
    Ok(args)
}
