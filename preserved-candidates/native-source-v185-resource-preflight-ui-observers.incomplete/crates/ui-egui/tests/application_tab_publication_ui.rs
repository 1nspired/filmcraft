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
    let dir = std::env::temp_dir().join(format!("generated-application-publication-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

fn activate(a: &mut FilmcraftApp, ctx: &egui::Context, prep: &Value, item: filmcraft_project::ItemId) -> Value {
    call(a, ctx, "ui.sequenceTabs.activate", json!({"reference":prep["reference"],"item":item.0})).unwrap()
}
fn other_tab(a: &mut FilmcraftApp) -> (filmcraft_project::ItemId, filmcraft_project::ItemId) {
    let main = a.session.state.active_sequence.unwrap();
    a.session.execute("file.newSequence", json!({"name":"Generated other tab","video":0,"audio":0})).unwrap();
    let other = a.session.state.active_sequence.unwrap();
    a.session.execute("sequence.open", json!({"item":main.0})).unwrap();
    (main, other)
}
fn start_trim(a: &mut FilmcraftApp, kind: &str, moved: bool) {
    let clip = a.session.active_sequence().unwrap().video_tracks[0].items[0].id;
    a.session.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":if kind=="loop"{"roll"}else{kind}})).unwrap();
    if kind == "loop" {
        a.session.execute("trim.playAround", json!({"clock":0})).unwrap();
    } else {
        a.session.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0})).unwrap();
        if moved {
            a.session.execute("trim.tick", json!({"clock":0.25})).unwrap();
        }
    }
}
fn confirmed(r: &Value) {
    assert_eq!(r["status"], "activated", "{r}");
    assert_eq!(r["receiptComplete"], true);
    assert_eq!(r["durable"], false);
    assert_eq!(r["readbackRequired"], true);
    assert_eq!(r["canReplay"], false);
    assert_eq!(r["agentUndoOwned"], false);
    assert_eq!(r["agentTimelineCommits"], 0);
    assert_eq!(r["native"]["status"], "activated");
    let before = ui_proof(&r["beforeUi"]);
    let after = ui_proof(&r["afterUi"]);
    assert_eq!(before["timeline"], after["timeline"]);
    assert_eq!(after["paintedVisibilityVerified"], false);
    assert_eq!(after["osKeyboardFocusVerified"], false);
    assert!(serde_json::to_vec(r).unwrap().len() <= 12 * 1024 * 1024);
}
#[test]
fn same_active_publication_preserves_native_selection_history_and_requires_later_readback() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let main = a.session.state.active_sequence.unwrap();
    let clip = a.session.active_sequence().unwrap().video_tracks[0].items[0].id;
    a.session.state.selection = vec![clip];
    a.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1 };
    let prep = prepare(&mut a, &ctx, 1100);
    let before = state(&a);
    let project = a.session.project.clone();
    let journal = a.session.journal.len();
    let r = activate(&mut a, &ctx, &prep, main);
    confirmed(&r);
    assert_eq!(r["native"]["switched"], false);
    assert_eq!(r["native"]["humanCommand"], Value::Null);
    assert_eq!(r["native"]["before"], r["native"]["after"]);
    assert!(a.ui.dock.is_visible(PanelKind::Timeline));
    assert_eq!(a.session.state.selection, vec![clip]);
    let mut after = state(&a);
    after["dock"] = before["dock"].clone();
    assert_eq!(after, before);
    assert!(Arc::ptr_eq(&project, &a.session.project));
    assert_eq!(a.session.journal.len(), journal);
    assert_eq!(call(&mut a, &ctx, "ui.sequenceTabs.outcome", prep["reference"].clone()).unwrap(), r);
    assert_eq!(prep["document"]["capabilities"]["applicationTabActivation"], false);
}
#[test]
fn active_publication_follows_all_original_live_trim_clocks_without_settlement() {
    for (kind, moved) in [("roll", false), ("roll", true), ("ripple", false), ("ripple", true), ("trim", false), ("trim", true), ("loop", false)] {
        let (_root, mut a) = setup();
        let ctx = egui::Context::default();
        start_trim(&mut a, kind, moved);
        let prep = prepare(&mut a, &ctx, 1200);
        a.session.execute("trim.tick", json!({"clock":0.5})).unwrap();
        let before = state(&a);
        let main = a.session.state.active_sequence.unwrap();
        let r = activate(&mut a, &ctx, &prep, main);
        confirmed(&r);
        assert_eq!(state(&a), before, "{kind} {moved}");
        assert!(a.session.trim_play.active());
        assert_eq!(r["native"]["before"], r["native"]["after"]);
        assert_eq!(r["native"]["humanCommand"], Value::Null);
    }
}
#[test]
fn other_tab_publication_uses_original_open_once_and_actual_trim_history_for_all_modes() {
    for (kind, moved) in [("roll", false), ("roll", true), ("ripple", false), ("ripple", true), ("trim", false), ("trim", true), ("loop", false)] {
        for limit in [0, 128] {
            let (_root, mut a) = setup();
            let ctx = egui::Context::default();
            let (_, other) = other_tab(&mut a);
            a.session.history.limit = limit;
            start_trim(&mut a, kind, moved);
            let prep = prepare(&mut a, &ctx, 1300);
            a.session.execute("trim.tick", json!({"clock":0.5})).unwrap();
            let journal = a.session.journal.len();
            let r = activate(&mut a, &ctx, &prep, other);
            confirmed(&r);
            assert_eq!(r["native"]["switched"], true, "{kind} {moved} {limit}");
            assert_eq!(r["native"]["humanCommand"], "sequence.open");
            assert_eq!(a.session.state.active_sequence, Some(other));
            assert!(!a.session.trim_play.active());
            assert_eq!(a.session.journal.len(), journal + 1);
            assert_eq!(a.session.journal.last().unwrap().0, "sequence.open");
            let p = args(&a, 1301);
            let current = a.session.execute("agent.readSequenceViews", p).unwrap();
            assert_eq!(current["projectState"]["sha256"], r["native"]["after"]["projectSha256"]);
            assert_eq!(current["editorStateSha256"], r["native"]["after"]["editorSha256"]);
            assert_eq!(a.session.history.limit, limit);
        }
    }
}
#[test]
fn actual_stop_before_publication_is_known_rejected_and_retained_without_effects() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let (_, other) = other_tab(&mut a);
    let prep = prepare(&mut a, &ctx, 1400);
    let before = state(&a);
    let stopped = a.session.agent_operation_control().cancel(&json!({"token":prep["reference"]["token"]})).unwrap();
    assert_eq!(stopped["cancellationAccepted"], true);
    let r = activate(&mut a, &ctx, &prep, other);
    assert_eq!(r["status"], "rejected");
    assert_eq!(r["publicationStarted"], false);
    assert_eq!(r["receiptComplete"], true);
    assert_eq!(state(&a), before);
    assert_eq!(activate(&mut a, &ctx, &prep, other), r);
    assert_eq!(state(&a), before);
    assert_eq!(call(&mut a, &ctx, "ui.sequenceTabs.outcome", prep["reference"].clone()).unwrap(), r);
}
#[test]
fn late_stop_after_original_native_fence_cannot_certify_rollback_or_cancel_actual_human_effect() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let (_, other) = other_tab(&mut a);
    let p = args(&a, 1500);
    let scope = a.session.prepare_application_tab_scope(&p).unwrap();
    let prepared = scope.prepare_activation(&mut a.session, other.0).unwrap();
    let expected = prepared.document().clone();
    let control = a.session.agent_operation_control();
    let publication = prepared.enter_publication(&mut a.session).unwrap();
    let stop = control.cancel(&json!({"token":token(1500)})).unwrap();
    assert_eq!(stop["cancellationAccepted"], false);
    assert_eq!(stop["status"], "publication_started");
    menus::invoke(&mut a, &ctx, "window.activateSequenceTab", json!({"item":other.0})).unwrap();
    let receipt = publication.finish(&a.session);
    assert!(receipt.confirmed(), "{}", receipt.document());
    assert_eq!(receipt.document(), &expected);
    assert!(receipt.retained_bytes().unwrap() > 0);
    assert_eq!(control.progress(&token(1500))["phase"], "committed");
    assert_eq!(a.session.state.active_sequence, Some(other));
}
#[test]
fn lost_reply_lookup_and_exact_retry_preserve_later_human_edit_and_view_without_replay() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let main = a.session.state.active_sequence.unwrap();
    let prep = prepare(&mut a, &ctx, 1600);
    let receipt = activate(&mut a, &ctx, &prep, main);
    confirmed(&receipt);
    a.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    a.ui.timeline.scroll = 8.;
    a.ui.timeline.target_scroll = 8.;
    let before = state(&a);
    assert_eq!(call(&mut a, &ctx, "ui.sequenceTabs.outcome", prep["reference"].clone()).unwrap(), receipt);
    assert_eq!(activate(&mut a, &ctx, &prep, main), receipt);
    assert_eq!(state(&a), before);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.activate", json!({"reference":prep["reference"],"item":main.0+1})).is_err());
    assert_eq!(state(&a), before);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.observe", prep["reference"].clone()).is_err());
}
#[test]
fn unrelated_ui_scroll_is_preserved_while_affected_dock_and_human_native_changes_refuse() {
    for mode in ["scroll", "dock", "native"] {
        let (_root, mut a) = setup();
        let ctx = egui::Context::default();
        let main = a.session.state.active_sequence.unwrap();
        let prep = prepare(&mut a, &ctx, 1700);
        match mode {
            "scroll" => {
                a.ui.timeline.scroll = 9.;
                a.ui.timeline.target_scroll = 9.;
            }
            "dock" => {
                menus::invoke(&mut a, &ctx, "window.floatPanel", json!({"panel":"Timeline","rect":[45,65,420,320]})).unwrap();
            }
            _ => {
                a.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
            }
        }
        let before = state(&a);
        let r = call(&mut a, &ctx, "ui.sequenceTabs.activate", json!({"reference":prep["reference"],"item":main.0}));
        if mode == "scroll" {
            let r = r.unwrap();
            confirmed(&r);
            assert_eq!(r["uiChangedSincePreparation"], true);
            assert_eq!(ui_proof(&r["beforeUi"])["timeline"], before["timeline"]);
        } else if mode == "dock" {
            assert!(r.is_err());
        } else {
            assert_eq!(r.unwrap()["status"], "rejected");
        }
        assert_eq!(state(&a), before);
    }
}
#[test]
fn wrong_actual_effect_retains_explicit_uncertainty_and_never_restores_or_replays() {
    let (_root, mut a) = setup();
    let p = args(&a, 1800);
    let scope = a.session.prepare_application_tab_scope(&p).unwrap();
    let main = a.session.state.active_sequence.unwrap();
    let prepared = scope.prepare_activation(&mut a.session, main.0).unwrap();
    let publication = prepared.enter_publication(&mut a.session).unwrap();
    a.session.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    let before = state(&a);
    let receipt = publication.finish(&a.session);
    assert!(!receipt.confirmed());
    assert_eq!(receipt.document()["status"], "unknown");
    assert_eq!(receipt.document()["canReplay"], false);
    assert_eq!(receipt.document()["actualSnapshotRetained"], true);
    assert!(receipt.retained_bytes().unwrap() > 0);
    assert_eq!(state(&a), before);
    assert_eq!(a.session.agent_operation_control().cancel(&json!({"token":token(1800)})).unwrap()["cancellationAccepted"], false);
}
#[test]
fn hostile_foreign_closed_and_unobserved_targets_do_not_publish_ui_or_native_effects() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let (main, other) = other_tab(&mut a);
    let prep = prepare(&mut a, &ctx, 1900);
    let before = state(&a);
    for bad in [
        json!(null),
        json!({}),
        json!({"reference":prep["reference"],"item":0}),
        json!({"reference":prep["reference"],"item":-1}),
        json!({"reference":prep["reference"],"item":1.5}),
        json!({"reference":prep["reference"],"item":"1"}),
        json!({"reference":prep["reference"],"item":u64::MAX}),
        json!({"reference":prep["reference"],"item":main.0,"extra":true}),
    ] {
        assert!(call(&mut a, &ctx, "ui.sequenceTabs.activate", bad).is_err());
        assert_eq!(state(&a), before);
    }
    let (_root2, mut foreign) = setup();
    assert!(call(&mut foreign, &ctx, "ui.sequenceTabs.activate", json!({"reference":prep["reference"],"item":main.0})).is_err());
    a.session.execute("sequence.close", json!({"item":other.0})).unwrap();
    let before = state(&a);
    let r = activate(&mut a, &ctx, &prep, other);
    assert_eq!(r["status"], "rejected");
    assert_eq!(r["publicationStarted"], false);
    assert_eq!(state(&a), before);
}
#[test]
fn retained_outcome_lookup_works_during_real_native_worker_and_action_is_not_queued() {
    let (_root, mut a) = setup();
    let ctx = egui::Context::default();
    let main = a.session.state.active_sequence.unwrap();
    let prep = prepare(&mut a, &ctx, 2000);
    let receipt = activate(&mut a, &ctx, &prep, main);
    confirmed(&receipt);
    let (_tx, rx) = std::sync::mpsc::channel();
    a = a.with_control(rx);
    let p = args(&a, 2001);
    a.session.execute("agent.readProject", p).unwrap();
    let (request, reply) = ControlRequest::new(
        "engine.execute",
        json!({"command":"agent.saveHistoryCheckpoint","params":{"token":token(2001),"scope":scope(),"workspace":W,"checkpoint":C}}),
    );
    assert!(matches!(control::handle(&mut a, &ctx, &request), Outcome::Pending));
    assert!(a.session.is_display_only());
    assert_eq!(call(&mut a, &ctx, "ui.sequenceTabs.outcome", prep["reference"].clone()).unwrap(), receipt);
    assert!(call(&mut a, &ctx, "ui.sequenceTabs.activate", json!({"reference":prep["reference"],"item":main.0})).is_err());
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000., 700.)).build_eframe(move |_| a);
    let start = std::time::Instant::now();
    let result = loop {
        let ctx = h.ctx.clone();
        let mut input = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
        *h.input_mut() = input;
        h.step();
        match reply.try_recv() {
            Ok(v) => break v,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(start.elapsed() < std::time::Duration::from_secs(20));
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => panic!("original worker reply disappeared: {e}"),
        }
    };
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["result"]["status"], "saved");
    assert!(!h.state().session.is_display_only());
    let ctx = h.ctx.clone();
    assert_eq!(call(h.state_mut(), &ctx, "ui.sequenceTabs.outcome", prep["reference"].clone()).unwrap(), receipt);
}
