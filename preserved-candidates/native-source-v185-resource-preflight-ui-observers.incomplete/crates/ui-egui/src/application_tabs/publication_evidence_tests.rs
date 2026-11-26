//! Same original App/Session/fence and human tab service; generated Root only.
use super::*;
use crate::dock::{DockNode, PanelKind};
use filmcraft_engine::{ApplicationTabHistory, History, Session};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
const W: &str = "6cd868fc-9e1c-485b-87aa-9c60e13b00bf";
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"application-actual-ui-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn setup() -> (Root, FilmcraftApp, egui::Context, filmcraft_project::ItemId) {
    let path = std::env::temp_dir().join(format!("application-actual-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("generated-root.txt"), b"application-actual-ui-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let main = s.state.active_sequence.unwrap();
    s.execute("project.associateEditorialOwner", json!({"scope":{"projectId":"p_application_tabs","anchorSequence":main.0},"workspaceId":W})).unwrap();
    Arc::make_mut(&mut s.project).metadata.insert(
        "poema.projectWorkspace.v1".into(),
        serde_json::to_string(&json!({"kind":"poema-native-project-workspace","version":1,"scope":{"projectId":"p_application_tabs","workspaceId":W}}))
            .unwrap(),
    );
    s.execute("file.newSequence", json!({"name":"Generated actual UI other","video":0,"audio":0})).unwrap();
    let other = s.state.active_sequence.unwrap();
    s.execute("sequence.open", json!({"item":main.0})).unwrap();
    s.execute("file.save", json!({"path":path.join("generated.fcproj")})).unwrap();
    s.history = History { limit: 128, ..History::default() };
    s.agent_operation_control().enable_transport();
    s.events.clear();
    let mut app = FilmcraftApp::new(s);
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1 };
    app.ui.floating.clear();
    (Root(path), app, egui::Context::default(), other)
}
fn begin(app: &mut FilmcraftApp, ctx: &egui::Context, item: filmcraft_project::ItemId, n: u64) -> (Value, Value, Started) {
    let p = json!({"token":format!("{n:064x}"),"scope":{"projectId":"p_application_tabs","workspaceId":W},"projectPath":app.session.path});
    let prep = prepare(app, ctx, &p).unwrap();
    let action = json!({"reference":prep["reference"],"item":item.0});
    let started = match begin_activation(app, ctx, &action).unwrap() {
        Beginning::Publish(s) => s,
        _ => panic!("fresh actual publication required"),
    };
    (prep, action, started)
}
fn native_state(app: &FilmcraftApp) -> Value {
    json!({"project":app.session.project,"state":app.session.state,"clipboard":app.session.state.clipboard,"undo":app.session.history.undo,"redo":app.session.history.redo,"limit":app.session.history.limit,"revision":app.session.revision,"saved":app.session.saved_revision,"path":app.session.path,"journal":app.session.journal,"trim":format!("{:?}",app.session.trim_play)})
}
fn proof(v: &Value) -> Value {
    assert_eq!(v["kind"], "native_application_tab_ui_proof");
    let data = v["data"].as_str().unwrap();
    assert_eq!(v["bytes"], data.len());
    assert_eq!(v["sha256"], format!("{:x}", Sha256::digest(data.as_bytes())));
    serde_json::from_str(data).unwrap()
}
// Optional test-output only. A managed runner creates this disposable arena
// and marker beneath its own TMPDIR. These are actual native/UI observations,
// never hand-built positive host fixtures or recoverable live grants.
fn export_observed_fixture(prep: &Value, value: &Value) {
    let Some(base) = std::env::var_os("POEMA_APPLICATION_EVIDENCE_OUTPUT") else { return };
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    let base = PathBuf::from(base);
    let root = std::env::temp_dir().canonicalize().unwrap();
    assert!(base.is_absolute());
    assert_eq!(base.canonicalize().unwrap(), base);
    assert!(base.starts_with(&root));
    assert_eq!(std::fs::read(base.join("generated-arena.txt")).unwrap(), b"generated-application-evidence-v1\n");
    let token = prep["reference"]["token"].as_str().unwrap();
    assert!(token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()));
    let payload = json!({"kind":"actual_native_application_ui_fixture","version":1,
        "producerPid":std::process::id(),"preparation":prep,"outcome":value});
    let raw = bytes(&payload, RECEIPT_BYTES + DOC_BYTES + 4096).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options.open(base.join(format!("actual-ui-{token}.json"))).unwrap();
    output.write_all(&raw).unwrap();
    output.sync_all().unwrap();
}
fn uncertain(app: &FilmcraftApp, prep: &Value, value: &Value) -> Value {
    assert_eq!(value["kind"], "native_application_tab_outcome");
    assert_eq!(value["status"], "unknown");
    assert_eq!(value["receiptComplete"], false);
    assert_eq!(value["publicationStarted"], true);
    assert_eq!(value["canReplay"], false);
    assert_eq!(value["reference"], prep["reference"]);
    let evidence = &value["publicationEvidence"];
    assert_eq!(evidence["kind"], "native_application_tab_publication_evidence");
    assert_eq!(evidence["version"], 1);
    assert_eq!(evidence["preparedUiSha256"], prep["document"]["uiSha256"]);
    assert_eq!(proof(&prep["document"]["ui"])["paintedVisibilityVerified"], false);
    assert_eq!(evidence["writeAuthority"], false);
    assert_eq!(evidence["mediaEvidence"], false);
    let stored = app.application_tabs.preparations.get(prep["reference"]["token"].as_str().unwrap()).unwrap();
    assert_eq!(stored.document, prep["document"]);
    assert_eq!(stored.evidence.as_ref().unwrap().document(), &evidence["native"]);
    assert_eq!(stored.evidence.as_ref().unwrap().confirmed(), evidence["nativeReceiptConfirmed"]);
    assert!(bytes(value, RECEIPT_BYTES).is_ok());
    assert!(retained(app).unwrap() <= RETAINED_BYTES);
    export_observed_fixture(prep, value);
    evidence.clone()
}
#[test]
fn unchanged_success_keeps_original_wire_and_actual_human_publication() {
    let (_root, mut app, ctx, other) = setup();
    let (_prep, _action, started) = begin(&mut app, &ctx, other, 3100);
    let planned = started.planned.clone();
    let effect = app.activate_sequence_tab(&ctx, other);
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    assert_eq!(value, planned);
    assert!(value.get("publicationEvidence").is_none());
    assert_eq!(native_state(&app), before);
    assert!(app.ui.dock.is_visible(PanelKind::Timeline));
    assert_eq!(app.session.state.active_sequence, Some(other));
}
#[test]
fn actual_ui_mismatch_retains_all_same_attempt_evidence_and_lookup_never_replays() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, action, started) = begin(&mut app, &ctx, other, 3200);
    let expected_native_hash = hash(&started.planned["native"], DOC_BYTES).unwrap();
    let effect = app.activate_sequence_tab(&ctx, other);
    app.ui.timeline.scroll = 19.;
    app.ui.timeline.target_scroll = 19.;
    let actual = ui_proof(&ui(&app).unwrap()).unwrap();
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    let evidence = uncertain(&app, &prep, &value);
    assert_eq!(evidence["expectedNativeSha256"], expected_native_hash);
    assert_eq!(evidence["actualUi"], actual);
    assert_eq!(evidence["actualUiMatchesExpected"], false);
    assert!(evidence["actualUiError"].is_null());
    assert!(evidence["humanServiceError"].is_null());
    assert_ne!(evidence["actualUi"], evidence["expectedAfterUi"]);
    assert_eq!(native_state(&app), before);
    app.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    app.ui.timeline.scroll = 23.;
    let later = native_state(&app);
    assert_eq!(recorded(&app, &prep["reference"]).unwrap(), value);
    assert_eq!(activate(&mut app, &ctx, &action).unwrap(), value);
    assert_eq!(native_state(&app), later);
    assert_eq!(app.ui.timeline.scroll, 23.);
}
#[test]
fn original_human_service_error_still_captures_actual_ui_and_native_graph() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, _action, started) = begin(&mut app, &ctx, other, 3300);
    app.session.execute("sequence.close", json!({"item":other.0})).unwrap();
    let effect = app.activate_sequence_tab(&ctx, other);
    assert!(effect.is_err());
    let actual = ui_proof(&ui(&app).unwrap()).unwrap();
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    let evidence = uncertain(&app, &prep, &value);
    assert_eq!(evidence["actualUi"], actual);
    assert!(evidence["humanServiceError"].is_string());
    assert!(evidence["actualUiError"].is_null());
    assert_eq!(evidence["native"]["actualSnapshotRetained"], true);
    assert_eq!(native_state(&app), before);
}
#[test]
fn unavailable_actual_ui_is_explicit_and_expected_ui_is_never_substituted() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, _action, started) = begin(&mut app, &ctx, other, 3400);
    let effect = app.activate_sequence_tab(&ctx, other);
    assert!(effect.is_ok());
    app.ui.timeline.pps = f64::NAN;
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    let evidence = uncertain(&app, &prep, &value);
    assert!(evidence["actualUi"].is_null());
    assert!(evidence["actualUiMatchesExpected"].is_null());
    assert!(evidence["actualUiError"].as_str().unwrap().contains("nonfinite"));
    assert!(evidence["expectedAfterUi"].is_object());
    assert_eq!(evidence["native"]["actualSnapshotRetained"], true);
    assert_eq!(native_state(&app), before);
    assert!(app.ui.timeline.pps.is_nan());
    assert_eq!(recorded(&app, &prep["reference"]).unwrap(), value);
}
#[test]
fn uncertain_native_effect_with_matching_actual_ui_retains_distinct_cow_roots() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, _action, started) = begin(&mut app, &ctx, other, 3500);
    let effect = app.activate_sequence_tab(&ctx, other);
    assert!(effect.is_ok());
    app.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    let evidence = uncertain(&app, &prep, &value);
    assert_eq!(evidence["actualUiMatchesExpected"], true);
    assert_eq!(evidence["nativeReceiptConfirmed"], false);
    assert_eq!(evidence["native"]["actualSnapshotRetained"], true);
    let receipt = app.application_tabs.preparations.get(prep["reference"]["token"].as_str().unwrap()).unwrap().evidence.as_ref().unwrap();
    let h = receipt.capture_history().unwrap();
    assert!(h.actual_snapshot_complete());
    assert!(h.projects().len() >= 2);
    let map: Vec<_> = (0..h.projects().len()).collect();
    assert!(ApplicationTabHistory::from_proof(&h.proof().unwrap(), h.projects(), &map).is_ok());
    assert_eq!(native_state(&app), before);
}
#[test]
fn missing_actual_native_snapshot_does_not_erase_available_actual_ui() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, _action, started) = begin(&mut app, &ctx, other, 3600);
    let effect = app.activate_sequence_tab(&ctx, other);
    assert!(effect.is_ok());
    app.session.history.limit = 1001;
    let before = native_state(&app);
    let value = finish_activation(&mut app, started, effect).unwrap();
    let evidence = uncertain(&app, &prep, &value);
    assert_eq!(evidence["actualUiMatchesExpected"], true);
    assert!(evidence["actualUi"].is_object());
    assert_eq!(evidence["native"]["actualSnapshotRetained"], false);
    assert!(evidence["native"]["actualSnapshotError"].is_string());
    let h = app.application_tabs.preparations.get(prep["reference"]["token"].as_str().unwrap()).unwrap().evidence.as_ref().unwrap().capture_history().unwrap();
    assert!(!h.actual_snapshot_complete());
    assert_eq!(native_state(&app), before);
}
#[test]
fn uncertain_reply_reserve_covers_escaped_carriers_and_refuses_oversized_prepublication_data() {
    let (_root, mut app, ctx, other) = setup();
    let (_prep, _action, started) = begin(&mut app, &ctx, other, 3700);
    let planned = started.planned.clone();
    let expected = planned["native"].clone();
    let effect = app.activate_sequence_tab(&ctx, other);
    finish_activation(&mut app, started, effect).unwrap();
    let before = native_state(&app);
    let mut unknown = unknown_skeleton(&planned).unwrap();
    let reserve = unknown_reply_reserve(&unknown, &expected).unwrap();
    let escaped = "\"\\\n".repeat(UI_BYTES / 12);
    let actual = ui_proof(&json!({"text":escaped})).unwrap();
    assert!(bytes(&actual, 2 * UI_BYTES + 4096).is_ok());
    let error = "\u{0}".repeat(512);
    unknown["publicationEvidence"]["actualUi"] = actual;
    unknown["publicationEvidence"]["actualUiError"] = json!(error);
    unknown["publicationEvidence"]["humanServiceError"] = json!(error);
    unknown["publicationEvidence"]["native"] = json!({"kind":"native_application_tab_uncertain","version":1,"status":"unknown","expected":expected,"error":error,"actualSnapshotRetained":false,"actualSnapshotError":error,"canReplay":false,"mediaEvidence":false,"writeAuthority":false});
    unknown["publicationEvidence"]["actualUiMatchesExpected"] = json!(false);
    assert!(bytes(&unknown, RECEIPT_BYTES).unwrap().len() <= reserve);
    let oversized = json!({"boundedOuter":"x".repeat(RECEIPT_BYTES-128)});
    assert!(unknown_reply_reserve(&oversized, &expected).is_err());
    assert_eq!(native_state(&app), before);
}

