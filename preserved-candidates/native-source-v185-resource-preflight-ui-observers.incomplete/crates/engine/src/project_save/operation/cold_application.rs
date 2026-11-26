//! Continues the SAME Cold owner. A receipt can advance its original journal
//! observation, never create an owner or release editing/previews/persistence.
use super::*;

#[derive(Clone, Copy)]
pub(crate) enum Action<'a> {
    PrepareSettlement(&'a Value),
    ConfirmSettlementIntent(&'a Value),
    ConfirmSettlementPublication(&'a Value),
    InspectSettlement,
    CompleteCurrentImageRelease(&'a Value),
    InspectCompletedCurrentImageRelease,
    PrepareCurrentImageRelease(&'a Value),
    RetainPreparation(&'a Value),
    ConfirmIntent(&'a Value),
    PrepareRoot(&'a Value),
    PrepareRecords(&'a Value),
    PrepareCheckpoint(&'a Value),
    PrepareCheckpointArchive(&'a Value),
    PublishCheckpointArchive(&'a Value),
    ConfirmCheckpointPublication(&'a Value),
    ConfirmCheckpointArchiveIntent(&'a Value),
    ConfirmRecordsIntent(&'a Value),
    ConfirmNativePresentation(&'a Value),
    ConfirmRootIntent(&'a Value),
    ConfirmRootPublication(&'a Value),
}

pub(crate) fn run(s: &mut Session, action: Action<'_>) -> Result<Value> {
    s.require_unprojected_owner()?;
    let Slot::Cold(held) = &s.project_save_operation else {
        return Err(error("the same actual cold Save owner is required for application continuation"));
    };
    held.basis_current(s)?;
    if crate::agent_bridge::retained_native_epoch(s) != Some(held.epoch.as_str()) {
        return Err(error("the original cold native epoch changed before application continuation"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    // Confirmation observes the exact expected successor of the OLD retained
    // journal. Requiring the old head to remain current here would reject the
    // legitimate publication. Its actual runtime alone validates the advance.
    if matches!(action, Action::RetainPreparation(_)) {
        held.current(s)?;
    }
    let slot = std::mem::replace(&mut s.project_save_operation, Slot::ColdBusy);
    let Slot::Cold(mut held) = slot else {
        s.project_save_operation = slot;
        return Err(error("the original cold application owner changed"));
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let runtime = held.runtime.as_mut().ok_or_else(|| error("actual retained cold runtime required for application continuation"))?;
        match action {
            Action::PrepareSettlement(request) => runtime.prepare_application_settlement(s, request),
            Action::ConfirmSettlementIntent(receipt) => runtime.confirm_application_settlement_intent(s, receipt),
            Action::ConfirmSettlementPublication(receipt) => runtime.confirm_application_settlement_publication(s, receipt),
            Action::InspectSettlement => runtime.inspect_application_settlement(s),
            Action::CompleteCurrentImageRelease(request) => runtime.complete_current_image_release(s, request),
            Action::InspectCompletedCurrentImageRelease => runtime.inspect_completed_current_image_release(s),
            Action::PrepareCurrentImageRelease(request) => runtime.prepare_current_image_release(s, request),
            Action::RetainPreparation(value) => runtime.retain_application_preparation(s, value),
            Action::ConfirmIntent(receipt) => runtime.confirm_application_intent(s, receipt),
            Action::PrepareRoot(request) => runtime.prepare_application_root(s, request),
            Action::PrepareRecords(request) => runtime.prepare_application_records(s, request),
            Action::PrepareCheckpoint(request) => runtime.prepare_application_checkpoint(s, request),
            Action::PrepareCheckpointArchive(request) => runtime.prepare_application_checkpoint_archive(s, request),
            Action::PublishCheckpointArchive(request) => runtime.publish_application_checkpoint(s, request),
            Action::ConfirmCheckpointPublication(receipt) => runtime.confirm_checkpoint_publication(s, receipt),
            Action::ConfirmCheckpointArchiveIntent(receipt) => runtime.confirm_application_checkpoint_archive_intent(s, receipt),
            Action::ConfirmRecordsIntent(receipt) => runtime.confirm_application_records_intent(s, receipt),
            Action::ConfirmNativePresentation(receipt) => runtime.confirm_application_native_presentation(s, receipt),
            Action::ConfirmRootIntent(receipt) => runtime.confirm_application_root_intent(s, receipt),
            Action::ConfirmRootPublication(receipt) => runtime.confirm_application_root_publication(s, receipt),
        }
    }))
    .unwrap_or_else(|_| Err(error("application continuation failed unexpectedly; the original owner remains held without replay")));
    s.project_save_operation = Slot::Cold(held);
    result
}
