//! Immutable UI data bound to the ORIGINAL native history pool. Never a live
//! application, input queue, callback, read ticket, Scope or writer grant.
use super::{MAX_BYTES, add, bytes, digest};
use crate::agent_bridge::native_editor;
use crate::{Result, Session, commands::bad};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
const UI_BYTES: usize = 8 * 1024 * 1024;
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("native application UI snapshot", message)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Saved {
    version: u8,
    project: usize,
    path: String,
    revision: u64,
    native: native_editor::Snapshot,
    proof: Value,
}
pub(crate) struct Record {
    project: Arc<Project>,
    path: String,
    revision: u64,
    native: native_editor::Snapshot,
    proof: Value,
}
fn validate_proof(proof: &Value) -> Result<()> {
    let data = proof
        .get("data")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= UI_BYTES)
        .ok_or_else(|| error("bounded original passive application UI payload required"))?;
    if proof.as_object().is_none_or(|o| o.len() != 5)
        || proof["kind"] != "native_passive_application_ui_proof"
        || proof["version"] != 1
        || proof["bytes"].as_u64() != u64::try_from(data.len()).ok()
        || proof["sha256"].as_str() != Some(digest(data.as_bytes()).as_str())
    {
        return Err(error("changed or unsupported passive application UI proof"));
    }
    let _: Value = serde_json::from_str(data).map_err(|e| error(e.to_string()))?;
    Ok(())
}
impl Record {
    pub(crate) fn capture(s: &Session, proof: Value) -> Result<Self> {
        s.require_owner()?;
        validate_proof(&proof)?;
        if crate::project_workspace::identity(&s.project)?.is_none() {
            return Err(error("independent native Root required for full passive application capture"));
        }
        let path = s.path.clone().ok_or_else(|| error("saved native Root path required"))?;
        let native = native_editor::Snapshot::capture(s)?;
        let record = Self { project: s.project.clone(), path, revision: s.revision, native, proof };
        record.validate()?;
        Ok(record)
    }
    pub(crate) fn refresh_attachment(&self, s: &Session) -> Result<Self> {
        if !Arc::ptr_eq(&self.project, &s.project) || s.path.as_ref() != Some(&self.path) || self.revision != s.revision {
            return Err(error("fresh passive UI attachment lost its original native owner"));
        }
        Self::capture(s, self.proof.clone())
    }
    fn validate(&self) -> Result<()> {
        if self.path.len() > 4096
            || !std::path::Path::new(&self.path).is_absolute()
            || !self.path.ends_with(".fcproj")
            || self.path.chars().any(char::is_control)
            || self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || crate::project_workspace::identity(&self.project)?.is_none()
            || self.native.auxiliary.is_none()
        {
            return Err(error("complete versioned native image and passive application namespace required"));
        }
        self.native.validate(self.revision, self.native.editor.active_sequence.map(|id| id.0))?;
        if self.native.editor.active_sequence.is_some_and(|id| self.project.sequence(id).is_none()) {
            return Err(error("passive UI native sequence is absent from captured project"));
        }
        validate_proof(&self.proof)?;
        bytes(&(&self.path, self.revision, &self.native, &self.proof), MAX_BYTES)?;
        Ok(())
    }
    pub(crate) fn proof_identity(&self) -> Result<Value> {
        self.validate()?;
        Ok(serde_json::json!({"kind":self.proof["kind"],"version":self.proof["version"],
            "bytes":self.proof["bytes"],"sha256":self.proof["sha256"]}))
    }
    pub(crate) fn current(&self, s: &Session) -> Result<bool> {
        // A real paused Session may read its own passive data before admission.
        // A worker display projection may never adopt another owner's image.
        if s.view_only || !Arc::ptr_eq(&self.project, &s.project) || s.path.as_ref() != Some(&self.path) || self.revision != s.revision {
            return Ok(false);
        }
        self.native.matches(s)
    }
    pub(crate) fn add_to_pool(&self, pool: &mut Vec<Arc<Project>>) -> Result<()> {
        add(pool, &self.project)?;
        Ok(())
    }
    pub(crate) fn encode(&self, pool: &[Arc<Project>]) -> Result<Saved> {
        self.validate()?;
        let project = pool.iter().position(|p| Arc::ptr_eq(p, &self.project)).ok_or_else(|| error("original complete pool omitted passive UI native image"))?;
        Ok(Saved { version: 1, project, path: self.path.clone(), revision: self.revision, native: self.native.clone(), proof: self.proof.clone() })
    }
    pub(crate) fn decode(saved: &Saved, pool: &[Arc<Project>], used: &mut std::collections::BTreeSet<usize>) -> Result<Arc<Self>> {
        if saved.version != 1 {
            return Err(error("unsupported passive application UI image"));
        }
        let result = Self {
            project: pool.get(saved.project).cloned().ok_or_else(|| error("missing passive UI native project image"))?,
            path: saved.path.clone(),
            revision: saved.revision,
            native: saved.native.clone(),
            proof: saved.proof.clone(),
        };
        result.validate()?;
        used.insert(saved.project);
        Ok(Arc::new(result))
    }
    pub(crate) fn origins(&self, s: &Session) -> Result<std::collections::BTreeMap<String, Option<std::path::PathBuf>>> {
        s.media.recovery_origins(std::iter::once(self.project.as_ref()), Some(&self.path))
    }
    pub(crate) fn retain_media(&self, s: &Session) {
        s.media.add_stored(&self.project, Some(&self.path));
    }
}
impl Session {
    pub fn has_native_application_root(&self) -> Result<bool> {
        Ok(crate::project_workspace::identity(&self.project)?.is_some())
    }
    /// Readonly saved data. A stale image cannot stand in for current UI.
    /// The owning UI performs its own closed typed decode before installation.
    pub fn saved_application_ui(&self) -> Result<Option<Value>> {
        let Some(record) = &self.agent_bridge.application_ui else { return Ok(None) };
        if !record.current(self)? {
            return Err(error("saved application UI belongs to an earlier native image; no layout or draft was adopted"));
        }
        Ok(Some(record.proof.clone()))
    }
}

#[cfg(test)]
mod tests;
