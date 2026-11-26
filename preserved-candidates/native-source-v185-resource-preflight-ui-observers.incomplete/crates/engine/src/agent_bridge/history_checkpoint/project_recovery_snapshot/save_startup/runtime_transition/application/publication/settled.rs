//! Completed-phase source reads borrow the SAME actual checkpoint, constructor
//! frame and canonical owner. Only actual typed journal/metadata successors are
//! supplied here. The old held readers remain unchanged and intentionally fail
//! after Resume or a metadata publication; no new source baseline is captured.
use super::*;
use crate::autosave::{current_startup::release::Frame, held_checkpoint::publication::CheckpointSource};

struct SettledRead<'a> {
    original: &'a SourceWitness,
    previous: &'a JournalContinuation,
    candidate: &'a JournalContinuation,
    catalog: &'a Control,
    activity: &'a Control,
    frame: &'a Frame,
    check: &'a media_guard::Check,
}
impl SettledRead<'_> {
    fn read(&self, complete: bool) -> Result<()> {
        let w = self.original;
        let check = self.check;
        check.check()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        with_original_successor(&w.successor, |retained| {
            if !retained.same_original(self.previous) {
                return Err(error("the completed metadata read crossed its SAME original retained journal Controls"));
            }
            if complete {
                w.source.current(check)?;
                let observed = w.root.observe_current(check, |manifest| {
                    w.records.checkpoint_ready()?;
                    w.records.read_with_activity(check, true, self.activity)?;
                    let preference = w.canonical.preference();
                    w.original.read_settled_application(check, self.candidate, &preference, manifest, self.catalog, ApplicationRead::Complete)?;
                    w.canonical.inspect_release_source(self.frame, &io)?;
                    w.records.read_with_activity(check, false, self.activity)
                });
                // Keep actual source/PID/Stop checks after failed reads too.
                w.source.fence_after_read(check)?;
                check.check()?;
                observed?;
            } else {
                w.source.fence_after_read(check)?;
                w.records.checkpoint_ready()?;
                w.records.read_with_activity(check, false, self.activity)?;
                let preference = w.canonical.preference();
                let manifest = w.root.acknowledged_manifest_after_read(check)?;
                w.original.read_settled_application(check, self.candidate, &preference, manifest, self.catalog, ApplicationRead::FenceAfterRead)?;
                w.canonical.fence_release_source_after_read(self.frame, &io)?;
                w.records.read_with_activity(check, false, self.activity)?;
                w.root.fence_after_read(check)?;
                w.source.fence_after_read(check)?;
            }
            check.check()
        })
    }
}
impl CheckpointSource for SettledRead<'_> {
    fn current(&self) -> io::Result<()> {
        self.read(true).map_err(|e| io::Error::other(e.to_string()))
    }
    fn fence_after_read(&self) -> io::Result<()> {
        self.read(false).map_err(|e| io::Error::other(e.to_string()))
    }
}
impl Pending {
    pub(in super::super) fn inspect_completed_settlement_source(
        &self,
        s: &Session,
        previous: &JournalContinuation,
        candidate: &JournalContinuation,
        catalog: &Control,
        activity: &Control,
        check: &media_guard::Check,
    ) -> Result<()> {
        check.check()?;
        let witness = self.application.publication_source.as_ref().ok_or_else(|| error("the SAME actual complete publication source is required"))?;
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the SAME actual canonical owner is required"))?.retained_published()?;
        let checkpoint = self.application.checkpoint.as_ref().ok_or_else(|| error("the SAME actual completed checkpoint is required"))?;
        let frame = checkpoint.retained_release_frame(s, check)?;
        let source = SettledRead { original: witness, previous, candidate, catalog, activity, frame: &frame, check };
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let memory = || canonical.inspect_release_memory(s, &frame, &|| CheckpointSource::current(&source), &io);
        checkpoint.current_completed_release(s, &source, &memory, check)?;
        check.check()
    }
    pub(in super::super) fn adopt_completed_settlement_successor(
        &mut self,
        s: &Session,
        next: JournalContinuation,
        catalog: &Control,
        activity: &Control,
        check: &media_guard::Check,
    ) -> Result<()> {
        let previous = self.successor.as_ref().ok_or_else(|| error("the SAME completed original journal predecessor is required"))?;
        self.inspect_completed_settlement_source(s, previous, &next, catalog, activity, check)?;
        check.check()?;
        let witness = self.application.publication_source.as_ref().ok_or_else(|| error("the SAME actual source witness disappeared"))?;
        replace_original_successor(&witness.successor, previous, next.clone())?;
        self.successor = Some(next);
        check.check()
    }
}
