//! Generated actual Application/Session preparation and legacy boundary checks.
use filmcraft_engine::{History, Session};
use filmcraft_ui_egui::{
    FilmcraftApp,
    control::{self, ControlRequest, Outcome},
    dock::{DockNode, PanelKind},
    menus,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
const W: &str = "6cd868fc-9e1c-485b-87aa-9c60e13b00bf";
const C: &str = "210a3798-5017-4d6e-a87f-410699dc19fe";
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"generated-application-tab-root-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn scope() -> Value {
    json!({"projectId":"p_application_tabs","workspaceId":W})
}
fn token(n: usize) -> String {
    format!("{n:064x}")
}
fn setup() -> (Root, FilmcraftApp) {
    let dir = std::env::temp_dir().join(format!("generated-application-tabs-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("generated-root.txt"), b"generated-application-tab-root-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let anchor = s.state.active_sequence.unwrap();
    s.execute("project.associateEditorialOwner", json!({"scope":{"projectId":"p_application_tabs","anchorSequence":anchor.0},"workspaceId":W})).unwrap();
    Arc::make_mut(&mut s.project).metadata.insert(
        "poema.projectWorkspace.v1".into(),
        serde_json::to_string(&json!({"kind":"poema-native-project-workspace","version":1,"scope":scope()})).unwrap(),
    );
    s.history = History { limit: 128, ..History::default() };
    for track in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
    }
    s.execute("file.save", json!({"path":dir.join("generated.fcproj")})).unwrap();
    s.agent_operation_control().enable_transport();
    s.events.clear();
    let mut app = FilmcraftApp::new(s);
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 0 };
    app.ui.floating.clear();
    app.ui.timeline.pps = 73.;
    app.ui.timeline.target_pps = 73.;
    (Root(dir), app)
}
fn args(app: &FilmcraftApp, n: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"projectPath":app.session.path})
}
fn call(app: &mut FilmcraftApp, ctx: &egui::Context, method: &str, p: Value) -> Result<Value, Value> {
    let (req, _) = ControlRequest::new(method, p);
    match control::handle(app, ctx, &req) {
        Outcome::Done(v) => {
            if v["ok"] == true {
                Ok(v["result"].clone())
            } else {
                Err(v)
            }
        }
        _ => panic!("unexpected deferred application read"),
    }
}
fn prepare(app: &mut FilmcraftApp, ctx: &egui::Context, n: usize) -> Value {
    let p = args(app, n);
    call(app, ctx, "ui.sequenceTabs.prepare", p).unwrap()
}
fn state(a: &FilmcraftApp) -> Value {
    json!({"project":a.session.project,"editor":a.session.state,"undo":a.session.history.undo,"redo":a.session.history.redo,"limit":a.session.history.limit,"merge":a.session.history.merge_key,"revision":a.session.revision,"saved":a.session.saved_revision,"path":a.session.path,"journal":a.session.journal,"prefs":a.session.prefs,"trim":format!("{:?}",a.session.trim_play),"events":a.session.events.len(),"dock":a.ui.dock,"floating":a.ui.floating,"timeline":a.ui.timeline,"maximized":a.ui.keys.maximized})
}
fn ui_proof(v: &Value) -> Value {
    assert_eq!(v["kind"], "native_application_tab_ui_proof");
    assert_eq!(v["version"], 1);
    let raw = v["data"].as_str().unwrap().as_bytes();
    assert_eq!(json!(raw.len()), v["bytes"]);
    assert_eq!(format!("{:x}", Sha256::digest(raw)), v["sha256"]);
    serde_json::from_slice(raw).unwrap()
}
#[test]
fn original_application_read_and_prepare_keep_native_ui_history_and_writer_capabilities_distinct() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let before = state(&a);
    let p = args(&a, 100);
    let read = call(&mut a, &ctx, "ui.sequenceTabs.read", p).unwrap();
    assert_eq!(read["readPurpose"], "application_tabs_only");
    assert_eq!(read["capabilities"]["applicationTabPreparation"], false);
    let prep = prepare(&mut a, &ctx, 101);
    let doc = &prep["document"];
    assert_eq!(doc["readPurpose"], "application_tab_preparation");
    assert_eq!(doc["native"]["version"], 1);
    assert_eq!(doc["native"]["capabilities"]["sequenceViewPublication"], false);
    assert_eq!(doc["capabilities"]["applicationTabActivation"], false);
    assert_eq!(doc["writeAuthority"], false);
    assert_eq!(doc["mediaEvidence"], false);
    assert_eq!(state(&a), before);
    let raw = serde_json::to_vec(doc).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(raw)), prep["reference"]["sha256"]);
    let ui = ui_proof(&doc["ui"]);
    assert_eq!(ui["paintedVisibilityVerified"], false);
    assert_eq!(ui["osKeyboardFocusVerified"], false);
    assert_eq!(doc["uiSha256"], doc["ui"]["sha256"]);
    let observed = call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).unwrap();
    assert_eq!(observed["status"], "current");
    assert_eq!(observed["original"], *doc);
    assert_eq!(observed["current"]["native"], doc["native"]);
    assert_eq!(observed["rebasePerformed"], false);
    assert_eq!(state(&a), before);
}
#[test]
fn application_observation_follows_all_original_human_trim_clocks_and_loop_without_settlement() {
    for (kind, moved) in [("roll", false), ("roll", true), ("ripple", false), ("ripple", true), ("trim", false), ("trim", true), ("loop", false)] {
        let (_root, mut a) = setup();
        let ctx = egui::Context::default();
        let clip = a.session.active_sequence().unwrap().video_tracks[0].items[0].id;
        a.session.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":if kind=="loop"{"roll"}else{kind}})).unwrap();
        assert_eq!(serde_json::to_value(a.session.state.edit_points[0].kind).unwrap(), json!(if kind == "loop" { "roll" } else { kind }));
        if kind == "loop" {
            a.session.execute("trim.playAround", json!({"clock":0})).unwrap();
        } else {
            a.session.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0})).unwrap();
            if moved {
                a.session.execute("trim.tick", json!({"clock":0.25})).unwrap();
            }
        }
        let before = state(&a);
        let prep = prepare(&mut a, &ctx, 200);
        assert_eq!(state(&a), before);
        assert!(a.session.trim_play.active());
        a.session.execute("trim.tick", json!({"clock":0.5})).unwrap();
        let after_tick = state(&a);
        let observed = call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).unwrap();
        assert_eq!(observed["status"], "current", "{kind} {moved}");
        assert_eq!(observed["original"], prep["document"]);
        assert_eq!(observed["current"]["native"]["trim"]["active"], true);
        assert_eq!(observed["writeAuthority"], false);
        assert_eq!(state(&a), after_tick);
        assert!(a.session.trim_play.active());
    }
}
#[test]
fn changed_original_dock_is_reported_without_rebasing_native_or_application_preparation() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let prep = prepare(&mut a, &ctx, 300);
    menus::invoke(&mut a, &ctx, "window.floatPanel", json!({"panel":"Timeline","rect":[45,65,420,320]})).unwrap();
    let before = state(&a);
    let observed = call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).unwrap();
    assert_eq!(observed["status"], "changed_since_preparation");
    assert_eq!(observed["uiUnchanged"], false);
    assert_eq!(observed["original"], prep["document"]);
    assert_eq!(observed["current"]["native"], prep["document"]["native"]);
    assert_ne!(observed["current"]["ui"], prep["document"]["ui"]);
    ui_proof(&observed["current"]["ui"]);
    assert_eq!(prepare(&mut a, &ctx, 300), prep);
    assert_eq!(state(&a), before);
}
#[test]
fn forged_foreign_reflection_and_wrong_native_purposes_cannot_import_application_scope() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let prep = prepare(&mut a, &ctx, 400);
    let before = state(&a);
    for field in ["token", "applicationInstance", "scope", "projectPath", "sha256"] {
        let mut forged = prep["reference"].clone();
        forged[field] = if field == "scope" { json!({"projectId":"p_other","workspaceId":W}) } else { json!("0".repeat(64)) };
        assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", forged).is_err());
        assert_eq!(state(&a), before);
    }
    let mut extra = prep["reference"].clone();
    extra["extra"] = json!(true);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", extra).is_err());
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", prep["document"].clone()).is_err());
    let p = args(&a, 401);
    let old = a.session.execute("agent.readSequenceViews", p).unwrap();
    let mut fake = prep["reference"].clone();
    fake["token"] = old["token"].clone();
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", fake).is_err());
    let (_other_root, mut other) = setup();
    assert!(call(&mut other, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).is_err());
    assert_eq!(state(&a), before);
}
#[test]
fn actual_independent_stop_expires_the_original_application_native_scope_without_an_action() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let prep = prepare(&mut a, &ctx, 500);
    let before = state(&a);
    let stopped = a.session.agent_operation_control().cancel(&json!({"token":prep["reference"]["token"]})).unwrap();
    assert_eq!(stopped["cancellationAccepted"], true);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).is_err());
    let again = args(&a, 500);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", again).is_err());
    assert_eq!(state(&a), before);
}
#[test]
fn later_human_native_change_remains_authoritative_and_same_token_never_rebases() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let prep = prepare(&mut a, &ctx, 600);
    a.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    let before = state(&a);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).is_err());
    let p = args(&a, 600);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", p).is_err());
    assert_eq!(state(&a), before);
}
#[test]
fn application_preparation_capacity_refuses_before_evicting_original_native_read_or_ui_state() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let first = prepare(&mut a, &ctx, 700);
    for n in 701..764 {
        prepare(&mut a, &ctx, n);
    }
    let before = state(&a);
    let p = args(&a, 764);
    let result = call(&mut a, &ctx, "ui.sequenceTabs.prepare", p);
    assert!(result.is_err());
    assert!(result.unwrap_err()["error"].as_str().unwrap().contains("full"));
    let observed = call(&mut a, &ctx, "ui.sequenceTabs.observe", first["reference"].clone()).unwrap();
    assert_eq!(observed["status"], "current");
    assert_eq!(state(&a), before);
}
#[test]
fn invalid_original_ui_geometry_and_nonfinite_view_refuse_before_native_ticket_creation() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    a.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Timeline], active: 0 };
    let p = args(&a, 800);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", p.clone()).is_err());
    a.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline], active: 0 };
    a.ui.timeline.pps = f64::NAN;
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", p.clone()).is_err());
    a.ui.timeline.pps = 73.;
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.prepare", p).is_ok());
}
#[test]
fn private_native_scope_never_promotes_old_readonly_writer_and_rejects_other_physical_session() {
    let (_root, mut a) = setup();
    let p = args(&a, 900);
    let native = a.session.prepare_application_tab_scope(&p).unwrap();
    let doc = native.document().clone();
    let before = state(&a);
    let item = a.session.state.active_sequence.unwrap();
    let wrong = json!({"version":1,"token":doc["token"],"scope":scope(),"beforeSha256":doc["projectState"]["sha256"],"beforeEditorSha256":doc["editorStateSha256"],"operation":{"type":"open","item":item.0,"owned":true}});
    assert!(a.session.execute("agent.navigateSequenceView", wrong).is_err());
    assert_eq!(state(&a), before);
    let (_other_root, mut other) = setup();
    assert!(native.observe(&mut other.session).is_err());
}
#[test]
fn native8_view_history_roundtrip_keeps_legacy_receipt_but_never_restores_private_application_scope() {
    let (root, mut a) = setup();
    let p = args(&a, 1000);
    let d = a.session.execute("agent.prepareSequenceViews", p).unwrap();
    let item = a.session.state.active_sequence.unwrap();
    let receipt=a.session.execute("agent.navigateSequenceView",json!({"version":1,"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"operation":{"type":"open","item":item.0,"owned":true}})).unwrap();
    assert_eq!(receipt["version"], 2);
    let path = a.session.path.clone().unwrap();
    a.session.execute("file.save", json!({"path":path})).unwrap();
    let p = args(&a, 1001);
    let native = a.session.prepare_application_tab_scope(&p).unwrap();
    let p = args(&a, 1002);
    a.session.execute("agent.readProject", p).unwrap();
    let saved = a.session.execute("agent.saveHistoryCheckpoint", json!({"token":token(1002),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap();
    let file = root.0.join(format!(".poema-history-{W}-{C}.json"));
    let raw = std::fs::read(file).unwrap();
    let cap: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(cap["version"], 8);
    assert!(!String::from_utf8(raw).unwrap().contains("application_scope"));
    assert!(!cap["tickets"].as_array().unwrap().iter().any(|t| t["token"] == token(1001)));
    let mut cold = Session::default();
    cold.execute("file.open", json!({"path":path})).unwrap();
    cold.agent_operation_control().enable_transport();
    cold.execute("agent.readProject", json!({"token":token(1003),"scope":scope(),"projectPath":path})).unwrap();
    cold.execute(
        "agent.restoreHistoryCheckpoint",
        json!({"token":token(1003),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}),
    )
    .unwrap();
    assert!(native.observe(&mut cold).is_err());
    let historical = cold.execute("agent.receipt", json!({"token":d["token"]})).unwrap();
    assert_eq!(historical["status"], "committed");
    assert_eq!(historical["result"], receipt);
}
