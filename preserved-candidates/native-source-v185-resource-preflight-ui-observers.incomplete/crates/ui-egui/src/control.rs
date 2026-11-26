//! Programmatic control of the running app (agents, tests, MCP bridge).
//!
//! Transport-agnostic: a transport thread (TCP in the desktop app) sends [`ControlRequest`]s; the
//! UI thread handles them between frames and replies with `{"ok":…, "result"|"error":…}`.
//!
//! Methods:
//! - `engine.execute {command, params}` / `ui.menu.invoke {id}`: run any engine or UI command
//! - `engine.commands`: list engine commands; `ui.menu.list`: menu tree with enablement
//! - `ui.inspect`: UI state + every registered widget (`elements`: id, label, rect)
//! - `ui.elements {prefix?}`: registered widgets (optionally filtered by id prefix)
//! - `ui.set {tool?, workspace?, mode?, theme?, focused?, playbackRes?, timeline?:{pps,scroll}}`
//! - `ui.panel.show {panel}` / `ui.panel.close {panel}`
//! - `ui.set {tts: {voice, pitch, pace, text…}}`: patch the Text to Speech panel draft
//! - `ui.click {id | x,y, button?, count?, modifiers?}` / `ui.move {x,y}` / `ui.scroll {x,y,dx,dy}`
//! - `ui.drag {from:{id|x,y}, to:{id|x,y}, steps?, modifiers?}`: synthetic press-move-release
//! - `ui.key {key, command?, shift?, alt?, ctrl?}` / `ui.type {text}`
//! - `ui.timeline.hit {x, y, modifiers?}`: what the timeline shows at a point (track, clip, edge,
//!   time) and `kind`, the trim a press of the current tool with `modifiers` starts there
//! - `ui.timeline.locate {clip, edge?}`: screen point of a clip body/edge (for drags)
//! - `ui.playback {action: play|stop|toggle, speed?}`
//! - `ui.screenshot {path?, panel?}`: PNG of the window (or one panel)
//! - `ui.resize {width, height}` / `ui.focus` / `app.quit`

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::state::{Mode, PlaybackRes, ThumbnailMode, Tool};

/// Prefix marking errors that may resolve after another frame.
pub const RETRY: &str = "\u{1}";

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, std::sync::mpsc::Receiver<Value>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { method: method.into(), params, reply: tx }, rx)
    }
}

pub enum Outcome {
    Done(Value),
    /// The exclusive native worker owns this request's one eventual reply.
    Pending,
    /// Try again on a later frame (e.g. the element is not on screen yet).
    Retry(String),
    /// Reply once queued synthetic input has been processed.
    AfterInput,
    Screenshot {
        path: Option<String>,
        crop: Option<[f32; 4]>,
    },
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}
fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

/// Modifier flags from `{modifiers: {ctrl, command, shift, alt}}` (or the same keys at the top
/// level), as the OS would report them pressed.
fn modifiers(p: &Value) -> egui::Modifiers {
    let m = p.get("modifiers").unwrap_or(p);
    let b = |k: &str| m.get(k).and_then(Value::as_bool).unwrap_or(false);
    as_pressed(egui::Modifiers { alt: b("alt"), ctrl: b("ctrl"), shift: b("shift"), mac_cmd: cfg!(target_os = "macos") && b("command"), command: b("command") })
}

/// `m` as egui-winit reports a physical press: off macOS Control is the primary modifier, so
/// `ctrl` and `command` are one key and a press of either carries both. A synthetic `Ctrl+Z` then
/// matches a `Cmd+Z` binding exactly as the real key does (#245).
fn as_pressed(mut m: egui::Modifiers) -> egui::Modifiers {
    if !cfg!(target_os = "macos") && (m.ctrl || m.command) {
        m.ctrl = true;
        m.command = true;
    }
    m
}

/// Resolve a point from `{id}` (element centre) or `{x, y}`.
fn point(app: &FilmcraftApp, p: &Value) -> Result<egui::Pos2, String> {
    if let Some(id) = p.get("id").and_then(Value::as_str) {
        let e = app.auto.find(id).ok_or_else(|| format!("{RETRY}no element `{id}` (see ui.elements)"))?;
        let fx = p.get("fx").and_then(Value::as_f64).unwrap_or(0.5) as f32;
        let fy = p.get("fy").and_then(Value::as_f64).unwrap_or(0.5) as f32;
        return Ok(egui::pos2(e.rect[0] + e.rect[2] * fx, e.rect[1] + e.rect[3] * fy));
    }
    let x = p.get("x").and_then(Value::as_f64).ok_or("need `id` or `x`,`y`")?;
    let y = p.get("y").and_then(Value::as_f64).ok_or("need `y`")?;
    Ok(egui::pos2(x as f32, y as f32))
}

// A first insert into an empty timeline schedules an automatic fit on the next UI pass.
// Do not mint a guard against the OLD view just before the frontend updates EditorState.
// Wait only for a read; never defer/retry an agent write or remove fields from its guard.
fn snapshot_view_pending(app: &FilmcraftApp) -> bool {
    let Some(active) = app.session.state.active_sequence else { return false };
    // A hidden Timeline cannot settle by drawing. Preserve its actual passive
    // UI state; use the ORIGINAL docking visibility instead of tab membership.
    let visible = app.ui.mode == Mode::Edit
        && match app.ui.keys.maximized {
            Some(panel) => panel == PanelKind::Timeline,
            None => app.ui.dock.is_visible(PanelKind::Timeline) || app.ui.floating.iter().any(|g| g.panels.get(g.active) == Some(&PanelKind::Timeline)),
        };
    if !visible {
        return false;
    }
    if app.timeline_view_of != Some(active) {
        return true;
    }
    let v = &app.ui.timeline;
    let first_clip_fit = v.fit_empty.is_some() && app.session.active_sequence().is_some_and(|q| q.duration() > filmcraft_time::Tick::ZERO);
    let unsaved_view = app.session.state.timeline_views.get(&active).is_none_or(|stored| {
        stored.pps != v.target_pps
            || stored.scroll != v.target_scroll
            || stored.v_scroll != v.v_scroll
            || stored.a_scroll != v.a_scroll
            || stored.video_track_h != v.video_track_h
            || stored.audio_track_h != v.audio_track_h
    });
    v.fit_pending || first_clip_fit || v.animating() || app.timeline_still < 2 || unsaved_view
}

