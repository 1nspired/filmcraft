use super::*;
use preview_context::{DirectoryFrame, NodeFrame};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
struct Fixture {
    root: PathBuf,
    target: NodeFrame,
    archive: Arc<DirectoryFrame>,
}
fn private(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}
fn operation() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed))
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!("poema-private-external-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        private(&root);
        let root = fs::canonicalize(root).unwrap();
        private(&root.join("session"));
        private(&root.join("archive"));
        let target = DirectoryFrame::capture(&root.join("session"), &|| Ok(())).unwrap().node("native-state.json", &|| Ok(())).unwrap();
        let archive = DirectoryFrame::capture(&root.join("archive"), &|| Ok(())).unwrap();
        Self { root, target, archive }
    }
    fn stage(&self, id: &str, data: &[u8]) -> StagedWrite {
        stage_private_from_frames(&self.target, &self.archive, id, data, 1024 * 1024, &|| Ok(())).unwrap()
    }
    fn move_archive_parent(&self, id: &str) {
        let original = self.root.join("archive");
        let held = self.root.join("held-archive");
        fs::rename(&original, &held).unwrap();
        private(&original);
        let name = format!(".poema-save-{id}.incomplete");
        fs::rename(held.join(&name), original.join(name)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn private_external_archive_uses_original_publisher_and_keeps_private_independent_bytes_outside_the_live_session() {
    let f = Fixture::new();
    let id = operation();
    let staged = f.stage(&id, b"complete original native bytes");
    let intent = staged.intent().clone();
    assert_eq!(staged.directory().parent(), Some(f.root.join("archive").as_path()));
    let p = staged.publish(&|| Ok(()));
    assert!(p.receipt_durable);
    assert_eq!(p.receipt.outcome, Outcome::Published);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"complete original native bytes");
    assert_eq!(fs::metadata(f.target.path()).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(p.directory.join(PAYLOAD)).unwrap().mode() & 0o777, 0o600);
    assert_ne!(fs::metadata(f.target.path()).unwrap().ino(), fs::metadata(p.directory.join(PAYLOAD)).unwrap().ino());
    assert_eq!(fs::read_dir(f.root.join("session")).unwrap().count(), 1);
    p.controls_current(&|| Ok(())).unwrap();
    assert_eq!(inspect_publication_after_directory_seals(&p, &intent, &[], &|| Ok(())).unwrap().current, CurrentState::Published);
    assert_eq!(inspect_expected(&p.directory, &intent, &|| Ok(())).unwrap().current, CurrentState::Published);
    fs::write(f.target.path(), b"later ordinary autosave").unwrap();
    assert_eq!(fs::read(p.directory.join(PAYLOAD)).unwrap(), b"complete original native bytes");
}
#[test]
fn private_external_archive_never_adopts_an_existing_or_later_appearing_target_before_staging() {
    for captured_existing in [false, true] {
        let f = Fixture::new();
        fs::write(f.target.path(), b"human bytes").unwrap();
        let existing = DirectoryFrame::capture(&f.root.join("session"), &|| Ok(())).unwrap().node("native-state.json", &|| Ok(())).unwrap();
        let target = if captured_existing { &existing } else { &f.target };
        assert!(stage_private_from_frames(target, &f.archive, &operation(), b"new", 0, &|| Ok(())).is_err());
        assert_eq!(fs::read(f.target.path()).unwrap(), b"human bytes");
        assert_eq!(fs::read_dir(f.root.join("archive")).unwrap().count(), 0);
    }
}
#[test]
fn private_external_archive_refuses_target_appearance_after_stage_and_preserves_original_archive_without_overwrite() {
    let f = Fixture::new();
    let staged = f.stage(&operation(), b"original archive");
    let archive = staged.directory().join(PAYLOAD);
    fs::write(f.target.path(), b"later human").unwrap();
    let p = staged.publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::NotPublished);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"later human");
    assert_eq!(fs::read(archive).unwrap(), b"original archive");
}
#[test]
fn private_external_archive_keeps_its_original_parent_identity_when_the_operation_directory_is_reattached() {
    let f = Fixture::new();
    let id = operation();
    let staged = f.stage(&id, b"original archive");
    let archive = staged.directory().join(PAYLOAD);
    let inode = fs::metadata(staged.directory()).unwrap().ino();
    f.move_archive_parent(&id);
    assert_eq!(fs::metadata(staged.directory()).unwrap().ino(), inode);
    let p = staged.publish(&|| Ok(()));
    assert_ne!(p.receipt.outcome, Outcome::Published);
    assert!(!f.target.path().exists());
    assert_eq!(fs::read(archive).unwrap(), b"original archive");
    assert!(p.controls_current(&|| Ok(())).is_err());
}
#[test]
fn private_external_archive_live_receipt_inspection_refuses_a_replaced_archive_parent_after_publication() {
    let f = Fixture::new();
    let id = operation();
    let staged = f.stage(&id, b"original archive");
    let intent = staged.intent().clone();
    let p = staged.publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::Published);
    f.move_archive_parent(&id);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"original archive");
    assert!(p.controls_current(&|| Ok(())).is_err());
    assert!(inspect_publication_after_directory_seals(&p, &intent, &[], &|| Ok(())).is_err());
}
#[test]
fn private_external_archive_refuses_nonprivate_redirected_missing_or_overlapping_contexts_without_creating_an_operation() {
    for kind in 0..5 {
        let f = Fixture::new();
        let path = f.root.join("context");
        let context = match kind {
            0 => {
                fs::DirBuilder::new().mode(0o755).create(&path).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                DirectoryFrame::capture(&path, &|| Ok(())).unwrap()
            }
            1 => DirectoryFrame::capture(&path, &|| Ok(())).unwrap(),
            2 => DirectoryFrame::capture(&f.root.join("session"), &|| Ok(())).unwrap(),
            3 => DirectoryFrame::capture(&f.root, &|| Ok(())).unwrap(),
            _ => {
                private(&path);
                let frame = DirectoryFrame::capture(&path, &|| Ok(())).unwrap();
                fs::rename(&path, f.root.join("held-context")).unwrap();
                std::os::unix::fs::symlink(f.root.join("held-context"), &path).unwrap();
                frame
            }
        };
        let id = operation();
        assert!(stage_private_from_frames(&f.target, &context, &id, b"new", 0, &|| Ok(())).is_err());
        assert!(!f.target.path().exists());
        assert!(!path.join(format!(".poema-save-{id}.incomplete")).exists());
        assert_eq!(fs::read_dir(f.root.join("session")).unwrap().count(), 0);
    }
}
#[test]
fn private_external_archive_keeps_reserve_overflow_capacity_and_initial_source_refusals_before_mkdir() {
    let f = Fixture::new();
    let free = f.archive.space(&|| Ok(())).unwrap().free;
    for reserve in [u64::MAX, free.checked_mul(2).unwrap()] {
        assert!(stage_private_from_frames(&f.target, &f.archive, &operation(), b"new", reserve, &|| Ok(())).is_err());
    }
    assert!(stage_private_from_frames(&f.target, &f.archive, &operation(), b"new", 0, &|| Err(error("controlled source loss"))).is_err());
    assert_eq!(fs::read_dir(f.root.join("archive")).unwrap().count(), 0);
    assert!(!f.target.path().exists());
}
#[test]
fn private_external_archive_cancellation_retains_actual_partial_staging_without_a_second_attempt_or_live_output() {
    let f = Fixture::new();
    let id = operation();
    let root = f.root.join("archive").join(format!(".poema-save-{id}.incomplete"));
    let payload = root.join(PAYLOAD);
    let data = vec![7u8; 3 * 65536];
    let check = || {
        if fs::metadata(&payload).is_ok_and(|m| m.len() > 0) { Err(error("controlled cancellation during original bytes")) } else { Ok(()) }
    };
    assert!(stage_private_from_frames(&f.target, &f.archive, &id, &data, 0, &check).is_err());
    let bytes = fs::read(&payload).unwrap();
    assert!(!bytes.is_empty() && bytes.len() <= data.len());
    assert!(!f.target.path().exists());
    assert!(stage_private_from_frames(&f.target, &f.archive, &id, &data, 0, &|| Ok(())).is_err());
    assert_eq!(fs::read(&payload).unwrap(), bytes);
}
#[test]
fn private_external_archive_before_and_after_kernel_parent_loss_retains_original_unknown_outcomes_without_receipt_adoption() {
    for after in [false, true] {
        let f = Fixture::new();
        let id = operation();
        let staged = f.stage(&id, b"original archive");
        let p = staged.publish_with(&|| Ok(()), &mut |point| {
            if matches!((point, after), (Point::BeforeKernel, false) | (Point::AfterKernel, true)) {
                f.move_archive_parent(&id);
            }
            Ok(())
        });
        assert_eq!(p.receipt.outcome, Outcome::Uncertain);
        assert!(!p.receipt_durable);
        assert_eq!(f.target.path().exists(), after);
        assert_eq!(fs::read(p.directory.join(PAYLOAD)).unwrap(), b"original archive");
        assert!(p.controls_current(&|| Ok(())).is_err());
    }
}
#[test]
fn ordinary_sibling_staging_keeps_its_original_location_modes_and_original_publication_inspector() {
    let f = Fixture::new();
    let mode_file = f.root.join("ordinary-mode");
    fs::write(&mode_file, b"mode").unwrap();
    let expected = fs::metadata(mode_file).unwrap().mode() & 0o777;
    let staged = stage_from_frame(&f.target, &operation(), b"ordinary project", &|| Ok(())).unwrap();
    let intent = staged.intent().clone();
    assert_eq!(staged.directory().parent(), Some(f.root.join("session").as_path()));
    let p = staged.publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::Published);
    assert_eq!(fs::metadata(f.target.path()).unwrap().mode() & 0o777, expected);
    assert_eq!(inspect_publication_after_directory_seals(&p, &intent, &[], &|| Ok(())).unwrap().current, CurrentState::Published);
    assert_eq!(fs::read_dir(f.root.join("archive")).unwrap().count(), 0);
}

