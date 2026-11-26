//! Replace a proved-unpublished activation token on the same paused Root owner.
//! The old operation remains cancelled; no activation or persistence is replayed.
use super::*;
pub(super) const REFRESH: &str = "agent.refreshWorkspaceActivation";
#[derive(Clone)]
pub(super) struct Record {
    request: Value,
    pub(super) receipt: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    native_session: String,
    previous_token: String,
    activation_token: String,
}
pub(super) fn reconciling(s: &Session, p: &Value) -> bool {
    s.agent_bridge.workspace_update.as_ref().and_then(|h| h.activation_refresh.as_ref()).is_some_and(|r| r.request == *p)
}
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse(REFRESH, p)?;
    if let Some(record) = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.activation_refresh.as_ref()).filter(|v| v.request == *p) {
        return Ok(record.receipt.clone());
    }
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(REFRESH, "exact paused Root candidate required"))?;
    let scope = LifecycleScope::Project(r.scope.clone());
    if !h.candidate
        || h.candidate_cancelled
        || h.retiring
        || h.scope != scope
        || h.workspace != r.workspace
        || h.update != r.update
        || h.native_session != r.native_session
        || s.agent_bridge.epoch.as_deref() != Some(r.native_session.as_str())
        || h.token != r.previous_token
        || h.activation_refresh_count >= 16
        || !operation_cancellation_available(s)
        || ![&r.token, &r.activation_token, &r.previous_token].iter().all(|v| token_ok(v))
        || r.token == r.activation_token
        || r.token == r.previous_token
        || r.activation_token == r.previous_token
    {
        return Err(bad(REFRESH, "current epoch, distinct private bases and uncancelled paused Root candidate required"));
    }
    if !s.agent_bridge.operations.cancelled_before_publication(&r.previous_token) {
        return Err(bad(REFRESH, "native cancellation must prove the old activation never began publication"));
    }
    let _ = history_checkpoint::retained_scoped_basis(s, &r.token, &scope)?;
    let next = history_checkpoint::retained_scoped_basis(s, &r.activation_token, &scope)?;
    let _ = s.agent_bridge.operations.preparation_stop(&r.activation_token)?;
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    require_idle_writer_jobs(s)?;
    bundle::check_saved_project(s)?;
    let receipt = json!({"status":"prepared","kind":"native_workspace_activation_refresh","version":1,"token":r.token,"previousToken":r.previous_token,"activationToken":r.activation_token,"scope":r.scope,"workspace":r.workspace,"update":r.update,"nativeSession":r.native_session,"nativePid":std::process::id(),"revision":s.revision,"projectPath":s.path,"editingPaused":true,"publication":"not-started","sourceValidation":"not-performed","writeAuthority":false,"execution":"activation-token-refreshed"});
    s.agent_bridge.operations.enter_publication(&r.token)?;
    let revision = s.revision;
    let dirty = s.is_dirty();
    let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(REFRESH, "candidate disappeared before token refresh"))?;
    h.promoted_identity(next, revision, dirty, r.activation_token);
    h.activation_refresh_count = h.activation_refresh_count.saturating_add(1);
    h.activation_refresh = Some(Record { request: p.clone(), receipt: receipt.clone() });
    Ok(receipt)
}
