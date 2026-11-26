//! Experimental, deliberately narrow agent boundary. A read ticket belongs to this live
//! session, not a filename. Stage only audited in-memory edits, then publish ONE project
//! snapshot: intermediate edits never reach playback, persistence or the human's undo stack.
//! This is not a general transaction wrapper for arbitrary commands or external side effects.

use std::collections::VecDeque;

use filmcraft_project::{ClipId, ItemId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

#[cfg(not(target_arch = "wasm32"))]
mod admission;
#[cfg(not(target_arch = "wasm32"))]
mod admission_jobs;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod admission_tests;
mod advanced_trim;
#[cfg(test)]
mod advanced_trim_tests;
#[cfg(not(target_arch = "wasm32"))]
mod application_admission;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod resource_preflight;
#[cfg(all(not(target_arch = "wasm32"), not(any(target_vendor = "apple", target_os = "linux"))))]
mod resource_preflight {
    use super::*;
    pub(super) fn held(_: &Session) -> bool { false }
    pub(super) fn command_allowed(_: &Session, _: &str) -> Result<()> { Ok(()) }
    pub(super) fn commands() -> Vec<CommandSpec> { Vec::new() }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod application_admission_tests;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod application_tab_scope;
mod arrange;
#[cfg(test)]
mod arrange_tests;
mod assembly;
#[cfg(test)]
mod assembly_tests;
#[cfg(test)]
mod bin_tests;
mod bins;
#[cfg(not(target_arch = "wasm32"))]
mod candidates;
#[cfg(test)]
mod caption_font_tests;
mod caption_fonts;
mod caption_ripple;
#[cfg(test)]
mod caption_tests;
mod caption_transfer;
#[cfg(test)]
mod caption_transfer_tests;
mod captions;
#[cfg(test)]
mod clip_mixer_parameter_tests;
#[cfg(not(target_arch = "wasm32"))]
mod compound_history;
#[cfg(not(target_arch = "wasm32"))]
mod compound_publish;
mod condense;
#[cfg(test)]
mod condense_caption_tests;
mod condense_captions;
#[cfg(test)]
mod condense_inputs_tests;
mod condense_restore;
#[cfg(test)]
mod condense_tests;
mod document_authority;
#[cfg(not(target_arch = "wasm32"))]
mod document_projection;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod document_projection_tests;
#[cfg(not(target_arch = "wasm32"))]
mod editor_compound;
mod editor_control;
mod editor_proof;
mod editor_setting;
mod editor_settings;
mod effect_stack;
#[cfg(test)]
mod effect_stack_tests;
#[cfg(not(target_arch = "wasm32"))]
mod export;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod export_tests;
mod finish;
#[cfg(test)]
mod finish_tests;
mod finishing;
#[cfg(test)]
mod finishing_tests;
mod font_catalog;
#[cfg(test)]
mod font_catalog_tests;
#[cfg(not(target_arch = "wasm32"))]
mod font_resource_jobs;
mod font_usage;
#[cfg(test)]
mod font_usage_tests;
mod graphics;
mod graphics_layout;
mod graphics_pins;
#[cfg(test)]
mod graphics_tests;
mod history;
#[cfg(not(target_arch = "wasm32"))]
mod history_checkpoint;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod history_checkpoint_tests;
#[cfg(test)]
mod history_tests;
#[cfg(not(target_arch = "wasm32"))]
mod inspection;
mod library_plane;
#[cfg(test)]
mod marker_tests;
mod markers;
mod media_guard;
mod media_scope;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod media_scope_tests;
mod mixer_automation;
mod mixer_automation_edit;
mod mixer_routes;
mod multicam_production;
#[cfg(not(target_arch = "wasm32"))]
mod multicam_read;
mod music;
#[cfg(test)]
mod music_tests;
#[cfg(not(target_arch = "wasm32"))]
pub mod operation_control;
mod organization_scope;
mod parameter;
#[cfg(test)]
mod parameter_tests;
mod picture_inputs;
#[cfg(test)]
mod picture_inputs_tests;
mod picture_place;
#[cfg(test)]
mod picture_place_tests;
mod preview;
#[cfg(test)]
mod preview_tests;
mod project_audit;
mod project_clear;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_clear_tests;
#[cfg(test)]
mod project_font_readback_tests;
mod project_font_replacement;
#[cfg(test)]
mod project_font_replacement_tests;
#[cfg(test)]
mod project_font_resource_tests;
mod project_font_resources;
mod project_font_write;
#[cfg(test)]
mod project_font_write_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_history_tests;
mod project_navigation;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_navigation_tests;
mod project_plane;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_plane_tests;
mod project_save_plan;
#[cfg(test)]
mod project_save_plan_tests;
mod reaction_reference_inputs;
#[cfg(test)]
mod reaction_reference_inputs_tests;
mod reaction_references;
#[cfg(test)]
mod reaction_references_tests;
#[cfg(test)]
mod reaction_tests;
mod reactions;
#[cfg(not(target_arch = "wasm32"))]
mod read_jobs;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod read_projection_tests;
mod retime;
#[cfg(test)]
mod retime_tests;
#[cfg(not(target_arch = "wasm32"))]
mod save_operation;
mod save_plan_jobs;
#[cfg(test)]
mod search_tests;
mod searches;
#[cfg(not(target_arch = "wasm32"))]
mod sequence_payload;
#[cfg(not(target_arch = "wasm32"))]
mod sequence_publish;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sequence_publish_tests;
#[cfg(test)]
mod sequence_tests;
#[cfg(not(target_arch = "wasm32"))]
mod sequence_views;
mod sequences;
#[cfg(not(target_arch = "wasm32"))]
mod snapshot_jobs;
#[cfg(not(target_arch = "wasm32"))]
mod source_audio;
mod source_catalog;
mod source_edit;
#[cfg(test)]
mod source_edit_tests;
mod source_events;
#[cfg(test)]
mod source_events_tests;
mod source_frames;
mod source_selection;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod source_selection_tests;
mod track_inserts;
#[cfg(test)]
mod track_tests;
mod tracks;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod workspace_ownership_tests;
#[cfg(not(target_arch = "wasm32"))]
mod workspace_update;
#[cfg(all(not(target_arch = "wasm32"), any(target_vendor = "apple", target_os = "linux")))]
pub(crate) use workspace_update::{PreferenceContinuation as ColdPreferenceContinuation, ServiceBindings as ColdServiceBindings};
#[cfg(all(test, not(target_arch = "wasm32")))]
mod workspace_update_bundle_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod workspace_update_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod legacy_identity_tests;

const BINDING_KEY: &str = "poema.binding.v1";
const TICKET_LIMIT: usize = 256;
const OP_LIMIT: usize = 32;

#[derive(Default)]
pub(crate) struct Bridge {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    resource_preflight: resource_preflight::State,
    #[cfg(not(target_arch = "wasm32"))]
    workspace_update: Option<workspace_update::Held>,
    #[cfg(not(target_arch = "wasm32"))]
    application_admission: Option<application_admission::Binding>,
    #[cfg(not(target_arch = "wasm32"))]
    closed_workspace_updates: VecDeque<workspace_update::Closed>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) operations: operation_control::AgentOperationControl,
    #[cfg(not(target_arch = "wasm32"))]
    epoch: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    history_checkpoints: VecDeque<history_checkpoint::LiveGuard>,
    #[cfg(not(target_arch = "wasm32"))]
    history_bases: VecDeque<history_checkpoint::Basis>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) application_history: Vec<std::sync::Arc<history_checkpoint::application_archive::Record>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) application_ui: Option<std::sync::Arc<history_checkpoint::application_ui::Record>>,
    // Same-worker physical data only. Never serialized, restored or a grant.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) pending_application_capture: Option<ApplicationSnapshotCapture>,
    tickets: VecDeque<Ticket>,
    #[cfg(not(target_arch = "wasm32"))]
    sequence_payloads: VecDeque<sequence_payload::Record>,
    head: Option<(u64, std::sync::Weak<filmcraft_project::Project>)>,
    media: Option<std::sync::Arc<media_guard::Witnesses>>,
    #[cfg(not(target_arch = "wasm32"))]
    inspections: VecDeque<inspection::Job>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_inspections: VecDeque<String>,
    #[cfg(not(target_arch = "wasm32"))]
    candidates: VecDeque<candidates::Record>,
    #[cfg(not(target_arch = "wasm32"))]
    snapshot_jobs: VecDeque<snapshot_jobs::Job>,
    #[cfg(not(target_arch = "wasm32"))]
    read_jobs: VecDeque<read_jobs::Job>,
    #[cfg(not(target_arch = "wasm32"))]
    font_resources: VecDeque<font_resource_jobs::Job>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_font_resources: VecDeque<String>,
    save_plans: VecDeque<save_plan_jobs::Job>,
    stopped_save_plans: VecDeque<String>,
    #[cfg(not(target_arch = "wasm32"))]
    save_operation: Option<save_operation::Owner>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    manager_save_origin: Option<std::sync::Arc<save_operation::managed_dispatch::Origin>>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    manager_save_startup: Option<std::sync::Arc<save_operation::managed_dispatch::Origin>>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_save_operations: VecDeque<save_operation::Request>,
    #[cfg(not(target_arch = "wasm32"))]
    admission_jobs: VecDeque<admission_jobs::Job>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_admissions: VecDeque<String>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_read_jobs: VecDeque<String>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_snapshots: VecDeque<String>,
    #[cfg(not(target_arch = "wasm32"))]
    exports: VecDeque<export::Record>,
    #[cfg(not(target_arch = "wasm32"))]
    stopped_exports: VecDeque<String>,
}

struct Ticket {
    token: String,
    revision: u64,
    sequence: Option<ItemId>,
    project_scope: Option<crate::project_workspace::ProjectScope>,
    // True only for a separately qualified Root source-library read.
    source_library: Option<Vec<u64>>,
    // A separate live-editor read never becomes a project/source/history writer.
    multicam_production: Option<Box<multicam_production::ReadOwner>>,
    #[cfg(not(target_arch = "wasm32"))]
    multicam_history: Option<Box<multicam_production::origin::Retained>>,
    editor_control: Option<Box<editor_control::ReadOwner>>,
    // A live view read cannot borrow editor-setting, source or history authority.
    #[cfg(not(target_arch = "wasm32"))]
    sequence_views: Option<Box<sequence_views::ReadOwner>>,
    project: std::sync::Weak<filmcraft_project::Project>,
    path: Option<String>,
    context: Value,
    committed: Option<(Value, Value)>,
    after: Option<std::sync::Weak<filmcraft_project::Project>>,
    after_sequence: Option<ItemId>,
    redo_consumed: bool,
    media: std::sync::Arc<media_guard::Witnesses>,
    #[cfg(not(target_arch = "wasm32"))]
    preparation: Option<String>,
}

