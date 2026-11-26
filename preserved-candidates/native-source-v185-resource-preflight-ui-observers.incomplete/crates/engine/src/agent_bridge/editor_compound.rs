//! Exact editor changes with their original live trim/history effects.
//! A retained outcome is descriptive; only the fresh purpose-bound Prepared
//! owner can cross the native publication fence. Human trim Undo is never owned
//! by this operation, including when a zero Undo limit retains no entry.
use super::{
    editor_control, editor_proof,
    editor_setting::{self, Setting},
    history_checkpoint, media_guard, parse, project_audit, project_clear, project_plane,
};
use crate::{
    EditorState, EngineError, Result, Session,
    commands::bad,
    project_workspace::{self, ProjectScope},
    trim::command_settlement::{self, historical},
};
use filmcraft_project::Project;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const COMMAND: &str = editor_control::CONFIGURE;
pub(super) const ACTION: &str = "editor_control_setting";
const LIMIT: usize = 4 * 1024 * 1024;
const EDITOR_KIND: &str = "native_editor_control_editor_proof";
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: u8,
    token: String,
    scope: ProjectScope,
    before_sha256: String,
    before_editor_sha256: String,
    setting: Setting,
}
fn request(value: &Value) -> Result<Request> {
    let r: Request = parse(COMMAND, value)?;
    if r.version != 2
        || !super::token_ok(&r.token)
        || !project_workspace::valid_scope(&r.scope)
        || !editor_setting::valid_hash(&r.before_sha256)
        || !editor_setting::valid_hash(&r.before_editor_sha256)
    {
        return Err(error("explicit version-two setting, exact Root and complete original project/editor digests required"));
    }
    Ok(r)
}
use super::compound_history::Purpose;
pub(super) use super::compound_history::{Opaque, Retained, history_state};
impl Retained {
    fn capture(prepared: &command_settlement::Prepared, setting: &Setting) -> Result<Self> {
        match setting {
            Setting::DefaultTransition { .. } => Self::capture_settled(prepared),
            Setting::ClipMixerMode { .. } => Self::capture_preserved(prepared, editor_setting::human(setting).0),
        }
    }
    pub(super) fn describes_after(&self, project: &Arc<Project>, history: &crate::History, setting: &Setting) -> Result<bool> {
        let h = self.history(setting)?;
        Ok(Arc::ptr_eq(project, h.after_project()) && historical::same_history(history, h.after_history()))
    }
    fn capture_forecast(prepared: &command_settlement::Prepared) -> Result<Self> {
        Self::capture_settled(prepared)
    }
    fn history(&self, setting: &Setting) -> Result<historical::Historical> {
        self.validate(editor_setting::human(setting).0)
    }
    fn proof(&self, setting: &Setting, check: Option<&media_guard::Check>) -> Result<Value> {
        self.proof_command(editor_setting::human(setting).0, check)
    }
    fn proof_command(&self, command: &str, check: Option<&media_guard::Check>) -> Result<Value> {
        self.proof_for(command, Purpose::EditorControl, check)
    }
    fn from_proof(value: &Value, pool: &[Arc<Project>], mapping: &[usize], setting: &Setting, check: Option<&media_guard::Check>) -> Result<Self> {
        Self::from_command_proof(value, pool, mapping, editor_setting::human(setting).0, check)
    }
    fn from_command_proof(value: &Value, pool: &[Arc<Project>], mapping: &[usize], command: &str, check: Option<&media_guard::Check>) -> Result<Self> {
        Self::from_for(value, pool, mapping, command, Purpose::EditorControl, check)
    }
}
fn trim_summary(history: &historical::Historical, settle: bool) -> Value {
    let x = history.summary();
    json!({"settlementApplied":settle,"dynamicFinished":x.dynamic_finished,"dynamicCommitted":x.dynamic_committed,
        "undoEntryRetained":x.undo_entry_retained,"redoDiscarded":x.redo_discarded,"loopStopped":x.loop_stopped,
        "restoredBeforeProject":x.restored_before_project,"offsetTicks":x.offset.0.to_string(),"label":x.label})
}
fn expected(retained: &Retained, before: &EditorState, r: &Request) -> Result<(historical::Historical, EditorState)> {
    let h = retained.history(&r.setting)?;
    h.validate_editor_basis(before.active_sequence, &before.edit_points, before.trim_shift)?;
    let mut settled = before.clone();
    settled.trim_shift = h.trim_shift_after();
    let after = editor_setting::prepare(h.after_project(), &settled, &r.scope, &r.setting)?.preview_for(h.after_project(), &settled)?;
    Ok((h, after))
}
fn receipt(
    retained: &Retained,
    path: &str,
    r: &Request,
    before: &EditorState,
    clipboard: &str,
    epoch: &str,
    revision: u64,
    check: Option<&media_guard::Check>,
) -> Result<Value> {
    let (h, after) = expected(retained, before, r)?;
    let before_hash = project_clear::editor_hash_with(before, clipboard)?;
    let after_hash = project_clear::editor_hash_with(&after, clipboard)?;
    let settle = matches!(&r.setting, Setting::DefaultTransition { .. });
    let project_same = Arc::ptr_eq(h.before_project(), h.after_project());
    let summary = h.summary();
    let result = json!({"status":"configured","kind":"native_editor_control_setting","version":2,"action":ACTION,"token":r.token,
        "beforeRevision":revision,"revision":revision,"nativeSession":epoch,"scope":r.scope,"projectPath":path,
        "sequence":before.active_sequence.map(|id|id.0),"beforeProjectSha256":project_plane::project_basis(h.before_project())?,"sha256":project_plane::project_basis(h.after_project())?,
        "beforeEditorSha256":before_hash,"afterEditorSha256":after_hash,"humanCommand":editor_setting::human(&r.setting).0,"setting":r.setting,
        "changed":before_hash!=after_hash||!project_same||summary.dynamic_finished||summary.loop_stopped,
        "projectUnchanged":project_same,"historyUnchanged":!summary.dynamic_committed,
        "beforeHistory":history_state(h.before_project(),h.before_history())?,"afterHistory":history_state(h.after_project(),h.after_history())?,"trim":trim_summary(&h,settle),
        "trimOwnerPreserved":!settle,"agentTimelineCommits":0,"agentUndoOwned":false,
        "requiresFreshEditorRead":true,"requiresFreshProjectRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,
        "editorProof":editor_proof::encode(before,&after,clipboard,EDITOR_KIND,LIMIT,COMMAND)?,"historyProof":retained.proof(&r.setting,check)?});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("complete compound editor receipt exceeds4MiB; nothing published"));
    }
    Ok(result)
}
pub(super) fn is_result(value: &Value) -> bool {
    value.get("action").and_then(Value::as_str) == Some(ACTION) || following::is_result(value)
}
pub(super) fn retained(t: &super::Ticket) -> Result<&Retained> {
    t.editor_control
        .as_ref()
        .filter(|o| o.change)
        .and_then(|o| o.outcome.as_ref())
        .ok_or_else(|| error("compound outcome lacks its exact retained native evidence roots"))
}
pub(super) fn configure(s: &mut Session, value: &Value) -> Result<Value> {
    if !super::operation_cancellation_available(s) {
        return Err(error("compound editor settings require native independent Stop"));
    }
    if value.get("version").and_then(Value::as_u64) == Some(3) {
        return following::configure(s, value);
    }
    let r = request(value)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("fresh change-purpose editor read required"))?;
    // Known outcomes reconcile before looking at later human state; never replay.
    if let Some((old, result)) = &t.committed {
        return if old == value && is_result(result) { Ok(result.clone()) } else { Err(error("read token already consumed by another operation")) };
    }
    let owner = t
        .editor_control
        .as_ref()
        .filter(|o| o.change && o.following.is_none())
        .ok_or_else(|| error("readonly, ordinary or source tickets cannot publish an editor-control change"))?;
    let prepared = owner.settlement.as_ref().ok_or_else(|| error("historical outcomes cannot reconstruct a live editor preparation"))?.clone();
    super::check_ticket_editor_basis(s, &r.token, false)?;
    project_plane::require_scope(s, &r.scope)?;
    if t.project_scope.as_ref() != Some(&r.scope)
        || t.source_library.is_some()
        || project_plane::basis(s)? != r.before_sha256
        || project_clear::editor_hash(&s.state)? != r.before_editor_sha256
    {
        return Err(error("original exact Root, project, editor or private clipboard changed"));
    }
    owner.validate(s)?;
    publish_prepared(s, value, &r, prepared, None)
}
// The same original human service and one publication fence for both purposes.
// A following caller supplies its separately checked ORIGINAL/current basis;
// it never rewrites an old strict request or borrows its old authority.
fn publish_prepared(
    s: &mut Session,
    value: &Value,
    r: &Request,
    prepared: command_settlement::Prepared,
    following_basis: Option<following::PreparedBasis>,
) -> Result<Value> {
    let effect = super::compound_publish::Effect::EditorSetting(&r.setting);
    super::compound_publish::validate(s, &effect)?;
    let mut retained = Retained::capture(&prepared, &r.setting)?;
    let before = s.state.clone();
    let clipboard = project_clear::clipboard_hash(&before)?;
    let epoch = history_checkpoint::epoch(s)?;
    let path = s.path.clone().ok_or_else(|| error("saved owning Root path required"))?;
    let check = s.agent_bridge.operations.check();
    let publication = receipt(&retained, &path, r, &before, &clipboard, &epoch, s.revision, check.as_deref())?;
    let result = if let Some(basis) = following_basis.as_ref() { following::receipt(value, basis, publication, check.as_deref())? } else { publication };
    if let Some(basis) = following_basis.as_ref() {
        following::history::validate_publication(
            value,
            basis,
            &retained,
            &result,
            &path,
            &r.scope,
            &r.token,
            s.revision,
            before.active_sequence.map(|id| id.0),
            check.as_deref(),
        )?;
    }
    let follows = following_basis.is_some();
    let post_binding = if follows {
        let (_, editor) = expected(&retained, &before, r)?;
        Some(crate::trim::clock::Following::after_command(s, &prepared, &editor, matches!(&r.setting, Setting::ClipMixerMode { .. }))?)
    } else {
        None
    };
    if let Some(basis) = following_basis {
        retained.original = Some(Box::new(basis.history));
    }
    let publication_before = Arc::downgrade(&s.project);
    let publication_revision = s.revision;
    let publication_context = if follows { Some(super::context(s)?) } else { None };
    super::compound_publish::publish(s, &r.token, prepared, &effect)?;
    let after = Arc::downgrade(&s.project);
    let sequence = s.state.active_sequence;
    let t = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token).ok_or_else(|| error("native compound outcome ticket disappeared"))?;
    let owner = t.editor_control.as_mut().ok_or_else(|| error("native compound purpose disappeared"))?;
    owner.settlement = None;
    owner.outcome = Some(retained);
    owner.post_binding = post_binding;
    if let Some(context) = publication_context {
        t.project = publication_before;
        t.revision = publication_revision;
        t.context = context;
    }
    t.committed = Some((value.clone(), result.clone()));
    t.after = Some(after);
    t.after_sequence = sequence;
    Ok(result)
}
// Immutable exact historical validation. No operation fence, current state
// mutation, reconstructed live owner, source witness or human Undo ownership.
pub(super) fn validate_history(
    pool: &[Arc<Project>],
    mapping: &[usize],
    before: &Arc<Project>,
    after: &Arc<Project>,
    path: &str,
    scope: &ProjectScope,
    token: &str,
    revision: u64,
    before_sequence: Option<u64>,
    after_sequence: Option<u64>,
    value: &Value,
    result: &Value,
    check: Option<&media_guard::Check>,
) -> Result<Retained> {
    let r = request(value)?;
    if r.token != token
        || &r.scope != scope
        || before_sequence != after_sequence
        || project_plane::project_basis(before)? != r.before_sha256
        || project_audit::value_bytes(result)?.len() > LIMIT
    {
        return Err(error("foreign historical compound identity or project basis"));
    }
    let retained = Retained::from_proof(result.get("historyProof").ok_or_else(|| error("missing compound history proof"))?, pool, mapping, &r.setting, check)?;
    let h = retained.history(&r.setting)?;
    if !Arc::ptr_eq(h.before_project(), before) || !Arc::ptr_eq(h.after_project(), after) {
        return Err(error("compound local-to-capsule snapshot relations differ"));
    }
    let decoded = editor_proof::decode(result.get("editorProof").ok_or_else(|| error("missing compound editor proof"))?, EDITOR_KIND, LIMIT, COMMAND)?;
    if decoded.before.active_sequence.map(|id| id.0) != before_sequence
        || project_clear::editor_hash_with(&decoded.before, &decoded.clipboard_sha256)? != r.before_editor_sha256
    {
        return Err(error("historical compound original editor/clipboard basis differs"));
    }
    let (_, expected) = expected(&retained, &decoded.before, &r)?;
    editor_proof::require_after(&decoded.after_editor, &expected, COMMAND)?;
    let epoch = result
        .get("nativeSession")
        .and_then(Value::as_str)
        .filter(|v| filmcraft_project::ClipIdentity::valid_id(v))
        .ok_or_else(|| error("missing original compound native epoch"))?;
    if receipt(&retained, path, &r, &decoded.before, &decoded.clipboard_sha256, epoch, revision, check)? != *result {
        return Err(error("historical compound result differs from the exact original native services"));
    }
    Ok(retained)
}
#[cfg(test)]
mod tests;

pub(super) mod following;
