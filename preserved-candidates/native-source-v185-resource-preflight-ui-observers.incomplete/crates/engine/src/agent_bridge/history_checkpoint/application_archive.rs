//! Immutable application evidence in the ORIGINAL checkpoint pool and worker.
//! Only live receipts can create an attachment. Decoding yields data, never a
//! Scope, publication permit, read ticket, or permission to replay an outcome.
use super::application_tabs::ApplicationTabHistory;
use super::{MAX_BYTES, add, bytes, digest};
use crate::agent_bridge::{application_tab_scope::ApplicationTabReceipt, native_editor, operation_control::AgentOperationControl};
use crate::{Result, Session, commands::bad};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Weak};
const MAX_RECORDS: usize = 64;
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("native application history", message)
}
/// Borrowed original-application evidence, not a deserializable caller request.
pub struct ApplicationTabArchiveInput<'a> {
    pub preparation: &'a Value,
    pub outcome: &'a Value,
    pub receipt: &'a ApplicationTabReceipt,
}
pub(crate) struct Record {
    preparation: Value,
    outcome: Value,
    history: ApplicationTabHistory,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Saved {
    preparation: Value,
    outcome: Value,
    proof: Value,
    snapshots: Vec<usize>,
}
/// Non-cloneable, nonserializable physical owner binding. The same original
/// worker consumes this once; immutable retained records are its only payload.
pub struct ApplicationSnapshotCapture {
    owner: AgentOperationControl,
    project: Weak<Project>,
    epoch: Option<String>,
    path: Option<String>,
    revision: u64,
    history: crate::History,
    editor: native_editor::Snapshot,
    records: Vec<Arc<Record>>,
    ui: Option<Arc<super::application_ui::Record>>,
    refresh_ui_native: bool,
}
fn token(p: &Value) -> Result<&str> {
    p.get("reference")
        .and_then(|r| r.get("token"))
        .and_then(Value::as_str)
        .filter(|t| (32..=128).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .ok_or_else(|| error("original application reference token required"))
}
fn proof(value: &Value) -> Result<()> {
    let data = value
        .get("data")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 2 * 1024 * 1024)
        .ok_or_else(|| error("bounded opaque original UI proof required"))?;
    if value.as_object().is_none_or(|o| o.len() != 5)
        || value["kind"] != "native_application_tab_ui_proof"
        || value["version"] != 1
        || value["bytes"].as_u64() != u64::try_from(data.len()).ok()
        || value["sha256"].as_str() != Some(digest(data.as_bytes()).as_str())
    {
        return Err(error("changed original application UI proof"));
    }
    let _: Value = serde_json::from_str(data).map_err(|e| error(e.to_string()))?;
    Ok(())
}
pub(super) fn reference_scope(s: &Session, reference: &Value) -> Result<Value> {
    bytes(reference, 16 * 1024)?;
    if s.view_only {
        return Err(error("original owning Session required for archived application data"));
    }
    let identity = crate::project_workspace::identity(&s.project)?.ok_or_else(|| error("independent current Root required for archived application data"))?;
    let object = reference.as_object().filter(|v| v.len() == 5).ok_or_else(|| error("closed original application reference required"))?;
    let scope = serde_json::to_value(&identity.scope).map_err(|e| error(e.to_string()))?;
    if !object.get("token").and_then(Value::as_str).is_some_and(crate::agent_bridge::token_ok)
        || !object.get("applicationInstance").and_then(Value::as_str).is_some_and(filmcraft_project::ClipIdentity::valid_id)
        || object.get("scope") != Some(&scope)
        || !object.get("sha256").and_then(Value::as_str).is_some_and(crate::agent_bridge::editor_setting::valid_hash)
        || !object
            .get("projectPath")
            .and_then(Value::as_str)
            .is_some_and(|p| p.len() <= 4096 && std::path::Path::new(p).is_absolute() && p.ends_with(".fcproj") && !p.chars().any(char::is_control))
    {
        return Err(error("foreign or malformed archived application reference"));
    }

    Ok(scope)
}

impl Record {
    fn checkpoint_identity(&self) -> Result<Value> {
        self.validate()?;
        let raw = bytes(&(&self.preparation, &self.outcome), MAX_BYTES)?;
        let actual_ui = if self.outcome["status"] == "activated" { true } else { !self.outcome["publicationEvidence"]["actualUi"].is_null() };
        Ok(serde_json::json!({
            "reference":self.preparation["reference"],
            "carrier":{"encoding":"hex-json-pair-v1","sha256":digest(&raw),"bytes":raw.len()},
            "outcomeSha256":digest(&bytes(&self.outcome,MAX_BYTES)?),"outcomeStatus":self.outcome["status"],
            "nativeSnapshotComplete":self.history.native_snapshot_complete(),
            "actualSnapshotComplete":self.history.actual_snapshot_complete(),"actualUiCaptured":actual_ui
        }))
    }
    pub(super) fn scope(&self) -> Option<&Value> {
        self.preparation["reference"].get("scope")
    }
    fn validate(&self) -> Result<()> {
        // Bound the original carriers before inspecting or cloning nested data.
        bytes(&(&self.preparation, &self.outcome), MAX_BYTES)?;
        let p = &self.preparation;
        let d = &p["document"];
        let r = &p["reference"];
        let o = &self.outcome;
        let t = token(p)?;
        if p["kind"] != "native_application_tab_preparation"
            || p["version"] != 1
            || p["status"] != "prepared"
            || d["kind"] != "native_application_tab_view"
            || d["version"] != 1
            || d["readPurpose"] != "application_tab_preparation"
            || d["native"] != *self.history.original_read()
            || d["token"] != t
            || d["native"]["token"] != t
            || r["scope"] != d["native"]["scope"]
            || r["projectPath"] != d["native"]["project"]["path"]
            || r["applicationInstance"] != d["applicationInstance"]
            || !d["applicationInstance"].as_str().is_some_and(filmcraft_project::ClipIdentity::valid_id)
            || r["sha256"].as_str() != Some(digest(&bytes(d, MAX_BYTES)?).as_str())
            || d["uiSha256"] != d["ui"]["sha256"]
            || o["reference"] != *r
            || o["canReplay"] != false
        {
            return Err(error("foreign or changed application preparation/outcome binding"));
        }
        proof(&d["ui"])?;
        let native = self.history.result();
        match o["kind"].as_str() {
            Some("native_application_tab_change") => {
                if o["version"] != 1
                    || o["status"] != "activated"
                    || o["receiptComplete"] != true
                    || o["native"] != *native
                    || native["status"] != "activated"
                    || o["preparedUiSha256"] != d["uiSha256"]
                    || o["operation"]["type"] != "activate_tab"
                    || o["operation"]["item"] != native["item"]
                {
                    return Err(error("application change differs from original native receipt"));
                }
                proof(&o["beforeUi"])?;
                proof(&o["afterUi"])?;
            }
            Some("native_application_tab_outcome") => {
                let v = &o["publicationEvidence"];
                if o["version"] != 1
                    || o["status"] != "unknown"
                    || o["receiptComplete"] != false
                    || o["publicationStarted"] != true
                    || v["kind"] != "native_application_tab_publication_evidence"
                    || v["version"] != 1
                    || v["native"] != *native
                    || v["preparedUiSha256"] != d["uiSha256"]
                    || v["nativeReceiptConfirmed"] != (native["status"] == "activated")
                {
                    return Err(error("application uncertainty differs from original native receipt"));
                }
                proof(&v["beforeUi"])?;
                proof(&v["expectedAfterUi"])?;
                if !v["actualUi"].is_null() {
                    proof(&v["actualUi"])?;
                }
                let expected = if native["status"] == "activated" { native } else { &native["expected"] };
                if o["item"] != expected["item"]
                    || v["expectedNativeSha256"].as_str() != Some(digest(&bytes(expected, MAX_BYTES)?).as_str())
                    || (v["actualUi"].is_null() && (!v["actualUiMatchesExpected"].is_null() || !v["actualUiError"].is_string()))
                    || (!v["actualUi"].is_null() && (!v["actualUiError"].is_null() || v["actualUiMatchesExpected"] != (v["actualUi"] == v["expectedAfterUi"])))
                {
                    return Err(error("contradictory expected/actual application uncertainty"));
                }
            }
            _ => return Err(error("only original published application outcomes can enter history")),
        }
        Ok(())
    }
}
impl ApplicationSnapshotCapture {
    pub fn capture(session: &Session, input: &[ApplicationTabArchiveInput<'_>]) -> Result<Self> {
        session.require_owner()?;
        if input.len() > MAX_RECORDS || session.agent_bridge.application_history.len() > MAX_RECORDS {
            return Err(error("original application history record bound exceeded"));
        }
        let prior = session.agent_bridge.application_history.iter().map(|r| (&r.preparation, &r.outcome)).collect::<Vec<_>>();
        let incoming = input.iter().map(|i| (i.preparation, i.outcome)).collect::<Vec<_>>();
        bytes(&(prior, incoming), MAX_BYTES)?;
        let mut records = session.agent_bridge.application_history.clone();
        for i in input {
            i.receipt.validate_original_capture_owner(session)?;
            bytes(&(i.preparation, i.outcome), MAX_BYTES)?;
            let r = Record { preparation: i.preparation.clone(), outcome: i.outcome.clone(), history: i.receipt.capture_history()? };
            r.validate()?;
            if let Some(old) = records.iter().find(|p| p.preparation["reference"] == r.preparation["reference"]) {
                if old.preparation != r.preparation || old.outcome != r.outcome || old.history.proof()? != r.history.proof()? {
                    return Err(error("original application history cannot be replaced or upgraded"));
                }
            } else {
                if records.iter().any(|p| p.preparation["reference"]["token"] == r.preparation["reference"]["token"]) {
                    return Err(error("application history token already belongs to another reference"));
                }
                records.push(Arc::new(r));
            }
        }
        let mut pool = vec![session.project.clone()];
        extend_pool(&records, &mut pool)?;
        encode(&records, &pool)?;
        Ok(Self {
            owner: session.agent_operation_control(),
            project: Arc::downgrade(&session.project),
            epoch: session.agent_bridge.epoch.clone(),
            path: session.path.clone(),
            revision: session.revision,
            history: session.history.clone(),
            editor: native_editor::Snapshot::capture(session)?,
            records,
            ui: session.agent_bridge.application_ui.clone(),
            refresh_ui_native: false,
        })
    }
    /// A finite original UI producer can attach immutable passive data before
    /// ownership is handed to the SAME worker. No serialized caller can do so.
    pub fn with_application_ui(mut self, s: &Session, proof: Value) -> Result<Self> {
        self.validate(s)?;
        self.ui = Some(Arc::new(super::application_ui::Record::capture(s, proof)?));
        self.refresh_ui_native = true;
        Ok(self)
    }
    pub(crate) fn validate(&self, s: &Session) -> Result<()> {
        if s.is_display_only()
            || !self.owner.same_original_owner(&s.agent_operation_control())
            || !self.project.ptr_eq(&Arc::downgrade(&s.project))
            || self.epoch != s.agent_bridge.epoch
            || self.path != s.path
            || self.revision != s.revision
            || !crate::trim::command_settlement::historical::same_history(&self.history, &s.history)
            || !self.editor.matches(s)?
        {
            return Err(error("original application capture owner or current native state changed"));
        }
        Ok(())
    }
    /// Only the original pause caller may settle its acknowledged persistence
    /// notifications. A newer saved revision is data from that SAME worker;
    /// every other captured native field and physical owner must still match.
    pub(crate) fn after_original_persistence_notifications(mut self, s: &Session) -> Result<Self> {
        if s.saved_revision < self.editor.saved_revision || s.saved_revision > s.revision {
            return Err(error("original persistence notification crossed the captured saved revision"));
        }
        self.editor.saved_revision = s.saved_revision;
        self.validate(s)?;
        Ok(self)
    }
    pub(crate) fn stage(self, s: &mut Session) -> Result<()> {
        self.validate(s)?;
        if s.agent_bridge.pending_application_capture.is_some() {
            return Err(error("original save already has a physical application attachment"));
        }
        s.agent_bridge.pending_application_capture = Some(self);
        Ok(())
    }
    pub(crate) fn install(self, s: &mut Session) -> Result<()> {
        self.validate(s)?;
        let previous_records = std::mem::replace(&mut s.agent_bridge.application_history, self.records);
        let previous_ui = std::mem::replace(&mut s.agent_bridge.application_ui, self.ui);
        if self.refresh_ui_native {
            // Capture auxiliary origins against the replacement graph, rather
            // than carrying origins retained only by the previous UI image.
            // An unchanged old UI record never becomes current by this path.
            let refreshed = s
                .agent_bridge
                .application_ui
                .as_ref()
                .ok_or_else(|| error("fresh physical application UI attachment missing"))
                .and_then(|record| record.refresh_attachment(s));
            match refreshed {
                Ok(record) => s.agent_bridge.application_ui = Some(Arc::new(record)),
                Err(error) => {
                    s.agent_bridge.application_history = previous_records;
                    s.agent_bridge.application_ui = previous_ui;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn extend_pool(records: &[Arc<Record>], pool: &mut Vec<Arc<Project>>) -> Result<()> {
    if records.len() > MAX_RECORDS {
        return Err(error("application history record bound exceeded"));
    }
    for r in records {
        for p in r.history.projects() {
            add(pool, p)?;
        }
    }
    Ok(())
}
pub(crate) fn encode(records: &[Arc<Record>], pool: &[Arc<Project>]) -> Result<Vec<Saved>> {
    let result = records
        .iter()
        .map(|r| {
            r.validate()?;
            Ok(Saved { preparation: r.preparation.clone(), outcome: r.outcome.clone(), proof: r.history.proof()?, snapshots: r.history.mapping(pool)? })
        })
        .collect::<Result<Vec<_>>>()?;
    bytes(&result, MAX_BYTES)?;
    Ok(result)
}
pub(crate) fn decode(saved: &[Saved], pool: &[Arc<Project>], used: &mut std::collections::BTreeSet<usize>) -> Result<Vec<Arc<Record>>> {
    if saved.len() > MAX_RECORDS {
        return Err(error("application history record bound exceeded"));
    }
    bytes(&saved, MAX_BYTES)?;
    let mut result: Vec<Arc<Record>> = Vec::new();
    for v in saved {
        let r = Record {
            preparation: v.preparation.clone(),
            outcome: v.outcome.clone(),
            history: ApplicationTabHistory::from_proof(&v.proof, pool, &v.snapshots)?,
        };
        r.validate()?;
        if result.iter().any(|p| p.preparation["reference"]["token"] == r.preparation["reference"]["token"]) {
            return Err(error("duplicated historical application token"));
        }
        used.extend(v.snapshots.iter().copied());
        result.push(Arc::new(r));
    }
    Ok(result)
}

pub(crate) fn retain_media(s: &Session) {
    for r in &s.agent_bridge.application_history {
        let path = r.history.original_read()["project"]["path"].as_str();
        for p in r.history.projects() {
            s.media.add_stored(p, path);
        }
    }
}
pub(crate) fn origins(s: &Session) -> Result<std::collections::BTreeMap<String, Option<std::path::PathBuf>>> {
    let mut result = s.media.recovery_origins(
        std::iter::once(s.project.as_ref()).chain(s.history.undo.iter().chain(&s.history.redo).map(|(_, p)| p.as_ref())),
        s.path.as_deref(),
    )?;
    for r in &s.agent_bridge.application_history {
        let additional = s.media.recovery_origins(r.history.projects().iter().map(|p| p.as_ref()), r.history.original_read()["project"]["path"].as_str())?;
        for (path, origin) in additional {
            if !result.contains_key(&path) && result.len() >= 65_536 {
                return Err(error("complete application media origin inventory exceeds existing bound"));
            }
            result.entry(path).or_insert(origin);
        }
    }
    if let Some(ui) = &s.agent_bridge.application_ui {
        for (path, origin) in ui.origins(s)? {
            if !result.contains_key(&path) && result.len() >= 65_536 {
                return Err(error("complete passive UI media origin inventory exceeds existing bound"));
            }
            result.entry(path).or_insert(origin);
        }
    }
    Ok(result)
}
impl Session {
    /// Immutable original checkpoint data, not a live application outcome or
    /// input handle. Even pointer-identical worker projections cannot supply it.
    /// Complete ORIGINAL immutable preparation/outcome data in bounded pages.
    /// The old summary route stays unchanged. Pages never mint live scopes,
    /// tickets or publication permits, including for an uncertain outcome.
    pub fn archived_application_tab_record_page(&self, reference: &Value, offset: usize, limit: usize) -> Result<Option<Value>> {
        const PAGE_BYTES: usize = 1024 * 1024;
        const REPLY_BYTES: usize = 12 * 1024 * 1024;
        if limit == 0 || limit > PAGE_BYTES || offset > MAX_BYTES {
            return Err(error("bounded original application page offset and 1MiB maximum limit required"));
        }
        // Reuse the ORIGINAL closed identity, graph and record validation.
        // This also refuses a worker projection before reading any carrier.
        let Some(archive) = self.archived_application_tab(reference)? else { return Ok(None) };
        let record = self
            .agent_bridge
            .application_history
            .iter()
            .find(|r| r.preparation["reference"] == *reference)
            .ok_or_else(|| error("original immutable application record is unavailable"))?;
        // This is the exact original pair encoding already bounded by Record.
        // No alternate history serializer or persistent page cache is created.
        let raw = bytes(&(&record.preparation, &record.outcome), MAX_BYTES)?;
        if offset > raw.len() {
            return Err(error("original application page offset exceeds its complete carrier"));
        }
        let end = offset.checked_add(limit).ok_or_else(|| error("application page range overflow"))?.min(raw.len());
        let page = raw.get(offset..end).ok_or_else(|| error("invalid original application page range"))?;
        let capacity = page.len().checked_mul(2).ok_or_else(|| error("application page encoding overflow"))?;
        let mut data = String::with_capacity(capacity);
        use std::fmt::Write as _;
        for byte in page {
            write!(&mut data, "{byte:02x}").map_err(|e| error(e.to_string()))?;
        }
        let next = (end < raw.len()).then_some(end);
        let value = serde_json::json!({"kind":"native_archived_application_tab_record_page","version":1,
            "archive":archive,"encoding":"hex-json-pair-v1","sha256":digest(&raw),"bytes":raw.len(),
            "offset":offset,"nextOffset":next,"data":data,
            "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"requestReplay":false});
        bytes(&value, REPLY_BYTES)?;
        Ok(Some(value))
    }
    pub fn archived_application_tab(&self, reference: &Value) -> Result<Option<Value>> {
        let scope = reference_scope(self, reference)?;
        // Preserve the existing complete native pool/history bound before any
        // current metadata snapshot clones its original COW/history vectors.
        super::graph(self)?;
        let Some(record) = self.agent_bridge.application_history.iter().find(|r| r.preparation["reference"] == *reference) else { return Ok(None) };
        record.validate()?;
        let actual_ui = if record.outcome["status"] == "activated" { true } else { !record.outcome["publicationEvidence"]["actualUi"].is_null() };
        let value = serde_json::json!({"kind":"native_archived_application_tab","version":1,
            "reference":reference,"outcomeSha256":digest(&bytes(&record.outcome, MAX_BYTES)?),"outcomeStatus":record.outcome["status"],
            "nativeSnapshotComplete":record.history.native_snapshot_complete(),"actualSnapshotComplete":record.history.actual_snapshot_complete(),"actualUiCaptured":actual_ui,
            "current":{"scope":scope,"native":crate::agent_bridge::application_tab_scope::Snapshot::current_metadata(self)?},
            "history":"native_checkpoint_data_only","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"requestReplay":false});
        bytes(&value, 65536)?;
        Ok(Some(value))
    }
}

// Attest data in an exact physical companion through the ORIGINAL decoder.
// This command neither inspects the live archive as a substitute nor installs it.
pub(super) const CHECKPOINT_INSPECTION: &str = "agent.inspectApplicationHistoryCheckpoint";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckpointInspection {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    checkpoint: String,
    sha256: String,
    basis: super::InspectionBasis,
    reference: Value,
}
pub(super) fn inspect_checkpoint(s: &mut Session, p: &Value) -> Result<Value> {
    bytes(p, 32 * 1024)?;
    let input: CheckpointInspection = crate::agent_bridge::parse(CHECKPOINT_INSPECTION, p)?;
    let current_scope = reference_scope(s, &input.reference)?;
    if current_scope != serde_json::to_value(&input.scope).map_err(|e| error(e.to_string()))?
        || p["basis"].get("sequence").is_none()
        || !filmcraft_project::ClipIdentity::valid_id(&input.basis.native_session)
        || input.basis.revision == 0
        || !crate::agent_bridge::editor_setting::valid_hash(&input.sha256)
    {
        return Err(error("exact Root, original save basis, companion digest and closed application reference required"));
    }
    let request = super::Request {
        token: input.token,
        scope: crate::agent_bridge::organization_scope::Scope::Project(input.scope),
        workspace: input.workspace,
        checkpoint: input.checkpoint,
        sha256: Some(input.sha256.clone()),
        metadata_basis: true,
    };
    let sequence = super::scope(s, &request, false)?;
    let check = s.agent_bridge.operations.check();
    let (capsule, size, hash) = super::load_with_digest(&super::path(s, &request)?, Some(&input.sha256), check.as_deref())?;
    if capsule.epoch != input.basis.native_session || capsule.revision != input.basis.revision || capsule.sequence != input.basis.sequence {
        return Err(error("application companion differs from its exact original save basis"));
    }
    let prepared = super::prepare_history(s, &request, &capsule, &hash, sequence, super::HistoryValidation::HistoricalIdentity)?;
    let record = prepared
        .application
        .iter()
        .find(|r| r.preparation["reference"] == input.reference)
        .ok_or_else(|| error("the exact application record is not retained in this companion"))?;
    let identity = record.checkpoint_identity()?;
    let ui = prepared.application_ui.as_ref().map(|record| record.proof_identity()).transpose()?;
    if let Some(check) = check {
        check.check()?;
    }
    let value = serde_json::json!({
        "status":"read","kind":"native_application_history_checkpoint","version":1,
        "token":request.token,"scope":request.scope,"projectPath":s.path,
        "nativeSession":super::epoch(s)?,"revision":s.revision,"sequence":sequence.map(|v|v.0),
        "basis":{"nativeSession":capsule.epoch,"revision":capsule.revision,"sequence":capsule.sequence},
        "checkpoint":super::summary(&capsule,&hash,size),"record":identity,"applicationUi":ui,
        "historyValidation":"validated_archive_only","projectUnchanged":true,"historyUnchanged":true,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,
        "requestReplay":false,"execution":"not-applied"
    });
    bytes(&value, 64 * 1024)?;
    Ok(value)
}

#[cfg(test)]
mod tests;
