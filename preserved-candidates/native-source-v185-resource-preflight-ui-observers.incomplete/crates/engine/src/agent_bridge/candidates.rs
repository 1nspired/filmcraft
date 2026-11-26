//! Private immutable condense candidates, not a second editor or generic
//! transaction API. Prepare/inspect never publish a project/history snapshot.
use std::sync::{Arc, Weak};
use std::time::Duration;
use web_time::Instant;

use filmcraft_project::{ItemId, Project};
use filmcraft_time::{Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Batch, Operation, Staged, check_edit_ticket, condense, condense_restore, context, inspection, parse, publish_batch, sequences, stage_batch, token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};

const PREPARE: &str = "agent.prepareCondense";
const COMMIT: &str = "agent.commitCondense";
const DISCARD: &str = "agent.discardCondense";
const RECORDS: usize = 8;
const PENDING: usize = 3;
const LIFETIME: Duration = Duration::from_secs(15 * 60);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    token: String,
    candidate: String,
    scope: sequences::Scope,
    label: String,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    token: String,
    candidate: String,
}

pub(super) struct Record {
    id: String,
    request: Value,
    operation: Operation,
    scope: sequences::Scope,
    label: String,
    project: Weak<Project>,
    path: Option<String>,
    revision: u64,
    sequence: ItemId,
    context: Value,
    started: Instant,
    stage: Option<Staged>,
    status: &'static str,
    receipt: Option<Value>,
    media: Arc<super::media_guard::Witnesses>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(PREPARE, message)
}
fn current(s: &Session, c: &Record) -> Result<()> {
    current_basis(s, c)?;
    super::validate_media(s, &c.media)
}
fn current_basis(s: &Session, c: &Record) -> Result<()> {
    super::idle(s)?;
    if c.status != "prepared"
        || c.stage.is_none()
        || c.started.elapsed() >= LIFETIME
        || c.revision != s.revision
        || c.path != s.path
        || s.state.active_sequence != Some(c.sequence)
        || !c.project.ptr_eq(&Arc::downgrade(&s.project))
        || c.context != context(s)?
    {
        return Err(error("candidate expired, was consumed/discarded, or its native document/editor basis changed"));
    }
    Ok(())
}
fn summary(c: &Record) -> Result<Value> {
    if c.status == "committed" {
        return Ok(json!({"status":"committed","candidate":c.id,"receipt":c.receipt}));
    }
    let stage = c.stage.as_ref().ok_or_else(|| error("candidate is unavailable; it was not reapplied"))?;
    let q = stage.project.sequence(c.sequence).ok_or(crate::EngineError::NoSequence)?;
    let (first, last) = filmcraft_export::frame_span(q.settings.frame_rate, TimeRange::new(Tick::ZERO, q.duration()));
    Ok(json!({"status":"prepared","candidate":c.id,"operation":c.operation.command,"scope":c.scope,"baseRevision":c.revision,"sequence":c.sequence.0,
        "durationTicks":q.duration().0,"durationFrames":last-first,"frameRate":q.settings.frame_rate,"sampleRate":q.settings.sample_rate,
        "result":stage.results.first(),"outputQualified":false,"execution":"not-applied"}))
}
fn sweep(s: &mut Session) {
    let mut expired = Vec::new();
    for c in &mut s.agent_bridge.candidates {
        if c.stage.is_some() && c.started.elapsed() >= LIFETIME {
            c.stage = None;
            c.status = "expired";
            expired.push(c.id.clone());
        }
    }
    for id in expired {
        inspection::discard_candidate(s, &id);
    }
}
fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Prepare = parse(PREPARE, p)?;
    if !token_ok(&r.token)
        || !token_ok(&r.candidate)
        || r.token == r.candidate
        || r.label.trim().is_empty()
        || r.label.len() > 256
        || ![condense::COMMAND, condense_restore::COMMAND, condense_restore::CHOOSE].contains(&r.operation.command.as_str())
        || p.to_string().len() > 72 * 1024
        || r.operation.params.get("projectId").and_then(Value::as_str) != Some(r.scope.project_id.as_str())
    {
        return Err(error(
            "need distinct bounded read/candidate IDs, explicit matching original scope, label and one private condense, saved-span inverse or saved-choice operation",
        ));
    }
    sweep(s);
    if let Some(c) = s.agent_bridge.candidates.iter().find(|c| c.id == r.candidate) {
        if c.request != *p {
            return Err(error("candidate ID already describes a different request"));
        }
        if c.status != "committed" {
            current(s, c)?;
        }
        return summary(c);
    }
    check_edit_ticket(s, &r.token)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &r.scope)? {
        return Err(error("candidate belongs to another native workspace"));
    }
    if s.agent_bridge.candidates.iter().filter(|c| c.stage.is_some()).count() >= PENDING {
        return Err(error("three prepared alternatives are retained; discard one before preparing another"));
    }
    if s.agent_bridge.candidates.len() >= RECORDS {
        let i = s.agent_bridge.candidates.iter().position(|c| c.stage.is_none()).ok_or_else(|| error("candidate receipt cache is busy"))?;
        s.agent_bridge.candidates.remove(i);
    }
    let batch = Batch {
        token: r.token,
        label: r.label.clone(),
        operations: vec![r.operation.clone()],
        exact: true,
        validate_at_publication: false,
        authority: super::document_authority::Authority::MediaInputs,
        admission_preparation: None,
    };
    let stage = stage_batch(s, &batch)?;
    let c = Record {
        id: r.candidate,
        request: p.clone(),
        operation: r.operation,
        scope: r.scope,
        label: r.label,
        project: Arc::downgrade(&s.project),
        path: s.path.clone(),
        revision: s.revision,
        sequence,
        context: context(s)?,
        started: Instant::now(),
        stage: Some(stage),
        status: "prepared",
        receipt: None,
        media: s.agent_bridge.tickets.iter().find(|t| t.token == batch.token).ok_or_else(|| error("candidate read expired"))?.media.clone(),
    };
    let response = summary(&c)?;
    s.agent_bridge.candidates.push_back(c);
    Ok(response)
}

