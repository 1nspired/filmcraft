use super::*;
use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    root: PathBuf,
    session: Session,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(worker) = self.session.persistence.as_mut() {
            worker.stop_worker();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn fixture(bound: bool) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("cold-bind-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let mut session = Session::default();
    crate::agent_bridge::native_auxiliary_tests::populate(&mut session);
    if bound {
        session.shortcuts.bind_directory(&root);
        session.presets.bind_directory(&root);
        session.export_presets.bind_directory(&root);
        session.prefs_path = Some(root.join("preferences.json"));
    }
    let cfg = AutosaveConfig::new(root.clone());
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    session.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    Fixture { root, session }
}
fn prepare(f: &Fixture) -> ColdCommit {
    ColdCommit::prepare(&f.session, DirectoryFrame::capture(&f.root, &|| Ok(())).unwrap(), &"b".repeat(64), &|| Ok(())).unwrap()
}
fn publish(f: &Fixture) -> ColdCommit {
    let mut original = prepare(f);
    assert_eq!(original.publish(&f.session, &|| Ok(())).unwrap()["published"], true);
    original
}
fn held_worker(s: &Session) -> Arc<crate::autosave::current_startup::Journal> {
    let worker = s.persistence.as_ref().unwrap();
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    worker.current_image_journal(&|| Ok(())).unwrap()
}
#[test]
fn cold_binding_original_unbound_services_preserve_library_memory_basis_and_same_actual_worker() {
    let mut f = fixture(false);
    let basis = crate::project_save::guard(&f.session).unwrap();
    let before = payloads(&f.session, true).unwrap().into_iter().map(|(name, bytes, _)| (name, bytes)).collect::<Vec<_>>();
    let worker = held_worker(&f.session);
    let graph = f.session.project.clone();
    let previews = f.session.previews.clone();
    let media = f.session.media.clone();
    let mut original = publish(&f);
    let original_sources = original.original.cold_sources.clone();
    assert!(original.same_original_journal(&worker));
    assert!(original.service_binding_description().is_none());
    let result = original.bind_services(&mut f.session, &|| Ok(())).unwrap();
    assert_eq!(result["bound"], true);
    assert_eq!(original.service_binding_description().unwrap()["phase"], "bound");
    assert_eq!(result["workersMayResume"], false);
    assert_eq!(original.original.cold_sources, original_sources);
    assert!(Arc::ptr_eq(&worker, &held_worker(&f.session)));
    assert!(Arc::ptr_eq(&graph, &f.session.project));
    assert!(Arc::ptr_eq(&previews, &f.session.previews));
    assert!(Arc::ptr_eq(&media, &f.session.media));
    assert!(basis.current(&f.session).is_err());
    basis.current_cold_binding(&f.session, original.service_bindings()).unwrap();
    assert_eq!(f.session.prefs_path, Some(f.root.join("preferences.json")));
    assert_eq!(f.session.shortcuts.recovery_directory(), Some(f.root.as_path()));
    assert_eq!(f.session.presets.recovery_directory(), Some(f.root.as_path()));
    assert_eq!(f.session.export_presets.dir(), Some(f.root.as_path()));
    assert_eq!(payloads(&f.session, true).unwrap().into_iter().map(|(name, bytes, _)| (name, bytes)).collect::<Vec<_>>(), before);
    original.verify_published(&f.session, &|| Ok(())).unwrap();
    assert!(original.bind_services(&mut f.session, &|| Ok(())).is_err());
}
#[test]
fn cold_binding_already_bound_services_keep_the_original_source_tuple_and_strict_basis() {
    let mut f = fixture(true);
    let basis = crate::project_save::guard(&f.session).unwrap();
    let mut original = publish(&f);
    let sources = original.original.cold_sources.clone();
    original.bind_services(&mut f.session, &|| Ok(())).unwrap();
    basis.current(&f.session).unwrap();
    basis.current_cold_binding(&f.session, original.service_bindings()).unwrap();
    assert_eq!(original.original.cold_sources, sources);
    original.verify_published(&f.session, &|| Ok(())).unwrap();
    held_worker(&f.session);
}
#[test]
fn cold_binding_before_canonical_publication_refuses_without_binding_or_consuming_original_writes() {
    let mut f = fixture(false);
    let basis = crate::project_save::guard(&f.session).unwrap();
    let mut original = prepare(&f);
    assert!(original.bind_services(&mut f.session, &|| Ok(())).is_err());
    basis.current(&f.session).unwrap();
    assert!(matches!(original.bindings.phase(), Phase::Before));
    assert_eq!(original.publish(&f.session, &|| Ok(())).unwrap()["published"], true);
    original.bind_services(&mut f.session, &|| Ok(())).unwrap();
    held_worker(&f.session);
}
#[test]
fn cold_binding_foreign_preference_or_each_library_path_refuses_without_rebinding_or_worker_release() {
    for role in 0..4 {
        let mut f = fixture(false);
        let mut original = publish(&f);
        original.bind_services(&mut f.session, &|| Ok(())).unwrap();
        let foreign = f.root.join("foreign-must-not-be-created");
        match role {
            0 => f.session.prefs_path = Some(foreign.join("preferences.json")),
            1 => f.session.shortcuts.bind_directory(&foreign),
            2 => f.session.presets.bind_directory(&foreign),
            _ => f.session.export_presets.bind_directory(&foreign),
        }
        assert!(original.verify_published(&f.session, &|| Ok(())).is_err());
        assert!(original.bind_services(&mut f.session, &|| Ok(())).is_err());
        assert!(!foreign.exists());
        held_worker(&f.session);
    }
}
#[test]
fn cold_binding_identical_value_replacement_project_cannot_reuse_the_original_continuation() {
    let mut f = fixture(false);
    let basis = crate::project_save::guard(&f.session).unwrap();
    let mut original = publish(&f);
    original.bind_services(&mut f.session, &|| Ok(())).unwrap();
    f.session.project = Arc::new((*f.session.project).clone());
    assert!(basis.current_cold_binding(&f.session, original.service_bindings()).is_err());
    assert!(original.verify_published(&f.session, &|| Ok(())).is_err());
    held_worker(&f.session);
}
#[test]
fn cold_binding_post_assignment_guard_failure_retains_actual_bound_paths_stopped_owner_and_worker() {
    let mut f = fixture(false);
    let mut original = publish(&f);
    let binding = original.bindings.clone();
    let worker = held_worker(&f.session);
    let check = || if binding.phase() == Phase::Bound { Err(io::Error::other("generated guard loss after actual assignments")) } else { Ok(()) };
    assert!(original.bind_services(&mut f.session, &check).is_err());
    assert!(matches!(binding.phase(), Phase::Stopped));
    assert_eq!(original.service_binding_description().unwrap()["phase"], "unresolved");
    assert_eq!(original.service_binding_description().unwrap()["bound"], false);
    assert_eq!(f.session.prefs_path, Some(f.root.join("preferences.json")));
    assert_eq!(f.session.presets.recovery_directory(), Some(f.root.as_path()));
    assert!(original.verify_published(&f.session, &|| Ok(())).is_err());
    assert!(original.bind_services(&mut f.session, &|| Ok(())).is_err());
    assert!(Arc::ptr_eq(&worker, &held_worker(&f.session)));
}
#[test]
fn cold_binding_post_assignment_callback_unwind_stops_only_the_same_continuation_without_replay() {
    let mut f = fixture(false);
    let mut original = publish(&f);
    let binding = original.bindings.clone();
    let worker = held_worker(&f.session);
    let check = || {
        assert!(binding.phase() != Phase::Bound, "generated callback unwind after original binding");
        Ok(())
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| original.bind_services(&mut f.session, &check)));
    assert!(result.is_err());
    assert!(matches!(binding.phase(), Phase::Stopped));
    assert_eq!(original.service_binding_description().unwrap()["phase"], "unresolved");
    assert_eq!(original.service_binding_description().unwrap()["bound"], false);
    assert_eq!(f.session.prefs_path, Some(f.root.join("preferences.json")));
    assert!(original.bind_services(&mut f.session, &|| Ok(())).is_err());
    assert!(Arc::ptr_eq(&worker, &held_worker(&f.session)));
}
#[test]
fn cold_binding_unrelated_preference_mutation_keeps_original_save_basis_stale() {
    let mut f = fixture(false);
    let basis = crate::project_save::guard(&f.session).unwrap();
    let mut original = publish(&f);
    original.bind_services(&mut f.session, &|| Ok(())).unwrap();
    f.session.prefs.auto_save.enabled = !f.session.prefs.auto_save.enabled;
    assert!(basis.current_cold_binding(&f.session, original.service_bindings()).is_err());
    assert!(original.verify_published(&f.session, &|| Ok(())).is_err());
    held_worker(&f.session);
}
