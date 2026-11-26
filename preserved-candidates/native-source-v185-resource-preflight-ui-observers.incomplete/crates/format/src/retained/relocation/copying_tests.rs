use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    process::Command,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) fn world() -> (PathBuf, PathBuf, PathBuf, String) {
    let root = std::env::temp_dir().join(format!(
        "preview-copy-{}-{}-{}",
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
    let destination = to.join("preview.mov");
    fs::write(&source, b"original generated preview").unwrap();
    (root, source, destination, format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst)))
}
#[test]
fn retained_preview_copy_streams_empty_and_multichunk_payloads_to_independent_destination_archive_and_slot_preserving_exact_original_permissions() {
    for data in [Vec::new(), b"actual preview bytes".to_vec(), vec![0x6b; 196615]] {
        for mode in [0o444, 0o600, 0o755] {
            let (_, source, destination, id) = world();
            fs::write(&source, &data).unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(mode)).unwrap();
            let original = fs::metadata(&source).unwrap();
            let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
            let root = staged.directory().to_path_buf();
            assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), data);
            assert_eq!(fs::read(root.join(SLOT)).unwrap(), data);
            assert_ne!(fs::metadata(root.join(ARCHIVE)).unwrap().ino(), fs::metadata(root.join(SLOT)).unwrap().ino());
            let p = staged.publish(&|| Ok(()));
            assert_eq!(p.receipt.outcome, Outcome::Copied);
            assert!(p.receipt_durable && p.receipt.directories_synced);
            p.verify_current(&|| Ok(())).unwrap();
            assert_eq!(fs::read(&source).unwrap(), data);
            assert_eq!(fs::read(&destination).unwrap(), data);
            assert_eq!(fs::metadata(&source).unwrap().ino(), original.ino());
            assert_ne!(fs::metadata(&destination).unwrap().ino(), original.ino());
            assert_ne!(fs::metadata(&destination).unwrap().ino(), fs::metadata(root.join(ARCHIVE)).unwrap().ino());
            assert_eq!(fs::metadata(&destination).unwrap().mode() & 0o7777, mode);
            let seen = inspect(&root, &|| Ok(())).unwrap();
            assert_eq!(seen.current, CurrentState::Copied);
            assert!(!seen.write_authority && !seen.retirement_authority && !seen.startup_authority && !seen.native_undo);
        }
    }
}
#[test]
fn retained_preview_copy_existing_equal_foreign_and_nonregular_destinations_are_preserved_before_any_new_operation_directory() {
    for kind in ["equal", "foreign", "directory"] {
        let (_, source, destination, id) = world();
        if kind == "directory" {
            fs::create_dir(&destination).unwrap();
        } else {
            fs::write(&destination, if kind == "equal" { b"original generated preview".as_slice() } else { b"foreign destination".as_slice() }).unwrap();
        }
        let original = fs::symlink_metadata(&destination).unwrap();
        assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
        assert_eq!(fs::symlink_metadata(&destination).unwrap().ino(), original.ino());
        assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
        assert!(!destination.parent().unwrap().join(format!(".poema-copy-{id}.incomplete")).exists());
    }
}
#[test]
fn retained_preview_copy_changed_source_before_publication_preserves_latest_source_and_both_original_staged_payloads_without_dispatch() {
    let (_, source, destination, id) = world();
    let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
    let root = staged.directory().to_path_buf();
    fs::write(&source, b"later human source").unwrap();
    let mut p = staged.publish(&|| Ok(()));
    assert_eq!(p.receipt.outcome, Outcome::NotCopied);
    assert!(!p.receipt.attempted);
    assert!(p.stage_retirement(&|| Ok(())).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"later human source");
    assert!(!destination.exists());
    assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert_eq!(fs::read(root.join(SLOT)).unwrap(), b"original generated preview");
    assert!(!root.join(DISPATCH).exists());
}
#[test]
fn retained_preview_copy_final_gap_destination_collision_is_never_replaced_and_never_receives_a_source_retirement_continuation() {
    let (_, source, destination, id) = world();
    let mut p = stage(&source, &destination, &id, &|| Ok(())).unwrap().publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            fs::write(&destination, b"exact-gap destination")?;
        }
        Ok(())
    });
    assert_ne!(p.receipt.outcome, Outcome::Copied);
    assert!(p.stage_retirement(&|| Ok(())).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
    assert_eq!(fs::read(&destination).unwrap(), b"exact-gap destination");
    assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert_eq!(fs::read(p.directory.join(SLOT)).unwrap(), b"original generated preview");
}
#[test]
fn retained_preview_copy_late_source_and_output_changes_keep_both_human_work_and_independent_archive_without_source_retirement() {
    for field in ["source", "replacement", "inplace"] {
        let (root, source, destination, id) = world();
        let mut p = stage(&source, &destination, &id, &|| Ok(())).unwrap().publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                if field == "source" {
                    fs::write(&source, b"later source")?;
                } else if field == "replacement" {
                    fs::rename(&destination, root.join("held-copied-output"))?;
                    fs::write(&destination, b"later output")?;
                } else {
                    fs::write(&destination, b"later output")?;
                }
            }
            Ok(())
        });
        assert_eq!(p.receipt.outcome, Outcome::ConflictRetained);
        assert!(p.stage_retirement(&|| Ok(())).is_err());
        assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
        assert_eq!(
            fs::read(if field == "source" { &source } else { &destination }).unwrap(),
            if field == "source" { b"later source".as_slice() } else { b"later output".as_slice() }
        );
        assert!(source.exists());
    }
}
#[test]
fn retained_preview_copy_Stop_before_after_kernel_or_foreign_receipt_never_allows_a_historical_copy_result_to_retire_source() {
    for at in ["before", "after", "receipt"] {
        let (_, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let root = staged.directory().to_path_buf();
        let stop = AtomicBool::new(false);
        let mut p = staged.publish_with(&|| if stop.load(Ordering::Acquire) { Err(error("generated Stop")) } else { Ok(()) }, &mut |point| {
            if at == "before" && matches!(point, Point::BeforeKernel) || at == "after" && matches!(point, Point::AfterKernel) {
                stop.store(true, Ordering::Release);
            }
            if at == "receipt" && matches!(point, Point::BeforeReceipt) {
                fs::write(root.join(RECEIPT), b"foreign copy receipt")?;
            }
            Ok(())
        });
        assert!(p.stage_retirement(&|| Ok(())).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"original generated preview");
        assert_eq!(destination.exists(), at != "before");
        if at != "before" {
            assert_eq!(p.receipt.outcome, Outcome::Uncertain);
        }
        if at == "receipt" {
            assert!(!p.receipt_durable);
            assert_eq!(fs::read(root.join(RECEIPT)).unwrap(), b"foreign copy receipt");
        }
        assert_eq!(fs::read(root.join(ARCHIVE)).unwrap(), b"original generated preview");
    }
}
#[test]
fn retained_preview_copy_serialized_or_modified_public_receipt_fields_cannot_mint_or_duplicate_the_live_source_retirement_continuation() {
    let (_, source, destination, id) = world();
    let mut p = stage(&source, &destination, &id, &|| Ok(())).unwrap().publish(&|| Ok(()));
    let receipt = p.receipt.clone();
    let mut copied = Publication {
        receipt: serde_json::from_slice(&serde_json::to_vec(&receipt).unwrap()).unwrap(),
        receipt_durable: true,
        directory: p.directory.clone(),
        retirement: None,
    };
    assert!(copied.stage_retirement(&|| Ok(())).is_err());
    p.receipt.operation = "forged metadata".into();
    assert!(p.stage_retirement(&|| Ok(())).is_err());
    p.receipt = receipt;
    p.verify_current(&|| Ok(())).unwrap();
    let staged = p.stage_retirement(&|| Ok(())).unwrap();
    assert!(p.stage_retirement(&|| Ok(())).is_err());
    assert!(source.exists());
    assert!(!staged.directory().join("retained-source.bin").exists());
    drop(staged);
    assert_eq!(fs::read(source).unwrap(), b"original generated preview");
}
#[test]
fn retained_preview_copy_corrupted_archive_slot_symlink_or_replaced_operation_directory_refuses_without_touching_original_source() {
    for field in ["archive", "slot", "symlink", "root"] {
        let (root, source, destination, id) = world();
        if field == "symlink" {
            let kept = root.join("held-source");
            fs::rename(&source, &kept).unwrap();
            symlink(&kept, &source).unwrap();
            assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
            continue;
        }
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let op = staged.directory().to_path_buf();
        if field == "root" {
            fs::rename(&op, root.join("held-operation")).unwrap();
            fs::create_dir(&op).unwrap();
            fs::set_permissions(&op, fs::Permissions::from_mode(0o700)).unwrap();
        } else {
            fs::write(op.join(if field == "archive" { ARCHIVE } else { SLOT }), b"foreign payload").unwrap();
        }
        let mut p = staged.publish(&|| Ok(()));
        assert!(!p.receipt.attempted);
        assert!(p.stage_retirement(&|| Ok(())).is_err());
        assert!(source.exists() && !destination.exists());
        if field == "root" {
            assert_eq!(fs::read(root.join("held-operation").join(ARCHIVE)).unwrap(), b"original generated preview");
            assert!(!op.join(RECEIPT).exists());
        }
    }
}
#[test]
fn retained_preview_copy_interrupted_multichunk_archive_or_slot_keeps_partial_files_and_original_source_without_a_publication_or_second_stage() {
    for output in [ARCHIVE, SLOT] {
        let (_, source, destination, id) = world();
        let data = vec![0x41; 196615];
        fs::write(&source, &data).unwrap();
        let op = destination.parent().unwrap().join(format!(".poema-copy-{id}.incomplete"));
        assert!(
            stage(&source, &destination, &id, &|| if fs::metadata(op.join(output)).is_ok_and(|v| v.len() >= 65536) {
                Err(error("Stop during shared stream"))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(fs::metadata(op.join(output)).unwrap().len() >= 65536);
        assert_eq!(fs::read(&source).unwrap(), data);
        assert!(!destination.exists());
        assert!(!op.join(DISPATCH).exists());
        assert!(stage(&source, &destination, &id, &|| Ok(())).is_err());
    }
}
#[test]
fn retained_preview_copy_actual_child_exit_after_kernel_keeps_original_source_output_and_archive_without_a_receipt_or_live_retirement_key() {
    const CHILD: &str = "POEMA_PREVIEW_COPY_CRASH_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let id = std::env::var("POEMA_PREVIEW_COPY_CRASH_ID").unwrap();
        stage(&root.join("from/preview.mov"), &root.join("to/preview.mov"), &id, &|| Ok(())).unwrap().publish_with(&|| Ok(()), &mut |point| {
            if matches!(point, Point::AfterKernel) {
                std::process::exit(71);
            }
            Ok(())
        });
        panic!("child must exit at actual copy kernel boundary");
    }
    let (root, source, destination, id) = world();
    let original = fs::metadata(&source).unwrap();
    let output=Command::new(std::env::current_exe().unwrap()).args(["retained::relocation::copying::tests::retained_preview_copy_actual_child_exit_after_kernel_keeps_original_source_output_and_archive_without_a_receipt_or_live_retirement_key","--exact","--nocapture"]).env(CHILD,&root).env("POEMA_PREVIEW_COPY_CRASH_ID",&id).output().unwrap();
    assert_eq!(output.status.code(), Some(71), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::metadata(&source).unwrap().ino(), original.ino());
    assert_ne!(fs::metadata(&destination).unwrap().ino(), original.ino());
    let op = destination.parent().unwrap().join(format!(".poema-copy-{id}.incomplete"));
    assert_eq!(fs::read(op.join(ARCHIVE)).unwrap(), b"original generated preview");
    assert!(!op.join(RECEIPT).exists());
    let seen = inspect(&op, &|| Ok(())).unwrap();
    assert_eq!(seen.current, CurrentState::Copied);
    assert!(seen.recorded_receipt.is_none());
    assert!(!seen.retirement_authority && !seen.write_authority);
}
#[test]
fn retained_preview_copy_old_open_source_or_output_writer_changes_refuse_live_retirement_and_keep_the_independent_archive() {
    for target in ["source", "output"] {
        let (_, source, destination, id) = world();
        let mut p = stage(&source, &destination, &id, &|| Ok(())).unwrap().publish(&|| Ok(()));
        let mut writer = fs::OpenOptions::new().write(true).open(if target == "source" { &source } else { &destination }).unwrap();
        writer.write_all(b"LATER").unwrap();
        writer.sync_all().unwrap();
        assert!(p.verify_current(&|| Ok(())).is_err());
        assert!(p.stage_retirement(&|| Ok(())).is_err());
        assert!(source.exists());
        assert_eq!(fs::read(p.directory.join(ARCHIVE)).unwrap(), b"original generated preview");
        assert!(!source.parent().unwrap().join(format!(".poema-retire-{id}.incomplete")).exists());
    }
}
#[test]
fn retained_preview_copy_fresh_inspection_fences_earlier_source_output_archive_slot_and_metadata_after_later_reads() {
    for field in ["source", "destination", "archive", "slot", "metadata"] {
        let (_, source, destination, id) = world();
        let staged = stage(&source, &destination, &id, &|| Ok(())).unwrap();
        let op = staged.directory().to_path_buf();
        assert!(
            inspect_with(&op, &|| Ok(()), &mut || {
                match field {
                    "source" => fs::write(&source, b"later source")?,
                    "destination" => fs::write(&destination, b"later output")?,
                    "archive" => fs::write(op.join(ARCHIVE), b"later archive")?,
                    "slot" => fs::write(op.join(SLOT), b"later slot")?,
                    _ => fs::write(op.join(INTENT), b"foreign intent")?,
                }
                Ok(())
            })
            .is_err()
        );
        assert!(source.exists());
        assert!(!op.join(DISPATCH).exists());
        drop(staged);
    }
}
