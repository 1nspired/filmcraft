//! Four finite Root view operations using original human state/history services.
use super::{COMMAND, Document, SequenceViewCommand, encode_document, error, valid_hash};
use crate::agent_bridge::{
    compound_history::{Purpose, Retained},
    compound_publish::{self, Effect},
    editor_proof, history_checkpoint, media_guard, parse, project_audit, project_clear, project_plane,
};
use crate::{
    EditorState, Result, Session, commands,
    project_workspace::{self, ProjectScope},
};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
const LIMIT: usize = 4 * 1024 * 1024;
const EDITOR_KIND: &str = "native_sequence_view_editor_proof";
const ORIGIN_EDITOR_KIND: &str = "native_sequence_view_original_editor_proof";
pub(super) const ACTION: &str = "sequence_view_change";
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Operation {
    Open { item: u64, owned: bool },
    Close { item: u64, owned: bool },
    CloseOthers { item: u64, owned: bool },
    MoveTab { item: u64, owned: bool, index: u64 },
}
impl Operation {
    fn command(&self) -> SequenceViewCommand {
        match self {
            Self::Open { .. } => SequenceViewCommand::Open,
            Self::Close { .. } => SequenceViewCommand::Close,
            Self::CloseOthers { .. } => SequenceViewCommand::CloseOthers,
            Self::MoveTab { .. } => SequenceViewCommand::MoveTab,
        }
    }
    fn item(&self) -> ItemId {
        ItemId(match self {
            Self::Open { item, .. } | Self::Close { item, .. } | Self::CloseOthers { item, .. } | Self::MoveTab { item, .. } => *item,
        })
    }
    fn owned(&self) -> bool {
        match self {
            Self::Open { owned, .. } | Self::Close { owned, .. } | Self::CloseOthers { owned, .. } | Self::MoveTab { owned, .. } => *owned,
        }
    }
    fn effect(&self) -> Effect<'_> {
        Effect::SequenceView {
            command: self.command(),
            item: self.item(),
            index: match self {
                Self::MoveTab { index, .. } => Some(*index),
                _ => None,
            },
        }
    }
    fn preview(&self, project: &Project, state: &mut EditorState) -> Result<Value> {
        match self {
            Self::Open { .. } => {
                commands::open_sequence_state(project, state, self.item())?;
                Ok(Value::Null)
            }
            Self::Close { .. } => {
                commands::close_sequence_state(state, self.item());
                Ok(Value::Null)
            }
            Self::CloseOthers { .. } => {
                let (closed, _) = commands::close_other_sequence_states(state, self.item())?;
                Ok(json!({"closed":closed}))
            }
            Self::MoveTab { index, .. } => {
                let index = commands::move_sequence_tab_state(state, self.item(), *index)?;
                Ok(json!({"index":index}))
            }
        }
    }
    fn require_observed(&self, document: &Value) -> Result<()> {
        let row = document["sequences"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["item"].as_u64() == Some(self.item().0)))
            .ok_or_else(|| error("view target is absent from the complete original Root inventory"))?;
        if row["owned"].as_bool() != Some(self.owned()) {
            return Err(error("the target's observed Root realm membership changed"));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: u8,
    token: String,
    scope: ProjectScope,
    before_sha256: String,
    before_editor_sha256: String,
    operation: Operation,
}
fn request(p: &Value) -> Result<Request> {
    let r: Request = parse(COMMAND, p)?;
    if r.version != 1
        || !crate::agent_bridge::token_ok(&r.token)
        || !project_workspace::valid_scope(&r.scope)
        || !valid_hash(&r.before_sha256)
        || !valid_hash(&r.before_editor_sha256)
        || !(1..=9_007_199_254_740_991).contains(&r.operation.item().0)
    {
        return Err(error("finite version-one view operation, exact Root and original native digests required"));
    }
    Ok(r)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OriginHeader {
    version: u8,
    revision: u64,
    saved_revision: u64,
    epoch: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    kind: String,
    version: u8,
    sha256: String,
    bytes: usize,
    data: String,
}
fn origin_header(revision: u64, saved_revision: u64, epoch: &str) -> Result<Value> {
    let raw = history_checkpoint::bytes(&OriginHeader { version: 1, revision, saved_revision, epoch: epoch.into() }, LIMIT)?;
    serde_json::to_value(Origin {
        kind: "native_sequence_view_original_basis".into(),
        version: 1,
        sha256: history_checkpoint::digest(&raw),
        bytes: raw.len(),
        data: String::from_utf8(raw).map_err(|e| error(e.to_string()))?,
    })
    .map_err(|e| error(e.to_string()))
}
fn decode_origin(value: &Value) -> Result<OriginHeader> {
    let o: Origin = parse(COMMAND, value)?;
    if o.kind != "native_sequence_view_original_basis"
        || o.version != 1
        || o.bytes == 0
        || o.bytes > LIMIT
        || o.data.len() != o.bytes
        || history_checkpoint::digest(o.data.as_bytes()) != o.sha256
    {
        return Err(error("changed original view basis"));
    }
    let h: OriginHeader = serde_json::from_str(&o.data).map_err(|e| error(e.to_string()))?;
    if h.version != 1 || !filmcraft_project::ClipIdentity::valid_id(&h.epoch) || origin_header(h.revision, h.saved_revision, &h.epoch)? != *value {
        return Err(error("noncanonical original view basis"));
    }
    Ok(h)
}
fn read_document(
    retained: &Retained,
    command: &str,
    editor: &EditorState,
    clipboard: &str,
    path: &str,
    r: &Request,
    epoch: &str,
    revision: u64,
    saved: u64,
) -> Result<Value> {
    let h = retained.validate(command)?;
    h.validate_editor_basis(editor.active_sequence, &editor.edit_points, editor.trim_shift)?;
    let summary = retained.evidence.default_forecast(&retained.projects)?;
    let (dynamic, around) = retained.evidence.playback_kind();
    encode_document(Document {
        project: h.before_project(),
        state: editor,
        clipboard_sha256: clipboard,
        history: h.before_history(),
        path,
        scope: &r.scope,
        token: &r.token,
        epoch,
        revision,
        saved_revision: saved,
        active: dynamic || around,
        summary: &summary,
        change: true,
        cancellation: true,
    })
}
fn publication(
    retained: &Retained,
    before: &EditorState,
    clipboard: &str,
    path: &str,
    r: &Request,
    epoch: &str,
    revision: u64,
    check: Option<&media_guard::Check>,
) -> Result<Value> {
    let h = retained.validate(r.operation.command().id())?;
    h.validate_editor_basis(before.active_sequence, &before.edit_points, before.trim_shift)?;
    let mut after = before.clone();
    after.trim_shift = h.trim_shift_after();
    let native_result = r.operation.preview(h.after_project(), &mut after)?;
    let before_hash = project_clear::editor_hash_with(before, clipboard)?;
    let after_hash = project_clear::editor_hash_with(&after, clipboard)?;
    let x = h.summary();
    let same = Arc::ptr_eq(h.before_project(), h.after_project());
    let result = json!({"status":"navigated","kind":"native_sequence_view_change","version":1,"action":ACTION,"token":r.token,"scope":r.scope,"nativeSession":epoch,"projectPath":path,
        "beforeRevision":revision,"revision":revision,"previousSequence":before.active_sequence.map(|id|id.0),"sequence":after.active_sequence.map(|id|id.0),"operation":r.operation,"humanCommand":r.operation.command().id(),"humanResult":native_result,
        "beforeProjectSha256":project_plane::project_basis(h.before_project())?,"sha256":project_plane::project_basis(h.after_project())?,"beforeEditorSha256":before_hash,"afterEditorSha256":after_hash,
        "beforeHistory":super::super::compound_history::history_state(h.before_project(),h.before_history())?,"afterHistory":super::super::compound_history::history_state(h.after_project(),h.after_history())?,
        "projectUnchanged":same,"historyUnchanged":!x.dynamic_committed,"changed":before_hash!=after_hash||!same||x.dynamic_finished||x.loop_stopped,
        "trim":{"settlementApplied":true,"dynamicFinished":x.dynamic_finished,"dynamicCommitted":x.dynamic_committed,"undoEntryRetained":x.undo_entry_retained,"redoDiscarded":x.redo_discarded,"loopStopped":x.loop_stopped,"restoredBeforeProject":x.restored_before_project,"offsetTicks":x.offset.0.to_string(),"label":x.label},
        "agentTimelineCommits":0,"agentUndoOwned":false,"requiresFreshProjectRead":true,"requiresFreshSequenceRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,
        "editorProof":editor_proof::encode(before,&after,clipboard,EDITOR_KIND,LIMIT,COMMAND)?,"historyProof":retained.proof_for(r.operation.command().id(),Purpose::SequenceView,check)?});
    if project_audit::value_bytes(&result)?.len() > LIMIT {
        return Err(error("complete view publication proof exceeds4MiB"));
    }
    Ok(result)
}
fn result(
    original: Value,
    current: Value,
    origin: Value,
    original_editor: editor_proof::OpaqueProof,
    original_history: Value,
    publication: Value,
    saved_revision: u64,
) -> Result<Value> {
    let value = json!({"status":"navigated","kind":"native_sequence_view_change","version":2,"action":ACTION,"token":publication["token"],"scope":publication["scope"],"nativeSession":publication["nativeSession"],"projectPath":publication["projectPath"],
        "beforeRevision":publication["beforeRevision"],"revision":publication["revision"],"currentSavedRevision":saved_revision,"previousSequence":publication["previousSequence"],"sequence":publication["sequence"],"humanCommand":publication["humanCommand"],"operation":publication["operation"],
        "original":original,"current":current,"originalBasis":origin,"originalEditorProof":original_editor,"originalHistoryProof":original_history,"publication":publication,
        "agentTimelineCommits":0,"agentUndoOwned":false,"requiresFreshProjectRead":true,"requiresFreshSequenceRead":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if project_audit::value_bytes(&value)?.len() > LIMIT.saturating_mul(4) {
        return Err(error("complete original/current view receipt exceeds16MiB"));
    }
    Ok(value)
}
pub(in crate::agent_bridge) fn is_result(v: &Value) -> bool {
    v.get("action").and_then(Value::as_str) == Some(ACTION)
}
pub(in crate::agent_bridge) fn retained(t: &crate::agent_bridge::Ticket) -> Result<&Retained> {
    t.sequence_views.as_ref().and_then(|o| o.outcome.as_ref()).ok_or_else(|| error("view receipt lost its immutable history evidence"))
}
pub(in crate::agent_bridge) fn navigate(s: &mut Session, value: &Value) -> Result<Value> {
    if !crate::agent_bridge::operation_cancellation_available(s) {
        return Err(error("view publication requires the independent native Stop transport"));
    }
    let r = request(value)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("prepare the original Root view change first"))?;
    if let Some((old, outcome)) = &t.committed {
        return if old == value && is_result(outcome) { Ok(outcome.clone()) } else { Err(error("view read token already consumed by a different operation")) };
    }
    let owner = t.sequence_views.as_ref().filter(|o| o.change).ok_or_else(|| error("readonly, old Root and editor-setting tickets cannot publish a view"))?;
    if t.project_scope.as_ref() != Some(&r.scope)
        || t.source_library.is_some()
        || t.editor_control.is_some()
        || t.path != s.path
        || owner.original["projectState"]["sha256"] != r.before_sha256
        || owner.original["editorStateSha256"] != r.before_editor_sha256
    {
        return Err(error("original view identity or original read digests changed"));
    }
    project_plane::require_scope(s, &r.scope)?;
    let epoch = s.agent_bridge.epoch.as_deref().ok_or_else(|| error("missing original native epoch"))?.to_owned();
    if owner.original["nativeSession"] != epoch {
        return Err(error("original view epoch changed"));
    }
    let prepared = owner.current(s, r.operation.command())?;
    let mut retained = Retained::capture_settled(&prepared)?;
    let original_prepared = owner.original_settlement.as_ref().ok_or_else(|| error("a restored record cannot prepare a new view"))?;
    let original_retained = Retained::capture_settled(original_prepared)?;
    let original_editor = owner.original_editor.as_deref().ok_or_else(|| error("original native editor capture missing"))?.clone();
    let original_clipboard = project_clear::clipboard_hash(&original_editor)?;
    let original = owner.original.clone();
    let original_revision = original["revision"].as_u64().ok_or_else(|| error("original revision missing"))?;
    let original_saved = owner.original_saved_revision;
    let before = s.state.clone();
    let clipboard = project_clear::clipboard_hash(&before)?;
    let path = s.path.clone().ok_or_else(|| error("saved Root path required"))?;
    let original_doc =
        read_document(&original_retained, "sequence.open", &original_editor, &original_clipboard, &path, &r, &epoch, original_revision, original_saved)?;
    if original_doc != original {
        return Err(error("original view read differs from the original native capture"));
    }
    let current = read_document(&retained, r.operation.command().id(), &before, &clipboard, &path, &r, &epoch, s.revision, s.saved_revision)?;
    r.operation.require_observed(&original)?;
    r.operation.require_observed(&current)?;
    original_retained.evidence.validate_sequence_view_following_to(&original_retained.projects, &retained.evidence, &retained.projects)?;
    compound_publish::validate(s, &r.operation.effect())?;
    let check = s.agent_bridge.operations.check();
    let p = publication(&retained, &before, &clipboard, &path, &r, &epoch, s.revision, check.as_deref())?;
    let origin = origin_header(original_revision, original_saved, &epoch)?;
    let origin_editor = editor_proof::encode(&original_editor, &original_editor, &original_clipboard, ORIGIN_EDITOR_KIND, LIMIT, COMMAND)?;
    let original_history = original_retained.proof_for("sequence.open", Purpose::SequenceView, check.as_deref())?;
    let receipt = result(original, current, origin, origin_editor, original_history, p, s.saved_revision)?;
    retained.original = Some(Box::new(original_retained));
    // Validate the same immutable historical proof before the effect, not a second publisher.
    let mut pool = Vec::new();
    for p in retained.projects().iter().chain(retained.original.as_ref().ok_or_else(|| error("original view evidence missing"))?.projects()) {
        if !pool.iter().any(|q| Arc::ptr_eq(p, q)) {
            pool.push(p.clone());
        }
    }
    let mapping = retained.mapping(&pool)?;
    let original_mapping = retained.original.as_ref().ok_or_else(|| error("original view evidence missing"))?.mapping(&pool)?;
    let historical = retained.validate(r.operation.command().id())?;
    validate_history(
        &pool,
        &mapping,
        &original_mapping,
        historical.before_project(),
        historical.after_project(),
        &path,
        &r.scope,
        &r.token,
        s.revision,
        before.active_sequence.map(|id| id.0),
        receipt["sequence"].as_u64(),
        value,
        &receipt,
        check.as_deref(),
    )?;
    let before_project = Arc::downgrade(&s.project);
    let revision = s.revision;
    let context = crate::agent_bridge::context(s)?;
    compound_publish::publish(s, &r.token, prepared, &r.operation.effect())?;
    let after = Arc::downgrade(&s.project);
    let after_sequence = s.state.active_sequence;
    let t = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token).ok_or_else(|| error("published view ticket disappeared"))?;
    let owner = t.sequence_views.as_mut().ok_or_else(|| error("published view purpose disappeared"))?;
    owner.following = None;
    owner.original_settlement = None;
    owner.original_editor = None;
    owner.outcome = Some(retained);
    t.project = before_project;
    t.revision = revision;
    t.context = context;
    t.committed = Some((value.clone(), receipt.clone()));
    t.after = Some(after);
    t.after_sequence = after_sequence;
    Ok(receipt)
}
pub(in crate::agent_bridge) fn validate_history(
    pool: &[Arc<Project>],
    mapping: &[usize],
    original_mapping: &[usize],
    before: &Arc<Project>,
    after: &Arc<Project>,
    path: &str,
    scope: &ProjectScope,
    token: &str,
    revision: u64,
    before_sequence: Option<u64>,
    after_sequence: Option<u64>,
    value: &Value,
    receipt: &Value,
    check: Option<&media_guard::Check>,
) -> Result<Retained> {
    let r = request(value)?;
    if &r.scope != scope
        || r.token != token
        || receipt["version"] != 2
        || receipt["beforeRevision"] != revision
        || receipt["revision"] != revision
        || receipt["previousSequence"] != json!(before_sequence)
        || receipt["sequence"] != json!(after_sequence)
        || project_audit::value_bytes(receipt)?.len() > LIMIT.saturating_mul(4)
    {
        return Err(error("foreign historical view identity, version or bound"));
    }
    let p = receipt.get("publication").ok_or_else(|| error("view publication proof missing"))?;
    let mut retained = Retained::from_for(
        p.get("historyProof").ok_or_else(|| error("view history missing"))?,
        pool,
        mapping,
        r.operation.command().id(),
        Purpose::SequenceView,
        check,
    )?;
    let original = Retained::from_for(
        receipt.get("originalHistoryProof").ok_or_else(|| error("original view history missing"))?,
        pool,
        original_mapping,
        "sequence.open",
        Purpose::SequenceView,
        check,
    )?;
    original.evidence.validate_sequence_view_following_to(&original.projects, &retained.evidence, &retained.projects)?;
    let h = retained.validate(r.operation.command().id())?;
    if !Arc::ptr_eq(h.before_project(), before) || !Arc::ptr_eq(h.after_project(), after) {
        return Err(error("view snapshot mapping collapsed or detached immutable roots"));
    }
    let decoded = editor_proof::decode(p.get("editorProof").ok_or_else(|| error("view editor proof missing"))?, EDITOR_KIND, LIMIT, COMMAND)?;
    let origin_editor = editor_proof::decode(
        receipt.get("originalEditorProof").ok_or_else(|| error("original view editor proof missing"))?,
        ORIGIN_EDITOR_KIND,
        LIMIT,
        COMMAND,
    )?;
    editor_proof::require_after(&origin_editor.after_editor, &origin_editor.before, COMMAND)?;
    let header = decode_origin(receipt.get("originalBasis").ok_or_else(|| error("original view basis missing"))?)?;
    let epoch = receipt["nativeSession"].as_str().filter(|v| filmcraft_project::ClipIdentity::valid_id(v)).ok_or_else(|| error("view epoch missing"))?;
    if header.epoch != epoch || header.revision > revision || decoded.before.active_sequence.map(|id| id.0) != before_sequence {
        return Err(error("historical view epoch, revision or original editor mismatch"));
    }
    let original_doc = read_document(
        &original,
        "sequence.open",
        &origin_editor.before,
        &origin_editor.clipboard_sha256,
        path,
        &r,
        epoch,
        header.revision,
        header.saved_revision,
    )?;
    if original_doc["projectState"]["sha256"] != r.before_sha256 || original_doc["editorStateSha256"] != r.before_editor_sha256 {
        return Err(error("historical original view digests differ"));
    }
    let current_saved = receipt["current"]["project"]["dirty"].as_bool().ok_or_else(|| error("current view dirty flag missing"))?;
    // Exact saved revision is native-only and encoded separately below; never infer it from dirty.
    let saved = receipt["currentSavedRevision"].as_u64().ok_or_else(|| error("current saved native revision missing"))?;
    let current = read_document(&retained, r.operation.command().id(), &decoded.before, &decoded.clipboard_sha256, path, &r, epoch, revision, saved)?;
    if current["project"]["dirty"] != current_saved {
        return Err(error("current saved view revision differs"));
    }
    r.operation.require_observed(&original_doc)?;
    r.operation.require_observed(&current)?;
    let mut expected_editor = decoded.before.clone();
    expected_editor.trim_shift = h.trim_shift_after();
    r.operation.preview(h.after_project(), &mut expected_editor)?;
    editor_proof::require_after(&decoded.after_editor, &expected_editor, COMMAND)?;
    let expected_publication = publication(&retained, &decoded.before, &decoded.clipboard_sha256, path, &r, epoch, revision, check)?;
    let expected = result(
        original_doc,
        current,
        origin_header(header.revision, header.saved_revision, epoch)?,
        editor_proof::encode(&origin_editor.before, &origin_editor.before, &origin_editor.clipboard_sha256, ORIGIN_EDITOR_KIND, LIMIT, COMMAND)?,
        original.proof_for("sequence.open", Purpose::SequenceView, check)?,
        expected_publication,
        saved,
    )?;
    if expected != *receipt {
        return Err(error("historical view receipt differs from original native services and complete graph"));
    }
    retained.original = Some(Box::new(original));
    Ok(retained)
}

#[cfg(test)]
mod tests;
