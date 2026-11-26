use super::*;
use crate::{FsServices, autosave::AutosaveConfig};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn id() -> String {
    format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed))
}
fn fixture() -> (Session, PathBuf) {
    let d = crate::temp_dir().join(format!(
        "Save-owner-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&d).unwrap();
    let d = fs::canonicalize(d).unwrap();
    let mut s = Session::default();
    s.start_autosave(AutosaveConfig::new(d.join("runtime"))).unwrap();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.saveAs", json!({"path":d.join("Original.fcproj")})).unwrap();
    s.execute("file.newBin", json!({"name":"human owned edit"})).unwrap();
    (s, d)
}
fn intent(s: &Session, d: &Path, kind: &str) -> Intent {
    Intent::parse(&if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":d.join("Chosen.fcproj")}) })
        .unwrap_or_else(|e| panic!("{} {:?}", e, s.path))
}
fn state(s: &Session) -> Value {
    let branch = |v: &[(String, Arc<filmcraft_project::Project>)]| v.iter().map(|(l, p)| json!({"label":l,"project":p.as_ref()})).collect::<Vec<_>>();
    json!({"project":s.project.as_ref(),"editor":s.state,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":s.saved_file,"path":s.path,"prefs":s.prefs,"schema":s.loaded_schema,"undo":branch(&s.history.undo),"redo":branch(&s.history.redo)})
}
fn held(s: &Session) {
    assert!(s.save_operation_held());
    assert!(s.is_display_only());
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
}
fn released(s: &Session) {
    assert!(!s.save_operation_held());
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    drop(s.previews.pause_writers(&|| Ok(())).unwrap());
}
#[test]
fn owned_save_pre_dispatch_cancellation_preserves_all_four_original_intents_and_blocks_competing_commands_direct_edits_undo_prefs_and_preview_owners() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, d) = fixture();
        s.sync_persistence();
        s.persistence.as_ref().unwrap().flush();
        s.apply_persistence_events();
        let before = state(&s);
        let mark = s.undo_mark();
        let request = intent(&s, &d, kind);
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        held(&s);
        assert!(begin(&mut s, &id(), &request, &|| Ok(())).is_err());
        for command in ["file.newBin", "file.save", "prefs.set", "edit.undo", "edit.redo", "file.openDemoProject"] {
            assert!(s.execute(command, json!({})).is_err());
        }
        assert!(s.edit("competing", |_, _| Ok(())).is_err());
        assert!(s.edit_merged("competing", "key", |_, _| Ok(())).is_err());
        assert!(s.edit_group("competing", |_| Ok(())).is_err());
        assert!(s.undo().is_none());
        assert!(s.redo().is_none());
        assert_eq!(s.revert_to(mark), 0);
        assert!(s.set_prefs(s.prefs.clone()).is_err());
        assert!(s.start_autosave(AutosaveConfig::new(d.join("foreign-runtime"))).is_err());
        s.previews_follow_path();
        s.note_recent_project();
        s.poll_persistence();
        s.pump_jobs(Duration::ZERO);
        assert_eq!(state(&s), before);
        assert!(!d.join("foreign-runtime").exists());
        assert!(!d.join("Chosen.fcproj").exists());
        assert_eq!(status(&s, &key, &|| Ok(())).unwrap()["nativeBasisCurrent"], true);
        cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
        released(&s);
        assert_eq!(state(&s), before);
        assert!(status(&s, &key, &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn owned_save_invalid_ids_missing_destinations_Stop_display_projections_and_original_busy_jobs_never_pause_or_replay_human_work() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save");
    for invalid in ["", "short", &"x".repeat(129), &"!".repeat(32)] {
        assert!(begin(&mut s, invalid, &request, &|| Ok(())).is_err());
        released(&s);
    }
    for kind in ["save_as", "save_copy"] {
        let request = Intent::parse(&json!({"kind":kind})).unwrap();
        assert!(begin(&mut s, &id(), &request, &|| Ok(())).is_err());
        released(&s);
    }
    assert!(begin(&mut s, &id(), &request, &|| Err(error("generated Stop before pause"))).is_err());
    released(&s);
    let mut display = s.display_projection();
    assert!(begin(&mut display, &id(), &request, &|| Ok(())).is_err());
    assert!(!display.save_operation_held());
    s.jobs.push(crate::Job { id: 1, label: "actual human job".into(), progress: Arc::default(), result: Arc::default() });
    assert!(begin(&mut s, &id(), &request, &|| Ok(())).is_err());
    assert!(!s.jobs[0].progress.cancel.load(Ordering::Acquire));
    s.jobs.clear();
    released(&s);
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn owned_save_retains_exact_original_retained_document_receipts_and_both_real_worker_leases_after_all_four_service_results_without_replay_or_native_Undo() {
    use filmcraft_format::retained::{CurrentState, inspect};
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, d) = fixture();
        s.loaded_schema = 1;
        let request = intent(&s, &d, kind);
        let history = s.history.undo.clone();
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let predicted = prepared(&s, &key).unwrap().description();
        let result = dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        let path = result["path"].as_str().unwrap();
        assert_eq!(crate::saved_file::sha256(&fs::read(path).unwrap()), predicted["output"]["sha256"].as_str().unwrap());
        held(&s);
        assert!(prepared(&s, &key).is_err());
        let docs = documents(&s, &key).unwrap();
        assert!(!docs.is_empty());
        for record in docs {
            assert!(record.qualified);
            let p = record.publication.as_ref().unwrap();
            assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::Published);
        }
        assert!(super::super::branch_same(&history, &s.history.undo));
        let observed = state(&s);
        let count = documents(&s, &key).unwrap().len();
        assert!(dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).is_err());
        assert_eq!(documents(&s, &key).unwrap().len(), count);
        assert!(cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        assert_eq!(state(&s), observed);
        let v = status(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(v["phase"], "AwaitingReconciliation");
        assert_eq!(v["nativeComplete"], false);
        assert_eq!(v["nativeBasisCurrent"], true);
        s.shutdown();
    }
}
struct Failure {
    panic_after_write: bool,
    writes: usize,
    stop: Option<Arc<AtomicBool>>,
}
impl FileEffects for Failure {
    fn document(&mut self, s: &dyn Services, _: effects::DocumentRole, p: &str, b: &[u8]) -> io::Result<()> {
        if !self.panic_after_write && self.stop.is_none() {
            return Err(io::Error::other("generated file refusal"));
        }
        s.write_file(p, b)?;
        self.writes += 1;
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Release);
        }
        if self.panic_after_write {
            panic!("generated lost native return after actual write");
        }
        Ok(())
    }
}
#[test]
fn owned_save_file_errors_and_caught_panic_after_real_publication_retain_workers_native_gate_and_latest_bytes_without_repeated_service_calls() {
    for panic_after_write in [false, true] {
        let (mut s, d) = fixture();
        let request = intent(&s, &d, "save_as");
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let predicted = prepared(&s, &key).unwrap().description();
        let mut files = Failure { panic_after_write, writes: 0, stop: None };
        assert!(dispatch(&mut s, &key, &mut files, &|| Ok(())).is_err());
        held(&s);
        assert_eq!(files.writes, usize::from(panic_after_write));
        if panic_after_write {
            assert_eq!(crate::saved_file::sha256(&fs::read(d.join("Chosen.fcproj")).unwrap()), predicted["output"]["sha256"].as_str().unwrap());
        } else {
            assert!(!d.join("Chosen.fcproj").exists());
        }
        assert!(dispatch(&mut s, &key, &mut files, &|| Ok(())).is_err());
        assert_eq!(files.writes, usize::from(panic_after_write));
        assert!(cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn owned_save_Stop_before_dispatch_keeps_prepared_value_cancellable_but_late_Stop_preserves_actual_return_and_never_releases_uncertain_workers() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_as");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    assert!(dispatch(&mut s, &key, &mut effects::Original, &|| Err(error("Stop before dispatch"))).is_err());
    assert!(prepared(&s, &key).is_ok());
    held(&s);
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    released(&s);
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let mut files = Failure { panic_after_write: false, writes: 0, stop: Some(stop.clone()) };
    assert!(dispatch(&mut s, &key, &mut files, &|| if stop.load(Ordering::Acquire) { Err(error("late Stop")) } else { Ok(()) }).is_err());
    assert_eq!(files.writes, 1);
    assert!(d.join("Chosen.fcproj").is_file());
    held(&s);
    assert!(owned(&s, &key).unwrap().returned.as_ref().unwrap().is_ok());
    assert!(dispatch(&mut s, &key, &mut files, &|| Ok(())).is_err());
    assert_eq!(files.writes, 1);
    s.shutdown();
}
#[test]
fn owned_save_live_key_cannot_be_recreated_from_status_operation_string_or_another_native_session() {
    let (mut a, da) = fixture();
    let (mut b, db) = fixture();
    let operation = id();
    let ia = intent(&a, &da, "save");
    let ib = intent(&b, &db, "save");
    let ka = begin(&mut a, &operation, &ia, &|| Ok(())).unwrap();
    let kb = begin(&mut b, &operation, &ib, &|| Ok(())).unwrap();
    assert!(status(&a, &kb, &|| Ok(())).is_err());
    assert!(cancel_before_dispatch(&mut a, &kb, &|| Ok(())).is_err());
    assert!(dispatch(&mut a, &kb, &mut effects::Original, &|| Ok(())).is_err());
    held(&a);
    held(&b);
    assert_eq!(status(&a, &ka, &|| Ok(())).unwrap()["writeAuthority"], false);
    cancel_before_dispatch(&mut a, &ka, &|| Ok(())).unwrap();
    cancel_before_dispatch(&mut b, &kb, &|| Ok(())).unwrap();
    a.shutdown();
    b.shutdown();
}
#[test]
fn owned_save_later_native_context_or_real_worker_replacement_refuses_dispatch_without_reverting_the_later_state_or_releasing_original_preview_owner() {
    for field in ["path", "preferences", "previews", "persistence"] {
        let (mut s, d) = fixture();
        let request = intent(&s, &d, "save_as");
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let original_store = s.previews.clone();
        let mut old_persistence = None;
        match field {
            "path" => s.path = Some(d.join("Later human.fcproj").to_str().unwrap().into()),
            "preferences" => s.prefs.general.recent_projects.clear(),
            "previews" => s.previews = Arc::new(previews::PreviewStore::temp()),
            _ => {
                old_persistence = s.persistence.take();
            }
        }
        let changed = state(&s);
        assert!(dispatch(&mut s, &key, &mut effects::Original, &|| Ok(())).is_err());
        assert_eq!(state(&s), changed);
        assert!(!d.join("Chosen.fcproj").exists());
        assert!(cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        assert!(original_store.pause_writers(&|| Ok(())).is_err());
        drop(old_persistence);
        s.shutdown();
    }
}
#[test]
fn owned_save_cancel_Stop_before_and_after_resume_keeps_one_original_acknowledgement_and_native_gate_until_same_owner_observation_succeeds() {
    for fail_at in [3, 4] {
        let (mut s, d) = fixture();
        let request = intent(&s, &d, "save_as");
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let calls = AtomicUsize::new(0);
        assert!(
            cancel_before_dispatch(&mut s, &key, &|| if calls.fetch_add(1, Ordering::SeqCst) + 1 == fail_at {
                Err(error("generated interrupted cancellation observation"))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(s.save_operation_held());
        assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        assert!(!d.join("Chosen.fcproj").exists());
        cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
        released(&s);
        s.shutdown();
    }
}
#[test]
fn owned_save_unresolved_shutdown_and_whole_session_drop_stop_original_worker_without_flush_cleanup_or_a_competing_Save() {
    for explicit in [false, true] {
        let (mut s, d) = fixture();
        s.sync_persistence();
        s.persistence.as_ref().unwrap().flush();
        let recovery = s.persistence.as_ref().unwrap().session_dir.clone();
        let request = intent(&s, &d, "save_as");
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        dispatch(&mut s, &key, &mut effects::Original, &|| Ok(())).unwrap();
        let output = fs::read(d.join("Chosen.fcproj")).unwrap();
        let started = Instant::now();
        if explicit {
            s.shutdown();
            assert!(s.save_operation_held());
            assert!(s.start_autosave(AutosaveConfig::new(d.join("replacement-runtime"))).is_err());
        }
        drop(s);
        assert!(started.elapsed() < Duration::from_secs(2), "shutdown must bypass the original 30-second Flush wait while held");
        assert!(recovery.is_dir());
        assert!(!d.join("replacement-runtime").exists());
        assert_eq!(fs::read(d.join("Chosen.fcproj")).unwrap(), output);
    }
}
#[test]
fn owned_save_effect_record_budget_refuses_new_physical_calls_and_explicitly_retains_incomplete_observation_instead_of_claiming_complete_receipts() {
    let d = crate::temp_dir().join(format!("Save-record-limit-{}", id()));
    fs::create_dir_all(&d).unwrap();
    let mut original = effects::Original;
    let mut recording = Recording { delegate: &mut original, events: Vec::new(), complete: true, planning_origin: None };
    for _ in 0..MAX_EVENTS {
        recording.event(Event::RecentProject { path: None, result: effects::CallResult::ReturnedOk { bytes: None } });
    }
    let p = d.join("must-not-write.fcproj");
    assert!(recording.document(&FsServices, effects::DocumentRole::Project, p.to_str().unwrap(), b"foreign").is_err());
    assert!(recording.preview_exists(&p));
    assert!(!p.exists());
    assert!(!recording.complete);
    recording.event(Event::RecentProject { path: None, result: effects::CallResult::ReturnedError("budget refused".into()) });
    assert_eq!(recording.events.len(), MAX_EVENTS);
}

#[test]
fn original_effect_read_before_dispatch_preserves_plan_and_workers_and_refuses_foreign_keys_or_display_copies() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    let before = state(&s);
    let original = fs::read(d.join("Original.fcproj")).unwrap();
    let v = observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(v["planned"], prepared(&s, &key).unwrap().description());
    assert_eq!(v["events"], json!([]));
    assert_eq!(v["documents"], json!([]));
    assert_eq!(v["body"], Value::Null);
    assert_eq!(v["postflight"], Value::Null);
    assert_eq!(v["effectBoundaryCrossed"], false);
    assert_eq!(v["nativeBeforeCurrent"], true);
    assert_eq!(v["nativeAfterAvailable"], false);
    assert!(observation::read(&s, &Key(Arc::new(())), &|| Ok(())).is_err());
    assert!(observation::read(&s.display_projection(), &key, &|| Ok(())).is_err());
    assert_eq!(state(&s), before);
    assert_eq!(fs::read(d.join("Original.fcproj")).unwrap(), original);
    held(&s);
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn original_effect_read_retains_all_four_real_document_publications_and_body_returns_without_native_completion() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, d) = fixture();
        s.loaded_schema = 1;
        let request = intent(&s, &d, kind);
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let planned = prepared(&s, &key).unwrap().description();
        dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        let after = state(&s);
        let v = observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(v["planned"], planned);
        assert_eq!(v["phase"], "AwaitingReconciliation");
        assert_eq!(v["bodyReturned"], true);
        assert_eq!(v["body"]["status"], "ok");
        assert_eq!(v["postflight"]["status"], "ok");
        assert_eq!(v["effectBoundaryCrossed"], true);
        assert_eq!(v["nativeAfterAvailable"], true);
        assert_eq!(v["nativeAfterCurrent"], true);
        assert_eq!(v["effectCallReturnsComplete"], true);
        assert!(!v["events"].as_array().unwrap().is_empty());
        for row in v["documents"].as_array().unwrap() {
            assert_eq!(row["qualifiedAtReturn"], true);
            assert_eq!(row["publication"]["inspection"]["observation"]["current"], "published");
            assert_eq!(row["publication"]["receiptDurable"], true);
        }
        for flag in ["current", "allEffectsVerified", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(v[flag], false);
        }
        assert_eq!(state(&s), after);
        assert_eq!(observation::read(&s, &key, &|| Ok(())).unwrap(), v);
        held(&s);
        assert!(cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn original_effect_read_distinguishes_returned_file_errors_from_caught_panics_after_actual_write() {
    for panic_after_write in [false, true] {
        let (mut s, d) = fixture();
        let request = intent(&s, &d, "save_as");
        let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
        let mut files = Failure { panic_after_write, writes: 0, stop: None };
        assert!(dispatch(&mut s, &key, &mut files, &|| Ok(())).is_err());
        let v = observation::read(&s, &key, &|| Ok(())).unwrap();
        assert_eq!(v["bodyReturned"], !panic_after_write);
        assert_eq!(v["body"]["status"], "error");
        assert_eq!(v["postflight"]["status"], "ok");
        assert_eq!(v["nativeAfterAvailable"], true);
        assert_eq!(v["effectBoundaryCrossed"], true);
        assert_eq!(v["documents"], json!([]));
        assert_eq!(files.writes, usize::from(panic_after_write));
        held(&s);
        assert!(dispatch(&mut s, &key, &mut files, &|| Ok(())).is_err());
        assert_eq!(files.writes, usize::from(panic_after_write));
        s.shutdown();
    }
}
#[test]
fn original_effect_read_keeps_successful_native_body_distinct_from_stopped_postflight_and_never_resumes() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_as");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let mut files = Failure { panic_after_write: false, writes: 0, stop: Some(stop.clone()) };
    let check = || if stop.load(Ordering::Acquire) { Err(error("generated stopped postflight")) } else { Ok(()) };
    assert!(dispatch(&mut s, &key, &mut files, &check).is_err());
    let v = observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(v["bodyReturned"], true);
    assert_eq!(v["body"]["status"], "ok");
    assert_eq!(v["postflight"]["status"], "error");
    assert!(stop.load(Ordering::Acquire));
    assert_eq!(v["workersMayResume"], false);
    assert!(d.join("Chosen.fcproj").is_file());
    held(&s);
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn original_effect_read_reports_later_document_changes_without_repairing_bytes_or_promoting_old_qualification() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    let target = d.join("Chosen.fcproj");
    fs::write(&target, b"later human project bytes").unwrap();
    let v = observation::read(&s, &key, &|| Ok(())).unwrap();
    let row = v["documents"].as_array().unwrap().iter().find(|r| r["role"] == "Project").unwrap();
    assert_eq!(row["qualifiedAtReturn"], true);
    assert_ne!(row["publication"]["inspection"]["observation"]["current"], "published");
    assert_eq!(v["allEffectsVerified"], false);
    assert_eq!(fs::read(target).unwrap(), b"later human project bytes");
    held(&s);
    s.shutdown();
}
#[test]
fn stopped_original_effect_read_preserves_original_owner_and_full_native_records_for_a_fresh_observation() {
    use std::cell::Cell;
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    let expected = observation::read(&s, &key, &|| Ok(())).unwrap();
    let calls = Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        if calls.get() > 4 { Err(error("generated effect read Stop")) } else { Ok(()) }
    };
    assert!(observation::read(&s, &key, &check).is_err());
    assert_eq!(calls.get(), 5);
    assert_eq!(observation::read(&s, &key, &|| Ok(())).unwrap(), expected);
    held(&s);
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn complete_original_effect_read_accepts_exact_full_bound_and_refuses_next_byte_without_truncating_owned_events() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        o.events.push(Event::BoundaryRefused { phase: "generated complete effect", path: d.clone(), error: String::new() });
    }
    let initial = observation::read(&s, &key, &|| Ok(())).unwrap();
    let size = serde_json::to_vec(&initial).unwrap().len();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        if let Event::BoundaryRefused { error, .. } = o.events.last_mut().unwrap() {
            *error = "x".repeat(observation::MAX_BYTES - size);
        }
    }
    let full = observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(serde_json::to_vec(&full).unwrap().len(), observation::MAX_BYTES);
    if let Slot::Owned(o) = &mut s.project_save_operation {
        if let Event::BoundaryRefused { error, .. } = o.events.last_mut().unwrap() {
            error.push('x');
        }
    }
    assert!(observation::read(&s, &key, &|| Ok(())).is_err());
    if let Slot::Owned(o) = &s.project_save_operation {
        assert_eq!(o.events.len(), 1);
    }
    held(&s);
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn original_effect_read_keeps_native_unknown_after_state_explicit_instead_of_reconstructing_it_from_returns() {
    let (mut s, d) = fixture();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    dispatch(&mut s, &key, &mut effects::Original, &|| Ok(())).unwrap();
    if let Slot::Owned(o) = &mut s.project_save_operation {
        o.after = None;
    }
    let v = observation::read(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(v["body"]["status"], "ok");
    assert_eq!(v["nativeAfterAvailable"], false);
    assert_eq!(v["nativeAfterCurrent"], false);
    assert_eq!(v["nativeComplete"], false);
    held(&s);
    assert!(recovery_owner(&s, &key, &|| Ok(())).is_err());
    s.shutdown();
}

// Core ownership tests only. The separate cross-language fixture must still
// prove genuine private input and original opaque native construction.
struct ColdTestOrigin {
    available: Arc<AtomicBool>,
}
impl ColdSaveOrigin for ColdTestOrigin {
    fn current(&self) -> Result<()> {
        if self.available.load(Ordering::Acquire) { Ok(()) } else { Err(EngineError::Other("generated cold origin unavailable".into())) }
    }
    fn description(&self) -> Result<Value> {
        self.current()?;
        Ok(json!({"kind":"native_cold_Save_candidate","writeAuthority":false,"workersMayResume":false}))
    }
}
fn cold_test_origin() -> (Arc<ColdTestOrigin>, Arc<AtomicBool>) {
    let available = Arc::new(AtomicBool::new(true));
    (Arc::new(ColdTestOrigin { available: available.clone() }), available)
}
#[test]
fn cold_save_hold_retains_distinct_preview_ownership_and_refuses_original_keys_commands_edits_and_worker_startup() {
    let (mut original, d) = fixture();
    let request = intent(&original, &d, "save_copy");
    let key = begin(&mut original, &id(), &request, &|| Ok(())).unwrap();
    let mut cold = Session::default();
    crate::agent_bridge::test_native_epoch(&mut cold, false).unwrap();
    let (origin, _) = cold_test_origin();
    let graph = cold.project.clone();
    let before = state(&cold);
    hold_cold_recovery(&mut cold, origin).unwrap();
    assert!(cold.save_operation_held());
    assert!(cold.is_display_only());
    assert!(cold.previews.pause_writers(&|| Ok(())).is_err());
    assert_eq!(cold_recovery_status(&cold).unwrap().unwrap()["nativeSession"], crate::agent_bridge::retained_native_epoch(&cold).unwrap());
    assert!(prepared(&cold, &key).is_err());
    assert!(cancel_before_dispatch(&mut cold, &key, &|| Ok(())).is_err());
    for command in ["file.save", "file.newBin", "prefs.set", "edit.undo", "edit.redo", "file.openDemoProject"] {
        assert!(cold.execute(command, json!({})).is_err(), "{command}");
    }
    assert!(cold.edit("blocked cold edit", |_, _| Ok(())).is_err());
    assert!(cold.undo().is_none());
    assert!(cold.redo().is_none());
    assert!(cold.set_prefs(cold.prefs.clone()).is_err());
    assert!(cold.start_autosave(AutosaveConfig::new(d.join("unadmitted-cold-worker"))).is_err());
    assert!(!d.join("unadmitted-cold-worker").exists());
    assert!(Arc::ptr_eq(&graph, &cold.project));
    assert_eq!(state(&cold), before);
    assert!(cold.persistence.is_none());
    cold.shutdown();
    assert!(cold.save_operation_held());
    cancel_before_dispatch(&mut original, &key, &|| Ok(())).unwrap();
    original.shutdown();
}
#[test]
fn cold_save_status_refuses_lost_origin_changed_epoch_and_replaced_native_graph_without_releasing_the_hold() {
    for fault in ["origin", "epoch", "graph"] {
        let mut s = Session::default();
        crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
        let (origin, available) = cold_test_origin();
        hold_cold_recovery(&mut s, origin).unwrap();
        match fault {
            "origin" => available.store(false, Ordering::Release),
            "epoch" => {
                crate::agent_bridge::test_native_epoch(&mut s, true).unwrap();
            }
            "graph" => s.project = Arc::new(s.project.as_ref().clone()),
            _ => panic!("known generated fault"),
        }
        assert!(cold_recovery_status(&s).is_err(), "{fault}");
        assert!(s.save_operation_held());
        assert!(s.execute("file.save", json!({})).is_err());
        s.shutdown();
        assert!(s.persistence.is_none());
    }
}
#[test]
fn cold_save_preflight_refuses_live_persistence_missing_epoch_and_lost_origin_without_leaking_a_pause() {
    let (mut live, _) = fixture();
    let (origin, _) = cold_test_origin();
    assert!(hold_cold_recovery(&mut live, origin).is_err());
    assert!(!live.save_operation_held());
    drop(live.previews.pause_writers(&|| Ok(())).unwrap());
    live.shutdown();
    for fault in ["epoch", "origin"] {
        let mut s = Session::default();
        let (origin, available) = cold_test_origin();
        if fault == "origin" {
            crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
            available.store(false, Ordering::Release);
        }
        let graph = s.project.clone();
        assert!(hold_cold_recovery(&mut s, origin).is_err());
        assert!(!s.save_operation_held());
        assert!(Arc::ptr_eq(&graph, &s.project));
        drop(s.previews.pause_writers(&|| Ok(())).unwrap());
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_save_direct_synchronization_keeps_clean_and_dirty_startup_fifo_unqueued() {
    use std::fs;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for dirty in [false, true] {
        let mut s = Session::default();
        if dirty {
            s.execute("file.newBin", json!({"name":"Original unsaved bin"})).unwrap();
        }
        assert_eq!(s.is_dirty(), dirty);
        crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
        let (origin, _) = cold_test_origin();
        hold_cold_recovery(&mut s, origin).unwrap();
        let graph = s.project.clone();
        let before = state(&s);
        let d = crate::temp_dir().join(format!("poema-cold-sync-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&d).unwrap();
        // Core gate coverage with an actual gated worker. The generated
        // cross-language probe separately exercises its genuine Input origin.
        s.persistence = Some(crate::autosave::Persistence::start_current_image(&AutosaveConfig::new(d), Default::default(), &|| Ok(())).unwrap());
        let path = s.persistence.as_ref().unwrap().session_dir.clone();
        assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
        s.sync_persistence();
        s.poll_persistence();
        s.sync_persistence();
        let p = s.persistence.as_ref().unwrap();
        assert!(p.last_sent.is_none());
        assert!(p.startup_pending());
        p.current_image_journal(&|| Ok(())).unwrap();
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(fs::read(path.join("lock")).unwrap(), b"");
        assert!(Arc::ptr_eq(&s.project, &graph));
        assert_eq!(state(&s), before);
        assert!(s.is_display_only());
        s.shutdown();
        assert!(path.join("lock").is_file());
        assert_eq!(fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().file_name()).collect::<Vec<_>>(), [std::ffi::OsString::from("lock")]);
    }
}

struct CanonicalFailureRuntime {
    panic_prepare: bool,
}
impl ColdRuntimePreparation for CanonicalFailureRuntime {
    fn current(&self) -> Result<()> {
        Ok(())
    }
    fn description(&self) -> Result<Value> {
        Ok(json!({"generatedNegativeRuntime":true}))
    }
    fn confirm(&mut self, _: &Value) -> Result<()> {
        Err(error("generated unsupported runtime confirmation"))
    }
    fn prepare_canonical(&mut self, _: &Session) -> Result<Value> {
        if self.panic_prepare {
            panic!("generated canonical callback failure");
        }
        Err(error("generated unsupported canonical preparation"))
    }
    fn bind_services(&mut self, s: &mut Session) -> Result<Value> {
        assert!(matches!(s.project_save_operation, Slot::ColdBusy));
        assert!(s.save_operation_held());
        s.sync_persistence();
        s.apply_media_cache();
        if self.panic_prepare {
            panic!("generated service binding callback failure");
        }
        Err(error("generated unsupported service binding"))
    }
    fn confirm_service_bindings(&mut self, s: &Session, _: &Value) -> Result<Value> {
        assert!(matches!(s.project_save_operation, Slot::ColdBusy));
        assert!(s.save_operation_held());
        if self.panic_prepare {
            panic!("generated binding observation callback failure");
        }
        Err(error("generated unsupported binding observation"))
    }
}
fn canonical_failure_candidate(panic_prepare: bool) -> Session {
    let mut cold = Session::default();
    crate::agent_bridge::test_native_epoch(&mut cold, false).unwrap();
    let (origin, _) = cold_test_origin();
    hold_cold_recovery(&mut cold, origin).unwrap();
    let Slot::Cold(held) = &mut cold.project_save_operation else {
        panic!("generated cold hold required");
    };
    held.runtime = Some(Box::new(CanonicalFailureRuntime { panic_prepare }));
    cold
}
#[test]
fn cold_canonical_trusted_lifecycle_errors_restore_the_same_actual_hold_without_busy_or_edit_release() {
    let mut cold = canonical_failure_candidate(false);
    let graph = cold.project.clone();
    let before = state(&cold);
    assert!(cold.prepare_cold_save_canonical().is_err());
    assert!(cold.confirm_cold_save_canonical_intent(&json!({})).is_err());
    assert!(cold.publish_cold_save_canonical().is_err());
    assert!(cold.confirm_cold_save_canonical_observation(&json!({})).is_err());
    assert!(matches!(cold.project_save_operation, Slot::Cold(_)));
    assert!(cold.is_display_only());
    assert!(cold.persistence.is_none());
    assert!(cold.cold_save_recovery_status().unwrap().is_some());
    assert!(Arc::ptr_eq(&graph, &cold.project));
    assert_eq!(state(&cold), before);
    assert!(cold.execute("file.newBin", json!({})).is_err());
    assert!(cold.previews.pause_writers(&|| Ok(())).is_err());
}
#[test]
fn cold_canonical_trusted_callback_unwind_restores_the_actual_owner_and_keeps_editing_held() {
    let mut cold = canonical_failure_candidate(true);
    let graph = cold.project.clone();
    let before = state(&cold);
    assert!(cold.prepare_cold_save_canonical().is_err());
    assert!(matches!(cold.project_save_operation, Slot::Cold(_)));
    assert!(cold.is_display_only());
    assert!(cold.cold_save_recovery_status().unwrap().is_some());
    assert!(Arc::ptr_eq(&graph, &cold.project));
    assert_eq!(state(&cold), before);
    assert!(cold.execute("file.save", json!({})).is_err());
}

#[test]
fn cold_save_direct_cache_housekeeping_preserves_preview_binding_and_physical_empty_directories() {
    let root = crate::temp_dir().join(format!("cold-cache-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let empty = root.join("must-remain-empty");
    fs::create_dir_all(&empty).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut cold = Session::default();
    cold.prefs.media_cache.location = root.to_string_lossy().into_owned();
    cold.prefs.media_cache.management = "never".into();
    crate::agent_bridge::test_native_epoch(&mut cold, false).unwrap();
    let (origin, _) = cold_test_origin();
    let graph = cold.project.clone();
    let before = state(&cold);
    let temp_root = cold.previews.temp_root();
    let preview_dir = cold.previews.dir();
    hold_cold_recovery(&mut cold, origin).unwrap();
    cold.apply_media_cache();
    assert!(empty.is_dir());
    assert_eq!(cold.previews.temp_root(), temp_root);
    assert_eq!(cold.previews.dir(), preview_dir);
    assert_eq!(state(&cold), before);
    assert!(Arc::ptr_eq(&graph, &cold.project));
    assert!(cold.cold_save_recovery_status().unwrap().is_some());
    assert!(cold.save_operation_held());
    assert!(cold.is_display_only());
    assert!(cold.persistence.is_none());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    cold.shutdown();
}
#[test]
fn ordinary_cache_housekeeping_still_binds_previews_and_prunes_unused_empty_directories() {
    let root = crate::temp_dir().join(format!("ordinary-cache-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let empty = root.join("unused-empty");
    fs::create_dir_all(&empty).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    s.prefs.media_cache.location = root.to_string_lossy().into_owned();
    s.prefs.media_cache.management = "never".into();
    assert!(!s.save_operation_held());
    s.apply_media_cache();
    assert_eq!(s.previews.temp_root(), Some(crate::settings::previews_root(&root)));
    assert!(!empty.exists());
    assert!(!s.save_operation_held());
    s.shutdown();
}

#[test]
fn owned_save_direct_cache_housekeeping_preserves_original_key_and_preview_and_directory_basis() {
    let (mut s, d) = fixture();
    let cache = d.join("cache-during-owned-save");
    let empty = cache.join("must-remain-empty");
    fs::create_dir_all(&empty).unwrap();
    s.prefs.media_cache.location = cache.to_string_lossy().into_owned();
    s.prefs.media_cache.management = "never".into();
    let before = state(&s);
    let temp_root = s.previews.temp_root();
    let preview_dir = s.previews.dir();
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    held(&s);
    s.apply_media_cache();
    assert!(empty.is_dir());
    assert_eq!(s.previews.temp_root(), temp_root);
    assert_eq!(s.previews.dir(), preview_dir);
    assert_eq!(state(&s), before);
    assert_eq!(status(&s, &key, &|| Ok(())).unwrap()["nativeBasisCurrent"], true);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    released(&s);
    assert_eq!(state(&s), before);
    s.shutdown();
}

#[test]
fn cold_service_binding_error_and_unwind_restore_the_original_cold_owner_and_preview_hold() {
    for panic_callback in [false, true] {
        let mut cold = canonical_failure_candidate(panic_callback);
        let graph = cold.project.clone();
        let before = state(&cold);
        let Slot::Cold(held) = &cold.project_save_operation else { panic!("actual generated cold hold required") };
        let origin = held.origin.clone();
        assert!(cold.bind_cold_save_services().is_err());
        let Slot::Cold(held) = &cold.project_save_operation else { panic!("original cold hold must survive") };
        assert!(Arc::ptr_eq(&origin, &held.origin));
        assert!(Arc::ptr_eq(&graph, &cold.project));
        assert_eq!(state(&cold), before);
        assert!(cold.is_display_only());
        assert!(cold.save_operation_held());
        assert!(cold.cold_save_recovery_status().unwrap().is_some());
        assert!(cold.previews.pause_writers(&|| Ok(())).is_err());
        assert!(cold.execute("file.newBin", json!({"name":"must remain refused"})).is_err());
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn cold_service_binding_busy_scope_keeps_the_actual_clean_and_dirty_worker_fifo_unqueued() {
    for dirty in [false, true] {
        let mut s = Session::default();
        if dirty {
            s.execute("file.newBin", json!({"name":"Original unsaved bin"})).unwrap();
        }
        crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
        let (origin, _) = cold_test_origin();
        hold_cold_recovery(&mut s, origin).unwrap();
        let graph = s.project.clone();
        let before = state(&s);
        let root = crate::temp_dir().join(format!("poema-cold-binding-busy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        s.persistence = Some(crate::autosave::Persistence::start_current_image(&AutosaveConfig::new(root), Default::default(), &|| Ok(())).unwrap());
        let path = s.persistence.as_ref().unwrap().session_dir.clone();
        let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
        let original = std::mem::replace(&mut s.project_save_operation, Slot::ColdBusy);
        assert!(matches!(original, Slot::Cold(_)));
        s.sync_persistence();
        s.poll_persistence();
        s.sync_persistence();
        let worker = s.persistence.as_ref().unwrap();
        assert!(worker.last_sent.is_none());
        assert!(worker.startup_pending());
        assert!(Arc::ptr_eq(&journal, &worker.current_image_journal(&|| Ok(())).unwrap()));
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(fs::read(path.join("lock")).unwrap(), b"");
        assert!(Arc::ptr_eq(&graph, &s.project));
        assert_eq!(state(&s), before);
        assert!(s.is_display_only());
        s.project_save_operation = original;
        assert!(matches!(s.project_save_operation, Slot::Cold(_)));
        s.shutdown();
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    }
}

#[test]
fn cold_binding_observation_callback_failure_retains_original_hold_and_cannot_release_previews_or_native_edits() {
    for panic_callback in [false, true] {
        let mut s = canonical_failure_candidate(panic_callback);
        let graph = s.project.clone();
        let before = state(&s);
        let Slot::Cold(held) = &s.project_save_operation else { panic!("generated original cold owner required") };
        let origin = held.origin.clone();
        assert!(s.confirm_cold_save_service_bindings(&json!({"copiedReceipt":true})).is_err());
        let Slot::Cold(held) = &s.project_save_operation else { panic!("same cold owner must be retained") };
        assert!(Arc::ptr_eq(&origin, &held.origin));
        assert!(Arc::ptr_eq(&graph, &s.project));
        assert_eq!(state(&s), before);
        assert!(s.save_operation_held());
        assert!(s.is_display_only());
        assert!(s.cold_save_recovery_status().unwrap().is_some());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        assert!(s.execute("file.save", json!({})).is_err());
    }
}

// Definitively consumed continuations need no fresh source read to refuse a
// replay. Unused attempts must still take the complete original source fence.
struct RepeatedColdRuntime {
    source_reads: Arc<AtomicUsize>,
    writer_calls: Arc<AtomicUsize>,
    canonical_repeated: bool,
    panic_replay: bool,
}
impl ColdRuntimePreparation for RepeatedColdRuntime {
    fn current(&self) -> Result<()> {
        self.source_reads.fetch_add(1, Ordering::SeqCst);
        Err(error("generated source is unavailable"))
    }
    fn description(&self) -> Result<Value> {
        Err(error("unused generated description"))
    }
    fn confirm(&mut self, _: &Value) -> Result<()> {
        Err(error("unused generated receipt"))
    }
    fn reject_repeated_canonical_action(&self, _: CanonicalAction<'_>) -> Result<()> {
        if self.panic_replay {
            panic!("generated negative precheck failure")
        }
        if self.canonical_repeated { Err(error("generated canonical attempt already consumed; no replay")) } else { Ok(()) }
    }
    fn prepare_canonical(&mut self, _: &Session) -> Result<Value> {
        self.writer_calls.fetch_add(1, Ordering::SeqCst);
        Err(error("unexpected unguarded canonical writer"))
    }
    fn publish_canonical(&mut self, _: &Session) -> Result<Value> {
        self.writer_calls.fetch_add(1, Ordering::SeqCst);
        Err(error("unexpected unguarded canonical writer"))
    }
    fn bind_services(&mut self, _: &mut Session) -> Result<Value> {
        self.writer_calls.fetch_add(1, Ordering::SeqCst);
        Err(error("unexpected unguarded canonical writer"))
    }
}
fn repeated_cold_candidate(canonical_repeated: bool) -> (Session, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let mut s = Session::default();
    crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
    let (origin, _) = cold_test_origin();
    hold_cold_recovery(&mut s, origin).unwrap();
    let source_reads = Arc::new(AtomicUsize::new(0));
    let writer_calls = Arc::new(AtomicUsize::new(0));
    let Slot::Cold(held) = &mut s.project_save_operation else { panic!("actual generated cold owner required") };
    held.runtime =
        Some(Box::new(RepeatedColdRuntime { source_reads: source_reads.clone(), writer_calls: writer_calls.clone(), canonical_repeated, panic_replay: false }));
    (s, source_reads, writer_calls)
}
#[test]
fn cold_runtime_replay_refuses_before_rereading_an_already_consumed_source() {
    let (mut s, reads, writes) = repeated_cold_candidate(false);
    let graph = s.project.clone();
    let before = state(&s);
    let result = s.prepare_cold_save_runtime().unwrap_err().to_string();
    assert!(result.contains("already prepared"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert_eq!(state(&s), before);
    assert!(s.is_display_only() && s.save_operation_held());
    assert!(s.cold_save_recovery_status().is_err());
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}
#[test]
fn cold_worker_recreation_refuses_without_rereading_or_releasing_the_existing_gated_worker() {
    let (mut s, reads, writes) = repeated_cold_candidate(false);
    let root = crate::temp_dir().join(format!("cold-repeat-worker-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    s.persistence = Some(crate::autosave::Persistence::start_current_image(&AutosaveConfig::new(root), Default::default(), &|| Ok(())).unwrap());
    let original_directory = s.persistence.as_ref().unwrap().session_dir.clone();
    let result = s.create_cold_save_persistence().unwrap_err().to_string();
    assert!(result.contains("no inherited worker"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let p = s.persistence.as_ref().unwrap();
    assert_eq!(p.session_dir, original_directory);
    assert!(p.startup_pending() && p.last_sent.is_none());
    assert!(s.is_display_only() && s.save_operation_held());
    s.shutdown();
    assert!(s.save_operation_held());
    assert!(original_directory.join("lock").is_file());
}
#[test]
fn cold_canonical_replay_is_read_free_but_unused_effects_keep_the_complete_original_source_guard() {
    for repeated in [false, true] {
        for action in [CanonicalAction::Prepare, CanonicalAction::Publish, CanonicalAction::BindServices] {
            let (mut s, reads, writes) = repeated_cold_candidate(repeated);
            let graph = s.project.clone();
            let before = state(&s);
            let result = cold_canonical_action(&mut s, action).unwrap_err().to_string();
            assert!(result.contains(if repeated { "already consumed" } else { "source is unavailable" }));
            assert_eq!(reads.load(Ordering::SeqCst), usize::from(!repeated));
            assert_eq!(writes.load(Ordering::SeqCst), 0);
            assert!(Arc::ptr_eq(&graph, &s.project));
            assert_eq!(state(&s), before);
            assert!(s.is_display_only() && s.save_operation_held());
            assert!(s.persistence.is_none());
        }
    }
}

#[test]
fn cold_replay_precheck_unwind_keeps_the_original_owner_without_source_reads_or_effects() {
    let (mut s, reads, writes) = repeated_cold_candidate(false);
    let Slot::Cold(held) = &mut s.project_save_operation else { panic!("actual generated cold owner required") };
    held.runtime =
        Some(Box::new(RepeatedColdRuntime { source_reads: reads.clone(), writer_calls: writes.clone(), canonical_repeated: true, panic_replay: true }));
    let graph = s.project.clone();
    let before = state(&s);
    let result = s.bind_cold_save_services().unwrap_err().to_string();
    assert!(result.contains("precheck failed unexpectedly"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(matches!(s.project_save_operation, Slot::Cold(_)));
    assert!(s.is_display_only() && s.save_operation_held());
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert_eq!(state(&s), before);
}

#[test]
fn desktop_startup_transfer_refuses_actual_save_and_busy_owners_until_original_cancellation() {
    let (mut s, d) = fixture();
    assert!(s.ensure_desktop_startup_transfer().is_ok());
    assert!(s.display_projection().ensure_desktop_startup_transfer().is_err());
    let before = state(&s);
    let graph = Arc::clone(&s.project);
    let request = intent(&s, &d, "save_copy");
    let key = begin(&mut s, &id(), &request, &|| Ok(())).unwrap();
    assert!(s.ensure_desktop_startup_transfer().is_err());
    assert!(matches!(s.project_save_operation, Slot::Owned(_)));
    assert_eq!(status(&s, &key, &|| Ok(())).unwrap()["nativeBasisCurrent"], true);
    let owned = std::mem::replace(&mut s.project_save_operation, Slot::Busy);
    assert!(s.ensure_desktop_startup_transfer().is_err());
    assert!(matches!(s.project_save_operation, Slot::Busy));
    s.project_save_operation = owned;
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&graph, &s.project));
    cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    assert!(s.ensure_desktop_startup_transfer().is_ok());
    assert_eq!(state(&s), before);
    s.shutdown();
}

#[test]
fn desktop_startup_transfer_refuses_original_cold_and_cold_busy_owners_even_with_lost_origin() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Retained original cold graph"})).unwrap();
    crate::agent_bridge::test_native_epoch(&mut s, false).unwrap();
    let (origin, available) = cold_test_origin();
    hold_cold_recovery(&mut s, origin).unwrap();
    let before = state(&s);
    let graph = Arc::clone(&s.project);
    assert!(s.ensure_desktop_startup_transfer().is_err());
    available.store(false, Ordering::Release);
    assert!(s.ensure_desktop_startup_transfer().is_err());
    let cold = std::mem::replace(&mut s.project_save_operation, Slot::ColdBusy);
    assert!(s.ensure_desktop_startup_transfer().is_err());
    assert!(matches!(s.project_save_operation, Slot::ColdBusy));
    s.project_save_operation = cold;
    assert!(matches!(s.project_save_operation, Slot::Cold(_)));
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert!(s.persistence.is_none());
}

// These synthetic runtimes test the ORIGINAL Cold action boundary only. They
// cannot certify a real worker receipt or the whole application controller.
#[derive(Clone, Copy)]
enum CurrentImageRouteOutcome {
    Complete,
    Refuse,
    Panic,
}
struct CurrentImageRouteRuntime {
    completes: Arc<AtomicUsize>,
    inspections: Arc<AtomicUsize>,
    held_reads: Arc<AtomicUsize>,
    completed: Arc<AtomicBool>,
    outcome: CurrentImageRouteOutcome,
    inspect_panics: bool,
}
impl ColdRuntimePreparation for CurrentImageRouteRuntime {
    fn current(&self) -> Result<()> {
        self.held_reads.fetch_add(1, Ordering::SeqCst);
        if self.completed.load(Ordering::SeqCst) { Err(error("generated old held reader refuses after Resume")) } else { Ok(()) }
    }
    fn description(&self) -> Result<Value> {
        self.current()?;
        Ok(json!({"syntheticRouteFixture":true}))
    }
    fn confirm(&mut self, _: &Value) -> Result<()> {
        Err(error("no generated confirmation"))
    }
    fn complete_current_image_release(&mut self, s: &Session, request: &Value) -> Result<Value> {
        assert!(matches!(s.project_save_operation, Slot::ColdBusy));
        assert!(s.is_display_only() && s.save_operation_held());
        assert!(s.persistence.is_none());
        if self.completes.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(error("generated completion was already attempted; no replay"));
        }
        match self.outcome {
            CurrentImageRouteOutcome::Refuse => Err(error("generated completion uncertainty")),
            CurrentImageRouteOutcome::Panic => panic!("generated completion callback panic"),
            CurrentImageRouteOutcome::Complete => {
                self.completed.store(true, Ordering::SeqCst);
                Ok(json!({"syntheticRouteFixture":true,"nativeSession":crate::agent_bridge::retained_native_epoch(s),"request":request}))
            }
        }
    }
    fn inspect_completed_current_image_release(&self, s: &Session) -> Result<Value> {
        assert!(matches!(s.project_save_operation, Slot::ColdBusy));
        assert!(s.is_display_only() && s.save_operation_held());
        self.inspections.fetch_add(1, Ordering::SeqCst);
        if self.inspect_panics {
            panic!("generated completed observation panic");
        }
        if !self.completed.load(Ordering::SeqCst) {
            return Err(error("generated original completion is absent"));
        }
        Ok(json!({"syntheticRouteFixture":true,"nativeSession":crate::agent_bridge::retained_native_epoch(s)}))
    }
}
fn current_image_route_candidate(outcome: CurrentImageRouteOutcome, inspect_panics: bool) -> (Session, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let mut s = canonical_failure_candidate(false);
    let completes = Arc::new(AtomicUsize::new(0));
    let inspections = Arc::new(AtomicUsize::new(0));
    let held_reads = Arc::new(AtomicUsize::new(0));
    let Slot::Cold(held) = &mut s.project_save_operation else { panic!("generated original Cold owner required") };
    held.runtime = Some(Box::new(CurrentImageRouteRuntime {
        completes: completes.clone(),
        inspections: inspections.clone(),
        held_reads: held_reads.clone(),
        completed: Arc::new(AtomicBool::new(false)),
        outcome,
        inspect_panics,
    }));
    (s, completes, inspections, held_reads)
}
#[test]
fn current_image_route_uses_the_same_original_cold_owner_and_distinct_completed_observer_without_old_held_status() {
    use crate::cold_save_startup::{complete_held_cold_save_current_image_release, inspect_held_cold_save_current_image_release};
    let (mut s, completes, inspections, held_reads) = current_image_route_candidate(CurrentImageRouteOutcome::Complete, false);
    let graph = s.project.clone();
    let before = state(&s);
    let Slot::Cold(owner) = &s.project_save_operation else { panic!("original Cold owner required") };
    let original_owner = owner.as_ref() as *const ColdRecovery;
    assert!(inspect_held_cold_save_current_image_release(&mut s).is_err());
    let request = json!({"generatedRoutingInput":true});
    let response = complete_held_cold_save_current_image_release(&mut s, &request).unwrap();
    assert_eq!(response["completion"]["request"], request);
    assert_eq!(response["status"], "actual-native-cold-current-image-completed");
    assert_eq!(response["nativeSession"], json!(crate::agent_bridge::retained_native_epoch(&s)));
    assert_eq!(response["fullApplicationClosure"], false);
    assert_eq!(response["editingPaused"], true);
    assert!(response.get("owner").is_none());
    assert!(inspect_held_cold_save_current_image_release(&mut s).is_ok());
    assert_eq!(held_reads.load(Ordering::SeqCst), 0);
    assert!(s.cold_save_recovery_status().is_err());
    assert_eq!(held_reads.load(Ordering::SeqCst), 1);
    assert!(complete_held_cold_save_current_image_release(&mut s, &request).is_err());
    assert_eq!(completes.load(Ordering::SeqCst), 2);
    assert_eq!(inspections.load(Ordering::SeqCst), 2);
    let Slot::Cold(owner) = &s.project_save_operation else { panic!("same original owner must remain held") };
    assert_eq!(original_owner, owner.as_ref() as *const ColdRecovery);
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert_eq!(state(&s), before);
    assert!(s.is_display_only() && s.save_operation_held());
    assert!(s.persistence.is_none());
    assert!(s.execute("file.save", json!({})).is_err());
    assert!(s.execute("edit.undo", json!({})).is_err());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
}
#[test]
fn current_image_route_failure_and_unwind_restore_the_same_cold_owner_and_consumed_runtime_without_editor_or_worker_replacement() {
    for outcome in [CurrentImageRouteOutcome::Refuse, CurrentImageRouteOutcome::Panic] {
        let (mut s, completes, _, held_reads) = current_image_route_candidate(outcome, false);
        let before = state(&s);
        let graph = s.project.clone();
        let Slot::Cold(owner) = &s.project_save_operation else { panic!("original Cold owner required") };
        let original_owner = owner.as_ref() as *const ColdRecovery;
        let error = cold_application_action(&mut s, ColdApplicationAction::CompleteCurrentImageRelease(&json!({}))).unwrap_err().to_string();
        assert!(error.contains(match outcome {
            CurrentImageRouteOutcome::Refuse => "completion uncertainty",
            _ => "failed unexpectedly",
        }));
        assert!(cold_application_action(&mut s, ColdApplicationAction::CompleteCurrentImageRelease(&json!({}))).unwrap_err().to_string().contains("no replay"));
        let Slot::Cold(owner) = &s.project_save_operation else { panic!("same original owner must remain held") };
        assert_eq!(original_owner, owner.as_ref() as *const ColdRecovery);
        assert_eq!(completes.load(Ordering::SeqCst), 2);
        assert_eq!(held_reads.load(Ordering::SeqCst), 0);
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&graph, &s.project));
        assert!(s.is_display_only() && s.save_operation_held());
        assert!(s.persistence.is_none());
    }
}
#[test]
fn current_image_route_default_runtime_cannot_turn_copied_fields_into_completion_or_post_resume_observation() {
    let mut s = canonical_failure_candidate(false);
    let graph = s.project.clone();
    let before = state(&s);
    for action in [
        ColdApplicationAction::CompleteCurrentImageRelease(&json!({"completeCurrentImageVerified":true,"workerReleased":true})),
        ColdApplicationAction::InspectCompletedCurrentImageRelease,
    ] {
        let error = cold_application_action(&mut s, action).unwrap_err().to_string();
        assert!(error.contains("no actual"));
        assert!(matches!(s.project_save_operation, Slot::Cold(_)));
        assert!(s.persistence.is_none());
    }
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert!(s.is_display_only() && s.save_operation_held());
}
#[test]
fn current_image_route_native_epoch_change_refuses_before_any_runtime_callback_or_new_owner() {
    let (mut s, completes, inspections, held_reads) = current_image_route_candidate(CurrentImageRouteOutcome::Complete, false);
    crate::agent_bridge::test_native_epoch(&mut s, true).unwrap();
    let before = state(&s);
    for action in [ColdApplicationAction::CompleteCurrentImageRelease(&json!({})), ColdApplicationAction::InspectCompletedCurrentImageRelease] {
        assert!(cold_application_action(&mut s, action).is_err());
    }
    assert_eq!(completes.load(Ordering::SeqCst), 0);
    assert_eq!(inspections.load(Ordering::SeqCst), 0);
    assert_eq!(held_reads.load(Ordering::SeqCst), 0);
    assert_eq!(state(&s), before);
    assert!(matches!(s.project_save_operation, Slot::Cold(_)));
    assert!(s.is_display_only() && s.save_operation_held());
}
#[test]
fn current_image_route_completed_observer_unwind_preserves_the_original_owner_without_replaying_completion() {
    let (mut s, completes, inspections, held_reads) = current_image_route_candidate(CurrentImageRouteOutcome::Complete, true);
    cold_application_action(&mut s, ColdApplicationAction::CompleteCurrentImageRelease(&json!({}))).unwrap();
    let before = state(&s);
    let Slot::Cold(owner) = &s.project_save_operation else { panic!("original Cold owner required") };
    let original_owner = owner.as_ref() as *const ColdRecovery;
    let error = cold_application_action(&mut s, ColdApplicationAction::InspectCompletedCurrentImageRelease).unwrap_err().to_string();
    assert!(error.contains("failed unexpectedly"));
    assert_eq!(completes.load(Ordering::SeqCst), 1);
    assert_eq!(inspections.load(Ordering::SeqCst), 1);
    assert_eq!(held_reads.load(Ordering::SeqCst), 0);
    let Slot::Cold(owner) = &s.project_save_operation else { panic!("same original owner must remain held") };
    assert_eq!(original_owner, owner.as_ref() as *const ColdRecovery);
    assert_eq!(state(&s), before);
    assert!(s.persistence.is_none());
    assert!(s.is_display_only() && s.save_operation_held());
}

#[test]
fn owned_save_busy_diagnostics_keep_real_unfinished_results_and_unapplied_media_scans_fenced_without_cancelling_or_replaying() {
    let (mut s, d) = fixture();
    let before = state(&s);
    let saved = fs::read(d.join("Original.fcproj")).unwrap();
    let request = intent(&s, &d, "save");
    let job = crate::Job { id: 71, label: "Original native writer".into(), progress: Arc::default(), result: Arc::default() };
    // A progress flag is not a settled result; the original result owner is authoritative.
    job.progress.finished.store(true, Ordering::Release);
    s.jobs.push(job);
    let refusal = begin(&mut s, &id(), &request, &|| Ok(())).err().unwrap().to_string();
    assert!(refusal.contains("unfinishedJobs=1"), "{refusal}");
    assert!(refusal.contains("mediaScan=false"), "{refusal}");
    assert!(!s.jobs[0].progress.cancel.load(Ordering::Acquire));
    assert!(!s.save_operation_held());
    assert_eq!(state(&s), before);
    assert_eq!(fs::read(d.join("Original.fcproj")).unwrap(), saved);
    *s.jobs[0].result.lock().unwrap() = Some(Err("Generated original worker finished".into()));
    s.media_scan = Some(Arc::default());
    let refusal = begin(&mut s, &id(), &request, &|| Ok(())).err().unwrap().to_string();
    assert!(refusal.contains("mediaScan=true"), "{refusal}");
    assert!(refusal.contains("unfinishedJobs=0"), "{refusal}");
    assert!(!s.save_operation_held());
    assert_eq!(state(&s), before);
    // Even a completed scan is a pending writer until its original owner applies it.
    *s.media_scan.as_ref().unwrap().lock().unwrap() = Some(Vec::new());
    assert!(crate::agent_bridge::require_idle_native_writer_jobs(&s).is_err());
    assert!(s.poll_media_scan().unwrap().is_ok());
    assert!(crate::agent_bridge::require_idle_native_writer_jobs(&s).is_ok());
    assert_eq!(state(&s), before);
    assert_eq!(fs::read(d.join("Original.fcproj")).unwrap(), saved);
    assert!(!s.jobs[0].progress.cancel.load(Ordering::Acquire));
    s.shutdown();
}
