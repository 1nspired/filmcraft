//! Bounded post-perception verification, never a ticket or publication authority.
//! Complete original-file witnesses are compared on the shared verifier workers.
use super::{
    ValidateRead,
    media_guard::{self, Check, Plan, Witnesses},
    parse,
    snapshot_jobs::Worker,
    token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};
use filmcraft_project::{ItemId, Project};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, Weak, atomic::Ordering};
use std::time::Duration;
use web_time::Instant;
const COMMAND: &str = "agent.validateSnapshot";
const LIMIT: usize = 4;
// A finished worker cannot become a long-lived proof while the caller is absent.
// Late delivery refuses; no verification is automatically repeated.
const DELIVERY: Duration = Duration::from_millis(250);
enum State {
    Running,
    Done(std::result::Result<(), String>, Instant),
    Cancelled,
    Delivered(Instant),
    Consumed,
}
pub(super) struct Job {
    id: String,
    request: Value,
    token: String,
    revision: u64,
    sequence: Option<ItemId>,
    library: Option<crate::project_workspace::ProjectScope>,
    path: Option<String>,
    project: Weak<Project>,
    candidate: Option<(String, Weak<Project>)>,
    inspection: Option<String>,
    apply: Option<Value>,
    plan: Arc<Plan>,
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
struct Target {
    job: String,
}
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, m)
}
pub(super) fn start(s: &mut Session, request: &Value, r: &ValidateRead) -> Result<Value> {
    start_with(s, request, r, |plan, expected, check| plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), check))
}
fn start_with(
    s: &mut Session,
    request: &Value,
    r: &ValidateRead,
    work: impl FnOnce(&Plan, &[Arc<Witnesses>], &Check) -> Result<()> + Send + 'static,
) -> Result<Value> {
    start_for(s, request, r, None, None, work)
}
// Root rechecks share the existing bounded verifier. They cannot ask for
// sequence candidates, Program inspections or Apply preparation.
pub(super) fn start_library(s: &mut Session, request: &Value, r: &ValidateRead, scope: crate::project_workspace::ProjectScope) -> Result<Value> {
    if r.candidate.is_some() {
        return Err(error("source-library validation cannot borrow sequence candidates"));
    }
    start_for(s, request, r, None, Some(scope), |plan, expected, check| plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), check))
}
fn read_basis<'a>(s: &'a Session, token: &str, library: Option<&crate::project_workspace::ProjectScope>) -> Result<&'a super::Ticket> {
    if let Some(scope) = library {
        super::library_plane::validate_ticket(s, token, scope, true)?;
        super::validate_read_basis_any(s, token)
    } else {
        super::validate_read_basis(s, token)
    }
}
fn start_for(
    s: &mut Session,
    request: &Value,
    r: &ValidateRead,
    apply: Option<Value>,
    library: Option<crate::project_workspace::ProjectScope>,
    work: impl FnOnce(&Plan, &[Arc<Witnesses>], &Check) -> Result<()> + Send + 'static,
) -> Result<Value> {
    let id = r.job.as_ref().filter(|id| token_ok(id)).ok_or_else(|| error("fresh verification job ID required"))?;
    if s.agent_bridge.stopped_read_jobs.contains(id) {
        return Ok(
            json!({"job":id,"status":"cancelled","kind":if apply.is_some(){"native_apply_preparation"}else{"native_read_validation"},"progress":{"completed_checks":0,"total_checks":0,"bytes_read":"0"}}),
        );
    }
    if let Some(j) = s.agent_bridge.read_jobs.iter().find(|j| j.id == *id) {
        if j.request != *request {
            return Err(error("verification ID already describes another read"));
        }
        return status(s, j, false);
    }
    let t = read_basis(s, &r.token, library.as_ref())?;
    let mut expected = vec![t.media.clone()];
    let candidate = if let Some(id) = &r.candidate {
        let (project, media) = super::candidates::read_basis(s, id)?;
        expected.push(media);
        Some((id.clone(), project))
    } else {
        None
    };
    let mut source_audio_proofs = Vec::new();
    if let Some(id) = &r.inspection {
        expected.push(super::inspection::read_basis(s, id, &r.token, r.candidate.as_deref())?);
        source_audio_proofs = super::inspection::admission_basis(s, id, &r.token, r.candidate.as_deref())?;
    }
    for witness in &expected {
        media_guard::admissible(witness)?;
    }
    let project = t.project.clone();
    let revision = t.revision;
    let sequence = if library.is_some() { t.sequence } else { Some(t.sequence_id()?) };
    let path = t.path.clone();
    let plan = Arc::new(Plan::for_witnesses(&s.project, &s.media, &expected)?);
    if s.agent_bridge.read_jobs.len() >= LIMIT {
        let i = s
            .agent_bridge
            .read_jobs
            .iter()
            .position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running))
            .ok_or_else(|| error("native read-verification cache is busy"))?;
        s.agent_bridge.read_jobs.remove(i);
    }
    let permit = Worker::acquire()?;
    let bytes = source_audio_proofs.iter().fold(expected.iter().fold(0u64, |bytes, witness| bytes.saturating_add(witness.source_bytes())), |bytes, witness| {
        bytes.saturating_add(witness.source_bytes())
    });
    let check = Arc::new(Check::for_sources(Instant::now(), Arc::new(std::sync::atomic::AtomicBool::new(false)), bytes));
    let state = Arc::new(Mutex::new(State::Running));
    let worker_plan = plan.clone();
    let worker_check = check.clone();
    let out = state.clone();
    std::thread::Builder::new()
        .name("poema-read-validation".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(&worker_plan, &expected, &worker_check)?;
                for proof in &source_audio_proofs {
                    media_guard::validate_admission(proof, &worker_check)?;
                }
                Ok(())
            }))
            .unwrap_or_else(|_| Err(error("native read-verification worker failed")))
            .map_err(|e| e.to_string());
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            if worker_check.check().is_err() {
                *out = State::Cancelled;
            } else {
                *out = State::Done(result, Instant::now());
            }
        })
        .map_err(|_| error("native read-verification worker could not start"))?;
    s.agent_bridge.read_jobs.push_back(Job {
        id: id.clone(),
        request: request.clone(),
        token: r.token.clone(),
        revision,
        sequence,
        library,
        path,
        project,
        candidate,
        inspection: r.inspection.clone(),
        apply,
        plan,
        check,
        state,
    });
    let j = s.agent_bridge.read_jobs.back().ok_or_else(|| error("verification job missing"))?;
    Ok(envelope(j, "running"))
}
fn current(s: &Session, j: &Job) -> Result<()> {
    if j.apply.is_some() {
        super::check_edit_ticket_basis(s, &j.token)?;
    }
    let t = read_basis(s, &j.token, j.library.as_ref())?;
    if t.revision != j.revision
        || t.sequence != j.sequence
        || t.path != j.path
        || !t.project.ptr_eq(&j.project)
        || j.plan.rebuild(&s.project, &s.media)? != *j.plan
    {
        return Err(error("native verification document or authorized paths changed"));
    }
    if let Some((id, project)) = &j.candidate {
        let (now, _) = super::candidates::read_basis(s, id)?;
        if !now.ptr_eq(project) {
            return Err(error("candidate basis changed during read verification"));
        }
    }
    if let Some(id) = &j.inspection {
        super::inspection::read_basis(s, id, &j.token, j.candidate.as_ref().map(|c| c.0.as_str()))?;
    }
    Ok(())
}
fn envelope(j: &Job, status: &str) -> Value {
    let mut result = json!({"job":j.id,"status":status,"kind":if j.apply.is_some(){"native_apply_preparation"}else{"native_read_validation"},"progress":{
 "completed_checks":j.check.files.load(Ordering::Relaxed),"total_checks":j.plan.validation_total(),"bytes_read":j.check.bytes.load(Ordering::Relaxed).to_string()}});
    if let Some(policy) = j.check.policy() {
        result["verification_policy"] = policy;
    }
    result
}
fn status(s: &Session, j: &Job, deliver: bool) -> Result<Value> {
    if let Err(e) = current(s, j) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
        return Err(e);
    }
    let mut state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    if j.check.check().is_err() {
        *state = State::Cancelled;
    }
    match &*state {
        State::Running => Ok(envelope(j, "running")),
        State::Cancelled => Ok(envelope(j, "cancelled")),
        State::Delivered(_) => Ok(envelope(j, "delivered")),
        State::Consumed => Ok(envelope(j, "consumed")),
        State::Done(Err(message), _) => {
            let mut out = envelope(j, "failed");
            out["error"] = json!(message);
            Ok(out)
        }
        State::Done(Ok(()), finished) => {
            if finished.elapsed() > DELIVERY {
                *state = State::Cancelled;
                return Ok(envelope(j, "expired"));
            }
            if !deliver {
                return Ok(envelope(j, "ready"));
            }
            let mut result = json!({"status":"current","token":j.token,"revision":j.revision,"sequence":j.sequence.map(|id|id.0)});
            if j.apply.is_some() {
                result = prepared_result(&j.token, j.revision, j.sequence.ok_or(crate::EngineError::NoSequence)?, false);
            }
            if let Some(scope) = &j.library {
                result["kind"] = json!("native_source_library_read");
                result["scope"] = json!(scope);
                result["writeAuthority"] = json!(false);
            }
            if let Some((id, _)) = &j.candidate {
                result["candidate"] = json!(id);
                result["view"] = json!("condense_candidate");
            }
            if let Some(id) = &j.inspection {
                result["inspection"] = json!(id);
            }
            let mut out = envelope(j, "complete");
            out["result"] = result;
            *state = State::Delivered(*finished);
            Ok(out)
        }
    }
}

