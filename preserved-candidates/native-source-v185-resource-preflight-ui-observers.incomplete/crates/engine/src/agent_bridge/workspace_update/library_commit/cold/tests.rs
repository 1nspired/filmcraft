use super::*;
use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
fn cold_checkpoint_wait_observation_keeps_actual_canonical_memory_the_same_guard_and_original_worker() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let check = media_guard::Check::with_stop(web_time::Instant::now(), stop_flag.clone());
    assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
    let published = commit.publish(&s, &|| Ok(())).unwrap();
    commit.bind_services(&mut s, &|| Ok(())).unwrap();
    commit.inspect_checkpoint_wait(&s, &check).unwrap();
    let before = s.prefs.auto_save.enabled;
    s.prefs.auto_save.enabled = !before;
    assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
    s.prefs.auto_save.enabled = before;
    commit.inspect_checkpoint_wait(&s, &check).unwrap();
    stop_flag.store(true, Ordering::Release);
    assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
    stop_flag.store(false, Ordering::Release);
    let expired = media_guard::Check::with_stop(web_time::Instant::now() - Duration::from_secs(46), stop_flag);
    assert!(commit.inspect_checkpoint_wait(&s, &expired).is_err());
    assert_eq!(commit.observation.as_ref(), Some(&published));
    assert_eq!(operation_count(&dir.0), 4);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    stop(&mut s);
    assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
}

#[test]
fn cold_checkpoint_wait_observation_refuses_actual_canonical_redirection_and_same_byte_replacement() {
    for redirect in [false, true] {
        let (dir, mut s) = fixture();
        let mut commit = prepare(&s, &dir.0);
        commit.publish(&s, &|| Ok(())).unwrap();
        commit.bind_services(&mut s, &|| Ok(())).unwrap();
        let check = media_guard::Check::new(web_time::Instant::now());
        commit.inspect_checkpoint_wait(&s, &check).unwrap();
        if redirect {
            let foreign = dir.0.join("foreign-generated-binding");
            std::fs::create_dir(&foreign).unwrap();
            s.shortcuts.bind_directory(&foreign);
            assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
        } else {
            let file = dir.0.join("preferences.json");
            let bytes = std::fs::read(&file).unwrap();
            let held = dir.0.join("retained-original-preferences");
            std::fs::rename(&file, &held).unwrap();
            std::fs::write(&file, &bytes).unwrap();
            assert!(commit.inspect_checkpoint_wait(&s, &check).is_err());
            assert_eq!(std::fs::read(&file).unwrap(), std::fs::read(&held).unwrap());
        }
        assert_eq!(operation_count(&dir.0), 4);
        assert!(s.persistence.as_ref().unwrap().startup_pending());
        assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
        stop(&mut s);
    }
}

#[test]
fn cold_canonical_paired_wait_read_keeps_original_memory_receipts_and_gated_worker() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let published = commit.publish(&s, &|| Ok(())).unwrap();
    commit.bind_services(&mut s, &|| Ok(())).unwrap();
    let memory = crate::project_save::guard(&s).unwrap();
    let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let complete = Cell::new(0);
    let final_fence = Cell::new(0);
    let current = || {
        complete.set(complete.get() + 1);
        commit.verify_published_source(&|| Ok(())).map_err(io_error)
    };
    let after = || {
        final_fence.set(final_fence.get() + 1);
        commit.fence_published_source_after_read(&|| Ok(())).map_err(io_error)
    };
    commit.inspect_published_source_read(&s, &retained::ReadSource::new(&current, &after)).unwrap();
    assert_eq!((complete.get(), final_fence.get()), (1, 1));
    assert_eq!(commit.observation.as_ref(), Some(&published));
    assert_eq!(operation_count(&dir.0), 4);
    memory.current(&s).unwrap();
    assert!(Arc::ptr_eq(&journal, &s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap()));
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    // The unchanged generic closure adapter remains conservative: full/full.
    complete.set(0);
    commit.inspect_published_source_read(&s, &current).unwrap();
    assert_eq!(complete.get(), 2);
    assert_eq!(final_fence.get(), 1);
    stop(&mut s);
}

