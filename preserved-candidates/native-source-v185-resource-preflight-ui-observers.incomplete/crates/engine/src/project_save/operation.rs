//! Genuine native Session ownership of original Save worker leases.
//! Private composition only. Post-dispatch release requires the still-unfinished
//! complete file/native/auxiliary reconciliation; no JSON or call return grants it.
use super::{
    Basis, Prepared,
    effects::{self, Event, FileEffects},
    intent::{Intent, Resolution},
};
use crate::{EngineError, Result, Services, Session, autosave::SavePause, previews};
use serde_json::{Value, json};
use std::{io, path::Path, sync::Arc};
mod cold_application;
pub(crate) mod observation;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) use super::preview_plan::provider::admission::Admission as FileAdmission;
pub(crate) use cold_application::{Action as ColdApplicationAction, run as cold_application_action};
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn describe_file_admission(admission: &FileAdmission, check: &impl Fn() -> Result<()>) -> Result<Value> {
    admission.description(check)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn validate_file_admission(s: &Session, key: &Key, admission: &FileAdmission, check: &dyn Fn() -> io::Result<()>) -> Result<()> {
    admission.current(s, key, check)
}
fn error(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}
/// Live, uncopyable key. An operation string or serialized status cannot mint it.
pub(crate) struct Key(Arc<()>);
/// Original native job lifetime, never reconstructed from serialized plan data.
pub(crate) trait PlanningOrigin: Send + Sync {
    fn before_dispatch(&self, s: &Session) -> Result<()>;
    fn io_current(&self) -> Result<()>;
}
#[derive(Default)]
pub(crate) enum Slot {
    #[default]
    Idle,
    Owned(Box<Operation>),
    Cold(Box<ColdRecovery>),
    ColdBusy,
    Busy,
}
impl Slot {
    pub(crate) fn held(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}
// A distinct fresh cold owner. Historical Save descriptions cannot supply an
// original Key, and no old dispatch/cancel/release resolver accepts this slot.
pub(crate) trait ColdSaveOrigin: Send + Sync {
    fn current(&self) -> Result<()>;
    fn description(&self) -> Result<Value>;
    fn prepare_runtime(self: Arc<Self>, _epoch: &str) -> Result<Box<dyn ColdRuntimePreparation>> {
        Err(error("this cold origin has no original journal transition service"))
    }
}
/// In-memory continuation of the actual cold owner; never deserialized.
pub(crate) trait ColdRuntimePreparation: Send + Sync {
    /// Continue only the actual retained original checkpoint. A descriptive
    /// response never admits editing or completes catalog/activity settlement.
    fn prepare_application_settlement(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no actual original metadata settlement continuation"))
    }
    fn confirm_application_settlement_intent(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no actual original metadata settlement continuation"))
    }
    fn confirm_application_settlement_publication(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no actual original metadata settlement continuation"))
    }
    fn inspect_application_settlement(&self, _s: &Session) -> Result<Value> {
        Err(error("this cold runtime has no actual original metadata settlement observation"))
    }
    fn complete_current_image_release(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no actual retained current-image completion"))
    }
    /// Distinct post-Resume observation. The original held readers stay strict.
    fn inspect_completed_current_image_release(&self, _s: &Session) -> Result<Value> {
        Err(error("this cold runtime has no actual completed current-image observation"))
    }
    fn prepare_current_image_release(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no actual current-image release preparation"))
    }
    fn confirm_checkpoint_publication(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original checkpoint publication acknowledgement"))
    }
    fn publish_application_checkpoint(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original checkpoint publication continuation"))
    }
    fn confirm_application_records_intent(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original application records intent continuation"))
    }
    fn confirm_application_native_presentation(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original native presentation publication continuation"))
    }
    fn confirm_application_checkpoint_archive_intent(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original checkpoint archive intent acknowledgement"))
    }
    fn prepare_application_checkpoint_archive(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original checkpoint archive preparation"))
    }
    fn prepare_application_checkpoint(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original held-worker checkpoint preparation"))
    }
    fn prepare_application_records(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original application records preparation"))
    }
    fn prepare_application_root(&mut self, _s: &Session, _request: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original application Root preparation"))
    }
    fn confirm_application_root_intent(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original Root intent continuation"))
    }
    fn confirm_application_root_publication(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold runtime has no original Root publication continuation"))
    }
    fn current(&self) -> Result<()>;
    fn description(&self) -> Result<Value>;
    fn confirm(&mut self, receipt: &Value) -> Result<()>;
    fn worker_current(&self, persistence: Option<&crate::autosave::Persistence>) -> Result<()> {
        if persistence.is_some() {
            return Err(error("this cold runtime has no actual persistence owner"));
        }
        Ok(())
    }
    fn create_worker(&mut self, _prefs: crate::autosave::AutoSavePrefs) -> Result<crate::autosave::Persistence> {
        Err(error("this cold origin has no original gated worker constructor"))
    }
    fn confirm_worker(&mut self, _receipt: &Value) -> Result<()> {
        Err(error("this cold origin has no original worker publication"))
    }
    /// Negative-only precheck of the actual retained continuation. Returning
    /// Ok grants no admission: every original current/write fence still runs.
    /// A known consumed attempt can be refused without rereading its source.
    fn reject_repeated_canonical_action(&self, _action: CanonicalAction<'_>) -> Result<()> {
        Ok(())
    }
    fn canonical_current(&self, _s: &Session) -> Result<()> {
        Ok(())
    }
    fn prepare_canonical(&mut self, _s: &Session) -> Result<Value> {
        Err(error("this cold origin has no original canonical writer"))
    }
    fn confirm_canonical_intent(&mut self, _receipt: &Value) -> Result<Value> {
        Err(error("this cold origin has no canonical intent continuation"))
    }
    fn publish_canonical(&mut self, _s: &Session) -> Result<Value> {
        Err(error("this cold origin has no original canonical writer"))
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    fn service_bindings(&self) -> Option<&crate::agent_bridge::ColdServiceBindings> {
        None
    }
    fn bind_services(&mut self, _s: &mut Session) -> Result<Value> {
        Err(error("this cold origin has no original service-binding continuation"))
    }
    fn confirm_service_bindings(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold origin has no original service-binding observation"))
    }
    fn confirm_canonical_observation(&mut self, _receipt: &Value) -> Result<Value> {
        Err(error("this cold origin has no canonical observation continuation"))
    }
    fn retain_application_preparation(&mut self, _s: &Session, _value: &Value) -> Result<Value> {
        Err(error("this cold origin has no actual application preparation continuation"))
    }
    fn confirm_application_intent(&mut self, _s: &Session, _receipt: &Value) -> Result<Value> {
        Err(error("this cold origin has no actual application intent continuation"))
    }
}
pub(crate) struct ColdRecovery {
    origin: Arc<dyn ColdSaveOrigin>,
    epoch: String,
    basis: Basis,
    _previews: previews::WriterPause,
    runtime: Option<Box<dyn ColdRuntimePreparation>>,
}
impl ColdRecovery {
    fn basis_current(&self, s: &Session) -> Result<()> {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            self.basis.current_cold_binding(s, self.runtime.as_ref().and_then(|runtime| runtime.service_bindings()))
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            self.basis.current(s)
        }
    }
    fn current(&self, s: &Session) -> Result<()> {
        s.require_unprojected_owner()?;
        if let Some(runtime) = &self.runtime {
            runtime.current()?;
        } else {
            self.origin.current()?;
        }
        self.basis_current(s)?;
        if crate::agent_bridge::retained_native_epoch(s) != Some(self.epoch.as_str()) {
            return Err(error("cold Save native epoch changed"));
        }
        if let Some(runtime) = &self.runtime {
            runtime.worker_current(s.persistence.as_ref())?;
            runtime.canonical_current(s)?;
        } else if s.persistence.is_some() {
            return Err(error("cold journal transition cannot inherit a persistence worker"));
        }
        crate::agent_bridge::require_idle_native_writer_jobs(s)
    }
}
pub(crate) fn hold_cold_recovery(s: &mut Session, origin: Arc<dyn ColdSaveOrigin>) -> Result<()> {
    s.require_owner()?;
    if s.persistence.is_some() {
        return Err(error("cold Save construction cannot inherit a persistence worker"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    origin.current()?;
    let previews = s.previews.pause_writers(&|| origin.current())?;
    let basis = Basis::capture(s)?;
    let epoch =
        crate::agent_bridge::retained_native_epoch(s).map(str::to_owned).ok_or_else(|| error("fresh native epoch required before cold Save ownership"))?;
    origin.current()?;
    basis.current(s)?;
    s.project_save_operation = Slot::Cold(Box::new(ColdRecovery { origin, epoch, basis, _previews: previews, runtime: None }));
    Ok(())
}
pub(crate) fn cold_recovery_status(s: &Session) -> Result<Option<Value>> {
    let Slot::Cold(held) = &s.project_save_operation else { return Ok(None) };
    held.current(s)?;
    let mut value = held.origin.description()?;
    let fields = value.as_object_mut().ok_or_else(|| error("typed cold Save description required"))?;
    fields.insert("nativeSession".into(), json!(held.epoch));
    if let Some(runtime) = &held.runtime {
        fields.insert("runtime".into(), runtime.description()?);
        fields.insert("persistenceStarted".into(), json!(s.persistence.is_some()));
    }
    held.current(s)?;
    Ok(Some(value))
}
pub(crate) fn prepare_cold_runtime(s: &mut Session) -> Result<Value> {
    let Slot::Cold(held) = &s.project_save_operation else { return Err(error("actual cold Save owner required before journal preparation")) };
    if held.runtime.is_some() {
        return Err(error("cold runtime was already prepared; read its exact status rather than reserving again"));
    }
    held.current(s)?;
    let origin = held.origin.clone();
    let epoch = held.epoch.clone();
    let runtime = origin.clone().prepare_runtime(&epoch)?;
    runtime.current()?;
    let value = runtime.description()?;
    let Slot::Cold(held) = &s.project_save_operation else { return Err(error("cold Save owner changed during journal preparation")) };
    held.current(s)?;
    if !Arc::ptr_eq(&held.origin, &origin) || held.epoch != epoch || held.runtime.is_some() {
        return Err(error("original cold owner or runtime preparation changed"));
    }
    let Slot::Cold(held) = &mut s.project_save_operation else { return Err(error("cold Save owner unavailable")) };
    held.runtime = Some(runtime);
    Ok(value)
}
pub(crate) fn confirm_cold_runtime(s: &mut Session, receipt: &Value) -> Result<Value> {
    let Slot::Cold(held) = &s.project_save_operation else { return Err(error("actual cold Save owner required before journal confirmation")) };
    // The old journal generation may have legitimately advanced. Its retained
    // runtime alone validates the exact successor; never refresh the old origin.
    s.require_unprojected_owner()?;
    held.basis_current(s)?;
    if crate::agent_bridge::retained_native_epoch(s) != Some(held.epoch.as_str()) || s.persistence.is_some() {
        return Err(error("cold native epoch or persistence changed before journal confirmation"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    let Slot::Cold(held) = &mut s.project_save_operation else { return Err(error("cold Save owner unavailable")) };
    let runtime = held.runtime.as_mut().ok_or_else(|| error("actual retained one-use journal preparation required"))?;
    runtime.confirm(receipt)?;
    cold_recovery_status(s)?.ok_or_else(|| error("cold Save hold disappeared after journal confirmation"))
}
pub(crate) fn create_cold_worker(s: &mut Session) -> Result<Value> {
    let Slot::Cold(held) = &s.project_save_operation else {
        return Err(error("actual cold Save owner required"));
    };
    // An existing worker is a definitive refusal, never a reason to re-read
    // its source or attempt the original constructor again.
    if s.persistence.is_some() {
        return Err(error("original restored auxiliaries and no inherited worker required"));
    }
    held.current(s)?;
    if !s.native_auxiliary_restored {
        return Err(error("original restored auxiliaries and no inherited worker required"));
    }
    let prefs = s.prefs.auto_save.clone();
    let Slot::Cold(held) = &mut s.project_save_operation else {
        return Err(error("cold owner disappeared"));
    };
    let runtime = held.runtime.as_mut().ok_or_else(|| error("actual confirmed runtime preparation required"))?;
    let persistence = runtime.create_worker(prefs)?;
    let Slot::Cold(held) = &s.project_save_operation else {
        return Err(error("cold owner disappeared"));
    };
    held.basis_current(s)?;
    if crate::agent_bridge::retained_native_epoch(s) != Some(held.epoch.as_str()) {
        return Err(error("native epoch changed during worker creation"));
    }
    s.persistence = Some(persistence);
    cold_recovery_status(s)?.ok_or_else(|| error("actual cold owner disappeared"))
}
pub(crate) fn confirm_cold_worker(s: &mut Session, receipt: &Value) -> Result<Value> {
    s.require_unprojected_owner()?;
    let Slot::Cold(held) = &s.project_save_operation else {
        return Err(error("actual cold Save owner required"));
    };
    held.basis_current(s)?;
    if crate::agent_bridge::retained_native_epoch(s) != Some(held.epoch.as_str()) {
        return Err(error("native epoch changed before worker confirmation"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    let Slot::Cold(held) = &mut s.project_save_operation else {
        return Err(error("cold owner disappeared"));
    };
    held.runtime.as_mut().ok_or_else(|| error("actual cold worker preparation required"))?.confirm_worker(receipt)?;
    cold_recovery_status(s)?.ok_or_else(|| error("actual cold owner disappeared"))
}
#[derive(Clone, Copy)]
pub(crate) enum CanonicalAction<'a> {
    Prepare,
    ConfirmIntent(&'a Value),
    Publish,
    ConfirmObservation(&'a Value),
    BindServices,
    ConfirmServiceBindings(&'a Value),
}
pub(crate) fn cold_canonical_action(s: &mut Session, action: CanonicalAction<'_>) -> Result<Value> {
    s.require_unprojected_owner()?;
    let Slot::Cold(held) = &s.project_save_operation else {
        return Err(error("actual cold Save owner required"));
    };
    held.basis_current(s)?;
    if crate::agent_bridge::retained_native_epoch(s) != Some(held.epoch.as_str()) {
        return Err(error("cold native epoch changed before canonical handoff"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    if matches!(action, CanonicalAction::Prepare | CanonicalAction::Publish | CanonicalAction::BindServices) {
        // Read only one-use state retained by THIS actual owner. This can
        // refuse a replay, but cannot authorize an unused effect or refresh
        // any source observation. Keep the complete original fence below.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            held.runtime.as_ref().ok_or_else(|| error("actual confirmed cold runtime required"))?.reject_repeated_canonical_action(action)
        }))
        .unwrap_or_else(|_| Err(error("cold replay precheck failed unexpectedly; retain the actual owner without replay")))?;
        held.current(s)?;
    }
    let slot = std::mem::replace(&mut s.project_save_operation, Slot::ColdBusy);
    let Slot::Cold(mut held) = slot else {
        s.project_save_operation = slot;
        return Err(error("cold Save owner changed before canonical handoff"));
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let runtime = held.runtime.as_mut().ok_or_else(|| error("actual confirmed cold runtime required"))?;
        match action {
            CanonicalAction::Prepare => runtime.prepare_canonical(s),
            CanonicalAction::ConfirmIntent(receipt) => runtime.confirm_canonical_intent(receipt),
            CanonicalAction::Publish => runtime.publish_canonical(s),
            CanonicalAction::ConfirmObservation(receipt) => runtime.confirm_canonical_observation(receipt),
            CanonicalAction::BindServices => runtime.bind_services(s),
            CanonicalAction::ConfirmServiceBindings(receipt) => runtime.confirm_service_bindings(s, receipt),
        }
    }))
    .unwrap_or_else(|_| Err(error("cold canonical service failed unexpectedly; the original owner remains held without replay")));
    s.project_save_operation = Slot::Cold(held);
    if result.is_ok() && matches!(action, CanonicalAction::BindServices) {
        cold_recovery_status(s)?.ok_or_else(|| error("actual cold owner disappeared after service binding"))?;
    }
    result
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prepared,
    AwaitingReconciliation,
    CancelReleasing,
}
pub(crate) struct Operation {
    seal: Arc<()>,
    id: String,
    phase: Phase,
    before: Basis,
    planned: Value,
    body_returned: bool,
    postflight: Option<std::result::Result<(), String>>,
    after: Option<Basis>,
    prepared: Option<Prepared>,
    planning_origin: Option<Arc<dyn PlanningOrigin>>,
    pause: Option<SavePause>,
    store: Arc<previews::PreviewStore>,
    _previews: previews::WriterPause,
    returned: Option<std::result::Result<Value, String>>,
    events: Vec<Event>,
    events_complete: bool,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    documents: Vec<super::retained_effects::DocumentReceipt>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    preview_report: Option<super::preview_plan::provider::Report>,
}
impl Operation {
    fn key(&self, key: &Key) -> Result<()> {
        if Arc::ptr_eq(&self.seal, &key.0) { Ok(()) } else { Err(error("native Save key belongs to another live operation")) }
    }
    fn workers(&self, s: &Session) -> Result<()> {
        if !Arc::ptr_eq(&self.store, &s.previews) || self.pause.is_some() != s.persistence.is_some() {
            return Err(error("original Save worker owner changed; pending work retained"));
        }
        if let (Some(pause), Some(p)) = (&self.pause, &s.persistence) {
            if self.phase == Phase::CancelReleasing {
                if !pause.same_owner(p) {
                    return Err(error("original Save persistence owner changed during cancellation"));
                }
            } else {
                pause.current(p).map_err(error)?;
            }
        }
        crate::agent_bridge::require_idle_native_writer_jobs(s)
    }
}
pub(crate) fn begin(s: &mut Session, id: &str, intent: &Intent, check: &impl Fn() -> Result<()>) -> Result<Key> {
    check()?;
    s.require_owner()?;
    if !(32..=128).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(error("fresh bounded native Save operation id required"));
    }
    let Resolution::Destination { path, adopt, .. } = intent.resolve(s)? else {
        return Err(error("original Save still needs its human destination; no worker was paused"));
    };
    begin_with(s, id, None, check, |s| super::prepare(s, &path, adopt))
}
// A genuine native Key and original Prepared payload, never a description or
// serialized grant. The exclusive Session call retains this acquisition until
// the SAME key is installed after the original worker pauses are acquired.
struct DeliveredSaveAcquisition {
    key: Key,
    prepared: Prepared,
    origin: Arc<dyn PlanningOrigin>,
}
/// Consume an in-window ORIGINAL delivery into native Save ownership before
/// any potentially slow persistence/preview pause. Delivery is not renewed:
/// both sides of binding and the actual Key transfer keep its original check.
/// The original job's independent deadline and Stop cover all later work.
pub(crate) fn begin_delivered(
    s: &mut Session,
    id: &str,
    origin: Arc<dyn PlanningOrigin>,
    delivery_check: &impl Fn() -> Result<()>,
    check: &impl Fn() -> Result<()>,
    prepare: impl FnOnce(&Session) -> Result<Prepared>,
) -> Result<Key> {
    delivery_check()?;
    s.require_owner()?;
    if !(32..=128).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(error("fresh bounded native Save operation id required"));
    }
    origin.before_dispatch(s)?;
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    let prepared = prepare(s)?;
    delivery_check()?;
    prepared.still_current(s)?;
    delivery_check()?;
    let acquisition = DeliveredSaveAcquisition { key: Key(Arc::new(())), prepared, origin };
    delivery_check()?;
    let DeliveredSaveAcquisition { key, prepared, origin } = acquisition;
    let revalidate = origin.clone();
    begin_with_key(s, id, Some(origin), key, check, move |s| {
        revalidate.before_dispatch(s)?;
        prepared.still_current(s)?;
        Ok(prepared)
    })
}
/// Same original pause/acquire/install body for original or worker-owned bytes.
pub(crate) fn begin_with(
    s: &mut Session,
    id: &str,
    planning_origin: Option<Arc<dyn PlanningOrigin>>,
    check: &impl Fn() -> Result<()>,
    prepare: impl FnOnce(&Session) -> Result<Prepared>,
) -> Result<Key> {
    begin_with_key(s, id, planning_origin, Key(Arc::new(())), check, prepare)
}
fn begin_with_key(
    s: &mut Session,
    id: &str,
    planning_origin: Option<Arc<dyn PlanningOrigin>>,
    key: Key,
    check: &impl Fn() -> Result<()>,
    prepare: impl FnOnce(&Session) -> Result<Prepared>,
) -> Result<Key> {
    check()?;
    s.require_owner()?;
    if !(32..=128).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(error("fresh bounded native Save operation id required"));
    }
    if let Some(origin) = &planning_origin {
        origin.before_dispatch(s)?;
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    s.sync_persistence();
    let pause = s.persistence.as_ref().map(|p| p.pause_project_save(|| check().map_err(|e| e.to_string()))).transpose().map_err(error)?;
    s.apply_persistence_events();
    let store = s.previews.clone();
    let previews = store.pause_writers(check)?;
    check()?;
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    if let (Some(pause), Some(p)) = (&pause, &s.persistence) {
        pause.current(p).map_err(error)?;
    }
    let prepared = prepare(s)?;
    check()?;
    prepared.still_current(s)?;
    let before = Basis::capture(s)?;
    let planned = prepared.description();
    let seal = key.0.clone();
    s.project_save_operation = Slot::Owned(Box::new(Operation {
        seal: seal.clone(),
        id: id.into(),
        phase: Phase::Prepared,
        before,
        planned,
        body_returned: false,
        postflight: None,
        after: None,
        prepared: Some(prepared),
        planning_origin,
        pause,
        store,
        _previews: previews,
        returned: None,
        events: Vec::new(),
        events_complete: true,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        documents: Vec::new(),
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        preview_report: None,
    }));
    Ok(key)
}
fn owned<'a>(s: &'a Session, key: &Key) -> Result<&'a Operation> {
    match &s.project_save_operation {
        Slot::Owned(o) => {
            o.key(key)?;
            Ok(o)
        }
        _ => Err(error("native Save operation is unavailable or already on its call stack")),
    }
}
pub(crate) fn prepared<'a>(s: &'a Session, key: &Key) -> Result<&'a Prepared> {
    let o = owned(s, key)?;
    o.prepared.as_ref().ok_or_else(|| error("native Save was already dispatched; its prepared bytes cannot be replayed"))
}
/// A private witness of the same genuinely paused original native operation.
/// It is neither serializable nor a filesystem/Root grant.
pub(super) struct PreviewOwner {
    seal: Arc<()>,
    autosave: Option<crate::autosave::SavePauseProbe>,
    previews: previews::WriterPauseProbe,
    planning_origin: Option<Arc<dyn PlanningOrigin>>,
}
pub(super) fn preview_inputs<'a>(s: &'a Session, key: &Key) -> Result<(PreviewOwner, &'a Prepared, &'a previews::WriterPause)> {
    let o = owned(s, key)?;
    o.workers(s)?;
    o.before.current(s)?;
    if o.phase != Phase::Prepared {
        return Err(error("preview preflight requires original undispatched Save owner"));
    }
    if let Some(origin) = &o.planning_origin {
        origin.before_dispatch(s)?;
    }
    Ok((
        PreviewOwner {
            seal: o.seal.clone(),
            autosave: o.pause.as_ref().map(|p| p.probe()).transpose().map_err(error)?,
            previews: o._previews.probe(),
            planning_origin: o.planning_origin.clone(),
        },
        prepared(s, key)?,
        &o._previews,
    ))
}
impl PreviewOwner {
    pub(super) fn workers_current(&self) -> Result<()> {
        if let Some(origin) = &self.planning_origin {
            origin.io_current()?;
        }
        if let Some(p) = &self.autosave {
            p.current().map_err(error)?;
        }
        self.previews.current()
    }
    pub(super) fn current(&self, s: &Session, key: &Key) -> Result<()> {
        let o = owned(s, key)?;
        if !Arc::ptr_eq(&self.seal, &o.seal) || o.phase != Phase::Prepared {
            return Err(error("preview preflight belongs to another or dispatched Save owner"));
        }
        o.workers(s)?;
        o.before.current(s)
    }
}
// A private read guard for the SAME actual native owner. Its seal/phase cannot
// be recreated from a request, status, digest or archived description.
pub(crate) struct RecoveryOwner {
    seal: Arc<()>,
    phase: Phase,
}
impl RecoveryOwner {
    pub(crate) fn current(&self, s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<()> {
        check()?;
        let o = owned(s, key)?;
        if !Arc::ptr_eq(&self.seal, &o.seal) || self.phase != o.phase || o.phase == Phase::CancelReleasing {
            return Err(error("original Save recovery read owner changed or is releasing"));
        }
        o.workers(s)?;
        o._previews.probe().current()?;
        match o.phase {
            Phase::Prepared => o.before.current(s)?,
            Phase::AwaitingReconciliation => {
                o.after.as_ref().ok_or_else(|| error("original Save after-state is still unknown; retain the owner"))?.current(s)?
            }
            Phase::CancelReleasing => return Err(error("Save cancellation is already releasing its original workers")),
        }
        check()
    }
    /// Private frame I/O uses the original planning lifetime as well as the
    /// actual same Key and paused workers. A new read Check cannot renew it.
    pub(crate) fn write_current(&self, s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<()> {
        self.current(s, key, check)?;
        s.require_unprojected_owner()?;
        let origin = owned(s, key)?.planning_origin.as_ref().ok_or_else(|| error("original Save planning lifetime required for private frame I/O"))?;
        origin.io_current()?;
        check()
    }
    pub(crate) fn phase(&self) -> String {
        format!("{:?}", self.phase)
    }
}
pub(crate) fn recovery_owner(s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<RecoveryOwner> {
    let o = owned(s, key)?;
    let guard = RecoveryOwner { seal: o.seal.clone(), phase: o.phase };
    guard.current(s, key, check)?;
    Ok(guard)
}

pub(crate) fn status(s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<Value> {
    check()?;
    let o = owned(s, key)?;
    o.workers(s)?;
    let current = if let Some(after) = &o.after { after.current(s).is_ok() } else { o.before.current(s).is_ok() };
    check()?;
    Ok(
        json!({"kind":"native_pending_Save_worker_owner","version":1,"operation":o.id,"phase":format!("{:?}",o.phase),"nativeBasisCurrent":current,"serviceReturned":o.returned.is_some(),"effectCallReturns":o.events.len(),"effectCallReturnsComplete":o.events_complete,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false}),
    )
}
/// The original publication body runs exactly once. Every returned result,
/// including Ok, remains pending until complete real-effect reconciliation.
pub(crate) fn dispatch(s: &mut Session, key: &Key, files: &mut dyn FileEffects, check: &impl Fn() -> Result<()>) -> Result<Value> {
    check()?;
    let o = owned(s, key)?;
    o.workers(s)?;
    o.before.current(s)?;
    if o.phase != Phase::Prepared {
        return Err(error("native Save already dispatched or releasing; inspect its exact outcome without repeating it"));
    }
    if let Some(origin) = &o.planning_origin {
        origin.before_dispatch(s)?;
    }
    let slot = std::mem::replace(&mut s.project_save_operation, Slot::Busy);
    let Slot::Owned(mut o) = slot else {
        s.project_save_operation = slot;
        return Err(error("native Save owner disappeared before dispatch"));
    };
    // Mark the possible-effect boundary BEFORE invoking any physical receiver.
    o.phase = Phase::AwaitingReconciliation;
    let origin = o.planning_origin.clone();
    let current = || {
        check()?;
        if let Some(origin) = &origin {
            origin.io_current()?;
        }
        Ok(())
    };
    let mut recording = Recording { delegate: files, events: Vec::new(), complete: true, planning_origin: origin.clone() };
    let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        current()?;
        let prepared = o.prepared.take().ok_or_else(|| error("native Save prepared value was already consumed"))?;
        prepared.publish_with(s, &mut recording)
    }));
    o.body_returned = body.is_ok();
    let result = body.unwrap_or_else(|_| Err(error("native Save body failed; possible effects and original workers remain owned")));
    o.events = recording.events;
    o.events_complete = recording.complete;
    o.returned = Some(result.as_ref().map(Clone::clone).map_err(ToString::to_string));
    o.after = Basis::capture(s).ok();
    let postflight = std::panic::catch_unwind(std::panic::AssertUnwindSafe(current))
        .unwrap_or_else(|_| Err(error("native Save postflight observation failed; operation remains owned")));
    o.postflight = Some(postflight.as_ref().map(|_| ()).map_err(ToString::to_string));
    s.project_save_operation = Slot::Owned(o);
    result.and_then(|value| postflight.map(|()| value))
}
/// Actual original Prepared output plus its retained physical receipts stay in
/// this same native owner. The supplied current filesystem checks/delegate still
/// must provide complete file/auxiliary custody; this is not a grant constructor.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn dispatch_retained(
    s: &mut Session,
    key: &Key,
    delegate: &mut dyn FileEffects,
    io_check: &dyn Fn() -> io::Result<()>,
    check: &impl Fn() -> Result<()>,
) -> Result<Value> {
    let o = owned(s, key)?;
    let origin = o.planning_origin.clone();
    let current = || {
        io_check()?;
        if let Some(origin) = &origin {
            origin.io_current().map_err(|e| io::Error::other(e.to_string()))?;
        }
        Ok(())
    };
    let mut files = super::retained_effects::RetainedDocuments::new(prepared(s, key)?, &o.id, delegate, &current).map_err(|e| error(e.to_string()))?;
    let result = dispatch(s, key, &mut files, check);
    let documents = files.into_documents();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        o.documents.extend(documents);
    }
    result
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn documents<'a>(s: &'a Session, key: &Key) -> Result<&'a [super::retained_effects::DocumentReceipt]> {
    Ok(&owned(s, key)?.documents)
}
/// Compose the original service once with live preview effects. This does not
/// admit a public Save hand or release workers: complete Root/document/auxiliary
/// reconciliation is still required after every original result.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn dispatch_preserving_previews(
    s: &mut Session,
    key: &Key,
    specs: &[super::preview_plan::VolumeSpec],
    delegate: &mut dyn FileEffects,
    io_check: &dyn Fn() -> io::Result<()>,
    check: &impl Fn() -> Result<()>,
) -> Result<Value> {
    let o = owned(s, key)?;
    if o.phase != Phase::Prepared || o.preview_report.is_some() {
        return Err(error("preview Save was already dispatched or inspected; no callback replay"));
    }
    let id = o.id.clone();
    let plan = super::preview_plan::Plan::capture(s, key, specs, &|| io_check())?;
    plan.current(s, key, &|| io_check())?;
    let mut files = super::preview_plan::provider::Provider::new(plan, &id, delegate, io_check).map_err(|e| error(e.to_string()))?;
    let result = dispatch_retained(s, key, &mut files, io_check, check);
    // The original dispatch catches its service panic and restores this exact
    // owner before returning. Keep physical publications even on Err/Stop.
    let report = files.into_report();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        if Arc::ptr_eq(&o.seal, &key.0) {
            o.preview_report = Some(report);
        } else {
            return Err(error("native preview owner changed after dispatch"));
        }
    } else {
        return Err(error("native preview owner unavailable after dispatch"));
    }
    result
}
/// Complete original document plus indexed-preview payload admission. This
/// still requires actual caller Root/overwrite/directory custody and complete
/// reconciliation; no public command or worker-release authority is added.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn dispatch_admitted_files(
    s: &mut Session,
    key: &Key,
    specs: &[super::preview_plan::VolumeSpec],
    delegate: &mut dyn FileEffects,
    io_check: &dyn Fn() -> io::Result<()>,
    check: &impl Fn() -> Result<()>,
) -> Result<Value> {
    let admission = prepare_dispatch_files(s, key, specs, io_check)?;
    dispatch_prepared_files(s, key, admission, delegate, io_check, check)
}
/// Capture once; retain the SAME actual native file frames and complete credits.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn prepare_dispatch_files(
    s: &Session,
    key: &Key,
    specs: &[super::preview_plan::VolumeSpec],
    io_check: &dyn Fn() -> io::Result<()>,
) -> Result<FileAdmission> {
    let o = owned(s, key)?;
    if o.phase != Phase::Prepared || o.preview_report.is_some() {
        return Err(error("original Save admission requires the same undispatched owner"));
    }
    super::preview_plan::provider::admission::Admission::capture(s, key, specs, &o.id, prepared(s, key)?, io_check)
}
/// Consume live original admission, never recapture later files or rebuild it
/// from a digest, catalog, status or copied description.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn dispatch_prepared_files(
    s: &mut Session,
    key: &Key,
    admission: FileAdmission,
    delegate: &mut dyn FileEffects,
    io_check: &dyn Fn() -> io::Result<()>,
    check: &impl Fn() -> Result<()>,
) -> Result<Value> {
    let o = owned(s, key)?;
    if o.phase != Phase::Prepared || o.preview_report.is_some() {
        return Err(error("original Save admission cannot be replayed"));
    }
    let mut files = admission.bind(s, key, &o.id, delegate, io_check)?;
    let result = dispatch_retained(s, key, &mut files, io_check, check);
    let report = files.into_report();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        if !Arc::ptr_eq(&o.seal, &key.0) {
            return Err(error("original native admission owner changed after dispatch"));
        }
        o.preview_report = Some(report);
    } else {
        return Err(error("original native admission owner unavailable after dispatch"));
    }
    result
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) fn preview_report<'a>(s: &'a Session, key: &Key) -> Result<Option<&'a super::preview_plan::provider::Report>> {
    Ok(owned(s, key)?.preview_report.as_ref())
}
/// Cancellation is available only before dispatch. Lost resume observation
/// keeps the same original acknowledgement and native gate; it never requeues.
pub(crate) fn cancel_before_dispatch(s: &mut Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<()> {
    check()?;
    let o = owned(s, key)?;
    o.workers(s)?;
    o.before.current(s)?;
    if !matches!(o.phase, Phase::Prepared | Phase::CancelReleasing) {
        return Err(error("Save crossed the physical-effect boundary; reconcile it instead of cancelling or rolling back"));
    }
    let slot = std::mem::replace(&mut s.project_save_operation, Slot::Busy);
    let Slot::Owned(mut o) = slot else {
        s.project_save_operation = slot;
        return Err(error("native Save owner disappeared before cancellation"));
    };
    o.phase = Phase::CancelReleasing;
    s.sync_persistence();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check()?;
        if let Some(pause) = &mut o.pause {
            pause.finish_owned(|| check().map_err(|e| e.to_string())).map_err(error)?;
        }
        check()
    }))
    .unwrap_or_else(|_| Err(error("native Save cancellation observation failed; the original acknowledgement remains owned")));
    if result.is_ok() {
        s.project_save_operation = Slot::Idle;
        drop(o);
    } else {
        s.project_save_operation = Slot::Owned(o);
    }
    result
}
const MAX_EVENTS: usize = 65536;
struct Recording<'a> {
    delegate: &'a mut dyn FileEffects,
    events: Vec<Event>,
    complete: bool,
    planning_origin: Option<Arc<dyn PlanningOrigin>>,
}
impl Recording<'_> {
    fn capacity(&mut self) -> io::Result<()> {
        if let Some(origin) = &self.planning_origin {
            origin.io_current().map_err(|e| io::Error::other(e.to_string()))?;
        }
        if self.events.len() >= MAX_EVENTS {
            self.complete = false;
            Err(io::Error::other("native Save effect record budget exhausted; no further physical effect admitted"))
        } else {
            Ok(())
        }
    }
}
impl FileEffects for Recording<'_> {
    fn document(&mut self, s: &dyn Services, r: effects::DocumentRole, p: &str, b: &[u8]) -> io::Result<()> {
        self.capacity()?;
        self.delegate.document(s, r, p, b)
    }
    fn directories(&mut self, r: effects::DirectoryRole, p: &Path) -> io::Result<()> {
        self.capacity()?;
        self.delegate.directories(r, p)
    }
    fn preferences(&mut self, p: &Path, b: &[u8]) -> io::Result<()> {
        self.capacity()?;
        self.delegate.preferences(p, b)
    }
    fn preview_exists(&mut self, p: &Path) -> bool {
        if self.capacity().is_err() { true } else { self.delegate.preview_exists(p) }
    }
    fn preview_rename(&mut self, a: &Path, b: &Path) -> io::Result<()> {
        self.capacity()?;
        self.delegate.preview_rename(a, b)
    }
    fn preview_copy(&mut self, a: &Path, b: &Path) -> io::Result<u64> {
        self.capacity()?;
        self.delegate.preview_copy(a, b)
    }
    fn preview_remove(&mut self, p: &Path) -> io::Result<()> {
        self.capacity()?;
        self.delegate.preview_remove(p)
    }
    fn event(&mut self, e: Event) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(e.clone());
        } else {
            self.complete = false;
        }
        self.delegate.event(e)
    }
}
#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn test_key_seal(key: &Key) -> Arc<()> {
    key.0.clone()
}
