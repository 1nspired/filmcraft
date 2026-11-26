use super::*;
use effects::{CallResult, DocumentRole, Event, FileEffects};
use sha2::Digest;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn root() -> PathBuf {
    let p = crate::temp_dir().join(format!(
        "poema-save-effects-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    fs::canonicalize(p).unwrap()
}
fn fixture(preserve: bool) -> (Session, PathBuf) {
    let d = root();
    let mut s = Session::default();
    if preserve {
        s.execute("file.newProject",json!({"name":"The actual film title","workspace":{"projectId":"p_effects","workspaceId":"12345678-1234-1234-1234-123456789abc"},"preserveDisplayTitle":true})).unwrap();
    } else {
        s.execute("file.openDemoProject", json!({})).unwrap();
    }
    s.execute("file.saveAs", json!({"path":d.join("Original.fcproj")})).unwrap();
    s.prefs_path = Some(d.join("native/preferences.json"));
    s.prefs.save(s.prefs_path.as_ref().unwrap()).unwrap();
    s.execute("file.newBin", json!({"name":"human edit remains undoable"})).unwrap();
    (s, d)
}
fn state(s: &Session) -> Value {
    let branch = |v: &[(String, Arc<filmcraft_project::Project>)]| v.iter().map(|(label, p)| json!({"label":label,"project":p.as_ref()})).collect::<Vec<_>>();
    json!({"project":s.project.as_ref(),"editor":s.state,"path":s.path,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":s.saved_file,"schema":s.loaded_schema,"prefs":s.prefs,"undo":branch(&s.history.undo),"redo":branch(&s.history.redo)})
}
#[derive(Default)]
struct Trace {
    events: Vec<Event>,
    fail_document: Option<DocumentRole>,
    fail_preferences: bool,
    late_file: Option<PathBuf>,
}
impl FileEffects for Trace {
    fn document(&mut self, services: &dyn Services, role: DocumentRole, path: &str, bytes: &[u8]) -> io::Result<()> {
        if self.fail_document == Some(role) {
            return Err(io::Error::other("generated document refusal"));
        }
        services.write_file(path, bytes)
    }
    fn preferences(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if self.fail_preferences {
            return Err(io::Error::other("generated ignored preference error"));
        }
        filmcraft_format::atomic_write(path, bytes)
    }
    fn event(&mut self, event: Event) {
        if matches!(event, Event::RecentProject { .. }) {
            if let Some(p) = &self.late_file {
                fs::write(p, b"later human file after preferences").unwrap();
            }
        }
        self.events.push(event);
    }
}
#[test]
fn shared_save_effects_preserve_original_error_order_no_adoption_and_visible_call_returns_for_backup_and_main_failures() {
    for role in [DocumentRole::SchemaBackup, DocumentRole::Project] {
        let (mut s, _) = fixture(false);
        s.loaded_schema = 1;
        let path = s.path.clone().unwrap();
        let before = state(&s);
        let old = fs::read(&path).unwrap();
        let mut trace = Trace { fail_document: Some(role), ..Default::default() };
        assert!(prepare(&s, &path, true).unwrap().publish_with(&mut s, &mut trace).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(fs::read(&path).unwrap(), old);
        assert!(trace.events.iter().any(|e| matches!(e,Event::Document{role:r,result:CallResult::ReturnedError(_),..}if *r==role)));
        assert!(!trace.events.iter().any(|e| matches!(e, Event::RecentProject { .. } | Event::Preferences { .. })));
    }
}
#[test]
fn shared_save_effects_report_ignored_preference_failure_while_retaining_original_saved_memory_proxy_revision_and_undo_semantics() {
    let (mut s, d) = fixture(false);
    let target = d.join("New identity.fcproj").to_str().unwrap().to_string();
    let previous = fs::read(s.prefs_path.as_ref().unwrap()).unwrap();
    let undo = s.history.undo.clone();
    let before_revision = s.revision;
    s.media.set_use_proxies(!s.prefs.media.enable_proxies);
    let mut trace = Trace { fail_preferences: true, ..Default::default() };
    let p = prepare(&s, &target, true).unwrap();
    let expected = p.description();
    p.publish_with(&mut s, &mut trace).unwrap();
    assert_eq!(s.path.as_deref(), Some(target.as_str()));
    assert!(!s.is_dirty());
    assert_eq!(s.saved_revision, s.revision);
    assert_eq!(s.revision, before_revision + 1);
    assert_eq!(s.prefs.general.recent_projects.first(), Some(&target));
    assert_eq!(fs::read(s.prefs_path.as_ref().unwrap()).unwrap(), previous);
    assert_eq!(saved_file::sha256(&fs::read(&target).unwrap()), expected["output"]["sha256"].as_str().unwrap());
    assert!(branch_same(&undo, &s.history.undo));
    assert!(trace.events.iter().any(|e| matches!(e, Event::Preferences { result: CallResult::ReturnedError(_), .. })));
    assert!(trace.events.iter().any(|e| matches!(e, Event::RecentProject { result: CallResult::ReturnedError(_), .. })));
}
#[test]
fn shared_save_copy_never_runs_auxiliary_effects_and_same_path_copy_only_clears_the_original_saved_baseline() {
    for same in [false, true] {
        let (mut s, d) = fixture(false);
        let path = if same { s.path.clone().unwrap() } else { d.join("Copy.fcproj").to_str().unwrap().into() };
        let before = state(&s);
        let mut trace = Trace { fail_preferences: true, ..Default::default() };
        prepare(&s, &path, false).unwrap().publish_with(&mut s, &mut trace).unwrap();
        let mut expected = before;
        if same {
            expected["savedFile"] = Value::Null;
        }
        assert_eq!(state(&s), expected);
        assert_eq!(trace.events.len(), 1);
        assert!(matches!(trace.events[0], Event::Document { role: DocumentRole::Project, result: CallResult::ReturnedOk { .. }, .. }));
    }
}
#[test]
fn shared_preference_effects_match_the_frozen_pretty_serializer_directory_policy_and_error_fallback_for_normal_and_save_callers() {
    for fail in [false, true] {
        let d = root();
        let prefs = autosave::Preferences::default();
        let a = d.join("old/preferences.json");
        let b = d.join("new/preferences.json");
        if fail {
            fs::create_dir_all(&a).unwrap();
            fs::write(a.join("keep"), b"kept old directory").unwrap();
            fs::create_dir_all(&b).unwrap();
            fs::write(b.join("keep"), b"kept new directory").unwrap();
        }
        // Frozen pre-refactor Preferences::save, independent from FileEffects.
        let original = (|| {
            if let Some(parent) = a.parent() {
                fs::create_dir_all(parent)?;
            }
            filmcraft_format::atomic_write(&a, &serde_json::to_vec_pretty(&prefs).unwrap_or_default())
        })();
        let mut trace = Trace::default();
        let actual = prefs.save_with(&b, &mut trace);
        assert_eq!(actual.is_ok(), original.is_ok());
        if !fail {
            assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        } else {
            assert_eq!(fs::read(b.join("keep")).unwrap(), b"kept new directory");
        }
        assert!(trace.events.iter().any(|e| matches!(e, Event::Preferences { result: CallResult::ReturnedError(_), .. })) == fail);
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn original_save_service_writes_all_four_variants_full_view_schema_backups_and_exact_preferences_through_retained_documents_without_new_undo() {
    use filmcraft_format::retained::{CurrentState, inspect};
    use retained_effects::{RetainedDocuments, Role};
    for preserve in [false, true] {
        for legacy in [false, true] {
            for (kind, target_kind) in [("save", 0), ("save_all", 0), ("save_as", 1), ("save_copy", 1), ("save_copy", 0), ("save_as", 2), ("save", 1)] {
                let (mut s, d) = fixture(preserve);
                if legacy {
                    s.loaded_schema = 1;
                }
                let original = s.path.clone().unwrap();
                let old = fs::read(&original).unwrap();
                let path = match target_kind {
                    0 => original.clone(),
                    1 => d.join("Chosen.fcproj").to_str().unwrap().into(),
                    _ => {
                        let p = d.join("Existing.fcproj");
                        fs::write(&p, b"earlier destination file").unwrap();
                        p.to_str().unwrap().into()
                    }
                };
                let intent = if kind == "save_all" || kind == "save" && target_kind == 0 { json!({"kind":kind}) } else { json!({"kind":kind,"path":path}) };
                let resolved = intent::Intent::parse(&intent).unwrap().resolve(&s).unwrap();
                let intent::Resolution::Destination { path, adopt, .. } = resolved else { panic!("actual test destination must resolve") };
                let undo = s.history.undo.clone();
                let p = prepare(&s, &path, adopt).unwrap();
                let predicted = p.description();
                let op = format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed));
                let mut trace = Trace::default();
                let check = || Ok(());
                let mut files = RetainedDocuments::new(&p, &op, &mut trace, &check).unwrap();
                p.publish_with(&mut s, &mut files).unwrap();
                files.verify_documents().unwrap();
                assert!(!files.has_unconfirmed_documents());
                for record in files.documents() {
                    assert!(record.qualified);
                    let pubr = record.publication.as_ref().unwrap();
                    let observed = inspect(&pubr.directory, &|| Ok(())).unwrap();
                    assert_eq!(observed.current, CurrentState::Published);
                    assert!(!observed.write_authority);
                    if record.role == Role::Preferences {
                        assert_eq!(fs::read(&record.physical).unwrap(), serde_json::to_vec_pretty(&s.prefs).unwrap());
                    }
                }
                assert_eq!(saved_file::sha256(&fs::read(&path).unwrap()), predicted["output"]["sha256"].as_str().unwrap());
                assert_eq!(s.path.as_deref(), Some(if adopt { path.as_str() } else { original.as_str() }));
                assert_eq!(s.is_dirty(), !adopt);
                assert!(branch_same(&undo, &s.history.undo));
                if legacy && adopt && path == original {
                    assert_eq!(fs::read(schema_backup_path(&path, 1)).unwrap(), old);
                    assert!(files.documents().iter().any(|r| r.role == Role::SchemaBackup));
                }
            }
        }
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn retained_save_receiver_refuses_wrong_role_path_payload_and_repeat_without_new_publication_or_arbitrary_file_forwarding() {
    use retained_effects::RetainedDocuments;
    let (s, d) = fixture(false);
    let path = s.path.clone().unwrap();
    let p = prepare(&s, &path, true).unwrap();
    let bytes = p.bytes.clone();
    let op = format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed));
    let mut trace = Trace::default();
    let check = || Ok(());
    let mut files = RetainedDocuments::new(&p, &op, &mut trace, &check).unwrap();
    assert!(files.document(&*s.services, DocumentRole::Project, d.join("Foreign.fcproj").to_str().unwrap(), &bytes).is_err());
    assert!(files.document(&*s.services, DocumentRole::Project, &path, b"foreign bytes").is_err());
    assert!(files.document(&*s.services, DocumentRole::SchemaBackup, &path, &bytes).is_err());
    assert!(files.preferences(d.join("foreign-preferences").as_path(), b"foreign").is_err());
    assert!(!d.join("Foreign.fcproj").exists());
    files.document(&*s.services, DocumentRole::Project, &path, &bytes).unwrap();
    let first = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(files.document(&*s.services, DocumentRole::Project, &path, &bytes).is_err());
    assert_eq!(files.documents().len(), 1);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), first);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let (mut s, _) = fixture(false);
    let path = s.path.clone().unwrap();
    let p = prepare(&s, &path, true).unwrap();
    fs::write(&path, b"later human saved file").unwrap();
    let mut trace = Trace::default();
    let mut files = RetainedDocuments::new(&p, &op, &mut trace, &check).unwrap();
    assert!(p.publish_with(&mut s, &mut files).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"later human saved file");
    assert!(files.documents().iter().all(|r| r.publication.is_none()));
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn retained_save_unconfirmed_file_receipt_returns_before_native_adoption_and_leaves_the_genuine_autosave_pause_owned_for_recovery() {
    use crate::autosave::AutosaveConfig;
    use retained_effects::RetainedDocuments;
    let (mut s, d) = fixture(false);
    s.start_autosave(AutosaveConfig::new(d.join("runtime"))).unwrap();
    s.sync_persistence();
    let held = s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap();
    s.apply_persistence_events();
    let preview = s.previews.clone();
    let _preview_hold = preview.pause_writers(&|| Ok(())).unwrap();
    let path = s.path.clone().unwrap();
    let before = state(&s);
    let p = prepare(&s, &path, true).unwrap();
    let bytes = p.bytes.clone();
    let op = format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed));
    let tag = format!("{:x}", sha2::Sha256::digest(format!("{op}:Project").as_bytes()));
    let receipt = d.join(format!(".poema-save-{tag}.incomplete/receipt.json"));
    let modified = AtomicBool::new(false);
    let check = || {
        if receipt.exists() && !modified.swap(true, Ordering::SeqCst) {
            fs::write(&receipt, b"foreign receipt during native postflight")?;
        }
        Ok(())
    };
    let mut trace = Trace::default();
    let mut files = RetainedDocuments::new(&p, &op, &mut trace, &check).unwrap();
    assert!(p.publish_with(&mut s, &mut files).is_err());
    assert!(modified.load(Ordering::SeqCst));
    assert!(files.has_unconfirmed_documents());
    assert_eq!(state(&s), before);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    // No scoped auto-resume after the uncertain write: stop this generated owner
    // as a crash would, retaining its original recovery journal and file records.
    drop(s.persistence.take());
    drop(held);
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn retained_save_post_auxiliary_recheck_preserves_later_human_file_and_never_turns_an_old_receipt_into_current_completion() {
    use retained_effects::RetainedDocuments;
    let (mut s, d) = fixture(false);
    let path = d.join("Adopted.fcproj").to_str().unwrap().to_string();
    let p = prepare(&s, &path, true).unwrap();
    let op = format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed));
    let mut trace = Trace { late_file: Some(path.clone().into()), ..Default::default() };
    let check = || Ok(());
    let mut files = RetainedDocuments::new(&p, &op, &mut trace, &check).unwrap();
    p.publish_with(&mut s, &mut files).unwrap();
    assert!(files.verify_documents().is_err());
    assert!(files.has_unconfirmed_documents());
    assert_eq!(fs::read(&path).unwrap(), b"later human file after preferences");
    assert!(files.documents().iter().any(|r| !r.qualified));
}
