//! Typed, read-ticket-scoped authored clip effects and native parameter definitions.
//! Existing optional persistent IDs are read, never allocated here. No evaluation,
//! resource opening, project migration, selection or history changes.
use std::collections::BTreeMap;

use filmcraft_project::{EffectInstance, Param, ParamDef, ParamKind, ParamValue};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{parse, sequences};
use crate::{Result, Session, commands::bad};

const COMMAND: &str = "agent.readFinishing";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    scope: sequences::Scope,
    identity: String,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
fn error(message: &str) -> crate::EngineError {
    bad(COMMAND, message)
}
fn charge(budget: &mut usize, n: usize) -> Result<()> {
    *budget = budget.checked_sub(n).ok_or_else(|| error("complete finishing page exceeds its bounded value budget; use a smaller page"))?;
    Ok(())
}
fn string(s: &str, max: usize, budget: &mut usize) -> Result<()> {
    if s.len() > max {
        return Err(error("finishing text exceeds its read bound; nothing was truncated"));
    }
    charge(budget, s.len().saturating_mul(6).saturating_add(32))
}
fn number(n: f64, budget: &mut usize) -> Result<()> {
    if !n.is_finite() {
        return Err(error("nonfinite finishing number"));
    }
    charge(budget, 32)
}
pub(super) fn value(v: &ParamValue, budget: &mut usize) -> Result<()> {
    charge(budget, 64)?;
    match v {
        ParamValue::Float(n) => number(*n, budget)?,
        ParamValue::Vec2(p) => {
            // Only native point parameters use NaN as authored automatic coordinates.
            for n in [p.x, p.y] {
                if n.is_nan() {
                    charge(budget, 4)?;
                } else {
                    number(n, budget)?;
                }
            }
        }
        ParamValue::Color(c) => {
            for n in c {
                number(f64::from(*n), budget)?;
            }
        }
        ParamValue::Bool(_) | ParamValue::Choice(_) => charge(budget, 16)?,
        ParamValue::Text(s) => string(s, 8192, budget)?,
        ParamValue::Curve(points) => {
            if points.len() > 2048 {
                return Err(error("curve exceeds the bounded complete-value read"));
            }
            for p in points {
                for n in p {
                    number(f64::from(*n), budget)?;
                }
            }
        }
        ParamValue::Path(path) => {
            if path.vertices.len() > 2048 {
                return Err(error("path exceeds the bounded complete-value read"));
            }
            for p in &path.vertices {
                charge(budget, 96)?;
                for p in [p.p, p.t_in, p.t_out] {
                    number(p.x, budget)?;
                    number(p.y, budget)?;
                }
            }
        }
    }
    Ok(())
}
fn parameter(p: &Param, budget: &mut usize) -> Result<()> {
    value(&p.value, budget)?;
    if p.keyframes.len() > 4096 {
        return Err(error("keyframes exceed the bounded complete-parameter read"));
    }
    let mut previous = None;
    for k in &p.keyframes {
        keyframe(k, &p.value, previous, budget)?;
        previous = Some(k.time);
    }
    Ok(())
}
pub(super) fn keyframe(k: &filmcraft_project::Keyframe, base: &ParamValue, previous: Option<filmcraft_time::Tick>, budget: &mut usize) -> Result<()> {
    if !(-SAFE..=SAFE).contains(&k.time.0) || previous.is_some_and(|t| t >= k.time) || std::mem::discriminant(&k.value) != std::mem::discriminant(base) {
        return Err(error("keyframe type, ordering or exact time is invalid"));
    }
    charge(budget, 256)?;
    number(k.out_influence, budget)?;
    number(k.in_influence, budget)?;
    value(&k.value, budget)
}
pub(super) fn definition(d: Option<&ParamDef>, budget: &mut usize) -> Result<Value> {
    let Some(d) = d else { return Ok(Value::Null) };
    charge(budget, 384)?;
    string(d.id, 256, budget)?;
    string(d.label, 1024, budget)?;
    if let Some(group) = d.group {
        string(group, 1024, budget)?;
    }
    value(&d.default, budget)?;
    match &d.kind {
        ParamKind::Float { min, max, soft_min, soft_max, unit, .. } => {
            for n in [min, max, soft_min, soft_max] {
                number(*n, budget)?;
            }
            string(unit, 128, budget)?;
        }
        ParamKind::Choice(options) => {
            if options.len() > 512 {
                return Err(error("choice definition exceeds its bound"));
            }
            for o in *options {
                string(o, 1024, budget)?;
            }
        }
        _ => {}
    }
    serde_json::to_value(d).map_err(|e| bad(COMMAND, e.to_string()))
}
fn param_json(p: &Param, origin: &str, def: Value) -> Value {
    json!({"origin":origin,"value":p.value,"keyframes":p.keyframes,"definition":def})
}
pub(super) fn effect(e: &EffectInstance, ordinal: usize, budget: &mut usize) -> Result<Value> {
    string(&e.effect, 256, budget)?;
    if e.effect.is_empty() || e.params.len() > 256 || e.masks.len() > 32 {
        return Err(error("effect parameter/mask bounds exceeded"));
    }
    charge(budget, 512)?;
    let def = e.def();
    if def.is_some_and(|d| d.params.len() > 256) {
        return Err(error("effect definition parameter bounds exceeded"));
    }
    // A saved instance can predate newly defined defaults. Return those defaults
    // explicitly, without adding parameters or resolving automatic points in it.
    let mut params: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(def) = def {
        for d in &def.params {
            if !e.params.contains_key(d.id) {
                string(d.id, 256, budget)?;
                let p = Param::new(d.default.clone());
                parameter(&p, budget)?;
                let def = definition(Some(d), budget)?;
                params.insert(d.id.into(), param_json(&p, "definition_default", def));
            }
        }
    }
    for (id, p) in &e.params {
        if id.is_empty() {
            return Err(error("empty parameter ID"));
        }
        string(id, 256, budget)?;
        parameter(p, budget)?;
        let definition = definition(def.and_then(|d| d.param(id)), budget)?;
        params.insert(id.clone(), param_json(p, "authored", definition));
    }
    if params.len() > 256 {
        return Err(error("combined authored/default parameter bounds exceeded"));
    }
    let mut masks = Vec::with_capacity(e.masks.len());
    for (index, m) in e.masks.iter().enumerate() {
        string(&m.name, 1024, budget)?;
        charge(budget, 512)?;
        if !matches!(m.path.value, ParamValue::Path(_)) || [&m.feather, &m.opacity, &m.expansion].iter().any(|p| !matches!(p.value, ParamValue::Float(_))) {
            return Err(error("mask parameter type is invalid"));
        }
        for p in [&m.path, &m.feather, &m.opacity, &m.expansion] {
            parameter(p, budget)?;
        }
        let mut params = BTreeMap::new();
        for id in filmcraft_project::mask::MASK_PARAMS {
            let p = m.param(id).ok_or_else(|| error("missing native mask parameter"))?;
            let def = definition(filmcraft_project::mask::mask_param_defs().iter().find(|d| d.id == id), budget)?;
            params.insert(id, param_json(p, "authored", def));
        }
        masks.push(json!({"ordinal":index+1,"instance_id":m.instance_id,"name":m.name,"inverted":m.inverted,"mode":m.mode,"track_method":m.track_method,"params":params}));
    }
    Ok(json!({"ordinal":ordinal,"instance_id":e.instance_id,"effect":e.effect,"enabled":e.enabled,"post_fader":e.post_fader,"essential":e.essential,
        "graphic_layer_present":e.layer.is_some(),"params":params,"masks":masks}))
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse(COMMAND, p)?;
    if !filmcraft_project::ClipIdentity::valid_id(&r.identity) || r.offset > 4096 || r.limit == 0 || r.limit > 8 {
        return Err(error("explicit persistent clip and bounded complete effect page required"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &r.scope)? {
        return Err(error("sequence is outside this explicit workspace"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut clip = None;
    let mut count = 0usize;
    for c in q.all_tracks().flat_map(|t| &t.items) {
        count = count.saturating_add(1);
        if count > 100_000 {
            return Err(error("clip inventory exceeds bounded finishing lookup"));
        }
        if c.identity.as_ref().is_some_and(|i| i.id == r.identity) && clip.replace(c).is_some() {
            return Err(error("ambiguous clip identity"));
        }
    }
    let clip = clip.ok_or_else(|| error("current clip identity not found"))?;
    if super::assembly::binding(s, clip.item).map_err(|_| error("clip requires this film's explicit original binding"))?.project_id != r.scope.project_id {
        return Err(error("clip belongs to another original owner"));
    }
    if clip.effects.len() > 4096 || r.offset > clip.effects.len() {
        return Err(error("effect stack/offset exceeds the bounded read"));
    }
    let end = r.offset.saturating_add(r.limit).min(clip.effects.len());
    let mut budget = 96 * 1024usize;
    let effects = clip
        .effects
        .get(r.offset..end)
        .ok_or_else(|| error("invalid effect page"))?
        .iter()
        .enumerate()
        .map(|(i, e)| effect(e, r.offset + i + 1, &mut budget))
        .collect::<Result<Vec<_>>>()?;
    let context = super::finish::context(s, &r.identity, &mut budget)?;
    let mut result = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":sequence.0,"scope":r.scope,
        "identity":r.identity,"offset":r.offset,"total":clip.effects.len(),"nextOffset":if end<clip.effects.len(){Some(end)}else{None},
        "effects":effects,"clipGainDb":context["gainDb"],"transitions":context["transitions"],"timeDomain":"native_source_ticks","referenceScope":"exact_read_ticket_only",
        "effectTracking":s.project.item(sequence).is_some_and(|i|i.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v|v=="1"))});
    if r.projection {
        result["requiresSourceValidation"] = json!(true);
    }
    if serde_json::to_vec(&result).map_err(|e| bad(COMMAND, e.to_string()))?.len() > 128 * 1024 {
        return Err(error("complete finishing page exceeds wire budget; nothing was truncated"));
    }
    Ok(result)
}
