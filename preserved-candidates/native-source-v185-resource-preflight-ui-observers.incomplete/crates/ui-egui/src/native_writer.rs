//! Desktop control routing for the exclusive native writer. While busy, only the
//! dedicated display/Cancel view runs; the usual panels, keys and job pumps cannot
//! turn the display projection into a second editor or mint read authority.
use crate::{
    FilmcraftApp,
    control::{ControlRequest, Outcome},
};
use filmcraft_engine::session_worker::{Pending, SessionWorker, supports};
use serde_json::{Value, json};
use std::sync::{Arc, mpsc::Sender};

const CANCEL: &str = "poema.nativeWrite.cancel";
const CANCEL_UPDATE: &str = "poema.workspaceUpdate.cancel";

#[derive(Default)]
pub(crate) struct WriterState {
    worker: Option<SessionWorker>,
    pub(super) pending: Option<PendingWrite>,
    close_requested: bool,
    deferred_save_cancel: Option<DeferredSaveCancel>,
    #[cfg(test)]
    hold_completion_delivery: bool,
}
pub(super) struct PendingWrite {
    // Input fence only; the actual Session keeps every namespace/worker guard.
    resource_preflight: bool,
    work: Pending,
    sink: Sink,
}
enum Sink {
    Control(Sender<Value>),
    SaveCancel(Option<Sender<Value>>),
    Media { operation: String, paths: Vec<String>, before: Arc<filmcraft_project::Project> },
}

struct DeferredSaveCancel {
    params: Value,
    reply: Option<Sender<Value>>,
}
fn start_save_cancel(app: &mut FilmcraftApp, ctx: &egui::Context, params: Value, reply: Option<Sender<Value>>) -> Result<(), String> {
    let worker = app.native_writer.worker.as_ref().ok_or("native Save worker is unavailable")?;
    let wake = ctx.clone();
    let work = worker
        .start(&mut app.session, "agent.cancelProjectSaveOperation", params, Some(Arc::new(move || wake.request_repaint())))
        .map_err(|e| e.to_string())?;
    app.native_writer.pending = Some(PendingWrite { resource_preflight:false, work, sink: Sink::SaveCancel(reply) });
    ctx.request_repaint();
    Ok(())
}
fn defer_save_cancel(app: &mut FilmcraftApp, params: Value, reply: Option<Sender<Value>>) -> Result<Value, String> {
    if app.native_writer.deferred_save_cancel.is_some() {
        return Err("the exact Save cancellation is already waiting for its original task".into());
    }
    let pending = app.native_writer.pending.as_ref().ok_or("no native Save task is running")?;
    if !pending.work.matches_save_operation(&params) {
        return Err("another original native Save task is running".into());
    }
    let stopped = pending.work.cancel().map_err(|e| e.to_string())?;
    app.native_writer.deferred_save_cancel = Some(DeferredSaveCancel { params, reply });
    Ok(stopped)
}
fn request_stop(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if let Some(pending) = &app.native_writer.pending {
        if let Some(params) = pending.work.save_operation_request() {
            if app.native_writer.deferred_save_cancel.is_some() {
                return pending.work.cancel().map_err(|e| e.to_string());
            }
            return defer_save_cancel(app, params, None);
        }
        return pending.work.cancel().map_err(|e| e.to_string());
    }
    let params = app.session.pending_save_operation_request().ok_or("no native operation is waiting")?;
    start_save_cancel(app, ctx, params, None)?;
    Ok(json!({"cancelPending":true,"nativeOperationCancelled":false,"nativeComplete":false}))
}

pub(super) fn start_media_import(app: &mut FilmcraftApp, ctx: &egui::Context, operation: String, paths: Vec<String>, params: Value) -> Result<(), String> {
    if app.native_writer.pending.is_some() || app.session.is_display_only() {
        return Err("the native writer is busy; no import was started".into());
    }
    let worker = app.native_writer.worker.as_ref().ok_or("native import worker is unavailable; no synchronous fallback was used")?;
    let before = app.session.project.clone();
    let wake = ctx.clone();
    let work = worker.start_media_import(&mut app.session, params, Some(Arc::new(move || wake.request_repaint()))).map_err(|e| e.to_string())?;
    app.native_writer.pending = Some(PendingWrite { resource_preflight:false, work, sink: Sink::Media { operation, paths, before } });
    app.stop();
    app.stop_source();
    ctx.request_repaint();
    Ok(())
}

