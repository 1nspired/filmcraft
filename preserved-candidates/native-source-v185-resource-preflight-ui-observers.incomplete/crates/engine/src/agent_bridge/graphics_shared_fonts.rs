//! Complete shared-source instance scope around the original propagation service.
use super::{Request, Target, error, project_audit, sequences};
use crate::{Result, Session};
use filmcraft_project::{EffectInstance, ItemId, Project, TrackItem, graphic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
const MAX_MEMBERS: usize = 4096;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SharedSource {
    item: u64,
    before_sha256: String,
}
impl SharedSource {
    pub(super) fn valid(&self) -> bool {
        super::id(self.item) && super::hash(&self.before_sha256)
    }
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(value)?)))
}
struct Inventory {
    state: Value,
    rows: Vec<Value>,
    can_edit: bool,
}
fn inventory(project: &Project, scope: &sequences::Scope, item: ItemId) -> Result<Inventory> {
    if !super::id(item.0) || project.items.len() > 100_000 {
        return Err(error("bounded source graphic required"));
    }
    let source = project.source_graphics.get(&item).ok_or_else(|| error("selected graphic is not a shared source"))?;
    if source.layers.len() > 4096 {
        return Err(error("shared source layer collection exceeds its bound"));
    }
    let master = project_audit::value_bytes(&(&source.layers, &source.meta))?;
    let mut rows = Vec::new();
    let mut can_edit = true;
    for sequence in project.sequences() {
        let q = project.sequence(sequence.id).ok_or_else(|| error("shared sequence missing"))?;
        for track in &q.video_tracks {
            for clip in track.items.iter().filter(|c| c.item == item) {
                if rows.len() >= MAX_MEMBERS
                    || clip.effects.len() > 4096
                    || !super::id(sequence.id.0)
                    || !super::id(track.id.0)
                    || !super::id(clip.id.0)
                    || !sequences::belongs(project, sequence.id, scope)?
                {
                    return Err(error("shared graphic has an out-of-scope or oversized instance collection"));
                }
                q.settings.validate().map_err(error)?;
                let identity = clip.identity.as_ref().map(|i| i.id.as_str());
                if identity.is_some_and(|id| !filmcraft_project::ClipIdentity::valid_id(id)) {
                    return Err(error("shared instance identity is invalid"));
                }
                let tracked = sequence.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v| v == "1");
                let protection = clip.identity.as_ref().and_then(|i| i.protection.as_deref());
                let layers = clip.effects.iter().filter(|e| graphic::is_layer(e)).collect::<Vec<_>>();
                let consistent = project_audit::value_bytes(&(&layers, clip.graphic.as_deref()))? == master;
                can_edit &= !track.locked && protection.is_none() && identity.is_some() && tracked && consistent;
                rows.push(json!({"sequence":sequence.id.0,"clip":clip.id.0,"identity":identity,"track":track.id.0,
                "locked":track.locked,"protection":protection,"effectTracking":tracked,"consistentWithMaster":consistent}));
            }
        }
    }
    if rows.is_empty() {
        return Err(error("shared source has no current owning instances"));
    }
    rows.sort_by_key(|r| (r["sequence"].as_u64(), r["clip"].as_u64()));
    let project_sha = format!("{:x}", Sha256::digest(project_audit::bytes(project)?));
    let state = json!({"item":item.0,"beforeSha256":digest(&("native_shared_graphic_scope_v1",scope,item.0,&project_sha))?,
        "sourceSha256":digest(source)?,"membersSha256":digest(&rows)?,"memberCount":rows.len(),"projectSha256":project_sha,
        "allInstancesEditable":can_edit,"rendererSampling":"requested_instance_frame_only","referenceScope":"complete_current_workspace_instances"});
    Ok(Inventory { state, rows, can_edit })
}
pub(super) fn page(s: &Session, scope: &sequences::Scope, identity: &str, offset: usize, limit: usize) -> Result<(usize, Vec<Value>, Value)> {
    let clip = super::characters::selected_clip(s, identity)?;
    let v = inventory(&s.project, scope, clip.item)?;
    if offset > v.rows.len() {
        return Err(error("shared source page outside its complete collection"));
    }
    let end = offset.saturating_add(limit).min(v.rows.len());
    Ok((v.rows.len(), v.rows.get(offset..end).ok_or_else(|| error("shared source page missing"))?.to_vec(), v.state))
}
pub(super) fn target<'a>(s: &'a Session, r: &Request, target: &Target, before: bool) -> Result<&'a TrackItem> {
    let declared = r.shared_source.as_ref().filter(|d| d.valid()).ok_or_else(|| error("explicit shared source scope required"))?;
    let clip = super::checked_clip_target(s, target, true, true)?;
    if declared.item != clip.item.0 {
        return Err(error("shared source scope differs from the selected instance"));
    }
    let v = inventory(&s.project, &r.scope, clip.item)?;
    if !v.can_edit || before && v.state["beforeSha256"] != declared.before_sha256 {
        return Err(error("complete shared source instance scope changed, is locked/protected/untracked or is not synchronized"));
    }
    if before {
        let mut unchanged = (*s.project).clone();
        crate::graphic_templates::sync_source_graphics_in_project(&mut unchanged);
        if project_audit::bytes(&unchanged)? != project_audit::bytes(&s.project)? {
            return Err(error("original human propagation would also repair unrelated source state; resolve it before this font edit"));
        }
    }
    Ok(clip)
}
pub(super) fn attach(project: &Project, r: &Request, state: &mut Value) -> Result<()> {
    let declared = r.shared_source.as_ref().ok_or_else(|| error("shared source declaration missing"))?;
    state["sharedSource"] = inventory(project, &r.scope, ItemId(declared.item))?.state;
    Ok(())
}
pub(super) fn observations(s: &Session, r: &Request, clip: &TrackItem, predicted: &EffectInstance, frame: i64) -> Result<(Value, Value)> {
    let sid = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let layer_id = predicted.instance_id.as_deref().ok_or_else(|| error("shared text layer identity missing"))?;
    let (_, index) = super::layer(clip, layer_id)?;
    let current = clip.effects.get(index).ok_or_else(|| error("shared current effect missing"))?;
    let mut expected = (*s.project).clone();
    let dst = expected
        .sequence_mut(sid)
        .and_then(|q| q.find_item_mut(clip.id))
        .and_then(|(_, c)| c.effects.get_mut(index))
        .ok_or_else(|| error("shared predicted effect missing"))?;
    *dst = predicted.clone();
    crate::graphic_templates::sync_source_graphics_in_project(&mut expected);
    if project_audit::bytes(&expected)? == project_audit::bytes(&s.project)? {
        return Err(error("original shared propagation leaves no authored change"));
    }
    let after_clip = expected.sequence(sid).and_then(|q| q.find_item(clip.id)).map(|(_, c)| c).ok_or_else(|| error("shared predicted instance missing"))?;
    let after_effect =
        after_clip.effects.iter().find(|e| e.instance_id.as_deref() == Some(layer_id)).ok_or_else(|| error("shared predicted layer identity changed"))?;
    for _ in 0..2 {
        let revision = filmcraft_text::fonts::registry_revision();
        let mut before = super::font_resources::observe_effect(s, clip, current, frame)?;
        attach(&s.project, r, &mut before)?;
        let mut after = super::font_resources::observe_effect(s, after_clip, after_effect, frame)?;
        attach(&expected, r, &mut after)?;
        if filmcraft_text::fonts::registry_revision() == revision {
            return Ok((before, after));
        }
    }
    Err(error("native fonts changed across the complete shared proposal; refresh"))
}
