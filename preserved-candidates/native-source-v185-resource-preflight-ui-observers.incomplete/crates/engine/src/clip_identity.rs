//! Local Poema instance tracking and editor-owned protection, opt-in per sequence. Human edits
//! reconcile carriers at the same project commit boundary as agent edits. No file/sidecar sync.

use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ClipId, ClipIdentity, ItemKind, Project};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

use filmcraft_project::clip_identity::EFFECT_TRACKING_KEY as EFFECTS;
use filmcraft_project::clip_identity::TRACKING_KEY as TRACKING;
mod effects;

fn error(msg: impl Into<String>) -> crate::EngineError {
    bad("poema.clipIdentity", msg)
}

fn fresh(reserved: &BTreeSet<String>, seen: &BTreeSet<String>) -> Result<String> {
    for _ in 0..8 {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| error(format!("could not allocate a persistent clip identity: {e}")))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        if !reserved.contains(&id) && !seen.contains(&id) {
            return Ok(id);
        }
    }
    Err(error("could not allocate a unique persistent clip identity"))
}

/// Called on the candidate, before it becomes visible. Clone-based splits/copies carry the
/// parent's id to this boundary: retain a surviving instance, fork NEW locations. Random ids
/// are not an undoable counter, so undo → new branch cannot resurrect another instance's notes.
#[derive(Clone, Copy)]
enum Carrier {
    Clip,
    Effect(usize),
    Mask(usize, usize),
}
enum Allocation<'a> {
    Native,
    Recorded(&'a Project),
}
impl Allocation<'_> {
    fn fresh(
        &mut self,
        sequence: filmcraft_project::ItemId,
        clip: ClipId,
        carrier: Carrier,
        reserved: &BTreeSet<String>,
        seen: &BTreeSet<String>,
    ) -> Result<String> {
        let Self::Recorded(project) = self else {
            return fresh(reserved, seen);
        };
        let clip = project.sequence(sequence).and_then(|q| q.find_item(clip)).map(|(_, c)| c).ok_or_else(|| error("recorded reconciliation clip missing"))?;
        let id = match carrier {
            Carrier::Clip => clip.identity.as_ref().map(|i| &i.id),
            Carrier::Effect(e) => clip.effects.get(e).and_then(|e| e.instance_id.as_ref()),
            Carrier::Mask(e, m) => clip.effects.get(e).and_then(|e| e.masks.get(m)).and_then(|m| m.instance_id.as_ref()),
        }
        .ok_or_else(|| error("recorded reconciliation identity missing"))?;
        if !ClipIdentity::valid_id(id) || reserved.contains(id) || seen.contains(id) {
            return Err(error("recorded reconciliation identity is invalid, reused or not fresh"));
        }
        Ok(id.clone())
    }
}
pub(crate) fn validate_reconciliation(before: &Project, expected: &mut Project, observed: &Project) -> Result<()> {
    reconcile_using(before, expected, &mut Allocation::Recorded(observed))
}
pub(crate) fn reconcile(before: &Project, after: &mut Project) -> Result<()> {
    reconcile_using(before, after, &mut Allocation::Native)
}
fn reconcile_using(before: &Project, after: &mut Project, allocation: &mut Allocation<'_>) -> Result<()> {
    filmcraft_project::clip_identity::validate_project(before).map_err(error)?;
    let mut previous = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut reserved = BTreeSet::new();
    for seq in before.sequences() {
        let Some(q) = seq.as_sequence() else { continue };
        for c in q.all_tracks().flat_map(|t| &t.items) {
            if let Some(i) = &c.identity {
                previous.insert((seq.id, c.id), c);
                origins.insert(i.id.clone(), c);
                reserved.insert(i.id.clone());
            }
        }
    }
    let mut seen = BTreeSet::new();
    for seq in after.items.values_mut() {
        let sid = seq.id;
        let marker = seq.metadata.get(TRACKING);
        if marker.is_some_and(|v| v != "1") {
            return Err(error("unsupported instance tracking version"));
        }
        // Keep untouched sequences structurally shared. Tracking must not deep-clone every
        // sequence just because the editor changed one clip, a bin or a project note.
        let unchanged = before.item(sid).is_some_and(|old| {
            old.metadata.get(TRACKING) == marker
                && old.metadata.get(EFFECTS) == seq.metadata.get(EFFECTS)
                && matches!((&old.kind, &seq.kind),
                (ItemKind::Sequence(a), ItemKind::Sequence(b)) if std::sync::Arc::ptr_eq(a, b))
        });
        if unchanged {
            if let Some(q) = seq.as_sequence() {
                for i in q.all_tracks().flat_map(|t| &t.items).filter_map(|c| c.identity.as_ref()) {
                    if seen.len() >= 100_000 || !seen.insert(i.id.clone()) {
                        return Err(error("duplicate identity or too many tracked instances"));
                    }
                }
            }
            continue;
        }
        let enabled = marker.is_some() || seq.as_sequence().is_some_and(|q| q.all_tracks().flat_map(|t| &t.items).any(|c| c.identity.is_some()));
        if !enabled {
            continue;
        }
        seq.metadata.insert(TRACKING.into(), "1".into());
        let effect_tracking = seq.metadata.get(EFFECTS).map(String::as_str);
        if effect_tracking.is_some_and(|v| v != "1") {
            return Err(error("unsupported effect tracking version"));
        }
        let effect_tracking = effect_tracking.is_some();
        let Some(q) = seq.as_sequence_mut() else { continue };
        for c in q.all_tracks_mut().flat_map(|t| &mut t.items) {
            if seen.len() >= 100_000 {
                return Err(error("more than 100000 tracked clip instances"));
            }
            let old = previous.get(&(sid, c.id)).copied();
            if let Some(i) = &c.identity {
                i.validate().map_err(error)?;
            }
            // Same location and source: keep the existing instance, including explicit human
            // annotation/protection changes. Replacement source: new identity, old notes cleared.
            let surviving = old.filter(|old| old.item == c.item);
            if let Some(old) = surviving {
                match (&c.identity, &old.identity) {
                    (Some(a), Some(b)) if a.id != b.id => return Err(error("an existing instance identity cannot be reassigned")),
                    (None, Some(b)) => c.identity = Some(b.clone()),
                    _ => {}
                }
            } else {
                let parent = c.identity.as_ref().map(|i| i.id.clone()).or_else(|| old.and_then(|c| c.identity.as_ref()).map(|i| i.id.clone()));
                let origin = c.identity.as_ref().and_then(|i| origins.get(&i.id)).copied();
                let source_replaced = old.or(origin).is_some_and(|old| old.item != c.item);
                let annotations = if source_replaced { BTreeMap::new() } else { c.identity.as_ref().map(|i| i.annotations.clone()).unwrap_or_default() };
                let protection =
                    c.identity.as_ref().and_then(|i| i.protection.clone()).or_else(|| old.and_then(|c| c.identity.as_ref()).and_then(|i| i.protection.clone()));
                c.identity =
                    Some(Box::new(ClipIdentity { id: allocation.fresh(sid, c.id, Carrier::Clip, &reserved, &seen)?, parent, annotations, protection }));
            }
            if c.identity.is_none() {
                c.identity = Some(Box::new(ClipIdentity {
                    id: allocation.fresh(sid, c.id, Carrier::Clip, &reserved, &seen)?,
                    parent: None,
                    annotations: BTreeMap::new(),
                    protection: None,
                }));
            }
            if let Some(i) = &c.identity {
                i.validate().map_err(error)?;
                if !seen.insert(i.id.clone()) {
                    return Err(error("duplicate persistent clip identity"));
                }
            }
            if effect_tracking {
                effects::reconcile(old, c, allocation, sid)?;
            }
        }
    }
    Ok(())
}

