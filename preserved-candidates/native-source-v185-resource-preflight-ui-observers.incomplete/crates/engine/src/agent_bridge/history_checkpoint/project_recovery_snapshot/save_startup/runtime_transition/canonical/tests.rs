use super::*;
use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
use std::sync::atomic::{AtomicU64, Ordering};

// Actual original native Commit and worker, with generated coordinator flags
// and controls for parser/status tests. This supplies no Pending/Input owner,
// original host durability, startup admission or worker release.
struct Fixture {
    root: PathBuf,
    session: Session,
    canonical: Canonical,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut worker) = self.session.persistence.take() {
            worker.stop_worker();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn fixture() -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("binding-observation-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let mut session = Session::default();
    let cfg = AutosaveConfig::new(root.clone());
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    session.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    let mut commit = workspace_update::ColdCommit::prepare(&session, DirectoryFrame::capture(&root, &|| Ok(())).unwrap(), &"a".repeat(64), &|| Ok(())).unwrap();
    let result = commit.publish(&session, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true);
    let canonical = Canonical {
        commit: Arc::new(commit),
        plan: json!({"generatedParserPlan":true}),
        intent_attempted: true,
        intent_confirmed: true,
        publication_attempted: true,
        result: Some(result),
        observation_attempted: true,
        observation_confirmed: true,
        binding_result: None,
        binding_observation_attempted: false,
        binding_observation_confirmed: false,
    };
    Fixture { root, session, canonical }
}
fn bind(f: &mut Fixture) {
    f.canonical.binding_result = Some(Arc::get_mut(&mut f.canonical.commit).unwrap().bind_services(&mut f.session, &|| Ok(())).unwrap());
}
fn receipt(f: &Fixture) -> Value {
    let observed = json!({"path":f.root.join("generated-control.json"),"sha256":"a".repeat(64),"bytes":1,
        "physical":{"dev":"1","ino":"2","mode":"33152","uid":"501","size":"1","mtimeNs":"1","ctimeNs":"1"}});
    json!({"kind":"poema-original-Save-native-service-bindings-receipt","version":1,"plan":f.canonical.plan,
        "result":f.canonical.result,"bindings":f.canonical.binding_result,"anchor":observed,"head":observed})
}
fn held_worker(f: &Fixture) {
    let worker = f.session.persistence.as_ref().unwrap();
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    worker.current_image_journal(&|| Ok(())).unwrap();
}
#[test]
fn native_binding_status_comes_only_from_original_bound_continuation_without_confirming_a_journal_or_releasing_worker() {
    let mut f = fixture();
    assert!(f.canonical.description().get("serviceBindings").is_none());
    bind(&mut f);
    let status = f.canonical.description();
    assert_eq!(status["serviceBindings"]["phase"], "bound");
    assert_eq!(status["serviceBindings"]["bound"], true);
    assert_eq!(status["serviceBindings"]["preferences"], json!(f.root.join("preferences.json")));
    assert_eq!(status["bindingObservationConfirmed"], false);
    assert_eq!(status["workersMayResume"], false);
    assert_eq!(status["serviceBindings"]["startupAuthority"], false);
    f.canonical.binding_receipt(&receipt(&f)).unwrap();
    assert!(!f.canonical.binding_observation_attempted);
    assert!(!f.canonical.binding_observation_confirmed);
    held_worker(&f);
}
#[test]
fn native_binding_receipt_cannot_supply_a_missing_original_binding_or_an_unretained_actual_result() {
    let mut f = fixture();
    f.canonical.binding_result = Some(json!({"copiedBinding":true}));
    assert!(f.canonical.binding_receipt(&receipt(&f)).is_err());
    assert!(f.canonical.description().get("serviceBindings").is_none());
    bind(&mut f);
    let message = receipt(&f);
    f.canonical.binding_result = None;
    assert!(f.canonical.binding_receipt(&message).is_err());
    assert!(!f.canonical.binding_observation_confirmed);
    held_worker(&f);
}
#[test]
fn native_binding_receipt_checks_complete_original_values_and_unknown_fields_and_pending_confirmation_stays_closed() {
    let mut f = fixture();
    bind(&mut f);
    let message = receipt(&f);
    f.canonical.binding_receipt(&message).unwrap();
    for field in ["plan", "result", "bindings", "kind", "version", "unknown", "anchor-extra"] {
        let mut changed = message.clone();
        match field {
            "plan" | "result" | "bindings" => changed[field] = json!({"foreign":true}),
            "kind" => changed[field] = json!("copied-receipt"),
            "version" => changed[field] = json!(2),
            "anchor-extra" => changed["anchor"]["unknown"] = json!(true),
            _ => changed[field] = json!(true),
        }
        assert!(f.canonical.binding_receipt(&changed).is_err(), "{field}");
    }
    f.canonical.binding_observation_attempted = true;
    assert!(f.canonical.transition_current().is_err());
    assert_eq!(f.canonical.description()["bindingObservationConfirmed"], false);
    assert_eq!(f.canonical.description()["workersMayResume"], false);
    held_worker(&f);
}

#[test]
fn canonical_source_sharing_requires_actual_bound_owner_and_both_original_acknowledgements_and_disallows_mutating_replay() {
    let mut f = fixture();
    assert!(f.canonical.retained_published().is_err());
    bind(&mut f);
    assert!(f.canonical.retained_published().is_err());
    // Coordinator flags here are synthetic, not a fabricated Input or runtime.
    f.canonical.binding_observation_attempted = true;
    f.canonical.binding_observation_confirmed = true;
    let retained = f.canonical.retained_published().unwrap();
    assert!(Arc::ptr_eq(&retained, &f.canonical.commit));
    assert!(Arc::get_mut(&mut f.canonical.commit).is_none());
    retained.verify_published_source(&|| Ok(())).unwrap();
    drop(retained);
    assert!(Arc::get_mut(&mut f.canonical.commit).is_some());
    held_worker(&f);
}
