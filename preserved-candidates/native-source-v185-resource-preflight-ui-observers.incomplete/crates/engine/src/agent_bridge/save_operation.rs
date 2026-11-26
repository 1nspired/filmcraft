//! Finite manager-facing lifecycle of the actual original Save operation.
//! The real Key remains in this Session. Requests/descriptions cannot recreate
//! it, release a dispatched owner or authorize startup. File publication uses
//! only its retained physical Record and exact previously prepared intent.
use super::{media_guard::Check, parse, token_ok};
mod admission;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) mod managed_dispatch;
mod record;
pub(crate) mod task;
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
    project_save::operation,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) const BEGIN: &str = "agent.beginProjectSaveOperation";
pub(super) const ADMISSION_PREPARE: &str = "agent.prepareProjectSaveAdmission";
pub(super) const ADMISSION_READ: &str = "agent.readProjectSaveAdmission";
pub(super) const EFFECTS: &str = "agent.readProjectSaveEffects";
pub(super) const STATUS: &str = "agent.projectSaveOperationStatus";
pub(super) const CANCEL: &str = "agent.cancelProjectSaveOperation";
pub(super) const RECOVERY: &str = "agent.prepareProjectSaveRecovery";
pub(super) const RECORD_PREPARE: &str = "agent.prepareProjectSaveRecoveryRecord";
pub(super) const RECORD_READ: &str = "agent.readProjectSaveRecoveryRecord";
pub(super) const RECORD_PUBLISH: &str = "agent.publishProjectSaveRecoveryRecord";

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    job: String,
    operation: String,
}
pub(super) struct Owner {
    request: Request,
    key: operation::Key,
    recovery: Option<super::history_checkpoint::SaveRecovery>,
    recovery_after: Option<super::history_checkpoint::SaveRecovery>,
    admission: Option<admission::Slot>,
    dispatch_attempted: bool,
    plan_description: Value,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    manager: Option<managed_dispatch::ManagerBasis>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(BEGIN, message)
}
fn request(p: &Value) -> Result<Request> {
    let r: Request = parse(BEGIN, p)?;
    if !token_ok(&r.job) || !token_ok(&r.operation) || r.job == r.operation {
        return Err(error("distinct exact original planning job and native operation ids required"));
    }
    Ok(r)
}
pub(super) fn allows(s: &Session, command: &str) -> bool {
    s.agent_bridge.save_operation.is_some()
        && (managed_dispatch_supports(command)
            || matches!(
                command,
                BEGIN
                    | STATUS
                    | EFFECTS
                    | CANCEL
                    | RECOVERY
                    | RECORD_PREPARE
                    | RECORD_READ
                    | RECORD_PUBLISH
                    | ADMISSION_PREPARE
                    | ADMISSION_READ
                    | super::project_plane::VALIDATE
            ))
}
fn managed_dispatch_supports(command: &str) -> bool {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        command == managed_dispatch::COMMAND
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = command;
        false
    }
}
pub(super) fn task_request(command: &str, p: &Value) -> Result<Request> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if command == managed_dispatch::COMMAND {
        return managed_dispatch::parameters(p);
    }
    if record::supports(command) {
        record::parameters(command, p).map(|r| r.0)
    } else if admission::supports(command) {
        admission::parameters(command, p).map(|r| r.0)
    } else {
        request(p)
    }
}
fn stopped(s: &Session, r: &Request) -> Result<bool> {
    if let Some(previous) = s.agent_bridge.stopped_save_operations.iter().find(|p| p.operation == r.operation) {
        if previous != r {
            return Err(error("native Save operation id already describes a different original job"));
        }
        return Ok(true);
    }
    Ok(false)
}
fn cancelled(r: &Request) -> Value {
    json!({"kind":"native_managed_Save_operation","version":1,"job":r.job,"operation":r.operation,
        "status":"cancelled","dispatched":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false})
}
fn remember(s: &mut Session, r: Request) {
    s.agent_bridge.stopped_save_operations.push_back(r);
    while s.agent_bridge.stopped_save_operations.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_save_operations.pop_front();
    }
}
fn status_for(s: &Session, r: &Request, check: &impl Fn() -> Result<()>) -> Result<Value> {
    check()?;
    if stopped(s, r)? {
        return Ok(cancelled(r));
    }
    let owner = s.agent_bridge.save_operation.as_ref().ok_or_else(|| error("unknown native Save owner; never reconstruct it from saved metadata"))?;
    if owner.request != *r {
        return Err(error("another exact original Save operation owns the native Session"));
    }
    status_for_owner(s, owner, check)
}
fn status_for_owner(s: &Session, owner: &Owner, check: &impl Fn() -> Result<()>) -> Result<Value> {
    let r = &owner.request;
    let pending = operation::status(s, &owner.key, check)?;
    Ok(json!({"kind":"native_managed_Save_operation","version":1,"job":r.job,"operation":r.operation,
        "status":"held","pending":pending,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false}))
}
fn begin(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(p)?;
    let task = s.save_lifecycle_control.clone();
    let check = || task.as_ref().map_or(Ok(()), |t| t.current(BEGIN, &r));
    check()?;
    if stopped(s, &r)? || s.agent_bridge.save_operation.is_some() {
        // A lost reply is observed through the SAME retained Key, without
        // starting a job, consuming another payload or reacquiring pauses.
        return status_for(s, &r, &check);
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let manager = s.agent_bridge.manager_save_origin.as_ref().map(|o| o.bind(s)).transpose()?.flatten();
    let (key, plan_description) = super::save_plan_jobs::begin_operation_detailed(s, &r.job, &r.operation, &check)?;
    s.agent_bridge.save_operation = Some(Owner {
        request: r.clone(),
        key,
        recovery: None,
        recovery_after: None,
        admission: None,
        dispatch_attempted: false,
        plan_description,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        manager,
    });
    // Install the key BEFORE attempting delivery so a lost status reply cannot
    // orphan the real operation or permit a second begin.
    status_for(s, &r, &check)
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(p)?;
    let task = s.save_lifecycle_control.clone();
    status_for(s, &r, &|| task.as_ref().map_or(Ok(()), |t| t.current(STATUS, &r)))
}
// Pure descriptive read of the SAME native key. Unlike frame preparation,
// this does not require an after-basis that may legitimately be unknown.
fn read_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(p)?;
    let task = s.save_lifecycle_control.clone();
    let fallback = Check::new(web_time::Instant::now());
    let check = task.as_ref().map_or(&fallback, |t| t.read_check());
    let current = || {
        check.check()?;
        task.as_ref().map_or(Ok(()), |t| t.current(EFFECTS, &r))
    };
    current()?;
    let owner = s.agent_bridge.save_operation.as_ref().ok_or_else(|| error("original pending Save key required for effect read; no cold key import"))?;
    if owner.request != r {
        return Err(error("foreign original Save effect read correlation"));
    }
    let observation = operation::observation::read(s, &owner.key, &current)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let scope = crate::project_workspace::identity(&s.project)?.ok_or_else(|| error("original saved Root identity required"))?.scope;
        super::project_plane::require_scope(s, &scope)?;
        let epoch = super::history_checkpoint::epoch(s)?;
        current()?;
        let result = json!({"kind":"native_Save_effects_read","version":1,"job":r.job,"operation":r.operation,
            "scope":scope,"nativeSession":epoch,"nativePid":std::process::id(),"observation":observation,
            "current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
        let size = serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len();
        if size > operation::observation::MAX_BYTES + 64 * 1024 {
            return Err(error("complete Save effect read envelope exceeds its bounded allowance; original owner retained"));
        }
        current()?;
        Ok(result)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = observation;
        Err(error("native Save effect reads unavailable on this target"))
    }
}
fn prepare_recovery(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(p)?;
    let task = s.save_lifecycle_control.clone();
    let fallback = super::media_guard::Check::new(web_time::Instant::now());
    let check = task.as_ref().map_or(&fallback, |t| t.read_check());
    if let Some(task) = &task {
        task.current(RECOVERY, &r)?;
    }
    check.check()?;
    let original = s.agent_bridge.save_operation.as_ref().ok_or_else(|| error("original pending Save key required; no cold key import"))?;
    if original.request != r {
        return Err(error("foreign Save recovery read correlation"));
    }
    let mut owner = s.agent_bridge.save_operation.take().ok_or_else(|| error("original Save owner disappeared"))?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let phase = operation::recovery_owner(s, &owner.key, &|| check.check())?.phase();
        let slot = if phase == "Prepared" {
            &mut owner.recovery
        } else if phase == "AwaitingReconciliation" {
            &mut owner.recovery_after
        } else {
            return Err(error("original Save recovery phase is unavailable"));
        };
        if slot.is_none() {
            *slot = Some(super::history_checkpoint::prepare_save_recovery(s, &owner.key, check)?);
        }
        let frame = slot.as_ref().ok_or_else(|| error("original Save recovery frame unavailable"))?;
        frame.current(s, &owner.key, check)?;
        if let Some(task) = &task {
            task.current(RECOVERY, &r)?;
        }
        let mut result = frame.describe(&r.job, &r.operation);
        result["retainedFrames"] = Value::Array(owner.recovery.iter().chain(owner.recovery_after.iter()).map(|f| f.describe(&r.job, &r.operation)).collect());
        Ok(result)
    }))
    .unwrap_or_else(|_| Err(error("native Save recovery read failed; original pending owner retained")));
    s.agent_bridge.save_operation = Some(owner);
    result
}

fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r = request(p)?;
    let task = s.save_lifecycle_control.clone();
    let current = || task.as_ref().map_or(Ok(()), |t| t.current(CANCEL, &r));
    current()?;
    if stopped(s, &r)? {
        return Ok(cancelled(&r));
    }
    if let Some(owner) = s.agent_bridge.save_operation.as_ref() {
        if owner.request != r {
            return Err(error("another exact original Save owns the Session; it was not cancelled"));
        }
    } else {
        // Stop may arrive before a queued begin. Tombstone both exact IDs so
        // delayed delivery cannot acquire workers behind the user's Stop.
        super::save_plan_jobs::cancel(s, &json!({"job":r.job}))?;
        remember(s, r.clone());
        return Ok(cancelled(&r));
    }
    let owner = s.agent_bridge.save_operation.take().ok_or_else(|| error("native Save owner disappeared before cancellation"))?;
    // This budget is only for joining the original Resume acknowledgement.
    // It cannot extend the original encoder/publication job's 45s lifetime.
    let check = Check::new(web_time::Instant::now());
    match operation::cancel_before_dispatch(s, &owner.key, &|| {
        current()?;
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        if let Some(manager) = &owner.manager {
            manager.current(&check)?;
        }
        check.check()
    }) {
        Ok(()) => {
            remember(s, r.clone());
            Ok(cancelled(&r))
        }
        Err(e) => {
            s.agent_bridge.save_operation = Some(owner);
            Err(e)
        }
    }
}
pub(super) fn commands() -> Vec<CommandSpec> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let dispatch = vec![CommandSpec {
        id: managed_dispatch::COMMAND,
        label: "Dispatch Retained Native Save",
        menu: &[],
        shortcut: None,
        params: r#"{"job":str,"operation":str,"capture":uuid,"manifestPath":str,"catalogSha256":sha256,"intentId":uuid}"#,
        enabled: always,
        run: managed_dispatch::run,
        journal: false,
    }];
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    let dispatch = Vec::new();
    [
        (BEGIN, "Retain Original Save Operation", begin as fn(&mut Session, &Value) -> Result<Value>),
        (STATUS, "Read Original Save Operation", status),
        (EFFECTS, "Read Complete Original Save Effects", read_effects),
        (ADMISSION_PREPARE, "Retain Original Save File Admission", admission::prepare),
        (ADMISSION_READ, "Read Retained Original Save File Admission", admission::read),
        (CANCEL, "Cancel Undispatched Original Save", cancel),
        (RECOVERY, "Prepare Original Save Recovery Read", prepare_recovery),
        (RECORD_PREPARE, "Prepare Original Save Recovery File Intent", record::prepare),
        (RECORD_READ, "Read Original Save Recovery File Intent", record::read),
        (RECORD_PUBLISH, "Publish Retained Original Save Recovery Record", record::publish),
    ]
    .into_iter()
    .map(|(id, label, run)| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params: if id == ADMISSION_PREPARE {
            r#"{"job":str,"operation":str,"capture":uuid,"volumes":[{"root":str,"device":str,"minimumFree":uint}]}"#
        } else if id == ADMISSION_READ {
            r#"{"job":str,"operation":str,"capture":uuid}"#
        } else if id == RECORD_PREPARE {
            r#"{"job":str,"operation":str,"capture":uuid,"reserveBytes":uint}"#
        } else if id == RECORD_PUBLISH {
            r#"{"job":str,"operation":str,"capture":uuid,"intent":object}"#
        } else if id == RECORD_READ {
            r#"{"job":str,"operation":str,"capture":uuid}"#
        } else {
            r#"{"job":str,"operation":str}"#
        },
        enabled: always,
        run,
        journal: false,
    })
    .chain(dispatch)
    .collect()
}
#[cfg(test)]
#[path = "save_operation_tests.rs"]
mod tests;

pub(super) fn pending_request(s: &Session) -> Option<Value> {
    s.agent_bridge.save_operation.as_ref().map(|o| json!({"job":o.request.job,"operation":o.request.operation}))
}
