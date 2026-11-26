//! Retain the SAME original worker's real release/FIFO/readback receipts.
//! This component creates no editor admission or catalog/activity settlement.
//! The actual Cold controller must supply its complete original source/Root/
//! records/canonical/runtime closure before invoking this private continuation.
use super::*;
use crate::autosave::{
    CompleteCurrentImage, CurrentImageRelease, OriginalCurrentImageInput, OriginalJournalFlush, Persistence,
    held_checkpoint::publication::{CheckpointSource, release::ReleasePreparation},
};

#[derive(Default)]
pub(super) struct Completion {
    release: Option<CurrentImageRelease>,
    input_attempted: bool,
    input: Option<OriginalCurrentImageInput>,
    flush: Option<OriginalJournalFlush>,
    image: Option<CompleteCurrentImage>,
}

struct Context<'a> {
    session: &'a Session,
    guard: &'a crate::project_save::Guard,
    graph: &'a Arc<Project>,
    description: &'a Value,
    worker: &'a Persistence,
    archive: &'a ReleasePreparation,
    source: &'a dyn CheckpointSource,
    live: &'a dyn Fn() -> Result<()>,
    check: &'a media_guard::Check,
}
impl Context<'_> {
    fn current(&self) -> Result<()> {
        self.check.check()?;
        let s = self.session;
        self.guard.current(s)?;
        if !Arc::ptr_eq(self.graph, &s.project)
            || self.description["nativeSession"] != json!(crate::agent_bridge::retained_native_epoch(s))
            || self.description["nativeData"] != json!(self.worker.data_dir)
            || self.description["sessionDirectory"] != json!(self.worker.session_dir)
            || self.worker.last_sent.is_some()
        {
            return Err(error("the original current-image continuation crossed its actual Session, COW image or ordinary synchronization"));
        }
        // This actual constructor frame was captured while the SAME worker was
        // held. Its stage-correct read never weakens/reuses Journal.current().
        self.archive.original_journal_frame().assert_worker(self.worker).map_err(error)?;
        let io = || self.check.check().map_err(|e| io::Error::other(e.to_string()));
        (self.live)()?;
        let observed = self.archive.inspect_transition(self.worker, self.source, &io).map_err(error);
        // A failed file read still finishes the original caller memory check.
        let live_after = (self.live)();
        self.guard.current(s)?;
        self.archive.original_journal_frame().assert_worker(self.worker).map_err(error)?;
        self.check.check()?;
        observed.and(live_after)
    }
}
impl Completion {
    fn current(&self, c: &Context<'_>) -> Result<()> {
        c.current()?;
        let release = self.release.as_ref().ok_or_else(|| error("the actual original Prepare/Resume acknowledgement is unresolved"))?;
        let input = self.input.as_ref().ok_or_else(|| error("the actual original FIFO input acknowledgement is unresolved"))?;
        let flush = self.flush.as_ref().ok_or_else(|| error("the actual original FIFO flush acknowledgement is unresolved"))?;
        let image = self.image.as_ref().ok_or_else(|| error("the complete original current-image readback is unresolved"))?;
        if !self.input_attempted {
            return Err(error("the original complete COW was not consumed by its one FIFO attempt"));
        }
        c.worker.assert_original_current_image_input(release, input, &c.session.project, c.session.revision, c.session.path.as_deref()).map_err(error)?;
        image.current(c.worker, release, input, flush, c.archive, c.source, &|| c.check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
        c.current()
    }
}
impl Prepared {
    pub(in super::super) fn actual_current_image_files(&self) -> Result<Value> {
        self.completion
            .as_ref()
            .and_then(|c| c.image.as_ref())
            .ok_or_else(|| error("the SAME actual completed original image is required"))?
            .file_observations()
            .map_err(error)
    }
    fn completion_context<'a>(
        &'a self,
        s: &'a Session,
        source: &'a dyn CheckpointSource,
        live: &'a dyn Fn() -> Result<()>,
        check: &'a media_guard::Check,
    ) -> Result<Context<'a>> {
        if !self.current_release_attempted || !self.current_release_ready {
            return Err(error("the actual original acknowledged current-image preparation is required"));
        }
        let archive = self.archive.as_ref().ok_or_else(|| error("the actual acknowledged original archive is missing"))?.release_preparation()?;
        Ok(Context {
            session: s,
            guard: &self.guard,
            graph: &self.graph,
            description: &self.description,
            worker: s.persistence.as_ref().ok_or_else(|| error("the SAME original persistence worker disappeared"))?,
            archive,
            source,
            live,
            check,
        })
    }

    /// Borrow this SAME pre-release constructor frame. This negative reference
    /// cannot mint a release ACK or authorize startup, and never re-captures it.
    pub(in super::super) fn retained_release_frame(
        &self,
        s: &Session,
        check: &media_guard::Check,
    ) -> Result<Arc<crate::autosave::current_startup::release::Frame>> {
        check.check()?;
        self.guard.current(s)?;
        if !self.current_release_attempted
            || !self.current_release_ready
            || !Arc::ptr_eq(&self.graph, &s.project)
            || self.description["nativeSession"] != json!(crate::agent_bridge::retained_native_epoch(s))
        {
            return Err(error("the actual original current-release preparation and unchanged Session are required"));
        }
        let frame =
            self.archive.as_ref().ok_or_else(|| error("the actual original archive is missing"))?.release_preparation()?.original_journal_frame().clone();
        frame.assert_worker(s.persistence.as_ref().ok_or_else(|| error("the original worker disappeared"))?).map_err(error)?;
        check.check()?;
        Ok(frame)
    }

    /// Once-only private composition of the ORIGINAL worker operations. Keep
    /// genuine receipts immediately, even if any subsequent source read fails.
    /// Neither a failed return nor absent image permits replay or a new worker.
    pub(in super::super) fn complete_current_release(
        &mut self,
        s: &Session,
        source: &dyn CheckpointSource,
        live: &impl Fn() -> Result<()>,
        check: &media_guard::Check,
    ) -> Result<()> {
        check.check()?;
        if self.completion.is_some() {
            return Err(error("the original current-image worker continuation was already attempted; no replay"));
        }
        self.current(s)?;
        let c = self.completion_context(s, source, live, check)?;
        c.current()?;
        self.current_native.as_ref().ok_or_else(|| error("the retained complete original current COW input is missing"))?.assert_current_checkpoint_input(
            &s.project,
            s.revision,
            s.path.as_deref(),
            &c.worker.data_dir,
            &c.worker.session_dir,
        )?;
        check.check()?;
        // Retain the attempted composition before its first original effect.
        self.completion = Some(Completion::default());
        // Borrow only unchanged original custody; the native COW and receipt
        // slots remain distinct so each genuine result can be retained at once.
        let archive = self.archive.as_ref().ok_or_else(|| error("the actual original archive disappeared"))?.release_preparation()?;
        let worker = s.persistence.as_ref().ok_or_else(|| error("the SAME original persistence worker disappeared"))?;
        let c = Context { session: s, guard: &self.guard, graph: &self.graph, description: &self.description, worker, archive, source, live, check };
        let state = self.completion.as_mut().ok_or_else(|| error("the attempted original continuation disappeared"))?;
        let release = worker.release_current_image(|_| c.current().map_err(|e| e.to_string()), || c.current().map_err(|e| e.to_string())).map_err(error)?;
        state.release = Some(release);
        c.current()?;
        let release = state.release.as_ref().ok_or_else(|| error("the actual original worker release receipt disappeared"))?;
        let native = self.current_native.take().ok_or_else(|| error("the complete original COW was already consumed; no new capture"))?;
        state.input_attempted = true;
        let input = worker
            .enqueue_current_image_input(release, self.graph.clone(), s.revision, s.path.clone(), native, || c.current().map_err(|e| e.to_string()))
            .map_err(error)?;
        state.input = Some(input);
        c.current()?;
        let flush = worker.flush_current_image_pending_journal(release, || c.current().map_err(|e| e.to_string())).map_err(error)?;
        state.flush = Some(flush);
        c.current()?;
        let image = worker
            .read_current_image(
                release,
                state.input.as_ref().ok_or_else(|| error("the actual original FIFO input receipt disappeared"))?,
                state.flush.as_ref().ok_or_else(|| error("the actual original FIFO flush receipt disappeared"))?,
                archive,
                source,
                &|| check.check().map_err(|e| io::Error::other(e.to_string())),
            )
            .map_err(error)?;
        state.image = Some(image);
        state.current(&c)
    }

    /// Distinct acknowledged-phase reader. The original held current() stays
    /// strict, including after Prepare but before Resume has acknowledged.
    pub(in super::super) fn current_completed_release(
        &self,
        s: &Session,
        source: &dyn CheckpointSource,
        live: &impl Fn() -> Result<()>,
        check: &media_guard::Check,
    ) -> Result<()> {
        let c = self.completion_context(s, source, live, check)?;
        self.completion.as_ref().ok_or_else(|| error("the actual original continuation was not attempted"))?.current(&c)
    }
}

#[cfg(test)]
mod tests;
