//! Publication-time following through the ORIGINAL compound setting publisher.
//! Receipt3 separates original and actual publication bases. The original
//! native owner is required for live publication; historical advertisements
//! are encoded independently and never restore a live owner or writer.
use super::{COMMAND, EDITOR_KIND, LIMIT, Request, Retained, error, publish_prepared};
use crate::{
    EditorState, Result, Session,
    agent_bridge::{editor_control, editor_setting, parse, project_audit, project_plane},
    project_workspace,
};
use serde_json::{Value, json};
pub(super) struct PreparedBasis {
    pub(super) basis: Value,
    pub(super) editor: Value,
    pub(super) history: Retained,
    pub(super) revision: u64,
    pub(super) saved_revision: u64,
    pub(super) advertised: bool,
}
const DEFAULT: &str = "effects.setDefaultTransition";
const ORIGIN_EDITOR_KIND: &str = "native_following_editor_basis";
pub(in crate::agent_bridge) const ACTION: &str = "editor_control_following_setting";
pub(in crate::agent_bridge) fn is_result(value: &Value) -> bool {
    value.get("action").and_then(Value::as_str) == Some(ACTION)
}
fn request(value: &Value) -> Result<Request> {
    let r: Request = parse(COMMAND, value)?;
    if r.version != 3
        || !crate::agent_bridge::token_ok(&r.token)
        || !project_workspace::valid_scope(&r.scope)
        || !editor_setting::valid_hash(&r.before_sha256)
        || !editor_setting::valid_hash(&r.before_editor_sha256)
    {
        return Err(error("explicit version-three following setting with ORIGINAL Root/project/editor basis required"));
    }
    Ok(r)
}
pub(super) fn configure(s: &mut Session, value: &Value) -> Result<Value> {
    let r = request(value)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("fresh following-purpose control preparation required"))?;
    if let Some((old, result)) = &t.committed {
        return if old == value && is_result(result) { Ok(result.clone()) } else { Err(error("following token already consumed by another operation")) };
    }
    let basis = editor_control::following::basis(s, &r.token, &r.scope, &r.before_editor_sha256)?;
    if basis["original"]["projectState"]["sha256"].as_str() != Some(r.before_sha256.as_str()) {
        return Err(error("the ORIGINAL following project digest is required; no substitution of a later read"));
    }
    let owner =
        s.agent_bridge.tickets.iter().find(|t| t.token == r.token).and_then(|t| t.editor_control.as_ref()).ok_or_else(|| error("following owner expired"))?;
    let prepared = owner.following.as_ref().ok_or_else(|| error("strict or historical reads cannot acquire following authority"))?.prepare_current(s)?;
    let original = owner.settlement.as_ref().ok_or_else(|| error("original following forecast is missing"))?;
    let history = Retained::capture_forecast(original)?;
    let editor = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("original following context expired"))?.context.clone();
    project_plane::require_scope(s, &r.scope)?;
    let publication = Request {
        version: 2,
        token: r.token,
        scope: r.scope,
        before_sha256: basis["current"]["projectState"]["sha256"].as_str().ok_or_else(|| error("missing exact publication project digest"))?.into(),
        before_editor_sha256: basis["current"]["editorStateSha256"].as_str().ok_or_else(|| error("missing exact publication editor digest"))?.into(),
        setting: r.setting,
    };
    let revision = basis["original"]["revision"].as_u64().ok_or_else(|| error("original native revision missing"))?;
    let saved_revision = s.saved_revision;
    let advertised = basis["original"]["capabilities"]["compoundEditorWrite"].as_bool().ok_or_else(|| error("original following advertisement missing"))?;
    publish_prepared(s, value, &publication, prepared, Some(PreparedBasis { basis, editor, history, revision, saved_revision, advertised }))
}
pub(super) fn receipt(value: &Value, basis: &PreparedBasis, publication: Value, check: Option<&crate::agent_bridge::media_guard::Check>) -> Result<Value> {
    let r = request(value)?;
    let original = &basis.basis["original"];
    let current = &basis.basis["current"];
    if original["version"] != 3
        || current["version"] != 3
        || original["capabilities"]["compoundEditorWrite"] != basis.advertised
        || current["capabilities"]["compoundEditorWrite"] != basis.advertised
        || original["token"] != r.token
        || current["token"] != r.token
        || original["scope"] != serde_json::to_value(&r.scope).map_err(|e| error(e.to_string()))?
        || current["scope"] != original["scope"]
        || original["nativeSession"] != current["nativeSession"]
        || original["project"]["path"] != current["project"]["path"]
        || original["sequence"] != current["sequence"]
        || original["projectState"]["sha256"] != r.before_sha256
        || original["editorStateSha256"] != r.before_editor_sha256
        || publication["version"] != 2
        || publication["token"] != r.token
        || publication["scope"] != current["scope"]
        || publication["nativeSession"] != current["nativeSession"]
        || publication["projectPath"] != current["project"]["path"]
        || publication["sequence"] != current["sequence"]
        || publication["beforeRevision"] != current["revision"]
        || publication["beforeProjectSha256"] != current["projectState"]["sha256"]
        || publication["beforeEditorSha256"] != current["editorStateSha256"]
        || publication["beforeHistory"] != current["historyState"]
        || publication["setting"] != value["setting"]
    {
        return Err(error("following receipt lost an exact ORIGINAL or publication basis; nothing published"));
    }
    let decoded = crate::agent_bridge::editor_proof::decode(
        publication.get("editorProof").ok_or_else(|| error("publication editor proof missing"))?,
        EDITOR_KIND,
        LIMIT,
        COMMAND,
    )?;
    let original_editor: EditorState = serde_json::from_value(basis.editor.clone()).map_err(|e| error(e.to_string()))?;
    if crate::agent_bridge::project_clear::editor_hash_with(&original_editor, &decoded.clipboard_sha256)? != r.before_editor_sha256 {
        return Err(error("original following editor/clipboard proof changed"));
    }
    let editor = crate::agent_bridge::editor_proof::encode(&original_editor, &decoded.before, &decoded.clipboard_sha256, ORIGIN_EDITOR_KIND, LIMIT, COMMAND)?;
    let history = basis.history.proof_command(DEFAULT, check)?;
    let origin_basis = history::encode_basis(basis.revision, basis.saved_revision, basis.advertised)?;
    let result = json!({"status":"configured","kind":"native_editor_control_following_setting","version":3,"action":ACTION,"token":r.token,
        "scope":r.scope,"projectPath":publication["projectPath"],"nativeSession":publication["nativeSession"],"sequence":publication["sequence"],
        "originalRead":original,"publicationRead":current,"publication":publication,"originalEditorProof":editor,"originalHistoryProof":history,"originalBasis":origin_basis,
        "agentTimelineCommits":0,"agentUndoOwned":false,"requiresFreshEditorRead":true,"requiresFreshProjectRead":true,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("complete original/publication following receipt exceeds4MiB; nothing published"));
    }
    Ok(result)
}

pub(in crate::agent_bridge) mod history;
