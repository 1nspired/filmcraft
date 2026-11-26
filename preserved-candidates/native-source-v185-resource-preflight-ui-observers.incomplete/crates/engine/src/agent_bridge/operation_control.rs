//! Out-of-band cancellation for an already issued native read ticket. This
//! handle can stop verification, but owns no Session, project or history writer.
use super::{media_guard::Check, token_ok};
use crate::{Result, Session, commands::bad};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

const AVAILABLE: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;
const PUBLISHING: u8 = 3;
const COMMITTED: u8 = 4;
const REJECTED: u8 = 5;
struct Flag {
    state: AtomicU8,
    check: Arc<Check>,
}
#[derive(Default)]
struct Inner {
    rows: VecDeque<(String, Arc<Flag>)>,
    active: Option<Arc<Flag>>,
    save_wait: Option<super::SaveTaskControl>,
}
/// Thread-safe, session-local cancellation only. Never an executor or rollback.
#[derive(Clone, Default)]
pub struct AgentOperationControl {
    inner: Arc<Mutex<Inner>>,
    transport: Arc<AtomicBool>,
}
impl AgentOperationControl {
    // Physical session identity only; never serialized or reconstructed from a
    // native epoch, historical JSON or a cancellation token.
    pub(super) fn same_original_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) && Arc::ptr_eq(&self.transport, &other.transport)
    }
    pub(crate) fn install_save_wait(&self, control: super::SaveTaskControl) -> Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.save_wait.is_some() || inner.active.is_some() {
            return Err(bad("native Save", "another original worker wait is active"));
        }
        inner.save_wait = Some(control);
        Ok(())
    }
    pub(crate) fn finish_save_wait(&self, control: &super::SaveTaskControl) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.save_wait.as_ref().is_some_and(|current| current.same(control)) {
            inner.save_wait = None;
        }
    }
    /// Independent atomics only. Stopping a wait is not native cancellation.
    pub fn cancel_save_wait(&self, params: &Value) -> Result<Value> {
        let parsed = super::SaveTaskControl::new("agent.cancelProjectSaveOperation", params)?;
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = inner.save_wait.as_ref().filter(|c| c.matches(params)) {
            return Ok(current.stop());
        }
        let mut value = parsed.progress();
        value["phase"] = json!("unknown");
        value["cancellable"] = json!(false);
        value["waitStopAccepted"] = json!(false);
        Ok(value)
    }

    /// Diagnostic atomics only; no Session, read ticket, writer or rollback authority.
    pub fn progress(&self, token: &str) -> Value {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, flag)) = inner.rows.iter().find(|(id, _)| id == token) else {
            return json!({"phase":"unknown","cancellable":false});
        };
        let state = flag.state.load(Ordering::Acquire);
        let phase = match state {
            AVAILABLE => "queued",
            RUNNING => "verifying_inputs",
            CANCELLED => "cancelled",
            PUBLISHING => "publishing",
            COMMITTED => "committed",
            _ => "rejected",
        };
        json!({"phase":phase,"cancellable":matches!(state, AVAILABLE | RUNNING),
            "bytes_read":flag.check.bytes.load(Ordering::Relaxed).to_string(),
            "completed_checks":flag.check.files.load(Ordering::Relaxed)})
    }

    /// A host calls this only after installing an independent cancellation door.
    pub fn enable_transport(&self) {
        self.transport.store(true, Ordering::Release);
    }
    pub(super) fn available(&self) -> bool {
        self.transport.load(Ordering::Acquire)
    }
    pub(super) fn register(&self, token: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.rows.iter().any(|(id, _)| id == token) {
            return Err(bad("agent.snapshot", "operation token was already issued; use a fresh read"));
        }
        if inner.rows.len() >= super::TICKET_LIMIT {
            let old = inner
                .rows
                .iter()
                .position(|(_, f)| !matches!(f.state.load(Ordering::Acquire), RUNNING | PUBLISHING))
                .ok_or_else(|| bad("agent.snapshot", "native operation cancellation cache is busy"))?;
            inner.rows.remove(old);
        }
        inner.rows.push_back((token.into(), Arc::new(Flag { state: AtomicU8::new(AVAILABLE), check: Arc::new(Check::new(web_time::Instant::now())) })));
        Ok(())
    }
    #[cfg(test)]
    fn begin(&self, token: &str) -> Result<Operation> {
        self.begin_sources(token, false, 0)
    }
    fn begin_sources(&self, token: &str, admission: bool, bytes: u64) -> Result<Operation> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.active.is_some() {
            return Err(bad("agent.apply", "another guarded native operation is active"));
        }
        let index =
            inner.rows.iter().position(|(id, _)| id == token).ok_or_else(|| bad("agent.apply", "operation ticket expired; reconcile and take a fresh read"))?;
        // Start the total verification budget at Send, not at the earlier read.
        let old = &inner.rows[index].1;
        old.state
            .compare_exchange(AVAILABLE, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| bad("agent.apply", "operation was cancelled or already attempted; reconcile, never replay"))?;
        let check = if admission {
            Check::for_admission_publication(web_time::Instant::now())
        } else {
            Check::for_sources(web_time::Instant::now(), Arc::new(AtomicBool::new(false)), bytes)
        };
        let flag = Arc::new(Flag { state: AtomicU8::new(RUNNING), check: Arc::new(check) });
        inner.rows[index].1 = flag.clone();
        inner.active = Some(flag.clone());
        Ok(Operation { control: self.clone(), flag })
    }
    pub(super) fn check(&self) -> Option<Arc<Check>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).active.as_ref().map(|f| f.check.clone())
    }
    // A candidate token may be replaced only after this same process proves
    // cancellation won before publication, with no old operation still active.
    pub(super) fn cancelled_before_publication(&self, token: &str) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .rows
            .iter()
            .find(|(id, _)| id == token)
            .is_some_and(|(_, flag)| flag.state.load(Ordering::Acquire) == CANCELLED && !inner.active.as_ref().is_some_and(|v| Arc::ptr_eq(v, flag)))
    }
    pub(super) fn preparation_stop(&self, token: &str) -> Result<Arc<AtomicBool>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let flag = inner
            .rows
            .iter()
            .find(|(id, _)| id == token)
            .map(|(_, f)| f)
            .ok_or_else(|| bad("agent.prepareSourceAdmission", "native operation token expired"))?;
        if flag.state.load(Ordering::Acquire) != AVAILABLE {
            return Err(bad("agent.prepareSourceAdmission", "native operation was already attempted or cancelled"));
        }
        Ok(flag.check.stop.clone())
    }
    pub(super) fn enter_publication(&self, token: &str) -> Result<()> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let flag = inner.rows.iter().find(|(id, _)| id == token).map(|(_, f)| f).ok_or_else(|| bad("agent.apply", "missing native operation fence"))?;
        if !inner.active.as_ref().is_some_and(|f| Arc::ptr_eq(f, flag)) {
            return Err(bad("agent.apply", "native publication requires its active operation fence"));
        }
        flag.check.check()?;
        flag.state
            .compare_exchange(RUNNING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| bad("agent.apply", "native operation cancelled before publication; no edit was published"))?;
        Ok(())
    }
    /// Transport-thread cancellation. Accepted means publication has not started.
    /// Unknown/late replies do not certify rollback; reconcile the native receipt.
    pub fn cancel(&self, params: &Value) -> Result<Value> {
        let token = params.get("token").and_then(Value::as_str).filter(|t| token_ok(t));
        if token.is_none() || params.as_object().is_none_or(|p| p.len() != 1) {
            return Err(bad("engine.cancelOperation", "one explicit issued operation token required"));
        }
        let token = token.ok_or_else(|| bad("engine.cancelOperation", "missing operation token"))?;
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, flag)) = inner.rows.iter().find(|(id, _)| id == token) else {
            return Ok(json!({"token":token,"status":"unknown","cancellationAccepted":false}));
        };
        let mut state = flag.state.load(Ordering::Acquire);
        while matches!(state, AVAILABLE | RUNNING) {
            match flag.state.compare_exchange(state, CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    state = CANCELLED;
                    break;
                }
                Err(actual) => state = actual,
            }
        }
        if state == CANCELLED {
            flag.check.stop.store(true, Ordering::Release);
        }
        let status = match state {
            CANCELLED => "cancelled",
            PUBLISHING => "publication_started",
            COMMITTED => "committed",
            _ => "rejected",
        };
        Ok(json!({"token":token,"status":status,"cancellationAccepted":state==CANCELLED}))
    }
}
// A private resource read uses the SAME cancellation/active-operation state.
// It never enters publication or grants an edit/startup/file owner.
pub(super) struct ResourcePreflightOperation(Operation);
impl AgentOperationControl {
    pub(super) fn begin_resource_preflight(&self, token: &str) -> Result<ResourcePreflightOperation> {
        Ok(ResourcePreflightOperation(self.begin_sources(token, false, 0)?))
    }
}
impl ResourcePreflightOperation {
    pub(super) fn check(&self) -> Result<()> {
        if self.0.flag.state.load(Ordering::Acquire) != RUNNING {
            return Err(bad("resource preflight", "the original resource read was cancelled or ended"));
        }
        self.0.flag.check.check()
    }
    pub(super) fn complete(self) {
        let _=self.0.flag.state.compare_exchange(RUNNING, COMMITTED, Ordering::AcqRel, Ordering::Acquire);
    }
}