pub fn handle(app: &mut FilmcraftApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(error) = &app.application_restore_error {
        if !matches!(req.method.as_str(), "ui.state" | "ui.screenshot" | "app.quit") {
            return err(error.clone());
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(outcome) = crate::native_writer::intercept(app, ctx, req) {
        return outcome;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(result) = crate::application_tabs::handle(app, ctx, &req.method, &req.params) {
        return match result {
            Ok(value) => ok(value),
            Err(error) => err(error),
        };
    }
    if let Some(result) = crate::media_intake::handle(app, ctx, &req.method, &req.params) {
        return match result {
            Ok(v) => ok(v),
            Err(e) => err(e),
        };
    }
    let p = &req.params;
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    match req.method.as_str() {
        "ui.poemaComposer" => {
            if req.params.as_object().is_none_or(|p| !p.is_empty()) {
                return err("ui.poemaComposer expects empty parameters");
            }
            ok(match app.composer_panel.as_ref() {
                None => json!({"enabled": false}),
                Some(panel) => {
                    let mut status = json!({"enabled": true, "binding": panel.binding,
                    "lifecycle":{"native_instance_binding":true,"update_preparation":1,"new_project":1,"deferred_update_presentation":true},
                    "close": panel.close, "update": panel.update, "newProject":panel.new_project,
                    "connected": panel.connected, "focused": panel.focused, "status": panel.status,
                    "visible": panel.visible, "hostState": panel.host_state,
                    "docked": app.ui.dock.contains(PanelKind::PoemaComposer),
                    "rect": panel.rect.map(|r| [r.min.x,r.min.y,r.width(),r.height()])});
                    if panel.binding.version == 2 {
                        status["documentMatched"] = json!(panel.binding.matches_session(&app.session).is_ok_and(|v| v));
                        status["lifecycle"]["independent_project_binding"] = json!(true);
                    }
                    status
                }
            })
        }

        "ui.poemaComposer.update" => {
            if p.as_object().is_none_or(|v| v.len() != 3 || !["action", "id", "binding"].iter().all(|k| v.contains_key(*k))) {
                return err("native update preparation requires action, id and binding");
            }
            let Some(action) = s("action") else {
                return err("missing native update action");
            };
            let Some(id) = s("id") else {
                return err("missing native update identity");
            };
            let binding: crate::composer_panel::Binding = match serde_json::from_value(p["binding"].clone()) {
                Ok(v) => v,
                Err(e) => return err(e),
            };
            if let Err(e) = binding.validate() {
                return err(e);
            }
            let Some(panel) = app.composer_panel.as_mut() else {
                return err("no embedded composer");
            };
            if panel.binding != binding || !panel.document_matches(&app.session).is_ok_and(|v| v) {
                return err("native update preparation belongs to another workspace");
            }
            if let Err(e) = panel.request_update(id, action) {
                return err(e);
            }
            ctx.request_repaint();
            ok(json!({"binding":panel.binding,"update":panel.update}))
        }

        "engine.execute" | "ui.menu.invoke" => {
            let Some(id) = s("command").or(s("id")) else { return err("missing `command`") };
            // A cold candidate is deliberately paused before its first editable UI
            // pass. Its private recovery basis must be available to reconcile an
            // unknown activation; waiting for that UI would deadlock recovery.
            // Ordinary reads and every editable/owner view keep the same gate.
            let paused_recovery_basis = id == "agent.readHistoryCheckpointBasis"
                && app.session.workspace_update_status().is_some_and(|v| v["role"] == "candidate" && v["status"] == "candidate_ready");
            if matches!(
                id,
                "agent.snapshot" | "agent.readProject" | "agent.readSourceLibrary" | "agent.readMulticamInputs" | "agent.readHistoryCheckpointBasis"
            ) && !paused_recovery_basis
                && snapshot_view_pending(app)
            {
                if ctx.input(|i| i.time) - app.last_ui_time > 0.25 {
                    app.raise_for_control(ctx);
                }
                return Outcome::Retry("the editor view is settling; no read ticket or write was issued".into());
            }
            let params = p.get("params").cloned().unwrap_or(json!({}));
            match crate::menus::invoke(app, ctx, id, params) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
        "engine.commands" => match app.session.execute("command.list", json!({})) {
            Ok(v) => ok(v),
            Err(e) => err(e),
        },
        "ui.menu.list" => ok(serde_json::to_value(crate::menus::menu_items(app)).unwrap_or_default()),
        "ui.inspect" => ok(inspect(app, ctx)),
        "perf.stats" => ok(crate::perf::stats(app)),
        "ui.elements" => {
            if app.timeline_still < 2 {
                // rects of timeline elements are still moving (e.g. the fit after opening a sequence)
                return Outcome::Retry("the timeline is still zooming".into());
            }
            let prefix = s("prefix").unwrap_or("");
            ok(serde_json::to_value(app.auto.query(prefix)).unwrap_or_default())
        }
        "ui.set" => {
            if let Some(patch) = p.get("poema")
                && let Err(e) = crate::panels::poema::patch(app, patch)
            {
                return err(e);
            }
            // Text to Speech panel draft (voice, pitch, pace, text…), merged into the current one
            if let Some(patch) = p.get("tts") {
                let mut cur = serde_json::to_value(&app.ui.tts).unwrap_or_default();
                merge(&mut cur, patch);
                match serde_json::from_value(cur) {
                    Ok(d) => app.ui.tts = d,
                    Err(e) => return err(format!("bad `tts`: {e}")),
                }
            }
            if let Some(t) = s("tool") {
                match Tool::from_name(t) {
                    Some(t) => app.ui.tool = t,
                    None => return err(format!("unknown tool `{t}`")),
                }
            }
            if let Some(w) = s("workspace") {
                match crate::dock::find(&app.workspaces, w) {
                    Some(w) => app.set_workspace(&w),
                    None => return err(format!("unknown workspace `{w}`")),
                }
            }
            if let Some(m) = s("mode") {
                app.ui.mode = match m.to_ascii_lowercase().as_str() {
                    "import" => Mode::Import,
                    "export" => Mode::Export,
                    _ => Mode::Edit,
                };
            }
            if let Some(th) = s("theme") {
                match crate::theme::ThemeKind::from_name(th) {
                    Some(k) => crate::panels::settings::set_theme(app, ctx, k),
                    None => return err("unknown theme (dark, medium, light)"),
                }
            }
            // the open Settings dialog: page and draft values (`{"page": id, "values": {key: value}}`)
            if let Some(st) = p.get("settings")
                && let Err(e) = crate::panels::settings::patch(app, st)
            {
                return err(e);
            }
            if let Some(f) = s("focused").and_then(PanelKind::from_name) {
                app.ui.focused = f;
            }
            if let Some(r) = s("playbackRes") {
                app.ui.program.res = PlaybackRes::ALL
                    .into_iter()
                    .find(|x| x.label().eq_ignore_ascii_case(r) || format!("{x:?}").eq_ignore_ascii_case(r))
                    .unwrap_or(app.ui.program.res);
            }
            if let Some(tl) = p.get("timeline") {
                if let Some(v) = tl.get("pps").and_then(Value::as_f64) {
                    app.ui.timeline.target_pps = v;
                }
                if let Some(v) = tl.get("scroll").and_then(Value::as_f64) {
                    app.ui.timeline.target_scroll = v;
                }
                if tl.get("fit").and_then(Value::as_bool) == Some(true) {
                    app.ui.timeline.fit_pending = true;
                }
                if let Some(v) = tl.get("videoTrackHeight").and_then(Value::as_f64) {
                    app.ui.timeline.video_track_h = v as f32;
                }
                if let Some(v) = tl.get("audioTrackHeight").and_then(Value::as_f64) {
                    app.ui.timeline.audio_track_h = v as f32;
                }
                // Show Video Thumbnails: "off" or a mode ("head", "headAndTail", "continuous")
                if let Some(m) = tl.get("thumbnails").and_then(Value::as_str) {
                    if m.eq_ignore_ascii_case("off") {
                        app.ui.timeline.show_thumbnails = false;
                    } else {
                        let Some(mode) = ThumbnailMode::from_name(m) else {
                            return err(format!("unknown thumbnail mode `{m}` (off, head, headAndTail, continuous)"));
                        };
                        app.ui.timeline.show_thumbnails = true;
                        app.ui.timeline.thumbnail_mode = mode;
                    }
                }
            }
            if let Some(q) = s("effectsSearch") {
                app.ui.effects_search = q.to_string();
            }
            // Effect Controls keyframe selection (`[{clip, effect, param, mask?, time}]`), replaced;
            // references to missing keyframes are dropped by the panel
            if let Some(v) = p.get("keyframeSelection") {
                match serde_json::from_value(v.clone()) {
                    Ok(sel) => app.ui.keyframe_selection = sel,
                    Err(e) => return err(format!("`keyframeSelection`: {e}")),
                }
            }
            if let Some(v) = p.get("safeMargins").and_then(Value::as_bool) {
                app.ui.program.safe_margins = v;
            }
            // Monitor view state (serde fields of `MonitorView`), merged into the current state.
            for k in ["program", "source"] {
                let Some(patch) = p.get(k).and_then(Value::as_object) else { continue };
                let mv = if k == "program" { &mut app.ui.program } else { &mut app.ui.source };
                let mut cur = serde_json::to_value(&*mv).unwrap_or_default();
                if let Some(o) = cur.as_object_mut() {
                    for (f, v) in patch {
                        o.insert(f.clone(), v.clone());
                    }
                }
                match serde_json::from_value(cur) {
                    Ok(m) => *mv = m,
                    Err(e) => return err(format!("`{k}`: {e}")),
                }
            }
            // Export mode state (`panels::export_mode::ExportUi`), deep-merged; `"preset": name`
            // applies that preset's settings first
            if let Some(patch) = p.get("export").filter(|v| v.is_object()) {
                if let Some(name) = patch.get("preset").and_then(Value::as_str)
                    && name != crate::panels::export_mode::CUSTOM
                    && !crate::panels::export_mode::apply_preset(app, name)
                {
                    return err(format!("no export preset named `{name}`"));
                }
                let mut cur = serde_json::to_value(&app.ui.export).unwrap_or_default();
                merge(&mut cur, patch);
                match serde_json::from_value(cur) {
                    Ok(e) => app.ui.export = e,
                    Err(e) => return err(format!("`export`: {e}")),
                }
            }
            // panel settings (`panels::panel_state`): {"scopes": {...}, "timecode": {...}, …}, merged
            if let Some(patch) = p.get("panels") {
                let mut cur = serde_json::to_value(&app.ui.panels).unwrap_or_default();
                merge(&mut cur, patch);
                match serde_json::from_value(cur) {
                    Ok(v) => app.ui.panels = v,
                    Err(e) => return err(format!("`panels`: {e}")),
                }
            }
            // Project panel / Media Browser view state (`panels::project`, `panels::media_browser`), merged
            for (k, which) in [("projectPanel", 0), ("mediaBrowser", 1)] {
                let Some(patch) = p.get(k) else { continue };
                let mut cur =
                    if which == 0 { serde_json::to_value(&app.ui.project_panel) } else { serde_json::to_value(&app.ui.media_browser) }.unwrap_or_default();
                merge(&mut cur, patch);
                let r = if which == 0 {
                    serde_json::from_value(cur).map(|v| app.ui.project_panel = v)
                } else {
                    serde_json::from_value(cur).map(|v| app.ui.media_browser = v)
                };
                if let Err(e) = r {
                    return err(format!("`{k}`: {e}"));
                }
            }
            // Essential Graphics ▸ Browse and the template / font dialogs (`panels::graphics_templates`), merged
            if let Some(patch) = p.get("gfxTemplates") {
                let mut cur = serde_json::to_value(&app.ui.gfx_templates).unwrap_or_default();
                merge(&mut cur, patch);
                match serde_json::from_value(cur) {
                    Ok(v) => app.ui.gfx_templates = v,
                    Err(e) => return err(format!("`gfxTemplates`: {e}")),
                }
            }
            // the Type tool's text editing state: {"clip", "layer", "caret", "anchor"} (byte offsets) or null
            if let Some(v) = p.get("gfxEdit") {
                match serde_json::from_value(v.clone()) {
                    Ok(e) => app.ui.gfx_edit = e,
                    Err(e) => return err(format!("`gfxEdit`: {e}")),
                }
            }
            // fields of the open Edit / Clip / File dialog (`panels::clip_dialogs`)
            if let Some(m) = p.get("clipDialog").and_then(Value::as_object) {
                let Some(d) = app.ui.clip_dialog.as_mut() else { return err("no clip dialog is open") };
                for (k, v) in m {
                    d.params[k.as_str()] = v.clone();
                }
            }
            // fields of the open M3.11 menu dialog (`panels::menu_dialogs`)
            if let Some(m) = p.get("menuDialog").and_then(Value::as_object) {
                let Some(d) = app.ui.extras.dialog.as_mut() else { return err("no menu dialog is open") };
                for (k, v) in m {
                    d.params[k.as_str()] = v.clone();
                }
            }
            ok(Value::Null)
        }
        "ui.panel.show" | "ui.panel.close" => {
            let Some(panel) = s("panel").and_then(PanelKind::from_name) else { return err("unknown `panel`") };
            if req.method == "ui.panel.show" {
                app.show_panel(panel);
            } else {
                if let Err(error) = app.apply_dock(craft_ui::docking::Action::Close { panel }) {
                    return err(&error);
                }
            }
            ok(Value::Null)
        }
        "ui.click" | "ui.move" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) if e.starts_with(RETRY) => return Outcome::Retry(e.trim_start_matches(RETRY).to_string()),
                Err(e) => return err(e),
            };
            let m = modifiers(p);
            app.synthetic.push(egui::Event::ModifiersChanged(m));
            app.synthetic.push(egui::Event::PointerMoved(pos));
            if req.method == "ui.click" {
                let button = match s("button") {
                    Some("right") => egui::PointerButton::Secondary,
                    Some("middle") => egui::PointerButton::Middle,
                    _ => egui::PointerButton::Primary,
                };
                let count = p.get("count").and_then(Value::as_u64).unwrap_or(1);
                for _ in 0..count {
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers: m });
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers: m });
                }
            }
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.drag" => {
            let (Some(from), Some(to)) = (p.get("from"), p.get("to")) else { return err("need `from` and `to`") };
            let (a, b) = match (point(app, from), point(app, to)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) if e.starts_with(RETRY) => return Outcome::Retry(e.trim_start_matches(RETRY).to_string()),
                (Err(e), _) | (_, Err(e)) => return err(e),
            };
            let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(12).max(2);
            let m = modifiers(p);
            app.synthetic.push(egui::Event::ModifiersChanged(m));
            app.synthetic.push(egui::Event::PointerMoved(a));
            app.synthetic.push(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
            for i in 1..=steps {
                let f = i as f32 / steps as f32;
                app.synthetic.push(egui::Event::PointerMoved(a + (b - a) * f));
            }
            app.synthetic.push(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.scroll" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            let dx = p.get("dx").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let dy = p.get("dy").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            app.synthetic.push(egui::Event::ModifiersChanged(modifiers(p)));
            app.synthetic.push(egui::Event::PointerMoved(pos));
            app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(dx, dy),
                modifiers: modifiers(p),
                phase: egui::TouchPhase::Move,
            });
            app.synthetic.push(egui::Event::PointerButton {
                pos: egui::pos2(-100.0, -100.0),
                button: egui::PointerButton::Extra1,
                pressed: false,
                modifiers: Default::default(),
            });
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.key" => {
            let Some(name) = s("key") else { return err("missing `key`") };
            let Some((m, key)) = crate::menus::parse_shortcut(name) else { return err(format!("unknown key `{name}`")) };
            let m = as_pressed(m | modifiers(p));
            app.synthetic.push(egui::Event::ModifiersChanged(m));
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m });
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m });
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.type" => {
            let Some(text) = s("text") else { return err("missing `text`") };
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            app.synthetic.push(egui::Event::Text(text.to_string()));
            app.synthetic.push(egui::Event::Key { key: egui::Key::F35, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
            Outcome::AfterInput
        }
        "ui.timeline.hit" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            ok(crate::panels::timeline::hit_json(app, pos, modifiers(p)))
        }
        "ui.timeline.locate" => {
            let Some(c) = p.get("clip").and_then(Value::as_u64) else { return err("need `clip`") };
            match crate::panels::timeline::locate(app, c, s("edge")) {
                Some((x, y)) => ok(json!({"x": x, "y": y})),
                None => err("clip not visible in the timeline"),
            }
        }
        "ui.playback" => {
            match s("action").unwrap_or("toggle") {
                "play" => app.play(p.get("speed").and_then(Value::as_f64).unwrap_or(1.0)),
                "stop" => app.stop(),
                _ => app.toggle_play(1.0),
            }
            ok(json!({"playing": app.playback.playing, "playhead": app.session.playhead().0}))
        }
        "ui.screenshot" => {
            let crop = s("panel").and_then(PanelKind::from_name).and_then(|pk| app.auto.find(&format!("panel.{}", pk.id())).map(|e| e.rect));
            let crop = crop.or_else(|| s("id").and_then(|id| app.auto.find(id).map(|e| e.rect)));
            // Bring the window on screen so a frame is presented, without taking keyboard focus.
            app.raise_for_control(ctx);
            Outcome::Screenshot { path: s("path").map(str::to_string), crop }
        }
        "ui.resize" => {
            let w = p.get("width").and_then(Value::as_f64).unwrap_or(1600.0) as f32;
            let h = p.get("height").and_then(Value::as_f64).unwrap_or(1000.0) as f32;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            ok(Value::Null)
        }
        "ui.focus" => {
            if p.is_null() || p.as_object().is_some_and(|object| object.is_empty()) {
                // Existing explicit request: activate the app and take OS keyboard focus.
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                return ok(Value::Null);
            }
            let Some(object) = p.as_object().filter(|object| object.len() == 1 && object.contains_key("id")) else {
                return err("ui.focus expects {} or {id: string}");
            };
            let Some(id) = object.get("id").and_then(Value::as_str) else { return err("ui.focus needs a string `id`") };
            let Some(element) = app.auto.find(id) else { return err(format!("no element `{id}` (see ui.elements)")) };
            let Some(widget_id) = element.response_id else { return err(format!("element `{id}` has no focusable widget")) };
            let Some(response) = ctx.read_response(widget_id) else { return err(format!("widget `{id}` is not present")) };
            if !response.enabled()
                || !response.sense.is_focusable()
                || !response.interact_rect.is_finite()
                || !response.interact_rect.is_positive()
                || !response.interact_rect.intersects(ctx.content_rect())
            {
                return err(format!("widget `{id}` is not enabled, visible and focusable"));
            }
            // A widget target never activates the native window. The next ui.inspect reports
            // actual focus ownership separately from this request acknowledgement.
            response.request_focus();
            ok(json!({"focusRequested": id, "widgetId": format!("{:016x}", widget_id.value())}))
        }
        "app.quit" => {
            // scripted quits don't wait on the save prompt; unsaved changes stay in the recovery journal
            app.quit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ok(Value::Null)
        }
        m => err(format!("unknown method `{m}`")),
    }
}

