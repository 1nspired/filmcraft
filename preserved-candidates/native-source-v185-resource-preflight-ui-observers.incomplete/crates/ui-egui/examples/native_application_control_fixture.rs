// Generated headless actor; original desktop parser and actual application handler.
#[path = "../../../apps/filmcraft/src/control_server.rs"]
mod original_control_server;
use filmcraft_engine::{History, Session};
use filmcraft_ui_egui::{
    FilmcraftApp,
    dock::{DockNode, PanelKind},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var("POEMA_GENERATED_TAB_ROOT")?);
    if !root.is_absolute() || root.canonicalize()? != root || std::fs::read(root.join("generated-root.txt"))? != b"generated-application-tab-client-v1\n" {
        return Err("real explicitly marked generated Root required".into());
    }
    let port = std::env::var("POEMA_GENERATED_TAB_PORT")?.parse::<u16>()?;
    if port == 0 {
        return Err("explicit loopback port required".into());
    }
    let recovery_source = match std::env::var("POEMA_GENERATED_TAB_RECOVERY_SOURCE") {
        Ok(v) if v == "1" => true,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err("explicit generated recovery-source flag required".into()),
    };
    let save_source = match std::env::var("POEMA_GENERATED_TAB_SAVE_SOURCE") {
        Ok(v) if v == "1" => true,
        Err(std::env::VarError::NotPresent) => false,
        _ => return Err("explicit generated Save-source flag required".into()),
    };
    let recovery_file = std::env::var_os("POEMA_GENERATED_TAB_RECOVERY").map(PathBuf::from);
    if save_source && (!recovery_source || recovery_file.is_some()) {
        return Err("Save-source mode requires original generated source persistence and excludes cold startup".into());
    }
    if recovery_source && recovery_file.is_some() {
        return Err("generated source and cold recovery modes are exclusive".into());
    }
    let mut s = Session::default();
    let (path, main, other, recovered) = if let Some(file) = recovery_file {
        let meta = std::fs::symlink_metadata(&file)?;
        if !file.is_absolute()
            || file.canonicalize()? != file
            || file.parent() != Some(root.as_path())
            || !meta.is_file()
            || meta.len() == 0
            || meta.len() > 16384
        {
            return Err("bounded generated Root-local original recovery request required".into());
        }
        use std::io::Read as _;
        let mut input = Vec::new();
        std::fs::File::open(&file)?.take(16385).read_to_end(&mut input)?;
        if input.len() > 16384 {
            return Err("generated recovery input grew beyond its bound".into());
        }
        let params: Value = serde_json::from_slice(&input)?;
        let path = PathBuf::from(params.get("projectPath").and_then(Value::as_str).ok_or("cold project path missing")?);
        if !path.is_absolute() || path.canonicalize()? != path || path.parent() != Some(root.as_path()) {
            return Err("exact generated Root-local current project path required".into());
        }
        s.open_project_recovery(&params)?;
        (path, None, None, true)
    } else {
        s.execute("file.openDemoProject", json!({}))?;
        let main = s.state.active_sequence.ok_or("main sequence missing")?;
        s.execute(
            "project.associateEditorialOwner",
            json!({"scope":{"projectId":"p_application_tabs","anchorSequence":main.0},"workspaceId":"6cd868fc-9e1c-485b-87aa-9c60e13b00bf"}),
        )?;
        Arc::make_mut(&mut s.project).metadata.insert("poema.projectWorkspace.v1".into(),serde_json::to_string(&json!({"kind":"poema-native-project-workspace","version":1,"scope":{"projectId":"p_application_tabs","workspaceId":"6cd868fc-9e1c-485b-87aa-9c60e13b00bf"}}))?);
        s.execute("file.newSequence", json!({"name":"Generated other tab","video":0,"audio":0}))?;
        let other = s.state.active_sequence.ok_or("other sequence missing")?;
        s.execute("sequence.open", json!({"item":main.0}))?;
        s.history = History { limit: 128, ..History::default() };
        for track in ["V2", "V3", "A2", "A3"] {
            s.execute("timeline.setTrack", json!({"track":track,"syncLock":false}))?;
        }
        let path = root.join("generated.fcproj");
        s.execute("file.save", json!({"path":path}))?;
        s.prefs_path = Some(root.join("preferences.json"));
        s.events.clear();
        if recovery_source {
            let mut cfg = filmcraft_engine::autosave::AutosaveConfig::new(root.join("native-data"));
            cfg.journal_debounce = Duration::from_secs(600);
            s.start_autosave(cfg)?;
        }
        (path, Some(main.0), Some(other.0), false)
    };
    if save_source {
        // Same original trusted constructor used by desktop startup. The host
        // acquired its real manager lease before this generated actor starts.
        s.install_managed_save_root(&root.join("generated-workspace.json"))?;
    }
    let operations = s.agent_operation_control();
    operations.enable_transport();
    let (control_tx, control_rx) = mpsc::channel();
    let mut app = FilmcraftApp::new(s).with_control(control_rx);
    if !recovered {
        app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1 };
        app.ui.floating.clear();
        app.ui.timeline.pps = 73.;
        app.ui.timeline.target_pps = 73.;
    }
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000., 700.)).build_eframe(move |_| app);
    let rx = original_control_server::start(port, h.ctx.clone(), operations);
    let mut transcript = Vec::<Value>::new();
    let mut transcript_bytes = 0usize;
    let mut quit = false;
    let begin = Instant::now();
    println!(
        "{}",
        json!({"port":port,"root":root,"projectPath":path,"main":main,"other":other,"pid":std::process::id(),"originalDesktopControlServer":true,"actualApplicationHandler":true,"originalApplicationControlQueue":true,"desktopWindow":false,"recovered":recovered,"recoverySource":recovery_source,"saveSource":save_source})
    );
    std::io::stdout().flush()?;
    loop {
        if begin.elapsed() > Duration::from_secs(180) {
            return Err("bounded original application fixture deadline".into());
        }
        while let Ok(req) = rx.try_recv() {
            let observed = json!({"method":req.method,"params":req.params});
            transcript_bytes = transcript_bytes.checked_add(serde_json::to_vec(&observed)?.len()).ok_or("generated control transcript size overflow")?;
            if transcript.len() >= 4096 || transcript_bytes > 64 * 1024 * 1024 {
                return Err("bounded generated control transcript exceeded".into());
            }
            transcript.push(observed);
            if req.method == "fixture.quit" {
                quit = true;
                let _ = req.reply.send(json!({"ok":true,"result":{"closed":true}}));
                continue;
            }
            if req.method == "fixture.status" {
                let _=req.reply.send(json!({"ok":true,"result":{"active":h.state().session.state.active_sequence.map(|i|i.0),"journal":h.state().session.journal,"displayOnly":h.state().session.is_display_only(),"trimActive":h.state().session.trim_play.active()}}));
                continue;
            }
            control_tx.send(req).map_err(|_| "original application control inbox closed")?;
        }
        let ctx = h.ctx.clone();
        let mut input = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
        *h.input_mut() = input;
        h.step();
        // Generated crash/capture test only: stop with the exact retained Save
        // still owned. A display projection has no pending Save request and
        // cannot pass this check. No Session is transferred or owner released.
        let retained_save_source = save_source && h.state().session.pending_save_operation_request().is_some();
        if quit && (h.state().session.ensure_desktop_startup_transfer().is_ok() || retained_save_source) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let path = root.join(if recovered { "actual-native-cold-control-transcript.json" } else { "actual-native-control-transcript.json" });
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(
        &mut out,
        &json!({"requests":transcript,"finalJournal":h.state().session.journal,"displayOnly":h.state().session.is_display_only(),"sameOriginalApplication":true,"transferReady":h.state().session.ensure_desktop_startup_transfer().is_ok(),"workspaceHeld":h.state().session.workspace_update_status().is_some(),"recovered":recovered,"normalConstructor":true,"sourceRecoveryEnabled":recovery_source,"saveSource":save_source,"savePendingAtShutdown":h.state().session.pending_save_operation_request()}),
    )?;
    out.write_all(b"\n")?;
    out.sync_all()?;
    Ok(())
}
