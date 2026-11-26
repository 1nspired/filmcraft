//! Read-only character evidence using the original text clock and saved run vector.
use super::{error, project_audit};
use crate::{Result, Session};
use filmcraft_project::{ClipIdentity, EffectInstance, TrackItem, graphic};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) const MAX_CHARACTERS: usize = 1_000_000;
const TEXT_CHUNK: usize = 256;
pub(super) fn selected_clip<'a>(s: &'a Session, identity: &str) -> Result<&'a TrackItem> {
    if !ClipIdentity::valid_id(identity) {
        return Err(error("persistent graphic identity required"));
    }
    let matches = super::clips(s)?.into_iter().filter(|(_, c)| c.identity.as_ref().is_some_and(|id| id.id == identity)).collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(error("graphic identity is absent or ambiguous"));
    }
    matches.first().map(|(_, c)| *c).ok_or_else(|| error("graphic is missing"))
}
pub(super) fn layer_index(s: &Session, identity: &str, offset: usize, limit: usize) -> Result<(usize, Vec<Value>)> {
    let c = selected_clip(s, identity)?;
    let indices = graphic::layer_indices(&c.effects);
    let end = offset.saturating_add(limit).min(indices.len());
    let selected = indices.get(offset..end).ok_or_else(|| error("layer index page is outside the graphic"))?;
    let rows = selected
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let e = c.effects.get(*i).ok_or_else(|| error("graphic layer missing"))?;
            Ok(json!({"layerId":e.instance_id,"position":offset+n,"definition":e.effect,"enabled":e.enabled}))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((indices.len(), rows))
}
pub(super) fn text_layer<'a>(c: &'a TrackItem, layer_id: &str) -> Result<&'a EffectInstance> {
    if !ClipIdentity::valid_id(layer_id) {
        return Err(error("persistent text layer identity required"));
    }
    let matches = graphic::layer_indices(&c.effects)
        .into_iter()
        .filter_map(|i| c.effects.get(i))
        .filter(|e| e.instance_id.as_deref() == Some(layer_id))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(error("text layer identity is absent or ambiguous"));
    }
    matches.first().copied().filter(|e| e.effect == graphic::TEXT_LAYER).ok_or_else(|| error("selected layer is not text"))
}
pub(super) fn page(
    s: &Session,
    identity: &str,
    layer_id: &str,
    frame: i64,
    offset: usize,
    limit: usize,
    want_text: bool,
) -> Result<(usize, Vec<Value>, Value)> {
    if !(0..=1_000_000).contains(&frame) {
        return Err(error("explicit bounded native character evaluation frame required"));
    }
    let c = selected_clip(s, identity)?;
    let e = text_layer(c, layer_id)?;
    let at = super::clock(s, frame)?;
    let end = c.start.0.checked_add(c.duration.0).filter(|end| *end > c.start.0).ok_or_else(|| error("graphic clock is invalid"))?;
    if at < c.start || at.0 >= end {
        return Err(error("explicit character frame is outside the graphic"));
    }
    let mt = crate::graphics::layout_source_time(c, at)?;
    let text = graphic::text_at(e, mt);
    let chars = text.chars().count();
    let runs = e.layer.as_ref().map_or(&[][..], |x| &x.runs[..]);
    if chars > MAX_CHARACTERS || runs.len() > MAX_CHARACTERS {
        return Err(error("complete character evidence exceeds its explicit bound"));
    }
    let state = json!({"identity":identity,"layerId":layer_id,"frame":frame,"textSha256":format!("{:x}",Sha256::digest(text.as_bytes())),
        "textCharacters":chars,"runsSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(&runs)?)),"runCount":runs.len(),
        "textChunkCharacters":TEXT_CHUNK,"indexUnit":"unicode_scalar","fontResources":"unverified"});
    let total = if want_text { chars.div_ceil(TEXT_CHUNK) } else { runs.len() };
    if offset > total {
        return Err(error("character page is outside the complete collection"));
    }
    let end = offset.saturating_add(limit).min(total);
    let mut rows = Vec::new();
    if want_text {
        let start = offset.checked_mul(TEXT_CHUNK).ok_or_else(|| error("text offset exceeds its bound"))?;
        let mut source = text.chars().skip(start);
        for position in offset..end {
            let value: String = source.by_ref().take(TEXT_CHUNK).collect();
            let start = position.checked_mul(TEXT_CHUNK).ok_or_else(|| error("text offset exceeds its bound"))?;
            let end = start.checked_add(value.chars().count()).ok_or_else(|| error("text range exceeds its bound"))?;
            rows.push(json!({"position":position,"start":start,"end":end,"text":value}));
        }
    } else {
        for (n, run) in runs.get(offset..end).ok_or_else(|| error("character run page missing"))?.iter().enumerate() {
            rows.push(json!({"position":offset+n,"run":run}));
        }
    }
    Ok((total, rows, state))
}
