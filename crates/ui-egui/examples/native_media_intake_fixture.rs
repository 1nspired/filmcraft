//! Generated-only native UI control fixture. No desktop window or real media.
//! Exercises the actual asynchronous picker continuation and Session services.
use egui_kittest::Harness;
use filmcraft_engine::{FsServices, Services, Session};
use filmcraft_ui_egui::{FilmcraftApp, control::ControlRequest, native_dialogs::FileDialog};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Sender},
    },
    time::Duration,
};

type Asked = Arc<Mutex<Vec<(FileDialog, Sender<Vec<String>>)>>>;
#[derive(Default)]
struct ReadGate {
    path: Option<String>,
    blocked: bool,
    released: bool,
}
type Gate = Arc<(Mutex<ReadGate>, Condvar)>;
struct GeneratedServices {
    gate: Gate,
}
impl GeneratedServices {
    fn wait(&self, path: &str) -> std::io::Result<()> {
        let mut gate = self.gate.0.lock().unwrap_or_else(|e| e.into_inner());
        if gate.path.as_deref() != Some(path) {
            return Ok(());
        }
        gate.blocked = true;
        while !gate.released {
            let (next, timeout) = self.gate.1.wait_timeout(gate, Duration::from_secs(30)).unwrap_or_else(|e| e.into_inner());
            gate = next;
            if timeout.timed_out() {
                return Err(std::io::Error::other("generated file read was not released"));
            }
        }
        Ok(())
    }
}
impl Services for GeneratedServices {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.wait(path)?;
        FsServices.read_file(path)
    }
    fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        FsServices.write_file(path, bytes)
    }
    fn file_size(&self, path: &str) -> std::io::Result<u64> {
        FsServices.file_size(path)
    }
    fn reader(&self, path: &str) -> Option<std::io::Result<filmcraft_media::SharedReader>> {
        if let Err(e) = self.wait(path) {
            return Some(Err(e));
        }
        FsServices.reader(path)
    }
    fn list_entries(&self, path: &str) -> Option<std::io::Result<Vec<filmcraft_engine::media_browser::DirEntry>>> {
        FsServices.list_entries(path)
    }
    fn list_dir(&self, path: &str) -> Option<std::io::Result<Vec<String>>> {
        FsServices.list_dir(path)
    }
    fn file_loader(&self) -> Option<filmcraft_media::sequence::FrameLoader> {
        FsServices.file_loader()
    }
}
fn release_read(gate: &Gate) {
    gate.0.lock().unwrap_or_else(|e| e.into_inner()).released = true;
    gate.1.notify_all();
}
fn allowed_path(root: &Path, value: &str) -> bool {
    let p = Path::new(value);
    if value.len() > 4096
        || value.chars().any(char::is_control)
        || !p.is_absolute()
        || !p.starts_with(root)
        || p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let existing = if p.exists() {
        p
    } else {
        match p.parent() {
            Some(p) => p,
            None => return false,
        }
    };
    std::fs::canonicalize(existing).is_ok_and(|p| p.starts_with(root))
}
fn connection(mut stream: TcpStream, tx: Sender<ControlRequest>, control: filmcraft_engine::AgentOperationControl) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut input = Vec::new();
    BufReader::new(stream.try_clone()?).take(4 * 1024 * 1024 + 1).read_until(b'\n', &mut input)?;
    if input.len() > 4 * 1024 * 1024 || input.last() != Some(&b'\n') {
        return Err("bounded newline request required".into());
    }
    let value: Value = serde_json::from_slice(&input)?;
    let id = value.get("id").cloned().ok_or("request ID required")?;
    let method = value.get("method").and_then(Value::as_str).ok_or("method required")?;
    if method == "engine.cancelOperation" {
        let result = match control.cancel(value.get("params").unwrap_or(&Value::Null)) {
            Ok(v) => json!({"id":id,"ok":true,"result":v}),
            Err(e) => json!({"id":id,"ok":false,"error":e.to_string()}),
        };
        serde_json::to_writer(&mut stream, &result)?;
        stream.write_all(b"\n")?;
        return Ok(());
    }
    let params = value.get("params").cloned().unwrap_or(json!({}));
    let wait = Duration::from_millis(filmcraft_engine::session_worker::control_reply_wait_ms(method, &params));
    let (req, reply) = ControlRequest::new(method, params);
    tx.send(req)?;
    let mut result = reply.recv_timeout(wait)?;
    if result["ok"] == false {
        eprintln!("generated native control {} {} refused: {}", method, value["params"]["command"], result["error"]);
    }
    result["id"] = id;
    serde_json::to_writer(&mut stream, &result)?;
    stream.write_all(b"\n")?;
    Ok(())
}
fn reply(req: ControlRequest, v: Result<Value, String>) {
    let value = match v {
        Ok(v) => json!({"ok":true,"result":v}),
        Err(e) => json!({"ok":false,"error":e}),
    };
    let _ = req.reply.send(value);
}
fn step(h: &mut Harness<'static, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
    *h.input_mut() = input;
    ctx.request_repaint();
    h.step();
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let persistent = args.last().is_some_and(|v| v == "--persistent");
    if persistent {
        args.pop();
    }
    let recovering = args == ["--project-recovery"];
    let creating = args == ["--project-create"];
    let resume = args.len() == 3 && args.get(2).is_some_and(|v| v == "--resume");
    if !creating && !recovering && (!(args.len() == 2 || resume) || args.first().is_none_or(|v| v != "--fixture-root")) {
        return Err("choose generated --fixture-root, --project-create or --project-recovery mode".into());
    }
    let root = if creating || recovering {
        PathBuf::from(std::env::var("POEMA_NATIVE_PROJECT_DIRECTORY")?)
    } else {
        PathBuf::from(args.get(1).ok_or("fixture root required")?)
    };
    if !root.is_absolute() || std::fs::canonicalize(&root)? != root {
        return Err("an explicit real generated root is required".into());
    }
    let created_root = creating || recovering && root.join("generated-fixture.txt").is_file();
    let marker = root.join(if created_root { "generated-fixture.txt" } else { ".poema-generated-intake-fixture" });
    let expected: &[u8] = if created_root { b"poema-generated-project-fixture-v1\n" } else { b"generated-native-media-intake-v1\n" };
    if !std::fs::symlink_metadata(&marker)?.is_file()
        || std::fs::read(&marker)? != expected
        || created_root && root.file_name().and_then(|v| v.to_str()).is_none_or(|v| !v.starts_with("poema-native-project-"))
    {
        return Err("generated fixture marker and matching scope required".into());
    }
    let gate: Gate = Arc::default();
    let mut s = Session::new(Arc::new(GeneratedServices { gate: gate.clone() }));
    let mut project = root.join(if creating { "generated.fcproj" } else { "generated-intake.fcproj" }).to_string_lossy().into_owned();
    let (native_data, project_id, binding) = if recovering {
        let raw = std::env::var("POEMA_NATIVE_PROJECT_RECOVERY_STARTUP")?;
        if raw.is_empty() || raw.len() > 16384 {
            return Err("bounded generated recovery startup required".into());
        }
        let startup: Value = serde_json::from_str(&raw)?;
        let saved = startup.get("projectPath").and_then(Value::as_str).ok_or("saved generated project path required")?;
        let filename = Path::new(saved).file_name().and_then(|s| s.to_str()).ok_or("generated recovery filename required")?;
        if !["generated.fcproj", "generated-intake.fcproj"].contains(&filename) || root.join(filename) != Path::new(saved) {
            return Err("recovery must use the exact marked generated document".into());
        }
        project = saved.into();
        s.open_project_recovery(&startup)?;
        let data = PathBuf::from(std::env::var("POEMA_NATIVE_PROJECT_RECOVERY_DATA")?);
        if !data.starts_with(&root) || data == root || data.parent().and_then(|p| std::fs::canonicalize(p).ok()).as_deref() != data.parent() {
            return Err("generated recovery data must stay under its real marked root".into());
        }
        s.configure_workspace_update_persistence(filmcraft_engine::autosave::AutosaveConfig::new(&data))?;
        let project_id = startup.get("scope").and_then(|v| v.get("projectId")).and_then(Value::as_str).ok_or("Root project identity required")?.to_string();
        let binding = std::env::var("POEMA_NATIVE_INTAKE_BINDING").ok().map(|raw| filmcraft_ui_egui::composer_panel::Binding::parse(&raw)).transpose()?;
        if let Some(b) = &binding {
            if b.project_path != project || !b.matches_session(&s)? {
                return Err("recovered composer identity differs".into());
            }
        }
        (data, project_id, binding)
    } else if creating {
        let raw = std::env::var("POEMA_NATIVE_PROJECT_CREATE_STARTUP")?;
        if raw.is_empty() || raw.len() > 16384 {
            return Err("bounded generated creation startup required".into());
        }
        let startup: Value = serde_json::from_str(&raw)?;
        let raw_binding = std::env::var("POEMA_NATIVE_INTAKE_BINDING")?;
        let binding = filmcraft_ui_egui::composer_panel::Binding::parse(&raw_binding)?;
        if binding.version != 2
            || startup["projectPath"] != project
            || binding.project_path != project
            || startup["scope"]["projectId"] != binding.project_id
            || startup["scope"]["workspaceId"] != binding.workspace_id
            || startup["workspace"] != binding.workspace_id
        {
            return Err("generated creation and composer identity differ".into());
        }
        s.create_project_workspace(&startup)?;
        if !binding.matches_session(&s)? {
            return Err("created Root does not match its independent binding".into());
        }
        let data = PathBuf::from(std::env::var("POEMA_NATIVE_PROJECT_RECOVERY_DATA")?);
        if data.parent() != Some(root.as_path()) || data.file_name().and_then(|v| v.to_str()) != Some("native-state") {
            return Err("generated native data must be the explicit Root child".into());
        }
        s.configure_workspace_update_persistence(filmcraft_engine::autosave::AutosaveConfig::new(&data))?;
        (data, binding.project_id.clone(), Some(binding))
    } else {
        if resume {
            let file = PathBuf::from(&project);
            let metadata = std::fs::symlink_metadata(&file)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("a generated saved project is required for fixture resume".into());
            }
            s.execute("file.open", json!({"path":project}))?;
        } else {
            if std::fs::symlink_metadata(&project).is_ok() {
                return Err("fixture project must not already exist".into());
            }
            s.execute("file.newSequence", json!({"name":"Generated intake sequence","fps":24}))?;
            s.execute("file.save", json!({"path":project}))?;
        }
        let data = root.join("native-state");
        if persistent {
            let mut config = filmcraft_engine::autosave::AutosaveConfig::new(&data);
            config.journal_debounce = Duration::from_secs(600);
            s.start_autosave(config)?;
        }
        (data, "p_native_intake".to_string(), None)
    };
    std::fs::create_dir_all(&native_data)?;
    s.prefs_path = Some(native_data.join("preferences.json"));
    let sequence = s.state.active_sequence.map(|id| id.0);
    let operation_control = s.agent_operation_control();
    operation_control.enable_transport();
    let (app_tx, app_rx) = mpsc::channel();
    let mut app = FilmcraftApp::new(s).with_control(app_rx);
    if let Some(binding) = binding {
        app.composer_panel = Some(filmcraft_ui_egui::composer_panel::Panel::new(binding));
    }
    let asked: Asked = Arc::default();
    let a = asked.clone();
    app.hooks.run_dialog = Some(Box::new(move |dialog| {
        let (tx, rx) = mpsc::channel();
        a.lock().unwrap_or_else(|e| e.into_inner()).push((dialog, tx));
        rx
    }));
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).with_max_steps(10000).build_eframe(move |_| app);
    for _ in 0..3 {
        step(&mut h);
    }
    let requested_port = if creating || recovering { std::env::var("POEMA_NATIVE_PROJECT_CONTROL_PORT")?.parse::<u16>()? } else { 0 };
    let listener = TcpListener::bind(("127.0.0.1", requested_port))?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = mpsc::channel::<ControlRequest>();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let tx = tx.clone();
                    let control = operation_control.clone();
                    std::thread::spawn(move || {
                        let result = std::panic::catch_unwind(|| connection(stream, tx, control));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => eprintln!("fixture connection: {e}"),
                            Err(_) => eprintln!("fixture connection failed"),
                        }
                    });
                }
                Err(e) => {
                    eprintln!("fixture listener: {e}");
                    break;
                }
            }
        }
    });
    writeln!(std::io::stdout(), "{}", json!({"port":port,"projectPath":project,"projectId":project_id,"sequenceId":sequence,"root":root,"desktop":false}))?;
    std::io::stdout().flush()?;
    loop {
        while let Ok(req) = rx.try_recv() {
            if req.method == "fixture.quit" {
                release_read(&gate);
                reply(req, Ok(json!({"closed":true})));
                return Ok(());
            }
            if req.method == "fixture.import_native_media" {
                let result = (|| -> Result<Value, String> {
                    if req.params.as_object().is_none_or(|p| p.len() != 1) {
                        return Err("one generated media path required".into());
                    }
                    let path = req
                        .params
                        .get("path")
                        .and_then(Value::as_str)
                        .filter(|p| allowed_path(&root, p) && Path::new(p).is_file())
                        .ok_or("generated regular media required")?;
                    h.state_mut().session.execute("file.import", json!({"paths":[path],"imageSequence":false})).map_err(|e| e.to_string())
                })();
                reply(req, result);
                continue;
            }
            if req.method == "fixture.hold_read" {
                let result = (|| -> Result<Value, String> {
                    let path = req
                        .params
                        .get("path")
                        .and_then(Value::as_str)
                        .filter(|p| allowed_path(&root, p) && Path::new(p).is_file())
                        .ok_or("a generated file is required")?;
                    let mut g = gate.0.lock().unwrap_or_else(|e| e.into_inner());
                    if g.path.is_some() && !g.released {
                        return Err("a generated read is already held".into());
                    }
                    *g = ReadGate { path: Some(path.into()), blocked: false, released: false };
                    Ok(json!({"held":true}))
                })();
                reply(req, result);
                continue;
            }
            if req.method == "fixture.release_read" {
                release_read(&gate);
                reply(req, Ok(json!({"released":true})));
                continue;
            }
            if req.method == "fixture.read_status" {
                let g = gate.0.lock().unwrap_or_else(|e| e.into_inner());
                reply(req, Ok(json!({"blocked":g.blocked,"released":g.released})));
                continue;
            }
            if req.method == "fixture.choose" {
                let result = (|| -> Result<Value, String> {
                    let paths = req.params.get("paths").and_then(Value::as_array).ok_or("generated paths required")?;
                    if paths.len() > 256 {
                        return Err("generated selection bound exceeded".into());
                    }
                    let paths = paths
                        .iter()
                        .map(|v| {
                            v.as_str().filter(|s| allowed_path(&root, s)).map(str::to_string).ok_or_else(|| "only generated root paths are allowed".to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let (_, answer) = asked.lock().unwrap_or_else(|e| e.into_inner()).pop().ok_or("no picker is pending")?;
                    answer.send(paths).map_err(|e| e.to_string())?;
                    Ok(json!({"answered":true}))
                })();
                reply(req, result);
                continue;
            }
            if req.method == "fixture.asked" {
                let count = asked.lock().unwrap_or_else(|e| e.into_inner()).len();
                reply(req, Ok(json!({"count":count})));
                continue;
            }
            // Save only this marked fixture's project. Never expose a general
            // filesystem destination to the integration harness.
            let generated_save = req.method == "engine.execute"
                && req.params.as_object().is_some_and(|o| o.len() == 2)
                && req.params["command"] == "file.save"
                && req.params["params"].as_object().is_some_and(|o| o.len() == 1)
                && req.params["params"]["path"].as_str() == Some(project.as_str());
            let allowed = generated_save
                || req.method.starts_with("ui.mediaIntake.")
                || req.method == "ui.poemaComposer"
                || req.method == "ui.poemaComposer.update"
                || req.method == "engine.commands"
                || req.method == "ui.menu.list"
                || req.method == "ui.inspect"
                || req.method == "engine.execute"
                    && req.params.get("command").and_then(Value::as_str).is_some_and(|id| {
                        matches!(
                            id,
                            "state.inspect"
                                | "project.select"
                                | "agent.readSourceCatalog"
                                | "agent.readProjectRecoveryDependencies"
                                | "agent.readArchivedRecoveryDependencies"
                                | "agent.readSourceLibrary"
                                | "agent.validateSourceLibrary"
                                | "agent.inspect"
                                | "agent.inspection"
                                | "agent.cancelInspection"
                                | "prefs.set"
                                | "agent.readApplicationProjectAdmission"
                                | "agent.readProject"
                                | "agent.readProjectSequenceScope"
                                | "agent.selectProjectSequence"
                                | "agent.validateProjectRead"
                                | "agent.readBins"
                                | "agent.readSearchBins"
                                | "agent.validateRetainedHistory"
                                | "agent.inspectPausedWorkspaceHistory"
                                | "agent.inspectHistoryCheckpoint"
                                | "agent.inspectCheckpointProvenance"
                                | "agent.beginWorkspaceUpdate"
                                | "agent.prepareProjectRecovery"
                                | "agent.stageProjectRecovery"
                                | "agent.workspaceUpdateStatus"
                                | "agent.cancelWorkspaceUpdate"
                                | "agent.retireWorkspaceUpdate"
                                | "agent.activateWorkspaceUpdate"
                                | "agent.refreshWorkspaceActivation"
                                | "agent.readSourceSelection"
                                | "agent.snapshot"
                                | "agent.snapshotStatus"
                                | "agent.cancelSnapshot"
                                | "agent.validateSnapshot"
                                | "agent.validationStatus"
                                | "agent.cancelValidation"
                                | "agent.readBasis"
                                | "agent.projectDocument"
                                | "agent.qualifiedDocument"
                                | "agent.readTracks"
                                | "agent.readFinishing"
                                | "agent.readCaptions"
                                | "agent.readCaptionDestination"
                                | "agent.apply"
                                | "agent.prepareApply"
                                | "agent.prepareSourceAdmission"
                                | "agent.sourceAdmissionPreparation"
                                | "agent.cancelSourceAdmissionPreparation"
                                | "agent.receipt"
                                | "agent.readHistory"
                                | "agent.saveHistoryCheckpoint"
                                | "agent.restoreHistoryCheckpoint"
                                | "agent.readHistoryCheckpointBasis"
                                | "agent.bindSource"
                                | "agent.undo"
                                | "agent.redo"
                                | "file.newBin"
                                | "file.newBinFromSelection"
                                | "project.renameBin"
                                | "project.moveToBin"
                                | "file.newSequence"
                                | "file.newProject"
                                | "poema.trackSequence"
                                | "sequence.inspect"
                                | "project.inspect"
                                | "edit.undo"
                                | "edit.redo"
                        )
                    });
            if !allowed {
                reply(req, Err("generated UI fixture command is outside its scope".into()));
                continue;
            }
            app_tx.send(req)?;
        }
        step(&mut h);
        if (persistent || recovering || creating)
            && h.state().session.workspace_update_status().is_some_and(|v| v["status"] == "retiring" || v["status"] == "candidate_cancelled")
        {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
