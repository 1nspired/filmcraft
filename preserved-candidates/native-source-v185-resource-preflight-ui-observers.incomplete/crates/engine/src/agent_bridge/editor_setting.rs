//! Root-scoped session settings through the original human control services.
//! Receipts describe one setting change; they never become a project edit,
//! source witness, Undo owner, or a replayable editor-state snapshot.
use super::{editor_proof, parse, project_audit, project_clear, project_plane, sequences};
use crate::{
    EditorState, EngineError, Result, Session,
    commands::bad,
    project_workspace::{self, ProjectScope},
};
use filmcraft_project::{AutomationMode, EffectKind, Project, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
pub(super) const COMMAND: &str = "agent.configureEditor";
pub(super) const CATALOG: &str = "agent.readEditorTransitionCatalog";
pub(super) const ACTION: &str = "editor_setting";
const LIMIT: usize = 4 * 1024 * 1024;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Setting {
    ClipMixerMode { track: u64, mode: AutomationMode },
    DefaultTransition { definition: String },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: ProjectScope,
    before_sha256: String,
    before_editor_sha256: String,
    setting: Setting,
}
fn request(value: &Value) -> Result<Request> {
    let r: Request = parse(COMMAND, value)?;
    if !super::token_ok(&r.token) || !project_workspace::valid_scope(&r.scope) || !valid_hash(&r.before_sha256) || !valid_hash(&r.before_editor_sha256) {
        return Err(error("exact Root identity and complete project/editor digests required"));
    }
    Ok(r)
}
fn basis(s: &Session, token: &str, scope: &ProjectScope, hash: &str) -> Result<()> {
    project_plane::require_scope(s, scope)?;
    super::check_ticket_editor_basis(s, token, false)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("expired Root read"))?;
    if let Some(owner) = t.editor_control.as_ref().filter(|o| o.change) {
        if owner.following.is_some() {
            return Err(error("following tickets cannot borrow strict settings or catalog authority"));
        }
        owner.validate(s)?;
    }
    if t.project_scope.as_ref() != Some(scope) || t.source_library.is_some() || !valid_hash(hash) || project_clear::editor_hash(&s.state)? != hash {
        return Err(error("a current exact Root metadata read and private clipboard basis are required"));
    }
    Ok(())
}
pub(super) fn prepare(project: &Project, state: &EditorState, scope: &ProjectScope, setting: &Setting) -> Result<crate::mixer::editor::Change> {
    if project_workspace::identity(project)?.is_none_or(|r| r.scope != *scope) {
        return Err(error("editor settings belong to another Root"));
    }
    match setting {
        Setting::ClipMixerMode { track, mode } => {
            let id = state.active_sequence.ok_or(EngineError::NoSequence)?;
            let owner = crate::editorial_workspace::association(project)?.ok_or_else(|| error("no owning sequence association"))?;
            if owner.scope.project_id != scope.project_id || !sequences::belongs(project, id, &owner.scope)? {
                return Err(error("mode target is outside the Root's actual owning sequence"));
            }
            let tracks = &project.sequence(id).ok_or(EngineError::NoSequence)?.audio_tracks;
            let mut ids = BTreeSet::new();
            if tracks.len() > 4096
                || !(1..=9_007_199_254_740_991).contains(track)
                || tracks.iter().any(|t| t.id.0 == 0 || t.id.0 > 9_007_199_254_740_991 || !ids.insert(t.id.0))
            {
                return Err(error("ambiguous or invalid bounded audio track inventory"));
            }
            crate::mixer::editor::prepare_mode_for(project, state, TrackId(*track), *mode)
        }
        Setting::DefaultTransition { definition } => {
            if definition.is_empty()
                || definition.len() > 256
                || filmcraft_project::find_effect(definition).is_none_or(|d| !matches!(d.kind, EffectKind::AudioTransition | EffectKind::VideoTransition))
            {
                return Err(error("an exact canonical original transition definition is required"));
            }
            crate::mixer::editor::prepare_default_transition(definition)
        }
    }
}
pub(super) fn human(setting: &Setting) -> (&'static str, Value) {
    match setting {
        Setting::ClipMixerMode { track, mode } => ("clipMixer.setMode", json!({"track":track,"mode":mode.label()})),
        Setting::DefaultTransition { definition } => ("effects.setDefaultTransition", json!({"effect":definition})),
    }
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
    let before_hash = project_clear::editor_hash_with(before, clipboard)?;
    let after_hash = project_clear::editor_hash_with(after, clipboard)?;
    let result = json!({"status":"configured","kind":"native_editor_setting","version":1,"action":ACTION,"token":r.token,
        "beforeRevision":revision,"revision":revision,"nativeSession":epoch,"scope":r.scope,"projectPath":path,
        "sequence":before.active_sequence.map(|id|id.0),"sha256":project_plane::project_basis(project)?,
        "beforeEditorSha256":before_hash,"afterEditorSha256":after_hash,"humanCommand":human(&r.setting).0,"setting":r.setting,
        "changed":before_hash!=after_hash,"projectUnchanged":true,"historyUnchanged":true,"requiresFreshEditorRead":true,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,
        "editorProof":editor_proof::encode(before,after,clipboard,"native_editor_setting_proof",LIMIT,COMMAND)?});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("editor-setting receipt exceeds its bounded wire size"));
    }
    Ok(result)
}
pub(super) fn is_result(value: &Value) -> bool {
    value.get("action").and_then(Value::as_str) == Some(ACTION)
}
pub(super) fn configure(s: &mut Session, value: &Value) -> Result<Value> {
    if cfg!(target_arch = "wasm32") || !super::operation_cancellation_available(s) {
        return Err(error("editor settings require the native independent Stop transport"));
    }
    let r = request(value)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("read the current owning Root first"))?;
    if let Some((old, result)) = &t.committed {
        return if old == value && is_result(result) { Ok(result.clone()) } else { Err(error("read token already used by another operation")) };
    }
    basis(s, &r.token, &r.scope, &r.before_editor_sha256)?;
    if project_plane::basis(s)? != r.before_sha256 {
        return Err(error("the complete original project changed; read again"));
    }
    // The human default command is journaled and can settle a pending dynamic
    // trim at top level. Refuse that compound gesture explicitly; never hide a
    // timeline commit inside a state-only receipt. Mode remains non-journaled.
    if matches!(&r.setting, Setting::DefaultTransition { .. }) && s.trim_play.active() {
        return Err(error("finish or cancel active trim playback before changing the transition default"));
    }
    let change = prepare(&s.project, &s.state, &r.scope, &r.setting)?;
    let before = s.state.clone();
    let expected = change.preview_for(&s.project, &before)?;
    let clipboard = project_clear::clipboard_hash(&before)?;
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = String::new();
    let path = s.path.clone().ok_or_else(|| error("a saved owning Root path is required"))?;
    let result = receipt(&s.project, &path, &r, &before, &expected, &clipboard, &epoch, s.revision)?;
    #[cfg(not(target_arch = "wasm32"))]
    s.agent_bridge.operations.enter_publication(&r.token)?;
    let (command, params) = human(&r.setting);
    s.execute(command, params)?;
    if let Some(t) = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token) {
        t.committed = Some((value.clone(), result.clone()));
        t.after = Some(Arc::downgrade(&s.project));
        t.after_sequence = s.state.active_sequence;
    }
    Ok(result)
}
// A validated retained receipt is descriptive. It never replays a setting or
// replaces the reopened editor, live owners, media witnesses or Undo head.
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
        || before_sequence != after_sequence
        || project_plane::project_basis(project)? != r.before_sha256
        || project_audit::value_bytes(result)?.len() > LIMIT
    {
        return Err(error("foreign historical editor-setting identity or project basis"));
    }
    let decoded = editor_proof::decode(
        result.get("editorProof").ok_or_else(|| error("missing native editor-setting proof"))?,
        "native_editor_setting_proof",
        LIMIT,
        COMMAND,
    )?;
    let before = decoded.before;
    if before.active_sequence.map(|id| id.0) != before_sequence
        || project_clear::editor_hash_with(&before, &decoded.clipboard_sha256)? != r.before_editor_sha256
    {
        return Err(error("historical original editor/clipboard basis differs"));
    }
    let after = prepare(project, &before, scope, &r.setting)?.preview_for(project, &before)?;
    editor_proof::require_after(&decoded.after_editor, &after, COMMAND)?;
    let epoch = result
        .get("nativeSession")
        .and_then(Value::as_str)
        .filter(|v| filmcraft_project::ClipIdentity::valid_id(v))
        .ok_or_else(|| error("missing historical native session epoch"))?;
    if receipt(project, path, &r, &before, &after, &decoded.clipboard_sha256, epoch, revision)? != *result {
        return Err(error("historical setting differs from the exact original human service"));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    #[serde(default)]
    version: Option<u8>,
    token: String,
    scope: ProjectScope,
    before_editor_sha256: String,
    offset: usize,
    limit: usize,
}
pub(super) fn catalog(s: &mut Session, value: &Value) -> Result<Value> {
    let r: Catalog = parse(CATALOG, value)?;
    let following = match r.version {
        None => {
            basis(s, &r.token, &r.scope, &r.before_editor_sha256)?;
            None
        }
        Some(2) => Some(super::editor_control::following::basis(s, &r.token, &r.scope, &r.before_editor_sha256)?),
        _ => return Err(error("explicit version-two following catalog metadata required")),
    };
    let defs =
        filmcraft_project::effect_defs().iter().filter(|d| matches!(d.kind, EffectKind::AudioTransition | EffectKind::VideoTransition)).collect::<Vec<_>>();
    let mut ids = BTreeSet::new();
    if defs.len() > 4096
        || defs.iter().any(|d| d.id.is_empty() || d.id.len() > 256 || d.name.len() > 1024 || !ids.insert(d.id))
        || r.offset > defs.len()
        || !(1..=32).contains(&r.limit)
    {
        return Err(error("bounded canonical original transition catalog page required"));
    }
    let end = r.offset.saturating_add(r.limit).min(defs.len());
    let rows = defs
        .get(r.offset..end)
        .ok_or_else(|| error("invalid catalog page"))?
        .iter()
        .map(|d| {
            json!({"definition":d.id,"name":d.name,
        "kind":if d.kind==EffectKind::AudioTransition {"audio_transition"} else {"video_transition"},"defaultEligible":true})
        })
        .collect::<Vec<_>>();
    let result = json!({"status":"read","kind":"native_editor_transition_catalog","version":1,"token":r.token,"scope":r.scope,"revision":s.revision,
        "editorStateSha256":r.before_editor_sha256,"offset":r.offset,"total":defs.len(),"nextOffset":if end<defs.len(){Some(end)}else{None},"definitions":rows,
        "referenceScope":"exact_read_ticket_only","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    let result = if let Some(following) = &following {
        let mut result = result;
        result["version"] = json!(2);
        result["editorStateSha256"] = following["current"]["editorStateSha256"].clone();
        result["followingBasis"] = following.clone();
        result
    } else {
        result
    };
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("transition catalog page exceeds the bounded wire budget"));
    }
    if let Some(following) = following {
        if following != super::editor_control::following::basis(s, &r.token, &r.scope, &r.before_editor_sha256)? {
            return Err(error("following catalog basis changed during its bounded native read"));
        }
    } else {
        basis(s, &r.token, &r.scope, &r.before_editor_sha256)?;
    }
    Ok(result)
}
#[cfg(test)]
mod tests;
