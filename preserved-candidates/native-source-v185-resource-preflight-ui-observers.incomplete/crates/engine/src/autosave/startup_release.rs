//! Receipt of the ORIGINAL persistence worker's completed startup exchange.
//!
//! This is process-local custody, never deserialized or manufactured from a
//! running PID, a pending bit or JSON. Complete application/source/file guards
//! remain the caller's responsibility. This receipt alone admits no editor.
use super::{Arc, Persistence, StartupControl};
use std::{
    sync::{
        atomic::Ordering,
        mpsc::{RecvTimeoutError, channel},
    },
    time::Duration,
};

#[derive(Debug)]
pub(crate) struct CurrentImageRelease {
    owner: Arc<()>,
    pub(super) input_attempted: std::sync::atomic::AtomicBool,
}

fn running_original_startup(worker: &Persistence) -> Result<(), String> {
    if worker.startup_gate.is_none()
        || !worker.startup_attempted.load(Ordering::Acquire)
        || worker.startup_pending()
        || worker.lock.is_none()
        || worker.thread.as_ref().is_none_or(|thread| thread.is_finished())
    {
        return Err("the original acknowledged startup worker is not still running".into());
    }
    Ok(())
}

impl CurrentImageRelease {
    // Called only by the ORIGINAL release_current_image after both original
    // acknowledgements and its final after-Resume callback have succeeded.
    fn confirmed(worker: &Persistence) -> Result<Self, String> {
        running_original_startup(worker)?;
        Ok(Self { owner: worker.startup_identity.clone(), input_attempted: std::sync::atomic::AtomicBool::new(false) })
    }
}

impl Persistence {
    /// Original once-only Prepare/Resume exchange, retaining all original
    /// before/after callbacks and creating a receipt only after both ACKs.
    pub(crate) fn release_current_image(
        &self,
        mut before: impl FnMut(bool) -> Result<(), String>,
        mut after: impl FnMut() -> Result<(), String>,
    ) -> Result<CurrentImageRelease, String> {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        self.require_checkpoint_preparation_resolved()?;
        before(false)?;
        let gate = self.startup_gate.as_ref().ok_or_else(|| "actual original startup gate required".to_owned())?;
        self.startup_attempted
            .compare_exchange(false, true, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire)
            .map_err(|_| "original worker release already attempted; no replay".to_owned())?;
        let (tx, rx) = channel();
        gate.send(StartupControl::Prepare(tx)).map_err(|_| "original startup worker ended before preparation".to_owned())?;
        loop {
            before(false)?;
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(()) => {
                    before(true)?;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("original worker preparation acknowledgement lost".into()),
            }
        }
        // Every authored file, canonical file, namespace and exact original
        // gated journal was fenced while the worker was still waiting here.
        let (tx, rx) = channel();
        gate.send(StartupControl::Resume(tx)).map_err(|_| "original startup worker ended before release".to_owned())?;
        loop {
            after()?;
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(()) => {
                    after()?;
                    return CurrentImageRelease::confirmed(self);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("original worker release acknowledgement lost".into()),
            }
        }
    }

