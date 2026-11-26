//! Bounded sequence creation and project-local navigation; no arbitrary project commands.
use filmcraft_project::{ItemId, Project};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{BINDING_KEY, Binding, Operation, parse};
use crate::{Result, Session, commands::bad};

pub(super) const CREATE: &str = "agent.createSequence";
pub(super) const KEY: &str = crate::editorial_workspace::SEQUENCE_KEY;
pub(super) use crate::editorial_workspace::Scope;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    scope: Scope,
}
pub(super) fn belongs(project: &Project, id: ItemId, scope: &Scope) -> Result<bool> {
    if !crate::editorial_workspace::valid_scope(scope) || project.sequence(ItemId(scope.anchor_sequence)).is_none() || project.sequence(id).is_none() {
        return Ok(false);
    }
    if crate::editorial_workspace::association(project)?.is_some_and(|r| r.scope != *scope) {
        return Ok(false);
    }
    if id.0 == scope.anchor_sequence {
        return Ok(true);
    }
    let Some(raw) = project.item(id).and_then(|it| it.metadata.get(KEY)) else { return Ok(false) };
    let found: Scope = serde_json::from_str(raw).map_err(|e| bad(CREATE, e.to_string()))?;
    Ok(found == *scope || crate::project_workspace::continues_retired_sequence_scope(project, &found, scope)?)
}
pub(super) fn catalog(s: &Session) -> Result<Vec<Value>> {
    s.project
        .sequences()
        .take(256)
        .map(|it| {
            let scope = it.metadata.get(KEY).map(|raw| serde_json::from_str::<Scope>(raw).map_err(|e| bad(CREATE, e.to_string()))).transpose()?;
            let q = it.as_sequence().ok_or_else(|| bad(CREATE, "missing sequence"))?;
            Ok(json!({"item": it.id.0, "name": it.name, "scope": scope,
            "frameRate": q.settings.frame_rate, "duration": q.duration().0,
            "tracked": it.metadata.get(filmcraft_project::clip_identity::TRACKING_KEY).is_some_and(|v| v == "1")}))
        })
        .collect()
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Create = parse(CREATE, &op.params)?;
    if !crate::editorial_workspace::valid_scope(&p.scope)
        || p.name.trim().is_empty()
        || p.name != p.name.trim()
        || p.name.len() > 160
        || p.name.chars().any(char::is_control)
    {
        return Err(bad(CREATE, "need a bounded name and explicit workspace sequence scope"));
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Create = parse(CREATE, &op.params)?;
    let current = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !belongs(&s.project, current, &p.scope)? || s.project.sequences().count() >= 128 {
        return Err(bad(CREATE, "current sequence is outside this workspace or its 128-sequence limit was reached"));
    }
    if s.project.sequences().any(|it| it.name.to_lowercase() == p.name.to_lowercase()) {
        return Err(bad(CREATE, "sequence name already exists; select it or choose a distinct name"));
    }
    let bound = s
        .project
        .items
        .values()
        .filter_map(|it| it.metadata.get(BINDING_KEY))
        .map(|raw| serde_json::from_str::<Binding>(raw).map_err(|e| bad(CREATE, e.to_string())))
        .collect::<Result<Vec<_>>>()?;
    if !bound.iter().any(|b| b.project_id == p.scope.project_id && b.role == "original") {
        return Err(bad(CREATE, "workspace has no matching bound original sources"));
    }
    let settings = s.active_sequence().ok_or(crate::EngineError::NoSequence)?.settings.clone();
    settings.validate().map_err(|e| bad(CREATE, e))?;
    // Explicit sequence creation carries the parent's existing opt-in. Reads
    // and untracked parents never enable persistent effect identities.
    let effect_tracking =
        s.project.item(current).is_some_and(|it| it.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v| v == "1"));
    let before = s.project.clone();
    if s.project.next_id == 0 || s.project.next_id.checked_add(3).is_none_or(|n| n > 9_007_199_254_740_991) {
        return Err(bad(CREATE, "project ID counter exceeds supported range"));
    }
    let raw = serde_json::to_string(&p.scope).map_err(|e| bad(CREATE, e.to_string()))?;
    let id = s.edit("Create Poema Sequence", |project, _| {
        let id = project.new_sequence(&p.name, settings, 1, 1, None);
        let it = project.item_mut(id).ok_or_else(|| bad(CREATE, "new sequence missing"))?;
        it.metadata.insert(KEY.into(), raw);
        it.metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
        if effect_tracking {
            it.metadata.insert(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY.into(), "1".into());
        }
        Ok(id)
    })?;
    // Prove every original item and setting survives; only one new empty sequence/root child
    // and the allocation counter are permitted to change.
    let mut stripped = (*s.project).clone();
    stripped.items.remove(&id);
    stripped.root.children.retain(|c| !matches!(c, filmcraft_project::BinEntry::Item(item) if *item == id));
    stripped.next_id = before.next_id;
    if stripped != *before || s.project.sequence(id).is_none_or(|q| q.all_tracks().any(|t| !t.items.is_empty())) {
        return Err(bad(CREATE, "sequence creation changed existing project content"));
    }
    s.execute("sequence.open", json!({"item": id.0}))?;
    Ok(json!({"sequence": id.0, "name": p.name, "scope": p.scope, "previousSequence": current.0}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Select {
    pub(super) token: String,
    item: u64,
    scope: Scope,
}
pub(super) fn select_target(s: &Session, p: &Select) -> Result<ItemId> {
    let id = ItemId(p.item);
    if !(1..=9_007_199_254_740_991).contains(&p.item)
        || !belongs(&s.project, id, &p.scope)?
        || !belongs(&s.project, s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?, &p.scope)?
    {
        return Err(bad("agent.selectSequence", "sequence is not an existing member of this workspace"));
    }
    Ok(id)
}
