//! Correlated human media selection. The existing native picker supplies the
//! paths; the existing Media Browser import supplies its exact item results.
//! No model path arguments, directory/name matching or original-project writer.
use crate::{FilmcraftApp, native_dialogs::FileDialog};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};
#[cfg(not(target_arch = "wasm32"))]
mod archive;
#[cfg(not(target_arch = "wasm32"))]
mod resolution;
#[cfg(not(target_arch = "wasm32"))]
use archive::{Archive, Entry};

const MAX_REQUESTS: usize = 128;
const MAX_SELECTED: usize = 256;
const MAX_FILES: usize = 2000;
const MAX_RECEIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Files,
    Folder,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SequenceScope {
    project_id: String,
    anchor_sequence: u64,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProjectScope {
    project_id: String,
    workspace_id: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
enum Scope {
    Sequence(SequenceScope),
    Project(ProjectScope),
}
impl Scope {
    fn valid(&self) -> bool {
        let project_id = match self {
            Self::Sequence(s) => &s.project_id,
            Self::Project(s) => &s.project_id,
        };
        if project_id.is_empty() || project_id.len() > 256 || project_id.chars().any(char::is_control) {
            return false;
        }
        match self {
            Self::Sequence(s) => (1..=9_007_199_254_740_991).contains(&s.anchor_sequence),
            Self::Project(s) => uuid(&s.workspace_id) && project_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        }
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Begin {
    operation_id: String,
    native_session: String,
    project_path: String,
    revision: u64,
    scope: Scope,
    kind: Kind,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Reference {
    operation_id: String,
    native_session: String,
    project_path: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Record {
    request: Begin,
    value: Value,
}
#[derive(Default)]
pub(crate) struct State {
    records: BTreeMap<String, Record>,
    // Stop can overtake a request still queued on another control connection.
    // A fence has no source/read authority and never expires within this app.
    fences: BTreeMap<String, Reference>,
    #[cfg(not(target_arch = "wasm32"))]
    archive: Option<Archive>,
    #[cfg(not(target_arch = "wasm32"))]
    retiring: Option<Retirement>,
    #[cfg(not(target_arch = "wasm32"))]
    releases: Option<Archive>,
    #[cfg(not(target_arch = "wasm32"))]
    resolution: Option<resolution::Job>,
    blocked: bool,
    #[cfg(test)]
    hold_retirement_delivery: bool,
}
#[cfg(not(target_arch = "wasm32"))]
struct Retirement {
    entries: Vec<Entry>,
    picker: Option<(String, Arc<Project>, u64)>,
    done: std::sync::mpsc::Receiver<Result<(), String>>,
}
impl State {
    pub(crate) fn new(data: Option<std::path::PathBuf>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let _ = data;
        // Resolve the explicitly configured runtime directory once (macOS's
        // standard /var alias included). The archive child itself is never
        // followed through a symlink, and its identity remains pinned.
        #[cfg(not(target_arch = "wasm32"))]
        let data = data.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            archive: data.as_ref().map(|p| Archive::new(p.join("MediaIntake"))),
            #[cfg(not(target_arch = "wasm32"))]
            releases: data.map(|p| Archive::new(p.join("MediaIntakeReleases"))),
            ..Self::default()
        }
    }
}
fn retirable(r: &Record) -> bool {
    r.value["receipt_complete"] == true && matches!(r.value["status"].as_str(), Some("complete" | "partial" | "cancelled" | "rejected" | "failed"))
}
#[cfg(not(target_arch = "wasm32"))]
fn retirement_batch(app: &FilmcraftApp) -> Result<Vec<Entry>, String> {
    if app.media_intake.blocked || app.media_intake.retiring.is_some() || resolution::running(app) {
        return Err("Native media receipt maintenance is unavailable; preserve this film and reopen after restoring its runtime storage".into());
    }
    if app.media_intake.archive.is_none() {
        return Err("Native media receipt retention needs the configured native runtime directory; no picker was started".into());
    }
    let mut entries: Vec<_> = app.media_intake.records.values().filter(|r| retirable(r)).take(64).cloned().map(|record| Entry::Record { record }).collect();
    entries.extend(app.media_intake.fences.values().take(64usize.saturating_sub(entries.len())).cloned().map(|reference| Entry::Fence { reference }));
    if entries.is_empty() {
        return Err("Native media retention has only unresolved requests; no picker was started".into());
    }
    Ok(entries)
}
#[cfg(not(target_arch = "wasm32"))]
fn start_retirement(app: &mut FilmcraftApp, ctx: &egui::Context, entries: Vec<Entry>, picker: Option<(String, Arc<Project>, u64)>) -> Result<(), String> {
    let store = app.media_intake.archive.clone().ok_or("Native media receipt archive is not configured")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let work = entries.clone();
    let wake = ctx.clone();
    std::thread::Builder::new()
        .name("native-media-retirement".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.publish(&work)))
                .unwrap_or_else(|_| Err("Native media retirement worker failed; all active records were retained".into()));
            let _ = tx.send(result);
            wake.request_repaint();
        })
        .map_err(|e| format!("Native media retirement worker could not start: {e}"))?;
    app.media_intake.retiring = Some(Retirement { entries, picker, done: rx });
    ctx.request_repaint();
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    resolution::poll(app, ctx);
    #[cfg(test)]
    if app.media_intake.hold_retirement_delivery {
        return;
    }
    let Some(work) = app.media_intake.retiring.take() else { return };
    let result = match work.done.try_recv() {
        Ok(result) => result,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            app.media_intake.retiring = Some(work);
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("Native media retirement worker disconnected; no active record was removed".into()),
    }
    .and_then(|()| {
        if work.entries.iter().any(|entry| match entry {
            Entry::Record { record } => app.media_intake.records.get(entry.id()) != Some(record),
            Entry::Fence { reference } => app.media_intake.fences.get(entry.id()) != Some(reference),
            Entry::Released { .. } => true,
        }) {
            return Err("Native media requests changed during retirement; active records were retained".into());
        }
        for entry in &work.entries {
            match entry {
                Entry::Record { .. } => {
                    app.media_intake.records.remove(entry.id());
                }
                Entry::Fence { .. } => {
                    app.media_intake.fences.remove(entry.id());
                }
                Entry::Released { .. } => {}
            }
        }
        Ok(())
    });
    if let Err(error) = &result {
        app.media_intake.blocked = true;
        app.ui.status = error.clone();
    }
    if let Some((id, project, bin)) = work.picker
        && let Some(record) = app.media_intake.records.get(&id)
    {
        // A queued Stop is already terminal. Never rewrite its receipt or
        // open a picker after the archive worker returns.
        if record.value["cancel_requested"] == true {
            return;
        }
        let q = record.request.clone();
        // A batch of small receipts may free slots without freeing enough
        // bytes. Continue bounded batches before opening this same picker;
        // never import files whose exact receipt could not fit afterward.
        if result.is_ok() {
            match retained_bytes(app) {
                Ok(bytes) if bytes > MAX_RETAINED_BYTES - MAX_RECEIPT_BYTES => {
                    let next = retirement_batch(app).and_then(|entries| start_retirement(app, ctx, entries, Some((id.clone(), project, bin))));
                    if let Err(error) = next
                        && let Err(error) = finish(app, &id, "failed", Vec::new(), None, Some(error))
                    {
                        app.ui.status = error;
                    }
                    return;
                }
                Err(error) => {
                    app.media_intake.blocked = true;
                    if let Err(error) = finish(app, &id, "failed", Vec::new(), None, Some(error)) {
                        app.ui.status = error;
                    }
                    return;
                }
                _ => {}
            }
        }
        let outcome = match result {
            Ok(()) => open_picker(app, ctx, q, project, bin),
            Err(e) => finish(app, &id, "failed", Vec::new(), None, Some(e)),
        };
        if let Err(error) = outcome {
            app.ui.status = error;
        }
    }
    ctx.request_repaint();
}
fn archived(app: &FilmcraftApp, r: &Reference) -> Result<Option<Value>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(value) = resolution::archived(app, r)? {
            return Ok(Some(value));
        }
        if app.media_intake.retiring.is_some() {
            return Err("Native media receipt maintenance is in progress; check this saved ID again without repeating it".into());
        }
        if let Some(store) = &app.media_intake.archive
            && let Some(entry) = store.read(&r.operation_id)?
        {
            if entry.reference() != *r {
                return Err("Retired native media receipt belongs to another request or document".into());
            }
            return Ok(Some(match entry {
                Entry::Record { record } => record.value,
                Entry::Fence { reference } => fenced(&reference),
                Entry::Released { .. } => return Err("Release evidence was found in the retirement store".into()),
            }));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, r);
    Ok(None)
}

fn parse<T: serde::de::DeserializeOwned>(p: &Value) -> Result<T, String> {
    serde_json::from_value(p.clone()).map_err(|e| format!("Invalid native media selection: {e}"))
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_digit() || (b'a'..=b'f').contains(&b) })
}
fn epoch(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn file_path(s: &str) -> bool {
    !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control) && Path::new(s).is_absolute()
}
fn reference(r: &Reference) -> bool {
    uuid(&r.operation_id) && epoch(&r.native_session) && file_path(&r.project_path)
}
fn size<T: Serialize>(v: &T) -> Result<usize, String> {
    serde_json::to_vec(v).map(|b| b.len()).map_err(|e| e.to_string())
}
fn retained_bytes(app: &FilmcraftApp) -> Result<usize, String> {
    let records = app
        .media_intake
        .records
        .values()
        .try_fold(0usize, |n, r| n.checked_add(size(&r.value)?).ok_or_else(|| "Native media receipt size overflow".to_string()))?;
    app.media_intake.fences.values().try_fold(records, |n, r| n.checked_add(size(r)?).ok_or_else(|| "Native media receipt size overflow".to_string()))
}
fn same_reference(r: &Begin, q: &Reference) -> bool {
    r.operation_id == q.operation_id && r.native_session == q.native_session && r.project_path == q.project_path
}
fn record(app: &FilmcraftApp, r: &Reference) -> Result<Value, String> {
    if !reference(r) {
        return Err("An exact saved native media request reference is required".into());
    }
    match app.media_intake.records.get(&r.operation_id) {
        Some(old) if same_reference(&old.request, r) => Ok(old.value.clone()),
        Some(_) => Err("Native media receipt belongs to another request or document".into()),
        None => match app.media_intake.fences.get(&r.operation_id) {
            Some(fence) if fence.native_session == r.native_session && fence.project_path == r.project_path => Ok(fenced(r)),
            Some(_) => Err("Native media cancellation belongs to another request or document".into()),
            None => Ok(archived(app, r)?.unwrap_or_else(|| {
                json!({"kind":"native_media_intake","version":1,"operation_id":r.operation_id,
                "status":"not_found","can_restart":false,"receipt_complete":false})
            })),
        },
    }
}
fn fenced(r: &Reference) -> Value {
    json!({"kind":"native_media_intake","version":1,"operation_id":r.operation_id,"status":"cancelled_before_start",
        "can_restart":false,"receipt_complete":true,"native_scope_verified":false})
}
fn finish(app: &mut FilmcraftApp, id: &str, status: &str, paths: Vec<String>, result: Option<Value>, error: Option<String>) -> Result<Value, String> {
    let mut value = app.media_intake.records.get(id).ok_or("Native media request disappeared")?.value.clone();
    let mut sources = Vec::new();
    if let Some(items) = result.as_ref().and_then(|r| r.get("items")).and_then(Value::as_array) {
        for id in items {
            let item = id.as_u64().and_then(|id| app.session.project.item(filmcraft_project::ItemId(id)));
            let row = match item.and_then(|i| i.as_media()) {
                Some(media) => match &media.media {
                    filmcraft_project::MediaRef::File { path } => json!({"item":id,"kind":"file","path":path}),
                    _ => json!({"item":id,"kind":"other","path":null}),
                },
                None => json!({"item":id,"kind":"other","path":null}),
            };
            sources.push(row);
        }
    }
    value["status"] = json!(status);
    value["selected_paths"] = json!(paths);
    value["result_revision"] = json!(app.session.revision);
    value["native_result"] = result.unwrap_or(Value::Null);
    value["imported_sources"] = json!(sources);
    // Diagnostic prose is bounded independently; actual import result arrays
    // are retained completely or reported unknown below.
    value["error"] = json!(error.map(|e| {
        let mut text: String = e.chars().take(4095).collect();
        if e.chars().count() > 4095 {
            text.push('…');
        }
        text
    }));
    value["receipt_complete"] = json!(status != "unknown");
    value["close_picker_to_finish"] = json!(false);
    let previous = app.media_intake.records.get(id).map(|r| size(&r.value)).transpose()?.unwrap_or(0);
    if size(&value)? > MAX_RECEIPT_BYTES || retained_bytes(app)?.saturating_sub(previous).saturating_add(size(&value)?) > MAX_RETAINED_BYTES {
        // A native import may have happened. Never make a truncated result look
        // complete or repeat the import in order to obtain a smaller reply.
        value["status"] = json!("unknown");
        value["selected_paths"] = json!([]);
        value["native_result"] = Value::Null;
        value["imported_sources"] = json!([]);
        value["receipt_complete"] = json!(false);
        value["error"] = json!("Native media result exceeds its retained bound; inspect the native project before another import");
    }
    app.media_intake.records.get_mut(id).ok_or("Native media request disappeared")?.value = value.clone();
    Ok(value)
}
fn catalog(app: &mut FilmcraftApp, q: &Begin) -> Result<Value, String> {
    app.session.execute("agent.readSourceCatalog", json!({"scope":q.scope})).map_err(|e| e.to_string())
}
fn chosen(app: &mut FilmcraftApp, ctx: &egui::Context, q: Begin, project: Arc<Project>, bin: u64, paths: Vec<String>) -> Result<Value, String> {
    let stopped = app.media_intake.records.get(&q.operation_id).is_some_and(|r| r.value["cancel_requested"] == true);
    if stopped || paths.is_empty() {
        return finish(app, &q.operation_id, "cancelled", Vec::new(), None, None);
    }
    if paths.len() > MAX_SELECTED || q.kind == Kind::Folder && paths.len() != 1 || paths.iter().any(|p| !file_path(p)) {
        return finish(app, &q.operation_id, "rejected", Vec::new(), None, Some("Native selection is oversized or contains an unsupported path".into()));
    }
    if app.session.is_display_only()
        || app.session.path.as_deref() != Some(q.project_path.as_str())
        || app.session.revision != q.revision
        || !Arc::ptr_eq(&app.session.project, &project)
    {
        return finish(app, &q.operation_id, "rejected", paths, None, Some("Native project changed while the picker was open; no import was requested".into()));
    }
    let current = match catalog(app, &q) {
        Ok(v) => v,
        Err(e) => return finish(app, &q.operation_id, "rejected", paths, None, Some(e)),
    };
    if current["nativeSession"] != q.native_session {
        return finish(app, &q.operation_id, "rejected", paths, None, Some("Native session changed while selecting media".into()));
    }
    if let Some(r) = app.media_intake.records.get_mut(&q.operation_id) {
        r.value["status"] = json!("importing");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result = crate::native_writer::start_media_import(
            app,
            ctx,
            q.operation_id.clone(),
            paths.clone(),
            json!({"paths":paths,"bin":bin,"imageSequence":false,"mediaOnly":true,"maxFiles":MAX_FILES}),
        );
        if let Err(e) = result {
            return finish(app, &q.operation_id, "failed", paths, None, Some(e));
        }
        app.media_intake.records.get(&q.operation_id).map(|r| r.value.clone()).ok_or_else(|| "Native media request disappeared".into())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        finish(app, &q.operation_id, "failed", paths, None, Some("Native media import requires a desktop worker".into()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn import_completed(
    app: &mut FilmcraftApp,
    id: &str,
    paths: Vec<String>,
    project: Arc<Project>,
    reply: filmcraft_engine::session_worker::Reply,
    stopped: bool,
) -> Result<Value, String> {
    let stopped = stopped || reply.result.as_ref().is_ok_and(|v| v["cancelled"] == true);
    let q = app.media_intake.records.get(id).ok_or("Native media request disappeared")?.request.clone();
    if stopped && let Some(r) = app.media_intake.records.get_mut(id) {
        r.value["cancel_requested"] = json!(true);
    }
    match reply.result {
        Ok(v) => {
            let status =
                if v["cancelled"] == true || v.get("errors").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) { "partial" } else { "complete" };
            finish(app, &q.operation_id, status, paths, Some(v), None)
        }
        Err(e) => {
            let unchanged = app.session.revision == q.revision && Arc::ptr_eq(&app.session.project, &project);
            let status = if reply.uncertain {
                "unknown"
            } else if unchanged && stopped {
                "cancelled"
            } else if unchanged {
                "failed"
            } else {
                "unknown"
            };
            finish(
                app,
                &q.operation_id,
                status,
                if status == "cancelled" { Vec::new() } else { paths },
                None,
                if status == "cancelled" { None } else { Some(e.to_string()) },
            )
        }
    }
}
fn dialog_for(kind: Kind) -> FileDialog {
    match kind {
        Kind::Folder => FileDialog::Folder { at: None },
        Kind::Files => {
            let exts: Vec<&str> =
                filmcraft_media::VIDEO_EXTENSIONS.iter().chain(filmcraft_media::AUDIO_EXTENSIONS).chain(filmcraft_media::STILL_EXTENSIONS).copied().collect();
            FileDialog::open_files(&exts)
        }
    }
}
fn input_busy(app: &FilmcraftApp, ctx: &egui::Context) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if resolution::running(app) {
        return true;
    }
    app.session.is_display_only()
        || app.native_dialog_open()
        || !app.synthetic.is_empty()
        || !app.input_waiters.is_empty()
        || app.value_press.is_some()
        || app.tl.drag.is_some()
        || ctx.input(|i| i.pointer.any_down())
}
fn open_picker(app: &mut FilmcraftApp, ctx: &egui::Context, q: Begin, project: Arc<Project>, bin: u64) -> Result<Value, String> {
    if input_busy(app, ctx)
        || app.session.path.as_deref() != Some(q.project_path.as_str())
        || app.session.revision != q.revision
        || !Arc::ptr_eq(&app.session.project, &project)
    {
        return finish(
            app,
            &q.operation_id,
            "rejected",
            Vec::new(),
            None,
            Some("Native project or input changed while preparing the picker; no import was requested".into()),
        );
    }
    let current = match catalog(app, &q) {
        Ok(v) => v,
        Err(e) => return finish(app, &q.operation_id, "rejected", Vec::new(), None, Some(e)),
    };
    if current["nativeSession"] != q.native_session {
        return finish(app, &q.operation_id, "rejected", Vec::new(), None, Some("Native session changed while preparing the picker".into()));
    }
    let dialog = dialog_for(q.kind);
    if !dialog.available(&app.hooks) {
        return finish(app, &q.operation_id, "failed", Vec::new(), None, Some("The native host has no media picker; no import was started".into()));
    }
    let request = q.clone();
    let context = ctx.clone();
    if let Err(e) = app.pick(dialog, move |app, paths| chosen(app, &context, request, project, bin, paths)) {
        return finish(app, &q.operation_id, "failed", Vec::new(), None, Some(e));
    }
    app.media_intake.records.get(&q.operation_id).map(|r| r.value.clone()).ok_or_else(|| "Native media request disappeared".into())
}
fn begin(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let q: Begin = parse(p)?;
    if !uuid(&q.operation_id) || !epoch(&q.native_session) || !file_path(&q.project_path) || !q.scope.valid() || q.revision > 9_007_199_254_740_991 {
        return Err("An exact current native document and unique media request are required".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    resolution::assert_request(app, &q)?;
    if let Some(old) = app.media_intake.records.get(&q.operation_id) {
        return if old.request == q { Ok(old.value.clone()) } else { Err("Native media request ID was already used with different arguments".into()) };
    }
    if let Some(old) = app.media_intake.fences.get(&q.operation_id) {
        return if old.native_session == q.native_session && old.project_path == q.project_path {
            Ok(fenced(old))
        } else {
            Err("Native media request ID was cancelled for another document".into())
        };
    }
    let reference = Reference { operation_id: q.operation_id.clone(), native_session: q.native_session.clone(), project_path: q.project_path.clone() };
    if let Some(old) = archived(app, &reference)? {
        if old["status"] == "cancelled_before_start"
            || old["start_revision"] == q.revision
                && old["scope"] == serde_json::to_value(&q.scope).map_err(|e| e.to_string())?
                && old["selection_kind"] == serde_json::to_value(q.kind).map_err(|e| e.to_string())?
        {
            return Ok(old);
        }
        return Err("Retired native media request ID was already used with different arguments".into());
    }
    if app.media_intake.blocked {
        return Err("Native media receipt archive failed; restore writable runtime storage and reopen this native app before another selection".into());
    }
    if cfg!(target_arch = "wasm32") || input_busy(app, ctx) {
        return Err("Finish the current native input, dialog or update before choosing media".into());
    }
    let full = app.media_intake.records.len().saturating_add(app.media_intake.fences.len()) >= MAX_REQUESTS
        || retained_bytes(app)? > MAX_RETAINED_BYTES - MAX_RECEIPT_BYTES;
    #[cfg(not(target_arch = "wasm32"))]
    let retirement = if full { Some(retirement_batch(app)?) } else { None };
    #[cfg(target_arch = "wasm32")]
    if full {
        return Err("Native media receipt retirement requires a desktop runtime".into());
    }
    let current = catalog(app, &q)?;
    if current["nativeSession"] != q.native_session || current["revision"] != q.revision || current["project"]["path"] != q.project_path {
        return Err("Native media request belongs to a stale or different document".into());
    }
    if !dialog_for(q.kind).available(&app.hooks) {
        return Err("The native host has no media picker; no import was started".into());
    }
    let bin = app.import_bin().0;
    let value = json!({"kind":"native_media_intake","version":1,"operation_id":q.operation_id,"native_session":q.native_session,"scope":q.scope,
        "project_path":q.project_path,"start_revision":q.revision,"selection_kind":q.kind,"destination_bin":bin,
        "status":"selecting","cancel_requested":false,"selected_paths":[],"imported_sources":[],"native_result":null,"result_revision":null,
        "receipt_complete":false,"close_picker_to_finish":false,"error":null,"can_restart":false,"original_owner_write":false,"perception_requested":false});
    app.media_intake.records.insert(q.operation_id.clone(), Record { request: q.clone(), value });
    let project = app.session.project.clone();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(entries) = retirement {
        if let Err(e) = start_retirement(app, ctx, entries, Some((q.operation_id.clone(), project, bin))) {
            app.media_intake.blocked = true;
            return finish(app, &q.operation_id, "failed", Vec::new(), None, Some(e));
        }
        return app.media_intake.records.get(&q.operation_id).map(|r| r.value.clone()).ok_or_else(|| "Native media request disappeared".into());
    }
    open_picker(app, ctx, q, project, bin)
}

#[cfg(test)]
#[path = "media_intake_tests.rs"]
mod tests;

pub(crate) fn handle(app: &mut FilmcraftApp, ctx: &egui::Context, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "ui.mediaIntake.info" => {
            if p.as_object().is_none_or(|o| !o.is_empty()) {
                Err("Native media discovery takes an empty object".into())
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                let durable = app.media_intake.archive.is_some();
                #[cfg(target_arch = "wasm32")]
                let durable = false;
                Ok(json!({"kind":"native_media_intake_capabilities","version":4,"native_only":true,
                    "files":!cfg!(target_arch="wasm32")&&FileDialog::open_files(&[]).available(&app.hooks),
                    "folder":!cfg!(target_arch="wasm32")&&FileDialog::Folder{at:None}.available(&app.hooks),
                    "maximum_selected_paths":MAX_SELECTED,"maximum_media_files":MAX_FILES,"maximum_retained_requests":MAX_REQUESTS+1,
                    "independent_project_scope":!cfg!(target_arch="wasm32"),"unconfirmed_resolution":durable,"maximum_active_requests":MAX_REQUESTS,"durable_retirement":durable,"background_retirement":true,
                    "correlated_async_results":true,"background_import":true,"cooperative_stop":true,"source_validation_required":true,"model_executor":false}))
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        "ui.mediaIntake.release" => resolution::release(app, ctx, p),
        #[cfg(not(target_arch = "wasm32"))]
        "ui.mediaIntake.resolution" => resolution::read(app, p),
        "ui.mediaIntake.begin" => begin(app, ctx, p),
        "ui.mediaIntake.read" => parse::<Reference>(p).and_then(|r| record(app, &r)),
        "ui.mediaIntake.stop" => parse::<Reference>(p).and_then(|r| {
            let value = record(app, &r)?;
            if value["status"] == "not_found" {
                if app.media_intake.blocked {
                    return Err("Native media receipt archive is blocked; cancellation remains unconfirmed".into());
                }
                let full = app.media_intake.records.len().saturating_add(app.media_intake.fences.len()) >= MAX_REQUESTS
                    || retained_bytes(app)?.saturating_add(size(&r)?) > MAX_RETAINED_BYTES;
                #[cfg(not(target_arch = "wasm32"))]
                let retirement = if full { Some(retirement_batch(app)?) } else { None };
                #[cfg(target_arch = "wasm32")]
                if full {
                    return Err("Native media cancellation retention needs a desktop runtime".into());
                }
                app.media_intake.fences.insert(r.operation_id.clone(), r.clone());
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(entries) = retirement
                    && let Err(e) = start_retirement(app, ctx, entries, None)
                {
                    app.media_intake.blocked = true;
                    app.ui.status = e;
                }
                Ok(fenced(&r))
            } else if value["status"] == "selecting" || value["status"] == "importing" || value["status"] == "stopping" {
                #[cfg(not(target_arch = "wasm32"))]
                if app.media_intake.retiring.as_ref().is_some_and(|w| w.picker.as_ref().is_some_and(|p| p.0 == r.operation_id)) {
                    if let Some(entry) = app.media_intake.records.get_mut(&r.operation_id) {
                        entry.value["cancel_requested"] = json!(true);
                    }
                    return finish(app, &r.operation_id, "cancelled", Vec::new(), None, None);
                }
                let importing = value["status"] == "importing" || value["status"] == "stopping" && value["close_picker_to_finish"] == false;
                #[cfg(not(target_arch = "wasm32"))]
                if importing {
                    crate::native_writer::stop_media_import(app, &r.operation_id)?;
                }
                let entry = app.media_intake.records.get_mut(&r.operation_id).ok_or("Native media request disappeared")?;
                entry.value["cancel_requested"] = json!(true);
                entry.value["status"] = json!("stopping");
                entry.value["close_picker_to_finish"] = json!(!importing);
                Ok(entry.value.clone())
            } else {
                Ok(value)
            }
        }),
        _ => return None,
    })
}
