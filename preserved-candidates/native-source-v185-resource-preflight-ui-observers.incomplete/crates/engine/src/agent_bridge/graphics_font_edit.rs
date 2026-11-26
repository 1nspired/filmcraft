//! Bind one exact human font gesture to current native renderer observations.
//! These hashes authorize no source-file access, embedding, or copied grant.
use super::{Edit, Request, character_edit, error, font_resources, project_audit};
use crate::{Result, Session};
use filmcraft_project::{EffectInstance, TrackItem, graphic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FontPlan {
    version: u32,
    pub(super) read_token: String,
    proposal_sha256: String,
    before_sha256: String,
    after_sha256: String,
}
impl FontPlan {
    pub(super) fn valid(&self) -> bool {
        self.version == 1
            && !self.read_token.is_empty()
            && super::text(&self.read_token, 128)
            && [&self.proposal_sha256, &self.before_sha256, &self.after_sha256].iter().all(|v| super::hash(v))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    token: String,
    request: Value,
    #[serde(default)]
    projection: bool,
    #[serde(default)]
    canonical: bool,
}
fn digest(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
fn proposed(r: &Request, token: &str) -> Result<String> {
    if let Some(shared) = &r.shared_source {
        return digest(&("native_shared_graphics_font_proposal_v1", token, &r.scope, &r.before_graphics_sha256, &r.edit, shared));
    }
    digest(&("native_graphics_font_proposal_v1", token, &r.scope, &r.before_graphics_sha256, &r.edit))
}
pub(super) fn eligible(edit: &Edit) -> bool {
    match edit {
        Edit::Set { props, .. } => props.as_object().is_some_and(|p| p.contains_key("font") || p.contains_key("font_style")),
        Edit::StyleRange { clear, style, .. } => {
            *clear || style.as_object().is_some_and(|p| p.keys().any(|k| ["font", "family", "fontStyle", "font_style", "style"].contains(&k.as_str())))
        }
        _ => false,
    }
}
fn inputs<'a>(s: &'a Session, r: &Request, before: bool) -> Result<(&'a TrackItem, &'a EffectInstance, i64)> {
    let (target, layer_id, frame, has_font) = match &r.edit {
        Edit::Set { target, layer_id, frame, props } => {
            (target, layer_id, *frame, props.as_object().is_some_and(|p| p.contains_key("font") || p.contains_key("font_style")))
        }
        Edit::StyleRange { target, layer_id, frame, clear, style, .. } => (
            target,
            layer_id,
            *frame,
            *clear || style.as_object().is_some_and(|p| p.keys().any(|k| ["font", "family", "fontStyle", "font_style", "style"].contains(&k.as_str()))),
        ),
        _ => return Err(error("font planning requires an explicit font property or character-range gesture")),
    };
    if !has_font {
        return Err(error("font planning requires a font reference change or explicit override clear"));
    }
    let clip = if r.shared_source.is_some() { super::shared_fonts::target(s, r, target, before)? } else { super::clip(s, target)? };
    let (_, index) = super::layer(clip, layer_id)?;
    let effect = clip.effects.get(index).filter(|e| e.effect == graphic::TEXT_LAYER).ok_or_else(|| error("font planning requires an exact text layer"))?;
    let at = super::clock(s, frame)?;
    if at < clip.start || clip.start.0.checked_add(clip.duration.0).is_none_or(|end| at.0 >= end) {
        return Err(error("exact font frame must be inside the graphic"));
    }
    Ok((clip, effect, frame))
}
fn prediction(s: &Session, r: &Request, c: &TrackItem, e: &EffectInstance, frame: i64) -> Result<EffectInstance> {
    let mut predicted = e.clone();
    let at = super::clock(s, frame)?;
    match &r.edit {
        Edit::Set { props, .. } => {
            super::properties(e, props, true)?;
            let source = c.source_time_at(at);
            if !(-super::SAFE..=super::SAFE).contains(&source.0) {
                return Err(error("font source time exceeds exact transport"));
            }
            crate::graphics::apply_props(&mut predicted, props.as_object().ok_or_else(|| error("font properties missing"))?, source)?;
        }
        Edit::StyleRange { start, end, text_sha256, runs_sha256, range_text_sha256, clear, style, .. } => {
            let (mt, parsed, _) = character_edit::prepare(c, e, at, *start, *end, text_sha256, runs_sha256, range_text_sha256, *clear, style, true)?;
            crate::graphic_templates::apply_character_style(&mut predicted, mt, Some(*start), Some(*end), &parsed, *clear)?;
        }
        _ => return Err(error("unsupported font planning gesture")),
    }
    if predicted == *e {
        return Err(error("font gesture has no authored change"));
    }
    Ok(predicted)
}
fn observations(s: &Session, r: &Request) -> Result<(Value, Value)> {
    let sid = super::scope(s, &r.scope)?;
    if !super::effect_tracking(s, sid) || super::digest(s)? != r.before_graphics_sha256 {
        return Err(error("font plan requires the current complete tracked graphics collection"));
    }
    let (c, e, frame) = inputs(s, r, true)?;
    let predicted = prediction(s, r, c, e, frame)?;
    if r.shared_source.is_some() {
        return super::shared_fonts::observations(s, r, c, &predicted, frame);
    }
    for _ in 0..2 {
        let revision = filmcraft_text::fonts::registry_revision();
        let before = font_resources::observe_effect(s, c, e, frame)?;
        let after = font_resources::observe_effect(s, c, &predicted, frame)?;
        if filmcraft_text::fonts::registry_revision() == revision {
            return Ok((before, after));
        }
    }
    Err(error("font registry changed across the proposed gesture; refresh the plan"))
}
pub(super) fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Query = super::super::parse(super::PLAN_FONTS, p)?;
    super::super::validate_metadata_read(s, &q.token, q.projection, super::PLAN_FONTS)?;
    let r: Request = super::super::parse(super::PLAN_FONTS, &q.request)?;
    if q.request.get("fontPlan").is_some() {
        return Err(error("provide an unplanned proposal, not a copied font plan"));
    }
    super::validate_proposal(&super::Operation { command: super::COMMAND.into(), params: q.request.clone() }, true)?;
    let (before, after) = observations(s, &r)?;
    let proof = FontPlan {
        version: 1,
        read_token: q.token.clone(),
        proposal_sha256: proposed(&r, &q.token)?,
        before_sha256: digest(&before)?,
        after_sha256: digest(&after)?,
    };
    let mut out = json!({"status":"planned","kind":"native_graphics_font_plan","version":1,"token":q.token,"revision":s.revision,
        "sequence":s.state.active_sequence.map(|id|id.0),"scope":r.scope,"before":before,"after":after,"fontPlan":proof,
        "referenceScope":"exact_read_ticket_and_proposal_only","sourceValidation":"not-performed","fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
    if q.canonical {
        // Preserve the exact native canonical bytes so another runtime can
        // verify the existing proof without reserializing native float values.
        out["beforeJson"] = json!(String::from_utf8(project_audit::value_bytes(&before)?).map_err(|e| error(e.to_string()))?);
        out["afterJson"] = json!(String::from_utf8(project_audit::value_bytes(&after)?).map_err(|e| error(e.to_string()))?);
    }
    if q.projection {
        out["requiresSourceValidation"] = json!(true);
    }
    if project_audit::value_bytes(&out)?.len() > 64 * 1024 {
        return Err(error("font plan response exceeds its bound"));
    }
    Ok(out)
}
pub(super) fn verify(s: &Session, r: &Request, token: Option<&str>) -> Result<()> {
    let p = r.font_plan.as_ref().filter(|p| p.valid()).ok_or_else(|| error("exact current font plan required"))?;
    if token.is_some_and(|t| p.read_token != t) || p.proposal_sha256 != proposed(r, &p.read_token)? {
        return Err(error("font plan belongs to a different read or proposal"));
    }
    let (before, after) = observations(s, r)?;
    if p.before_sha256 != digest(&before)? || p.after_sha256 != digest(&after)? {
        return Err(error("native font or glyph resolution changed; take a fresh font plan"));
    }
    Ok(())
}
pub(super) fn verify_after(s: &Session, r: &Request) -> Result<()> {
    let p = r.font_plan.as_ref().ok_or_else(|| error("font plan missing after staged gesture"))?;
    let (c, e, frame) = inputs(s, r, false)?;
    let mut observed = font_resources::observe_effect(s, c, e, frame)?;
    if r.shared_source.is_some() {
        super::shared_fonts::attach(&s.project, r, &mut observed)?;
    }
    if digest(&observed)? != p.after_sha256 {
        return Err(error("human font gesture differs from its native renderer prediction; staging discarded"));
    }
    Ok(())
}
