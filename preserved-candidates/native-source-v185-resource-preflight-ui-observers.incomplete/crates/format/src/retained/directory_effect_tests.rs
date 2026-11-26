use super::*;
use std::{
    fs,
    os::unix::fs::symlink,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "preview-directory-effects-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&p).unwrap();
    fs::canonicalize(p).unwrap()
}
#[test]
fn original_missing_chain_and_existing_target_are_observed_without_claiming_directory_custody() {
    let root = root();
    let target = root.join("missing/deep");
    let frame = DirectoryFrame::capture(&target, &|| Ok(())).unwrap();
    let p = stage(frame, &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert!(p.complete);
    assert_eq!(p.successful_calls, vec![root.join("missing"), target.clone()]);
    assert_eq!(p.observed.len(), 2);
    assert!(!p.custody && !p.write_authority);
    let existing = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert!(existing.complete);
    assert!(existing.successful_calls.is_empty());
    assert!(existing.observed.is_empty());
}
#[test]
fn later_foreign_directory_is_not_adopted_or_removed() {
    let root = root();
    let target = root.join("new");
    let staged = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"later human data").unwrap();
    let p = staged.publish(&|| Ok(()));
    assert!(!p.complete);
    assert!(p.successful_calls.is_empty());
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"later human data");
}
#[test]
fn partial_Stop_keeps_exact_successful_mkdir_calls_and_does_not_retry_or_clean_up() {
    let root = root();
    let target = root.join("one/two");
    let p = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(()))
        .unwrap()
        .publish_with(&|| Ok(()), &mut |_| Err(error("generated Stop after actual mkdir")));
    assert!(!p.complete);
    assert_eq!(p.successful_calls, vec![root.join("one")]);
    assert!(p.observed.is_empty());
    assert!(root.join("one").is_dir());
    assert!(!target.exists());
    assert!(!p.custody);
}
#[test]
fn replaced_known_parent_stops_before_any_directory_is_created() {
    let root = root();
    fs::create_dir(root.join("parent")).unwrap();
    let path = root.join("parent/new");
    let staged = stage(DirectoryFrame::capture(&path, &|| Ok(())).unwrap(), &|| Ok(())).unwrap();
    fs::rename(root.join("parent"), root.join("held")).unwrap();
    fs::create_dir(root.join("parent")).unwrap();
    let p = staged.publish(&|| Ok(()));
    assert!(!p.complete);
    assert!(p.successful_calls.is_empty());
    assert!(!path.exists());
    assert!(!root.join("held/new").exists());
}
#[test]
fn symlink_after_mkdir_never_redirects_the_next_child_and_partial_evidence_remains() {
    let root = root();
    let elsewhere = root.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let path = root.join("one/two");
    let staged = stage(DirectoryFrame::capture(&path, &|| Ok(())).unwrap(), &|| Ok(())).unwrap();
    let p = staged.publish_with(&|| Ok(()), &mut |made| {
        fs::rename(made, root.join("held"))?;
        symlink(&elsewhere, made)
    });
    assert!(!p.complete);
    assert_eq!(p.successful_calls, vec![root.join("one")]);
    assert!(!elsewhere.join("two").exists());
    assert!(root.join("held").is_dir());
}

