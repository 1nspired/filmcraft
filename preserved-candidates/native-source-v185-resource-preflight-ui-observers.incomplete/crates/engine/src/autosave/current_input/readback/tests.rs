//! Real original archive, worker release, FIFO serializers and complete files.
//! Source callbacks are generated component fences, not a Cold application grant.
use super::*;
use crate::autosave::held_checkpoint::publication::Source;
use crate::{
    Session,
    autosave::{AutoSavePrefs, AutosaveConfig, session_reservation::ColdJournalReservation},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    session: Session,
    prepared: ReleasePreparation,
    release: CurrentImageRelease,
    input: OriginalCurrentImageInput,
    flush: OriginalJournalFlush,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(p) = self.session.persistence.as_mut() {
            p.stop_worker();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn source() -> Source {
    Arc::new(|| Ok(()))
}
fn fixture(enabled: bool, already_written: bool) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::temp_dir().join(format!("original-current-image-readback-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut session = Session::default();
    let scope = json!({"projectId":"p_readback","workspaceId":"11111111-1111-4111-8111-111111111111"});
    session.execute("file.newProject", json!({"name":"Original FIFO readback","workspace":scope})).unwrap();
    session.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    let owner = crate::session_worker::SessionWorker::new().unwrap();
    owner.install(&mut session).unwrap();
    session.agent_operation_control().enable_transport();
    let token = "a".repeat(64);
    let before = session.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":session.path})).unwrap();
    session.execute("agent.apply",json!({"token":token,"label":"Owned before readback","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope,"beforeSha256":before["binState"]["sha256"],"edit":{"type":"create","name":"Owned bin","parent":null}}}]})).unwrap();
    session.execute("file.newBin", json!({"name":"Human before readback"})).unwrap();
    session.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_991);
    session.prefs.auto_save.recovery_journal = enabled;
    let data = root.join("native");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let mut cfg = AutosaveConfig::new(&data);
    cfg.journal_debounce = Duration::from_secs(600);
    let mut prefs = AutoSavePrefs::default();
    prefs.recovery_journal = enabled;
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    session.persistence = Some(Persistence::start_cold_reserved(&cfg, prefs, reservation, &|| Ok(())).unwrap());
    let native = crate::agent_bridge::capture_native_journal(&mut session).unwrap().unwrap();
    let p = session.persistence.as_ref().unwrap();
    let encoded = p.encode_held_current_checkpoint(session.project.clone(), session.revision, session.path.clone(), native, || Ok(())).unwrap();
    let root_frame = DirectoryFrame::capture(&data.join(".poema-checkpoint-abcdef0123456789abcdef0123456789"), &|| Ok(())).unwrap();
    let published = p.publish_held_current_checkpoint(encoded, "abcdef0123456789abcdef0123456789", root_frame, 1024 * 1024, source(), || Ok(())).unwrap();
    let prepared = published.prepare_release(p, &|| Ok(())).unwrap();
    let next_native = crate::agent_bridge::capture_native_journal(&mut session).unwrap().unwrap();
    let p = session.persistence.as_ref().unwrap();
    let release = p
        .release_current_image(
            |_| prepared.inspect_transition(p, source().as_ref(), &|| Ok(())),
            || prepared.inspect_transition(p, source().as_ref(), &|| Ok(())),
        )
        .unwrap();
    let input = p
        .enqueue_current_image_input(&release, session.project.clone(), session.revision, session.path.clone(), next_native, || {
            prepared.original_journal_frame().inspect_source(&|| Ok(())).map_err(error)
        })
        .unwrap();
    if already_written {
        p.flush();
    }
    let flush = p.flush_current_image_pending_journal(&release, || prepared.original_journal_frame().inspect_source(&|| Ok(())).map_err(error)).unwrap();
    Fixture { root, session, prepared, release, input, flush }
}
fn verify(f: &Fixture) -> Result<CompleteCurrentImage, String> {
    f.session.persistence.as_ref().unwrap().read_current_image(&f.release, &f.input, &f.flush, &f.prepared, source().as_ref(), &|| Ok(()))
}
fn current(f: &Fixture, image: &CompleteCurrentImage) -> Result<(), String> {
    image.current(f.session.persistence.as_ref().unwrap(), &f.release, &f.input, &f.flush, &f.prepared, source().as_ref(), &|| Ok(()))
}
fn put(path: &std::path::Path, bytes: &[u8], mode: u32) {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn generation(p: &std::path::Path) -> (u64, u64, u32, u64, i64, i64) {
    let m = fs::symlink_metadata(p).unwrap();
    (m.dev(), m.ino(), m.mode(), m.len(), m.mtime(), m.ctime())
}

#[test]
fn complete_current_image_reads_original_fifo_snapshot_metadata_owned_human_history_and_full_auxiliary_without_rewriting() {
    let f = fixture(true, false);
    let p = f.session.persistence.as_ref().unwrap();
    assert_eq!(f.flush.written_revision(), Some(f.session.revision));
    let saved = fs::read(f.session.path.as_ref().unwrap()).unwrap();
    let before = [SNAPSHOT, META, NATIVE].map(|name| generation(&p.session_dir.join(name)));
    let guard = crate::project_save::guard(&f.session).unwrap();
    let image = verify(&f).unwrap();
    current(&f, &image).unwrap();
    assert_eq!([SNAPSHOT, META, NATIVE].map(|name| generation(&p.session_dir.join(name))), before);
    let native: Value = serde_json::from_slice(&fs::read(p.session_dir.join(NATIVE)).unwrap()).unwrap();
    assert_eq!(native["payload"]["history"]["undo"].as_array().unwrap().len(), 2);
    assert_eq!(native["payload"]["history"]["tickets"].as_array().unwrap().len(), 1);
    assert!(native["payload"]["editor"]["auxiliary"].is_object());
    assert_eq!(native["payload"]["editor"]["editor"]["source_playhead"], serde_json::to_value(f.session.state.source_playhead).unwrap());
    assert_eq!(fs::read(f.session.path.as_ref().unwrap()).unwrap(), saved);
    guard.current(&f.session).unwrap();
    assert!(verify(&f).is_err());
    assert!(p.last_sent.is_none());
    assert!(p.current_image_journal(&|| Ok(())).is_err());
}

#[test]
fn complete_current_image_accepts_none_flush_only_when_original_files_already_finished_and_fully_match() {
    let f = fixture(true, true);
    assert_eq!(f.flush.written_revision(), None);
    let p = f.session.persistence.as_ref().unwrap();
    let before = [SNAPSHOT, META, NATIVE].map(|name| generation(&p.session_dir.join(name)));
    let image = verify(&f).unwrap();
    current(&f, &image).unwrap();
    assert_eq!([SNAPSHOT, META, NATIVE].map(|name| generation(&p.session_dir.join(name))), before);
}

#[test]
fn complete_current_image_refuses_disabled_recovery_without_enabling_it_or_certifying_none_as_an_image() {
    let f = fixture(false, false);
    assert_eq!(f.flush.written_revision(), None);
    assert!(!f.session.prefs.auto_save.recovery_journal);
    assert!(verify(&f).is_err());
    assert!(f.flush.current_readback_started());
    assert!(verify(&f).is_err());
    let p = f.session.persistence.as_ref().unwrap();
    assert!(!p.session_dir.join(SNAPSHOT).exists());
    assert!(!p.session_dir.join(META).exists());
    assert!(!p.session_dir.join(NATIVE).exists());
    assert!(!f.session.prefs.auto_save.recovery_journal);
}

#[test]
fn complete_current_image_refuses_missing_partial_unknown_and_directory_members_without_rebasing_after_failure() {
    for kind in ["missing", "unknown", "directory"] {
        let f = fixture(true, false);
        let p = f.session.persistence.as_ref().unwrap();
        let metadata = p.session_dir.join(META);
        let held = f.root.join("retained-metadata");
        fs::rename(&metadata, &held).unwrap();
        match kind {
            "unknown" => {
                fs::rename(&held, &metadata).unwrap();
                put(&p.session_dir.join("foreign-later-work"), b"generated later work", 0o600);
            }
            "directory" => {
                fs::create_dir(&metadata).unwrap();
            }
            _ => {}
        }
        assert!(verify(&f).is_err());
        assert!(f.flush.current_readback_started());
        if kind == "missing" {
            fs::rename(&held, &metadata).unwrap();
        }
        assert!(verify(&f).is_err());
        if kind == "unknown" {
            assert!(p.session_dir.join("foreign-later-work").is_file());
        }
    }
}

#[test]
fn complete_current_image_retains_first_file_generations_and_refuses_equal_bytes_replaced_after_success() {
    for name in [SNAPSHOT, META, NATIVE] {
        let f = fixture(true, false);
        let image = verify(&f).unwrap();
        let p = f.session.persistence.as_ref().unwrap();
        let target = p.session_dir.join(name);
        let held = f.root.join("retained-original-file");
        let bytes = fs::read(&target).unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        fs::rename(&target, &held).unwrap();
        put(&target, &bytes, mode);
        assert!(current(&f, &image).is_err());
        assert!(verify(&f).is_err());
        assert_eq!(fs::read(&held).unwrap(), bytes);
        assert_eq!(fs::read(&target).unwrap(), bytes);
    }
}

#[test]
fn complete_current_image_cannot_cross_actual_worker_receipts_original_archive_or_enqueued_cow_input() {
    let mut first = fixture(true, false);
    let second = fixture(true, false);
    let p = first.session.persistence.as_ref().unwrap();
    assert!(p.read_current_image(&second.release, &first.input, &first.flush, &first.prepared, source().as_ref(), &|| Ok(())).is_err());
    assert!(p.read_current_image(&first.release, &second.input, &first.flush, &first.prepared, source().as_ref(), &|| Ok(())).is_err());
    assert!(p.read_current_image(&first.release, &first.input, &second.flush, &first.prepared, source().as_ref(), &|| Ok(())).is_err());
    assert!(!first.flush.current_readback_started());
    let image = verify(&first).unwrap();
    assert!(image.current(p, &first.release, &first.input, &first.flush, &second.prepared, source().as_ref(), &|| Ok(())).is_err());
    first.input.project = Arc::new((*first.input.project).clone());
    assert!(current(&first, &image).is_err());
}

#[test]
fn complete_current_image_keeps_original_metadata_recipe_and_all_fields_except_the_new_write_timestamp() {
    for kind in ["timestamp", "revision", "pid", "path", "clean-exit"] {
        let f = fixture(true, false);
        let target = f.session.persistence.as_ref().unwrap().session_dir.join(META);
        let mut value: JournalMeta = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        match kind {
            "timestamp" => value.saved_unix = value.saved_unix.saturating_add(1),
            "revision" => value.revision = value.revision.saturating_add(1),
            "pid" => value.pid = value.pid.saturating_add(1),
            "path" => value.project_path = Some("/generated/foreign.fcproj".into()),
            _ => value.clean_exit = true,
        };
        fs::write(&target, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert_eq!(verify(&f).is_ok(), kind == "timestamp");
    }
}

struct MutatingSource {
    target: PathBuf,
    held: PathBuf,
    fired: AtomicBool,
}
impl CheckpointSource for MutatingSource {
    fn current(&self) -> io::Result<()> {
        Ok(())
    }
    fn fence_after_read(&self) -> io::Result<()> {
        if !self.fired.swap(true, Ordering::SeqCst) {
            let bytes = fs::read(&self.target)?;
            let mode = fs::metadata(&self.target)?.permissions().mode() & 0o7777;
            fs::rename(&self.target, &self.held)?;
            put(&self.target, &bytes, mode);
        }
        Ok(())
    }
}
#[test]
fn complete_current_image_fences_files_after_source_callbacks_and_keeps_failed_readback_consumed_under_stop() {
    let f = fixture(true, false);
    let p = f.session.persistence.as_ref().unwrap();
    let changing = MutatingSource { target: p.session_dir.join(META), held: f.root.join("retained-before-source-fence"), fired: AtomicBool::new(false) };
    assert!(p.read_current_image(&f.release, &f.input, &f.flush, &f.prepared, &changing, &|| Ok(())).is_err());
    assert!(changing.fired.load(Ordering::SeqCst));
    assert!(changing.held.exists());
    assert!(verify(&f).is_err());
    let stopped = fixture(true, false);
    let calls = AtomicU64::new(0);
    let guard = || {
        if calls.fetch_add(1, Ordering::SeqCst) >= 2 { Err(io::Error::other("original readback stopped")) } else { Ok(()) }
    };
    assert!(
        stopped
            .session
            .persistence
            .as_ref()
            .unwrap()
            .read_current_image(&stopped.release, &stopped.input, &stopped.flush, &stopped.prepared, source().as_ref(), &guard)
            .is_err()
    );
    assert!(stopped.flush.current_readback_started());
    assert!(verify(&stopped).is_err());
}

#[test]
fn complete_current_image_checks_existing_stop_and_deadline_before_any_external_source_callback() {
    struct CountSource(AtomicU64);
    impl CheckpointSource for CountSource {
        fn current(&self) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn fence_after_read(&self) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let f = fixture(true, false);
    let image = verify(&f).unwrap();
    for reason in ["original Stop", "original deadline"] {
        let source = CountSource(AtomicU64::new(0));
        let guard = || Err(io::Error::other(reason));
        let p = f.session.persistence.as_ref().unwrap();
        assert!(image.current(p, &f.release, &f.input, &f.flush, &f.prepared, &source, &guard).unwrap_err().contains(reason));
        assert_eq!(source.0.load(Ordering::SeqCst), 0);
    }
    current(&f, &image).unwrap();
    let initial = fixture(true, false);
    let source = CountSource(AtomicU64::new(0));
    assert!(
        initial
            .session
            .persistence
            .as_ref()
            .unwrap()
            .read_current_image(&initial.release, &initial.input, &initial.flush, &initial.prepared, &source, &|| Err(io::Error::other("already stopped")))
            .is_err()
    );
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert!(!initial.flush.current_readback_started());
}
