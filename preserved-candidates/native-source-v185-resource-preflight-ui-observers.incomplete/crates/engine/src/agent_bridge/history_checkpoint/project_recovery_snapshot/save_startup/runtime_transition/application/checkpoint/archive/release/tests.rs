//! Real original Prepared/Archive/worker custody; generated component source
//! callbacks do not supply an actual Cold Input or whole-application authority.
use super::super::super::tests::stop;
use super::super::published;
use super::*;
use std::{
    cell::Cell,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};
fn check() -> media_guard::Check {
    media_guard::Check::new(web_time::Instant::now())
}
fn archive_path(s: &Session) -> std::path::PathBuf {
    s.persistence.as_ref().unwrap().data_dir.join(format!(".poema-checkpoint-{}", crate::agent_bridge::retained_native_epoch(s).unwrap()))
}
fn generation(p: &std::path::Path) -> (u64, u64, u32, u64, i64, i64) {
    let m = fs::symlink_metadata(p).unwrap();
    (m.dev(), m.ino(), m.mode(), m.len(), m.mtime_nsec(), m.ctime_nsec())
}
#[test]
fn actual_current_release_preparation_consumes_same_publisher_keeps_complete_cow_and_has_no_file_or_worker_effect() {
    let (_d, mut s, mut p) = published(true);
    let root = archive_path(&s).join("files");
    let before = ["snapshot.fcproj", "session.json", "native-state.json"].map(|name| (fs::read(root.join(name)).unwrap(), generation(&root.join(name))));
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let result = p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    assert_eq!(result["completeCOWRetained"], true);
    assert_eq!(result["startupPending"], true);
    for flag in ["workerReleased", "fifoEnqueued", "writeAuthority", "startupAuthority", "workersMayResume", "fullApplicationClosure"] {
        assert_eq!(result[flag], false);
    }
    assert!(p.current_release_attempted && p.current_release_ready && p.current_native.is_some());
    let a = p.archive.as_ref().unwrap();
    assert!(a.published.is_none() && a.release.is_some());
    let worker = s.persistence.as_ref().unwrap();
    a.release_preparation().unwrap().original_journal_frame().assert_worker(worker).unwrap();
    worker.current_image_journal(&|| Ok(())).unwrap();
    p.current_native
        .as_ref()
        .unwrap()
        .assert_current_checkpoint_input(&s.project, s.revision, s.path.as_deref(), &worker.data_dir, &worker.session_dir)
        .unwrap();
    p.current(&s).unwrap();
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    assert_eq!(["snapshot.fcproj", "session.json", "native-state.json"].map(|name| (fs::read(root.join(name)).unwrap(), generation(&root.join(name)))), before);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert!(worker.last_sent.is_none());
    assert_eq!(fs::read_dir(&worker.session_dir).unwrap().count(), 1);
    stop(&mut s);
}
#[test]
fn actual_current_release_preparation_requires_real_archive_observation_before_consuming_publisher_or_native_input() {
    let (_d, mut s, mut p) = published(false);
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    assert!(!p.current_release_attempted && !p.current_release_ready && p.current_native.is_none());
    assert!(p.archive.as_ref().unwrap().published.is_some());
    assert!(!p.archive.as_ref().unwrap().release_attempted);
    p.current(&s).unwrap();
    p.begin_publication_observation(&s).unwrap();
    p.confirm_publication_observation(&s).unwrap();
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    stop(&mut s);
}
#[test]
fn actual_current_release_initial_source_refusal_preserves_the_original_unconsumed_preparation() {
    let (_d, mut s, mut p) = published(true);
    assert!(p.prepare_current_release(&s, &|| Err(error("initial original source refused")), &check()).is_err());
    assert!(!p.current_release_attempted);
    assert!(p.current_native.is_none());
    assert!(p.archive.as_ref().unwrap().published.is_some());
    p.current(&s).unwrap();
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    stop(&mut s);
}
#[test]
fn actual_current_release_late_source_refusal_retains_consumed_cow_without_another_capture_or_preparation() {
    let (_d, mut s, mut p) = published(true);
    let calls = Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 { Err(error("source lost after real COW capture")) } else { Ok(()) }
    };
    assert!(p.prepare_current_release(&s, &source, &check()).is_err());
    assert!(p.current_release_attempted && !p.current_release_ready && p.current_native.is_some());
    assert!(p.archive.as_ref().unwrap().published.is_some());
    assert!(p.resolved().is_err());
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn actual_current_release_final_source_refusal_retains_actual_preparation_and_archive_without_replay() {
    let (_d, mut s, mut p) = published(true);
    let calls = Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 { Err(error("source lost after real publisher consumption")) } else { Ok(()) }
    };
    assert!(p.prepare_current_release(&s, &source, &check()).is_err());
    let a = p.archive.as_ref().unwrap();
    assert!(a.published.is_none() && a.release.is_some());
    assert!(p.current_release_attempted && !p.current_release_ready && p.current_native.is_some());
    assert!(p.current(&s).is_err());
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn actual_current_release_refuses_equal_byte_archive_replacement_after_consumption_without_adopting_or_removing_it() {
    let (d, mut s, mut p) = published(true);
    let target = archive_path(&s).join("files/snapshot.fcproj");
    let held = d.0.join("retained-actual-snapshot");
    let original = fs::read(&target).unwrap();
    let calls = Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            fs::rename(&target, &held).unwrap();
            let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target).unwrap();
            out.write_all(&original).unwrap();
            out.sync_all().unwrap();
        }
        Ok(())
    };
    assert!(p.prepare_current_release(&s, &source, &check()).is_err());
    assert!(p.archive.as_ref().unwrap().release.is_some());
    assert!(!p.current_release_ready);
    assert_eq!(fs::read(&held).unwrap(), original);
    assert_eq!(fs::read(&target).unwrap(), original);
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    stop(&mut s);
}
#[test]
fn actual_current_release_archive_preparation_cannot_cross_another_worker_or_survive_original_worker_stop() {
    let (_d, mut s, mut p) = published(true);
    let (_o, mut other, _other) = published(true);
    let a = p.archive.as_mut().unwrap();
    assert!(a.prepare_current_release(other.persistence.as_ref().unwrap(), &|| Ok(())).is_err());
    assert!(!a.release_attempted);
    stop(&mut s);
    assert!(p.prepare_current_release(&s, &|| Ok(()), &check()).is_err());
    assert!(!p.current_release_attempted);
    stop(&mut other);
}
#[test]
fn actual_current_release_preparation_preserves_strict_held_readers_after_the_original_real_prepare_resume_exchange() {
    let (_d, mut s, mut p) = published(true);
    p.prepare_current_release(&s, &|| Ok(()), &check()).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let prepared = p.archive.as_ref().unwrap().release_preparation().unwrap();
    let source = || Ok(());
    let ack = worker
        .release_current_image(|_| prepared.inspect_transition(worker, &source, &|| Ok(())), || prepared.inspect_transition(worker, &source, &|| Ok(())))
        .unwrap();
    prepared.inspect_acknowledged_release(worker, &ack, &source, &|| Ok(())).unwrap();
    assert!(prepared.held_current(worker, &|| Ok(())).is_err());
    assert!(prepared.held_description(worker).is_err());
    assert!(p.current(&s).is_err());
    assert!(p.current_native.is_some());
    stop(&mut s);
}
