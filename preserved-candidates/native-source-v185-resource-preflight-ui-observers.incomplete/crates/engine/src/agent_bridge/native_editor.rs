//! Complete native editor payload shared by lifecycle snapshots. No edit engine.
use super::*;
use serde::{Deserialize, Serialize};
pub(super) mod command_journal;
pub(super) use command_journal::CommandJournal;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) editor: crate::EditorState,
    pub(super) clipboard: Vec<(filmcraft_project::TrackKind, usize, filmcraft_project::TrackItem)>,
    pub(super) clipboard_markers: Vec<filmcraft_project::Marker>,
    pub(super) preferences: crate::autosave::Preferences,
    pub(super) merge_key: Option<String>,
    pub(super) saved_revision: u64,
    pub(super) loaded_schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) auxiliary: Option<super::native_auxiliary::Snapshot>,
    // Absent in ordinary live editor/tab scopes and in legacy lifecycle files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) command_journal: Option<CommandJournal>,
}
pub(super) fn preferences(p: &crate::autosave::Preferences) -> Result<()> {
    let raw = serde_json::to_value(p).map_err(|e| bad("native.snapshotEditor", e.to_string()))?;
    if serde_json::to_value(crate::autosave::Preferences::from_value(raw.clone())).map_err(|e| bad("native.snapshotEditor", e.to_string()))? != raw {
        return Err(bad("native.snapshotEditor", "native preferences require migration or repair"));
    }
    Ok(())
}
impl Snapshot {
    pub(super) fn capture(s: &Session) -> Result<Self> {
        Self::capture_with_auxiliary(s, Some(super::native_auxiliary::Snapshot::capture(s)?))
    }
    pub(super) fn capture_complete(s: &Session) -> Result<Self> {
        let mut result = Self::capture(s)?;
        result.command_journal = Some(CommandJournal::capture(s)?);
        Ok(result)
    }
    pub(super) fn capture_complete_journal(s: &Session) -> Result<Self> {
        let mut result = Self::capture_journal(s)?;
        result.command_journal = Some(CommandJournal::capture(s)?);
        Ok(result)
    }
    // An ordinary autosave must retain the edit even while a job is running.
    // Live jobs and their owners are never manufactured as recovery data.
    // Busy journals retain the legacy editor payload; complete guarded handoff
    // still requires capture() and the original idle-writer fence.
    pub(super) fn capture_journal(s: &Session) -> Result<Self> {
        if super::workspace_update::require_idle_writer_jobs(s).is_err() { Self::capture_with_auxiliary(s, None) } else { Self::capture(s) }
    }
    fn capture_with_auxiliary(s: &Session, auxiliary: Option<super::native_auxiliary::Snapshot>) -> Result<Self> {
        let result = Self {
            editor: s.state.clone(),
            clipboard: s.state.clipboard.clone(),
            clipboard_markers: s.state.clipboard_markers.clone(),
            preferences: s.prefs.clone(),
            merge_key: s.history.merge_key.clone(),
            saved_revision: s.saved_revision,
            loaded_schema: s.loaded_schema,
            auxiliary,
            command_journal: None,
        };
        result.validate(s.revision, s.state.active_sequence.map(|v| v.0))?;
        Ok(result)
    }
    pub(super) fn validate(&self, revision: u64, sequence: Option<u64>) -> Result<()> {
        if self.saved_revision > revision
            || self.loaded_schema > filmcraft_format::SCHEMA_VERSION
            || self.merge_key.as_ref().is_some_and(|v| v.len() > 2048)
            || self.editor.active_sequence.map(|v| v.0) != sequence
        {
            return Err(bad("native.snapshotEditor", "native editor snapshot is incompatible"));
        }
        if let Some(aux) = &self.auxiliary {
            aux.validate()?;
        }
        if let Some(journal) = &self.command_journal {
            journal.validate()?;
        }
        preferences(&self.preferences)
    }
    pub(super) fn install(&self, s: &mut Session) -> Result<()> {
        s.prefs = self.preferences.clone();
        s.media.set_use_proxies(s.prefs.media.enable_proxies);
        s.state = self.editor.clone();
        s.state.clipboard = self.clipboard.clone();
        s.state.clipboard_markers = self.clipboard_markers.clone();
        s.loaded_schema = self.loaded_schema;
        if let Some(aux) = &self.auxiliary {
            aux.install(s)?;
        }
        Ok(())
    }
    // Only original cold constructors call this on their unpublished fresh
    // candidate. Ordinary live installs must never rewind this passive log.
    pub(super) fn install_cold(&self, s: &mut Session) -> Result<()> {
        if let Some(journal) = &self.command_journal {
            journal.require_empty_target(s)?;
        }
        self.install(s)?;
        if let Some(journal) = &self.command_journal {
            journal.install_cold(s)?;
        }
        Ok(())
    }
    pub(super) fn matches(&self, s: &Session) -> Result<bool> {
        // Legacy snapshots did not contain auxiliary data. Compare their
        // original scope without upgrading by-value evidence into a new claim.
        let mut current = if self.auxiliary.is_some() { Self::capture(s)? } else { Self::capture_with_auxiliary(s, None)? };
        if self.command_journal.is_some() {
            current.command_journal = Some(CommandJournal::capture(s)?);
        }
        Ok(history_checkpoint::bytes(&current, 80 * 1024 * 1024)? == history_checkpoint::bytes(self, 80 * 1024 * 1024)?)
    }
}
pub(super) fn require_fresh(s: &Session) -> Result<()> {
    let fresh = Session::new(s.services.clone());
    super::workspace_update::require_idle_writer_jobs(s)?;
    if s.path.is_some()
        || s.save_operation_held()
        || s.revision != 1
        || s.saved_revision != 1
        || s.worker_available
        || s.view_only
        || s.persistence.is_some()
        || s.agent_bridge.workspace_update.is_some()
        || !s.history.undo.is_empty()
        || !s.history.redo.is_empty()
        || s.history.merge_key.is_some()
        || !s.agent_bridge.tickets.is_empty()
        || !s.agent_bridge.history_bases.is_empty()
        || !s.agent_bridge.application_history.is_empty()
        || s.agent_bridge.application_ui.is_some()
        || s.agent_bridge.pending_application_capture.is_some()
        || s.agent_bridge.epoch.is_some()
        || !s.state.clipboard.is_empty()
        || !s.state.clipboard_markers.is_empty()
        || context(s)? != context(&fresh)?
        || history_checkpoint::project_bytes(&s.project, 32 * 1024 * 1024)? != history_checkpoint::project_bytes(&fresh.project, 32 * 1024 * 1024)?
    {
        return Err(bad("native.snapshotStartup", "fresh native startup required before project, history, worker, persistence or human interaction"));
    }
    Ok(())
}

// A scheduling hint only. It grants neither source evidence nor mutation or
// recovery authority; actual capture and cold startup still validate all bytes.
pub(super) fn recovery_state(s: &Session) -> Value {
    let fingerprint = Snapshot::capture(s)
        .and_then(|editor| history_checkpoint::bytes(&(s.path.as_ref(), s.revision, s.saved_file.as_ref(), editor), 16 * 1024 * 1024))
        .map(|raw| history_checkpoint::digest(&raw));
    let directory = s.persistence.as_ref().filter(|p| p.data_dir.is_absolute()).and_then(|p| p.data_dir.to_str());
    let ready = s.worker_available
        && directory.is_some()
        && s.saved_file.is_some()
        && operation_cancellation_available(s)
        && !workspace_update::held(s)
        && fingerprint.is_ok();
    json!({"kind":"native_editor_recovery_scheduling","version":1,"ready":ready,"sha256":fingerprint.ok(),"nativePid":std::process::id(),"dataDirectory":directory,"sourceValidation":"not-performed","writeAuthority":false})
}