// Finite application-tab publication uses the SAME original operation/CAS.
// This carrier stays private to the native agent boundary; no generic public
// publication callback or serialized permission is exposed to the application.
pub(super) struct ApplicationTabOperation(Operation);
impl AgentOperationControl {
    pub(super) fn begin_application_tab(&self, token: &str) -> Result<ApplicationTabOperation> {
        let operation = self.begin_sources(token, false, 0)?;
        self.enter_publication(token)?;
        Ok(ApplicationTabOperation(operation))
    }
}
impl ApplicationTabOperation {
    pub(super) fn complete(self) {
        self.0.flag.state.store(COMMITTED, Ordering::Release);
    }
}
struct Operation {
    control: AgentOperationControl,
    flag: Arc<Flag>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        let state = self.flag.state.load(Ordering::Acquire);
        if matches!(state, RUNNING | PUBLISHING) {
            let _ = self.flag.state.compare_exchange(state, REJECTED, Ordering::AcqRel, Ordering::Acquire);
        }
        self.flag.check.stop.store(true, Ordering::Release);
        let mut inner = self.control.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.active.as_ref().is_some_and(|f| Arc::ptr_eq(f, &self.flag)) {
            inner.active = None;
        }
    }
}
pub(super) fn run(s: &mut Session, id: &str, p: &Value, run: impl FnOnce(&mut Session, &Value) -> Result<Value>) -> Result<Value> {
    if !matches!(
        id,
        "agent.apply"
            | super::project_navigation::COMMAND
            | super::editor_setting::COMMAND
            | super::editor_compound::COMMAND
            | super::sequence_views::COMMAND
            | "agent.undo"
            | "agent.redo"
            | "agent.commitCondense"
            | "agent.saveHistoryCheckpoint"
            | "agent.restoreHistoryCheckpoint"
            | "agent.validateRetainedHistory"
            | super::workspace_update::INSPECT_PAUSED_HISTORY
            | super::workspace_update::PREPARE_PAUSED_IMAGE
            | super::workspace_update::PAUSED_RECOVERY_RECORD
            | "agent.inspectHistoryCheckpoint"
            | "agent.inspectCheckpointProvenance"
            | "agent.readHistoryCheckpointOutcome"
            | "agent.inspectIdentityMigration"
            | "agent.publishIdentityMigration"
            | "agent.readIdentityPublication"
            | "agent.startExport"
            | "agent.publishExport"
            | "agent.beginWorkspaceUpdate"
            | "agent.stageWorkspaceUpdate"
            | "agent.stageProjectRecovery"
            | "agent.prepareProjectRecovery"
            | "agent.activateWorkspaceUpdate"
            | "agent.refreshWorkspaceActivation"
            | "agent.retireWorkspaceUpdate"
    ) {
        return run(s, p);
    }
    if matches!(id, "agent.startExport" | "agent.publishExport") && super::export::reconciling(s, id, p) {
        return run(s, p);
    }
    if id == "agent.beginWorkspaceUpdate" && super::workspace_update::reconciling(s, p) {
        return run(s, p);
    }
    if matches!(id, "agent.stageWorkspaceUpdate" | "agent.stageProjectRecovery") && super::workspace_update::stage_reconciling(s, p) {
        return run(s, p);
    }
    if id == "agent.refreshWorkspaceActivation" && super::workspace_update::activation_refresh_reconciling(s, p) {
        return run(s, p);
    }
    if id == "agent.activateWorkspaceUpdate" && super::workspace_update::activation_reconciling(s, p) {
        return run(s, p);
    }
    let paused_record = id == super::workspace_update::PAUSED_RECOVERY_RECORD;
    if paused_record {
        super::workspace_update::prepare_paused_record_operation(s, p)?;
    }
    let paused_image = id == super::workspace_update::PREPARE_PAUSED_IMAGE;
    if paused_image {
        super::workspace_update::prepare_paused_image_operation(s, p)?;
    }
    let paused_inspection = id == super::workspace_update::INSPECT_PAUSED_HISTORY || paused_image || paused_record;
    if paused_inspection && !paused_image && !paused_record {
        super::workspace_update::prepare_paused_history_inspection(s, p)?;
    }
    let token = p.get("token").and_then(Value::as_str).unwrap_or("");
    // Preserve historical receipt reconciliation even after cancellation,
    // expiry, human Undo or a native-process checkpoint restore.
    let persistence = (matches!(id, "agent.inspectIdentityMigration" | "agent.publishIdentityMigration" | "agent.readIdentityPublication")
        || id == "agent.saveHistoryCheckpoint" && p.get("metadataBasis") == Some(&Value::Bool(true))
        || matches!(
            id,
            "agent.beginWorkspaceUpdate"
                | "agent.stageWorkspaceUpdate"
                | "agent.stageProjectRecovery"
                | "agent.prepareProjectRecovery"
                | "agent.activateWorkspaceUpdate"
                | "agent.refreshWorkspaceActivation"
                | "agent.retireWorkspaceUpdate"
        ))
        && super::history_checkpoint::has_basis(s, token)
        || id == "agent.activateWorkspaceUpdate" && super::workspace_update::pending_activation_basis(s, p);
    let export_publication = id == "agent.publishExport" && super::export::publication_ticket(s, p);
    if !paused_inspection && !persistence && !export_publication && !s.agent_bridge.tickets.iter().any(|t| t.token == token && t.committed.is_none()) {
        return run(s, p);
    }
    let admission = id == "agent.apply" && p.get("admissionPreparation").is_some();
    if admission && !super::admission_jobs::eligible(s, p) {
        return Err(bad("agent.apply", "exact source admission preparation is not ready/current; no publication or fallback was started"));
    }
    let mut bytes = s.agent_bridge.tickets.iter().find(|t| t.token == token).map_or(0, |t| t.media.source_bytes());
    if id == "agent.redo"
        && let Some((_, project)) = s.history.redo.last()
    {
        bytes = bytes.saturating_add(super::admission::restored_source_bytes(s, project));
    }
    if id == "agent.commitCondense"
        && let Some(candidate) = p.get("candidate").and_then(Value::as_str)
    {
        let (_, media) = super::candidates::read_basis(s, candidate)?;
        bytes = bytes.saturating_add(media.source_bytes());
    }
    // Match capability advertising: synchronous/legacy clients do not gain a
    // long exclusive operation without the worker and independent Stop path.
    if !s.worker_available || !super::operation_cancellation_available(s) {
        bytes = 0;
    }
    let operation = s.agent_bridge.operations.begin_sources(token, admission, bytes)?;
    let result = run(s, p);
    if result.as_ref().is_ok_and(|r| {
        r["status"] == "committed"
            || id == super::project_navigation::COMMAND && r["status"] == "selected"
            || id == super::editor_setting::COMMAND && r["status"] == "configured"
            || id == super::editor_compound::COMMAND && r["status"] == "configured"
            || id == super::sequence_views::COMMAND && r["status"] == "navigated"
            || id == "agent.saveHistoryCheckpoint" && r["status"] == "saved"
            || id == "agent.startExport" && r["started"] == true
            || id == "agent.publishExport" && r["status"] == "delivered"
            || id == "agent.prepareProjectRecovery" && r["status"] == "prepared"
            || paused_image && r["status"] == "prepared"
            || paused_record && r["status"] == "recorded"
            || id == "agent.beginWorkspaceUpdate" && r["status"] == "paused"
            || matches!(id, "agent.stageWorkspaceUpdate" | "agent.stageProjectRecovery") && r["status"] == "staged"
            || id == "agent.refreshWorkspaceActivation" && r["status"] == "prepared"
            || id == "agent.activateWorkspaceUpdate" && r["status"] == "activated"
            || matches!(id, "agent.validateRetainedHistory" | super::workspace_update::INSPECT_PAUSED_HISTORY) && r["status"] == "retained"
            || id == "agent.retireWorkspaceUpdate" && r["status"] == "retiring"
    }) {
        operation.flag.state.store(COMMITTED, Ordering::Release);
    } else if result.is_err() {
        // Preserve the existing contract for a KNOWN pre-publication refusal.
        // Cancellation wins the CAS and never becomes reusable; panic/late
        // publication outcomes are not reopened by this path.
        let _ = operation.flag.state.compare_exchange(RUNNING, AVAILABLE, Ordering::AcqRel, Ordering::Acquire);
    }
    result
}

