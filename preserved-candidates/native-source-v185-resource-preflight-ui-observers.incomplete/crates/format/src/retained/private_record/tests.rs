use super::*;
use std::{
    cell::Cell,
    os::unix::fs::{PermissionsExt, symlink},
    time::{SystemTime, UNIX_EPOCH},
};
fn root(tag: &str) -> PathBuf {
    let p =
        std::env::temp_dir().join(format!("private-record-{tag}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&p).unwrap();
    fs::canonicalize(p).unwrap()
}
fn ready(p: &std::path::Path, bytes: &[u8]) -> Record {
    let d = DirectoryFrame::capture(p, &|| Ok(())).unwrap();
    let f = d.node(".recovery.json", &|| Ok(())).unwrap();
    Record::prepare(&f, "original_operation_01234567890123456789", bytes, 1024 * 1024, &|| Ok(())).unwrap()
}
#[test]
fn private_record_preparation_has_no_file_effect_and_publication_uses_private_original_bytes_and_retains_owner() {
    let p = root("basic");
    let data = b"complete original opaque native envelope";
    let mut r = ready(&p, data);
    assert_eq!(fs::read_dir(&p).unwrap().count(), 0);
    assert!(!r.intent().overwrite);
    let receipt = r.publish(data, &|| Ok(()), &|| Ok(())).unwrap();
    assert_eq!(receipt.phase, Phase::Published);
    assert!(receipt.directory_synced);
    assert!(!receipt.native_complete && !receipt.startup_authority && !receipt.workers_may_resume);
    for path in [&receipt.intent.destination, &receipt.intent.candidate] {
        assert_eq!(fs::read(path).unwrap(), data);
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(fs::read_dir(&p).unwrap().count(), 2);
    assert!(r.inspect(&|| Ok(())).unwrap().matches_published);
    assert!(r.publish(data, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read(&receipt.intent.destination).unwrap(), data);
}
#[test]
fn private_record_existing_and_later_human_destinations_are_never_overwritten_or_adopted() {
    let p = root("human");
    let mut r = ready(&p, b"native");
    fs::write(&r.intent().destination, b"later human").unwrap();
    assert!(r.publish(b"native", &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read(&r.intent().destination).unwrap(), b"later human");
    assert!(!r.receipt().candidate_created);
    let d = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let f = d.node(".recovery.json", &|| Ok(())).unwrap();
    assert!(Record::prepare(&f, "original_operation_01234567890123456789", b"native", 0, &|| Ok(())).is_err());
}
#[test]
fn private_record_exclusive_link_race_retains_human_bytes_and_the_complete_private_candidate_without_cleanup() {
    let p = root("race");
    let mut r = ready(&p, b"native full frame");
    let target = r.intent().destination.clone();
    assert!(
        r.publish_with(b"native full frame", &|| Ok(()), &|| Ok(()), &mut |point| {
            if point == Point::BeforeKernel {
                fs::write(&target, b"later human")?;
            }
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), b"later human");
    assert_eq!(fs::read(&r.intent().candidate).unwrap(), b"native full frame");
    assert!(!r.receipt().directory_synced);
    assert!(r.publish(b"native full frame", &|| Ok(()), &|| Ok(())).is_err());
}
#[test]
fn private_record_replaced_parent_and_symlink_refuse_before_writing_into_another_directory() {
    let p = root("redirect");
    let mut r = ready(&p, b"frame");
    let moved = p.with_extension("retained");
    fs::rename(&p, &moved).unwrap();
    fs::create_dir(&p).unwrap();
    assert!(r.publish(b"frame", &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read_dir(&p).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    fs::remove_dir(&p).unwrap();
    symlink(&moved, &p).unwrap();
    assert!(DirectoryFrame::capture(&p, &|| Ok(())).is_err());
}
#[test]
fn private_record_same_byte_foreign_candidate_inode_is_not_lent_original_descriptor_authority() {
    let p = root("inode");
    let data = b"same byte replacement";
    let mut r = ready(&p, data);
    let candidate = r.intent().candidate.clone();
    let old = p.join("retained-original-candidate");
    assert!(
        r.publish_with(data, &|| Ok(()), &|| Ok(()), &mut |point| {
            if point == Point::AfterBytes {
                fs::rename(&candidate, &old)?;
                fs::write(&candidate, data)?;
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        })
        .is_err()
    );
    assert!(!r.intent().destination.exists());
    assert_eq!(fs::read(&old).unwrap(), data);
    assert_eq!(fs::read(&candidate).unwrap(), data);
}
#[test]
fn private_record_Stop_during_complete_bytes_keeps_partial_candidate_and_never_restarts_the_same_owner() {
    let p = root("stop");
    let data = vec![0xa5; 1024 * 1024];
    let mut r = ready(&p, &data);
    let candidate = r.intent().candidate.clone();
    let stopped = Cell::new(false);
    let check = || {
        if fs::metadata(&candidate).is_ok_and(|v| v.len() >= 65536) {
            stopped.set(true);
        }
        if stopped.get() { Err(error("original Stop")) } else { Ok(()) }
    };
    assert!(r.publish(&data, &check, &|| Ok(())).is_err());
    let size = fs::metadata(&candidate).unwrap().len();
    assert!(size >= 65536 && size < data.len() as u64);
    assert!(!r.intent().destination.exists());
    assert!(r.publish(&data, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::metadata(&candidate).unwrap().len(), size);
    assert!(!r.inspect(&|| Ok(())).unwrap().matches_published);
}
#[test]
fn private_record_unconfirmed_directory_sync_retains_exact_link_and_refuses_publication_replay() {
    let p = root("fsync");
    let mut r = ready(&p, b"complete");
    assert!(
        r.publish_with(b"complete", &|| Ok(()), &|| Ok(()), &mut |point| {
            if point == Point::BeforeDirectorySync { Err(error("directory sync interrupted")) } else { Ok(()) }
        })
        .is_err()
    );
    assert_eq!(r.receipt().phase, Phase::LinkUnconfirmed);
    assert!(r.receipt().link_attempted);
    assert!(!r.receipt().directory_synced);
    assert_eq!(fs::read(&r.intent().destination).unwrap(), b"complete");
    assert!(r.publish(b"complete", &|| Ok(()), &|| Ok(())).is_err());
    let observed = r.inspect(&|| Ok(())).unwrap();
    assert!(!observed.matches_published && !observed.receipt.native_complete);
}
#[test]
fn private_record_lost_reply_observation_detects_later_human_same_byte_inode_without_republication() {
    let p = root("lost");
    let mut r = ready(&p, b"native");
    r.publish(b"native", &|| Ok(()), &|| Ok(())).unwrap();
    assert!(r.inspect(&|| Ok(())).unwrap().matches_published);
    let old = p.join("displaced-native");
    fs::rename(&r.intent().destination, &old).unwrap();
    fs::write(&r.intent().destination, b"native").unwrap();
    fs::set_permissions(&r.intent().destination, fs::Permissions::from_mode(0o600)).unwrap();
    let observed = r.inspect(&|| Ok(())).unwrap();
    assert!(!observed.matches_published && !observed.current && !observed.write_authority);
    assert_eq!(fs::read(&old).unwrap(), b"native");
    assert!(r.publish(b"native", &|| Ok(()), &|| Ok(())).is_err());
}
#[test]
fn private_record_full_original_eighty_MiB_bound_is_written_without_reducing_the_payload() {
    let p = root("full-bound");
    let data = vec![0x5a; MAX_RECORD_BYTES as usize];
    let mut r = ready(&p, &data);
    r.publish(&data, &|| Ok(()), &|| Ok(())).unwrap();
    assert_eq!(fs::metadata(&r.intent().destination).unwrap().len(), MAX_RECORD_BYTES);
    assert!(r.inspect(&|| Ok(())).unwrap().matches_published);
}
#[test]
fn private_record_bound_reserve_and_changed_payload_fail_before_any_file_is_created() {
    let p = root("bounds");
    let d = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let f = d.node(".recovery.json", &|| Ok(())).unwrap();
    assert!(Record::prepare(&f, "original_operation_01234567890123456789", b"small", u64::MAX, &|| Ok(())).is_err());
    assert!(Record::prepare(&f, "original_operation_01234567890123456789", &[], 0, &|| Ok(())).is_err());
    let data = vec![0; MAX_RECORD_BYTES as usize + 1];
    assert!(Record::prepare(&f, "original_operation_01234567890123456789", &data, 0, &|| Ok(())).is_err());
    let mut r = ready(&p, b"before");
    assert!(r.publish(b"after!", &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(fs::read_dir(&p).unwrap().count(), 0);
}

#[test]
fn private_record_last_boundary_in_place_change_refuses_before_the_exclusive_namespace_link() {
    let p = root("last-boundary");
    let mut r = ready(&p, b"native");
    let candidate = r.intent().candidate.clone();
    assert!(
        r.publish_with(b"native", &|| Ok(()), &|| Ok(()), &mut |point| {
            if point == Point::BeforeKernel {
                fs::write(&candidate, b"tamper")?;
            }
            Ok(())
        })
        .is_err()
    );
    assert!(!r.intent().destination.exists(), "changed candidate must never enter the published namespace");
    assert_eq!(fs::read(candidate).unwrap(), b"tamper");
    assert!(r.publish(b"native", &|| Ok(()), &|| Ok(())).is_err());
}
