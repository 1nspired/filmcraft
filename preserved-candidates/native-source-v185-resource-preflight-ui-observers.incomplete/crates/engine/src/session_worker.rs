//! A bounded worker temporarily owns the real Session for guarded agent writes.
//! The UI keeps a display projection with no command, history or persistence authority.
//! The same Session::execute stages, verifies and publishes; no file guard is bypassed.

use crate::{AgentOperationControl, EngineError, Result, Session};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Fixed control-reply bound, never a caller timeout or publication permission.
pub fn control_reply_wait_ms(method: &str, params: &Value) -> u64 {
    if matches!(method, "engine.execute" | "ui.menu.invoke")
        && let Some(wait) = crate::agent_bridge::source_verification_reply_wait(params)
    {
        return wait;
    }
    if matches!(method, "engine.execute" | "ui.menu.invoke")
        && params.get("command").or_else(|| params.get("id")).and_then(Value::as_str) == Some("agent.apply")
        && let Some(wait) = crate::agent_bridge::admission_reply_wait(params.get("params").unwrap_or(&Value::Null))
    {
        return wait;
    }
    60_000
}

/// Guarded edits, private history and typed export publication. Never a general executor.
pub fn supports(command: &str) -> bool {
    crate::agent_bridge::save_lifecycle_command(command)
        || matches!(
            command,
            "agent.beginResourcePreflight"
                | "agent.releaseResourcePreflight"
                | "agent.prepareMulticamProduction"
                | "agent.apply"
                | "agent.undo"
                | "agent.redo"
                | "agent.commitCondense"
                | "agent.saveHistoryCheckpoint"
                | "agent.restoreHistoryCheckpoint"
                | "agent.validateRetainedHistory"
                | "agent.preparePausedProjectRecovery"
                | "agent.pausedProjectRecoveryRecord"
                | "agent.inspectHistoryCheckpoint"
                | "agent.inspectCheckpointProvenance"
            | "agent.readHistoryCheckpointOutcome"
                | "agent.inspectApplicationHistoryCheckpoint"
                | "agent.inspectIdentityMigration"
                | "agent.publishIdentityMigration"
                | "agent.readIdentityPublication"
                | "agent.startExport"
                | "agent.publishExport"
                | "agent.beginWorkspaceUpdate"
                | "agent.stageWorkspaceUpdate"
                | "agent.activateWorkspaceUpdate"
                | "agent.retireWorkspaceUpdate"
        )
}

#[derive(Default)]
struct OwnerSlot {
    session: Mutex<Option<Session>>,
    abandoned: AtomicBool,
}
impl OwnerSlot {
    fn take(&self) -> Option<Session> {
        self.session.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    fn release(&self, session: Session) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
        self.finish_abandoned();
    }
    fn finish_abandoned(&self) {
        if self.abandoned.load(Ordering::Acquire)
            && let Some(mut session) = self.take()
        {
            // The actual owner flushes recovery, even if its UI disappeared mid-write.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.shutdown()));
        }
    }
}

struct Task {
    slot: Arc<OwnerSlot>,
    command: String,
    params: Value,
    done: mpsc::Sender<Reply>,
    wake: Option<Wake>,
    import_control: Option<crate::media_browser::ImportControl>,
    save_control: Option<(crate::agent_bridge::SaveTaskControl, AgentOperationControl)>,
    application_capture: Option<crate::ApplicationSnapshotCapture>,
}

/// Result of the one attempt. Unknown never authorizes replay or implies rollback.
pub struct Reply {
    pub result: Result<Value>,
    pub uncertain: bool,
}
impl Reply {
    pub fn envelope(self) -> Value {
        match self.result {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":error.to_string(),"outcome":if self.uncertain {"unknown"} else {"rejected"}}),
        }
    }
}

pub struct Completion {
    pub session: Session,
    pub reply: Reply,
}

/// Holds no second writer. Dropping it cancels and flushes the real owner on exit.
pub struct Pending {
    slot: Arc<OwnerSlot>,
    done: Receiver<Reply>,
    control: Control,
    finished: bool,
}
enum Control {
    Agent(AgentOperationControl, String),
    Media(crate::media_browser::ImportControl),
    Save(crate::agent_bridge::SaveTaskControl),
}
impl Pending {
    pub fn save_operation_request(&self) -> Option<Value> {
        if let Control::Save(c) = &self.control { Some(c.params()) } else { None }
    }
    pub fn matches_save_operation(&self, params: &Value) -> bool {
        if let Control::Save(c) = &self.control { c.matches(params) } else { false }
    }
    pub fn progress(&self) -> Value {
        match &self.control {
            Control::Agent(c, t) => c.progress(t),
            Control::Media(c) => c.progress(),
            Control::Save(c) => c.progress(),
        }
    }
    pub fn cancel(&self) -> Result<Value> {
        match &self.control {
            Control::Agent(c, t) => c.cancel(&json!({"token":t})),
            Control::Media(c) => Ok(c.cancel()),
            Control::Save(c) => Ok(c.stop()),
        }
    }
    /// Restore this Session before processing another native command or any queued UI edit.
    pub fn try_finish(&mut self) -> Option<Completion> {
        if self.finished {
            return None;
        }
        let reply = match self.done.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Reply { result: Err(EngineError::Other("native writer disconnected; reconcile the original operation, never replay".into())), uncertain: true }
            }
        };
        let session = self.slot.take()?;
        self.finished = true;
        Some(Completion { session, reply })
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished {
            self.slot.abandoned.store(true, Ordering::Release);
            let _ = self.cancel();
            self.slot.finish_abandoned();
        }
    }
}