fn value<T: serde::Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| error(e.to_string()))
}

/// A freeze protects the whole clip (including its track/position, source, effects and notes),
/// not merely explicitly targeted trims. Compare the FINAL staged document so collateral ripple
/// changes also fail. Dependencies of protected nested clips are walked with cycle/count bounds.
pub(crate) fn enforce_protection(before: &Project, after: &Project) -> Result<()> {
    filmcraft_project::clip_identity::validate_project(before).map_err(error)?;
    filmcraft_project::clip_identity::validate_project(after).map_err(error)?;
    let mut dependencies = Vec::new();
    for seq in before.sequences() {
        let Some(q) = seq.as_sequence() else { continue };
        for track in q.all_tracks() {
            for c in &track.items {
                let Some(identity) = c.identity.as_ref().filter(|i| i.protection.is_some()) else { continue };
                let changed = match after.sequence(seq.id).and_then(|q| q.find_item(c.id)) {
                    Some((t, other)) => t != track.id || value(c)? != value(other)?,
                    None => true,
                };
                if changed {
                    return Err(error(format!("protected clip {} would change: {}", identity.id, identity.protection.as_deref().unwrap_or_default())));
                }
                dependencies.push(c.item);
            }
        }
    }
    let mut visited = BTreeSet::new();
    while let Some(id) = dependencies.pop() {
        if !visited.insert(id) {
            continue;
        }
        if visited.len() > 100_000 {
            return Err(error("protected dependency graph is too large"));
        }
        let old = before.item(id).ok_or_else(|| error("protected clip has a missing source"))?;
        let new = after.item(id).ok_or_else(|| error("protected clip source would be removed"))?;
        if value(old)? != value(new)? {
            return Err(error(format!("protected clip source/nested sequence {} would change", id.0)));
        }
        match &old.kind {
            ItemKind::Sequence(q) => {
                for c in q.all_tracks().flat_map(|t| &t.items) {
                    if dependencies.len() >= 100_000 {
                        return Err(error("protected dependency graph is too large"));
                    }
                    dependencies.push(c.item);
                }
            }
            ItemKind::Subclip { parent, .. } => dependencies.push(*parent),
            _ => {}
        }
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| error(e.to_string()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn track(s: &mut Session, p: &Value) -> Result<Value> {
    let _: Empty = parse(p)?;
    let id = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    s.edit("Track Poema Instances", |p, _| {
        p.items.get_mut(&id).ok_or(crate::EngineError::NoSequence)?.metadata.insert(TRACKING.into(), "1".into());
        Ok(())
    })?;
    Ok(json!({"sequence": id.0, "tracked": true}))
}

fn track_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let _: Empty = parse(p)?;
    let id = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    if s.project.item(id).is_some_and(|i| i.metadata.get(EFFECTS).is_some_and(|v| v == "1")) {
        return Ok(json!({"sequence":id.0,"trackedEffects":true}));
    }
    s.edit("Track Poema Effect Instances", |p, _| {
        let item = p.items.get_mut(&id).ok_or(crate::EngineError::NoSequence)?;
        item.metadata.insert(TRACKING.into(), "1".into());
        item.metadata.insert(EFFECTS.into(), "1".into());
        Ok(())
    })?;
    Ok(json!({"sequence":id.0,"trackedEffects":true}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Annotations {
    clips: Vec<u64>,
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Protection {
    clips: Vec<u64>,
    // `null` is an explicit human unlock; absence is refused.
    reason: Value,
}

fn targets(s: &Session, ids: &[u64], linked: bool) -> Result<Vec<ClipId>> {
    if ids.is_empty() || ids.len() > 256 {
        return Err(error("need 1–256 explicit clip ids"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut targets = Vec::new();
    let mut links = BTreeSet::new();
    for id in ids {
        let id = ClipId(*id);
        let (_, c) = q.find_item(id).ok_or_else(|| error("clip is missing from the active sequence"))?;
        if c.identity.is_none() {
            return Err(error("enable poema.trackSequence before annotating/protecting clips"));
        }
        if !targets.contains(&id) {
            targets.push(id);
        }
        if let Some(link) = c.link {
            links.insert(link);
        }
    }
    if linked {
        for c in q.all_tracks().flat_map(|t| &t.items) {
            if c.link.is_some_and(|l| links.contains(&l)) && !targets.contains(&c.id) {
                targets.push(c.id);
            }
        }
    }
    Ok(targets)
}

fn annotate(s: &mut Session, p: &Value) -> Result<Value> {
    let params: Annotations = parse(p)?;
    let ids = targets(s, &params.clips, false)?;
    s.edit_sequence("Annotate Poema Clips", |q, _, _| {
        for id in &ids {
            let i = q.find_item_mut(*id).and_then(|(_, c)| c.identity.as_mut()).ok_or_else(|| error("clip identity is missing"))?;
            i.annotations = params.annotations.clone();
            i.validate().map_err(error)?;
        }
        Ok(())
    })?;
    Ok(json!({"clips": ids.iter().map(|id| id.0).collect::<Vec<_>>()}))
}

fn protect(s: &mut Session, p: &Value) -> Result<Value> {
    let params: Protection = parse(p)?;
    let reason: Option<String> = parse(&params.reason)?;
    let ids = targets(s, &params.clips, true)?;
    s.edit_sequence("Protect Poema Clips", |q, _, _| {
        for id in &ids {
            let i = q.find_item_mut(*id).and_then(|(_, c)| c.identity.as_mut()).ok_or_else(|| error("clip identity is missing"))?;
            i.protection = reason.clone();
            i.validate().map_err(error)?;
        }
        Ok(())
    })?;
    Ok(json!({"clips": ids.iter().map(|id| id.0).collect::<Vec<_>>(), "reason": reason}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: true };
    vec![
        spec("poema.trackSequence", "Track Poema Instances", "{}", track),
        spec("poema.trackEffects", "Track Poema Effect Instances", "{}", track_effects),
        spec("poema.annotateClips", "Annotate Poema Clips", r#"{"clips":[id],"annotations":{key:str}}"#, annotate),
        spec("poema.protectClips", "Protect Poema Clips", r#"{"clips":[id],"reason":str|null}"#, protect),
    ]
}

#[cfg(test)]
mod effect_tests;
#[cfg(test)]
mod tests;