#[test]
fn cold_canonical_paired_wait_read_fences_original_source_after_failure_and_same_byte_replacement() {
    use std::cell::Cell;
    for failed_file_read in [false, true] {
        let (dir, mut s) = fixture();
        let mut commit = prepare(&s, &dir.0);
        commit.publish(&s, &|| Ok(())).unwrap();
        commit.bind_services(&mut s, &|| Ok(())).unwrap();
        let path = dir.0.join("original-source.keep");
        let held = dir.0.join("original-source-retained.keep");
        std::fs::write(&path, b"same original source bytes").unwrap();
        let root = DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap();
        let source = root.node("original-source.keep", &|| Ok(())).unwrap();
        if failed_file_read {
            let canonical = dir.0.join("effect-presets.json");
            let raw = std::fs::read(&canonical).unwrap();
            std::fs::rename(&canonical, dir.0.join("retained-original-effect-presets")).unwrap();
            std::fs::write(&canonical, raw).unwrap();
        }
        let complete = Cell::new(0);
        let finals = Cell::new(0);
        let inspections = Cell::new(0);
        let current = || {
            complete.set(complete.get() + 1);
            source.current(&|| Ok(()))
        };
        let after = || {
            finals.set(finals.get() + 1);
            source.fence(&|| Ok(()))
        };
        let inspect = || {
            inspections.set(inspections.get() + 1);
            if !failed_file_read && inspections.get() == 3 {
                std::fs::rename(&path, &held)?;
                std::fs::write(&path, b"same original source bytes")?;
            }
            Ok(())
        };
        assert!(commit.verify_published_with_source_fence(&s, &current, &after, &inspect).is_err());
        assert_eq!((complete.get(), finals.get()), (1, 1));
        if !failed_file_read {
            assert_eq!(std::fs::read(&held).unwrap(), std::fs::read(&path).unwrap());
            assert!(source.fence(&|| Ok(())).is_err());
        }
        assert_eq!(operation_count(&dir.0), 4);
        assert!(s.persistence.as_ref().unwrap().startup_pending());
        assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
        stop(&mut s);
    }
}

#[test]
fn cold_canonical_paired_wait_read_preserves_deadline_stop_and_actual_worker_loss_refusals() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    commit.publish(&s, &|| Ok(())).unwrap();
    commit.bind_services(&mut s, &|| Ok(())).unwrap();
    for expired in [false, true] {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let began = web_time::Instant::now() - if expired { std::time::Duration::from_secs(46) } else { std::time::Duration::ZERO };
        let deadline = media_guard::Check::with_stop(began, stop_flag.clone());
        let calls = Cell::new(0);
        let inspect = || {
            calls.set(calls.get() + 1);
            if !expired && calls.get() == 3 {
                stop_flag.store(true, Ordering::Release);
            }
            deadline.check().map_err(io_error)
        };
        assert!(commit.verify_published_with_source_fence(&s, &|| Ok(()), &|| Ok(()), &inspect).is_err());
    }
    stop(&mut s);
    assert!(commit.inspect_published_source_read(&s, &retained::ReadSource::new(&|| Ok(()), &|| Ok(()))).is_err());
    assert_eq!(operation_count(&dir.0), 4);
}

