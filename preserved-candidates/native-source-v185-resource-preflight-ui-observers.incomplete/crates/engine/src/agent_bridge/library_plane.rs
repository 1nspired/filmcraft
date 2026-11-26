//! Qualified independent Root source reads. Metadata tickets are never promoted;
//! the existing background verifier, guarded batch and history services remain owners.
use super::{Batch, parse, project_plane};
#[cfg(not(target_arch = "wasm32"))]
use super::{Ticket, admission, context, media_guard};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
pub(super) const READ: &str = "agent.readSourceLibrary";
pub(super) const VALIDATE: &str = "agent.validateSourceLibrary";
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(READ, m)
}
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub(super) token: String,
    pub(super) scope: ProjectScope,
    project_path: String,
    sources: Vec<u64>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Request {
    fn selected(&self, s: &Session) -> Result<Vec<u64>> {
        project_plane::require_scope(s, &self.scope)?;
        if self.project_path.len() > 4096
            || !self.project_path.starts_with('/')
            || self.project_path.chars().any(char::is_control)
            || s.path.as_ref() != Some(&self.project_path)
            || self.sources.len() > 2048
            || self.sources.iter().any(|id| !(1..=9_007_199_254_740_991).contains(id))
        {
            return Err(error("exact saved Root path and bounded explicit source IDs required; no library fallback"));
        }
        let unique = self.sources.iter().copied().collect::<std::collections::BTreeSet<_>>();
        if unique.len() != self.sources.len() {
            return Err(error("source IDs must be distinct"));
        }
        let mut original = Vec::new();
        for id in unique {
            let it = s.project.item(filmcraft_project::ItemId(id)).ok_or_else(|| error("requested source is missing"))?;
            let media = it.as_media().ok_or_else(|| error("requested item is not file media"))?;
            if !matches!(media.media, filmcraft_project::MediaRef::File { .. }) {
                return Err(error("library source qualification requires explicit file media"));
            }
            if let Some(binding) = super::source_binding(it.metadata.get(super::BINDING_KEY), READ)? {
                if binding.project_id != self.scope.project_id || binding.role != "original" {
                    return Err(error("requested source belongs to another original owner"));
                }
                original.push(id);
            }
        }
        Ok(original)
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let request: Request = parse(READ, p)?;
    let selected = request.selected(s)?;
    super::snapshot_jobs::start_library(s, request, selected)
}
#[cfg(target_arch = "wasm32")]
pub(super) fn read(_: &mut Session, _: &Value) -> Result<Value> {
    Err(error("independent source-library qualification requires the native verifier"))
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn deliver(s: &mut Session, r: Request, media: Arc<media_guard::Witnesses>) -> Result<Value> {
    super::fresh_snapshot(s, &r.token)?;
    r.selected(s)?;
    let rows = super::source_rows(s, &media, true)?;
    let bytes = rows.iter().try_fold(0usize, |n, row| {
        serde_json::to_vec(row).map_err(|e| error(e.to_string())).and_then(|raw| n.checked_add(raw.len()).ok_or_else(|| error("source metadata byte overflow")))
    })?;
    if bytes > 7 * 1024 * 1024 {
        return Err(error("complete source metadata exceeds the transport bound; no rows truncated"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = Value::Null;
    let mut result = json!({"protocol":1,"kind":"native_source_library_read","version":1,"token":r.token,"revision":s.revision,"nativeSession":epoch,"scope":r.scope,
        "project":{"path":s.path,"name":s.project.name,"dirty":s.revision!=s.saved_revision},"sequence":s.state.active_sequence.map(|id|id.0),"requestedSources":r.sources,"sources":rows,"mediaScope":media.scope_json(),
        "capabilities":{"sourceLibraryRead":true,"exactEdits":true,"guardedUndo":true,"guardedRedo":true,"guardedHistoryRead":true,"historyProjection":true,"sourceFileGuard":true,"scopedSourceVerification":true,"backgroundSnapshots":true,"backgroundValidation":true,"backgroundSnapshotDeadlineMs":media_guard::SNAPSHOT_READ_DEADLINE_MS,
            "sourceAdmission":true,"sourceAdmissionBatch":true,"sourceAudioLayout":true,"backgroundInspection":true,"backgroundPublication":s.worker_available,"publicationValidatedApply":s.worker_available&&super::operation_cancellation_available(s),"sourceAdmissionPreparation":s.worker_available&&super::operation_cancellation_available(s),"sourceAdmissionPreparationDeadlineMs":super::admission_jobs::PREPARE_MS,"sourceAdmissionPublicationDeadlineMs":super::admission_jobs::PUBLICATION_MS,
            "operationCancellation":super::operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS,"historyCheckpointOperations":true,"historyCheckpointRetention":true},
        "writeAuthority":false});
    let redo_bytes = s.history.redo.last().map_or(0, |(_, target)| admission::restored_source_bytes(s, target));
    result["capabilities"]["largeSourceVerification"] = json!(
        media_guard::source_deadline(media.source_bytes().saturating_add(redo_bytes)) == media_guard::LARGE_SOURCE_DEADLINE_MS
            && s.worker_available
            && super::operation_cancellation_available(s)
    );
    result["capabilities"]["largeSourceVerificationDeadlineMs"] = json!(media_guard::LARGE_SOURCE_DEADLINE_MS);
    result["capabilities"]["largeSourceVerificationThresholdBytes"] = json!(media_guard::LARGE_SOURCE_BYTES);
    media_guard::forget_changed(&s.media, &media, s.agent_bridge.media.as_deref());
    s.agent_bridge.media = Some(media.clone());
    let editor = context(s)?;
    super::register_ticket(
        s,
        Ticket {
            token: r.token,
            revision: s.revision,
            sequence: s.state.active_sequence,
            project_scope: Some(r.scope),
            source_library: Some(r.sources.clone()),
            #[cfg(not(target_arch = "wasm32"))]
            multicam_history: None,
            multicam_production: None,
            editor_control: None,
            #[cfg(not(target_arch = "wasm32"))]
            sequence_views: None,
            project: Arc::downgrade(&s.project),
            path: s.path.clone(),
            context: editor,
            committed: None,
            after: None,
            after_sequence: None,
            redo_consumed: false,
            media,
            #[cfg(not(target_arch = "wasm32"))]
            preparation: None,
        },
    )?;
    Ok(result)
}
pub(super) fn is_ticket(s: &Session, token: &str) -> bool {
    s.agent_bridge.tickets.iter().any(|t| t.token == token && t.source_library.is_some())
}
pub(super) fn validate_ticket(s: &Session, token: &str, scope: &ProjectScope, projection: bool) -> Result<()> {
    project_plane::require_scope(s, scope)?;
    super::check_edit_ticket_basis(s, token)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("expired source-library read"))?;
    if ticket.source_library.is_none() || ticket.project_scope.as_ref() != Some(scope) {
        return Err(error("a fresh qualified library read with exact Root identity is required; metadata cannot supply it"));
    }
    if !projection {
        super::validate_media(s, &ticket.media)?;
    }
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validate {
    token: String,
    scope: ProjectScope,
    job: String,
    #[serde(default)]
    inspection: Option<String>,
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Validate = parse(VALIDATE, p)?;
    if !super::token_ok(&r.job) || r.job == r.token {
        return Err(error("distinct explicit read and verification job IDs required"));
    }
    if p.get("inspection").is_some_and(|v| v.as_str().is_none_or(|id| !super::token_ok(id))) {
        return Err(error("an explicit inspection must retain its exact owned source-layout job"));
    }
    validate_ticket(s, &r.token, &r.scope, true)?;
    super::read_jobs::start_library(s, p, &super::ValidateRead { token: r.token, job: Some(r.job), candidate: None, inspection: r.inspection }, r.scope)
}
#[cfg(target_arch = "wasm32")]
pub(super) fn validate(_: &mut Session, _: &Value) -> Result<Value> {
    Err(error("independent source-library verification requires the native verifier"))
}
pub(super) fn command_scope(s: &Session, id: &str, p: &Value) -> Result<()> {
    let token = p.get("token").or_else(|| p.get("batch").and_then(|b| b.get("token"))).and_then(Value::as_str);
    if token.is_some_and(|token| is_ticket(s, token))
        && !matches!(
            id,
            VALIDATE | "agent.readHistory" | "agent.apply" | "agent.undo" | "agent.redo" | "agent.receipt" | "agent.prepareSourceAdmission" | "agent.inspect"
        )
    {
        return Err(error("library reads authorize only exact source admission and owned history, never metadata or timeline operations"));
    }
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn valid_sources(sources: &[u64]) -> bool {
    sources.len() <= 2048
        && sources.iter().all(|id| (1..=9_007_199_254_740_991).contains(id))
        && sources.iter().collect::<std::collections::BTreeSet<_>>().len() == sources.len()
}
#[cfg(target_arch = "wasm32")]
pub(super) fn validate_batch(_: &ProjectScope, _: &Batch, _: &[u64]) -> Result<()> {
    Err(error("native source-library admission is unavailable"))
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate_batch(scope: &ProjectScope, b: &Batch, sources: &[u64]) -> Result<()> {
    if b.authority != super::document_authority::Authority::MediaInputs {
        return Err(error("source-library work requires media authority, never document-state metadata"));
    }
    if !valid_sources(sources) {
        return Err(error("invalid retained explicit library selection"));
    }
    admission::validate_group(&b.operations, b.exact)?;
    if b.operations.iter().any(|op| op.params["item"].as_u64().is_none_or(|id| !sources.contains(&id))) {
        return Err(error("admission target was not requested in this source-library read"));
    }
    let expected = serde_json::to_value(scope).map_err(|e| error(e.to_string()))?;
    if b.operations.iter().any(|op| op.params.get("scope") != Some(&expected)) {
        return Err(error("every library admission must retain its exact independent Root identity"));
    }
    Ok(())
}
#[cfg(target_arch = "wasm32")]
pub(super) fn validate_transition(_: &filmcraft_project::Project, _: &filmcraft_project::Project, _: &Batch, _: &[Value]) -> Result<()> {
    Err(error("native source-library admission is unavailable"))
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate_transition(before: &filmcraft_project::Project, after: &filmcraft_project::Project, b: &Batch, results: &[Value]) -> Result<()> {
    admission::validate_results(&b.operations, results)?;
    let mut collateral = after.clone();
    for op in &b.operations {
        let id = op.params["item"].as_u64().ok_or_else(|| error("missing historical source ID"))?;
        let id = filmcraft_project::ItemId(id);
        let original = before.item(id).ok_or_else(|| error("historical source input is missing"))?;
        let bound = after.item(id).ok_or_else(|| error("historical admitted source is missing"))?;
        let media = original.as_media().ok_or_else(|| error("historical input is not imported media"))?;
        if original.metadata.contains_key(super::BINDING_KEY)
            || original.metadata.contains_key(admission::KEY)
            || media.offline
            || media.proxy.is_some()
            || media.interpret != filmcraft_project::Interpretation::default()
            || !matches!(&media.media,filmcraft_project::MediaRef::File{path} if Some(path.as_str())==op.params["path"].as_str())
        {
            return Err(error("historical admission does not start with the exact unbound online imported original"));
        }
        let marker =
            json!({"kind":"poema-native-source-binding","version":1,"ownerReceiptSha256":op.params["ownerReceiptSha256"],"generation":op.params["generation"]});
        let mut expected = original.clone();
        let binding: super::Binding = serde_json::from_value(op.params["binding"].clone()).map_err(|e| error(e.to_string()))?;
        expected.metadata.insert(super::BINDING_KEY.into(), serde_json::to_string(&binding).map_err(|e| error(e.to_string()))?);
        expected.metadata.insert(admission::KEY.into(), serde_json::to_string(&marker).map_err(|e| error(e.to_string()))?);
        if super::project_audit::value_bytes(&expected)? != super::project_audit::value_bytes(bound)? {
            return Err(error("historical source binding differs from its exact original-owner receipt or changes collateral media"));
        }
        collateral.items.insert(id, original.clone());
    }
    if super::project_audit::bytes(&collateral)? != super::project_audit::bytes(before)? {
        return Err(error("library admission cannot change timelines, bins or any other authored project state"));
    }
    Ok(())
}
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "library_plane_tests.rs"]
mod tests;
