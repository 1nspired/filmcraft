//! Background original-file verification for a snapshot. Workers have no
//! Session, media cache, project writer or ticket authority. Explicit delivery
//! checks the live native basis; cancellation can revoke an unused ticket.
use super::{
    context, fresh_snapshot, idle,
    media_guard::{Check, Plan, Witnesses},
    parse, token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};
use filmcraft_project::{ItemId, Project};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicUsize, Ordering},
};
use web_time::Instant;

const LIMIT: usize = 4;
static WORKERS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(super) static TEST_SERIAL: Mutex<()> = Mutex::new(());
#[cfg(test)]
pub(super) fn worker_count() -> usize {
    WORKERS.load(Ordering::SeqCst)
}
#[cfg(test)]
#[path = "snapshot_jobs_tests.rs"]
mod tests;
enum State {
    Running,
    Done(std::result::Result<Arc<Witnesses>, String>),
    Delivered,
    Cancelled,
}
pub(super) struct Job {
    token: String,
    revision: u64,
    sequence: Option<ItemId>,
    library: Option<super::library_plane::Request>,
    production: Option<super::multicam_production::Request>,
    project: Weak<Project>,
    path: Option<String>,
    context: Value,
    plan: Arc<Plan>,
    check: Arc<Check>,
    state: Arc<Mutex<State>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.check.stop.store(true, Ordering::Release);
    }
}
pub(super) struct Worker;
impl Worker {
    pub(super) fn acquire() -> Result<Self> {
        WORKERS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < 2).then_some(n + 1))
            .map(|_| Self)
            .map_err(|_| bad("agent.snapshot", "two native file-verification workers are still running; wait before a fresh read"))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    job: String,
}

pub(super) fn has_token(job: &Job, token: &str) -> bool {
    job.token == token
}

