//! Exclusive trusted cold Save entry before default paths, file logging, device
//! registration or UI construction. The same original Session stays alive and
//! held after the five-receipt exchange. A private application preparation may
//! freshly observe that same owner before its exact Stop. Original full
//! Root/shared/DataRoot/application closure remains required before activation
//! or worker release.
use crate::{args::Launch, startup_session::StartupSession};
use filmcraft_engine::cold_save_startup::confirm_held_cold_save_checkpoint_archive_intent;
use filmcraft_engine::cold_save_startup::confirm_held_cold_save_checkpoint_publication;
use filmcraft_engine::cold_save_startup::prepare_held_cold_save_application_checkpoint_archive;
use filmcraft_engine::cold_save_startup::prepare_held_cold_save_current_image_release;
use filmcraft_engine::cold_save_startup::publish_held_cold_save_application_checkpoint;
use filmcraft_engine::cold_save_startup::{
    ColdSavePipes, ColdSaveTransport, confirm_held_cold_save_application_intent, prepare_held_cold_save_application, prepare_held_cold_save_application_root,
    run_held_cold_save_startup,
};
use filmcraft_engine::cold_save_startup::{
    complete_held_cold_save_current_image_release, confirm_held_cold_save_application_settlement_intent,
    confirm_held_cold_save_application_settlement_publication, prepare_held_cold_save_application_settlement,
};
use filmcraft_engine::cold_save_startup::{confirm_held_cold_save_application_native_presentation, confirm_held_cold_save_application_records_intent};
use filmcraft_engine::cold_save_startup::{confirm_held_cold_save_application_root_intent, confirm_held_cold_save_application_root_publication};
use filmcraft_engine::cold_save_startup::{prepare_held_cold_save_application_checkpoint, prepare_held_cold_save_application_records};
use filmcraft_engine::{EngineError, Result as EngineResult, Session};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
};

fn error(message: &str) -> EngineError {
    EngineError::Other(format!("managed cold Save: {message}"))
}