#[test]
fn owned_directory_seal_keeps_readonly_authored_copy_and_immutable_copy_evidence() {
    use crate::retained::relocation::copying;
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let source = root.join("original.json");
    fs::write(&source, b"authored complete bytes").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let target = root.join("Authored");
    let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    let output = target.join("current.json");
    let staged = copying::stage(&source, &output, "generated-authored-copy-operation-0001", &|| Ok(())).unwrap();
    let intent = fs::read(staged.directory().join("intent.json")).unwrap();
    let copy = staged.publish(&|| Ok(()));
    copy.verify_current(&|| Ok(())).unwrap();
    let receipt = fs::read(copy.directory.join("receipt.json")).unwrap();
    let seals = vec![owner.seal(0o550, &|| Ok(())).unwrap()];
    assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o7777, 0o550);
    assert_eq!(fs::metadata(&output).unwrap().permissions().mode() & 0o7777, 0o640);
    assert!(copy.verify_current(&|| Ok(())).is_err());
    copy.verify_current_after_directory_seals(&seals, &|| Ok(())).unwrap();
    assert_eq!(fs::read(copy.directory.join("intent.json")).unwrap(), intent);
    assert_eq!(fs::read(copy.directory.join("receipt.json")).unwrap(), receipt);
    assert_eq!(fs::read(&source).unwrap(), b"authored complete bytes");
    assert_eq!(fs::read(&output).unwrap(), b"authored complete bytes");
    // Complete same-byte replacement remains foreign after the intentional mode effect.
    fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
    fs::rename(&output, root.join("held-output")).unwrap();
    fs::write(&output, b"authored complete bytes").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o550)).unwrap();
    assert!(copy.verify_current_after_directory_seals(&seals, &|| Ok(())).is_err());
}
#[test]
fn owned_directory_seal_preserves_canonical_retained_writer_and_refuses_changed_receipt() {
    use crate::retained;
    let root = root();
    let target = root.join("Keyboard Shortcuts");
    let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    let frame = DirectoryFrame::capture(&target, &|| Ok(())).unwrap().node("active.json", &|| Ok(())).unwrap();
    let staged = retained::stage_from_frame(&frame, "generated-canonical-operation-0001", b"native current memory", &|| Ok(())).unwrap();
    let intent = staged.intent().clone();
    let publication = staged.publish(&|| Ok(()));
    let seals = vec![owner.seal(0o750, &|| Ok(())).unwrap()];
    assert!(retained::inspect_expected(&publication.directory, &intent, &|| Ok(())).is_err());
    let seen = retained::inspect_expected_after_directory_seals(&publication.directory, &intent, &seals, &|| Ok(())).unwrap();
    assert_eq!(seen.current, retained::CurrentState::Published);
    assert_eq!(seen.recorded_receipt.as_ref(), Some(&publication.receipt));
    assert!(!seen.write_authority && !seen.startup_authority && !seen.native_undo);
    let receipt = publication.directory.join("receipt.json");
    fs::write(&receipt, b"{}").unwrap();
    assert!(retained::inspect_expected_after_directory_seals(&publication.directory, &intent, &seals, &|| Ok(())).is_err());
}
#[test]
fn owned_directory_seal_refuses_existing_multilevel_or_invalid_mode() {
    let root = root();
    assert!(stage(DirectoryFrame::capture(&root, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).is_err());
    assert!(stage(DirectoryFrame::capture(&root.join("missing/deep"), &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).is_err());
    assert!(!root.join("missing").exists());
    let target = root.join("new");
    let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    let before = fs::metadata(&target).unwrap();
    assert!(owner.seal(0o10000, &|| Ok(())).is_err());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&target).unwrap().permissions().mode(), before.permissions().mode());
}
#[test]
fn owned_directory_seal_refuses_replaced_directory_and_foreign_mode_before_effect() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    for replace in [true, false] {
        let target = root.join(if replace { "replace" } else { "foreign-mode" });
        let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
        if replace {
            fs::rename(&target, root.join("held")).unwrap();
            fs::create_dir(&target).unwrap();
        } else {
            fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        }
        // Choose a definitely different mode even under a private process umask.
        if !replace && owner.current(&|| Ok(())).is_ok() {
            fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let before = fs::metadata(&target).unwrap().permissions().mode();
        assert!(owner.seal(0o550, &|| Ok(())).is_err());
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode(), before);
    }
}
#[test]
fn owned_directory_seal_rejects_foreign_generation_after_seal_and_stop_keeps_effect() {
    use std::{cell::Cell, os::unix::fs::PermissionsExt};
    let root = root();
    let target = root.join("sealed");
    let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    let seal = owner.seal(0o750, &|| Ok(())).unwrap();
    seal.current(&|| Ok(())).unwrap();
    fs::rename(&target, root.join("old-sealed")).unwrap();
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(seal.current(&|| Ok(())).is_err());
    let target = root.join("stopped");
    let owner = stage(DirectoryFrame::capture(&target, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    let calls = Cell::new(0);
    assert!(
        owner
            .seal(0o750, &|| {
                let n = calls.get() + 1;
                calls.set(n);
                if n >= 3 { Err(error("generated Stop after actual chmod")) } else { Ok(()) }
            })
            .is_err()
    );
    assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o7777, 0o750);
    assert!(target.is_dir());
}

#[test]
fn owned_empty_private_file_requires_actual_directory_absence_and_retains_named_generation() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = root();
    let path = root.join("journal");
    let owner = stage(DirectoryFrame::capture(&path, &|| Ok(())).unwrap(), &|| Ok(())).unwrap().publish_owned(&|| Ok(())).unwrap();
    for name in ["../escape", "child/escape", ""] {
        assert!(owner.create_empty_private_file(name, &|| Ok(())).is_err());
    }
    let (file, frame) = owner.create_empty_private_file("lock", &|| Ok(())).unwrap();
    assert_eq!(file.metadata().unwrap().len(), 0);
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o7777, 0o600);
    assert!(owner.create_empty_private_file("lock", &|| Ok(())).is_err());
    frame.current(&|| Ok(())).unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    let checks = std::cell::Cell::new(0u32);
    assert!(
        frame
            .current(&|| {
                let n = checks.get() + 1;
                checks.set(n);
                if n > 64 { Err(error("bounded read budget")) } else { Ok(()) }
            })
            .is_err()
    );
    assert!(checks.get() < 64, "grown empty lock must refuse before a bulk read");
    file.set_len(0).unwrap();
    fs::rename(path.join("lock"), root.join("held-lock")).unwrap();
    fs::write(path.join("lock"), []).unwrap();
    assert!(frame.current(&|| Ok(())).is_err());
    symlink(root.join("absent"), path.join("redirect")).unwrap();
    assert!(owner.create_empty_private_file("redirect", &|| Ok(())).is_err());
    assert!(!root.join("absent").exists());
    fs::rename(&path, root.join("held-journal")).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(owner.create_empty_private_file("new-lock", &|| Ok(())).is_err());
    assert!(!path.join("new-lock").exists());
}
