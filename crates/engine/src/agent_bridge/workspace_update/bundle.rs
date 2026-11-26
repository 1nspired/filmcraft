//! Opaque native handoff. Current project, full native history and editor state
//! stay in Rust; no saved-project replacement, JavaScript reconstruction or
//! renewal of footage/read authority occurs here.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const STAGE: &str = "agent.stageWorkspaceUpdate";
const MAX_BYTES: usize = 48 * 1024 * 1024;
const KIND: &str = "poema-native-workspace-update";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    token: String,
    scope: sequences::Scope,
    workspace: String,
    update: String,
    #[serde(rename = "pauseToken")]
    pause_token: String,
    checkpoint: String,
    #[serde(rename = "checkpointSha256")]
    checkpoint_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    kind: String,
    version: u32,
    scope: sequences::Scope,
    workspace: String,
    update: String,
    project_path: String,
    saved_project_hash: String,
    activation_token: String,
    revision: u64,
    saved_revision: u64,
    loaded_schema: u32,
    preferences: crate::autosave::Preferences,
    editor: crate::EditorState,
    // EditorState deliberately skips the live clipboard in ordinary project
    // files. A workspace update retains those native values explicitly.
    clipboard: Vec<(filmcraft_project::TrackKind, usize, filmcraft_project::TrackItem)>,
    clipboard_markers: Vec<filmcraft_project::Marker>,
    merge_key: Option<String>,
    checkpoint: String,
    checkpoint_sha256: String,
    history: history_checkpoint::Capsule,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Startup {
    #[serde(rename = "projectPath")]
    project_path: String,
    scope: sequences::Scope,
    workspace: String,
    update: String,
    sha256: String,
}
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn hash_ok(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())
}
fn target(directory: &Path, workspace: &str, update: &str) -> PathBuf {
    directory.join(format!(".poema-update-{workspace}-{update}.json"))
}
pub(super) fn reconciling(s: &Session, request: &Value) -> bool {
    s.agent_bridge.workspace_update.as_ref().is_some_and(|h| h.stage.as_ref().is_some_and(|stage| stage.request == *request))
}
pub(super) fn stage(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Stage = parse(STAGE, p)?;
    address(&r.scope, &r.workspace, &r.update)?;
    if !token_ok(&r.token) || !uuid(&r.checkpoint) || !hash_ok(&r.checkpoint_sha256) {
        return Err(bad(STAGE, "fresh persistence basis and exact saved composer checkpoint required"));
    }
    let held = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(STAGE, "pause this workspace before staging"))?;
    if held.candidate
        || held.scope != LifecycleScope::Sequence(r.scope.clone())
        || held.workspace != r.workspace
        || held.update != r.update
        || held.token != r.pause_token
    {
        return Err(bad(STAGE, "staging belongs to a different paused owner"));
    }
    history_checkpoint::validate_retained_basis(s, &held.basis)?;
    if let Some(stage) = &held.stage {
        return if stage.request == *p {
            Ok(stage.receipt.clone().unwrap_or_else(|| json!({"status":"unknown","workspace":r.workspace,"update":r.update,"replayed":false})))
        } else {
            Err(bad(STAGE, "this update already attempted staging; reconcile it, never replace its files"))
        };
    }
    history_checkpoint::retained_basis(s, &r.token, &r.scope)?;
    let history = history_checkpoint::capture_workspace(s, &r.scope, &r.workspace, &r.checkpoint, &r.checkpoint_sha256)?;
    let directory = history_checkpoint::workspace_directory(s)?;
    let path = target(&directory, &r.workspace, &r.update);
    let editor = super::super::native_editor::Snapshot::capture(s)?;
    let bundle = Bundle {
        kind: KIND.into(),
        version: 1,
        scope: r.scope.clone(),
        workspace: r.workspace.clone(),
        update: r.update.clone(),
        project_path: s.path.clone().ok_or_else(|| bad(STAGE, "saved workspace path missing"))?,
        saved_project_hash: project_file_hash(Path::new(s.path.as_deref().ok_or_else(|| bad(STAGE, "saved workspace path missing"))?))?,
        activation_token: r.token.clone(),
        revision: s.revision,
        saved_revision: editor.saved_revision,
        loaded_schema: editor.loaded_schema,
        preferences: editor.preferences,
        editor: editor.editor,
        clipboard: editor.clipboard,
        clipboard_markers: editor.clipboard_markers,
        merge_key: editor.merge_key,
        checkpoint: r.checkpoint.clone(),
        checkpoint_sha256: r.checkpoint_sha256.clone(),
        history,
    };
    let raw = history_checkpoint::bytes(&bundle, MAX_BYTES)?;
    // Refuse values that serde would normalize or discard during the update.
    let decoded: Bundle = serde_json::from_slice(&raw).map_err(|e| bad(STAGE, e.to_string()))?;
    if history_checkpoint::bytes(&decoded, MAX_BYTES)? != raw {
        return Err(bad(STAGE, "native workspace cannot round-trip completely; nothing staged"));
    }
    s.agent_bridge.operations.enter_publication(&r.token)?;
    let held = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(STAGE, "workspace pause disappeared"))?;
    held.stage = Some(Staged { request: p.clone(), receipt: None });
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|e| bad(STAGE, format!("exclusive workspace bundle: {e}; no existing file was replaced")))?;
    file.write_all(&raw).and_then(|_| file.sync_all()).map_err(|e| bad(STAGE, format!("incomplete bundle: {e}; inspect it, never replay staging")))?;
    File::open(&directory).and_then(|f| f.sync_all()).map_err(|e| bad(STAGE, e.to_string()))?;
    let receipt = json!({"status":"staged","workspace":r.workspace,"update":r.update,"scope":r.scope,"token":r.token,
        "revision":s.revision,"nativeSession":bundle.history.epoch,"projectPath":bundle.project_path,"path":path,"sha256":hash(&raw),"bytes":raw.len(),
        "checkpoint":r.checkpoint,"checkpointSha256":r.checkpoint_sha256,"dirty":s.is_dirty(),
        "projectUnchanged":true,"historyUnchanged":true,"savedFileUnchanged":true,"sourceValidation":"not-performed","writeAuthority":false});
    if let Some(stage) = s.agent_bridge.workspace_update.as_mut().and_then(|h| h.stage.as_mut()) {
        stage.receipt = Some(receipt.clone());
    }
    if let Some(h) = s.agent_bridge.workspace_update.as_mut() {
        h.saved_project_hash = Some(bundle.saved_project_hash);
    }
    Ok(receipt)
}