pub(super) fn for_read(s: &Session, id: &str, scope: &sequences::Scope) -> Result<Arc<Project>> {
    let c = s.agent_bridge.candidates.iter().find(|c| c.id == id).ok_or_else(|| error("unknown/expired candidate"))?;
    current(s, c)?;
    if &c.scope != scope {
        return Err(error("candidate evidence belongs to another original workspace"));
    }
    Ok(c.stage.as_ref().ok_or_else(|| error("candidate is unavailable"))?.project.clone())
}
pub(super) struct ReadView {
    pub project: Arc<Project>,
    pub media: Arc<super::media_guard::Witnesses>,
    pub operation: String,
}
// Immutable inputs only. The evidence worker must verify both original-file
// witness sets before decoding and again before making bytes available.
pub(super) fn for_background_read(s: &Session, id: &str, scope: &sequences::Scope) -> Result<ReadView> {
    let c = s.agent_bridge.candidates.iter().find(|c| c.id == id).ok_or_else(|| error("unknown/expired candidate"))?;
    current_basis(s, c)?;
    if &c.scope != scope {
        return Err(error("candidate evidence belongs to another original workspace"));
    }
    Ok(ReadView {
        project: c.stage.as_ref().ok_or_else(|| error("candidate is unavailable"))?.project.clone(),
        media: c.media.clone(),
        operation: c.operation.command.clone(),
    })
}
pub(super) fn read_current(s: &Session, id: &str, project: &Weak<Project>) -> bool {
    s.agent_bridge
        .candidates
        .iter()
        .find(|c| c.id == id)
        .is_some_and(|c| current(s, c).is_ok() && c.stage.as_ref().is_some_and(|stage| project.ptr_eq(&Arc::downgrade(&stage.project))))
}
pub(super) fn operation(s: &Session, id: &str) -> Result<String> {
    let c = s.agent_bridge.candidates.iter().find(|c| c.id == id).ok_or_else(|| error("unknown/expired candidate"))?;
    current(s, c)?;
    Ok(c.operation.command.clone())
}
pub(super) fn validate_read(s: &Session, id: &str) -> Result<()> {
    let c = s.agent_bridge.candidates.iter().find(|c| c.id == id).ok_or_else(|| error("unknown/expired candidate"))?;
    current(s, c)
}
// Immutable read-job basis only. The worker must compare these original media
// witnesses before any result can be delivered; this grants no publication.
pub(super) fn read_basis(s: &Session, id: &str) -> Result<(Weak<Project>, Arc<super::media_guard::Witnesses>)> {
    let c = s.agent_bridge.candidates.iter().find(|c| c.id == id).ok_or_else(|| error("unknown/expired candidate"))?;
    current_basis(s, c)?;
    Ok((Arc::downgrade(&c.stage.as_ref().ok_or_else(|| error("candidate is unavailable"))?.project), c.media.clone()))
}