#[test]
fn cold_canonical_final_source_fence_keeps_same_completed_services_and_original_worker() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    assert!(commit.fence_published_source_after_read(&|| Ok(())).is_err());
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
    commit.bind_services(&mut s, &|| Ok(())).unwrap();
    commit.verify_published_source(&|| Ok(())).unwrap();
    commit.fence_published_source_after_read(&|| Ok(())).unwrap();
    let before = crate::project_save::guard(&s).unwrap();
    assert!(commit.fence_published_source_after_read(&|| Err(io::Error::other("controlled Stop"))).is_err());
    assert_eq!(operation_count(&dir.0), 4);
    before.current(&s).unwrap();
    stop(&mut s);
    assert!(commit.fence_published_source_after_read(&|| Ok(())).is_err());
}
#[test]
fn cold_canonical_final_source_fence_refuses_original_payload_target_and_control_replacements() {
    for name in ["Keyboard Shortcuts/active.json", "effect-presets.json", "export-presets.json", "preferences.json", "intent"] {
        let (dir, mut s) = fixture();
        let mut commit = prepare(&s, &dir.0);
        assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
        commit.bind_services(&mut s, &|| Ok(())).unwrap();
        commit.verify_published_source(&|| Ok(())).unwrap();
        let path = if name == "intent" {
            let Effect::Published { publication, .. } = &commit.original.entries[0].effect else { panic!("actual original publication required") };
            publication.directory.join("intent.json")
        } else {
            dir.0.join(name)
        };
        let bytes = std::fs::read(&path).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        std::fs::rename(&path, dir.0.join("held-generation")).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(commit.fence_published_source_after_read(&|| Ok(())).is_err(), "{name}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(operation_count(&dir.0), 4);
        stop(&mut s);
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scratch() -> Scratch {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = crate::temp_dir().join(format!("cold-canonical-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path).unwrap();
    Scratch(std::fs::canonicalize(path).unwrap())
}
fn worker(s: &mut Session, root: &Path) {
    let cfg = AutosaveConfig::new(root.to_owned());
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    s.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
}
fn prepare(s: &Session, root: &Path) -> ColdCommit {
    ColdCommit::prepare(s, DirectoryFrame::capture(root, &|| Ok(())).unwrap(), &"d".repeat(64), &|| Ok(())).unwrap()
}
fn fixture() -> (Scratch, Session) {
    let dir = scratch();
    let mut s = Session::default();
    // Real canonical generations can predate the latest restored in-memory
    // shortcut history, effects and export libraries.
    for (relative, bytes, _) in payloads(&s, true).unwrap() {
        let path = dir.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    // Populate while unbound: the ordinary shortcut service persists on
    // set/undo. Bind only after memory diverges from the older canonical files.
    crate::agent_bridge::native_auxiliary_tests::populate(&mut s);
    s.shortcuts.bind_directory(&dir.0);
    s.presets.bind_directory(&dir.0);
    s.export_presets.bind_directory(&dir.0);
    // Preferences also differ from the original complete disk generation.
    s.prefs.auto_save.enabled = !s.prefs.auto_save.enabled;
    worker(&mut s, &dir.0);
    (dir, s)
}
fn operation_count(root: &Path) -> usize {
    [root.to_owned(), root.join("Keyboard Shortcuts")]
        .iter()
        .filter(|p| p.is_dir())
        .map(|p| std::fs::read_dir(p).unwrap().filter_map(|v| v.ok()).filter(|v| v.file_name().to_string_lossy().starts_with(".poema-save-")).count())
        .sum()
}
fn stop(s: &mut Session) {
    s.persistence.as_mut().unwrap().stop_worker();
}
#[test]
fn cold_canonical_actual_worker_publishes_four_original_services_without_changing_native_or_editor_history() {
    let (dir, mut s) = fixture();
    let before = crate::project_save::guard(&s).unwrap();
    let auxiliary = serde_json::to_value(crate::agent_bridge::native_auxiliary::Snapshot::capture(&s).unwrap()).unwrap();
    let mut commit = prepare(&s, &dir.0);
    let plan = commit.prepare_description(&s, &|| Ok(())).unwrap();
    assert_eq!(plan["entries"].as_array().unwrap().len(), 4);
    assert_eq!(operation_count(&dir.0), 0);
    let preference = commit.preference();
    let original = &preference.before;
    assert!(preference.current_node(original, &|| Ok(())).unwrap());
    let record = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(record["published"], true, "{record}");
    assert_eq!(operation_count(&dir.0), 4);
    for (relative, bytes, _) in payloads(&s, true).unwrap() {
        assert_eq!(std::fs::read(dir.0.join(relative)).unwrap(), bytes);
    }
    assert!(original.current(&|| Ok(())).is_err());
    assert!(preference.current_node(original, &|| Ok(())).unwrap());
    commit.verify_published(&s, &|| Ok(())).unwrap();
    before.current(&s).unwrap();
    assert_eq!(serde_json::to_value(crate::agent_bridge::native_auxiliary::Snapshot::capture(&s).unwrap()).unwrap(), auxiliary);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    let mut loaded = Session::default();
    loaded.shortcuts.set_dir(&dir.0);
    loaded.presets.set_dir(&dir.0);
    loaded.export_presets.set_dir(&dir.0);
    assert_eq!(loaded.shortcuts.recovery_file_bytes().unwrap(), s.shortcuts.recovery_file_bytes().unwrap());
    assert_eq!(loaded.presets.recovery_file_bytes().unwrap(), s.presets.recovery_file_bytes().unwrap());
    assert_eq!(loaded.export_presets.recovery_file_bytes().unwrap(), s.export_presets.recovery_file_bytes().unwrap());
    assert_eq!(crate::autosave::Preferences::load(&dir.0.join("preferences.json")), s.prefs);
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    assert!(commit.prepare_description(&s, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn cold_canonical_original_directory_writer_handles_missing_shortcut_parent_and_absent_preferences() {
    let dir = scratch();
    let mut s = Session::default();
    worker(&mut s, &dir.0);
    let mut commit = prepare(&s, &dir.0);
    let preference = commit.preference();
    assert_eq!(preference.before.state(), &NodeState::Absent);
    let record = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(record["published"], true, "{record}");
    assert!(commit.original.owned_shortcut.is_some());
    assert!(preference.current_node(&preference.before, &|| Ok(())).unwrap());
    commit.verify_published(&s, &|| Ok(())).unwrap();
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn cold_canonical_partial_write_retains_later_human_bytes_and_original_receipts_without_retry() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let first = dir.0.join("Keyboard Shortcuts/active.json");
    let second = dir.0.join("effect-presets.json");
    let expected = s.shortcuts.recovery_file_bytes().unwrap();
    let changed = AtomicBool::new(false);
    let check = || {
        if std::fs::read(&first).is_ok_and(|v| v == expected) && !changed.swap(true, Ordering::SeqCst) {
            std::fs::write(&second, b"later human library edit")?;
        }
        Ok(())
    };
    let record = commit.publish(&s, &check).unwrap();
    assert_eq!(record["published"], false, "{record}");
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(&first).unwrap(), expected);
    assert_eq!(std::fs::read(&second).unwrap(), b"later human library edit");
    assert_eq!(record["effects"]["entries"][0]["phase"], "published");
    assert_eq!(record["effects"]["entries"][0]["originalReceiptDurable"], true);
    let count = operation_count(&dir.0);
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    assert_eq!(operation_count(&dir.0), count);
    assert_eq!(std::fs::read(&second).unwrap(), b"later human library edit");
    assert!(commit.preference().current_node(&commit.preference.before, &|| Ok(())).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    stop(&mut s);
}
#[test]
fn cold_canonical_same_byte_preference_replacement_is_not_a_successor() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let prefs = dir.0.join("preferences.json");
    let replacement = dir.0.join("human-replacement");
    let original = std::fs::read(&prefs).unwrap();
    std::fs::write(&replacement, &original).unwrap();
    std::fs::rename(&replacement, &prefs).unwrap();
    assert!(commit.prepare_description(&s, &|| Ok(())).is_err());
    let record = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(record["published"], false);
    assert_eq!(operation_count(&dir.0), 0);
    assert_eq!(std::fs::read(&prefs).unwrap(), original);
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn cold_canonical_changed_library_binding_refuses_before_staging_even_with_identical_memory() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let foreign = dir.0.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    s.presets.bind_directory(&foreign);
    assert!(commit.prepare_description(&s, &|| Ok(())).is_err());
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], false);
    assert_eq!(operation_count(&dir.0), 0);
    stop(&mut s);
}
#[test]
fn cold_canonical_foreign_original_binding_is_rejected_without_following_or_writing_it() {
    let (dir, mut s) = fixture();
    let foreign = dir.0.join("foreign-original");
    s.shortcuts.bind_directory(&foreign);
    assert!(ColdCommit::prepare(&s, DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap(), &"e".repeat(64), &|| Ok(())).is_err());
    assert!(!foreign.exists());
    assert_eq!(operation_count(&dir.0), 0);
    stop(&mut s);
}
#[test]
fn cold_canonical_stopped_or_foreign_actual_worker_cannot_publish() {
    let (dir, mut s) = fixture();
    let foreign = dir.0.join("foreign-root");
    std::fs::create_dir(&foreign).unwrap();
    assert!(ColdCommit::prepare(&s, DirectoryFrame::capture(&foreign, &|| Ok(())).unwrap(), &"f".repeat(64), &|| Ok(())).is_err());
    let mut commit = prepare(&s, &dir.0);
    stop(&mut s);
    assert!(commit.prepare_description(&s, &|| Ok(())).is_err());
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], false);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(commit.publish(&s, &|| Ok(())).is_err());
}
#[test]
fn cold_canonical_unchanged_generations_keep_their_physical_identity_without_staging() {
    let dir = scratch();
    let mut s = Session::default();
    for (relative, bytes, _) in payloads(&s, true).unwrap() {
        let path = dir.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    worker(&mut s, &dir.0);
    let mut commit = prepare(&s, &dir.0);
    let before = commit.prepare_description(&s, &|| Ok(())).unwrap();
    assert!(before["entries"].as_array().unwrap().iter().all(|e| e["phase"] == "unchanged"));
    let record = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(record["published"], true, "{record}");
    assert_eq!(operation_count(&dir.0), 0);
    commit.verify_published(&s, &|| Ok(())).unwrap();
    for (old, new) in before["entries"].as_array().unwrap().iter().zip(record["effects"]["entries"].as_array().unwrap()) {
        assert_eq!(old["target"], new["target"]);
    }
    stop(&mut s);
}
#[test]
fn cold_canonical_successor_does_not_accept_later_same_byte_replacement() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
    let prefs = dir.0.join("preferences.json");
    let replacement = dir.0.join("later-replacement");
    std::fs::write(&replacement, std::fs::read(&prefs).unwrap()).unwrap();
    std::fs::rename(replacement, prefs).unwrap();
    assert!(commit.verify_published(&s, &|| Ok(())).is_err());
    assert!(commit.preference().current_node(&commit.preference.before, &|| Ok(())).is_err());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn cold_canonical_inflight_preference_delegation_is_scoped_to_original_writer_thread_and_role() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let preference = commit.preference();
    let unrelated = frame(&dir.0.join("effect-presets.json"), &|| Ok(())).unwrap();
    assert!(!preference.current_node(&unrelated, &|| Ok(())).unwrap());
    let checked = AtomicBool::new(false);
    let check = || {
        if !checked.swap(true, Ordering::SeqCst) {
            assert!(preference.current_node(&preference.before, &|| Ok(())).unwrap());
            std::thread::scope(|scope| {
                let p = preference.clone();
                assert!(scope.spawn(move || p.current_node(&p.before, &|| Ok(())).is_err()).join().unwrap());
            });
        }
        Ok(())
    };
    assert_eq!(commit.publish(&s, &check).unwrap()["published"], true);
    assert!(checked.load(Ordering::SeqCst));
    stop(&mut s);
}

#[test]
fn cold_canonical_preference_continuation_reentrant_guard_does_not_hold_the_phase_mutex() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let preference = commit.preference();
    let first = AtomicBool::new(false);
    let check = || {
        if !first.swap(true, Ordering::SeqCst) {
            assert!(preference.current_node(&preference.before, &|| Ok(())).unwrap());
        }
        Ok(())
    };
    assert!(preference.current_node(&preference.before, &check).unwrap());
    assert!(first.load(Ordering::SeqCst));
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
    first.store(false, Ordering::SeqCst);
    assert!(preference.current_node(&preference.before, &check).unwrap());
    assert!(first.load(Ordering::SeqCst));
    stop(&mut s);
}

#[test]
fn cold_canonical_preferences_binding_cannot_escape_or_change_during_publication() {
    let (dir, mut s) = fixture();
    s.prefs_path = Some(dir.0.join("foreign-preferences.json"));
    assert!(ColdCommit::prepare(&s, DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap(), &"e".repeat(64), &|| Ok(())).is_err());
    s.prefs_path = Some(dir.0.join("preferences.json"));
    let mut commit = prepare(&s, &dir.0);
    s.prefs_path = None;
    assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], false);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(!dir.0.join("foreign-preferences.json").exists());
    stop(&mut s);
}

#[test]
fn cold_canonical_replaced_created_shortcut_directory_is_never_adopted() {
    let dir = scratch();
    let mut s = Session::default();
    worker(&mut s, &dir.0);
    let mut commit = prepare(&s, &dir.0);
    let shortcut = dir.0.join("Keyboard Shortcuts");
    let retained = dir.0.join("original-created-shortcut");
    let changed = AtomicBool::new(false);
    let observed = std::sync::atomic::AtomicUsize::new(0);
    let check = || {
        // First callback sees the created child inside directory_effect. The
        // second is the caller boundary after publish_owned returned its owner.
        if shortcut.is_dir() && observed.fetch_add(1, Ordering::SeqCst) == 1 && !changed.swap(true, Ordering::SeqCst) {
            std::fs::rename(&shortcut, &retained)?;
            std::fs::create_dir(&shortcut)?;
        }
        Ok(())
    };
    let result = commit.publish(&s, &check).unwrap();
    assert_eq!(result["published"], false);
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(std::fs::read_dir(&shortcut).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&retained).unwrap().count(), 0);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn cold_canonical_unwinding_guard_stops_preference_delegation_without_replaying() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let preference = commit.preference();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = commit.publish(&s, &|| -> std::io::Result<()> {
            panic!("generated trusted guard failure");
        });
    }));
    assert!(failed.is_err());
    assert!(preference.current_node(&preference.before, &|| Ok(())).is_err());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    assert_eq!(operation_count(&dir.0), 0);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn cold_canonical_same_byte_receipt_replacement_cannot_renew_actual_publication_custody() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let result = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true);
    let row = &result["effects"]["entries"][0];
    let root = PathBuf::from(row["operationDirectory"].as_str().unwrap());
    let receipt = root.join("receipt.json");
    let replacement = root.join("later-receipt");
    std::fs::write(&replacement, std::fs::read(&receipt).unwrap()).unwrap();
    std::fs::rename(&replacement, &receipt).unwrap();
    assert!(commit.verify_published(&s, &|| Ok(())).is_err());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}

