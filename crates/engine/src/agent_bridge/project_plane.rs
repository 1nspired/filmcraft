//! Project-only COW reads and exact first-sequence publication, using the
//! existing bridge tickets, operation ledger, native services and history.
use super::{Batch, Operation, Ticket, bins, context, document_authority, fresh_snapshot, idle, media_guard, parse, project_audit, searches};
use crate::{
    EngineError, Result, Session,
    commands::bad,
    project_workspace::{self, ProjectScope},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
pub(super) const READ: &str = "agent.readProject";
pub(super) const SEQUENCE_SCOPE: &str = "agent.readProjectSequenceScope";
pub(super) const VALIDATE: &str = "agent.validateProjectRead";
pub(super) const CREATE: &str = "agent.createInitialSequence";
fn error(message: impl Into<String>) -> EngineError {
    bad(READ, message)
}
pub(super) fn require_scope(s: &Session, scope: &ProjectScope) -> Result<()> {
    if !project_workspace::valid_scope(scope) || project_workspace::identity(&s.project)?.is_none_or(|r| r.scope != *scope) {
        return Err(error("project is outside the exact saved owning-film/workspace identity"));
    }
    crate::editorial_workspace::association(&s.project)?;
    if s.project.items.len() > 100_000
        || s.state.active_sequence.is_some_and(|id| id.0 == 0 || id.0 > 9_007_199_254_740_991 || s.project.sequence(id).is_none())
    {
        return Err(error("invalid bounded project or active sequence"));
    }
    Ok(())
}
pub(super) fn basis(s: &Session) -> Result<String> {
    for summary in [bins::summary(s), searches::summary(s)] {
        if summary["status"] != "current" {
            return Err(error("complete project organization inventory is unavailable"));
        }
    }
    Ok(format!("{:x}", Sha256::digest(project_audit::bytes(&s.project)?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    token: String,
    scope: ProjectScope,
    project_path: String,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    if cfg!(target_arch = "wasm32") {
        return Err(error("project-plane native identity is unavailable on this engine"));
    }
    let r: Read = parse(READ, p)?;
    fresh_snapshot(s, &r.token)?;
    require_scope(s, &r.scope)?;
    if r.project_path.len() > 4096
        || !r.project_path.starts_with('/')
        || r.project_path.chars().any(char::is_control)
        || s.path.as_ref() != Some(&r.project_path)
    {
        return Err(error("an exact explicit saved project path is required"));
    }
    let sha = basis(s)?;
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = Value::Null;
    let result = json!({"status":"read","kind":"native_project_read","version":1,"protocol":1,"token":r.token,"revision":s.revision,"nativeSession":epoch,"scope":r.scope,
        "project":{"path":s.path,"name":s.project.name,"dirty":s.revision!=s.saved_revision},"sequence":s.state.active_sequence.map(|id|id.0),
        "editorStateSha256":super::project_clear::editor_hash(&s.state)?,
        "projectState":{"sha256":sha,"items":s.project.items.len(),"sequences":s.project.sequences().count()},"binState":bins::summary(s),"searchState":searches::summary(s),
        "capabilities":{"projectMetadata":true,"projectSequenceScope":true,"projectSequenceNavigation":cfg!(not(target_arch = "wasm32"))&&super::operation_cancellation_available(s),"projectHistoryInspection":cfg!(not(target_arch = "wasm32")),"projectHistoryProvenance":cfg!(not(target_arch = "wasm32")),"projectOrganization":true,"binSelectionGrouping":true,"projectClear":true,"projectFirstSequence":s.state.active_sequence.is_none()&&s.project.sequences().next().is_none(),
        "authoredDocumentEdits":true,"guardedUndo":true,"guardedRedo":true,"snapshotValidation":true,"projectHistoryPersistence":true,"guardedHistoryRead":true,"projectHistoryCheckpointOperations":true,"projectHistoryCheckpointRetention":true,"operationCancellation":super::operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS},
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    super::register_ticket(
        s,
        Ticket {
            token: r.token,
            revision: s.revision,
            sequence: s.state.active_sequence,
            project_scope: Some(r.scope),
            source_library: None,
            project: Arc::downgrade(&s.project),
            path: s.path.clone(),
            context: context(s)?,
            committed: None,
            after: None,
            after_sequence: None,
            redo_consumed: false,
            media: Arc::new(media_guard::Witnesses::default()),
            #[cfg(not(target_arch = "wasm32"))]
            preparation: None,
        },
    )?;
    Ok(result)
}
pub(super) fn validate_ticket(s: &Session, token: &str, scope: &ProjectScope) -> Result<()> {
    require_scope(s, scope)?;
    super::check_edit_ticket_basis(s, token)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("expired project read"))?;
    if t.source_library.is_some() || t.project_scope.as_ref() != Some(scope) {
        return Err(error("this read does not own the requested project plane"));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validate {
    token: String,
    scope: ProjectScope,
}
pub(super) fn validate(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Validate = parse(VALIDATE, p)?;
    validate_ticket(s, &r.token, &r.scope)?;
    Ok(
        json!({"status":"current","kind":"native_project_read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"sha256":basis(s)?,"sourceValidation":"not-performed","writeAuthority":false}),
    )
}
/// Exact existing owning association, not a guessed active/first item. This
/// metadata does not turn a project ticket into a qualified sequence read.
pub(super) fn sequence_scope(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Validate = parse(SEQUENCE_SCOPE, p)?;
    validate_ticket(s, &r.token, &r.scope)?;
    let association = crate::editorial_workspace::association(&s.project)?;
    let sequence_scope = association.map(|a| a.scope);
    if s.state
        .active_sequence
        .and_then(|id| s.project.item(id))
        .and_then(|item| item.metadata.get(crate::editorial_workspace::SEQUENCE_KEY))
        .is_some_and(|raw| raw.len() > 2048)
    {
        return Err(error("active sequence association exceeds its bounded metadata size"));
    }
    let owned = match (&sequence_scope, s.state.active_sequence) {
        (Some(scope), Some(id)) => super::sequences::belongs(&s.project, id, scope)?,
        _ => false,
    };
    let state = if sequence_scope.is_none() {
        if s.project.sequences().next().is_none() { "absent" } else { "unassociated" }
    } else if s.state.active_sequence.is_none() {
        "closed"
    } else if owned {
        "current"
    } else {
        "outside_scope"
    };
    #[cfg(not(target_arch = "wasm32"))]
    let epoch = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let epoch = Value::Null;
    Ok(json!({"status":"read","kind":"native_project_sequence_scope","version":1,"token":r.token,"revision":s.revision,"nativeSession":epoch,"scope":r.scope,
        "projectPath":s.path,"sequence":s.state.active_sequence.map(|id|id.0),"sequenceScope":sequence_scope,"state":state,"activeOwned":owned,"sha256":basis(s)?,
        "requiresFreshSequenceRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false}))
}
/// One finite project-only hand. Refuse all source/timeline/background doors
/// even when a project read happens to have an active sequence.
pub(super) fn command_scope(s: &Session, id: &str, p: &Value) -> Result<()> {
    super::library_plane::command_scope(s, id, p)?;
    let project = p
        .get("token")
        .and_then(Value::as_str)
        .and_then(|token| s.agent_bridge.tickets.iter().find(|t| t.token == token))
        .is_some_and(|t| t.project_scope.is_some() && t.source_library.is_none());
    if project
        && !matches!(
            id,
            READ | VALIDATE
                | SEQUENCE_SCOPE
                | super::project_navigation::COMMAND
                | "agent.readBins"
                | "agent.readSearchBins"
                | "agent.readHistory"
                | "agent.apply"
                | "agent.undo"
                | "agent.redo"
                | "agent.receipt"
                | "agent.cancelSnapshot"
                | "agent.saveHistoryCheckpoint"
                | "agent.restoreHistoryCheckpoint"
                | "agent.validateRetainedHistory"
                | "agent.inspectHistoryCheckpoint"
                | "agent.inspectCheckpointProvenance"
        )
    {
        return Err(error("project-plane metadata is not a source, timeline, background or persistence read; take the appropriate qualified read"));
    }
    Ok(())
}
pub(super) fn batch_scope(s: &Session, b: &Batch) -> Result<()> {
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == b.token).ok_or_else(|| error("unknown project batch read"))?;
    if let Some(scope) = &t.project_scope {
        if t.source_library.is_some() {
            return super::library_plane::validate_batch(scope, b, t.source_library.as_deref().ok_or_else(|| error("missing library selection"))?);
        }
        validate_project_batch(scope, b)?;
        require_scope(s, scope)?;
    } else if b.operations.iter().any(|op| op.command == CREATE || op.params.get("scope").is_some_and(|scope| scope.get("workspaceId").is_some())) {
        return Err(error("project-scoped gestures require a project-plane read"));
    }
    Ok(())
}
// Shared by live publication and historical validation. This checks only a
// closed metadata gesture; it never executes a stored recipe or qualifies media.
pub(super) fn validate_project_batch(scope: &ProjectScope, b: &Batch) -> Result<()> {
    if b.authority != document_authority::Authority::DocumentState
        || !b.exact
        || b.operations.len() != 1
        || b.validate_at_publication
        || b.admission_preparation.is_some()
    {
        return Err(error("project read requires one exact authored publication, without media preparation"));
    }
    let op = b.operations.first().ok_or_else(|| error("missing project gesture"))?;
    if !matches!(op.command.as_str(), bins::COMMAND | searches::COMMAND | CREATE | super::project_clear::COMMAND)
        || op.params.get("scope") != Some(&serde_json::to_value(scope).map_err(|e| error(e.to_string()))?)
    {
        return Err(error("project gesture must keep its exact project scope and finite metadata hand"));
    }
    Ok(())
}
pub(super) fn validate_history_transition(
    before: &filmcraft_project::Project,
    after: &filmcraft_project::Project,
    op: &Operation,
    result: &Value,
) -> Result<()> {
    if op.command == super::project_clear::COMMAND {
        return super::project_clear::validate_transition(before, after, op, result);
    }
    document_authority::transition(before, after)?;
    let mut collateral = after.clone();
    let (a, b, command, scope) = match op.command.as_str() {
        bins::COMMAND => {
            bins::validate_group_history(before, after, op, result)?;
            collateral.root = before.root.clone();
            collateral.next_id = before.next_id;
            let command = match op.params["edit"]["type"].as_str() {
                Some("create") => "file.newBin",
                Some("rename") => "project.renameBin",
                Some("move") => "project.moveToBin",
                Some("group") => "file.newBinFromSelection",
                _ => return Err(error("invalid historical bin gesture")),
            };
            (bins::project_summary(before), bins::project_summary(after), command, json!("physical_project_tree"))
        }
        searches::COMMAND => {
            collateral.search_bins = before.search_bins.clone();
            collateral.next_id = before.next_id;
            let command = match op.params["edit"]["type"].as_str() {
                Some("create") => "file.newSearchBin",
                Some("update") => "project.editSearchBin",
                Some("delete") => "project.deleteSearchBin",
                _ => return Err(error("invalid historical query gesture")),
            };
            (searches::project_summary(before), searches::project_summary(after), command, json!("saved_search_bins"))
        }
        CREATE => {
            let r = creation(op)?;
            let c = r.sequence;
            let id = filmcraft_project::ItemId(
                result["sequence"]
                    .as_u64()
                    .filter(|id| (1..=9_007_199_254_740_991).contains(id))
                    .ok_or_else(|| error("missing exact initial sequence receipt"))?,
            );
            let q = after.sequence(id).ok_or_else(|| error("initial sequence snapshot is missing"))?;
            if before.sequences().next().is_some()
                || before.items.contains_key(&id)
                || after.sequences().count() != 1
                || after.items.len() != before.items.len().saturating_add(1)
                || after.root.children.iter().filter(|e| matches!(e,filmcraft_project::BinEntry::Item(i) if *i==id)).count() != 1
                || after.item(id).is_none_or(|i| i.name != c.name)
                || q.settings.width != c.width
                || q.settings.height != c.height
                || q.settings.frame_rate != filmcraft_time::FrameRate::new(i64::from(c.frame_rate.num), i64::from(c.frame_rate.den))
                || q.settings.sample_rate != c.sample_rate
                || Some(q.settings.audio_master) != crate::mixer::channels_from(&c.mix)
                || q.video_tracks.len() != c.video as usize
                || q.audio_tracks.len() != c.audio as usize
                || q.audio_tracks.iter().any(|t| Some(t.channels) != crate::mixer::channels_from(&c.track_type))
                || q.all_tracks().any(|t| !t.items.is_empty() || !t.transitions.is_empty() || !t.effects.is_empty())
                || !q.caption_tracks.is_empty()
                || !q.markers.is_empty()
                || !q.submix_tracks.is_empty()
                || !q.master_effects.is_empty()
                || result.get("previousSequence") != Some(&Value::Null)
            {
                return Err(error("historical first sequence differs from its exact empty authored settings"));
            }
            collateral.items.remove(&id);
            collateral.root.children.retain(|e| !matches!(e,filmcraft_project::BinEntry::Item(i) if *i==id));
            collateral.next_id = before.next_id;
            let digest = |p| -> Result<Value> { Ok(json!({"status":"current","sha256":format!("{:x}",Sha256::digest(project_audit::bytes(p)?))})) };
            (digest(before)?, digest(after)?, "file.newSequence", serde_json::to_value(r.scope).map_err(|e| error(e.to_string()))?)
        }
        _ => {
            return Err(error("historical project publication is outside the finite metadata hand"));
        }
    };
    if a["status"] != "current"
        || b["status"] != "current"
        || a["sha256"] == b["sha256"]
        || op.params.get("beforeSha256") != a.get("sha256")
        || result.get("beforeSha256") != a.get("sha256")
        || result.get("afterSha256") != b.get("sha256")
        || result["humanCommand"] != command
        || result["scope"] != scope
        || project_audit::bytes(&collateral)? != project_audit::bytes(before)?
    {
        return Err(error("historical project receipt/digest or unrelated project collateral differs"));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rate {
    num: u32,
    den: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Initial {
    name: String,
    width: u32,
    height: u32,
    frame_rate: Rate,
    sample_rate: u32,
    mix: String,
    track_type: String,
    video: u32,
    audio: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    scope: ProjectScope,
    before_sha256: String,
    sequence: Initial,
}
fn creation(op: &Operation) -> Result<Create> {
    let r: Create = parse(CREATE, &op.params)?;
    let c = &r.sequence;
    if !project_workspace::valid_scope(&r.scope)
        || r.before_sha256.len() != 64
        || !r.before_sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || c.name.trim().is_empty()
        || c.name.len() > 1024
        || c.name.chars().any(char::is_control)
        || c.video > 256
        || c.audio > 256
        || c.frame_rate.num == 0
        || c.frame_rate.den == 0
        || crate::mixer::channels_from(&c.mix).is_none()
        || crate::mixer::channels_from(&c.track_type).is_none()
    {
        return Err(bad(CREATE, "closed bounded exact initial sequence settings and complete project basis required"));
    }
    filmcraft_project::SequenceSettings {
        width: c.width,
        height: c.height,
        frame_rate: filmcraft_time::FrameRate::new(i64::from(c.frame_rate.num), i64::from(c.frame_rate.den)),
        sample_rate: c.sample_rate,
        ..Default::default()
    }
    .validate()
    .map_err(|e| bad(CREATE, e))?;
    Ok(r)
}
pub(super) fn validate_create(op: &Operation) -> Result<()> {
    creation(op).map(|_| ())
}
pub(super) fn create(s: &mut Session, op: &Operation) -> Result<Value> {
    let r = creation(op)?;
    require_scope(s, &r.scope)?;
    idle(s)?;
    if s.state.active_sequence.is_some() || s.project.sequences().next().is_some() || basis(s)? != r.before_sha256 {
        return Err(bad(CREATE, "first-sequence basis changed or a sequence already exists"));
    }
    let before = s.project.clone();
    let editor = s.state.clone();
    let c = r.sequence;
    let result=s.execute("file.newSequence",json!({"name":c.name,"width":c.width,"height":c.height,"fps":{"num":c.frame_rate.num,"den":c.frame_rate.den},"sampleRate":c.sample_rate,"mix":c.mix,"trackType":c.track_type,"video":c.video,"audio":c.audio}))?;
    let id = filmcraft_project::ItemId(result["sequence"].as_u64().ok_or_else(|| bad(CREATE, "shared human sequence returned no exact ID"))?);
    let mut expected = (*s.project).clone();
    expected.items.remove(&id);
    expected.root.children.retain(|entry| !matches!(entry,filmcraft_project::BinEntry::Item(i) if *i==id));
    expected.next_id = before.next_id;
    let mut after_editor = s.state.clone();
    after_editor.active_sequence = editor.active_sequence;
    after_editor.open_sequences = editor.open_sequences.clone();
    if project_audit::bytes(&expected)? != project_audit::bytes(&before)?
        || project_audit::value_bytes(&after_editor)? != project_audit::value_bytes(&editor)?
        || s.project.sequence(id).is_none_or(|q| q.all_tracks().any(|t| !t.items.is_empty()))
    {
        return Err(bad(CREATE, "initial sequence changed existing project/editor content"));
    }
    Ok(
        json!({"sequence":id.0,"previousSequence":null,"scope":r.scope,"beforeSha256":r.before_sha256,"afterSha256":basis(s)?,"humanCommand":"file.newSequence"}),
    )
}