impl Ticket {
    fn sequence_id(&self) -> Result<ItemId> {
        if self.project_scope.is_some() {
            return Err(bad("agent.scope", "project metadata does not qualify a sequence read"));
        }
        self.sequence.ok_or(crate::EngineError::NoSequence)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRead {
    token: String,
    #[serde(default)]
    background: bool,
    #[serde(default, rename = "mediaScope")]
    media_scope: Option<media_scope::Request>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateRead {
    token: String,
    #[serde(default)]
    candidate: Option<String>,
    #[serde(default)]
    job: Option<String>,
    #[serde(default)]
    inspection: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    token: String,
    label: String,
    operations: Vec<Operation>,
    #[serde(default)]
    exact: bool,
    #[serde(default, rename = "validateAtPublication")]
    validate_at_publication: bool,
    #[serde(default)]
    authority: document_authority::Authority,
    #[serde(default, rename = "admissionPreparation")]
    admission_preparation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Undo {
    token: String,
    commit: String,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    command: String,
    params: Value,
}

#[derive(Clone)]
struct Staged {
    project: std::sync::Arc<filmcraft_project::Project>,
    state: crate::EditorState,
    events: Vec<crate::Event>,
    results: Vec<Value>,
    effective: Vec<Value>,
    #[cfg(not(target_arch = "wasm32"))]
    admission: Vec<std::sync::Arc<admission::Proof>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replay {
    label: String,
    operations: Vec<Operation>,
    #[serde(default)]
    exact: bool,
    #[serde(default)]
    authority: document_authority::Authority,
}

/// Explicit source correspondence, not a filename guess. `mediaId` distinguishes an original
/// from a derivative of the same Poema asset. Only identity-timebase derivatives are supported
/// here; this does not pretend to implement proxy time remapping or instance-level taste data.
#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    project_id: String,
    asset_id: String,
    media_id: String,
    role: String,
}

fn source_binding(raw: Option<&String>, command: &str) -> Result<Option<Binding>> {
    raw.map(|raw| serde_json::from_str::<Binding>(raw).map_err(|e| bad(command, e.to_string()))).transpose()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindSource {
    item: u64,
    binding: Binding,
}

pub(crate) fn canonical_native_value(v: &impl serde::Serialize) -> Result<Vec<u8>> {
    project_audit::value_bytes(v)
}

fn parse<T: serde::de::DeserializeOwned>(id: &str, p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| bad(id, e.to_string()))
}

fn token_ok(token: &str) -> bool {
    (32..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn context(s: &Session) -> Result<Value> {
    serde_json::to_value(&s.state).map_err(|e| bad("agent.snapshot", e.to_string()))
}

fn idle(s: &Session) -> Result<()> {
    if s.trim_play.active() || s.mixrec.active() || s.mcrec.active() {
        return Err(bad("agent.apply", "finish the active trim/recording gesture before planning an agent edit"));
    }
    Ok(())
}

fn snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("mediaScope").is_some_and(Value::is_null) {
        return Err(bad("agent.snapshot", "an explicit mediaScope must be an object; no whole-library fallback"));
    }
    let read: SnapshotRead = parse("agent.snapshot", p)?;
    #[cfg(not(target_arch = "wasm32"))]
    if read.background {
        return snapshot_jobs::start_scoped(s, read.token, read.media_scope.as_ref());
    }
    #[cfg(target_arch = "wasm32")]
    let _ = read.background;
    #[cfg(not(target_arch = "wasm32"))]
    if s.agent_bridge.snapshot_jobs.iter().any(|j| snapshot_jobs::has_token(j, &read.token)) {
        return Err(bad("agent.snapshot", "this token already owns a background read; poll or cancel it, never start a synchronous duplicate"));
    }
    fresh_snapshot(s, &read.token)?;
    let media = if let Some(scope) = &read.media_scope {
        let items = media_scope::resolve(&s.project, s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?, s.state.source_item, scope)?;
        media_guard::capture_scoped(&s.project, &s.media, items)?
    } else {
        media_guard::capture(&s.project, &s.media)?
    };
    snapshot_with_media(s, read.token, media)
}

fn fresh_ticket_token(s: &Session, token: &str) -> Result<()> {
    if !token_ok(token) || s.agent_bridge.tickets.iter().any(|t| t.token == token) {
        return Err(bad("agent.snapshot", "need a fresh, never-reused random token (32–128 ASCII characters)"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if s.agent_bridge.stopped_snapshots.iter().any(|t| t == token) {
        return Err(bad("agent.snapshot", "this snapshot was cancelled; use a fresh read, never restart the old request"));
    }
    Ok(())
}
fn fresh_snapshot(s: &Session, token: &str) -> Result<()> {
    fresh_ticket_token(s, token)?;
    idle(s)
}

// The worker only produces witnesses. Ticket creation and cache invalidation
// remain on the engine thread, after the native document basis is checked.
fn snapshot_with_media(s: &mut Session, token: String, observed: std::sync::Arc<media_guard::Witnesses>) -> Result<Value> {
    snapshot_document(s, token, observed, true)
}

fn snapshot_document(s: &mut Session, token: String, observed: std::sync::Arc<media_guard::Witnesses>, qualified: bool) -> Result<Value> {
    fresh_snapshot(s, &token)?;
    #[cfg(not(target_arch = "wasm32"))]
    history_checkpoint::epoch(s)?;
    let result = document_view(s, &token, &observed, qualified)?;
    let media = if qualified {
        media_guard::forget_changed(&s.media, &observed, s.agent_bridge.media.as_deref());
        let media = s.agent_bridge.media.as_ref().filter(|m| m.as_ref() == observed.as_ref()).cloned().unwrap_or(observed);
        s.agent_bridge.media = Some(media.clone());
        media
    } else {
        observed
    };
    let ticket = Ticket {
        token,
        revision: s.revision,
        sequence: Some(s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?),
        project_scope: None,
        source_library: None,
        #[cfg(not(target_arch = "wasm32"))]
        multicam_history: None,
        multicam_production: None,
        editor_control: None,
        #[cfg(not(target_arch = "wasm32"))]
        sequence_views: None,
        project: std::sync::Arc::downgrade(&s.project),
        path: s.path.clone(),
        context: context(s)?,
        committed: None,
        after: None,
        after_sequence: None,
        redo_consumed: false,
        media,
        #[cfg(not(target_arch = "wasm32"))]
        preparation: None,
    };
    register_ticket(s, ticket)?;
    Ok(result)
}
fn register_ticket(s: &mut Session, ticket: Ticket) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if ticket.sequence_views.is_some() && (ticket.project_scope.is_none() || ticket.source_library.is_some() || ticket.editor_control.is_some()) {
        return Err(bad("agent.scope", "sequence-view reads require only their exact Root and distinct view purpose"));
    }
    if ticket.editor_control.is_some() && (ticket.project_scope.is_none() || ticket.source_library.is_some()) {
        return Err(bad("agent.scope", "live editor reads require their exact Root and cannot carry source-library authority"));
    }
    if ticket.source_library.is_some() && ticket.project_scope.is_none() {
        return Err(bad("agent.scope", "a source-library read requires exact saved Root identity"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    s.agent_bridge.operations.register(&ticket.token)?;
    s.agent_bridge.tickets.push_back(ticket);
    while s.agent_bridge.tickets.len() > TICKET_LIMIT {
        // Read-only traffic must not erase the canonical result needed for
        // owned history or receipt-only reconciliation after a lost reply.
        // Keep the just-issued purpose usable. Prefer an older unused read;
        // an all-published window still ages out its oldest result at the same
        // fixed bound. Retention never turns a result into fresh edit evidence.
        let older = s.agent_bridge.tickets.len().saturating_sub(1);
        let disposable = s.agent_bridge.tickets.iter().take(older).position(|t| t.committed.is_none()).unwrap_or(0);
        s.agent_bridge.tickets.remove(disposable);
    }
    Ok(())
}

// Pure canonical native document projection. The caller distinguishes current
// file observation from retained expected generations; no disk/cache mutation.
fn source_rows(s: &Session, observed: &media_guard::Witnesses, qualified: bool) -> Result<Vec<Value>> {
    if s.project.items.len() > 100_000 {
        return Err(bad("agent.read", "complete source inventory exceeds its bound"));
    }
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    for it in s.project.items.values() {
        let Some(media) = it.as_media() else { continue };
        if it.name.len() > 1024
            || it.metadata.get(BINDING_KEY).is_some_and(|v| v.len() > 8192)
            || matches!(&media.media,filmcraft_project::MediaRef::File{path} if path.len()>4096)
        {
            return Err(bad("agent.read", "complete source metadata exceeds its field bound"));
        }
        let binding = source_binding(it.metadata.get(BINDING_KEY), "agent.read")?;
        let row = json!({"item": it.id.0, "media": media.media, "binding": binding,
                "fileVerification": if matches!(media.media, filmcraft_project::MediaRef::File { .. }) { if qualified { observed.qualification(it.id.0) } else { "unverified" } } else { "generated" },
                "name": it.name, "duration": media.info.duration.0, "video": media.info.has_video(),
                "sourceFrameRate": media.info.video.as_ref().map(|v| v.frame_rate),
                "videoWidth": media.info.video.as_ref().map_or(0, |v| v.width), "videoHeight": media.info.video.as_ref().map_or(0, |v| v.height),
                "audioSampleRate": media.info.audio().map_or(0, |a| a.sample_rate),
                "audioChannels": media.info.audio().map_or(0, |a| a.channels), "audioStreams": media.info.audio_streams.len(), "audioPlacements":source_edit::audio_layout(media), "offline": media.offline});
        let len = serde_json::to_vec(&row).map_err(|e| bad("agent.read", e.to_string()))?.len();
        bytes = bytes.checked_add(len).ok_or_else(|| bad("agent.read", "source byte budget overflow"))?;
        if bytes > 7 * 1024 * 1024 {
            return Err(bad("agent.read", "complete source metadata exceeds its transport bound; no rows truncated"));
        }
        rows.push(row);
    }
    Ok(rows)
}

fn document_view(s: &Session, token: &str, observed: &media_guard::Witnesses, qualified: bool) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let native_session = s.agent_bridge.epoch.as_ref().ok_or_else(|| bad("agent.snapshot", "native session epoch missing"))?;
    #[cfg(target_arch = "wasm32")]
    let native_session = Value::Null;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(|e| bad("agent.snapshot", e))?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let q = s.project.sequence(sequence).ok_or(crate::EngineError::NoSequence)?;
    let sources = source_rows(s, observed, qualified)?;
    let mut result = json!({
        "protocol": 1, "token": token, "revision": s.revision, "nativeSession":native_session,
        "capabilities": {"exactEdits": true, "guardedUndo": true, "guardedRedo": true, "guardedHistoryRead": true, "historyProjection":cfg!(not(target_arch="wasm32")), "assemblyEdits": true, "sourcePreview": true, "sequenceTools": true,
            "programInspection": cfg!(not(target_arch = "wasm32")), "backgroundInspection":cfg!(not(target_arch="wasm32")), "finishingProjection":cfg!(not(target_arch="wasm32")), "snapshotValidation": true, "backgroundValidation":cfg!(not(target_arch = "wasm32")), "retiming": true, "sourceMapping": true, "sourceFrameMapping": true,
            "condenseEdits": true, "condenseCandidates": cfg!(not(target_arch = "wasm32")),
            "condenseMixRegions": cfg!(not(target_arch = "wasm32")),"condensePassageOwnership":true,"condenseRestoration":true,
            "condenseRestoreCandidates":cfg!(not(target_arch = "wasm32")),"condenseVersionChoices":true,"typedClipFinishing":true,
            "persistentEffectIdentity":true,"finishingDefinitions":true,"parameterEdits":true,"finishingEdits":true,"clipFinishingContext":true,"canonicalCondenseChoices":true,
            "condenseLogicalInputs":true,"condenseReceiptTransitions":true,"historyCheckpoint":cfg!(not(target_arch = "wasm32")),"sourceFileGuard":cfg!(not(target_arch = "wasm32")),"backgroundSnapshots":cfg!(not(target_arch = "wasm32")),"operationCancellation":operation_cancellation_available(s)},
        "condenseRestorations": condense_restore::catalog(s, sequence)?,
        "sequences": sequences::catalog(s)?,
        "sourceMonitor": s.state.source_item.and_then(|id| crate::clip_ops::source_view(s, id)).map(|v| {
            let mut view = v.to_json(s.state.source_playhead); view["frameRate"] = json!(v.rate); view
        }),
        "linkedSelection": s.state.linked_selection,
        "project": crate::commands::inspect_project(s),
        "sequence": crate::commands::inspect_sequence(s, sequence, q), "sources": sources,
    });
    result["capabilities"]["editorialWorkspaceAssociation"] = json!(true);
    result["editorialWorkspace"] = serde_json::to_value(crate::editorial_workspace::association(&s.project)?).map_err(|e| bad("agent.read", e.to_string()))?;
    result["capabilities"]["currentPictureInputs"] = json!(true);
    result["capabilities"]["nativeExportDelivery"] = json!(cfg!(not(target_arch = "wasm32")) && operation_cancellation_available(s));
    result["capabilities"]["workspaceUpdateFence"] = json!(cfg!(not(target_arch = "wasm32")) && operation_cancellation_available(s));
    result["capabilities"]["workspaceUpdateBundle"] = json!(cfg!(not(target_arch = "wasm32")) && operation_cancellation_available(s));
    result["capabilities"]["captionTools"] = json!(true);
    result["capabilities"]["markerTools"] = json!(true);
    result["capabilities"]["trackTools"] = json!(true);
    result["capabilities"]["trackInsertTools"] = json!(true);
    result["capabilities"]["trackInsertStackTools"] = json!(true);
    result["capabilities"]["mixerRoutingTools"] = json!(true);
    result["capabilities"]["mixerAutomationRead"] = json!(true);
    result["capabilities"]["mixerAutomationTools"] = json!(true);
    result["capabilities"]["clipMixerTools"] = json!(true);
    result["capabilities"]["editorSettingsRead"] = json!(true);
    result["editorStateSha256"] = json!(project_clear::editor_hash(&s.state)?);
    result["capabilities"]["binTools"] = json!(true);
    result["capabilities"]["binSelectionGrouping"] = json!(true);
    result["capabilities"]["searchBinTools"] = json!(true);
    result["capabilities"]["effectStackTools"] = json!(true);
    result["capabilities"]["graphicsTools"] = json!(true);
    result["capabilities"]["fontCatalog"] = json!(true);
    result["capabilities"]["graphicsFontResources"] = json!(true);
    result["capabilities"]["graphicsFontEdits"] = json!(true);
    result["capabilities"]["graphicsFontCanonicalPlans"] = json!(true);
    result["capabilities"]["graphicsSharedFontEdits"] = json!(true);
    result["capabilities"]["graphicsTracking"] = json!(true);
    result["capabilities"]["graphicsGeometry"] = json!(true);
    result["capabilities"]["graphicsCharacterStyles"] = json!(true);
    result["capabilities"]["graphicsCharacterEdits"] = json!(true);
    result["capabilities"]["graphicsLayout"] = json!(true);
    result["capabilities"]["graphicsLayoutIndex"] = json!(true);
    result["capabilities"]["graphicsLayoutPins"] = json!(true);
    result["capabilities"]["clipArrangement"] = json!(true);
    result["capabilities"]["advancedTrims"] = json!(true);
    result["capabilities"]["sourceEdits"] = json!(true);
    result["capabilities"]["picturePlacement"] = json!(true);
    result["capabilities"]["musicPlacement"] = json!(true);
    result["musicState"] = music::summary(s);
    result["capabilities"]["reactionPublication"] = json!(true);
    result["capabilities"]["reactionReferenceInputs"] = json!(true);
    result["capabilities"]["sourceEventInputs"] = json!(true);
    result["capabilities"]["reactionReferenceHandoff"] = json!(true);
    #[cfg(not(target_arch = "wasm32"))]
    {
        result["capabilities"]["sequencePayloads"] = json!(true);
        result["capabilities"]["sequenceAssembly"] = json!(true);
        result["capabilities"]["sequenceAssemblyReadback"] = json!(true);
    }
    result["sourceEditState"] = source_edit::summary(s);
    result["trackState"] = tracks::summary(s);
    result["binState"] = bins::summary(s);
    result["searchState"] = searches::summary(s);
    result["capabilities"]["captionRippleDelete"] = json!(true);
    result["capabilities"]["captionRippleTrim"] = json!(true);
    result["capabilities"]["captionRippleInsert"] = json!(true);
    result["capabilities"]["captionRippleMove"] = json!(true);
    result["capabilities"]["captionRippleRetime"] = json!(true);
    result["capabilities"]["captionCondenseTiming"] = json!(true);
    result["capabilities"]["scopedSourceVerification"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["readBasisProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceCatalogProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceSelectionContext"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["guardedOperationDeadlineMs"] = json!(media_guard::OPERATION_DEADLINE_MS);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let redo_bytes = s.history.redo.last().map_or(0, |(_, target)| admission::restored_source_bytes(s, target));
        let large = media_guard::source_deadline(observed.source_bytes().saturating_add(redo_bytes)) == media_guard::LARGE_SOURCE_DEADLINE_MS;
        result["capabilities"]["largeSourceVerification"] = json!(large && s.worker_available && operation_cancellation_available(s));
        result["capabilities"]["largeSourceVerificationDeadlineMs"] = json!(media_guard::LARGE_SOURCE_DEADLINE_MS);
        result["capabilities"]["largeSourceVerificationThresholdBytes"] = json!(media_guard::LARGE_SOURCE_BYTES);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        result["capabilities"]["backgroundSnapshotDeadlineMs"] = json!(media_guard::SNAPSHOT_READ_DEADLINE_MS);
    }
    result["capabilities"]["historyCheckpointOperations"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["historyCheckpointRetention"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["historyCheckpointMetadataBasis"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["historyAvailabilityProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["documentProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["authoredDocumentEdits"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceFrameProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["captionDestinationProjection"] = json!(cfg!(not(target_arch = "wasm32")));
    result["mediaScope"] = observed.scope_json();
    if !qualified {
        result["mediaScope"]["libraryVerified"] = json!(false);
        result["documentProjection"] = json!({"expectedGenerations":true,"sourceValidation":"not-performed","writeAuthority":false});
    }
    result["capabilities"]["exactCaptionImport"] = json!(true);
    result["capabilities"]["currentMixCaptionImport"] = json!(true);
    result["captionState"] = captions::summary(s);
    result["capabilities"]["applyPreparation"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceAdmission"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceAdmissionBatch"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["sourceAudioLayout"] = json!(cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["backgroundPublication"] = json!(s.worker_available && cfg!(not(target_arch = "wasm32")));
    result["capabilities"]["publicationValidatedApply"] = json!(s.worker_available && cfg!(not(target_arch = "wasm32")) && operation_cancellation_available(s));
    #[cfg(not(target_arch = "wasm32"))]
    {
        result["capabilities"]["sourceAdmissionPreparation"] = json!(s.worker_available && operation_cancellation_available(s));
        result["capabilities"]["sourceAdmissionPreparationDeadlineMs"] = json!(admission_jobs::PREPARE_MS);
        result["capabilities"]["sourceAdmissionPublicationDeadlineMs"] = json!(admission_jobs::PUBLICATION_MS);
    }
    Ok(result)
}

fn operation_cancellation_available(s: &Session) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.agent_bridge.operations.available()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        false
    }
}
pub(crate) fn run_command(s: &mut Session, id: &str, p: &Value, run: fn(&mut Session, &Value) -> Result<Value>) -> Result<Value> {
    project_plane::command_scope(s, id, p)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        operation_control::run(s, id, p, run)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
        run(s, p)
    }
}
fn validate_media(s: &Session, expected: &media_guard::Witnesses) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(check) = s.agent_bridge.operations.check() {
        return media_guard::validate_checked(&s.project, &s.media, expected, &check);
    }
    media_guard::validate(&s.project, &s.media, expected)
}
fn enter_publication(s: &Session, token: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.agent_bridge.operations.enter_publication(token)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, token);
        Ok(())
    }
}

fn bind_source(s: &mut Session, p: &Value) -> Result<Value> {
    let bind: BindSource = parse("agent.bindSource", p)?;
    let b = &bind.binding;
    if [&b.project_id, &b.asset_id, &b.media_id].iter().any(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
        || !matches!(b.role.as_str(), "original" | "proxy")
    {
        return Err(bad("agent.bindSource", "need bounded projectId, assetId, mediaId and role original|proxy"));
    }
    let raw = serde_json::to_string(b).map_err(|e| bad("agent.bindSource", e.to_string()))?;
    let id = ItemId(bind.item);
    let it = s.project.item(id).filter(|it| it.as_media().is_some()).ok_or_else(|| bad("agent.bindSource", "item is not a media source"))?;
    if let Some(old) = it.metadata.get(BINDING_KEY) {
        if old != &raw {
            return Err(bad("agent.bindSource", "source already has a different binding; do not silently reassign annotations"));
        }
        return Ok(json!({"item": id.0, "binding": b}));
    }
    s.edit("Bind Poema Source", |p, _| {
        let it = p.items.get_mut(&id).ok_or_else(|| bad("agent.bindSource", "missing source"))?;
        it.metadata.insert(BINDING_KEY.to_string(), raw);
        Ok(())
    })?;
    Ok(json!({"item": id.0, "binding": b}))
}

/// Validate all operation shapes before staging. Explicit targets only; no selection/playhead
/// defaults, file writes, jobs, project replacement, arbitrary recursion or unchecked time math.
fn validate(op: &Operation) -> Result<()> {
    if op.command == multicam_production::COMMIT {
        return multicam_production::validate_operation(op);
    }
    if op.command == project_font_write::COMMAND {
        return project_font_write::validate(op);
    }
    if op.command == project_clear::COMMAND {
        return project_clear::validate(op);
    }
    if op.command == project_plane::CREATE {
        return project_plane::validate_create(op);
    }
    if op.command == crate::editorial_workspace::COMMAND {
        crate::editorial_workspace::validate(&op.params)?;
        return Ok(());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if op.command == sequence_publish::COMMAND {
        return sequence_publish::validate(op);
    }
    if op.command == picture_place::COMMAND {
        return picture_place::validate(op);
    }
    if op.command == music::COMMAND {
        return music::validate(op);
    }
    if op.command == reactions::COMMAND {
        return reactions::validate(op);
    }
    if op.command == reaction_references::COMMAND {
        return reaction_references::validate(op);
    }
    if op.command == source_edit::COMMAND {
        return source_edit::validate(op);
    }
    if op.command == advanced_trim::COMMAND {
        return advanced_trim::validate(op);
    }
    if op.command == arrange::COMMAND {
        return arrange::validate(op);
    }
    if op.command == tracks::COMMAND {
        return tracks::validate(op);
    }
    if op.command == bins::COMMAND {
        return bins::validate(op);
    }
    if op.command == searches::COMMAND {
        return searches::validate(op);
    }
    if op.command == markers::COMMAND {
        return markers::validate(op);
    }
    if op.command == caption_transfer::COMMAND {
        return caption_transfer::validate(op);
    }
    if op.command == captions::COMMAND {
        return captions::validate(op);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if op.command == admission::COMMAND {
        return admission::validate(op);
    }
    if op.command == finish::COMMAND {
        return finish::validate(op);
    }
    if op.command == parameter::COMMAND {
        return parameter::validate(op);
    }
    if op.command == effect_stack::COMMAND {
        return effect_stack::validate(op);
    }
    if op.command == graphics::COMMAND {
        return graphics::validate(op);
    }
    if op.command == graphics_layout::COMMAND {
        return graphics_layout::validate(op);
    }
    if op.command == condense_restore::CHOOSE {
        return condense_restore::validate_choice(op);
    }
    if op.command == condense_restore::COMMAND {
        return condense_restore::validate(op);
    }
    if op.command == condense::COMMAND {
        return condense::validate(op);
    }
    if op.command == retime::COMMAND {
        return retime::validate(op);
    }
    if op.command == sequences::CREATE {
        return sequences::validate(op);
    }
    if op.command == preview::COMMAND {
        return preview::validate(op);
    }
    if assembly::is_assembly(&op.command) {
        return assembly::validate(op);
    }
    let p = op.params.as_object().ok_or_else(|| bad("agent.apply", "operation params must be an object"))?;
    let clip = || p.get("clip").and_then(Value::as_u64).is_some_and(|id| id > 0 && id <= 9_007_199_254_740_991);
    let ok = match op.command.as_str() {
        "agent.bindSource" => {
            let _: BindSource = parse("agent.bindSource", &op.params)?;
            true
        }
        "timeline.trim" => {
            (p.len() == 4
                || p.len() == 5
                    && p.get("mode").and_then(Value::as_str) == Some("ripple")
                    && p.get("captionBasis").is_some_and(|v| serde_json::from_value::<caption_ripple::Basis>(v.clone()).is_ok_and(|b| b.valid())))
                && clip()
                && matches!(p.get("edge").and_then(Value::as_str), Some("in" | "out"))
                && matches!(p.get("mode").and_then(Value::as_str), Some("regular" | "ripple"))
                && p.get("deltaFrames").and_then(Value::as_i64).is_some_and(|v| (-1_000_000..=1_000_000).contains(&v))
        }
        "timeline.razor" => p.len() == 2 && clip() && p.get("time").and_then(Value::as_i64).is_some_and(|v| (0..=9_007_199_254_740_991).contains(&v)),
        _ => false,
    };
    if !ok {
        return Err(bad("agent.apply", "unsupported operation or invalid explicit targets/time; allowed: bindSource, trim, razor"));
    }
    Ok(())
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let batch: Batch = parse("agent.apply", p)?;
    validate_batch(&batch)?;
    let ticket = s
        .agent_bridge
        .tickets
        .iter()
        .find(|t| t.token == batch.token)
        .ok_or_else(|| bad("agent.apply", "unknown/expired read token; reconcile and take a new snapshot, never blindly replay"))?;
    // A lost reply is recoverable without applying twice, even if a human subsequently edited.
    if let Some((request, result)) = &ticket.committed {
        return if request == p { Ok(result.clone()) } else { Err(bad("agent.apply", "read token already committed a different batch")) };
    }
    project_plane::batch_scope(s, &batch)?;
    #[cfg(not(target_arch = "wasm32"))]
    let prepared_admission = if batch.admission_preparation.is_some() {
        if !s.worker_available || !operation_cancellation_available(s) {
            return Err(bad("agent.apply", "prepared source admission requires the installed writer and independent Stop"));
        }
        Some(admission_jobs::consume(s, &batch, p)?)
    } else {
        None
    };
    if batch.admission_preparation.is_some() {
        check_edit_ticket_basis(s, &batch.token)?;
    } else if batch.authority == document_authority::Authority::DocumentState {
        #[cfg(not(target_arch = "wasm32"))]
        if ticket.preparation.is_some() {
            return Err(bad("agent.apply", "document publication cannot borrow an attempted media preparation; take a fresh read"));
        }
        check_edit_ticket_basis(s, &batch.token)?;
    } else if batch.validate_at_publication {
        #[cfg(not(target_arch = "wasm32"))]
        let prepared = ticket.preparation.is_some();
        #[cfg(target_arch = "wasm32")]
        let prepared = true;
        if !s.worker_available || !operation_cancellation_available(s) || prepared {
            return Err(bad("agent.apply", "publication validation requires the installed native writer, independent cancellation and an unprepared read"));
        }
        // Staging owns only a private native project. Its COW/context basis is
        // checked now; publish_batch still rechecks every original byte against
        // the retained witness immediately before the unchanged publication fence.
        // Avoid hashing the same files once in preparation and again at commit.
        check_edit_ticket_basis(s, &batch.token)?;
    } else {
        #[cfg(not(target_arch = "wasm32"))]
        let prepared = read_jobs::consume_apply(s, p, &batch.token)?;
        #[cfg(target_arch = "wasm32")]
        let prepared = false;
        if !prepared {
            check_edit_ticket(s, &batch.token)?;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let stage = if let Some(proofs) = prepared_admission { stage_batch_with_admission(s, &batch, Some(&proofs))? } else { stage_batch(s, &batch)? };
    #[cfg(target_arch = "wasm32")]
    let stage = stage_batch(s, &batch)?;
    publish_batch(s, &batch, stage, p, None)
}

fn operation_limit(batch: &Batch) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    if batch.exact && !batch.operations.is_empty() && batch.operations.iter().all(|op| op.command == admission::COMMAND) {
        return admission::GROUP_LIMIT;
    }
    OP_LIMIT
}
fn validate_batch(batch: &Batch) -> Result<()> {
    document_authority::validate(batch)?;
    if batch.admission_preparation.is_some() {
        #[cfg(not(target_arch = "wasm32"))]
        admission_jobs::validate(batch)?;
        #[cfg(target_arch = "wasm32")]
        return Err(bad("agent.apply", "prepared source admission is native only"));
    }
    let limit = operation_limit(batch);
    if !token_ok(&batch.token) || batch.label.trim().is_empty() || batch.label.len() > 256 || batch.operations.is_empty() || batch.operations.len() > limit {
        let operations = if limit == OP_LIMIT { "1–32 operations" } else { "1–2000 exact original bindings" };
        return Err(bad("agent.apply", format!("need a read token, label (1–256 bytes) and {operations}")));
    }
    Ok(())
}

// Transport waiting is not authority. The live preparation record and native
// operation fence independently decide whether the source budget is available.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn admission_reply_wait(p: &Value) -> Option<u64> {
    let batch: Batch = parse("agent.apply", p).ok()?;
    if batch.admission_preparation.is_none() || validate_batch(&batch).is_err() {
        return None;
    }
    Some(admission_jobs::PUBLICATION_MS + 2_000)
}

// A fixed reply class only. Native operation_control independently selects its
// clock from retained native witnesses/history; this hint grants no authority.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn source_verification_reply_wait(p: &Value) -> Option<u64> {
    let command = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str)?;
    if p.get("verificationPolicy").and_then(Value::as_str) != Some(media_guard::LARGE_SOURCE_POLICY)
        || !crate::session_worker::supports(command)
        || !p.get("params").and_then(|p| p.get("token")).and_then(Value::as_str).is_some_and(token_ok)
    {
        return None;
    }
    Some(media_guard::LARGE_SOURCE_REPLY_MS)
}

fn check_edit_ticket(s: &Session, token: &str) -> Result<()> {
    check_edit_ticket_basis(s, token)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| bad("agent.apply", "unknown/expired read token"))?;
    if ticket.project_scope.is_some() && ticket.source_library.is_none() && !multicam_production::source_purpose(ticket) {
        return Err(bad("agent.apply", "project metadata cannot authorize media-input operations"));
    }
    validate_media(s, &ticket.media)
}

// Exact native edit context without disk I/O. Read projections are provisional;
// all mutation entry points retain check_edit_ticket and final media guards.
fn check_edit_ticket_basis(s: &Session, token: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if s.agent_bridge.tickets.iter().find(|t| t.token == token).is_some_and(|t| t.sequence_views.is_some()) {
        return Err(bad("agent.apply", "a view-only read cannot authorize project, source, timeline, settings or persistence editing"));
    }
    if s.agent_bridge.tickets.iter().find(|t| t.token == token).is_some_and(|t| t.editor_control.is_some()) {
        return Err(bad("agent.apply", "an editor-only read cannot authorize project, source, timeline or persistence editing"));
    }
    check_ticket_editor_basis(s, token, true)
}
fn check_ticket_editor_basis(s: &Session, token: &str, require_idle: bool) -> Result<()> {
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| bad("agent.apply", "unknown/expired read token"))?;
    if ticket.committed.is_some() {
        return Err(bad("agent.apply", "read token already consumed"));
    }
    if require_idle {
        idle(s)?;
    }
    if let Some(scope) = &ticket.project_scope {
        project_plane::require_scope(s, scope)?;
    }
    let live_context = context(s)?;
    if ticket.revision != s.revision
        || s.state.active_sequence != ticket.sequence
        || ticket.context != live_context
        || ticket.path != s.path
        || !ticket.project.ptr_eq(&std::sync::Arc::downgrade(&s.project))
    {
        let changed: Vec<&str> = live_context
            .as_object()
            .map(|fields| fields.iter().filter(|(key, value)| ticket.context.get(*key) != Some(*value)).map(|(key, _)| key.as_str()).take(16).collect())
            .unwrap_or_default();
        return Err(bad(
            "agent.apply",
            format!("stale snapshot: document or editor context changed; read and re-plan (changed context fields: {})", changed.join(", ")),
        ));
    }
    Ok(())
}

fn stage_batch(s: &Session, batch: &Batch) -> Result<Staged> {
    stage_batch_with_admission(
        s,
        batch,
        #[cfg(not(target_arch = "wasm32"))]
        None,
    )
}
fn stage_batch_with_admission(
    s: &Session,
    batch: &Batch,
    #[cfg(not(target_arch = "wasm32"))] prepared: Option<&[std::sync::Arc<admission::Prepared>]>,
) -> Result<Staged> {
    document_authority::validate(batch)?;
    #[cfg(not(target_arch = "wasm32"))]
    if multicam_production::is_batch(batch) {
        return multicam_production::stage(s, batch);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let mut admission_proofs = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    let admission_check = s.agent_bridge.operations.check().unwrap_or_else(|| std::sync::Arc::new(media_guard::Check::new(web_time::Instant::now())));
    if batch.operations.iter().any(|op| op.command == crate::editorial_workspace::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "workspace association requires one exact initial metadata operation"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if batch.operations.iter().any(|op| op.command == admission::COMMAND) {
        admission::validate_group(&batch.operations, batch.exact)?;
    }
    // Exact mode treats linked A/V as one logical operation. Identical requests for its
    // picture and sound collapse; conflicting requests on that same group are rejected.
    let mut groups: Vec<(Vec<ClipId>, String, Value, usize)> = Vec::new();
    for op in &batch.operations {
        validate(op)?;
        mixer_automation_edit::live_basis(s, op)?;
        parameter::live_basis(s, op)?;
    }
    if batch.operations.iter().any(|op| op.command == captions::COMMAND || op.command == caption_transfer::COMMAND)
        && (!batch.exact || batch.operations.len() != 1)
    {
        return Err(bad("agent.apply", "caption edits require one exact human gesture"));
    }
    if batch.operations.iter().any(|op| op.command == searches::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "saved-search authoring requires one exact human gesture"));
    }
    if batch.operations.iter().any(|op| op.command == bins::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "bin organization requires one exact human gesture"));
    }
    if batch.operations.iter().any(|op| op.command == markers::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "marker edits require one exact human gesture"));
    }
    if batch.operations.iter().any(|op| {
        op.command == tracks::COMMAND
            || op.command == arrange::COMMAND
            || op.command == advanced_trim::COMMAND
            || op.command == source_edit::COMMAND
            || op.command == picture_place::COMMAND
            || op.command == music::COMMAND
            || op.command == reactions::COMMAND
            || op.command == reaction_references::COMMAND
    }) && (!batch.exact || batch.operations.len() != 1)
    {
        return Err(bad("agent.apply", "track edits require one exact human gesture"));
    }
    if batch.operations.iter().any(|op| op.command == "timeline.trim" && op.params.get("captionBasis").is_some())
        && (!batch.exact || batch.operations.len() != 1)
    {
        return Err(bad("agent.apply", "caption-aware trim requires one exact linked gesture"));
    }
    if batch.operations.iter().any(|op| op.command == parameter::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "parameter edits require one exact native gesture per batch"));
    }
    if batch.operations.iter().any(|op| op.command == effect_stack::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "effect stacks require one exact shared human gesture per batch"));
    }
    if batch.operations.iter().any(|op| op.command == graphics::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "graphics require one exact shared human gesture per batch"));
    }
    if batch.operations.iter().any(|op| op.command == graphics_layout::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "graphics layout requires one exact shared human gesture per batch"));
    }
    if batch.operations.iter().any(|op| op.command == finish::COMMAND) && (!batch.exact || batch.operations.len() != 1) {
        return Err(bad("agent.apply", "finishing needs one exact native human gesture"));
    }
    if batch.operations.iter().any(|op| {
        assembly::is_assembly(&op.command)
            || op.command == preview::COMMAND
            || op.command == sequences::CREATE
            || op.command == "agent.assembleSequence"
            || op.command == project_plane::CREATE
            || op.command == retime::COMMAND
            || op.command == condense::COMMAND
            || op.command == condense_restore::COMMAND
            || op.command == condense_restore::CHOOSE
    }) && (!batch.exact || batch.operations.len() != 1)
    {
        return Err(bad("agent.apply", "assembly needs exact mode and exactly one logical operation per batch"));
    }
    for op in batch.operations.iter().filter(|op| op.command == music::COMMAND) {
        let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == batch.token).ok_or_else(|| bad("agent.apply", "music source ticket expired"))?;
        music::require_sources(s, op, &ticket.media)?;
    }
    let before = s.revision;
    if before > u64::MAX - OP_LIMIT.max(batch.operations.len()) as u64 - 2 {
        return Err(bad("agent.apply", "revision counter exhausted"));
    }
    // Staging has no persistence, no live preview connection and no history shared with the host.
    // The allowlist above is essential: Session is NOT a sandbox for arbitrary engine commands.
    let mut stage = Session::new(s.services.clone());
    stage.project = s.project.clone();
    if !project_clear::is_batch(batch) {
        stage.media = s.media.clone();
    }
    stage.state = s.state.clone();
    stage.revision = s.revision;
    stage.saved_revision = s.saved_revision;
    // Source binding only uses the final private project. Keep one intermediate
    // snapshot while applying the shared binding service; the live publication
    // still owns one complete history step with its original project.
    #[cfg(not(target_arch = "wasm32"))]
    if batch.operations.iter().all(|op| op.command == admission::COMMAND) {
        stage.history.limit = 1;
    }
    if batch.operations.iter().any(|op| {
        op.command == source_edit::COMMAND
            || op.command == picture_place::COMMAND
            || op.command == music::COMMAND
            || op.command == reactions::COMMAND
            || op.command == "agent.assembleSequence"
            || op.command == project_plane::CREATE
            || op.command == graphics::COMMAND
            || op.command == graphics_layout::COMMAND
    }) {
        stage.prefs = s.prefs.clone();
    }
    let mut results = Vec::with_capacity(batch.operations.len());
    let mut effective = Vec::new();
    for op in &batch.operations {
        if op.command == crate::editorial_workspace::COMMAND {
            results.push(stage.execute(&op.command, op.params.clone())?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if op.command == sequence_publish::COMMAND {
            let payload = sequence_publish::live(s, op, &batch.token)?;
            let (result, journal) = sequence_publish::run(&mut stage, &payload)?;
            results.push(result);
            effective.push(journal);
            continue;
        }
        if op.command == picture_place::COMMAND
            || op.command == music::COMMAND
            || op.command == reactions::COMMAND
            || op.command == reaction_references::COMMAND
        {
            // Long passes honor the same operation Stop/deadline between
            // human gestures; all staged work still publishes only once.
            #[cfg(not(target_arch = "wasm32"))]
            let check = s.agent_bridge.operations.check();
            let checkpoint = || {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(check) = &check {
                    check.check()?;
                }
                Ok(())
            };
            results.push(if op.command == music::COMMAND {
                music::run(&mut stage, op, &checkpoint)?
            } else if op.command == reactions::COMMAND {
                reactions::run(&mut stage, op, &checkpoint)?
            } else if op.command == reaction_references::COMMAND {
                reaction_references::run(&mut stage, op, &checkpoint)?
            } else {
                picture_place::run(&mut stage, op, &checkpoint)?
            });
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == source_edit::COMMAND {
            results.push(source_edit::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == advanced_trim::COMMAND {
            results.push(advanced_trim::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == arrange::COMMAND {
            results.push(arrange::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == caption_transfer::COMMAND {
            results.push(caption_transfer::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == searches::COMMAND {
            results.push(searches::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == project_font_write::COMMAND {
            results.push(project_font_write::run(&mut stage, op, &batch.token, &|| {
                #[cfg(not(target_arch = "wasm32"))]
                admission_check.check()?;
                Ok(())
            })?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == project_clear::COMMAND {
            results.push(project_clear::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == bins::COMMAND {
            results.push(bins::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == tracks::COMMAND {
            results.push(tracks::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == markers::COMMAND {
            results.push(markers::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == captions::COMMAND {
            results.push(captions::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if op.command == admission::COMMAND {
            let (result, proof) = if let Some(proofs) = prepared {
                admission::run_prepared(&mut stage, op, proofs.get(results.len()).ok_or_else(|| bad("agent.apply", "prepared source group is incomplete"))?)?
            } else {
                admission::run(&mut stage, op, &admission_check)?
            };
            admission_proofs.push(proof);
            results.push(result);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == finish::COMMAND {
            results.push(finish::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == parameter::COMMAND {
            results.push(parameter::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == effect_stack::COMMAND {
            results.push(effect_stack::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == graphics::COMMAND {
            results.push(graphics::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == graphics_layout::COMMAND {
            results.push(graphics_layout::run(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == condense_restore::CHOOSE {
            results.push(condense_restore::run_choice(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == condense_restore::COMMAND {
            results.push(condense_restore::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        if op.command == condense::COMMAND {
            results.push(condense::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        if op.command == retime::COMMAND {
            results.push(retime::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        if op.command == project_plane::CREATE {
            results.push(project_plane::create(&mut stage, op)?);
            effective.push(json!({"command":op.command,"params":op.params}));
            continue;
        }
        if op.command == sequences::CREATE {
            results.push(sequences::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        if op.command == preview::COMMAND {
            results.push(preview::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        if assembly::is_assembly(&op.command) {
            results.push(assembly::run(&mut stage, op)?);
            effective.push(json!({"command": op.command, "params": op.params}));
            continue;
        }
        let caption_trim = if op.command == "timeline.trim" && op.params.get("mode").and_then(Value::as_str) == Some("ripple") {
            caption_ripple::prepare_trim(&stage, op)?
        } else {
            None
        };
        if let Some(id) = op.params.get("clip").and_then(Value::as_u64)
            && stage.active_sequence().is_none_or(|q| q.find_item(ClipId(id)).is_none())
        {
            return Err(bad("agent.apply", "explicit clip is missing from the active sequence"));
        }
        let mut members = Vec::new();
        if batch.exact {
            if !matches!(op.command.as_str(), "timeline.trim" | "timeline.razor") {
                return Err(bad("agent.apply", "exact mode supports only trim/razor"));
            }
            let target = op.params.get("clip").and_then(Value::as_u64).ok_or_else(|| bad("agent.apply", "missing exact target"))?;
            members = crate::commands::with_links(s, &[ClipId(target)]);
            members.sort_by_key(|c| c.0);
            let mut signature = op.params.clone();
            if let Some(p) = signature.as_object_mut() {
                p.remove("clip");
            }
            if let Some((_, command, params, index)) = groups.iter().find(|(g, _, _, _)| *g == members) {
                if command != &op.command || params != &signature {
                    return Err(bad("agent.apply", "conflicting operations on one linked group; read and plan one logical edit"));
                }
                results.push(json!({"deduplicated": true, "of": index}));
                continue;
            }
            groups.push((members.clone(), op.command.clone(), signature, results.len()));
            for member in &members {
                let q = stage.active_sequence().ok_or(crate::EngineError::NoSequence)?;
                let (track, item) = q.find_item(*member).ok_or_else(|| bad("agent.apply", "missing linked exact target"))?;
                if q.track(track).is_none_or(|t| t.locked) {
                    return Err(bad("agent.apply", "exact edits cannot skip a locked linked partner"));
                }
                if op.command == "timeline.razor" {
                    let time = op.params.get("time").and_then(Value::as_i64).ok_or_else(|| bad("agent.apply", "missing split time"))?;
                    let end = item.start.0.checked_add(item.duration.0).ok_or_else(|| bad("agent.apply", "split range overflow"))?;
                    if time <= item.start.0 || time >= end {
                        return Err(bad("agent.apply", "split must be strictly inside every linked target"));
                    }
                }
            }
        }
        let durations: Vec<(ClipId, i64)> = if batch.exact {
            let q = stage.active_sequence().ok_or(crate::EngineError::NoSequence)?;
            members
                .iter()
                .map(|id| q.find_item(*id).map(|(_, i)| (*id, i.duration.0)).ok_or_else(|| bad("agent.apply", "missing exact target")))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let mut human_params = op.params.clone();
        if op.command == "timeline.trim"
            && let Some(p) = human_params.as_object_mut()
        {
            p.remove("captionBasis");
        }
        let mut result = stage.execute(&op.command, human_params)?;
        if batch.exact {
            let matches = if op.command == "timeline.trim" {
                let frames = op.params.get("deltaFrames").and_then(Value::as_i64).ok_or_else(|| bad("agent.apply", "missing trim frames"))?;
                let delta = stage.sequence_rate().tick_of(frames).0;
                let q = stage.active_sequence().ok_or(crate::EngineError::NoSequence)?;
                result.get("delta").and_then(Value::as_i64) == Some(delta)
                    && durations.iter().all(|(id, duration)| {
                        let expected = if op.params.get("edge").and_then(Value::as_str) == Some("out") {
                            duration.checked_add(delta)
                        } else {
                            duration.checked_sub(delta)
                        };
                        q.find_item(*id).is_some_and(|(_, item)| Some(item.duration.0) == expected)
                    })
            } else {
                result.get("cuts").and_then(Value::as_u64) == u64::try_from(members.len()).ok()
            };
            if !matches {
                return Err(bad("agent.apply", "requested edit would be clamped or incomplete; no live changes committed"));
            }
        }
        if let Some(proof) = caption_ripple::finish_trim(&stage, caption_trim)? {
            result["captionRipple"] = proof;
        }
        results.push(result);
        effective.push(json!({"command": op.command, "params": op.params}));
    }
    crate::clip_identity::enforce_protection(&s.project, &stage.project)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == batch.token).ok_or_else(|| bad("agent.apply", "source-scope read expired"))?;
    #[cfg(not(target_arch = "wasm32"))]
    let admitted = admission_proofs.iter().map(|proof| proof.item).collect();
    #[cfg(target_arch = "wasm32")]
    let admitted = Default::default();
    if batch.authority == document_authority::Authority::DocumentState {
        project_clear::live_basis(s, batch)?;
        project_clear::document_transition(&s.project, &stage.project, batch, &results)?;
    } else {
        media_scope::require_transition(
            &s.project,
            &stage.project,
            &ticket.media,
            media_scope::Transition {
                before_active: s.state.active_sequence,
                after_active: stage.state.active_sequence,
                before_source: s.state.source_item,
                after_source: stage.state.source_item,
                admitted,
            },
        )?;
    }
    Ok(Staged {
        project: stage.project,
        state: stage.state,
        events: stage.events,
        results,
        effective,
        #[cfg(not(target_arch = "wasm32"))]
        admission: admission_proofs,
    })
}

fn publish_batch(s: &mut Session, batch: &Batch, stage: Staged, request: &Value, candidate: Option<&str>) -> Result<Value> {
    for op in &batch.operations {
        mixer_automation_edit::live_basis(s, op)?;
        parameter::live_basis(s, op)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    for op in batch.operations.iter().filter(|op| op.command == sequence_publish::COMMAND) {
        sequence_publish::live(s, op, &batch.token)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == music::COMMAND) {
        music::validate_live_basis(s, op)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == picture_place::COMMAND) {
        picture_place::validate_live_basis(s, op)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == reactions::COMMAND) {
        reactions::validate_live_basis(s, op)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == reaction_references::COMMAND) {
        reaction_references::validate_live_basis(s, op)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == source_edit::COMMAND) {
        source_edit::validate_live_basis(s, op)?;
    }
    if s.revision > u64::MAX - OP_LIMIT as u64 - 2 {
        return Err(bad("agent.apply", "revision counter exhausted"));
    }
    crate::clip_identity::enforce_protection(&s.project, &stage.project)?;
    if batch.authority == document_authority::Authority::DocumentState {
        document_authority::validate(batch)?;
        project_clear::live_basis(s, batch)?;
        project_clear::document_transition(&s.project, &stage.project, batch, &stage.results)?;
        check_edit_ticket_basis(s, &batch.token)?;
    } else {
        check_edit_ticket(s, &batch.token)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if !stage.admission.is_empty() {
        let check = s.agent_bridge.operations.check().unwrap_or_else(|| std::sync::Arc::new(media_guard::Check::new(web_time::Instant::now())));
        for proof in &stage.admission {
            admission::publication(proof, &check)?;
        }
    }
    if s.agent_bridge.tickets.iter().any(|t| t.token == batch.token && t.source_library.is_some()) {
        library_plane::validate_transition(&s.project, &stage.project, batch, &stage.results)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == graphics::COMMAND) {
        graphics::validate_live_basis(s, op, &batch.token)?;
    }
    for op in batch.operations.iter().filter(|op| op.command == project_font_write::COMMAND) {
        project_font_write::validate_live_basis(s, op, &batch.token)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let source_publication = multicam_production::publication(s, batch, &stage)?;
    #[cfg(not(target_arch = "wasm32"))]
    let unchanged = source_publication.as_ref().is_some_and(|p| p.unchanged);
    #[cfg(target_arch = "wasm32")]
    let unchanged = false;
    let before = s.revision;
    enter_publication(s, &batch.token)?;
    if !unchanged {
        s.commit_edit(&batch.label, (*stage.project).clone(), stage.state)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    for proof in &stage.admission {
        s.media.remove(proof.item);
    }
    if project_clear::is_batch(batch) {
        if let Some(result) = stage.results.first() {
            project_clear::finish_publication(s, result);
        }
    }
    // Only the audited source-opening event is forwarded, never intermediate stage changes.
    s.events.extend(stage.events.into_iter().filter(|event| matches!(event, crate::Event::OpenSource(_) | crate::Event::OpenSequence(_))));
    let mut result = json!({"status": "committed", "token": batch.token, "beforeRevision": before, "revision": s.revision, "results": stage.results, "validated": batch.exact});
    document_authority::mark(&mut result, batch.authority)?;
    if let Some(candidate) = candidate {
        result["candidate"] = json!(candidate);
    }
    if batch.operations.first().is_some_and(|op| op.command == preview::COMMAND) {
        result["action"] = json!("source_preview");
    }
    if project_font_write::is_batch(batch) {
        result["action"] = json!("project_font_replace");
        result["sequence"] = json!(s.state.active_sequence.map(|id| id.0));
    }
    if project_clear::is_batch(batch) {
        result["action"] = json!("project_clear");
        result["sequence"] = s.state.active_sequence.map(|id| json!(id.0)).unwrap_or(Value::Null);
    }
    if batch.operations.first().is_some_and(|op| op.command == project_plane::CREATE) {
        result["action"] = json!("project_sequence_create");
        result["sequence"] = result["results"][0]["sequence"].clone();
    }
    if batch.operations.first().is_some_and(|op| op.command == sequences::CREATE) {
        result["action"] = json!("sequence_create");
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(op) = batch.operations.first().filter(|op| op.command == sequence_publish::COMMAND) {
        result["action"] = json!("sequence_assembly");
        result["sequence"] = result["results"][0]["sequence"].clone();
        if let Some(id) = op.params.get("payload").and_then(Value::as_str) {
            sequence_payload::committed(s, id);
        }
    }
    if batch.operations.first().is_some_and(|op| op.command == parameter::COMMAND) {
        result["action"] = json!("parameter_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == effect_stack::COMMAND) {
        result["action"] = json!("effect_stack_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == graphics::COMMAND) {
        result["action"] = json!("graphics_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == graphics_layout::COMMAND) {
        result["action"] = json!("graphics_layout_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == finish::COMMAND) {
        result["action"] = json!("finishing_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == captions::COMMAND) {
        result["action"] = json!("caption_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == markers::COMMAND) {
        result["action"] = json!("marker_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == tracks::COMMAND) {
        result["action"] = json!("track_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == searches::COMMAND) {
        result["action"] = json!("search_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == bins::COMMAND) {
        result["action"] = json!("bin_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == arrange::COMMAND) {
        result["action"] = json!("arrange_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == advanced_trim::COMMAND) {
        result["action"] = json!("advanced_trim");
    }
    if batch.operations.first().is_some_and(|op| op.command == music::COMMAND) {
        result["action"] = json!("music_placement");
    }
    if batch.operations.first().is_some_and(|op| op.command == picture_place::COMMAND) {
        result["action"] = json!("picture_placement");
    }
    if batch.operations.first().is_some_and(|op| op.command == reactions::COMMAND) {
        result["action"] = json!("reaction_reveal");
    }
    if batch.operations.first().is_some_and(|op| op.command == reaction_references::COMMAND) {
        result["action"] = json!("reaction_reference_handoff");
    }
    if batch.operations.first().is_some_and(|op| op.command == source_edit::COMMAND) {
        result["action"] = json!("source_edit");
    }
    if batch.operations.first().is_some_and(|op| op.command == caption_transfer::COMMAND) {
        result["action"] = json!("caption_import");
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(p) = &source_publication {
        result["action"] = json!("multicam_production");
        result["sequence"] = json!(s.state.active_sequence.map(|id| id.0));
        result["computedRecord"] = p.record.clone();
        result["retainedOrigin"] = p.origin_metadata.clone();
        if p.unchanged {
            result["outcome"] = json!("unchanged");
            result["timelineCommits"] = json!(0);
            result["undoOwned"] = json!(false);
        }
    }
    if let Some(ticket) = s.agent_bridge.tickets.iter_mut().find(|t| t.token == batch.token) {
        ticket.committed = Some((request.clone(), result.clone()));
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) = &source_publication {
            let mut origin = p.origin.clone();
            origin.rebind_published_after(&s.project);
            ticket.multicam_history = Some(Box::new(origin));
            ticket.multicam_production = None;
        }
        ticket.after = Some(std::sync::Arc::downgrade(&s.project));
        ticket.after_sequence = s.state.active_sequence;
    }
    if !unchanged {
        s.agent_bridge.head = Some((s.revision, std::sync::Arc::downgrade(&s.project)));
    }
    // Recovery must replay ONE history step too; flattening the batch into commands makes
    // a subsequent Undo recover only its last operation (or double-trim linked duplicates).
    let mut replay = json!({"label": batch.label, "operations": stage.effective, "exact": batch.exact});
    document_authority::mark(&mut replay, batch.authority)?;
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(p) = source_publication {
        if !p.unchanged {
            s.journal.push(p.journal);
        }
    } else {
        s.journal.push(("agent.replayBatch".into(), replay));
    }
    #[cfg(target_arch = "wasm32")]
    s.journal.push(("agent.replayBatch".into(), replay));
    if s.journal.len() > 10_000 {
        s.journal.drain(..s.journal.len() - 10_000);
    }
    Ok(result)
}

fn replay(s: &mut Session, p: &Value) -> Result<Value> {
    let request: Replay = parse("agent.replayBatch", p)?;
    #[cfg(not(target_arch = "wasm32"))]
    let mut request = request;
    // Recovery has no live ticket from the old process. Mint one internally, then use the
    // same bounded allowlist/staging path. No file/project replacement operations are added.
    let mut entropy = [0u8; 32];
    getrandom::fill(&mut entropy).map_err(|e| bad("agent.replayBatch", e.to_string()))?;
    let token: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    snapshot(s, &json!({"token": token}))?;
    #[cfg(not(target_arch = "wasm32"))]
    for op in &mut request.operations {
        if op.command == sequence_publish::COMMAND {
            op.params = sequence_payload::restore(s, &token, &op.params)?;
        }
    }
    let mut batch = json!({"token": token, "label": request.label, "operations": request.operations, "exact": request.exact});
    document_authority::mark(&mut batch, request.authority)?;
    s.execute("agent.apply", batch)
}

/// Undo only a known agent batch at the current owned history head. One command performs
/// the read-ticket, revision, project identity and history-top checks and the undo atomically.
/// Fresh tickets make lost replies reconcilable by agent.receipt, never blind replay.
fn undo(s: &mut Session, p: &Value) -> Result<Value> {
    let request: Undo = parse("agent.undo", p)?;
    if !token_ok(&request.token) || !token_ok(&request.commit) || request.token == request.commit {
        return Err(bad("agent.undo", "need a fresh read token and a distinct committed batch token"));
    }
    let read = s.agent_bridge.tickets.iter().find(|t| t.token == request.token).ok_or_else(|| bad("agent.undo", "unknown/expired read token"))?;
    if let Some((old, result)) = &read.committed {
        return if old == p { Ok(result.clone()) } else { Err(bad("agent.undo", "read token already used")) };
    }
    idle(s)?;
    let live = std::sync::Arc::downgrade(&s.project);
    if read.revision != s.revision
        || read.context != context(s)?
        || read.path != s.path
        || s.state.active_sequence != read.sequence
        || !read.project.ptr_eq(&live)
    {
        return Err(bad("agent.undo", "stale snapshot; read before undo"));
    }
    let commit = s.agent_bridge.tickets.iter().find(|t| t.token == request.commit).ok_or_else(|| bad("agent.undo", "unknown/expired agent commit"))?;
    if !history::owns_undo(s, read, commit) {
        return Err(bad("agent.undo", "not the current agent-owned edit; a manual edit/undo or document change intervened"));
    }
    if s.revision >= u64::MAX - 1 {
        return Err(bad("agent.undo", "revision counter exhausted"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let source_history = commit.multicam_history.clone();
    let previous_sequence = commit.sequence;
    let source_owned = multicam_production::source_purpose(commit);
    let restore_sequence =
        !source_owned && commit.after_sequence != previous_sequence && !project_clear::is_commit(commit) && !project_font_write::is_commit(commit);
    let authority = document_authority::committed(commit)?;
    if authority == document_authority::Authority::DocumentState {
        project_clear::owned_transition(
            s.history.undo.last().map(|(_, p)| p.as_ref()).ok_or_else(|| bad("agent.undo", "owned history target disappeared"))?,
            &s.project,
            commit,
        )?;
    } else {
        validate_media(s, &read.media)?;
        let target = s.history.undo.last().map(|(_, project)| project.as_ref()).ok_or_else(|| bad("agent.undo", "owned history target disappeared"))?;
        let transition = if source_owned {
            history::source_transition(s, target)
        } else {
            media_scope::Transition {
                before_active: s.state.active_sequence,
                after_active: if restore_sequence { previous_sequence } else { s.state.active_sequence },
                before_source: s.state.source_item,
                after_source: s.state.source_item,
                admitted: Default::default(),
            }
        };
        media_scope::require_transition(&s.project, target, &read.media, transition)?;
    }
    enter_publication(s, &request.token)?;
    let before_sequence = s.state.active_sequence;
    let before = s.revision;
    let label = s.undo().ok_or_else(|| bad("agent.undo", "no owned edit to undo"))?;
    if restore_sequence {
        s.state.active_sequence = previous_sequence.filter(|id| s.project.sequence(*id).is_some());
        if let Some(id) = s.state.active_sequence
            && !s.state.open_sequences.contains(&id)
        {
            s.state.open_sequences.push(id);
        }
    }
    if before_sequence != s.state.active_sequence
        && let Some(id) = s.state.active_sequence
    {
        s.events.push(crate::Event::OpenSequence(id));
    }
    let mut result = json!({"status": "committed", "token": request.token, "beforeRevision": before, "revision": s.revision,
        "results": [{"undone": request.commit, "label": label}], "sequence": s.state.active_sequence.map(|id| id.0), "validated": true, "action": "undo"});
    document_authority::mark(&mut result, authority)?;
    if let Some(ticket) = s.agent_bridge.tickets.iter_mut().find(|t| t.token == request.token) {
        ticket.committed = Some((p.clone(), result.clone()));
        #[cfg(not(target_arch = "wasm32"))]
        if source_history.is_some() {
            ticket.multicam_history = source_history;
            ticket.multicam_production = None;
        }
        ticket.after = Some(std::sync::Arc::downgrade(&s.project));
        ticket.after_sequence = s.state.active_sequence;
    }
    s.agent_bridge.head = Some((s.revision, std::sync::Arc::downgrade(&s.project)));
    s.journal.push(("edit.undo".into(), json!({})));
    if s.journal.len() > 10_000 {
        s.journal.drain(..s.journal.len() - 10_000);
    }
    Ok(result)
}

fn receipt(s: &mut Session, p: &Value) -> Result<Value> {
    let read: Read = parse("agent.receipt", p)?;
    if !token_ok(&read.token) {
        return Err(bad("agent.receipt", "invalid token"));
    }
    Ok(match s.agent_bridge.tickets.iter().find(|t| t.token == read.token) {
        Some(t) => {
            json!({"status": if t.committed.is_some() { "committed" } else { "unapplied" }, "result": t.committed.as_ref().map(|(_, r)| r)})
        }
        None => json!({"status": "unknown", "result": null}),
    })
}

// Navigation is guarded and receipted, but changes only UI state: no revision/dirty/history step.
fn select_sequence(s: &mut Session, p: &Value) -> Result<Value> {
    let request: sequences::Select = parse("agent.selectSequence", p)?;
    let ticket = s
        .agent_bridge
        .tickets
        .iter()
        .find(|t| t.token == request.token)
        .ok_or_else(|| bad("agent.selectSequence", "unknown read token; read before selecting a sequence"))?;
    if let Some((old, result)) = &ticket.committed {
        return if old == p { Ok(result.clone()) } else { Err(bad("agent.selectSequence", "read token already used")) };
    }
    idle(s)?;
    if ticket.revision != s.revision
        || ticket.context != context(s)?
        || ticket.path != s.path
        || !ticket.project.ptr_eq(&std::sync::Arc::downgrade(&s.project))
        || s.state.active_sequence != ticket.sequence
    {
        return Err(bad("agent.selectSequence", "stale sequence selection; read again"));
    }
    let target = sequences::select_target(s, &request)?;
    media_scope::require_sequence(&s.project, &ticket.media, target)?;
    s.execute("sequence.open", json!({"item":target.0}))?;
    let result = json!({"status":"selected","action":"sequence_switch","token":request.token,
        "beforeRevision":s.revision,"revision":s.revision,"sequence":target.0});
    if let Some(ticket) = s.agent_bridge.tickets.iter_mut().find(|t| t.token == request.token) {
        ticket.committed = Some((p.clone(), result.clone()));
    }
    Ok(result)
}

// A long local perception read can outlive the media job. Validate its original
// snapshot without decoding again, taking an edit, or changing editor context.
fn validate_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("candidate").is_some_and(|v| v.as_str().is_none_or(|id| !token_ok(id))) {
        return Err(bad("agent.validateSnapshot", "invalid explicit candidate; no live validation fallback"));
    }
    if ["job", "inspection"].iter().any(|key| p.get(*key).is_some_and(|v| v.as_str().is_none_or(|id| !token_ok(id)))) {
        return Err(bad("agent.validateSnapshot", "explicit valid verification/inspection IDs required; no synchronous fallback"));
    }
    let r: ValidateRead = parse("agent.validateSnapshot", p)?;
    if r.job.is_some() {
        #[cfg(not(target_arch = "wasm32"))]
        return read_jobs::start(s, p, &r);
        #[cfg(target_arch = "wasm32")]
        return Err(bad("agent.validateSnapshot", "background file validation is native only"));
    }
    if r.inspection.is_some() {
        return Err(bad("agent.validateSnapshot", "inspection postflight requires a background verification job"));
    }
    let t = validate_read_basis(s, &r.token)?;
    validate_media(s, &t.media)?;
    let mut result = json!({"status":"current","token":r.token,"revision":s.revision,"sequence":t.sequence_id()?.0});
    if let Some(id) = r.candidate {
        validate_candidate_read(s, &id)?;
        result["candidate"] = json!(id);
        result["view"] = json!("condense_candidate");
    }
    Ok(result)
}
fn validate_read_basis<'a>(s: &'a Session, token: &str) -> Result<&'a Ticket> {
    let ticket = validate_read_basis_any(s, token)?;
    ticket.sequence_id()?;
    Ok(ticket)
}
fn validate_read_basis_any<'a>(s: &'a Session, token: &str) -> Result<&'a Ticket> {
    let t = s
        .agent_bridge
        .tickets
        .iter()
        .find(|t| t.token == token)
        .ok_or_else(|| bad("agent.validateSnapshot", "unknown/expired read ticket; discard evidence and read again"))?;
    idle(s)?;
    if t.revision != s.revision || t.path != s.path || s.state.active_sequence != t.sequence || !t.project.ptr_eq(&std::sync::Arc::downgrade(&s.project)) {
        return Err(bad("agent.validateSnapshot", "document/sequence changed during perception; no current evidence returned"));
    }
    Ok(t)
}

// Same current native document/sequence basis as read validation, without a
// disk observation. Projected metadata is not source freshness or edit authority.
fn validate_metadata_read(s: &Session, token: &str, projection: bool, command: &str) -> Result<()> {
    if projection && cfg!(target_arch = "wasm32") {
        return Err(bad(command, "background-qualified metadata projections require a native engine"));
    }
    let ticket = validate_read_basis(s, token)?;
    if !projection {
        validate_media(s, &ticket.media)?;
    }
    Ok(())
}

fn read_basis(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readBasis", p)?;
    validate_metadata_read(s, &r.token, true, "agent.readBasis")?;
    let ticket = validate_read_basis(s, &r.token)?;
    Ok(json!({"status":"basis","kind":"native_read_basis","token":r.token,"revision":s.revision,"sequence":ticket.sequence_id()?.0,
        "requiresSourceValidation":true,"writeAuthority":false}))
}

fn validate_candidate_read(s: &Session, id: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        candidates::validate_read(s, id)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, id);
        Err(bad("agent.validateSnapshot", "native candidate inspection is unavailable"))
    }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    let commands = vec![
        spec(
            multicam_production::READ,
            "Read Qualified Multicam Inputs",
            r#"{"token":freshRandomString,"scope":Root,"projectPath":savedPath,"targets":typedSourceOrTimelineSelection}"#,
            multicam_production::read,
        ),
        spec(
            multicam_production::PLAN,
            "Prepare Original Multicam Production",
            r#"{"token":qualifiedInputToken,"scope":Root,"edit":typedGesture} — native worker only; no publication"#,
            multicam_production::prepare,
        ),
        spec(
            multicam_production::STATUS,
            "Read Retained Multicam Plan",
            r#"{"token":qualifiedInputToken,"scope":Root} — same retained plan only"#,
            multicam_production::status,
        ),
        spec(
            library_plane::READ,
            "Read Independent Source Library",
            r#"{"token":freshRandomString,"scope":{"projectId":id,"workspaceId":uuid},"projectPath":absoluteSavedPath,"sources":[itemId]} — background, no timeline authority"#,
            library_plane::read,
        ),
        spec(
            library_plane::VALIDATE,
            "Validate Independent Source Library",
            r#"{"token":readToken,"scope":{"projectId":id,"workspaceId":uuid},"job":freshVerificationId} — background recheck only"#,
            library_plane::validate,
        ),
        spec("agent.readBasis", "Read Native Document Basis", r#"{"token":readToken}"#, read_basis),
        spec(
            "agent.readSourceSelection",
            "Read Native Source Selection",
            r#"{"scope":{"projectId":id,"anchorSequence":id},"nonce":string}"#,
            source_selection::read,
        ),
        spec(
            "agent.readSourceCatalog",
            "Read Native Source Metadata",
            r#"{"scope":{"projectId":id,"anchorSequence":id}|{"projectId":id,"workspaceId":uuid}} — provisional metadata only; independent project scope needs no sequence"#,
            source_catalog::read,
        ),
        spec(
            "agent.snapshot",
            "Agent Snapshot",
            r#"{"token":freshRandomString,"background":bool?,"mediaScope"?:{"sequences":[itemId],"sources":[itemId],"assets":[{"projectId":str,"assetId":str}]}}"#,
            snapshot,
        ),
        spec(
            "agent.validateSnapshot",
            "Validate Native Read",
            r#"{"token":readToken,"candidate"?:candidateId,"job"?:verificationJob,"inspection"?:completedInspectionJob}"#,
            validate_snapshot,
        ),
        spec(
            "agent.condenseInputs",
            "Read Original Logical Condense Inputs",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"identities":[str]}"#,
            condense_restore::read_inputs,
        ),
        spec(
            "agent.pictureInputs",
            "Read Current Picture Fit Inputs",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"dialogueTrack":id,"pictureTrack":id|null,"bites":[{"key":sha256,"assetId":str,"sourceInTicks":ticks,"sourceOutTicks":ticks}],"projection":bool?}"#,
            picture_inputs::read,
        ),
        spec(
            "agent.reactionReferenceInputs",
            "Read Current Original Reaction Source Events",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"originalTimeline":str,"queries":[{"key":sha256,"assetId":str,"componentIds":[str],"sourceTicks":ticks}],"projection":bool?}"#,
            reaction_reference_inputs::read,
        ),
        spec(
            "agent.readMusicDestination",
            "Read native music destination",
            r#"{"token":str,"scope":object,"item":id,"assetId":str,"anchor":object,"projection":bool?}"#,
            music::read,
        ),
        spec(
            "agent.sourceEventInputs",
            "Read Current Source Events",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"tracks":[id],"queries":[{"key":sha256,"assetId":str,"sourceTicks":ticks}],"projection":bool?}"#,
            source_events::read,
        ),
        spec(
            "agent.readFinishing",
            "Read Authored Clip Finishing",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"identity":str,"offset":n,"limit":n,"projection"?:bool}"#,
            finishing::read,
        ),
        spec(
            "agent.readCaptions",
            "Read Authored Captions",
            r#"{"token":str,"scope":object,"track":id|null,"offset":n,"limit":n,"projection"?:bool}"#,
            captions::read,
        ),
        spec(
            "agent.readMarkers",
            "Read Authored Markers",
            r#"{"token":str,"scope":object,"target":{"kind":"sequence"}|{"kind":"source","item":id},"offset":n,"limit":n,"projection":bool?}"#,
            markers::read,
        ),
        spec(
            "agent.readSearchBins",
            "Read Saved Searches",
            r#"{"token":str,"scope":object,"query":id|null,"offset":n,"limit":n,"projection":bool?}"#,
            searches::read,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            sequence_views::PREPARE,
            "Prepare Original Sequence View Change",
            r#"{"token":str,"scope":object,"projectPath":absoluteSavedPath}"#,
            sequence_views::prepare,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            sequence_views::COMMAND,
            "Navigate Original Sequence View With Trim Effects",
            r#"{"version":1,"token":str,"scope":object,"beforeSha256":sha256,"beforeEditorSha256":sha256,"operation":object}"#,
            sequence_views::publication::navigate,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            sequence_views::READ,
            "Read Original Sequence Views",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"projectPath":absoluteSavedPath}"#,
            sequence_views::read,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            sequence_views::OBSERVE,
            "Observe Same Original Sequence View Read",
            r#"{"token":str,"scope":object,"beforeSha256":sha256,"beforeEditorSha256":sha256}"#,
            sequence_views::observe,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            editor_control::PREPARE,
            "Prepare Original Editor Control Change",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"projectPath":absoluteSavedPath,"clockPolicy":"original_human_trim"?}"#,
            editor_control::prepare,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            editor_compound::COMMAND,
            "Configure Original Editor Control With Trim Effects",
            r#"{"version":2|3,"token":str,"scope":object,"beforeSha256":sha256,"beforeEditorSha256":sha256,"setting":object}"#,
            editor_compound::configure,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            editor_control::OUTCOME,
            "Read Original Editor Setting Outcome",
            r#"{"version":1,"token":str,"scope":object,"receiptSha256":sha256}"#,
            editor_control::read_outcome,
        ),
        spec(
            editor_control::READ,
            "Read Live Editor Controls",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"projectPath":absoluteSavedPath}"#,
            editor_control::read,
        ),
        #[cfg(not(target_arch = "wasm32"))]
        spec(
            multicam_read::READ,
            "Read Original Multi-Camera Context",
            r#"{"version":1,"requestId":str,"scope":object,"projectPath":absoluteSavedPath,"timeTicks":str|null,"cameraOffset":n,"cutOffset":n,"limit":1..32}"#,
            multicam_read::read,
        ),
        spec(
            project_plane::READ,
            "Read Saved Project Metadata",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"projectPath":absoluteSavedPath}"#,
            project_plane::read,
        ),
        spec(
            project_plane::SEQUENCE_SCOPE,
            "Read Owning Project Sequence Scope",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid}}"#,
            project_plane::sequence_scope,
        ),
        spec(
            project_plane::VALIDATE,
            "Validate Project Metadata Read",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid}}"#,
            project_plane::validate,
        ),
        spec("agent.readBins", "Read Physical Project Tree", r#"{"token":str,"scope":object,"offset":n,"limit":n,"projection":bool?}"#, bins::read),
        spec("agent.readTracks", "Read Authored Tracks", r#"{"token":str,"scope":object,"offset":n,"limit":n,"projection":bool?}"#, tracks::read),
        spec(
            editor_setting::COMMAND,
            "Configure Original Editor Setting",
            r#"{"token":str,"scope":object,"beforeSha256":sha256,"beforeEditorSha256":sha256,"setting":object}"#,
            editor_setting::configure,
        ),
        spec(
            editor_setting::CATALOG,
            "Read Original Transition Defaults Catalog",
            r#"{"token":str,"scope":object,"beforeEditorSha256":sha256,"offset":n,"limit":n}"#,
            editor_setting::catalog,
        ),
        spec(
            editor_settings::READ,
            "Read Original Editor Settings",
            r#"{"token":str,"scope":object,"beforeEditorSha256":sha256,"offset":n,"limit":n}"#,
            editor_settings::read,
        ),
        spec(
            "agent.readMixerAutomation",
            "Read Authored Mixer Automation",
            r#"{"token":str,"scope":object,"strip":id|null,"lane":index|null,"offset":n,"limit":n,"projection":bool?}"#,
            mixer_automation::read,
        ),
        spec(
            "agent.readTrackInserts",
            "Read Authored Mixer Inserts",
            r#"{"token":str,"scope":object,"strip":id|null,"offset":n,"limit":n,"projection":bool?}"#,
            track_inserts::read,
        ),
        spec(
            "agent.readEffectCatalog",
            "Read Native Effect Catalog",
            r#"{"token":str,"scope":object,"offset":n,"limit":n,"projection":bool?}"#,
            effect_stack::read_catalog,
        ),
        spec(
            "agent.readEffectStack",
            "Read Authored Effect Stack",
            r#"{"token":str,"scope":object,"identity":str,"offset":n,"limit":n,"projection":bool?}"#,
            effect_stack::read_stack,
        ),
        spec(
            project_save_plan::PLAN,
            "Plan Original Project Save Intent",
            r#"{"token":str,"scope":object,"beforeProjectSha256":str,"intent":object}"#,
            project_save_plan::plan,
        ),
        spec(
            project_font_resources::READ,
            "Read Current Original Font Operation Targets",
            r#"{"token":str,"scope":object,"commit":str,"side":"before"|"after","offset":n,"limit":n}"#,
            project_font_resources::read,
        ),
        spec(
            project_font_resources::PLAN,
            "Plan Whole Project Native Font Resources",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"beforeProjectSha256":sha256,"replacement":{"from":{"family":str,"style":str?},"to":{"family":str,"style":str?}},"offset":n,"limit":n}"#,
            project_font_resources::plan,
        ),
        spec(
            project_font_replacement::PLAN,
            "Plan Exact Authored Project Font Replacement",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"beforeProjectSha256":sha256,"replacement":{"from":{"family":str,"style":str?},"to":{"family":str,"style":str?}},"offset":n,"limit":n}"#,
            project_font_replacement::plan,
        ),
        spec(
            caption_fonts::READ,
            "Read Exact Caption Font Resources",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"target":{"sequence":id,"track":id,"caption":id}}"#,
            caption_fonts::read,
        ),
        spec(
            font_usage::READ,
            "Read Owning Project Font Usage",
            r#"{"token":str,"scope":{"projectId":str,"workspaceId":uuid},"offset":n,"limit":n}"#,
            font_usage::read,
        ),
        spec(
            font_catalog::READ,
            "Read Scoped Native Font Catalog",
            r#"{"token":str,"realm":{"kind":"project"|"sequence","scope":object},"system":bool,"offset":n,"limit":n}"#,
            font_catalog::read,
        ),
        spec(
            graphics::PLAN_FONTS,
            "Plan Native Graphics Font Change",
            r#"{"token":str,"request":{"scope":object,"beforeGraphicsSha256":sha256,"edit":object},"projection":bool?}"#,
            graphics::plan_fonts,
        ),
        spec(
            "agent.readGraphics",
            "Read Native Graphics",
            r#"{"token":str,"scope":object,"target":{"kind":"clips"|"layers"|"geometry"|"layer_index"|"shared_source"|"character_styles"|"character_text"|"font_resources"|"font_requests"|"font_glyphs","identity":str?,"layer_id":str?,"frame":n?},"offset":n,"limit":n,"projection":bool?}"#,
            graphics::read,
        ),
        spec(
            "agent.readGraphicsLayout",
            "Read Native Layout Targets",
            r#"{"token":str,"scope":object,"target":{"kind":"clips"}|{"kind":"layers","identity":str}|{"kind":"pins","identity":str},"offset":n,"limit":n,"projection":bool?,"index":bool?}"#,
            graphics_layout::read,
        ),
        spec(
            "agent.readCaptionDestination",
            "Read Exact Caption Destination",
            r#"{"token":str,"scope":object,"sequence":id,"projection":bool?,"sourceKind":"original_companion"|"current_mix"?}"#,
            caption_transfer::read,
        ),
        spec(
            "agent.sourceFrames",
            "Map Native Source Frames",
            r#"{"token":readToken,"scope":{"projectId":str,"anchorSequence":id},"frames":[frame],"projection":bool?}"#,
            source_frames::map,
        ),
        spec(
            "agent.apply",
            "Apply Guarded Agent Batch",
            r#"{"token":str,"label":str,"operations":[{"command":str,"params":object}],"exact"?:bool,"authority"?:"media_inputs"|"document_state","validateAtPublication"?:bool,"admissionPreparation"?:id}"#,
            apply,
        ),
        spec("agent.receipt", "Agent Batch Receipt", r#"{"token":str}"#, receipt),
        spec("agent.undo", "Undo Guarded Agent Batch", r#"{"token":freshReadToken,"commit":committedBatchToken}"#, undo),
        spec("agent.redo", "Redo Guarded Agent Batch", r#"{"token":freshReadToken,"commit":committedBatchToken,"undo":ownedUndoToken}"#, history::redo),
        spec(
            "agent.readHistory",
            "Read Current Guarded History Ownership",
            r#"{"token":readToken,"commit":committedBatchToken,"undo":ownedUndoToken|null,"scope":{"projectId":str,"anchorSequence":id},"projection":bool?}"#,
            history::read,
        ),
        spec(
            "agent.selectSequence",
            "Select Workspace Sequence",
            r#"{"token":freshReadToken,"item":id,"scope":{"projectId":str,"anchorSequence":id}}"#,
            select_sequence,
        ),
        spec(
            project_navigation::COMMAND,
            "Select Owning Project Sequence",
            r#"{"token":rootReadToken,"scope":{"projectId":str,"workspaceId":uuid},"sequenceScope":{"projectId":str,"anchorSequence":id},"item":id,"beforeSha256":sha256,"beforeEditorSha256":sha256}"#,
            project_navigation::select,
        ),
        spec("agent.replayBatch", "Recover Audited Agent Batch", r#"{"label":str,"operations":[{"command":str,"params":object}],"exact":bool?}"#, replay),
        // bindSource is undoable; no side effects outside project/state. Available alone for
        // debugging, but Poema callers should put it behind the same snapshot guard as edits.
        CommandSpec {
            journal: true,
            ..spec(
                "agent.bindSource",
                "Bind Poema Source",
                r#"{"item":id,"binding":{"projectId":str,"assetId":str,"mediaId":str,"role":"original|proxy"}}"#,
                bind_source,
            )
        },
    ];
    #[cfg(not(target_arch = "wasm32"))]
    let commands = commands
        .into_iter()
        .chain(inspection::commands())
        .chain(export::commands())
        .chain(workspace_update::commands())
        .chain(application_admission::commands())
        .chain(resource_preflight::commands())
        .chain(candidates::commands())
        .chain(history_checkpoint::commands())
        .chain(document_projection::commands())
        .chain(snapshot_jobs::commands())
        .chain(read_jobs::commands())
        .chain(font_resource_jobs::commands())
        .chain(save_plan_jobs::commands())
        .chain(save_operation::commands())
        .chain(admission_jobs::commands())
        .chain(sequence_payload::commands())
        .chain(sequence_publish::commands())
        .collect();
    commands
}

#[cfg(test)]
mod tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod ticket_retention_tests;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prepare_resource_preflight_worker(s:&mut Session,command:&str,p:&Value)->Result<String>{
    #[cfg(any(target_vendor="apple",target_os="linux"))] {resource_preflight::prepare_worker(s,command,p)}
    #[cfg(not(any(target_vendor="apple",target_os="linux")))] {let _=(s,command,p);Err(bad("resource preflight","original resource worker coordination is unsupported on this platform"))}
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn discard_resource_preflight_worker(s:&mut Session,request:&str)->Result<()> {
    #[cfg(any(target_vendor="apple",target_os="linux"))] {resource_preflight::discard_prepared_worker(s,request)}
    #[cfg(not(any(target_vendor="apple",target_os="linux")))] {let _=(s,request);Err(bad("resource preflight","original resource worker coordination is unsupported on this platform"))}
}
pub(crate) fn resource_preflight_held(s: &Session) -> bool {
    #[cfg(not(target_arch = "wasm32"))] { resource_preflight::held(s) }
    #[cfg(target_arch = "wasm32")] { let _=s; false }
}
pub(crate) fn workspace_update_held(s: &Session) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        workspace_update::held(s) || application_admission::held(s) || resource_preflight::held(s)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        false
    }
}
pub(crate) fn workspace_update_command(s: &Session, command: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        resource_preflight::command_allowed(s, command)?;
        application_admission::command_allowed(s, command)?;
        workspace_update::command_allowed(s, command)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, command);
        Ok(())
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn workspace_update_view(s: &Session) -> Option<Value> {
    workspace_update::view(s)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cancel_workspace_update(s: &mut Session) -> Result<Option<Value>> {
    let Some(params) = workspace_update::cancel_request(s) else {
        return Ok(None);
    };
    s.execute("agent.cancelWorkspaceUpdate", params).map(Some)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_workspace_update(s: &mut Session, params: &Value) -> Result<Value> {
    workspace_update::open_candidate(s, params)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure_workspace_update_persistence(s: &mut Session, cfg: crate::autosave::AutosaveConfig) -> Result<()> {
    workspace_update::configure_candidate_persistence(s, cfg)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_identity_publication(s: &mut Session, p: &Value) -> Result<Value> {
    history_checkpoint::open_identity_candidate(s, p)
}

#[cfg(not(target_arch = "wasm32"))]
mod project_recovery;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_recovery_tests;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn capture_project_recovery_basis(s: &mut Session, p: &Value) -> Result<Value> {
    project_recovery::capture(s, p)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_project_recovery_basis(p: &Value) -> Result<()> {
    project_recovery::validate(p)
}

#[cfg(not(target_arch = "wasm32"))]
mod native_auxiliary;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_auxiliary_tests;
#[cfg(not(target_arch = "wasm32"))]
mod native_editor;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_recovery_snapshot_tests;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_project_recovery(s: &mut Session, p: &Value) -> Result<Value> {
    history_checkpoint::open_project_recovery(s, p)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use history_checkpoint::journal::Pending as NativeJournalPending;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn capture_native_journal(s: &mut Session) -> Result<Option<NativeJournalPending>> {
    history_checkpoint::journal::capture(s)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use history_checkpoint::journal::FILE as NATIVE_JOURNAL_FILE;

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn validate_held_checkpoint_bytes(native: &[u8], project: &[u8]) -> Result<()> {
    history_checkpoint::journal::validate_current_checkpoint_bytes(native, project)
}

#[cfg(not(target_arch = "wasm32"))]
mod project_creation;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod project_creation_tests;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn create_project_workspace(s: &mut Session, params: &Value) -> Result<Value> {
    project_creation::create(s, params)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn application_project_status(s: &Session) -> Option<Value> {
    application_admission::status(s)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn application_only_held(s: &Session) -> bool {
    application_admission::held(s) && !workspace_update::held(s)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn application_setup_held(s: &Session) -> bool {
    application_admission::held(s)
}

#[cfg(not(target_arch = "wasm32"))]
/// Native-only continuation of the original bounded immutable Save job.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn begin_planned_project_save(
    s: &mut Session,
    job: &str,
    operation: &str,
    check: &impl Fn() -> Result<()>,
) -> Result<crate::project_save::operation::Key> {
    save_plan_jobs::begin_operation(s, job, operation, check)
}
pub(crate) fn require_idle_native_writer_jobs(s: &Session) -> Result<()> {
    workspace_update::require_idle_writer_jobs(s)
        .map_err(|e| crate::EngineError::Other(format!("wait for current native work before Save; no job was stopped or replayed: {e}")))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn pending_save_lifecycle_command(s: &Session, command: &str) -> bool {
    save_operation::allows(s, command)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use save_operation::task::{Control as SaveTaskControl, supports as save_lifecycle_command};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn pending_save_request(s: &Session) -> Option<Value> {
    save_operation::pending_request(s)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn native_read_test_serial() -> std::sync::MutexGuard<'static, ()> {
    snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn open_save_recovery(s: &mut Session, p: &Value) -> Result<Value> {
    history_checkpoint::open_save_recovery(s, p)
}

// Descriptive access to this Session's actual native epoch. This cannot create
// a ticket, change an epoch or reconstruct any Save operation ownership.
pub(crate) fn retained_native_epoch(s: &Session) -> Option<&str> {
    s.agent_bridge.epoch.as_deref()
}
#[cfg(test)]
pub(crate) fn test_native_epoch(s: &mut Session, rotate: bool) -> Result<String> {
    if rotate {
        s.agent_bridge.epoch = None;
    }
    history_checkpoint::epoch(s)
}

/// Read-only original bounded journal inventory for the reserved cold worker.
pub(crate) fn native_recovery_startup_inventory(data_directory: &std::path::Path, session_directory: &str) -> Result<Value> {
    project_recovery::startup_persistence_inventory(data_directory, session_directory, &media_guard::Check::new(web_time::Instant::now()))
}

#[cfg(not(target_arch = "wasm32"))]
pub use history_checkpoint::application_archive::{ApplicationSnapshotCapture, ApplicationTabArchiveInput};
