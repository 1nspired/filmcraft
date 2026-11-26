//! Once-only actual Cold controller preparation before original worker release.
//! It consumes no serialized owner and emits no positive admission authority.
use super::*;
impl Pending {
    pub(in super::super) fn prepare_actual_current_image_release(&mut self, s: &Session, request: &Value) -> Result<Value> {
        bytes(request, 16384)?;
        if !fields(request, "kind,version,nativeSession,privateInput")
            || request["kind"] != "poema-original-Save-native-current-image-prepare"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("one closed current-image preparation on the same actual private native pipe is required"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed || self.application.prepared.is_none() {
            return Err(error("the SAME actual acknowledged application preparation is required"));
        }
        self.application.root.as_ref().ok_or_else(|| error("the actual acknowledged Root is missing"))?.acknowledged_manifest()?;
        self.application.records.as_ref().ok_or_else(|| error("the actual acknowledged records are missing"))?.checkpoint_ready()?;
        let witness = self.application.publication_source.clone().ok_or_else(|| error("the SAME original complete publication source is missing"))?;
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the SAME actual canonical owner is missing"))?.retained_published()?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let full = || witness.inspect_held_source(&check).map_err(|e| io::Error::other(e.to_string()));
        let after = || witness.fence_held_source(&check).map_err(|e| io::Error::other(e.to_string()));
        let source = filmcraft_format::retained::ReadSource::new(&full, &after);
        let current = || canonical.inspect_published_source_read(s, &source);
        let checkpoint = self.application.checkpoint.as_mut().ok_or_else(|| error("the SAME original captured checkpoint is required"))?;
        let result = checkpoint.prepare_current_release(s, &current, &check)?;
        check.check()?;
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        check.check()?;
        Ok(result)
    }
}

// The original engine Cold action retains this SAME owner across completion.
// Desktop/host admission and original metadata settlement remain separate.
// Neither this success nor its wire request grants application/runtime readiness.
impl Pending {
    pub(in super::super) fn complete_actual_current_image_release(&mut self, s: &Session, request: &Value) -> Result<Value> {
        if self.application.image_completion_attempted {
            return Err(error("the SAME actual original current-image completion was already attempted; no replay"));
        }
        self.application.image_completion_attempted = true;
        bytes(request, INPUT)?;
        if !fields(request, "kind,version,nativeSession,privateInput,intent")
            || request["kind"] != "poema-original-Save-native-current-image-complete"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("the same actual private native current-image completion request is required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let receipt: IntentReceipt = parse("native.coldSaveCurrentImageIntent", &request["intent"])?;
        let checkpoint = self.application.checkpoint.as_ref().ok_or_else(|| error("the SAME original checkpoint is required before image intent"))?;
        if receipt.kind != "poema-original-Save-native-current-image-intent-receipt"
            || receipt.version != 1
            || receipt.event != json!({"kind":"native-current-image-intent","checkpoint":checkpoint.description()})
        {
            return Err(error("the SAME actual original current-image intent journal receipt is required"));
        }
        let previous = self.successor.clone().ok_or_else(|| error("the SAME original current-image journal predecessor is required"))?;
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.application.publication_source.as_ref().ok_or_else(|| error("the SAME actual original image source is required"))?.advance_with_check(
            &previous,
            next.clone(),
            &check,
        )?;
        self.successor = Some(next);
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed || self.application.prepared.is_none() {
            return Err(error("the SAME original acknowledged Cold application is required"));
        }

        self.inspect_actual_application_release_controls(&check)?;
        let witness = self.application.publication_source.clone().ok_or_else(|| error("the SAME actual original source witness is required"))?;
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the SAME actual original canonical owner is required"))?.retained_published()?;
        let checkpoint = self.application.checkpoint.as_mut().ok_or_else(|| error("the actual complete original checkpoint is required"))?;
        let frame = checkpoint.retained_release_frame(s, &check)?;
        let source = witness.release_read(&frame, &check);
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let memory = || canonical.inspect_release_memory(s, &frame, &|| crate::autosave::held_checkpoint::publication::CheckpointSource::current(&source), &io);
        checkpoint.complete_current_release(s, &source, &memory, &check)?;
        checkpoint.current_completed_release(s, &source, &memory, &check)?;
        check.check()?;
        self.current_image_completion_description()
    }

    fn inspect_actual_application_release_controls(&self, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        self.application.root.as_ref().ok_or_else(|| error("the SAME acknowledged actual Root is required"))?.acknowledged_manifest()?;
        self.application.records.as_ref().ok_or_else(|| error("the SAME original records are required"))?.checkpoint_ready()?;
        self.application.source.as_ref().ok_or_else(|| error("the SAME actual replacement source is required"))?.current(check)?;
        check.check()
    }
}

impl Pending {
    /// Actual retained completed-image observation for later original metadata
    /// settlement. Never call the old gated current/status readers after Resume.
    pub(in super::super) fn inspect_actual_completed_current_image(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        if !self.application.confirmed || self.application.prepared.is_none() {
            return Err(error("the SAME actual acknowledged application is required"));
        }
        self.inspect_actual_application_release_controls(check)?;
        if let Some(settlement) = &self.application.settlement {
            if let Some((catalog, activity)) = settlement.completed_controls()? {
                let successor = self.successor.as_ref().ok_or_else(|| error("the SAME actual settled original journal is required"))?;
                return self.inspect_completed_settlement_source(s, successor, successor, catalog, activity, check);
            }
        }
        let witness = self.application.publication_source.as_ref().ok_or_else(|| error("the SAME actual source witness is required"))?;
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the SAME actual canonical owner is required"))?.retained_published()?;
        let checkpoint = self.application.checkpoint.as_ref().ok_or_else(|| error("the SAME completed checkpoint is required"))?;
        let frame = checkpoint.retained_release_frame(s, check)?;
        let source = witness.release_read(&frame, check);
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let memory = || canonical.inspect_release_memory(s, &frame, &|| crate::autosave::held_checkpoint::publication::CheckpointSource::current(&source), &io);
        checkpoint.current_completed_release(s, &source, &memory, check)?;
        check.check()
    }
}

impl Pending {
    /// Description emitted only after the actual original receipt/readback
    /// checks. These fields cannot reconstruct any receipt, owner or admission.
    pub(in super::super) fn current_image_completion_description(&self) -> Result<Value> {
        let files =
            self.application.checkpoint.as_ref().ok_or_else(|| error("the SAME actual completed checkpoint is required"))?.actual_current_image_files()?;
        Ok(json!({"kind":"native_cold_Save_current_image_completion","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],
            "originalReleaseAcknowledged":true,"originalInputAcknowledged":true,
            "originalJournalFlushAcknowledged":true,"completeCurrentImageVerified":true,
            "workerReleased":true,"editingPaused":true,"directSynchronizationHeld":true,
            "startupAuthority":false,"writeAuthority":false,"workersMayResume":false,
            "runtimeReadiness":"not-inferred-from-current-image","runtimeImage":files,"fullApplicationClosure":false}))
    }
}
