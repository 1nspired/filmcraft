//! One Root-authenticated view operation through the original sequence.open
//! service. No pixels, source witness, native Undo ownership or project edit.
use super::{parse, project_audit, project_clear, project_plane, sequences};
use crate::{
    EditorState, EngineError, Result, Session,
    commands::bad,
    project_workspace::{self, ProjectScope},
};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
pub(super) const COMMAND: &str = "agent.selectProjectSequence";
pub(super) const ACTION: &str = "project_sequence_select";
const LIMIT: usize = 4 * 1024 * 1024;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: ProjectScope,
    sequence_scope: sequences::Scope,
    item: u64,
    before_sha256: String,
    before_editor_sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    version: u8,
    before_editor: Value,
    after_editor: Value,
    clipboard_sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct OpaqueProof {
    kind: String,
    version: u8,
    sha256: String,
    bytes: usize,
    data: String,
}
fn request(value: &Value) -> Result<Request> {
    let r: Request = parse(COMMAND, value)?;
    if !super::token_ok(&r.token)
        || !project_workspace::valid_scope(&r.scope)
        || !crate::editorial_workspace::valid_scope(&r.sequence_scope)
        || r.sequence_scope.project_id != r.scope.project_id
        || !(1..=9_007_199_254_740_991).contains(&r.item)
        || !valid_hash(&r.before_sha256)
        || !valid_hash(&r.before_editor_sha256)
    {
        return Err(error("exact current Root identity, owning sequence scope, target and complete project/editor digests required"));
    }
    Ok(r)
}
fn require_target(project: &Project, r: &Request) -> Result<ItemId> {
    if project_workspace::identity(project)?.is_none_or(|owner| owner.scope != r.scope)
        || crate::editorial_workspace::association(project)?.is_none_or(|owner| owner.scope != r.sequence_scope)
        || !sequences::belongs(project, ItemId(r.item), &r.sequence_scope)?
    {
        return Err(error("target is not a current member of this exact Root owning sequence realm"));
    }
    Ok(ItemId(r.item))
}
fn project_hash(project: &Project) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::bytes(project)?)))
}
fn proof(before: &EditorState, after: &EditorState, clipboard: &str) -> Result<OpaqueProof> {
    let inner = Proof {
        version: 1,
        before_editor: serde_json::to_value(before).map_err(|e| error(e.to_string()))?,
        after_editor: serde_json::to_value(after).map_err(|e| error(e.to_string()))?,
        clipboard_sha256: clipboard.into(),
    };
    // Keep native i64 editor clocks opaque across JavaScript JSON round trips.
    let data = serde_json::to_string(&inner).map_err(|e| error(e.to_string()))?;
    if data.len() > LIMIT {
        return Err(error("view proof exceeds its private byte bound"));
    }
    Ok(OpaqueProof {
        kind: "native_project_sequence_view_proof".into(),
        version: 1,
        sha256: format!("{:x}", Sha256::digest(data.as_bytes())),
        bytes: data.len(),
        data,
    })
}
fn receipt(
    project: &Project,
    path: &str,
    r: &Request,
    before: &EditorState,
    after: &EditorState,
    clipboard: &str,
    epoch: &str,
    revision: u64,
) -> Result<Value> {
    let result = json!({"status":"selected","kind":"native_project_sequence_selection","version":1,"action":ACTION,"token":r.token,
  "beforeRevision":revision,"revision":revision,"nativeSession":epoch,"scope":r.scope,"sequenceScope":r.sequence_scope,"projectPath":path,
  "previousSequence":before.active_sequence.map(|id|id.0),"sequence":r.item,"sha256":project_hash(project)?,
  "beforeEditorSha256":project_clear::editor_hash_with(before,clipboard)?,"afterEditorSha256":project_clear::editor_hash_with(after,clipboard)?,
  "humanCommand":"sequence.open","requiresFreshSequenceRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,
  "viewProof":proof(before,after,clipboard)?});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("view receipt exceeds its private byte bound"));
    }
    Ok(result)
}
pub(super) fn is_result(value: &Value) -> bool {
    value.get("action").and_then(Value::as_str) == Some(ACTION)
}
pub(super) fn select(s: &mut Session, value: &Value) -> Result<Value> {
    if cfg!(target_arch = "wasm32") || !super::operation_cancellation_available(s) {
        return Err(error("Root sequence selection requires the native independent Stop transport"));
    }
    let r = request(value)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("read the current Root before selecting its sequence"))?;
    if let Some((old, result)) = &ticket.committed {
        return if old == value && is_result(result) { Ok(result.clone()) } else { Err(error("read token already used by a different operation")) };
    }
    super::idle(s)?;
    project_plane::validate_ticket(s, &r.token, &r.scope)?;
    if project_hash(&s.project)? != r.before_sha256 || project_clear::editor_hash(&s.state)? != r.before_editor_sha256 {
        return Err(error("Root project, editor or clipboard changed; read again"));
    }
    let target = require_target(&s.project, &r)?;
    let before = s.state.clone();
    let mut expected = before.clone();
    crate::commands::open_sequence_state(&s.project, &mut expected, target)?;
    let clipboard = project_clear::clipboard_hash(&before)?;
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = String::new();
    let path = s.path.clone().ok_or_else(|| error("saved Root path required"))?;
    let result = receipt(&s.project, &path, &r, &before, &expected, &clipboard, &epoch, s.revision)?;
    #[cfg(not(target_arch = "wasm32"))]
    s.agent_bridge.operations.enter_publication(&r.token)?;
    // The exact shared human service, with its normal event and journal behavior.
    s.execute("sequence.open", json!({"item":target.0}))?;
    if let Some(ticket) = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token) {
        ticket.committed = Some((value.clone(), result.clone()));
        ticket.after = Some(Arc::downgrade(&s.project));
        ticket.after_sequence = Some(target);
    }
    Ok(result)
}
// Descriptive historical proof only. Never replay this editor state or request.
// The original immutable native companion and controller still own admission.
pub(super) fn validate_history(
    project: &Project,
    path: &str,
    scope: &ProjectScope,
    token: &str,
    revision: u64,
    before_sequence: Option<u64>,
    after_sequence: Option<u64>,
    value: &Value,
    result: &Value,
) -> Result<()> {
    let r = request(value)?;
    if &r.scope != scope
        || r.token != token
        || Some(r.item) != after_sequence
        || project_hash(project)? != r.before_sha256
        || project_audit::value_bytes(result)?.len() > LIMIT
    {
        return Err(error("foreign historical Root view identity or complete project basis"));
    }
    require_target(project, &r)?;
    let opaque: OpaqueProof = parse(COMMAND, result.get("viewProof").ok_or_else(|| error("missing historical native view proof"))?)?;
    if opaque.kind != "native_project_sequence_view_proof"
        || opaque.version != 1
        || opaque.bytes == 0
        || opaque.bytes > LIMIT
        || opaque.bytes != opaque.data.len()
        || !valid_hash(&opaque.sha256)
        || format!("{:x}", Sha256::digest(opaque.data.as_bytes())) != opaque.sha256
    {
        return Err(error("changed or oversized opaque native view proof"));
    }
    let inner: Proof = serde_json::from_str(&opaque.data).map_err(|e| error(e.to_string()))?;
    if inner.version != 1 || !valid_hash(&inner.clipboard_sha256) {
        return Err(error("invalid historical native editor/clipboard proof"));
    }
    let before: EditorState = serde_json::from_value(inner.before_editor.clone()).map_err(|e| error(e.to_string()))?;
    if serde_json::to_value(&before).map_err(|e| error(e.to_string()))? != inner.before_editor
        || before.active_sequence.map(|id| id.0) != before_sequence
        || project_clear::editor_hash_with(&before, &inner.clipboard_sha256)? != r.before_editor_sha256
    {
        return Err(error("historical editor must roundtrip exactly without unknown or dropped fields"));
    }
    let mut after = before.clone();
    crate::commands::open_sequence_state(project, &mut after, ItemId(r.item))?;
    let epoch = result
        .get("nativeSession")
        .and_then(Value::as_str)
        .filter(|value| filmcraft_project::ClipIdentity::valid_id(value))
        .ok_or_else(|| error("missing historical native session epoch"))?;
    if receipt(project, path, &r, &before, &after, &inner.clipboard_sha256, epoch, revision)? != *result {
        return Err(error("historical view differs from the exact original human sequence.open service"));
    }
    Ok(())
}
