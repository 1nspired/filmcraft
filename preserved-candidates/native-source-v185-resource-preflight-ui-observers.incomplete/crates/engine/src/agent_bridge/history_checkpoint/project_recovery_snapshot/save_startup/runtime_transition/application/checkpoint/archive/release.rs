//! Consume the actual archive publisher before original startup_attempted.
//! Failed attempts retain their unknown preparation and can never be repeated.
use super::*;
use crate::autosave::{Persistence, held_checkpoint::publication::release::ReleasePreparation};
impl Archive {
    pub(in super::super) fn require_current_release(&self, worker: &Persistence) -> Result<()> {
        self.current(worker)?;
        self.intent_resolved()?;
        if !self.intent_confirmed || !self.observation_confirmed || self.release_attempted || self.release.is_some() {
            return Err(error("one actual fully acknowledged archive and unused current-release preparation are required"));
        }
        Ok(())
    }
    pub(in super::super) fn prepare_current_release(&mut self, worker: &Persistence, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        check().map_err(error)?;
        self.require_current_release(worker)?;
        self.release_attempted = true;
        let published = self.published.take().ok_or_else(|| error("the SAME original archive publisher is required; no reconstruction"))?;
        let prepared = published.prepare_release(worker, check).map_err(error)?;
        self.release = Some(prepared);
        // Retain the actual owner before any final source observation can fail.
        let prepared = self.release.as_ref().ok_or_else(|| error("actual original release preparation disappeared"))?;
        prepared.held_current(worker, check).map_err(error)?;
        let description = prepared.held_description(worker).map_err(error)?;
        check().map_err(error)?;
        self.release_ready = true;
        Ok(description)
    }
    pub(in super::super) fn release_preparation(&self) -> Result<&ReleasePreparation> {
        self.publication_resolved()?;
        if !self.release_attempted || !self.release_ready || !self.observation_confirmed {
            return Err(error("the same acknowledged actual release preparation is required"));
        }
        self.release.as_ref().ok_or_else(|| error("the actual original release preparation is missing"))
    }
}

#[cfg(test)]
mod tests;