#[test]
fn original_application_capture_hands_real_success_and_uncertainty_to_same_worker() {
    for uncertain_ui in [false, true] {
        let (_root, mut app, ctx, other) = setup();
        let (prep, _action, started) = begin(&mut app, &ctx, other, 3800);
        let effect = app.activate_sequence_tab(&ctx, other);
        if uncertain_ui {
            app.ui.timeline.scroll = 19.;
        }
        let outcome = finish_activation(&mut app, started, effect).unwrap();
        let before = native_state(&app);
        let capture = super::capture_history(&app).unwrap().unwrap();
        assert_eq!(native_state(&app), before);
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut app.session).unwrap();
        let token = format!("{:064x}", 3900);
        let scope = prep["reference"]["scope"].clone();
        app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
        let params = json!({"token":token,"scope":scope,"workspace":W,"checkpoint":"901c432f-8c98-4f7e-806a-e53fc4745ee1","metadataBasis":true});
        let mut work = worker.start_with_application_capture(&mut app.session, "agent.saveHistoryCheckpoint", params, None, capture).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let completed = loop {
            if let Some(done) = work.try_finish() {
                break done;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        };
        let saved = completed.reply.result.unwrap();
        assert_eq!(saved["status"], "saved");
        app.session = completed.session;
        assert_eq!(native_state(&app), before);
        assert_eq!(recorded(&app, &prep["reference"]).unwrap(), outcome);
        let path = std::path::Path::new(app.session.path.as_ref().unwrap())
            .parent()
            .unwrap()
            .join(format!(".poema-history-{W}-901c432f-8c98-4f7e-806a-e53fc4745ee1.json"));
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["version"], 10);
        assert_eq!(value["application"][0]["preparation"], prep);
        assert_eq!(value["application"][0]["outcome"], outcome);
    }
}

