use super::preview_context::DirectoryFrame;
use super::*;
use std::{
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "observed-stage-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&p).unwrap();
    fs::canonicalize(p).unwrap()
}
fn op() -> String {
    format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst))
}
#[test]
fn captured_absent_document_frame_publishes_original_payload_without_reobserving_a_different_target() {
    let r = root();
    let frame = DirectoryFrame::capture(&r, &|| Ok(())).unwrap().node("new.fcproj", &|| Ok(())).unwrap();
    let p = stage_from_frame(&frame, &op(), b"original encoded payload", &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert!(p.receipt_durable);
    assert_eq!(p.receipt.outcome, Outcome::Published);
    assert_eq!(fs::read(frame.path()).unwrap(), b"original encoded payload");
    assert_eq!(inspect(&p.directory, &|| Ok(())).unwrap().current, CurrentState::Published);
}
#[test]
fn captured_existing_document_frame_preserves_displaced_actual_inode_and_independent_new_archive() {
    let r = root();
    let path = r.join("current.fcproj");
    fs::write(&path, b"old original").unwrap();
    let ino = fs::metadata(&path).unwrap().ino();
    let frame = DirectoryFrame::capture(&r, &|| Ok(())).unwrap().node("current.fcproj", &|| Ok(())).unwrap();
    let p = stage_from_frame(&frame, &op(), b"new original", &|| Ok(())).unwrap().publish(&|| Ok(()));
    assert!(p.receipt_durable);
    assert_eq!(p.receipt.outcome, Outcome::Published);
    assert_eq!(fs::metadata(p.directory.join(SLOT)).unwrap().ino(), ino);
    assert_eq!(fs::read(p.directory.join(SLOT)).unwrap(), b"old original");
    assert_eq!(fs::read(p.directory.join(PAYLOAD)).unwrap(), b"new original");
    assert_ne!(fs::metadata(p.directory.join(PAYLOAD)).unwrap().ino(), fs::metadata(&path).unwrap().ino());
}
#[test]
fn later_identical_bytes_at_a_different_inode_refuse_before_any_operation_directory_is_created() {
    let r = root();
    let path = r.join("current.fcproj");
    fs::write(&path, b"same bytes").unwrap();
    let frame = DirectoryFrame::capture(&r, &|| Ok(())).unwrap().node("current.fcproj", &|| Ok(())).unwrap();
    let operation = op();
    fs::rename(&path, r.join("held.fcproj")).unwrap();
    fs::write(&path, b"same bytes").unwrap();
    assert!(stage_from_frame(&frame, &operation, b"new", &|| Ok(())).is_err());
    assert!(!r.join(format!(".poema-save-{operation}.incomplete")).exists());
    assert_eq!(fs::read(path).unwrap(), b"same bytes");
    assert_eq!(fs::read(r.join("held.fcproj")).unwrap(), b"same bytes");
}
#[test]
fn original_parent_replacement_is_not_adopted_even_with_identical_file_contents() {
    let r = root();
    fs::create_dir(r.join("parent")).unwrap();
    fs::write(r.join("parent/current.fcproj"), b"same").unwrap();
    let frame = DirectoryFrame::capture(&r.join("parent"), &|| Ok(())).unwrap().node("current.fcproj", &|| Ok(())).unwrap();
    fs::rename(r.join("parent"), r.join("held")).unwrap();
    fs::create_dir(r.join("parent")).unwrap();
    fs::write(r.join("parent/current.fcproj"), b"same").unwrap();
    assert!(stage_from_frame(&frame, &op(), b"new", &|| Ok(())).is_err());
    assert_eq!(fs::read(r.join("held/current.fcproj")).unwrap(), b"same");
    assert_eq!(fs::read(r.join("parent/current.fcproj")).unwrap(), b"same");
}
#[test]
fn final_stage_open_compares_original_parent_and_target_before_mkdir_after_an_earlier_full_observation() {
    for absent in [true, false] {
        let r = root();
        let path = r.join("current.fcproj");
        if !absent {
            fs::write(&path, b"original").unwrap();
        }
        let frame = DirectoryFrame::capture(&r, &|| Ok(())).unwrap().node("current.fcproj", &|| Ok(())).unwrap();
        frame.current(&|| Ok(())).unwrap();
        let (parent, target) = frame.publication_context().unwrap();
        fs::write(&path, b"later human bytes").unwrap();
        let operation = op();
        assert!(stage_expected(&path, &operation, b"new", Some((&parent, target.as_ref())), &|| Ok(())).is_err());
        assert!(!r.join(format!(".poema-save-{operation}.incomplete")).exists());
        assert_eq!(fs::read(path).unwrap(), b"later human bytes");
    }
}
#[test]
fn missing_original_parent_is_not_implicitly_created_or_adopted_by_document_staging() {
    let r = root();
    let parent = r.join("missing");
    let frame = DirectoryFrame::capture(&parent, &|| Ok(())).unwrap().node("current.fcproj", &|| Ok(())).unwrap();
    assert!(stage_from_frame(&frame, &op(), b"new", &|| Ok(())).is_err());
    assert!(!parent.exists());
}