    /// Same-worker acknowledgement/liveness only. Keep independent actual
    /// source, canonical, authored-state and publication fences at the call
    /// site. Do not call the old gated-only journal current() after Resume:
    /// that worker may now legitimately consume its original FIFO and write.
    pub(crate) fn assert_current_image_release(&self, receipt: &CurrentImageRelease) -> Result<(), String> {
        if !Arc::ptr_eq(&receipt.owner, &self.startup_identity) {
            return Err("the startup release receipt belongs to another actual persistence worker".into());
        }
        running_original_startup(self)
    }
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod tests {
    use super::*;
    use crate::autosave::{AutoSavePrefs, AutosaveConfig, session_reservation::ColdJournalReservation};
    use std::{cell::Cell, fs, path::PathBuf, sync::atomic::AtomicU64};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = crate::temp_dir().join(format!("poema-startup-release-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn config(&self) -> AutosaveConfig {
            AutosaveConfig::new(&self.0)
        }
        fn gated(&self) -> Persistence {
            let cfg = self.config();
            let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
            Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn acknowledged_release_receipt_keeps_same_worker_without_reusing_its_gated_journal_guard() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated();
        let journal = worker.current_image_journal(&|| Ok(())).unwrap();
        let before = Cell::new(0);
        let after = Cell::new(0);
        assert!(CurrentImageRelease::confirmed(&worker).is_err());
        let receipt = worker
            .release_current_image(
                |_| {
                    before.set(before.get() + 1);
                    assert!(worker.startup_pending());
                    journal.current(&|| Ok(())).map_err(|error| error.to_string())
                },
                || {
                    after.set(after.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
        assert!(before.get() >= 3 && after.get() >= 2);
        assert!(!worker.startup_pending());
        assert!(journal.current(&|| Ok(())).is_err());
        assert!(worker.current_image_journal(&|| Ok(())).is_err());
        worker.assert_current_image_release(&receipt).unwrap();
        worker.stop_worker();
    }

    #[test]
    fn release_receipt_cannot_cross_a_second_actual_worker_or_an_ordinary_owner() {
        let first = Scratch::new();
        let mut worker = first.gated();
        let receipt = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        let second = Scratch::new();
        let mut other = second.gated();
        assert!(other.assert_current_image_release(&receipt).is_err());
        assert!(other.startup_pending());
        let third = Scratch::new();
        let mut ordinary = Persistence::start(&third.config(), AutoSavePrefs::default()).unwrap();
        assert!(ordinary.assert_current_image_release(&receipt).is_err());
        assert!(CurrentImageRelease::confirmed(&ordinary).is_err());
        ordinary.stop_worker();
        other.stop_worker();
        worker.stop_worker();
    }

    #[test]
    fn final_before_guard_refusal_mints_no_receipt_and_keeps_same_worker_gated_without_release_replay() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated();
        let result = worker.release_current_image(|prepared| if prepared { Err("controlled final source refusal".into()) } else { Ok(()) }, || Ok(()));
        assert!(result.is_err());
        assert!(worker.startup_pending());
        assert!(worker.startup_attempted.load(Ordering::Acquire));
        assert!(CurrentImageRelease::confirmed(&worker).is_err());
        assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
        worker.stop_worker();
    }

    #[test]
    fn after_guard_refusal_retains_unknown_release_without_a_receipt_or_another_prepare_resume() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated();
        let result = worker.release_current_image(|_| Ok(()), || Err("controlled after-Resume refusal".into()));
        assert!(result.is_err());
        assert!(worker.startup_attempted.load(Ordering::Acquire));
        assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
        // Resume may already have happened. Join this actual worker; never
        // infer an acknowledgement from its pending bit or construct a receipt.
        worker.stop_worker();
        assert!(worker.thread.is_none());
    }

    #[test]
    fn stopped_worker_invalidates_its_release_receipt_without_authorizing_a_replacement() {
        let first = Scratch::new();
        let mut worker = first.gated();
        let receipt = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.assert_current_image_release(&receipt).unwrap();
        worker.stop_worker();
        assert!(worker.assert_current_image_release(&receipt).is_err());
        let second = Scratch::new();
        let mut replacement = second.gated();
        assert!(replacement.assert_current_image_release(&receipt).is_err());
        replacement.stop_worker();
    }

    #[test]
    fn initial_source_refusal_sends_no_control_and_preserves_the_original_unattempted_release() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated();
        assert!(worker.release_current_image(|_| Err("controlled initial source refusal".into()), || Ok(())).is_err());
        assert!(!worker.startup_attempted.load(Ordering::Acquire));
        assert!(worker.startup_pending());
        assert!(CurrentImageRelease::confirmed(&worker).is_err());
        let receipt = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.assert_current_image_release(&receipt).unwrap();
        assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
        worker.stop_worker();
    }
}
