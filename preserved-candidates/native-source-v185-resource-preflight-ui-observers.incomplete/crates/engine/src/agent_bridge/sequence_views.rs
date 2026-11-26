//! Distinct Root view preparation, using the original ticket and human clock.
//! No publisher, source witness, project edit, Undo or serialized live owner.
use super::{Ticket, context, editor_control, fresh_ticket_token, media_guard, parse, project_audit, project_clear, project_plane, sequences};
use crate::{
    EngineError, Result, Session,
    commands::bad,
    project_workspace::ProjectScope,
    trim::{
        clock::{Following, SequenceViewCommand},
        command_settlement,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const PREPARE: &str = "agent.prepareSequenceViews";
pub(super) const COMMAND: &str = "agent.navigateSequenceView";
pub(super) const READ: &str = "agent.readSequenceViews";
pub(super) const OBSERVE: &str = "agent.observeSequenceViews";
const LIMIT: usize = 4 * 1024 * 1024;
const ROW_LIMIT: usize = 4096;
fn error(message: impl Into<String>) -> EngineError {
    bad(READ, message)
}

// Private in-memory authority is retained only by the original native Ticket.
// Neither history restore nor an opaque receipt can reconstruct this owner.
pub(super) struct ReadOwner {
    pub(super) following: Option<Following>,
    // Physical application preparation only; never serialized or reconstructed.
    pub(super) application_scope: Option<std::sync::Weak<()>>,
    pub(super) original: Value,
    pub(super) original_settlement: Option<command_settlement::Prepared>,
    pub(super) original_editor: Option<Box<crate::EditorState>>,
    pub(super) original_saved_revision: u64,
    pub(super) change: bool,
    pub(super) outcome: Option<super::compound_history::Retained>,
}
impl ReadOwner {
    pub(super) fn restored(outcome: super::compound_history::Retained) -> Self {
        Self {
            following: None,
            application_scope: None,
            original: Value::Null,
            original_settlement: None,
            original_editor: None,
            original_saved_revision: 0,
            change: true,
            outcome: Some(outcome),
        }
    }
    pub(super) fn current(&self, s: &Session, command: SequenceViewCommand) -> Result<command_settlement::Prepared> {
        self.following.as_ref().ok_or_else(|| error("historical view records cannot recreate a live clock"))?.prepare_current_sequence_view(s, command)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    token: String,
    scope: ProjectScope,
    project_path: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observe {
    token: String,
    scope: ProjectScope,
    before_sha256: String,
    before_editor_sha256: String,
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) struct Document<'a> {
    pub(super) project: &'a Arc<filmcraft_project::Project>,
    pub(super) state: &'a crate::EditorState,
    pub(super) clipboard_sha256: &'a str,
    pub(super) history: &'a crate::History,
    pub(super) path: &'a str,
    pub(super) scope: &'a ProjectScope,
    pub(super) token: &'a str,
    pub(super) epoch: &'a str,
    pub(super) revision: u64,
    pub(super) saved_revision: u64,
    pub(super) active: bool,
    pub(super) summary: &'a command_settlement::Summary,
    pub(super) change: bool,
    pub(super) cancellation: bool,
}
fn document(s: &Session, token: &str, scope: &ProjectScope, epoch: &str, prepared: &command_settlement::Prepared, change: bool) -> Result<Value> {
    let clipboard = project_clear::clipboard_hash(&s.state)?;
    encode_document(Document {
        project: &s.project,
        state: &s.state,
        clipboard_sha256: &clipboard,
        history: &s.history,
        path: s.path.as_deref().ok_or_else(|| error("saved Root path required"))?,
        scope,
        token,
        epoch,
        revision: s.revision,
        saved_revision: s.saved_revision,
        active: s.trim_play.active(),
        summary: prepared.summary(),
        change,
        cancellation: super::operation_cancellation_available(s),
    })
}
pub(super) fn encode_document(d: Document<'_>) -> Result<Value> {
    project_plane::require_scope_for(d.project, d.state, d.scope)?;
    if d.state.open_sequences.len() > ROW_LIMIT || d.project.sequences().count() > ROW_LIMIT {
        return Err(error("complete sequence/tab inventory exceeds4096 rows; no partial view preparation"));
    }
    let association = crate::editorial_workspace::association(d.project)?.map(|a| a.scope);
    let mut rows = Vec::new();
    for item in d.project.sequences() {
        if !(1..=9_007_199_254_740_991).contains(&item.id.0) || item.name.len() > 1024 {
            return Err(error("exact sequence identity or name exceeds the view metadata bound"));
        }
        let owned = match &association {
            Some(a) => sequences::belongs(d.project, item.id, a)?,
            None => false,
        };
        rows.push(json!({"item":item.id.0,"name":item.name,"owned":owned}));
    }
    let mut tabs = Vec::with_capacity(d.state.open_sequences.len());
    for (index, id) in d.state.open_sequences.iter().enumerate() {
        let row = rows.iter().find(|r| r["item"].as_u64() == Some(id.0)).ok_or_else(|| error("an open tab has no exact current project sequence"))?;
        tabs.push(json!({"index":index,"item":id.0,"owned":row["owned"]}));
    }
    let result = json!({"status":"read","kind":"native_sequence_view_preparation","version":if d.change {2}else{1},"protocol":1,"token":d.token,
        "readPurpose":if d.change {"sequence_view_change_preparation"}else{"sequence_views_only"},"clockPolicy":"original_human_trim","nativeSession":d.epoch,"scope":d.scope,"sequenceScope":association,
        "project":{"path":d.path,"dirty":d.revision!=d.saved_revision},"revision":d.revision,"sequence":d.state.active_sequence.map(|id|id.0),
        "projectState":{"sha256":project_plane::project_basis(d.project)?},"editorStateSha256":project_clear::editor_hash_with(d.state,d.clipboard_sha256)?,
        "historyState":super::compound_history::history_state(d.project,d.history)?,"sequences":rows,"openTabs":tabs,
        "humanCommands":SequenceViewCommand::ALL.map(SequenceViewCommand::id),
        "trim":{"active":d.active,"settlement":editor_control::settlement_json(d.summary)},
        "capabilities":{"sequenceViewsRead":true,"sequenceViewPublication":d.change,"operationCancellation":d.cancellation},
        "requiresFreshProjectRead":true,"requiresFreshSequenceRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("complete sequence-view metadata exceeds4MiB"));
    }
    Ok(result)
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    read_for(s, p, false)
}
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    read_for(s, p, true)
}
fn read_for(s: &mut Session, p: &Value, change: bool) -> Result<Value> {
    if !super::operation_cancellation_available(s) {
        return Err(error("view preparation requires the independent native Stop transport"));
    }
    let r: Read = parse(READ, p)?;
    fresh_ticket_token(s, &r.token)?;
    project_plane::require_scope(s, &r.scope)?;
    if r.project_path.len() > 4096
        || !r.project_path.starts_with('/')
        || r.project_path.chars().any(char::is_control)
        || s.path.as_ref() != Some(&r.project_path)
    {
        return Err(error("the exact current saved Root path is required"));
    }
    let following = Following::capture_sequence_views(s)?;
    let prepared = following.prepare_current_sequence_view(s, SequenceViewCommand::Open)?;
    let epoch = super::history_checkpoint::epoch(s)?;
    let result = document(s, &r.token, &r.scope, &epoch, &prepared, change)?;
    let owner = ReadOwner {
        following: Some(following),
        application_scope: None,
        original: result.clone(),
        original_settlement: change.then_some(prepared),
        original_editor: change.then(|| Box::new(s.state.clone())),
        original_saved_revision: s.saved_revision,
        change,
        outcome: None,
    };
    let _ = owner.current(s, SequenceViewCommand::Open)?;
    super::register_ticket(
        s,
        Ticket {
            token: r.token,
            revision: s.revision,
            sequence: s.state.active_sequence,
            project_scope: Some(r.scope),
            source_library: None,
            #[cfg(not(target_arch = "wasm32"))]
            multicam_history: None,
            multicam_production: None,
            editor_control: None,
            sequence_views: Some(Box::new(owner)),
            project: Arc::downgrade(&s.project),
            path: s.path.clone(),
            context: context(s)?,
            committed: None,
            after: None,
            after_sequence: None,
            redo_consumed: false,
            media: Arc::new(media_guard::Witnesses::default()),
            preparation: None,
        },
    )?;
    Ok(result)
}
pub(super) fn observe(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Observe = parse(OBSERVE, p)?;
    if !super::token_ok(&r.token) || !valid_hash(&r.before_sha256) || !valid_hash(&r.before_editor_sha256) {
        return Err(error("exact original view token and complete native project/editor digests required"));
    }
    project_plane::require_scope(s, &r.scope)?;
    if !super::operation_cancellation_available(s) {
        return Err(error("the original Stop transport is required"));
    }
    s.agent_bridge.operations.preparation_stop(&r.token)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("expired sequence-view read"))?;
    let owner = t.sequence_views.as_ref().ok_or_else(|| error("a distinct original sequence-view read is required"))?;
    if t.committed.is_some() || t.project_scope.as_ref() != Some(&r.scope) || t.source_library.is_some() || t.editor_control.is_some() || t.path != s.path {
        return Err(error("foreign, consumed or changed sequence-view Root purpose"));
    }
    if owner.original["projectState"]["sha256"] != r.before_sha256 || owner.original["editorStateSha256"] != r.before_editor_sha256 {
        return Err(error("view observation must retain the ORIGINAL native digests"));
    }
    let prepared = owner.current(s, SequenceViewCommand::Open)?;
    let epoch = s.agent_bridge.epoch.as_deref().ok_or_else(|| error("the original native epoch is missing"))?;
    if owner.original["nativeSession"] != epoch {
        return Err(error("the original native epoch changed"));
    }
    let current = document(s, &r.token, &r.scope, epoch, &prepared, owner.change)?;
    s.agent_bridge.operations.preparation_stop(&r.token)?;
    let result = json!({"status":"current","kind":"native_sequence_view_observation","version":1,"original":owner.original,"current":current});
    if project_audit::value_bytes(&result)?.len() > LIMIT.saturating_mul(2) {
        return Err(error("original/current sequence-view metadata exceeds8MiB"));
    }
    Ok(result)
}
pub(super) fn command_scope(s: &Session, command: &str, p: &Value) -> Result<()> {
    let owned = p
        .get("token")
        .and_then(Value::as_str)
        .and_then(|token| s.agent_bridge.tickets.iter().find(|t| t.token == token))
        .and_then(|t| t.sequence_views.as_ref());
    if let Some(owner) = owned {
        if !matches!(command, OBSERVE | "agent.receipt" | "agent.cancelSnapshot") && !(owner.change && command == COMMAND) {
            return Err(error("a view-only ticket cannot authorize settings, source, timeline, history, project or persistence operations"));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;

pub(super) mod publication;