#[cfg(test)]
#[path = "operation_control_tests.rs"]
mod tests;

#[cfg(test)]
mod activation_publication_proof_tests {
    use super::*;
    #[test]
    fn cancellation_proof_requires_a_drained_operation_and_never_accepts_started_or_rejected_publication() {
        let c = AgentOperationControl::default();
        let a = "a".repeat(64);
        assert!(!c.cancelled_before_publication(&a));
        c.register(&a).unwrap();
        assert!(!c.cancelled_before_publication(&a));
        let op = c.begin(&a).unwrap();
        assert_eq!(c.cancel(&json!({"token":a})).unwrap()["cancellationAccepted"], true);
        assert!(!c.cancelled_before_publication(&a));
        drop(op);
        assert!(c.cancelled_before_publication(&a));
        let b = "b".repeat(64);
        c.register(&b).unwrap();
        let op = c.begin(&b).unwrap();
        c.enter_publication(&b).unwrap();
        assert_eq!(c.cancel(&json!({"token":b})).unwrap()["cancellationAccepted"], false);
        assert!(!c.cancelled_before_publication(&b));
        drop(op);
        assert!(!c.cancelled_before_publication(&b));
        assert_eq!(c.cancel(&json!({"token":b})).unwrap()["status"], "rejected");
        assert!(!c.cancelled_before_publication(&b));
    }
}
