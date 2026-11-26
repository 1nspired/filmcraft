//! Actual original worker/FIFO/complete-file composition; generated source
//! callbacks are component witnesses, never a whole Cold application permit.
use super::super::archive::published;
use super::super::tests::stop;
use super::*;
use std::{
    fs,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
fn check() -> media_guard::Check {
    media_guard::Check::new(web_time::Instant::now())
}

#[test]
fn original_checkpoint_continuation_retains_same_release_fifo_flush_and_complete_history_without_editor_admission() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let original_graph = s.project.clone();
    let original_editor = serde_json::to_value(&s.state).unwrap();
    let native_epoch = crate::agent_bridge::retained_native_epoch(&s).unwrap().to_owned();
    p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    assert!(!s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    assert!(Arc::ptr_eq(&original_graph, &s.project));
    assert_eq!(original_editor, serde_json::to_value(&s.state).unwrap());
    assert_eq!(crate::agent_bridge::retained_native_epoch(&s), Some(native_epoch.as_str()));
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert!(p.current_native.is_none());
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_some() && state.input_attempted && state.input.is_some() && state.flush.is_some() && state.image.is_some());
    p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    assert!(p.current(&s).is_err(), "held-only reader must remain strict");
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap_err().to_string().contains("no replay"));
    let worker = s.persistence.as_ref().unwrap();
    assert_eq!(fs::read_dir(&worker.session_dir).unwrap().count(), 4);
    stop(&mut s);
    assert!(p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
}

#[test]
fn original_checkpoint_continuation_refuses_initial_source_and_stale_editor_before_consuming_real_preparation() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    assert!(p.complete_current_release(&s, &|| Err(io::Error::other("actual source refused")), &|| Ok(()), &check()).is_err());
    assert!(p.completion.is_none() && p.current_native.is_some());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    let before = s.state.source_playhead;
    s.state.source_playhead = filmcraft_time::Tick(42);
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    assert!(p.completion.is_none());
    s.state.source_playhead = before;
    p.current(&s).unwrap();
    p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    stop(&mut s);
}

#[test]
fn original_checkpoint_continuation_keeps_unknown_after_resume_without_minting_or_repeating_release_receipts() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let journal = worker.current_image_journal(&|| Ok(())).unwrap();
    let source = || if journal.current(&|| Ok(())).is_ok() { Ok(()) } else { Err(io::Error::other("actual source lost after Resume")) };
    assert!(p.complete_current_release(&s, &source, &|| Ok(()), &check()).is_err());
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_none() && !state.input_attempted && state.input.is_none() && state.flush.is_none() && state.image.is_none());
    assert!(p.current_native.is_some());
    assert!(p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    stop(&mut s);
}

#[test]
fn original_checkpoint_continuation_retains_consumed_fifo_outcome_if_source_is_lost_after_real_file_publication() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let target = worker.session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    let observed = AtomicBool::new(false);
    let source = || {
        if target.is_file() {
            observed.store(true, Ordering::Release);
            Err(io::Error::other("source lost after ordinary worker output"))
        } else {
            Ok(())
        }
    };
    let memory_after_failed_source = std::rc::Rc::new(std::cell::Cell::new(0));
    let memory = || {
        if observed.load(Ordering::Acquire) {
            memory_after_failed_source.set(memory_after_failed_source.get() + 1);
        }
        Ok(())
    };
    assert!(p.complete_current_release(&s, &source, &memory, &check()).is_err());
    assert!(memory_after_failed_source.get() > 0);
    assert!(observed.load(Ordering::Acquire), "a genuine original worker publication must trigger this fault");
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_some() && state.input_attempted && state.image.is_none());
    assert!(p.current_native.is_none(), "the consumed native COW cannot be recaptured or replayed");
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
    assert!(target.is_file(), "retain original uncertain output through Stop");
}

#[test]
fn original_checkpoint_continuation_refuses_corrupt_actual_fifo_bytes_without_settling_or_overwriting_them() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let target = worker.session_dir.join(crate::autosave::SNAPSHOT);
    let native = worker.session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    let changed = AtomicBool::new(false);
    let source = || {
        if native.is_file() && !changed.swap(true, Ordering::AcqRel) {
            fs::write(&target, b"generated corrupt current image").unwrap();
        }
        Ok(())
    };
    assert!(p.complete_current_release(&s, &source, &|| Ok(()), &check()).is_err());
    assert!(changed.load(Ordering::Acquire));
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_some() && state.input_attempted && state.image.is_none());
    assert_eq!(fs::read(&target).unwrap(), b"generated corrupt current image");
    assert!(p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
    assert_eq!(fs::read(&target).unwrap(), b"generated corrupt current image");
}

