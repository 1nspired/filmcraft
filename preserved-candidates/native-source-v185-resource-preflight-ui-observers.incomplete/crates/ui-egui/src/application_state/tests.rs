use super::*;
use filmcraft_engine::{ApplicationSnapshotCapture, Session, session_worker::SessionWorker};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
const W: &str = "11111111-1111-4111-8111-111111111111";
const C: &str = "33333333-3333-4333-8333-333333333333";
const U: &str = "22222222-2222-4222-8222-222222222222";
fn scope() -> Value {
    json!({"projectId":"p_passive_ui","workspaceId":W})
}
fn token(n: u64) -> String {
    format!("{n:064x}")
}
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"passive-ui-app-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn setup() -> (Root, FilmcraftApp) {
    let root = std::env::temp_dir().join(format!("passive-ui-app-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("generated-root.txt"), b"passive-ui-app-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Passive UI","workspace":scope()})).unwrap();
    let mut cfg = filmcraft_engine::autosave::AutosaveConfig::new(root.join("source-state"));
    cfg.journal_debounce = std::time::Duration::from_secs(600);
    s.start_autosave(cfg).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    s.agent_operation_control().enable_transport();
    let app = FilmcraftApp::new(s);
    (Root(root), app)
}
fn decorate(app: &mut FilmcraftApp) {
    app.ui.project_search = "human search".into();
    app.ui.effects_search = "human effects".into();
    app.ui.caption_search = "human caption".into();
    app.ui.timeline.pps = 71.25;
    app.ui.timeline.target_pps = 83.5;
    app.ui.timeline.scroll = 0.125;
    app.ui.timeline.target_scroll = 0.25;
    app.ui.timeline.fit_empty = Some(62.5);
    app.ui.timeline.seen_playhead = Some((filmcraft_project::ItemId(1), 0.375));
    app.ui.project_panel = Default::default();
    app.dialog = Some(Dialog::About);
    app.file_dialogs.recovery_choice = 7;
    app.shortcut_editor.search = "custom key draft".into();
    app.shortcut_editor.message = "unsent shortcut note".into();
    app.ui.eyedropper = Some(Eyedropper { clip: 77, effect: 2, param: "Tint".into(), mask: None });
    app.pending_panel_raise = Some(PanelKind::Project);
    app.stale_clipboard_media = Some("old clipboard fingerprint".into());
    app.workspaces.current = "Human layout".into();
    app.playback.speed = 2.0;
    app.playback.looping = true;
}
fn save(app: &mut FilmcraftApp, n: u64) -> Value {
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope()})).unwrap();
    let cap = crate::application_tabs::capture_history(app).unwrap().unwrap();
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut app.session).unwrap();
    let mut work = worker
        .start_with_application_capture(
            &mut app.session,
            "agent.saveHistoryCheckpoint",
            json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true}),
            None,
            cap,
        )
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let done = loop {
        if let Some(done) = work.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    let result = done.reply.result.unwrap();
    app.session = done.session;
    result
}
#[test]
fn all_ui_and_serde_skipped_passive_fields_survive_original_worker_and_constructor() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    let expected = capture(&app).unwrap();
    let project = app.session.project.clone();
    save(&mut app, 10);
    assert!(app.session.saved_application_ui().unwrap().is_some(), "same worker must return a current native UI image");
    let shortcut_revision = app.session.shortcuts.revision;
    let source = std::mem::take(&mut app.session);
    let fresh = FilmcraftApp::new(source);
    assert_eq!(fresh.session.shortcuts.revision, shortcut_revision + 1, "original registration behavior must remain unchanged");
    assert!(fresh.application_restore_error.is_none(), "{:?}", fresh.application_restore_error);
    assert!(Arc::ptr_eq(&project, &fresh.session.project));
    assert_eq!(capture(&fresh).unwrap(), expected);
    assert_eq!(fresh.ui.timeline.fit_empty, Some(62.5));
    assert_eq!(fresh.ui.eyedropper.as_ref().unwrap().param, "Tint");
    assert_eq!(fresh.file_dialogs.recovery_choice, 7);
}
#[test]
fn genuine_trusted_native_cold_recovery_installs_ui_through_normal_constructor() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    let expected = capture(&app).unwrap();
    let checkpoint = save(&mut app, 20);
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(30),"scope":scope()})).unwrap();
    app.session.execute("agent.beginWorkspaceUpdate", json!({"token":token(30),"scope":scope(),"workspace":W,"update":U})).unwrap();
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(40),"scope":scope()})).unwrap();
    let stage=app.session.execute("agent.stageProjectRecovery",json!({"token":token(40),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"checkpoint":C,"checkpointSha256":checkpoint["checkpoint"]["sha256"]})).unwrap();
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(50),"scope":scope()})).unwrap();
    app.session
        .execute(
            "agent.retireWorkspaceUpdate",
            json!({"token":token(50),"scope":scope(),"workspace":W,"update":U,"pauseToken":token(30),"stageSha256":stage["sha256"]}),
        )
        .unwrap();
    app.session.shutdown();
    let mut cold = Session::default();
    cold.open_project_recovery(
        &json!({"projectPath":app.session.path,"scope":scope(),"workspace":W,"update":U,"sha256":stage["sha256"],"activationToken":token(100)}),
    )
    .unwrap();
    let fresh = FilmcraftApp::new(cold);
    assert!(fresh.application_restore_error.is_none(), "{:?}", fresh.application_restore_error);
    assert_eq!(capture(&fresh).unwrap(), expected);
    assert!(fresh.session.workspace_update_status().is_some());
    assert_eq!(crate::native_writer::inspect(&fresh)["available"], false);
    assert!(fresh.application_tabs.retained_preparation_count() == 0);
}
#[test]
fn stale_native_image_blocks_constructor_adoption_and_preserves_later_human_work() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    save(&mut app, 11);
    app.session.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    let project = app.session.project.clone();
    let fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
    assert!(fresh.application_restore_error.is_some());
    assert!(Arc::ptr_eq(&project, &fresh.session.project));
    assert!(crate::native_writer::application_held(&fresh));
}
#[test]
fn unknown_payload_schema_refuses_atomically_without_current_ui_or_old_grants() {
    let (_root, mut app) = setup();
    let mut proof = capture(&app).unwrap();
    let mut body: Value = serde_json::from_str(proof["data"].as_str().unwrap()).unwrap();
    body["extra"] = json!(true);
    let data = serde_json::to_string(&body).unwrap();
    proof["data"] = json!(data);
    proof["bytes"] = json!(data.len());
    proof["sha256"] = json!(format!("{:x}", Sha256::digest(data.as_bytes())));
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut app.session).unwrap();
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(12),"scope":scope()})).unwrap();
    let capture = ApplicationSnapshotCapture::capture(&app.session, &[]).unwrap().with_application_ui(&app.session, proof).unwrap();
    // The same native worker installs immutable data; it is not an alternate UI importer.
    let mut pending = worker
        .start_with_application_capture(
            &mut app.session,
            "agent.saveHistoryCheckpoint",
            json!({"token":token(12),"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true}),
            None,
            capture,
        )
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let result = loop {
        if let Some(done) = pending.try_finish() {
            break done;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(result.reply.result.is_ok());
    let fresh = FilmcraftApp::new(result.session);
    assert!(fresh.application_restore_error.is_some());
    assert!(fresh.application_tabs.retained_preparation_count() == 0);
    assert_eq!(crate::native_writer::inspect(&fresh)["available"], false);
    assert!(fresh.control_rx.is_none());
}
#[test]
fn pending_real_input_is_not_serialized_as_passive_state_or_replayed() {
    let (_root, mut app) = setup();
    let project = app.session.project.clone();
    app.synthetic.push(egui::Event::Text("pending".into()));
    assert!(capture(&app).is_err());
    assert!(Arc::ptr_eq(&project, &app.session.project));
    app.synthetic.clear();
    assert!(capture(&app).is_ok());
}
#[test]
fn fresh_runtime_preserves_playback_choices_without_reconstructing_audio_or_callbacks() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    app.playback.playing = true;
    save(&mut app, 13);
    let fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
    assert!(fresh.application_restore_error.is_none());
    assert!(!fresh.playback.playing);
    assert_eq!(fresh.playback.speed, 2.0);
    assert!(fresh.playback.looping);
    assert!(fresh.audio.is_none());
    assert_eq!(crate::native_writer::inspect(&fresh)["available"], false);
    assert!(fresh.control_rx.is_none());
    assert!(fresh.synthetic.is_empty());
    assert!(fresh.pending_screenshots.is_empty());
}
#[test]
fn passive_timers_rebase_on_fresh_native_clock_only_once() {
    let (_root, mut app) = setup();
    app.last_ui_time = 100.0;
    app.status_seen = ("status".into(), 95.0);
    app.toast = Some(("toast".into(), 110.0));
    save(&mut app, 14);
    let mut fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
    assert!(fresh.application_restore_error.is_none());
    rebase_clock(&mut fresh, 2.0);
    assert_eq!(fresh.status_seen.1, -3.0);
    assert_eq!(fresh.toast.as_ref().unwrap().1, 12.0);
    rebase_clock(&mut fresh, 7.0);
    assert_eq!(fresh.toast.as_ref().unwrap().1, 12.0);
}

#[test]
fn rejected_restore_cannot_install_writer_or_accept_control_edits() {
    let (_root, mut app) = setup();
    save(&mut app, 15);
    app.session.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    let project = app.session.project.clone();
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut fresh = FilmcraftApp::new(std::mem::take(&mut app.session)).with_control(rx);
    assert!(fresh.application_restore_error.is_some());
    assert_eq!(crate::native_writer::inspect(&fresh)["available"], false);
    let (request, _reply) = crate::control::ControlRequest::new("engine.execute", json!({"command":"file.newBin","params":{"name":"Must refuse"}}));
    let result = crate::control::handle(&mut fresh, &egui::Context::default(), &request);
    let crate::control::Outcome::Done(reply) = result else { panic!("unexpected nonterminal refusal") };
    assert_eq!(reply["ok"], false);
    assert!(Arc::ptr_eq(&project, &fresh.session.project));
}

#[test]
fn first_original_preference_pass_preserves_recovered_custom_layout() {
    let (_root, mut app) = setup();
    app.workspaces.current = "Editing".into();
    app.ui.dock = crate::dock::workspace("Color");
    app.pending_panel_raise = Some(PanelKind::Source);
    app.ui.keys.maximized = Some(PanelKind::Program);
    let expected = crate::dock_shared::encode(&app.ui.dock, &app.ui.floating).unwrap();
    let preset = crate::dock::saved_docking(&app.workspaces, "Editing");
    assert_ne!(serde_json::to_value(&expected).unwrap(), serde_json::to_value(crate::dock_shared::encode(&preset.root, &preset.floating).unwrap()).unwrap());
    save(&mut app, 16);
    let mut fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
    assert!(fresh.application_restore_error.is_none());
    assert!(fresh.workspace_restored);
    assert!(fresh.audio.is_none());
    assert!(fresh.session.voiceover.input.is_none());
    fresh.apply_prefs(&egui::Context::default());
    assert_eq!(serde_json::to_value(crate::dock_shared::encode(&fresh.ui.dock, &fresh.ui.floating).unwrap()).unwrap(), serde_json::to_value(expected).unwrap());
    assert_eq!(fresh.pending_panel_raise, Some(PanelKind::Source));
    assert_eq!(fresh.ui.keys.maximized, Some(PanelKind::Program));
    assert!(fresh.audio.is_none());
}

#[test]
fn recapture_before_first_ui_frame_preserves_original_clock_base() {
    let (_root, mut app) = setup();
    app.last_ui_time = 100.0;
    app.status_seen = ("status".into(), 95.0);
    app.toast = Some(("toast".into(), 110.0));
    save(&mut app, 17);
    let mut fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
    assert!(fresh.application_restore_error.is_none());
    let proof = capture(&fresh).unwrap();
    let saved: Value = serde_json::from_str(proof["data"].as_str().unwrap()).unwrap();
    assert_eq!(saved["captured_ui_time"], 100.0);
    assert_eq!(saved["status_seen"][1], 95.0);
    rebase_clock(&mut fresh, 2.0);
    assert_eq!(fresh.status_seen.1, -3.0);
    assert_eq!(fresh.toast.as_ref().unwrap().1, 12.0);
}
#[test]
fn clock_overflow_or_nonfinite_fresh_time_refuses_atomically_without_native_mutation() {
    let (_root, mut app) = setup();
    let project = app.session.project.clone();
    for now in [f64::MAX, f64::NAN, f64::INFINITY] {
        app.application_clock_rebase = Some(-f64::MAX);
        app.application_restore_error = None;
        app.ui_error = None;
        app.status_seen = ("status".into(), f64::MAX);
        app.toast = Some(("toast".into(), 3.0));
        rebase_clock(&mut app, now);
        assert!(app.application_restore_error.is_some());
        assert!(crate::native_writer::application_held(&app));
        assert_eq!(app.application_clock_rebase, Some(-f64::MAX));
        assert_eq!(app.status_seen.1, f64::MAX);
        assert_eq!(app.toast.as_ref().unwrap().1, 3.0);
        assert!(Arc::ptr_eq(&project, &app.session.project));
    }
}

#[test]
fn readonly_ui_capture_during_pause_cannot_become_native_writer_attachment() {
    let (_root, mut app) = setup();
    save(&mut app, 18);
    let project = app.session.project.clone();
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(180),"scope":scope()})).unwrap();
    app.session.execute("agent.beginWorkspaceUpdate", json!({"token":token(180),"scope":scope(),"workspace":W,"update":U})).unwrap();
    let before = app.session.workspace_update_status();
    assert!(app.session.is_display_only());
    assert!(capture(&app).is_ok());
    assert!(crate::application_tabs::capture_history(&app).is_err());
    assert!(app.session.execute("file.newBin", json!({"name":"Must stay paused"})).is_err());
    assert_eq!(app.session.workspace_update_status(), before);
    assert!(Arc::ptr_eq(&project, &app.session.project));
}

