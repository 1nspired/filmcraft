//! Trusted parent-owned cold Save startup, shared by native entrypoints and
//! generated probes. This is not an agent command or an activation API.
//!
//! The caller retains the same actual Session and the original manager retains
//! the actual journal/input handles. Wire values describe those owners; they
//! never reconstruct them. Successful completion still leaves the Session held.
use crate::{EngineError, Result, Session};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};

const RECEIPT_LIMIT: u64 = 512 * 1024;
const OUTPUT_LIMIT: usize = 1024 * 1024;

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("cold Save startup: {message}"))
}

/// Private parent-owned transport only. Implementations must not retry an
/// uncertain write or replace the manager, native actor or input on failure.
pub trait ColdSaveTransport {
    fn send(&mut self, packet: &Value) -> Result<()>;
    fn receipt(&mut self) -> Result<Value>;
}

/// Bounded newline JSON over the original child pipes. No file, socket, default
/// data directory or alternate transport is opened here. A failed exchange
/// poisons this transport, including partial writes and flush failures.
pub struct ColdSavePipes<R, W> {
    reader: R,
    writer: W,
    output_bytes: usize,
    failed: bool,
}

impl<R: BufRead, W: Write> ColdSavePipes<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer, output_bytes: 0, failed: false }
    }
}

impl<R: BufRead, W: Write> ColdSaveTransport for ColdSavePipes<R, W> {
    fn send(&mut self, packet: &Value) -> Result<()> {
        if self.failed {
            return Err(error("transport already failed; reconcile without replay"));
        }
        self.failed = true;
        let mut raw = serde_json::to_vec(packet).map_err(error)?;
        raw.push(b'\n');
        let total =
            self.output_bytes.checked_add(raw.len()).filter(|n| *n <= OUTPUT_LIMIT).ok_or_else(|| error("native packet transcript exceeds its bound"))?;
        self.writer.write_all(&raw).map_err(error)?;
        self.writer.flush().map_err(error)?;
        self.output_bytes = total;
        self.failed = false;
        Ok(())
    }

    fn receipt(&mut self) -> Result<Value> {
        if self.failed {
            return Err(error("transport already failed; reconcile without replay"));
        }
        self.failed = true;
        let mut raw = Vec::new();
        (&mut self.reader).take(RECEIPT_LIMIT + 1).read_until(b'\n', &mut raw).map_err(error)?;
        if raw.is_empty() || raw.len() as u64 > RECEIPT_LIMIT || raw.last() != Some(&b'\n') {
            return Err(error("one bounded newline-terminated original receipt required"));
        }
        let receipt = serde_json::from_slice(&raw).map_err(error)?;
        self.failed = false;
        Ok(receipt)
    }
}

/// Descriptions retained for reporting/reconciliation, never an ownership or
/// worker-release ticket. A refused receipt can follow a real publication.
/// The caller must preserve that distinction instead of replaying the effect.
pub struct HeldColdSaveStartup {
    pub outcome: Result<Value>,
    pub plan: Value,
    pub publication: Value,
    pub service_bindings: Value,
    pub binding_observation_confirmed: bool,
}

