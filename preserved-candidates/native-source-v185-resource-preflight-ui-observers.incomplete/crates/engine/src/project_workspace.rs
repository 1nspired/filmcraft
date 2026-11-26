//! Persistent project identity before the first sequence. Shared human services,
//! not a model executor, media qualifier, independent history or project owner.
use crate::{EngineError, Event, Result, Session, commands::bad, editorial_workspace};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) const KEY: &str = "poema.projectWorkspace.v1";
const TITLE_KEY: &str = "poema.displayTitle.v1";
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectScope {
    pub(crate) project_id: String,
    pub(crate) workspace_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    kind: String,
    version: u8,
    pub(crate) scope: ProjectScope,
}
fn error(message: impl Into<String>) -> EngineError {
    bad("project.workspace", message)
}
/// Native merged/multicam source sequences do not become an owning edit anchor.
/// A real human-created ordinary sequence still needs its explicit association.
pub(crate) fn ordinary_sequences(project: &Project) -> impl Iterator<Item = &filmcraft_project::ProjectItem> {
    project.sequences().filter(|item| {
        item.metadata.contains_key(editorial_workspace::KEY)
            || item.metadata.contains_key(editorial_workspace::SEQUENCE_KEY)
            || item.as_sequence().is_some_and(|q| q.multicam.is_none() && q.merged.is_none())
    })
}
pub(crate) fn valid_scope(scope: &ProjectScope) -> bool {
    !scope.project_id.is_empty()
        && scope.project_id.len() <= 256
        && scope.project_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && editorial_workspace::uuid(&scope.workspace_id)
}
/// Read only a bounded, closed saved identity. Never adopt from an item, name
/// or current sequence; a copied identity retains its existing workspace.
pub(crate) fn identity(project: &Project) -> Result<Option<Identity>> {
    let Some(raw) = project.metadata.get(KEY) else { return Ok(None) };
    if raw.len() > 2048 || project.items.len() > 100_000 {
        return Err(error("project workspace identity or inventory exceeds its bound"));
    }
    if project.items.values().any(|it| it.metadata.contains_key(KEY)) {
        return Err(error("project workspace identity cannot also appear on an item"));
    }
    let found: Identity = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    if found.kind != "poema-native-project-workspace" || found.version != 1 || !valid_scope(&found.scope) {
        return Err(error("invalid saved project workspace identity"));
    }
    Ok(Some(found))
}
// A native owning anchor can be removed by the original human Clear command.
// Retired IDs preserve membership provenance only; they never provide an active
// sequence, source witness, restored grant or executable timeline by themselves.
pub(crate) const RETIRED_ANCHORS_KEY: &str = "poema.retiredSequenceAnchors.v1";
const RETIRED_LIMIT: usize = 100_000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredAnchors {
    kind: String,
    version: u8,
    scope: ProjectScope,
    anchors: Vec<u64>,
}
pub(crate) fn retired_anchor_ids(project: &Project) -> Result<Vec<u64>> {
    let Some(raw) = project.metadata.get(RETIRED_ANCHORS_KEY) else { return Ok(Vec::new()) };
    if raw.len() > 4 * 1024 * 1024 {
        return Err(error("retired anchor metadata exceeds its byte bound"));
    }
    let identity = identity(project)?.ok_or_else(|| error("retired anchors require the existing independent project identity"))?;
    let record: RetiredAnchors = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    if record.kind != "poema-native-retired-sequence-anchors"
        || record.version != 1
        || record.scope != identity.scope
        || record.anchors.is_empty()
        || record.anchors.len() > RETIRED_LIMIT
        || project.items.values().any(|item| item.metadata.contains_key(RETIRED_ANCHORS_KEY))
    {
        return Err(error("invalid retired anchor metadata or owning project scope"));
    }
    let mut previous = 0u64;
    for id in &record.anchors {
        if *id <= previous || *id > 9_007_199_254_740_991 || project.items.contains_key(&ItemId(*id)) {
            return Err(error("retired anchors must be unique ordered absent project item identities"));
        }
        previous = *id;
    }
    Ok(record.anchors)
}
/// Only a real current anchor of this same Root can continue old memberships.
/// A retired number without that current physical sequence authorizes nothing.
pub(crate) fn continues_retired_sequence_scope(project: &Project, previous: &editorial_workspace::Scope, current: &editorial_workspace::Scope) -> Result<bool> {
    if !editorial_workspace::valid_scope(previous)
        || !editorial_workspace::valid_scope(current)
        || previous.project_id != current.project_id
        || project.sequence(ItemId(current.anchor_sequence)).is_none()
    {
        return Ok(false);
    }
    let Some(identity) = identity(project)? else { return Ok(false) };
    if identity.scope.project_id != current.project_id
        || editorial_workspace::association(project)?.is_none_or(|record| record.scope != *current || record.workspace_id != identity.scope.workspace_id)
    {
        return Ok(false);
    }
    Ok(retired_anchor_ids(project)?.binary_search(&previous.anchor_sequence).is_ok())
}
/// Repair only bookkeeping belonging to this existing independent Root, in the
/// SAME Clear candidate. Never adopt an unmarked/foreign survivor or move tabs.
/// Legacy projects retain their original human behavior and identity format.
pub(crate) fn repair_cleared_anchor(before: &Project, after: &mut Project) -> Result<()> {
    let Some(identity) = identity(before)? else { return Ok(()) };
    if self::identity(after)?.as_ref() != Some(&identity) {
        return Err(error("Clear changed the independent owning project identity"));
    }
    let Some(owner) = editorial_workspace::association(before)? else {
        editorial_workspace::association(after)?;
        return Ok(());
    };
    if after.sequence(ItemId(owner.scope.anchor_sequence)).is_some() {
        return Ok(());
    }
    let mut retired = retired_anchor_ids(before)?;
    retired.push(owner.scope.anchor_sequence);
    retired.sort_unstable();
    retired.dedup();
    if retired.len() > RETIRED_LIMIT {
        return Err(error("retired owning anchor inventory exceeds its bound"));
    }
    let record = RetiredAnchors { kind: "poema-native-retired-sequence-anchors".into(), version: 1, scope: identity.scope.clone(), anchors: retired.clone() };
    let raw = serde_json::to_string(&record).map_err(|e| error(e.to_string()))?;
    if raw.len() > 4 * 1024 * 1024 {
        return Err(error("retired anchor metadata exceeds its byte bound"));
    }
    after.metadata.insert(RETIRED_ANCHORS_KEY.into(), raw);
    let successor = after.sequences().find_map(|item| {
        let raw = item.metadata.get(editorial_workspace::SEQUENCE_KEY)?;
        if raw.len() > 2048 {
            return None;
        }
        let scope: editorial_workspace::Scope = serde_json::from_str(raw).ok()?;
        (editorial_workspace::valid_scope(&scope) && scope.project_id == identity.scope.project_id && retired.binary_search(&scope.anchor_sequence).is_ok())
            .then_some(item.id)
    });
    if let Some(id) = successor {
        let scope = editorial_workspace::Scope { project_id: identity.scope.project_id.clone(), anchor_sequence: id.0 };
        let association = editorial_workspace::validate(&json!({"scope":scope,"workspaceId":identity.scope.workspace_id}))?;
        let raw = serde_json::to_string(&association).map_err(|e| error(e.to_string()))?;
        let item = after.item_mut(id).ok_or_else(|| error("surviving owned anchor disappeared"))?;
        item.metadata.remove(editorial_workspace::SEQUENCE_KEY);
        item.metadata.insert(editorial_workspace::KEY.into(), raw);
    }
    editorial_workspace::association(after)?;
    Ok(())
}

