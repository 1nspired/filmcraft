use super::*;
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    process::Command,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn world() -> (PathBuf, PathBuf, PathBuf, String) {
    let root = std::env::temp_dir().join(format!(
        "preview-relocation-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let from = root.join("from");
    let to = root.join("to");
    fs::create_dir(&from).unwrap();
    fs::create_dir(&to).unwrap();
    let source = from.join("preview.mov");
    fs::write(&source, b"original generated preview").unwrap();
    let destination = to.join("preview.mov");
    (root, source, destination, format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst)))
}
#[test]
fn retained_preview_rename_preserves_actual_source_inode_permissions_empty_and_multichunk_bytes_with_one_independent_streamed_archive() {
    for data in [Vec::new(), b"actual preview bytes".to_vec(), vec![0x6b; 196615]] {
        let (_, source, destination, id) = world();
        fs::write(&source, &data).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let before = fs::metadata(&source).unwrap();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let root = staged.directory().to_path_buf();
        assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), data);
        assert_ne!(fs::metadata(root.join(ARCHIVE)).unwrap().ino(), before.ino());
        assert!(source.exists());
        assert!(!destination.exists());
        let p = staged.publish(&|| Ok(()));
        assert_eq!(p.receipt.outcome, Outcome::Moved);
        assert!(p.receipt_durable && p.receipt.directories_synced && p.receipt.attempted);
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), data);
        assert_eq!(fs::metadata(&destination).unwrap().ino(), before.ino());
        assert_eq!(fs::metadata(&destination).unwrap().mode(), before.mode());
        let seen = inspect(&root, &|| Ok(())).unwrap();
        assert_eq!(seen.current, CurrentState::Moved);
        assert_eq!(seen.recorded_receipt, Some(p.receipt));
        assert!(!seen.write_authority && !seen.startup_authority && !seen.native_undo);
    }
}
#[test]
fn retained_preview_rename_existing_equal_foreign_or_nonregular_destinations_are_never_overwritten_or_staged() {
    for kind in ["same", "foreign", "directory"] {
        let (_, source, destination, id) = world();
        if kind == "directory" {
            fs::create_dir(&destination).unwrap();
        } else {
            fs::write(&destination, if kind == "same" { b"original generated preview".as_slice() } else { b"foreign destination".as_slice() }).unwrap();
        }
        let before = fs::symlink_metadata(&destination).unwrap();
        assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
        assert_eq!(fs::symlink_metadata(&destination).unwrap().ino(), before.ino());
        assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
        assert!(!source.parent().unwrap().join(format!(".poema-move-{id}.incomplete")).exists());
    }
}
#[test]
fn retained_preview_rename_operation_collision_and_copied_metadata_cannot_reconstruct_a_live_dispatch_owner() {
    let (_, source, destination, id) = world();
    let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
    let root = staged.directory().to_path_buf();
    assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
    let copy = source.parent().unwrap().join(format!(".poema-move-{}.incomplete", "f".repeat(32)));
    fs::create_dir(&copy).unwrap();
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o700)).unwrap();
    for name in [ARCHIVE, INTENT] {
        fs::copy(root.join(name), copy.join(name)).unwrap();
    }
    assert!(inspect(&copy, &|| Ok(())).is_err());
    assert_eq!(inspect(&root, &|| Ok(())).unwrap().current, CurrentState::Prepared);
    assert!(source.exists() && !destination.exists());
    drop(staged);
}
#[test]
fn retained_preview_rename_changed_source_or_new_destination_before_dispatch_preserves_later_files_and_original_archive_without_attempt() {
    for field in ["source", "destination"] {
        let (_, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let root = staged.directory().to_path_buf();
        let changed = if field == "source" { &source } else { &destination };
        fs::write(changed, b"later human file").unwrap();
        let p = staged.publish(&|| Ok(()));
        assert_eq!(p.receipt.outcome, Outcome::NotMoved);
        assert!(!p.receipt.attempted);
        assert!(p.receipt_durable);
        assert!(!root.join(DISPATCH).exists());
        assert_eq!(fs::read(changed).unwrap(), b"later human file");
        assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), b"original generated preview");
        assert_eq!(inspect(&root, &|| Ok(())).unwrap().current, CurrentState::Changed);
    }
}
#[test]
fn retained_preview_rename_destination_creation_in_final_kernel_gap_refuses_replacement_and_preserves_both_files() {
    let (_, source, destination, id) = world();
    let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
    let p = staged.publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            fs::write(&destination, b"exact-gap destination")?;
        }
        Ok(())
    });
    assert!(p.receipt.attempted);
    assert_ne!(p.receipt.outcome, Outcome::Moved);
    assert!(p.receipt.kernel_error.is_some());
    assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
    assert_eq!(fs::read(&destination).unwrap(), b"exact-gap destination");
    assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
}
#[test]
fn retained_preview_rename_source_replacement_in_kernel_gap_retains_unexpected_actual_file_and_original_archive_without_reverse_move() {
    let (root, source, destination, id) = world();
    let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
    let held = root.join("human-held-original");
    let p = staged.publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            fs::rename(&source, &held)?;
            fs::write(&source, b"foreign exact-gap source")?;
        }
        Ok(())
    });
    assert_eq!(p.receipt.outcome, Outcome::ConflictRetained);
    assert!(p.receipt_durable);
    assert!(!source.exists());
    assert_eq!(fs::read(&destination).unwrap(), b"foreign exact-gap source");
    assert_eq!(fs::read(held).unwrap(), b"original generated preview");
    assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
}
#[test]
fn retained_preview_rename_later_output_replacement_inplace_change_or_recreated_source_are_kept_without_cleanup_or_replay() {
    for field in ["replacement", "inplace", "source"] {
        let (root, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let p = staged.publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                if field == "replacement" {
                    fs::rename(&destination, root.join("held-moved-preview"))?;
                    fs::write(&destination, b"later replacement")?;
                } else if field == "inplace" {
                    fs::write(&destination, b"later in-place writer")?;
                } else {
                    fs::write(&source, b"later source recreation")?;
                }
            }
            Ok(())
        });
        assert_eq!(p.receipt.outcome, Outcome::ConflictRetained);
        assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
        assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
        assert_eq!(
            fs::read(if field == "source" { &source } else { &destination }).unwrap(),
            if field == "replacement" {
                b"later replacement".as_slice()
            } else if field == "inplace" {
                b"later in-place writer".as_slice()
            } else {
                b"later source recreation".as_slice()
            }
        );
    }
}
#[test]
fn retained_preview_rename_Stop_before_and_after_kernel_and_foreign_receipt_preserve_actual_effects_without_ordinary_move_fallback() {
    for point in ["before", "after", "receipt"] {
        let (_, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let root = staged.directory().to_path_buf();
        let stop = AtomicBool::new(false);
        let p = staged.publish_with(&|| if stop.load(Ordering::Acquire) { Err(error("generated Stop")) } else { Ok(()) }, &mut |phase| {
            if point == "before" && matches!(phase, Point::BeforeKernel) || point == "after" && matches!(phase, Point::AfterKernel) {
                stop.store(true, Ordering::Release);
            }
            if point == "receipt" && matches!(phase, Point::BeforeReceipt) {
                fs::write(root.join(RECEIPT), b"foreign receipt remains")?;
            }
            Ok(())
        });
        if point == "before" {
            assert!(!p.receipt.attempted);
            assert!(source.exists() && !destination.exists());
        } else {
            assert_eq!(p.receipt.outcome, Outcome::Uncertain);
            assert!(!source.exists() && destination.exists());
        }
        if point == "receipt" {
            assert!(!p.receipt_durable);
            assert_eq!(fs::read(root.join(RECEIPT)).unwrap(), b"foreign receipt remains");
        }
        assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), b"original generated preview");
    }
}
#[test]
fn retained_preview_rename_redirected_sources_parents_replaced_operation_directories_and_new_destination_parents_are_refused_without_deleting_held_bytes() {
    for kind in ["symlink", "source-directory", "redirect-parent", "root-replaced", "target-parent"] {
        let (root, source, destination, id) = world();
        if kind == "symlink" {
            let kept = root.join("kept-preview");
            fs::rename(&source, &kept).unwrap();
            symlink(&kept, &source).unwrap();
            assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
            assert_eq!(fs::read(kept).unwrap(), b"original generated preview");
            continue;
        }
        if kind == "source-directory" {
            fs::remove_file(&source).unwrap();
            fs::create_dir(&source).unwrap();
            assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
            continue;
        }
        if kind == "redirect-parent" {
            let alias = root.join("alias");
            symlink(source.parent().unwrap(), &alias).unwrap();
            assert!(stage(&alias.join("preview.mov"), &destination, &id, &|| Ok(())).is_err());
            continue;
        }
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let original = staged.directory().to_path_buf();
        if kind == "root-replaced" {
            let kept = root.join("held-operation");
            fs::rename(&original, &kept).unwrap();
            fs::create_dir(&original).unwrap();
            fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
            let p = staged.publish(&|| Ok(()));
            assert!(!p.receipt.attempted);
            assert!(!p.receipt_durable);
            assert_eq!(fs::read(kept.join(ARCHIVE)).unwrap(), b"original generated preview");
            assert!(!original.join(RECEIPT).exists());
        } else {
            fs::rename(destination.parent().unwrap(), root.join("held-target-parent")).unwrap();
            fs::create_dir(destination.parent().unwrap()).unwrap();
            let p = staged.publish(&|| Ok(()));
            assert!(!p.receipt.attempted);
            assert!(p.receipt_durable);
            assert_eq!(fs::read(original.join(ARCHIVE)).unwrap(), b"original generated preview");
        }
        assert!(source.exists() && !destination.exists());
    }
}
#[test]
fn retained_preview_rename_cancelled_multichunk_archive_and_corrupted_archive_leave_original_source_and_all_partial_artifacts_without_dispatch() {
    for corrupt in [false, true] {
        let (_, source, destination, id) = world();
        let data = vec![0x41; 196615];
        fs::write(&source, &data).unwrap();
        let root = source.parent().unwrap().join(format!(".poema-move-{id}.incomplete"));
        if corrupt {
            let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
            fs::write(root.join(ARCHIVE), b"foreign archive").unwrap();
            let p = staged.publish(&|| Ok(()));
            assert!(!p.receipt.attempted);
            assert!(inspect(&root, &|| Ok(())).is_err());
        } else {
            assert!(
                stage(&source, &destination, &id, &|| if fs::metadata(root.join(ARCHIVE)).is_ok_and(|v| v.len() >= 65536) {
                    Err(error("Stop during stream"))
                } else {
                    Ok(())
                })
                .is_err()
            );
            assert!(fs::metadata(root.join(ARCHIVE)).unwrap().len() >= 65536);
            assert!(!root.join(INTENT).exists());
            assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
        }
        assert_eq!(fs::read(&source).unwrap(), data);
        assert!(!destination.exists());
        assert!(!root.join(DISPATCH).exists());
    }
}
#[test]
fn retained_preview_rename_actual_child_exit_after_kernel_keeps_source_archive_and_moved_inode_for_fresh_read_only_inspection_without_a_receipt() {
    const CHILD: &str = "POEMA_PREVIEW_RELOCATION_CRASH_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let source = root.join("from/preview.mov");
        let destination = root.join("to/preview.mov");
        let id = std::env::var("POEMA_PREVIEW_RELOCATION_CRASH_ID").unwrap();
        stage(&source, &destination, &id, &|| Ok(())).unwrap().publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                std::process::exit(71);
            }
            Ok(())
        });
        panic!("crash child must exit at the actual kernel move boundary");
    }
    let (root, source, destination, id) = world();
    let before = fs::metadata(&source).unwrap();
    let output=Command::new(std::env::current_exe().unwrap()).args(["retained::relocation::tests::retained_preview_rename_actual_child_exit_after_kernel_keeps_source_archive_and_moved_inode_for_fresh_read_only_inspection_without_a_receipt","--exact","--nocapture"]).env(CHILD,&root).env("POEMA_PREVIEW_RELOCATION_CRASH_ID",&id).output().unwrap();
    assert_eq!(output.status.code(), Some(71), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!source.exists());
    assert_eq!(fs::metadata(&destination).unwrap().ino(), before.ino());
    let operation = source.parent().unwrap().join(format!(".poema-move-{id}.incomplete"));
    assert_eq!(fs::read(operation.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert!(!operation.join(RECEIPT).exists());
    let seen = inspect(&operation, &|| Ok(())).unwrap();
    assert_eq!(seen.current, CurrentState::Moved);
    assert!(seen.dispatch && seen.recorded_receipt.is_none());
    assert!(!seen.write_authority && !seen.startup_authority && !seen.native_undo);
}
#[test]
fn retained_preview_rename_old_open_source_writer_after_move_cannot_change_the_independent_archive_or_make_old_success_current() {
    let (_, source, destination, id) = world();
    let mut writer = fs::OpenOptions::new().write(true).open(&source).unwrap();
    let p = stage(&source, &destination, &id, &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::Moved);
    writer.seek(SeekFrom::Start(0)).unwrap();
    writer.write_all(b"LATER").unwrap();
    writer.sync_all().unwrap();
    assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
    assert!(fs::read(&destination).unwrap().starts_with(b"LATER"));
    assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
}
#[test]
fn retained_preview_rename_fresh_inspection_fences_earlier_source_destination_archive_and_metadata_after_later_reads() {
    for field in ["source", "destination", "archive", "metadata"] {
        let (_, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let operation = staged.directory().to_path_buf();
        assert!(
            inspect_with(&operation, &|| Ok(()), &mut || {
                match field {
                    "source" => fs::write(&source, b"later source")?,
                    "destination" => fs::write(&destination, b"later target")?,
                    "archive" => fs::write(operation.join(ARCHIVE), b"later archive")?,
                    _ => fs::write(operation.join(INTENT), b"foreign intent")?,
                }
                Ok(())
            })
            .is_err()
        );
        assert!(source.exists());
        assert!(!operation.join(DISPATCH).exists());
        drop(staged);
    }
}
