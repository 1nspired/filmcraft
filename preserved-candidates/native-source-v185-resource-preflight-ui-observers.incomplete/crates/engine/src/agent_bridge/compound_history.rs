//! Original immutable compound history services, shared by finite native families.
//! Descriptive evidence only: no live owner, source witness or edit executor.
use super::{editor_setting, history_checkpoint, media_guard, parse, project_audit, project_plane};
use crate::{
    EngineError, Result,
    commands::bad,
    trim::command_settlement::{self, historical},
};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
const LIMIT: usize = 4 * 1024 * 1024;
const POOL_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Copy)]
pub(super) enum Purpose {
    EditorControl,
    SequenceView,
}
impl Purpose {
    fn kind(self) -> &'static str {
        match self {
            Self::EditorControl => "native_editor_control_history_proof",
            Self::SequenceView => "native_sequence_view_history_proof",
        }
    }
    fn command(self) -> &'static str {
        match self {
            Self::EditorControl => "agent.configureEditorControl",
            Self::SequenceView => "agent.navigateSequenceView",
        }
    }
}
fn error(message: impl Into<String>) -> EngineError {
    bad("agent.configureEditorControl", message)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Opaque {
    pub(super) kind: String,
    pub(super) version: u8,
    pub(super) sha256: String,
    pub(super) bytes: usize,
    pub(super) data: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryProof {
    version: u8,
    command: String,
    snapshots: Vec<String>,
    evidence: String,
}
/// Strong immutable evidence roots, on the original Ticket only. No live owner.
pub(super) struct Retained {
    pub(super) evidence: historical::Evidence,
    pub(super) projects: Vec<Arc<Project>>,
    pub(super) original: Option<Box<Retained>>,
}
impl Retained {
    pub(super) fn capture_settled(prepared: &command_settlement::Prepared) -> Result<Self> {
        Self::from_capture(prepared.capture_history()?)
    }
    pub(super) fn capture_preserved(prepared: &command_settlement::Prepared, command: &str) -> Result<Self> {
        Self::from_capture(prepared.capture_preserved_history(command)?)
    }
    fn from_capture(capture: historical::Capture) -> Result<Self> {
        Ok(Self { evidence: capture.evidence().clone(), projects: capture.projects().to_vec(), original: None })
    }
    pub(super) fn projects(&self) -> &[Arc<Project>] {
        &self.projects
    }
    pub(super) fn mapping(&self, pool: &[Arc<Project>]) -> Result<Vec<usize>> {
        self.projects
            .iter()
            .map(|p| pool.iter().position(|q| Arc::ptr_eq(p, q)).ok_or_else(|| error("compound receipt lost an exact immutable evidence snapshot")))
            .collect()
    }
    pub(super) fn validate(&self, command: &str) -> Result<historical::Historical> {
        self.evidence.validate(&self.projects, command)
    }
    fn hashes(&self, check: Option<&media_guard::Check>) -> Result<Vec<String>> {
        let mut budget = POOL_BYTES;
        self.projects
            .iter()
            .map(|p| {
                if let Some(check) = check {
                    check.check()?;
                }
                let raw = project_audit::bytes(p)?;
                budget = budget.checked_sub(raw.len()).ok_or_else(|| error("complete compound evidence snapshots exceed their32MiB budget"))?;
                // Use the same complete organization and exact native project digest.
                project_plane::project_basis(p)
            })
            .collect()
    }
    pub(super) fn proof_for(&self, command: &str, purpose: Purpose, check: Option<&media_guard::Check>) -> Result<Value> {
        let inner = HistoryProof {
            version: 1,
            command: command.into(),
            snapshots: self.hashes(check)?,
            evidence: String::from_utf8(self.evidence.encode()?).map_err(|e| error(e.to_string()))?,
        };
        let raw = history_checkpoint::bytes(&inner, LIMIT)?;
        let opaque = Opaque {
            kind: purpose.kind().into(),
            version: 1,
            sha256: history_checkpoint::digest(&raw),
            bytes: raw.len(),
            data: String::from_utf8(raw).map_err(|e| error(e.to_string()))?,
        };
        serde_json::to_value(opaque).map_err(|e| error(e.to_string()))
    }
    pub(super) fn from_for(
        value: &Value,
        pool: &[Arc<Project>],
        mapping: &[usize],
        command: &str,
        purpose: Purpose,
        check: Option<&media_guard::Check>,
    ) -> Result<Self> {
        let opaque: Opaque = parse(purpose.command(), value)?;
        if opaque.kind != purpose.kind()
            || opaque.version != 1
            || opaque.bytes == 0
            || opaque.bytes > LIMIT
            || opaque.data.len() != opaque.bytes
            || history_checkpoint::digest(opaque.data.as_bytes()) != opaque.sha256
        {
            return Err(error("changed or oversized compound history proof"));
        }
        let inner: HistoryProof = serde_json::from_str(&opaque.data).map_err(|e| error(e.to_string()))?;
        if inner.version != 1
            || inner.command != command
            || !(1..=257).contains(&inner.snapshots.len())
            || mapping.len() != inner.snapshots.len()
            || inner.snapshots.iter().any(|h| !editor_setting::valid_hash(h))
        {
            return Err(error("foreign or incomplete compound history proof"));
        }
        let evidence = historical::Evidence::decode(inner.evidence.as_bytes())?;
        evidence.validate_mapped(pool, mapping, &inner.command)?;
        let projects = mapping.iter().map(|i| pool.get(*i).cloned().ok_or_else(|| error("invalid compound snapshot map"))).collect::<Result<Vec<_>>>()?;
        let retained = Self { evidence, projects, original: None };
        if retained.hashes(check)? != inner.snapshots || retained.proof_for(command, purpose, check)? != *value {
            return Err(error("historical compound snapshots or exact native evidence differ"));
        }
        Ok(retained)
    }
}
/// Exact bounded metadata for targeted control readback. The native project
/// pool records Arc identity relations, while private labels and merge keys
/// remain inside the digest. This is neither a checkpoint nor an Undo grant.
pub(super) fn history_state(project: &Arc<Project>, history: &crate::History) -> Result<Value> {
    if history.limit > 1000 || history.undo.len().saturating_add(history.redo.len()) > 256 || history.merge_key.as_ref().is_some_and(|k| k.len() > 2048) {
        return Err(error("current native control history exceeds its bounded descriptor"));
    }
    let mut pool = vec![project.clone()];
    let mut steps = |stack: &[(String, Arc<Project>)]| -> Result<Vec<Value>> {
        stack
            .iter()
            .map(|(label, p)| {
                if label.len() > 2048 {
                    return Err(error("current native history label exceeds its bound"));
                }
                let index = if let Some(i) = pool.iter().position(|q| Arc::ptr_eq(p, q)) {
                    i
                } else {
                    if pool.len() >= 257 {
                        return Err(error("native control history exceeds its snapshot bound"));
                    }
                    pool.push(p.clone());
                    pool.len() - 1
                };
                Ok(json!({"label":label,"project":index}))
            })
            .collect()
    };
    let undo = steps(&history.undo)?;
    let redo = steps(&history.redo)?;
    let mut budget = POOL_BYTES;
    let hashes = pool
        .iter()
        .map(|p| {
            let bytes = project_audit::bytes(p)?;
            budget = budget.checked_sub(bytes.len()).ok_or_else(|| error("native control history exceeds32MiB"))?;
            project_plane::project_basis(p)
        })
        .collect::<Result<Vec<_>>>()?;
    let raw =
        history_checkpoint::bytes(&json!({"version":1,"projects":hashes,"undo":undo,"redo":redo,"limit":history.limit,"mergeKey":history.merge_key}), LIMIT)?;
    Ok(json!({"sha256":history_checkpoint::digest(&raw),"undoSteps":history.undo.len(),"redoSteps":history.redo.len(),"limit":history.limit}))
}