fn save_archive(app: &mut FilmcraftApp, prep: &Value, n: u64) -> Value {
    let cap = super::capture_history(app).unwrap().unwrap();
    let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut app.session).unwrap();
    let token = format!("{n:064x}");
    let scope = prep["reference"]["scope"].clone();
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
    let mut work = worker
        .start_with_application_capture(
            &mut app.session,
            "agent.saveHistoryCheckpoint",
            json!({"token":token,"scope":scope,"workspace":W,"checkpoint":"901c432f-8c98-4f7e-806a-e53fc4745ee1","metadataBasis":true}),
            None,
            cap,
        )
        .unwrap();
    // This is the actual projection installed by the ORIGINAL worker, not a fabricated Session.
    assert!(archived_record_page(app, &json!({"reference":prep["reference"],"offset":0,"limit":1})).is_err());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let done = loop {
        if let Some(done) = work.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    app.session = done.session;
    done.reply.result.unwrap()
}
fn export_archive_fixture(prep: &Value, outcome: &Value, observation: &Value) {
    let Some(base) = std::env::var_os("POEMA_APPLICATION_ARCHIVE_OUTPUT") else { return };
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    let base = PathBuf::from(base);
    assert!(base.is_absolute());
    assert_eq!(base.canonicalize().unwrap(), base);
    assert!(base.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert_eq!(std::fs::read(base.join("generated-arena.txt")).unwrap(), b"generated-application-archive-v1\n");
    let token = prep["reference"]["token"].as_str().unwrap();
    assert!(token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()));
    let payload = json!({"kind":"actual_native_application_archive_fixture","version":1,"producerPid":std::process::id(),
        "preparation":prep,"outcome":outcome,"observation":observation});
    let raw = bytes(&payload, RECEIPT_BYTES + DOC_BYTES + 65536).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options.open(base.join(format!("archived-ui-{token}.json"))).unwrap();
    output.write_all(&raw).unwrap();
    output.sync_all().unwrap();
}
#[test]
fn archived_original_outcome_observation_is_readonly_for_success_and_actual_ui_uncertainty() {
    for uncertain_ui in [false, true] {
        let (_root, mut app, ctx, other) = setup();
        let (prep, _action, started) = begin(&mut app, &ctx, other, if uncertain_ui { 9401 } else { 9400 });
        let effect = app.activate_sequence_tab(&ctx, other);
        if uncertain_ui {
            app.ui.timeline.scroll = 23.5;
        }
        let outcome = finish_activation(&mut app, started, effect).unwrap();
        save_archive(&mut app, &prep, 9500);
        let before = native_state(&app);
        let instance = app.application_tabs.instance.clone();
        let count = app.application_tabs.retained_preparation_count();
        let observed = archived(&app, &json!({"reference":prep["reference"]})).unwrap();
        assert_eq!(observed["archive"]["outcomeStatus"], outcome["status"]);
        assert_eq!(observed["archive"]["outcomeSha256"], hash(&outcome, RECEIPT_BYTES).unwrap());
        assert_eq!(proof(&observed["currentUi"]), ui(&app).unwrap());
        assert_eq!(observed["writeAuthority"], false);
        assert_eq!(observed["requestReplay"], false);
        assert_eq!(native_state(&app), before);
        assert_eq!(app.application_tabs.instance, instance);
        assert_eq!(app.application_tabs.retained_preparation_count(), count);
        assert_eq!(recorded(&app, &prep["reference"]).unwrap(), outcome);
        let mut malformed = json!({"reference":prep["reference"],"activate":true});
        assert!(archived(&app, &malformed).is_err());
        malformed.as_object_mut().unwrap().remove("activate");
        malformed["reference"]["scope"]["projectId"] = json!("p_foreign");
        assert!(archived(&app, &malformed).is_err());
        export_archive_fixture(&prep, &outcome, &observed);
    }
}
#[test]
fn archived_outcome_survives_original_recovery_loader_normal_constructor_without_live_handles() {
    let (root, mut app, ctx, other) = setup();
    let mut cfg = filmcraft_engine::autosave::AutosaveConfig::new(root.0.join("source-state"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    app.session.start_autosave(cfg).unwrap();
    let (prep, action, started) = begin(&mut app, &ctx, other, 9600);
    let effect = app.activate_sequence_tab(&ctx, other);
    let outcome = finish_activation(&mut app, started, effect).unwrap();
    let checkpoint = save_archive(&mut app, &prep, 9601);
    let scope = prep["reference"]["scope"].clone();
    let update = "22222222-2222-4222-8222-222222222222";
    let pause = format!("{:064x}", 9602);
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":pause,"scope":scope})).unwrap();
    app.session.execute("agent.beginWorkspaceUpdate", json!({"token":pause,"scope":scope,"workspace":W,"update":update})).unwrap();
    let stage_token = format!("{:064x}", 9603);
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":stage_token,"scope":scope})).unwrap();
    let stage=app.session.execute("agent.stageProjectRecovery",json!({"token":stage_token,"scope":scope,"workspace":W,"update":update,"pauseToken":pause,"checkpoint":"901c432f-8c98-4f7e-806a-e53fc4745ee1","checkpointSha256":checkpoint["checkpoint"]["sha256"]})).unwrap();
    let retire = format!("{:064x}", 9604);
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":retire,"scope":scope})).unwrap();
    app.session
        .execute(
            "agent.retireWorkspaceUpdate",
            json!({"token":retire,"scope":scope,"workspace":W,"update":update,"pauseToken":pause,"stageSha256":stage["sha256"]}),
        )
        .unwrap();
    app.session.shutdown();
    let mut cold = Session::default();
    cold.open_project_recovery(
        &json!({"projectPath":app.session.path,"scope":scope,"workspace":W,"update":update,"sha256":stage["sha256"],"activationToken":format!("{:064x}",9605)}),
    )
    .unwrap();
    let mut fresh = FilmcraftApp::new(cold);
    assert!(fresh.application_restore_error.is_none(), "{:?}", fresh.application_restore_error);
    assert!(fresh.session.workspace_update_status().is_some());
    assert_eq!(fresh.application_tabs.retained_preparation_count(), 0);
    assert!(fresh.application_tabs.instance.is_none());
    let before = native_state(&fresh);
    let observed = archived(&fresh, &json!({"reference":prep["reference"]})).unwrap();
    let page =
        handle(&mut fresh, &ctx, "ui.sequenceTabs.archivedRecordPage", &json!({"reference":prep["reference"],"offset":0,"limit":1024})).unwrap().unwrap();
    assert_eq!(page["sha256"], format!("{:x}", Sha256::digest(serde_json::to_vec(&(&prep, &outcome)).unwrap())));
    assert_eq!(page["archive"]["outcomeStatus"], "activated");
    assert_eq!(page["writeAuthority"], false);

    assert_eq!(observed["archive"]["outcomeSha256"], hash(&outcome, RECEIPT_BYTES).unwrap());
    assert_ne!(observed["archive"]["current"]["native"]["nativeSession"], prep["document"]["native"]["nativeSession"]);
    assert_eq!(proof(&observed["currentUi"]), ui(&fresh).unwrap());
    assert_eq!(recorded(&fresh, &prep["reference"]).unwrap()["status"], "unknown");
    assert!(begin_activation(&mut fresh, &ctx, &action).is_err());
    assert_eq!(fresh.application_tabs.retained_preparation_count(), 0);
    assert!(fresh.application_tabs.instance.is_none());
    assert_eq!(native_state(&fresh), before);
    export_archive_fixture(&prep, &outcome, &observed);
    fresh.session.shutdown();
}

#[test]
fn archived_record_page_original_ui_route_is_readonly_bounded_and_cannot_activate() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, _action, started) = begin(&mut app, &ctx, other, 10100);
    let effect = app.activate_sequence_tab(&ctx, other);
    let outcome = finish_activation(&mut app, started, effect).unwrap();
    save_archive(&mut app, &prep, 10101);
    let before = native_state(&app);
    let count = app.application_tabs.retained_preparation_count();
    let request = json!({"reference":prep["reference"],"offset":0,"limit":1024});
    let page = handle(&mut app, &ctx, "ui.sequenceTabs.archivedRecordPage", &request).unwrap().unwrap();
    assert_eq!(page["kind"], "native_archived_application_tab_record_page");
    assert_eq!(page["sha256"], format!("{:x}", Sha256::digest(serde_json::to_vec(&(&prep, &outcome)).unwrap())));
    assert!(page["data"].as_str().unwrap().len() <= 2048);
    assert_eq!(page["requestReplay"], false);
    for bad in [
        json!({"reference":prep["reference"]}),
        json!({"reference":prep["reference"],"offset":-1,"limit":1}),
        json!({"reference":prep["reference"],"offset":0,"limit":0}),
        json!({"reference":prep["reference"],"offset":0,"limit":1048577}),
        json!({"reference":prep["reference"],"offset":0,"limit":1,"activate":true}),
    ] {
        assert!(handle(&mut app, &ctx, "ui.sequenceTabs.archivedRecordPage", &bad).unwrap().is_err());
    }
    assert_eq!(native_state(&app), before);
    assert_eq!(app.application_tabs.retained_preparation_count(), count);
    // Physical projection refusal is exercised in the original save_archive worker handoff above.
}


