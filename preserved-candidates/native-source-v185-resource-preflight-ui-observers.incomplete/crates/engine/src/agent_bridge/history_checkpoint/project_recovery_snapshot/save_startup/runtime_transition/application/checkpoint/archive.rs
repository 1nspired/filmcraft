//! One exact absent archive location, retained from the SAME actual worker's
//! original data frame. A plan is read-only: no directory, file, publisher or
//! release owner can be reconstructed from this description.
use super::*;
mod release;
use filmcraft_format::retained::preview_context::{DirectoryFrame, allocation};

pub(super) struct Archive {
    data: Arc<DirectoryFrame>,
    root: Arc<DirectoryFrame>,
    journal: Arc<crate::autosave::current_startup::Journal>,
    description: Value,
    intent_attempted: bool,
    intent_confirmed: bool,
    epoch: String,
    reserve: u64,
    publication_attempted: bool,
    published: Option<crate::autosave::held_checkpoint::publication::Published>,
    release_attempted: bool,
    release_ready: bool,
    release: Option<crate::autosave::held_checkpoint::publication::release::ReleasePreparation>,
    publication_result: Option<Value>,
    observation_attempted: bool,
    observation_confirmed: bool,
}
impl Archive {
    pub(super) fn prepare(
        s: &Session,
        encoded: &crate::autosave::held_checkpoint::Encoded,
        capture: &Value,
        original: &Input,
        runtime: &Value,
    ) -> Result<Self> {
        let worker = s.persistence.as_ref().ok_or_else(|| error("the actual original checkpoint worker is required"))?;
        let epoch =
            crate::agent_bridge::retained_native_epoch(s).filter(|v| token_ok(v)).ok_or_else(|| error("the actual original checkpoint epoch is required"))?;
        let basis = &original.body.basis;
        if capture["nativeSession"] != epoch
            || runtime["nativeSession"] != epoch
            || runtime["nativePid"] != std::process::id()
            || capture["nativeData"] != json!(worker.data_dir)
            || worker.data_dir != basis.input.native_data
            || runtime["privateInput"] != capture["privateInput"]
            || basis.reserve_bytes == 0
        {
            return Err(error("archive preparation crossed the actual original COW, worker, native input or reserve"));
        }
        if capture["sourceCapture"] != basis.frame.capture || capture["afterFrameSha256"] != basis.frame.sha256 {
            return Err(error("archive preparation crossed its retained original source frame"));
        }
        Self::capture(s, encoded, capture, runtime, basis.reserve_bytes)
    }
    // Shared original-worker/frame composition. Only the actual Cold wrapper
    // above supplies the original Input scope in production.
    fn capture(s: &Session, encoded: &crate::autosave::held_checkpoint::Encoded, capture: &Value, runtime: &Value, reserve_bytes: u64) -> Result<Self> {
        let worker = s.persistence.as_ref().ok_or_else(|| error("the original held checkpoint worker is required"))?;
        let epoch = crate::agent_bridge::retained_native_epoch(s).filter(|e| token_ok(e)).ok_or_else(|| error("the original native epoch is required"))?;
        if reserve_bytes == 0
            || capture["nativeSession"] != epoch
            || runtime["nativeSession"] != epoch
            || runtime["nativePid"] != std::process::id()
            || capture["nativeData"] != json!(worker.data_dir)
            || capture["privateInput"] != runtime["privateInput"]
        {
            return Err(error("the archive location crossed its original native worker, epoch, input or reserve"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let data = encoded.retained_data_frame(worker, &io).map_err(error)?;
        let data_record = data.record();
        if data_record.path != worker.data_dir || !data_record.missing.is_empty() {
            return Err(error("the same existing original worker data frame is required"));
        }
        let journal = worker.current_image_journal(&io).map_err(error)?;
        if !journal.belongs_to(&data_record) {
            return Err(error("the actual checkpoint journal crossed its original data parent"));
        }
        let root_path = worker.data_dir.join(format!(".poema-checkpoint-{epoch}"));
        let root = DirectoryFrame::capture(&root_path, &io).map_err(error)?;
        let root_record = root.record();
        if root.exists() || root_record.anchor != data_record.anchor || root_record.missing != [root_path.clone()] {
            return Err(error("the checkpoint archive must be exactly one originally absent child of its actual worker data directory"));
        }
        let space = data.space(&io).map_err(error)?;
        if space.read_only || space.device != data_record.anchor.identity.device {
            return Err(error("the original checkpoint archive storage is unavailable; no fallback"));
        }
        let files_directory = root_path.join("files");
        let writes_directory = root_path.join("writes");
        let mut files = Vec::new();
        let mut logical_bytes = 0u64;
        let mut allocated = 0u64;
        for (name, bytes) in encoded.files(worker).map_err(error)? {
            let count = u64::try_from(bytes.len()).map_err(error)?;
            if count == 0 {
                return Err(error("complete original checkpoint files must be nonempty"));
            }
            logical_bytes = logical_bytes.checked_add(count).ok_or_else(|| error("checkpoint byte allowance overflow"))?;
            let payload = allocation(count, space.allocation_unit).map_err(error)?.checked_mul(2).ok_or_else(|| error("checkpoint allocation overflow"))?;
            let controls =
                allocation(64 * 1024, space.allocation_unit).map_err(error)?.checked_mul(8).ok_or_else(|| error("checkpoint control allowance overflow"))?;
            allocated = allocated.checked_add(payload).and_then(|v| v.checked_add(controls)).ok_or_else(|| error("complete checkpoint allocation overflow"))?;
            files.push(json!({"name":name,"path":files_directory.join(name),"bytes":count,"sha256":crate::saved_file::sha256(bytes)}));
        }
        let directories =
            allocation(4096, space.allocation_unit).map_err(error)?.checked_mul(3).ok_or_else(|| error("checkpoint directory allowance overflow"))?;
        allocated = allocated.checked_add(directories).ok_or_else(|| error("complete checkpoint directory allowance overflow"))?;
        let minimum = reserve_bytes.checked_add(allocated).ok_or_else(|| error("checkpoint reserve overflow"))?;
        if space.free < minimum {
            return Err(error("the full original checkpoint archive would consume its reserved storage"));
        }
        let description = json!({"kind":"poema-native-Save-current-checkpoint-archive-plan","version":1,
            "nativePid":std::process::id(),"nativeSession":epoch,"privateInput":runtime["privateInput"],
            "sourceCapture":capture["sourceCapture"],"afterFrameSha256":capture["afterFrameSha256"],
            "projectPath":s.path,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":capture["savedFile"],
            "data":data_record,"archive":root_record,"directories":{"files":files_directory,"writes":writes_directory},
            "files":files,"logicalBytes":logical_bytes,"allocatedAllowance":allocated,"reserveBytes":reserve_bytes,"allocationUnit":space.allocation_unit,
            "directoryMode":0o700,"fileMode":0o600,"sameOriginalWorker":true,"completeAuxiliaryRequired":true,
            "serialized":true,"editingPaused":true,"startupPending":true,"archiveCreated":false,"filesWritten":false,
            "workerReleased":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false});
        bytes(&description, 32768)?;
        let result = Self {
            data,
            root,
            journal,
            description,
            intent_attempted: false,
            intent_confirmed: false,
            epoch: epoch.to_owned(),
            reserve: reserve_bytes,
            publication_attempted: false,
            published: None,
            release_attempted: false,
            release_ready: false,
            release: None,
            publication_result: None,
            observation_attempted: false,
            observation_confirmed: false,
        };
        result.current(worker)?;
        Ok(result)
    }
    pub(super) fn current(&self, worker: &crate::autosave::Persistence) -> Result<()> {
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let journal = worker.current_image_journal(&io).map_err(error)?;
        if !Arc::ptr_eq(&journal, &self.journal) || !journal.belongs_to(&self.data.record()) || worker.data_dir != self.data.record().path {
            return Err(error("the retained checkpoint archive belongs to another actual original worker"));
        }
        self.data.current(&io).map_err(error)?;
        self.publication_resolved()?;
        if let Some(published) = &self.published {
            published.current(worker, &io).map_err(error)?;
        } else if let Some(prepared) = &self.release {
            prepared.held_current(worker, &io).map_err(error)?;
        } else {
            self.root.current(&io).map_err(error)?;
        }
        check.check()
    }
    pub(super) fn publication_resolved(&self) -> Result<()> {
        if self.publication_attempted != self.has_publication()
            || (self.observation_attempted && !self.observation_confirmed)
            || (self.release_attempted && !self.release_ready)
        {
            return Err(error("the original checkpoint publication is unresolved; no replay or worker release"));
        }
        Ok(())
    }
    pub(super) fn has_publication(&self) -> bool {
        (self.published.is_some() ^ self.release.is_some()) && self.publication_result.is_some()
    }
    pub(super) fn begin_publication(&mut self) -> Result<(Arc<DirectoryFrame>, String, u64)> {
        self.intent_resolved()?;
        if !self.intent_confirmed || self.publication_attempted {
            return Err(error("one actual acknowledged original archive intent and unused publication are required"));
        }
        self.publication_attempted = true;
        Ok((self.root.clone(), self.epoch.clone(), self.reserve))
    }
    pub(super) fn retain_publication(
        &mut self,
        published: crate::autosave::held_checkpoint::publication::Published,
        worker: &crate::autosave::Persistence,
    ) -> Result<()> {
        if !self.publication_attempted || self.published.is_some() {
            return Err(error("the same original checkpoint publication attempt is required"));
        }
        self.published = Some(published);
        // Keep actual ownership before a later read can refuse. The descriptor
        // only correlates the next original event; it cannot replace this owner.
        let value = self.published.as_ref().ok_or_else(|| error("actual original publication disappeared"))?.description(worker).map_err(error)?;
        self.publication_result = Some(value);
        Ok(())
    }
    pub(super) fn publication_description(&self, worker: &crate::autosave::Persistence) -> Result<Value> {
        self.publication_resolved()?;
        if let Some(prepared) = &self.release {
            prepared.held_description(worker).map_err(error)
        } else {
            self.published.as_ref().ok_or_else(|| error("actual original checkpoint publication required"))?.description(worker).map_err(error)
        }
    }
    pub(super) fn begin_observation(&mut self) -> Result<Value> {
        self.publication_resolved()?;
        if !self.has_publication() || self.observation_attempted {
            return Err(error("one complete original checkpoint publication and unused observation are required"));
        }
        self.observation_attempted = true;
        Ok(json!({"kind":"native-checkpoint-archive-observation","plan":self.description,
            "publication":self.publication_result.as_ref().ok_or_else(|| error("actual original publication description missing"))?}))
    }
    pub(super) fn confirm_observation(&mut self, worker: &crate::autosave::Persistence) -> Result<()> {
        if !self.observation_attempted || self.observation_confirmed || !self.has_publication() {
            return Err(error("the same actual checkpoint publication observation attempt is required"));
        }
        // The actual source witness has advanced only through the original
        // validated journal successor. Reinspect the SAME native owners/bytes.
        let value = self.published.as_ref().ok_or_else(|| error("actual publication missing"))?.description(worker).map_err(error)?;
        if self.publication_result.as_ref() != Some(&value) {
            return Err(error("the actual original checkpoint publication changed across its journal observation"));
        }
        self.observation_confirmed = true;
        Ok(())
    }
    pub(super) fn intent_resolved(&self) -> Result<()> {
        if self.intent_attempted && !self.intent_confirmed {
            return Err(error("the original archive intent acknowledgement is unresolved; no replay or release"));
        }
        Ok(())
    }
    pub(super) fn begin_intent(&mut self) -> Result<Value> {
        if self.intent_attempted {
            return Err(error("the original archive intent acknowledgement was already attempted"));
        }
        self.intent_attempted = true;
        Ok(self.description())
    }
    pub(super) fn confirm_intent(&mut self) -> Result<()> {
        if !self.intent_attempted || self.intent_confirmed {
            return Err(error("the same original archive intent attempt is required"));
        }
        self.intent_confirmed = true;
        Ok(())
    }
    pub(super) fn description(&self) -> Value {
        self.description.clone()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) fn published(acknowledged: bool) -> (super::tests::Scratch, Session, Prepared) {
    use super::tests::fixture;
    let (d, s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    let mut capture = p.description();
    capture["privateInput"] = json!({"path":"/generated/private/input.json","bytes":100,"sha256":"a".repeat(64)});
    capture["sourceCapture"] = json!("11111111-1111-4111-8111-111111111111");
    capture["afterFrameSha256"] = json!("b".repeat(64));
    capture["savedFile"] = json!(s.saved_file);
    let runtime = json!({"nativePid":std::process::id(),"nativeSession":capture["nativeSession"],"privateInput":capture["privateInput"]});
    let archive = Archive::capture(&s, p.encoded.as_ref().unwrap(), &capture, &runtime, 1024 * 1024).unwrap();
    p.archive = Some(archive);
    p.archive_attempted = true;
    p.begin_archive_intent(&s).unwrap();
    p.confirm_archive_intent().unwrap();
    p.publish(&s, Arc::new(|| Ok(())), &|| Ok(())).unwrap();
    if acknowledged {
        p.begin_publication_observation(&s).unwrap();
        // Actual production Pending independently validates/advances its real
        // original host journal successor before this component confirmation.
        p.confirm_publication_observation(&s).unwrap();
    }
    (d, s, p)
}