/// Merge `patch` into `v` (objects recursively; other values replace).
fn merge(v: &mut Value, patch: &Value) {
    match (v, patch) {
        (Value::Object(o), Value::Object(p)) => {
            for (k, pv) in p {
                match o.get_mut(k) {
                    Some(cur) => merge(cur, pv),
                    None => {
                        o.insert(k.clone(), pv.clone());
                    }
                }
            }
        }
        (v, p) => *v = p.clone(),
    }
}

pub fn inspect(app: &FilmcraftApp, ctx: &egui::Context) -> Value {
    let size = ctx.content_rect().size();
    let keyboard_focus = ctx.memory(|memory| memory.focused()).map(|id| {
        let elements: Vec<_> = app.auto.previous.iter().filter(|element| element.response_id == Some(id)).collect();
        json!({
            "widgetId": format!("{:016x}", id.value()),
            "elementIds": elements.iter().map(|element| element.id.as_str()).collect::<Vec<_>>(),
            "label": elements.first().map(|element| element.label.as_str()),
            "active": ctx.read_response(id).is_some_and(|response| response.has_focus()),
        })
    });
    json!({
        "window": [size.x, size.y],
        "pixelsPerPoint": ctx.pixels_per_point(),
        "fps": app.fps,
        "nativeWrite": native_write_status(app),
        "keyboardFocus": keyboard_focus,
        "input": {"mode": if app.control_input_only { "controlOnly" } else { "interactive" },
            "nativeWindowFocused": app.native_window_focused, "effectiveWindowFocused": ctx.input(|input| input.focused)},
        "ui": app.ui,
        "poema": crate::panels::poema::inspect(app),
        "sourcePlayback": {"playing": app.source_playback.clock.playing, "audioClock": app.source_playback.clock.audio_clock, "playhead": app.session.state.source_playhead.0},
        "playback": {"playing": app.playback.playing, "speed": app.playback.speed, "loop": app.playback.looping, "audioClock": app.playback.audio_clock, "dropped": app.playback.meter.counts().1, "shown": app.playback.meter.counts().0, "preroll": app.playback.preroll.is_some()},
        "playhead": app.session.playhead().0,
        "activeSequence": app.session.state.active_sequence.map(|i| i.0),
        "selection": app.session.state.selection.iter().map(|c| c.0).collect::<Vec<_>>(),
        "elements": app.auto.previous.len(),
        "dialog": app.dialog.map(|d| format!("{d:?}")),
    })
}