/// Fresh observation of the SAME held Session after its parent has completed
/// the original source handoff and file verification. This is preparation for
/// full application reconciliation, never an activation or release receipt.
/// The original Cold owner checks its actual basis, runtime, journal, canonical
/// publication, bindings and exact worker pointer before returning this value.
pub fn prepare_held_cold_save_application(session: &mut Session) -> Result<Value> {
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("actual original cold Save owner required for application preparation"))?;
    let runtime = &owner["runtime"];
    let preparation = &runtime["preparation"];
    let native_session = preparation["nativeSession"].as_str().ok_or_else(|| error("original cold native epoch required"))?;
    if owner["nativeSession"] != native_session
        || preparation["nativePid"] != std::process::id()
        || runtime["worker"]["nativePid"] != std::process::id()
        || runtime["worker"]["nativeSession"] != native_session
        || runtime["journalConfirmed"] != true
        || runtime["workerConfirmed"] != true
        || runtime["canonical"]["observationConfirmed"] != true
        || runtime["canonical"]["bindingObservationConfirmed"] != true
        || runtime["canonical"]["serviceBindings"]["phase"] != "bound"
        || runtime["canonical"]["serviceBindings"]["bound"] != true
        || runtime["workersMayResume"] != false
        || !session.is_display_only()
        || session.persistence.as_ref().is_none_or(|worker| !worker.startup_pending())
    {
        return Err(error("the same confirmed original runtime, canonical bindings and gated worker must remain held"));
    }
    let value = json!({"status":"actual-native-cold-application-prepared","pid":std::process::id(),
        "nativeSession":native_session,"privateInput":preparation["privateInput"],
        "projectPath":session.path,"revision":session.revision,"savedRevision":session.saved_revision,
        "owner":owner,"editingPaused":true,"directSynchronizationHeld":true,
        "workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false});
    crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::RetainPreparation(&value))
}