fn private_read(path: &Path, expected: &str) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).map_err(|e| bad(STAGE, e.to_string()))?;
    if !before.is_file() || before.len() == 0 || before.len() > MAX_BYTES as u64 {
        return Err(bad(STAGE, "workspace bundle must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if before.permissions().mode() & 0o077 != 0 {
            return Err(bad(STAGE, "workspace bundle must be private"));
        }
    }
    let mut file = File::open(path).map_err(|e| bad(STAGE, e.to_string()))?;
    let mut raw = Vec::new();
    Read::by_ref(&mut file).take(MAX_BYTES as u64 + 1).read_to_end(&mut raw).map_err(|e| bad(STAGE, e.to_string()))?;
    let after = file.metadata().map_err(|e| bad(STAGE, e.to_string()))?;
    let named = fs::symlink_metadata(path).map_err(|e| bad(STAGE, e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if [&after, &named].iter().any(|m| {
            !m.is_file()
                || m.dev() != before.dev()
                || m.ino() != before.ino()
                || m.size() != before.size()
                || m.mtime() != before.mtime()
                || m.mtime_nsec() != before.mtime_nsec()
                || m.ctime() != before.ctime()
                || m.ctime_nsec() != before.ctime_nsec()
        }) {
            return Err(bad(STAGE, "workspace bundle changed while reading"));
        }
    }
    if !after.is_file() || !named.is_file() || raw.len() as u64 != before.len() || hash(&raw) != expected {
        return Err(bad(STAGE, "workspace bundle checksum differs; nothing adopted"));
    }
    Ok(raw)
}

pub(super) fn project_file_hash(path: &Path) -> Result<String> {
    let before = fs::symlink_metadata(path).map_err(|e| bad(STAGE, e.to_string()))?;
    if !before.is_file() || before.len() > 128 * 1024 * 1024 {
        return Err(bad(STAGE, "saved project must be a bounded regular file"));
    }
    let mut file = File::open(path).map_err(|e| bad(STAGE, e.to_string()))?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| bad(STAGE, e.to_string()))?;
        if n == 0 {
            break;
        }
        size = size.saturating_add(n as u64);
        if size > 128 * 1024 * 1024 {
            return Err(bad(STAGE, "saved project exceeded its file bound"));
        }
        digest.update(buffer.get(..n).ok_or_else(|| bad(STAGE, "invalid project read size"))?);
    }
    let after = file.metadata().map_err(|e| bad(STAGE, e.to_string()))?;
    let named = fs::symlink_metadata(path).map_err(|e| bad(STAGE, e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if [&after, &named].iter().any(|m| {
            !m.is_file()
                || m.dev() != before.dev()
                || m.ino() != before.ino()
                || m.size() != before.size()
                || m.mtime() != before.mtime()
                || m.mtime_nsec() != before.mtime_nsec()
                || m.ctime() != before.ctime()
                || m.ctime_nsec() != before.ctime_nsec()
        }) {
            return Err(bad(STAGE, "saved project changed while reading"));
        }
    }
    if !after.is_file() || !named.is_file() || size != before.len() {
        return Err(bad(STAGE, "saved project changed while reading"));
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub(super) fn check_saved_project(s: &Session) -> Result<()> {
    if let Some(h) = s.agent_bridge.workspace_update.as_ref()
        && let Some(basis) = h.recovery_basis.as_ref()
    {
        if h.candidate {
            super::super::project_recovery::validate(basis)?;
        } else {
            history_checkpoint::validate_staged_project_recovery(s)?;
        }
    }

    if let Some(expected) = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.saved_project_hash.as_deref()) {
        let path = s.path.as_deref().ok_or_else(|| bad(STAGE, "workspace path missing"))?;
        history_checkpoint::workspace_directory(s)?;
        if project_file_hash(Path::new(path))? != expected {
            return Err(bad(STAGE, "saved project changed during handoff; candidate remains paused"));
        }
    }
    Ok(())
}

/// Trusted startup only, before a project, persistence writer or human frame.
/// The candidate remains paused; adopting a bundle does not activate editing.
pub(super) fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Startup = parse(STAGE, p)?;
    address(&r.scope, &r.workspace, &r.update)?;
    if !hash_ok(&r.sha256)
        || s.path.is_some()
        || s.revision != 1
        || s.saved_revision != 1
        || !s.history.undo.is_empty()
        || !s.history.redo.is_empty()
        || s.persistence.is_some()
        || s.view_only
    {
        return Err(bad(STAGE, "workspace adoption requires exact checksum and a fresh native startup before persistence"));
    }
    let mut candidate = Session::new(s.services.clone());
    candidate.prefs = s.prefs.clone();
    candidate.path = Some(r.project_path.clone());
    let directory = history_checkpoint::workspace_directory(&candidate)?;
    let path = target(&directory, &r.workspace, &r.update);
    let raw = private_read(&path, &r.sha256)?;
    let mut b: Bundle = serde_json::from_slice(&raw).map_err(|e| bad(STAGE, e.to_string()))?;
    if b.kind != KIND
        || b.version != 1
        || b.scope != r.scope
        || b.workspace != r.workspace
        || b.update != r.update
        || b.project_path != r.project_path
        || !token_ok(&b.activation_token)
        || !uuid(&b.checkpoint)
        || !hash_ok(&b.checkpoint_sha256)
        || b.revision == 0
        || b.revision >= u64::MAX - 1
        || b.history.revision != b.revision
        || b.saved_revision > b.revision
        || !hash_ok(&b.saved_project_hash)
        || project_file_hash(Path::new(&b.project_path))? != b.saved_project_hash
        || b.loaded_schema > filmcraft_format::SCHEMA_VERSION
        || b.merge_key.as_ref().is_some_and(|v| v.len() > 2048)
        || b.editor.active_sequence.is_none()
    {
        return Err(bad(STAGE, "foreign, malformed or incompatible workspace bundle"));
    }
    if hash(&history_checkpoint::bytes(&b.history, 32 * 1024 * 1024)?) != b.checkpoint_sha256 {
        return Err(bad(STAGE, "bundled native history differs from the saved composer checkpoint"));
    }
    super::super::native_editor::preferences(&b.preferences)?;
    candidate.prefs = b.preferences;
    candidate.media.set_use_proxies(candidate.prefs.media.enable_proxies);
    let project = history_checkpoint::workspace_project(&b.history)?;
    crate::commands::install_project(&mut candidate, project, Some(r.project_path), true);
    candidate.saved_file = Some(crate::saved_file::Baseline { path: b.project_path.clone(), sha256: b.saved_project_hash.clone() });
    candidate.state = b.editor;
    candidate.state.clipboard = std::mem::take(&mut b.clipboard);
    candidate.state.clipboard_markers = std::mem::take(&mut b.clipboard_markers);
    candidate.revision = b.revision;
    candidate.saved_revision = b.revision; // clean prerequisite for shared history hydration
    let prepared = history_checkpoint::prepare_workspace(&candidate, &b.history, &r.scope, &r.workspace, &b.checkpoint)?;
    candidate.history = prepared.history;
    history_checkpoint::retain_history_media(&candidate);
    candidate.history.merge_key = b.merge_key;
    candidate.agent_bridge.tickets = prepared.tickets;
    candidate.agent_bridge.head = prepared.owned_head.then(|| (candidate.revision, Arc::downgrade(&candidate.project)));
    candidate.saved_revision = b.saved_revision;
    candidate.loaded_schema = b.loaded_schema;
    history_checkpoint::remember_workspace(&mut candidate, &r.workspace, &b.checkpoint, &b.checkpoint_sha256, &b.history.epoch)?;
    let receipt = super::hold_candidate(&mut candidate, r.scope, r.workspace, r.update, b.activation_token, b.saved_project_hash)?;
    *s = candidate;
    Ok(receipt)
}
