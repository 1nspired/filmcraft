//! One exact existing-control parameter gesture. Resolves persistent local IDs,
//! then calls the human effects.setParam; no alternate authoring implementation.
use super::{Operation, finishing, parse, project_audit::bytes, sequences};
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, ClipIdentity, Param, ParamValue};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) const COMMAND: &str = "agent.setParameter";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    clip: u64,
    identity: String,
    effect_id: String,
    mask_id: Option<String>,
    param: String,
    value: Value,
    frame: i64,
    keyframe: bool,
    scope: sequences::Scope,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
fn fields(v: &Value, required: &[&str], optional: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|o| required.iter().all(|k| o.contains_key(*k)) && o.keys().all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str())))
}
fn point(v: &Value) -> bool {
    fields(v, &["x", "y"], &[]) && ["x", "y"].iter().all(|k| v[*k].as_f64().is_some_and(f64::is_finite))
}
fn numbers(v: &Value, depth: usize, budget: &mut usize) -> bool {
    if depth > 16 || *budget == 0 {
        return false;
    }
    *budget -= 1;
    match v {
        Value::Number(n) => n.as_f64().is_some_and(|x| x.is_finite() && (x.fract() != 0.0 || x.abs() <= SAFE as f64)),
        Value::Array(a) => a.iter().all(|v| numbers(v, depth + 1, budget)),
        Value::Object(o) => o.values().all(|v| numbers(v, depth + 1, budget)),
        _ => true,
    }
}
fn typed(v: &Value) -> Result<ParamValue> {
    if v.as_object().is_none_or(|o| o.len() != 1) || !numbers(v, 0, &mut 16384) {
        return Err(error("need one bounded exact tagged native value"));
    }
    let parsed: ParamValue = serde_json::from_value(v.clone()).map_err(|e| error(e.to_string()))?;
    let shape = match &parsed {
        ParamValue::Text(_) => false, // File/resource/text authoring needs its own admission contract.
        ParamValue::Vec2(_) => point(&v["Vec2"]),
        ParamValue::Color(_) => v["Color"].as_array().is_some_and(|a| a.len() == 4),
        ParamValue::Curve(_) => v["Curve"].as_array().is_some_and(|a| a.len() <= 2048 && a.iter().all(|p| p.as_array().is_some_and(|p| p.len() == 2))),
        ParamValue::Path(_) => {
            fields(&v["Path"], &["closed", "vertices"], &[])
                && v["Path"]["vertices"].as_array().is_some_and(|a| {
                    a.len() <= 2048
                        && a.iter()
                            .all(|p| fields(p, &["p"], &["t_in", "t_out"]) && point(&p["p"]) && ["t_in", "t_out"].iter().all(|k| p.get(*k).is_none_or(point)))
                })
        }
        _ => true,
    };
    if !shape {
        return Err(error("malformed/unsupported literal; text/resources and automatic-point reset need separate tools"));
    }
    finishing::value(&parsed, &mut (64 * 1024))?;
    Ok(parsed)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if !fields(&op.params, &["clip", "identity", "effectId", "maskId", "param", "value", "frame", "keyframe", "scope"], &[])
        || p.clip == 0
        || p.clip > SAFE as u64
        || !ClipIdentity::valid_id(&p.identity)
        || !ClipIdentity::valid_id(&p.effect_id)
        || p.mask_id.as_ref().is_some_and(|m| !ClipIdentity::valid_id(m))
        || p.param.is_empty()
        || p.param.len() > 256
        || !(0..=1_000_000).contains(&p.frame)
    {
        return Err(error("explicit clip/effect/mask identity, parameter, sequence frame, keyframe mode and workspace required"));
    }
    if serde_json::to_vec(&p.value).map_err(|e| error(e.to_string()))?.len() > 64 * 1024 {
        return Err(error("parameter literal exceeds budget"));
    }
    typed(&p.value)?;
    Ok(())
}
fn literal(v: &ParamValue) -> Result<Value> {
    Ok(match v {
        ParamValue::Float(x) => json!(x),
        ParamValue::Choice(x) => json!(x),
        ParamValue::Bool(x) => json!(x),
        ParamValue::Vec2(x) => json!([x.x, x.y]),
        ParamValue::Color(x) => json!(x),
        ParamValue::Curve(x) => json!(x),
        ParamValue::Path(x) => serde_json::to_value(x).map_err(|e| error(e.to_string()))?,
        ParamValue::Text(_) => return Err(error("text/resource authoring is not supported")),
    })
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let requested = typed(&p.value)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    let sid = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &p.scope)?
        || s.project.items.len() > 100_000
        || s.project.item(sid).is_none_or(|i| i.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_none_or(|v| v != "1"))
    {
        return Err(error("explicitly enable persistent effect tracking in this workspace before editing"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("parameter clip audit exceeds its bound"));
    }
    let (track, c) = q.find_item(ClipId(p.clip)).ok_or_else(|| error("missing current clip"))?;
    if c.identity.as_ref().is_none_or(|i| i.id != p.identity || i.protection.is_some())
        || q.track(track).is_none_or(|t| t.locked)
        || super::assembly::binding(s, c.item)?.project_id != p.scope.project_id
        || s.project.item(c.item).and_then(|i| i.as_media()).is_none_or(|m| m.offline)
        || s.media.offline_status(c.item).is_some()
    {
        return Err(error("parameter target is locked, protected, offline, untracked or foreign"));
    }
    let rate = q.settings.frame_rate;
    let end = c.start.0.checked_add(c.duration.0).filter(|n| (0..=SAFE).contains(n)).ok_or_else(|| error("unsafe clip end"))?;
    let span = c.duration.0 as f64 * c.speed.abs();
    if rate.num <= 0
        || rate.den <= 0
        || !(0..=SAFE).contains(&c.start.0)
        || !(1..=SAFE).contains(&c.duration.0)
        || !(0..=SAFE).contains(&c.source_in.0)
        || !c.speed.is_finite()
        || c.speed <= 0.0
        || !span.is_finite()
        || span < 1.0
        || span > (SAFE - c.source_in.0) as f64
        || c.frame_hold.is_some_and(|h| !(-SAFE..=SAFE).contains(&h.0))
    {
        return Err(error("unsafe native clip clock"));
    }
    let numerator = filmcraft_time::TICKS_PER_SECOND as i128 * rate.den as i128;
    if numerator % rate.num as i128 != 0 {
        return Err(error("sequence frame rate has no exact integer-tick frame"));
    }
    let at = rate.frame_duration().0.checked_mul(p.frame).filter(|n| (0..=SAFE).contains(n)).map(Tick).ok_or_else(|| error("unsafe sequence time"))?;
    if rate.frame_duration() <= Tick::ZERO || at < c.start || at.0 >= end {
        return Err(error("explicit sequence frame must be inside the current clip"));
    }
    let mt = c.effect_time_at(at);
    if !(-SAFE..=SAFE).contains(&mt.0) {
        return Err(error("unsafe native source time"));
    }
    let index = c.effects.iter().position(|e| e.instance_id.as_ref() == Some(&p.effect_id)).ok_or_else(|| error("current effect identity is absent"))?;
    let old = c.effects.get(index).ok_or_else(|| error("missing effect"))?;
    if old.layer.is_some() || filmcraft_project::graphic::is_layer(old) {
        return Err(error("graphic-layer authoring requires its own contract"));
    }
    let mask = p
        .mask_id
        .as_ref()
        .map(|id| old.masks.iter().position(|m| m.instance_id.as_ref() == Some(id)).ok_or_else(|| error("current mask identity is absent")))
        .transpose()?;
    let def =
        if mask.is_some() { filmcraft_project::mask::mask_param_defs().iter().find(|d| d.id == p.param) } else { old.def().and_then(|d| d.param(&p.param)) }
            .ok_or_else(|| error("unknown native parameter definition"))?;
    if std::mem::discriminant(&requested) != std::mem::discriminant(&def.default) || p.keyframe && !def.animatable {
        return Err(error("wrong parameter type or nonanimatable native control"));
    }
    let val = literal(&requested)?;
    if crate::commands::effect_param_value(def, &val, COMMAND)? != requested {
        return Err(error("parameter would be clamped; no edit published"));
    }
    let mut expected_effect = old.clone();
    let target = if let Some(i) = mask {
        expected_effect.masks.get_mut(i).and_then(|m| m.param_mut(&p.param)).ok_or_else(|| error("missing mask parameter"))?
    } else {
        expected_effect.params.entry(p.param.clone()).or_insert_with(|| Param::new(def.default.clone()))
    };
    if p.keyframe {
        target.put_keyframe(mt, requested.clone());
    } else {
        target.set_at(mt, requested.clone());
    }
    // This native control is a preset, not an unrelated one-field write. Keep
    // the human preset's documented coupled settings/automation behavior.
    if mask.is_none()
        && old.effect == "ultra_key"
        && p.param == "setting"
        && let ParamValue::Choice(setting) = requested
        && let Some(rows) = filmcraft_project::effect::ultra_key_setting(setting)
    {
        for (id, value) in rows {
            let x = expected_effect.params.entry((*id).into()).or_insert_with(|| Param::new(ParamValue::Float(*value)));
            x.value = ParamValue::Float(*value);
            x.keyframes.clear();
        }
    }
    let expected_read = finishing::effect(&expected_effect, index + 1, &mut (96 * 1024))?;
    let before_read = finishing::effect(old, index + 1, &mut (96 * 1024))?;
    if expected_read == before_read {
        return Err(error("parameter already has the requested authored state"));
    }
    let mut expected = (*s.project).clone();
    let dst = expected.sequence_mut(sid).and_then(|q| q.find_item_mut(ClipId(p.clip))).ok_or_else(|| error("missing expected clip"))?.1;
    *dst.effects.get_mut(index).ok_or_else(|| error("missing expected effect"))? = expected_effect;
    let expected_bytes = bytes(&expected)?;
    let mut args = json!({"clip":p.clip,"effect":index,"param":p.param,"value":val,"time":at.0,"keyframe":p.keyframe,"merge":false,"begin":false});
    if let Some(i) = mask {
        args["mask"] = json!(i);
    }
    s.execute("effects.setParam", args)?;
    if bytes(&s.project)? != expected_bytes {
        return Err(error("native parameter gesture changed unexpected state; staging discarded"));
    }
    Ok(json!({"identity":p.identity,"effectId":p.effect_id,"maskId":p.mask_id,"param":p.param,"sequenceTime":at.0,"sourceTime":mt.0,
        "effect":expected_read}))
}