/// Validate the first description produced by the ACTUAL shared native owner,
/// before forwarding it or accepting any receipt. Runtime preparation is
/// in-memory only; a mismatched CLI directory therefore starts no worker or
/// canonical writer. This does not reconstruct ownership from a description.
struct DirectoryTransport<'a, T> {
    directory: &'a Path,
    inner: &'a mut T,
    checked: bool,
}
impl<T: ColdSaveTransport> ColdSaveTransport for DirectoryTransport<'_, T> {
    fn send(&mut self, packet: &Value) -> EngineResult<()> {
        if !self.checked {
            if packet["status"] != "actual-native-runtime-reserved"
                || packet["pid"] != std::process::id()
                || packet["runtime"]["preparation"]["session"]["dataDirectory"].as_str().is_none_or(|p| Path::new(p) != self.directory)
            {
                return Err(error("explicit data directory differs from the same actual original runtime"));
            }
            self.checked = true;
        }
        self.inner.send(packet)
    }
    fn receipt(&mut self) -> EngineResult<Value> {
        if !self.checked {
            return Err(error("the actual runtime directory has not been confirmed"));
        }
        self.inner.receipt()
    }
}
fn validate_request(value: &Value, native_session: &str, private_input: &Value, kind: &str) -> EngineResult<()> {
    if serde_json::to_vec(value).map_err(|e| EngineError::Other(e.to_string()))?.len() > 16384 {
        return Err(error("bounded exact private held-stop required"));
    }
    let Some(fields) = value
        .as_object()
        .filter(|fields| fields.len() == 4 && fields.keys().all(|key| matches!(key.as_str(), "kind" | "version" | "nativeSession" | "privateInput")))
    else {
        return Err(error("closed exact private held-stop fields required"));
    };
    if fields.get("kind").and_then(Value::as_str) != Some(kind)
        || fields.get("version").and_then(Value::as_u64) != Some(1)
        || fields.get("nativeSession").and_then(Value::as_str) != Some(native_session)
        || fields.get("privateInput") != Some(private_input)
    {
        return Err(error("held-stop belongs to another original native continuation"));
    }
    Ok(())
}
fn validate_stop(value: &Value, native_session: &str, private_input: &Value) -> EngineResult<()> {
    validate_request(value, native_session, private_input, "poema-original-Save-native-held-stop")
}
fn validate_launch(launch: &Launch) -> Result<Option<Value>, String> {
    let Some(raw) = &launch.cold_save_recovery else { return Ok(None) };
    if raw.is_empty()
        || raw.len() > 16384
        || launch.control_port.is_some()
        || launch.control_input_only
        || !launch.files.is_empty()
        || launch.demo
        || !launch.startup_flag
        || launch.recover != Some(false)
        || launch.graphics.is_some()
        || launch.poema_composer.is_some()
        || launch.poema_save_root.is_some()
        || launch.workspace_update.is_some()
        || launch.identity_recovery.is_some()
        || launch.project_recovery.is_some()
        || launch.project_create.is_some()
    {
        return Err("exclusive bounded cold Save launch with no ordinary fallback required".into());
    }
    let directory = launch.data_dir.as_deref().filter(|p| p.is_absolute()).ok_or("explicit absolute original native data directory required")?;
    if directory.components().any(|p| !matches!(p, std::path::Component::RootDir | std::path::Component::Normal(_)))
        || std::fs::canonicalize(directory).map_err(|e| e.to_string())? != directory
    {
        return Err("original native data directory must remain literal and canonical".into());
    }
    serde_json::from_str(raw).map(Some).map_err(|e| e.to_string())
}
// Closed original private-pipe continuation after checkpoint preparation.
// The same actual Session remains in StartupSession throughout. Successful
// metadata settlement still cannot enter ordinary desktop initialization.
fn continue_current_image<T: ColdSaveTransport>(
    session: &mut Session,
    pipes: &mut T,
    mut next: Value,
    native_session: &str,
    private_input: &Value,
    released: &mut bool,
) -> Result<Value, String> {
    if next["kind"] != "poema-original-Save-native-current-image-complete" {
        return Ok(next);
    }
    // The original engine validates the closed request INCLUDING its durable
    // same-journal image intent; the old four-field Stop validator stays strict.
    let completed = complete_held_cold_save_current_image_release(session, &next).map_err(|e| e.to_string())?;
    *released = true;
    pipes.send(&completed).map_err(|e| e.to_string())?;
    next = pipes.receipt().map_err(|e| e.to_string())?;
    if next["kind"] != "poema-original-Save-native-application-settlement-prepare" {
        return Ok(next);
    }
    let prepared = prepare_held_cold_save_application_settlement(session, &next).map_err(|e| e.to_string())?;
    pipes.send(&prepared).map_err(|e| e.to_string())?;
    next = pipes.receipt().map_err(|e| e.to_string())?;
    if next["kind"] != "poema-original-Save-native-application-settlement-intent-receipt" {
        return Ok(next);
    }
    let intent = confirm_held_cold_save_application_settlement_intent(session, &next).map_err(|e| e.to_string())?;
    pipes.send(&intent).map_err(|e| e.to_string())?;
    next = pipes.receipt().map_err(|e| e.to_string())?;
    if next["kind"] != "poema-original-Save-native-application-settlement-observation-receipt" {
        return Ok(next);
    }
    let observed = confirm_held_cold_save_application_settlement_publication(session, &next).map_err(|e| e.to_string())?;
    pipes.send(&observed).map_err(|e| e.to_string())?;
    pipes.receipt().map_err(|e| e.to_string())
}

