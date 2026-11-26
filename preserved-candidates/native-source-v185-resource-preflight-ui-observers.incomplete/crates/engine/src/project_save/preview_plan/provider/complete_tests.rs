use super::super::VolumeSpec;
use super::*;
use crate::{
    Session,
    autosave::AutosaveConfig,
    project_save::{self, intent::Intent, operation, retained_effects},
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn id() -> String {
    format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst))
}
fn scope() -> serde_json::Value {
    json!({"projectId":"p_complete_save","workspaceId":"9f102c8b-f321-4379-8407-a74af10da5d1"})
}
fn name() -> String {
    format!("{}.mov", "a".repeat(32))
}
pub(super) fn fixture() -> (Session, PathBuf, PathBuf) {
    let base = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir);
    let root = base.join(format!(
        "complete-save-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let source = root.join("cache");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(name()), vec![0x41; 131075]).unwrap();
    fs::write(source.join("unindexed.keep"), b"human data").unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Complete Save","workspace":scope()})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    s.start_autosave(AutosaveConfig::new(root.join("runtime"))).unwrap();
    s.prefs_path = Some(root.join("preferences/missing/prefs.json"));
    s.prefs.general.recent_projects.clear();
    s.loaded_schema = 1;
    s.previews.set_temp_root(Some(root.clone()));
    s.previews.set_dir(Some(source.clone()));
    s.sync_persistence();
    s.persistence.as_ref().unwrap().flush();
    s.apply_persistence_events();
    (s, root, source)
}
fn specs(root: &Path) -> Vec<VolumeSpec> {
    vec![VolumeSpec { root: root.into(), device: fs::metadata(root).unwrap().dev().to_string(), minimum_free: 0 }]
}
fn begin(s: &mut Session, root: &Path, kind: &str) -> (operation::Key, String) {
    let intent =
        Intent::parse(&if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) })
            .unwrap();
    let operation = id();
    (operation::begin(s, &operation, &intent, &|| Ok(())).unwrap(), operation)
}
fn pending(s: &Session, key: &operation::Key) {
    assert_eq!(operation::status(s, key, &|| Ok(())).unwrap()["phase"], "AwaitingReconciliation");
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
}
fn row<'a>(docs: &'a documents::Documents, role: retained_effects::Role) -> &'a documents::Row {
    docs.rows.iter().find(|r| r.role == role).unwrap()
}
#[test]
fn complete_native_planned_Save_retains_all_original_roles_preview_receipts_and_both_worker_leases_for_all_four_intents() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root, source) = fixture();
        let original = fs::read(root.join("Original.fcproj")).unwrap();
        let token = id();
        let job = id();
        let doc = s.execute("agent.readProject", json!({"token":token,"scope":scope(),"projectPath":s.path})).unwrap();
        let intent = if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) };
        s.execute(
            "agent.startProjectSavePlan",
            json!({"job":job,"query":{"token":token,"scope":scope(),"beforeProjectSha256":doc["projectState"]["sha256"],"intent":intent}}),
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let r = s.execute("agent.projectSavePlanStatus", json!({"job":job})).unwrap();
            if r["status"] == "delivered" {
                break;
            }
            assert_eq!(r["status"], "running");
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        let key = crate::agent_bridge::begin_planned_project_save(&mut s, &job, &id(), &|| Ok(())).unwrap();
        operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        pending(&s, &key);
        let report = operation::preview_report(&s, &key).unwrap().unwrap();
        let docs = report.document_admission.as_ref().unwrap();
        assert_eq!(docs.description()["allDocumentBudgetsIncluded"], true);
        assert_eq!(docs.description()["writeAuthority"], false);
        assert!(row(docs, retained_effects::Role::Project).assigned);
        assert!(operation::documents(&s, &key).unwrap().iter().all(|d| d.qualified));
        if kind == "save_copy" {
            assert_eq!(docs.rows.len(), 1);
            assert!(docs.preferences.is_none());
            assert!(source.join(name()).is_file());
        } else {
            assert!(row(docs, retained_effects::Role::Preferences).assigned);
            assert!(root.join("preferences/missing/prefs.json").is_file());
            let directory = docs.preferences.as_ref().unwrap().publication.as_ref().unwrap();
            assert!(directory.complete && !directory.custody);
            assert_eq!(directory.successful_calls.len(), 2);
            assert_eq!(report.files.iter().find(|r| r.name == name()).unwrap().phase, Phase::Renamed);
        }
        if matches!(kind, "save" | "save_all") {
            assert!(row(docs, retained_effects::Role::SchemaBackup).assigned);
            assert_eq!(fs::read(root.join("Original (schema v1 backup).fcproj")).unwrap(), original);
        }
        assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"human data");
        assert!(report.initial_demand.values().sum::<u64>() > 0);
        assert!(operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn complete_schema_backup_candidate_keeps_original_readable_existing_backup_skip_and_unused_credit() {
    let (mut s, root, _) = fixture();
    let backup = root.join("Original (schema v1 backup).fcproj");
    fs::write(&backup, b"existing human backup").unwrap();
    let (key, _) = begin(&mut s, &root, "save");
    operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    let docs = report.document_admission.as_ref().unwrap();
    assert!(!row(docs, retained_effects::Role::SchemaBackup).assigned);
    assert_eq!(fs::read(backup).unwrap(), b"existing human backup");
    assert!(report.remaining.values().sum::<u64>() > 0);
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn document_roles_need_explicit_matching_working_roots_before_any_publication_and_wrong_devices_never_fallback() {
    let (mut s, root, source) = fixture();
    s.previews.set_temp_root(None);
    let (key, _) = begin(&mut s, &root, "save_copy");
    let mut wrong = specs(&source);
    wrong[0].device = "wrong".into();
    assert!(operation::dispatch_admitted_files(&mut s, &key, &wrong, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(operation::dispatch_admitted_files(&mut s, &key, &specs(&source), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(operation::preview_report(&s, &key).unwrap().is_none());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn shared_document_and_preview_credits_cover_two_payloads_per_document_once_and_refuse_overflow_before_io() {
    let (mut s, root, _) = fixture();
    let (key, operation) = begin(&mut s, &root, "save");
    let mut roots = specs(&root);
    let mut nested = roots[0].clone();
    nested.root = root.join("cache");
    nested.minimum_free = 17;
    roots.push(nested);
    let plan = Plan::capture(&s, &key, &roots, &|| Ok(())).unwrap();
    let preview = plan.demands[0].required_bytes;
    let unit = plan.volumes[0].before.allocation_unit;
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new_complete(plan, operation::prepared(&s, &key).unwrap(), &operation, &mut original, &|| Ok(())).unwrap();
    let expected = p
        .documents
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| {
            let size = r.payload.as_ref().unwrap().0;
            2 * allocation(size, unit).unwrap() + 8 * allocation(super::super::METADATA_BOUND, unit).unwrap()
        })
        .sum::<u64>()
        + 2 * 8 * unit;
    assert_eq!(p.initial_demand.values().sum::<u64>(), preview + expected);
    assert_eq!(p.ledger.remaining.len(), 1);
    let role = Role { name: String::new(), kind: Kind::Document(retained_effects::Role::Project) };
    p.ledger.enter(role.clone()).unwrap();
    assert!(p.ledger.enter(role).is_err());
    let device = p.ledger.remaining.keys().next().unwrap().clone();
    p.ledger.remaining.insert(device, u64::MAX);
    assert!(p.guard().is_err());
    assert!(!root.join("preferences").exists());
    drop(p);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn later_destination_file_is_preserved_and_never_reinterpreted_as_an_admitted_overwrite() {
    let (mut s, root, _) = fixture();
    let (key, operation) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new_complete(plan, operation::prepared(&s, &key).unwrap(), &operation, &mut original, &|| Ok(())).unwrap();
    p.complete_current(&s, &key).unwrap();
    fs::write(root.join("Chosen.fcproj"), b"later human destination").unwrap();
    assert!(operation::dispatch_retained(&mut s, &key, &mut p, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read(root.join("Chosen.fcproj")).unwrap(), b"later human destination");
    assert!(!row(p.documents.as_ref().unwrap(), retained_effects::Role::Project).assigned);
    assert!(operation::documents(&s, &key).unwrap().iter().all(|d| d.publication.is_none()));
    drop(p);
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn later_preferences_parent_replacement_is_not_adopted_after_project_publication() {
    let (mut s, root, _) = fixture();
    fs::create_dir_all(root.join("preferences/missing")).unwrap();
    fs::write(root.join("preferences/missing/prefs.json"), b"original preferences").unwrap();
    let (key, operation) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new_complete(plan, operation::prepared(&s, &key).unwrap(), &operation, &mut original, &|| Ok(())).unwrap();
    p.complete_current(&s, &key).unwrap();
    fs::rename(root.join("preferences"), root.join("held-preferences")).unwrap();
    fs::create_dir_all(root.join("preferences/missing")).unwrap();
    fs::write(root.join("preferences/missing/prefs.json"), b"later human preferences").unwrap();
    operation::dispatch_retained(&mut s, &key, &mut p, &|| Ok(()), &|| Ok(())).unwrap();
    assert!(root.join("Chosen.fcproj").is_file());
    assert!(!row(p.documents.as_ref().unwrap(), retained_effects::Role::Preferences).assigned);
    assert_eq!(fs::read(root.join("held-preferences/missing/prefs.json")).unwrap(), b"original preferences");
    assert_eq!(fs::read(root.join("preferences/missing/prefs.json")).unwrap(), b"later human preferences");
    drop(p);
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn Stop_after_first_real_preferences_mkdir_keeps_partial_calls_and_never_creates_the_file() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let io = || if root.join("preferences").exists() { Err(error("generated Stop after first preferences mkdir")) } else { Ok(()) };
    operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &io, &|| Ok(())).unwrap();
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    let p = report.document_admission.as_ref().unwrap().preferences.as_ref().unwrap().publication.as_ref().unwrap();
    assert!(!p.complete);
    assert_eq!(p.successful_calls, vec![root.join("preferences")]);
    assert!(!root.join("preferences/missing/prefs.json").exists());
    assert!(root.join("Chosen.fcproj").is_file());
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn document_preview_path_collision_is_refused_before_any_original_file_is_changed() {
    let (mut s, root, source) = fixture();
    let before = fs::read(source.join(name())).unwrap();
    let request = Intent::parse(&json!({"kind":"save_as","path":source.join(name())})).unwrap();
    let key = operation::begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    assert!(operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read(source.join(name())).unwrap(), before);
    assert!(!root.join("preferences").exists());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn complete_staging_requires_original_role_path_operation_and_payload_without_direct_file_fallback_or_replay() {
    let (mut s, root, _) = fixture();
    let (key, operation) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let prepared = operation::prepared(&s, &key).unwrap();
    let bytes = prepared.bytes.clone();
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new_complete(plan, prepared, &operation, &mut original, &|| Ok(())).unwrap();
    let tag = format!("{:x}", Sha256::digest(format!("{}:{:?}", operation, retained_effects::Role::Project).as_bytes()));
    let path = root.join("Chosen.fcproj");
    assert!(p.document(&*s.services, DocumentRole::Project, path.to_str().unwrap(), &bytes).is_err());
    assert!(p.stage_document(retained_effects::Role::Project, &path, &id(), &bytes, &|| Ok(())).is_err());
    assert!(p.stage_document(retained_effects::Role::Project, &path, &tag, b"wrong payload", &|| Ok(())).is_err());
    assert!(p.stage_document(retained_effects::Role::Project, &root.join("different.fcproj"), &tag, &bytes, &|| Ok(())).is_err());
    let staged = p.stage_document(retained_effects::Role::Project, &path, &tag, &bytes, &|| Ok(())).unwrap();
    assert!(!path.exists());
    assert!(p.stage_document(retained_effects::Role::Project, &path, &tag, &bytes, &|| Ok(())).is_err());
    drop(staged);
    drop(p);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
struct StopEvent(std::sync::Arc<AtomicBool>);
impl FileEffects for StopEvent {
    fn event(&mut self, event: Event) {
        if matches!(event, Event::Document { role: DocumentRole::Project, result: crate::project_save::effects::CallResult::ReturnedOk { .. }, .. }) {
            self.0.store(true, Ordering::Release);
        }
    }
}
#[test]
fn Stop_after_actual_document_write_keeps_whole_file_frames_real_receipts_unused_credits_and_workers_owned() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let io = || if stop.load(Ordering::Acquire) { Err(error("generated late file Stop")) } else { Ok(()) };
    assert!(
        operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut StopEvent(stop.clone()), &io, &|| if stop.load(Ordering::Acquire) {
            Err(crate::EngineError::Other("late Save observation Stop".into()))
        } else {
            Ok(())
        })
        .is_err()
    );
    assert!(root.join("Chosen.fcproj").is_file());
    assert!(!root.join("preferences").exists());
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    assert!(row(report.document_admission.as_ref().unwrap(), retained_effects::Role::Project).assigned);
    assert!(report.remaining.values().sum::<u64>() > 0);
    assert!(operation::documents(&s, &key).unwrap().iter().any(|d| d.publication.is_some()));
    pending(&s, &key);
    assert!(operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
    s.shutdown();
}
struct Cwd(PathBuf);
impl Drop for Cwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}
#[test]
fn relative_save_uses_actual_admitted_native_directory_and_later_directory_change_refuses_without_fallback() {
    let previous = std::env::current_dir().unwrap();
    let _restore = Cwd(previous.clone());
    let (mut s, root, _) = fixture();
    std::env::set_current_dir(&root).unwrap();
    let request = Intent::parse(&json!({"kind":"save_copy","path":"Relative.fcproj"})).unwrap();
    let key = operation::begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    assert!(root.join("Relative.fcproj").is_file());
    pending(&s, &key);
    s.shutdown();
    std::env::set_current_dir(&previous).unwrap();
    let (mut s, root, _) = fixture();
    let (key, operation) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut original = crate::project_save::effects::Original;
    let p = Provider::new_complete(plan, operation::prepared(&s, &key).unwrap(), &operation, &mut original, &|| Ok(())).unwrap();
    std::env::set_current_dir(&root).unwrap();
    assert!(p.guard().is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    drop(p);
    std::env::set_current_dir(&previous).unwrap();
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}

#[test]
fn complete_effect_observation_retains_every_original_document_preview_directory_and_credit_field_for_all_intents() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root, source) = fixture();
        let (key, _) = begin(&mut s, &root, kind);
        operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        let report = operation::preview_report(&s, &key).unwrap().unwrap();
        let expected_files = report.files.len();
        let expected_admission = report.document_admission.as_ref().unwrap().description();
        let v = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(v["previews"]["files"].as_array().unwrap().len(), expected_files);
        assert_eq!(v["previews"]["documentAdmission"], expected_admission);
        assert_eq!(v["previews"]["initialDemand"], serde_json::to_value(&report.initial_demand).unwrap());
        assert_eq!(v["previews"]["remaining"], serde_json::to_value(&report.remaining).unwrap());
        assert_eq!(v["previews"]["directories"], serde_json::to_value(&report.directories).unwrap());
        for row in v["previews"]["files"].as_array().unwrap() {
            for role in ["rename", "copy", "retirement"] {
                if !row[role].is_null() {
                    assert!(row[role]["receipt"].is_object());
                    assert!(row[role]["inspection"]["observation"].is_object() || row[role]["inspection"]["error"].is_string());
                }
            }
        }
        assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"human data");
        assert_eq!(v["nativeComplete"], false);
        pending(&s, &key);
        s.shutdown();
    }
}
#[test]
fn original_successful_body_with_auxiliary_io_refusal_remains_explicit_in_complete_effect_observation() {
    struct StopAfterProject(Arc<AtomicBool>);
    impl crate::project_save::effects::FileEffects for StopAfterProject {
        fn event(&mut self, event: crate::project_save::effects::Event) {
            if matches!(event, crate::project_save::effects::Event::Document { role: crate::project_save::effects::DocumentRole::Project, .. }) {
                self.0.store(true, Ordering::Release);
            }
        }
    }
    let (mut s, root, source) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let stopped = Arc::new(AtomicBool::new(false));
    let io = || if stopped.load(Ordering::Acquire) { Err(io::Error::other("generated auxiliary IO refusal")) } else { Ok(()) };
    let mut delegate = StopAfterProject(stopped.clone());
    let _ = operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut delegate, &io, &|| Ok(()));
    let v = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(v["body"]["status"], "ok");
    // The retained document guard refuses before the preview provider runs.
    // Require the actual auxiliary call/error, not an unvisited inner reader.
    assert_eq!(v["previews"]["boundaryErrors"], json!([]));
    let events = v["events"].as_array().unwrap();
    let written = events.iter().position(|e| e["type"] == "document" && e["role"] == "Project" && e["result"]["status"] == "returned_ok").unwrap();
    let refused = events
        .iter()
        .position(|e| {
            e["type"] == "directories"
                && e["role"] == "Previews"
                && e["result"]["status"] == "returned_error"
                && e["result"]["detail"] == "generated auxiliary IO refusal"
        })
        .unwrap();
    assert!(written < refused);
    assert_eq!(v["previews"]["directories"], Value::Null);
    assert_eq!(v["previews"]["files"][0]["phase"], "Fresh");
    assert_eq!(v["previews"]["documentAdmission"]["preferencesDirectory"]["attempted"], false);
    assert_eq!(s.saved_revision, s.revision);
    assert_eq!(s.path.as_deref(), root.join("Chosen.fcproj").to_str());
    assert!(root.join("Chosen.fcproj").is_file());
    assert!(source.join(name()).is_file());
    assert!(!root.join("preferences/missing/prefs.json").exists());
    assert_eq!(v["allEffectsVerified"], false);
    assert_eq!(v["workersMayResume"], false);
    assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"human data");
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn original_effect_observation_keeps_corrupt_physical_receipt_errors_beside_full_historical_publications_without_repair() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_copy");
    operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    let directory = operation::documents(&s, &key).unwrap()[0].publication.as_ref().unwrap().directory.clone();
    let file = directory.join("intent.json");
    fs::write(&file, b"generated damaged immutable intent").unwrap();
    let v = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
    let row = &v["documents"][0];
    assert_eq!(row["qualifiedAtReturn"], true);
    assert!(row["publication"]["inspection"]["error"].is_string());
    assert_eq!(row["publication"]["inspection"]["observation"], Value::Null);
    assert!(row["publication"]["receipt"].is_object());
    assert_eq!(fs::read(file).unwrap(), b"generated damaged immutable intent");
    assert_eq!(v["readComplete"], true);
    assert_eq!(v["allEffectsVerified"], false);
    pending(&s, &key);
    s.shutdown();
}

fn prepared_pending(s: &Session, key: &operation::Key) {
    assert_eq!(operation::status(s, key, &|| Ok(())).unwrap()["phase"], "Prepared");
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
}
#[test]
fn retained_complete_admission_uses_same_original_frames_credits_and_service_for_all_four_intents() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root, source) = fixture();
        let (key, _) = begin(&mut s, &root, kind);
        let old = fs::read(root.join("Original.fcproj")).unwrap();
        let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
        let description = admission.description(&|| Ok(())).unwrap();
        assert_eq!(description["kind"], "native_original_complete_Save_file_admission");
        assert_eq!(description["documents"]["allDocumentBudgetsIncluded"], true);
        assert_eq!(description["initialDemand"], description["remaining"]);
        assert_eq!(admission.description(&|| Ok(())).unwrap(), description);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), old);
        assert!(!root.join("Chosen.fcproj").exists());
        operation::dispatch_prepared_files(&mut s, &key, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        let observed = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(observed["body"]["status"], "ok");
        assert_eq!(observed["previews"]["initialDemand"], description["initialDemand"]);
        assert_eq!(observed["previews"]["planned"], description["planned"]);
        assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"human data");
        assert_eq!(observed["workersMayResume"], false);
        pending(&s, &key);
        assert!(operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn retained_admission_preserves_later_new_destination_instead_of_recapturing_it_as_an_overwrite() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_copy");
    let original = fs::read(root.join("Original.fcproj")).unwrap();
    let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let target = root.join("Chosen.fcproj");
    fs::write(&target, b"later human destination").unwrap();
    assert!(operation::dispatch_prepared_files(&mut s, &key, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read(target).unwrap(), b"later human destination");
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), original);
    assert_eq!(operation::status(&s, &key, &|| Ok(())).unwrap()["phase"], "Prepared");
    assert_eq!(operation::observation::read(&s, &key, &|| Ok(())).unwrap()["events"], json!([]));
    prepared_pending(&s, &key);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn retained_admission_preserves_later_existing_destination_bytes_and_same_byte_replacement_generations() {
    for same_bytes in [false, true] {
        let (mut s, root, _) = fixture();
        let target = root.join("Chosen.fcproj");
        fs::write(&target, b"original target bytes").unwrap();
        let (key, _) = begin(&mut s, &root, "save_copy");
        let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
        let newer = if same_bytes { b"original target bytes".as_slice() } else { b"new human target bytes".as_slice() };
        let replacement = root.join("replacement.fcproj");
        fs::write(&replacement, newer).unwrap();
        fs::rename(&replacement, &target).unwrap();
        assert!(operation::dispatch_prepared_files(&mut s, &key, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
        assert_eq!(fs::read(target).unwrap(), newer);
        assert_eq!(operation::observation::read(&s, &key, &|| Ok(())).unwrap()["body"], Value::Null);
        prepared_pending(&s, &key);
        s.shutdown();
    }
}
#[test]
fn retained_admission_later_preferences_or_indexed_preview_changes_refuse_before_project_publication() {
    for role in ["preferences", "preview"] {
        let (mut s, root, source) = fixture();
        let (key, _) = begin(&mut s, &root, "save_as");
        let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
        let changed = if role == "preferences" {
            let p = root.join("preferences/missing/prefs.json");
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            p
        } else {
            source.join(name())
        };
        fs::write(&changed, b"later human auxiliary bytes").unwrap();
        assert!(operation::dispatch_prepared_files(&mut s, &key, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
        assert!(!root.join("Chosen.fcproj").exists());
        assert_eq!(fs::read(changed).unwrap(), b"later human auxiliary bytes");
        assert_eq!(operation::observation::read(&s, &key, &|| Ok(())).unwrap()["events"], json!([]));
        prepared_pending(&s, &key);
        s.shutdown();
    }
}
#[test]
fn retained_admission_later_parent_replacement_refuses_without_adopting_the_new_directory() {
    let (mut s, root, _) = fixture();
    fs::create_dir_all(root.join("preferences/missing")).unwrap();
    let (key, _) = begin(&mut s, &root, "save_as");
    let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    fs::rename(root.join("preferences"), root.join("old-preferences")).unwrap();
    fs::create_dir_all(root.join("preferences/missing")).unwrap();
    assert!(operation::dispatch_prepared_files(&mut s, &key, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(root.join("old-preferences/missing").is_dir());
    prepared_pending(&s, &key);
    s.shutdown();
}
#[test]
fn retained_admission_cannot_be_bound_to_another_real_native_key_or_session() {
    let (mut first, a, _) = fixture();
    let (key, _) = begin(&mut first, &a, "save_copy");
    let admission = operation::prepare_dispatch_files(&first, &key, &specs(&a), &|| Ok(())).unwrap();
    let (mut second, b, _) = fixture();
    let (other, _) = begin(&mut second, &b, "save_copy");
    assert!(operation::dispatch_prepared_files(&mut second, &other, admission, &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(!a.join("Chosen.fcproj").exists());
    assert!(!b.join("Chosen.fcproj").exists());
    prepared_pending(&first, &key);
    prepared_pending(&second, &other);
    first.shutdown();
    second.shutdown();
}
#[test]
fn retained_admission_stopped_bind_or_read_preserves_original_owner_without_starting_another_file_plan() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let expected = admission.description(&|| Ok(())).unwrap();
    assert!(admission.description(&|| Err(crate::EngineError::Other("generated stopped description".into()))).is_err());
    assert_eq!(admission.description(&|| Ok(())).unwrap(), expected);
    assert!(
        operation::dispatch_prepared_files(
            &mut s,
            &key,
            admission,
            &mut crate::project_save::effects::Original,
            &|| Err(io::Error::other("generated stopped original admission")),
            &|| Ok(())
        )
        .is_err()
    );
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(operation::status(&s, &key, &|| Ok(())).unwrap()["phase"], "Prepared");
    prepared_pending(&s, &key);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn retained_admission_original_partial_auxiliary_failure_keeps_receipts_unused_credits_and_after_state() {
    struct Stop(Arc<AtomicBool>);
    impl crate::project_save::effects::FileEffects for Stop {
        fn event(&mut self, event: crate::project_save::effects::Event) {
            if matches!(event, crate::project_save::effects::Event::Document { role: crate::project_save::effects::DocumentRole::Project, .. }) {
                self.0.store(true, Ordering::Release);
            }
        }
    }
    let (mut s, root, source) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let admission = operation::prepare_dispatch_files(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let declared = admission.description(&|| Ok(())).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let io = || if stopped.load(Ordering::Acquire) { Err(io::Error::other("generated stopped auxiliary work")) } else { Ok(()) };
    let mut delegate = Stop(stopped.clone());
    let _ = operation::dispatch_prepared_files(&mut s, &key, admission, &mut delegate, &io, &|| Ok(()));
    let observed = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(observed["body"]["status"], "ok");
    assert_eq!(observed["previews"]["initialDemand"], declared["initialDemand"]);
    assert_eq!(observed["nativeAfterAvailable"], true);
    assert!(root.join("Chosen.fcproj").is_file());
    assert!(source.join(name()).is_file());
    assert!(!root.join("preferences/missing/prefs.json").exists());
    assert_eq!(observed["workersMayResume"], false);
    pending(&s, &key);
    s.shutdown();
}

#[test]
fn original_returned_Save_receipt_verification_checks_all_four_intents_without_resuming_workers_or_replaying_publication() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root, _) = fixture();
        let (key, _) = begin(&mut s, &root, kind);
        operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        let first = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(first["version"], 2);
        let receipt = &first["returnedReceiptVerification"];
        assert_eq!(receipt["status"], "verified", "{kind}: {receipt}");
        assert_eq!(receipt["documents"], operation::documents(&s, &key).unwrap().len());
        for flag in ["current", "allEffectsVerified", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(receipt[flag], false);
            assert_eq!(first[flag], false);
        }
        assert_eq!(first, operation::observation::read(&s, &key, &|| Ok(())).unwrap());
        pending(&s, &key);
        assert!(operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
        pending(&s, &key);
        s.shutdown();
    }
}
#[test]
fn original_returned_Save_receipt_verification_refuses_same_byte_output_archive_and_directory_replacements_and_keeps_real_workers_owned() {
    for fault in ["output", "archive", "directory"] {
        let (mut s, root, _) = fixture();
        let (key, _) = begin(&mut s, &root, "save_as");
        operation::dispatch_admitted_files(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        assert_eq!(operation::observation::read(&s, &key, &|| Ok(())).unwrap()["returnedReceiptVerification"]["status"], "verified");
        let record = operation::documents(&s, &key).unwrap().iter().find(|d| d.role == retained_effects::Role::Project).unwrap();
        let publication = record.publication.as_ref().unwrap();
        let chosen = if fault == "output" {
            record.physical.clone()
        } else if fault == "archive" {
            publication.directory.join("payload.fcproj")
        } else {
            publication.directory.clone()
        };
        let held = root.join("retained-foreign-generation");
        if fault == "directory" {
            fs::rename(&chosen, &held).unwrap();
            fs::create_dir(&chosen).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&chosen, fs::Permissions::from_mode(0o700)).unwrap();
            for entry in fs::read_dir(&held).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), chosen.join(entry.file_name())).unwrap();
            }
        } else {
            let bytes = fs::read(&chosen).unwrap();
            let mode = fs::metadata(&chosen).unwrap().permissions();
            fs::rename(&chosen, &held).unwrap();
            fs::write(&chosen, bytes).unwrap();
            fs::set_permissions(&chosen, mode).unwrap();
        }
        let report = operation::observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(report["returnedReceiptVerification"]["status"], "unresolved", "{fault}: {report}");
        assert_eq!(report["allEffectsVerified"], false);
        assert_eq!(report["workersMayResume"], false);
        pending(&s, &key);
        assert!(operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        assert!(held.exists());
        s.shutdown();
    }
}
