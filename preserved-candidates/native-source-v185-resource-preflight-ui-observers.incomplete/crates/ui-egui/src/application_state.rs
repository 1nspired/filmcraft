//! Passive application data for the ORIGINAL checkpoint/normal constructor.
//! Runtime callbacks/channels/weak grants are never serialized or replayed.
use crate::{Dialog, FilmcraftApp, Playback};
use crate::{
    dock::PanelKind,
    state::{Eyedropper, UiState},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineSkipped {
    seen_playhead: Option<(filmcraft_project::ItemId, f64)>,
    fit_empty: Option<f64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgramPlayback {
    speed: f64,
    looping: bool,
    stop_at: Option<filmcraft_time::Tick>,
    rest_at: Option<filmcraft_time::Tick>,
    was_playing: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphicsData {
    mode: String,
    reset_done: bool,
    closed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    kind: String,
    version: u8,
    ui: UiState,
    timeline_skipped: TimelineSkipped,
    eyedropper: Option<Eyedropper>,
    dialog: Option<Dialog>,
    recovery_choice: usize,
    shortcut_editor: crate::panels::shortcuts_dialog::EditorState,
    workspaces: crate::dock::WorkspacePrefs,
    timeline_view_of: Option<filmcraft_project::ItemId>,
    timeline_view_last: Option<filmcraft_project::SequenceView>,
    pending_panel_raise: Option<PanelKind>,
    stale_clipboard_media: Option<String>,
    poema: Option<crate::panels::poema::PassiveDraft>,
    program_playback: ProgramPlayback,
    source_playback: crate::source_playback::PassivePlayback,
    last_timeline_width: f32,
    status_seen: (String, f64),
    toast: Option<(String, f64)>,
    captured_ui_time: f64,
    ui_error: Option<String>,
    graphics_notice: Option<GraphicsData>,
}
fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let raw = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if raw.len() > MAX_BYTES {
        return Err("complete passive application UI exceeds original8MiB payload bound".into());
    }
    Ok(raw)
}
impl Snapshot {
    fn capture(app: &FilmcraftApp) -> Result<Self, String> {
        // This serializes readonly UI data. The original attachment API still
        // requires the real unpaused owner before any native capture/install.
        if app.native_dialog_open()
            || !app.synthetic.is_empty()
            || !app.input_waiters.is_empty()
            || app.value_press.is_some()
            || app.tl.drag.is_some()
            || app.tl.pressed.is_some()
        {
            return Err("finish the original live input before passive application capture".into());
        }
        let v = Self {
            kind: "native_passive_application_ui".into(),
            version: 1,
            ui: app.ui.clone(),
            timeline_skipped: TimelineSkipped { seen_playhead: app.ui.timeline.seen_playhead, fit_empty: app.ui.timeline.fit_empty },
            eyedropper: app.ui.eyedropper.clone(),
            dialog: app.dialog,
            recovery_choice: app.file_dialogs.recovery_choice,
            shortcut_editor: app.shortcut_editor.clone(),
            workspaces: app.workspaces.clone(),
            timeline_view_of: app.timeline_view_of,
            timeline_view_last: app.timeline_view_last.clone(),
            pending_panel_raise: app.pending_panel_raise,
            stale_clipboard_media: app.stale_clipboard_media.clone(),
            poema: crate::panels::poema::passive(app),
            program_playback: ProgramPlayback {
                speed: app.playback.speed,
                looping: app.playback.looping,
                stop_at: app.playback.stop_at,
                rest_at: app.playback.rest_at,
                was_playing: app.playback.playing,
            },
            source_playback: app.source_playback.passive(),
            last_timeline_width: app.last_timeline_width,
            status_seen: app.status_seen.clone(),
            toast: app.toast.clone(),
            captured_ui_time: app.application_clock_rebase.unwrap_or(app.last_ui_time),
            ui_error: app.ui_error.clone(),
            graphics_notice: app.graphics_notice.as_ref().map(|g| GraphicsData { mode: g.mode.clone(), reset_done: g.reset_done, closed: g.closed }),
        };
        v.validate()?;
        Ok(v)
    }
    fn validate(&self) -> Result<(), String> {
        if self.kind != "native_passive_application_ui"
            || self.version != 1
            || !self.captured_ui_time.is_finite()
            || !self.status_seen.1.is_finite()
            || self.toast.as_ref().is_some_and(|v| !v.1.is_finite())
            || !self.last_timeline_width.is_finite()
            || self.last_timeline_width <= 0.0
            || !self.program_playback.speed.is_finite()
            || self.timeline_skipped.seen_playhead.is_some_and(|v| !v.1.is_finite())
            || self.timeline_skipped.fit_empty.is_some_and(|v| !v.is_finite())
        {
            return Err("invalid finite passive application UI state".into());
        }
        self.source_playback.validate()?;
        crate::dock_shared::encode(&self.ui.dock, &self.ui.floating)?.validate().map_err(|e| e.to_string())?;
        // Closed typed roundtrip rejects hidden unknown/missing/defaulted data
        // and non-finite values that serde would otherwise render as null.
        let raw = bytes(self)?;
        let decoded: Self = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        if bytes(&decoded)? != raw {
            return Err("passive application state cannot roundtrip exactly".into());
        }
        Ok(())
    }
}
pub(crate) fn capture(app: &FilmcraftApp) -> Result<Value, String> {
    let snapshot = Snapshot::capture(app)?;
    let raw = bytes(&snapshot)?;
    let count = raw.len();
    let sha = format!("{:x}", Sha256::digest(&raw));
    Ok(json!({"kind":"native_passive_application_ui_proof","version":1,"bytes":count,"sha256":sha,"data":String::from_utf8(raw).map_err(|e|e.to_string())?}))
}
pub(crate) fn restore(app: &mut FilmcraftApp) -> Result<(), String> {
    let Some(proof) = app.session.saved_application_ui().map_err(|e| e.to_string())? else { return Ok(()) };
    let data = proof["data"].as_str().filter(|d| d.len() <= MAX_BYTES).ok_or("bounded native passive UI payload required")?;
    let snapshot: Snapshot = serde_json::from_str(data).map_err(|e| e.to_string())?;
    snapshot.validate()?;
    if bytes(&snapshot)?.as_slice() != data.as_bytes() {
        return Err("passive UI payload is not the exact closed native encoding".into());
    }
    let note = snapshot.poema.as_ref().map(|p| crate::panels::poema::restore_passive(app, p)).transpose()?;
    // Prepare everything before applying passive data. All native/control/input
    // owners remain those constructed by the original new(Session) path.
    app.ui = snapshot.ui;
    app.ui.timeline.seen_playhead = snapshot.timeline_skipped.seen_playhead;
    app.ui.timeline.fit_empty = snapshot.timeline_skipped.fit_empty;
    app.ui.eyedropper = snapshot.eyedropper;
    app.dialog = snapshot.dialog;
    app.file_dialogs.recovery_choice = snapshot.recovery_choice;
    app.shortcut_editor = snapshot.shortcut_editor;
    app.workspaces = snapshot.workspaces;
    app.timeline_view_of = snapshot.timeline_view_of;
    app.timeline_view_last = snapshot.timeline_view_last;
    app.pending_panel_raise = snapshot.pending_panel_raise;
    app.stale_clipboard_media = snapshot.stale_clipboard_media;
    app.poema = note;
    app.playback = Playback {
        speed: snapshot.program_playback.speed,
        looping: snapshot.program_playback.looping,
        stop_at: snapshot.program_playback.stop_at,
        rest_at: snapshot.program_playback.rest_at,
        ..Default::default()
    };
    app.source_playback.restore_passive(snapshot.source_playback);
    app.last_timeline_width = snapshot.last_timeline_width;
    app.status_seen = snapshot.status_seen;
    app.toast = snapshot.toast;
    app.ui_error = snapshot.ui_error;
    app.application_clock_rebase = Some(snapshot.captured_ui_time);
    // The complete recovered layout already passed original dock validation.
    // First preference application must not replace it with its named preset.
    app.workspace_restored = true;
    // graphics_notice's data is retained in the original immutable proof; the
    // fresh host supplies its real reset callback and current hardware mode.
    Ok(())
}
pub(crate) fn rebase_clock(app: &mut FilmcraftApp, now: f64) {
    let Some(old) = app.application_clock_rebase else {
        return;
    };
    let shift = now - old;
    let status = app.status_seen.1 + shift;
    let toast = app.toast.as_ref().map(|(_, time)| *time + shift);
    if !now.is_finite() || !shift.is_finite() || !status.is_finite() || toast.is_some_and(|time| !time.is_finite()) {
        let message = "The fresh UI clock cannot restore the saved timer values. The native project is preserved.".to_string();
        app.application_restore_error = Some(message.clone());
        app.ui_error = Some(message);
        return;
    }
    // Calculate every result before changing any timer or consuming its base.
    app.application_clock_rebase = None;
    app.status_seen.1 = status;
    if let (Some((_, time)), Some(value)) = (&mut app.toast, toast) {
        *time = value;
    }
}

#[cfg(test)]
mod tests;
