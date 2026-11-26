//! Self-contained historical origin data for native graph/checkpoint consumers.
//! A saved origin can recover data relationships, never a fresh qualified read,
//! retained live preparation, source witness, operation fence or publication grant.
use super::{Request, error, project_audit, publication};
use crate::{EditorState, Result, Session, project_workspace::ProjectScope};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
const LIMIT: usize = 16 * 1024 * 1024;
const KIND: &str = "poema-native-multicam-retained-origin";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    kind: String,
    version: u8,
    record: String,
    request: Request,
    projection: Value,
    dependencies: Vec<u64>,
    before_editor: EditorState,
    after_editor: EditorState,
}
/// Serialized indices point to the original complete native COW graph, including
/// the original commit edge when a later Undo/Redo carries this historical role.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::agent_bridge) struct Saved {
    version: u8,
    before: usize,
    after: usize,
    evidence: Evidence,
}
/// Historical data only. No constructor installs this as ReadOwner or Sealed.
#[derive(Clone)]
pub(in crate::agent_bridge) struct Retained {
    evidence: Evidence,
    before: Arc<Project>,
    after: Arc<Project>,
}
fn bytes(v: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = project_audit::value_bytes(v)?;
    if bytes.len() > LIMIT {
        return Err(error("complete retained source origin exceeds its byte bound; no truncation"));
    }
    Ok(bytes)
}
fn hash(v: &[u8]) -> String {
    format!("{:x}", Sha256::digest(v))
}
impl Evidence {
    fn validate(&self, before: &Arc<Project>, after: &Arc<Project>, scope: &ProjectScope, path: &str, token: &str, revision: u64) -> Result<()> {
        if self.kind != KIND || self.version != 1 {
            return Err(error("unknown historical source origin kind or version"));
        }
        bytes(self)?;
        let record = publication::Historical::decode(self.record.as_bytes())?;
        record.validate_identity(scope, path, token, revision)?;
        record.validate_origin(&self.request, &self.projection, &self.dependencies)?;
        record.validate_original_content(before, after, &self.before_editor, &self.after_editor)?;
        // Metadata resolution is the SAME original target/dependency walker. This
        // Session has no ticket, media witness, worker, transport or execution power.
        let mut data = Session::default();
        data.project = before.clone();
        data.state = self.before_editor.clone();
        data.path = Some(path.into());
        let resolved = self.request.resolve(&data)?;
        if resolved.projection != self.projection || resolved.dependencies != self.dependencies {
            return Err(error("historical source origin is not the exact original input projection and complete dependencies"));
        }
        crate::clip_identity::enforce_protection(before, after)?;
        for item in before.sequences() {
            if let Some(q) = item.as_sequence() {
                for track in q.all_tracks().filter(|t| t.locked) {
                    let observed = after.sequence(item.id).and_then(|q| q.track(track.id));
                    if project_audit::value_bytes(&Some(track))? != project_audit::value_bytes(&observed)? {
                        return Err(error("historical source origin changes an originally locked track"));
                    }
                }
            }
        }
        Ok(())
    }
}
pub(in crate::agent_bridge) fn capture(s: &Session, token: &str, candidate: &super::super::Staged, record: &publication::Sealed) -> Result<Retained> {
    record.validate_live(s, candidate)?;
    let ticket = super::current(
        s,
        token,
        &s.agent_bridge.tickets.iter().find(|t| t.token == token).and_then(|t| t.project_scope.clone()).ok_or_else(|| error("original source Root missing"))?,
    )?;
    let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("actual qualified source producer required"))?;
    let record = String::from_utf8(record.bytes()?).map_err(|e| error(e.to_string()))?;
    let evidence = Evidence {
        kind: KIND.into(),
        version: 1,
        record,
        request: owner.request.clone(),
        projection: owner.projection.clone(),
        dependencies: owner.dependencies.clone(),
        before_editor: s.state.clone(),
        after_editor: candidate.state.clone(),
    };
    evidence.validate(&s.project, &candidate.project, &owner.request.scope, owner.request.project_path.as_str(), token, s.revision)?;
    Ok(Retained { evidence, before: s.project.clone(), after: candidate.project.clone() })
}
impl Retained {
    pub(in crate::agent_bridge) fn projects(&self) -> [Arc<Project>; 2] {
        [self.before.clone(), self.after.clone()]
    }
    pub(in crate::agent_bridge) fn rebind_published_after(&mut self, after: &Arc<Project>) {
        self.after = after.clone();
    }
    pub(in crate::agent_bridge) fn passive_entry(&self) -> Result<Value> {
        let raw = bytes(&self.evidence)?;
        Ok(
            json!({"kind":KIND,"version":1,"evidence":String::from_utf8(raw.clone()).map_err(|e|error(e.to_string()))?,"sha256":hash(&raw),"writeAuthority":false}),
        )
    }

