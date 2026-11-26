//! Owns the actual native Session until startup transfers it exactly once.
//!
//! This is lifetime custody, not admission. Cold Save still requires genuine
//! Root/shared/DataRoot/application closure before any future desktop transfer.
use filmcraft_engine::Session;

pub(crate) struct StartupSession {
    session: Option<Session>,
}

impl StartupSession {
    pub(crate) fn new(session: Session) -> Self {
        Self { session: Some(session) }
    }

    pub(crate) fn session_mut(&mut self) -> Result<&mut Session, String> {
        self.session.as_mut().ok_or_else(|| "native startup Session was already transferred or stopped".into())
    }

    /// Move the original Session; never rebuild a graph, history or worker.
    /// The caller must already own the appropriate original startup admission.
    pub(crate) fn take(&mut self) -> Result<Session, String> {
        self.session
            .as_ref()
            .ok_or_else(|| "native startup Session was already transferred or stopped".to_string())?
            .ensure_desktop_startup_transfer()
            .map_err(|error| error.to_string())?;
        self.session.take().ok_or_else(|| "native startup Session was already transferred or stopped".into())
    }

    /// Abandoned startup is not a clean editor shutdown. Stop/join the original
    /// persistence worker before dropping the remaining Session and its pause
    /// owners. Do not Flush, Clean, replay, synchronize or release a startup gate.
    pub(crate) fn stop(&mut self) -> Result<(), String> {
        let Some(mut session) = self.session.take() else { return Ok(()) };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(session.persistence.take());
            drop(session);
        }))
        .map_err(|_| "native startup cleanup failed unexpectedly; keep recovery data and inspect the startup log".into())
    }
}

