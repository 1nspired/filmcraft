use super::super::tests::world;
use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
};
fn copied() -> (PathBuf, PathBuf, PathBuf, String, super::super::Publication) {
    let (root, source, destination, id) = world();
    let p = super::super::stage(&source, &destination, &id, &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, super::super::Outcome::Copied);
    (root, source, destination, id, p)
}
#[test]
fn retained_preview_source_retirement_moves_original_inode_to_source_side_private_storage_only_after_a_live_complete_copy_and_never_unlinks_it() {
    let (_, source, destination, _id, mut copy) = copied();
    let original = fs::metadata(&source).unwrap();
    let staged = copy.stage_retirement(&|| Ok(())).unwrap();
    let root = staged.directory().to_path_buf();
    assert!(source.exists());
    assert_eq!(inspect(&root, &|| Ok(())).unwrap().current, CurrentState::Prepared);
    assert!(copy.stage_retirement(&|| Ok(())).is_err());
    let p = staged.publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::Retired);
    assert!(p.receipt_durable && p.receipt.directories_synced);
    assert!(!source.exists());
    assert_eq!(fs::metadata(root.join(RETAINED)).unwrap().ino(), original.ino());
    for path in [root.join(RETAINED), destination, copy.directory.join(super::super::ARCHIVE)] {
        assert_eq!(fs::read(path).unwrap(), b"original generated preview");
    }
    let seen = inspect(&root, &|| Ok(())).unwrap();
    assert_eq!(seen.current, CurrentState::Retired);
    assert!(!seen.write_authority && !seen.startup_authority && !seen.native_undo);
}
#[test]
fn retained_preview_source_retirement_before_staging_refuses_later_source_destination_archive_or_copy_receipt_without_creating_source_side_artifacts() {
    for field in ["source", "destination", "archive", "receipt"] {
        let (_, source, destination, id, mut copy) = copied();
        let path = match field {
            "source" => source.clone(),
            "destination" => destination.clone(),
            "archive" => copy.directory.join(super::super::ARCHIVE),
            _ => copy.directory.join(RECEIPT),
        };
        fs::write(&path, b"later human bytes").unwrap();
        assert!(copy.stage_retirement(&|| Ok(())).is_err());
        assert!(source.exists());
        assert_eq!(fs::read(path).unwrap(), b"later human bytes");
        assert!(!source.parent().unwrap().join(format!(".poema-retire-{id}.incomplete")).exists());
    }
}
#[test]
fn retained_preview_source_retirement_changed_source_output_or_replaced_copy_and_retirement_roots_refuse_kernel_move_without_reverting_later_work() {
    for field in ["source", "destination", "copy-root", "retire-root"] {
        let (root, source, destination, _id, mut copy) = copied();
        let staged = copy.stage_retirement(&|| Ok(())).unwrap();
        let op = staged.directory().to_path_buf();
        match field {
            "source" => fs::write(&source, b"later source").unwrap(),
            "destination" => fs::write(&destination, b"later destination").unwrap(),
            "copy-root" => {
                fs::rename(&copy.directory, root.join("held-copy-root")).unwrap();
                fs::create_dir(&copy.directory).unwrap();
                fs::set_permissions(&copy.directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
            _ => {
                fs::rename(&op, root.join("held-retirement-root")).unwrap();
                fs::create_dir(&op).unwrap();
                fs::set_permissions(&op, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let p = staged.publish(&|| Ok(()));
        assert!(!p.receipt.attempted);
        assert!(source.exists());
        assert!(!op.join(RETAINED).exists());
        if field == "source" {
            assert_eq!(fs::read(&source).unwrap(), b"later source");
        }
        if field == "destination" {
            assert_eq!(fs::read(&destination).unwrap(), b"later destination");
        }
        if field == "retire-root" {
            assert!(!p.receipt_durable);
            assert!(!op.join(RECEIPT).exists());
        }
    }
}
#[test]
fn retained_preview_source_retirement_final_gap_foreign_source_is_kept_in_retention_with_original_archive_and_never_moved_back_over_human_work() {
    let (root, source, destination, _id, mut copy) = copied();
    let staged = copy.stage_retirement(&|| Ok(())).unwrap();
    let held = root.join("human-held-original");
    let p = staged.publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            fs::rename(&source, &held)?;
            fs::write(&source, b"foreign gap source")?;
        }
        Ok(())
    });
    assert_eq!(p.receipt.outcome, Outcome::ConflictRetained);
    assert!(!source.exists());
    assert_eq!(fs::read(p.directory.join(RETAINED)).unwrap(), b"foreign gap source");
    for path in [held, destination, copy.directory.join(super::super::ARCHIVE)] {
        assert_eq!(fs::read(path).unwrap(), b"original generated preview");
    }
    assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
}
#[test]
fn retained_preview_source_retirement_final_gap_retention_collision_never_overwrites_either_original_source_or_foreign_retained_file() {
    let (_, source, _destination, _id, mut copy) = copied();
    let staged = copy.stage_retirement(&|| Ok(())).unwrap();
    let root = staged.directory().to_path_buf();
    let p = staged.publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            fs::write(root.join(RETAINED), b"foreign retention slot")?;
        }
        Ok(())
    });
    assert_ne!(p.receipt.outcome, Outcome::Retired);
    assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
    assert_eq!(fs::read(root.join(RETAINED)).unwrap(), b"foreign retention slot");
    assert_eq!(inspect(&root, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
}
#[test]
fn retained_preview_source_retirement_late_source_recreation_output_change_or_old_open_writer_preserves_all_actual_files_and_never_claims_clean_retirement() {
    for field in ["source", "destination", "writer"] {
        let (_, source, destination, _id, mut copy) = copied();
        let mut writer = fs::OpenOptions::new().write(true).open(&source).unwrap();
        let staged = copy.stage_retirement(&|| Ok(())).unwrap();
        let p = staged.publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                match field {
                    "source" => fs::write(&source, b"later recreated source")?,
                    "destination" => fs::write(&destination, b"later copied output")?,
                    _ => {
                        writer.write_all(b"LATER")?;
                        writer.sync_all()?;
                    }
                }
            }
            Ok(())
        });
        assert_ne!(p.receipt.outcome, Outcome::Retired);
        assert_eq!(fs::read(copy.directory.join(super::super::ARCHIVE)).unwrap(), b"original generated preview");
        assert!(p.directory.join(RETAINED).exists());
        if field == "source" {
            assert_eq!(fs::read(&source).unwrap(), b"later recreated source");
        }
        if field == "destination" {
            assert_eq!(fs::read(&destination).unwrap(), b"later copied output");
        }
        if field == "writer" {
            assert!(fs::read(p.directory.join(RETAINED)).unwrap().starts_with(b"LATER"));
        }
        assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
    }
}
#[test]
fn retained_preview_source_retirement_Stop_before_after_kernel_or_foreign_receipt_preserves_source_or_retained_inode_without_unlink_or_retry() {
    for at in ["before", "after", "receipt"] {
        let (_, source, _destination, _id, mut copy) = copied();
        let staged = copy.stage_retirement(&|| Ok(())).unwrap();
        let root = staged.directory().to_path_buf();
        let stop = AtomicBool::new(false);
        let p = staged.publish_with(&|| if stop.load(Ordering::Acquire) { Err(error("generated Stop observing retirement")) } else { Ok(()) }, &mut |point| {
            if at == "before" && matches!(point, Point::BeforeKernel) || at == "after" && matches!(point, Point::AfterKernel) {
                stop.store(true, Ordering::Release);
            }
            if at == "receipt" && matches!(point, Point::BeforeReceipt) {
                fs::write(root.join(RECEIPT), b"foreign retirement receipt")?;
            }
            Ok(())
        });
        assert_eq!(source.exists(), at == "before");
        assert_eq!(root.join(RETAINED).exists(), at != "before");
        if at != "before" {
            assert_eq!(p.receipt.outcome, Outcome::Uncertain);
        }
        if at == "receipt" {
            assert!(!p.receipt_durable);
            assert_eq!(fs::read(root.join(RECEIPT)).unwrap(), b"foreign retirement receipt");
        }
        assert_eq!(fs::read(copy.directory.join(super::super::ARCHIVE)).unwrap(), b"original generated preview");
    }
}
#[test]
fn retained_preview_source_retirement_actual_child_exit_after_kernel_keeps_original_inode_archive_and_output_for_read_only_recovery_without_a_dispatch_key() {
    const CHILD: &str = "POEMA_PREVIEW_RETIRE_CRASH_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let id = std::env::var("POEMA_PREVIEW_RETIRE_CRASH_ID").unwrap();
        let mut copy = super::super::stage(&root.join("from/preview.mov"), &root.join("to/preview.mov"), &id, &|| Ok(())).unwrap().publish(&|| Ok(()));
        copy.stage_retirement(&|| Ok(())).unwrap().publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                std::process::exit(71);
            }
            Ok(())
        });
        panic!("child must exit at actual source retirement kernel boundary");
    }
    let (root, source, destination, id) = world();
    let original = fs::metadata(&source).unwrap();
    let output=Command::new(std::env::current_exe().unwrap()).args(["retained::relocation::copying::retirement::tests::retained_preview_source_retirement_actual_child_exit_after_kernel_keeps_original_inode_archive_and_output_for_read_only_recovery_without_a_dispatch_key","--exact","--nocapture"]).env(CHILD,&root).env("POEMA_PREVIEW_RETIRE_CRASH_ID",&id).output().unwrap();
    assert_eq!(output.status.code(), Some(71), "{}", String::from_utf8_lossy(&output.stderr));
    let operation = source.parent().unwrap().join(format!(".poema-retire-{id}.incomplete"));
    assert!(!source.exists());
    assert_eq!(fs::metadata(operation.join(RETAINED)).unwrap().ino(), original.ino());
    assert_eq!(fs::read(&destination).unwrap(), b"original generated preview");
    assert!(!operation.join(RECEIPT).exists());
    let seen = inspect(&operation, &|| Ok(())).unwrap();
    assert_eq!(seen.current, CurrentState::Retired);
    assert!(seen.recorded_receipt.is_none());
    assert!(!seen.write_authority && !seen.startup_authority && !seen.native_undo);
}
#[test]
fn retained_preview_source_retirement_fresh_inspection_fences_prior_copy_archive_output_source_retained_file_and_metadata_after_later_reads() {
    for field in ["archive", "output", "source", "retained", "metadata"] {
        let (_, source, destination, _id, mut copy) = copied();
        let p = copy.stage_retirement(&|| Ok(())).unwrap().publish(&|| Ok(()));
        assert!(
            inspect_with(&p.directory, &|| Ok(()), &mut || {
                match field {
                    "archive" => fs::write(copy.directory.join(super::super::ARCHIVE), b"foreign archive")?,
                    "output" => fs::write(&destination, b"later output")?,
                    "source" => fs::write(&source, b"later source")?,
                    "retained" => fs::write(p.directory.join(RETAINED), b"later retained bytes")?,
                    _ => fs::write(p.directory.join(INTENT), b"foreign intent")?,
                }
                Ok(())
            })
            .is_err()
        );
        assert!(p.directory.join(RETAINED).exists());
    }
}