// These observations use the SAME original native/UI services at three real
// application boundaries. A tab command acknowledges a request before painting;
// the frame's own fit/view bookkeeping is real state, never an unchanged receipt.
fn actual_current(app: &mut FilmcraftApp, prep: &Value, nonce: u64) -> Value {
    read(app, &json!({"token":format!("{nonce:064x}"),"scope":prep["reference"]["scope"],"projectPath":app.session.path})).unwrap()
}
fn export_authored_observation(prep: &Value, outcome: &Value, current: &Value, boundary: &str) {
    let Some(base) = std::env::var_os("POEMA_APPLICATION_EVIDENCE_OUTPUT") else { return };
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    let base = PathBuf::from(base);
    assert_eq!(base.canonicalize().unwrap(), base);
    assert!(base.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert_eq!(std::fs::read(base.join("generated-arena.txt")).unwrap(), b"generated-application-evidence-v1\n");
    let payload = json!({"kind":"actual_native_application_readback_boundary","version":1,
        "boundary":boundary,"preparation":prep,"outcome":outcome,"current":current,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"requestReplay":false});
    let raw = bytes(&payload, RECEIPT_BYTES + 2 * DOC_BYTES + 4096).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut out = options.open(base.join(format!("actual-authored-boundary-{boundary}.json"))).unwrap();
    out.write_all(&raw).unwrap();out.sync_all().unwrap();
}
#[test]
fn authored_readback_before_frame_matches_exact_original_activation_without_edits() {
    let (_root, mut app, ctx, other) = setup();
    let (prep, action, started) = begin(&mut app, &ctx, other, 4100);
    let effect = app.activate_sequence_tab(&ctx, other);
    let outcome = finish_activation(&mut app, started, effect).unwrap();
    assert_eq!(outcome["status"], "activated");
    let before = native_state(&app);
    let current = actual_current(&mut app, &prep, 4101);
    assert_eq!(native_state(&app), before);
    assert_eq!(current["ui"], outcome["afterUi"]);
    assert_eq!(current["native"]["editorStateSha256"], outcome["native"]["after"]["editorSha256"]);
    assert_eq!(current["native"]["projectState"]["sha256"], outcome["native"]["after"]["projectSha256"]);
    assert_eq!(current["native"]["historyState"], outcome["native"]["after"]["history"]);
    assert_eq!(recorded(&app, &prep["reference"]).unwrap(), outcome);
    assert_eq!(activate(&mut app, &ctx, &action).unwrap(), outcome);
    assert_eq!(native_state(&app), before);
    export_authored_observation(&prep, &outcome, &current, "before-frame");
}
#[test]
fn authored_readback_after_passive_frames_keeps_real_fit_and_immutable_receipt() {
    let (_root, mut app, ctx, other) = setup();
    app.session.prefs.appearance.animations = false;
    let (prep, _action, started) = begin(&mut app, &ctx, other, 4200);
    let effect = app.activate_sequence_tab(&ctx, other);
    let outcome = finish_activation(&mut app, started, effect).unwrap();
    assert_eq!(outcome["status"], "activated");
    let original = native_state(&app);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000.,700.)).build_eframe(move |_| app);
    for _ in 0..3 { h.step(); }
    let before = native_state(h.state());
    let current = actual_current(h.state_mut(), &prep, 4201);
    assert_eq!(native_state(h.state()), before);
    assert_eq!(before["project"], original["project"]);
    assert_eq!(before["undo"], original["undo"]);assert_eq!(before["redo"], original["redo"]);
    assert_eq!(before["revision"], original["revision"]);
    assert_eq!(current["native"]["sequence"], other.0);
    assert_eq!(current["native"]["projectState"]["sha256"], outcome["native"]["after"]["projectSha256"]);
    assert_ne!(current["ui"], outcome["afterUi"]);
    assert_ne!(current["native"]["editorStateSha256"], outcome["native"]["after"]["editorSha256"]);
    assert_eq!(proof(&outcome["afterUi"])["pendingPanelRaise"], "Timeline");
    assert!(proof(&current["ui"])["pendingPanelRaise"].is_null());
    assert_eq!(proof(&current["ui"])["timelineViewOf"], other.0);
    assert_eq!(recorded(h.state(), &prep["reference"]).unwrap(), outcome);
    export_authored_observation(&prep, &outcome, &current, "after-passive-frames");
}
#[test]
fn authored_readback_after_human_return_keeps_human_target_without_replaying_activation() {
    let (_root, mut app, ctx, other) = setup();
    let main = app.session.state.active_sequence.unwrap();
    let (prep, action, started) = begin(&mut app, &ctx, other, 4300);
    let effect = app.activate_sequence_tab(&ctx, other);
    let outcome = finish_activation(&mut app, started, effect).unwrap();
    assert_eq!(outcome["status"], "activated");
    app.activate_sequence_tab(&ctx, main).unwrap();
    let before = native_state(&app);
    let current = actual_current(&mut app, &prep, 4301);
    assert_eq!(current["native"]["sequence"], main.0);
    assert_ne!(current["native"]["sequence"], outcome["native"]["after"]["sequence"]);
    assert_eq!(native_state(&app), before);
    assert_eq!(recorded(&app, &prep["reference"]).unwrap(), outcome);
    assert_eq!(activate(&mut app, &ctx, &action).unwrap(), outcome);
    assert_eq!(native_state(&app), before);
    export_authored_observation(&prep, &outcome, &current, "after-human-return");
}