// Native presentation identity only. Never an agent read, history guard,
// original-source witness or editor mutation. None keeps legacy presentation
// distinguishable from a required independent project identity.
pub(crate) fn matches_identity(project: &Project, project_id: &str, workspace_id: &str) -> Result<Option<bool>> {
    let wanted = ProjectScope { project_id: project_id.into(), workspace_id: workspace_id.into() };
    if !valid_scope(&wanted) {
        return Err(error("explicit project and workspace identity required"));
    }
    let Some(found) = identity(project)? else { return Ok(None) };
    // Reuse the shared full anchor-consistency check; no active tab is needed.
    crate::editorial_workspace::association(project)?;
    Ok(Some(found.scope == wanted))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    name: Option<String>,
    workspace: Option<ProjectScope>,
    #[serde(rename = "preserveDisplayTitle")]
    preserve_display_title: Option<bool>,
}
/// The existing human New Project command and the future isolated manager use
/// this same initialization. No association operation adopts an existing edit.
pub(crate) fn new_project(params: &Value) -> Result<Project> {
    let p: NewProject = serde_json::from_value(params.clone()).map_err(|e| bad("file.newProject", e.to_string()))?;
    if params.get("name").is_some_and(|v| !v.is_string())
        || params.get("workspace").is_some_and(|v| !v.is_object())
        || params.get("preserveDisplayTitle").is_some_and(|v| !v.is_boolean())
    {
        return Err(bad("file.newProject", "name and workspace must have their explicit supported types"));
    }
    let name = p.name.as_deref().unwrap_or("Untitled");
    if name.trim().is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
        return Err(bad("file.newProject", "need a nonempty project name of at most1024 UTF-8 bytes without control characters"));
    }
    if p.preserve_display_title == Some(true) && p.workspace.is_none() {
        return Err(error("an independent workspace is required to preserve its display title"));
    }
    let mut project = Project::new(name);
    if let Some(scope) = p.workspace {
        if !valid_scope(&scope) {
            return Err(error("new workspace requires an exact film ID and workspace UUID, without a sequence"));
        }
        let record = Identity { kind: "poema-native-project-workspace".into(), version: 1, scope: scope.clone() };
        let raw = serde_json::to_string(&record).map_err(|e| error(e.to_string()))?;
        project.metadata.insert(KEY.into(), raw);
        if p.preserve_display_title == Some(true) {
            let marker = Identity { kind: "poema-native-display-title".into(), version: 1, scope };
            let raw = serde_json::to_string(&marker).map_err(|e| error(e.to_string()))?;
            project.metadata.insert(TITLE_KEY.into(), raw);
        }
    }
    Ok(project)
}
/// Explicitly opted-in independent films keep their title when storage paths
/// change. Malformed/foreign metadata refuses a save instead of renaming it.
pub(crate) fn preserves_display_title(project: &Project) -> Result<bool> {
    let Some(raw) = project.metadata.get(TITLE_KEY) else { return Ok(false) };
    if raw.len() > 2048 || project.items.len() > 100_000 || project.items.values().any(|it| it.metadata.contains_key(TITLE_KEY)) {
        return Err(error("display title metadata exceeds its bound or appears on an item"));
    }
    let marker: Identity = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    let owner = identity(project)?.ok_or_else(|| error("display title requires its native workspace"))?;
    if marker.kind != "poema-native-display-title" || marker.version != 1 || marker.scope != owner.scope {
        return Err(error("display title belongs to another native workspace"));
    }
    Ok(true)
}
pub(crate) fn reset(s: &mut Session, params: &Value) -> Result<Value> {
    // Prepare all fallible input and revision work before replacing anything.
    let project = new_project(params)?;
    let revision = s.revision.checked_add(1).ok_or_else(|| error("native revision space exhausted"))?;
    s.project = Arc::new(project);
    s.history = Default::default();
    s.history.limit = 200;
    s.state = Session::default().state;
    if s.path.take().is_some() {
        s.previews.reset_temp();
    }
    s.revision = revision;
    s.saved_revision = revision;
    s.saved_file = None;
    s.events.push(Event::ProjectChanged { revision });
    Ok(Value::Null)
}
#[derive(Clone)]
pub(crate) struct SequenceContext {
    identity: Identity,
    anchor: Option<u64>,
}
/// A real project association supports the no-sequence state explicitly.
/// Legacy documents keep their existing human sequence behavior unchanged.
pub(crate) fn sequence_context(project: &Project) -> Result<Option<SequenceContext>> {
    let Some(identity) = identity(project)? else { return Ok(None) };
    let association = editorial_workspace::association(project)?;
    Ok(Some(SequenceContext { identity, anchor: association.map(|a| a.scope.anchor_sequence) }))
}
pub(crate) fn sequence_start(project: &Project, context: &Option<SequenceContext>, video: u32, audio: u32) -> Result<Option<u64>> {
    if context.is_none() {
        return Ok(None);
    }
    let needed = u64::from(video).checked_add(u64::from(audio)).and_then(|n| n.checked_add(1)).ok_or_else(|| error("sequence allocation overflow"))?;
    let start = crate::project_ids::allocation_start(project, needed)?;
    if start.checked_add(needed).is_none_or(|last| last > 9_007_199_254_740_991) {
        return Err(error("sequence allocation exceeds exact transport identities"));
    }
    Ok(Some(start))
}
/// Attach identity/tracking in the SAME human sequence candidate before commit.
/// First-sequence Undo removes its anchor but retains the project identity;
/// later human sequences reuse that anchor, including after a saved reopen.
pub(crate) fn attach_sequence(project: &mut Project, context: Option<SequenceContext>, id: ItemId) -> Result<()> {
    let Some(context) = context else { return Ok(()) };
    if identity(project)?.as_ref() != Some(&context.identity) {
        return Err(error("project identity changed during sequence creation"));
    }
    let anchor = context.anchor.unwrap_or(id.0);
    let scope = editorial_workspace::Scope { project_id: context.identity.scope.project_id.clone(), anchor_sequence: anchor };
    let raw = if context.anchor.is_none() {
        serde_json::to_string(&editorial_workspace::validate(&json!({"scope":scope,"workspaceId":context.identity.scope.workspace_id}))?)
    } else {
        serde_json::to_string(&scope)
    }
    .map_err(|e| error(e.to_string()))?;
    let item = project.item_mut(id).ok_or_else(|| error("new workspace sequence is missing"))?;
    if item.as_sequence().is_none() {
        return Err(error("workspace target is not a sequence"));
    }
    let key = if context.anchor.is_none() { editorial_workspace::KEY } else { editorial_workspace::SEQUENCE_KEY };
    if item.metadata.contains_key(key) {
        return Err(error("new sequence already has an owning association"));
    }
    item.metadata.insert(key.into(), raw);
    item.metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
    item.metadata.insert(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY.into(), "1".into());
    if !editorial_workspace::matches(project, &scope, Some(&context.identity.scope.workspace_id))? {
        return Err(error("new workspace anchor failed complete identity validation"));
    }
    Ok(())
}