pub(super) fn start_scoped(s: &mut Session, token: String, scope: Option<&super::media_scope::Request>) -> Result<Value> {
    start_selected(s, token, scope, Instant::now(), |plan, check| plan.capture(check))
}
#[cfg(test)]
fn start_with(s: &mut Session, token: String, started: Instant, work: impl FnOnce(&Plan, &Check) -> Result<Arc<Witnesses>> + Send + 'static) -> Result<Value> {
    start_selected(s, token, None, started, work)
}
fn start_selected(
    s: &mut Session,
    token: String,
    scope: Option<&super::media_scope::Request>,
    started: Instant,
    work: impl FnOnce(&Plan, &Check) -> Result<Arc<Witnesses>> + Send + 'static,
) -> Result<Value> {
    fresh_snapshot(s, &token)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(|e| bad("agent.snapshot", e))?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let plan = Arc::new(match scope {
        Some(scope) => Plan::scoped(&s.project, &s.media, super::media_scope::resolve(&s.project, sequence, s.state.source_item, scope)?)?,
        None => Plan::prepare(&s.project, &s.media)?,
    });
    start_plan(s, token, Some(sequence), None, None, plan, started, work)
}
pub(super) fn start_library(s: &mut Session, request: super::library_plane::Request, items: Vec<u64>) -> Result<Value> {
    let plan = Arc::new(Plan::scoped(&s.project, &s.media, items)?);
    start_plan(s, request.token.clone(), s.state.active_sequence, Some(request), None, plan, Instant::now(), |plan, check| plan.capture(check))
}
pub(super) fn start_production(s: &mut Session, request: super::multicam_production::Request, items: Vec<u64>) -> Result<Value> {
    let plan = Arc::new(Plan::scoped(&s.project, &s.media, items)?);
    start_plan(s, request.token.clone(), s.state.active_sequence, None, Some(request), plan, Instant::now(), |plan, check| plan.capture(check))
}
fn start_plan(
    s: &mut Session,
    token: String,
    sequence: Option<ItemId>,
    library: Option<super::library_plane::Request>,
    production: Option<super::multicam_production::Request>,
    plan: Arc<Plan>,
    started: Instant,
    work: impl FnOnce(&Plan, &Check) -> Result<Arc<Witnesses>> + Send + 'static,
) -> Result<Value> {
    fresh_snapshot(s, &token)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(|e| bad("agent.snapshot", e))?;
    if let Some(j) = s.agent_bridge.snapshot_jobs.iter().find(|j| j.token == token) {
        if j.plan.as_ref() != plan.as_ref() || j.library != library || j.production != production {
            return Err(bad("agent.snapshot", "this background token already describes another source scope"));
        }
        return status(s, j);
    }
    let context = context(s)?;
    if s.agent_bridge.snapshot_jobs.len() >= LIMIT {
        let old = s.agent_bridge.snapshot_jobs.iter().position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running));
        let i = old.ok_or_else(|| bad("agent.snapshot", "native verification cache is busy; no active read was evicted"))?;
        s.agent_bridge.snapshot_jobs.remove(i);
    }
    let permit = Worker::acquire()?;
    let check = Arc::new(Check::for_snapshot(started, plan.source_bytes_hint(&s.project)));
    let state = Arc::new(Mutex::new(State::Running));
    let job = Job {
        token,
        revision: s.revision,
        sequence,
        library,
        production,
        project: Arc::downgrade(&s.project),
        path: s.path.clone(),
        context,
        plan: plan.clone(),
        check: check.clone(),
        state: state.clone(),
    };
    std::thread::Builder::new()
        .name("poema-file-verification".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&plan, &check)))
                .unwrap_or_else(|_| Err(bad("agent.snapshot", "native file-verification worker failed; no edit was made")))
                .map_err(|e| e.to_string());
            let mut out = state.lock().unwrap_or_else(|e| e.into_inner());
            if check.check().is_err() {
                *out = State::Cancelled;
            } else {
                *out = State::Done(result);
            }
        })
        .map_err(|_| bad("agent.snapshot", "native file-verification worker could not start"))?;
    s.agent_bridge.snapshot_jobs.push_back(job);
    let j = s.agent_bridge.snapshot_jobs.back().ok_or_else(|| bad("agent.snapshot", "native verification job missing"))?;
    // Starting never issues a ticket, even when the worker finishes at once.
    Ok(envelope(j, "running"))
}
fn current(s: &Session, j: &Job) -> Result<()> {
    if s.revision != j.revision
        || s.path != j.path
        || s.state.active_sequence != j.sequence
        || !j.project.ptr_eq(&Arc::downgrade(&s.project))
        || context(s)? != j.context
        || j.plan.rebuild(&s.project, &s.media)? != *j.plan
    {
        return Err(bad("agent.snapshot", "native document, editor context or authorized paths changed during verification; take a fresh read"));
    }
    idle(s)
}
fn envelope(j: &Job, status: &str) -> Value {
    json!({"job":j.token,"status":status,"kind":"native_snapshot_verification",
        "verification_policy":{"version":1,"deadline_ms":j.check.deadline_ms(),"publication_deadline_ms":super::media_guard::OPERATION_DEADLINE_MS},"progress":{
        "completed_checks":j.check.files.load(Ordering::Relaxed),"total_checks":j.plan.total(),"bytes_read":j.check.bytes.load(Ordering::Relaxed).to_string()}})
}
fn status(s: &Session, j: &Job) -> Result<Value> {
    if let Err(e) = current(s, j) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
        return Err(e);
    }
    if j.check.check().is_err() {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    let state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(match &*state {
        State::Running => envelope(j, "running"),
        State::Cancelled => envelope(j, "cancelled"),
        State::Delivered => envelope(j, "delivered"),
        State::Done(Ok(_)) => envelope(j, "ready"),
        State::Done(Err(e)) => {
            let mut out = envelope(j, "failed");
            out["error"] = json!(e);
            out
        }
    })
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse("agent.snapshotStatus", p)?;
    if !token_ok(&r.job) {
        return Err(bad("agent.snapshotStatus", "valid owned job ID required"));
    }
    let j = s
        .agent_bridge
        .snapshot_jobs
        .iter()
        .find(|j| j.token == r.job)
        .ok_or_else(|| bad("agent.snapshotStatus", "unknown/expired verification job; never restart the old read"))?;
    let out = status(s, j)?;
    if out["status"] != "ready" {
        return Ok(out);
    }
    let media = match &*j.state.lock().unwrap_or_else(|e| e.into_inner()) {
        State::Done(Ok(media)) => media.clone(),
        _ => return Err(bad("agent.snapshotStatus", "native verification was interrupted")),
    };
    j.check.check()?;
    let read_deadline_ms = j.check.deadline_ms();
    let library = j.library.clone();
    let production = j.production.clone();
    let mut out = out;
    let mut doc = match (library, production) {
        (Some(request), None) => super::library_plane::deliver(s, request, media)?,
        (None, Some(request)) => super::multicam_production::deliver(s, request, media)?,
        (None, None) => super::snapshot_with_media(s, r.job.clone(), media)?,
        _ => return Err(bad("agent.snapshotStatus", "conflicting native source purposes")),
    };
    // Confirm this exact job's fixed read policy. An overstated size hint can
    // extend this read without changing the actual witnessed publication set.
    doc["capabilities"]["backgroundSnapshotDeadlineMs"] = json!(read_deadline_ms);
    let j =
        s.agent_bridge.snapshot_jobs.iter_mut().find(|j| j.token == r.job).ok_or_else(|| bad("agent.snapshotStatus", "verification delivery lost its job"))?;
    *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Delivered;
    out["status"] = json!("complete");
    out["result"] = doc;
    Ok(out)
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Request = parse("agent.cancelSnapshot", p)?;
    if !token_ok(&r.job) {
        return Err(bad("agent.cancelSnapshot", "valid owned job ID required"));
    }
    if let Some(j) = s.agent_bridge.snapshot_jobs.iter_mut().find(|j| j.token == r.job) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    let previous = s.agent_bridge.tickets.len();
    s.agent_bridge.tickets.retain(|t| t.token != r.job || t.committed.is_some());
    let revoked = s.agent_bridge.tickets.len() != previous;
    if !s.agent_bridge.stopped_snapshots.contains(&r.job) {
        s.agent_bridge.stopped_snapshots.push_back(r.job.clone());
    }
    while s.agent_bridge.stopped_snapshots.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_snapshots.pop_front();
    }
    Ok(json!({"job":r.job,"status":"cancelled","ticketRevoked":revoked}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "agent.snapshotStatus",
            label: "Read Background Snapshot Verification",
            menu: &[],
            shortcut: None,
            params: r#"{"job":snapshotToken}"#,
            enabled: always,
            run: poll,
            journal: false,
        },
        CommandSpec {
            id: "agent.cancelSnapshot",
            label: "Cancel Background Snapshot Verification",
            menu: &[],
            shortcut: None,
            params: r#"{"job":snapshotToken}"#,
            enabled: always,
            run: cancel,
            journal: false,
        },
    ]
}
