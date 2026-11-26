//! The recovery reader must not clean old evidence during guarded startup.
use super::*;

fn directory(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = crate::temp_dir().join(format!("poema-preserved-journal-{tag}-{}-{sequence}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn guarded_scan_preserves_empty_and_partial_old_journals() {
    let data = directory("partial");
    let root = recovery_root(&data);
    let empty = root.join("100-1");
    let partial = root.join("101-1");
    fs::create_dir_all(&empty).unwrap();
    fs::create_dir(&partial).unwrap();
    fs::write(partial.join(META), b"incomplete metadata {").unwrap();
    fs::write(partial.join("interrupted-write.tmp"), b"unindexed recovery evidence").unwrap();
    let found = scan_candidates_with_cleanup(&data, None, CandidateCleanup::Preserve);
    assert!(found.is_empty(), "incomplete journals are preserved, not falsely offered as snapshots");
    assert!(empty.is_dir());
    assert_eq!(fs::read(partial.join(META)).unwrap(), b"incomplete metadata {");
    assert_eq!(fs::read(partial.join("interrupted-write.tmp")).unwrap(), b"unindexed recovery evidence");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
}

#[test]
fn guarded_and_ordinary_scans_share_candidate_order_and_metadata() {
    let data = directory("candidates");
    let root = recovery_root(&data);
    for (id, saved) in [("100-1", 10), ("101-1", 20)] {
        let session = root.join(id);
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join(SNAPSHOT), b"generated snapshot bytes").unwrap();
        let meta = JournalMeta { saved_unix: saved, project_name: id.to_owned(), ..Default::default() };
        fs::write(session.join(META), serde_json::to_vec(&meta).unwrap()).unwrap();
    }
    let guarded = scan_candidates_with_cleanup(&data, None, CandidateCleanup::Preserve);
    assert_eq!(guarded, scan_candidates(&data, None));
    assert_eq!(guarded.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["101-1", "100-1"]);
    for candidate in &guarded {
        assert_eq!(fs::read(candidate.dir.join(SNAPSHOT)).unwrap(), b"generated snapshot bytes");
    }
}

#[test]
fn guarded_scan_skips_live_and_own_sessions_without_changing_them() {
    let data = directory("locks");
    let root = recovery_root(&data);
    let live = root.join("100-1");
    let own = root.join("101-1");
    fs::create_dir_all(&live).unwrap();
    fs::create_dir(&own).unwrap();
    fs::write(live.join(SNAPSHOT), b"live snapshot").unwrap();
    fs::write(own.join(SNAPSHOT), b"own snapshot").unwrap();
    let lock = File::options().create_new(true).read(true).write(true).open(live.join(LOCK)).unwrap();
    lock.try_lock().unwrap();
    assert!(scan_candidates_with_cleanup(&data, Some(&own), CandidateCleanup::Preserve).is_empty());
    assert_eq!(fs::read(live.join(SNAPSHOT)).unwrap(), b"live snapshot");
    assert_eq!(fs::read(own.join(SNAPSHOT)).unwrap(), b"own snapshot");
    assert!(live.join(LOCK).is_file());
    drop(lock);
    assert_eq!(scan_candidates_with_cleanup(&data, Some(&own), CandidateCleanup::Preserve).len(), 1);
}

#[test]
fn ordinary_scan_retains_existing_empty_session_cleanup() {
    let data = directory("ordinary");
    let empty = recovery_root(&data).join("100-1");
    fs::create_dir_all(&empty).unwrap();
    assert!(scan_candidates_with_cleanup(&data, None, CandidateCleanup::Preserve).is_empty());
    assert!(empty.is_dir());
    assert!(scan_candidates(&data, None).is_empty());
    assert!(!empty.exists());
}
