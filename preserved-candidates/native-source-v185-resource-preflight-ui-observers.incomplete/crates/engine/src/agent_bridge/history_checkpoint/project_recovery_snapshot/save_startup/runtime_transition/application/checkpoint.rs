//! The SAME cold owner's original COW checkpoint input. No disk write,
//! persistence notification, release or editor admission occurs here.
use super::*;
use filmcraft_project::Project;
mod archive;
mod completion;
mod release;

pub(super) struct Prepared {
    guard: crate::project_save::Guard,
    graph: Arc<Project>,
    native: Option<crate::agent_bridge::NativeJournalPending>,
    encoding_attempted: bool,
    encoded: Option<crate::autosave::held_checkpoint::Encoded>,
    archive_attempted: bool,
    archive: Option<archive::Archive>,
    current_release_attempted: bool,
    current_release_ready: bool,
    current_native: Option<crate::agent_bridge::NativeJournalPending>,
    completion: Option<completion::Completion>,
    description: Value,
}

impl Prepared {
    pub(super) fn capture(s: &Session, original: &Input, runtime: &Value) -> Result<Self> {
        let epoch = crate::agent_bridge::retained_native_epoch(s).ok_or_else(|| error("the same actual native epoch is required for checkpoint capture"))?;
        let worker = s.persistence.as_ref().ok_or_else(|| error("the same original persistence worker is required for checkpoint capture"))?;
        let frame = &original.body.basis.frame;
        if !worker.startup_pending()
            || worker.last_sent.is_some()
            || runtime["nativePid"] != std::process::id()
            || runtime["nativeSession"] != epoch
            || s.revision != frame.revision
            || s.path.as_deref() != Some(frame.project_path.as_str())
        {
            return Err(error("checkpoint capture requires the unchanged original held Session and unqueued startup worker"));
        }
        let saved = s.saved_file.as_ref().ok_or_else(|| error("original saved-file baseline required for complete checkpoint capture"))?;
        let guard = crate::project_save::guard(s)?;
        let native = crate::agent_bridge::history_checkpoint::journal::capture_retained(s)?;
        guard.current(s)?;
        let description = json!({
            "kind":"native_cold_Save_current_checkpoint_preparation","version":1,
            "nativePid":std::process::id(),"nativeSession":epoch,
            "privateInput":runtime["privateInput"],"sourceCapture":frame.capture,"afterFrameSha256":frame.sha256,
            "projectPath":s.path,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":saved,
            "nativeData":worker.data_dir,"sessionDirectory":worker.session_dir,
            "files":{"snapshot":worker.session_dir.join(crate::autosave::SNAPSHOT),
                "metadata":worker.session_dir.join(crate::autosave::META),
                "native":worker.session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE)},
            "capture":"original-complete-native-journal-cow","completeAuxiliaryRequired":true,
            "editingPaused":true,"persistenceStarted":true,"startupPending":true,"directSynchronizationHeld":true,
            "serialized":false,"filesWritten":false,"workerReleased":false,"current":false,
            "writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false
        });
        bytes(&description, 16384)?;
        Ok(Self {
            guard,
            graph: s.project.clone(),
            native: Some(native),
            encoding_attempted: false,
            encoded: None,
            archive_attempted: false,
            archive: None,
            current_release_attempted: false,
            current_release_ready: false,
            current_native: None,
            completion: None,
            description,
        })
    }

    fn base_current(&self, s: &Session) -> Result<()> {
        self.guard.current(s)?;
        if !Arc::ptr_eq(&self.graph, &s.project)
            || self.description["nativeSession"] != json!(crate::agent_bridge::retained_native_epoch(s))
            || s.persistence.as_ref().is_none_or(|p| {
                !p.startup_pending()
                    || p.last_sent.is_some()
                    || self.description["nativeData"] != json!(p.data_dir)
                    || self.description["sessionDirectory"] != json!(p.session_dir)
            })
        {
            return Err(error("the actual original captured checkpoint and gated persistence owner changed"));
        }
        Ok(())
    }

    pub(super) fn description(&self) -> Value {
        self.description.clone()
    }

    pub(super) fn resolved(&self) -> Result<()> {
        if self.completion.is_some() {
            return Err(error("the original worker continuation was attempted; use its actual acknowledged-phase reader without replay"));
        }
        if !self.current_release_attempted && (self.current_release_ready || self.current_native.is_some()) {
            return Err(error("original current-release custody has no actual preparation attempt"));
        }
        if self.current_release_attempted && (!self.current_release_ready || self.current_native.is_none()) {
            return Err(error("original current-release preparation is unresolved; retain its actual owner without replay"));
        }
        if self.encoding_attempted {
            if self.native.is_some() || (self.encoded.is_none() && !self.archive.as_ref().is_some_and(|a| a.has_publication())) {
                return Err(error("original checkpoint encoding is unresolved; no replay or release"));
            }
        } else if self.native.is_none() || self.encoded.is_some() {
            return Err(error("original checkpoint COW custody is incomplete"));
        }
        if self.archive_attempted && self.archive.is_none()
            || self.archive.is_some() && (!self.archive_attempted || (self.encoded.is_none() && !self.archive.as_ref().is_some_and(|a| a.has_publication())))
        {
            return Err(error("original checkpoint archive preparation is unresolved; no replay or release"));
        }
        if let Some(archive) = &self.archive {
            archive.publication_resolved()?;
        }
        Ok(())
    }
    pub(super) fn current(&self, s: &Session) -> Result<()> {
        self.resolved()?;
        self.base_current(s)?;
        if let Some(encoded) = &self.encoded {
            encoded.current(s.persistence.as_ref().ok_or_else(|| error("original checkpoint worker disappeared"))?).map_err(error)?;
        }
        if let Some(archive) = &self.archive {
            archive.current(s.persistence.as_ref().ok_or_else(|| error("original checkpoint archive worker disappeared"))?)?;
        }
        Ok(())
    }

    pub(super) fn prepare_archive(&mut self, s: &Session, original: &Input, runtime: &Value, source: &impl Fn() -> Result<()>) -> Result<Value> {
        self.current(s)?;
        if self.archive_attempted {
            return Err(error("the same original checkpoint archive was already prepared; no replay"));
        }
        let encoded = self.encoded.as_ref().ok_or_else(|| error("the actual same-worker encoded checkpoint is required before archive preparation"))?;
        source()?;
        self.archive_attempted = true;
        let archive = archive::Archive::prepare(s, encoded, &self.description, original, runtime)?;
        self.archive = Some(archive);
        source()?;
        self.current(s)?;
        Ok(self.archive.as_ref().ok_or_else(|| error("the original checkpoint archive preparation disappeared"))?.description())
    }

    // Keep the original strict entry point for internal callers and its
    // original actual-worker compatibility tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn publish(
        &mut self,
        s: &Session,
        source: crate::autosave::held_checkpoint::publication::Source,
        current: &impl Fn() -> Result<()>,
    ) -> Result<Value> {
        self.publish_observed(s, source, current, None)
    }
    pub(super) fn publish_with_wait(
        &mut self,
        s: &Session,
        source: crate::autosave::held_checkpoint::publication::Source,
        current: &impl Fn() -> Result<()>,
        wait: &impl Fn() -> Result<()>,
    ) -> Result<Value> {
        self.publish_observed(s, source, current, Some(wait))
    }
    fn publish_observed(
        &mut self,
        s: &Session,
        source: crate::autosave::held_checkpoint::publication::Source,
        current: &impl Fn() -> Result<()>,
        wait: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Value> {
        self.current(s)?;
        current()?;
        source.current().map_err(error)?;
        let (root, epoch, reserve) = self.archive.as_mut().ok_or_else(|| error("the actual retained archive is required"))?.begin_publication()?;
        let encoded = self.encoded.take().ok_or_else(|| error("the SAME original encoded COW checkpoint is required; no reconstruction"))?;
        let worker = s.persistence.as_ref().ok_or_else(|| error("the actual original held worker disappeared"))?;
        let complete = || self.base_current(s).and_then(|_| current()).map_err(|e| e.to_string());
        let published = match wait {
            Some(wait) => worker.publish_held_current_checkpoint_with_wait(encoded, &epoch, root, reserve, source, &complete, &|| {
                self.base_current(s).and_then(|_| wait()).map_err(|e| e.to_string())
            }),
            None => worker.publish_held_current_checkpoint(encoded, &epoch, root, reserve, source, complete),
        }
        .map_err(error)?;
        // Retain the actual returned writer owners before any later guard may
        // fail. Unknown or lost replies never restore the consumed Encoded.
        self.archive.as_mut().ok_or_else(|| error("the actual archive disappeared"))?.retain_publication(published, worker)?;
        self.current(s)?;
        current()?;
        self.archive.as_ref().ok_or_else(|| error("the actual archive disappeared"))?.publication_description(worker)
    }
    pub(super) fn publication_complete(&self) -> bool {
        self.archive.as_ref().is_some_and(|archive| archive.has_publication())
    }
    pub(super) fn begin_publication_observation(&mut self, s: &Session) -> Result<Value> {
        // The old original head has legitimately advanced. Only the original
        // COW/epoch guard may be used before validating its exact successor.
        self.base_current(s)?;
        self.archive.as_mut().ok_or_else(|| error("actual original archive missing"))?.begin_observation()
    }
    pub(super) fn confirm_publication_observation(&mut self, s: &Session) -> Result<()> {
        self.base_current(s)?;
        let worker = s.persistence.as_ref().ok_or_else(|| error("original checkpoint worker disappeared"))?;
        self.archive.as_mut().ok_or_else(|| error("actual original archive missing"))?.confirm_observation(worker)?;
        self.current(s)
    }
    pub(super) fn archive_intent_resolved(&self) -> Result<()> {
        if let Some(archive) = &self.archive {
            archive.intent_resolved()?;
        }
        Ok(())
    }
    pub(super) fn begin_archive_intent(&mut self, s: &Session) -> Result<Value> {
        self.current(s)?;
        self.archive.as_mut().ok_or_else(|| error("the SAME actual retained archive plan is required"))?.begin_intent()
    }
    pub(super) fn confirm_archive_intent(&mut self) -> Result<()> {
        self.archive.as_mut().ok_or_else(|| error("the actual archive plan disappeared"))?.confirm_intent()
    }
    /// Keep the original Guard and COW graph while the SAME original gated
    /// worker consumes its complete native input exactly once. The callback
    /// checks the retained source controls; the caller brackets this pure read
    /// with the original complete source/canonical observation.
    pub(super) fn encode(&mut self, s: &Session, source: &impl Fn() -> Result<()>) -> Result<Value> {
        self.current(s)?;
        if self.encoding_attempted {
            return Err(error("the original checkpoint encoding was already attempted; no replay"));
        }
        source()?;
        self.encoding_attempted = true;
        let native = self.native.take().ok_or_else(|| error("the original complete native COW input disappeared"))?;
        let worker = s.persistence.as_ref().ok_or_else(|| error("the same original held persistence worker is required"))?;
        let encoded = worker
            .encode_held_current_checkpoint(self.graph.clone(), s.revision, s.path.clone(), native, || {
                self.base_current(s).and_then(|_| source()).map_err(|e| e.to_string())
            })
            .map_err(error)?;
        // Retain the actual opaque result before any subsequent observation
        // can fail. An unresolved attempt is never reset or encoded again.
        self.encoded = Some(encoded);
        self.current(s)?;
        source()?;
        let encoded = self.encoded.as_ref().ok_or_else(|| error("the original encoded checkpoint disappeared"))?;
        Ok(json!({"preparation":self.description,"bytes":encoded.description(worker).map_err(error)?}))
    }
}

#[cfg(test)]
mod tests;
