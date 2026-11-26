//! Complete owner-generation hashing and fresh probing without a live writer.
//! Preparation is not publication: Apply still compares every original byte.
use super::{Batch, admission, check_edit_ticket_basis, media_guard::Check, parse, snapshot_jobs::Worker, token_ok};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
use web_time::Instant;

pub(super) const PREPARE_MS: u64 = 900_000;
pub(super) const PUBLICATION_MS: u64 = 240_000;
const LIMIT: usize = 4;
const START: &str = "agent.prepareSourceAdmission";
const STATUS: &str = "agent.sourceAdmissionPreparation";
const CANCEL: &str = "agent.cancelSourceAdmissionPreparation";
enum State {
    Running,
    Done(std::result::Result<Vec<Arc<admission::Prepared>>, String>),
    Consumed,
    Cancelled,
}
pub(super) struct Job {
    id: String,
    request: Value,
    token: String,
    count: usize,
    check: Arc<Check>,
    state: Arc<Mutex<State>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.check.stop.store(true, Ordering::Release);
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    job: String,
    batch: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: String,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(START, message)
}
pub(super) fn validate(batch: &Batch) -> Result<()> {
    if batch.authority != super::document_authority::Authority::MediaInputs
        || !batch.validate_at_publication
        || !batch.admission_preparation.as_ref().is_some_and(|id| token_ok(id) && *id != batch.token)
    {
        return Err(error("prepared admission needs one exact media-domain source group, publication validation and a distinct preparation ID"));
    }
    admission::validate_group(&batch.operations, batch.exact)
}
fn current(s: &Session, job: &Job) -> Result<()> {
    check_edit_ticket_basis(s, &job.token)?;
    job.check.check()?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == job.token).ok_or_else(|| error("admission ticket expired"))?;
    if ticket.preparation.as_ref() != Some(&job.id) {
        return Err(error("admission preparation no longer owns its exact native ticket"));
    }
    Ok(())
}
fn envelope(s: &Session, job: &Job) -> Result<Value> {
    current(s, job)?;
    let state = job.state.lock().unwrap_or_else(|e| e.into_inner());
    let status = match &*state {
        State::Running => "running",
        State::Done(Ok(_)) => "prepared",
        State::Done(Err(_)) => "failed",
        State::Consumed => "consumed",
        State::Cancelled => "cancelled",
    };
    let mut result = json!({"job":job.id,"kind":"native_source_admission_preparation","status":status,"token":job.token,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),
        "policy":{"version":1,"preparation_deadline_ms":PREPARE_MS,"publication_deadline_ms":PUBLICATION_MS},
        "progress":{"completed_checks":job.check.files.load(Ordering::Relaxed),"total_checks":job.count.saturating_mul(2),"bytes_read":job.check.bytes.load(Ordering::Relaxed).to_string()},
        "writeAuthority":false,"requiresFinalValidation":true});
    if let State::Done(Err(message)) = &*state {
        result["error"] = json!(message);
    }
    Ok(result)
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Start = parse(START, p)?;
    let batch: Batch = parse(START, &r.batch)?;
    super::validate_batch(&batch)?;
    super::project_plane::batch_scope(s, &batch)?;
    validate(&batch)?;
    if !s.worker_available
        || !super::operation_cancellation_available(s)
        || batch.admission_preparation.as_ref() != Some(&r.job)
        || !token_ok(&r.job)
        || p.to_string().len() > 4 * 1024 * 1024
    {
        return Err(error("prepared admission requires the installed writer/Stop transport and exact bounded batch"));
    }
    if s.agent_bridge.stopped_admissions.contains(&r.job) {
        return Err(error("admission preparation was cancelled before start; take a fresh read"));
    }
    if let Some(job) = s.agent_bridge.admission_jobs.iter().find(|j| j.id == r.job) {
        if job.request != r.batch {
            return Err(error("admission preparation ID already describes another exact batch"));
        }
        return envelope(s, job);
    }
    check_edit_ticket_basis(s, &batch.token)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == batch.token).ok_or_else(|| error("admission ticket expired"))?;
    if ticket.preparation.is_some() {
        return Err(error("native ticket already attempted preparation; no replacement or fallback"));
    }
    let inputs = batch.operations.iter().map(|op| admission::input(s, op)).collect::<Result<Vec<_>>>()?;
    if s.agent_bridge.admission_jobs.len() >= LIMIT {
        let i = s
            .agent_bridge
            .admission_jobs
            .iter()
            .position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running))
            .ok_or_else(|| error("admission preparation cache is busy"))?;
        s.agent_bridge.admission_jobs.remove(i);
    }
    let permit = Worker::acquire()?;
    let check = Arc::new(Check::for_admission_preparation(Instant::now(), s.agent_bridge.operations.preparation_stop(&batch.token)?));
    let state = Arc::new(Mutex::new(State::Running));
    let worker_check = check.clone();
    let worker_state = state.clone();
    let media = s.media.clone();
    let services = s.services.clone();
    // Attempt ownership survives cancellation, worker failure and eviction.
    let ticket = s.agent_bridge.tickets.iter_mut().find(|t| t.token == batch.token).ok_or_else(|| error("admission ticket disappeared"))?;
    ticket.preparation = Some(r.job.clone());
    std::thread::Builder::new()
        .name("poema-source-admission".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                inputs.into_iter().map(|input| admission::prepare(input, &media, services.as_ref(), &worker_check)).collect::<Result<Vec<_>>>()
            }))
            .unwrap_or_else(|_| Err(error("native admission preparation worker failed")))
            .map_err(|e| e.to_string());
            let mut state = worker_state.lock().unwrap_or_else(|e| e.into_inner());
            if worker_check.check().is_err() || matches!(*state, State::Cancelled) {
                *state = State::Cancelled;
            } else {
                *state = State::Done(result);
            }
        })
        .map_err(|_| error("native admission preparation worker could not start"))?;
    s.agent_bridge.admission_jobs.push_back(Job { id: r.job, request: r.batch, token: batch.token, count: batch.operations.len(), check, state });
    envelope(s, s.agent_bridge.admission_jobs.back().ok_or_else(|| error("admission preparation disappeared"))?)
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(STATUS, p)?;
    if !token_ok(&r.job) {
        return Err(error("exact admission preparation ID required"));
    }
    let job =
        s.agent_bridge.admission_jobs.iter().find(|j| j.id == r.job).ok_or_else(|| error("admission preparation expired or is unknown; no work restarted"))?;
    match envelope(s, job) {
        Ok(value) => Ok(value),
        Err(e) => {
            job.check.stop.store(true, Ordering::Release);
            *job.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
            Err(e)
        }
    }
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(CANCEL, p)?;
    if !token_ok(&r.job) {
        return Err(error("exact admission preparation ID required"));
    }
    if !s.agent_bridge.stopped_admissions.contains(&r.job) {
        s.agent_bridge.stopped_admissions.push_back(r.job.clone());
        while s.agent_bridge.stopped_admissions.len() > 64 {
            s.agent_bridge.stopped_admissions.pop_front();
        }
    }
    if let Some(job) = s.agent_bridge.admission_jobs.iter().find(|j| j.id == r.job) {
        job.check.stop.store(true, Ordering::Release);
        *job.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    Ok(json!({"job":r.job,"status":"cancelled","writeAuthority":false}))
}
pub(super) fn eligible(s: &Session, p: &Value) -> bool {
    let Ok(batch) = parse::<Batch>(START, p) else { return false };
    if validate(&batch).is_err() {
        return false;
    }
    s.agent_bridge.admission_jobs.iter().any(|j| {
        Some(&j.id) == batch.admission_preparation.as_ref()
            && j.request == *p
            && current(s, j).is_ok()
            && matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Done(Ok(_)))
    })
}
pub(super) fn consume(s: &Session, batch: &Batch, p: &Value) -> Result<Vec<Arc<admission::Prepared>>> {
    validate(batch)?;
    let job = s
        .agent_bridge
        .admission_jobs
        .iter()
        .find(|j| Some(&j.id) == batch.admission_preparation.as_ref())
        .ok_or_else(|| error("unknown admission preparation; no synchronous fallback"))?;
    current(s, job)?;
    if job.request != *p {
        return Err(error("admission preparation belongs to another exact batch"));
    }
    let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
    match std::mem::replace(&mut *state, State::Consumed) {
        State::Done(Ok(proofs)) => Ok(proofs),
        other => {
            *state = other;
            Err(error("admission preparation unfinished, failed, cancelled or consumed; no fallback"))
        }
    }
}
pub(super) fn commands() -> Vec<CommandSpec> {
    [
        (START, "Prepare Native Source Admission", r#"{"job":freshId,"batch":exactAdmissionBatch}"#, start as fn(&mut Session, &Value) -> Result<Value>),
        (STATUS, "Read Native Source Admission Preparation", r#"{"job":id}"#, status),
        (CANCEL, "Cancel Native Source Admission Preparation", r#"{"job":id}"#, cancel),
    ]
    .into_iter()
    .map(|(id, label, params, run)| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false })
    .collect()
}
