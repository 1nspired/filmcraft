//! A distinct native read of live controls; never an idle project/source read.
//! Its in-memory owner cannot be restored from a history file or a receipt.
use super::{Ticket, context, fresh_ticket_token, media_guard, parse, project_audit, project_clear, project_plane};
use crate::{
    EngineError, Result, Session,
    commands::bad,
    project_workspace::ProjectScope,
    trim::{clock, command_settlement},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const READ: &str = "agent.readEditorControl";
#[cfg(not(target_arch = "wasm32"))]
pub(super) const PREPARE: &str = "agent.prepareEditorControl";
pub(super) const CONFIGURE: &str = "agent.configureEditorControl";
pub(super) const OUTCOME: &str = "agent.readEditorControlOutcome";
const DEFAULT: &str = "effects.setDefaultTransition";
fn error(message: impl Into<String>) -> EngineError {
    bad(READ, message)
}

// Boxed on the original Ticket; neither a second owner store nor serializable authority.
pub(super) struct ReadOwner {
    pub(super) settlement: Option<command_settlement::Prepared>,
    pub(super) change: bool,
    pub(super) following: Option<clock::Following>,
    observation: Option<Value>,
    pub(super) post_binding: Option<clock::Following>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) outcome: Option<super::editor_compound::Retained>,
}
impl ReadOwner {
    fn capture(s: &Session) -> Result<Self> {
        // Read-only lifecycle projections need not acquire a competing writer.
        let settlement = if s.is_display_only() { None } else { Some(command_settlement::prepare(s, DEFAULT)?) };
        Ok(Self {
            settlement,
            change: false,
            following: None,
            observation: None,
            post_binding: None,
            #[cfg(not(target_arch = "wasm32"))]
            outcome: None,
        })
    }
    fn summary(&self) -> Value {
        self.settlement.as_ref().map_or(Value::Null, |p| settlement_json(p.summary()))
    }
    pub(super) fn validate(&self, s: &Session) -> Result<()> {
        if let Some(plan) = &self.following {
            plan.prepare_current(s)?;
        } else if let Some(plan) = &self.settlement {
            plan.validate(s)?;
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClockPolicy {
    OriginalHumanTrim,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    token: String,
    scope: ProjectScope,
    project_path: String,
    #[serde(default)]
    clock_policy: Option<ClockPolicy>,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    read_for(s, p, false)
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    if !super::operation_cancellation_available(s) {
        return Err(error("editor change preparation requires the independent native Stop transport"));
    }
    read_for(s, p, true)
}
fn read_for(s: &mut Session, p: &Value, change: bool) -> Result<Value> {
    if cfg!(target_arch = "wasm32") {
        return Err(error("live editor-control identity requires a native engine"));
    }
    let r: Read = parse(READ, p)?;
    if p.get("clockPolicy").is_some_and(Value::is_null) || r.clock_policy.is_some() && !change {
        return Err(error("an explicit following policy belongs only to native change preparation"));
    }
    fresh_ticket_token(s, &r.token)?;
    project_plane::require_scope(s, &r.scope)?;
    if r.project_path.len() > 4096
        || !r.project_path.starts_with('/')
        || r.project_path.chars().any(char::is_control)
        || s.path.as_ref() != Some(&r.project_path)
    {
        return Err(error("the exact current saved Root path is required"));
    }
    let mut owner = ReadOwner::capture(s)?;
    if change && owner.settlement.is_none() {
        return Err(error("a live native command owner is required for change preparation"));
    }
    owner.change = change;
    let following = r.clock_policy.is_some();
    if following {
        owner.following = Some(clock::Following::capture(s)?);
    }
    let sha = project_plane::basis(s)?;
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = String::new();
    let result = document(s, &r, &owner, change, following, &sha, &epoch)?;
    if following {
        owner.observation = Some(result.clone());
    }
    owner.validate(s)?;
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
            editor_control: Some(Box::new(owner)),
            #[cfg(not(target_arch = "wasm32"))]
            sequence_views: None,
            project: Arc::downgrade(&s.project),
            path: s.path.clone(),
            context: context(s)?,
            committed: None,
            after: None,
            after_sequence: None,
            redo_consumed: false,
            media: Arc::new(media_guard::Witnesses::default()),
            #[cfg(not(target_arch = "wasm32"))]
            preparation: None,
        },
    )?;
    Ok(result)
}
fn document(s: &Session, r: &Read, owner: &ReadOwner, change: bool, following: bool, sha: &str, epoch: &str) -> Result<Value> {
    let clipboard = project_clear::clipboard_hash(&s.state)?;
    encode_document(Document {
        token: &r.token,
        scope: &r.scope,
        path: &r.project_path,
        project: &s.project,
        state: &s.state,
        history: &s.history,
        project_sha256: sha,
        clipboard_sha256: &clipboard,
        revision: s.revision,
        saved_revision: s.saved_revision,
        epoch,
        dynamic: s.trim_play.dynamic.is_some(),
        around: s.trim_play.around.is_some(),
        settlement: owner.summary(),
        change,
        following,
        following_write: true,
        cancellation: super::operation_cancellation_available(s),
    })
}
pub(in crate::agent_bridge) struct Document<'a> {
    pub(in crate::agent_bridge) token: &'a str,
    pub(in crate::agent_bridge) scope: &'a ProjectScope,
    pub(in crate::agent_bridge) path: &'a str,
    pub(in crate::agent_bridge) project: &'a Arc<filmcraft_project::Project>,
    pub(in crate::agent_bridge) state: &'a crate::EditorState,
    pub(in crate::agent_bridge) history: &'a crate::History,
    pub(in crate::agent_bridge) project_sha256: &'a str,
    pub(in crate::agent_bridge) clipboard_sha256: &'a str,
    pub(in crate::agent_bridge) revision: u64,
    pub(in crate::agent_bridge) saved_revision: u64,
    pub(in crate::agent_bridge) epoch: &'a str,
    pub(in crate::agent_bridge) dynamic: bool,
    pub(in crate::agent_bridge) around: bool,
    pub(in crate::agent_bridge) settlement: Value,
    pub(in crate::agent_bridge) change: bool,
    pub(in crate::agent_bridge) following: bool,
    pub(in crate::agent_bridge) following_write: bool,
    pub(in crate::agent_bridge) cancellation: bool,
}
pub(in crate::agent_bridge) fn encode_document(d: Document<'_>) -> Result<Value> {
    let result = json!({"status":"read","kind":"native_editor_control_read","version":if d.following {3}else if d.change {2}else{1},"protocol":1,
        "token":d.token,"revision":d.revision,"nativeSession":d.epoch,"scope":d.scope,
        "project":{"path":d.path,"dirty":d.revision!=d.saved_revision},
        "sequence":d.state.active_sequence.map(|id|id.0),"editorStateSha256":project_clear::editor_hash_with(d.state,d.clipboard_sha256)?,
        "projectState":{"sha256":d.project_sha256,"items":d.project.items.len(),"sequences":d.project.sequences().count()},
        "trim":{"active":d.dynamic||d.around,"dynamic":d.dynamic,"loop":d.around,"settlement":d.settlement},
        "capabilities":{"editorControlRead":true,"editorSettingsRead":true,"editorTransitionCatalog":true,
            "compoundEditorWrite":d.change&&(!d.following||d.following_write),"operationCancellation":d.cancellation},
        "readPurpose":if d.following {"editor_setting_clock_following"}else if d.change {"editor_setting_preparation"}else{"editor_controls_only"},
        "requiresFreshProjectRead":true,"referenceScope":"exact_read_ticket_only","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    #[cfg(not(target_arch = "wasm32"))]
    let result = if d.change {
        let mut result = result;
        result["historyState"] = super::editor_compound::history_state(d.project, d.history)?;
        result
    } else {
        result
    };
    let result = if d.following {
        let mut result = result;
        result["clockPolicy"] = json!("original_human_trim");
        result
    } else {
        result
    };
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("live editor metadata exceeds the bounded64KiB reply"));
    }
    Ok(result)
}
pub(in crate::agent_bridge) fn settlement_json(x: &command_settlement::Summary) -> Value {
    json!({"dynamicFinished":x.dynamic_finished,"dynamicCommitted":x.dynamic_committed,"undoEntryRetained":x.undo_entry_retained,
        "redoDiscarded":x.redo_discarded,"loopStopped":x.loop_stopped,"restoredBeforeProject":x.restored_before_project,
        "offsetTicks":x.offset.0.to_string(),"label":x.label})
}

