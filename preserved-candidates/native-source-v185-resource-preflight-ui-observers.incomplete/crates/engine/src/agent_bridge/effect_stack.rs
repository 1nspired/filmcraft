//! Exact clip-stack gestures over the same services used by human controls.
//! Reads are documentary: no resources, clocks, files or identity allocation.
use super::{Operation, parse, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, ClipIdentity, EffectDef, EffectInstance, EffectKind, ParamValue, TrackKind};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.editEffectStack";
const MAX_EFFECTS: usize = 4096;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
pub(super) fn digest(effects: &[EffectInstance]) -> Result<String> {
    if effects.len() > MAX_EFFECTS {
        return Err(error("effect stack exceeds 4096 instances"));
    }
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(&effects)?)))
}
pub(super) fn reason(def: &EffectDef) -> Option<&'static str> {
    if def.intrinsic {
        Some("fixed_effect")
    } else if filmcraft_project::graphic::is_layer_id(def.id) {
        Some("graphic_layer")
    } else if !matches!(def.kind, EffectKind::Video | EffectKind::Audio) {
        Some("transition")
    } else if def.params.iter().any(|p| matches!(&p.default, ParamValue::Text(s) if !s.is_empty())) {
        Some("text_or_resource_defaults")
    } else {
        None
    }
}
fn catalog_header(def: &EffectDef) -> Value {
    json!({"definition":def.id,"name":def.name,"kind":def.kind,"category":def.category,
        "intrinsic":def.intrinsic,"accelerated":def.accelerated,"float32":def.float32,
        "addable":reason(def).is_none(),"unavailableReason":reason(def)})
}
fn header(effect: &EffectInstance) -> Value {
    json!({"instanceId":effect.instance_id,"definition":effect.effect,"enabled":effect.enabled,
        "intrinsic":effect.def().map(|d|d.intrinsic),
        "graphicLayer":effect.layer.is_some() || filmcraft_project::graphic::is_layer(effect)})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogRead {
    token: String,
    scope: sequences::Scope,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StackRead {
    token: String,
    scope: sequences::Scope,
    identity: String,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
fn read_scope(s: &mut Session, token: &str, scope: &sequences::Scope, projection: bool, command: &str) -> Result<()> {
    super::validate_metadata_read(s, token, projection, command)?;
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, scope)? {
        return Err(error("sequence is outside the explicit workspace"));
    }
    Ok(())
}
pub(super) fn read_catalog(s: &mut Session, params: &Value) -> Result<Value> {
    let r: CatalogRead = parse("agent.readEffectCatalog", params)?;
    let defs = filmcraft_project::effect::effect_defs();
    if defs.len() > 1024 || r.offset > defs.len() || !(1..=32).contains(&r.limit) {
        return Err(error("bounded explicit catalog page required"));
    }
    read_scope(s, &r.token, &r.scope, r.projection, "agent.readEffectCatalog")?;
    let end = r.offset.saturating_add(r.limit).min(defs.len());
    let rows = defs.get(r.offset..end).ok_or_else(|| error("invalid catalog page"))?.iter().map(catalog_header).collect::<Vec<_>>();
    let mut result = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"scope":r.scope,
        "offset":r.offset,"total":defs.len(),"nextOffset":if end<defs.len(){Some(end)}else{None},"effects":rows,"referenceScope":"exact_read_ticket_only"});
    if r.projection {
        result["requiresSourceValidation"] = json!(true);
    }
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("catalog page exceeds wire budget; use a smaller page"));
    }
    Ok(result)
}
pub(super) fn read_stack(s: &mut Session, params: &Value) -> Result<Value> {
    let r: StackRead = parse("agent.readEffectStack", params)?;
    if !ClipIdentity::valid_id(&r.identity) || !(1..=32).contains(&r.limit) || r.offset > MAX_EFFECTS {
        return Err(error("persistent clip and bounded stack page required"));
    }
    read_scope(s, &r.token, &r.scope, r.projection, "agent.readEffectStack")?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("clip inventory exceeds stack lookup bound"));
    }
    let mut matches = q.all_tracks().flat_map(|t| &t.items).filter(|c| c.identity.as_ref().is_some_and(|i| i.id == r.identity));
    let c = matches.next().ok_or_else(|| error("current clip identity absent"))?;
    if matches.next().is_some() || super::assembly::binding(s, c.item)?.project_id != r.scope.project_id {
        return Err(error("ambiguous or foreign clip identity"));
    }
    let sha = digest(&c.effects)?;
    if r.offset > c.effects.len() {
        return Err(error("stack page offset is outside current stack"));
    }
    let end = r.offset.saturating_add(r.limit).min(c.effects.len());
    let effects = c.effects.get(r.offset..end).ok_or_else(|| error("invalid stack page"))?.iter().map(header).collect::<Vec<_>>();
    let tracking = s
        .state
        .active_sequence
        .and_then(|id| s.project.item(id))
        .is_some_and(|i| i.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v| v == "1"));
    let mut result = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"scope":r.scope,
        "identity":r.identity,"sha256":sha,"effectTracking":tracking,"offset":r.offset,"total":c.effects.len(),
        "nextOffset":if end<c.effects.len(){Some(end)}else{None},"effects":effects,"referenceScope":"exact_read_ticket_only"});
    if r.projection {
        result["requiresSourceValidation"] = json!(true);
    }
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("stack page exceeds wire budget; use a smaller page"));
    }
    Ok(result)
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
enum Edit {
    Add { definition: String },
    Remove { effect_id: String },
    SetEnabled { effect_id: String, enabled: bool },
    Reorder { effect_id: String, before_effect_id: Option<String> },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    clip: u64,
    identity: String,
    before_stack_sha256: String,
    scope: sequences::Scope,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    if r.clip == 0 || r.clip > 9_007_199_254_740_991 || !ClipIdentity::valid_id(&r.identity) || !hash(&r.before_stack_sha256) {
        return Err(error("explicit current clip, persistent identity and complete stack digest required"));
    }
    let valid = match &r.edit {
        Edit::Add { definition } => !definition.is_empty() && definition.len() <= 256,
        Edit::Remove { effect_id } | Edit::SetEnabled { effect_id, .. } => ClipIdentity::valid_id(effect_id),
        Edit::Reorder { effect_id, before_effect_id } => {
            ClipIdentity::valid_id(effect_id)
                && before_effect_id.as_ref().is_none_or(|id| ClipIdentity::valid_id(id) && id != effect_id)
                && op.params.get("edit").is_some_and(|v| v.get("beforeEffectId").is_some())
        }
    };
    if !valid {
        return Err(error("bounded exact definition/instance identities and explicit reorder destination required"));
    }
    Ok(())
}
fn index(effects: &[EffectInstance], identity: &str) -> Result<usize> {
    let position = effects.iter().position(|e| e.instance_id.as_deref() == Some(identity)).ok_or_else(|| error("effect instance is absent"))?;
    let e = effects.get(position).ok_or_else(|| error("effect position missing"))?;
    if e.layer.is_some() || filmcraft_project::graphic::is_layer(e) {
        return Err(error("graphic layers require their own authoring family"));
    }
    if e.def().is_none() {
        return Err(error("unknown effect definition cannot be authored"));
    }
    Ok(position)
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)?
        || s.project.items.len() > 100_000
        || s.project.item(sid).is_none_or(|i| i.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_none_or(|v| v != "1"))
    {
        return Err(error("explicit workspace and persistent effect tracking required"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("clip inventory exceeds stack edit bound"));
    }
    let (track, c) = q.find_item(ClipId(r.clip)).ok_or_else(|| error("current clip absent"))?;
    let track = q.track(track).ok_or_else(|| error("target track absent"))?;
    if c.identity.as_ref().is_none_or(|i| i.id != r.identity || i.protection.is_some())
        || track.locked
        || super::assembly::binding(s, c.item)?.project_id != r.scope.project_id
    {
        return Err(error("effect target is locked, protected, untracked or foreign"));
    }
    if digest(&c.effects)? != r.before_stack_sha256 {
        return Err(error("complete effect stack changed since read"));
    }
    let mut ids = BTreeSet::new();
    if c.effects.iter().any(|e| e.instance_id.as_ref().is_none_or(|id| !ClipIdentity::valid_id(id) || !ids.insert(id.clone()))) {
        return Err(error("all effect identities must be valid and unique"));
    }
    // Serialize the complete project before cloning. Oversized authored data
    // refuses without consuming the caller's real history or Redo branch.
    project_audit::bytes(&s.project)?;
    let mut expected = (*s.project).clone();
    let mut next = c.effects.clone();
    let count = next.len();
    let kind = track.kind;
    let frame = (q.settings.width, q.settings.height);
    let (command, args, changed, added_position) = match &r.edit {
        Edit::Add { definition } => {
            if next.len() >= MAX_EFFECTS {
                return Err(error("effect stack is full"));
            }
            let def =
                filmcraft_project::find_effect(definition).filter(|d| d.id == definition).ok_or_else(|| error("exact native effect definition required"))?;
            if reason(def).is_some() || !matches!((kind, def.kind), (TrackKind::Video, EffectKind::Video) | (TrackKind::Audio, EffectKind::Audio)) {
                return Err(error("effect cannot be added to this explicit track kind"));
            }
            let mut instance = def.instance();
            filmcraft_project::resolve_auto_points(&mut instance, frame, frame);
            let pos = if kind == TrackKind::Audio { next.len() } else { next.iter().position(|e| e.def().is_some_and(|d| d.intrinsic)).unwrap_or(next.len()) };
            next.insert(pos, instance);
            ("effects.apply", json!({"clips":[r.clip],"effect":def.id}), None, Some(pos))
        }
        Edit::Remove { effect_id } => {
            let pos = index(&next, effect_id)?;
            if next.get(pos).and_then(|e| e.def()).is_some_and(|d| d.intrinsic) {
                return Err(error("fixed effects cannot be removed"));
            }
            next.remove(pos);
            ("effects.remove", json!({"clip":r.clip,"index":pos}), Some(effect_id.clone()), None)
        }
        Edit::SetEnabled { effect_id, enabled } => {
            let pos = index(&next, effect_id)?;
            let e = next.get_mut(pos).ok_or_else(|| error("target effect missing"))?;
            if e.enabled == *enabled {
                return Err(error("effect already has requested enabled state"));
            }
            e.enabled = *enabled;
            ("effects.toggleEnabled", json!({"clip":r.clip,"index":pos}), Some(effect_id.clone()), None)
        }
        Edit::Reorder { effect_id, before_effect_id } => {
            let from = index(&next, effect_id)?;
            let to = if let Some(before) = before_effect_id {
                let at = next.iter().position(|e| e.instance_id.as_ref() == Some(before)).ok_or_else(|| error("destination effect absent"))?;
                if at > from { at - 1 } else { at }
            } else {
                next.iter().enumerate().skip(from + 1).find(|(_, e)| e.def().is_some_and(|d| d.intrinsic)).map_or(next.len() - 1, |(i, _)| i - 1)
            };
            if next.get(from.min(to)..=from.max(to)).is_none_or(|span| span.iter().any(|e| e.layer.is_some() || filmcraft_project::graphic::is_layer(e))) {
                return Err(error("graphic-layer ordering needs its own family"));
            }
            next = crate::effect_stack::reordered(&next, from, to)?;
            ("effects.reorder", json!({"clip":r.clip,"from":from,"to":to}), Some(effect_id.clone()), None)
        }
    };
    s.execute(command, args)?;
    let actual = &s.active_sequence().ok_or(EngineError::NoSequence)?.find_item(ClipId(r.clip)).ok_or_else(|| error("target clip disappeared"))?.1.effects;
    let changed = if let Some(position) = added_position {
        let id = actual
            .get(position)
            .and_then(|e| e.instance_id.as_ref())
            .filter(|id| ClipIdentity::valid_id(id) && !ids.contains(*id))
            .ok_or_else(|| error("new effect identity missing or reused"))?
            .clone();
        next.get_mut(position).ok_or_else(|| error("expected new effect absent"))?.instance_id = Some(id.clone());
        Some(id)
    } else {
        changed
    };
    let after_sha = digest(&next)?;
    let after_count = next.len();
    expected.sequence_mut(sid).and_then(|q| q.find_item_mut(ClipId(r.clip))).ok_or_else(|| error("expected clip missing"))?.1.effects = next;
    if project_audit::bytes(&s.project)? != project_audit::bytes(&expected)? {
        return Err(error("human effect gesture changed unexpected project state; staging discarded"));
    }
    Ok(json!({"identity":r.identity,"beforeStackSha256":r.before_stack_sha256,"afterStackSha256":after_sha,
        "beforeCount":count,"afterCount":after_count,"changedEffectId":changed}))
}