// One bounded native qualification delivery, not host relabeling of an
// unverified document. Canonical rows come from this exact current Session.
pub(super) fn qualified_document(s: &mut Session, token: &str, id: &str) -> Result<Value> {
    let index = s.agent_bridge.read_jobs.iter().position(|j| j.id == id && j.token == token).ok_or_else(|| error("unknown native document qualification"))?;
    let j = s.agent_bridge.read_jobs.get(index).ok_or_else(|| error("native qualification disappeared"))?;
    current(s, j)?;
    super::check_edit_ticket_basis(s, token)?;
    j.check.check()?;
    if j.apply.is_some() || j.candidate.is_some() || j.inspection.is_some() {
        return Err(error("document qualification requires a dedicated original-file read"));
    }
    let state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(&*state,State::Delivered(finished) if finished.elapsed()<=DELIVERY) {
        return Err(error("native qualification was not delivered, expired or already consumed"));
    }
    let t = super::validate_read_basis(s, token)?;
    let doc = super::document_view(s, token, &t.media, true)?;
    drop(state);
    *s.agent_bridge.read_jobs.get(index).ok_or_else(|| error("native qualification disappeared"))?.state.lock().unwrap_or_else(|e| e.into_inner()) =
        State::Consumed;
    Ok(doc)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareApply {
    job: String,
    batch: Value,
}
fn prepared_result(token: &str, revision: u64, sequence: ItemId, historical: bool) -> Value {
    json!({"status":"prepared","token":token,"revision":revision,"sequence":sequence.0,
        "historical":historical,"writeAuthority":false,"execution":"not-applied","requiresFinalValidation":true})
}
fn prepare_apply(s: &mut Session, p: &Value) -> Result<Value> {
    prepare_apply_with(s, p, |plan, expected, check| plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), check))
}
fn prepare_apply_with(s: &mut Session, p: &Value, work: impl FnOnce(&Plan, &[Arc<Witnesses>], &Check) -> Result<()> + Send + 'static) -> Result<Value> {
    let r: PrepareApply = parse("agent.prepareApply", p)?;
    let batch: super::Batch = parse("agent.prepareApply", &r.batch)?;
    if batch.validate_at_publication {
        return Err(error("publication-validated Apply cannot also start a preparation job"));
    }
    super::validate_batch(&batch)?;
    if batch.authority == super::document_authority::Authority::DocumentState {
        return Err(error("authored document publication does not consume a media preparation receipt"));
    }
    if !token_ok(&r.job) || r.job == batch.token {
        return Err(error("distinct fresh Apply preparation ID required"));
    }
    for op in &batch.operations {
        super::validate(op)?;
    }
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == batch.token).ok_or_else(|| error("unknown/expired Apply read"))?;
    if let Some((request, _)) = &ticket.committed {
        if *request != r.batch {
            return Err(error("ticket already committed a different request"));
        }
        return Ok(json!({"job":r.job,"status":"complete","kind":"native_apply_preparation",
            "progress":{"completed_checks":0,"total_checks":0,"bytes_read":"0"},
            "result":prepared_result(&ticket.token,ticket.revision,ticket.sequence_id()?,true)}));
    }
    super::check_edit_ticket_basis(s, &batch.token)?;
    if let Some(id) = &ticket.preparation
        && (*id != r.job || !s.agent_bridge.read_jobs.iter().any(|j| j.id == *id))
    {
        return Err(error("Apply preparation already attempted or expired; read again, never restart"));
    }
    // Retain this requirement on the ticket even if the bounded job is later
    // canceled, fails to start or is evicted. Delayed Apply cannot fall back.
    let ticket = s.agent_bridge.tickets.iter_mut().find(|t| t.token == batch.token).ok_or_else(|| error("Apply read disappeared"))?;
    ticket.preparation = Some(r.job.clone());
    let read = ValidateRead { token: batch.token, candidate: None, job: Some(r.job), inspection: None };
    start_for(s, p, &read, Some(r.batch), None, work)
}
pub(super) fn consume_apply(s: &Session, request: &Value, token: &str) -> Result<bool> {
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("unknown Apply ticket"))?;
    let Some(id) = &ticket.preparation else {
        return Ok(false);
    };
    let j = s.agent_bridge.read_jobs.iter().find(|j| j.id == *id).ok_or_else(|| error("Apply preparation evicted/unknown; no synchronous fallback"))?;
    if j.apply.as_ref() != Some(request) || j.token != token {
        return Err(error("Apply preparation belongs to another exact batch"));
    }
    current(s, j)?;
    j.check.check()?;
    let mut state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    match &*state {
        State::Delivered(finished) if finished.elapsed() <= DELIVERY => {
            *state = State::Consumed;
            Ok(true)
        }
        _ => Err(error("Apply preparation incomplete, consumed, expired or stopped; read again, no fallback")),
    }
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(COMMAND, p)?;
    if !token_ok(&r.job) {
        return Err(error("explicit verification job required"));
    }
    let j = s.agent_bridge.read_jobs.iter().find(|j| j.id == r.job).ok_or_else(|| error("unknown/expired read-verification job; never restart it"))?;
    status(s, j, true)
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(COMMAND, p)?;
    if !token_ok(&r.job) {
        return Err(error("explicit verification job required"));
    }
    if let Some(j) = s.agent_bridge.read_jobs.iter_mut().find(|j| j.id == r.job) {
        j.check.stop.store(true, Ordering::Release);
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    if !s.agent_bridge.stopped_read_jobs.contains(&r.job) {
        s.agent_bridge.stopped_read_jobs.push_back(r.job.clone());
    }
    while s.agent_bridge.stopped_read_jobs.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_read_jobs.pop_front();
    }
    Ok(json!({"job":r.job,"status":"cancelled","ticketRevoked":false}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "agent.prepareApply",
            label: "Prepare Guarded Agent Batch",
            menu: &[],
            shortcut: None,
            params: r#"{"job":freshJob,"batch":{"token":readToken,"label":str,"operations":[operation],"exact":bool?}}"#,
            enabled: always,
            run: prepare_apply,
            journal: false,
        },
        CommandSpec {
            id: "agent.validationStatus",
            label: "Read Verification Status",
            menu: &[],
            shortcut: None,
            params: r#"{"job":verificationJob}"#,
            enabled: always,
            run: poll,
            journal: false,
        },
        CommandSpec {
            id: "agent.cancelValidation",
            label: "Cancel Read Verification",
            menu: &[],
            shortcut: None,
            params: r#"{"job":verificationJob}"#,
            enabled: always,
            run: cancel,
            journal: false,
        },
    ]
}
#[cfg(test)]
#[path = "read_jobs_tests.rs"]
mod tests;