#[test]
fn cold_canonical_readonly_worker_observation_refuses_source_generation_lost_at_its_final_fence() {
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let source = dir.0.join("original-source-control");
    let replacement = dir.0.join("replacement-source-control");
    std::fs::write(&source, b"retained source bytes").unwrap();
    let retained = frame(&source, &|| Ok(())).unwrap();
    let calls = std::cell::Cell::new(0usize);
    let guard = || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            // Same bytes are insufficient: replace the actual retained source
            // generation after the read-only worker observation has run.
            std::fs::write(&replacement, b"retained source bytes")?;
            std::fs::rename(&replacement, &source)?;
        }
        retained.current(&|| Ok(()))
    };
    assert!(commit.worker_current(&s, &guard).is_err());
    assert_eq!(calls.get(), 2);
    assert_eq!(operation_count(&dir.0), 0);
    assert_eq!(std::fs::read(&source).unwrap(), b"retained source bytes");
    let result = commit.publish(&s, &|| retained.current(&|| Ok(()))).unwrap();
    assert_eq!(result["published"], false);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}

fn retained_preference_fixture() -> (Scratch, Session, PathBuf) {
    let dir = scratch();
    let mut s = Session::default();
    for (relative, bytes, _) in payloads(&s, true).unwrap() {
        let target = dir.0.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    let preferences = dir.0.join("preferences.json");
    let mut bytes = std::fs::read(&preferences).unwrap();
    bytes.push(b'\n');
    std::fs::write(preferences, bytes).unwrap();
    let old = crate::autosave::recovery_root(&dir.0).join("100-1");
    std::fs::create_dir_all(&old).unwrap();
    let retained = old.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    std::fs::write(&retained, b"original retained native recovery bytes").unwrap();
    worker(&mut s, &dir.0);
    (dir, s, retained)
}
#[test]
fn cold_canonical_existing_worker_retains_original_preference_continuation_across_real_file_exchange() {
    let (dir, mut s, retained) = retained_preference_fixture();
    let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let mut commit = prepare(&s, &dir.0);
    let plan = commit.prepare_description(&s, &|| Ok(())).unwrap();
    assert_eq!(plan["entries"][3]["phase"], "before");
    let result = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true, "{result}");
    assert_eq!(result["effects"]["entries"][3]["originalReceiptDurable"], true);
    assert_eq!(result["effects"]["entries"][3]["originalReceipt"]["outcome"], "published");
    assert_eq!(std::fs::read(retained).unwrap(), b"original retained native recovery bytes");
    let actual = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    assert!(Arc::ptr_eq(&journal, &actual));
    commit.verify_published(&s, &|| Ok(())).unwrap();
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn cold_canonical_existing_worker_refuses_same_byte_preference_replacement_after_original_publication() {
    let (dir, mut s, _) = retained_preference_fixture();
    let mut commit = prepare(&s, &dir.0);
    let result = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true, "{result}");
    let target = dir.0.join("preferences.json");
    let bytes = std::fs::read(&target).unwrap();
    let replacement = dir.0.join("later-human-preferences");
    std::fs::write(&replacement, &bytes).unwrap();
    std::fs::rename(&replacement, &target).unwrap();
    assert!(s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).is_err());
    assert!(commit.verify_published(&s, &|| Ok(())).is_err());
    assert!(commit.publish(&s, &|| Ok(())).is_err());
    assert_eq!(std::fs::read(target).unwrap(), bytes);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn cold_canonical_existing_worker_still_refuses_changed_original_recovery_members_after_preference_publication() {
    let (dir, mut s, retained) = retained_preference_fixture();
    let mut commit = prepare(&s, &dir.0);
    let result = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true, "{result}");
    std::fs::write(&retained, b"later human recovery bytes").unwrap();
    assert!(s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).is_err());
    assert!(commit.verify_published(&s, &|| Ok(())).is_err());
    assert_eq!(std::fs::read(retained).unwrap(), b"later human recovery bytes");
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn cold_canonical_worker_refuses_foreign_or_repeated_preference_continuation_without_replacing_its_owner() {
    let (first_dir, mut first, _) = retained_preference_fixture();
    let first_commit = prepare(&first, &first_dir.0);
    let original = first.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let first_root = DirectoryFrame::capture(&first_dir.0, &|| Ok(())).unwrap().record();
    assert!(original.retain_canonical_preferences(&first_root, first_commit.preference(), &|| Ok(())).is_err());
    original.current(&|| Ok(())).unwrap();
    let (second_dir, mut second, _) = retained_preference_fixture();
    let other = second.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let second_root = DirectoryFrame::capture(&second_dir.0, &|| Ok(())).unwrap().record();
    assert!(other.retain_canonical_preferences(&second_root, first_commit.preference(), &|| Ok(())).is_err());
    other.current(&|| Ok(())).unwrap();
    let mut second_commit = prepare(&second, &second_dir.0);
    let result = second_commit.publish(&second, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true, "{result}");
    assert!(Arc::ptr_eq(&other, &second.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap()));
    stop(&mut first);
    stop(&mut second);
}

