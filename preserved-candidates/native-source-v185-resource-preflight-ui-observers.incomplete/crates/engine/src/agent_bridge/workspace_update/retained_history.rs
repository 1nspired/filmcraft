//! Private one-shot proof of the original checkpoint before candidate activation.
//! Issues no view/source ticket and cannot restore or unlock editing.
use super::*;
pub(super) const INSPECT: &str = "agent.inspectPausedWorkspaceHistory";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    project_path: String,
    activation_token: String,
    native_session: String,
    revision: u64,
    sequence: Option<u64>,
    checkpoint: String,
    sha256: String,
    #[serde(default)]
    snapshot_sha256: Option<String>,
}
fn request(s: &Session, p: &Value) -> Result<Request> {
    let r: Request = parse(INSPECT, p)?;
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(INSPECT, "exact paused Root candidate required"))?;
    if !h.candidate
        || h.candidate_cancelled
        || h.retiring
        || h.scope != LifecycleScope::Project(r.scope.clone())
        || h.workspace != r.workspace
        || h.update != r.update
        || h.token != r.activation_token
        || h.native_session != r.native_session
        || s.agent_bridge.epoch.as_deref() != Some(r.native_session.as_str())
        || h.revision != r.revision
        || s.revision != r.revision
        || s.path.as_deref() != Some(r.project_path.as_str())
        || p.get("sequence").is_none()
        || s.state.active_sequence.map(|id| id.0) != r.sequence
        || !token_ok(&r.token)
        || r.token == h.token
        || !uuid(&r.checkpoint)
        || r.sha256.len() != 64
        || !r.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || !operation_cancellation_available(s)
        || history_checkpoint::has_basis(s, &r.token)
        || s.agent_bridge.tickets.iter().any(|t| t.token == r.token)
    {
        return Err(bad(INSPECT, "exact paused candidate epoch view update and fresh private operation token required"));
    }
    if p.get("snapshotSha256").is_some() && r.snapshot_sha256.is_none()
        || r.snapshot_sha256.as_ref().is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(bad(INSPECT, "exact complete snapshot checksum required"));
    }
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    require_idle_writer_jobs(s)?;
    bundle::check_saved_project(s)?;
    Ok(r)
}
// Stop and the native deadline cover the original full checkpoint/graph check.
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<()> {
    let r = request(s, p)?;
    s.agent_bridge.operations.register(&r.token)
}
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(s, p)?;
    let snapshot = r
        .snapshot_sha256
        .as_ref()
        .map(|hash| history_checkpoint::validate_paused_project_snapshot(s, &r.scope, &r.update, &r.checkpoint, (&r.sha256, hash)))
        .transpose()?;
    let result = history_checkpoint::validate_paused_candidate_history(s, &r.token, &r.scope, &r.checkpoint, &r.sha256)?;
    let mut receipt = json!({"status":"retained","kind":"native_paused_workspace_history","version":1,
        "token":r.token,"scope":r.scope,"workspace":r.workspace,"update":r.update,"projectPath":r.project_path,
        "activationToken":r.activation_token,"nativeSession":r.native_session,"nativePid":std::process::id(),
        "revision":result["revision"],"sequence":result["sequence"],"checkpoint":result["checkpoint"],
        "editingPaused":true,"projectUnchanged":true,"historyUnchanged":true,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if let Some(value) = snapshot {
        receipt["snapshot"] = value;
    }
    Ok(receipt)
}
