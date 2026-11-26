//! Private workspace lifecycle fence. This is not a model tool, source witness,
//! edit ticket or a permission to recover a native writer from old metadata.
use super::organization_scope::Scope as LifecycleScope;
use super::*;
mod activation_refresh;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod current_image;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod library_commit;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(in crate::agent_bridge) use library_commit::cold::ColdCommit;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) use library_commit::cold::{PreferenceContinuation, ServiceBindings};
mod retained_history;
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
mod current_image {
    use super::*;
    pub(super) const COMMAND: &str = "agent.preparePausedProjectRecovery";
    pub(super) struct Image;
    pub(super) const RECORD_COMMAND: &str = "agent.pausedProjectRecoveryRecord";
    pub(super) fn prepare_record(s: &mut Session, p: &Value) -> Result<()> {
        prepare(s, p)
    }
    pub(super) fn run_record(s: &mut Session, p: &Value) -> Result<Value> {
        run(s, p)
    }
    pub(super) fn prepare(_: &mut Session, _: &Value) -> Result<()> {
        Err(bad(COMMAND, "current native file-frame capture unsupported on this platform"))
    }
    pub(super) fn run(_: &mut Session, _: &Value) -> Result<Value> {
        Err(bad(COMMAND, "current native file-frame capture unsupported on this platform"))
    }
}
pub(super) const PREPARE_PAUSED_IMAGE: &str = current_image::COMMAND;
pub(super) const PAUSED_RECOVERY_RECORD: &str = current_image::RECORD_COMMAND;
pub(super) fn prepare_paused_record_operation(s: &mut Session, p: &Value) -> Result<()> {
    current_image::prepare_record(s, p)
}
pub(super) fn prepare_paused_image_operation(s: &mut Session, p: &Value) -> Result<()> {
    current_image::prepare(s, p)
}
pub(super) const INSPECT_PAUSED_HISTORY: &str = retained_history::INSPECT;
pub(super) fn prepare_paused_history_inspection(s: &mut Session, p: &Value) -> Result<()> {
    retained_history::prepare(s, p)
}
mod bundle;

pub(super) const BEGIN: &str = "agent.beginWorkspaceUpdate";
const STATUS: &str = "agent.workspaceUpdateStatus";
const CANCEL: &str = "agent.cancelWorkspaceUpdate";
const ACTIVATE: &str = "agent.activateWorkspaceUpdate";
const RETIRE: &str = "agent.retireWorkspaceUpdate";

#[derive(Clone)]
struct Staged {
    request: Value,
    receipt: Option<Value>,
}

