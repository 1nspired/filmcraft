//! Same actual Cold source Controls across the original worker exchange.
//! This borrows the existing producer, its actual constructor frame and the
//! caller's existing deadline/Stop guard. It creates no owner, writer, durable
//! receipt, successor, replacement baseline or positive application admission.
use super::*;
use crate::autosave::{current_startup::release::Frame, held_checkpoint::publication::CheckpointSource};

pub(crate) struct ReleaseRead<'a> {
    original: &'a SourceWitness,
    frame: &'a Frame,
    check: &'a media_guard::Check,
}
impl SourceWitness {
    pub(crate) fn release_read<'a>(&'a self, frame: &'a Frame, check: &'a media_guard::Check) -> ReleaseRead<'a> {
        ReleaseRead { original: self, frame, check }
    }
}
impl CheckpointSource for ReleaseRead<'_> {
    fn current(&self) -> io::Result<()> {
        let check = self.check;
        check.check().map_err(|e| io::Error::other(e.to_string()))?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        with_original_successor(&self.original.successor, |successor| {
            self.original.read_current_at_with_canonical(successor, check, &|| self.original.canonical.inspect_release_source(self.frame, &io))
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
        io()
    }
    fn fence_after_read(&self) -> io::Result<()> {
        let check = self.check;
        check.check().map_err(|e| io::Error::other(e.to_string()))?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        with_original_successor(&self.original.successor, |successor| {
            self.original.fence_at_with_canonical(successor, check, &|| self.original.canonical.fence_release_source_after_read(self.frame, &io))
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
        io()
    }
}