#[test]
fn private_inspector_keeps_full_payload_and_control_fences_after_a_read_only_pass_changes_metadata() {
    let f = Fixture::new();
    let staged = f.stage(&operation(), b"original bytes");
    let intent = staged.intent().clone();
    let publication = staged.publish(&|| Ok(()));
    let calls = std::cell::Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    let guarded = archive_guard(publication.archive_context.as_deref(), &source);
    let result = inspect_expected_with_controls(&publication.directory, Some(&intent), &[], Some(&publication), &guarded, &mut || {
        fs::write(publication.directory.join(PAYLOAD), b"mutated bytes!")?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(calls.get() >= 4, "same full source must fence both sides of inspection");
    assert_eq!(fs::read(f.target.path()).unwrap(), b"original bytes");
    assert_eq!(fs::read(publication.directory.join(PAYLOAD)).unwrap(), b"mutated bytes!");
}

#[test]
fn private_original_writer_stop_after_dispatch_creation_keeps_target_absent_and_receipt_without_replaying() {
    let f = Fixture::new();
    let id = operation();
    let staged = f.stage(&id, b"original bytes");
    let dispatch = staged.directory().join(DISPATCH);
    let check = || if dispatch.exists() { Err(error("source stopped after actual dispatch creation")) } else { Ok(()) };
    let publication = staged.publish(&check);
    assert!(!publication.receipt.attempted);
    assert!(publication.receipt_durable);
    assert_eq!(publication.receipt.outcome, Outcome::NotPublished);
    assert!(!f.target.path().exists());
    assert!(dispatch.exists());
    assert_eq!(fs::read(publication.directory.join(PAYLOAD)).unwrap(), b"original bytes");
    assert!(stage_private_from_frames(&f.target, &f.archive, &id, b"original bytes", 1024 * 1024, &|| Ok(())).is_err());
}

#[test]
fn private_archive_read_failure_still_rechecks_same_source_and_preserves_original_operation() {
    let f = Fixture::new();
    let id = operation();
    let staged = f.stage(&id, b"original bytes");
    let publication = staged.publish(&|| Ok(()));
    f.move_archive_parent(&id);
    let calls = std::cell::Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        if calls.get() >= 2 { Err(error("source revoked after failed archive read")) } else { Ok(()) }
    };
    let result = publication.controls_current(&source);
    assert!(result.is_err());
    assert!(calls.get() >= 2);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"original bytes");
    assert!(publication.directory.join(RECEIPT).exists());
}