fn native_write_status(app: &FilmcraftApp) -> Value {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::native_writer::inspect(app)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Value::Null
    }
}

/// Save a screenshot (optionally cropped to a rect in points) as PNG; returns the reply JSON.
pub fn save_screenshot(ctx: &egui::Context, image: &egui::ColorImage, path: Option<&str>, crop: Option<[f32; 4]>) -> Value {
    let ppp = ctx.pixels_per_point();
    let [w, h] = image.size;
    let (x0, y0, cw, ch) = match crop {
        Some([x, y, cw, ch]) => {
            let x0 = ((x * ppp) as usize).min(w);
            let y0 = ((y * ppp) as usize).min(h);
            (x0, y0, ((cw * ppp) as usize).min(w - x0), ((ch * ppp) as usize).min(h - y0))
        }
        None => (0, 0, w, h),
    };
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            let c = image.pixels[y * w + x];
            rgba.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
    }
    if cfg!(target_arch = "wasm32") {
        // No filesystem on the web: the PNG comes back in the reply.
        return match encode_png(&rgba, cw as u32, ch as u32) {
            Ok(png) => json!({"ok": true, "result": {"pngBase64": base64(&png), "width": cw, "height": ch}}),
            Err(e) => json!({"ok": false, "error": e}),
        };
    }
    let path = path.map(str::to_string).unwrap_or_else(|| filmcraft_engine::temp_dir().join("filmcraft-screenshot.png").to_string_lossy().to_string());
    match encode_png(&rgba, cw as u32, ch as u32) {
        Ok(png) => match std::fs::write(&path, png) {
            Ok(()) => json!({"ok": true, "result": {"path": path, "width": cw, "height": ch}}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Minimal PNG encoder (stored deflate blocks, no compression dependency).
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = Vec::with_capacity(4 + data.len());
        c.extend_from_slice(ty);
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    fn crc(d: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in d {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    // zlib stream with stored blocks
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod base64_tests {
    #[test]
    fn rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::base64(i.as_bytes()), o);
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    fn app() -> FilmcraftApp {
        let mut session = filmcraft_engine::Session::default();
        session.execute("file.openDemoProject", json!({})).unwrap();
        let mut app = FilmcraftApp::new(session);
        app.timeline_view_of = app.session.state.active_sequence;
        app.timeline_still = 2;
        app.ui.timeline.fit_pending = false;
        app.ui.timeline.fit_empty = None;
        let v = &app.ui.timeline;
        let view = filmcraft_engine::project::SequenceView {
            pps: v.target_pps,
            scroll: v.target_scroll,
            v_scroll: v.v_scroll,
            a_scroll: v.a_scroll,
            video_track_h: v.video_track_h,
            audio_track_h: v.audio_track_h,
        };
        app.session.state.timeline_views.insert(app.session.state.active_sequence.unwrap(), view);
        app
    }
    #[test]
    fn snapshot_waits_for_the_first_nonempty_fit_before_minting_a_ticket() {
        let mut app = app();
        app.ui.timeline.fit_empty = Some(app.ui.timeline.target_pps);
        app.last_ui_time = -1.0;
        let raises = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let raised = raises.clone();
        app.hooks.raise_without_focus = Some(Box::new(move || {
            raised.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
        let before = app.session.project.clone();
        let revision = app.session.revision;
        let (request, _) =
            ControlRequest::new("engine.execute", json!({"command":"agent.snapshot","params":{"token":"guard-test-ticket-000000000000000000000001"}}));
        assert!(matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Retry(_)));
        assert_eq!(raises.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        app.ui.timeline.fit_empty = None;
        assert!(matches!(handle(&mut app,&egui::Context::default(),&request),Outcome::Done(v) if v["ok"]==true));
        assert_eq!(raises.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
    #[test]
    fn paused_root_candidate_exposes_only_private_recovery_basis_before_first_view() {
        use filmcraft_engine::{Session, autosave::AutosaveConfig, session_worker::SessionWorker};
        let root = std::env::temp_dir().join(format!(
            "filmcraft-paused-candidate-view-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let workspace = "11111111-1111-4111-8111-111111111111";
        let update = "22222222-2222-4222-8222-222222222222";
        let checkpoint = "33333333-3333-4333-8333-333333333333";
        let scope = json!({"projectId":"p_paused_view","workspaceId":workspace});
        let file = root.join("Generated.fcproj");
        let mut source = Session::default();
        source.execute("file.newProject", json!({"name":"Generated paused view","workspace":scope})).unwrap();
        source.execute("file.newSequence", json!({"name":"Original timeline","fps":24})).unwrap();
        let mut cfg = AutosaveConfig::new(root.join("source-state"));
        cfg.journal_debounce = std::time::Duration::from_secs(600);
        source.start_autosave(cfg).unwrap();
        source.execute("file.saveAs", json!({"path":file})).unwrap();
        let worker = SessionWorker::new().unwrap();
        worker.install(&mut source).unwrap();
        source.agent_operation_control().enable_transport();
        let token = |n: usize| format!("{n:064x}");
        let basis = |s: &mut Session, n| s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope})).unwrap();
        basis(&mut source, 1);
        let saved = source
            .execute("agent.saveHistoryCheckpoint", json!({"token":token(1),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}))
            .unwrap();
        basis(&mut source, 2);
        source.execute("agent.beginWorkspaceUpdate", json!({"token":token(2),"scope":scope,"workspace":workspace,"update":update})).unwrap();
        basis(&mut source, 3);
        let staged = source.execute("agent.stageProjectRecovery", json!({"token":token(3),"scope":scope,"workspace":workspace,"update":update,"pauseToken":token(2),"checkpoint":checkpoint,"checkpointSha256":saved["checkpoint"]["sha256"]})).unwrap();
        basis(&mut source, 4);
        source
            .execute(
                "agent.retireWorkspaceUpdate",
                json!({"token":token(4),"scope":scope,"workspace":workspace,"update":update,"pauseToken":token(2),"stageSha256":staged["sha256"]}),
            )
            .unwrap();
        source.shutdown();
        drop(source);
        let mut candidate = Session::default();
        candidate
            .open_project_recovery(
                &json!({"projectPath":file,"scope":scope,"workspace":workspace,"update":update,"sha256":staged["sha256"],"activationToken":token(10)}),
            )
            .unwrap();
        candidate.configure_workspace_update_persistence(AutosaveConfig::new(root.join("candidate-state"))).unwrap();
        // A production candidate installs its own cancellation transport before
        // activation. Reuse that real setup rather than bypassing its fence.
        let candidate_worker = SessionWorker::new().unwrap();
        candidate_worker.install(&mut candidate).unwrap();
        candidate.agent_operation_control().enable_transport();
        let mut app = FilmcraftApp::new(candidate);
        app.timeline_view_of = None;
        assert!(snapshot_view_pending(&app));
        let project = app.session.project.clone();
        let state = serde_json::to_value(&app.session.state).unwrap();
        let revision = app.session.revision;
        let (request, _) =
            ControlRequest::new("engine.execute", json!({"command":"agent.readHistoryCheckpointBasis","params":{"token":token(11),"scope":scope}}));
        assert!(
            matches!(handle(&mut app,&egui::Context::default(),&request),Outcome::Done(v) if v["ok"]==true && v["result"]["kind"]=="native_project_history_checkpoint_basis")
        );
        assert_eq!(app.session.project, project);
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.state).unwrap(), state);
        let ready = app.session.workspace_update_status().unwrap();
        let private = json!({"token":token(15),"scope":scope,"workspace":workspace,"update":update,"projectPath":file,
            "activationToken":ready["token"],"nativeSession":ready["nativeSession"],"revision":ready["revision"],
            "sequence":saved["sequence"],"checkpoint":checkpoint,"sha256":saved["checkpoint"]["sha256"]});
        let (request, _) = ControlRequest::new("engine.execute", json!({"command":"agent.inspectPausedWorkspaceHistory","params":private}));
        assert!(matches!(handle(&mut app,&egui::Context::default(),&request),Outcome::Done(v)
            if v["ok"]==true && v["result"]["kind"]=="native_paused_workspace_history" && v["result"]["editingPaused"]==true));
        let (ordinary, _) =
            ControlRequest::new("engine.execute", json!({"command":"agent.readProject","params":{"token":token(16),"scope":scope,"projectPath":file}}));
        assert!(matches!(handle(&mut app, &egui::Context::default(), &ordinary), Outcome::Retry(_)));
        // The lifecycle fence rejects ordinary reads even before their view
        // gate. The private basis exception must not widen that fence.
        app.timeline_view_of = None;
        assert!(snapshot_view_pending(&app));
        for (command, n) in [("agent.snapshot", 12), ("agent.readSourceLibrary", 13)] {
            assert!(!app.session.workspace_update_allows(command));
            let (ordinary, _) = ControlRequest::new("engine.execute", json!({"command":command,"params":{"token":token(n),"scope":scope}}));
            assert!(matches!(handle(&mut app, &egui::Context::default(), &ordinary), Outcome::Done(v)
                if v["ok"] == false && v["outcome"] == "rejected" && v["error"].as_str().is_some_and(|e| e.contains("paused editing"))));
        }
        assert_eq!(app.session.project, project);
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.state).unwrap(), state);
        app.session.execute("agent.activateWorkspaceUpdate", json!({"token":token(10),"scope":scope,"workspace":workspace,"update":update,"nativeSession":app.session.workspace_update_status().unwrap()["nativeSession"]})).unwrap();
        app.timeline_view_of = None;
        let (active, _) =
            ControlRequest::new("engine.execute", json!({"command":"agent.readHistoryCheckpointBasis","params":{"token":token(14),"scope":scope}}));
        assert!(matches!(handle(&mut app, &egui::Context::default(), &active), Outcome::Retry(_)));
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn metadata_checkpoint_basis_waits_for_first_view_then_keeps_the_complete_view_guard() {
        let mut app = app();
        let directory = std::env::temp_dir().join(format!(
            "filmcraft-checkpoint-view-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let file = directory.join("generated.fcproj");
        let scope = json!({"projectId":"p_view_guard","workspaceId":"11111111-1111-4111-8111-111111111111"});
        app.session.execute("file.newProject", json!({"name":"Generated view guard","workspace":scope})).unwrap();
        app.session.execute("file.newSequence", json!({"name":"Generated initial timeline","fps":24})).unwrap();
        app.session.execute("file.save", json!({"path":file})).unwrap();
        let active = app.session.state.active_sequence.unwrap();
        let v = &app.ui.timeline;
        app.session.state.timeline_views.insert(
            active,
            filmcraft_engine::project::SequenceView {
                pps: v.target_pps,
                scroll: v.target_scroll,
                v_scroll: v.v_scroll,
                a_scroll: v.a_scroll,
                video_track_h: v.video_track_h,
                audio_track_h: v.audio_track_h,
            },
        );
        let token = "view-checkpoint-test-basis-00000000000000000001";
        let (request, _) = ControlRequest::new("engine.execute", json!({"command":"agent.readHistoryCheckpointBasis","params":{"token":token,"scope":scope}}));
        let before = app.session.project.clone();
        let revision = app.session.revision;
        app.timeline_view_of = None;
        assert!(matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Retry(_)));
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        app.timeline_view_of = Some(active);
        // The same token can now be issued: the deferred read minted nothing.
        assert!(
            matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Done(v) if v["ok"]==true && v["result"]["kind"]=="native_project_history_checkpoint_basis")
        );
        app.session.state.timeline_views.get_mut(&active).unwrap().scroll += 1.0;
        let workspace = "11111111-1111-4111-8111-111111111111";
        let checkpoint = "22222222-2222-4222-8222-222222222222";
        let error = app
            .session
            .execute("agent.saveHistoryCheckpoint", json!({"token":token,"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}))
            .unwrap_err();
        assert!(error.to_string().contains("changed before persistence"), "{error}");
        assert!(!directory.join(format!(".poema-history-{workspace}-{checkpoint}.json")).exists());
        assert_eq!(app.session.revision, revision);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn root_project_read_waits_for_the_settled_view_and_retains_its_editor_guard() {
        let mut app = app();
        let directory = std::env::temp_dir().join(format!(
            "filmcraft-root-view-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let scope = json!({"projectId":"p_root_view","workspaceId":"11111111-1111-4111-8111-111111111111"});
        app.session.execute("file.newProject", json!({"name":"Generated Root view guard","workspace":scope})).unwrap();
        app.session.execute("file.newSequence", json!({"name":"Actual owning timeline","fps":24})).unwrap();
        app.session.execute("file.save", json!({"path":directory.join("generated.fcproj")})).unwrap();
        let active = app.session.state.active_sequence.unwrap();
        let v = &app.ui.timeline;
        app.session.state.timeline_views.insert(
            active,
            filmcraft_engine::project::SequenceView {
                pps: v.target_pps,
                scroll: v.target_scroll,
                v_scroll: v.v_scroll,
                a_scroll: v.a_scroll,
                video_track_h: v.video_track_h,
                audio_track_h: v.audio_track_h,
            },
        );
        app.timeline_view_of = None;
        let token = "root-view-guard-ticket-000000000000000000001";
        let (request, _) = ControlRequest::new(
            "engine.execute",
            json!({"command":"agent.readProject","params":{"token":token,"scope":scope,"projectPath":directory.join("generated.fcproj")}}),
        );
        let before = app.session.project.clone();
        let revision = app.session.revision;
        assert!(matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Retry(_)));
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        app.timeline_view_of = Some(active);
        // The same token is still unused: a deferred read created no ticket.
        assert!(
            matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Done(v) if v["ok"]==true && v["result"]["kind"]=="native_project_read")
        );
        app.session.state.timeline_views.get_mut(&active).unwrap().scroll += 1.0;
        assert!(app.session.execute("agent.validateProjectRead", json!({"token":token,"scope":scope})).is_err());
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn source_production_read_uses_original_view_settling_before_any_input_dispatch() {
        let mut app = app();
        let token = "source-view-wait-0000000000000000000000001";
        app.timeline_view_of = None;
        assert!(snapshot_view_pending(&app));
        let before = app.session.project.clone();
        let revision = app.session.revision;
        let state = serde_json::to_value(&app.session.state).unwrap();
        let (request, _) = ControlRequest::new(
            "engine.execute",
            json!({
            "command":"agent.readMulticamInputs","params":{"token":token}}),
        );
        assert!(matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Retry(_)));
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.state).unwrap(), state);
        app.timeline_view_of = app.session.state.active_sequence;
        assert!(!snapshot_view_pending(&app));
        // Now the SAME original handler reaches schema validation, rather than
        // issuing a source ticket from a view that has not finished settling.
        assert!(matches!(handle(&mut app,&egui::Context::default(),&request),Outcome::Done(v) if v["ok"]==false));
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(app.session.revision, revision);
    }

    #[test]
    fn active_tab_and_unsaved_view_changes_wait_but_a_stable_view_does_not() {
        let mut app = app();
        assert!(!snapshot_view_pending(&app));
        app.timeline_view_of = None;
        assert!(snapshot_view_pending(&app));
        app.timeline_view_of = app.session.state.active_sequence;
        app.ui.timeline.target_scroll += 1.0;
        assert!(snapshot_view_pending(&app));
    }
    #[test]
    fn inactive_timeline_allows_metadata_read_without_focus_or_native_mutation_and_keeps_visible_guard() {
        let mut app = app();
        let directory = std::env::temp_dir().join(format!(
            "filmcraft-hidden-metadata-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let scope = json!({"projectId":"p_hidden_timeline","workspaceId":"11111111-1111-4111-8111-111111111111"});
        app.session.execute("file.newProject", json!({"name":"Hidden timeline metadata","workspace":scope})).unwrap();
        app.session.execute("file.newSequence", json!({"name":"Human timeline","fps":24})).unwrap();
        app.session.execute("file.save", json!({"path":directory.join("generated.fcproj")})).unwrap();
        app.ui.dock = crate::dock::DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1 };
        app.ui.floating.clear();
        app.timeline_view_of = None;
        app.timeline_still = 0;
        app.ui.timeline.target_scroll += 1.0;
        let before = app.session.project.clone();
        let revision = app.session.revision;
        let native = serde_json::to_value(&app.session.state).unwrap();
        let layout = serde_json::to_value(&app.ui.dock).unwrap();
        assert!(!snapshot_view_pending(&app));
        let (request, _) = ControlRequest::new(
            "engine.execute",
            json!({"command":"agent.readHistoryCheckpointBasis","params":{"token":"hidden-timeline-read-basis-00000000000000001","scope":scope}}),
        );
        assert!(
            matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Done(v) if v["ok"]==true && v["result"]["kind"]=="native_project_history_checkpoint_basis")
        );
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.state).unwrap(), native);
        assert_eq!(serde_json::to_value(&app.ui.dock).unwrap(), layout);
        app.ui.dock.activate(PanelKind::Timeline);
        assert!(snapshot_view_pending(&app));
        assert!(matches!(handle(&mut app, &egui::Context::default(), &request), Outcome::Retry(_)));
        assert_eq!(app.session.project, before);
        assert_eq!(app.session.revision, revision);
        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn hidden_floating_tabs_maximized_panels_and_nonedit_modes_use_original_visibility_for_settling() {
        let mut app = app();
        app.timeline_view_of = None;
        app.ui.dock = crate::dock::DockNode::Tabs { panels: vec![PanelKind::Program], active: 0 };
        app.ui.floating = vec![crate::dock::Floating { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1, rect: [0.0, 0.0, 400.0, 300.0] }];
        assert!(!snapshot_view_pending(&app));
        app.ui.floating[0].active = 0;
        assert!(snapshot_view_pending(&app));
        app.ui.keys.maximized = Some(PanelKind::Program);
        assert!(!snapshot_view_pending(&app));
        app.ui.keys.maximized = Some(PanelKind::Timeline);
        assert!(snapshot_view_pending(&app));
        app.ui.mode = Mode::Import;
        assert!(!snapshot_view_pending(&app));
        app.ui.mode = Mode::Export;
        assert!(!snapshot_view_pending(&app));
    }
}
