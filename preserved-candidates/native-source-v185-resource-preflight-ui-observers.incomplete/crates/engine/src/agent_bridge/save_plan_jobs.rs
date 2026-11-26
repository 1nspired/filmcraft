//! Bounded immutable Save plan reads on the original shared native verifier pool.
//! A completed result is delivered only against its exact live Root and saved/editor/preview context.
use super::{
    media_guard::Check,
    parse,
    project_save_plan::{self, Completed, Computation, Prepared},
    snapshot_jobs::Worker,
    token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Duration;
use web_time::Instant;
pub(super) const START: &str = "agent.startProjectSavePlan";
const LIMIT: usize = 4;
const DELIVERY: Duration = Duration::from_millis(250);
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(START, m)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    job: String,
    query: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: String,
}
enum State {
    Running,
    Done(std::result::Result<Completed, String>, Instant),
    Delivered(Completed, Instant),
    Consumed,
    Cancelled,
}
pub(super) struct Job {
    id: String,
    request: Value,
    prepared: Prepared,
    check: Arc<Check>,
    state: Arc<Mutex<State>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.check.stop.store(true, Ordering::Release);
    }
}
fn remember(s: &mut Session, id: String) {
    if !s.agent_bridge.stopped_save_plans.contains(&id) {
        s.agent_bridge.stopped_save_plans.push_back(id);
    }
    while s.agent_bridge.stopped_save_plans.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_save_plans.pop_front();
    }
}
fn envelope(j: &Job, status: &str) -> Value {
    json!({"status":status,"kind":"native_project_save_plan_read","version":1,"job":j.id,"token":j.prepared.token(),"deadlineMs":super::media_guard::OPERATION_DEADLINE_MS,"deliveryMs":250,"writeAuthority":false,"projectUnchanged":true,"historyUnchanged":true})
}
fn refuse(j: &Job, m: impl Into<String>) -> Result<Value> {
    j.check.stop.store(true, Ordering::Release);
    *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    Err(error(m))
}
fn status(s: &Session, j: &Job) -> Result<Value> {
    if matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Cancelled) {
        return Ok(envelope(j, "cancelled"));
    }
    if let Err(e) = j.prepared.live(s) {
        return refuse(j, e.to_string());
    }
    if let Err(e) = j.check.check() {
        return refuse(j, e.to_string());
    }
    let mut state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    match &*state {
        State::Running => Ok(envelope(j, "running")),
        State::Consumed => Ok(envelope(j, "cancelled")),
        State::Cancelled => Ok(envelope(j, "cancelled")),
        State::Done(Err(e), _) => {
            let mut out = envelope(j, "failed");
            out["error"] = json!(e);
            Ok(out)
        }
        State::Done(Ok(result), at) | State::Delivered(result, at) => {
            if at.elapsed() > DELIVERY {
                drop(state);
                return refuse(j, "Save plan result expired before delivery; take a fresh read, never restart this job");
            }
            if let Err(e) = j.check.check() {
                drop(state);
                return refuse(j, e.to_string());
            }
            // Only the description is cloned for read replies. The encoded
            // payload stays in the same original job across repeated delivery.
            let description = result.description().clone();
            if matches!(*state, State::Done(Ok(_), _)) {
                let prior = std::mem::replace(&mut *state, State::Consumed);
                if let State::Done(Ok(result), at) = prior {
                    *state = State::Delivered(result, at);
                } else {
                    *state = prior;
                    return Err(error("Original Save delivery state changed; no continuation minted"));
                }
            }
            let mut out = envelope(j, "delivered");
            out["result"] = description;
            Ok(out)
        }
    }
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    start_with(s, p, None, |prepared, check| prepared.compute_owned(&|| check.check()))
}
fn start_with(
    s: &mut Session,
    p: &Value,
    started: Option<Instant>,
    work: impl FnOnce(&Computation, &Check) -> Result<Completed> + Send + 'static,
) -> Result<Value> {
    let began = started.unwrap_or_else(Instant::now);
    let r: Request = parse(START, p)?;
    if !token_ok(&r.job) {
        return Err(error("fresh bounded save-plan job ID required"));
    }
    if let Some(j) = s.agent_bridge.save_plans.iter().find(|j| j.id == r.job) {
        if j.request != *p {
            return Err(error("Save plan job ID already describes another exact read"));
        }
        return status(s, j);
    }
    if s.agent_bridge.stopped_save_plans.contains(&r.job) {
        return Ok(json!({"job":r.job,"status":"cancelled","ticketRevoked":false,"writeAuthority":false}));
    }
    let prepared = project_save_plan::prepare(s, &r.query)?;
    if r.job == prepared.token() {
        return Err(error("Save plan job ID must differ from the unused Root read token"));
    }
    let old = if s.agent_bridge.save_plans.len() >= LIMIT {
        Some(
            s.agent_bridge
                .save_plans
                .iter()
                .position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running))
                .ok_or_else(|| error("save-plan cache is busy; no running read was evicted"))?,
        )
    } else {
        None
    };
    let permit = Worker::acquire()?;
    if let Some(index) = old
        && let Some(old) = s.agent_bridge.save_plans.remove(index)
    {
        remember(s, old.id.clone());
    }
    let check = Arc::new(Check::new(began));
    let state = Arc::new(Mutex::new(State::Running));
    let worker_prepared = prepared.computation();
    let worker_check = check.clone();
    let worker_state = state.clone();
    std::thread::Builder::new()
        .name("poema-save-plan-read".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_check.check()?;
                work(&worker_prepared, &worker_check)
            }))
            .unwrap_or_else(|_| Err(error("native save-plan worker failed; no edit was made")))
            .map_err(|e| e.to_string());
            let mut out = worker_state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*out, State::Cancelled) || worker_check.check().is_err() {
                *out = State::Cancelled;
            } else {
                *out = State::Done(result, Instant::now());
            }
        })
        .map_err(|_| error("native save-plan worker could not start"))?;
    s.agent_bridge.save_plans.push_back(Job { id: r.job, request: p.clone(), prepared, check, state });
    let job = s.agent_bridge.save_plans.back().ok_or_else(|| error("Save plan job disappeared"))?;
    Ok(envelope(job, "running"))
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Target = parse(START, p)?;
    if !token_ok(&q.job) {
        return Err(error("exact owned save-plan job required"));
    }
    let j = s.agent_bridge.save_plans.iter().find(|j| j.id == q.job).ok_or_else(|| error("unknown/expired Save plan job; never automatically restart it"))?;
    status(s, j)
}
pub(super) fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Target = parse(START, p)?;
    if !token_ok(&q.job) {
        return Err(error("exact save-plan job required"));
    }
    if let Some(j) = s.agent_bridge.save_plans.iter_mut().find(|j| j.id == q.job) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    remember(s, q.job.clone());
    Ok(json!({"job":q.job,"status":"cancelled","ticketRevoked":false,"writeAuthority":false}))
}
#[cfg(not(target_arch = "wasm32"))]
impl crate::project_save::operation::PlanningOrigin for Job {
    fn before_dispatch(&self, s: &Session) -> Result<()> {
        self.check.check()?;
        self.prepared.live(s)
    }
    fn io_current(&self) -> Result<()> {
        self.check.check()
    }
}
/// Trusted native composition, no JSON command or file grant. Remove and
/// tombstone the real delivered job before handing its actual bytes over.
/// The same job remains alive in the pending operation until that owner ends.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn begin_operation(s: &mut Session, job: &str, operation: &str, check: &impl Fn() -> Result<()>) -> Result<crate::project_save::operation::Key> {
    begin_operation_detailed(s, job, operation, check).map(|v| v.0)
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn begin_operation_detailed(
    s: &mut Session,
    job: &str,
    operation: &str,
    check: &impl Fn() -> Result<()>,
) -> Result<(crate::project_save::operation::Key, Value)> {
    check()?;
    s.require_owner()?;
    if !token_ok(job) || !token_ok(operation) || job == operation {
        return Err(error("Distinct exact planning job and fresh native operation ids required"));
    }
    let index = s
        .agent_bridge
        .save_plans
        .iter()
        .position(|j| j.id == job)
        .ok_or_else(|| error("Original Save job is unknown or already consumed; no reconstruction/restart"))?;
    let original = s.agent_bridge.save_plans.get(index).ok_or_else(|| error("Original Save job disappeared"))?;
    let delivered = status(s, original)?;
    if delivered["status"] != "delivered" {
        return Err(error("Original Save job has not delivered its encoded result"));
    }
    {
        let state = original.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*state,State::Delivered(result,_) if result.has_destination()) {
            return Err(error("Original Save still needs its native human destination; no worker paused"));
        }
    }
    // This exact original owner may have been transferred off the UI with a
    // completed media-watch result. Apply only that already completed result;
    // never wait, clear a running scan, or renew the encoded payload/deadlines.
    // A refreshed media graph invalidates the existing plan normally.
    if let Some(refreshed) = s.poll_media_scan() {
        refreshed?;
        check()?;
        let original = s.agent_bridge.save_plans.get(index).ok_or_else(|| error("Original Save job disappeared after media refresh"))?;
        if status(s, original)?["status"] != "delivered" {
            return Err(error("Original Save delivery changed after media refresh; no payload was consumed or restarted"));
        }
    }
    let original = s.agent_bridge.save_plans.remove(index).ok_or_else(|| error("Original Save job disappeared before consumption"))?;
    remember(s, original.id.clone());
    let (completed, at) = {
        let mut state = original.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *state, State::Consumed) {
            State::Delivered(result, at) => (result, at),
            _ => return Err(error("Original Save delivery unavailable; consumed id cannot restart")),
        }
    };
    let description = completed.description().clone();
    let original = Arc::new(original);
    let io = original.clone();
    let bounded = || {
        check()?;
        io.check.check()
    };
    let delivery = || {
        bounded()?;
        if at.elapsed() > DELIVERY {
            return Err(error("Original encoded Save delivery expired before native ownership transfer; no restart"));
        }
        Ok(())
    };
    let prepare = original.clone();
    let key = crate::project_save::operation::begin_delivered(s, operation, original, &delivery, &bounded, move |s| prepare.prepared.bind(s, completed))?;
    Ok((key, description))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: START,
            label: "Start Original Project Save Plan",
            menu: &[],
            shortcut: None,
            params: r#"{"job":str,"query":object}"#,
            enabled: always,
            run: start,
            journal: false,
        },
        CommandSpec {
            id: "agent.projectSavePlanStatus",
            label: "Read Project Save Plan Job",
            menu: &[],
            shortcut: None,
            params: r#"{"job":str}"#,
            enabled: always,
            run: poll,
            journal: false,
        },
        CommandSpec {
            id: "agent.cancelProjectSavePlan",
            label: "Cancel Project Save Plan Job",
            menu: &[],
            shortcut: None,
            params: r#"{"job":str}"#,
            enabled: always,
            run: cancel,
            journal: false,
        },
    ]
}
#[cfg(test)]
#[path = "save_plan_jobs_tests.rs"]
mod tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "save_plan_handoff_tests.rs"]
mod handoff_tests;
