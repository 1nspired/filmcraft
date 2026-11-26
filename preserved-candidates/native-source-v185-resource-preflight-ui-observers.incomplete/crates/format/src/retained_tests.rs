use super::*;
use std::os::unix::fs::PermissionsExt;
use std::{
    io::{Seek, SeekFrom, Write},
    process::Command,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
#[test]
fn retained_final_read_fence_refuses_in_place_payload_and_output_writers_and_final_stop() {
    for payload in [false, true] {
        let (_root, target, id) = world();
        let staged = stage(&target, &id, b"complete original bytes", &|| Ok(())).unwrap();
        let intent = staged.intent().clone();
        let publication = staged.publish(&|| Ok(()));
        inspect_publication_after_directory_seals(&publication, &intent, &[], &|| Ok(())).unwrap();
        let calls = std::cell::Cell::new(0);
        let final_stop = || {
            calls.set(calls.get() + 1);
            if calls.get() >= 2 { Err(error("controlled final Stop")) } else { Ok(()) }
        };
        assert!(fence_publication_after_read(&publication, &intent, &[], &final_stop).is_err());
        assert!(calls.get() >= 2);
        let path = if payload { publication.directory.join(PAYLOAD) } else { target };
        let inode = fs::metadata(&path).unwrap().ino();
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(b"later changed contents").unwrap();
        file.sync_all().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert!(fence_publication_after_read(&publication, &intent, &[], &|| Ok(())).is_err());
        assert!(path.is_file());
    }
}
#[test]
fn retained_final_read_fence_preserves_original_published_new_and_exchanged_generations() {
    for exists in [false, true] {
        for data in [Vec::new(), vec![0x6b; 131075]] {
            let (_root, target, id) = world();
            if exists {
                old(&target);
            }
            let staged = stage(&target, &id, &data, &|| Ok(())).unwrap();
            let intent = staged.intent().clone();
            let publication = staged.publish(&|| Ok(()));
            let complete = inspect_publication_after_directory_seals(&publication, &intent, &[], &|| Ok(())).unwrap();
            assert_eq!(complete.current, CurrentState::Published);
            fence_publication_after_read(&publication, &intent, &[], &|| Ok(())).unwrap();
            assert_eq!(fs::read(target).unwrap(), data);
        }
    }
}
#[test]
fn retained_final_read_fence_refuses_same_byte_target_payload_and_control_replacements() {
    for name in ["target", PAYLOAD, SLOT, INTENT, DISPATCH, RECEIPT] {
        let (root, target, id) = world();
        old(&target);
        let staged = stage(&target, &id, b"complete original payload", &|| Ok(())).unwrap();
        let intent = staged.intent().clone();
        let publication = staged.publish(&|| Ok(()));
        assert_eq!(inspect_publication_after_directory_seals(&publication, &intent, &[], &|| Ok(())).unwrap().current, CurrentState::Published);
        let path = if name == "target" { target } else { publication.directory.join(name) };
        let bytes = fs::read(&path).unwrap();
        let permissions = fs::metadata(&path).unwrap().permissions();
        fs::rename(&path, root.join("held-original-generation")).unwrap();
        fs::write(&path, &bytes).unwrap();
        fs::set_permissions(&path, permissions).unwrap();
        assert!(fence_publication_after_read(&publication, &intent, &[], &|| Ok(())).is_err(), "{name}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(root.join("held-original-generation").is_file());
    }
}
#[test]
fn retained_final_read_fence_refuses_unknown_public_descriptions_and_stop() {
    let (_root, target, id) = world();
    let staged = stage(&target, &id, b"complete payload", &|| Ok(())).unwrap();
    let intent = staged.intent().clone();
    let mut publication = staged.publish(&|| Ok(()));
    inspect_publication_after_directory_seals(&publication, &intent, &[], &|| Ok(())).unwrap();
    assert!(fence_publication_after_read(&publication, &intent, &[], &|| Err(error("controlled Stop"))).is_err());
    let mut foreign = intent.clone();
    foreign.payload_archive.sha256 = "0".repeat(64);
    assert!(fence_publication_after_read(&publication, &foreign, &[], &|| Ok(())).is_err());
    let receipt = publication.receipt.clone();
    publication.receipt.error = Some("x".repeat(MAX_METADATA as usize + 1));
    assert!(fence_publication_after_read(&publication, &intent, &[], &|| Ok(())).is_err());
    publication.receipt = receipt;
    fence_publication_after_read(&publication, &intent, &[], &|| Ok(())).unwrap();
    let staged = stage(&target, &format!("{id}_unacknowledged"), b"later", &|| Ok(())).unwrap();
    let intent = staged.intent().clone();
    let uncertain =
        staged.publish_with(&|| Ok(()), &mut |point| if matches!(point, Point::AfterKernel) { Err(error("lost original acknowledgement")) } else { Ok(()) });
    assert_eq!(uncertain.receipt.outcome, Outcome::Uncertain);
    assert!(fence_publication_after_read(&uncertain, &intent, &[], &|| Ok(())).is_err());
}
fn world() -> (PathBuf, PathBuf, String) {
    let root = std::env::temp_dir().join(format!(
        "format-retained-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let target = root.join("Current.fcproj");
    let id = format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst));
    (root, target, id)
}
fn old(target: &Path) {
    fs::write(target, b"old complete human document").unwrap();
}
#[test]
fn retained_publication_handles_new_existing_empty_and_chunked_complete_payloads_with_independent_archive_and_exact_displaced_inode() {
    for exists in [false, true] {
        for data in [Vec::new(), b"new complete native document".to_vec(), vec![0x6b; 131075]] {
            let (_root, target, id) = world();
            if exists {
                old(&target);
            }
            let previous = fs::symlink_metadata(&target).ok();
            let staged = stage(&target, &id, &data, &|| Ok(())).unwrap();
            let dir = staged.directory().to_path_buf();
            assert_eq!(fs::read(dir.join(PAYLOAD)).unwrap(), data);
            assert_eq!(fs::read(dir.join(SLOT)).unwrap(), data);
            assert_ne!(fs::metadata(dir.join(PAYLOAD)).unwrap().ino(), fs::metadata(dir.join(SLOT)).unwrap().ino());
            assert_eq!(target.exists(), exists);
            let published = staged.publish(&|| Ok(()));
            assert_eq!(published.receipt.outcome, Outcome::Published);
            assert!(published.receipt_durable);
            assert!(published.receipt.directories_synced);
            assert_eq!(fs::read(&target).unwrap(), data);
            assert_eq!(fs::read(dir.join(PAYLOAD)).unwrap(), data);
            if let Some(before) = previous {
                assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"old complete human document");
                assert_eq!(fs::metadata(dir.join(SLOT)).unwrap().ino(), before.ino());
            } else {
                assert!(!dir.join(SLOT).exists());
            }
            let recovered = inspect(&dir, &|| Ok(())).unwrap();
            assert_eq!(recovered.current, CurrentState::Published);
            assert!(!recovered.write_authority);
            assert!(!recovered.startup_authority);
            assert!(!recovered.native_undo);
        }
    }
}
#[test]
fn retained_changed_preflight_refuses_before_dispatch_and_preserves_later_destination_and_all_staged_bytes() {
    let (_root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    fs::write(&target, b"later human bytes").unwrap();
    let r = staged.publish(&|| Ok(()));
    assert_eq!(r.receipt.outcome, Outcome::NotPublished);
    assert!(!r.receipt.attempted);
    assert!(r.receipt_durable);
    assert!(!dir.join(DISPATCH).exists());
    assert_eq!(fs::read(&target).unwrap(), b"later human bytes");
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"new native bytes");
    assert_eq!(inspect(&dir, &|| Ok(())).unwrap().current, CurrentState::Changed);
}
#[test]
fn retained_exchange_preserves_a_foreign_replacement_in_the_kernel_gap_and_never_claims_an_ordinary_success_or_swaps_back() {
    let (root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let mut called = 0;
    let r = staged.publish_with(&|| Ok(()), &mut |point| {
        if matches!(point, Point::BeforeKernel) {
            called += 1;
            fs::rename(&target, root.join("earlier-human-held"))?;
            fs::write(&target, b"foreign exact-gap document")?;
        }
        Ok(())
    });
    assert_eq!(called, 1);
    assert_eq!(r.receipt.outcome, Outcome::ConflictRetained);
    assert!(r.receipt_durable);
    assert_eq!(fs::read(&target).unwrap(), b"new native bytes");
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"foreign exact-gap document");
    assert_eq!(fs::read(root.join("earlier-human-held")).unwrap(), b"old complete human document");
    assert_eq!(inspect(&dir, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"foreign exact-gap document");
}
#[test]
fn retained_no_replace_preserves_a_destination_created_after_the_final_check_without_a_rename_fallback() {
    let (_root, target, id) = world();
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let r = staged.publish_with(&|| Ok(()), &mut |p| {
        if matches!(p, Point::BeforeKernel) {
            fs::write(&target, b"late foreign creation")?;
        }
        Ok(())
    });
    assert_eq!(r.receipt.outcome, Outcome::NotPublished);
    assert!(r.receipt.attempted);
    assert_eq!(fs::read(&target).unwrap(), b"late foreign creation");
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"new native bytes");
    assert_eq!(fs::read(dir.join(PAYLOAD)).unwrap(), b"new native bytes");
}
#[test]
fn retained_later_replacement_and_in_place_writer_keep_their_files_and_the_independent_payload_without_reverse_exchange() {
    for replace in [false, true] {
        let (root, target, id) = world();
        old(&target);
        let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
        let dir = staged.directory().to_path_buf();
        let r = staged.publish_with(&|| Ok(()), &mut |p| {
            if matches!(p, Point::AfterKernel) {
                if replace {
                    fs::rename(&target, root.join("new-native-held"))?;
                }
                fs::write(&target, b"later human work wins its current name")?;
            }
            Ok(())
        });
        assert_eq!(r.receipt.outcome, Outcome::Uncertain);
        assert_eq!(fs::read(&target).unwrap(), b"later human work wins its current name");
        assert_eq!(fs::read(dir.join(PAYLOAD)).unwrap(), b"new native bytes");
        assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"old complete human document");
        assert_eq!(inspect(&dir, &|| Ok(())).unwrap().current, CurrentState::Changed);
    }
}
#[test]
fn retained_lost_reply_after_kernel_publication_is_resolved_by_fresh_observation_without_a_second_dispatch() {
    let (_root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let mut calls = 0;
    let r = staged.publish_with(&|| Ok(()), &mut |p| {
        if matches!(p, Point::BeforeKernel) {
            calls += 1;
        }
        if matches!(p, Point::AfterKernel) {
            return Err(error("generated lost local publication continuation"));
        }
        Ok(())
    });
    assert_eq!(calls, 1);
    assert_eq!(r.receipt.outcome, Outcome::Uncertain);
    assert!(r.receipt_durable);
    let before = fs::metadata(&target).unwrap();
    for _ in 0..2 {
        let r = inspect(&dir, &|| Ok(())).unwrap();
        assert_eq!(r.current, CurrentState::Published);
        assert_eq!(r.recorded_receipt.unwrap().outcome, Outcome::Uncertain);
        assert!(!r.write_authority);
    }
    assert_eq!(fs::metadata(&target).unwrap().ino(), before.ino());
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"old complete human document");
}
#[test]
fn retained_receipt_publication_failure_preserves_a_foreign_record_and_reports_uncertainty_after_actual_file_publication() {
    let (_root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let r = staged.publish_with(&|| Ok(()), &mut |p| {
        if matches!(p, Point::BeforeReceipt) {
            fs::write(dir.join(RECEIPT), b"foreign receipt must not be overwritten")?;
        }
        Ok(())
    });
    assert!(!r.receipt_durable);
    assert_eq!(r.receipt.outcome, Outcome::Uncertain);
    assert_eq!(fs::read(dir.join(RECEIPT)).unwrap(), b"foreign receipt must not be overwritten");
    assert_eq!(fs::read(&target).unwrap(), b"new native bytes");
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"old complete human document");
    assert!(inspect(&dir, &|| Ok(())).is_err());
}
#[test]
fn retained_stop_before_and_after_kernel_publication_never_replays_or_deletes_a_possible_saved_file() {
    for after in [false, true] {
        let (_root, target, id) = world();
        old(&target);
        let stop = AtomicBool::new(false);
        let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
        let dir = staged.directory().to_path_buf();
        let check = || if stop.load(Ordering::Acquire) { Err(error("generated original Stop/deadline")) } else { Ok(()) };
        let r = staged.publish_with(&check, &mut |p| {
            if matches!(p, Point::BeforeKernel) && !after || matches!(p, Point::AfterKernel) && after {
                stop.store(true, Ordering::Release);
            }
            Ok(())
        });
        assert_eq!(r.receipt.outcome, if after { Outcome::Uncertain } else { Outcome::NotPublished });
        assert_eq!(r.receipt.attempted, after);
        let v = inspect(&dir, &|| Ok(())).unwrap();
        assert_eq!(v.current, if after { CurrentState::Published } else { CurrentState::NotPublished });
        assert_eq!(fs::read(&target).unwrap(), if after { b"new native bytes".as_slice() } else { b"old complete human document".as_slice() });
    }
}
#[test]
fn retained_cancelled_or_changed_stage_never_publishes_and_keeps_its_partial_and_foreign_artifacts() {
    let (root, target, id) = world();
    old(&target);
    let calls = AtomicU64::new(0);
    let result = stage(&target, &id, &vec![0x77; 131075], &|| {
        if calls.fetch_add(1, Ordering::SeqCst) > 9 { Err(error("generated stop during complete stage")) } else { Ok(()) }
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&target).unwrap(), b"old complete human document");
    assert!(root.exists());
    for which in [PAYLOAD, SLOT] {
        let (_root, target, id) = world();
        old(&target);
        let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
        let dir = staged.directory().to_path_buf();
        fs::write(dir.join(which), b"foreign staging bytes").unwrap();
        let r = staged.publish(&|| Ok(()));
        assert_eq!(r.receipt.outcome, Outcome::NotPublished);
        assert_eq!(fs::read(&target).unwrap(), b"old complete human document");
        assert_eq!(fs::read(dir.join(which)).unwrap(), b"foreign staging bytes");
    }
}
#[test]
fn retained_paths_refuse_redirected_leaves_ancestors_and_nonregular_inputs_without_following_them() {
    use std::os::unix::fs::symlink;
    let (root, target, id) = world();
    let original = root.join("original");
    old(&original);
    symlink(&original, &target).unwrap();
    assert!(stage(&target, &id, b"no write", &|| Ok(())).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"old complete human document");
    let alias = root.with_extension("alias");
    symlink(&root, &alias).unwrap();
    assert!(stage(&alias.join("output"), &id, b"no write", &|| Ok(())).is_err());
    fs::create_dir(root.join("directory-target")).unwrap();
    assert!(stage(&root.join("directory-target"), &id, b"no write", &|| Ok(())).is_err());
    assert!(stage(Path::new("relative.fcproj"), &id, b"no write", &|| Ok(())).is_err());
}
#[test]
fn retained_directory_replacement_refuses_adoption_and_never_deletes_or_writes_through_the_replacement() {
    let (_root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let held = dir.with_extension("held");
    fs::rename(&dir, &held).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("foreign"), b"later directory").unwrap();
    let r = staged.publish(&|| Ok(()));
    assert_eq!(r.receipt.outcome, Outcome::Uncertain);
    assert!(!r.receipt.attempted);
    assert!(!r.receipt_durable);
    assert_eq!(fs::read(dir.join("foreign")).unwrap(), b"later directory");
    assert_eq!(fs::read(held.join(PAYLOAD)).unwrap(), b"new native bytes");
    assert_eq!(fs::read(&target).unwrap(), b"old complete human document");
    assert!(inspect(&dir, &|| Ok(())).is_err());
}
#[test]
fn retained_immutable_private_metadata_and_same_operation_collision_cannot_be_adopted_or_used_as_a_second_dispatch() {
    for extra in [false, true] {
        let (_root, target, id) = world();
        old(&target);
        let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
        let dir = staged.directory().to_path_buf();
        assert!(stage(&target, &id, b"different second payload", &|| Ok(())).is_err());
        if extra {
            let mut v: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(INTENT)).unwrap()).unwrap();
            v["write_authority"] = serde_json::json!(true);
            fs::write(dir.join(INTENT), serde_json::to_vec(&v).unwrap()).unwrap();
        } else {
            fs::set_permissions(dir.join(INTENT), fs::Permissions::from_mode(0o644)).unwrap();
        }
        let r = staged.publish(&|| Ok(()));
        assert_eq!(r.receipt.outcome, Outcome::NotPublished);
        assert_eq!(fs::read(&target).unwrap(), b"old complete human document");
        assert!(inspect(&dir, &|| Ok(())).is_err());
    }
}
#[test]
fn retained_process_exit_immediately_after_exchange_leaves_a_read_only_recoverable_operation_with_both_original_and_new_bytes() {
    const KEY: &str = "POEMA_RETAINED_CRASH_ROOT";
    const ID: &str = "cccccccccccccccccccccccccccccccc";
    if let Some(root) = std::env::var_os(KEY) {
        let root = PathBuf::from(root);
        assert_eq!(fs::canonicalize(&root).unwrap(), root);
        assert!(root.file_name().unwrap().to_string_lossy().starts_with("format-retained-"));
        assert_eq!(fs::read(root.join("generated-marker")).unwrap(), b"poema-retained-publication-generated-test-v1\n");
        let staged = stage(&root.join("Current.fcproj"), ID, b"new crash-owned bytes", &|| Ok(())).unwrap();
        let _ = staged.publish_with(&|| Ok(()), &mut |p| {
            if matches!(p, Point::AfterKernel) {
                std::process::exit(71);
            }
            Ok(())
        });
        panic!("generated crash boundary was not reached");
    }
    let (root, target, _) = world();
    old(&target);
    fs::write(root.join("generated-marker"), b"poema-retained-publication-generated-test-v1\n").unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "retained::tests::retained_process_exit_immediately_after_exchange_leaves_a_read_only_recoverable_operation_with_both_original_and_new_bytes",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KEY, &root)
        .output()
        .unwrap();
    assert_eq!(child.status.code(), Some(71), "{}", String::from_utf8_lossy(&child.stderr));
    fs::write(root.join("child-output.log"), [child.stdout, child.stderr].concat()).unwrap();
    let directory = root.join(format!(".poema-save-{ID}.incomplete"));
    let before = fs::metadata(&target).unwrap();
    for _ in 0..2 {
        let r = inspect(&directory, &|| Ok(())).unwrap();
        assert_eq!(r.current, CurrentState::Published);
        assert!(r.dispatch);
        assert!(r.recorded_receipt.is_none());
        assert!(!r.write_authority);
        assert!(!r.startup_authority);
    }
    assert_eq!(fs::read(&target).unwrap(), b"new crash-owned bytes");
    assert_eq!(fs::read(directory.join(SLOT)).unwrap(), b"old complete human document");
    assert_eq!(fs::metadata(&target).unwrap().ino(), before.ino());
    assert!(!directory.join(RECEIPT).exists());
}
#[test]
fn retained_open_original_file_writer_after_exchange_is_kept_as_a_displaced_conflict_with_the_complete_payload_separate() {
    let (_root, target, id) = world();
    old(&target);
    let mut original = fs::OpenOptions::new().write(true).open(&target).unwrap();
    let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
    let dir = staged.directory().to_path_buf();
    let r = staged.publish_with(&|| Ok(()), &mut |p| {
        if matches!(p, Point::AfterKernel) {
            original.seek(SeekFrom::Start(0))?;
            original.write_all(b"human wrote original open file")?;
            original.set_len(30)?;
            original.sync_all()?;
        }
        Ok(())
    });
    assert_eq!(r.receipt.outcome, Outcome::ConflictRetained);
    assert_eq!(fs::read(&target).unwrap(), b"new native bytes");
    assert_eq!(fs::read(dir.join(SLOT)).unwrap(), b"human wrote original open file");
    assert_eq!(fs::read(dir.join(PAYLOAD)).unwrap(), b"new native bytes");
    assert_eq!(inspect(&dir, &|| Ok(())).unwrap().current, CurrentState::ConflictRetained);
}
#[test]
fn retained_inspection_fences_earlier_payload_output_and_receipt_after_later_reads_and_never_renews_changed_evidence() {
    for which in [PAYLOAD, "output", RECEIPT] {
        let (_root, target, id) = world();
        old(&target);
        let staged = stage(&target, &id, b"new native bytes", &|| Ok(())).unwrap();
        let dir = staged.directory().to_path_buf();
        assert_eq!(staged.publish(&|| Ok(())).receipt.outcome, Outcome::Published);
        let changed = if which == "output" { target.clone() } else { dir.join(which) };
        let result = inspect_with(&dir, &|| Ok(()), &mut || {
            fs::write(&changed, b"later bytes during final observation")?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(changed).unwrap(), b"later bytes during final observation");
    }
}

#[test]
fn original_publication_control_generations_refuse_same_byte_intent_dispatch_and_receipt_replacements() {
    for name in [INTENT, DISPATCH, RECEIPT] {
        let (_root, target, id) = world();
        old(&target);
        let published = stage(&target, &id, b"new original native bytes", &|| Ok(())).unwrap().publish(&|| Ok(()));
        assert_eq!(published.receipt.outcome, Outcome::Published);
        published.controls_current(&|| Ok(())).unwrap();
        let original = published.directory.join(name);
        let replacement = published.directory.join("same-byte-replacement");
        fs::write(&replacement, fs::read(&original).unwrap()).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, &original).unwrap();
        assert!(published.controls_current(&|| Ok(())).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"new original native bytes");
    }
}
#[test]
fn original_staged_intent_generation_replacement_refuses_before_dispatch_without_repinning() {
    let (_root, target, id) = world();
    old(&target);
    let staged = stage(&target, &id, b"new staged native bytes", &|| Ok(())).unwrap();
    let directory = staged.directory().to_owned();
    let original = directory.join(INTENT);
    let replacement = directory.join("later-intent");
    fs::write(&replacement, fs::read(&original).unwrap()).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(replacement, &original).unwrap();
    assert_eq!(inspect(&directory, &|| Ok(())).unwrap().current, CurrentState::Prepared);
    let result = staged.publish(&|| Ok(()));
    assert_eq!(result.receipt.outcome, Outcome::NotPublished);
    assert!(!result.receipt.attempted);
    assert!(!directory.join(DISPATCH).exists());
    assert_eq!(fs::read(&target).unwrap(), b"old complete human document");
}