#[test]
fn original_checkpoint_continuation_borrows_initial_late_stop_and_expired_check_without_resetting_clock() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let stopped = Arc::new(AtomicBool::new(true));
    let initial = media_guard::Check::with_stop(web_time::Instant::now(), stopped.clone());
    let expired = media_guard::Check::new(web_time::Instant::now() - Duration::from_secs(46));
    for guard in [&initial, &expired] {
        assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), guard).is_err());
        assert!(p.completion.is_none() && p.current_native.is_some());
        assert!(s.persistence.as_ref().unwrap().startup_pending());
    }
    stopped.store(false, Ordering::Release);
    let late = media_guard::Check::with_stop(web_time::Instant::now(), stopped.clone());
    let worker = s.persistence.as_ref().unwrap();
    let journal = worker.current_image_journal(&|| Ok(())).unwrap();
    let source = || {
        if journal.current(&|| Ok(())).is_err() {
            stopped.store(true, Ordering::Release);
        }
        Ok(())
    };
    assert!(p.complete_current_release(&s, &source, &|| Ok(()), &late).is_err());
    assert!(stopped.load(Ordering::Acquire));
    assert!(p.completion.is_some());
    stopped.store(false, Ordering::Release);
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
}

#[test]
fn original_checkpoint_live_memory_guard_stays_on_non_send_caller_and_rechecks_retained_completion() {
    let (_d, mut s, mut p) = published(true);
    assert!(p.retained_release_frame(&s, &check()).is_err());
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let frame = p.retained_release_frame(&s, &check()).unwrap();
    let caller = std::thread::current().id();
    let allowed = std::rc::Rc::new(std::cell::Cell::new(true));
    let observations = std::rc::Rc::new(std::cell::Cell::new(0));
    let memory = || {
        assert_eq!(std::thread::current().id(), caller);
        observations.set(observations.get() + 1);
        if allowed.get() { Ok(()) } else { Err(error("actual original caller memory refused")) }
    };
    p.complete_current_release(&s, &|| Ok(()), &memory, &check()).unwrap();
    assert!(Arc::ptr_eq(&frame, &p.retained_release_frame(&s, &check()).unwrap()));
    let before = observations.get();
    p.current_completed_release(&s, &|| Ok(()), &memory, &check()).unwrap();
    assert!(observations.get() > before);
    allowed.set(false);
    assert!(p.current_completed_release(&s, &|| Ok(()), &memory, &check()).is_err());
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_some() && state.input.is_some() && state.flush.is_some() && state.image.is_some());
    assert!(p.complete_current_release(&s, &|| Ok(()), &memory, &check()).is_err());
    assert!(p.current(&s).is_err());
    stop(&mut s);
    assert!(p.retained_release_frame(&s, &check()).is_err());
}

#[test]
fn original_checkpoint_live_memory_guard_refusal_precedes_original_prepare_and_preserves_same_cow() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let frame = p.retained_release_frame(&s, &check()).unwrap();
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let memory = || {
        calls.set(calls.get() + 1);
        Err(error("actual original memory refused before release"))
    };
    assert!(p.complete_current_release(&s, &|| Ok(()), &memory, &check()).is_err());
    assert!(calls.get() > 0);
    assert!(p.completion.is_none() && p.current_native.is_some());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(Arc::ptr_eq(&frame, &p.retained_release_frame(&s, &check()).unwrap()));
    p.current(&s).unwrap();
    stop(&mut s);
}

#[test]
fn original_checkpoint_live_memory_guard_after_resume_mints_no_ack_or_replay_even_with_current_files() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let journal = s.persistence.as_ref().unwrap().current_image_journal(&|| Ok(())).unwrap();
    let refused = std::rc::Rc::new(std::cell::Cell::new(false));
    let memory = || {
        if journal.current(&|| Ok(())).is_err() {
            refused.set(true);
            Err(error("original canonical memory lost after Resume"))
        } else {
            Ok(())
        }
    };
    assert!(p.complete_current_release(&s, &|| Ok(()), &memory, &check()).is_err());
    assert!(refused.get());
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_none() && !state.input_attempted && state.input.is_none() && state.flush.is_none() && state.image.is_none());
    assert!(p.current_native.is_some());
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
}