pub(super) fn stop_media_import(app: &FilmcraftApp, operation: &str) -> Result<(), String> {
    match &app.native_writer.pending {
        Some(p) if matches!(&p.sink,Sink::Media {operation:id,..} if id==operation) => p.work.cancel().map(|_| ()).map_err(|e| e.to_string()),
        _ => Err("the correlated native import is not running; read its retained result".into()),
    }
}

pub(super) fn install(app: &mut FilmcraftApp) {
    if app.application_restore_error.is_some() {
        return;
    }
    match SessionWorker::new().and_then(|worker| {
        worker.install(&mut app.session)?;
        Ok(worker)
    }) {
        Ok(worker) => app.native_writer.worker = Some(worker),
        Err(error) => app.ui.status = error.to_string(),
    }
}

pub(super) fn inspect(app: &FilmcraftApp) -> Value {
    match &app.native_writer.pending {
        Some(pending) => json!({"active":true,"displayOnly":true,"progress":pending.work.progress(),"closePending":app.native_writer.close_requested}),
        None => {
            let mut value = json!({"active":false,"available":app.native_writer.worker.is_some() && !app.session.is_display_only() && !application_held(app),"displayOnly":app.session.is_display_only()});
            if let Some(save) = app.session.pending_save_operation_request() {
                value["saveOperation"] = save;
            }
            if let Some(update) = app.session.workspace_update_status() {
                value["workspaceUpdate"] = update;
            }
            value
        }
    }
}

// Only initial setup may lack any composer host. Once admitted, ordinary
// close must keep its existing draft flush/drain handshake, even if held again.
pub(super) fn application_initial(app: &FilmcraftApp) -> bool {
    app.session.application_project_admission().is_some_and(|v| v["status"] == "held" && v["admitted"] == false)
}

pub(super) fn resource_preflight_in_progress(app:&FilmcraftApp)->bool {
    app.session.resource_preflight_pending() || app.native_writer.pending.as_ref().is_some_and(|p|p.resource_preflight)
}

pub(super) fn application_held(app: &FilmcraftApp) -> bool {
    app.application_restore_error.is_some() || app.session.application_project_admission().is_some_and(|v| v["status"] == "held")
}

