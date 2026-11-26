//! Bounded immutable font reads on the original shared native verifier pool.
//! A completed result is delivered only against its exact live Root and registry.
use super::{
    media_guard::Check,
    parse,
    project_font_resources::{self, Computed, Prepared},
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
pub(super) const START: &str = "agent.startFontResources";
const LIMIT: usize = 4;
const DELIVERY: Duration = Duration::from_millis(250);
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(START, m)
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    ReplacementPlan,
    OperationReadback,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::ReplacementPlan => "replacement_plan",
            Self::OperationReadback => "operation_readback",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    job: String,
    kind: Kind,
    query: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: String,
}
enum State {
    Running,
    Done(std::result::Result<Computed, String>, Instant),
    Delivered(Computed, Instant),
    Cancelled,
}
pub(super) struct Job {
    id: String,
    request: Value,
    kind: Kind,
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
    if !s.agent_bridge.stopped_font_resources.contains(&id) {
        s.agent_bridge.stopped_font_resources.push_back(id);
    }
    while s.agent_bridge.stopped_font_resources.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_font_resources.pop_front();
    }
}
fn envelope(j: &Job, status: &str) -> Value {
    json!({"status":status,"kind":"native_font_resource_read","version":1,"job":j.id,"token":j.prepared.token(),"requestKind":j.kind.name(),"deadlineMs":super::media_guard::OPERATION_DEADLINE_MS,"deliveryMs":250,"writeAuthority":false,"projectUnchanged":true,"historyUnchanged":true})
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
        State::Cancelled => Ok(envelope(j, "cancelled")),
        State::Done(Err(e), _) => {
            let mut out = envelope(j, "failed");
            out["error"] = json!(e);
            Ok(out)
        }
        State::Done(Ok(result), at) | State::Delivered(result, at) => {
            if at.elapsed() > DELIVERY || result.registry != filmcraft_text::fonts::registry_revision() {
                drop(state);
                return refuse(j, "font result expired or registry changed before delivery; take a fresh read, never restart this job");
            }
            if let Err(e) = j.check.check() {
                drop(state);
                return refuse(j, e.to_string());
            }
            let result = result.clone();
            let at = *at;
            let mut out = envelope(j, "delivered");
            out["result"] = result.value.clone();
            *state = State::Delivered(result, at);
            Ok(out)
        }
    }
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    start_with(s, p, None, |prepared, check| prepared.compute(&|| check.check()))
}
fn start_with(
    s: &mut Session,
    p: &Value,
    started: Option<Instant>,
    work: impl FnOnce(&Prepared, &Check) -> Result<Computed> + Send + 'static,
) -> Result<Value> {
    let began = started.unwrap_or_else(Instant::now);
    let r: Request = parse(START, p)?;
    if !token_ok(&r.job) {
        return Err(error("fresh bounded font-read job ID required"));
    }
    if let Some(j) = s.agent_bridge.font_resources.iter().find(|j| j.id == r.job) {
        if j.request != *p {
            return Err(error("font job ID already describes another exact read"));
        }
        return status(s, j);
    }
    if s.agent_bridge.stopped_font_resources.contains(&r.job) {
        return Ok(json!({"job":r.job,"status":"cancelled","ticketRevoked":false,"writeAuthority":false}));
    }
    let prepared = project_font_resources::prepare(s, r.kind.name(), &r.query)?;
    if r.job == prepared.token() {
        return Err(error("font job ID must differ from the unused Root read token"));
    }
    let old = if s.agent_bridge.font_resources.len() >= LIMIT {
        Some(
            s.agent_bridge
                .font_resources
                .iter()
                .position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running))
                .ok_or_else(|| error("font-read cache is busy; no running read was evicted"))?,
        )
    } else {
        None
    };
    let permit = Worker::acquire()?;
    if let Some(index) = old
        && let Some(old) = s.agent_bridge.font_resources.remove(index)
    {
        remember(s, old.id.clone());
    }
    let check = Arc::new(Check::new(began));
    let state = Arc::new(Mutex::new(State::Running));
    let worker_prepared = prepared.clone();
    let worker_check = check.clone();
    let worker_state = state.clone();
    std::thread::Builder::new()
        .name("poema-font-resource-read".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_check.check()?;
                work(&worker_prepared, &worker_check)
            }))
            .unwrap_or_else(|_| Err(error("native font-resource worker failed; no edit was made")))
            .map_err(|e| e.to_string());
            let mut out = worker_state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*out, State::Cancelled) || worker_check.check().is_err() {
                *out = State::Cancelled;
            } else {
                *out = State::Done(result, Instant::now());
            }
        })
        .map_err(|_| error("native font-resource worker could not start"))?;
    s.agent_bridge.font_resources.push_back(Job { id: r.job, request: p.clone(), kind: r.kind, prepared, check, state });
    let job = s.agent_bridge.font_resources.back().ok_or_else(|| error("font job disappeared"))?;
    Ok(envelope(job, "running"))
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Target = parse(START, p)?;
    if !token_ok(&q.job) {
        return Err(error("exact owned font-read job required"));
    }
    let j = s.agent_bridge.font_resources.iter().find(|j| j.id == q.job).ok_or_else(|| error("unknown/expired font job; never automatically restart it"))?;
    status(s, j)
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Target = parse(START, p)?;
    if !token_ok(&q.job) {
        return Err(error("exact font-read job required"));
    }
    if let Some(j) = s.agent_bridge.font_resources.iter_mut().find(|j| j.id == q.job) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    remember(s, q.job.clone());
    Ok(json!({"job":q.job,"status":"cancelled","ticketRevoked":false,"writeAuthority":false}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: START,
            label: "Start Whole Project Font Resource Read",
            menu: &[],
            shortcut: None,
            params: r#"{"job":str,"kind":"replacement_plan"|"operation_readback","query":object}"#,
            enabled: always,
            run: start,
            journal: false,
        },
        CommandSpec {
            id: "agent.fontResourcesStatus",
            label: "Read Font Resource Job",
            menu: &[],
            shortcut: None,
            params: r#"{"job":str}"#,
            enabled: always,
            run: poll,
            journal: false,
        },
        CommandSpec {
            id: "agent.cancelFontResources",
            label: "Cancel Font Resource Job",
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
#[path = "font_resource_jobs_tests.rs"]
mod tests;
