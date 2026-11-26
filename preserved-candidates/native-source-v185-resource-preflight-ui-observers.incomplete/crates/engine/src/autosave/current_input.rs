//! Checked complete current-image input to the ORIGINAL ordinary Dirty FIFO.
//!
//! Actual Prepare/Resume ACK custody is required before this once-only send.
//! An enqueue is not a worker write ACK, file readback or editor admission.
//! Keep original source/Root/canonical/records/COW guards at the caller, and
//! retain unknown outcomes after a late refusal without replaying the input.
use super::{Arc, CurrentImageRelease, Msg, Persistence, Project, Snapshot};
use std::sync::atomic::Ordering;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) mod readback;

#[derive(Debug)]
pub(crate) struct OriginalCurrentImageInput {
    owner: Arc<()>,
    project: Arc<Project>,
    revision: u64,
    path: Option<String>,
}

impl Persistence {
    pub(crate) fn enqueue_current_image_input(
        &self,
        release: &CurrentImageRelease,
        project: Arc<Project>,
        revision: u64,
        path: Option<String>,
        native: crate::agent_bridge::NativeJournalPending,
        mut current: impl FnMut() -> Result<(), String>,
    ) -> Result<OriginalCurrentImageInput, String> {
        self.assert_current_image_release(release)?;
        current()?;
        if self.last_sent.is_some() || self.startup_flush_attempted.load(Ordering::Acquire) {
            return Err("the original current-image input must precede ordinary synchronization and the checked startup flush".into());
        }
        native.assert_current_checkpoint_input(&project, revision, path.as_deref(), &self.data_dir, &self.session_dir).map_err(|error| error.to_string())?;
        self.assert_current_image_release(release)?;
        current()?;
        release
            .input_attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "original current-image enqueue already attempted; no replay".to_owned())?;
        let input = OriginalCurrentImageInput { owner: self.startup_identity.clone(), project: project.clone(), revision, path: path.clone() };
        // Exactly the ordinary worker's original message and serializers.
        // Do not convert a failed send into a successful descriptive receipt.
        self.tx
            .send(Msg::Dirty(Snapshot { project, revision, path, native: Some(Ok(Box::new(native))) }))
            .map_err(|_| "original worker ended before current-image input enqueue; no replay".to_owned())?;
        // The original FIFO may already have consumed this input. Refusal here
        // must retain the consumed attempt and cannot certify a returned receipt.
        current()?;
        self.assert_original_current_image_input(release, &input, &input.project, revision, input.path.as_deref())?;
        Ok(input)
    }

    /// Same acknowledged worker and original input only. Keep the full actual
    /// Session guard and independent completed native-file readback at the call
    /// site; neither this receipt nor a matching revision/PID replaces them.
    pub(crate) fn assert_original_current_image_input(
        &self,
        release: &CurrentImageRelease,
        input: &OriginalCurrentImageInput,
        project: &Arc<Project>,
        revision: u64,
        path: Option<&str>,
    ) -> Result<(), String> {
        self.assert_current_image_release(release)?;
        if !release.input_attempted.load(Ordering::Acquire)
            || !Arc::ptr_eq(&input.owner, &self.startup_identity)
            || !Arc::ptr_eq(&input.project, project)
            || input.revision != revision
            || input.path.as_deref() != path
        {
            return Err("the original current-image FIFO input belongs to another worker or COW image".into());
        }
        Ok(())
    }
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod tests;