/// Confirms only a private original-journal intent against the same retained
/// Cold runtime and worker. This is not full application closure or admission.
pub fn confirm_held_cold_save_application_intent(session: &mut Session, receipt: &Value) -> Result<Value> {
    let confirmation =
        crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::ConfirmIntent(receipt))?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the actual cold application owner disappeared after intent confirmation"))?;
    Ok(json!({"status":"actual-native-cold-application-intent-confirmed","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"confirmation":confirmation,"owner":owner,
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":false,
        "providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Exact Root/composer plan from this SAME retained native process. It does
/// not publish a manifest or release the Session, worker or startup carrier.
pub fn prepare_held_cold_save_application_root(session: &mut Session, request: &Value) -> Result<Value> {
    let plan = crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::PrepareRoot(request))?;
    Ok(json!({"status":"actual-native-cold-application-root-prepared","pid":std::process::id(),
        "nativeSession":plan["nativeSession"],"plan":plan,"editingPaused":true,
        "workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Complete original activity/catalog/native presentation observations on the
/// SAME acknowledged Root owner. This publishes no metadata or settlement.
pub fn prepare_held_cold_save_application_records(session: &mut Session, request: &Value) -> Result<Value> {
    let plan =
        crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::PrepareRecords(request))?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original application records owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-application-records-prepared","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"plan":plan,"owner":owner,"editingPaused":true,
        "directSynchronizationHeld":true,"workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Encode the retained complete COW checkpoint on the SAME original held
/// persistence worker after the original Root/presentation acknowledgements.
/// This returns bounded byte descriptions only, with no file or startup grant.
pub fn prepare_held_cold_save_application_checkpoint(session: &mut Session, request: &Value) -> Result<Value> {
    let checkpoint =
        crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::PrepareCheckpoint(request))?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original held checkpoint owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-application-checkpoint-prepared","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"checkpoint":checkpoint,"owner":owner,"editingPaused":true,
        "directSynchronizationHeld":true,"workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Retain one exact absent archive child of the SAME original worker data
/// frame. This is a plan only; its JSON cannot create directory/write custody.
pub fn prepare_held_cold_save_application_checkpoint_archive(session: &mut Session, request: &Value) -> Result<Value> {
    let plan = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::PrepareCheckpointArchive(request),
    )?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original checkpoint archive owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-application-checkpoint-archive-prepared","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"plan":plan,"owner":owner,"editingPaused":true,"directSynchronizationHeld":true,
        "workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Observe only this SAME retained archive plan's sole original journal intent.
/// This confirms no archive creation, file publication or worker release.
pub fn confirm_held_cold_save_checkpoint_archive_intent(session: &mut Session, receipt: &Value) -> Result<Value> {
    let confirmation = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::ConfirmCheckpointArchiveIntent(receipt),
    )?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original archive intent owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-checkpoint-archive-intent-confirmed","pid":std::process::id(),"nativeSession":owner["nativeSession"],
        "confirmation":confirmation,"owner":owner,"editingPaused":true,"directSynchronizationHeld":true,
        "workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Publish the exact captured current-image bytes through the SAME original
/// held worker. This does not select a restart image or release the application.
pub fn publish_held_cold_save_application_checkpoint(session: &mut Session, request: &Value) -> Result<Value> {
    let publication = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::PublishCheckpointArchive(request),
    )?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original checkpoint publication owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-application-checkpoint-published","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"publication":publication,"owner":owner,"editingPaused":true,
        "directSynchronizationHeld":true,"workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Acknowledge the same original journal's complete publication observation.
/// The original worker stays held; restart/admission remain separate.
pub fn confirm_held_cold_save_checkpoint_publication(session: &mut Session, receipt: &Value) -> Result<Value> {
    let confirmation = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::ConfirmCheckpointPublication(receipt),
    )?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original checkpoint publication owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-checkpoint-archive-publication-confirmed","pid":std::process::id(),"nativeSession":owner["nativeSession"],
        "confirmation":confirmation,"owner":owner,"editingPaused":true,"directSynchronizationHeld":true,
        "workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Consume the actual acknowledged archive into a same-worker preparation.
/// Editing, synchronization and persistence remain held. This JSON cannot
/// reconstruct its owner or authorize Prepare/Resume or desktop admission.
pub fn prepare_held_cold_save_current_image_release(session: &mut Session, request: &Value) -> Result<Value> {
    let preparation = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::PrepareCurrentImageRelease(request),
    )?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the original current-image preparation owner disappeared"))?;
    Ok(json!({"status":"actual-native-cold-current-image-prepared","pid":std::process::id(),
        "nativeSession":owner["nativeSession"],"preparation":preparation,"owner":owner,
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":false,
        "providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}

/// Complete the SAME original retained checkpoint through its real worker.
/// This trusted engine continuation does not settle the original catalog or
/// admit an editor. It cannot construct a Cold owner from the request.
pub fn complete_held_cold_save_current_image_release(session: &mut Session, request: &Value) -> Result<Value> {
    let completion = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::CompleteCurrentImageRelease(request),
    )?;
    Ok(current_image_completion_response(completion))
}

/// Observe actual retained completion without invoking a gated pre-Resume
/// current/status reader. No receipt or admission is reconstructed from JSON.
pub fn inspect_held_cold_save_current_image_release(session: &mut Session) -> Result<Value> {
    let completion = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::InspectCompletedCurrentImageRelease,
    )?;
    Ok(current_image_completion_response(completion))
}

fn current_image_completion_response(completion: Value) -> Value {
    json!({"status":"actual-native-cold-current-image-completed","pid":std::process::id(),
        "nativeSession":completion["nativeSession"],"completion":completion,
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":true,
        "providerUsed":false,"guiOpened":false,"fullApplicationClosure":false})
}

/// Continue the SAME original completed owner; old gated status readers are
/// deliberately absent from these post-Resume packet builders.
pub fn prepare_held_cold_save_application_settlement(session: &mut Session, request: &Value) -> Result<Value> {
    let preparation =
        crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::PrepareSettlement(request))?;
    Ok(settlement_response("actual-native-cold-application-settlement-prepared", "preparation", preparation))
}
pub fn confirm_held_cold_save_application_settlement_intent(session: &mut Session, receipt: &Value) -> Result<Value> {
    let confirmation = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::ConfirmSettlementIntent(receipt),
    )?;
    Ok(settlement_response("actual-native-cold-application-settlement-intent-confirmed", "confirmation", confirmation))
}
pub fn confirm_held_cold_save_application_settlement_publication(session: &mut Session, receipt: &Value) -> Result<Value> {
    let confirmation = crate::project_save::operation::cold_application_action(
        session,
        crate::project_save::operation::ColdApplicationAction::ConfirmSettlementPublication(receipt),
    )?;
    Ok(settlement_response("actual-native-cold-application-settlement-observed", "confirmation", confirmation))
}
pub fn inspect_held_cold_save_application_settlement(session: &mut Session) -> Result<Value> {
    let confirmation =
        crate::project_save::operation::cold_application_action(session, crate::project_save::operation::ColdApplicationAction::InspectSettlement)?;
    Ok(settlement_response("actual-native-cold-application-settlement-observed", "confirmation", confirmation))
}
fn settlement_response(status: &str, field: &str, result: Value) -> Value {
    let mut packet = json!({"status":status,"pid":std::process::id(),"nativeSession":result["nativeSession"],
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":true,"acceptedSendRetained":true,
        "providerUsed":false,"guiOpened":false,"fullApplicationClosure":false});
    // Both keys are fixed internal literals. No request can create a packet key.
    if let Some(object) = packet.as_object_mut() {
        object.insert(field.into(), result);
    }
    packet
}

fn root_confirmation(session: &mut Session, receipt: &Value, publication: bool) -> Result<Value> {
    use crate::project_save::operation::{ColdApplicationAction, cold_application_action};
    let action = if publication { ColdApplicationAction::ConfirmRootPublication(receipt) } else { ColdApplicationAction::ConfirmRootIntent(receipt) };
    let confirmation = cold_application_action(session, action)?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the same original Root continuation owner disappeared"))?;
    Ok(json!({"status":if publication {"actual-native-cold-application-root-observed"} else {"actual-native-cold-application-root-intent-confirmed"},
        "pid":std::process::id(),"nativeSession":owner["nativeSession"],"confirmation":confirmation,"owner":owner,
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}
fn records_confirmation(session: &mut Session, receipt: &Value, publication: bool) -> Result<Value> {
    use crate::project_save::operation::{ColdApplicationAction, cold_application_action};
    let action = if publication { ColdApplicationAction::ConfirmNativePresentation(receipt) } else { ColdApplicationAction::ConfirmRecordsIntent(receipt) };
    let confirmation = cold_application_action(session, action)?;
    let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("the same original application records owner disappeared"))?;
    Ok(json!({"status":if publication {"actual-native-cold-application-records-observed"} else {"actual-native-cold-application-records-intent-confirmed"},
        "pid":std::process::id(),"nativeSession":owner["nativeSession"],"confirmation":confirmation,"owner":owner,
        "editingPaused":true,"directSynchronizationHeld":true,"workerReleased":false,"providerUsed":false,"guiOpened":false,"fullApplicationClosure":false}))
}
/// Confirms the sole original journal intent on this SAME retained Cold owner.
pub fn confirm_held_cold_save_application_records_intent(session: &mut Session, receipt: &Value) -> Result<Value> {
    records_confirmation(session, receipt, false)
}
/// Confirms this original publisher's exact native presentation generation.
/// No Save settlement, persistence release or StartupSession transfer occurs.
pub fn confirm_held_cold_save_application_native_presentation(session: &mut Session, receipt: &Value) -> Result<Value> {
    records_confirmation(session, receipt, true)
}
/// Acknowledges only the sole private original Root intent. No manifest write
/// or application/worker/startup release is authorized by this description.
pub fn confirm_held_cold_save_application_root_intent(session: &mut Session, receipt: &Value) -> Result<Value> {
    root_confirmation(session, receipt, false)
}
/// Observes the expected actual manifest replacement and sole original journal
/// successor on the SAME retained owner; every other original control is kept.
pub fn confirm_held_cold_save_application_root_publication(session: &mut Session, receipt: &Value) -> Result<Value> {
    root_confirmation(session, receipt, true)
}

/// Execute the original five-receipt handshake against the SAME already-open
/// cold Session. No reconstruction, new owner, baseline re-pin, document-path
/// adoption, startup activation or worker release occurs here. On transport or
/// original-owner failure the caller must stop/reconcile, never call a normal
/// startup fallback. In particular a successful result is NOT full application
/// closure and must not be passed into ordinary UI/log/bootstrap setup yet.
pub fn run_held_cold_save_startup(session: &mut Session, transport: &mut impl ColdSaveTransport) -> Result<HeldColdSaveStartup> {
    let fence = session.cold_save_recovery_status()?.ok_or_else(|| error("actual original cold Save owner required"))?;
    let retained_path = session.path.clone();
    let graph = session.project.clone();
    let revision = session.revision;
    let saved_revision = session.saved_revision;
    let editor = serde_json::to_value(&session.state).map_err(error)?;
    let pid = std::process::id();
    let runtime = session.prepare_cold_save_runtime()?;
    transport.send(&json!({"status":"actual-native-runtime-reserved","pid":pid,"fence":fence,"runtime":runtime}))?;
    session.confirm_cold_save_runtime(&transport.receipt()?)?;
    let worker = session.create_cold_save_persistence()?;
    transport.send(&json!({"status":"actual-native-worker-created","pid":pid,"owner":worker}))?;
    session.confirm_cold_save_persistence(&transport.receipt()?)?;
    let plan = session.prepare_cold_save_canonical()?;
    transport.send(&json!({"status":"actual-native-canonical-prepared","pid":pid,"plan":plan}))?;
    let mut outcome = session.confirm_cold_save_canonical_intent(&transport.receipt()?);
    let mut publication = Value::Null;
    let service_bindings;
    let mut binding_observation_confirmed = false;
    if outcome.is_ok() {
        publication = session.publish_cold_save_canonical()?;
        transport.send(&json!({"status":"actual-native-canonical-published","pid":pid,"publication":publication}))?;
        outcome = session.confirm_cold_save_canonical_observation(&transport.receipt()?);
    }
    if outcome.is_ok() {
        service_bindings = session.bind_cold_save_services()?;
        let directory = plan["effects"]["directory"]["path"].as_str().ok_or_else(|| error("actual canonical directory missing"))?;
        if service_bindings["bound"] != true
            || service_bindings["workersMayResume"] != false
            || session.prefs_path != Some(std::path::PathBuf::from(directory).join("preferences.json"))
        {
            return Err(error("original services did not bind to the exact canonical directory"));
        }
        let owner = session.cold_save_recovery_status()?.ok_or_else(|| error("binding lost the original owner"))?;
        if owner["runtime"]["canonical"]["serviceBindings"]["phase"] != "bound"
            || owner["runtime"]["canonical"]["serviceBindings"]["bound"] != true
            || owner["runtime"]["canonical"]["bindingObservationConfirmed"] != false
        {
            return Err(error("binding status differs from the retained native continuation"));
        }
        transport.send(&json!({"status":"actual-native-service-bindings-bound","pid":pid,"bindings":service_bindings,"owner":owner}))?;
        outcome = session.confirm_cold_save_service_bindings(&transport.receipt()?);
        binding_observation_confirmed = outcome.is_ok();
    } else {
        service_bindings = json!({"bound":false,"refused":true});
    }
    if session.path != retained_path
        || !std::sync::Arc::ptr_eq(&graph, &session.project)
        || session.revision != revision
        || session.saved_revision != saved_revision
        || serde_json::to_value(&session.state).map_err(error)? != editor
        || !session.is_display_only()
    {
        return Err(error("cold handshake changed the retained cut, editor, path or hold"));
    }
    if outcome.is_ok() {
        session.cold_save_recovery_status()?.ok_or_else(|| error("confirmed binding lost its original hold"))?;
    }
    Ok(HeldColdSaveStartup { outcome, plan, publication, service_bindings, binding_observation_confirmed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Arc};

    #[test]
    fn cold_save_pipes_read_exact_frames_and_flush_packets() {
        let mut output = Vec::new();
        let mut pipes = ColdSavePipes::new(Cursor::new(b"{\"first\":1}\n{\"second\":2}\n"), &mut output);
        assert_eq!(pipes.receipt().unwrap(), json!({"first":1}));
        assert_eq!(pipes.receipt().unwrap(), json!({"second":2}));
        pipes.send(&json!({"held":true})).unwrap();
        assert_eq!(output, b"{\"held\":true}\n");
    }

    #[test]
    fn cold_save_pipes_reject_eof_truncation_bad_json_and_oversize_without_retry() {
        for raw in [Vec::new(), b"{}".to_vec(), b"x\n{}\n".to_vec(), vec![b' '; RECEIPT_LIMIT as usize + 1]] {
            let mut output = Vec::new();
            let mut pipes = ColdSavePipes::new(Cursor::new(raw), &mut output);
            assert!(pipes.receipt().is_err());
            assert!(pipes.receipt().is_err());
            assert!(pipes.send(&json!({"retry":true})).is_err());
            assert!(output.is_empty());
        }
    }

    #[test]
    fn cold_save_pipes_bound_complete_transcript_before_writing_excess() {
        let mut output = Vec::new();
        let mut pipes = ColdSavePipes::new(Cursor::new(b"{}\n"), &mut output);
        let packet = json!("x".repeat(OUTPUT_LIMIT / 2));
        pipes.send(&packet).unwrap();
        assert!(pipes.send(&packet).is_err());
        assert!(pipes.receipt().is_err());
        assert_eq!(output.len(), OUTPUT_LIMIT / 2 + 3);
    }

    #[test]
    fn cold_save_pipes_partial_write_and_flush_failure_never_retry() {
        struct Fail {
            writes: usize,
            flushes: usize,
            flush_failure: bool,
        }
        impl Write for Fail {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writes += 1;
                if self.flush_failure {
                    return Ok(bytes.len());
                }
                if self.writes == 1 {
                    return Ok(1);
                }
                Err(std::io::Error::other("manager disconnected"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                Err(std::io::Error::other("manager disconnected"))
            }
        }
        for flush_failure in [false, true] {
            let mut writer = Fail { writes: 0, flushes: 0, flush_failure };
            let mut pipes = ColdSavePipes::new(Cursor::new(b"{}\n"), &mut writer);
            assert!(pipes.send(&json!({"held":true})).is_err());
            assert!(pipes.send(&json!({"retry":true})).is_err());
            assert!(pipes.receipt().is_err());
            assert_eq!(writer.writes, if flush_failure { 1 } else { 2 });
            assert_eq!(writer.flushes, usize::from(flush_failure));
        }
    }

    #[test]
    fn cold_save_startup_requires_actual_held_owner_before_any_exchange() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        let mut output = Vec::new();
        let mut pipes = ColdSavePipes::new(Cursor::new(b"{}\n"), &mut output);
        assert!(run_held_cold_save_startup(&mut session, &mut pipes).is_err());
        assert!(output.is_empty());
        assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
        assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
    }

    #[test]
    fn current_image_completion_and_observation_cannot_import_an_owner_from_json_or_change_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let before = serde_json::to_value(&session.state).unwrap();
        for request in [
            Value::Null,
            json!({}),
            json!({"kind":"poema-original-Save-native-current-image-complete","version":1,"nativeSession":"copied","privateInput":{},"completeCurrentImageVerified":true}),
        ] {
            assert!(complete_held_cold_save_current_image_release(&mut session, &request).is_err());
            assert!(inspect_held_cold_save_current_image_release(&mut session).is_err());
        }
        assert!(Arc::ptr_eq(&graph, &session.project));
        assert_eq!(serde_json::to_value(&session.state).unwrap(), before);
        assert!(session.persistence.is_none());
        assert!(!session.save_operation_held());
    }

    #[test]
    fn current_image_preparation_cannot_import_a_cold_owner_from_json_or_change_an_ordinary_session() {
        let mut session = Session::default();
        session.execute("file.newProject", json!({"name":"Untouched ordinary edit"})).unwrap();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for request in
            [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-current-image-prepare","version":1,"nativeSession":"copied","privateInput":{}})]
        {
            assert!(prepare_held_cold_save_current_image_release(&mut session, &request).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn application_preparation_refuses_an_ordinary_session_without_changing_its_graph_or_editor() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        assert!(prepare_held_cold_save_application(&mut session).is_err());
        assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
        assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
        assert!(session.persistence.is_none());
    }

    #[test]
    fn application_intent_cannot_create_a_cold_owner_from_a_receipt_or_change_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for receipt in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-intent-receipt","version":1})] {
            assert!(confirm_held_cold_save_application_intent(&mut session, &receipt).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn checkpoint_archive_intent_cannot_create_a_cold_owner_or_change_an_ordinary_session() {
        let mut s = Session::default();
        let graph = s.project.clone();
        let editor = serde_json::to_value(&s.state).unwrap();
        for receipt in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-checkpoint-archive-intent-receipt","version":1})] {
            assert!(confirm_held_cold_save_checkpoint_archive_intent(&mut s, &receipt).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &s.project));
            assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
            assert!(s.persistence.is_none());
        }
    }
    #[test]
    fn checkpoint_publication_receipt_cannot_create_an_owner_or_change_an_ordinary_session() {
        let mut s = Session::default();
        let project = s.project.clone();
        for receipt in [json!({}), json!({"kind":"poema-original-Save-native-checkpoint-archive-observation-receipt","version":1,"event":{"copied":true}})] {
            assert!(confirm_held_cold_save_checkpoint_publication(&mut s, &receipt).is_err());
            assert!(std::sync::Arc::ptr_eq(&project, &s.project));
            assert!(s.persistence.is_none());
            assert!(s.cold_save_recovery_status().unwrap().is_none());
        }
    }
    #[test]
    fn checkpoint_publication_request_cannot_create_an_owner_or_change_an_ordinary_session() {
        let mut s = Session::default();
        let project = s.project.clone();
        let revision = s.revision;
        for request in [
            json!({}),
            json!({"kind":"poema-original-Save-native-application-checkpoint-archive-publish", "version":1,
            "nativeSession":"a".repeat(32), "privateInput":{"copied":true}}),
        ] {
            assert!(publish_held_cold_save_application_checkpoint(&mut s, &request).is_err());
            assert!(std::sync::Arc::ptr_eq(&project, &s.project));
            assert_eq!(s.revision, revision);
            assert!(s.persistence.is_none());
            assert!(s.cold_save_recovery_status().unwrap().is_none());
        }
    }
    #[test]
    fn checkpoint_archive_request_cannot_create_a_cold_owner_or_change_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for request in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-checkpoint-archive-prepare","version":1})] {
            assert!(prepare_held_cold_save_application_checkpoint_archive(&mut session, &request).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn checkpoint_request_cannot_create_a_cold_owner_or_change_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for request in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-checkpoint-prepare","version":1})] {
            assert!(prepare_held_cold_save_application_checkpoint(&mut session, &request).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn application_records_request_cannot_create_an_owner_or_modify_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for request in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-records-prepare","version":1})] {
            assert!(prepare_held_cold_save_application_records(&mut session, &request).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn application_records_receipts_cannot_create_a_cold_owner_or_change_an_ordinary_editor() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for receipt in [
            Value::Null,
            json!({}),
            json!({"kind":"poema-original-Save-native-application-records-intent-receipt","version":1}),
            json!({"kind":"poema-original-Save-native-application-records-observation-receipt","version":1}),
        ] {
            assert!(confirm_held_cold_save_application_records_intent(&mut session, &receipt).is_err());
            assert!(confirm_held_cold_save_application_native_presentation(&mut session, &receipt).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn application_root_plan_cannot_create_an_owner_or_modify_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for value in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-root-prepare","version":1})] {
            assert!(prepare_held_cold_save_application_root(&mut session, &value).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
    #[test]
    fn root_intent_and_publication_receipts_cannot_create_an_owner_or_change_an_ordinary_session() {
        let mut session = Session::default();
        let graph = session.project.clone();
        let editor = serde_json::to_value(&session.state).unwrap();
        for receipt in [Value::Null, json!({}), json!({"kind":"poema-original-Save-native-application-root-observation-receipt","version":1})] {
            assert!(confirm_held_cold_save_application_root_intent(&mut session, &receipt).is_err());
            assert!(confirm_held_cold_save_application_root_publication(&mut session, &receipt).is_err());
            assert!(std::sync::Arc::ptr_eq(&graph, &session.project));
            assert_eq!(serde_json::to_value(&session.state).unwrap(), editor);
            assert!(session.persistence.is_none());
        }
    }
}