#[test]
fn original_checkpoint_live_memory_guard_after_fifo_keeps_actual_output_without_false_readback_or_replay() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let target = s.persistence.as_ref().unwrap().session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    let refused = std::rc::Rc::new(std::cell::Cell::new(false));
    let memory = || {
        if target.is_file() {
            refused.set(true);
            Err(error("original canonical memory lost after real FIFO output"))
        } else {
            Ok(())
        }
    };
    assert!(p.complete_current_release(&s, &|| Ok(()), &memory, &check()).is_err());
    assert!(refused.get());
    let state = p.completion.as_ref().unwrap();
    assert!(state.release.is_some() && state.input_attempted && state.image.is_none());
    assert!(p.current_native.is_none());
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
    assert!(target.is_file());
}

// These compose the actual checkpoint and original FIFO worker. The full
// Pending must also validate/adopt its real private journal successor, source
// witness and canonical frame; this description check cannot supply them.
fn assert_observation(p: &Prepared, event: &Value) -> Result<()> {
    super::super::super::settlement::assert_current_image_observation_description(p, event)
}
fn observation(p: &Prepared) -> Value {
    json!({"kind":"native-current-image-observation","checkpoint":p.description(),"image":p.actual_current_image_files().unwrap()})
}

#[test]
fn original_current_image_observation_requires_actual_fifo_completion_and_preserves_the_same_checkpoint_and_worker() {
    let (_d, mut s, mut p) = published(true);
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let checkpoint = p.description();
    let graph = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let premature = json!({"kind":"native-current-image-observation","checkpoint":checkpoint,"image":[]});
    assert!(assert_observation(&p, &premature).is_err());
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    assert!(assert_observation(&p, &premature).is_err());
    p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    let event = observation(&p);
    assert_observation(&p, &event).unwrap();
    p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    assert_eq!(event["checkpoint"], checkpoint);
    assert_eq!(event["image"].as_array().unwrap().len(), 3);
    assert!(Arc::ptr_eq(&graph, &s.project));
    assert_eq!(editor, serde_json::to_value(&s.state).unwrap());
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert!(!s.persistence.as_ref().unwrap().startup_pending());
    assert!(p.current(&s).is_err(), "historical descriptions cannot revive the held reader");
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
}

#[test]
fn original_current_image_observation_refuses_crossed_checkpoint_and_each_complete_original_file_witness() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    let event = observation(&p);
    let mut mutations = vec![Value::Null];
    for field in ["nativePid", "nativeSession", "privateInput", "projectPath", "revision", "files", "workerReleased", "sourceCapture", "afterFrameSha256"] {
        let mut crossed = event.clone();
        crossed["checkpoint"][field] = Value::Null;
        mutations.push(crossed);
    }
    for index in 0..3 {
        for field in ["path", "bytes", "sha256", "dev", "ino", "mode", "uid", "nlink", "mtimeNs", "ctimeNs"] {
            let mut crossed = event.clone();
            crossed["image"][index][field] = Value::Null;
            mutations.push(crossed);
        }
    }
    let mut reordered = event.clone();
    reordered["image"].as_array_mut().unwrap().swap(0, 1);
    mutations.push(reordered);
    let mut partial = event.clone();
    partial["image"].as_array_mut().unwrap().pop();
    mutations.push(partial);
    let mut surplus = event.clone();
    surplus["startupAuthority"] = json!(true);
    mutations.push(surplus);
    for crossed in mutations {
        assert!(assert_observation(&p, &crossed).is_err(), "crossed description was accepted: {crossed}");
    }
    assert_observation(&p, &event).unwrap();
    p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    stop(&mut s);
}

#[test]
fn original_current_image_observation_cannot_replace_actual_final_file_and_worker_checks_with_matching_descriptions() {
    let (d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    let event = observation(&p);
    assert_observation(&p, &event).unwrap();
    p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).unwrap();
    let target = s.persistence.as_ref().unwrap().session_dir.join(crate::autosave::SNAPSHOT);
    let bytes = fs::read(&target).unwrap();
    fs::rename(&target, d.0.join("retained-original-current-image")).unwrap();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut output = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target).unwrap();
    output.write_all(&bytes).unwrap();
    output.sync_all().unwrap();
    drop(output);
    // The retained description remains historical; it grants no file currency.
    assert_observation(&p, &event).unwrap();
    assert!(p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert!(p.complete_current_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
    stop(&mut s);
    assert!(p.current_completed_release(&s, &|| Ok(()), &|| Ok(()), &check()).is_err());
}