fn original_pause_rpc(app: &mut FilmcraftApp, params: Value) -> Value {
    if crate::native_writer::inspect(app)["available"] != true {
        crate::native_writer::install(app);
    }
    let ctx = egui::Context::default();
    let (request, reply) = crate::ControlRequest::new("engine.execute", json!({"command":"agent.beginWorkspaceUpdate","params":params}));
    match crate::native_writer::intercept(app, &ctx, &request) {
        Some(crate::control::Outcome::Pending) => {}
        Some(crate::control::Outcome::Done(value)) => return value,
        _ => panic!("original pause must use native worker interception"),
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while crate::native_writer::inspect(app)["active"] == true {
        assert!(std::time::Instant::now() < until, "bounded original UI pause worker");
        crate::native_writer::poll(app, &ctx);
        std::thread::yield_now();
    }
    reply.recv_timeout(std::time::Duration::from_secs(1)).unwrap()
}
fn begin_current_ui(app: &mut FilmcraftApp, n: u64) -> (Value, Value) {
    if crate::native_writer::inspect(app)["available"] != true {
        crate::native_writer::install(app);
    }
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope()})).unwrap();
    let params = json!({"token":token(n),"scope":scope(),"workspace":W,"update":U});
    (original_pause_rpc(app, params.clone()), params)
}
#[test]
fn original_pause_captures_ui_only_clean_changes_without_creating_project_edits_or_journal_input() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    let expected = capture(&app).unwrap();
    let project = app.session.project.clone();
    let revision = app.session.revision;
    let saved_revision = app.session.saved_revision;
    let history = serde_json::to_value(&app.session.history.undo).unwrap();
    let saved = std::fs::read(app.session.path.as_ref().unwrap()).unwrap();
    assert!(app.session.saved_application_ui().unwrap().is_none());
    assert!(!app.session.is_dirty());
    let (reply, _) = begin_current_ui(&mut app, 301);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["status"], "paused");
    assert_eq!(app.session.saved_application_ui().unwrap(), Some(expected));
    assert!(Arc::ptr_eq(&project, &app.session.project));
    assert_eq!(app.session.revision, revision);
    assert_eq!(app.session.saved_revision, saved_revision);
    assert!(!app.session.is_dirty());
    assert_eq!(serde_json::to_value(&app.session.history.undo).unwrap(), history);
    assert_eq!(std::fs::read(app.session.path.as_ref().unwrap()).unwrap(), saved);
    assert_eq!(app.session.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
    assert!(!app.session.is_dirty());
    assert_eq!(app.session.persistence.as_ref().unwrap().auto_save_now().unwrap(), None);
}
#[test]
fn actual_current_ui_after_human_save_save_as_and_copy_enters_original_pause_and_constructor() {
    for command in ["file.save", "file.saveAs", "file.saveCopy"] {
        let (root, mut app) = setup();
        save(&mut app, 302);
        app.session.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
        decorate(&mut app);
        let original_path = app.session.path.clone().unwrap();
        let original_bytes = std::fs::read(&original_path).unwrap();
        let output = root.0.join("Human-output.fcproj");
        let params = if command == "file.save" { json!({}) } else { json!({"path":output}) };
        app.session.execute(command, params).unwrap();
        if command == "file.saveAs" {
            assert_eq!(app.session.path.as_ref().unwrap(), output.to_str().unwrap());
        } else {
            assert_eq!(app.session.path.as_ref().unwrap(), &original_path);
        }
        assert_eq!(app.session.is_dirty(), command == "file.saveCopy");
        if command == "file.saveCopy" {
            assert_eq!(std::fs::read(&original_path).unwrap(), original_bytes);
            assert!(output.is_file());
        }
        assert!(app.session.saved_application_ui().is_err());
        let expected = capture(&app).unwrap();
        let path = app.session.path.clone();
        let project = app.session.project.clone();
        let revision = app.session.revision;
        let dirty = app.session.is_dirty();
        let history = serde_json::to_value(&app.session.history.undo).unwrap();
        let saved = std::fs::read(app.session.path.as_ref().unwrap()).unwrap();
        let (reply, _) = begin_current_ui(&mut app, 303);
        assert_eq!(reply["ok"], true, "{command}: {reply}");
        assert_eq!(app.session.saved_application_ui().unwrap(), Some(expected.clone()));
        assert_eq!(app.session.path, path);
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!(app.session.revision, revision);
        assert_eq!(app.session.is_dirty(), dirty);
        assert_eq!(serde_json::to_value(&app.session.history.undo).unwrap(), history);
        assert_eq!(std::fs::read(app.session.path.as_ref().unwrap()).unwrap(), saved);
        assert_eq!(app.session.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
        let fresh = FilmcraftApp::new(std::mem::take(&mut app.session));
        assert!(fresh.application_restore_error.is_none(), "{command}: {:?}", fresh.application_restore_error);
        assert_eq!(capture(&fresh).unwrap(), expected);
        assert_eq!(fresh.session.path, path);
        assert_eq!(fresh.session.is_dirty(), dirty);
    }
}
#[test]
fn repeated_original_pause_keeps_its_physical_ui_capture_without_replaying_held_display_changes() {
    let (_root, mut app) = setup();
    decorate(&mut app);
    let expected = capture(&app).unwrap();
    let (reply, params) = begin_current_ui(&mut app, 304);
    assert_eq!(reply["ok"], true, "{reply}");
    app.ui.project_search = "read-only held display changed".into();
    let repeated = original_pause_rpc(&mut app, params);
    assert_eq!(repeated["ok"], true, "{repeated}");
    assert_eq!(repeated["result"]["status"], "paused");
    assert_eq!(app.session.saved_application_ui().unwrap(), Some(expected));
    assert!(app.session.is_display_only());
    assert!(crate::application_tabs::capture_history(&app).is_err());
    assert_eq!(app.session.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
}

fn original_checkpoint_rpc(app: &mut FilmcraftApp, params: Value) -> Value {
    let ctx = egui::Context::default();
    let (request, reply) = crate::ControlRequest::new("engine.execute", json!({"command":"agent.saveHistoryCheckpoint","params":params}));
    match crate::native_writer::intercept(app, &ctx, &request) {
        Some(crate::control::Outcome::Pending) => {}
        Some(crate::control::Outcome::Done(value)) => return value,
        _ => panic!("original metadata checkpoint must use its installed same Session worker"),
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while crate::native_writer::inspect(app)["active"] == true {
        assert!(std::time::Instant::now() < until);
        crate::native_writer::poll(app, &ctx);
        std::thread::yield_now();
    }
    reply.recv_timeout(std::time::Duration::from_secs(1)).unwrap()
}
#[test]
fn source_pause_metadata_checkpoint_keeps_actual_frozen_ui_and_original_worker_without_recapturing_display() {
    let (root, mut app) = setup();
    save(&mut app, 1900);
    app.ui.project_search = "Current UI before actual pause".into();
    let expected = capture(&app).unwrap();
    let (pause, _) = begin_current_ui(&mut app, 1901);
    assert_eq!(pause["ok"], true, "{pause}");
    assert_eq!(app.session.saved_application_ui().unwrap(), Some(expected.clone()));
    let project = app.session.project.clone();
    let revision = app.session.revision;
    let saved_revision = app.session.saved_revision;
    let history = serde_json::to_value(&app.session.history.undo).unwrap();
    let file = std::fs::read(app.session.path.as_ref().unwrap()).unwrap();
    app.ui.project_search = "Later paused display must not replace frozen state".into();
    assert!(crate::application_tabs::capture_history(&app).is_err());
    assert_ne!(capture(&app).unwrap(), expected);
    let checkpoint = "44444444-4444-4444-8444-444444444444";
    let target = root.0.join(format!(".poema-history-{W}-{checkpoint}.json"));
    let bad = original_checkpoint_rpc(
        &mut app,
        json!({"token":token(1902),"scope":{"projectId":"p_foreign","workspaceId":W},"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}),
    );
    assert_eq!(bad["ok"], false);
    assert!(!target.exists());
    assert!(app.session.is_display_only());
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(1903),"scope":scope()})).unwrap();
    let reply = original_checkpoint_rpc(&mut app, json!({"token":token(1903),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["status"], "saved");
    let capsule: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(capsule["version"], 10);
    assert_eq!(capsule["application_ui"]["proof"], expected);
    assert_eq!(app.session.saved_application_ui().unwrap(), Some(expected));
    assert!(Arc::ptr_eq(&project, &app.session.project));
    assert_eq!(app.session.revision, revision);
    assert_eq!(app.session.saved_revision, saved_revision);
    assert_eq!(serde_json::to_value(&app.session.history.undo).unwrap(), history);
    assert_eq!(std::fs::read(app.session.path.as_ref().unwrap()).unwrap(), file);
    assert!(app.session.is_display_only());
    assert!(app.session.execute("file.newBin", json!({"name":"Must stay paused"})).is_err());
    assert_eq!(app.session.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
    assert!(!app.session.is_dirty());
}
#[test]
fn source_pause_metadata_checkpoint_without_a_retained_ui_refuses_without_promoting_readonly_capture() {
    let (root, mut app) = setup();
    crate::native_writer::install(&mut app);
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(1910),"scope":scope()})).unwrap();
    // Deliberate negative setup: an original native pause with no UI attachment.
    app.session.execute("agent.beginWorkspaceUpdate", json!({"token":token(1910),"scope":scope(),"workspace":W,"update":U})).unwrap();
    assert!(app.session.saved_application_ui().unwrap().is_none());
    assert!(capture(&app).is_ok());
    assert!(crate::application_tabs::capture_history(&app).is_err());
    let checkpoint = "55555555-5555-4555-8555-555555555555";
    let reply = original_checkpoint_rpc(&mut app, json!({"token":token(1911),"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}));
    assert_eq!(reply["ok"], false);
    assert!(reply["error"].as_str().unwrap().contains("no validated frozen application UI"));
    assert!(!root.0.join(format!(".poema-history-{W}-{checkpoint}.json")).exists());
    assert!(app.session.is_display_only());
    assert!(app.session.saved_application_ui().unwrap().is_none());
    assert_eq!(app.session.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
}

#[test]
fn finite_nonbinary_ui_clocks_and_views_roundtrip_exactly_through_original_checkpoint() {
    // These finite clock values changed by one ULP with the default JSON parser.
    // Keep the exact-byte guard: the parser must preserve human UI state instead.
    let (_root, mut app) = setup();
    decorate(&mut app);
    let clock = f64::from_bits(0x3feddb280fed1b1d);
    let view = f64::from_bits(0x3fed1d0b0129b56f);
    app.last_ui_time = clock;
    app.status_seen = ("finite clock precision".into(), view);
    app.toast = Some(("passive toast".into(), clock));
    app.ui.timeline.scroll = view;
    app.ui.timeline.target_scroll = view;
    app.ui.timeline.seen_playhead = Some((filmcraft_project::ItemId(1), clock));
    app.ui.timeline.fit_empty = Some(view);
    let expected = capture(&app).unwrap();
    let decoded: Snapshot = serde_json::from_str(expected["data"].as_str().unwrap()).unwrap();
    assert_eq!(decoded.captured_ui_time.to_bits(), clock.to_bits());
    assert_eq!(decoded.status_seen.1.to_bits(), view.to_bits());
    assert_eq!(decoded.ui.timeline.scroll.to_bits(), view.to_bits());
    assert_eq!(decoded.timeline_skipped.seen_playhead.unwrap().1.to_bits(), clock.to_bits());
    assert_eq!(decoded.timeline_skipped.fit_empty.unwrap().to_bits(), view.to_bits());
    save(&mut app, 1250);
    let source = std::mem::take(&mut app.session);
    let fresh = FilmcraftApp::new(source);
    assert!(fresh.application_restore_error.is_none(), "{:?}", fresh.application_restore_error);
    assert_eq!(capture(&fresh).unwrap(), expected);
    assert_eq!(fresh.ui.timeline.scroll.to_bits(), view.to_bits());
}

#[test]
fn native_writer_availability_reports_paused_original_session_without_releasing_or_reinstalling_it() {
    let (_root, mut app) = setup();
    if crate::native_writer::inspect(&app)["available"] != true {
        crate::native_writer::install(&mut app);
    }
    assert_eq!(crate::native_writer::inspect(&app)["available"], true);
    let (reply, _) = begin_current_ui(&mut app, 1400);
    assert_eq!(reply["ok"], true);
    let before = serde_json::to_value(&app.session.state).unwrap();
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let observed = crate::native_writer::inspect(&app);
    assert_eq!(observed["active"], false);
    assert_eq!(observed["available"], false);
    assert_eq!(observed["displayOnly"], true);
    assert_eq!(observed["workspaceUpdate"]["status"], "paused");
    assert_eq!(serde_json::to_value(&app.session.state).unwrap(), before);
    assert_eq!(app.session.revision, revision);
    assert!(Arc::ptr_eq(&app.session.project, &project));
    assert!(app.session.workspace_update_status().is_some());
    // A generic edit is unavailable, while the SAME original worker can still
    // make its explicitly permitted held metadata checkpoint.
    app.session.execute("agent.readHistoryCheckpointBasis", json!({"token":token(1401),"scope":scope()})).unwrap();
    let saved = original_checkpoint_rpc(
        &mut app,
        json!({"token":token(1401),"scope":scope(),"workspace":W,"checkpoint":"55555555-5555-4555-8555-555555555555","metadataBasis":true}),
    );
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["result"]["status"], "saved");
    assert_eq!(crate::native_writer::inspect(&app)["available"], false);
    assert!(app.session.is_display_only());
}
