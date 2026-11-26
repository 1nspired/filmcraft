use super::*;
use crate::{
    Session,
    autosave::{AutoSavePrefs, AutosaveConfig, session_reservation::ColdJournalReservation},
};
use std::{cell::Cell, fs, os::unix::fs::DirBuilderExt, path::PathBuf, sync::atomic::AtomicU64};

fn fixture(dirty: bool) -> (Scratch, Session) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("original-release-frame-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut session = Session::default();
    session
        .execute(
            "file.newProject",
            json!({"name":"Original release frame","workspace":{"projectId":"p_frame","workspaceId":"11111111-1111-4111-8111-111111111111"}}),
        )
        .unwrap();
    session.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    if dirty {
        session.execute("file.newBin", json!({"name":"Human before original release"})).unwrap();
    }
    let data = root.join("native");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let mut cfg = AutosaveConfig::new(data);
    cfg.journal_debounce = Duration::from_secs(600);
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    session.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    (Scratch(root), session)
}
fn stop(session: &mut Session) {
    session.persistence.as_mut().unwrap().stop_worker();
}

fn frame(session: &Session) -> Arc<Frame> {
    let p = session.persistence.as_ref().unwrap();
    let journal = p.current_image_journal(&|| Ok(())).unwrap();
    Frame::capture_held(p, &journal, &|| Ok(())).unwrap()
}

#[test]
fn release_frame_keeps_actual_constructor_and_lock_through_prepare_resume_without_weakening_original_gated_reader() {
    let (_d, mut s) = fixture(true);
    let p = s.persistence.as_ref().unwrap();
    let f = frame(&s);
    let journal = p.current_image_journal(&|| Ok(())).unwrap();
    assert!(f.same_original_journal(&journal));
    let before = Cell::new(0);
    let after = Cell::new(0);
    let ack = p
        .release_current_image(
            |_| {
                before.set(before.get() + 1);
                f.assert_worker(p).and_then(|_| f.inspect_source(&|| Ok(()))).map_err(|e| e.to_string())
            },
            || {
                after.set(after.get() + 1);
                f.assert_worker(p).and_then(|_| f.inspect_source(&|| Ok(()))).map_err(|e| e.to_string())
            },
        )
        .unwrap();
    assert!(before.get() >= 3 && after.get() >= 2);
    assert!(journal.current(&|| Ok(())).is_err());
    assert!(p.current_image_journal(&|| Ok(())).is_err());
    f.inspect_acknowledged(p, &ack, &|| Ok(())).unwrap();
    stop(&mut s);
}