#[test]
fn cold_canonical_bounded_preparation_keeps_every_original_observation_with_two_complete_source_fences() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let commit = prepare(&s, &dir.0);
    let memory = crate::project_save::guard(&s).unwrap();
    let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let legacy = Cell::new(0);
    let original_source = || {
        legacy.set(legacy.get() + 1);
        Ok(())
    };
    commit.current_before(&s, &original_source).unwrap();
    let original = commit.original.observe(&s, &original_source).unwrap();
    let bounded = Cell::new(0);
    let actual = commit
        .prepare_description(&s, &|| {
            bounded.set(bounded.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(actual, original);
    assert_eq!(bounded.get(), 2);
    assert!(legacy.get() > bounded.get());
    assert_eq!(operation_count(&dir.0), 0);
    memory.current(&s).unwrap();
    assert!(Arc::ptr_eq(&journal, &s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap()));
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    stop(&mut s);
}
#[test]
fn cold_canonical_bounded_published_read_keeps_original_receipts_and_same_actual_gated_worker() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let mut commit = prepare(&s, &dir.0);
    let published = commit.publish(&s, &|| Ok(())).unwrap();
    assert_eq!(published["published"], true);
    let memory = crate::project_save::guard(&s).unwrap();
    let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let legacy = Cell::new(0);
    let original_source = || {
        legacy.set(legacy.get() + 1);
        Ok(())
    };
    commit.worker_current(&s, &original_source).unwrap();
    commit.original.verify_published(&s, &original_source).unwrap();
    assert!(commit.preference.current_node(&commit.preference.before, &original_source).unwrap());
    let bounded = Cell::new(0);
    commit
        .verify_published(&s, &|| {
            bounded.set(bounded.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(bounded.get(), 2);
    assert!(legacy.get() > bounded.get());
    assert_eq!(commit.observation.as_ref(), Some(&published));
    assert_eq!(operation_count(&dir.0), 4);
    memory.current(&s).unwrap();
    assert!(Arc::ptr_eq(&journal, &s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap()));
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    stop(&mut s);
}
#[test]
fn cold_canonical_bounded_read_refuses_source_loss_before_any_complete_observation_or_new_effect() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let commit = prepare(&s, &dir.0);
    let count = Cell::new(0);
    let source = || {
        count.set(count.get() + 1);
        Err(io::Error::other("controlled source loss before read"))
    };
    assert!(commit.prepare_description(&s, &source).is_err());
    assert_eq!(count.get(), 1);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(!commit.attempted);
    commit.prepare_description(&s, &|| Ok(())).unwrap();
    stop(&mut s);
}
#[test]
fn cold_canonical_bounded_read_rechecks_source_loss_inside_both_original_read_paths() {
    use std::cell::Cell;
    for published in [false, true] {
        let (dir, mut s) = fixture();
        let mut commit = prepare(&s, &dir.0);
        if published {
            assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
        }
        let alive = Cell::new(true);
        let outer = Cell::new(0);
        let inner = Cell::new(0);
        let source = || {
            outer.set(outer.get() + 1);
            if alive.get() { Ok(()) } else { Err(io::Error::other("controlled source loss inside read")) }
        };
        let inspect = || {
            inner.set(inner.get() + 1);
            if inner.get() == 3 {
                alive.set(false);
            }
            Ok(())
        };
        let result = if published {
            commit.verify_published_with_inspection(&s, &source, &inspect)
        } else {
            commit.prepare_description_with_inspection(&s, &source, &inspect).map(|_| ())
        };
        assert!(result.is_err());
        assert_eq!(outer.get(), 2);
        assert!(inner.get() > 3);
        assert_eq!(operation_count(&dir.0), if published { 4 } else { 0 });
        assert!(s.persistence.as_ref().unwrap().startup_pending());
        assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
        stop(&mut s);
    }
}
#[test]
fn cold_canonical_bounded_read_keeps_expired_deadline_and_independent_stop_refusals() {
    use std::cell::Cell;
    for expired in [false, true] {
        let (dir, mut s) = fixture();
        let commit = prepare(&s, &dir.0);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let began = web_time::Instant::now() - if expired { std::time::Duration::from_secs(46) } else { std::time::Duration::ZERO };
        let deadline = media_guard::Check::with_stop(began, stop_flag.clone());
        let calls = Cell::new(0);
        let inspect = || {
            calls.set(calls.get() + 1);
            if !expired && calls.get() == 4 {
                stop_flag.store(true, Ordering::Release);
            }
            deadline.check().map_err(io_error)
        };
        assert!(commit.prepare_description_with_inspection(&s, &|| Ok(()), &inspect).is_err());
        assert_eq!(operation_count(&dir.0), 0);
        assert!(!commit.attempted);
        commit.prepare_description(&s, &|| Ok(())).unwrap();
        stop(&mut s);
    }
}
#[test]
fn cold_canonical_bounded_read_refuses_a_same_byte_preference_replacement_during_inspection() {
    use std::cell::Cell;
    for published in [false, true] {
        let (dir, mut s) = fixture();
        let mut commit = prepare(&s, &dir.0);
        if published {
            assert_eq!(commit.publish(&s, &|| Ok(())).unwrap()["published"], true);
        }
        let path = dir.0.join("preferences.json");
        let held = dir.0.join("held-original-preferences");
        let bytes = std::fs::read(&path).unwrap();
        let calls = Cell::new(0);
        let inspect = || {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                std::fs::rename(&path, &held)?;
                std::fs::write(&path, &bytes)?;
            }
            Ok(())
        };
        let result = if published {
            commit.verify_published_with_inspection(&s, &|| Ok(()), &inspect)
        } else {
            commit.prepare_description_with_inspection(&s, &|| Ok(()), &inspect).map(|_| ())
        };
        assert!(result.is_err());
        assert_eq!(std::fs::read(&held).unwrap(), bytes);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(s.persistence.as_ref().unwrap().startup_pending());
        assert_eq!(operation_count(&dir.0), if published { 4 } else { 0 });
        stop(&mut s);
    }
}
#[test]
fn cold_canonical_bounded_read_keeps_complete_source_fences_even_when_an_original_file_observation_fails() {
    use std::cell::Cell;
    let (dir, mut s) = fixture();
    let commit = prepare(&s, &dir.0);
    let path = dir.0.join("effect-presets.json");
    let held = dir.0.join("held-original-effect-presets");
    std::fs::rename(&path, &held).unwrap();
    std::fs::write(&path, std::fs::read(&held).unwrap()).unwrap();
    let calls = Cell::new(0);
    assert!(
        commit
            .prepare_description(&s, &|| {
                calls.set(calls.get() + 1);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(calls.get(), 2);
    assert_eq!(operation_count(&dir.0), 0);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}

#[test]
fn original_preference_after_read_fence_uses_same_before_and_published_frames_and_refuses_fresh_matching_baselines() {
    let (dir, mut session) = fixture();
    let mut commit = prepare(&session, &dir.0);
    let preference = commit.preference();
    let before = &preference.before;
    assert!(preference.current_node(before, &|| Ok(())).unwrap());
    assert!(preference.fence_node_after_read(before, &|| Ok(())).unwrap());
    let result = commit.publish(&session, &|| Ok(())).unwrap();
    assert_eq!(result["published"], true);
    assert!(preference.current_node(before, &|| Ok(())).unwrap());
    assert!(preference.fence_node_after_read(before, &|| Ok(())).unwrap());
    let fresh = DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap().node("preferences.json", &|| Ok(())).unwrap();
    assert!(preference.fence_node_after_read(&fresh, &|| Ok(())).is_err());
    let path = dir.0.join("preferences.json");
    let bytes = std::fs::read(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions();
    std::fs::rename(&path, dir.0.join("held-preference")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    std::fs::set_permissions(&path, mode).unwrap();
    assert!(preference.fence_node_after_read(before, &|| Ok(())).is_err());
    stop(&mut session);
}
#[test]
fn original_preference_after_read_fence_keeps_stop_and_crossed_path_guards_without_publication() {
    let (dir, mut session) = fixture();
    let commit = prepare(&session, &dir.0);
    let preference = commit.preference();
    let before = &preference.before;
    preference.current_node(before, &|| Ok(())).unwrap();
    assert!(preference.fence_node_after_read(before, &|| Err(io::Error::other("controlled Stop"))).is_err());
    let other = DirectoryFrame::capture(&dir.0, &|| Ok(())).unwrap().node("export-presets.json", &|| Ok(())).unwrap();
    assert!(!preference.fence_node_after_read(&other, &|| Ok(())).unwrap());
    assert_eq!(operation_count(&dir.0), 0);
    assert!(session.persistence.as_ref().unwrap().startup_pending());
    stop(&mut session);
}