#[derive(Clone)]
pub(super) struct Held {
    request: Value,
    scope: LifecycleScope,
    workspace: String,
    update: String,
    token: String,
    revision: u64,
    native_session: String,
    dirty: bool,
    basis: history_checkpoint::Basis,
    stage: Option<Staged>,
    candidate: bool,
    current_image: Option<std::sync::Arc<current_image::Image>>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    restored_current_image: Option<std::sync::Arc<current_image::restore::Input>>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    current_image_retirement: Option<current_image::publication::Retirement>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    current_activation_owner: Option<std::sync::Arc<std::sync::Mutex<current_image::activation::Owner>>>,
    candidate_cancelled: bool,
    retiring: bool,
    saved_project_hash: Option<String>,
    persistence_paused: bool,
    persistence_start: Option<crate::autosave::AutosaveConfig>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    persistence_directory: Option<std::sync::Arc<filmcraft_format::retained::preview_context::DirectoryFrame>>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    library_commit: Option<std::sync::Arc<std::sync::Mutex<library_commit::Commit>>>,
    recovery_basis: Option<Value>,
    activation_refresh: Option<activation_refresh::Record>,
    activation_refresh_count: usize,
    journal_recovery: Option<Value>,
    project_creation: Option<Value>,
}
pub(super) struct Closed {
    scope: LifecycleScope,
    workspace: String,
    update: String,
    token: String,
    activated: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Begin {
    token: String,
    scope: LifecycleScope,
    workspace: String,
    update: String,
    #[serde(default, rename = "nativeSession")]
    native_session: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Address {
    scope: LifecycleScope,
    workspace: String,
    update: String,
}
fn uuid(v: &str) -> bool {
    v.len() == 36 && v.bytes().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == b'-' } else { c.is_ascii_hexdigit() })
}
fn address(scope: &sequences::Scope, workspace: &str, update: &str) -> Result<()> {
    if !uuid(workspace) || !uuid(update) || scope.project_id.is_empty() || scope.project_id.len() > 256 || scope.anchor_sequence == 0 {
        return Err(bad(BEGIN, "explicit workspace, update UUID and owning film/anchor required"));
    }
    Ok(())
}
fn scoped_address(scope: &LifecycleScope, workspace: &str, update: &str) -> Result<()> {
    match scope {
        LifecycleScope::Sequence(s) => address(s, workspace, update),
        LifecycleScope::Project(s) if crate::project_workspace::valid_scope(s) && s.workspace_id == workspace && uuid(update) => Ok(()),
        _ => Err(bad(BEGIN, "exact independent project/workspace identity and update UUID required")),
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn require_save_startup_candidate(s: &Session) -> Result<()> {
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(ACTIVATE, "original paused candidate required for Save startup"))?;
    if !h.candidate || h.candidate_cancelled || h.retiring || h.persistence_start.is_none() || !matches!(h.scope, LifecycleScope::Project(_)) || s.view_only {
        return Err(bad(ACTIVATE, "Save startup needs the original independent paused candidate and configured persistence"));
    }
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    bundle::check_saved_project(s)
}
pub(super) fn held(s: &Session) -> bool {
    s.agent_bridge.workspace_update.is_some()
}
pub(super) fn command_allowed(s: &Session, command: &str) -> Result<()> {
    if s.agent_bridge.workspace_update.as_ref().is_some_and(|h| h.retiring) && command != STATUS {
        return Err(bad(command, "this native owner is retiring; editing and persistence remain paused"));
    }
    let project_read = s.agent_bridge.workspace_update.as_ref().is_some_and(|h| matches!(h.scope, LifecycleScope::Project(_)))
        && matches!(command, "agent.readProject" | "agent.validateProjectRead" | "agent.inspectApplicationHistoryCheckpoint" | "agent.inspectHistoryCheckpoint" | "agent.readHistoryCheckpointOutcome" | "agent.inspectCheckpointProvenance");
    let candidate_history = s.agent_bridge.workspace_update.as_ref().is_some_and(|h| h.candidate && matches!(h.scope, LifecycleScope::Project(_)))
        && command == "agent.validateRetainedHistory";
    if held(s)
        && !candidate_history
        && !project_read
        && !matches!(
            command,
            BEGIN
                | STATUS
                | CANCEL
                | ACTIVATE
                | RETIRE
                | bundle::STAGE
                | "agent.readApplicationProjectAdmission"
                | "agent.stageProjectRecovery"
                | "agent.prepareProjectRecovery"
                | activation_refresh::REFRESH
                | INSPECT_PAUSED_HISTORY
                | PREPARE_PAUSED_IMAGE
                | PAUSED_RECOVERY_RECORD
                | "agent.readHistoryCheckpointBasis"
                | "agent.saveHistoryCheckpoint"
                | "agent.readSourceCatalog"
                | "agent.readProjectRecoveryDependencies"
                | "agent.readArchivedRecoveryDependencies"
                | "agent.readIdentityPublication"
                | "agent.inspectIdentityMigration"
                | "agent.publishIdentityMigration"
                | "prefs.get"
        )
    {
        return Err(bad(command, "workspace update has paused editing; cancel the update to resume; no input was queued"));
    }
    Ok(())
}
fn result(s: &Session, h: &Held) -> Value {
    let mut value = json!({"protocol":1,"kind":"native_workspace_update_fence","version":1,"token":h.token,"project":{"path":s.path},
        "capabilities":{"workspaceUpdateBundle":true,"workspaceUpdateRetirement":true,"historyCheckpointRetention":true,"workspaceUpdateActivationEpoch":true,"operationCancellation":operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS},
        "nativePid":std::process::id(),
        "status":if h.retiring{"retiring"}else if h.candidate_cancelled{"candidate_cancelled"}else if h.candidate{"candidate_ready"}else{"paused"},
        "role":if h.candidate{"candidate"}else{"owner"},"workspace":h.workspace,"update":h.update,
        "scope":h.scope,"nativeSession":h.native_session,"revision":h.revision,"dirty":h.dirty,
        "stage":h.stage.as_ref().map(|stage|stage.receipt.clone().unwrap_or_else(||json!({"status":"unknown","replayed":false}))),
        "projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","writeAuthority":false});
    if matches!(h.scope, LifecycleScope::Project(_)) {
        value["kind"] = json!("native_project_recovery_fence");
        value["version"] = json!(2);
        value["capabilities"]["projectRecoverySnapshot"] = json!(true);
        value["capabilities"]["projectRecoveryPreparation"] = json!(true);
        value["capabilities"]["projectRecoveryDependencies"] = json!(true);
        value["capabilities"]["projectRecoveryArchiveDependencies"] = json!(true);
        value["capabilities"]["workspaceActivationRefresh"] = json!(true);
        value["capabilities"]["pausedWorkspaceHistoryInspection"] = json!(true);
        value["capabilities"]["pausedProjectSnapshotInspection"] = json!(true);
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            value["capabilities"]["pausedCurrentRecoveryImage"] = json!(true);
            value["capabilities"]["pausedCurrentRecoveryDependencies"] = json!(true);
            value["capabilities"]["pausedCurrentRecoveryResources"] = json!(true);
            value["capabilities"]["pausedCurrentRecoveryPublication"] = json!(true);
            value["capabilities"]["pausedCurrentRecoveryRetirement"] = json!(true);
            if let Some(retirement) = &h.current_image_retirement {
                value["currentImageRetirement"] = retirement.value();
            }
        }
        value["activationRefresh"] = h.activation_refresh.as_ref().map_or(Value::Null, |r| r.receipt.clone());
        value["capabilities"]["workspaceUpdateBundle"] = json!(false);
        if let Some(receipt) = &h.project_creation {
            value["capabilities"]["projectCreation"] = json!(true);
            value["projectCreation"] = receipt.clone();
        }
        if let Some(receipt) = &h.journal_recovery {
            value["capabilities"]["projectRecoveryJournal"] = json!(true);
            value["journalRecovery"] = receipt.clone();
        }
    }
    value
}
pub(super) fn view(s: &Session) -> Option<Value> {
    s.agent_bridge.workspace_update.as_ref().map(|h| result(s, h))
}
pub(super) fn cancel_request(s: &Session) -> Option<Value> {
    s.agent_bridge
        .workspace_update
        .as_ref()
        .map(|h| json!({"token":h.token,"scope":h.scope,"workspace":h.workspace,"update":h.update,"nativeSession":h.native_session}))
}
pub(super) fn reconciling(s: &Session, p: &Value) -> bool {
    s.agent_bridge.workspace_update.as_ref().is_some_and(|h| h.request == *p)
        || s.agent_bridge.closed_workspace_updates.iter().any(|h| {
            p["token"] == h.token
                && p["workspace"] == h.workspace
                && p["update"] == h.update
                && serde_json::to_value(&h.scope).is_ok_and(|scope| p["scope"] == scope)
        })
}
fn begin(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Begin = parse(BEGIN, p)?;
    if r.native_session.as_deref().is_some_and(|v| Some(v) != s.agent_bridge.epoch.as_deref()) {
        return Err(bad(BEGIN, "pause belongs to another native process epoch"));
    }
    scoped_address(&r.scope, &r.workspace, &r.update)?;
    if let Some(h) = &s.agent_bridge.workspace_update {
        return if h.request == *p { Ok(result(s, h)) } else { Err(bad(BEGIN, "another workspace update owns the native pause")) };
    }
    if let Some(closed) = s.agent_bridge.closed_workspace_updates.iter().find(|h| h.workspace == r.workspace && h.update == r.update) {
        return if closed.scope == r.scope && closed.token == r.token {
            Ok(
                json!({"status":if closed.activated{"activated"}else{"cancelled"},"workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,"replayed":false}),
            )
        } else {
            Err(bad(BEGIN, "update identity already settled under a different native request"))
        };
    }
    if !token_ok(&r.token) || !operation_cancellation_available(s) {
        return Err(bad(BEGIN, "current private persistence basis and independent Stop required"));
    }
    require_idle_writer_jobs(s)?;
    let basis = history_checkpoint::retained_scoped_basis(s, &r.token, &r.scope)?;
    let native_session = history_checkpoint::epoch(s)?;
    s.sync_persistence();
    let persistence_paused = if let Some(p) = s.persistence.as_ref() {
        let check = s.agent_bridge.operations.check().ok_or_else(|| bad(BEGIN, "active native update fence missing"))?;
        p.pause_workspace(|| check.check().map_err(|e| e.to_string())).map_err(|e| bad(BEGIN, e))?;
        s.apply_persistence_events();
        true
    } else {
        false
    };
    let basis = match history_checkpoint::retain_application_pause_basis(s, basis) {
        Ok(basis) => basis,
        Err(error) => {
            if persistence_paused {
                let resume = s
                    .persistence
                    .as_ref()
                    .ok_or_else(|| "original persistence disappeared after its pause".to_string())
                    .and_then(|worker| worker.resume_workspace());
                if let Err(resume) = resume {
                    return Err(bad(BEGIN, format!("{error}; original persistence resume failed: {resume}")));
                }
            }
            return Err(error);
        }
    };
    let record = Held {
        request: p.clone(),
        scope: r.scope,
        workspace: r.workspace,
        update: r.update,
        token: r.token.clone(),
        revision: s.revision,
        native_session,
        dirty: s.is_dirty(),
        basis,
        stage: None,
        candidate: false,
        current_image: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        restored_current_image: None,
        candidate_cancelled: false,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        current_image_retirement: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        current_activation_owner: None,
        retiring: false,
        saved_project_hash: None,
        persistence_paused,
        persistence_start: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        persistence_directory: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        library_commit: None,
        recovery_basis: None,
        activation_refresh: None,
        activation_refresh_count: 0,
        journal_recovery: None,
        project_creation: None,
    };
    if let Err(error) = s.agent_bridge.operations.enter_publication(&r.token) {
        if persistence_paused && let Some(p) = s.persistence.as_ref() {
            let _ = p.resume_workspace();
        }
        return Err(error);
    }
    let reply = result(s, &record);
    s.agent_bridge.workspace_update = Some(record);
    Ok(reply)
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Address = parse(STATUS, p)?;
    scoped_address(&r.scope, &r.workspace, &r.update)?;
    if let Some(h) = &s.agent_bridge.workspace_update {
        if h.workspace != r.workspace || h.update != r.update || h.scope != r.scope {
            return Err(bad(STATUS, "workspace update belongs to another native scope"));
        }
        history_checkpoint::validate_retained_basis(s, &h.basis)?;
        return Ok(result(s, h));
    }
    let closed = s.agent_bridge.closed_workspace_updates.iter().find(|h| h.workspace == r.workspace && h.update == r.update && h.scope == r.scope);
    Ok(
        json!({"status":closed.map_or("unknown",|h|if h.activated{"activated"}else{"cancelled"}),"workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,"replayed":false,
            "nativePid":std::process::id(),"nativeSession":history_checkpoint::epoch(s)?}),
    )
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Begin = parse(CANCEL, p)?;
    if r.native_session.is_none() || r.native_session != s.agent_bridge.epoch {
        return Err(bad(CANCEL, "only the exact native process epoch can cancel this update"));
    }
    scoped_address(&r.scope, &r.workspace, &r.update)?;
    if !token_ok(&r.token) {
        return Err(bad(CANCEL, "original workspace update token required"));
    }
    if let Some(h) = &s.agent_bridge.workspace_update {
        if h.workspace != r.workspace || h.update != r.update || h.scope != r.scope || h.token != r.token {
            return Err(bad(CANCEL, "only the owning workspace update can release this pause"));
        }
        history_checkpoint::validate_retained_basis(s, &h.basis)?;
        if h.candidate {
            if let Some(h) = s.agent_bridge.workspace_update.as_mut() {
                h.candidate_cancelled = true;
            }
            return Ok(json!({"status":"candidate_cancelled","workspace":r.workspace,"update":r.update,"scope":r.scope,
                "editingPaused":true,"closeRequested":true,"projectUnchanged":true,"historyUnchanged":true,"replayed":false}));
        }
        // The retained weak graph is separate from the ordinary sixteen-entry
        // metadata LRU; saving recovery cannot strand the user's Cancel action.
        if h.persistence_paused
            && let Some(p) = s.persistence.as_ref()
            && let Err(error) = p.resume_workspace()
        {
            s.error_toast("workspace update", error);
        }
        s.agent_bridge.workspace_update = None;
    } else if s
        .agent_bridge
        .closed_workspace_updates
        .iter()
        .any(|h| h.workspace == r.workspace && h.update == r.update && h.scope == r.scope && h.token == r.token)
    {
        let activated = s
            .agent_bridge
            .closed_workspace_updates
            .iter()
            .any(|h| h.workspace == r.workspace && h.update == r.update && h.scope == r.scope && h.token == r.token && h.activated);
        return Ok(
            json!({"status":if activated{"activated"}else{"cancelled"},"workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,"replayed":false}),
        );
    } else {
        let cancelled = s.agent_bridge.operations.cancel(&json!({"token":r.token}))?;
        if cancelled["cancellationAccepted"] != true {
            return Ok(json!({"status":"unknown","workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,"replayed":false}));
        }
    }
    s.agent_bridge.closed_workspace_updates.push_back(Closed {
        scope: r.scope.clone(),
        workspace: r.workspace.clone(),
        update: r.update.clone(),
        token: r.token,
        activated: false,
    });
    while s.agent_bridge.closed_workspace_updates.len() > TICKET_LIMIT {
        s.agent_bridge.closed_workspace_updates.pop_front();
    }
    Ok(
        json!({"status":"cancelled","workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,"projectUnchanged":true,"historyUnchanged":true,"replayed":false}),
    )
}
fn activate(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Begin = parse(ACTIVATE, p)?;
    if r.native_session.is_none() || r.native_session != s.agent_bridge.epoch {
        return Err(bad(ACTIVATE, "activation belongs to another native process epoch"));
    }
    scoped_address(&r.scope, &r.workspace, &r.update)?;
    if !token_ok(&r.token) {
        return Err(bad(ACTIVATE, "exact staged activation token required"));
    }
    if s.agent_bridge
        .closed_workspace_updates
        .iter()
        .any(|h| h.workspace == r.workspace && h.update == r.update && h.scope == r.scope && h.token == r.token && h.activated)
    {
        return Ok(json!({"status":"activated","workspace":r.workspace,"update":r.update,"editingPaused":false,"replayed":false}));
    }
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(ACTIVATE, "no staged candidate owns this update"))?;
    if !operation_cancellation_available(s)
        || !h.candidate
        || h.candidate_cancelled
        || h.workspace != r.workspace
        || h.update != r.update
        || h.scope != r.scope
        || h.token != r.token
    {
        return Err(bad(ACTIVATE, "only the exact uncancelled candidate can be activated"));
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if let Some(origin) = &h.restored_current_image {
        origin.current(&media_guard::Check::new(web_time::Instant::now()))?;
    }
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    bundle::check_saved_project(s)?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    super::save_operation::managed_dispatch::check_startup(s)?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let current_owner = current_image::activation::admit(s, &media_guard::Check::new(web_time::Instant::now()))?;
    s.agent_bridge.operations.enter_publication(&r.token)?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let preferences = if let Some(owner) = &current_owner {
        let check = s.agent_bridge.operations.check().ok_or_else(|| bad(ACTIVATE, "original current activation I/O fence required"))?;
        let frame = owner.lock().unwrap_or_else(|e| e.into_inner()).publish(s, &check)?;
        Some(frame)
    } else {
        None
    };
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if preferences.is_none() && s.native_auxiliary_restored && s.agent_bridge.workspace_update.as_ref().is_some_and(|h| h.persistence_start.is_some()) {
        let check = s.agent_bridge.operations.check().ok_or_else(|| bad(ACTIVATE, "original activation I/O fence required"))?;
        library_commit::activate(s, &check)?;
    }
    let mut held = s.agent_bridge.workspace_update.take().ok_or_else(|| bad(ACTIVATE, "candidate fence disappeared"))?;
    if let Some(cfg) = held.persistence_start.take() {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        let started = if let (Some(preferences), Some(owner)) = (&preferences, &current_owner) {
            if let Some(check) = s.agent_bridge.operations.check() {
                owner.lock().unwrap_or_else(|e| e.into_inner()).start_persistence(s, cfg.clone(), preferences, &check)
            } else {
                Err(std::io::Error::other("original activation persistence fence disappeared"))
            }
        } else {
            s.start_workspace_persistence(cfg.clone())
        };
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        let started = s.start_workspace_persistence(cfg.clone());
        if let Err(error) = started {
            held.persistence_start = Some(cfg);
            s.agent_bridge.workspace_update = Some(held);
            return Err(bad(ACTIVATE, format!("candidate persistence could not start: {error}; candidate remains paused")));
        }
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    super::save_operation::managed_dispatch::commit_startup(s);
    s.agent_bridge.closed_workspace_updates.push_back(Closed {
        scope: r.scope.clone(),
        workspace: r.workspace.clone(),
        update: r.update.clone(),
        token: r.token,
        activated: true,
    });
    while s.agent_bridge.closed_workspace_updates.len() > TICKET_LIMIT {
        s.agent_bridge.closed_workspace_updates.pop_front();
    }
    Ok(json!({"status":"activated","workspace":r.workspace,"update":r.update,"scope":r.scope,"editingPaused":false,
        "projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","writeAuthority":false}))
}
// The native owner closes itself after its UI delivers this receipt. No PID
// signal, autosave resume or dirty Save dialog can run between this fence and
// shutdown. The manager must observe this process exit before activation.
fn retire(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        token: String,
        scope: LifecycleScope,
        workspace: String,
        update: String,
        #[serde(rename = "pauseToken")]
        pause_token: String,
        #[serde(rename = "stageSha256")]
        stage_sha256: String,
    }
    let r: Request = parse(RETIRE, p)?;
    scoped_address(&r.scope, &r.workspace, &r.update)?;
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(RETIRE, "paused staged owner required"))?;
    if h.candidate
        || h.retiring
        || h.token != r.pause_token
        || h.scope != r.scope
        || h.workspace != r.workspace
        || h.update != r.update
        || h.stage.as_ref().and_then(|v| v.receipt.as_ref()).is_none_or(|v| v["sha256"] != r.stage_sha256)
        || r.token == r.pause_token
    {
        return Err(bad(RETIRE, "only the exact staged owner can retire; no process was closed"));
    }
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    history_checkpoint::retained_scoped_basis(s, &r.token, &r.scope)?;
    bundle::check_saved_project(s)?;
    s.agent_bridge.operations.enter_publication(&r.token)?;
    let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(RETIRE, "owner pause disappeared"))?;
    h.retiring = true;
    Ok(result(s, s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(RETIRE, "owner pause disappeared"))?))
}
pub(super) fn matches_candidate_recovery_basis(s: &Session, basis: &Value) -> bool {
    s.agent_bridge.workspace_update.as_ref().is_some_and(|h| {
        h.candidate
            && !h.candidate_cancelled
            && !h.retiring
            && h.project_creation.is_none()
            && h.journal_recovery.is_none()
            && h.recovery_basis.as_ref() == Some(basis)
    })
}
pub(super) fn stage_reconciling(s: &Session, p: &Value) -> bool {
    bundle::reconciling(s, p)
}
pub(super) fn activation_reconciling(s: &Session, p: &Value) -> bool {
    p["nativeSession"].as_str() == s.agent_bridge.epoch.as_deref()
        && s.agent_bridge.closed_workspace_updates.iter().any(|h| {
            h.activated
                && p["token"] == h.token
                && p["workspace"] == h.workspace
                && p["update"] == h.update
                && serde_json::to_value(&h.scope).is_ok_and(|scope| p["scope"] == scope)
        })
}
pub(super) fn open_candidate(s: &mut Session, p: &Value) -> Result<Value> {
    bundle::open(s, p)
}
pub(super) fn configure_candidate_persistence(s: &mut Session, cfg: crate::autosave::AutosaveConfig) -> Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if let Some(input) = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.restored_current_image.clone()) {
        input.preflight_directory(&cfg.data_dir, &media_guard::Check::new(web_time::Instant::now()))?;
    }
    let h = s
        .agent_bridge
        .workspace_update
        .as_mut()
        .filter(|h| h.candidate && !h.candidate_cancelled && h.persistence_start.is_none())
        .ok_or_else(|| bad(ACTIVATE, "fresh paused candidate required before configuring persistence"))?;
    let directory = &cfg.data_dir;
    if !directory.is_absolute() || directory.parent().is_none_or(|p| std::fs::canonicalize(p).ok().as_deref() != Some(p)) {
        return Err(bad(ACTIVATE, "new absolute unredirected candidate data directory required"));
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory).map_err(|e| bad(ACTIVATE, format!("candidate data directory must be new: {e}")))?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        let frame = filmcraft_format::retained::preview_context::DirectoryFrame::capture(directory, &|| Ok(()))
            .map_err(|e| bad(ACTIVATE, format!("candidate directory descriptor unavailable: {e}")))?;
        if !frame.exists() {
            return Err(bad(ACTIVATE, "new candidate directory disappeared"));
        }
        h.persistence_directory = Some(frame);
    }
    h.persistence_start = Some(cfg);
    Ok(())
}
pub(super) fn commands() -> Vec<CommandSpec> {
    [
        (BEGIN, "Pause Native Workspace for Update", "{token,scope,workspace,update}", begin as fn(&mut Session, &Value) -> Result<Value>),
        (STATUS, "Read Native Workspace Update", "{scope,workspace,update}", status),
        (
            INSPECT_PAUSED_HISTORY,
            "Inspect Paused Native Workspace History",
            "{token,scope,workspace,update,projectPath,activationToken,nativeSession,revision,sequence,checkpoint,sha256,snapshotSha256?}",
            retained_history::run,
        ),
        (
            PREPARE_PAUSED_IMAGE,
            "Prepare Current Paused Native Recovery Image",
            "{token,scope,workspace,update,projectPath,activationToken,nativeSession,capture,action:prepare|read|dependencies}",
            current_image::run,
        ),
        (
            PAUSED_RECOVERY_RECORD,
            "Retain Current Paused Recovery Publication",
            "{token,scope,workspace,update,projectPath,activationToken,nativeSession,capture,action,manifestPath,reserveBytes}",
            current_image::run_record,
        ),
        (CANCEL, "Cancel Native Workspace Update", "{token,scope,workspace,update,nativeSession}", cancel),
        (bundle::STAGE, "Stage Native Workspace Update", "{token,scope,workspace,update,pauseToken,checkpoint,checkpointSha256}", bundle::stage),
        (ACTIVATE, "Activate Native Workspace Update", "{token,scope,workspace,update,nativeSession}", activate),
        (RETIRE, "Retire Staged Native Workspace Owner", "{token,scope,workspace,update,pauseToken,stageSha256}", retire),
        (
            activation_refresh::REFRESH,
            "Refresh Cancelled Native Activation",
            "{token,scope,workspace,update,nativeSession,previousToken,activationToken}",
            activation_refresh::run,
        ),
    ]
    .into_iter()
    .map(|(id, label, params, run)| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false })
    .collect()
}

// Shared complete stable saved-file hashing for private native handoffs.
pub(super) fn project_file_hash(path: &std::path::Path) -> Result<String> {
    bundle::project_file_hash(path)
}

pub(super) fn require_idle_writer_jobs(s: &Session) -> Result<()> {
    // Jobs that can change the project must settle on the original owner. Never
    // freeze an intermediate recording or transfer an unowned background writer.
    if s.voiceover.recording()
        || !s.media_jobs.is_empty()
        || !s.mask_jobs.is_empty()
        || !s.scene_jobs.is_empty()
        || !s.stepped.is_empty()
        || s.media_scan.is_some()
        || s.export_queue.is_active()
        || s.jobs.iter().any(|job| job.result.lock().unwrap_or_else(|e| e.into_inner()).is_none())
    {
        return Err(bad(
            BEGIN,
            format!(
                "wait for current native work before the workspace update; no job was stopped or replayed \
                 (voiceover={}, mediaJobs={}, maskJobs={}, sceneJobs={}, stepped={}, mediaScan={}, exportQueue={}, unfinishedJobs={})",
                s.voiceover.recording(),
                s.media_jobs.len(),
                s.mask_jobs.len(),
                s.scene_jobs.len(),
                s.stepped.len(),
                s.media_scan.is_some(),
                s.export_queue.is_active(),
                s.jobs.iter().filter(|job| job.result.lock().unwrap_or_else(|e| e.into_inner()).is_none()).count()
            ),
        ));
    }
    Ok(())
}

// Shared startup fence for verified native workspace and identity recovery.
// The caller builds a private candidate; no live editor can be replaced here.
pub(super) fn hold_candidate(
    candidate: &mut Session,
    scope: sequences::Scope,
    workspace: String,
    update: String,
    activation_token: String,
    saved_project_hash: String,
) -> Result<Value> {
    hold_scoped_candidate(candidate, LifecycleScope::Sequence(scope), workspace, update, activation_token, saved_project_hash)
}
pub(super) fn hold_scoped_candidate(
    candidate: &mut Session,
    scope: LifecycleScope,
    workspace: String,
    update: String,
    activation_token: String,
    saved_project_hash: String,
) -> Result<Value> {
    scoped_address(&scope, &workspace, &update)?;
    if !token_ok(&activation_token) || candidate.persistence.is_some() || candidate.agent_bridge.workspace_update.is_some() {
        return Err(bad(BEGIN, "fresh private candidate and activation token required"));
    }
    let native_session = history_checkpoint::epoch(candidate)?;
    let request = json!({"token":activation_token,"scope":scope,"workspace":workspace,"update":update});
    candidate.execute("agent.readHistoryCheckpointBasis", json!({"token":activation_token,"scope":scope}))?;
    let basis = history_checkpoint::retained_scoped_basis(candidate, &activation_token, &scope)?;
    candidate.agent_bridge.workspace_update = Some(Held {
        request,
        scope,
        workspace,
        update,
        token: activation_token,
        revision: candidate.revision,
        native_session,
        dirty: candidate.is_dirty(),
        basis,
        stage: None,
        candidate: true,
        current_image: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        restored_current_image: None,
        candidate_cancelled: false,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        current_image_retirement: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        current_activation_owner: None,
        retiring: false,
        saved_project_hash: Some(saved_project_hash),
        persistence_paused: false,
        persistence_start: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        persistence_directory: None,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        library_commit: None,
        recovery_basis: None,
        activation_refresh: None,
        activation_refresh_count: 0,
        journal_recovery: None,
        project_creation: None,
    });
    view(candidate).ok_or_else(|| bad(BEGIN, "candidate fence missing"))
}

// Identity conversion may publish only on the exact staged candidate. The
// original owner (including its acknowledged autosave pause) never changes.
pub(super) fn identity_candidate_hold(s: &Session, scope: &sequences::Scope, workspace: &str, update: &str) -> Result<Option<Held>> {
    let Some(h) = s.agent_bridge.workspace_update.as_ref() else { return Ok(None) };
    if !h.candidate
        || h.candidate_cancelled
        || h.retiring
        || h.scope != LifecycleScope::Sequence(scope.clone())
        || h.workspace != workspace
        || h.update != update
        || h.stage.is_some()
        || h.persistence_paused
        || h.saved_project_hash.is_none()
        || s.persistence.is_some()
        || crate::project_workspace::identity(&s.project)?.is_some()
    {
        return Err(bad(
            BEGIN,
            "identity publication requires this exact uncancelled legacy candidate with persistence still deferred; the source owner remains unchanged",
        ));
    }
    history_checkpoint::validate_retained_basis(s, &h.basis)?;
    bundle::check_saved_project(s)?;
    Ok(Some(h.clone()))
}
impl Held {
    pub(super) fn promoted_identity(&mut self, basis: history_checkpoint::Basis, revision: u64, dirty: bool, activation_token: String) {
        self.basis = basis;
        self.revision = revision;
        self.dirty = dirty;
        self.token = activation_token;
        self.request = json!({"token":self.token,"scope":self.scope,"workspace":self.workspace,"update":self.update});
    }
}

// Lifecycle authority stays in the native retained candidate guard, separately
// from the bounded metadata-basis LRU. This starts the ordinary operation fence;
// activate still validates the complete guard, epoch and disk before publishing.
pub(super) fn pending_activation_basis(s: &Session, p: &Value) -> bool {
    s.agent_bridge.workspace_update.as_ref().is_some_and(|h| {
        h.candidate
            && !h.candidate_cancelled
            && !h.retiring
            && p["token"] == h.token
            && p["workspace"] == h.workspace
            && p["update"] == h.update
            && p["nativeSession"] == h.native_session
            && s.agent_bridge.epoch.as_deref() == Some(h.native_session.as_str())
            && serde_json::to_value(&h.scope).is_ok_and(|scope| p["scope"] == scope)
    })
}

pub(super) fn paused_project_owner(s: &Session, scope: &crate::project_workspace::ProjectScope) -> bool {
    s.agent_bridge
        .workspace_update
        .as_ref()
        .is_some_and(|h| !h.candidate && !h.retiring && !h.candidate_cancelled && h.persistence_paused && h.scope == LifecycleScope::Project(scope.clone()))
}
pub(super) fn install_project_stage(s: &mut Session, request: Value, basis: Value, saved_hash: String, receipt: Option<Value>) -> Result<()> {
    let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(BEGIN, "project owner pause disappeared"))?;
    h.stage = Some(Staged { request, receipt });
    h.recovery_basis = Some(basis);
    h.saved_project_hash = Some(saved_hash);
    Ok(())
}
pub(super) fn project_stage(s: &Session) -> Option<(&Value, &Value)> {
    let h = s.agent_bridge.workspace_update.as_ref()?;
    Some((h.stage.as_ref()?.receipt.as_ref()?, h.recovery_basis.as_ref()?))
}
pub(super) fn install_candidate_recovery_basis(s: &mut Session, basis: Value) -> Result<()> {
    let h = s
        .agent_bridge
        .workspace_update
        .as_mut()
        .filter(|h| h.candidate && matches!(h.scope, LifecycleScope::Project(_)))
        .ok_or_else(|| bad(ACTIVATE, "independent paused candidate required"))?;
    h.recovery_basis = Some(basis);
    Ok(())
}

pub(super) fn activation_refresh_reconciling(s: &Session, p: &Value) -> bool {
    activation_refresh::reconciling(s, p)
}

pub(super) fn install_candidate_journal_receipt(s: &mut Session, receipt: Value) -> Result<()> {
    let h = s
        .agent_bridge
        .workspace_update
        .as_mut()
        .filter(|h| h.candidate && matches!(h.scope, LifecycleScope::Project(_)) && h.project_creation.is_none())
        .ok_or_else(|| bad(ACTIVATE, "independent paused journal candidate required"))?;
    h.journal_recovery = Some(receipt);
    Ok(())
}

pub(super) fn install_project_creation_receipt(s: &mut Session, receipt: Value) -> Result<()> {
    let h = s
        .agent_bridge
        .workspace_update
        .as_mut()
        .filter(|h| h.candidate && matches!(h.scope, LifecycleScope::Project(_)) && h.journal_recovery.is_none())
        .ok_or_else(|| bad(ACTIVATE, "fresh independent creation candidate required"))?;
    h.project_creation = Some(receipt);
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) use current_image::restore::{Expected as CurrentImageExpected, Loaded as LoadedCurrentImage};
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn read_current_image(value: &Value, expected: CurrentImageExpected<'_>, check: &media_guard::Check) -> Result<LoadedCurrentImage> {
    current_image::restore::load(value, expected, check)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn retain_restored_current_image(s: &mut Session, input: std::sync::Arc<current_image::restore::Input>) -> Result<()> {
    input.current(&media_guard::Check::new(web_time::Instant::now()))?;
    let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(BEGIN, "actual original fresh candidate required"))?;
    if !h.candidate || h.restored_current_image.is_some() || s.persistence.is_some() {
        return Err(bad(BEGIN, "current image may only be retained by its original fresh paused candidate"));
    }
    h.restored_current_image = Some(input);
    Ok(())
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
pub(super) fn prepare_current_activation_test(s: &mut Session, stop_after_directory: bool) -> Result<()> {
    current_image::activation::prepare_test(s, stop_after_directory)
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
pub(super) fn current_activation_input_test(s: &Session) -> Result<std::sync::Arc<current_image::restore::Input>> {
    s.agent_bridge.workspace_update.as_ref().and_then(|h| h.restored_current_image.clone()).ok_or_else(|| bad(ACTIVATE, "actual current input required"))
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
pub(super) fn prepare_current_activation_worker_stop_test(s: &mut Session) -> Result<()> {
    current_image::activation::prepare_worker_stop_test(s)
}

#[cfg(test)]
pub(super) fn prepare_current_activation_authored_stop_test(s: &mut Session, phase: u8) -> Result<()> {
    current_image::activation::prepare_authored_stop_test(s, phase)
}

#[cfg(test)]
pub(super) fn prepare_current_activation_journal_fault_test(s: &mut Session, fault: u8) -> Result<()> {
    current_image::activation::prepare_journal_fault_test(s, fault)
}
