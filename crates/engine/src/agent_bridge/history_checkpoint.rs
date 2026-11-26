//! Private, immutable native history companions. Restore snapshots and pointer
//! relationships, NEVER replay edit recipes. No model tool or arbitrary path.
use super::organization_scope::Scope as CheckpointScope;
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod identity_publication;
pub(super) mod journal;
mod project_recovery_snapshot;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_STEPS: usize = 256;
const SAVE: &str = "agent.saveHistoryCheckpoint";
const RESTORE: &str = "agent.restoreHistoryCheckpoint";
const RETAINED: &str = "agent.validateRetainedHistory";
const INSPECT: &str = "agent.inspectHistoryCheckpoint";
const PROVENANCE: &str = "agent.inspectCheckpointProvenance";
const IDENTITY: &str = "agent.inspectIdentityMigration";
const KIND: &str = "poema-native-history";
const BASIS: &str = "agent.readHistoryCheckpointBasis";
// A private persistence basis is deliberately NOT a read/edit ticket. It holds
// only weak native identities, expires after sixteen issues, and cannot qualify
// footage, restoration or any timeline mutation.
#[derive(Clone)]
pub(super) struct Basis {
    token: String,
    revision: u64,
    sequence: Option<ItemId>,
    path: Option<String>,
    context: Value,
    scope: CheckpointScope,
    projects: Vec<std::sync::Weak<filmcraft_project::Project>>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    limit: usize,
    merge_key: Option<String>,
}
pub(super) fn has_basis(s: &Session, token: &str) -> bool {
    s.agent_bridge.history_bases.iter().any(|b| b.token == token)
}
// Keep weak graph identities separately from the read-ticket LRU. A checkpoint
// must not adopt a replacement live document merely because its bytes match.
// These guards retain no project snapshots and expire after sixteen saves.
pub(super) struct LiveGuard {
    workspace: String,
    checkpoint: String,
    sha256: String,
    projects: Vec<std::sync::Weak<filmcraft_project::Project>>,
    merge_key: Option<String>,
    // Set only by verified native startup adoption. A new epoch can retain the
    // exact old checkpoint graph, but never its uncommitted source/edit tickets.
    origin_epoch: Option<String>,
    identity_publication: Option<String>,
}
pub(super) fn epoch(s: &mut Session) -> Result<String> {
    if let Some(epoch) = &s.agent_bridge.epoch {
        return Ok(epoch.clone());
    }
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| error(e.to_string()))?;
    let value = raw.iter().map(|b| format!("{b:02x}")).collect::<String>();
    s.agent_bridge.epoch = Some(value.clone());
    Ok(value)
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(RESTORE, message)
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn uuid(s: &str) -> bool {
    s.len() == 36 && s.bytes().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == b'-' } else { c.is_ascii_hexdigit() })
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    scope: CheckpointScope,
    workspace: String,
    checkpoint: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default, rename = "metadataBasis")]
    metadata_basis: bool,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Step {
    label: String,
    project: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTicket {
    token: String,
    revision: u64,
    sequence: Option<u64>,
    project: usize,
    after: usize,
    after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_scope: Option<crate::project_workspace::ProjectScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_library: Option<Vec<u64>>,
    request: Value,
    result: Value,
    redo_consumed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capsule {
    kind: String,
    version: u32,
    schema: u32,
    scope: CheckpointScope,
    workspace: String,
    checkpoint: String,
    path: String,
    sequence: Option<u64>,
    pub(super) epoch: String,
    pub(super) revision: u64,
    projects: Vec<String>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    limit: usize,
    tickets: Vec<SavedTicket>,
    owned_head: bool,
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            return Err(std::io::Error::other("complete history checkpoint exceeds its byte bound"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn bytes<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>> {
    let mut out = Bounded { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut out, value).map_err(|e| error(e.to_string()))?;
    Ok(out.bytes)
}
// Canonical JSON preserves native i64/u64 and the project's existing NaN-auto
// point encoding. It never crosses JSON.parse in JavaScript.
pub(super) fn project_bytes(project: &filmcraft_project::Project, limit: usize) -> Result<Vec<u8>> {
    let raw = bytes(project, limit)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|e| error(e.to_string()))?;
    bytes(&value, limit)
}
fn project(raw: &str) -> Result<filmcraft_project::Project> {
    let value: Value = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    let envelope = json!({"format":filmcraft_format::FORMAT_ID,"schema_version":filmcraft_format::SCHEMA_VERSION,"project":value});
    let loaded = filmcraft_format::decode(&bytes(&envelope, MAX_BYTES)?).map_err(|e| error(e.to_string()))?;
    if project_bytes(&loaded.project, MAX_BYTES)? != raw.as_bytes() {
        return Err(error("history project would be repaired, migrated or lose data; nothing restored"));
    }
    Ok(loaded.project)
}
fn workspace_sequence(s: &Session, scope: &sequences::Scope) -> Result<ItemId> {
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, scope)? || s.project.items.len() > 100_000 {
        return Err(error("foreign or oversized current workspace"));
    }
    Ok(sequence)
}
fn workspace(s: &Session, scope: &sequences::Scope) -> Result<ItemId> {
    let sequence = workspace_sequence(s, scope)?;
    if !history::workspace_scope(s, scope)? {
        return Err(error("workspace has no matching original sources"));
    }
    Ok(sequence)
}
fn checkpoint_version(scope: &CheckpointScope) -> u32 {
    match scope {
        CheckpointScope::Sequence(_) => 1,
        CheckpointScope::Project(_) => 2,
    }
}
fn checkpoint_workspace(s: &Session, scope: &CheckpointScope, restoring: bool) -> Result<Option<ItemId>> {
    match scope {
        CheckpointScope::Sequence(scope) => {
            Ok(Some(if restoring && !history::has_original(&s.project, None)? { workspace_sequence(s, scope)? } else { workspace(s, scope)? }))
        }
        CheckpointScope::Project(scope) => {
            project_plane::require_scope(s, scope)?;
            Ok(s.state.active_sequence)
        }
    }
}
fn validate_association(project: &filmcraft_project::Project, scope: &CheckpointScope, workspace_id: &str) -> Result<()> {
    let owner = crate::editorial_workspace::association(project)?;
    match scope {
        CheckpointScope::Sequence(scope) => {
            if owner.is_some_and(|r| r.scope != *scope || r.workspace_id != workspace_id) {
                return Err(error("checkpoint belongs to a different explicitly associated workspace"));
            }
        }
        CheckpointScope::Project(scope) => {
            if scope.workspace_id != workspace_id || crate::project_workspace::identity(project)?.is_none_or(|r| r.scope != *scope) {
                return Err(error("checkpoint graph differs from the exact saved project/workspace identity"));
            }
        }
    }
    Ok(())
}
fn scope(s: &Session, request: &Request, restoring: bool) -> Result<Option<ItemId>> {
    if !token_ok(&request.token) || !uuid(&request.workspace) || !uuid(&request.checkpoint) {
        return Err(error("explicit unused ticket and workspace/checkpoint UUIDs required"));
    }
    validate_association(&s.project, &request.scope, &request.workspace)?;
    if request.metadata_basis {
        validate_basis(s, &request.token, &request.scope)?;
    } else {
        match &request.scope {
            CheckpointScope::Sequence(_) => check_edit_ticket(s, &request.token)?,
            CheckpointScope::Project(scope) => project_plane::validate_ticket(s, &request.token, scope)?,
        }
    }
    checkpoint_workspace(s, &request.scope, restoring)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BasisRequest {
    token: String,
    scope: CheckpointScope,
}
fn read_basis(s: &mut Session, p: &Value) -> Result<Value> {
    let r: BasisRequest = parse(BASIS, p)?;
    idle(s)?;
    if !token_ok(&r.token) || s.agent_bridge.tickets.iter().any(|t| t.token == r.token) || has_basis(s, &r.token) {
        return Err(bad(BASIS, "fresh private persistence token required"));
    }
    let basis = capture_basis(s, &r.token, &r.scope)?;
    let sequence = basis.sequence;
    let source = basis.path.clone().ok_or_else(|| bad(BASIS, "saved native workspace required"))?;
    s.agent_bridge.operations.register(&r.token)?;
    let epoch = epoch(s)?;
    let result = json!({"protocol":1,"kind":"native_history_checkpoint_basis","token":r.token,"revision":s.revision,"nativeSession":epoch,
        "scope":r.scope,"project":{"path":source},"sequence":{"id":sequence.map(|id|id.0)},"writeAuthority":false,"sourceValidation":"not-performed",
        "capabilities":{"historyCheckpointMetadataBasis":true,"historyCheckpointOperations":true,"historyCheckpointRetention":true,"workspaceUpdateActivationEpoch":true,"workspaceUpdateBundle":true,"workspaceUpdateRetirement":true,
            "operationCancellation":operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS}});
    let result = if matches!(&r.scope, CheckpointScope::Project(_)) {
        let mut result = result;
        result["kind"] = json!("native_project_history_checkpoint_basis");
        result["sequence"] = json!(sequence.map(|id| id.0));
        result["recoveryState"] = super::native_editor::recovery_state(s);
        result["capabilities"] = json!({"projectHistoryCheckpointMetadataBasis":true,"projectHistoryCheckpointOperations":true,"projectHistoryCheckpointRetention":true,"projectRecoverySnapshot":true,"projectRecoveryPreparation":true,"projectRecoveryScheduling":true,"projectRecoveryJournalStartup":true,"workspaceActivationRefresh":true,"workspaceUpdateRetirement":true,"workspaceUpdateActivationEpoch":true,"operationCancellation":operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS});
        result
    } else {
        result
    };
    s.agent_bridge.history_bases.push_back(basis);
    while s.agent_bridge.history_bases.len() > 16 {
        s.agent_bridge.history_bases.pop_front();
    }
    Ok(result)
}
fn validate_basis(s: &Session, token: &str, scope: &CheckpointScope) -> Result<()> {
    idle(s)?;
    let b = s.agent_bridge.history_bases.iter().find(|b| b.token == token).ok_or_else(|| error("unknown/expired private persistence basis"))?;
    if b.scope != *scope {
        return Err(error("foreign private persistence basis"));
    }
    validate_retained_basis(s, b)
}
pub(super) fn retained_basis(s: &Session, token: &str, scope: &sequences::Scope) -> Result<Basis> {
    retained_scoped_basis(s, token, &CheckpointScope::Sequence(scope.clone()))
}
pub(super) fn retained_scoped_basis(s: &Session, token: &str, scope: &CheckpointScope) -> Result<Basis> {
    validate_basis(s, token, scope)?;
    s.agent_bridge.history_bases.iter().find(|b| b.token == token).cloned().ok_or_else(|| error("private persistence basis expired"))
}
pub(super) fn validate_retained_basis(s: &Session, b: &Basis) -> Result<()> {
    idle(s)?;
    let (pool, undo, redo) = graph(s)?;
    if b.revision != s.revision
        || b.path != s.path
        || b.sequence != s.state.active_sequence
        || b.context != context(s)?
        || b.limit != s.history.limit
        || b.merge_key != s.history.merge_key
        || b.undo != undo
        || b.redo != redo
        || b.projects.len() != pool.len()
        || !b.projects.iter().zip(&pool).all(|(a, b)| a.ptr_eq(&Arc::downgrade(b)))
    {
        return Err(error("native project, view or history changed before persistence; no checkpoint published"));
    }
    Ok(())
}
fn path(s: &Session, request: &Request) -> Result<PathBuf> {
    Ok(workspace_directory(s)?.join(format!(".poema-history-{}-{}.json", request.workspace, request.checkpoint)))
}
pub(super) fn workspace_directory(s: &Session) -> Result<PathBuf> {
    let source = s.path.as_deref().ok_or_else(|| error("saved native project required"))?;
    let p = Path::new(source);
    let parent = p.parent().ok_or_else(|| error("project parent missing"))?;
    let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| error("invalid project filename"))?;
    if !p.is_absolute()
        || !source.ends_with(".fcproj")
        || name.is_empty()
        || fs::canonicalize(parent).map_err(|e| error(e.to_string()))? != parent
        || !fs::symlink_metadata(p).map_err(|e| error(e.to_string()))?.is_file()
        || fs::canonicalize(p).map_err(|e| error(e.to_string()))? != p
    {
        return Err(error("real saved project and unredirected absolute parent required"));
    }
    Ok(parent.to_path_buf())
}
fn index(pool: &[Arc<filmcraft_project::Project>], p: &std::sync::Weak<filmcraft_project::Project>) -> Option<usize> {
    pool.iter().position(|q| p.ptr_eq(&Arc::downgrade(q)))
}
fn add(pool: &mut Vec<Arc<filmcraft_project::Project>>, p: &Arc<filmcraft_project::Project>) -> Result<usize> {
    if let Some(i) = pool.iter().position(|q| Arc::ptr_eq(p, q)) {
        return Ok(i);
    }
    if pool.len() > MAX_STEPS {
        return Err(error("native history graph exceeds its complete snapshot bound"));
    }
    pool.push(p.clone());
    Ok(pool.len() - 1)
}
fn graph(s: &Session) -> Result<(Vec<Arc<filmcraft_project::Project>>, Vec<Step>, Vec<Step>)> {
    if s.history.undo.len().saturating_add(s.history.redo.len()) > MAX_STEPS
        || !(1..=1000).contains(&s.history.limit)
        || s.history.merge_key.as_ref().is_some_and(|key| key.len() > 2048)
    {
        return Err(error("complete native history exceeds checkpoint bounds"));
    }
    let mut pool = vec![s.project.clone()];
    let mut steps = |history: &[(String, Arc<filmcraft_project::Project>)]| -> Result<Vec<Step>> {
        history
            .iter()
            .map(|(label, p)| {
                if label.len() > 2048 {
                    return Err(error("history label exceeds checkpoint bound"));
                }
                Ok(Step { label: label.clone(), project: add(&mut pool, p)? })
            })
            .collect()
    };
    let undo = steps(&s.history.undo)?;
    let redo = steps(&s.history.redo)?;
    Ok((pool, undo, redo))
}
fn validate_graph_association(pool: &[Arc<filmcraft_project::Project>], r: &Request) -> Result<()> {
    for project in pool {
        validate_association(project, &r.scope, &r.workspace)?;
        if matches!(&r.scope, CheckpointScope::Project(_)) {
            for summary in [bins::project_summary(project), searches::project_summary(project)] {
                if summary["status"] != "current" {
                    return Err(error("complete project graph organization is unavailable"));
                }
            }
        }
    }
    Ok(())
}
fn valid_sequence(project: &filmcraft_project::Project, id: Option<u64>) -> bool {
    id.is_none_or(|id| (1..=9_007_199_254_740_991).contains(&id) && project.sequence(ItemId(id)).is_some())
}
fn ticket_scope(before: &filmcraft_project::Project, after: &filmcraft_project::Project, r: &Request, t: &SavedTicket) -> Result<bool> {
    match &r.scope {
        CheckpointScope::Sequence(scope) => {
            if t.project_scope.is_some() {
                return Ok(false);
            }
            let (Some(before_id), Some(after_id)) = (t.sequence, t.after_sequence) else { return Ok(false) };
            Ok(sequences::belongs(before, ItemId(before_id), scope)? && sequences::belongs(after, ItemId(after_id), scope)?)
        }
        CheckpointScope::Project(scope) => {
            if !valid_sequence(before, t.sequence) || !valid_sequence(after, t.after_sequence) {
                return Ok(false);
            }
            if let Some(project) = &t.project_scope {
                return Ok(project == scope);
            }
            let (Some(before_id), Some(after_id)) = (t.sequence, t.after_sequence) else { return Ok(false) };
            let (Some(a), Some(b)) = (crate::editorial_workspace::association(before)?, crate::editorial_workspace::association(after)?) else {
                return Ok(false);
            };
            Ok(a.scope == b.scope
                && a.workspace_id == scope.workspace_id
                && sequences::belongs(before, ItemId(before_id), &a.scope)?
                && sequences::belongs(after, ItemId(after_id), &b.scope)?)
        }
    }
}

fn validate_ticket_batch(t: &SavedTicket, batch: &Batch, before: &filmcraft_project::Project, after: &filmcraft_project::Project) -> Result<()> {
    if t.source_library.is_some() {
        let scope = t.project_scope.as_ref().ok_or_else(|| error("library history requires exact Root identity"))?;
        super::library_plane::validate_batch(scope, batch, t.source_library.as_deref().ok_or_else(|| error("missing library read sources"))?)?;
        if t.sequence != t.after_sequence {
            return Err(error("source admission cannot create or change the timeline view"));
        }
        let results = t.result["results"].as_array().ok_or_else(|| error("missing library admission results"))?;
        return super::library_plane::validate_transition(before, after, batch, results);
    }
    if let Some(scope) = &t.project_scope {
        project_plane::validate_project_batch(scope, batch)?;
        let op = batch.operations.first().ok_or_else(|| error("missing historical project operation"))?;
        validate(op)?;
        let action = match op.command.as_str() {
            bins::COMMAND => "bin_edit",
            searches::COMMAND => "search_edit",
            project_plane::CREATE => "project_sequence_create",
            super::project_clear::COMMAND => "project_clear",
            _ => return Err(error("invalid historical project action")),
        };
        let results = t.result["results"].as_array().filter(|v| v.len() == 1).ok_or_else(|| error("one historical project result required"))?;
        if t.result["action"] != action
            || t.result.get("sequence").is_some_and(|value| value != &json!(t.after_sequence))
            || if op.command == project_plane::CREATE {
                t.sequence.is_some() || t.after_sequence != results.first().and_then(|r| r["sequence"].as_u64())
            } else if op.command == super::project_clear::COMMAND {
                results.first().is_none_or(|r| r.get("beforeSequence") != Some(&json!(t.sequence)) || r.get("sequence") != Some(&json!(t.after_sequence)))
            } else {
                t.sequence != t.after_sequence
            }
        {
            return Err(error("historical project action or explicit sequence state differs"));
        }
        project_plane::validate_history_transition(before, after, op, results.first().ok_or_else(|| error("missing historical project result"))?)?;
    } else if batch.operations.iter().any(|op| op.command == project_plane::CREATE || op.params.get("scope").is_some_and(|s| s.get("workspaceId").is_some())) {
        return Err(error("sequence history cannot masquerade as project metadata"));
    }
    if t.project_scope.is_none() && batch.operations.iter().any(|op| op.command == bins::COMMAND && op.params["edit"]["type"] == "group") {
        if batch.operations.len() != 1 || !batch.exact || t.result["action"] != "bin_edit" {
            return Err(error("historical group must retain one exact native gesture"));
        }
        let op = batch.operations.first().ok_or_else(|| error("missing historical group gesture"))?;
        let results = t.result["results"].as_array().filter(|results| results.len() == 1).ok_or_else(|| error("one historical group result required"))?;
        bins::validate_group_history(before, after, op, results.first().ok_or_else(|| error("missing historical group result"))?)?;
    }
    Ok(())
}
fn validate_history_links(c: &Capsule, t: &SavedTicket) -> Result<()> {
    let Some(scope) = &t.project_scope else { return Ok(()) };
    if (if t.source_library.is_some() { t.result.get("authority").is_some() } else { t.result["authority"] != "document_state" })
        || t.result.get("sequence") != Some(&json!(t.after_sequence))
    {
        return Err(error("historical project history lacks exact metadata authority/sequence state"));
    }
    let commit = t.request["commit"].as_str().and_then(|id| c.tickets.iter().find(|v| v.token == id));
    if t.source_library.is_some() && commit.is_none() {
        return Err(error("library history requires its retained exact original commit"));
    }
    if let Some(commit) = commit {
        if commit.project_scope.as_ref() != Some(scope)
            || commit.source_library.is_some() != t.source_library.is_some()
            || commit.request.get("operations").is_none()
        {
            return Err(error("historical project ownership crosses planes"));
        }
        if t.result["action"] == "undo" {
            if t.project != commit.after || t.after != commit.project {
                return Err(error("historical Undo snapshot links differ"));
            }
        } else if let Some(undo) = t.request["undo"].as_str().and_then(|id| c.tickets.iter().find(|v| v.token == id))
            && (undo.project_scope.as_ref() != Some(scope)
                || undo.source_library.is_some() != t.source_library.is_some()
                || undo.result["action"] != "undo"
                || undo.request["commit"] != commit.token
                || t.project != undo.after
                || t.after != commit.after
                || undo.project != commit.after
                || undo.after != commit.project)
        {
            return Err(error("historical Redo snapshot links differ"));
        }
    }
    Ok(())
}

fn pending_capsule(s: &Session, r: &Request, sequence: Option<ItemId>) -> Result<PendingCapsule> {
    let (pool, undo, redo) = graph(s)?;
    let mut tickets = Vec::new();
    for t in &s.agent_bridge.tickets {
        let Some((request, result)) = &t.committed else { continue };
        let Some(before) = index(&pool, &t.project) else { continue };
        let Some(after) = t.after.as_ref().and_then(|p| index(&pool, p)) else { continue };
        if matches!(&r.scope, CheckpointScope::Sequence(_)) && t.project_scope.is_some() {
            return Err(error("project-plane history requires its typed persistence contract; no sequence checkpoint fallback or dropped owned ticket"));
        }
        let sequence = t.sequence.map(|id| id.0);
        let after_sequence = t.after_sequence.map(|id| id.0);
        if t.path != s.path {
            continue;
        }
        if request.get("operations").is_none()
            && !matches!(result.get("action").and_then(Value::as_str), Some("undo" | "redo"))
            && !super::project_navigation::is_result(result)
        {
            continue;
        }
        let saved = SavedTicket {
            token: t.token.clone(),
            revision: t.revision,
            sequence,
            project: before,
            after,
            after_sequence,
            project_scope: t.project_scope.clone(),
            source_library: t.source_library.clone(),
            request: request.clone(),
            result: result.clone(),
            redo_consumed: t.redo_consumed,
        };
        tickets.push(saved);
    }
    if tickets.len() >= TICKET_LIMIT {
        return Err(error("checkpoint must retain room for the fresh restore receipt"));
    }
    let c = Capsule {
        kind: KIND.into(),
        version: if tickets.iter().any(|t| super::project_navigation::is_result(&t.result)) {
            4
        } else if tickets.iter().any(|t| t.source_library.is_some()) {
            3
        } else {
            checkpoint_version(&r.scope)
        },
        schema: filmcraft_format::SCHEMA_VERSION,
        scope: r.scope.clone(),
        workspace: r.workspace.clone(),
        checkpoint: r.checkpoint.clone(),
        path: s.path.clone().ok_or_else(|| error("saved path missing"))?,
        sequence: sequence.map(|id| id.0),
        epoch: s.agent_bridge.epoch.clone().ok_or_else(|| error("native session epoch missing"))?,
        revision: s.revision,
        projects: Vec::new(),
        undo,
        redo,
        limit: s.history.limit,
        tickets,
        owned_head: s.agent_bridge.head.as_ref().is_some_and(|(rev, p)| *rev == s.revision && p.ptr_eq(&Arc::downgrade(&s.project))),
    };
    Ok(PendingCapsule { pool, request: r.clone(), header: c })
}

// Cheap, immutable COW capture; project serialization/validation may run on the
// existing persistence worker without borrowing the live editor or media cache.
struct PendingCapsule {
    pool: Vec<Arc<filmcraft_project::Project>>,
    request: Request,
    header: Capsule,
}
impl PendingCapsule {
    fn encode(&self) -> Result<Capsule> {
        validate_graph_association(&self.pool, &self.request)?;
        let mut c = self.header.clone();
        let mut tickets = Vec::new();
        for t in &c.tickets {
            let before = self.pool.get(t.project).ok_or_else(|| error("missing captured history before-state"))?;
            let after = self.pool.get(t.after).ok_or_else(|| error("missing captured history after-state"))?;
            if !ticket_scope(before, after, &self.request, t)? {
                if matches!(&self.request.scope, CheckpointScope::Project(_)) {
                    return Err(error("complete owned ticket graph has a foreign project/sequence plane"));
                }
                continue;
            }
            if super::project_navigation::is_result(&t.result) {
                let CheckpointScope::Project(scope) = &self.request.scope else {
                    return Err(error("Root view receipts need project-scoped persistence"));
                };
                if t.project != t.after || t.project_scope.as_ref() != Some(scope) || t.source_library.is_some() || t.redo_consumed {
                    return Err(error("invalid captured Root view receipt plane or project transition"));
                }
                super::project_navigation::validate_history(
                    before,
                    &self.header.path,
                    scope,
                    &t.token,
                    t.revision,
                    t.sequence,
                    t.after_sequence,
                    &t.request,
                    &t.result,
                )?;
            }
            tickets.push(t.clone());
        }
        c.tickets = tickets;
        // A live cache eviction must refuse before creating a companion, not
        // report a successful source-history save that can never be reopened.
        for t in c.tickets.iter().filter(|t| t.source_library.is_some() && t.request.get("operations").is_none()) {
            validate_history_links(&c, t)?;
        }
        let mut budget = MAX_BYTES;
        let projects = self
            .pool
            .iter()
            .map(|p| {
                let encoded = project_bytes(p, budget)?;
                budget = budget.checked_sub(encoded.len()).ok_or_else(|| error("history snapshot budget exhausted"))?;
                String::from_utf8(encoded).map_err(|e| error(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        c.projects = projects;
        Ok(c)
    }
}
fn capsule(s: &Session, r: &Request, sequence: Option<ItemId>) -> Result<Capsule> {
    pending_capsule(s, r, sequence)?.encode()
}

fn summary(c: &Capsule, hash: &str, size: usize) -> Value {
    json!({"workspace":c.workspace,"checkpoint":c.checkpoint,"scope":c.scope,"sha256":hash,"bytes":size,
        "undoSteps":c.undo.len(),"redoSteps":c.redo.len(),"retainedTickets":c.tickets.len(),"nativeSession":c.epoch,"nativeRevision":c.revision,
        "currentProjectSha256":digest(c.projects.first().map_or(&[],|s|s.as_bytes()))})
}
fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse(SAVE, p)?;
    if r.sha256.is_some() {
        return Err(error("save does not accept an input checksum"));
    }
    let sequence = scope(s, &r, false)?;
    let target = path(s, &r)?;
    let c = capsule(s, &r, sequence)?;
    let raw = bytes(&c, MAX_BYTES)?;
    let (pool, _, _) = graph(s)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Stop can win until this exclusive file publication starts. Afterwards an
    // incomplete/lost result remains unknown; no caller may repeat the save.
    enter_publication(s, &r.token)?;
    let mut file = options.open(&target).map_err(|e| error(format!("exclusive history checkpoint: {e}; existing files are never replaced")))?;
    file.write_all(&raw).and_then(|_| file.sync_all()).map_err(|e| error(format!("history checkpoint incomplete: {e}; inspect it, do not assume success")))?;
    File::open(target.parent().ok_or_else(|| error("checkpoint parent missing"))?).and_then(|f| f.sync_all()).map_err(|e| error(e.to_string()))?;
    let hash = digest(&raw);
    s.agent_bridge.history_checkpoints.push_back(LiveGuard {
        workspace: r.workspace.clone(),
        checkpoint: r.checkpoint.clone(),
        sha256: hash.clone(),
        projects: pool.iter().map(Arc::downgrade).collect(),
        merge_key: s.history.merge_key.clone(),
        origin_epoch: None,
        identity_publication: None,
    });
    while s.agent_bridge.history_checkpoints.len() > 16 {
        s.agent_bridge.history_checkpoints.pop_front();
    }
    Ok(
        json!({"status":"saved","token":r.token,"revision":s.revision,"sequence":sequence.map(|id|id.0),"projectUnchanged":true,"historyUnchanged":true,"checkpoint":summary(&c,&hash,raw.len())}),
    )
}
fn load(path: &Path, expected: &str) -> Result<(Capsule, usize)> {
    let (c, size, _) = load_with_digest(path, Some(expected), None)?;
    Ok((c, size))
}
pub(super) fn private_native_bytes(path: &Path, maximum: usize, check: Option<&media_guard::Check>) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| error(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(error("checkpoint must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(error("history checkpoint must be private"));
        }
    }
    let file = File::open(path).map_err(|e| error(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata().map_err(|e| error(e.to_string()))?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(error("checkpoint changed while opening"));
        }
    }
    let mut raw = Vec::new();
    let mut reader = (&file).take(maximum as u64 + 1);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if let Some(check) = check {
            check.check()?;
        }
        let count = reader.read(&mut chunk).map_err(|e| error(e.to_string()))?;
        if count == 0 {
            break;
        }
        if raw.len().checked_add(count).is_none_or(|n| n > maximum) {
            return Err(error("checkpoint exceeds its byte bound"));
        }
        raw.extend_from_slice(chunk.get(..count).ok_or_else(|| error("invalid checkpoint read size"))?);
    }
    let after = file.metadata().map_err(|e| error(e.to_string()))?;
    let now = fs::symlink_metadata(path).map_err(|e| error(e.to_string()))?;
    if !now.is_file()
        || after.len() != metadata.len()
        || now.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
        || now.modified().ok() != metadata.modified().ok()
    {
        return Err(error("checkpoint changed during its complete read"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if [after, now].iter().any(|m| m.dev() != metadata.dev() || m.ino() != metadata.ino()) {
            return Err(error("checkpoint identity changed during read"));
        }
    }
    Ok(raw)
}
fn load_with_digest(path: &Path, expected: Option<&str>, check: Option<&media_guard::Check>) -> Result<(Capsule, usize, String)> {
    let raw = private_native_bytes(path, MAX_BYTES, check)?;
    let hash = digest(&raw);
    if expected.is_some_and(|value| value != hash) {
        return Err(error("checkpoint checksum differs; nothing restored"));
    }
    if let Some(check) = check {
        check.check()?;
    }
    let value: Value = serde_json::from_slice(&raw).map_err(|e| error(e.to_string()))?;
    if value.get("sequence").is_none()
        || value["tickets"].as_array().is_none_or(|tickets| {
            tickets.iter().any(|t| {
                t.get("sequence").is_none()
                    || t.get("after_sequence").is_none()
                    || value["version"] == 1 && t.get("project_scope").is_some()
                    || value["version"] != 3 && value["version"] != 4 && t.get("source_library").is_some()
            })
        })
    {
        return Err(error("explicit versioned sequence/plane state required in complete checkpoint"));
    }
    let c = serde_json::from_value(value).map_err(|e| error(e.to_string()))?;
    Ok((c, raw.len(), hash))
}
pub(super) struct PreparedHistory {
    pub(super) history: crate::History,
    pub(super) tickets: VecDeque<Ticket>,
    pub(super) owned_head: bool,
}

// Both guarded saved recovery and trusted startup handoff validate the same
// complete graph and historical receipts. This helper grants no fresh read,
// source witness, operation fence or current edit authority.
#[derive(Clone, Copy, PartialEq)]
enum HistoryValidation {
    RestoreCurrent,
    HistoricalIdentity,
}
fn restore_saved_ticket(s: &Session, t: &SavedTicket, before: &Arc<filmcraft_project::Project>, after: &Arc<filmcraft_project::Project>) -> Ticket {
    Ticket {
        token: t.token.clone(),
        revision: t.revision,
        sequence: t.sequence.map(ItemId),
        project_scope: t.project_scope.clone(),
        source_library: t.source_library.clone(),
        project: Arc::downgrade(before),
        path: s.path.clone(),
        context: Value::Null,
        committed: Some((t.request.clone(), t.result.clone())),
        after: Some(Arc::downgrade(after)),
        after_sequence: t.after_sequence.map(ItemId),
        redo_consumed: t.redo_consumed,
        media: if t.source_library.is_some() { Arc::new(media_guard::Witnesses::unqualified()) } else { Arc::default() },
        preparation: None,
    }
}
fn prepare_restore(s: &Session, r: &Request, c: &Capsule, hash: &str, sequence: Option<ItemId>) -> Result<PreparedHistory> {
    prepare_history(s, r, c, hash, sequence, HistoryValidation::RestoreCurrent)
}
fn prepare_history(s: &Session, r: &Request, c: &Capsule, hash: &str, sequence: Option<ItemId>, purpose: HistoryValidation) -> Result<PreparedHistory> {
    let current = purpose == HistoryValidation::RestoreCurrent;
    if !current && !matches!(&r.scope, CheckpointScope::Project(_)) {
        return Err(error("historical identity inspection requires an independent Root scope"));
    }
    if c.kind != KIND
        || !(c.version == checkpoint_version(&r.scope)
            || c.version == 3 && matches!(&r.scope, CheckpointScope::Project(_)) && c.tickets.iter().any(|t| t.source_library.is_some())
            || c.version == 4 && matches!(&r.scope, CheckpointScope::Project(_)) && c.tickets.iter().any(|t| super::project_navigation::is_result(&t.result)))
        || c.schema != filmcraft_format::SCHEMA_VERSION
        || c.scope != r.scope
        || c.workspace != r.workspace
        || c.checkpoint != r.checkpoint
        || Some(&c.path) != s.path.as_ref()
        || match &r.scope {
            CheckpointScope::Sequence(_) => c.sequence != sequence.map(|id| id.0),
            CheckpointScope::Project(_) => current && !valid_sequence(&s.project, c.sequence),
        }
        || c.projects.is_empty()
        || c.projects.len() > MAX_STEPS + 1
        || c.undo.len().saturating_add(c.redo.len()) > MAX_STEPS
        || !(1..=1000).contains(&c.limit)
        || c.tickets.len() >= TICKET_LIMIT
        || !filmcraft_project::ClipIdentity::valid_id(&c.epoch)
        || c.revision == 0
    {
        return Err(error("foreign, malformed or oversized checkpoint; nothing restored"));
    }
    if current && c.projects.first().map(|v| v.as_bytes()) != Some(project_bytes(&s.project, MAX_BYTES)?.as_slice()) {
        return Err(error("current saved native project differs from checkpoint; nothing restored"));
    }
    let same_session = current
        && (s.agent_bridge.epoch.as_deref() == Some(&c.epoch)
            || s.agent_bridge
                .history_checkpoints
                .iter()
                .any(|g| g.workspace == r.workspace && g.checkpoint == r.checkpoint && g.sha256 == hash && g.origin_epoch.as_deref() == Some(&c.epoch)));
    let pool = if same_session {
        let (pool, undo, redo) = graph(s)?;
        let guard = s
            .agent_bridge
            .history_checkpoints
            .iter()
            .find(|g| g.workspace == r.workspace && g.checkpoint == r.checkpoint && g.sha256 == hash)
            .ok_or_else(|| error("live checkpoint identity expired; human history was not adopted"))?;
        if guard.merge_key != s.history.merge_key
            || guard.projects.len() != pool.len()
            || !guard.projects.iter().zip(&pool).all(|(a, b)| a.ptr_eq(&Arc::downgrade(b)))
        {
            return Err(error("live document/history snapshot identity changed; nothing restored"));
        }
        let live = capsule(s, r, sequence)?;
        if c.revision != s.revision
            || c.projects != live.projects
            || c.undo != undo
            || c.redo != redo
            || c.limit != s.history.limit
            || c.owned_head != live.owned_head
        {
            return Err(error("live native history changed since checkpoint; nothing restored"));
        }
        pool
    } else {
        if current
            && (!s.history.undo.is_empty()
                || !s.history.redo.is_empty()
                || s.history.merge_key.is_some()
                || s.agent_bridge.head.is_some()
                || s.agent_bridge.tickets.iter().any(|t| t.committed.is_some())
                || s.saved_revision != s.revision)
        {
            return Err(error("new native session must be clean with no intervening history"));
        }
        let mut pool = if current { vec![s.project.clone()] } else { Vec::new() };
        for raw in c.projects.iter().skip(usize::from(current)) {
            pool.push(Arc::new(project(raw)?));
        }
        let first = pool.first().ok_or_else(|| error("missing original archived project"))?;
        if !valid_sequence(first, c.sequence) {
            return Err(error("archive view does not belong to its actual saved project"));
        }
        pool
    };
    let steps = |entries: &[Step]| -> Result<Vec<(String, Arc<filmcraft_project::Project>)>> {
        entries
            .iter()
            .map(|v| {
                if v.label.len() > 2048 {
                    return Err(error("oversized history label"));
                }
                Ok((v.label.clone(), pool.get(v.project).cloned().ok_or_else(|| error("invalid history snapshot reference"))?))
            })
            .collect()
    };
    let undo = steps(&c.undo)?;
    let redo = steps(&c.redo)?;
    let used = c.undo.iter().chain(&c.redo).map(|s| s.project).chain(std::iter::once(0)).collect::<BTreeSet<_>>();
    if used.len() != pool.len() {
        return Err(error("checkpoint has detached project snapshots"));
    }
    let mut seen = BTreeSet::new();
    validate_graph_association(&pool, r)?;
    let mut tickets = VecDeque::new();
    for t in &c.tickets {
        let before = pool.get(t.project).ok_or_else(|| error("invalid ticket before snapshot"))?;
        let after = pool.get(t.after).ok_or_else(|| error("invalid ticket after snapshot"))?;
        if super::project_navigation::is_result(&t.result) {
            let CheckpointScope::Project(scope) = &r.scope else {
                return Err(error("Root view receipts cannot enter sequence persistence"));
            };
            if c.version != 4
                || !token_ok(&t.token)
                || t.token == r.token
                || !seen.insert(t.token.clone())
                || t.project != t.after
                || t.project_scope.as_ref() != Some(scope)
                || t.source_library.is_some()
                || t.redo_consumed
                || !ticket_scope(before, after, r, t)?
            {
                return Err(error("foreign or malformed version-four Root view receipt"));
            }
            super::project_navigation::validate_history(before, &c.path, scope, &t.token, t.revision, t.sequence, t.after_sequence, &t.request, &t.result)?;
            tickets.push_back(restore_saved_ticket(s, t, before, after));
            continue;
        }

        if !token_ok(&t.token)
            || t.token == r.token
            || !seen.insert(t.token.clone())
            || t.request.get("token") != Some(&json!(t.token))
            || t.result.get("token") != Some(&json!(t.token))
            || t.result.get("status") != Some(&json!("committed"))
            || t.result.get("validated").and_then(Value::as_bool).is_none()
            || t.result.get("beforeRevision").and_then(Value::as_u64) != Some(t.revision)
            || t.result.get("revision").and_then(Value::as_u64) != t.revision.checked_add(1)
            || t.source_library.as_ref().is_some_and(|ids| !super::library_plane::valid_sources(ids))
            || t.source_library.is_some() && t.project_scope.is_none()
            || !ticket_scope(before, after, r, t)?
            || t.request.get("operations").is_none() && !matches!(t.result.get("action").and_then(Value::as_str), Some("undo" | "redo"))
        {
            return Err(error("invalid/foreign historical receipt; nothing restored"));
        }
        let results = t
            .result
            .get("results")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty() && v.len() <= admission::GROUP_LIMIT)
            .ok_or_else(|| error("malformed historical results"))?;
        if t.request.get("operations").is_some() {
            let batch: Batch = parse(RESTORE, &t.request)?;
            super::document_authority::validate(&batch)?;
            super::document_authority::receipt(&batch, &t.result)?;
            validate_ticket_batch(t, &batch, before, after)?;
            let limit = operation_limit(&batch);
            if batch.label.len() > 1024
                || batch.operations.is_empty()
                || batch.operations.len() > limit
                || results.len() > limit
                || t.result.get("validated").and_then(Value::as_bool) != Some(batch.exact)
            {
                return Err(error("malformed historical batch"));
            }
            for op in &batch.operations {
                validate(op)?;
            }
            if batch.operations.iter().any(|op| op.command == admission::COMMAND) {
                admission::validate_group(&batch.operations, batch.exact)?;
                admission::validate_results(&batch.operations, results)?;
            }
        } else {
            validate_history_links(c, t)?;
            if t.project_scope.is_some() && t.source_library.is_none() {
                let original = t.request["commit"].as_str().and_then(|id| c.tickets.iter().find(|ticket| ticket.token == id));
                if let Some(original) = original.filter(|ticket| ticket.result["action"] == "project_clear") {
                    let batch: Batch = parse(RESTORE, &original.request)?;
                    let results = original.result["results"].as_array().ok_or_else(|| error("missing original Clear history results"))?;
                    if t.result["action"] == "undo" {
                        super::project_clear::document_transition(after, before, &batch, results)?;
                    } else {
                        super::project_clear::document_transition(before, after, &batch, results)?;
                    }
                } else {
                    document_authority::transition(before, after)?;
                }
            }
            let command = t.result.get("action").and_then(Value::as_str);
            let commit =
                t.request.get("commit").and_then(Value::as_str).filter(|id| token_ok(id)).ok_or_else(|| error("missing original historical commit"))?;
            let expected = if command == Some("undo") { "undone" } else { "redone" };
            if t.result["results"][0][expected] != commit || t.result["results"].as_array().is_none_or(|a| a.len() != 1) {
                return Err(error("historical Undo/Redo correlation differs"));
            }
            let expected_keys = if command == Some("undo") { vec!["commit", "token"] } else { vec!["commit", "token", "undo"] };
            if t.request.as_object().is_none_or(|o| o.len() != expected_keys.len() || expected_keys.iter().any(|k| !o.contains_key(*k))) {
                return Err(error("malformed historical history request"));
            }
            if command == Some("redo") && t.request.get("undo") != t.result["results"][0].get("undo") {
                return Err(error("historical Redo Undo relation differs"));
            }
        }
        tickets.push_back(restore_saved_ticket(s, t, before, after));
    }
    let merge_key = if same_session { s.history.merge_key.clone() } else { None };
    validate_association(&s.project, &r.scope, &r.workspace)?;
    let prepared = PreparedHistory { history: crate::History { undo, redo, limit: c.limit, merge_key }, tickets, owned_head: c.owned_head };
    if let CheckpointScope::Sequence(scope) = &r.scope
        && !history::graph_workspace_scope(
            &s.project,
            &s.path,
            sequence.ok_or(crate::EngineError::NoSequence)?,
            &prepared.history,
            &prepared.tickets,
            prepared.owned_head,
            scope,
        )?
    {
        return Err(error("checkpoint has no current or exact owned restored original in this film"));
    }
    Ok(prepared)
}

pub(super) fn capture_workspace(s: &Session, scope: &sequences::Scope, workspace_id: &str, checkpoint: &str, hash: &str) -> Result<Capsule> {
    let sequence = workspace(s, scope)?;
    let request = Request {
        token: String::new(),
        scope: CheckpointScope::Sequence(scope.clone()),
        workspace: workspace_id.into(),
        checkpoint: checkpoint.into(),
        sha256: None,
        metadata_basis: true,
    };
    let (c, _) = load(&path(s, &request)?, hash)?;
    if s.agent_bridge.epoch.as_deref() != Some(&c.epoch) {
        return Err(error("workspace staging requires this live owner's checkpoint"));
    }
    prepare_restore(s, &request, &c, hash, Some(sequence))?;
    Ok(c)
}

pub(super) fn workspace_project(c: &Capsule) -> Result<filmcraft_project::Project> {
    project(c.projects.first().ok_or_else(|| error("workspace history has no current project"))?)
}

pub(super) fn prepare_workspace(s: &Session, c: &Capsule, scope: &sequences::Scope, workspace_id: &str, checkpoint: &str) -> Result<PreparedHistory> {
    if s.agent_bridge.epoch.as_deref() == Some(&c.epoch) {
        return Err(error("workspace adoption requires a new native session"));
    }
    let sequence = if history::has_original(&s.project, None)? { workspace(s, scope)? } else { workspace_sequence(s, scope)? };
    let request = Request {
        token: "workspace-update-private-bootstrap-00000000".into(),
        scope: CheckpointScope::Sequence(scope.clone()),
        workspace: workspace_id.into(),
        checkpoint: checkpoint.into(),
        sha256: None,
        metadata_basis: false,
    };
    prepare_restore(s, &request, c, "", Some(sequence))
}

pub(super) fn remember_workspace(s: &mut Session, workspace: &str, checkpoint: &str, hash: &str, origin_epoch: &str) -> Result<()> {
    let (pool, _, _) = graph(s)?;
    s.agent_bridge.history_checkpoints.push_back(LiveGuard {
        workspace: workspace.into(),
        checkpoint: checkpoint.into(),
        sha256: hash.into(),
        projects: pool.iter().map(Arc::downgrade).collect(),
        merge_key: s.history.merge_key.clone(),
        origin_epoch: Some(origin_epoch.into()),
        identity_publication: None,
    });
    while s.agent_bridge.history_checkpoints.len() > 16 {
        s.agent_bridge.history_checkpoints.pop_front();
    }
    Ok(())
}

pub(super) fn retain_history_media(s: &Session) {
    // Undo can restore a relative file removed or relinked in the current cut.
    // Keep the same native document-relative resolution for every graph node;
    // recording paths neither opens files nor qualifies their bytes.
    for (_, project) in s.history.undo.iter().chain(&s.history.redo) {
        s.media.add_stored(project, s.path.as_deref());
    }
}

fn restore(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse(RESTORE, p)?;
    if r.metadata_basis {
        return Err(error("restoration requires a current scoped read ticket, not a private persistence basis"));
    }
    if let Some((old, result)) = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).and_then(|t| t.committed.as_ref()) {
        return if old == p { Ok(result.clone()) } else { Err(error("restore ticket already consumed")) };
    }
    let sequence = scope(s, &r, true)?;
    let target = path(s, &r)?;
    let hash =
        r.sha256.as_deref().filter(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit())).ok_or_else(|| error("exact retained SHA-256 required"))?;
    if s.revision >= u64::MAX - 1 {
        return Err(error("native revision exhausted"));
    }
    let (c, size) = load(&target, hash)?;
    let mut prepared = prepare_restore(s, &r, &c, hash, sequence)?;
    let before_revision = s.revision;
    let result = json!({"status":"committed","action":"history_restore","token":r.token,"beforeRevision":before_revision,"revision":before_revision+1,
        "sequence":sequence.map(|id|id.0),"validated":true,"projectUnchanged":true,"results":[summary(&c,hash,size)]});
    // Publish only after complete decoding, identity/bounds and receipt checks.
    // Keep this live project Arc and view; only rehydrate the history graph.
    let fresh = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("fresh restore read disappeared"))?;
    prepared.tickets.push_back(Ticket {
        token: r.token.clone(),
        revision: before_revision,
        sequence,
        project_scope: match &r.scope {
            CheckpointScope::Project(scope) => Some(scope.clone()),
            CheckpointScope::Sequence(_) => None,
        },
        source_library: None,
        project: Arc::downgrade(&s.project),
        path: s.path.clone(),
        context: fresh.context.clone(),
        committed: Some((p.clone(), result.clone())),
        after: Some(Arc::downgrade(&s.project)),
        after_sequence: sequence,
        redo_consumed: false,
        media: fresh.media.clone(),
        preparation: None,
    });
    enter_publication(s, &r.token)?;
    s.history = prepared.history;
    retain_history_media(s);
    let clean = s.saved_revision == s.revision;
    s.agent_bridge.tickets = prepared.tickets;
    s.revision += 1;
    if clean {
        s.saved_revision = s.revision;
    }
    s.agent_bridge.head = prepared.owned_head.then(|| (s.revision, Arc::downgrade(&s.project)));
    s.events.push(crate::Event::ProjectChanged { revision: s.revision });
    Ok(result)
}
// Native journal bootstrap testimony can establish a composer's checkpoint
// identity only when the entire graph and original receipts match.
fn equivalent_journal_checkpoint(s: &Session, r: &Request, c: &Capsule, sequence: Option<ItemId>) -> Result<bool> {
    if !matches!(r.scope, CheckpointScope::Project(_))
        || c.revision != s.revision
        || c.scope != r.scope
        || c.workspace != r.workspace
        || c.checkpoint != r.checkpoint
        || c.sequence != sequence.map(|v| v.0)
    {
        return Ok(false);
    }
    let (pool, _, _) = graph(s)?;
    let original = s.agent_bridge.history_checkpoints.iter().any(|g| {
        g.workspace == r.workspace
            && g.checkpoint == r.workspace
            && g.origin_epoch.as_deref() == Some(&c.epoch)
            && g.merge_key == s.history.merge_key
            && g.projects.len() == pool.len()
            && g.projects.iter().zip(&pool).all(|(a, b)| a.ptr_eq(&Arc::downgrade(b)))
    });
    if !original {
        return Ok(false);
    }
    let mut live = capsule(s, r, sequence)?;
    live.epoch = c.epoch.clone();
    Ok(bytes(&live, MAX_BYTES)? == bytes(c, MAX_BYTES)?)
}
fn validate_retained(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse(RETAINED, p)?;
    if r.metadata_basis {
        return Err(error("retained history validation requires a current scoped read, not a private persistence basis"));
    }
    let sequence = scope(s, &r, false)?;
    validate_retained_graph(s, &r, sequence)
}
// Called only by the independently fenced paused-candidate lifecycle operation.
// A private persistence basis never becomes a scoped read or edit ticket.
pub(super) fn validate_paused_candidate_history(
    s: &mut Session,
    token: &str,
    scope: &crate::project_workspace::ProjectScope,
    checkpoint: &str,
    hash: &str,
) -> Result<Value> {
    let r = Request {
        token: token.into(),
        scope: CheckpointScope::Project(scope.clone()),
        workspace: scope.workspace_id.clone(),
        checkpoint: checkpoint.into(),
        sha256: Some(hash.into()),
        metadata_basis: false,
    };
    validate_association(&s.project, &r.scope, &r.workspace)?;
    let sequence = checkpoint_workspace(s, &r.scope, false)?;
    validate_retained_graph(s, &r, sequence)
}
fn validate_retained_graph(s: &mut Session, r: &Request, sequence: Option<ItemId>) -> Result<Value> {
    let hash =
        r.sha256.as_deref().filter(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())).ok_or_else(|| error("exact checkpoint SHA-256 required"))?;
    let (c, size) = load(&path(s, r)?, hash)?;
    let exact = s.agent_bridge.history_checkpoints.iter().find(|g| g.workspace == r.workspace && g.checkpoint == r.checkpoint && g.sha256 == hash);
    let equivalent = if let Some(guard) = exact {
        if s.agent_bridge.epoch.as_deref() != Some(&c.epoch) && guard.origin_epoch.as_deref() != Some(&c.epoch) {
            return Err(error("only a current live or verified native bootstrap graph can be retained"));
        }
        prepare_restore(s, r, &c, hash, sequence)?;
        false
    } else {
        if !equivalent_journal_checkpoint(s, r, &c, sequence)? {
            return Err(error("this native process does not retain the checkpoint graph"));
        }
        true
    };
    enter_publication(s, &r.token)?;
    if equivalent {
        remember_workspace(s, &r.workspace, &r.checkpoint, hash, &c.epoch)?;
    }
    Ok(json!({"status":"retained","token":r.token,"revision":s.revision,"nativeSession":epoch(s)?,"sequence":sequence.map(|id|id.0),
        "projectUnchanged":true,"historyUnchanged":true,"checkpoint":summary(&c,hash,size)}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InspectionBasis {
    native_session: String,
    revision: u64,
    sequence: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectionRequest {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    checkpoint: String,
    basis: InspectionBasis,
}
fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let input: InspectionRequest = parse(INSPECT, p)?;
    if p["basis"].get("sequence").is_none() || !filmcraft_project::ClipIdentity::valid_id(&input.basis.native_session) || input.basis.revision == 0 {
        return Err(bad(INSPECT, "explicit original checkpoint epoch/revision/nullable sequence required"));
    }
    let request = Request {
        token: input.token,
        scope: CheckpointScope::Project(input.scope),
        workspace: input.workspace,
        checkpoint: input.checkpoint,
        sha256: None,
        metadata_basis: false,
    };
    let sequence = scope(s, &request, true)?;
    let check = s.agent_bridge.operations.check();
    let (capsule, size, hash) = load_with_digest(&path(s, &request)?, None, check.as_deref())?;
    if capsule.epoch != input.basis.native_session || capsule.revision != input.basis.revision || capsule.sequence != input.basis.sequence {
        return Err(bad(INSPECT, "companion differs from the exact original save basis"));
    }
    // Complete native graph/recipe-shape checks only. No graph is installed,
    // no weak identity guard minted, no source opened and no recipe executed.
    if let Err(original) = prepare_restore(s, &request, &capsule, &hash, sequence) {
        // A trusted identity-decision startup has a different epoch and an
        // already hydrated graph. Inspect a lost save only when its entire
        // graph still matches that native origin. This exception is read-only;
        // restoration and retained-history ownership keep their stricter guards.
        if !identity_publication::inspect_recovered_checkpoint(s, &request, &capsule, &hash, sequence)? {
            return Err(original);
        }
    }
    if let Some(check) = check {
        check.check()?;
    }
    Ok(
        json!({"status":"read","kind":"native_history_companion","version":1,"token":request.token,"revision":s.revision,"nativeSession":epoch(s)?,"scope":request.scope,"projectPath":s.path,"sequence":sequence.map(|v|v.0),"basis":{"nativeSession":capsule.epoch,"revision":capsule.revision,"sequence":capsule.sequence},"checkpoint":summary(&capsule,&hash,size),"historyValidation":"validated_for_current_project","projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"not-applied"}),
    )
}
// Independent archival identity only. The shared complete graph validator is
// reused; its live restore/retention policy remains exclusive to prepare_restore.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceRequest {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    checkpoint: String,
    sha256: String,
    basis: InspectionBasis,
}
fn inspect_provenance(s: &mut Session, p: &Value) -> Result<Value> {
    let input: ProvenanceRequest = parse(PROVENANCE, p)?;
    if p["basis"].get("sequence").is_none()
        || !filmcraft_project::ClipIdentity::valid_id(&input.basis.native_session)
        || input.basis.revision == 0
        || input.sha256.len() != 64
        || !input.sha256.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(bad(PROVENANCE, "exact original checkpoint digest and nullable save basis required"));
    }
    let root = input.scope.clone();
    let request = Request {
        token: input.token,
        scope: CheckpointScope::Project(input.scope),
        workspace: input.workspace,
        checkpoint: input.checkpoint,
        sha256: Some(input.sha256.clone()),
        metadata_basis: false,
    };
    let sequence = scope(s, &request, true)?;
    let check = s.agent_bridge.operations.check();
    let (capsule, size, hash) = load_with_digest(&path(s, &request)?, Some(&input.sha256), check.as_deref())?;
    if capsule.epoch != input.basis.native_session || capsule.revision != input.basis.revision || capsule.sequence != input.basis.sequence {
        return Err(bad(PROVENANCE, "archive differs from the exact original save basis"));
    }
    let realms = validate_archived_provenance(s, &request, &capsule, &hash, sequence, &root)?;
    if let Some(check) = check {
        check.check()?;
    }
    Ok(json!({"status":"historical","kind":"native_checkpoint_provenance","version":1,
        "token":request.token,"revision":s.revision,"nativeSession":epoch(s)?,"scope":request.scope,"projectPath":s.path,"sequence":sequence.map(|v|v.0),
        "basis":{"nativeSession":capsule.epoch,"revision":capsule.revision,"sequence":capsule.sequence},"checkpoint":summary(&capsule,&hash,size),
        "owningRealms":realms.into_iter().map(|anchor|json!({"projectId":root.project_id,"anchorSequence":anchor})).collect::<Vec<_>>(),
        "historyValidation":"validated_archive_only","projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed",
        "mediaEvidence":false,"writeAuthority":false,"execution":"not-applied"}))
}
fn validate_archived_provenance(
    s: &Session,
    request: &Request,
    capsule: &Capsule,
    hash: &str,
    sequence: Option<ItemId>,
    root: &crate::project_workspace::ProjectScope,
) -> Result<BTreeSet<u64>> {
    let check = s.agent_bridge.operations.check();
    let prepared = prepare_history(s, request, capsule, hash, sequence, HistoryValidation::HistoricalIdentity)?;
    let first = Arc::new(project(capsule.projects.first().ok_or_else(|| error("missing archived current project"))?)?);
    let mut realms = BTreeSet::new();
    for project in std::iter::once(&first).chain(prepared.history.undo.iter().chain(&prepared.history.redo).map(|(_, p)| p)) {
        if let Some(owner) = crate::editorial_workspace::association(project)? {
            if owner.workspace_id != request.workspace || owner.scope.project_id != root.project_id {
                return Err(bad(PROVENANCE, "archived owning association is foreign to this exact Root"));
            }
            realms.insert(owner.scope.anchor_sequence);
        }
        if let Some(check) = &check {
            check.check()?;
        }
    }
    Ok(realms)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityInspection {
    token: String,
    scope: sequences::Scope,
    workspace: String,
}
// Shared preparation for the later guarded publisher. Carries the exact
// current Arc used by the validated history; never reconstruct it from a DTO.
struct PreparedIdentity {
    original: Capsule,
    candidate: Capsule,
    current: Arc<filmcraft_project::Project>,
    history: PreparedHistory,
    source_bytes: Vec<u8>,
    changed: usize,
}
fn identity_candidate(
    s: &Session,
    request: &Request,
    input: &IdentityInspection,
    sequence: Option<ItemId>,
    check: &media_guard::Check,
) -> Result<PreparedIdentity> {
    let (pool, _, _) = graph(s)?;
    for project in &pool {
        check.check()?;
        if bins::project_summary(project)["status"] != "current" || searches::project_summary(project)["status"] != "current" {
            return Err(bad(IDENTITY, "legacy organization graph is malformed or exceeds bounded depth/inventory"));
        }
    }
    let original = capsule(s, request, sequence)?;
    let source = bytes(&original, MAX_BYTES)?;
    // Work only on a bounded opaque native clone. Original pointer aliases,
    // tickets, guards and every live history node remain unchanged.
    let mut candidate: Capsule = serde_json::from_slice(&source).map_err(|e| bad(IDENTITY, e.to_string()))?;
    let target = crate::project_workspace::ProjectScope { project_id: input.scope.project_id.clone(), workspace_id: input.workspace.clone() };
    let mut changed = 0usize;
    for raw in &mut candidate.projects {
        check.check()?;
        let before = project(raw)?;
        for item in before.items.values() {
            if let Some(value) = item.metadata.get(BINDING_KEY) {
                let binding: Binding = serde_json::from_str(value).map_err(|e| bad(IDENTITY, e.to_string()))?;
                if binding.role == "original" && binding.project_id != input.scope.project_id {
                    return Err(bad(IDENTITY, "historical source belongs to another film"));
                }
            }
        }
        let next = crate::project_workspace::legacy_identity_candidate(&before, &target, ItemId(input.scope.anchor_sequence))?;
        let encoded = String::from_utf8(project_bytes(&next, MAX_BYTES)?).map_err(|e| bad(IDENTITY, e.to_string()))?;
        if encoded != *raw {
            changed = changed.checked_add(1).ok_or_else(|| bad(IDENTITY, "snapshot count overflow"))?;
        }
        *raw = encoded;
    }
    candidate.scope = CheckpointScope::Project(target.clone());
    candidate.version = 2;
    let current = candidate.projects.first().ok_or_else(|| bad(IDENTITY, "complete graph has no current project"))?;
    let mut projection = Session::default();
    projection.project = Arc::new(project(current)?);
    projection.path = s.path.clone();
    projection.saved_revision = projection.revision;
    projection.state.active_sequence = sequence;
    let target_request = Request {
        token: request.token.clone(),
        scope: CheckpointScope::Project(target.clone()),
        workspace: request.workspace.clone(),
        checkpoint: request.checkpoint.clone(),
        sha256: None,
        metadata_basis: false,
    };
    // The same restore validator proves full graph, recipe shape, collateral
    // and historical receipt links. It neither installs nor executes anything.
    let validated = prepare_restore(&projection, &target_request, &candidate, "", sequence)?;
    check.check()?;
    Ok(PreparedIdentity { original, candidate, current: projection.project, history: validated, source_bytes: source, changed })
}
fn inspect_identity(s: &mut Session, p: &Value) -> Result<Value> {
    let input: IdentityInspection = parse(IDENTITY, p)?;
    if !s.worker_available || !operation_cancellation_available(s) {
        return Err(bad(IDENTITY, "exclusive worker and independent Stop required"));
    }
    let request = Request {
        token: input.token.clone(),
        scope: CheckpointScope::Sequence(input.scope.clone()),
        workspace: input.workspace.clone(),
        checkpoint: input.workspace.clone(),
        sha256: None,
        metadata_basis: true,
    };
    let sequence = scope(s, &request, false)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| bad(IDENTITY, "current one-attempt metadata operation required"))?;
    check.check()?;
    let prepared = identity_candidate(s, &request, &input, sequence, &check)?;
    if prepared.history.history.undo.len() != prepared.original.undo.len()
        || prepared.history.history.redo.len() != prepared.original.redo.len()
        || prepared.history.tickets.len() != prepared.original.tickets.len()
        || prepared.history.owned_head != prepared.original.owned_head
    {
        return Err(bad(IDENTITY, "migration preparation would lose native ownership or history"));
    }
    let PreparedIdentity { original, candidate, current, source_bytes: source, changed, .. } = prepared;
    let projected = bytes(&candidate, MAX_BYTES)?;
    let target = crate::project_workspace::ProjectScope { project_id: input.scope.project_id.clone(), workspace_id: input.workspace.clone() };
    validate_basis(s, &request.token, &request.scope)?;
    let graph = |c: &Capsule, raw: &[u8]| json!({"sha256":digest(raw),"bytes":raw.len(),"snapshots":c.projects.len(),"undoSteps":c.undo.len(),"redoSteps":c.redo.len(),"retainedTickets":c.tickets.len(),"ownedHead":c.owned_head});
    Ok(
        json!({"status":"read","kind":"native_identity_migration_inspection","version":1,"token":request.token,"nativeSession":original.epoch,"revision":s.revision,"sequence":sequence.map(|v|v.0),"scope":input.scope,"projectScope":target,"projectPath":s.path,"sourceGraph":graph(&original,&source),"candidateGraph":graph(&candidate,&projected),"changedSnapshots":changed,"sourceProjectSha256":digest(original.projects.first().map_or(&[],|v|v.as_bytes())),"candidateProjectSha256":digest(&project_bytes(&current,MAX_BYTES)?),"historyValidation":"complete_candidate_graph","projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"not-applied"}),
    )
}
pub(super) fn commands() -> Vec<CommandSpec> {
    let mut commands = vec![
        CommandSpec {
            id: IDENTITY,
            label: "Inspect Legacy Identity Migration Graph",
            menu: &[],
            shortcut: None,
            params: r#"{token:privateBasis,scope:{projectId,anchorSequence},workspace:uuid}"#,
            enabled: always,
            run: inspect_identity,
            journal: false,
        },
        CommandSpec {
            id: INSPECT,
            label: "Inspect Saved Native History Companion",
            menu: &[],
            shortcut: None,
            params: r#"{token,scope:{projectId,workspaceId},workspace:uuid,checkpoint:uuid,basis:{nativeSession,revision,sequence:null|item}}"#,
            enabled: always,
            run: inspect,
            journal: false,
        },
        CommandSpec {
            id: PROVENANCE,
            label: "Inspect Archived Native Owning Realms",
            menu: &[],
            shortcut: None,
            params: r#"{token,scope:{projectId,workspaceId},workspace:uuid,checkpoint:uuid,sha256,basis:{nativeSession,revision,sequence:null|item}}"#,
            enabled: always,
            run: inspect_provenance,
            journal: false,
        },
        CommandSpec {
            id: RETAINED,
            label: "Validate Retained Native History",
            menu: &[],
            shortcut: None,
            params: r#"{token,scope,workspace:uuid,checkpoint:uuid,sha256}"#,
            enabled: always,
            run: validate_retained,
            journal: false,
        },
        CommandSpec {
            id: BASIS,
            label: "Read Private Native History Persistence Basis",
            menu: &[],
            shortcut: None,
            params: r#"{token:freshRandomString,scope}"#,
            enabled: always,
            run: read_basis,
            journal: false,
        },
        CommandSpec {
            id: SAVE,
            label: "Save Scoped Native History Checkpoint",
            menu: &[],
            shortcut: None,
            params: r#"{token,scope,workspace:uuid,checkpoint:uuid,metadataBasis?:bool}"#,
            enabled: always,
            run: save,
            journal: false,
        },
        CommandSpec {
            id: RESTORE,
            label: "Restore Scoped Native History Checkpoint",
            menu: &[],
            shortcut: None,
            params: r#"{token,scope,workspace:uuid,checkpoint:uuid,sha256}"#,
            enabled: always,
            run: restore,
            journal: false,
        },
    ];
    commands.extend(identity_publication::commands());
    commands.extend(project_recovery_snapshot::commands());
    commands
}

pub(super) fn open_identity_candidate(s: &mut Session, p: &Value) -> Result<Value> {
    identity_publication::open_candidate(s, p)
}

fn capture_basis(s: &Session, token: &str, scope: &CheckpointScope) -> Result<Basis> {
    let sequence = checkpoint_workspace(s, scope, false)?;
    let _source =
        s.path.as_ref().filter(|p| Path::new(p).is_absolute() && p.ends_with(".fcproj")).ok_or_else(|| bad(BASIS, "saved native workspace required"))?;
    let (pool, undo, redo) = graph(s)?;
    Ok(Basis {
        token: token.into(),
        revision: s.revision,
        sequence,
        path: s.path.clone(),
        context: context(s)?,
        scope: scope.clone(),
        projects: pool.iter().map(Arc::downgrade).collect(),
        undo,
        redo,
        limit: s.history.limit,
        merge_key: s.history.merge_key.clone(),
    })
}

pub(super) fn open_project_recovery(s: &mut Session, p: &Value) -> Result<Value> {
    project_recovery_snapshot::open(s, p)
}
pub(super) fn validate_staged_project_recovery(s: &Session) -> Result<()> {
    project_recovery_snapshot::validate_staged(s)
}

// Exclusive complete native file publication shared by identity/recovery and new projects.
pub(super) fn publish_new_native_file(path: &Path, raw: &[u8], token: &str) -> Result<()> {
    identity_publication::publish_record(path, raw, token)
}
