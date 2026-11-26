//! One-use, in-memory identity for the original journal constructor.
//! Reserving a name creates no directory, worker or startup authority. It cannot
//! be copied or reconstructed from a saved description.
use super::*;

pub(crate) struct SessionReservation {
    pub(super) data_dir: PathBuf,
    pub(super) started: i64,
    pub(super) pid: u32,
    pub(super) session_name: String,
}

impl SessionReservation {
    /// Descriptive planning data only; the original constructor still consumes
    /// this in-memory reservation before any journal or worker can exist.
    pub(crate) fn description(&self, cfg: &AutosaveConfig) -> io::Result<Value> {
        self.current(cfg)?;
        Ok(json!({"name":self.session_name,"path":recovery_root(&self.data_dir).join(&self.session_name),
            "started":self.started,"pid":self.pid,"dataDirectory":self.data_dir,
            "writeAuthority":false,"startupAuthority":false,"workersMayResume":false}))
    }
    pub(crate) fn reserve(cfg: &AutosaveConfig) -> Self {
        let started = unix_now();
        let pid = std::process::id();
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let session_name = if seq == 0 { format!("{started}-{pid}") } else { format!("{started}-{pid}-{seq}") };
        Self { data_dir: cfg.data_dir.clone(), started, pid, session_name }
    }

    pub(crate) fn current(&self, cfg: &AutosaveConfig) -> io::Result<()> {
        if self.pid != std::process::id() || self.data_dir != cfg.data_dir {
            return Err(io::Error::other("journal reservation belongs to another native process or configured directory"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn absent(tag: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = crate::temp_dir().join(format!("poema-reserved-journal-{tag}-{}-{sequence}", std::process::id()));
        assert!(!path.exists());
        path
    }

    #[test]
    fn reservation_keeps_original_identity_names_without_creating_storage_or_workers() {
        let cfg = AutosaveConfig::new(absent("pure"));
        let first = SessionReservation::reserve(&cfg);
        let second = SessionReservation::reserve(&cfg);
        first.current(&cfg).unwrap();
        second.current(&cfg).unwrap();
        assert_eq!(first.pid, std::process::id());
        assert_eq!(first.data_dir, cfg.data_dir);
        let prefix = format!("{}-{}", first.started, first.pid);
        assert!(first.session_name == prefix || first.session_name.starts_with(&(prefix + "-")));
        assert_ne!(first.session_name, second.session_name);
        assert!(!cfg.data_dir.exists());
    }

    #[test]
    fn constructor_refuses_foreign_directory_or_process_reservations_before_effects() {
        let cfg = AutosaveConfig::new(absent("original"));
        let other = AutosaveConfig::new(absent("foreign"));
        let reservation = SessionReservation::reserve(&cfg);
        assert!(Persistence::start_reserved_inner(&other, AutoSavePrefs::default(), true, reservation, &|| Ok(())).is_err());
        let mut reservation = SessionReservation::reserve(&cfg);
        reservation.pid = reservation.pid.wrapping_add(1);
        assert!(Persistence::start_reserved_inner(&cfg, AutoSavePrefs::default(), true, reservation, &|| Ok(())).is_err());
        assert!(!cfg.data_dir.exists());
        assert!(!other.data_dir.exists());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn original_startup_checks_stop_initial_and_reserved_construction_before_effects() {
        let cfg = AutosaveConfig::new(absent("stop"));
        let stop = || Err(io::Error::other("reserved startup stopped"));
        assert!(Persistence::start_current_image(&cfg, AutoSavePrefs::default(), &stop).is_err());
        let reservation = SessionReservation::reserve(&cfg);
        assert!(Persistence::start_reserved_inner(&cfg, AutoSavePrefs::default(), true, reservation, &stop).is_err());
        assert!(!cfg.data_dir.exists());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn original_gated_worker_consumes_reserved_identity_and_drop_preserves_its_journal() {
        let cfg = AutosaveConfig::new(absent("worker"));
        fs::create_dir(&cfg.data_dir).unwrap();
        let reservation = SessionReservation::reserve(&cfg);
        let name = reservation.session_name.clone();
        let expected = recovery_root(&cfg.data_dir).join(&name);
        let mut worker = Persistence::start_reserved_inner(&cfg, AutoSavePrefs::default(), true, reservation, &|| Ok(())).unwrap();
        assert_eq!(worker.session_dir, expected);
        assert!(worker.startup_pending());
        assert!(!worker.startup_attempted.load(std::sync::atomic::Ordering::Acquire));
        assert!(worker.lock.is_some());
        assert!(!worker.thread.as_ref().unwrap().is_finished());
        let journal = worker.current_image_journal(&|| Ok(())).unwrap();
        journal.current(&|| Ok(())).unwrap();
        assert_eq!(fs::read_dir(recovery_root(&cfg.data_dir)).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&expected).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), [std::ffi::OsString::from(LOCK)]);
        worker.stop_worker();
        assert!(worker.thread.is_none());
        assert!(worker.startup_pending());
        assert!(journal.current(&|| Ok(())).is_err());
        drop(worker);
        assert!(expected.join(LOCK).is_file());
        assert!(!expected.join(SNAPSHOT).exists());
        assert!(!expected.join(META).exists());
    }
}

/// One actual journal identity together with its original absent/retained root.
/// No Clone, Deserialize or construction from the descriptive statement.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) struct ColdJournalReservation {
    pub(super) reservation: SessionReservation,
    pub(super) root: super::current_startup::PreparedRoot,
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl ColdJournalReservation {
    pub(crate) fn prepare(cfg: &AutosaveConfig, check: &impl Fn() -> io::Result<()>) -> io::Result<Self> {
        check()?;
        let reservation = SessionReservation::reserve(cfg);
        let root = super::current_startup::PreparedRoot::capture(cfg, &reservation.session_name, check)?;
        root.current_before(check)?;
        Ok(Self { reservation, root })
    }
    pub(crate) fn current(&self, cfg: &AutosaveConfig, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.reservation.current(cfg)?;
        self.root.current_before(check)
    }
    pub(crate) fn description(&self, cfg: &AutosaveConfig) -> io::Result<Value> {
        self.reservation.description(cfg)
    }
    pub(crate) fn root_description(&self) -> Value {
        self.root.description()
    }
}