fn commit(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(COMMIT, p)?;
    if !token_ok(&r.token) || !token_ok(&r.candidate) || r.token == r.candidate {
        return Err(bad(COMMIT, "need an explicit fresh commit ticket and candidate"));
    }
    let ticket =
        s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| bad(COMMIT, "unknown/expired commit ticket; reconcile rather than replay"))?;
    // Historical receipt lookup is idempotent even after subsequent human edits.
    if let Some((_, result)) = &ticket.committed {
        return if result.get("candidate").and_then(Value::as_str) == Some(&r.candidate) {
            Ok(result.clone())
        } else {
            Err(bad(COMMIT, "ticket already committed a different operation"))
        };
    }
    check_edit_ticket(s, &r.token)?;
    sweep(s);
    let c =
        s.agent_bridge.candidates.iter().find(|c| c.id == r.candidate).ok_or_else(|| bad(COMMIT, "unknown/expired candidate; no replacement was rebuilt"))?;
    current(s, c)?;
    let stage = c.stage.as_ref().ok_or_else(|| bad(COMMIT, "candidate is not prepared"))?.clone();
    let batch = Batch {
        token: r.token,
        label: c.label.clone(),
        operations: vec![c.operation.clone()],
        exact: true,
        validate_at_publication: false,
        authority: super::document_authority::Authority::MediaInputs,
        admission_preparation: None,
    };
    let request = json!({"token":batch.token,"label":batch.label,"operations":batch.operations,"exact":true});
    // Publish the SAME already-reconciled candidate, including the identities
    // used during evidence reads. Never rerun the operation after inspection.
    let result = publish_batch(s, &batch, stage, &request, Some(&r.candidate))?;
    if let Some(c) = s.agent_bridge.candidates.iter_mut().find(|c| c.id == r.candidate) {
        c.stage = None;
        c.status = "committed";
        c.receipt = Some(result.clone());
    }
    inspection::discard_candidate(s, &r.candidate);
    Ok(result)
}
fn discard(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse(DISCARD, p)?;
    if !token_ok(&r.token) || !token_ok(&r.candidate) {
        return Err(bad(DISCARD, "need explicit prepared read ticket and candidate"));
    }
    let c = s.agent_bridge.candidates.iter_mut().find(|c| c.id == r.candidate).ok_or_else(|| bad(DISCARD, "unknown/expired candidate"))?;
    if c.request.get("token").and_then(Value::as_str) != Some(r.token.as_str()) {
        return Err(bad(DISCARD, "candidate was prepared by another ticket"));
    }
    if c.status == "committed" {
        return Err(bad(DISCARD, "candidate already committed; inspect its receipt/current native history"));
    }
    c.stage = None;
    c.status = "discarded";
    inspection::discard_candidate(s, &r.candidate);
    Ok(json!({"status":"discarded","candidate":r.candidate,"execution":"not-applied"}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: PREPARE,
            label: "Prepare Native Condense Candidate",
            menu: &[],
            shortcut: None,
            params: r#"{"token":readToken,"candidate":freshId,"scope":workspaceScope,"label":str,"operation":condenseOrSavedChoiceOperation}"#,
            enabled: always,
            run: prepare,
            journal: false,
        },
        CommandSpec {
            id: COMMIT,
            label: "Commit Prepared Condense Candidate",
            menu: &[],
            shortcut: None,
            params: r#"{"token":freshReadToken,"candidate":preparedId}"#,
            enabled: always,
            run: commit,
            journal: false,
        },
        CommandSpec {
            id: DISCARD,
            label: "Discard Native Condense Candidate",
            menu: &[],
            shortcut: None,
            params: r#"{"token":preparedReadToken,"candidate":preparedId}"#,
            enabled: always,
            run: discard,
            journal: false,
        },
    ]
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
mod tests;
