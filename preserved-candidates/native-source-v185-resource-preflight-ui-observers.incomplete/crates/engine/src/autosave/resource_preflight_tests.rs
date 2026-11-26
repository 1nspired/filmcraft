//! Generated actual-worker regressions for the checked acquisition of SavePause.
#![cfg(unix)]
use super::*;
use crate::Session;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::Path, sync::{atomic::{AtomicUsize, Ordering}, mpsc}};

fn persistence() -> Persistence {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = crate::temp_dir().join(format!("poema-resource-preflight-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let mut cfg = AutosaveConfig::new(&root);
    cfg.journal_debounce = Duration::from_secs(600);
    let mut prefs = AutoSavePrefs::default();
    prefs.recovery_interval_seconds = 1;
    Persistence::start(&cfg, prefs).unwrap()
}
fn dirty(p: &Persistence, revision: u64) {
    p.send_dirty_native(Session::default().project, revision, None, Ok(None));
}
fn fence(p: &Persistence) {
    let (tx, rx) = mpsc::channel();
    p.tx.send(Msg::Flush(tx)).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
}
#[derive(Debug, PartialEq)]
struct Observed {
    dev: u64, ino: u64, mode: u32, uid: u32, nlink: u64, size: u64,
    mtime: (i64, i64), ctime: (i64, i64), contents: Option<Vec<u8>>,
}
fn namespace(root: &Path) -> BTreeMap<PathBuf, Observed> {
    let mut out = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((p, depth)) = pending.pop() {
        assert!(depth < 8 && out.len() < 128);
        let m = fs::symlink_metadata(&p).unwrap();
        assert!(m.is_dir() || m.is_file());
        out.insert(p.strip_prefix(root).unwrap().to_path_buf(), Observed {dev:m.dev(),ino:m.ino(),mode:m.mode(),uid:m.uid(),nlink:m.nlink(),size:m.len(),mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec()),contents:m.is_file().then(||fs::read(&p).unwrap())});
        if m.is_dir() { for e in fs::read_dir(&p).unwrap() { pending.push((e.unwrap().path(), depth+1)); } }
    }
    out
}

#[test]
fn checked_resource_pause_flushes_original_pending_journal_and_freezes_all_original_fifo_writes_and_automatic_timer() {
    let p = persistence();
    dirty(&p, 41);
    let held = p.pause_resource_preflight(||Ok(())).unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&fs::read(p.session_dir.join(META)).unwrap()).unwrap();
    assert_eq!(meta["revision"],41);
    let before = namespace(&p.data_dir);
    dirty(&p, 42);
    let mut prefs = AutoSavePrefs::default(); prefs.recovery_journal=false;
    p.set_prefs(prefs); p.send_clean();
    let (tx, flushed) = mpsc::channel(); p.tx.send(Msg::Flush(tx)).unwrap();
    let (tx, autosaved) = mpsc::channel(); p.tx.send(Msg::AutoSaveNow(tx)).unwrap();
    // Cross the actual one-second maximum journal cadence, not just debounce.
    std::thread::sleep(Duration::from_millis(1100));
    held.current(&p).unwrap();
    assert!(matches!(flushed.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert!(matches!(autosaved.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert_eq!(namespace(&p.data_dir),before);
    held.finish(||Ok(())).unwrap();
    flushed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(autosaved.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
    assert!(!p.session_dir.join(SNAPSHOT).exists());
}

#[test]
fn checked_resource_pause_reports_real_pending_snapshot_write_failure_and_preserves_unknown_partial_paths() {
    let p = persistence(); let snapshot=p.session_dir.join(SNAPSHOT);
    fs::create_dir(&snapshot).unwrap(); dirty(&p, 43);
    let error=p.pause_resource_preflight(||Ok(())).err().expect("checked original snapshot writer must refuse");
    assert!(error.contains("journal") || error.contains("directory") || error.contains("Directory"),"{error}");
    assert!(snapshot.is_dir());
    fence(&p);
    // Existing ordinary Save acquisition intentionally retains its old contract.
    p.pause_project_save(||Ok(())).unwrap().finish(||Ok(())).unwrap();
    assert!(snapshot.is_dir());
}

#[test]
fn checked_resource_pause_reports_the_original_native_companion_failure_after_any_partial_project_and_metadata_write() {
    let p = persistence();
    p.send_dirty_native(Session::default().project,44,None,Err("GENERATED_ORIGINAL_NATIVE_CAPTURE_REFUSAL".into()));
    let error=p.pause_resource_preflight(||Ok(())).err().expect("checked native companion producer must refuse");
    assert!(error.contains("GENERATED_ORIGINAL_NATIVE_CAPTURE_REFUSAL"),"{error}");
    assert!(p.session_dir.join(SNAPSHOT).is_file()); assert!(p.session_dir.join(META).is_file());
    assert!(!p.session_dir.join("native-state.json").exists());
    fence(&p);
    p.pause_project_save(||Ok(())).unwrap().finish(||Ok(())).unwrap();
}

#[test]
fn checked_resource_pause_first_Stop_at_each_initial_owner_check_cannot_leave_an_original_pause_stranded() {
    for fail_at in [1,2,3,4] {
        let p=persistence(); let mut calls=0;
        let result=p.pause_resource_preflight(||{ calls+=1; if calls==fail_at {Err("GENERATED_ORIGINAL_STOP".into())}else{Ok(())} });
        assert!(matches!(result,Err(ref e)if e=="GENERATED_ORIGINAL_STOP"));
        fence(&p);
        p.pause_resource_preflight(||Ok(())).unwrap().finish(||Ok(())).unwrap();
    }
}

#[test]
fn checked_resource_pause_shares_the_same_owner_slot_and_cannot_borrow_another_worker_or_resume_an_independent_workspace_pause() {
    let p=persistence(); let other=persistence();
    dirty(&p,45); p.pause_workspace(||Ok(())).unwrap();
    let held=p.pause_resource_preflight(||Ok(())).unwrap();
    assert!(p.pause_project_save(||Ok(())).is_err());
    assert!(p.pause_resource_preflight(||Ok(())).is_err());
    assert!(held.current(&other).is_err()); held.current(&p).unwrap();
    let probe=held.probe().unwrap(); probe.current().unwrap();
    held.finish(||Ok(())).unwrap(); assert!(probe.current().is_err());
    assert!(p.auto_save_now().unwrap_err().contains("workspace update"));
    p.resume_workspace().unwrap(); fence(&p);
    other.pause_resource_preflight(||Ok(())).unwrap().finish(||Ok(())).unwrap();
}

#[test]
fn checked_resource_pause_retains_the_same_resume_ack_after_Stop_and_never_queues_another_marker() {
    for fail_at in [1,2] {
        let p=persistence(); let mut held=p.pause_resource_preflight(||Ok(())).unwrap(); let mut calls=0;
        assert!(held.finish_owned(||{calls+=1;if calls==fail_at {Err("GENERATED_RESUME_OBSERVATION_STOP".into())}else{Ok(())}}).is_err());
        assert!(held.same_owner(&p)); assert!(p.pause_resource_preflight(||Ok(())).is_err());
        held.finish_owned(||Ok(())).unwrap(); held.finish_owned(||Ok(())).unwrap(); drop(held);
        p.pause_resource_preflight(||Ok(())).unwrap().finish(||Ok(())).unwrap();
    }
}

#[test]
fn checked_resource_pause_drop_and_worker_shutdown_release_only_their_actual_original_lifetime() {
    let p=persistence(); let held=p.pause_resource_preflight(||Ok(())).unwrap(); let probe=held.probe().unwrap();
    drop(held); fence(&p); assert!(probe.current().is_err());
    let held=p.pause_resource_preflight(||Ok(())).unwrap(); let next=held.probe().unwrap();
    drop(p); assert!(next.current().is_err()); let other=persistence(); assert!(held.current(&other).is_err()); drop(held);
    other.pause_resource_preflight(||Ok(())).unwrap().finish(||Ok(())).unwrap();
}

#[test]
fn checked_resource_pause_refuses_cold_startup_and_stopped_workers_without_consuming_original_startup_authority() {
    let p=persistence(); let cfg=AutosaveConfig::new(p.data_dir.join("generated-cold"));
    fs::create_dir(&cfg.data_dir).unwrap();
    let reservation=session_reservation::ColdJournalReservation::prepare(&cfg,&||Ok(())).unwrap();
    let mut cold=Persistence::start_cold_reserved(&cfg,AutoSavePrefs::default(),reservation,&||Ok(())).unwrap();
    assert!(cold.pause_resource_preflight(||Ok(())).err().unwrap().contains("not a cold/startup owner"));
    assert!(cold.startup_pending()); cold.stop_worker();
    let mut stopped=persistence(); stopped.stop_worker();
    assert!(stopped.pause_resource_preflight(||Ok(())).is_err());
}
