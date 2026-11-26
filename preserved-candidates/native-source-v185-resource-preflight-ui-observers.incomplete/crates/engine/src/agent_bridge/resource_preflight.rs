//! Same-Session metadata coordination. Wire records describe a private original
//! SavePause/preview lease; they never mint a filesystem, edit or startup owner.
use super::*;
use crate::{
    autosave::SavePause,
    previews::{PreviewStore, WriterPause},
};
use filmcraft_format::retained::preview_context::DirectoryFrame;
use std::{collections::VecDeque, path::Path, sync::Arc};
const OWNER: &str = "agent.resourcePreflightOwner";
const BEGIN: &str = "agent.beginResourcePreflight";
const STATUS: &str = "agent.resourcePreflightStatus";
const RELEASE: &str = "agent.releaseResourcePreflight";
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Address {
    request: String,
    native_pid: u32,
    native_session: String,
    data_directory: String,
}
struct Record {
    address: Address,
    status: &'static str,
    error: Option<String>,
}
pub(super) struct Held {
    address: Address,
    pause: SavePause,
    store: Arc<PreviewStore>,
    previews: Option<WriterPause>,
    directory: Arc<DirectoryFrame>,
    session: Arc<DirectoryFrame>,
    project: Arc<filmcraft_project::Project>,
    revision: u64,
    saved_revision: u64,
    path: Option<String>,
    operation: operation_control::ResourcePreflightOperation,
    releasing: bool,
    failure: Option<String>,
}
struct Prepared {
    address: Address,
    operation: operation_control::ResourcePreflightOperation,
    directory: Arc<DirectoryFrame>,
    session: Arc<DirectoryFrame>,
}
#[derive(Default)]
pub(super) struct State {
    // Prepared ONLY on the actual Session before transfer to its original worker.
    // This is an editing fence, not an acknowledged namespace hold.
    prepared: Option<Prepared>,
    live: Option<Held>,
    records: VecDeque<Record>,
}
pub(super) fn held(s: &Session) -> bool {
    s.agent_bridge.resource_preflight.live.is_some() || s.agent_bridge.resource_preflight.prepared.is_some()
}
pub(super) fn command_allowed(s: &Session, id: &str) -> Result<()> {
    if held(s) && !matches!(id, OWNER | BEGIN | STATUS | RELEASE) {
        return Err(bad(id, "original resource preflight owns the native namespace; competing commands remain blocked"));
    }
    Ok(())
}
fn err(m: impl Into<String>) -> crate::EngineError {
    bad(BEGIN, m)
}
fn clipped(e: &crate::EngineError) -> String {
    e.to_string().chars().take(1024).collect()
}
fn address(s: &Session, p: &Value) -> Result<Address> {
    let a: Address = parse(STATUS, p)?;
    if !token_ok(&a.request)
        || a.native_pid != std::process::id()
        || s.agent_bridge.epoch.as_deref() != Some(a.native_session.as_str())
        || a.data_directory.len() > 4096
        || a.data_directory.chars().any(char::is_control)
        || !Path::new(&a.data_directory).is_absolute()
    {
        return Err(err("exact original native epoch, request and data directory required"));
    }
    let worker = s.persistence.as_ref().ok_or_else(|| err("an actual original persistence worker is required"))?;
    if worker.data_dir != Path::new(&a.data_directory) {
        return Err(err("resource read belongs to another original worker directory"));
    }
    Ok(a)
}
fn reply(a: &Address, status: &str, ready: bool, error: Option<&str>) -> Value {
    json!({"kind":"poema-native-resource-preflight","version":1,"request":a.request,"nativePid":a.native_pid,"nativeSession":a.native_session,"dataDirectory":a.data_directory,"status":status,"namespaceHeld":ready,"metadataOnly":true,"writeAuthority":false,"nativeStartupAuthority":false,"workersMayResume":false,"error":error})
}
fn owner(s: &mut Session, p: &Value) -> Result<Value> {
    if p.as_object().is_none_or(|o| !o.is_empty()) {
        return Err(err("resource owner observation takes an empty object"));
    }
    let epoch = history_checkpoint::epoch(s)?;
    let directory = s.persistence.as_ref().and_then(|p| p.data_dir.to_str());
    Ok(
        json!({"kind":"poema-native-resource-preflight-owner","version":1,"nativePid":std::process::id(),"nativeSession":epoch,"dataDirectory":directory,"projectPath":s.path,"available":!s.is_display_only()&&directory.is_some(),"activeRequest":s.agent_bridge.resource_preflight.live.as_ref().map(|h|&h.address).or(s.agent_bridge.resource_preflight.prepared.as_ref().map(|p|&p.address)),"metadataOnly":true,"writeAuthority":false,"nativeStartupAuthority":false}),
    )
}
impl Held {
    fn identity(&self, s: &Session, check: &impl Fn() -> Result<()>) -> Result<()> {
        check()?;
        let worker = s.persistence.as_ref().ok_or_else(|| err("original held worker disappeared"))?;
        if self.address.native_pid != std::process::id()
            || s.agent_bridge.epoch.as_deref() != Some(self.address.native_session.as_str())
            || worker.data_dir != Path::new(&self.address.data_directory)
            || !self.pause.same_owner(worker)
            || !Arc::ptr_eq(&self.store, &s.previews)
        {
            return Err(err("the actual resource-preflight worker/epoch changed; no replacement owner was released"));
        }
        let io_check = || check().map_err(|e| std::io::Error::other(e.to_string()));
        self.directory.current(&io_check).map_err(|e| err(e.to_string()))?;
        self.session.current(&io_check).map_err(|e| err(e.to_string()))?;
        check()
    }
    fn current(&self, s: &Session) -> Result<()> {
        self.identity(s, &|| self.operation.check())?;
        if self.releasing
            || self.failure.is_some()
            || !Arc::ptr_eq(&self.project, &s.project)
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
            || self.path != s.path
        {
            return Err(err("resource preflight basis changed, failed or began release"));
        }
        let worker = s.persistence.as_ref().ok_or_else(|| err("original held worker disappeared"))?;
        self.pause.current(worker).map_err(err)?;
        self.previews.as_ref().ok_or_else(|| err("original preview producer hold is unavailable"))?.probe().current()?;
        Ok(())
    }
}
fn record(s: &mut Session, a: &Address, status: &'static str, error: Option<String>) {
    let state = &mut s.agent_bridge.resource_preflight;
    if let Some(r) = state.records.iter_mut().find(|r| r.address.request == a.request) {
        r.status = status;
        r.error = error;
        return;
    }
    if state.records.len() >= OP_LIMIT {
        state.records.pop_front();
    }
    state.records.push_back(Record { address: a.clone(), status, error });
}
fn status_for(s: &Session, a: &Address) -> Result<Value> {
    if let Some(h) = s.agent_bridge.resource_preflight.live.as_ref() {
        if &h.address != a {
            return Err(err("another exact original request owns the resource hold"));
        }
        if h.releasing {
            return Ok(reply(a, "releasing", false, h.failure.as_deref()));
        }
        return match h.current(s) {
            Ok(()) => Ok(reply(a, "held", true, None)),
            Err(e) => Ok(reply(a, "stale_held", false, Some(&clipped(&e)))),
        };
    }
    if let Some(r) = s.agent_bridge.resource_preflight.records.iter().find(|r| r.address.request == a.request) {
        if &r.address != a {
            return Err(err("the retained resource request has different original identity"));
        }
        return Ok(reply(a, r.status, false, r.error.as_deref()));
    }
    Ok(reply(a, "unknown", false, None))
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let a = address(s, p)?;
    status_for(s, &a)
}
// Register cancellation before queuing the original Session, so Stop may
// overtake a Begin whose worker execution has not started. No pause exists yet.
fn prepare_new(s: &mut Session, a: &Address) -> Result<()> {
    if held(s) || s.is_display_only() || !s.agent_bridge.operations.available() {
        return Err(err("the editable original Session and independent cancellation transport are required"));
    }
    if s.agent_bridge.resource_preflight.records.iter().any(|r| r.address.request == a.request) {
        return Err(err("resource preflight was already attempted; inspect its same request, never reacquire it"));
    }
    idle(s)?;
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    let worker = s.persistence.as_ref().ok_or_else(|| err("original persistence worker required"))?;
    if std::fs::canonicalize(&worker.data_dir).map_err(|e| err(e.to_string()))? != worker.data_dir {
        return Err(err("resource directory is redirected"));
    }
    s.agent_bridge.operations.register(&a.request)?;
    let operation = s.agent_bridge.operations.begin_resource_preflight(&a.request)?;
    record(s, a, "preparing", None);
    let frames: Result<_> = (|| {
        let worker = s.persistence.as_ref().ok_or_else(|| err("original persistence worker disappeared"))?;
        let io_check = || operation.check().map_err(|e| std::io::Error::other(e.to_string()));
        let directory = DirectoryFrame::capture(&worker.data_dir, &io_check).map_err(|e| err(e.to_string()))?;
        let session = DirectoryFrame::capture(&worker.session_dir, &io_check).map_err(|e| err(e.to_string()))?;
        if !directory.exists() || !session.exists() {
            return Err(err("original resource directories disappeared before worker handoff"));
        }
        Ok((directory, session))
    })();
    let (directory, session) = match frames {
        Ok(v) => v,
        Err(e) => {
            record(s, a, "failed", Some(clipped(&e)));
            return Err(e);
        }
    };
    s.agent_bridge.resource_preflight.prepared = Some(Prepared { address: a.clone(), operation, directory, session });
    Ok(())
}
pub(super) fn prepare_worker(s: &mut Session, command: &str, p: &Value) -> Result<String> {
    let a = address(s, p)?;
    if !matches!(command, BEGIN | RELEASE) {
        return Err(err("only the exact resource acquisition/release uses this worker purpose"));
    }
    if command == BEGIN {
        if s.agent_bridge.resource_preflight.live.is_some() {
            status_for(s, &a)?;
        } else {
            prepare_new(s, &a)?;
        }
    } else {
        status_for(s, &a)?;
    }
    Ok(a.request)
}
pub(super) fn discard_prepared_worker(s: &mut Session, request: &str) -> Result<()> {
    let Some(a) = s.agent_bridge.resource_preflight.prepared.as_ref() else {
        return Ok(());
    };
    if a.address.request != request {
        return Err(err("a different original preparation remains owned"));
    }
    let a = a.address.clone();
    s.agent_bridge.operations.cancel(&json!({"token":a.request}))?;
    s.agent_bridge.resource_preflight.prepared = None;
    record(s, &a, "failed", Some("original worker rejected this preparation before submission; it was not replayed".into()));
    Ok(())
}
fn begin(s: &mut Session, p: &Value) -> Result<Value> {
    let a = address(s, p)?;
    if s.agent_bridge.resource_preflight.live.is_some() {
        return status_for(s, &a);
    }
    match s.agent_bridge.resource_preflight.prepared.as_ref() {
        Some(prepared) if prepared.address == a => {}
        Some(_) => return Err(err("another exact original preparation owns the worker handoff")),
        None => prepare_new(s, &a)?,
    }
    // Retain the SAME running operation/deadline and physical frames through
    // the original queue; never start a new lifetime or rebase a replaced root.
    let Prepared { operation, directory, session, .. } =
        s.agent_bridge.resource_preflight.prepared.take().ok_or_else(|| err("original preparation disappeared"))?;
    let acquire: Result<_> = (|| {
        operation.check()?;
        let io_check = || operation.check().map_err(|e| std::io::Error::other(e.to_string()));
        directory.current(&io_check).map_err(|e| err(e.to_string()))?;
        session.current(&io_check).map_err(|e| err(e.to_string()))?;
        s.sync_persistence();
        let pause = s
            .persistence
            .as_ref()
            .ok_or_else(|| err("original worker disappeared"))?
            .pause_resource_preflight(|| operation.check().map_err(|e| e.to_string()))
            .map_err(err)?;
        s.apply_persistence_events();
        let store = s.previews.clone();
        let previews = store.pause_writers(&|| operation.check());
        let worker = s.persistence.as_ref().ok_or_else(|| err("original worker disappeared after acknowledgement"))?;
        if worker.data_dir != Path::new(&a.data_directory) {
            return Err(err("original resource worker changed during acquisition"));
        }
        directory.current(&io_check).map_err(|e| err(e.to_string()))?;
        session.current(&io_check).map_err(|e| err(e.to_string()))?;
        let (previews, failure) = match previews {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(clipped(&e))),
        };
        Ok((pause, store, previews, directory, session, failure))
    })();
    let (pause, store, previews, directory, session, failure) = match acquire {
        Ok(v) => v,
        Err(e) => {
            record(s, &a, "failed", Some(clipped(&e)));
            return Err(e);
        }
    };
    s.agent_bridge.resource_preflight.live = Some(Held {
        address: a.clone(),
        pause,
        store,
        previews,
        directory,
        session,
        project: s.project.clone(),
        revision: s.revision,
        saved_revision: s.saved_revision,
        path: s.path.clone(),
        operation,
        releasing: false,
        failure,
    });
    status_for(s, &a)
}
fn release(s: &mut Session, p: &Value) -> Result<Value> {
    let a = address(s, p)?;
    let Some(mut h) = s.agent_bridge.resource_preflight.live.take() else {
        if let Some(prepared) = s.agent_bridge.resource_preflight.prepared.as_ref() {
            if prepared.address != a {
                return Err(err("another exact original preparation owns the editing fence"));
            }
            // The real Session has returned without acquiring a worker pause.
            // Cancel that same registered request; never queue a resume marker.
            s.agent_bridge.operations.cancel(&json!({"token":a.request}))?;
            s.agent_bridge.resource_preflight.prepared = None;
            record(s, &a, "released", None);
        }
        return status_for(s, &a);
    };
    if h.address != a {
        s.agent_bridge.resource_preflight.live = Some(h);
        return Err(err("only the exact resource request can release its original hold"));
    }
    let check = media_guard::Check::new(web_time::Instant::now());
    let result = (|| {
        h.identity(s, &|| check.check())?;
        h.releasing = true;
        // Cleanup checks the SAME physical worker; cancellation/expiry of the
        // read cannot mint a replacement owner or prevent its exact resume.
        let worker = s.persistence.as_ref().ok_or_else(|| err("original worker disappeared before resume"))?;
        let expected = &h.address;
        let store = &h.store;
        let directory = h.directory.clone();
        let session = h.session.clone();
        h.pause
            .finish_owned(|| {
                check.check().map_err(|e| e.to_string())?;
                if s.agent_bridge.epoch.as_deref() != Some(expected.native_session.as_str())
                    || worker.data_dir != Path::new(&expected.data_directory)
                    || !Arc::ptr_eq(store, &s.previews)
                {
                    return Err("original resource owner changed while observing its same resume marker".into());
                }
                let io_check = || check.check().map_err(|e| std::io::Error::other(e.to_string()));
                directory.current(&io_check).map_err(|e| e.to_string())?;
                session.current(&io_check).map_err(|e| e.to_string())?;
                Ok(())
            })
            .map_err(err)
    })();
    if let Err(e) = result {
        h.failure = Some(clipped(&e));
        s.agent_bridge.resource_preflight.live = Some(h);
        return Err(e);
    }
    h.previews.take();
    h.operation.complete();
    record(s, &a, "released", None);
    Ok(reply(&a, "released", false, None))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    [
        (OWNER, "Observe Original Resource Preflight Owner", owner as fn(&mut Session, &Value) -> Result<Value>, "{}"),
        (BEGIN, "Hold Original Resource Namespace", begin, "{request:str,nativePid:uint,nativeSession:str,dataDirectory:absolute}"),
        (STATUS, "Inspect Same Resource Namespace Hold", status, "{request:str,nativePid:uint,nativeSession:str,dataDirectory:absolute}"),
        (RELEASE, "Release Same Resource Namespace Hold", release, "{request:str,nativePid:uint,nativeSession:str,dataDirectory:absolute}"),
    ]
    .into_iter()
    .map(|(id, label, run, params)| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false })
    .collect()
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod worker_tests;
