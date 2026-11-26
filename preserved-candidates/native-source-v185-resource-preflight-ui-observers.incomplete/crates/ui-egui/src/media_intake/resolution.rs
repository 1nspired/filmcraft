//! Explicit human release of uncertain staged input. This never imports,
//! edits the project, changes Undo, or grants original-source authority.
use super::{Begin, Entry, FilmcraftApp, Record, Reference, catalog, epoch, fenced, input_busy, parse, reference, same_reference, size};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ACTION: &str = "keep_imports_release_context";
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Observed {
    native_session: String,
    project_path: String,
    revision: u64,
    scope: super::Scope,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    kind: String,
    version: u32,
    pub(super) request: Begin,
    observed: Observed,
    outcome: String,
    action: String,
    source_authority: bool,
    imports_undone: bool,
    can_restart: bool,
    original_owner_write: bool,
    perception_requested: bool,
}
impl Proof {
    pub(super) fn valid(&self) -> bool {
        let q = &self.request;
        super::reference(&reference_of(q))
            && q.revision <= 9_007_199_254_740_991
            && q.scope.valid()
            && self.kind == "native_media_input_release"
            && self.version == 1
            && self.outcome == "unconfirmed"
            && self.action == ACTION
            && !self.source_authority
            && !self.imports_undone
            && !self.can_restart
            && !self.original_owner_write
            && !self.perception_requested
            && epoch(&self.observed.native_session)
            && self.observed.project_path == q.project_path
            && self.observed.scope == q.scope
            && self.observed.revision <= 9_007_199_254_740_991
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    request: Begin,
    action: String,
}
pub(super) struct Job {
    request: Begin,
    proof: Proof,
    original: Option<Record>,
    done: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    status: &'static str,
    error: Option<String>,
}
fn reference_of(q: &Begin) -> Reference {
    Reference { operation_id: q.operation_id.clone(), native_session: q.native_session.clone(), project_path: q.project_path.clone() }
}
pub(super) fn running(app: &FilmcraftApp) -> bool {
    app.media_intake.resolution.as_ref().is_some_and(|j| j.done.is_some())
}
fn reply(q: &Begin, status: &str, proof: Option<&Proof>, receipt: Option<Value>, error: Option<&str>) -> Value {
    json!({"kind":"native_media_resolution","version":1,"operation_id":q.operation_id,"status":status,"proof":proof,"receipt":receipt,"error":error})
}
fn job_reply(j: &Job) -> Value {
    reply(&j.request, j.status, if j.status == "released" { Some(&j.proof) } else { None }, None, j.error.as_deref())
}
fn saved(app: &FilmcraftApp, q: &Begin) -> Result<Option<Proof>, String> {
    if running(app) {
        return Err("Native media release is being saved; check the same ID without repeating it".into());
    }
    let Some(store) = &app.media_intake.releases else { return Ok(None) };
    match store.read(&q.operation_id)? {
        Some(Entry::Released { proof, .. }) if proof.request == *q => Ok(Some(*proof)),
        Some(_) => Err("Native media release belongs to different arguments or scope".into()),
        None => Ok(None),
    }
}
pub(super) fn assert_request(app: &FilmcraftApp, q: &Begin) -> Result<(), String> {
    if let Some(j) = &app.media_intake.resolution
        && j.request.operation_id == q.operation_id
    {
        return if j.request == *q { Ok(()) } else { Err("Released native request ID belongs to different arguments".into()) };
    }
    if !running(app) {
        let _ = saved(app, q)?;
    }
    Ok(())
}
pub(super) fn archived(app: &FilmcraftApp, r: &Reference) -> Result<Option<Value>, String> {
    if running(app) {
        return Err("Native media release is being saved; check this ID again".into());
    }
    let Some(store) = &app.media_intake.releases else { return Ok(None) };
    match store.read(&r.operation_id)? {
        Some(Entry::Released { proof, original }) if same_reference(&proof.request, r) => Ok(Some(original.map_or_else(|| fenced(r), |old| old.value))),
        Some(_) => Err("Native media release belongs to another request or document".into()),
        None => Ok(None),
    }
}
pub(super) fn read(app: &FilmcraftApp, p: &Value) -> Result<Value, String> {
    let q: Begin = parse(p)?;
    if !reference(&reference_of(&q)) {
        return Err("An exact saved native media request is required".into());
    }
    if let Some(j) = &app.media_intake.resolution
        && j.request.operation_id == q.operation_id
    {
        return if j.request == q { Ok(job_reply(j)) } else { Err("Native media release ID belongs to different arguments".into()) };
    }
    if let Some(proof) = saved(app, &q)? {
        return Ok(reply(&q, "released", Some(&proof), None, None));
    }
    Ok(reply(&q, "not_found", None, None, None))
}
pub(super) fn release(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let args: Release = parse(p)?;
    let q = args.request;
    if args.action != ACTION || !reference(&reference_of(&q)) {
        return Err("Explicit keep-imports release and an exact saved request are required".into());
    }
    if let Some(j) = &app.media_intake.resolution
        && j.request.operation_id == q.operation_id
    {
        return if j.request == q { Ok(job_reply(j)) } else { Err("Native media release ID belongs to different arguments".into()) };
    }
    if let Some(proof) = saved(app, &q)? {
        return Ok(reply(&q, "released", Some(&proof), None, None));
    }
    if app.media_intake.blocked || app.media_intake.retiring.is_some() || input_busy(app, ctx) || app.native_writer.pending.is_some() {
        return Err("Finish native input and restore writable receipt storage before releasing this selection".into());
    }
    let original = app.media_intake.records.get(&q.operation_id).cloned();
    if let Some(old) = &original {
        if old.request != q {
            return Err("Native request ID belongs to different arguments".into());
        }
        if super::retirable(old) {
            return Ok(reply(&q, "known", None, Some(old.value.clone()), None));
        }
        if old.value["status"] != "unknown" || old.value["receipt_complete"] != false {
            return Err("The native import is still active; stop it or wait for its result before releasing context".into());
        }
    }
    if let Some(old) = app.media_intake.fences.get(&q.operation_id)
        && *old != reference_of(&q)
    {
        return Err("Native cancellation fence belongs to another request".into());
    }
    // Read old terminal archives before deciding that an outcome is missing.
    if original.is_none()
        && let Some(old) = super::archived(app, &reference_of(&q))?
        && old["status"] != "cancelled_before_start"
    {
        return Ok(reply(&q, "known", None, Some(old), None));
    }
    let current = catalog(app, &q)?;
    let proof = Proof {
        kind: "native_media_input_release".into(),
        version: 1,
        request: q.clone(),
        observed: Observed {
            native_session: current["nativeSession"].as_str().ok_or("Native session is unavailable")?.into(),
            project_path: current["project"]["path"].as_str().ok_or("Native document is unavailable")?.into(),
            revision: current["revision"].as_u64().ok_or("Native revision is unavailable")?,
            scope: q.scope.clone(),
        },
        outcome: "unconfirmed".into(),
        action: ACTION.into(),
        source_authority: false,
        imports_undone: false,
        can_restart: false,
        original_owner_write: false,
        perception_requested: false,
    };
    if !proof.valid() {
        return Err("Native release lost its exact request or observed document".into());
    }
    let store = app.media_intake.releases.clone().ok_or("Native release requires a configured runtime directory")?;
    let fence = reference_of(&q);
    if original.is_none() && !app.media_intake.fences.contains_key(&q.operation_id) {
        if app.media_intake.records.len().saturating_add(app.media_intake.fences.len()) > super::MAX_REQUESTS
            || super::retained_bytes(app)?.saturating_add(size(&fence)?) > super::MAX_RETAINED_BYTES
        {
            return Err("Native release fence exceeds its bounded retention; no request was repeated".into());
        }
        // Fence BEFORE dispatch: a delayed Begin cannot race filesystem publication.
        app.media_intake.fences.insert(q.operation_id.clone(), fence);
    }
    let entry = Entry::Released { proof: Box::new(proof.clone()), original: original.clone() };
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = ctx.clone();
    let started = std::thread::Builder::new().name("native-media-release".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.publish(&[entry])))
            .unwrap_or_else(|_| Err("Native media release worker failed; request evidence was kept".into()));
        let _ = tx.send(result);
        wake.request_repaint();
    });
    let (done, status, error) = match started {
        Ok(_) => (Some(rx), "running", None),
        Err(e) => (None, "failed", Some(format!("Native release worker could not start: {e}"))),
    };
    app.media_intake.resolution = Some(Job { request: q, proof, original, done, status, error });
    ctx.request_repaint();
    app.media_intake.resolution.as_ref().map(job_reply).ok_or_else(|| "Native release disappeared".into())
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut j) = app.media_intake.resolution.take() else { return };
    let Some(rx) = j.done.take() else {
        app.media_intake.resolution = Some(j);
        return;
    };
    let result = match rx.try_recv() {
        Ok(v) => v,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            j.done = Some(rx);
            app.media_intake.resolution = Some(j);
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("Native release worker disconnected; request evidence was kept".into()),
    }
    .and_then(|()| {
        if app.media_intake.records.get(&j.request.operation_id) != j.original.as_ref()
            || j.original.is_none() && app.media_intake.fences.get(&j.request.operation_id) != Some(&reference_of(&j.request))
        {
            return Err("Native media state changed during release; request evidence was kept".into());
        }
        app.media_intake.records.remove(&j.request.operation_id);
        app.media_intake.fences.remove(&j.request.operation_id);
        Ok(())
    });
    match result {
        Ok(()) => j.status = "released",
        Err(e) => {
            j.status = "failed";
            j.error = Some(e.clone());
            app.media_intake.blocked = true;
            app.ui.status = e;
        }
    }
    app.media_intake.resolution = Some(j);
    ctx.request_repaint();
}
