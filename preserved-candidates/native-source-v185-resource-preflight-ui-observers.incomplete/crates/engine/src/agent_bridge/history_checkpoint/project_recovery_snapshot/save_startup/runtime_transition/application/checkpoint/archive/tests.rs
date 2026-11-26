use super::super::tests::{fixture, no_writes, stop};
use super::*;
use std::{fs, os::unix::fs::DirBuilderExt};
fn context(s: &Session, p: &Prepared) -> (Value, Value) {
    let mut capture = p.description();
    capture["privateInput"] = json!({"path":"/generated/private/input.json","bytes":100,"sha256":"a".repeat(64)});
    capture["sourceCapture"] = json!("11111111-1111-4111-8111-111111111111");
    capture["afterFrameSha256"] = json!("b".repeat(64));
    capture["savedFile"] = json!(s.saved_file);
    let runtime = json!({"nativePid":std::process::id(),"nativeSession":capture["nativeSession"],"privateInput":capture["privateInput"]});
    (capture, runtime)
}
fn capture(s: &Session, p: &Prepared, reserve: u64) -> Result<Archive> {
    let (c, r) = context(s, p);
    Archive::capture(s, p.encoded.as_ref().unwrap(), &c, &r, reserve)
}
fn root(s: &Session) -> std::path::PathBuf {
    s.persistence.as_ref().unwrap().data_dir.join(format!(".poema-checkpoint-{}", crate::agent_bridge::retained_native_epoch(s).unwrap()))
}
#[test]
fn archive_plan_retains_same_original_worker_parent_and_exact_absence_without_creating_or_writing_files() {
    let (_d, mut s, mut p) = fixture();
    let before = fs::read(s.path.as_ref().unwrap()).unwrap();
    p.encode(&s, &|| Ok(())).unwrap();
    let a = capture(&s, &p, 1024 * 1024).unwrap();
    a.current(s.persistence.as_ref().unwrap()).unwrap();
    let v = a.description();
    assert_eq!(v["archive"]["path"], json!(root(&s)));
    assert_eq!(v["archive"]["missing"], json!([root(&s)]));
    assert_eq!(v["archive"]["anchor"], v["data"]["anchor"]);
    let original = p.encoded.as_ref().unwrap().retained_data_frame(s.persistence.as_ref().unwrap(), &|| Ok(())).unwrap();
    assert!(Arc::ptr_eq(&a.data, &original));
    let files = p.encoded.as_ref().unwrap().files(s.persistence.as_ref().unwrap()).unwrap();
    let unit = v["allocationUnit"].as_u64().unwrap();
    let mut expected = allocation(4096, unit).unwrap() * 3;
    let mut logical = 0;
    for ((name, bytes), row) in files.into_iter().zip(v["files"].as_array().unwrap()) {
        assert_eq!(row["name"], name);
        assert_eq!(row["sha256"], crate::saved_file::sha256(bytes));
        assert_eq!(row["path"], json!(root(&s).join("files").join(name)));
        logical += bytes.len() as u64;
        expected += allocation(bytes.len() as u64, unit).unwrap() * 2 + allocation(65536, unit).unwrap() * 8;
    }
    assert_eq!(v["allocatedAllowance"], expected);
    assert_eq!(v["logicalBytes"], logical);
    assert_eq!(v["directoryMode"], 0o700);
    assert_eq!(v["fileMode"], 0o600);
    for flag in [
        "archiveCreated",
        "filesWritten",
        "workerReleased",
        "writeAuthority",
        "nativeComplete",
        "startupAuthority",
        "workersMayResume",
        "fullApplicationClosure",
    ] {
        assert_eq!(v[flag], false);
    }
    assert!(!root(&s).exists());
    no_writes(&s, &before);
    stop(&mut s);
}
#[test]
fn archive_plan_refuses_existing_directories_files_and_symlinks_without_adopting_or_overwriting_them() {
    for kind in 0..3 {
        let (_d, mut s, mut p) = fixture();
        p.encode(&s, &|| Ok(())).unwrap();
        let r = root(&s);
        match kind {
            0 => fs::DirBuilder::new().mode(0o700).create(&r).unwrap(),
            1 => fs::write(&r, b"foreign").unwrap(),
            _ => std::os::unix::fs::symlink("absent-foreign-target", &r).unwrap(),
        }
        assert!(capture(&s, &p, 1).is_err());
        assert!(fs::symlink_metadata(&r).is_ok());
        if kind == 1 {
            assert_eq!(fs::read(&r).unwrap(), b"foreign");
        }
        stop(&mut s);
    }
}
#[test]
fn archive_plan_refuses_a_later_occupied_name_without_rebaselining_or_removing_foreign_work() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    let a = capture(&s, &p, 1).unwrap();
    fs::DirBuilder::new().mode(0o700).create(root(&s)).unwrap();
    fs::write(root(&s).join("foreign"), b"retained").unwrap();
    assert!(a.current(s.persistence.as_ref().unwrap()).is_err());
    assert_eq!(fs::read(root(&s).join("foreign")).unwrap(), b"retained");
    stop(&mut s);
}
#[test]
fn archive_plan_refuses_replaced_original_data_parent_even_if_the_old_directory_still_exists() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    let a = capture(&s, &p, 1).unwrap();
    let data = s.persistence.as_ref().unwrap().data_dir.clone();
    fs::rename(&data, data.with_file_name("retained-original-native")).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    assert!(a.current(s.persistence.as_ref().unwrap()).is_err());
    assert!(!root(&s).exists());
    stop(&mut s);
}
#[test]
fn archive_plan_cannot_cross_another_actual_worker_or_use_its_encoded_bytes() {
    let (_d, mut first, mut p) = fixture();
    p.encode(&first, &|| Ok(())).unwrap();
    let a = capture(&first, &p, 1).unwrap();
    let (_o, mut second, _other) = fixture();
    assert!(a.current(second.persistence.as_ref().unwrap()).is_err());
    let (c, r) = context(&first, &p);
    assert!(Archive::capture(&second, p.encoded.as_ref().unwrap(), &c, &r, 1).is_err());
    stop(&mut second);
    stop(&mut first);
}
#[test]
fn archive_plan_requires_full_allocated_payload_control_directory_and_reserve_headroom() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    for reserve in [0, u64::MAX, u64::MAX - 1024] {
        assert!(capture(&s, &p, reserve).is_err());
        assert!(!root(&s).exists());
    }
    capture(&s, &p, 1).unwrap();
    assert!(!root(&s).exists());
    stop(&mut s);
}
#[test]
fn archive_plan_loses_currency_after_the_same_worker_stops_or_original_prepare_resume_releases_it() {
    for released in [false, true] {
        let (_d, mut s, mut p) = fixture();
        p.encode(&s, &|| Ok(())).unwrap();
        let a = capture(&s, &p, 1).unwrap();
        if released {
            s.persistence.as_ref().unwrap().release_current_image(|_| Ok(()), || Ok(())).unwrap();
        } else {
            stop(&mut s);
        }
        assert!(a.current(s.persistence.as_ref().unwrap()).is_err());
        assert!(!root(&s).exists());
        stop(&mut s);
    }
}
#[test]
fn archive_intent_attempt_is_once_only_and_unresolved_acknowledgement_cannot_be_described_as_settled() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    let mut a = capture(&s, &p, 1).unwrap();
    let plan = a.description();
    a.intent_resolved().unwrap();
    assert!(a.confirm_intent().is_err());
    assert_eq!(a.begin_intent().unwrap(), plan);
    assert!(a.intent_resolved().is_err());
    assert!(a.begin_intent().is_err());
    a.current(s.persistence.as_ref().unwrap()).unwrap();
    a.confirm_intent().unwrap();
    a.intent_resolved().unwrap();
    assert!(a.confirm_intent().is_err());
    assert!(a.begin_intent().is_err());
    assert!(!root(&s).exists());
    stop(&mut s);
}
#[test]
fn archive_plan_refuses_crossed_native_epoch_pid_private_input_and_worker_directory() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    for kind in 0..4 {
        let (mut c, mut r) = context(&s, &p);
        match kind {
            0 => r["nativeSession"] = json!("f".repeat(32)),
            1 => r["nativePid"] = json!(1),
            2 => r["privateInput"]["bytes"] = json!(101),
            _ => c["nativeData"] = json!("/foreign/native"),
        }
        assert!(Archive::capture(&s, p.encoded.as_ref().unwrap(), &c, &r, 1).is_err());
        assert!(!root(&s).exists());
    }
    stop(&mut s);
}
#[test]
fn archive_plan_preserves_and_refuses_unknown_work_in_the_original_gated_recovery_session() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    let foreign = s.persistence.as_ref().unwrap().session_dir.join("foreign");
    fs::write(&foreign, b"keep").unwrap();
    assert!(capture(&s, &p, 1).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), b"keep");
    assert!(!root(&s).exists());
    stop(&mut s);
}

