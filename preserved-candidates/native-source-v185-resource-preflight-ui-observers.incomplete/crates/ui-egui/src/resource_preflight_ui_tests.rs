use super::*;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
const OWNER: &str = "agent.resourcePreflightOwner";
const BEGIN: &str = "agent.beginResourcePreflight";
const STATUS: &str = "agent.resourcePreflightStatus";
const RELEASE: &str = "agent.releaseResourcePreflight";
fn step(h: &mut Harness<'static, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    ctx.request_repaint();
    h.step();
}
fn restore(h: &mut Harness<'static, FilmcraftApp>) {
    h.state_mut().native_writer.hold_completion_delivery = false;
    let until = Instant::now() + Duration::from_secs(10);
    while h.state().native_writer.pending.is_some() {
        assert!(Instant::now() < until);
        step(h);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn world() -> (Harness<'static, FilmcraftApp>, Sender<ControlRequest>, PathBuf) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!("poema-resource-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Generated recovery gate","fps":24})).unwrap();
    s.start_autosave(filmcraft_engine::autosave::AutosaveConfig::new(root.join("native-data"))).unwrap();
    s.set_prefs(s.prefs.clone()).unwrap();
    s.agent_operation_control().enable_transport();
    let (tx, rx) = mpsc::channel();
    let app = FilmcraftApp::new(s).with_control(rx);
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 650.0)).with_max_steps(10000).build_eframe(move |_| app);
    for _ in 0..4 {
        step(&mut h);
    }
    assert!(h.state().native_writer.worker.is_some());
    (h, tx, root)
}
fn rpc(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, method: &str, p: Value) -> Value {
    h.state_mut().native_writer.hold_completion_delivery = true;
    let (request, reply) = ControlRequest::new(method, p);
    tx.send(request).unwrap();
    step(h);
    restore(h);
    reply.recv_timeout(Duration::from_secs(2)).unwrap()
}
fn execute(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, command: &str, p: Value) -> Value {
    let v = rpc(h, tx, "engine.execute", json!({"command":command,"params":p}));
    assert_eq!(v["ok"], true, "{v}");
    v["result"].clone()
}
fn address(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, letter: char) -> Value {
    let owner = execute(h, tx, OWNER, json!({}));
    json!({"request":letter.to_string().repeat(64),"nativePid":owner["nativePid"],"nativeSession":owner["nativeSession"],"dataDirectory":owner["dataDirectory"]})
}
fn tree(root: &Path) -> Vec<(PathBuf, String, Vec<u8>)> {
    fn walk(root: &Path, p: &Path, out: &mut Vec<(PathBuf, String, Vec<u8>)>) {
        let m = fs::symlink_metadata(p).unwrap();
        assert!(!m.is_symlink());
        let generation = format!(
            "{}:{}:{}:{}:{}:{}:{}.{:09}:{}.{:09}",
            m.dev(),
            m.ino(),
            m.mode(),
            m.uid(),
            m.nlink(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec()
        );
        assert!(m.len() < 2 * 1024 * 1024);
        out.push((p.strip_prefix(root).unwrap().into(), generation, if m.is_file() { fs::read(p).unwrap() } else { Vec::new() }));
        assert!(out.len() < 128);
        if m.is_dir() {
            let mut children = fs::read_dir(p).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
            children.sort();
            for child in children {
                walk(root, &child, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}
#[test]
fn resource_hold_renders_only_its_original_native_gate_and_refuses_human_control_and_direct_menu_mutations() {
    let (mut h, tx, root) = world();
    let r = address(&mut h, &tx, 'a');
    let before = h.state().session.project.clone();
    let revision = h.state().session.revision;
    let history = h.state().session.history.undo.len();
    assert_eq!(execute(&mut h, &tx, BEGIN, r.clone())["namespaceHeld"], true);
    let bytes = tree(&root.join("native-data"));
    for (method, p) in [
        ("ui.key", json!({"key":"Z","command":true})),
        ("ui.click", json!({"x":100,"y":100})),
        ("ui.menu.invoke", json!({"id":"app.settings.general"})),
        ("engine.execute", json!({"command":"prefs.set","params":{"values":{"trim.largeTrimOffset":17}}})),
        ("ui.mediaIntake.begin", json!({})),
        ("ui.mediaIntake.stop", json!({})),
        ("ui.mediaIntake.release", json!({})),
        ("ui.poemaComposer.update", json!({})),
    ] {
        let reply = rpc(&mut h, &tx, method, p);
        assert_eq!(reply["ok"], false, "{reply}");
    }
    let ctx = h.ctx.clone();
    for id in ["app.settings.general", "file.newProject", "edit.undo", "presets.new"] {
        assert!(crate::menus::invoke(h.state_mut(), &ctx, id, json!({})).is_err());
    }
    for _ in 0..4 {
        step(&mut h);
    }
    assert!(h.state().auto.elements.iter().any(|e| e.id == "native-resource-preflight"));
    assert!(!h.state().auto.elements.iter().any(|e| e.id.starts_with("timeline.")));
    assert!(h.state().synthetic.is_empty());
    assert!(Arc::ptr_eq(&before, &h.state().session.project));
    assert_eq!(h.state().session.revision, revision);
    assert_eq!(h.state().session.history.undo.len(), history);
    assert_eq!(tree(&root.join("native-data")), bytes);
    assert_eq!(execute(&mut h, &tx, RELEASE, r)["status"], "released");
    step(&mut h);
    assert!(!h.state().session.resource_preflight_pending());
    assert!(!h.state().auto.elements.iter().any(|e| e.id == "native-resource-preflight"));
}
#[test]
fn original_raw_input_hook_discards_held_text_keys_drops_and_synthetic_input_without_reading_media() {
    let (mut h, tx, root) = world();
    let r = address(&mut h, &tx, 'b');
    execute(&mut h, &tx, BEGIN, r.clone());
    let bytes = tree(&root.join("native-data"));
    #[derive(Debug)]
    struct HeldDrop(PathBuf);
    impl egui::DroppedFile for HeldDrop {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            panic!("held media must never be read")
        }
    }
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Text("Must not become a later edit".into()));
    raw.dropped_files.push(Arc::new(HeldDrop(root.join("not-imported.mxf"))));
    h.state_mut().synthetic.push(egui::Event::Text("Must not queue".into()));
    let ctx = h.ctx.clone();
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    assert!(raw.events.iter().all(|e| matches!(e,egui::Event::ModifiersChanged(m)if *m==egui::Modifiers::NONE)));
    assert!(raw.dropped_files.is_empty());
    assert!(raw.hovered_files.is_empty());
    assert!(h.state().synthetic.is_empty());
    assert_eq!(tree(&root.join("native-data")), bytes);
    execute(&mut h, &tx, RELEASE, r);
    assert!(h.state().synthetic.is_empty());
}
#[test]
fn held_close_never_quits_cleans_native_files_or_releases_the_original_request() {
    let (mut h, tx, root) = world();
    let r = address(&mut h, &tx, 'c');
    execute(&mut h, &tx, BEGIN, r.clone());
    let bytes = tree(&root.join("native-data"));
    assert!(request_close(h.state_mut()));
    assert!(!h.state().quit_confirmed);
    let reply = rpc(&mut h, &tx, "app.quit", json!({}));
    assert_eq!(reply["result"]["closePending"], true);
    assert_eq!(reply["result"]["closed"], false);
    assert!(!h.state().quit_confirmed);
    assert_eq!(execute(&mut h, &tx, STATUS, r.clone())["namespaceHeld"], true);
    assert_eq!(tree(&root.join("native-data")), bytes);
    execute(&mut h, &tx, RELEASE, r);
    assert!(!h.state().native_writer.close_requested);
}
#[test]
fn lost_begin_and_release_reply_delivery_keeps_the_same_returned_session_and_explicit_status_reconciliation() {
    let (mut h, tx, _) = world();
    let r = address(&mut h, &tx, 'd');
    let before = h.state().session.project.clone();
    h.state_mut().native_writer.hold_completion_delivery = true;
    let (request, lost) = ControlRequest::new("engine.execute", json!({"command":BEGIN,"params":r}));
    drop(lost);
    tx.send(request).unwrap();
    step(&mut h);
    assert!(h.state().native_writer.pending.is_some());
    restore(&mut h);
    assert!(Arc::ptr_eq(&before, &h.state().session.project));
    assert_eq!(execute(&mut h, &tx, STATUS, r.clone())["status"], "held");
    assert_eq!(execute(&mut h, &tx, BEGIN, r.clone())["status"], "held");
    h.state_mut().native_writer.hold_completion_delivery = true;
    let (request, lost) = ControlRequest::new("engine.execute", json!({"command":RELEASE,"params":r}));
    drop(lost);
    tx.send(request).unwrap();
    step(&mut h);
    restore(&mut h);
    assert_eq!(execute(&mut h, &tx, STATUS, r)["status"], "released");
    assert!(!h.state().session.resource_preflight_pending());
    assert!(Arc::ptr_eq(&before, &h.state().session.project));
}
#[test]
fn original_independent_stop_revokes_readiness_while_desktop_gate_remains_until_exact_cleanup() {
    let (mut h, tx, root) = world();
    let r = address(&mut h, &tx, 'e');
    execute(&mut h, &tx, BEGIN, r.clone());
    let bytes = tree(&root.join("native-data"));
    assert_eq!(h.state().session.agent_operation_control().cancel(&json!({"token":r["request"]})).unwrap()["cancellationAccepted"], true);
    let v = execute(&mut h, &tx, STATUS, r.clone());
    assert_eq!(v["status"], "stale_held");
    assert_eq!(v["namespaceHeld"], false);
    assert!(h.state().session.resource_preflight_pending());
    assert_eq!(tree(&root.join("native-data")), bytes);
    execute(&mut h, &tx, RELEASE, r);
    assert!(!h.state().session.resource_preflight_pending());
}
#[test]
fn unavailable_original_worker_never_falls_back_to_synchronous_acquisition_or_release() {
    let (mut h, tx, _) = world();
    let r = address(&mut h, &tx, 'f');
    let original = h.state_mut().native_writer.worker.take();
    assert_eq!(rpc(&mut h, &tx, "engine.execute", json!({"command":BEGIN,"params":r}))["ok"], false);
    assert!(!h.state().session.resource_preflight_pending());
    h.state_mut().native_writer.worker = original;
    execute(&mut h, &tx, BEGIN, r.clone());
    let original = h.state_mut().native_writer.worker.take();
    assert_eq!(rpc(&mut h, &tx, "engine.execute", json!({"command":RELEASE,"params":r}))["ok"], false);
    assert_eq!(execute(&mut h, &tx, STATUS, r.clone())["namespaceHeld"], true);
    h.state_mut().native_writer.worker = original;
    execute(&mut h, &tx, RELEASE, r);
}
#[test]
fn preexisting_native_input_refuses_preparation_and_os_menu_actions_received_during_hold_are_not_replayed() {
    let (mut h, tx, _) = world();
    let r = address(&mut h, &tx, 'g');
    let (request, _) = ControlRequest::new("engine.execute", json!({"command":BEGIN,"params":r}));
    h.state_mut().synthetic.push(egui::Event::Text("kept pending native input".into()));
    let ctx = h.ctx.clone();
    assert!(matches!(intercept(h.state_mut(),&ctx,&request),Some(Outcome::Done(v))if v["ok"]==false));
    assert!(!h.state().session.resource_preflight_pending());
    assert_eq!(h.state().synthetic.len(), 1);
    h.state_mut().synthetic.clear();
    execute(&mut h, &tx, BEGIN, r.clone());
    let before = h.state().session.project.clone();
    let (menu, rx) = mpsc::channel();
    h.state_mut().command_inbox = Some(rx);
    menu.send("edit.undo".into()).unwrap();
    step(&mut h);
    execute(&mut h, &tx, RELEASE, r);
    step(&mut h);
    assert!(Arc::ptr_eq(&before, &h.state().session.project));
    assert!(h.state().synthetic.is_empty());
}

#[test]
fn same_session_menu_read_observers_remain_available_without_synchronous_acquire_or_release_fallback() {
    let (mut h, tx, _) = world();
    let r = address(&mut h, &tx, 'h');
    execute(&mut h, &tx, BEGIN, r.clone());
    let ctx = h.ctx.clone();
    assert_eq!(crate::menus::invoke(h.state_mut(), &ctx, STATUS, r.clone()).unwrap()["namespaceHeld"], true);
    assert_eq!(crate::menus::invoke(h.state_mut(), &ctx, OWNER, json!({})).unwrap()["available"], false);
    for command in [BEGIN, RELEASE] {
        assert!(crate::menus::invoke(h.state_mut(), &ctx, command, r.clone()).is_err());
    }
    execute(&mut h, &tx, RELEASE, r);
}
#[test]
fn release_completion_never_replays_raw_input_received_while_original_session_is_still_on_worker() {
    let (mut h, tx, _) = world();
    let r = address(&mut h, &tx, 'i');
    execute(&mut h, &tx, BEGIN, r.clone());
    let before = h.state().session.project.clone();
    let revision = h.state().session.revision;
    h.state_mut().native_writer.hold_completion_delivery = true;
    let (request, reply) = ControlRequest::new("engine.execute", json!({"command":RELEASE,"params":r}));
    tx.send(request).unwrap();
    step(&mut h);
    assert!(h.state().native_writer.pending.is_some());
    assert!(resource_preflight_in_progress(h.state()));
    assert!(h.state().session.is_display_only());
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Text("Must not land after release delivery".into()));
    h.state_mut().synthetic.push(egui::Event::Text("No deferred edit".into()));
    let ctx = h.ctx.clone();
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    assert!(raw.events.iter().all(|e| matches!(e,egui::Event::ModifiersChanged(m)if *m==egui::Modifiers::NONE)));
    assert!(h.state().synthetic.is_empty());
    assert!(h.state().auto.elements.iter().any(|e| e.id == "native-resource-preflight"));
    assert!(!h.state().auto.elements.iter().any(|e| e.id == CANCEL));
    restore(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "released");
    assert!(!resource_preflight_in_progress(h.state()));
    assert!(Arc::ptr_eq(&before, &h.state().session.project));
    assert_eq!(h.state().session.revision, revision);
    step(&mut h);
    assert!(h.state().synthetic.is_empty());
}