/// An explicit finite metadata hand, even when the live gesture later stops.
/// In particular, neither old settings mutations nor checkpoint operations borrow it.
pub(super) fn command_scope(s: &Session, command: &str, p: &Value) -> Result<()> {
    let control = p
        .get("token")
        .and_then(Value::as_str)
        .and_then(|token| s.agent_bridge.tickets.iter().find(|t| t.token == token))
        .and_then(|t| t.editor_control.as_ref());
    if let Some(owner) = control {
        let metadata =
            matches!(command, READ | OUTCOME | super::editor_settings::READ | super::editor_setting::CATALOG | "agent.receipt" | "agent.cancelSnapshot");
        if !metadata && !(owner.change && command == CONFIGURE && (owner.following.is_none() || p.get("version").and_then(Value::as_u64) == Some(3))) {
            return Err(error("an editor-only ticket cannot authorize project, source, timeline, history or persistence operations"));
        }
    }
    Ok(())
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

#[cfg(not(target_arch = "wasm32"))]
impl ReadOwner {
    pub(super) fn restored(outcome: super::editor_compound::Retained) -> Self {
        Self { settlement: None, change: true, following: None, observation: None, post_binding: None, outcome: Some(outcome) }
    }
}

pub(super) mod following;

#[cfg(not(target_arch = "wasm32"))]
mod outcome;
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_outcome(s: &mut Session, p: &Value) -> Result<Value> {
    outcome::read(s, p)
}