fn run_with_io(launch: &Launch, reader: impl BufRead, writer: impl Write) -> Result<bool, String> {
    let Some(reference) = validate_launch(launch)? else { return Ok(false) };
    let directory = launch.data_dir.as_deref().ok_or("explicit original native directory required")?;
    let mut owner = StartupSession::new(Session::default());
    let mut pipes = ColdSavePipes::new(reader, writer);
    // Keep the ACTUAL Session alive across the complete exchange. Every return
    // and callback unwind joins/stops its original worker before the cold pause
    // owner can be dropped. No error enters normal desktop startup.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(String, bool), String> {
        let session = owner.session_mut()?;
        session.open_save_recovery(&reference).map_err(|e| e.to_string())?;
        let revision = session.revision;
        let saved_revision = session.saved_revision;
        let handoff =
            run_held_cold_save_startup(session, &mut DirectoryTransport { directory, inner: &mut pipes, checked: false }).map_err(|e| e.to_string())?;
        let native_session = handoff.plan["preparation"]["nativeSession"].as_str().ok_or("actual native epoch missing")?.to_owned();
        let private_input = handoff.plan["preparation"]["privateInput"].clone();
        let result = match handoff.outcome {
            Ok(status) => json!({"accepted":true,"status":status}),
            Err(e) => json!({"accepted":false,"error":e.to_string()}),
        };
        pipes
            .send(&json!({"status":"actual-native-canonical-result","pid":std::process::id(),
            "result":result,"publication":handoff.publication,"serviceBindings":handoff.service_bindings,
            "bindingObservationRequired":true,"bindingObservationConfirmed":handoff.binding_observation_confirmed,
            "editingPaused":true,"workerReleased":false,"directSynchronizationHeld":true,"graphAndEditorUnchanged":true,
            "revision":revision,"savedRevision":saved_revision,"providerUsed":false,"guiOpened":false}))
            .map_err(|e| e.to_string())?;
        // This is neither activation nor a successful return into ordinary UI.
        // The parent keeps its private pipe open while the same held native
        // owner awaits original closure/reconciliation. One exact preparation
        // request can freshly observe that SAME owner after source replacement.
        // It neither releases the worker nor enters ordinary desktop startup.
        // The acknowledged intent may prepare one same-owner Root plan. Stop
        // follows; EOF, repeated/unknown requests and parent loss
        // fail closed without retry or a reconstructed Session.
        let mut worker_released = false;
        let request = pipes.receipt().map_err(|e| e.to_string())?;
        let stop = if request["kind"] == "poema-original-Save-native-application-prepare" {
            validate_request(&request, &native_session, &private_input, "poema-original-Save-native-application-prepare").map_err(|e| e.to_string())?;
            let preparation = prepare_held_cold_save_application(session).map_err(|e| e.to_string())?;
            pipes.send(&preparation).map_err(|e| e.to_string())?;
            let next = pipes.receipt().map_err(|e| e.to_string())?;
            if next["kind"] == "poema-original-Save-native-application-intent-receipt" {
                let confirmed = confirm_held_cold_save_application_intent(session, &next).map_err(|e| e.to_string())?;
                pipes.send(&confirmed).map_err(|e| e.to_string())?;
                let next = pipes.receipt().map_err(|e| e.to_string())?;
                if next["kind"] == "poema-original-Save-native-application-root-prepare" {
                    let prepared = prepare_held_cold_save_application_root(session, &next).map_err(|e| e.to_string())?;
                    pipes.send(&prepared).map_err(|e| e.to_string())?;
                    let next = pipes.receipt().map_err(|e| e.to_string())?;
                    if next["kind"] == "poema-original-Save-native-application-root-intent-receipt" {
                        let confirmed = confirm_held_cold_save_application_root_intent(session, &next).map_err(|e| e.to_string())?;
                        pipes.send(&confirmed).map_err(|e| e.to_string())?;
                        let next = pipes.receipt().map_err(|e| e.to_string())?;
                        if next["kind"] == "poema-original-Save-native-application-root-observation-receipt" {
                            let observed = confirm_held_cold_save_application_root_publication(session, &next).map_err(|e| e.to_string())?;
                            pipes.send(&observed).map_err(|e| e.to_string())?;
                            let next = pipes.receipt().map_err(|e| e.to_string())?;
                            if next["kind"] == "poema-original-Save-native-application-records-prepare" {
                                let records = prepare_held_cold_save_application_records(session, &next).map_err(|e| e.to_string())?;
                                pipes.send(&records).map_err(|e| e.to_string())?;
                                let next = pipes.receipt().map_err(|e| e.to_string())?;
                                if next["kind"] == "poema-original-Save-native-application-records-intent-receipt" {
                                    let confirmed = confirm_held_cold_save_application_records_intent(session, &next).map_err(|e| e.to_string())?;
                                    pipes.send(&confirmed).map_err(|e| e.to_string())?;
                                    let next = pipes.receipt().map_err(|e| e.to_string())?;
                                    if next["kind"] == "poema-original-Save-native-application-records-observation-receipt" {
                                        let observed = confirm_held_cold_save_application_native_presentation(session, &next).map_err(|e| e.to_string())?;
                                        pipes.send(&observed).map_err(|e| e.to_string())?;
                                        let next = pipes.receipt().map_err(|e| e.to_string())?;
                                        if next["kind"] == "poema-original-Save-native-application-checkpoint-prepare" {
                                            validate_request(
                                                &next,
                                                &native_session,
                                                &private_input,
                                                "poema-original-Save-native-application-checkpoint-prepare",
                                            )
                                            .map_err(|e| e.to_string())?;
                                            let checkpoint = prepare_held_cold_save_application_checkpoint(session, &next).map_err(|e| e.to_string())?;
                                            pipes.send(&checkpoint).map_err(|e| e.to_string())?;
                                            let next = pipes.receipt().map_err(|e| e.to_string())?;
                                            if next["kind"] == "poema-original-Save-native-application-checkpoint-archive-prepare" {
                                                validate_request(
                                                    &next,
                                                    &native_session,
                                                    &private_input,
                                                    "poema-original-Save-native-application-checkpoint-archive-prepare",
                                                )
                                                .map_err(|e| e.to_string())?;
                                                let archive =
                                                    prepare_held_cold_save_application_checkpoint_archive(session, &next).map_err(|e| e.to_string())?;
                                                pipes.send(&archive).map_err(|e| e.to_string())?;
                                                let next = pipes.receipt().map_err(|e| e.to_string())?;
                                                if next["kind"] == "poema-original-Save-native-checkpoint-archive-intent-receipt" {
                                                    let acknowledgement =
                                                        confirm_held_cold_save_checkpoint_archive_intent(session, &next).map_err(|e| e.to_string())?;
                                                    pipes.send(&acknowledgement).map_err(|e| e.to_string())?;
                                                    let next = pipes.receipt().map_err(|e| e.to_string())?;
                                                    if next["kind"] == "poema-original-Save-native-application-checkpoint-archive-publish" {
                                                        validate_request(
                                                            &next,
                                                            &native_session,
                                                            &private_input,
                                                            "poema-original-Save-native-application-checkpoint-archive-publish",
                                                        )
                                                        .map_err(|e| e.to_string())?;
                                                        let publication =
                                                            publish_held_cold_save_application_checkpoint(session, &next).map_err(|e| e.to_string())?;
                                                        pipes.send(&publication).map_err(|e| e.to_string())?;
                                                        let next = pipes.receipt().map_err(|e| e.to_string())?;
                                                        if next["kind"] == "poema-original-Save-native-checkpoint-archive-observation-receipt" {
                                                            let confirmation =
                                                                confirm_held_cold_save_checkpoint_publication(session, &next).map_err(|e| e.to_string())?;
                                                            pipes.send(&confirmation).map_err(|e| e.to_string())?;
                                                            let next = pipes.receipt().map_err(|e| e.to_string())?;
                                                            if next["kind"] == "poema-original-Save-native-current-image-prepare" {
                                                                validate_request(
                                                                    &next,
                                                                    &native_session,
                                                                    &private_input,
                                                                    "poema-original-Save-native-current-image-prepare",
                                                                )
                                                                .map_err(|e| e.to_string())?;
                                                                let preparation =
                                                                    prepare_held_cold_save_current_image_release(session, &next).map_err(|e| e.to_string())?;
                                                                pipes.send(&preparation).map_err(|e| e.to_string())?;
                                                                let next = pipes.receipt().map_err(|e| e.to_string())?;
                                                                continue_current_image(
                                                                    session,
                                                                    &mut pipes,
                                                                    next,
                                                                    &native_session,
                                                                    &private_input,
                                                                    &mut worker_released,
                                                                )?
                                                            } else {
                                                                next
                                                            }
                                                        } else {
                                                            next
                                                        }
                                                    } else {
                                                        next
                                                    }
                                                } else {
                                                    next
                                                }
                                            } else {
                                                next
                                            }
                                        } else {
                                            next
                                        }
                                    } else {
                                        next
                                    }
                                } else {
                                    next
                                }
                            } else {
                                next
                            }
                        } else {
                            next
                        }
                    } else {
                        next
                    }
                } else {
                    next
                }
            } else {
                next
            }
        } else {
            request
        };
        validate_stop(&stop, &native_session, &private_input).map_err(|e| e.to_string())?;
        Ok((native_session, worker_released))
    }))
    .unwrap_or_else(|_| Err("cold Save startup failed unexpectedly; retain original state without replay".into()));
    owner.stop()?;
    let (native_session, worker_released) = outcome?;
    pipes
        .send(&json!({"status":"actual-native-cold-held-stopped","pid":std::process::id(),
        "nativeSession":native_session,"stopped":true,"editingPaused":true,"workerReleased":worker_released,
        "providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
        .map_err(|e| e.to_string())?;
    Ok(true)
}
pub fn run(launch: &Launch) -> Result<bool, String> {
    if launch.cold_save_recovery.is_none() {
        return Ok(false);
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_with_io(launch, stdin.lock(), stdout.lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn current_image_preparation_is_closed_bound_to_the_same_pipe_and_cannot_substitute_for_stop() {
        let input = json!({"path":"/generated/private/input.json","sha256":"a".repeat(64),"bytes":10});
        let kind = "poema-original-Save-native-current-image-prepare";
        let request = json!({"kind":kind,"version":1,"nativeSession":"actual-epoch","privateInput":input});
        validate_request(&request, "actual-epoch", &input, kind).unwrap();
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        for key in ["nativeSession", "privateInput", "version"] {
            let mut wrong = request.clone();
            wrong[key] = Value::Null;
            assert!(validate_request(&wrong, "actual-epoch", &input, kind).is_err());
        }
        for key in ["release", "grant", "startupAuthority"] {
            let mut wrong = request.clone();
            wrong[key] = json!(true);
            assert!(validate_request(&wrong, "actual-epoch", &input, kind).is_err());
        }
    }
    #[test]
    fn archive_intent_receipt_cannot_substitute_for_stop_or_preparation_requests() {
        let input = json!({"path":"/generated/private/input.json","sha256":"a".repeat(64),"bytes":10});
        let receipt = json!({"kind":"poema-original-Save-native-checkpoint-archive-intent-receipt","version":1});
        assert!(validate_stop(&receipt, "actual-epoch", &input).is_err());
        for kind in ["poema-original-Save-native-application-checkpoint-prepare", "poema-original-Save-native-application-checkpoint-archive-prepare"] {
            assert!(validate_request(&receipt, "actual-epoch", &input, kind).is_err());
        }
    }
    #[test]
    fn archive_preparation_request_cannot_substitute_for_stop_or_checkpoint_byte_preparation() {
        let input = json!({"path":"/generated/private/input.json","sha256":"a".repeat(64),"bytes":10});
        let kind = "poema-original-Save-native-application-checkpoint-archive-prepare";
        let request = json!({"kind":kind,"version":1,"nativeSession":"actual-epoch","privateInput":input});
        validate_request(&request, "actual-epoch", &input, kind).unwrap();
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        assert!(validate_request(&request, "actual-epoch", &input, "poema-original-Save-native-application-checkpoint-prepare").is_err());
        let mut extra = request.clone();
        extra["writeAuthority"] = json!(true);
        assert!(validate_request(&extra, "actual-epoch", &input, kind).is_err());
        let mut wrong = request;
        wrong["privateInput"]["sha256"] = json!("b".repeat(64));
        assert!(validate_request(&wrong, "actual-epoch", &input, kind).is_err());
    }
    #[test]
    fn checkpoint_request_is_closed_and_cannot_substitute_for_stop_or_prior_application_phases() {
        let input = json!({"path":"/generated/private/input.json","sha256":"a".repeat(64),"bytes":10});
        let kind = "poema-original-Save-native-application-checkpoint-prepare";
        let request = json!({"kind":kind,"version":1,"nativeSession":"actual-epoch","privateInput":input});
        validate_request(&request, "actual-epoch", &input, kind).unwrap();
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        for prior in [
            "poema-original-Save-native-application-prepare",
            "poema-original-Save-native-application-root-prepare",
            "poema-original-Save-native-application-records-prepare",
        ] {
            assert!(validate_request(&request, "actual-epoch", &input, prior).is_err());
        }
        let mut crossed = request.clone();
        crossed["nativeSession"] = json!("foreign");
        assert!(validate_request(&crossed, "actual-epoch", &input, kind).is_err());
        let mut extra = request;
        extra["release"] = json!(true);
        assert!(validate_request(&extra, "actual-epoch", &input, kind).is_err());
    }
    #[test]
    fn application_records_request_cannot_substitute_for_stop_or_prior_root_preparation() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        let request = json!({"kind":"poema-original-Save-native-application-records-prepare","version":1,"nativeSession":"actual-epoch","privateInput":input,"started":"actual-child-start"});
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        assert!(validate_request(&request, "actual-epoch", &input, "poema-original-Save-native-application-root-prepare").is_err());
    }
    #[test]
    fn application_records_receipts_cannot_substitute_for_stop_or_an_earlier_native_request() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        for kind in ["poema-original-Save-native-application-records-intent-receipt", "poema-original-Save-native-application-records-observation-receipt"] {
            let value = json!({"kind":kind,"version":1,"event":{},"anchor":{},"head":{}});
            assert!(validate_stop(&value, "actual-epoch", &input).is_err());
            for request in ["poema-original-Save-native-application-prepare", "poema-original-Save-native-application-root-prepare"] {
                assert!(validate_request(&value, "actual-epoch", &input, request).is_err());
            }
        }
    }
    #[test]
    fn application_root_request_cannot_substitute_for_held_stop_or_prior_acknowledgement() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        let request = json!({"kind":"poema-original-Save-native-application-root-prepare","version":1,"nativeSession":"actual-epoch","privateInput":input,"binary":"/generated/native","sha256":"b".repeat(64)});
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        assert!(validate_request(&request, "actual-epoch", &input, "poema-original-Save-native-application-prepare").is_err());
    }
    #[test]
    fn root_intent_and_observation_receipts_cannot_substitute_for_stop_or_root_preparation() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        for kind in ["poema-original-Save-native-application-root-intent-receipt", "poema-original-Save-native-application-root-observation-receipt"] {
            let value = json!({"kind":kind,"version":1,"event":{},"anchor":{},"head":{}});
            assert!(validate_stop(&value, "actual-epoch", &input).is_err());
            assert!(validate_request(&value, "actual-epoch", &input, "poema-original-Save-native-application-root-prepare").is_err());
        }
    }
    fn launch(args: &[&str]) -> Launch {
        let crate::args::Cli::Run(value) = crate::args::parse(None, args.iter().map(std::ffi::OsString::from)).unwrap() else { panic!("run required") };
        *value
    }
    #[test]
    fn ordinary_launch_never_reads_cold_input_or_writes_pipe_bytes() {
        let launch = launch(&["--empty"]);
        let mut output = Vec::new();
        assert!(!run_with_io(&launch, Cursor::new(b"malformed"), &mut output).unwrap());
        assert!(output.is_empty());
    }
    #[test]
    fn cold_mode_cannot_fall_through_ordinary_managed_preparation() {
        let launch = launch(&["--cold-save-recovery", "{}", "--data-dir", "/generated"]);
        assert!(crate::managed_startup::prepare(&launch, |_| 0).err().unwrap().contains("no ordinary managed startup fallback"));
    }
    #[test]
    fn exact_stop_is_bound_to_the_same_native_epoch_input_and_closed_shape() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        let good = json!({"kind":"poema-original-Save-native-held-stop","version":1,"nativeSession":"actual-epoch","privateInput":input});
        validate_stop(&good, "actual-epoch", &input).unwrap();
        for field in ["kind", "version", "nativeSession", "privateInput"] {
            let mut bad = good.clone();
            bad[field] = Value::Null;
            assert!(validate_stop(&bad, "actual-epoch", &input).is_err());
        }
        let mut bad = good.clone();
        bad["writeAuthority"] = json!(true);
        assert!(validate_stop(&bad, "actual-epoch", &input).is_err());
        assert!(validate_stop(&good, "other-epoch", &input).is_err());
        let mut oversized = good;
        oversized["padding"] = json!("x".repeat(16385));
        assert!(validate_stop(&oversized, "actual-epoch", &input).is_err());
    }
    #[test]
    fn application_prepare_request_is_exact_and_cannot_substitute_for_held_stop() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        let kind = "poema-original-Save-native-application-prepare";
        let request = json!({"kind":kind,"version":1,"nativeSession":"actual-epoch","privateInput":input});
        validate_request(&request, "actual-epoch", &input, kind).unwrap();
        assert!(validate_stop(&request, "actual-epoch", &input).is_err());
        assert!(validate_request(&request, "other-epoch", &input, kind).is_err());
        assert!(validate_request(&request, "actual-epoch", &json!({}), kind).is_err());
        for key in ["kind", "version", "nativeSession", "privateInput"] {
            let mut changed = request.clone();
            changed[key] = Value::Null;
            assert!(validate_request(&changed, "actual-epoch", &input, kind).is_err());
        }
        for key in ["release", "grant", "applicationClosure", "owner"] {
            let mut widened = request.clone();
            widened[key] = json!(true);
            assert!(validate_request(&widened, "actual-epoch", &input, kind).is_err());
        }
    }
    #[test]
    fn application_intent_receipt_cannot_substitute_for_preparation_or_stop() {
        let input = json!({"path":"/private/input.json","sha256":"a".repeat(64),"bytes":1});
        let receipt = json!({"kind":"poema-original-Save-native-application-intent-receipt","version":1,"event":{},"anchor":{},"head":{}});
        assert!(validate_stop(&receipt, "actual-epoch", &input).is_err());
        assert!(validate_request(&receipt, "actual-epoch", &input, "poema-original-Save-native-application-prepare").is_err());
    }
    struct Sink {
        packets: Vec<Value>,
        receipts: usize,
    }
    impl ColdSaveTransport for Sink {
        fn send(&mut self, value: &Value) -> EngineResult<()> {
            self.packets.push(value.clone());
            Ok(())
        }
        fn receipt(&mut self) -> EngineResult<Value> {
            self.receipts += 1;
            Ok(Value::Null)
        }
    }
    #[test]
    fn actual_runtime_directory_mismatch_refuses_before_forwarding_or_any_receipt() {
        for packet in [
            json!({}),
            json!({"status":"actual-native-runtime-reserved","pid":std::process::id(),"runtime":{"preparation":{"session":{"dataDirectory":"/other"}}}}),
        ] {
            let mut sink = Sink { packets: vec![], receipts: 0 };
            let mut transport = DirectoryTransport { directory: Path::new("/original"), inner: &mut sink, checked: false };
            assert!(transport.send(&packet).is_err());
            assert!(transport.receipt().is_err());
            assert!(sink.packets.is_empty());
            assert_eq!(sink.receipts, 0);
        }
    }
    #[test]
    fn exact_actual_directory_forwards_the_original_packet_without_replacement() {
        let first =
            json!({"status":"actual-native-runtime-reserved","pid":std::process::id(),"runtime":{"preparation":{"session":{"dataDirectory":"/original"}}}});
        let next = json!({"status":"actual-native-worker-created","pid":std::process::id()});
        let mut sink = Sink { packets: vec![], receipts: 0 };
        let mut transport = DirectoryTransport { directory: Path::new("/original"), inner: &mut sink, checked: false };
        transport.send(&first).unwrap();
        transport.receipt().unwrap();
        transport.send(&next).unwrap();
        assert_eq!(sink.packets, vec![first, next]);
        assert_eq!(sink.receipts, 1);
    }
    #[test]
    fn malformed_private_input_keeps_the_existing_directory_without_bootstrap_or_log_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!("managed-cold-input-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let directory = std::fs::canonicalize(directory).unwrap();
        std::fs::write(directory.join("sentinel.keep"), b"later user bytes").unwrap();
        let directory_text = directory.to_str().unwrap();
        let launch = launch(&["--cold-save-recovery", "{}", "--data-dir", directory_text]);
        let mut output = Vec::new();
        assert!(run_with_io(&launch, Cursor::new(b""), &mut output).is_err());
        assert!(output.is_empty());
        assert_eq!(std::fs::read(directory.join("sentinel.keep")).unwrap(), b"later user bytes");
        let names: Vec<_> = std::fs::read_dir(&directory).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("sentinel.keep")]);
    }
}
