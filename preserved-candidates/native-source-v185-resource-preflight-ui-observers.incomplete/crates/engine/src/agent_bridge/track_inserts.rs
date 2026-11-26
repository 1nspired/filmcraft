//! Same-plan mixer insert inspection and shared human stored-base edits.
use super::{finishing, parameter, parse, project_audit, sequences, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{EffectInstance, EffectKind, Sequence, TrackId, mixer::MASTER_STRIP};
use serde::Deserialize;
use serde_json::{Map, Value, json};

fn error(m: &str) -> EngineError {
    bad(tracks::COMMAND, m)
}
fn identity(id: u64) -> bool {
    id > 0 && id <= 9_007_199_254_740_991
}
fn effects(q: &Sequence, strip: Option<u64>) -> Result<&[EffectInstance]> {
    let values = match strip {
        None => &q.master_effects,
        Some(id) => &q.mix_track(TrackId(id)).ok_or_else(|| error("read an existing audio or submix strip"))?.effects,
    };
    if values.len() > 4096 {
        return Err(error("mixer insert stack exceeds 4096"));
    }
    Ok(values)
}
pub(super) fn valid_changes(value: &Value) -> bool {
    let Some(o) = value.as_object().filter(|o| !o.is_empty() && o.len() <= 3) else { return false };
    if !serde_json::to_vec(value).is_ok_and(|v| v.len() <= 64 * 1024) {
        return false;
    }
    o.iter().all(|(key, v)| match key.as_str() {
        "enabled" | "postFader" => v.is_boolean(),
        "params" => v.as_object().is_some_and(|p| {
            !p.is_empty() && p.len() <= 32 && p.iter().all(|(id, v)| !id.is_empty() && id.len() <= 256 && !id.contains('\0') && parameter::typed(v).is_ok())
        }),
        _ => false,
    })
}
pub(super) fn prepare(q: &Sequence, strip: Option<u64>, slot: usize, changes: &Value) -> Result<()> {
    if !strip.is_none_or(identity) || !valid_changes(changes) {
        return Err(error("explicit bounded typed insert changes required"));
    }
    if strip.and_then(|id| q.mix_track(TrackId(id))).is_some_and(|t| t.locked) {
        return Err(error("unlock the target strip explicitly before insert edits"));
    }
    let e = effects(q, strip)?.get(slot).ok_or_else(|| error("insert slot is absent in this exact read"))?;
    let def = e.def().filter(|d| d.kind == EffectKind::Audio).ok_or_else(|| error("known native audio insert definition required"))?;
    if e.layer.is_some() || !e.masks.is_empty() {
        return Err(error("mixer insert layers and masks require their own contract"));
    }
    if let Some(params) = changes.get("params").and_then(Value::as_object) {
        for (id, value) in params {
            let requested = parameter::typed(value)?;
            let pd = def.param(id).ok_or_else(|| error("unknown insert parameter"))?;
            let literal = parameter::literal(&requested)?;
            if crate::commands::effect_param_value(pd, &literal, tracks::COMMAND)? != requested {
                return Err(error("insert parameter would be clamped; no change published"));
            }
        }
    }
    Ok(())
}
pub(super) fn apply_expected(q: &mut Sequence, strip: Option<u64>, slot: usize, changes: &Value) -> Result<Value> {
    prepare(q, strip, slot, changes)?;
    let mut expected = effects(q, strip)?.get(slot).ok_or_else(|| error("missing expected insert"))?.clone();
    let mut args = json!({"strip":strip.map_or_else(||json!("Mix"),|id|json!(id)),"slot":slot});
    for key in ["enabled", "postFader"] {
        if let Some(v) = changes.get(key) {
            let value = v.as_bool().ok_or_else(|| error("explicit insert boolean required"))?;
            if key == "enabled" {
                expected.enabled = value;
            } else {
                expected.post_fader = value;
            }
            args[key] = json!(value);
        }
    }
    if let Some(values) = changes.get("params").and_then(Value::as_object) {
        let mut params = Map::new();
        for (id, value) in values {
            params.insert(id.clone(), parameter::literal(&parameter::typed(value)?)?);
        }
        let params = Value::Object(params);
        // The exact same native conversion preserves every existing keyframe.
        crate::commands::set_effect_params(&mut expected, &params, tracks::COMMAND)?;
        args["params"] = params;
    }
    q.with_strip_mut(strip.map(TrackId).unwrap_or(MASTER_STRIP), |t| -> Result<()> {
        *t.effects.get_mut(slot).ok_or_else(|| error("expected insert disappeared"))? = expected;
        Ok(())
    })
    .ok_or_else(|| error("expected strip disappeared"))??;
    Ok(args)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    strip: Option<u64>,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readTrackInserts", p)?;
    if p.get("strip").is_none() || !r.strip.is_none_or(identity) || r.offset > 4096 || !(1..=4).contains(&r.limit) {
        return Err(error("explicit strip and bounded insert page required"));
    }
    if r.projection {
        #[cfg(target_arch = "wasm32")]
        return Err(error("insert projection requires native source qualification"));
        #[cfg(not(target_arch = "wasm32"))]
        super::validate_read_basis(s, &r.token)?;
    } else {
        super::validate_snapshot(s, &json!({"token":r.token}))?;
    }
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("insert read is outside its original workspace"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let sha = tracks::digest(q)?;
    let inserts = effects(q, r.strip)?;
    if r.offset > inserts.len() {
        return Err(error("insert offset exceeds complete stack"));
    }
    let end = r.offset.saturating_add(r.limit).min(inserts.len());
    let mut page = Vec::new();
    let mut budget = 96 * 1024;
    for (index, e) in inserts.get(r.offset..end).ok_or_else(|| error("invalid insert page"))?.iter().enumerate() {
        page.push(finishing::effect(e, r.offset + index + 1, &mut budget)?);
    }
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sid.0,"sha256":sha,"strip":r.strip,
        "offset":r.offset,"total":inserts.len(),"nextOffset":if end<inserts.len(){Some(end)}else{None},"effects":page,
        "timeDomain":"native_sequence_ticks","referenceScope":"exact_read_ticket_only","requiresSourceValidation":r.projection});
    if project_audit::value_bytes(&result)?.len() > 128 * 1024 {
        return Err(error("insert page exceeds 128KiB; reduce page size"));
    }
    Ok(result)
}

