//! Checked result of the ORIGINAL worker's existing pending-journal writer.
//!
//! This once-only startup exchange retains the original FIFO, serializers,
//! native companion writer, events and retry policy. Its private receipt records
//! an acknowledged FIFO outcome, including whether a write occurred. Callers must verify the expected
//! complete current image/history, physical files and application closure.
//! Neither legacy `flush()` nor a revision/PID/JSON supplies this receipt.
use super::{Arc, CurrentImageRelease, Msg, Persistence};
use std::{
    sync::{
        atomic::Ordering,
        mpsc::{RecvTimeoutError, channel},
    },
    time::Duration,
};

#[derive(Debug)]
pub(crate) struct OriginalJournalFlush {
    owner: Arc<()>,
    written_revision: Option<u64>,
    readback_attempted: std::sync::atomic::AtomicBool,
}

impl OriginalJournalFlush {
    pub(crate) fn written_revision(&self) -> Option<u64> {
        self.written_revision
    }
    pub(crate) fn begin_current_readback(&self) -> Result<(), String> {
        self.readback_attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "original current-image readback already attempted; no replacement baseline".to_owned())
    }
    pub(crate) fn current_readback_started(&self) -> bool {
        self.readback_attempted.load(Ordering::Acquire)
    }
}

impl Persistence {
    /// Once-only checked flush after this SAME original worker's startup ACKs.
    /// Source/deadline refusal after sending leaves an unknown write outcome;
    /// retain recovery evidence and stop this worker on abandonment. Do not
    /// replay this exchange, infer a receipt, or clear pending Save evidence.
    /// No pending/enabled journal returns written_revision=None. This can mean
    /// an earlier automatic write already finished, or no write was possible;
    /// it is never proof of the expected complete current image.
    pub(crate) fn flush_current_image_pending_journal(
        &self,
        release: &CurrentImageRelease,
        mut current: impl FnMut() -> Result<(), String>,
    ) -> Result<OriginalJournalFlush, String> {
        self.assert_current_image_release(release)?;
        current()?;
        self.startup_flush_attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "original startup journal flush already attempted; no replay".to_owned())?;
        let (tx, rx) = channel();
        self.tx.send(Msg::CheckedPendingFlush(tx)).map_err(|_| "original worker ended before checked journal flush".to_owned())?;
        loop {
            self.assert_current_image_release(release)?;
            current()?;
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => {
                    let written_revision = result?;
                    current()?;
                    self.assert_current_image_release(release)?;
                    return Ok(OriginalJournalFlush {
                        owner: self.startup_identity.clone(),
                        written_revision,
                        readback_attempted: std::sync::atomic::AtomicBool::new(false),
                    });
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("original checked journal flush acknowledgement lost; no replay".into()),
            }
        }
    }

    /// Process-local acknowledgement/liveness only. Full native checkpoint and
    /// file/control validation must accompany this at the original call site.
    pub(crate) fn assert_original_journal_flush(&self, release: &CurrentImageRelease, flush: &OriginalJournalFlush) -> Result<(), String> {
        self.assert_current_image_release(release)?;
        if !self.startup_flush_attempted.load(Ordering::Acquire) || !Arc::ptr_eq(&flush.owner, &self.startup_identity) {
            return Err("checked journal receipt belongs to another actual original worker".into());
        }
        Ok(())
    }
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod tests {
    use super::*;
    use crate::autosave::{AutoSavePrefs, AutosaveConfig, META, SNAPSHOT, session_reservation::ColdJournalReservation};
    use filmcraft_project::Project;
    use std::{
        cell::Cell,
        fs,
        path::{Path, PathBuf},
        sync::atomic::AtomicU64,
    };

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = crate::temp_dir().join(format!("poema-checked-original-flush-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn gated(&self, prefs: AutoSavePrefs) -> Persistence {
            let cfg = AutosaveConfig::new(&self.0);
            let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
            Persistence::start_cold_reserved(&cfg, prefs, reservation, &|| Ok(())).unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checked_flush_keeps_original_fifo_writer_and_reports_the_actual_written_revision() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        let project = Arc::new(Project::default());
        worker.send_dirty_native(project.clone(), 41, None, Ok(None));
        let receipt = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
        assert_eq!(receipt.written_revision(), Some(41));
        worker.assert_original_journal_flush(&release, &receipt).unwrap();
        assert_eq!(fs::read(worker.session_dir.join(SNAPSHOT)).unwrap(), filmcraft_format::encode(&project, false));
        let meta: serde_json::Value = serde_json::from_slice(&fs::read(worker.session_dir.join(META)).unwrap()).unwrap();
        assert_eq!(meta["revision"], 41);
        assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).is_err());
        worker.flush();
        worker.stop_worker();
        assert!(worker.assert_original_journal_flush(&release, &receipt).is_err());
    }

    #[test]
    fn checked_flush_propagates_actual_snapshot_write_failure_without_minting_or_replaying_a_receipt() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        fs::create_dir(worker.session_dir.join(SNAPSHOT)).unwrap();
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.send_dirty_native(Arc::new(Project::default()), 42, None, Ok(None));
        let error = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap_err();
        assert!(error.contains("recovery journal"));
        assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap_err().contains("no replay"));
        assert!(worker.session_dir.join(SNAPSHOT).is_dir());
        worker.stop_worker();
    }

    #[test]
    fn checked_flush_reports_native_companion_failure_even_after_project_and_metadata_were_written() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.send_dirty_native(Arc::new(Project::default()), 43, None, Err("controlled complete-native capture failure".into()));
        let error = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap_err();
        assert!(error.contains("controlled complete-native capture failure"));
        assert!(worker.session_dir.join(SNAPSHOT).is_file());
        assert!(worker.session_dir.join(META).is_file());
        assert!(!worker.session_dir.join("native-state.json").exists());
        assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).is_err());
        worker.stop_worker();
    }

    #[test]
    fn checked_flush_refuses_crossed_release_and_flush_receipts_and_stopped_workers() {
        let first = Scratch::new();
        let mut worker = first.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.send_dirty_native(Arc::new(Project::default()), 44, None, Ok(None));
        let flush = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
        let second = Scratch::new();
        let mut other = second.gated(AutoSavePrefs::default());
        let other_release = other.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        assert!(other.flush_current_image_pending_journal(&release, || Ok(())).is_err());
        assert!(!other.startup_flush_attempted.load(Ordering::Acquire));
        assert!(other.assert_original_journal_flush(&other_release, &flush).is_err());
        other.stop_worker();
        worker.stop_worker();
        assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).is_err());
    }

    #[test]
    fn checked_flush_cannot_treat_an_empty_or_disabled_journal_as_a_written_current_image() {
        for disabled in [false, true] {
            let scratch = Scratch::new();
            let mut prefs = AutoSavePrefs::default();
            prefs.recovery_journal = !disabled;
            let mut worker = scratch.gated(prefs);
            let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
            if disabled {
                worker.send_dirty_native(Arc::new(Project::default()), 45, None, Ok(None));
            }
            let flush = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
            assert_eq!(flush.written_revision(), None);
            worker.assert_original_journal_flush(&release, &flush).unwrap();
            assert!(!worker.session_dir.join(SNAPSHOT).exists());
            assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).is_err());
            worker.stop_worker();
        }
    }

    #[test]
    fn checked_flush_distinguishes_an_already_finished_original_write_without_rewriting_its_files() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.send_dirty_native(Arc::new(Project::default()), 48, None, Ok(None));
        worker.flush();
        use std::os::unix::fs::MetadataExt;
        let generation = |path: &Path| {
            let m = fs::metadata(path).unwrap();
            (m.dev(), m.ino(), m.len(), m.mode(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec())
        };
        let snapshot_generation = generation(&worker.session_dir.join(SNAPSHOT));
        let metadata_generation = generation(&worker.session_dir.join(META));
        let snapshot = fs::read(worker.session_dir.join(SNAPSHOT)).unwrap();
        let metadata = fs::read(worker.session_dir.join(META)).unwrap();
        let flush = worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
        assert_eq!(flush.written_revision(), None);
        worker.assert_original_journal_flush(&release, &flush).unwrap();
        assert_eq!(fs::read(worker.session_dir.join(SNAPSHOT)).unwrap(), snapshot);
        assert_eq!(fs::read(worker.session_dir.join(META)).unwrap(), metadata);
        assert_eq!(generation(&worker.session_dir.join(SNAPSHOT)), snapshot_generation);
        assert_eq!(generation(&worker.session_dir.join(META)), metadata_generation);
        worker.stop_worker();
    }

    #[test]
    fn checked_flush_initial_source_refusal_sends_no_request_and_preserves_the_unused_exchange() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        assert!(worker.flush_current_image_pending_journal(&release, || Err("controlled initial source refusal".into())).is_err());
        assert!(!worker.startup_flush_attempted.load(Ordering::Acquire));
        worker.send_dirty_native(Arc::new(Project::default()), 46, None, Ok(None));
        worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
        worker.stop_worker();
    }

    #[test]
    fn checked_flush_late_source_refusal_retains_unknown_write_without_another_checked_flush() {
        let scratch = Scratch::new();
        let mut worker = scratch.gated(AutoSavePrefs::default());
        let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        worker.send_dirty_native(Arc::new(Project::default()), 47, None, Ok(None));
        let checks = Cell::new(0);
        let result = worker.flush_current_image_pending_journal(&release, || {
            checks.set(checks.get() + 1);
            if checks.get() == 1 { Ok(()) } else { Err("controlled source loss after checked request".into()) }
        });
        assert!(result.is_err());
        assert!(worker.startup_flush_attempted.load(Ordering::Acquire));
        assert!(worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap_err().contains("no replay"));
        // Settle this actual already-enqueued request through the unchanged
        // legacy FIFO barrier, solely to inspect the generated test effects.
        worker.flush();
        assert!(worker.session_dir.join(SNAPSHOT).is_file());
        worker.stop_worker();
    }
}