#[test]
fn release_frame_refuses_late_capture_crossed_original_worker_journal_receipt_and_stopped_owner() {
    let (_d, mut first) = fixture(false);
    let (_e, mut second) = fixture(false);
    let p = first.persistence.as_ref().unwrap();
    let other = second.persistence.as_ref().unwrap();
    let f = frame(&first);
    let other_j = other.current_image_journal(&|| Ok(())).unwrap();
    assert!(Frame::capture_held(p, &other_j, &|| Ok(())).is_err());
    assert!(f.assert_worker(other).is_err());
    assert!(!f.same_original_journal(&other_j));
    let original_j = p.current_image_journal(&|| Ok(())).unwrap();
    let seen = Cell::new(false);
    let ack = p
        .release_current_image(
            |_| {
                if p.startup_attempted.load(Ordering::Acquire) {
                    seen.set(true);
                    assert!(Frame::capture_held(p, &original_j, &|| Ok(())).is_err());
                }
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
    assert!(seen.get());
    assert!(Frame::capture_held(p, &original_j, &|| Ok(())).is_err());
    let other_ack = other.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    assert!(f.inspect_acknowledged(p, &other_ack, &|| Ok(())).is_err());
    assert!(f.inspect_acknowledged(other, &ack, &|| Ok(())).is_err());
    stop(&mut second);
    stop(&mut first);
    assert!(f.assert_worker(first.persistence.as_ref().unwrap()).is_err());
    assert!(f.inspect_source(&|| Ok(())).is_err());
}

#[test]
fn release_frame_retains_original_data_session_and_lock_generations_and_preserves_displaced_work() {
    for which in ["data", "session", "lock"] {
        let (d, mut s) = fixture(false);
        let f = frame(&s);
        let p = s.persistence.as_ref().unwrap();
        let target = match which {
            "data" => p.data_dir.clone(),
            "session" => p.session_dir.clone(),
            _ => p.session_dir.join(LOCK),
        };
        let held = d.0.join(format!("retained-{which}"));
        fs::rename(&target, &held).unwrap();
        if which == "lock" {
            fs::write(&target, b"").unwrap();
        } else {
            fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
            fs::write(target.join("later-human-work"), b"generated later work").unwrap();
        }
        assert!(f.inspect_source(&|| Ok(())).is_err());
        assert!(held.exists());
        assert!(target.exists());
        stop(&mut s);
        assert!(held.exists());
        assert!(target.exists());
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn release_frame_checks_all_retained_prior_recovery_members_without_cleaning_partial_or_unknown_work() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("release-frame-prior-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let d = Scratch(root);
    let cfg = AutosaveConfig::new(&d.0);
    let prior = recovery_root(&d.0).join("100-1");
    fs::create_dir_all(&prior).unwrap();
    let partial = prior.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    fs::write(&partial, b"original retained partial native state").unwrap();
    let original = fs::read(&partial).unwrap();
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    let mut worker = Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap();
    let journal = worker.current_image_journal(&|| Ok(())).unwrap();
    let f = Frame::capture_held(&worker, &journal, &|| Ok(())).unwrap();
    let ack = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    f.inspect_acknowledged(&worker, &ack, &|| Ok(())).unwrap();
    fs::write(prior.join("foreign-later-file"), b"generated unknown later work").unwrap();
    assert!(f.inspect_acknowledged(&worker, &ack, &|| Ok(())).is_err());
    assert_eq!(fs::read(&partial).unwrap(), original);
    assert!(!prior.join(LOCK).exists());
    worker.stop_worker();
    assert!(prior.join("foreign-later-file").is_file());
}

#[test]
fn release_frame_allows_original_fifo_to_write_its_new_session_without_certifying_image_contents_from_a_frame() {
    let (_d, mut s) = fixture(true);
    let native = crate::agent_bridge::capture_native_journal(&mut s).unwrap().unwrap();
    let f = frame(&s);
    let guard = crate::project_save::guard(&s).unwrap();
    let p = s.persistence.as_ref().unwrap();
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let ack = p.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    let input = p
        .enqueue_current_image_input(&ack, s.project.clone(), s.revision, s.path.clone(), native, || {
            guard.current(&s).map_err(|e| e.to_string())?;
            f.inspect_source(&|| Ok(())).map_err(|e| e.to_string())
        })
        .unwrap();
    let flush = p.flush_current_image_pending_journal(&ack, || f.inspect_source(&|| Ok(())).map_err(|e| e.to_string())).unwrap();
    assert_eq!(flush.written_revision(), Some(s.revision));
    p.assert_original_current_image_input(&ack, &input, &s.project, s.revision, s.path.as_deref()).unwrap();
    f.inspect_acknowledged(p, &ack, &|| Ok(())).unwrap();
    assert!(p.current_image_journal(&|| Ok(())).is_err());
    assert_eq!(fs::read(p.session_dir.join(SNAPSHOT)).unwrap(), filmcraft_format::encode(&s.project, false));
    assert!(p.session_dir.join(META).is_file());
    assert!(p.session_dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE).is_file());
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    guard.current(&s).unwrap();
    stop(&mut s);
}

#[test]
fn release_frame_preserves_original_stop_deadline_and_same_worker_paths_without_rebasing() {
    let (_d, mut s) = fixture(false);
    let f = frame(&s);
    let before = s.persistence.as_ref().unwrap().session_dir.clone();
    for message in ["original Stop", "original deadline"] {
        assert!(f.inspect_source(&|| Err(io::Error::other(message))).unwrap_err().to_string().contains(message));
    }
    s.persistence.as_mut().unwrap().session_dir = before.join("foreign");
    assert!(f.assert_worker(s.persistence.as_ref().unwrap()).is_err());
    s.persistence.as_mut().unwrap().session_dir = before;
    f.assert_worker(s.persistence.as_ref().unwrap()).unwrap();
    f.inspect_source(&|| Ok(())).unwrap();
    assert!(!s.persistence.as_ref().unwrap().startup_attempted.load(Ordering::Acquire));
    stop(&mut s);
}
