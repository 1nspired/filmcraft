//! Original constructor/lock custody during the startup release exchange.
//!
//! This is a read-only frame, captured before the original release attempt.
//! It validates the original data, recovery members, session directory and
//! lock while the SAME worker may legitimately write its new session. It does
//! not certify that new session's contents: complete acknowledged FIFO/image
//! readback remains mandatory. Held Journal.current stays strictly held-only.
use super::*;
use crate::autosave::CurrentImageRelease;
use std::thread::ThreadId;

pub(crate) struct Frame {
    journal: Arc<Journal>,
    owner: Arc<()>,
    thread: ThreadId,
}

impl Frame {
    pub(in crate::autosave) fn capture_held(worker: &Persistence, original: &Arc<Journal>, check: &impl Fn() -> io::Result<()>) -> io::Result<Arc<Self>> {
        check()?;
        if worker.startup_attempted.load(Ordering::Acquire) || worker.last_sent.is_some() || !worker.startup_pending() {
            return Err(io::Error::other("retain the actual journal release frame before its original release attempt"));
        }
        let journal = worker.current_image_journal(check)?;
        if !Arc::ptr_eq(&journal, original) {
            return Err(io::Error::other("journal release frame crossed its actual original constructor"));
        }
        let thread = worker.thread.as_ref().ok_or_else(|| io::Error::other("actual original journal worker thread required"))?.thread().id();
        let frame = Arc::new(Self { journal, owner: worker.startup_identity.clone(), thread });
        frame.assert_worker(worker)?;
        frame.inspect_source(check)?;
        Ok(frame)
    }

    pub(crate) fn same_original_journal(&self, journal: &Arc<Journal>) -> bool {
        Arc::ptr_eq(&self.journal, journal)
    }

    pub(crate) fn assert_worker(&self, worker: &Persistence) -> io::Result<()> {
        if !Arc::ptr_eq(&self.owner, &worker.startup_identity)
            || worker.current_journal.as_ref().is_none_or(|journal| !Arc::ptr_eq(journal, &self.journal))
            || worker.thread.as_ref().is_none_or(|thread| thread.is_finished() || thread.thread().id() != self.thread)
            || worker.startup_gate.is_none()
            || worker.lock.is_none()
            || worker.data_dir != self.journal.data.record().path
            || worker.session_dir != self.journal.session.pin().path
            || !self.journal.live.load(Ordering::Acquire)
        {
            return Err(io::Error::other("the release frame belongs to another or ended original journal worker"));
        }
        Ok(())
    }

    /// Original constructor and prior recovery namespace only. New current
    /// session image files are checked separately after the actual FIFO ACK.
    /// The private actual Journal Arc and its live flag retain worker custody;
    /// the controller separately asserts its same original Session/worker.
    pub(crate) fn inspect_source(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let journal = &self.journal;
        let preference = journal.canonical_preference.lock().unwrap_or_else(|error| error.into_inner()).clone();
        check()?;
        if !journal.live.load(Ordering::Acquire) {
            return Err(io::Error::other("the original journal worker ended during release"));
        }
        journal.data.current(check)?;
        journal.root.current(preference.as_deref(), check)?;
        journal.session.current(check)?;
        journal.lock.current(check)?;
        journal.lock.fence(check)?;
        let session_name =
            journal.session.pin().path.file_name().and_then(|name| name.to_str()).ok_or_else(|| io::Error::other("literal original session required"))?;
        journal.root.namespace(session_name, preference.as_deref(), check)?;
        journal.data.current(check)?;
        journal.root.current(preference.as_deref(), check)?;
        journal.session.current(check)?;
        journal.lock.fence(check)?;
        if !journal.live.load(Ordering::Acquire) {
            return Err(io::Error::other("the original journal worker ended during release observation"));
        }
        check()
    }

    pub(crate) fn inspect_acknowledged(&self, worker: &Persistence, release: &CurrentImageRelease, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        worker.assert_current_image_release(release).map_err(io::Error::other)?;
        self.assert_worker(worker)?;
        self.inspect_source(check)?;
        self.assert_worker(worker)?;
        worker.assert_current_image_release(release).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests;
