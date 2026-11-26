//! Parent-owned generated empty-project fixture. Actual Session commands and
//! native saved format only; no GUI, media decoder, model hand or user paths.
use filmcraft_engine::Session;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    time::Duration,
};

const KEY: &str = "poema.projectWorkspace.v1";
fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) && std::fs::canonicalize(path).is_ok_and(|p| p == path)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if !arguments.is_empty() && arguments != ["--reopen"] && arguments != ["--project-recovery"] && arguments != ["--project-create"] {
        return Err("choose new or explicit generated reopen".into());
    }
    let reopen = arguments == ["--reopen"];
    let recovery = arguments == ["--project-recovery"];
    let creating = arguments == ["--project-create"];
    let persistent = std::env::var("POEMA_NATIVE_PROJECT_PERSISTENCE").as_deref() == Ok("1");
    if (recovery || creating) && !persistent {
        return Err("explicit generated persistent recovery mode required".into());
    }
    let root = PathBuf::from(std::env::var("POEMA_NATIVE_PROJECT_DIRECTORY")?);
    let marker = root.join("generated-fixture.txt");
    if !root.is_absolute()
        || std::fs::canonicalize(&root)? != root
        || root.file_name().and_then(|s| s.to_str()).is_none_or(|s| !s.starts_with("poema-native-project-"))
        || !regular(&marker)
        || std::fs::metadata(&marker)?.len() > 128
        || std::fs::read_to_string(&marker)? != "poema-generated-project-fixture-v1\n"
    {
        return Err("project fixture requires its marked unredirected parent-owned generated directory".into());
    }
    let raw = std::env::var("POEMA_NATIVE_PROJECT_DESCRIPTOR")?;
    if raw.len() > 4096 {
        return Err("generated project descriptor exceeds its bound".into());
    }
    let descriptor: Value = serde_json::from_str(&raw)?;
    let mut session = Session::default();
    let path = root.join("generated.fcproj");
    if recovery || creating {
        let raw = std::env::var(if creating { "POEMA_NATIVE_PROJECT_CREATE_STARTUP" } else { "POEMA_NATIVE_PROJECT_RECOVERY_STARTUP" })?;
        if raw.is_empty() || raw.len() > 16384 {
            return Err("bounded explicit native recovery startup required".into());
        }
        let startup: Value = serde_json::from_str(&raw)?;
        if startup["projectPath"] != path.to_string_lossy().as_ref() || startup["scope"] != descriptor["workspace"] {
            return Err("generated recovery scope/path differs".into());
        }
        if creating {
            session.create_project_workspace(&startup)?;
        } else {
            session.open_project_recovery(&startup)?;
        }
        session.configure_workspace_update_persistence(filmcraft_engine::autosave::AutosaveConfig::new({
            let data = std::env::var("POEMA_NATIVE_PROJECT_RECOVERY_DATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| root.join(format!("recovery-state-{}", std::process::id())));
            if !data.starts_with(&root) || data == root || data.parent().and_then(|p| std::fs::canonicalize(p).ok()).as_deref() != data.parent() {
                return Err("generated recovery data must stay under its real marked root".into());
            }
            data
        }))?;
    } else {
        session.execute("file.newProject", descriptor.clone())?;
        let identity = session.project.metadata.get(KEY).ok_or("generated project identity required")?.clone();
        if persistent {
            let mut config = filmcraft_engine::autosave::AutosaveConfig::new(root.join("native-state"));
            config.journal_debounce = Duration::from_secs(600);
            session.start_autosave(config)?;
        }
        if reopen {
            if !regular(&path) || std::fs::metadata(&path)?.len() > 16 * 1024 * 1024 {
                return Err("only this bounded generated regular project may reopen".into());
            }
            session.execute("file.open", json!({"path":path}))?;
            if session.project.metadata.get(KEY) != Some(&identity) {
                return Err("saved generated workspace identity changed".into());
            }
        } else {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => return Err("generated project already exists; nothing overwritten".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            session.path = Some(path.to_str().ok_or("generated project path must be UTF-8")?.into());
            session.execute("file.save", json!({}))?;
        }
    }
    // Only this parent's marked generated Root may retain its actual original
    // manager. This is a trusted startup constructor, never a control command.
    if let Some(manifest) = std::env::var_os("POEMA_NATIVE_SAVE_STARTUP_ROOT") {
        let manifest = PathBuf::from(manifest);
        if !persistent || manifest != root.join("workspace.json") {
            return Err("generated Save manager requires the original persistent Root".into());
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if recovery || creating {
            session.retain_managed_save_startup_root(&manifest)?;
        } else {
            session.install_managed_save_root(&manifest)?;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err("generated managed Save startup is unavailable".into());
    }
    if persistent {
        let worker = filmcraft_engine::session_worker::SessionWorker::new()?;
        worker.install(&mut session)?;
    }
    let publication_delay = std::env::var("POEMA_NATIVE_PROJECT_PUBLICATION_DELAY_MS").ok().map(|v| v.parse::<u64>()).transpose()?.unwrap_or(0);
    if publication_delay > 1000 {
        return Err("generated publication delay exceeds its bound".into());
    }
    let requested_port = std::env::var("POEMA_NATIVE_PROJECT_CONTROL_PORT").ok().map(|v| v.parse::<u16>()).transpose()?.unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", requested_port))?;
    let port = listener.local_addr()?.port();
    let delayed = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let requests = transport(listener, session.agent_operation_control(), delayed.clone())?;
    println!(
        "{}",
        json!({"port":port,"projectPath":path,"projectId":descriptor["workspace"]["projectId"],
        "workspaceId":descriptor["workspace"]["workspaceId"],"sequenceId":session.state.active_sequence.map(|id|id.0)})
    );
    std::io::stdout().flush()?;
    // A genuine all-published window needs 256 read/apply pairs plus final probes.
    // Keep transport, payload and original runner deadlines unchanged.
    for (request, reply) in requests.into_iter().take(1024) {
        let command = request["params"]["command"].as_str().unwrap_or("");
        let allowed = request["method"] == "engine.execute"
            && matches!(
                command,
                "prefs.set"
                    | "file.newBlackVideo"
                    | "file.newBarsAndTone"
                    | "file.import"
                    | "source.open"
                    | "source.overwrite"
                    | "project.setMarks"
                    | "timeline.select"
                    | "playhead.set"
                    | "clip.createMulticam"
                    | "clip.multicamEnable"
                    | "multicam.inspect"
                    | "multicam.editCameras"
                    | "multicam.gridLayout"
                    | "multicam.page"
                    | "multicam.audioFollowsVideo"
                    | "multicam.recordStart"
                    | "multicam.cut"
                    | "multicam.recordStop"
                    | "agent.readMulticam"
                    | "agent.readSourceCatalog"
                    | "agent.readSourceLibrary"
                    | "agent.snapshotStatus"
                    | "agent.cancelSnapshot"
                    | "agent.validateSourceLibrary"
                    | "file.newSequence"
                    | "file.newBin"
                    | "file.newSearchBin"
                    | "project.editSearchBin"
                    | "project.deleteSearchBin"
                    | "project.delete"
                    | "project.searchBinItems"
                    | "project.renameBin"
                    | "project.moveToBin"
                    | "edit.undo"
                    | "edit.redo"
                    | "project.inspect"
                    | "state.inspect"
                    | "history.list"
                    | "sequence.inspect"
                    | "sequence.open"
                    | "sequence.close"
                    | "sequence.settings"
                    | "file.save"
                    | "agent.snapshot"
                    | "agent.readApplicationProjectAdmission"
                    | "agent.readProject"
                    | "agent.readFontCatalog"
                    | "agent.readFontsUsed"
                    | "agent.readCaptionFontResources"
                    | "agent.planProjectFontReplacement"
                    | "agent.planProjectFontResources"
                    | "agent.readProjectFontResources"
                    | "agent.startFontResources"
                    | "agent.fontResourcesStatus"
                    | "agent.cancelFontResources"
                    | "agent.planGraphicsFonts"
                    | "agent.readProjectSequenceScope"
                    | "agent.selectProjectSequence"
                    | "agent.validateProjectRead"
                    | "agent.readBins"
                    | "agent.readSearchBins"
                    | "agent.readHistory"
                    | "agent.readHistoryCheckpointBasis"
                    | "agent.saveHistoryCheckpoint"
                    | "agent.restoreHistoryCheckpoint"
                    | "agent.validateRetainedHistory"
                    | "agent.inspectPausedWorkspaceHistory"
                    | "agent.preparePausedProjectRecovery"
                    | "agent.pausedProjectRecoveryRecord"
                    | "agent.inspectHistoryCheckpoint"
                    | "agent.beginWorkspaceUpdate"
                    | "agent.prepareProjectRecovery"
                    | "agent.stageProjectRecovery"
                    | "agent.workspaceUpdateStatus"
                    | "agent.cancelWorkspaceUpdate"
                    | "agent.retireWorkspaceUpdate"
                    | "agent.activateWorkspaceUpdate"
                    | "agent.refreshWorkspaceActivation"
                    | "agent.apply"
                    | "agent.undo"
                    | "agent.redo"
                    | "agent.receipt"
                    | "agent.validateSnapshot"
                    | "agent.prepareApply"
                    | "agent.inspect"
                    | "agent.readSourceSelection"
            );
        if publication_delay > 0
            && matches!(command, "agent.apply" | "agent.undo" | "agent.redo" | "agent.saveHistoryCheckpoint" | "agent.restoreHistoryCheckpoint")
        {
            *delayed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = request["params"]["params"]["token"].as_str().map(str::to_owned);
            std::thread::sleep(Duration::from_millis(publication_delay));
            *delayed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        // Import only the parent's one bounded, generated PCM file. Never let a
        // fixture allowlist addition open arbitrary host media or replace a Root.
        let generated_import = command != "file.import"
            || request["params"]["params"].as_object().is_some_and(|params| {
                params.len() == 1
                    && params.get("paths").is_some_and(|paths| {
                        paths.as_array().is_some_and(|paths| {
                            paths.len() == 1
                                && paths.first().is_some_and(|path| {
                                    let expected = root.join("generated-audio.wav");
                                    path.as_str() == expected.to_str()
                                        && regular(&expected)
                                        && std::fs::metadata(&expected).is_ok_and(|m| m.len() <= 2 * 1024 * 1024)
                                })
                        })
                    })
            });
        let result = if allowed && generated_import {
            session.execute(command, request["params"]["params"].clone()).map_err(|e| e.to_string())
        } else {
            Err("generated project fixture only permits bounded shared native project commands".into())
        };
        let retire_exit = std::env::var("POEMA_NATIVE_PROJECT_RETIRE_EXIT").as_deref() == Ok("1") && result.as_ref().is_ok_and(|v| v["status"] == "retiring");
        let envelope = match result {
            Ok(result) => json!({"id":request["id"],"ok":true,"result":result}),
            Err(error) => json!({"id":request["id"],"ok":false,"error":error}),
        };
        let _ = reply.send(envelope);
        if retire_exit {
            std::thread::sleep(Duration::from_millis(50));
            session.shutdown();
            break;
        }
    }
    Ok(())
}

// Same single Session owner; only cancellation/diagnostic atomics bypass its
// bounded queue. Advertise Stop only after the separate transport is installed.
type RequestQueue = std::sync::mpsc::Receiver<(Value, std::sync::mpsc::Sender<Value>)>;
fn transport(
    listener: TcpListener,
    operations: filmcraft_engine::AgentOperationControl,
    delayed: std::sync::Arc<std::sync::Mutex<Option<String>>>,
) -> Result<RequestQueue, Box<dyn std::error::Error>> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    struct Slot(Arc<AtomicUsize>);
    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let (tx, rx) = mpsc::sync_channel::<(Value, mpsc::Sender<Value>)>(16);
    let remote = operations.clone();
    std::thread::Builder::new().name("generated-project-listener".into()).spawn(move||{
  let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
   let count=Arc::new(AtomicUsize::new(0));
   for stream in listener.incoming().take(1024) {
    let Ok(mut stream)=stream else {continue};
    if count.fetch_add(1,Ordering::SeqCst)>=8 {count.fetch_sub(1,Ordering::SeqCst);let _=writeln!(stream,"{}",json!({"ok":false,"error":"generated project fixture busy"}));continue;}
    let slot=Slot(count.clone());let tx=tx.clone();let operations=remote.clone();let delayed=delayed.clone();
    if let Err(error)=std::thread::Builder::new().name("generated-project-connection".into()).spawn(move||{
     let _slot=slot;
     let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
      let handle=||->Result<Value,Box<dyn std::error::Error>> {
       stream.set_read_timeout(Some(Duration::from_secs(5)))?;stream.set_write_timeout(Some(Duration::from_secs(5)))?;
       let mut line=String::new();BufReader::new((&stream).take(1024*1024+1)).read_line(&mut line)?;if line.len()>1024*1024{return Err("generated request exceeds bound".into());}
       let request:Value=serde_json::from_str(&line)?;
       if request["method"]=="engine.cancelOperation" {return Ok(match operations.cancel(&request["params"]) {Ok(result)=>json!({"id":request["id"],"ok":true,"result":result}),Err(error)=>json!({"id":request["id"],"ok":false,"error":error.to_string()})});}
       if request["method"]=="fixture.projectWriterStatus" {return Ok(json!({"id":request["id"],"ok":true,"result":{"delayedToken":delayed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()}}));}
       let (reply,wait)=mpsc::channel();tx.try_send((request,reply))?;Ok(wait.recv_timeout(Duration::from_secs(47))?)
      };
      match handle() {Ok(reply)=>{let _=writeln!(stream,"{reply}");},Err(error)=>{let _=writeln!(stream,"{}",json!({"ok":false,"error":error.to_string()}));}}
     }));
    }) {eprintln!("generated project connection failed: {error}");}
   }
  }));
 })?;
    operations.enable_transport();
    Ok(rx)
}
