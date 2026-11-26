use super::*;
use crate::{
    Session,
    autosave::{AutoSavePrefs, AutosaveConfig},
};
use std::{
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn persistence() -> (Persistence, std::path::PathBuf) {
    let root = crate::temp_dir().join(format!("poema-save-pause-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&root).unwrap();
    let p = Persistence::start(&AutosaveConfig::new(&root), AutoSavePrefs::default()).unwrap();
    (p, root)
}
fn fence(p: &Persistence) {
    let (tx, rx) = channel();
    p.tx.send(Msg::Flush(tx)).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
}
fn dirty(p: &Persistence, path: Option<String>) {
    p.send_dirty_native(Session::default().project, 1, path, Ok(None));
    fence(p);
}
#[test]
fn save_pause_keeps_clean_preferences_flush_and_explicit_autosave_in_original_fifo_until_exact_release() {
    let (p, _) = persistence();
    dirty(&p, None);
    let journal = p.session_dir.join("snapshot.fcproj");
    let bytes = fs::read(&journal).unwrap();
    let held = p.pause_project_save(|| Ok(())).unwrap();
    p.send_clean();
    let mut prefs = AutoSavePrefs::default();
    prefs.recovery_journal = false;
    p.set_prefs(prefs);
    let (tx, flushed) = channel();
    p.tx.send(Msg::Flush(tx)).unwrap();
    let (tx, autosaved) = channel();
    p.tx.send(Msg::AutoSaveNow(tx)).unwrap();
    assert!(matches!(flushed.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    assert!(matches!(autosaved.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    assert_eq!(fs::read(&journal).unwrap(), bytes);
    held.finish(|| Ok(())).unwrap();
    flushed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(autosaved.recv_timeout(Duration::from_secs(2)).unwrap().unwrap_err().contains("settling"));
    assert!(!journal.exists());
}
#[test]
fn save_pause_rejects_another_owner_without_releasing_the_first_and_can_be_reacquired_after_finish() {
    let (p, _) = persistence();
    let held = p.pause_project_save(|| Ok(())).unwrap();
    assert!(p.pause_project_save(|| Ok(())).is_err());
    held.current(&p).unwrap();
    let (tx, rx) = channel();
    p.tx.send(Msg::Flush(tx)).unwrap();
    assert!(matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    held.finish(|| Ok(())).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    p.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
#[test]
fn save_pause_stop_before_queue_and_before_ack_cannot_leave_a_late_pause_stranded_or_replay_work() {
    for after_queue in [false, true] {
        let (p, _) = persistence();
        let mut calls = 0;
        let result = p.pause_project_save(|| {
            calls += 1;
            if !after_queue || calls >= 2 { Err("generated Stop or original deadline".into()) } else { Ok(()) }
        });
        assert!(result.is_err());
        fence(&p);
        p.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    }
}
#[test]
fn save_pause_release_preserves_an_independent_workspace_update_pause() {
    let (p, _) = persistence();
    dirty(&p, None);
    p.pause_workspace(|| Ok(())).unwrap();
    p.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    assert!(p.auto_save_now().unwrap_err().contains("workspace update"));
    p.resume_workspace().unwrap();
    assert!(p.auto_save_now().unwrap().is_some());
}
#[test]
fn save_resume_marker_prevents_an_expired_autosave_timer_from_overwriting_newly_published_bytes_with_queued_old_state() {
    let (p, root) = persistence();
    let target = root.join("Current.fcproj");
    dirty(&p, Some(target.to_str().unwrap().into()));
    let held = p.pause_project_save(|| Ok(())).unwrap();
    fs::write(&target, b"new complete native publication must survive").unwrap();
    // Expired timer synthetic: direct worker preferences, not a product setting.
    let mut prefs = AutoSavePrefs::default();
    prefs.interval_minutes = 0;
    prefs.save_current_project = true;
    p.set_prefs(prefs);
    p.send_clean();
    held.finish(|| Ok(())).unwrap();
    p.set_prefs(AutoSavePrefs::default());
    fence(&p);
    assert_eq!(fs::read(&target).unwrap(), b"new complete native publication must survive");
}
#[test]
fn save_pause_dropped_guard_and_persistence_shutdown_release_only_their_original_worker() {
    let (p, _) = persistence();
    let held = p.pause_project_save(|| Ok(())).unwrap();
    drop(held);
    fence(&p);
    let held = p.pause_project_save(|| Ok(())).unwrap();
    drop(p);
    assert!(!held.active.as_ref().unwrap().load(Ordering::Acquire));
    let (other, _) = persistence();
    assert!(held.current(&other).is_err());
    drop(held);
    other.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}

#[test]
fn original_save_owned_resume_keeps_the_same_acknowledgement_after_Stop_without_requeueing_or_releasing_another_owner() {
    for fail_at in [1, 2] {
        let (p, _root) = persistence();
        let mut held = p.pause_project_save(|| Ok(())).unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert!(
            held.finish_owned(|| if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 == fail_at {
                Err("generated Stop observing owned resume".into())
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(held.same_owner(&p));
        assert!(p.pause_project_save(|| Ok(())).is_err());
        held.finish_owned(|| Ok(())).unwrap();
        assert!(held.resume_confirmed);
        held.finish_owned(|| Ok(())).unwrap();
        drop(held);
        p.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
        p.close();
    }
}

#[test]
fn autosave_observer_ends_at_original_resume_and_does_not_accept_later_pause_or_shutdown() {
    let (p, _) = persistence();
    let held = p.pause_project_save(|| Ok(())).unwrap();
    let probe = held.probe().unwrap();
    probe.current().unwrap();
    held.finish(|| Ok(())).unwrap();
    assert!(probe.current().is_err());
    let next = p.pause_project_save(|| Ok(())).unwrap();
    let next_probe = next.probe().unwrap();
    next_probe.current().unwrap();
    assert!(probe.current().is_err());
    drop(p);
    assert!(next_probe.current().is_err());
    drop(next);
}