fn unlocked_effects(q: &Sequence, strip: Option<u64>) -> Result<&[EffectInstance]> {
    if !strip.is_none_or(identity) {
        return Err(error("explicit safe strip or null master required"));
    }
    if strip.and_then(|id| q.mix_track(TrackId(id))).is_some_and(|t| t.locked) {
        return Err(error("unlock the target strip explicitly before changing insert structure"));
    }
    effects(q, strip)
}
pub(super) fn add_expected(q: &mut Sequence, strip: Option<u64>, slot: usize, definition: &str, post_fader: bool) -> Result<Value> {
    let stack = unlocked_effects(q, strip)?;
    if stack.len() >= filmcraft_project::mixer::MAX_INSERTS || slot > stack.len() {
        return Err(error("native insert capacity or exact insertion position exceeded; no clamping"));
    }
    let def = filmcraft_project::find_effect(definition)
        .filter(|d| d.kind == EffectKind::Audio && super::effect_stack::reason(d).is_none() && filmcraft_render::audio_fx::supported(d.id))
        .ok_or_else(|| error("read a native DSP-backed addable audio definition; resources require separate admission"))?;
    let mut effect = def.instance();
    effect.post_fader = post_fader;
    q.with_strip_mut(strip.map(TrackId).unwrap_or(MASTER_STRIP), |t| -> Result<()> {
        if slot > t.effects.len() {
            return Err(error("expected exact insert position changed"));
        }
        t.effects.insert(slot, effect);
        Ok(())
    })
    .ok_or_else(|| error("expected insertion strip disappeared"))??;
    Ok(json!({"strip":strip.map_or_else(||json!("Mix"),|id|json!(id)),"slot":slot,"effect":definition,"postFader":post_fader}))
}
pub(super) fn remove_expected(q: &mut Sequence, strip: Option<u64>, slot: usize) -> Result<Value> {
    unlocked_effects(q, strip)?.get(slot).ok_or_else(|| error("read an existing insert before removing it"))?;
    // Use the same native rack operation as mixer.removeInsert: this removes
    // the effect's owned parameter animation with it, preserving the others.
    let removed = q.with_strip_mut(strip.map(TrackId).unwrap_or(MASTER_STRIP), |t| t.remove_insert(slot));
    if removed != Some(true) {
        return Err(error("expected removal strip or slot disappeared"));
    }
    Ok(json!({"strip":strip.map_or_else(||json!("Mix"),|id|json!(id)),"slot":slot}))
}