impl Drop for StartupSession {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            log::error!("{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_engine::autosave::{self, AutosaveConfig};
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("poema-startup-session-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn generated() -> Session {
        let mut session = Session::default();
        session.execute("file.newProject", json!({"name":"Generated startup"})).unwrap();
        session
    }

    fn persisted(scratch: &Scratch) -> (Session, PathBuf, Vec<u8>, Vec<u8>) {
        let mut session = generated();
        let mut config = AutosaveConfig::new(&scratch.0);
        // Explicit Flush below is test setup only. An abandoned carrier must
        // not Flush a later queued image or mark the existing journal clean.
        config.journal_debounce = Duration::from_secs(3600);
        session.start_autosave(config).unwrap();
        session.prefs.auto_save.enabled = false;
        session.prefs.auto_save.recovery_interval_seconds = 3600;
        session.execute("file.newSequence", json!({"name":"Retained unsaved sequence"})).unwrap();
        session.sync_persistence();
        let persistence = session.persistence.as_ref().unwrap();
        persistence.flush();
        let directory = persistence.session_dir.clone();
        let snapshot = fs::read(directory.join("snapshot.fcproj")).unwrap();
        let meta = fs::read(directory.join("session.json")).unwrap();
        assert!(!serde_json::from_slice::<serde_json::Value>(&meta).unwrap()["cleanExit"].as_bool().unwrap());
        assert!(autosave::scan_candidates(&scratch.0, None).is_empty());
        (session, directory, snapshot, meta)
    }

    fn preserved(scratch: &Scratch, directory: &PathBuf, snapshot: &[u8], meta: &[u8]) {
        assert_eq!(fs::read(directory.join("snapshot.fcproj")).unwrap(), snapshot);
        assert_eq!(fs::read(directory.join("session.json")).unwrap(), meta);
        let candidates = autosave::scan_candidates(&scratch.0, None);
        assert_eq!(candidates.len(), 1);
        assert_eq!(&candidates[0].dir, directory);
        assert!(!candidates[0].meta.clean_exit);
        autosave::load_candidate(&candidates[0]).unwrap();
    }

    #[test]
    fn transfer_moves_the_same_graph_editor_history_path_and_runtime_once() {
        let mut session = generated();
        let previous = Arc::clone(&session.project);
        session.execute("file.newSequence", json!({"name":"Original history step"})).unwrap();
        session.path = Some("generated-startup.fcproj".into());
        let project = Arc::clone(&session.project);
        let media = Arc::clone(&session.media);
        let services = Arc::clone(&session.services);
        let state = serde_json::to_value(&session.state).unwrap();
        let journal = session.journal.clone();
        let revision = session.revision;
        let saved_revision = session.saved_revision;
        let mut owner = StartupSession::new(session);
        let mut actual = owner.take().unwrap();
        assert!(owner.take().is_err());
        assert!(owner.session_mut().is_err());
        owner.stop().unwrap();
        assert!(Arc::ptr_eq(&project, &actual.project));
        assert!(Arc::ptr_eq(&media, &actual.media));
        assert!(Arc::ptr_eq(&services, &actual.services));
        assert_eq!(serde_json::to_value(&actual.state).unwrap(), state);
        assert_eq!(actual.path.as_deref(), Some("generated-startup.fcproj"));
        assert_eq!(actual.journal, journal);
        assert_eq!((actual.revision, actual.saved_revision), (revision, saved_revision));
        actual.undo().unwrap();
        assert!(Arc::ptr_eq(&previous, &actual.project));
        actual.redo().unwrap();
        assert!(Arc::ptr_eq(&project, &actual.project));
    }

    #[test]
    fn mutable_startup_borrow_keeps_the_actual_session_until_transfer() {
        let mut owner = StartupSession::new(generated());
        let project = Arc::clone(&owner.session_mut().unwrap().project);
        assert!(Arc::ptr_eq(&project, &owner.session_mut().unwrap().project));
        owner.session_mut().unwrap().execute("file.newSequence", json!({"name":"Prepared once"})).unwrap();
        let prepared = Arc::clone(&owner.session_mut().unwrap().project);
        assert!(!Arc::ptr_eq(&project, &prepared));
        assert!(Arc::ptr_eq(&prepared, &owner.take().unwrap().project));
    }

    #[test]
    fn abandoned_startup_joins_original_worker_and_preserves_recovery() {
        let scratch = Scratch::new();
        let (session, directory, snapshot, meta) = persisted(&scratch);
        drop(StartupSession::new(session));
        preserved(&scratch, &directory, &snapshot, &meta);
    }

    #[test]
    fn explicit_stop_preserves_recovery_and_cannot_transfer_or_stop_twice() {
        let scratch = Scratch::new();
        let (session, directory, snapshot, meta) = persisted(&scratch);
        let mut owner = StartupSession::new(session);
        owner.session_mut().unwrap().execute("file.newSequence", json!({"name":"Queued but not flushed"})).unwrap();
        owner.stop().unwrap();
        assert!(owner.take().is_err());
        assert!(owner.session_mut().is_err());
        owner.stop().unwrap();
        drop(owner);
        preserved(&scratch, &directory, &snapshot, &meta);
    }

    #[test]
    fn callback_unwind_abandons_startup_without_cleaning_recovery() {
        let scratch = Scratch::new();
        let (session, directory, snapshot, meta) = persisted(&scratch);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _owner = StartupSession::new(session);
            panic!("generated startup callback failure");
        }));
        assert!(outcome.is_err());
        preserved(&scratch, &directory, &snapshot, &meta);
    }

    #[test]
    fn prepared_slot_last_owner_stops_worker_without_a_window_or_default_session() {
        let scratch = Scratch::new();
        let (session, directory, snapshot, meta) = persisted(&scratch);
        let prepared = Arc::new(Mutex::new(StartupSession::new(session)));
        let graphics_retry = Arc::clone(&prepared);
        drop(prepared);
        assert!(autosave::scan_candidates(&scratch.0, None).is_empty());
        drop(graphics_retry);
        preserved(&scratch, &directory, &snapshot, &meta);
    }

    #[test]
    fn transferred_worker_stays_live_until_the_actual_session_is_dropped() {
        let scratch = Scratch::new();
        let (session, directory, snapshot, meta) = persisted(&scratch);
        let mut owner = StartupSession::new(session);
        let actual = owner.take().unwrap();
        drop(owner);
        assert!(actual.persistence.is_some());
        assert_eq!(actual.persistence.as_ref().unwrap().session_dir, directory);
        assert!(autosave::scan_candidates(&scratch.0, None).is_empty());
        // No replacement Session or second persistence worker is constructed.
        drop(actual);
        preserved(&scratch, &directory, &snapshot, &meta);
    }
}
