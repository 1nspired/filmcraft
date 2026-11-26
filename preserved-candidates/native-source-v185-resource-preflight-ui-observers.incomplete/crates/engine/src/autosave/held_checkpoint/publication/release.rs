//! Original published archive custody, consumed BEFORE the release attempt.
//!
//! The original held reader rejects startup_attempted=true, including while
//! Prepare still has the worker gated. This distinct preparation retains the
//! actual publisher, original constructor frame, receipts and complete bytes.
//! Its inspections are read-only negative fences. Only the existing original
//! release_current_image can create CurrentImageRelease after both real ACKs.
use super::*;
use crate::autosave::CurrentImageRelease;

pub(crate) struct ReleasePreparation {
    published: Published,
    original_data: Arc<DirectoryFrame>,
    journal_frame: Arc<crate::autosave::current_startup::release::Frame>,
}

impl Published {
    pub(crate) fn prepare_release(self, worker: &Persistence, check: &impl Fn() -> io::Result<()>) -> Result<ReleasePreparation, String> {
        // Consume the actual publisher while its original held-only API and
        // full retained source checks are still valid. Never recreate it from
        // descriptive paths, hashes, copied receipts or an attempted release.
        self.current(worker, check)?;
        let original_data = self.encoded.retained_data_frame(worker, check)?;
        if !Arc::ptr_eq(&original_data, &self.data) {
            return Err("the original release preparation crossed its constructor data frame".into());
        }
        let journal_frame = crate::autosave::current_startup::release::Frame::capture_held(worker, &self.encoded.journal, check).map_err(failure)?;
        check().map_err(failure)?;
        Ok(ReleasePreparation { published: self, original_data, journal_frame })
    }
}

impl ReleasePreparation {
    /// Held-only observation of the retained original publisher. The original
    /// worker gate check remains strict and rejects a started release attempt.
    pub(crate) fn held_current(&self, worker: &Persistence, check: &impl Fn() -> io::Result<()>) -> Result<(), String> {
        self.published.current(worker, check)
    }
    pub(crate) fn held_description(&self, worker: &Persistence) -> Result<serde_json::Value, String> {
        self.published.description(worker)
    }

    fn same_running_worker(&self, worker: &Persistence) -> Result<(), String> {
        self.published.encoded.same_worker_identity(worker)?;
        self.journal_frame.assert_worker(worker).map_err(failure)?;
        if worker.startup_gate.is_none()
            || worker.lock.is_none()
            || worker.thread.as_ref().is_none_or(|h| h.is_finished())
            || worker.last_sent.is_some()
            || !worker.startup_checkpoint_ready.load(Ordering::Acquire)
            || !worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire)
            || !worker.startup_checkpoint_publication_ready.load(Ordering::Acquire)
        {
            return Err("the same original checkpoint worker and completed publication are required during release".into());
        }
        Ok(())
    }

    /// Complete archive read, including during the actual after-Resume callback
    /// before its ACK has returned. The caller supplies its stage-correct full
    /// original source/Root/canonical/records/COW checks and deadline/Stop guard.
    /// Passing this negative inspection creates no release or admission receipt.
    pub(crate) fn inspect_transition(&self, worker: &Persistence, source: &dyn CheckpointSource, check: &impl Fn() -> io::Result<()>) -> Result<(), String> {
        self.same_running_worker(worker)?;
        check().map_err(failure)?;
        let observed = source_read(source, || {
            self.journal_frame.inspect_source(check)?;
            self.published.root.current(check)?;
            self.published.files.current(check)?;
            self.published.writes.current(check)?;
            self.published.physical_current_with_original_data(&self.original_data, check)?;
            self.journal_frame.inspect_source(check)
        });
        check().map_err(failure)?;
        observed.map_err(failure)?;
        self.same_running_worker(worker)
    }

    /// The acknowledged phase additionally requires the SAME actual original
    /// release receipt. Reuse all original archive/payload/native validators;
    /// no held Journal.current() is invoked after Resume.
    pub(crate) fn inspect_acknowledged_release(
        &self,
        worker: &Persistence,
        release: &CurrentImageRelease,
        source: &dyn CheckpointSource,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<(), String> {
        worker.assert_current_image_release(release)?;
        self.inspect_transition(worker, source, check)?;
        worker.assert_current_image_release(release)
    }

    /// Immutable original bytes for the later original-FIFO readback only.
    /// Data cannot recreate this preparation, a writer, a release ACK or an
    /// admitted editor. Keep the SAME actual worker and complete source fences.
    pub(crate) fn expected_files<'a>(&'a self, worker: &Persistence) -> Result<[(&'static str, &'a [u8]); 3], String> {
        self.same_running_worker(worker)?;
        Ok(encoded_slices(&self.published.encoded))
    }

    /// Same actual constructor frame for the original canonical/source reader.
    /// This read-only reference cannot construct a release ACK or an editor.
    pub(crate) fn original_journal_frame(&self) -> &Arc<crate::autosave::current_startup::release::Frame> {
        &self.journal_frame
    }
}

#[cfg(test)]
mod tests;