#[test]
fn prepared_controller_publication_requires_original_archive_intent_and_retains_actual_worker_receipts_and_cow_guard() {
    let (_d, mut s, mut p) = fixture();
    let original = fs::read(s.path.as_ref().unwrap()).unwrap();
    p.encode(&s, &|| Ok(())).unwrap();
    p.archive = Some(capture(&s, &p, 1024 * 1024).unwrap());
    p.archive_attempted = true;
    // This composes the original Prepared/Archive and worker only. The real
    // Pending still independently requires its Input, canonical and source.
    let source: crate::autosave::held_checkpoint::publication::Source = Arc::new(|| Ok(()));
    assert!(p.publish(&s, source.clone(), &|| Ok(())).is_err());
    assert!(!p.archive.as_ref().unwrap().publication_attempted);
    assert!(p.encoded.is_some());
    let plan = p.begin_archive_intent(&s).unwrap();
    p.confirm_archive_intent().unwrap();
    let result = p.publish(&s, source.clone(), &|| Ok(())).unwrap();
    assert_eq!(result["filesWritten"], true);
    assert_eq!(result["completeAuxiliaryReadback"], true);
    assert_eq!(result["workerReleased"], false);
    assert!(p.encoded.is_none());
    assert!(p.archive.as_ref().unwrap().has_publication());
    p.current(&s).unwrap();
    assert!(p.publish(&s, source, &|| Ok(())).is_err());
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), original);
    assert_eq!(plan["archive"]["path"], json!(root(&s)));
    s.execute("file.newBin", json!({"name":"Later live edit"})).unwrap();
    assert!(p.current(&s).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
#[test]
fn prepared_controller_retains_unresolved_publication_after_source_loss_and_never_reconstructs_consumed_encoded_input() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    p.archive = Some(capture(&s, &p, 1024 * 1024).unwrap());
    p.archive_attempted = true;
    p.begin_archive_intent(&s).unwrap();
    p.confirm_archive_intent().unwrap();
    let path = root(&s);
    let effect = path.clone();
    let lost = Arc::new(AtomicBool::new(false));
    let flag = lost.clone();
    let source = Arc::new(move || {
        if effect.exists() {
            flag.store(true, Ordering::Release);
            return Err(io::Error::other("generated original-source loss"));
        }
        Ok(())
    });
    assert!(p.publish(&s, source, &|| Ok(())).is_err());
    assert!(lost.load(Ordering::Acquire));
    assert!(path.is_dir());
    assert!(p.encoded.is_none());
    assert!(p.resolved().is_err());
    assert!(p.current(&s).is_err());
    assert!(p.publish(&s, Arc::new(|| Ok(())), &|| Ok(())).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}

#[test]
fn original_prepared_checkpoint_observation_retains_actual_publication_descriptor_and_blocks_release_until_complete_readback() {
    let (_d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    p.archive = Some(capture(&s, &p, 1024 * 1024).unwrap());
    p.archive_attempted = true;
    p.begin_archive_intent(&s).unwrap();
    p.confirm_archive_intent().unwrap();
    let published = p.publish(&s, Arc::new(|| Ok(())), &|| Ok(())).unwrap();
    let event = p.begin_publication_observation(&s).unwrap();
    assert_eq!(event["kind"], "native-checkpoint-archive-observation");
    assert_eq!(event["publication"], published);
    assert!(p.current(&s).is_err());
    assert!(p.begin_publication_observation(&s).is_err());
    // Original Prepared/Archive/worker composition only: actual Pending must
    // first validate its real host successor and advance its typed witness.
    p.confirm_publication_observation(&s).unwrap();
    p.current(&s).unwrap();
    assert!(p.confirm_publication_observation(&s).is_err());
    assert!(p.begin_publication_observation(&s).is_err());
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    assert!(s.persistence.as_ref().unwrap().last_sent.is_none());
    stop(&mut s);
}
#[test]
fn original_checkpoint_observation_refuses_same_byte_replaced_output_and_retains_unknown_attempt_without_replay() {
    let (d, mut s, mut p) = fixture();
    p.encode(&s, &|| Ok(())).unwrap();
    p.archive = Some(capture(&s, &p, 1024 * 1024).unwrap());
    p.archive_attempted = true;
    p.begin_archive_intent(&s).unwrap();
    p.confirm_archive_intent().unwrap();
    p.publish(&s, Arc::new(|| Ok(())), &|| Ok(())).unwrap();
    p.begin_publication_observation(&s).unwrap();
    let target = root(&s).join("files/snapshot.fcproj");
    let bytes = fs::read(&target).unwrap();
    fs::rename(&target, d.0.join("held-original-snapshot")).unwrap();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut output = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target).unwrap();
    output.write_all(&bytes).unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert!(p.confirm_publication_observation(&s).is_err());
    assert!(p.current(&s).is_err());
    assert!(p.begin_publication_observation(&s).is_err());
    assert!(!p.archive.as_ref().unwrap().observation_confirmed);
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert!(s.persistence.as_ref().unwrap().startup_pending());
    stop(&mut s);
}
