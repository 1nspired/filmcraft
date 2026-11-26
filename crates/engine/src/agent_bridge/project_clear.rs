//! Exact Root-only adapter for the original Project-panel Clear service.
//! Complete Project/editor prediction is descriptive historical proof, never a
//! stored recipe to execute, file qualification, clipboard restore or model RPC.
use super::{Batch, Operation, Ticket, bins, document_authority, parse, project_audit, project_plane};
use crate::{
    EditorState, EngineError, Result, Session,
    commands::bad,
    project_workspace::{self, ProjectScope},
};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub(super) const COMMAND: &str = "agent.clearProjectEntries";
const PROOF_LIMIT: usize = 4 * 1024 * 1024;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn hash(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
// BOTH fields are serde-skipped. Bind them without exporting clipboard content
// into a model, saved proof, media grant or replayable recipe.
pub(super) fn clipboard_hash(state: &EditorState) -> Result<String> {
    hash(&(&state.clipboard, &state.clipboard_markers))
}
pub(super) fn editor_hash_with(state: &EditorState, clipboard: &str) -> Result<String> {
    hash(&(state, clipboard))
}
pub(super) fn editor_hash(state: &EditorState) -> Result<String> {
    editor_hash_with(state, &clipboard_hash(state)?)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: ProjectScope,
    before_sha256: String,
    before_editor_sha256: String,
    entries: Vec<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditorProof {
    version: u8,
    before_editor: Value,
    after_editor: Value,
    clipboard_sha256: String,
}
fn request(op: &Operation) -> Result<Request> {
    let r: Request = parse(COMMAND, &op.params)?;
    if op.command != COMMAND
        || !project_workspace::valid_scope(&r.scope)
        || !valid_hash(&r.before_sha256)
        || !valid_hash(&r.before_editor_sha256)
        || r.entries.is_empty()
        || r.entries.len() > 4096
        || r.entries.iter().any(|id| !(1..=9_007_199_254_740_991).contains(id))
        || r.entries.iter().collect::<BTreeSet<_>>().len() != r.entries.len()
    {
        return Err(error("one explicit bounded Root Clear selection and full current project/editor digests required"));
    }
    Ok(r)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    request(op).map(|_| ())
}
pub(super) fn is_batch(batch: &Batch) -> bool {
    batch.operations.iter().any(|op| op.command == COMMAND)
}
pub(super) fn is_commit(ticket: &Ticket) -> bool {
    ticket.committed.as_ref().is_some_and(|(request, result)| {
        result["action"] == "project_clear"
            && request["operations"].as_array().is_some_and(|ops| ops.len() == 1 && ops.first().is_some_and(|op| op["command"] == COMMAND))
    })
}
fn prediction(before: &Project, state: &EditorState, r: &Request) -> Result<(Project, EditorState, crate::project_clear::ClearEffects)> {
    if project_workspace::identity(before)?.is_none_or(|owner| owner.scope != r.scope) {
        return Err(error("Clear requires the exact existing independent Root identity"));
    }
    crate::editorial_workspace::association(before)?;
    bins::require_visible_targets(before, &r.entries)?;
    let ids = r.entries.iter().copied().map(ItemId).collect::<Vec<_>>();
    let (mut project, effects) = crate::project_clear::predict(before, &ids)?;
    crate::clip_identity::reconcile(before, &mut project)?;
    crate::clip_identity::enforce_protection(before, &project)?;
    let mut editor = state.clone();
    editor.project_selection.clear();
    crate::editor_state::remap_committed_mask(before, &project, state, &mut editor);
    crate::editor_state::drop_stale_gap(&project, &mut editor);
    crate::editor_state::repair(&project, &mut editor);
    Ok((project, editor, effects))
}
fn receipt(
    before: &Project,
    after: &Project,
    previous: &EditorState,
    next: &EditorState,
    effects: &crate::project_clear::ClearEffects,
    r: &Request,
    clipboard: &str,
) -> Result<Value> {
    let before_hash = hash(before)?;
    let before_editor = editor_hash_with(previous, clipboard)?;
    if before_hash != r.before_sha256 || before_editor != r.before_editor_sha256 {
        return Err(error("Clear project or complete editor basis changed; read again"));
    }
    let proof = EditorProof {
        version: 1,
        before_editor: serde_json::to_value(previous).map_err(|e| error(e.to_string()))?,
        after_editor: serde_json::to_value(next).map_err(|e| error(e.to_string()))?,
        clipboard_sha256: clipboard.into(),
    };
    let result = json!({"scope":r.scope,"beforeSha256":before_hash,"afterSha256":hash(after)?,"beforeEditorSha256":before_editor,"afterEditorSha256":editor_hash_with(next,clipboard)?,"beforeSequence":previous.active_sequence,"sequence":next.active_sequence,"beforeSequenceScope":crate::editorial_workspace::association(before)?.map(|owner|owner.scope),"sequenceScope":crate::editorial_workspace::association(after)?.map(|owner|owner.scope),"entries":r.entries,
  "effects":{"bins":effects.bins,"removed":effects.removed,"offlined":effects.offlined,"retainedSubclips":effects.retained_subclips},"humanCommand":"project.delete","humanResult":{"items":effects.removed.len(),"bins":effects.bins.len(),"offline":effects.offlined.len()},"editorProof":proof});
    if project_audit::value_bytes(&result)?.len() > PROOF_LIMIT {
        return Err(error("complete private Clear proof exceeds its bounded reply size"));
    }
    Ok(result)
}
/// Historical forward predicate. No source/path I/O and no execution of saved
/// commands; exact shared pure prediction binds every declaration and effect.
pub(super) fn validate_transition(before: &Project, after: &Project, op: &Operation, result: &Value) -> Result<()> {
    let r = request(op)?;
    if project_audit::value_bytes(result)?.len() > PROOF_LIMIT {
        return Err(error("Clear proof exceeds its byte bound"));
    }
    let proof: EditorProof = parse(COMMAND, result.get("editorProof").ok_or_else(|| error("missing complete Clear editor proof"))?)?;
    if proof.version != 1 || !valid_hash(&proof.clipboard_sha256) {
        return Err(error("invalid Clear editor/clipboard proof"));
    }
    let previous: EditorState = serde_json::from_value(proof.before_editor.clone()).map_err(|e| error(e.to_string()))?;
    if serde_json::to_value(&previous).map_err(|e| error(e.to_string()))? != proof.before_editor {
        return Err(error("historical Clear editor must roundtrip exactly; unknown fields cannot disappear"));
    }
    let (predicted, next, effects) = prediction(before, &previous, &r)?;
    if project_audit::bytes(&predicted)? != project_audit::bytes(after)?
        || receipt(before, &predicted, &previous, &next, &effects, &r, &proof.clipboard_sha256)? != *result
    {
        return Err(error("Clear differs from the exact complete original human Project/editor/effects prediction"));
    }
    Ok(())
}
pub(super) fn document_transition(before: &Project, after: &Project, batch: &Batch, results: &[Value]) -> Result<()> {
    if !is_batch(batch) {
        return document_authority::transition(before, after);
    }
    let op = batch
        .operations
        .first()
        .filter(|_| batch.operations.len() == 1 && results.len() == 1)
        .ok_or_else(|| error("Clear cannot be mixed with another publication"))?;
    let r = request(op)?;
    project_plane::validate_project_batch(&r.scope, batch)?;
    validate_transition(before, after, op, results.first().ok_or_else(|| error("missing native Clear receipt"))?)
}
pub(super) fn live_basis(s: &Session, batch: &Batch) -> Result<()> {
    if is_batch(batch) {
        let r = request(batch.operations.first().ok_or_else(|| error("missing Clear operation"))?)?;
        if editor_hash(&s.state)? != r.before_editor_sha256 {
            return Err(error("human editor or clipboard changed before Clear publication; read again"));
        }
    }
    Ok(())
}
pub(super) fn owned_transition(before: &Project, after: &Project, commit: &Ticket) -> Result<()> {
    let (request, result) = commit.committed.as_ref().ok_or_else(|| error("missing original owned Clear publication"))?;
    let batch: Batch = parse(COMMAND, request)?;
    document_transition(before, after, &batch, result["results"].as_array().ok_or_else(|| error("missing owned native results"))?)
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    let r = request(op)?;
    project_plane::require_scope(s, &r.scope)?;
    let (project, editor, effects) = prediction(&s.project, &s.state, &r)?;
    let expected = receipt(&s.project, &project, &s.state, &editor, &effects, &r, &clipboard_hash(&s.state)?)?;
    let human = s.execute("project.delete", json!({"items":r.entries}))?;
    if human != expected["humanResult"]
        || project_audit::bytes(&s.project)? != project_audit::bytes(&project)?
        || editor_hash(&s.state)? != editor_hash(&editor)?
    {
        return Err(error("isolated human Clear differs from its exact predicted result; no live publication"));
    }
    Ok(expected)
}
// Called ONLY after the guarded live publication. Staging owns a separate
// cache, so cancellation/refusal cannot evict from the live pool in advance.
pub(super) fn finish_publication(s: &mut Session, result: &Value) {
    for key in ["removed", "offlined"] {
        if let Some(ids) = result["effects"][key].as_array() {
            for id in ids.iter().filter_map(Value::as_u64) {
                s.media.remove(ItemId(id));
            }
        }
    }
    crate::relink::refresh(s);
}
