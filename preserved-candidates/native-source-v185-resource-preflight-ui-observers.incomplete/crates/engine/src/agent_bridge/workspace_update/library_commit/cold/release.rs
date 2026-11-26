//! Read-only original canonical receipts across the actual worker exchange.
//! The original held methods remain unchanged. These distinct inspections
//! require the SAME constructor frame retained before release, preserve all
//! canonical file/preference/memory checks and grant no writer or admission.
use super::*;
use crate::autosave::current_startup::release::Frame;

impl ColdCommit {
    fn same_release_frame(&self, frame: &Frame) -> Result<()> {
        if !frame.same_original_journal(&self.journal)
            || !self.attempted
            || self.observation.as_ref().is_none_or(|value| value["published"] != true)
            || self.service_binding_description().is_none_or(|value| value["bound"] != true)
        {
            return Err(fail("the actual completed canonical owner and its same original release frame are required"));
        }
        Ok(())
    }

    pub(in crate::agent_bridge) fn inspect_release_source(&self, frame: &Frame, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_release_source(frame, check, true)
    }

    /// Final original generations after the same complete release-source read.
    /// Every write still requires a new complete source query; no phase fallback.
    pub(in crate::agent_bridge) fn fence_release_source_after_read(&self, frame: &Frame, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_release_source(frame, check, false)
    }

    fn read_release_source(&self, frame: &Frame, check: &impl Fn() -> io::Result<()>, read_bytes: bool) -> Result<()> {
        self.same_release_frame(frame)?;
        frame.inspect_source(check).map_err(fail)?;
        // The SAME original file and preference observations remain retained.
        // Only the actual constructor frame changes the worker-phase check.
        if read_bytes {
            self.original.verify_published_files(check)?;
        } else {
            self.original.fence_published_files_after_read(check)?;
        }
        let preference = if read_bytes {
            self.preference.current_node(&self.preference.before, check)?
        } else {
            self.preference.fence_node_after_read(&self.preference.before, check)?
        };
        if !preference {
            return Err(fail("original canonical preference continuation missing during release"));
        }
        frame.inspect_source(check).map_err(fail)
    }

    pub(in crate::agent_bridge) fn inspect_release_memory(
        &self,
        session: &Session,
        frame: &Frame,
        source: &impl Fn() -> io::Result<()>,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<()> {
        self.same_release_frame(frame)?;
        inspect_read(source, check, || {
            let worker = session.persistence.as_ref().ok_or_else(|| fail("the same original release worker disappeared"))?;
            frame.assert_worker(worker).map_err(fail)?;
            frame.inspect_source(check).map_err(fail)?;
            self.original.verify_published(session, check)?;
            if !self.preference.current_node(&self.preference.before, check)? {
                return Err(fail("original canonical preference continuation missing during release"));
            }
            frame.assert_worker(worker).map_err(fail)?;
            frame.inspect_source(check).map_err(fail)
        })
    }
}

#[cfg(test)]
mod tests;
