//! Exact predecessor/successor custody of the original cold Save journal.
//! This retains the original one-use reservation and can create its gated worker;
//! it owns no original Save Key or application owner and cannot release editing.
use super::*;
use crate::{
    autosave::{AutosaveConfig, session_reservation::ColdJournalReservation},
    project_save::operation::ColdRuntimePreparation,
};
mod application;
mod canonical;
const HEAD: u64 = 1024 * 1024;
const PAGE: u64 = 512 * 1024;

#[derive(Clone)]
pub(super) struct JournalContinuation {
    anchor: Arc<Control>,
    head: Arc<Control>,
    pages: Vec<Arc<Control>>,
}
enum WorkerState {
    NotStarted,
    Attempted,
    Created { journal: Arc<crate::autosave::current_startup::Journal>, description: Value, confirmation_attempted: bool, confirmed: bool },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerReceipt {
    kind: String,
    version: u8,
    preparation: Value,
    worker: Value,
    anchor: Observed,
    head: Observed,
}

impl JournalContinuation {
    fn same_original(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.anchor, &other.anchor)
            && Arc::ptr_eq(&self.head, &other.head)
            && self.pages.len() == other.pages.len()
            && self.pages.iter().zip(&other.pages).all(|(a, b)| Arc::ptr_eq(a, b))
    }
    pub(super) fn control<'a>(&'a self, index: usize, original: &'a Control) -> &'a Control {
        match index {
            4 => &self.anchor,
            5 => &self.head,
            _ => original,
        }
    }
    pub(super) fn current(&self, check: &media_guard::Check) -> Result<()> {
        for control in [&self.anchor, &self.head].into_iter().chain(self.pages.iter()) {
            control.current(check)?;
            control.fence(check)?;
        }
        Ok(())
    }
    // A complete application source query still reads every original Control.
    // Only that query's effect-free after-read pass may use exact generation
    // fences. No new Check, Control, successor, publisher or owner is created.
    pub(super) fn begin_application_read(&self, check: &media_guard::Check, mode: ApplicationRead) -> Result<()> {
        match mode {
            ApplicationRead::Complete => self.current(check),
            ApplicationRead::FenceAfterRead => self.fence_after_read(check),
        }
    }
    pub(super) fn fence_after_read(&self, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        for control in [&self.anchor, &self.head].into_iter().chain(self.pages.iter()) {
            control.fence(check)?;
        }
        check.check()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    kind: String,
    version: u8,
    preparation: Value,
    anchor: Observed,
    head: Observed,
}
pub(super) struct Pending {
    original: Arc<Input>,
    cfg: AutosaveConfig,
    reservation: Option<ColdJournalReservation>,
    preparation: Value,
    successor: Option<JournalContinuation>,
    attempted: bool,
    worker: WorkerState,
    canonical: Option<canonical::Canonical>,
    application: application::Application,
}
fn control<'a>(input: &'a Input, index: usize) -> Result<&'a Control> {
    input.controls.get(index).ok_or_else(|| error("complete original journal controls required"))
}
fn original_description(input: &Input, index: usize, path: &Path) -> Result<Value> {
    control_description(control(input, index)?, path)
}
fn control_description(control: &Control, path: &Path) -> Result<Value> {
    let value = control.observation()?;
    Ok(json!({"path":path,"sha256":value.sha256,"bytes":value.bytes,
        "physical":{"dev":value.identity.device,"ino":value.identity.inode,"mode":value.identity.mode.to_string(),
        "uid":value.identity.uid.to_string(),"size":value.bytes.to_string(),
        "mtimeNs":nanos(&value.modified)?,"ctimeNs":nanos(&value.changed)?}}))
}
impl Pending {
    pub(super) fn prepare(original: Arc<Input>, epoch: &str) -> Result<Self> {
        let check = media_guard::Check::new(web_time::Instant::now());
        original.current(&check)?;
        if !token_ok(epoch) || epoch == original.body.basis.frame.native_session {
            return Err(error("fresh actual cold native epoch required"));
        }
        let cfg = AutosaveConfig::new(original.body.basis.input.native_data.clone());
        let reservation = ColdJournalReservation::prepare(&cfg, &|| original.current(&check).map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
        let session = reservation.description(&cfg).map_err(error)?;
        let root_before = reservation.root_description();
        let basis = &original.body.basis;
        let frame = &basis.frame;
        let preparation = json!({"kind":"poema-native-Save-runtime-preparation","version":1,
            "nativePid":std::process::id(),"nativeSession":epoch,"scope":frame.scope,
            "sourceNativePid":frame.native_pid,"sourceNativeSession":frame.native_session,
            "capture":frame.capture,"afterFrameSha256":frame.sha256,
            "privateInput":{"path":original.input_directory.record().path.join("input.json"),
                "sha256":original.reference_hash,"bytes":original.input.observation()?.bytes},
            "journal":{"directory":basis.whole_journal.directory,"binding":basis.whole_journal.binding,
                "anchor":original_description(&original,4,&basis.whole_journal.anchor)?,
                "head":original_description(&original,5,&basis.whole_journal.head)?},
            "session":session,"recoveryRootBefore":root_before,"reserveBytes":basis.reserve_bytes,
            "editingPaused":true,"persistenceStarted":false,"writeAuthority":false,
            "startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false});
        bytes(&preparation, INPUT)?;
        original.current(&check)?;
        reservation.current(&cfg, &|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
        Ok(Self {
            original,
            cfg,
            reservation: Some(reservation),
            preparation,
            successor: None,
            attempted: false,
            worker: WorkerState::NotStarted,
            canonical: None,
            application: application::Application::default(),
        })
    }
    fn read_successor(&self, receipt: &Value, check: &media_guard::Check) -> Result<JournalContinuation> {
        bytes(receipt, INPUT)?;
        let receipt: Receipt = parse("native.coldSaveRuntime", receipt)?;
        let basis = &self.original.body.basis;
        if receipt.kind != "poema-original-Save-cold-runtime-intent-receipt"
            || receipt.version != 1
            || receipt.preparation != self.preparation
            || receipt.anchor.path != basis.whole_journal.anchor
            || receipt.head.path != basis.whole_journal.head
        {
            return Err(error("exact original cold runtime intent receipt required"));
        }
        self.read_published_successor(&receipt.anchor, &receipt.head, json!({"kind":"native-runtime-intent","preparation":self.preparation}), check)
    }
    fn source_current(&self) -> Result<()> {
        if self.preparation["nativePid"] != std::process::id() {
            return Err(error("cold runtime belongs to another native process"));
        }
        self.application.source_current(&media_guard::Check::new(web_time::Instant::now()))?;
        let preference = self.canonical.as_ref().map(|v| v.commit.preference());
        self.original.current_with_application(
            &media_guard::Check::new(web_time::Instant::now()),
            self.successor.as_ref(),
            preference.as_deref(),
            self.application.manifest(),
        )
    }
    fn read_published_successor(
        &self,
        anchor_receipt: &Observed,
        head_receipt: &Observed,
        intent: Value,
        check: &media_guard::Check,
    ) -> Result<JournalContinuation> {
        let result = self.read_published_successor_candidate(anchor_receipt, head_receipt, intent, check)?;
        let preference = self.canonical.as_ref().map(|v| v.commit.preference());
        self.original.current_with_application(check, Some(&result), preference.as_deref(), self.application.manifest())?;
        Ok(result)
    }
    fn read_published_successor_candidate(
        &self,
        anchor_receipt: &Observed,
        head_receipt: &Observed,
        intent: Value,
        check: &media_guard::Check,
    ) -> Result<JournalContinuation> {
        let basis = &self.original.body.basis;
        let previous_anchor = self.successor.as_ref().map(|v| v.anchor.as_ref()).unwrap_or(control(&self.original, 4)?);
        let previous_head = self.successor.as_ref().map(|v| v.head.as_ref()).unwrap_or(control(&self.original, 5)?);
        let pages = self.successor.as_ref().map(|v| v.pages.as_slice()).unwrap_or(&[]);
        let result = published_successor(
            &basis.whole_journal.directory,
            &basis.whole_journal.anchor,
            &basis.whole_journal.head,
            previous_anchor,
            previous_head,
            pages,
            anchor_receipt,
            head_receipt,
            intent,
            check,
        )?;
        Ok(result)
    }
}
// Read-only predecessor verification shared by both actual runtime transitions.
// Controls remain retained original physical observations, never supplied owners.
#[allow(clippy::too_many_arguments)]
fn published_successor(
    directory: &Path,
    anchor_path: &Path,
    head_path: &Path,
    previous_anchor_control: &Control,
    previous_head_control: &Control,
    previous_pages: &[Arc<Control>],
    anchor_receipt: &Observed,
    head_receipt: &Observed,
    intent: Value,
    check: &media_guard::Check,
) -> Result<JournalContinuation> {
    if anchor_receipt.path != anchor_path || head_receipt.path != head_path {
        return Err(error("cold native journal successor crossed its original scope"));
    }
    let anchor = Arc::new(observed(anchor_receipt, CATALOG, check)?);
    let head = Arc::new(observed(head_receipt, HEAD, check)?);
    let previous_anchor_observed = previous_anchor_control.observation()?;
    let previous_head_observed = previous_head_control.observation()?;
    if anchor_receipt.physical.ino == previous_anchor_observed.identity.inode && anchor_receipt.physical.dev == previous_anchor_observed.identity.device
        || head_receipt.physical.ino == previous_head_observed.identity.inode && head_receipt.physical.dev == previous_head_observed.identity.device
    {
        return Err(error("actual original journal successor publication required"));
    }
    let previous_anchor = previous_anchor_control.value();
    let previous_head = previous_head_control.value();
    let mut expected = previous_head.clone();
    let previous = previous_head["current"].as_array().filter(|v| v.len() <= 128).ok_or_else(|| error("bounded original journal current events required"))?;
    let current = head.value()["current"].as_array().filter(|v| v.len() <= 128).ok_or_else(|| error("bounded successor journal current events required"))?;
    let mut same_page = previous.clone();
    same_page.push(intent.clone());
    let page = if current == &same_page {
        expected["current"] = json!(same_page);
        None
    } else if current == &vec![intent] && !previous.is_empty() {
        let reference = &head.value()["events"];
        let sequence = previous_head["events"]["sequence"].as_u64().unwrap_or(0).checked_add(1).ok_or_else(|| error("journal event sequence overflow"))?;
        let hash = reference["sha256"].as_str().filter(|v| hash_ok(v)).ok_or_else(|| error("original event page hash required"))?;
        if reference.as_object().is_none_or(|v| v.len() != 3) || reference["sequence"] != sequence {
            return Err(error("exact next original journal event page required"));
        }
        let path = directory.join("pages").join(format!("events-{sequence}-{hash}.json"));
        let page = Control::capture(&path, PAGE, check)?;
        let observed = page.observation()?;
        let physical: Physical = serde_json::from_value(reference["physical"].clone()).map_err(error)?;
        physical.matches(observed)?;
        if observed.sha256 != hash
            || page.value()
                != &json!({"kind":"poema-original-Save-journal-page","version":1,
                "chain":"events","sequence":sequence,"previous":previous_head["events"],"records":previous})
        {
            return Err(error("original event page changed its predecessor or records"));
        }
        expected["events"] = reference.clone();
        expected["current"] = json!(current);
        Some(page)
    } else {
        return Err(error("cold runtime intent is not the sole expected original journal successor"));
    };
    if head.value() != &expected {
        return Err(error("foreign original journal fields changed with cold runtime intent"));
    }
    let mut expected_anchor = previous_anchor.clone();
    expected_anchor["head"] = json!({"physical":head_receipt.physical,"sha256":head_receipt.sha256});
    if anchor.value() != &expected_anchor {
        return Err(error("original anchor does not confirm the exact expected successor"));
    }
    let mut pages = previous_pages.to_vec();
    if let Some(page) = page {
        if pages.len() >= 16 {
            return Err(error("cold runtime journal continuation bound exceeded"));
        }
        pages.push(Arc::new(page));
    }
    let result = JournalContinuation { anchor, head, pages };
    Ok(result)
}

impl ColdRuntimePreparation for Pending {
    fn prepare_application_settlement(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_actual_application_settlement(s, request)
    }
    fn confirm_application_settlement_intent(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_actual_application_settlement_intent(s, receipt)
    }
    fn confirm_application_settlement_publication(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_actual_application_settlement_publication(s, receipt)
    }
    fn inspect_application_settlement(&self, s: &Session) -> Result<Value> {
        self.inspect_actual_application_settlement(s)
    }
    fn complete_current_image_release(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.complete_actual_current_image_release(s, request)
    }
    fn inspect_completed_current_image_release(&self, s: &Session) -> Result<Value> {
        let check = media_guard::Check::new(web_time::Instant::now());
        self.inspect_actual_completed_current_image(s, &check)?;
        check.check()?;
        self.current_image_completion_description()
    }
    fn prepare_current_image_release(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_actual_current_image_release(s, request)
    }
    fn confirm_checkpoint_publication(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_checkpoint_publication_successor(s, receipt)
    }
    fn publish_application_checkpoint(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.publish_application_checkpoint_files(s, request)
    }
    fn current(&self) -> Result<()> {
        self.application.current()?;
        if let Some(canonical) = &self.canonical {
            canonical.transition_current()?;
        }
        if self.attempted && self.successor.is_none() {
            return Err(error("cold runtime journal confirmation is incomplete; retained state cannot be reissued"));
        }
        if matches!(self.worker, WorkerState::Attempted) {
            return Err(error("cold worker construction is unresolved; no replay"));
        }
        if let Some(reservation) = &self.reservation {
            reservation.current(&self.cfg, &|| Ok(())).map_err(error)?;
        }
        if let WorkerState::Created { journal, .. } = &self.worker {
            // This is a read-only observation. Fence its complete original
            // source before and after, rather than recursively re-reading that
            // source at every metadata step. The journal still performs all of
            // its original physical, namespace, liveness and lock checks.
            self.source_current()?;
            let check = media_guard::Check::new(web_time::Instant::now());
            journal.current(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
        }
        self.source_current()
    }
    fn description(&self) -> Result<Value> {
        self.current()?;
        Ok(json!({"kind":"native_cold_Save_runtime_transition","version":1,
            "preparation":self.preparation,"journalConfirmed":self.successor.is_some(),
            "editingPaused":true,"persistenceStarted":matches!(self.worker,WorkerState::Created{..}),"writeAuthority":false,
            "worker":match &self.worker{WorkerState::Created{description,..}=>Some(description),_=>None},
            "workerConfirmed":matches!(self.worker,WorkerState::Created{confirmed:true,..}),
            "canonical":self.canonical.as_ref().map(|v|v.description()),
            "startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false}))
    }
    fn confirm(&mut self, receipt: &Value) -> Result<()> {
        if self.attempted {
            return Err(error("cold runtime journal confirmation was already attempted; no replay"));
        }
        self.attempted = true;
        self.reservation.as_ref().ok_or_else(|| error("actual unused journal reservation required"))?.current(&self.cfg, &|| Ok(())).map_err(error)?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let successor = self.read_successor(receipt, &check)?;
        self.successor = Some(successor);
        self.current()
    }
    fn worker_current(&self, persistence: Option<&crate::autosave::Persistence>) -> Result<()> {
        match (&self.worker, persistence) {
            (WorkerState::NotStarted, None) => Ok(()),
            (WorkerState::Created { journal, .. }, Some(p)) => {
                if p.last_sent.is_some() {
                    return Err(error("cold native synchronization queued input before service ownership transfer"));
                }
                self.source_current()?;
                let check = media_guard::Check::new(web_time::Instant::now());
                let actual = p.current_image_journal(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
                if !Arc::ptr_eq(journal, &actual) {
                    return Err(error("cold persistence belongs to another actual worker"));
                }
                self.source_current()
            }
            _ => Err(error("cold persistence ownership changed or its creation is unresolved")),
        }
    }
    fn create_worker(&mut self, prefs: crate::autosave::AutoSavePrefs) -> Result<crate::autosave::Persistence> {
        self.current()?;
        if self.successor.is_none() || !matches!(self.worker, WorkerState::NotStarted) {
            return Err(error("one confirmed cold intent and unused reservation required"));
        }
        let reservation = self.reservation.take().ok_or_else(|| error("actual one-use journal reservation missing"))?;
        self.worker = WorkerState::Attempted;
        let check = || self.source_current().map_err(|e| io::Error::other(e.to_string()));
        // One bounded read observer spans construction and its subsequent
        // journal inspection. Only pure retained-root reads use it; the same
        // complete original source guard still protects every constructor
        // effect, before and after each retained-root observation.
        let inspection = media_guard::Check::new(web_time::Instant::now());
        let inspect = || inspection.check().map_err(|e| io::Error::other(e.to_string()));
        let persistence = crate::autosave::Persistence::start_cold_reserved_with_inspection(&self.cfg, prefs, reservation, &check, &inspect).map_err(error)?;
        // The original constructor above keeps the complete source guard at
        // every write. These two inspections are read-only: bracket their
        // unchanged journal/worker checks with complete fresh source fences,
        // rather than recursively reading that source at every metadata step.
        self.source_current()?;
        let journal = persistence.current_image_journal(&inspect).map_err(error)?;
        let observed = journal.description(&inspect).map_err(error)?;
        self.source_current()?;
        let description = json!({"kind":"poema-native-Save-gated-worker","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],
            "preparation":self.preparation,"journal":observed,
            "workerStarted":true,"startupPending":true,"editingPaused":true,
            "workersMayResume":false,"startupAuthority":false,"writeAuthority":false,"fullApplicationClosure":false});
        self.worker = WorkerState::Created { journal, description, confirmation_attempted: false, confirmed: false };
        self.current()?;
        Ok(persistence)
    }
    fn reject_repeated_canonical_action(&self, action: crate::project_save::operation::CanonicalAction<'_>) -> Result<()> {
        self.refuse_repeated_canonical_effect(action)
    }
    fn canonical_current(&self, s: &Session) -> Result<()> {
        self.check_canonical(s)
    }
    fn prepare_canonical(&mut self, s: &Session) -> Result<Value> {
        self.prepare_canonical_publication(s)
    }
    fn confirm_canonical_intent(&mut self, receipt: &Value) -> Result<Value> {
        self.confirm_canonical_publication_intent(receipt)
    }
    fn publish_canonical(&mut self, s: &Session) -> Result<Value> {
        self.publish_canonical_publication(s)
    }
    fn confirm_canonical_observation(&mut self, receipt: &Value) -> Result<Value> {
        self.confirm_canonical_publication_observation(receipt)
    }
    fn service_bindings(&self) -> Option<&crate::agent_bridge::ColdServiceBindings> {
        self.canonical.as_ref().map(|canonical| canonical.commit.service_bindings())
    }
    fn bind_services(&mut self, s: &mut Session) -> Result<Value> {
        self.bind_canonical_services(s)
    }
    fn confirm_service_bindings(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_service_binding_observation(s, receipt)
    }
    fn retain_application_preparation(&mut self, s: &Session, value: &Value) -> Result<Value> {
        self.retain_application(s, value)
    }
    fn confirm_application_intent(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application(s, receipt)
    }
    fn confirm_application_checkpoint_archive_intent(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application_checkpoint_archive_intent_successor(s, receipt)
    }
    fn prepare_application_checkpoint_archive(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_application_checkpoint_archive_plan(s, request)
    }
    fn prepare_application_checkpoint(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_application_checkpoint_bytes(s, request)
    }
    fn prepare_application_records(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_application_records_plan(s, request)
    }
    fn confirm_application_records_intent(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application_records_intent_publication(s, receipt)
    }
    fn confirm_application_native_presentation(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application_native_presentation_publication(s, receipt)
    }
    fn prepare_application_root(&mut self, s: &Session, request: &Value) -> Result<Value> {
        self.prepare_application_root_plan(s, request)
    }
    fn confirm_application_root_intent(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application_root_intent_publication(s, receipt)
    }
    fn confirm_application_root_publication(&mut self, s: &Session, receipt: &Value) -> Result<Value> {
        self.confirm_application_root_file_publication(s, receipt)
    }
    fn confirm_worker(&mut self, receipt: &Value) -> Result<()> {
        bytes(receipt, INPUT)?;
        let WorkerState::Created { description, confirmation_attempted, .. } = &mut self.worker else {
            return Err(error("actual original gated worker required"));
        };
        if *confirmation_attempted {
            return Err(error("cold worker confirmation was already attempted; no replay"));
        }
        *confirmation_attempted = true;
        let expected_worker = description.clone();
        let receipt: WorkerReceipt = parse("native.coldSaveWorker", receipt)?;
        if receipt.kind != "poema-original-Save-cold-runtime-worker-receipt"
            || receipt.version != 1
            || receipt.preparation != self.preparation
            || receipt.worker != expected_worker
        {
            return Err(error("exact original gated worker receipt required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(
            &receipt.anchor,
            &receipt.head,
            json!({"kind":"native-runtime-worker","preparation":self.preparation,"worker":expected_worker}),
            &check,
        )?;
        self.successor = Some(next);
        let WorkerState::Created { confirmed, .. } = &mut self.worker else {
            return Err(error("cold worker owner changed"));
        };
        *confirmed = true;
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    fn root() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = crate::temp_dir().join(format!("poema-cold-journal-pages-{}-{n}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        std::fs::DirBuilder::new().mode(0o700).create(root.join("pages")).unwrap();
        root
    }
    fn check() -> media_guard::Check {
        media_guard::Check::new(web_time::Instant::now())
    }
    fn put(path: &Path, value: &Value) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_extension(format!("pending-{n}"));
        let mut f = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temp).unwrap();
        f.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
        f.sync_all().unwrap();
        std::fs::rename(temp, path).unwrap();
    }
    fn observation(path: &Path, max: u64) -> Observed {
        let c = Control::capture(path, max, &check()).unwrap();
        let o = c.observation().unwrap();
        Observed {
            path: path.into(),
            sha256: o.sha256.clone(),
            bytes: o.bytes,
            physical: Physical {
                dev: o.identity.device.clone(),
                ino: o.identity.inode.clone(),
                mode: o.identity.mode.to_string(),
                uid: o.identity.uid.to_string(),
                size: o.bytes.to_string(),
                mtime_ns: nanos(&o.modified).unwrap(),
                ctime_ns: nanos(&o.changed).unwrap(),
            },
        }
    }
    #[test]
    fn retained_journal_source_continuation_requires_same_actual_control_arcs_and_ordered_pages_not_matching_observations() {
        let root = root();
        let original = initial(&root);
        assert!(original.same_original(&original.clone()));
        let recaptured = JournalContinuation {
            anchor: Arc::new(Control::capture(&root.join("anchor.json"), CATALOG, &check()).unwrap()),
            head: Arc::new(Control::capture(&root.join("head.json"), HEAD, &check()).unwrap()),
            pages: Vec::new(),
        };
        original.current(&check()).unwrap();
        recaptured.current(&check()).unwrap();
        assert!(!original.same_original(&recaptured));
        let mut changed = original.clone();
        changed.pages.push(original.head.clone());
        assert!(!original.same_original(&changed));
        changed.pages.clear();
        changed.anchor = original.head.clone();
        assert!(!original.same_original(&changed));
        std::fs::remove_dir_all(root).unwrap();
    }
    fn initial(root: &Path) -> JournalContinuation {
        put(&root.join("head.json"), &json!({"kind":"original-head","events":null,"current":[{"kind":"old"}],"owner":"preserve-original"}));
        put(&root.join("anchor.json"), &json!({"kind":"original-anchor","head":{"initial":true},"scope":"preserve-original"}));
        JournalContinuation {
            anchor: Arc::new(Control::capture(&root.join("anchor.json"), CATALOG, &check()).unwrap()),
            head: Arc::new(Control::capture(&root.join("head.json"), HEAD, &check()).unwrap()),
            pages: Vec::new(),
        }
    }
    fn next(root: &Path, previous: &JournalContinuation, event: Value, roll: bool, tamper: &str) -> (Observed, Observed) {
        let old = previous.head.value();
        let mut head = old.clone();
        if roll {
            let sequence = old["events"]["sequence"].as_u64().unwrap_or(0) + 1;
            let mut page = json!({"kind":"poema-original-Save-journal-page","version":1,"chain":"events",
                "sequence":sequence,"previous":old["events"],"records":old["current"]});
            if tamper == "records" {
                page["records"] = json!([{"kind":"foreign"}]);
            }
            if tamper == "previous" {
                page["previous"] = json!({"foreign":"chain"});
            }
            use sha2::{Digest, Sha256};
            let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&page).unwrap()));
            let path = root.join("pages").join(format!("events-{sequence}-{hash}.json"));
            put(&path, &page);
            let o = observation(&path, PAGE);
            head["events"] = json!({"sequence":sequence,"sha256":hash,"physical":o.physical});
            head["current"] = json!([event]);
        } else {
            let mut events = old["current"].as_array().unwrap().clone();
            events.push(event);
            head["current"] = json!(events);
        }
        if tamper == "head" {
            head["owner"] = json!("foreign");
        }
        put(&root.join("head.json"), &head);
        let h = observation(&root.join("head.json"), HEAD);
        let mut anchor = previous.anchor.value().clone();
        anchor["head"] = json!({"physical":h.physical,"sha256":h.sha256});
        if tamper == "anchor" {
            anchor["scope"] = json!("foreign");
        }
        put(&root.join("anchor.json"), &anchor);
        let a = observation(&root.join("anchor.json"), CATALOG);
        (a, h)
    }
    fn advance(root: &Path, p: &JournalContinuation, a: &Observed, h: &Observed, event: Value) -> Result<JournalContinuation> {
        published_successor(root, &root.join("anchor.json"), &root.join("head.json"), &p.anchor, &p.head, &p.pages, a, h, event, &check())
    }
    #[test]
    fn original_append_and_repeated_rollover_retain_every_page_generation() {
        let root = root();
        let mut p = initial(&root);
        for (i, roll) in [false, true, false, true].into_iter().enumerate() {
            let e = json!({"kind":"native-runtime-event","index":i});
            let (a, h) = next(&root, &p, e.clone(), roll, "");
            p = advance(&root, &p, &a, &h, e).unwrap();
            p.current(&check()).unwrap();
        }
        assert_eq!(p.pages.len(), 2);
        let first = p.pages.first().unwrap();
        let record = first.value();
        let path = root.join("pages").join(format!("events-1-{}.json", first.observation().unwrap().sha256));
        let held = root.join("original-page-held");
        std::fs::rename(&path, &held).unwrap();
        put(&path, record);
        assert!(p.current(&check()).is_err());
        assert_eq!(std::fs::read(&held).unwrap(), std::fs::read(path).unwrap());
    }
    #[test]
    fn rollover_rejects_foreign_records_and_previous_page_chain() {
        for tamper in ["records", "previous"] {
            let root = root();
            let p = initial(&root);
            let event = json!({"kind":"native-runtime-intent"});
            let (a, h) = next(&root, &p, event.clone(), true, tamper);
            assert!(advance(&root, &p, &a, &h, event).is_err());
            assert_eq!(std::fs::read_dir(root.join("pages")).unwrap().count(), 1);
        }
    }
    #[test]
    fn successor_refuses_changed_original_head_or_anchor_fields() {
        for tamper in ["head", "anchor"] {
            let root = root();
            let p = initial(&root);
            let event = json!({"kind":"native-runtime-worker"});
            let (a, h) = next(&root, &p, event.clone(), false, tamper);
            assert!(advance(&root, &p, &a, &h, event).is_err());
        }
    }
    #[test]
    fn successor_refuses_copied_old_physical_receipts_even_with_current_bytes() {
        let root = root();
        let p = initial(&root);
        let old = observation(&root.join("head.json"), HEAD);
        let event = json!({"kind":"native-runtime-intent"});
        let (a, mut h) = next(&root, &p, event.clone(), false, "");
        h.physical.ino = old.physical.ino;
        h.physical.dev = old.physical.dev;
        assert!(advance(&root, &p, &a, &h, event).is_err());
    }

    #[test]
    fn original_journal_after_read_fence_keeps_every_control_generation_stop_and_original_deadline() {
        for index in 0..5 {
            let root = root();
            let mut journal = initial(&root);
            let mut files = vec![root.join("anchor.json"), root.join("head.json")];
            for n in 0..3 {
                let file = root.join("pages").join(format!("generated-{n}.json"));
                put(&file, &json!({"generatedPage":n,"preserve":"actual original control"}));
                journal.pages.push(Arc::new(Control::capture(&file, PAGE, &check()).unwrap()));
                files.push(file);
            }
            let same = journal.clone();
            let budget = check();
            journal.begin_application_read(&budget, ApplicationRead::Complete).unwrap();
            journal.fence_after_read(&budget).unwrap();
            journal.begin_application_read(&budget, ApplicationRead::FenceAfterRead).unwrap();
            assert!(journal.same_original(&same));
            let file = &files[index];
            let bytes = std::fs::read(file).unwrap();
            std::fs::rename(file, file.with_extension("retained-original")).unwrap();
            let mut replacement = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(file).unwrap();
            replacement.write_all(&bytes).unwrap();
            replacement.sync_all().unwrap();
            assert!(journal.fence_after_read(&budget).is_err(), "original control {index}");
            assert!(journal.begin_application_read(&budget, ApplicationRead::Complete).is_err());
            assert!(journal.begin_application_read(&budget, ApplicationRead::FenceAfterRead).is_err());
            assert!(journal.same_original(&same));
            assert_eq!(std::fs::read(file).unwrap(), bytes);
            std::fs::remove_dir_all(root).unwrap();
        }
        let root = root();
        let journal = initial(&root);
        journal.current(&check()).unwrap();
        let cancelled = media_guard::Check::with_stop(web_time::Instant::now(), Arc::new(std::sync::atomic::AtomicBool::new(true)));
        let expired = media_guard::Check::new(web_time::Instant::now() - std::time::Duration::from_secs(3600));
        for budget in [&cancelled, &expired] {
            assert!(journal.fence_after_read(budget).is_err());
            assert!(journal.begin_application_read(budget, ApplicationRead::Complete).is_err());
            assert!(journal.begin_application_read(budget, ApplicationRead::FenceAfterRead).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn original_journal_read_modes_refuse_changed_bytes_restored_mtime_and_redirected_or_missing_pages() {
        use std::os::unix::fs::symlink;
        for fault in ["restored-mtime", "page-parent", "missing-page", "mode"] {
            let root = root();
            let mut journal = initial(&root);
            let page = root.join("pages").join("generated.json");
            put(&page, &json!({"generated":true,"preserve":"same original source"}));
            journal.pages.push(Arc::new(Control::capture(&page, PAGE, &check()).unwrap()));
            let before = journal.clone();
            journal.current(&check()).unwrap();
            match fault {
                "restored-mtime" => {
                    let time = std::fs::metadata(&page).unwrap().modified().unwrap();
                    let mut bytes = std::fs::read(&page).unwrap();
                    bytes[0] = b' ';
                    std::fs::write(&page, bytes).unwrap();
                    std::fs::File::options().write(true).open(&page).unwrap().set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
                }
                "page-parent" => {
                    std::fs::rename(root.join("pages"), root.join("retained-pages")).unwrap();
                    symlink(root.join("retained-pages"), root.join("pages")).unwrap();
                }
                "missing-page" => std::fs::remove_file(&page).unwrap(),
                "mode" => {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o644)).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(journal.current(&check()).is_err(), "full original query {fault}");
            assert!(journal.fence_after_read(&check()).is_err(), "final original query {fault}");
            assert!(journal.same_original(&before));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn original_journal_complete_then_final_fence_balanced_component_preserves_actual_controls() {
        let root = root();
        // Generated bytes only. Sizes match the preserved original V83 image
        // intent head and settlement anchor; no old contents/owner are copied.
        fn sized(path: &Path, size: usize) {
            let mut value = json!({"generatedJournalReadOnly":true,"payload":""});
            let overhead = serde_json::to_vec(&value).unwrap().len();
            value["payload"] = json!("x".repeat(size.checked_sub(overhead).unwrap()));
            let raw = serde_json::to_vec(&value).unwrap();
            assert_eq!(raw.len(), size);
            let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(path).unwrap();
            file.write_all(&raw).unwrap();
            file.sync_all().unwrap();
        }
        sized(&root.join("anchor.json"), 1107);
        sized(&root.join("head.json"), 256892);
        let journal = JournalContinuation {
            anchor: Arc::new(Control::capture(&root.join("anchor.json"), CATALOG, &check()).unwrap()),
            head: Arc::new(Control::capture(&root.join("head.json"), HEAD, &check()).unwrap()),
            pages: Vec::new(),
        };
        let original = journal.clone();
        let before = [std::fs::read(root.join("anchor.json")).unwrap(), std::fs::read(root.join("head.json")).unwrap()];
        let budget = check();
        let read = |legacy: bool| {
            if legacy {
                journal.current(&budget).unwrap();
            } else {
                journal.begin_application_read(&budget, ApplicationRead::Complete).unwrap();
            }
            // The original selected Control byte reads and all their final
            // fences stay in BOTH paths. Only redundant journal rereads differ.
            for control in [&journal.anchor, &journal.head] {
                control.current(&budget).unwrap();
            }
            for control in [&journal.anchor, &journal.head] {
                control.fence(&budget).unwrap();
            }
            if legacy {
                journal.current(&budget).unwrap();
            } else {
                journal.fence_after_read(&budget).unwrap();
            }
            if legacy {
                journal.current(&budget).unwrap();
            } else {
                journal.begin_application_read(&budget, ApplicationRead::FenceAfterRead).unwrap();
            }
            for _ in 0..2 {
                for control in [&journal.anchor, &journal.head] {
                    control.fence(&budget).unwrap();
                }
            }
            if legacy {
                journal.current(&budget).unwrap();
            } else {
                journal.fence_after_read(&budget).unwrap();
            }
        };
        let current = || {
            journal.fence_after_read(&budget).unwrap();
            assert!(journal.same_original(&original));
            assert_eq!(std::fs::read(root.join("anchor.json")).unwrap(), before[0]);
            assert_eq!(std::fs::read(root.join("head.json")).unwrap(), before[1]);
        };
        read(true);
        read(false);
        current();
        let began = std::time::Instant::now();
        let mut pairs = Vec::new();
        for pair in 0..32 {
            assert!(began.elapsed() < std::time::Duration::from_secs(30));
            let mut elapsed = [0.0; 2];
            for side in if pair % 2 == 0 { [0, 1] } else { [1, 0] } {
                current();
                let start = std::time::Instant::now();
                for _ in 0..4 {
                    read(side == 0);
                }
                elapsed[side] = start.elapsed().as_secs_f64() * 1000.0;
                current();
            }
            pairs.push(json!({"pair":pair,"order":if pair%2==0 {vec!["baseline","candidate"]}else{vec!["candidate","baseline"]},"baselineMs":elapsed[0],"candidateMs":elapsed[1],"speedup":elapsed[0]/elapsed[1]}));
        }
        let mut ratios = pairs.iter().map(|v| v["speedup"].as_f64().unwrap()).collect::<Vec<_>>();
        ratios.sort_by(f64::total_cmp);
        let median = (ratios[15] + ratios[16]) / 2.0;
        let wins = ratios.iter().filter(|r| **r > 1.0).count();
        current();
        println!(
            "poema-journal-final-fence-pilot {}",
            json!({"pairs":pairs,"medianSpeedup":median,"candidateWins":wins,"iterationsPerSide":4,"anchorBytes":1107,"headBytes":256892,"actualControlArcsPreserved":true,"allBytesAndGenerationsPreserved":true,"nativeOwnerAndSourceWitnessNotConstructed":true,"wholeSourcePerformanceProved":false})
        );
        assert!(median >= 1.15 && wins >= 24, "predeclared original journal component criterion: {median}x,{wins}wins");
        std::fs::remove_dir_all(root).unwrap();
    }
}