pub(super) fn intercept(app: &mut FilmcraftApp, ctx: &egui::Context, request: &ControlRequest) -> Option<Outcome> {
    // Retained application outcome lookup reads only this app's bounded
    // records. It cannot issue a native read, execute or renew a preparation.
    if request.method == "ui.sequenceTabs.outcome" {
        return None;
    }
    // Archived data is observed only on the actual returned Session. It is
    // readonly even during a logical pause, and never queries a projection.
    if matches!(request.method.as_str(), "ui.sequenceTabs.archivedOutcome" | "ui.sequenceTabs.archivedRecordPage") {
        return if app.native_writer.pending.is_some() {
            Some(Outcome::Done(json!({"ok":false,"error":"native Session is on its worker; no archived read was queued","outcome":"rejected"})))
        } else {
            None
        };
    }
    // Reads/Stop touch retained UI state. Begin first reconciles an existing
    // ID; a NEW Begin refuses a display-only Session before any native read.
    if matches!(request.method.as_str(), "ui.mediaIntake.info" | "ui.mediaIntake.read" | "ui.mediaIntake.stop" | "ui.mediaIntake.begin") {
        if app.session.resource_preflight_pending() && matches!(request.method.as_str(),"ui.mediaIntake.stop"|"ui.mediaIntake.begin") {
            return Some(Outcome::Done(json!({"ok":false,"error":"recovery validation owns this native state; no media action was queued","outcome":"rejected"})));
        }
        return None;
    }
    if app.native_writer.pending.is_some() {
        let command = request.params.get("command").and_then(Value::as_str).or_else(|| request.params.get("id").and_then(Value::as_str));
        if matches!(request.method.as_str(), "engine.execute" | "ui.menu.invoke") && command == Some("agent.cancelProjectSaveOperation") {
            let params = request.params.get("params").cloned().unwrap_or(Value::Null);
            if app.native_writer.pending.as_ref().is_some_and(|p| p.work.matches_save_operation(&params)) {
                return Some(match defer_save_cancel(app, params, Some(request.reply.clone())) {
                    Ok(_) => {
                        ctx.request_repaint();
                        Outcome::Pending
                    }
                    Err(error) => Outcome::Done(json!({"ok":false,"error":error,"outcome":"rejected"})),
                });
            }
        }
        if request.method == "app.quit" {
            request_close(app);
            return Some(Outcome::Done(json!({"ok":true,"result":{"closePending":true,"closed":false}})));
        }
        if request.method == "ui.click" && request.params.get("id").and_then(Value::as_str) == Some(CANCEL)
            || request.method == "ui.key" && request.params.get("key").and_then(Value::as_str) == Some("Escape")
        {
            let result = Some(request_stop(app, ctx));
            return Some(Outcome::Done(match result {
                Some(Ok(value)) => json!({"ok":true,"result":value}),
                Some(Err(error)) => json!({"ok":false,"error":error.to_string()}),
                None => json!({"ok":false,"error":"native operation already finished"}),
            }));
        }
        // Never queue synthetic editing input that could land on the restored editor.
        if matches!(request.method.as_str(), "ui.inspect" | "ui.elements" | "ui.screenshot" | "perf.stats" | "ui.resize" | "ui.focus" | "ui.poemaComposer") {
            return None;
        }
        return Some(Outcome::Done(
            json!({"ok":false,"error":"a native write is in progress; no editor command was queued; wait or Stop","outcome":"rejected"}),
        ));
    }
    if app.session.resource_preflight_pending() {
        if request.method=="app.quit" {
            request_close(app);
            return Some(Outcome::Done(json!({"ok":true,"result":{"closePending":true,"closed":false}})));
        }
        let observe=matches!(request.method.as_str(),"ui.inspect"|"ui.elements"|"ui.screenshot"|"perf.stats"|"ui.resize"|"ui.focus"|"ui.poemaComposer");
        let same_control=request.method=="engine.execute" && request.params.get("command").and_then(Value::as_str).is_some_and(|command|matches!(command,"agent.resourcePreflightOwner"|"agent.beginResourcePreflight"|"agent.resourcePreflightStatus"|"agent.releaseResourcePreflight"));
        if !observe && !same_control {
            return Some(Outcome::Done(json!({"ok":false,"error":"recovery validation owns editing; no input or settings action was queued","outcome":"rejected"})));
        }
        // Observers fall through to the real returned Session. Begin/Release
        // continue to the SAME original worker below; no synchronous fallback.
    }
    if app.session.pending_save_operation_request().is_some() {
        if request.method == "app.quit" {
            app.ui.status = "Finish or stop Save before closing.".into();
            return Some(Outcome::Done(json!({"ok":true,"result":{"closePending":true,"closed":false}})));
        }
        if request.method == "ui.click" && request.params.get("id").and_then(Value::as_str) == Some(CANCEL)
            || request.method == "ui.key" && request.params.get("key").and_then(Value::as_str) == Some("Escape")
        {
            return Some(Outcome::Done(match request_stop(app, ctx) {
                Ok(result) => json!({"ok":true,"result":result}),
                Err(error) => json!({"ok":false,"error":error}),
            }));
        }
        if !matches!(
            request.method.as_str(),
            "engine.execute" | "ui.menu.invoke" | "ui.inspect" | "ui.elements" | "ui.screenshot" | "perf.stats" | "ui.resize" | "ui.focus" | "ui.poemaComposer"
        ) {
            return Some(Outcome::Done(json!({"ok":false,"error":"Save is pending; no editing input was queued","outcome":"rejected"})));
        }
    }
    if application_held(app) {
        if matches!(
            request.method.as_str(),
            "app.quit" | "ui.inspect" | "ui.elements" | "ui.screenshot" | "perf.stats" | "ui.resize" | "ui.focus" | "ui.poemaComposer"
        ) {
            return None;
        }
        if request.method == "engine.execute" && request.params.get("command").and_then(Value::as_str).is_some_and(|c| app.session.workspace_update_allows(c)) {
            return None;
        }
        return Some(Outcome::Done(json!({"ok":false,"error":"Poema is completing this project setup; no editing input was queued","outcome":"rejected"})));
    }
    if app.session.workspace_update_status().is_some() {
        if request.method == "app.quit" {
            if let Err(error) = app.session.cancel_workspace_update() {
                return Some(Outcome::Done(json!({"ok":false,"error":error.to_string()})));
            }
            return None;
        }
        if request.method == "ui.click" && request.params.get("id").and_then(Value::as_str) == Some(CANCEL_UPDATE)
            || request.method == "ui.key" && request.params.get("key").and_then(Value::as_str) == Some("Escape")
        {
            return Some(Outcome::Done(match app.session.cancel_workspace_update() {
                Ok(value) => json!({"ok":true,"result":value}),
                Err(error) => json!({"ok":false,"error":error.to_string()}),
            }));
        }
        if matches!(request.method.as_str(), "ui.inspect" | "ui.elements" | "ui.screenshot" | "perf.stats" | "ui.resize" | "ui.focus" | "ui.poemaComposer") {
            return None;
        }
        if matches!(request.method.as_str(), "engine.execute" | "ui.menu.invoke") {
            let command = request.params.get("command").and_then(Value::as_str).or_else(|| request.params.get("id").and_then(Value::as_str)).unwrap_or("");
            if !app.session.workspace_update_allows(command) {
                return Some(Outcome::Done(json!({"ok":false,"error":"workspace update has paused editing; no input was queued","outcome":"rejected"})));
            }
            if !supports(command) {
                return None;
            }
            // Compatible persistence still runs on the exclusive Session worker.
        } else {
            return Some(Outcome::Done(json!({"ok":false,"error":"workspace update has paused editing; no input was queued","outcome":"rejected"})));
        }
    }
    if !matches!(request.method.as_str(), "engine.execute" | "ui.menu.invoke") {
        return None;
    }
    let command = request.params.get("command").and_then(Value::as_str).or_else(|| request.params.get("id").and_then(Value::as_str))?;
    if !supports(command) {
        return None;
    }
    if !app.synthetic.is_empty()
        || !app.input_waiters.is_empty()
        || app.native_dialog.is_some()
        || app.value_press.is_some()
        || app.tl.drag.is_some()
        || ctx.input(|input| input.pointer.any_down())
    {
        return Some(Outcome::Done(
            json!({"ok":false,"error":"finish the current native input or dialog before an agent write; no write was queued","outcome":"rejected"}),
        ));
    }
    if command=="agent.beginResourcePreflight" && !app.session.resource_preflight_pending() {
        if ctx.input(|input|input.events.iter().any(|event|matches!(event,egui::Event::Text(_)|egui::Event::Paste(_)|egui::Event::Key{pressed:true,..}|egui::Event::PointerButton{..}|egui::Event::MouseWheel{..}))) {
            return Some(Outcome::Done(json!({"ok":false,"error":"finish current native input before recovery validation; the input was kept","outcome":"rejected"})));
        }
        if let Err(error)=crate::media_intake::require_idle_resource_writers(app) {
            return Some(Outcome::Done(json!({"ok":false,"error":error,"outcome":"rejected"})));
        }
    }
    let Some(worker) = app.native_writer.worker.as_ref() else {
        return Some(Outcome::Done(json!({"ok":false,"error":"native writer is unavailable; no synchronous write fallback was used","outcome":"rejected"})));
    };
    let workspace = app.session.workspace_update_status();
    let frozen_checkpoint =
        command == "agent.saveHistoryCheckpoint" && workspace.as_ref().is_some_and(|value| value["status"] == "paused" && value["role"] == "owner");
    // The original source pause already attached its physical UI on this same
    // worker. Metadata SAVE uses that validated frozen record; a paused display
    // cannot manufacture a new attachment. Other lifecycle roles keep their fence.
    if frozen_checkpoint {
        let retained = (|| -> Result<(), String> {
            if app.session.has_native_application_root().map_err(|e| e.to_string())? && app.session.saved_application_ui().map_err(|e| e.to_string())?.is_none()
            {
                return Err("the source pause has no validated frozen application UI; no checkpoint was written".into());
            }
            Ok(())
        })();
        if let Err(error) = retained {
            return Some(Outcome::Done(json!({"ok":false,"error":error,"outcome":"rejected"})));
        }
    }
    let capture = if command == "agent.saveHistoryCheckpoint" && !frozen_checkpoint || command == "agent.beginWorkspaceUpdate" && workspace.is_none() {
        match crate::application_tabs::capture_history(app) {
            Ok(value) => value,
            Err(error) => return Some(Outcome::Done(json!({"ok":false,"error":error,"outcome":"rejected"}))),
        }
    } else {
        None
    };
    let params = request.params.get("params").cloned().unwrap_or_else(|| json!({}));
    let wake = ctx.clone();
    let wake: Option<filmcraft_engine::session_worker::Wake> = Some(Arc::new(move || wake.request_repaint()));
    let started = match capture {
        Some(capture) => worker.start_with_application_capture(&mut app.session, command, params, wake, capture),
        None => worker.start(&mut app.session, command, params, wake),
    };
    match started {
        Ok(work) => {
            app.native_writer.pending = Some(PendingWrite { resource_preflight:matches!(command,"agent.beginResourcePreflight"|"agent.releaseResourcePreflight"), work, sink: Sink::Control(request.reply.clone()) });
            app.ui.status.clear();
            // The real Session is already on the worker. Stop audition without committing a
            // user recording/trim: active gestures were refused before the transfer.
            app.stop();
            app.stop_source();
            ctx.request_repaint();
            Some(Outcome::Pending)
        }
        Err(error) => Some(Outcome::Done(json!({"ok":false,"error":error.to_string(),"outcome":"rejected"}))),
    }
}

pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.session.workspace_update_status().is_some_and(|v| v["status"] == "retiring" || v["status"] == "candidate_cancelled") {
        app.quit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if (app.native_writer.pending.is_some()
        || app.session.pending_save_operation_request().is_some()
        || app.session.workspace_update_status().is_some()
        || app.session.resource_preflight_pending()
        || application_held(app))
        && let Some(inbox) = &app.command_inbox
    {
        while inbox.try_recv().is_ok() {
            app.ui.status = "Native edit in progress; the menu action was not queued.".into();
        }
    }
    #[cfg(test)]
    if app.native_writer.hold_completion_delivery {
        return;
    }
    let Some(mut pending) = app.native_writer.pending.take() else { return };
    let import_stop_requested = pending.work.progress()["cancel_requested"] == true;
    let Some(completion) = pending.work.try_finish() else {
        app.native_writer.pending = Some(pending);
        return;
    };
    app.session = completion.session;
    if let Err(error) = &completion.reply.result {
        app.ui.status = error.to_string();
    }
    match pending.sink {
        Sink::Control(reply) => {
            let _ = reply.send(completion.reply.envelope());
        }
        Sink::SaveCancel(reply) => {
            if let Some(reply) = reply {
                let _ = reply.send(completion.reply.envelope());
            }
        }
        Sink::Media { operation, paths, before } => {
            if let Err(e) = crate::media_intake::import_completed(app, &operation, paths, before, completion.reply, import_stop_requested) {
                app.ui.status = e;
            }
        }
    }
    let closing = std::mem::take(&mut app.native_writer.close_requested);
    if let Some(cancel) = app.native_writer.deferred_save_cancel.take() {
        let reply = cancel.reply.clone();
        if let Err(error) = start_save_cancel(app, ctx, cancel.params, cancel.reply) {
            app.ui.status = error.clone();
            if let Some(reply) = reply {
                let _ = reply.send(json!({"ok":false,"error":error,"outcome":"rejected"}));
            }
        }
    }
    if closing {
        if app.native_writer.pending.is_some() {
            app.native_writer.close_requested = true;
        } else if app.session.pending_save_operation_request().is_some() {
            app.ui.status = "Finish or stop Save before closing.".into();
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    ctx.request_repaint();
}

#[cfg(test)]
#[path = "native_writer_tests.rs"]
mod tests;

pub(super) fn request_close(app: &mut FilmcraftApp) -> bool {
    if resource_preflight_in_progress(app) {
        app.ui.status="Recovery validation is finishing; close is held until its exact release.".into();
        return true;
    }
    if app.native_writer.pending.is_none() && app.session.pending_save_operation_request().is_some() {
        app.ui.status = "Finish or stop Save before closing.".into();
        return true;
    }
    if application_held(app) && app.native_writer.pending.is_none() {
        app.quit_confirmed = true;
        return false;
    }
    if app.session.workspace_update_status().is_some_and(|v| v["status"] == "retiring" || v["status"] == "candidate_cancelled") {
        app.quit_confirmed = true;
        return false;
    }
    if app.session.workspace_update_status().is_some_and(|v| v["role"] == "candidate") {
        if let Err(error) = app.session.cancel_workspace_update() {
            app.ui.status = error.to_string();
        }
        // The old workspace may still be the writer. Never show a Save dialog
        // or release this candidate into editing while cancellation settles.
        return true;
    }
    if app.native_writer.pending.is_none()
        && app.session.workspace_update_status().is_some()
        && let Err(error) = app.session.cancel_workspace_update()
    {
        app.ui.status = error.to_string();
        return true;
    }
    let Some(pending) = &app.native_writer.pending else { return false };
    app.native_writer.close_requested = true;
    if let Err(error) = pending.work.cancel() {
        app.ui.status = error.to_string();
    }
    true
}

pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    app.auto.begin_frame();
    if resource_preflight_in_progress(app) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("Checking recovery");
            ui.label(&app.session.project.name);
            ui.add_space(16.0);
            ui.spinner();
            let response=ui.label("Editing is paused while recovery files are checked. Input is not queued.");
            app.auto.add_response("native-resource-preflight",&response,"Recovery validation owns the current edit");
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    if app.native_writer.pending.is_none() && application_held(app) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("Setting up Poema");
            ui.label(&app.session.project.name);
            ui.add_space(16.0);
            ui.spinner();
            let response = ui.label("Your project is paused while Poema finishes setup and saves its recovery checkpoint.");
            app.auto.add_response("native-application-setup", &response, "Application setup is pending; editing input is not queued");
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    if app.native_writer.pending.is_none() && app.session.workspace_update_status().is_some() {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("Updating Poema");
            ui.label(&app.session.project.name);
            ui.add_space(16.0);
            ui.spinner();
            ui.label(if app.session.workspace_update_status().is_some_and(|v| v["status"] == "candidate_cancelled") {
                "Update cancelled. Waiting for the current workspace to resume."
            } else {
                "Your edit is paused while the update is prepared."
            });
            let response = ui.button("Cancel update");
            app.auto.add_response(CANCEL_UPDATE, &response, "Cancel workspace update");
            if (response.clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape)))
                && let Err(error) = app.session.cancel_workspace_update()
            {
                app.ui.status = error.to_string();
            }
            if !app.ui.status.is_empty() {
                ui.label(&app.ui.status);
            }
        });
        return;
    }
    if app.native_writer.pending.is_none() && app.session.pending_save_operation_request().is_some() {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.heading("Poema · Save");
            ui.label(&app.session.project.name);
            ui.spinner();
            ui.label("Save is pending. Editing is paused.");
            let response = ui.button("Stop");
            app.auto.add_response(CANCEL, &response, "Stop Save");
            if response.clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                if let Err(e) = request_stop(app, ui.ctx()) {
                    app.ui.status = e;
                }
            }
            if !app.ui.status.is_empty() {
                ui.label(&app.ui.status);
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        return;
    }
    let Some(pending) = &app.native_writer.pending else { return };
    let progress = pending.work.progress();
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        let importing = progress["kind"] == "native_media_import";
        ui.heading(if importing {
            "Poema · Importing media"
        } else if progress["kind"] == "native_Save_lifecycle_wait" {
            "Poema · Save"
        } else {
            "Poema · Native edit"
        });
        ui.label(&app.session.project.name);
        ui.add_space(16.0);
        ui.spinner();
        let phase = progress["phase"].as_str().unwrap_or("unknown");
        ui.label(if importing {
            if progress["cancel_requested"] == true {
                "Stopping after the current file. Imported media stays in the project."
            } else if phase == "scanning" {
                "Reading the selected folder. No media has been imported yet."
            } else {
                "Importing the selected media."
            }
        } else if progress["kind"] == "native_Save_lifecycle_wait" {
            if progress["cancel_requested"] == true { "Stopping Save…" } else { "Checking Save…" }
        } else {
            match phase {
                "cancelled" => "Stopping the native operation…",
                "publishing" | "committed" => "Finishing the native edit…",
                _ => "Checking the edit and original media…",
            }
        });
        if importing {
            ui.label(format!("{} / {} files processed", progress["completed_files"], progress["planned_files"]));
        }
        if let Some(bytes) = progress["bytes_read"].as_str() {
            ui.label(format!("{} file checks · {bytes} bytes read", progress["completed_checks"]));
        }
        ui.label("The native editor will return when this operation finishes.");
        let response = ui.add_enabled(progress["cancellable"] == true || progress["kind"] == "native_Save_lifecycle_wait", egui::Button::new("Stop"));
        app.auto.add_response(CANCEL, &response, "Stop native edit");
        if (response.clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape)))
            && let Err(error) = request_stop(app, ui.ctx())
        {
            app.ui.status = error.to_string();
        }
        if !app.ui.status.is_empty() {
            ui.label(&app.ui.status);
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
}

#[cfg(all(test,any(target_vendor="apple",target_os="linux")))]
#[path="resource_preflight_ui_tests.rs"]
mod resource_preflight_tests;
