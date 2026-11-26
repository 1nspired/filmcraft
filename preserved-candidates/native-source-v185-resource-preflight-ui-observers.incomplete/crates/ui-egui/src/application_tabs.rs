//! Actual application tab preparation and finite original-human publication.
//! Publication outcomes are current-process evidence; durable/model integration
//! remains closed until the original archive/journal consumers are complete.
//! Serialized views and references cannot reconstruct the private native scope.
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use craft_ui::docking::Action;
use filmcraft_engine::{ApplicationTabPublication, ApplicationTabReceipt, ApplicationTabScope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const UI_BYTES: usize = 1024 * 1024;
const DOC_BYTES: usize = 8 * 1024 * 1024;
const RETAINED_BYTES: usize = 32 * 1024 * 1024;
const PREPARATIONS: usize = 64;
#[derive(Default)]
pub(crate) struct State {
    instance: Option<String>,
    preparations: BTreeMap<String, Prepared>,
}
#[cfg(test)]
impl State {
    pub(crate) fn retained_preparation_count(&self) -> usize {
        self.preparations.len()
    }
}
struct Prepared {
    request: Value,
    reference: Value,
    document: Value,
    native: Option<ApplicationTabScope>,
    attempt: Option<Attempt>,
    evidence: Option<ApplicationTabReceipt>,
}
struct Attempt {
    request: Value,
    outcome: Value,
}
const RECEIPT_BYTES: usize = 12 * 1024 * 1024;
const UNKNOWN_NATIVE_RESERVE: usize = 8 * 1024 * 1024 + 16384;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activate {
    reference: Value,
    item: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    token: String,
    scope: Value,
    project_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedRead {
    reference: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedPageRead {
    reference: Value,
    offset: usize,
    limit: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reference {
    token: String,
    application_instance: String,
    scope: Value,
    project_path: String,
    sha256: String,
}
fn bytes<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, String> {
    let b = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if b.len() > limit {
        return Err("complete original application tab data exceeds its bound".into());
    }
    Ok(b)
}
fn hash(value: &Value, limit: usize) -> Result<String, String> {
    Ok(format!("{:x}", Sha256::digest(bytes(value, limit)?)))
}
fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    bytes(value, 16384)?;
    serde_json::from_value(value.clone()).map_err(|e| format!("invalid application tab request: {e}"))
}
fn instance(app: &mut FilmcraftApp) -> Result<String, String> {
    if let Some(id) = &app.application_tabs.instance {
        return Ok(id.clone());
    }
    let id = ApplicationTabScope::new_instance_identity().map_err(|e| e.to_string())?;
    app.application_tabs.instance = Some(id.clone());
    Ok(id)
}
fn ui(app: &FilmcraftApp) -> Result<Value, String> {
    let t = &app.ui.timeline;
    if ![
        t.pps,
        t.target_pps,
        t.scroll,
        t.target_scroll,
        f64::from(t.v_scroll),
        f64::from(t.a_scroll),
        f64::from(t.split),
        f64::from(t.video_track_h),
        f64::from(t.audio_track_h),
        f64::from(t.header_w),
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return Err("the original timeline view contains a nonfinite value".into());
    }
    let layout = crate::dock_shared::encode(&app.ui.dock, &app.ui.floating)?;
    layout.validate().map_err(|e| e.to_string())?;
    let value = json!({"kind":"native_application_tab_state","version":1,"dock":layout,"mode":app.ui.mode,"focusedPanel":app.ui.focused,
        "maximized":app.ui.keys.maximized,"timeline":app.ui.timeline,"timelineViewOf":app.timeline_view_of.map(|id|id.0),"pendingPanelRaise":app.pending_panel_raise,
        "paintedVisibilityVerified":false,"osKeyboardFocusVerified":false});
    bytes(&value, UI_BYTES)?;
    Ok(value)
}
fn input_ready(app: &FilmcraftApp, ctx: &egui::Context) -> Result<(), String> {
    if app.session.is_display_only()
        || app.native_writer.pending.is_some()
        || app.session.pending_save_operation_request().is_some()
        || app.session.workspace_update_status().is_some()
        || crate::native_writer::application_held(app)
        || app.native_dialog_open()
        || !app.synthetic.is_empty()
        || !app.input_waiters.is_empty()
        || app.value_press.is_some()
        || app.tl.drag.is_some()
        || ctx.input(|i| i.pointer.any_down())
    {
        return Err("finish the original native input,writer or setup before preparing application tabs".into());
    }
    Ok(())
}
fn document(app: &mut FilmcraftApp, native: Value, state: Value, prepare: bool) -> Result<Value, String> {
    let id = instance(app)?;
    let ui_hash = hash(&state, UI_BYTES)?;
    let proof = ui_proof(&state)?;
    let value = json!({"kind":"native_application_tab_view","version":1,"status":"read","applicationInstance":id,"token":native["token"],
        "readPurpose":if prepare{"application_tab_preparation"}else{"application_tabs_only"},"native":native,"ui":proof,"uiSha256":ui_hash,
        "capabilities":{"applicationTabRead":true,"applicationTabPreparation":prepare,"applicationTabActivation":false},
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"agentUndoOwned":false});
    bytes(&value, DOC_BYTES)?;
    Ok(value)
}
// Keep floating point UI values as native-produced UTF-8 bytes. A client
// hashes this carrier instead of rounding/re-encoding them in another language.
fn ui_proof(state: &Value) -> Result<Value, String> {
    let raw = bytes(state, UI_BYTES)?;
    let digest = format!("{:x}", Sha256::digest(&raw));
    let count = raw.len();
    let data = String::from_utf8(raw).map_err(|e| e.to_string())?;
    Ok(json!({"kind":"native_application_tab_ui_proof","version":1,"bytes":count,"sha256":digest,"data":data}))
}
fn retained(app: &FilmcraftApp) -> Result<usize, String> {
    app.application_tabs.preparations.values().try_fold(0usize, |total, p| {
        let native = p.native.as_ref().map(ApplicationTabScope::retained_bytes).transpose().map_err(|e| e.to_string())?.unwrap_or(0);
        let evidence = p.evidence.as_ref().map(ApplicationTabReceipt::retained_bytes).transpose().map_err(|e| e.to_string())?.unwrap_or(0);
        let attempt = match &p.attempt {
            Some(a) => bytes(&a.request, 16384)?.len().checked_add(bytes(&a.outcome, RECEIPT_BYTES)?.len()).ok_or("application attempt size overflow")?,
            None => 0,
        };
        [bytes(&p.document, DOC_BYTES)?.len(), native, evidence, attempt]
            .into_iter()
            .try_fold(total, |n, v| n.checked_add(v).ok_or_else(|| "complete application retention size overflow".to_string()))
    })
}
fn prepare(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let r: Read = parse(p)?;
    input_ready(app, ctx)?;
    if let Some(old) = app.application_tabs.preparations.get(&r.token) {
        if old.request != *p {
            return Err("application tab token was already prepared for different arguments".into());
        }
        old.native
            .as_ref()
            .ok_or("application preparation was consumed; inspect its exact outcome without replay")?
            .observe(&mut app.session)
            .map_err(|e| e.to_string())?;
        return Ok(json!({"kind":"native_application_tab_preparation","version":1,"status":"prepared","reference":old.reference,"document":old.document}));
    }
    if app.application_tabs.preparations.len() >= PREPARATIONS || retained(app)? > RETAINED_BYTES - 20 * 1024 * 1024 {
        return Err("original application tab preparations are full; no new native read was issued".into());
    }
    let state = ui(app)?;
    let scope = app.session.prepare_application_tab_scope(&json!({"token":r.token,"scope":r.scope,"projectPath":r.project_path})).map_err(|e| e.to_string())?;
    let value = document(app, scope.document().clone(), state, true)?;
    let native_size = scope.retained_bytes().map_err(|e| e.to_string())?;
    if retained(app)?.checked_add(bytes(&value, DOC_BYTES)?.len()).and_then(|n| n.checked_add(native_size)).is_none_or(|n| n > RETAINED_BYTES) {
        return Err("complete application preparation does not fit its original retained bound".into());
    }
    let reference = json!({"token":scope.token(),"applicationInstance":value["applicationInstance"],"scope":value["native"]["scope"],"projectPath":value["native"]["project"]["path"],"sha256":hash(&value,DOC_BYTES)?});
    let result = json!({"kind":"native_application_tab_preparation","version":1,"status":"prepared","reference":reference,"document":value});
    bytes(&result, DOC_BYTES)?;
    app.application_tabs
        .preparations
        .insert(r.token, Prepared { request: p.clone(), reference, document: value, native: Some(scope), attempt: None, evidence: None });
    Ok(result)
}
fn read(app: &mut FilmcraftApp, p: &Value) -> Result<Value, String> {
    let r: Read = parse(p)?;
    let state = ui(app)?;
    let native =
        app.session.execute("agent.readSequenceViews", json!({"token":r.token,"scope":r.scope,"projectPath":r.project_path})).map_err(|e| e.to_string())?;
    document(app, native, state, false)
}
fn observe(app: &mut FilmcraftApp, p: &Value) -> Result<Value, String> {
    let r: Reference = parse(p)?;
    let old = app.application_tabs.preparations.get(&r.token).ok_or("no original application tab preparation exists for this token")?;
    if old.reference != *p
        || app.application_tabs.instance.as_ref() != Some(&r.application_instance)
        || old.document["native"]["scope"] != r.scope
        || old.document["native"]["project"]["path"] != r.project_path
        || hash(&old.document, DOC_BYTES)? != r.sha256
    {
        return Err("application tab reference belongs to a different original owner or document".into());
    }
    let native = old
        .native
        .as_ref()
        .ok_or("application preparation was consumed; inspect its exact outcome without replay")?
        .observe(&mut app.session)
        .map_err(|e| e.to_string())?;
    let state = ui(app)?;
    let proof = ui_proof(&state)?;
    let unchanged = proof == old.document["ui"];
    let ui_hash = hash(&state, UI_BYTES)?;
    let value = json!({"kind":"native_application_tab_observation","version":1,"status":if unchanged{"current"}else{"changed_since_preparation"},"reference":old.reference,"original":old.document,
        "current":{"native":native["current"],"ui":proof,"uiSha256":ui_hash},"uiUnchanged":unchanged,"rebasePerformed":false,"writeAuthority":false,"mediaEvidence":false});
    bytes(&value, DOC_BYTES.saturating_mul(2))?;
    Ok(value)
}

fn reference_record<'a>(app: &'a FilmcraftApp, p: &Value) -> Result<&'a Prepared, String> {
    let r: Reference = parse(p)?;
    let old = app.application_tabs.preparations.get(&r.token).ok_or("no original application tab preparation exists for this token")?;
    if old.reference != *p || app.application_tabs.instance.as_ref() != Some(&r.application_instance) || hash(&old.document, DOC_BYTES)? != r.sha256 {
        return Err("application tab reference belongs to a different original owner or document".into());
    }
    Ok(old)
}
fn plan_ui(app: &FilmcraftApp, before: &Value) -> Result<Value, String> {
    let mut layout = crate::dock_shared::encode(&app.ui.dock, &app.ui.floating)?;
    layout.validate().map_err(|e| e.to_string())?;
    let action = Action::Activate { panel: PanelKind::Timeline };
    crate::dock_shared::check_last_docked(&layout, &action)?;
    layout.apply(action).map_err(|e| e.to_string())?;
    let decoded = crate::dock_shared::decode(layout.clone())?;
    crate::dock_shared::encode(&decoded.root, &decoded.floating)?;
    let mut result = before.clone();
    result["dock"] = serde_json::to_value(&layout).map_err(|e| e.to_string())?;
    result["pendingPanelRaise"] = json!(PanelKind::Timeline);
    if app.ui.keys.maximized.is_some_and(|panel| !decoded.root.contains(panel) && !decoded.floating.iter().any(|g| g.panels.contains(&panel))) {
        result["maximized"] = Value::Null;
    }
    bytes(&result, UI_BYTES)?;
    Ok(result)
}
fn archived_record_page(app: &FilmcraftApp, p: &Value) -> Result<Value, String> {
    let request: ArchivedPageRead = parse(p)?;
    let _: Reference = parse(&request.reference)?;
    let value = app
        .session
        .archived_application_tab_record_page(&request.reference, request.offset, request.limit)
        .map_err(|e| e.to_string())?
        .ok_or("no original archived application record matches this reference")?;
    bytes(&value, RECEIPT_BYTES)?;
    Ok(value)
}
fn archived(app: &FilmcraftApp, p: &Value) -> Result<Value, String> {
    let request: ArchivedRead = parse(p)?;
    let _: Reference = parse(&request.reference)?;
    let native = app
        .session
        .archived_application_tab(&request.reference)
        .map_err(|e| e.to_string())?
        .ok_or("no original archived application record matches this reference")?;
    let value = json!({"kind":"native_archived_application_tab_observation","version":1,"archive":native,
        "currentUi":ui_proof(&ui(app)?)?,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"requestReplay":false});
    bytes(&value, RECEIPT_BYTES)?;
    Ok(value)
}
fn recorded(app: &FilmcraftApp, p: &Value) -> Result<Value, String> {
    let r: Reference = parse(p)?;
    if !app.application_tabs.preparations.contains_key(&r.token) {
        return Ok(
            json!({"kind":"native_application_tab_outcome","version":1,"status":"unknown","reference":p,"receiptComplete":false,"canReplay":false,"retention":"current_application_instance_only"}),
        );
    }
    let old = reference_record(app, p)?;
    Ok(old.attempt.as_ref().map(|a|a.outcome.clone()).unwrap_or_else(||json!({"kind":"native_application_tab_outcome","version":1,"status":"prepared","reference":p,"receiptComplete":false,"canReplay":false,"retention":"current_application_instance_only"})))
}
fn save_outcome(app: &mut FilmcraftApp, token: &str, value: Value, evidence: Option<ApplicationTabReceipt>) -> Result<Value, String> {
    bytes(&value, RECEIPT_BYTES)?;
    let old = app.application_tabs.preparations.get_mut(token).ok_or("original application attempt disappeared; preserve this session")?;
    old.attempt.as_mut().ok_or("original application attempt is missing")?.outcome = value.clone();
    old.evidence = evidence;
    if retained(app)? > RETAINED_BYTES {
        return Err("application evidence exceeded its retained bound; preserve this session and do not replay".into());
    }
    Ok(value)
}
// This stack-only continuation owns the ORIGINAL one-use publication. It is
// neither serializable nor constructible through the control channel.
struct Started {
    token: String,
    item: u64,
    planned: Value,
    unknown: Value,
    publication: ApplicationTabPublication,
}
enum Beginning {
    Recorded(Value),
    Publish(Started),
}
fn unknown_skeleton(planned: &Value) -> Result<Value, String> {
    // The original full UI proof remains in this SAME Prepared.document and in
    // the original Root row's preparation document. Bind it by its exact hash.
    let native_hash = hash(&planned["native"], DOC_BYTES)?;
    Ok(json!({"kind":"native_application_tab_outcome","version":1,"status":"unknown",
        "reference":planned["reference"],"item":planned["operation"]["item"],
        "publicationStarted":true,"receiptComplete":false,"canReplay":false,
        "retention":"current_application_instance_only",
        "error":"original native/application publication is uncertain; preserve this session without replay",
        "publicationEvidence":{"kind":"native_application_tab_publication_evidence","version":1,
            "preparedUiSha256":planned["preparedUiSha256"],"beforeUi":planned["beforeUi"],
            "expectedAfterUi":planned["afterUi"],"expectedNativeSha256":native_hash,
            "actualUi":null,"actualUiError":null,"actualUiMatchesExpected":null,
            "humanServiceError":null,"native":null,"nativeReceiptConfirmed":false,
            "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false}}))
}
fn unknown_reply_reserve(skeleton: &Value, expected_native: &Value) -> Result<usize, String> {
    // A UI proof wraps at most1MiB of compact native JSON. Its outer UTF-8 JSON
    // string can at most double that valid JSON's bytes; the header is bounded.
    // Native uncertainty wraps this SAME expected document with two512-char
    // errors;8192 bytes cover their worst JSON escapes and fixed fields.
    let size = bytes(skeleton, RECEIPT_BYTES)?
        .len()
        .checked_add(bytes(expected_native, DOC_BYTES)?.len())
        .and_then(|n| n.checked_add(8192))
        .and_then(|n| n.checked_add(2 * UI_BYTES + 4096))
        .and_then(|n| n.checked_add(8192))
        .ok_or("complete uncertain application receipt reserve overflow")?;
    if size > RECEIPT_BYTES {
        return Err("complete original and uncertain application UI evidence does not fit before publication".into());
    }
    Ok(size)
}
fn finish_activation(app: &mut FilmcraftApp, started: Started, effect: Result<Value, String>) -> Result<Value, String> {
    let Started { token, planned, mut unknown, publication, .. } = started;
    // Capture actual UI EVEN when the original human service reports failure.
    // An unavailable capture stays explicit; expected UI never stands in for it.
    let actual = ui(app).and_then(|state| ui_proof(&state));
    let human_error = effect.err().map(|e| e.chars().take(512).collect::<String>());
    let matches = actual.as_ref().ok().map(|proof| proof == &planned["afterUi"]);
    let failure = human_error
        .clone()
        .or_else(|| actual.as_ref().err().cloned())
        .or_else(|| (matches != Some(true)).then(|| "application tab UI differs from its exact original human service result".into()));
    let evidence = match failure {
        Some(error) => publication.abandon(&app.session, error),
        None => publication.finish(&app.session),
    };
    if evidence.confirmed() && evidence.document() == &planned["native"] {
        return save_outcome(app, &token, planned, Some(evidence));
    }
    let carrier = &mut unknown["publicationEvidence"];
    match actual {
        Ok(proof) => carrier["actualUi"] = proof,
        Err(error) => carrier["actualUiError"] = json!(error.chars().take(512).collect::<String>()),
    }
    carrier["actualUiMatchesExpected"] = json!(matches);
    carrier["humanServiceError"] = json!(human_error);
    carrier["native"] = evidence.document().clone();
    carrier["nativeReceiptConfirmed"] = json!(evidence.confirmed());
    save_outcome(app, &token, unknown, Some(evidence))
}
fn activate(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let started = match begin_activation(app, ctx, p)? {
        Beginning::Recorded(value) => return Ok(value),
        Beginning::Publish(started) => started,
    };
    let effect = app.activate_sequence_tab(ctx, filmcraft_project::ItemId(started.item));
    finish_activation(app, started, effect)
}
fn begin_activation(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Beginning, String> {
    let r: Activate = parse(p)?;
    let q: Reference = parse(&r.reference)?;
    if !(1..=9_007_199_254_740_991).contains(&r.item) {
        return Err("an exact positive observed native tab item is required".into());
    }
    let old = reference_record(app, &r.reference)?;
    if let Some(a) = &old.attempt {
        return if a.request == *p {
            Ok(Beginning::Recorded(a.outcome.clone()))
        } else {
            Err("application tab token was already attempted with different arguments".into())
        };
    }
    input_ready(app, ctx)?;
    let before = ui(app)?;
    let original_ui: Value =
        serde_json::from_str(old.document["ui"]["data"].as_str().ok_or("original application UI proof missing")?).map_err(|e| e.to_string())?;
    if ["dock", "mode", "maximized"].into_iter().any(|key| before[key] != original_ui[key]) {
        return Err("the affected original application dock changed; read again without applying an old layout".into());
    }
    let after = plan_ui(app, &before)?;
    let before_proof = ui_proof(&before)?;
    let after_proof = ui_proof(&after)?;
    let prepared_ui_sha = old.document["uiSha256"].clone();
    let stub = json!({"kind":"native_application_tab_outcome","version":1,"status":"unknown","reference":r.reference,"item":r.item,"receiptComplete":false,"canReplay":false,"retention":"current_application_instance_only","phase":"attempting"});
    let native = {
        let old = app.application_tabs.preparations.get_mut(&q.token).ok_or("original preparation disappeared")?;
        old.attempt = Some(Attempt { request: p.clone(), outcome: stub.clone() });
        old.native.take().ok_or("original native preparation already consumed")?
    };
    let rejection = |message: String| json!({"kind":"native_application_tab_outcome","version":1,"status":"rejected","reference":r.reference,"item":r.item,"receiptComplete":true,"publicationStarted":false,"canReplay":false,"retention":"current_application_instance_only","error":message.chars().take(512).collect::<String>()});
    let native = match native.prepare_activation(&mut app.session, r.item) {
        Ok(a) => a,
        Err(e) => return save_outcome(app, &q.token, rejection(e.to_string()), None).map(Beginning::Recorded),
    };
    let planned = json!({"kind":"native_application_tab_change","version":1,"status":"activated","reference":r.reference,"operation":{"type":"activate_tab","item":r.item},"native":native.document(),
        "beforeUi":before_proof,"afterUi":after_proof,"preparedUiSha256":prepared_ui_sha,"uiChangedSincePreparation":before_proof!=old_ui_proof(&original_ui)?,
        "agentTimelineCommits":0,"agentUndoOwned":false,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"receiptComplete":true,"readbackRequired":true,"canReplay":false,"durable":false,"retention":"current_application_instance_only"});
    let planned_bytes = match bytes(&planned, RECEIPT_BYTES) {
        Ok(v) => v.len(),
        Err(e) => return save_outcome(app, &q.token, rejection(e), None).map(Beginning::Recorded),
    };
    let native_bytes = match native.retained_bytes() {
        Ok(n) => n,
        Err(e) => return save_outcome(app, &q.token, rejection(e.to_string()), None).map(Beginning::Recorded),
    };
    let unknown = unknown_skeleton(&planned)?;
    let unknown_bytes = match unknown_reply_reserve(&unknown, native.document()) {
        Ok(size) => size,
        Err(e) => return save_outcome(app, &q.token, rejection(e), None).map(Beginning::Recorded),
    };
    let projected = retained(app)?
        .checked_sub(bytes(&stub, RECEIPT_BYTES)?.len())
        .and_then(|n| n.checked_add(planned_bytes.max(unknown_bytes)))
        .and_then(|n| n.checked_add(native_bytes))
        .and_then(|n| n.checked_add(UNKNOWN_NATIVE_RESERVE));
    if projected.is_none_or(|n| n > RETAINED_BYTES) {
        return save_outcome(
            app,
            &q.token,
            rejection("complete original receipt and unknown-outcome evidence space is required before publication".into()),
            None,
        )
        .map(Beginning::Recorded);
    }
    let publication = match native.enter_publication(&mut app.session) {
        Ok(a) => a,
        Err(e) => return save_outcome(app, &q.token, rejection(e.to_string()), None).map(Beginning::Recorded),
    };
    Ok(Beginning::Publish(Started { token: q.token, item: r.item, planned, unknown, publication }))
}

fn old_ui_proof(state: &Value) -> Result<Value, String> {
    ui_proof(state)
}
pub(crate) fn handle(app: &mut FilmcraftApp, ctx: &egui::Context, method: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match method {
        "ui.sequenceTabs.read" => read(app, p),
        "ui.sequenceTabs.prepare" => prepare(app, ctx, p),
        "ui.sequenceTabs.observe" => observe(app, p),
        "ui.sequenceTabs.activate" => activate(app, ctx, p),
        "ui.sequenceTabs.outcome" => recorded(app, p),
        "ui.sequenceTabs.archivedOutcome" => archived(app, p),
        "ui.sequenceTabs.archivedRecordPage" => archived_record_page(app, p),
        _ => return None,
    })
}

#[cfg(test)]
mod publication_evidence_tests;

/// Borrow original live receipts while the real Session is still on the UI.
/// This attachment has no deserializer and travels only to the same worker.
pub(crate) fn capture_history(app: &FilmcraftApp) -> Result<Option<filmcraft_engine::ApplicationSnapshotCapture>, String> {
    let mut preparations = Vec::new();
    let mut records = Vec::new();
    for old in app.application_tabs.preparations.values() {
        let Some(attempt) = &old.attempt else { continue };
        if attempt.outcome["status"] == "rejected" {
            continue;
        }
        let evidence = old.evidence.as_ref().ok_or("unsettled application publication cannot be omitted from history")?;
        preparations.push((
            json!({"kind":"native_application_tab_preparation","version":1,"status":"prepared","reference":old.reference,"document":old.document}),
            &attempt.outcome,
            evidence,
        ));
    }
    if preparations.is_empty() && !app.session.has_native_application_root().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    for (preparation, outcome, receipt) in &preparations {
        records.push(filmcraft_engine::ApplicationTabArchiveInput { preparation, outcome, receipt });
    }
    let ui = crate::application_state::capture(app)?;
    filmcraft_engine::ApplicationSnapshotCapture::capture(&app.session, &records)
        .and_then(|capture| capture.with_application_ui(&app.session, ui))
        .map(Some)
        .map_err(|e| e.to_string())
}