/// Native Redo restores project snapshots, not stale editor selections. Reopen
/// only the unique initial anchor restored from a genuinely sequence-free
/// snapshot of this same project identity; closing later tabs stays respected.
pub(crate) fn restored_initial_sequence(before: &Project, after: &Project) -> Result<Option<ItemId>> {
    if ordinary_sequences(before).next().is_some() {
        return Ok(None);
    }
    let Some(old) = identity(before)? else { return Ok(None) };
    if identity(after)?.as_ref() != Some(&old) || ordinary_sequences(after).take(2).count() != 1 {
        return Ok(None);
    }
    let association = editorial_workspace::association(after)?.ok_or_else(|| error("restored first sequence lacks its owning anchor"))?;
    Ok(Some(ItemId(association.scope.anchor_sequence)))
}

/// Prepare independent identity for one legacy history node. Original fields,
/// timeline values and arbitrary human metadata stay untouched. The caller owns
/// the complete graph and must validate receipts before any later publication.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn legacy_identity_candidate(project: &Project, scope: &ProjectScope, anchor: ItemId) -> Result<Project> {
    if !valid_scope(scope) || anchor.0 == 0 || anchor.0 > 9_007_199_254_740_991 {
        return Err(error("exact independent identity and real legacy anchor required"));
    }
    // Shared iterative depth/identity checks precede any recursive clone.
    crate::search_organization::validate_inventory(project)?;
    let existing = identity(project)?;
    if existing.as_ref().is_some_and(|v| v.scope != *scope) {
        return Err(error("legacy graph already has another independent identity"));
    }
    let owner = editorial_workspace::association(project)?;
    let legacy = editorial_workspace::Scope { project_id: scope.project_id.clone(), anchor_sequence: anchor.0 };
    if owner.as_ref().is_some_and(|v| v.scope != legacy || v.workspace_id != scope.workspace_id) {
        return Err(error("legacy graph has a foreign owning association"));
    }
    let has_anchor = project.sequence(anchor).is_some();
    if !has_anchor && (project.item(anchor).is_some() || project.sequences().next().is_some()) {
        return Err(error("legacy graph has sequences without its real anchor; no identity was invented"));
    }
    let association = if has_anchor && owner.is_none() {
        Some(
            serde_json::to_string(&editorial_workspace::validate(&json!({"scope":legacy,"workspaceId":scope.workspace_id}))?)
                .map_err(|e| error(e.to_string()))?,
        )
    } else {
        None
    };
    let root = if existing.is_none() {
        Some(
            serde_json::to_string(&Identity { kind: "poema-native-project-workspace".into(), version: 1, scope: scope.clone() })
                .map_err(|e| error(e.to_string()))?,
        )
    } else {
        None
    };
    let mut candidate = project.clone();
    if let Some(raw) = root {
        candidate.metadata.insert(KEY.into(), raw);
    }
    if let Some(raw) = association {
        candidate.item_mut(anchor).ok_or_else(|| error("legacy anchor disappeared during preparation"))?.metadata.insert(editorial_workspace::KEY.into(), raw);
    }
    if identity(&candidate)?.as_ref().is_none_or(|v| v.scope != *scope) {
        return Err(error("prepared independent identity differs"));
    }
    if has_anchor && !editorial_workspace::matches(&candidate, &legacy, Some(&scope.workspace_id))? {
        return Err(error("prepared legacy anchor differs"));
    }
    Ok(candidate)
}