    pub(in crate::agent_bridge) fn saved(&self, pool: &[Arc<Project>]) -> Result<Saved> {
        let index = |node: &Arc<Project>| {
            pool.iter()
                .position(|p| Arc::ptr_eq(node, p))
                .ok_or_else(|| error("historical source origin requires its exact original before/after COW edge; never drop or substitute it"))
        };
        let saved = Saved { version: 1, before: index(&self.before)?, after: index(&self.after)?, evidence: self.evidence.clone() };
        bytes(&saved)?;
        Ok(saved)
    }
    pub(in crate::agent_bridge) fn metadata(&self) -> Result<Value> {
        let raw = bytes(&self.evidence)?;
        Ok(
            json!({"kind":KIND,"version":1,"bytes":raw.len(),"sha256":hash(&raw),"writeAuthority":false,"sourceValidation":"not-performed","execution":"not-applied"}),
        )
    }
}
impl Saved {
    pub(in crate::agent_bridge) fn edge(&self) -> (usize, usize) {
        (self.before, self.after)
    }
    pub(in crate::agent_bridge) fn validate_commit(
        &self,
        pool: &[Arc<Project>],
        scope: &ProjectScope,
        path: &str,
        token: &str,
        revision: u64,
        batch: &super::super::Batch,
        result: &Value,
        before_sequence: Option<u64>,
        after_sequence: Option<u64>,
    ) -> Result<Retained> {
        super::validate_batch(scope, batch)?;
        let retained = self.restore(pool, scope, path, token, revision)?;
        let record = publication::Historical::decode(self.evidence.record.as_bytes())?;
        let op = batch.operations.first().ok_or_else(|| error("historical source operation missing"))?;
        let one = result["results"]
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .ok_or_else(|| error("one exact historical native source result required"))?;
        record.validate_result(one)?;
        let unchanged = self.before == self.after;
        if unchanged != super::unchanged_result(result)
            || unchanged
                && (one["moved"] != 0
                    || result["timelineCommits"] != 0
                    || result["undoOwned"] != false
                    || result["revision"] != revision
                    || result["beforeRevision"] != revision
                    || before_sequence != after_sequence
                    || bytes(&self.evidence.before_editor)? != bytes(&self.evidence.after_editor)?)
            || !unchanged && (result.get("outcome").is_some() || result.get("timelineCommits").is_some() || result.get("undoOwned").is_some())
        {
            return Err(error("historical unchanged intent must have its exact single COW node, native result, editor and zero-edit receipt"));
        }
        if op.params["recordSha256"] != record.metadata()?["sha256"]
            || result["action"] != "multicam_production"
            || result["computedRecord"] != record.metadata()?
            || result["retainedOrigin"] != retained.metadata()?
            || result["sequence"] != json!(after_sequence)
            || self.evidence.before_editor.active_sequence.map(|id| id.0) != before_sequence
            || self.evidence.after_editor.active_sequence.map(|id| id.0) != after_sequence
        {
            return Err(error("historical source receipt contradicts its exact original computed origin"));
        }
        Ok(retained)
    }

    pub(in crate::agent_bridge) fn decode(raw: &[u8]) -> Result<Self> {
        if raw.is_empty() || raw.len() > LIMIT {
            return Err(error("bounded complete historical source origin required"));
        }
        let saved: Self = serde_json::from_slice(raw).map_err(|e| error(e.to_string()))?;
        if saved.version != 1 || bytes(&saved)? != raw {
            return Err(error("unknown or noncanonical historical source origin encoding"));
        }
        Ok(saved)
    }
    pub(in crate::agent_bridge) fn encoded(&self) -> Result<Vec<u8>> {
        bytes(self)
    }
    pub(in crate::agent_bridge) fn restore(&self, pool: &[Arc<Project>], scope: &ProjectScope, path: &str, token: &str, revision: u64) -> Result<Retained> {
        if self.version != 1 {
            return Err(error("historical source origin version differs"));
        }
        bytes(self)?;
        let before = pool.get(self.before).ok_or_else(|| error("historical source origin before snapshot is missing"))?;
        let after = pool.get(self.after).ok_or_else(|| error("historical source origin after snapshot is missing"))?;
        self.evidence.validate(before, after, scope, path, token, revision)?;
        Ok(Retained { evidence: self.evidence.clone(), before: before.clone(), after: after.clone() })
    }
}