// Callback pairs are generated SDK validation only; no Cold/native owner exists here.
#[test]
fn paired_private_source_keeps_original_publisher_receipts_bytes_and_independent_payload() {
    let f = Fixture::new();
    let full = std::cell::Cell::new(0);
    let fenced = std::cell::Cell::new(0);
    let current = || {
        full.set(full.get() + 1);
        Ok(())
    };
    let after = || {
        fenced.set(fenced.get() + 1);
        Ok(())
    };
    let checks = ReadSource::new(&current, &after);
    let staged = stage_private_from_frames(&f.target, &f.archive, &operation(), b"paired original bytes", 1024 * 1024, &checks).unwrap();
    let intent = staged.intent().clone();
    let staged_calls = full.get();
    let publication = staged.publish(&checks);
    assert!(full.get() > staged_calls && staged_calls > 0 && fenced.get() > 0);
    assert!(publication.receipt_durable);
    assert_eq!(publication.receipt.outcome, Outcome::Published);
    publication.controls_current(&checks).unwrap();
    assert_eq!(inspect_publication_after_directory_seals(&publication, &intent, &[], &checks).unwrap().current, CurrentState::Published);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"paired original bytes");
    assert_ne!(fs::metadata(f.target.path()).unwrap().ino(), fs::metadata(publication.directory.join(PAYLOAD)).unwrap().ino());
}
#[test]
fn paired_private_source_cannot_use_permissive_final_fence_instead_of_complete_source_before_staging() {
    let f = Fixture::new();
    let checks = ReadSource::new(&|| Err(error("complete source lost")), &|| Ok(()));
    assert!(stage_private_from_frames(&f.target, &f.archive, &operation(), b"new", 0, &checks).is_err());
    assert!(!f.target.path().exists());
    assert_eq!(fs::read_dir(f.root.join("archive")).unwrap().count(), 0);
}
#[test]
fn paired_private_source_rechecks_complete_source_during_original_byte_writer_and_retains_partial_data() {
    let f = Fixture::new();
    let id = operation();
    let root = f.root.join("archive").join(format!(".poema-save-{id}.incomplete"));
    let payload = root.join(PAYLOAD);
    let current =
        || if fs::metadata(&payload).is_ok_and(|m| m.len() > 0) { Err(error("complete source cancelled between actual byte writes")) } else { Ok(()) };
    let checks = ReadSource::new(&current, &|| Ok(()));
    let data = vec![7; 3 * 65536];
    assert!(stage_private_from_frames(&f.target, &f.archive, &id, &data, 0, &checks).is_err());
    let bytes = fs::read(&payload).unwrap();
    assert!(!bytes.is_empty() && bytes.len() <= data.len());
    assert!(!f.target.path().exists());
    assert!(stage_private_from_frames(&f.target, &f.archive, &id, &data, 0, &|| Ok(())).is_err());
    assert_eq!(fs::read(&payload).unwrap(), bytes);
}
#[test]
fn paired_private_final_fence_loss_after_dispatch_creation_keeps_actual_not_published_receipt_without_target_or_replay() {
    let f = Fixture::new();
    let staged = f.stage(&operation(), b"original bytes");
    let dispatch = staged.directory().join(DISPATCH);
    let current = || Ok(());
    let after = || if dispatch.exists() { Err(error("final source generation changed")) } else { Ok(()) };
    let publication = staged.publish(&ReadSource::new(&current, &after));
    assert!(!publication.receipt.attempted);
    assert!(publication.receipt_durable);
    assert_eq!(publication.receipt.outcome, Outcome::NotPublished);
    assert!(!f.target.path().exists());
    assert!(dispatch.exists());
    assert_eq!(fs::read(publication.directory.join(PAYLOAD)).unwrap(), b"original bytes");
}
#[test]
fn paired_private_source_loss_after_kernel_keeps_uncertain_actual_output_and_original_receipt_owner() {
    let f = Fixture::new();
    let staged = f.stage(&operation(), b"original bytes");
    let stopped = std::cell::Cell::new(false);
    let current = || if stopped.get() { Err(error("source stopped after kernel")) } else { Ok(()) };
    let after = || Ok(());
    let checks = ReadSource::new(&current, &after);
    let publication = staged.publish_with(&checks, &mut |point| {
        if matches!(point, Point::AfterKernel) {
            stopped.set(true);
        }
        Ok(())
    });
    assert!(publication.receipt.attempted);
    assert_eq!(publication.receipt.outcome, Outcome::Uncertain);
    assert!(publication.receipt_durable);
    assert_eq!(fs::read(f.target.path()).unwrap(), b"original bytes");
    assert_eq!(fs::read(publication.directory.join(PAYLOAD)).unwrap(), b"original bytes");
}