pub struct SessionWorker {
    sender: SyncSender<Task>,
}
impl SessionWorker {
    /// One persistent thread, one bounded waiting slot; no threads are spawned per request.
    pub fn new() -> Result<Self> {
        Self::spawn(|session, command, params| session.execute(command, params))
    }
    #[cfg(test)]
    pub(crate) fn test_with_executor(execute: impl Fn(&mut Session, &str, Value) -> Result<Value> + Send + 'static) -> Result<Self> {
        Self::spawn(execute)
    }
    fn spawn(execute: impl Fn(&mut Session, &str, Value) -> Result<Value> + Send + 'static) -> Result<Self> {
        let (sender, tasks) = mpsc::sync_channel::<Task>(1);
        std::thread::Builder::new()
            .name("filmcraft-native-writer".into())
            .spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(task) = tasks.recv() {
                        let Some(mut session) = task.slot.take() else {
                            if let Some((control, registry)) = &task.save_control {
                                control.finish();
                                registry.finish_save_wait(control);
                            }
                            continue;
                        };
                        // Keep the actual Session outside catch_unwind so it is returned on panic.
                        session.media_import_control = task.import_control.clone();
                        if let Some((control, _)) = &task.save_control {
                            control.enter();
                            session.save_lifecycle_control = Some(control.clone());
                        }
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Some(capture) = task.application_capture {
                                capture.stage(&mut session)?;
                            }
                            execute(&mut session, &task.command, task.params)
                        }));
                        // A cancelled request must never poison a later human
                        // import, including when the import unwinds.
                        // Parsing, owner checks and panics may return before the
                        // original save consumes its attachment. Never leak it
                        // into a later human or agent command.
                        session.agent_bridge.pending_application_capture = None;
                        session.media_import_control = None;
                        session.save_lifecycle_control = None;
                        if let Some((control, registry)) = &task.save_control {
                            control.finish();
                            registry.finish_save_wait(control);
                        }
                        if let Some(c) = &task.import_control {
                            c.finish();
                        }
                        let result = result.unwrap_or(Err(EngineError::CommandPanicked { command: task.command }));
                        let uncertain = matches!(&result, Err(EngineError::CommandPanicked { .. }));
                        task.slot.release(session);
                        let _ = task.done.send(Reply { result, uncertain });
                        if let Some(wake) = task.wake {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake()));
                        }
                    }
                }));
            })
            .map_err(|error| EngineError::Other(format!("cannot start the native writer: {error}")))?;
        Ok(Self { sender })
    }

    /// A host advertises the path only after its bounded worker has been installed.
    pub fn install(&self, session: &mut Session) -> Result<()> {
        // Installing the worker supplies an owner transport, not an edit. A
        // startup candidate may already hold the logical update pause.
        session.require_command_owner("agent.workspaceUpdateStatus")?;
        session.worker_available = true;
        Ok(())
    }

    pub fn start(&self, session: &mut Session, command: &str, params: Value, wake: Option<Wake>) -> Result<Pending> {
        self.start_captured(session, command, params, wake, None)
    }

    /// Same bounded original worker; only private history checkpoints and the
    /// original handoff pause accept this physical data attachment. Ordinary
    /// editing and all gesture guards stay intact.
    pub fn start_with_application_capture(
        &self,
        session: &mut Session,
        command: &str,
        params: Value,
        wake: Option<Wake>,
        capture: crate::ApplicationSnapshotCapture,
    ) -> Result<Pending> {
        if !matches!(command, "agent.saveHistoryCheckpoint" | "agent.beginWorkspaceUpdate") {
            return Err(EngineError::Other("application capture requires an original history checkpoint or handoff pause".into()));
        }
        capture.validate(session)?;
        self.start_captured(session, command, params, wake, Some(capture))
    }
    fn start_captured(
        &self,
        session: &mut Session,
        command: &str,
        params: Value,
        wake: Option<Wake>,
        application_capture: Option<crate::ApplicationSnapshotCapture>,
    ) -> Result<Pending> {
        session.require_command_owner(command)?;
        if !supports(command) || !session.worker_available || session.exec_depth != 0 {
            return Err(EngineError::Other("native writer accepts only an installed, non-nested guarded operation".into()));
        }
        if session.trim_play.active() || session.mixrec.active() || session.mcrec.active() || session.voiceover.recording() {
            return Err(EngineError::Other("finish the active trim or recording gesture before a native agent write".into()));
        }
        if crate::agent_bridge::save_lifecycle_command(command) {
            let control = crate::agent_bridge::SaveTaskControl::new(command, &params)?;
            let registry = session.agent_operation_control();
            registry.install_save_wait(control.clone())?;
            let result = self.submit(session, command, params, wake, Control::Save(control.clone()), None, application_capture);
            if result.is_err() {
                control.finish();
                registry.finish_save_wait(&control);
            }
            return result;
        }
        if matches!(command,"agent.beginResourcePreflight"|"agent.releaseResourcePreflight") {
            let token=crate::agent_bridge::prepare_resource_preflight_worker(session,command,&params)?;
            let control=Control::Agent(session.agent_operation_control(),token.clone());
            let result=self.submit(session,command,params,wake,control,None,application_capture);
            if let Err(error)=result {
                if command=="agent.beginResourcePreflight" {
                    if let Err(cleanup)=crate::agent_bridge::discard_resource_preflight_worker(session,&token) {
                        return Err(EngineError::Other(format!("{error}; original resource preparation cleanup failed: {cleanup}")));
                    }
                }
                return Err(error);
            }
            return result;
        }
        let token = params
            .get("token")
            .and_then(Value::as_str)
            .filter(|token| (32..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
            .ok_or_else(|| EngineError::Other("native write requires one explicit valid read token".into()))?
            .to_string();
        let control = Control::Agent(session.agent_operation_control(), token);
        self.submit(session, command, params, wake, control, None, application_capture)
    }

    /// Trusted human selection only. Runs the SAME Media Browser command on
    /// the exclusive owner, without minting an agent read ticket or executor.
    pub fn start_media_import(&self, session: &mut Session, params: Value, wake: Option<Wake>) -> Result<Pending> {
        session.require_owner()?;
        if !session.worker_available
            || session.exec_depth != 0
            || session.trim_play.active()
            || session.mixrec.active()
            || session.mcrec.active()
            || session.voiceover.recording()
        {
            return Err(EngineError::Other("finish the current native gesture before importing media".into()));
        }
        let valid_paths = params.get("paths").and_then(Value::as_array).is_some_and(|paths| {
            !paths.is_empty()
                && paths.len() <= 256
                && paths.iter().all(|p| {
                    p.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control) && std::path::Path::new(s).is_absolute())
                })
        });
        if params.as_object().is_none_or(|o| o.keys().any(|k| !matches!(k.as_str(), "paths" | "bin" | "imageSequence" | "mediaOnly" | "maxFiles")))
            || !valid_paths
            || params["imageSequence"] != false
            || params["mediaOnly"] != true
            || !params["maxFiles"].as_u64().is_some_and(|n| (1..=2000).contains(&n))
        {
            return Err(EngineError::Other("one bounded media-only native selection is required".into()));
        }
        let control = crate::media_browser::ImportControl::default();
        self.submit(session, "mediaBrowser.import", params, wake, Control::Media(control.clone()), Some(control), None)
    }

    fn submit(
        &self,
        session: &mut Session,
        command: &str,
        params: Value,
        wake: Option<Wake>,
        control: Control,
        import_control: Option<crate::media_browser::ImportControl>,
        application_capture: Option<crate::ApplicationSnapshotCapture>,
    ) -> Result<Pending> {
        let save_control = if let Control::Save(c) = &control { Some((c.clone(), session.agent_operation_control())) } else { None };
        let projection = session.display_projection();
        let slot = Arc::new(OwnerSlot { session: Mutex::new(Some(std::mem::replace(session, projection))), abandoned: AtomicBool::new(false) });
        let (done, receiver) = mpsc::channel();
        let task = Task { slot: slot.clone(), command: command.into(), params, done, wake, import_control, save_control, application_capture };
        if let Err(error) = self.sender.try_send(task) {
            // A failed submission never loses the unsaved Session or leaves a projection behind.
            if let Some(owner) = slot.take() {
                *session = owner;
            }
            return Err(EngineError::Other(format!("native writer did not accept the operation; no write was sent: {error}")));
        }
        Ok(Pending { slot, done: receiver, control, finished: false })
    }
}

#[cfg(test)]
#[path = "session_worker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_save_worker_tests.rs"]
mod save_tests;
