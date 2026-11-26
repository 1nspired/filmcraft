//! Actual original canonical publisher, complete checkpoint and worker exchange.
//! This is component source custody, not a fabricated Cold Input or application.
use super::*;
use crate::autosave::held_checkpoint::publication::{CheckpointSource, Source, release::ReleasePreparation};
use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    root: PathBuf,
    session: Session,
    commit: Arc<ColdCommit>,
    checkpoint: ReleasePreparation,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(p) = self.session.persistence.as_mut() {
            p.stop_worker();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn fixture() -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("canonical-release-source-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let data = root.join("native");
    fs::create_dir(&data).unwrap();
    let mut session = Session::default();
    session
        .execute(
            "file.newProject",
            json!({"name":"Original canonical release","workspace":{"projectId":"p_release","workspaceId":"11111111-1111-4111-8111-111111111111"}}),
        )
        .unwrap();
    session.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    crate::agent_bridge::native_auxiliary_tests::populate(&mut session);
    let cfg = AutosaveConfig::new(&data);
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    session.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    let mut commit = ColdCommit::prepare(&session, DirectoryFrame::capture(&data, &|| Ok(())).unwrap(), &"b".repeat(64), &|| Ok(())).unwrap();
    assert_eq!(commit.publish(&session, &|| Ok(())).unwrap()["published"], true);
    assert_eq!(commit.bind_services(&mut session, &|| Ok(())).unwrap()["bound"], true);
    let commit = Arc::new(commit);
    let native = crate::agent_bridge::capture_native_journal(&mut session).unwrap().unwrap();
    let worker = session.persistence.as_ref().unwrap();
    let encoded = worker
        .encode_held_current_checkpoint(session.project.clone(), session.revision, session.path.clone(), native, || {
            commit.verify_published(&session, &|| Ok(())).map_err(|e| e.to_string())
        })
        .unwrap();
    let old = commit.clone();
    let source: Source = Arc::new(move || old.verify_published_source(&|| Ok(())).map_err(|e| io::Error::other(e.to_string())));
    let target = DirectoryFrame::capture(&data.join(".poema-checkpoint-abcdef0123456789abcdef0123456789"), &|| Ok(())).unwrap();
    let published = worker
        .publish_held_current_checkpoint(encoded, "abcdef0123456789abcdef0123456789", target, 1024 * 1024, source, || {
            commit.verify_published(&session, &|| Ok(())).map_err(|e| e.to_string())
        })
        .unwrap();
    let checkpoint = published.prepare_release(worker, &|| Ok(())).unwrap();
    Fixture { root, session, commit, checkpoint }
}
struct OriginalSource {
    commit: Arc<ColdCommit>,
    frame: Arc<Frame>,
}
impl CheckpointSource for OriginalSource {
    fn current(&self) -> io::Result<()> {
        self.commit.inspect_release_source(&self.frame, &|| Ok(())).map_err(|e| io::Error::other(e.to_string()))
    }
    fn fence_after_read(&self) -> io::Result<()> {
        self.current()
    }
}
fn source(f: &Fixture) -> OriginalSource {
    OriginalSource { commit: f.commit.clone(), frame: f.checkpoint.original_journal_frame().clone() }
}
fn release(f: &Fixture) -> crate::autosave::CurrentImageRelease {
    let p = f.session.persistence.as_ref().unwrap();
    let source = source(f);
    p.release_current_image(
        |_| {
            f.commit.inspect_release_memory(&f.session, &source.frame, &|| Ok(()), &|| Ok(())).map_err(|e| e.to_string())?;
            f.checkpoint.inspect_transition(p, &source, &|| Ok(()))
        },
        || {
            f.commit.inspect_release_memory(&f.session, &source.frame, &|| Ok(()), &|| Ok(())).map_err(|e| e.to_string())?;
            f.checkpoint.inspect_transition(p, &source, &|| Ok(()))
        },
    )
    .unwrap()
}

#[test]
fn canonical_release_source_keeps_original_four_files_memory_and_actual_constructor_across_prepare_resume() {
    let f = fixture();
    let original = payloads(&f.session, true).unwrap();
    let guard = crate::project_save::guard(&f.session).unwrap();
    let ack = release(&f);
    let p = f.session.persistence.as_ref().unwrap();
    let source = source(&f);
    assert!(f.commit.verify_published_source(&|| Ok(())).is_err());
    assert!(f.commit.verify_published(&f.session, &|| Ok(())).is_err());
    assert!(p.current_image_journal(&|| Ok(())).is_err());
    f.commit.inspect_release_source(&source.frame, &|| Ok(())).unwrap();
    f.commit.inspect_release_memory(&f.session, &source.frame, &|| Ok(()), &|| Ok(())).unwrap();
    f.checkpoint.inspect_acknowledged_release(p, &ack, &source, &|| Ok(())).unwrap();
    for (name, bytes, _) in original {
        assert_eq!(fs::read(p.data_dir.join(name)).unwrap(), bytes);
    }
    guard.current(&f.session).unwrap();
    assert!(p.last_sent.is_none());
}

#[test]
fn canonical_release_source_refuses_another_actual_constructor_even_when_both_workers_have_real_release_acks() {
    let first = fixture();
    let second = fixture();
    let _a = release(&first);
    let _b = release(&second);
    let frame = second.checkpoint.original_journal_frame();
    assert!(first.commit.inspect_release_source(frame, &|| Ok(())).is_err());
    assert!(first.commit.inspect_release_memory(&first.session, frame, &|| Ok(()), &|| Ok(())).is_err());
    assert!(frame.assert_worker(first.session.persistence.as_ref().unwrap()).is_err());
}

#[test]
fn canonical_release_source_does_not_adopt_equal_byte_replacements_of_any_original_service_file() {
    for relative in ["preferences.json", "effect-presets.json", "export-presets.json", "Keyboard Shortcuts/active.json"] {
        let f = fixture();
        let _ack = release(&f);
        let p = f.session.persistence.as_ref().unwrap();
        let path = p.data_dir.join(relative);
        let bytes = fs::read(&path).unwrap();
        let held = f.root.join("retained-original-service");
        fs::rename(&path, &held).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(f.commit.inspect_release_source(f.checkpoint.original_journal_frame(), &|| Ok(())).is_err());
        assert_eq!(fs::read(&held).unwrap(), bytes);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn canonical_release_memory_keeps_original_project_services_and_preferences_after_worker_ack() {
    for what in ["graph", "preferences", "service-path"] {
        let mut f = fixture();
        let _ack = release(&f);
        match what {
            "graph" => f.session.project = Arc::new((*f.session.project).clone()),
            "preferences" => f.session.prefs.auto_save.enabled = !f.session.prefs.auto_save.enabled,
            _ => f.session.prefs_path = Some(f.root.join("foreign-preferences.json")),
        }
        assert!(f.commit.inspect_release_memory(&f.session, f.checkpoint.original_journal_frame(), &|| Ok(()), &|| Ok(())).is_err());
    }
}

#[test]
fn canonical_release_memory_keeps_final_original_source_fence_even_when_full_canonical_read_fails() {
    let f = fixture();
    let _ack = release(&f);
    let calls = Cell::new(0);
    let full = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    f.commit.inspect_release_memory(&f.session, f.checkpoint.original_journal_frame(), &full, &|| Ok(())).unwrap();
    assert_eq!(calls.get(), 2);
    fs::write(f.session.persistence.as_ref().unwrap().data_dir.join("effect-presets.json"), b"later human library").unwrap();
    calls.set(0);
    assert!(f.commit.inspect_release_memory(&f.session, f.checkpoint.original_journal_frame(), &full, &|| Ok(())).is_err());
    assert_eq!(calls.get(), 2);
}

#[test]
fn canonical_release_source_retains_prior_namespace_and_original_stop_deadline_without_publication_replay() {
    let f = fixture();
    let _ack = release(&f);
    let frame = f.checkpoint.original_journal_frame();
    for message in ["original Stop", "original deadline"] {
        assert!(f.commit.inspect_release_source(frame, &|| Err(io::Error::other(message))).unwrap_err().to_string().contains(message));
    }
    let recovery = crate::autosave::recovery_root(&f.session.persistence.as_ref().unwrap().data_dir);
    fs::create_dir(recovery.join("unknown-later-session")).unwrap();
    assert!(f.commit.inspect_release_source(frame, &|| Ok(())).is_err());
    assert!(recovery.join("unknown-later-session").is_dir());
}

#[test]
fn canonical_release_source_refuses_an_unbound_or_unpublished_original_owner_without_creating_authority() {
    for published in [false, true] {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = crate::temp_dir().join(format!("canonical-release-unbound-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let data = root.join("native");
        fs::create_dir(&data).unwrap();
        let mut session = Session::default();
        session
            .execute(
                "file.newProject",
                json!({"name":"Unbound original canonical owner","workspace":{"projectId":"p_release","workspaceId":"11111111-1111-4111-8111-111111111111"}}),
            )
            .unwrap();
        session.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
        let cfg = AutosaveConfig::new(&data);
        let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
        session.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
        let mut commit = ColdCommit::prepare(&session, DirectoryFrame::capture(&data, &|| Ok(())).unwrap(), &"c".repeat(64), &|| Ok(())).unwrap();
        if published {
            assert_eq!(commit.publish(&session, &|| Ok(())).unwrap()["published"], true);
        }
        let native = crate::agent_bridge::capture_native_journal(&mut session).unwrap().unwrap();
        let worker = session.persistence.as_ref().unwrap();
        let encoded = worker.encode_held_current_checkpoint(session.project.clone(), session.revision, session.path.clone(), native, || Ok(())).unwrap();
        let target = DirectoryFrame::capture(&data.join(".poema-checkpoint-abcdef0123456789abcdef0123456789"), &|| Ok(())).unwrap();
        // Real archive/constructor custody with a synthetic source callback is
        // insufficient to turn this SAME unpublished/unbound Commit into one.
        let checkpoint = worker
            .publish_held_current_checkpoint(encoded, "abcdef0123456789abcdef0123456789", target, 1024 * 1024, Arc::new(|| Ok(())), || Ok(()))
            .unwrap()
            .prepare_release(worker, &|| Ok(()))
            .unwrap();
        let frame = checkpoint.original_journal_frame();
        assert!(frame.same_original_journal(&commit.journal));
        assert!(commit.service_binding_description().is_none());
        assert!(commit.inspect_release_source(frame, &|| Ok(())).is_err());
        assert!(commit.inspect_release_memory(&session, frame, &|| Ok(()), &|| Ok(())).is_err());
        session.persistence.as_mut().unwrap().stop_worker();
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn canonical_release_source_and_archive_inspection_invalidate_when_the_same_original_worker_stops() {
    let mut f = fixture();
    let ack = release(&f);
    let source = source(&f);
    f.session.persistence.as_mut().unwrap().stop_worker();
    let p = f.session.persistence.as_ref().unwrap();
    assert!(f.commit.inspect_release_source(&source.frame, &|| Ok(())).is_err());
    assert!(f.commit.inspect_release_memory(&f.session, &source.frame, &|| Ok(()), &|| Ok(())).is_err());
    assert!(f.checkpoint.inspect_acknowledged_release(p, &ack, &source, &|| Ok(())).is_err());
}

#[test]
fn canonical_release_final_fence_reuses_original_files_without_rewriting_or_adopting_equal_byte_replacements() {
    use std::os::unix::fs::MetadataExt;
    for relative in ["preferences.json", "effect-presets.json", "export-presets.json", "Keyboard Shortcuts/active.json"] {
        let f = fixture();
        let frame = f.checkpoint.original_journal_frame();
        f.commit.fence_release_source_after_read(frame, &|| Ok(())).unwrap();
        let ack = release(&f);
        let p = f.session.persistence.as_ref().unwrap();
        let path = p.data_dir.join(relative);
        let before = fs::metadata(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        f.commit.inspect_release_source(frame, &|| Ok(())).unwrap();
        f.commit.fence_release_source_after_read(frame, &|| Ok(())).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.len(), before.mtime_nsec(), before.ctime_nsec()),
            (after.dev(), after.ino(), after.len(), after.mtime_nsec(), after.ctime_nsec())
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(f.commit.fence_published_source_after_read(&|| Ok(())).is_err());
        p.assert_current_image_release(&ack).unwrap();
        let held = f.root.join("retained-original-final-fence");
        fs::rename(&path, &held).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(f.commit.fence_release_source_after_read(frame, &|| Ok(())).is_err());
        assert_eq!(fs::read(&held).unwrap(), bytes);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn canonical_release_final_fence_keeps_actual_constructor_stop_deadline_and_worker_liveness() {
    let mut first = fixture();
    let second = fixture();
    let _ack = release(&first);
    let _other = release(&second);
    assert!(first.commit.fence_release_source_after_read(second.checkpoint.original_journal_frame(), &|| Ok(())).is_err());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stopped = crate::agent_bridge::media_guard::Check::with_stop(web_time::Instant::now(), stop);
    let expired = crate::agent_bridge::media_guard::Check::new(web_time::Instant::now() - std::time::Duration::from_secs(46));
    for check in [&stopped, &expired] {
        assert!(
            first
                .commit
                .fence_release_source_after_read(first.checkpoint.original_journal_frame(), &|| check.check().map_err(|e| io::Error::other(e.to_string())))
                .is_err()
        );
    }
    first.session.persistence.as_mut().unwrap().stop_worker();
    assert!(first.commit.fence_release_source_after_read(first.checkpoint.original_journal_frame(), &|| Ok(())).is_err());
}
