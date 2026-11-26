use super::*;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn step(h: &mut Harness<'static, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
    *h.input_mut() = input;
    ctx.request_repaint();
    h.step();
}
fn harness() -> (Harness<'static, FilmcraftApp>, Sender<ControlRequest>) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Native writer UI fixture","fps":24})).unwrap();
    let item = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video() && m.info.has_audio())).unwrap().id;
    s.execute("agent.bindSource", json!({"item":item.0,"binding":{"projectId":"p_writer_ui","assetId":"a_ui","mediaId":"original","role":"original"}}))
        .unwrap();
    s.execute("timeline.place", json!({"item":item.0,"track":"V1","frame":0,"duration":s.sequence_rate().tick_of(12).0,"video":true,"audio":false})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.agent_operation_control().enable_transport();
    let (tx, rx) = mpsc::channel();
    let mut app = FilmcraftApp::new(s).with_control(rx);
    // Hold only completion delivery so the actual busy renderer is deterministic.
    // Native execution and file/history guards remain real, on the production worker.
    app.native_writer.hold_completion_delivery = true;
    let mut builder = Harness::builder().with_size(egui::vec2(1000.0, 650.0)).with_max_steps(10_000);
    if std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_| app);
    for _ in 0..4 {
        step(&mut h);
    }
    (h, tx)
}
fn batch(h: &mut Harness<'static, FilmcraftApp>) -> Value {
    let s = &mut h.state_mut().session;
    let token = "native-writer-ui-read-ticket-00000001";
    s.execute("agent.snapshot", json!({"token":token})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    json!({"command":"agent.apply","params":{"token":token,"label":"UI worker trim","exact":true,"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","mode":"regular","deltaFrames":-1}}]}})
}
fn restore(h: &mut Harness<'static, FilmcraftApp>) {
    h.state_mut().native_writer.hold_completion_delivery = false;
    let end = Instant::now() + Duration::from_secs(10);
    while h.state().native_writer.pending.is_some() {
        assert!(Instant::now() < end);
        step(h);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn workspace_update_pause_has_cancel_only_frames_and_rejects_queued_edit_input_without_changing_the_dirty_cut() {
    let (mut h, tx) = harness();
    let s = &mut h.state_mut().session;
    s.path = Some("/fixture/native-ui-update.fcproj".into());
    let before = s.project.clone();
    let revision = s.revision;
    let dirty = s.is_dirty();
    let history = s.history.undo.len();
    let scope = json!({"projectId":"p_writer_ui","anchorSequence":s.state.active_sequence.unwrap().0});
    let token = "workspace-ui-update-private-token-00000001";
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
    let params = json!({"command":"agent.beginWorkspaceUpdate","params":{"token":token,"scope":scope,
        "workspace":"11111111-1111-4111-8111-111111111111","update":"22222222-2222-4222-8222-222222222222"}});
    let (request, reply) = ControlRequest::new("engine.execute", params);
    tx.send(request).unwrap();
    step(&mut h);
    restore(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "paused");
    assert!(h.state().session.is_display_only());
    assert!(h.state().auto.elements.iter().any(|e| e.id == CANCEL_UPDATE));
    assert!(!h.state().auto.elements.iter().any(|e| e.id.starts_with("timeline.clip.")));
    for (method, params) in
        [("engine.execute", json!({"command":"edit.undo","params":{}})), ("ui.key", json!({"key":"Z","command":true})), ("ui.click", json!({"x":100,"y":100}))]
    {
        let (request, reply) = ControlRequest::new(method, params);
        tx.send(request).unwrap();
        step(&mut h);
        assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["ok"], false);
    }
    assert!(h.state().synthetic.is_empty());
    assert!(Arc::ptr_eq(&h.state().session.project, &before));
    assert_eq!(h.state().session.revision, revision);
    assert_eq!(h.state().session.is_dirty(), dirty);
    assert_eq!(h.state().session.history.undo.len(), history);
    let (request, reply) = ControlRequest::new("ui.click", json!({"id":CANCEL_UPDATE}));
    tx.send(request).unwrap();
    step(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "cancelled");
    assert!(!h.state().session.is_display_only());
    assert!(Arc::ptr_eq(&h.state().session.project, &before));
    assert_eq!(h.state().session.history.undo.len(), history);
    assert!(h.state().synthetic.is_empty());
}

#[test]
fn workspace_candidate_boots_into_cancel_only_frames_and_close_never_saves_or_releases_a_second_editor() {
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("workspace-candidate-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&dir).unwrap();
    let dir = Scratch(std::fs::canonicalize(dir).unwrap());
    let (mut owner, tx) = harness();
    let path = dir.0.join("generated.fcproj");
    owner.state_mut().session.execute("file.save", json!({"path":path})).unwrap();
    let saved = std::fs::read(&path).unwrap();
    owner.state_mut().session.execute("prefs.set", json!({"values":{"media.enableProxies":false,"trim.largeTrimOffset":12}})).unwrap();
    let clip = owner.state().session.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    owner.state_mut().session.execute("timeline.trim", json!({"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1})).unwrap();
    fn rpc(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, command: &str, params: Value) -> Value {
        let (request, reply) = ControlRequest::new("engine.execute", json!({"command":command,"params":params}));
        tx.send(request).unwrap();
        step(h);
        restore(h);
        let r = reply.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        r["result"].clone()
    }
    let scope = json!({"projectId":"p_writer_ui","anchorSequence":owner.state().session.state.active_sequence.unwrap().0});
    let workspace = "11111111-1111-4111-8111-111111111111";
    let update = "22222222-2222-4222-8222-222222222222";
    let checkpoint = "33333333-3333-4333-8333-333333333333";
    let token = "workspace-candidate-ui-owner-pause-000001";
    rpc(&mut owner, &tx, "agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope}));
    rpc(&mut owner, &tx, "agent.beginWorkspaceUpdate", json!({"token":token,"scope":scope,"workspace":workspace,"update":update}));
    let save_token = "workspace-candidate-ui-save-basis-000001";
    rpc(&mut owner, &tx, "agent.readHistoryCheckpointBasis", json!({"token":save_token,"scope":scope}));
    let history = rpc(
        &mut owner,
        &tx,
        "agent.saveHistoryCheckpoint",
        json!({"token":save_token,"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}),
    );
    let stage_token = "workspace-candidate-ui-stage-basis-00001";
    rpc(&mut owner, &tx, "agent.readHistoryCheckpointBasis", json!({"token":stage_token,"scope":scope}));
    let staged = rpc(
        &mut owner,
        &tx,
        "agent.stageWorkspaceUpdate",
        json!({"token":stage_token,"scope":scope,"workspace":workspace,"update":update,
        "pauseToken":token,"checkpoint":checkpoint,"checkpointSha256":history["checkpoint"]["sha256"]}),
    );
    let mut candidate = Session::default();
    candidate.open_workspace_update(&json!({"projectPath":path,"scope":scope,"workspace":workspace,"update":update,"sha256":staged["sha256"]})).unwrap();
    candidate.agent_operation_control().enable_transport();
    let preferences = candidate.prefs.clone();
    assert!(!preferences.media.enable_proxies);
    assert_eq!(preferences.trim.large_trim_offset, 12);
    let before = candidate.project.clone();
    let state = serde_json::to_value(&candidate.state).unwrap();
    let history = serde_json::to_value(&candidate.history.undo).unwrap();
    let (ctx_tx, ctx_rx) = mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 650.0)).build_eframe(move |_| FilmcraftApp::new(candidate).with_control(ctx_rx));
    for _ in 0..4 {
        step(&mut h);
    }
    assert!(h.state().native_writer.worker.is_some());
    assert_eq!(h.state().session.prefs, preferences);
    assert!(h.state().session.is_display_only());
    assert_eq!(serde_json::to_value(&h.state().session.state).unwrap(), state);
    assert_eq!(serde_json::to_value(&h.state().session.history.undo).unwrap(), history);
    assert!(h.state().auto.elements.iter().any(|e| e.id == CANCEL_UPDATE));
    let (request, reply) = ControlRequest::new("ui.key", json!({"key":"Z","command":true}));
    ctx_tx.send(request).unwrap();
    step(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["ok"], false);
    assert!(super::request_close(h.state_mut()));
    assert_eq!(h.state().session.workspace_update_status().unwrap()["status"], "candidate_cancelled");
    assert!(h.state().session.is_display_only());
    assert!(h.state().dialog.is_none());
    assert!(Arc::ptr_eq(&h.state().session.project, &before));
    assert_eq!(std::fs::read(path).unwrap(), saved);
    assert!(owner.state().session.is_display_only());
    // A separately prepared candidate can activate through the actual worker
    // and show normal editing frames without replacing its captured cut/view.
    let mut active = Session::default();
    active
        .open_workspace_update(&json!({"projectPath":staged["projectPath"],"scope":scope,"workspace":workspace,"update":update,"sha256":staged["sha256"]}))
        .unwrap();
    active.agent_operation_control().enable_transport();
    let project = active.project.clone();
    let editor = serde_json::to_value(&active.state).unwrap();
    let (active_tx, active_rx) = mpsc::channel();
    let mut activated = Harness::builder().with_size(egui::vec2(1000.0, 650.0)).build_eframe(move |_| FilmcraftApp::new(active).with_control(active_rx));
    for _ in 0..4 {
        step(&mut activated);
    }
    let epoch = activated.state().session.workspace_update_status().unwrap()["nativeSession"].clone();
    rpc(
        &mut activated,
        &active_tx,
        "agent.activateWorkspaceUpdate",
        json!({"token":stage_token,"scope":scope,"workspace":workspace,"update":update,"nativeSession":epoch}),
    );
    for _ in 0..4 {
        step(&mut activated);
    }
    assert!(!activated.state().session.is_display_only());
    assert!(Arc::ptr_eq(&activated.state().session.project, &project));
    assert_eq!(serde_json::to_value(&activated.state().session.state).unwrap(), editor);
    let retirement_token = "workspace-candidate-ui-retire-basis-001";
    rpc(&mut owner, &tx, "agent.readHistoryCheckpointBasis", json!({"token":retirement_token,"scope":scope}));
    let retired = rpc(
        &mut owner,
        &tx,
        "agent.retireWorkspaceUpdate",
        json!({"token":retirement_token,"scope":scope,
        "workspace":workspace,"update":update,"pauseToken":token,"stageSha256":staged["sha256"]}),
    );
    assert_eq!(retired["status"], "retiring");
    step(&mut owner);
    assert!(!super::request_close(owner.state_mut()));
    assert!(owner.state().quit_confirmed);
    assert!(owner.state().ui.clip_dialog.is_none());
    assert!(owner.state_mut().session.cancel_workspace_update().is_err());
    assert_eq!(std::fs::read(staged["projectPath"].as_str().unwrap()).unwrap(), saved);
}

#[test]
fn actual_control_write_has_one_delayed_reply_busy_frames_and_no_queued_edit_input() {
    let (mut h, tx) = harness();
    let params = batch(&mut h);
    let before = h.state().session.project.clone();
    let history = h.state().session.history.undo.len();
    let (request, result) = ControlRequest::new("engine.execute", params);
    tx.send(request).unwrap();
    step(&mut h);
    assert!(h.state().session.is_display_only());
    assert!(result.try_recv().is_err());
    for _ in 0..3 {
        step(&mut h);
        assert!(result.try_recv().is_err());
    }
    assert!(h.state().auto.elements.iter().any(|e| e.id == CANCEL));
    assert!(!h.state().auto.elements.iter().any(|e| e.id.starts_with("timeline.clip.")));
    let (request, inspection) = ControlRequest::new("ui.inspect", json!({}));
    tx.send(request).unwrap();
    step(&mut h);
    let read = inspection.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(read["ok"], true);
    assert_eq!(read["result"]["nativeWrite"]["displayOnly"], true);
    for (method, params) in
        [("engine.execute", json!({"command":"edit.undo","params":{}})), ("ui.key", json!({"key":"Z","command":true})), ("ui.click", json!({"x":100,"y":100}))]
    {
        let (request, rejected) = ControlRequest::new(method, params);
        tx.send(request).unwrap();
        step(&mut h);
        assert_eq!(rejected.recv_timeout(Duration::from_secs(2)).unwrap()["ok"], false);
    }
    let (menu, menu_rx) = mpsc::channel();
    h.state_mut().command_inbox = Some(menu_rx);
    menu.send("edit.undo".into()).unwrap();
    step(&mut h);
    assert!(h.state().synthetic.is_empty());
    if let Some(dir) = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join("native-write-busy.png")).unwrap();
    }
    restore(&mut h);
    let receipt = result.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(receipt["ok"], true);
    assert!(result.try_recv().is_err());
    assert!(!h.state().session.is_display_only());
    assert_eq!(h.state().session.history.undo.len(), history + 1);
    assert_ne!(h.state().session.project, before);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project, before);
}

#[test]
fn close_while_busy_waits_for_the_actual_owner_then_keeps_dirty_quit_policy() {
    let (mut h, tx) = harness();
    let params = batch(&mut h);
    let (request, result) = ControlRequest::new("engine.execute", params);
    tx.send(request).unwrap();
    step(&mut h);
    assert!(request_close(h.state_mut()));
    assert!(h.state().native_writer.close_requested);
    assert!(!h.state().quit_confirmed);
    assert!(h.state().session.is_display_only());
    restore(&mut h);
    let _ = result.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!h.state().session.is_display_only());
    assert!(!h.state().quit_confirmed);
    assert!(crate::panels::clip_dialogs::intercept_quit(h.state_mut()));
    assert_eq!(h.state().ui.clip_dialog.as_ref().unwrap().command, "app.quit");
}

#[test]
fn pending_native_input_refuses_writer_start_without_consuming_the_session() {
    let (mut h, _) = harness();
    let params = batch(&mut h);
    let before = h.state().session.project.clone();
    h.state_mut().synthetic.push(egui::Event::Text("held input".into()));
    let (request, _) = ControlRequest::new("engine.execute", params);
    let ctx = h.ctx.clone();
    assert!(matches!(intercept(h.state_mut(),&ctx,&request),Some(Outcome::Done(v))if v["ok"]==false));
    assert!(!h.state().session.is_display_only());
    assert_eq!(h.state().session.project, before);
    assert!(h.state().native_writer.pending.is_none());
}

#[test]
fn menu_id_fallback_uses_the_same_worker_when_command_field_has_the_wrong_type() {
    let (mut h, _) = harness();
    let mut params = batch(&mut h);
    params["id"] = params["command"].take();
    params["command"] = json!(42);
    let (request, result) = ControlRequest::new("ui.menu.invoke", params);
    let ctx = h.ctx.clone();
    assert!(matches!(intercept(h.state_mut(), &ctx, &request), Some(Outcome::Pending)));
    assert!(h.state().session.is_display_only());
    restore(&mut h);
    assert_eq!(result.recv_timeout(Duration::from_secs(2)).unwrap()["ok"], true);
}

#[test]
fn application_setup_has_only_held_first_frames_and_never_queues_editing_input_or_cancel_bypass() {
    use std::fs;
    let root = std::env::temp_dir().join(format!("poema-ui-application-admission-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    fs::create_dir(root.join("edit")).unwrap();
    let request = json!({"kind":"poema-native-project-creation","version":3,"scope":{"projectId":"p_ui_setup","workspaceId":"11111111-1111-4111-8111-111111111111"},"workspace":"11111111-1111-4111-8111-111111111111","update":"22222222-2222-4222-8222-222222222222","name":"New film","projectPath":root.join("edit/project.fcproj"),"activationToken":"a".repeat(64),"applicationProject":{"kind":"poema-native-application-admission","version":1,"operationId":"33333333-3333-4333-8333-333333333333","readyFile":root.join("native-admission-ready.json")}});
    let mut s = Session::default();
    s.create_project_workspace(&request).unwrap();
    s.configure_workspace_update_persistence(filmcraft_engine::autosave::AutosaveConfig::new(root.join("native-state"))).unwrap();
    s.agent_operation_control().enable_transport();
    let f = s.workspace_update_status().unwrap();
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":f["token"],"scope":request["scope"],"workspace":request["workspace"],"update":request["update"],"nativeSession":f["nativeSession"]}),
    )
    .unwrap();
    let before = s.project.clone();
    let (tx, rx) = mpsc::channel();
    let app = FilmcraftApp::new(s).with_control(rx);
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 650.0)).with_max_steps(10_000).build_eframe(move |_| app);
    for _ in 0..4 {
        step(&mut h);
    }
    assert!(h.state().session.is_display_only());
    assert!(h.state().auto.elements.iter().any(|e| e.id == "native-application-setup"));
    assert!(application_initial(h.state()));
    assert!(!h.state().auto.elements.iter().any(|e| e.id == CANCEL_UPDATE));
    for (method, params) in [
        ("engine.execute", json!({"command":"file.newBin","params":{"name":"Not queued"}})),
        ("ui.key", json!({"key":"N","modifiers":["command"]})),
        ("ui.click", json!({"id":CANCEL_UPDATE})),
    ] {
        let (req, reply) = ControlRequest::new(method, params);
        tx.send(req).unwrap();
        step(&mut h);
        let r = reply.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(r["ok"], false, "{r}");
        assert_eq!(*h.state().session.project, *before);
    }
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Text("Must not become a later edit".into()));
    #[derive(Debug)]
    struct HeldDrop(std::path::PathBuf);
    impl egui::DroppedFile for HeldDrop {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("held drop must not be read".into())
        }
    }
    raw.dropped_files.push(std::sync::Arc::new(HeldDrop(root.join("not-imported.mov"))));
    h.state_mut().synthetic.push(egui::Event::Text("Must not queue".into()));
    let context = h.ctx.clone();
    eframe::App::raw_input_hook(h.state_mut(), &context, &mut raw);
    assert!(raw.events.iter().all(|e| matches!(e,egui::Event::ModifiersChanged(m)if *m==egui::Modifiers::NONE)));
    assert!(raw.dropped_files.is_empty());
    assert!(h.state().synthetic.is_empty());
    let status = h.state().session.application_project_admission().unwrap();
    let ready = json!({"kind":"poema-native-application-ready","version":1,"operationId":status["operationId"],"birthOperationId":request["update"],"scope":request["scope"],"projectPath":request["projectPath"],"nativePid":status["nativePid"],"nativeSession":status["nativeSession"],"checkpointSha256":"b".repeat(64),"fingerprint":"c".repeat(64),"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    let file = root.join("native-admission-ready.json");
    fs::write(&file, serde_json::to_vec(&ready).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    for _ in 0..2 {
        step(&mut h);
    }
    assert!(!h.state().session.is_display_only());
    assert_eq!(*h.state().session.project, *before);
    h.state_mut().session.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    assert_eq!(h.state().session.history.undo.len(), 1);
    fs::remove_file(file).unwrap();
    step(&mut h);
    assert!(h.state().session.is_display_only());
    assert!(!application_initial(h.state()), "a previously admitted workspace must still join its composer on close");
    assert!(!request_close(h.state_mut())); // Called after the normal composer handshake in logic.
    assert!(h.state().quit_confirmed);
    drop(h);
    fs::remove_dir_all(root).unwrap();
}

fn save_request(h: &mut Harness<'static, FilmcraftApp>, n: usize) -> (Value, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "save-worker-ui-{}-{n}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let s = &mut h.state_mut().session;
    let scope = json!({"projectId":"p_save_worker_ui","workspaceId":"843c582b-2295-4659-bb6c-09600b48c714"});
    s.execute("file.newProject", json!({"name":"Generated Save UI","workspace":scope})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Generated.fcproj")})).unwrap();
    let token = format!("save-ui-root-{n:032}");
    let job = format!("save-ui-job-{n:032}");
    let operation = format!("save-ui-operation-{n:032}");
    let doc = s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path})).unwrap();
    s.execute("agent.startProjectSavePlan",json!({"job":job,"query":{"token":token,"scope":scope,"beforeProjectSha256":doc["projectState"]["sha256"],"intent":{"kind":"save_copy","path":root.join("Copy.fcproj")}}})).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let r = s.execute("agent.projectSavePlanStatus", json!({"job":job})).unwrap();
        if r["status"] == "delivered" {
            break;
        }
        assert_eq!(r["status"], "running");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    (json!({"job":job,"operation":operation}), root)
}
#[test]
fn save_worker_desktop_retains_exact_cancel_request_and_restores_one_owner_before_cancellation() {
    let (mut h, tx) = harness();
    let (p, root) = save_request(&mut h, 1);
    let bytes = std::fs::read(root.join("Generated.fcproj")).unwrap();
    let (req, begin) = ControlRequest::new("engine.execute", json!({"command":"agent.beginProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    assert!(h.state().native_writer.pending.is_some());
    assert!(h.state().session.is_display_only());
    let (req, cancel) = ControlRequest::new("engine.execute", json!({"command":"agent.cancelProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    assert!(h.state().native_writer.deferred_save_cancel.is_some());
    assert!(cancel.try_recv().is_err());
    restore(&mut h);
    let _ = begin.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(cancel.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "cancelled");
    assert!(h.state().session.pending_save_operation_request().is_none());
    assert!(h.state().native_writer.deferred_save_cancel.is_none());
    assert_eq!(std::fs::read(root.join("Generated.fcproj")).unwrap(), bytes);
    assert!(!root.join("Copy.fcproj").exists());
}
#[test]
fn save_worker_desktop_idle_owner_has_stop_widget_and_rejects_synthetic_editing_until_settled() {
    let (mut h, tx) = harness();
    let (p, _) = save_request(&mut h, 2);
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command":"agent.beginProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    restore(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "held");
    step(&mut h);
    assert!(h.state().auto.elements.iter().any(|e| e.id == CANCEL));
    assert!(!h.state().auto.elements.iter().any(|e| e.id.starts_with("timeline.clip.")));
    let (req, reply) = ControlRequest::new("ui.key", json!({"key":"Z","command":true}));
    tx.send(req).unwrap();
    step(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["ok"], false);
    assert!(h.state().synthetic.is_empty());
    let (req, reply) = ControlRequest::new("ui.click", json!({"id":CANCEL}));
    tx.send(req).unwrap();
    step(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["nativeOperationCancelled"], false);
    restore(&mut h);
    assert!(h.state().session.pending_save_operation_request().is_none());
}
#[test]
fn save_worker_desktop_pending_owner_blocks_scripted_and_window_close_without_discarding_the_owner() {
    let (mut h, tx) = harness();
    let (p, root) = save_request(&mut h, 3);
    let bytes = std::fs::read(root.join("Generated.fcproj")).unwrap();
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command":"agent.beginProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    restore(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "held");
    let (req, reply) = ControlRequest::new("app.quit", json!({}));
    tx.send(req).unwrap();
    step(&mut h);
    assert_eq!(reply.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["closed"], false);
    assert!(!h.state().quit_confirmed);
    assert!(request_close(h.state_mut()));
    assert!(h.state().session.pending_save_operation_request().is_some());
    assert_eq!(std::fs::read(root.join("Generated.fcproj")).unwrap(), bytes);
    let ctx = h.ctx.clone();
    request_stop(h.state_mut(), &ctx).unwrap();
    restore(&mut h);
}

#[test]
fn save_worker_desktop_menu_cancel_alias_keeps_original_wrong_type_command_fallback_while_busy() {
    let (mut h, tx) = harness();
    let (p, root) = save_request(&mut h, 4);
    let bytes = std::fs::read(root.join("Generated.fcproj")).unwrap();
    let (req, begin) = ControlRequest::new("engine.execute", json!({"command":"agent.beginProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    let (req, cancel) = ControlRequest::new("ui.menu.invoke", json!({"command":7,"id":"agent.cancelProjectSaveOperation","params":p}));
    tx.send(req).unwrap();
    step(&mut h);
    assert!(h.state().native_writer.deferred_save_cancel.is_some());
    restore(&mut h);
    let _ = begin.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(cancel.recv_timeout(Duration::from_secs(2)).unwrap()["result"]["status"], "cancelled");
    assert!(h.state().session.pending_save_operation_request().is_none());
    assert_eq!(std::fs::read(root.join("Generated.fcproj")).unwrap(), bytes);
}
