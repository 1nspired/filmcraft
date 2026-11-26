//! Parent-owned generated-input probe. No GUI, decoder, credentials, provider,
//! grant import or activation. The explicit worker probe creates only the
//! original gated persistence worker and never releases it.
use filmcraft_engine::Session;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    path::PathBuf,
    sync::Arc,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().len() != 1 {
        return Err("this generated probe accepts its bounded parent environment only".into());
    }
    let root = PathBuf::from(std::env::var("POEMA_NATIVE_COLD_SAVE_FIXTURE_ROOT")?);
    let marker = root.join("generated-native-cold-save-fixture.txt");
    if !root.is_absolute()
        || std::fs::canonicalize(&root)? != root
        || root.file_name().and_then(|s| s.to_str()).is_none_or(|s| !s.starts_with("poema-save-root-file-"))
        || !std::fs::symlink_metadata(&marker)?.is_file()
        || std::fs::metadata(&marker)?.len() != 38
        || std::fs::read_to_string(&marker)? != "poema-native-cold-Save-input-probe-v1\n"
    {
        return Err("marked generated cold Save probe root required".into());
    }
    let raw = std::env::var("POEMA_NATIVE_COLD_SAVE_REFERENCE")?;
    if raw.is_empty() || raw.len() > 16384 {
        return Err("bounded explicit original input reference required".into());
    }
    let reference: Value = serde_json::from_str(&raw)?;
    let input = PathBuf::from(reference["path"].as_str().ok_or("explicit private reference path required")?);
    if !input.starts_with(&root) || input == root || std::fs::canonicalize(&input)? != input {
        return Err("probe reference escapes its generated output root".into());
    }
    let mut s = Session::default();
    let fence = s.open_save_recovery(&reference)?;
    let graph = s.project.clone();
    let revision = s.revision;
    let saved_revision = s.saved_revision;
    let editor = serde_json::to_value(&s.state)?;
    for command in ["file.save", "file.newBin", "edit.undo", "edit.redo", "prefs.set"] {
        if s.execute(command, json!({})).is_ok() {
            return Err("a native cold Save hold allowed editing".into());
        }
    }
    if !Arc::ptr_eq(&graph, &s.project)
        || s.revision != revision
        || s.saved_revision != saved_revision
        || serde_json::to_value(&s.state)? != editor
        || !s.is_display_only()
    {
        return Err("generated native state changed under its cold hold".into());
    }
    let status = s.cold_save_recovery_status()?.ok_or("actual native cold hold required")?;
    if status != fence {
        return Err("native cold hold changed".into());
    }
    // The complete production handshake is shared with trusted native startup.
    // Legacy prefix probes below remain available for earlier component tests.
    let full_handshake = [
        "POEMA_NATIVE_COLD_SAVE_RUNTIME",
        "POEMA_NATIVE_COLD_SAVE_WORKER",
        "POEMA_NATIVE_COLD_SAVE_CANONICAL",
        "POEMA_NATIVE_COLD_SAVE_BINDINGS",
        "POEMA_NATIVE_COLD_SAVE_BINDING_OBSERVATION",
    ]
    .iter()
    .all(|key| std::env::var_os(key).is_some_and(|value| value == "1"));
    if full_handshake {
        use filmcraft_engine::cold_save_startup::{ColdSavePipes, run_held_cold_save_startup};
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut pipes = ColdSavePipes::new(stdin.lock(), stdout.lock());
        let handoff = run_held_cold_save_startup(&mut s, &mut pipes)?;
        // Replay probes belong here, not in the shared startup implementation.
        for replay in [
            s.prepare_cold_save_runtime(),
            s.create_cold_save_persistence(),
            s.prepare_cold_save_canonical(),
            s.publish_cold_save_canonical(),
            s.bind_cold_save_services(),
        ] {
            if replay.is_ok() {
                return Err("shared cold startup replayed an effect".into());
            }
        }
        if handoff.outcome.is_ok() {
            s.sync_persistence();
            s.poll_persistence();
            s.cold_save_recovery_status()?.ok_or("shared startup lost its original cold synchronization hold")?;
        }
        for command in ["file.save", "file.newBin", "edit.undo", "edit.redo", "prefs.set"] {
            if s.execute(command, json!({})).is_ok() {
                return Err("shared cold startup released editing".into());
            }
        }
        if !Arc::ptr_eq(&graph, &s.project)
            || s.revision != revision
            || s.saved_revision != saved_revision
            || serde_json::to_value(&s.state)? != editor
            || !s.is_display_only()
        {
            return Err("shared cold startup changed the original native cut or editor".into());
        }
        let result = match handoff.outcome {
            Ok(status) => json!({"accepted":true,"status":status}),
            Err(error) => json!({"accepted":false,"error":error.to_string()}),
        };
        s.shutdown();
        println!(
            "{}",
            serde_json::to_string(&json!({"status":"actual-native-canonical-result",
            "pid":std::process::id(),"result":result,"publication":handoff.publication,
            "serviceBindings":handoff.service_bindings,"bindingObservationRequired":true,
            "bindingObservationConfirmed":handoff.binding_observation_confirmed,
            "editingPaused":true,"workerReleased":false,"directSynchronizationHeld":true,
            "graphAndEditorUnchanged":true,"revision":revision,"savedRevision":saved_revision,
            "providerUsed":false,"guiOpened":false}))?
        );
        return Ok(());
    }
    if std::env::var_os("POEMA_NATIVE_COLD_SAVE_RUNTIME").is_some() {
        if std::env::var("POEMA_NATIVE_COLD_SAVE_RUNTIME")? != "1" {
            return Err("explicit generated runtime handshake required".into());
        }
        let runtime = s.prepare_cold_save_runtime()?;
        if !Arc::ptr_eq(&graph, &s.project) || s.revision != revision || !s.is_display_only() {
            return Err("runtime reservation changed the native cut".into());
        }
        if s.prepare_cold_save_runtime().is_ok() {
            return Err("a second native runtime reservation was issued".into());
        }
        println!("{}", serde_json::to_string(&json!({"status":"actual-native-runtime-reserved","pid":std::process::id(),"fence":fence,"runtime":runtime}))?);
        std::io::stdout().flush()?;
        if std::env::var_os("POEMA_NATIVE_COLD_SAVE_WORKER").is_some() {
            if std::env::var("POEMA_NATIVE_COLD_SAVE_WORKER")? != "1" {
                return Err("explicit generated gated-worker handshake required".into());
            }
            fn receipt(reader: &mut impl BufRead) -> Result<Value, Box<dyn std::error::Error>> {
                let mut raw = Vec::new();
                reader.take(512 * 1024 + 1).read_until(b'\n', &mut raw)?;
                if raw.is_empty() || raw.len() > 512 * 1024 || raw.last() != Some(&b'\n') {
                    return Err("one bounded newline-terminated original receipt required".into());
                }
                Ok(serde_json::from_slice(&raw)?)
            }
            let stdin = std::io::stdin();
            let mut reader = stdin.lock();
            let first = receipt(&mut reader)?;
            let confirmed = s.confirm_cold_save_runtime(&first)?;
            if confirmed["runtime"]["journalConfirmed"] != true || s.confirm_cold_save_runtime(&first).is_ok() {
                return Err("exactly one original intent confirmation required".into());
            }
            let worker = s.create_cold_save_persistence()?;
            if worker["persistenceStarted"] != true || worker["runtime"]["workerConfirmed"] != false || s.create_cold_save_persistence().is_ok() {
                return Err("exactly one actual gated persistence worker required".into());
            }
            s.sync_persistence();
            s.poll_persistence();
            s.cold_save_recovery_status()?.ok_or("cold worker lost its direct synchronization hold")?;
            println!("{}", serde_json::to_string(&json!({"status":"actual-native-worker-created","pid":std::process::id(),"owner":worker}))?);
            std::io::stdout().flush()?;
            let second = receipt(&mut reader)?;
            let outcome = s.confirm_cold_save_persistence(&second);
            let accepted = outcome.is_ok();
            if accepted {
                s.sync_persistence();
                s.poll_persistence();
                s.cold_save_recovery_status()?.ok_or("confirmed cold worker lost its direct synchronization hold")?;
            }
            if std::env::var_os("POEMA_NATIVE_COLD_SAVE_CANONICAL").is_some() {
                if std::env::var("POEMA_NATIVE_COLD_SAVE_CANONICAL")? != "1" || !accepted {
                    return Err("canonical fixture requires its confirmed actual gated worker".into());
                }
                let plan = s.prepare_cold_save_canonical()?;
                if s.prepare_cold_save_canonical().is_ok() {
                    return Err("native canonical preparation was replayed".into());
                }
                println!("{}", serde_json::to_string(&json!({"status":"actual-native-canonical-prepared","pid":std::process::id(),"plan":plan}))?);
                std::io::stdout().flush()?;
                let intent = receipt(&mut reader)?;
                let intent_result = s.confirm_cold_save_canonical_intent(&intent);
                let mut publication = Value::Null;
                let mut canonical_outcome = match intent_result {
                    Ok(_) => {
                        if s.confirm_cold_save_canonical_intent(&intent).is_ok() {
                            return Err("native canonical intent confirmation was replayed".into());
                        }
                        publication = s.publish_cold_save_canonical()?;
                        if s.publish_cold_save_canonical().is_ok() {
                            return Err("native canonical writer was replayed".into());
                        }
                        println!(
                            "{}",
                            serde_json::to_string(&json!({"status":"actual-native-canonical-published","pid":std::process::id(),"publication":publication}))?
                        );
                        std::io::stdout().flush()?;
                        let observation = receipt(&mut reader)?;
                        let observed = s.confirm_cold_save_canonical_observation(&observation);
                        if s.confirm_cold_save_canonical_observation(&observation).is_ok() {
                            return Err("native canonical observation was replayed".into());
                        }
                        observed
                    }
                    Err(error) => Err(error),
                };
                let canonical_accepted = canonical_outcome.is_ok();
                let mut service_bindings = Value::Null;
                let observe_bindings = match std::env::var_os("POEMA_NATIVE_COLD_SAVE_BINDING_OBSERVATION") {
                    None => false,
                    Some(mode) if mode == "1" => true,
                    _ => return Err("explicit generated binding-observation handshake required".into()),
                };
                if observe_bindings && std::env::var_os("POEMA_NATIVE_COLD_SAVE_BINDINGS").is_none() {
                    return Err("binding observation requires the original binding step".into());
                }
                let mut binding_observation_confirmed = false;
                if let Some(mode) = std::env::var_os("POEMA_NATIVE_COLD_SAVE_BINDINGS") {
                    if mode != "1" {
                        return Err("explicit generated original service-binding handshake required".into());
                    }
                    let retained_path = s.path.clone();
                    if canonical_accepted {
                        service_bindings = s.bind_cold_save_services()?;
                        let directory = plan["effects"]["directory"]["path"].as_str().ok_or("actual canonical data directory missing")?;
                        if service_bindings["bound"] != true
                            || service_bindings["workersMayResume"] != false
                            || s.prefs_path != Some(PathBuf::from(directory).join("preferences.json"))
                        {
                            return Err("original cold services did not bind to the exact canonical directory".into());
                        }
                        let owner = s.cold_save_recovery_status()?.ok_or("service binding lost the same original cold owner")?;
                        if owner["runtime"]["canonical"]["serviceBindings"]["phase"] != "bound"
                            || owner["runtime"]["canonical"]["serviceBindings"]["bound"] != true
                            || owner["runtime"]["canonical"]["bindingObservationConfirmed"] != false
                        {
                            return Err("binding status did not come from the actual retained native continuation".into());
                        }
                        if observe_bindings {
                            println!(
                                "{}",
                                serde_json::to_string(&json!({"status":"actual-native-service-bindings-bound",
                                "pid":std::process::id(),"bindings":service_bindings,"owner":owner}))?
                            );
                            std::io::stdout().flush()?;
                            let observation = receipt(&mut reader)?;
                            let observed = s.confirm_cold_save_service_bindings(&observation);
                            binding_observation_confirmed = observed.is_ok();
                            if s.confirm_cold_save_service_bindings(&observation).is_ok() {
                                return Err("native binding observation was replayed".into());
                            }
                            canonical_outcome = observed;
                        }
                    } else {
                        if s.bind_cold_save_services().is_ok() {
                            return Err("unconfirmed canonical observation allowed service binding".into());
                        }
                        service_bindings = json!({"bound":false,"refused":true});
                    }
                    if s.path != retained_path {
                        return Err("binding native services changed the retained human document path".into());
                    }
                }
                if canonical_outcome.is_ok() {
                    s.sync_persistence();
                    s.poll_persistence();
                    s.cold_save_recovery_status()?.ok_or("canonical handoff lost its original cold synchronization hold")?;
                }
                for command in ["file.save", "file.newBin", "edit.undo", "edit.redo", "prefs.set"] {
                    if s.execute(command, json!({})).is_ok() {
                        return Err("canonical handoff released editing".into());
                    }
                }
                if !Arc::ptr_eq(&graph, &s.project)
                    || s.revision != revision
                    || s.saved_revision != saved_revision
                    || serde_json::to_value(&s.state)? != editor
                    || !s.is_display_only()
                {
                    return Err("canonical handoff changed the original native cut/editor/history or edit hold".into());
                }
                let final_result = match canonical_outcome {
                    Ok(status) => json!({"accepted":true,"status":status}),
                    Err(error) => json!({"accepted":false,"error":error.to_string()}),
                };
                s.shutdown();
                println!(
                    "{}",
                    serde_json::to_string(&json!({"status":"actual-native-canonical-result",
                    "pid":std::process::id(),"result":final_result,"publication":publication,"serviceBindings":service_bindings,
                    "bindingObservationRequired":observe_bindings,"bindingObservationConfirmed":binding_observation_confirmed,
                    "editingPaused":true,"workerReleased":false,"directSynchronizationHeld":true,
                    "graphAndEditorUnchanged":true,"revision":revision,"savedRevision":saved_revision,
                    "providerUsed":false,"guiOpened":false}))?
                );
                return Ok(());
            }
            if s.confirm_cold_save_persistence(&second).is_ok() || s.create_cold_save_persistence().is_ok() {
                return Err("worker confirmation or construction was replayed".into());
            }
            for command in ["file.save", "file.newBin", "edit.undo", "edit.redo", "prefs.set"] {
                if s.execute(command, json!({})).is_ok() {
                    return Err("gated worker released native editing".into());
                }
            }
            if !Arc::ptr_eq(&graph, &s.project)
                || s.revision != revision
                || s.saved_revision != saved_revision
                || serde_json::to_value(&s.state)? != editor
                || !s.is_display_only()
            {
                return Err("gated worker changed original native graph, editor, revision or edit hold".into());
            }
            let result = match outcome {
                Ok(status) => json!({"accepted":true,"status":status}),
                Err(error) => json!({"accepted":false,"error":error.to_string()}),
            };
            s.shutdown();
            println!(
                "{}",
                serde_json::to_string(&json!({"status":"actual-native-worker-result","pid":std::process::id(),
                "result":result,"editingPaused":true,"workerStarted":true,"workerReleased":false,"directSynchronizationHeld":true,"providerUsed":false,"guiOpened":false,
                "revision":revision,"savedRevision":saved_revision,"graphAndEditorUnchanged":true,"confirmationAccepted":accepted}))?
            );
            return Ok(());
        }
        let mut raw = Vec::new();
        std::io::stdin().take(512 * 1024 + 1).read_to_end(&mut raw)?;
        if raw.is_empty() || raw.len() > 512 * 1024 {
            return Err("one bounded original runtime receipt required".into());
        }
        let receipt: Value = serde_json::from_slice(&raw)?;
        let outcome = s.confirm_cold_save_runtime(&receipt);
        let accepted = outcome.is_ok();
        if accepted && s.confirm_cold_save_runtime(&receipt).is_ok() {
            return Err("native runtime receipt was replayed".into());
        }
        for command in ["file.save", "file.newBin", "edit.undo", "edit.redo", "prefs.set"] {
            if s.execute(command, json!({})).is_ok() {
                return Err("journal confirmation released native editing".into());
            }
        }
        if !Arc::ptr_eq(&graph, &s.project)
            || s.revision != revision
            || s.saved_revision != saved_revision
            || serde_json::to_value(&s.state)? != editor
            || !s.is_display_only()
        {
            return Err("runtime handoff changed original native state".into());
        }
        let result = match outcome {
            Ok(status) => json!({"accepted":true,"status":status}),
            Err(error) => json!({"accepted":false,"error":error.to_string()}),
        };
        s.shutdown();
        println!(
            "{}",
            serde_json::to_string(&json!({"status":"actual-native-runtime-result","pid":std::process::id(),
            "result":result,"editingPaused":true,"workerStarted":false,"providerUsed":false,"guiOpened":false,
            "revision":revision,"savedRevision":saved_revision,"graphAndEditorUnchanged":true}))?
        );
        return Ok(());
    }
    let value = json!({"status":"prepared-actual-native-cold-Save-candidate","fence":fence,"revision":revision,"savedRevision":saved_revision,"sequence":s.state.active_sequence.map(|id|id.0),"path":s.path,"editingPaused":true,"blockedNativeEditDoors":5,"workerStarted":false,"providerUsed":false,"guiOpened":false});
    s.shutdown();
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
