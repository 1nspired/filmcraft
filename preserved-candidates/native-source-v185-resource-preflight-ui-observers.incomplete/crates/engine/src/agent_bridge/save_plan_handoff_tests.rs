use super::super::{
    project_font_replacement_tests::{scope, setup as owner, state as native_state},
    project_plane, snapshot_jobs,
    tests::token,
};
use super::*;
use crate::{
    autosave::AutosaveConfig,
    project_save::{
        self,
        effects::{self, DocumentRole, Event, FileEffects},
        operation,
    },
};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
fn fixture(n: usize, kind: &str, persistence: bool) -> (Session, Value, PathBuf) {
    let base = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir);
    let root = base.join(format!("encoded-handoff-{}-{}-{}", std::process::id(), n, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = owner();
    s.path = None;
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    if persistence {
        s.start_autosave(AutosaveConfig::new(root.join("runtime"))).unwrap();
        s.sync_persistence();
        s.persistence.as_ref().unwrap().flush();
        s.apply_persistence_events();
    }
    let read = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    let intent = if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) };
    let p = json!({"job":token(n+10000),"query":{"token":token(n),"scope":scope(),"beforeProjectSha256":read["projectState"]["sha256"],"intent":intent}});
    (s, p, root)
}
fn job<'a>(s: &'a Session, p: &Value) -> &'a Job {
    s.agent_bridge.save_plans.iter().find(|j| j.id == p["job"].as_str().unwrap()).unwrap()
}
fn delivered(s: &mut Session, p: &Value) -> Value {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let r = poll(s, &json!({"job":p["job"]})).unwrap();
        if r["status"] == "delivered" {
            return r;
        }
        assert_eq!(r["status"], "running");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn drained() {
    let end = Instant::now() + Duration::from_secs(3);
    while snapshot_jobs::worker_count() != 0 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn free(s: &Session) {
    assert!(!s.save_operation_held());
    drop(s.previews.pause_writers(&|| Ok(())).unwrap());
    if let Some(p) = &s.persistence {
        p.pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    }
}
fn pending(s: &Session, key: &operation::Key) {
    assert_eq!(operation::status(s, key, &|| Ok(())).unwrap()["phase"], "AwaitingReconciliation");
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    if let Some(p) = &s.persistence {
        assert!(p.pause_project_save(|| Ok(())).is_err());
    }
}
#[test]
fn original_worker_payload_moves_without_clone_or_reencode_into_same_native_Save_owner_for_all_four_intents() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    for (index, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, root) = fixture(140000 + index, kind, true);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (tx, rx) = mpsc::channel();
        start_with(&mut s, &p, None, move |c, check| {
            count.fetch_add(1, Ordering::SeqCst);
            let done = c.compute_owned(&|| check.check())?;
            tx.send(done.payload_identity().unwrap()).unwrap();
            Ok(done)
        })
        .unwrap();
        let reply = delivered(&mut s, &p);
        let identity = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let original_stop = job(&s, &p).check.stop.clone();
        let key = super::super::begin_planned_project_save(&mut s, p["job"].as_str().unwrap(), &token(160000 + index), &|| Ok(())).unwrap();
        assert_eq!(operation::prepared(&s, &key).unwrap().payload_identity(), identity);
        assert_eq!(operation::prepared(&s, &key).unwrap().description(), reply["result"]["save"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!original_stop.load(Ordering::Acquire));
        assert!(s.agent_bridge.save_plans.is_empty());
        assert!(s.agent_bridge.stopped_save_plans.contains(&p["job"].as_str().unwrap().to_string()));
        let roots = [project_save::preview_plan::VolumeSpec { root: root.clone(), device: fs::metadata(&root).unwrap().dev().to_string(), minimum_free: 0 }];
        let result = operation::dispatch_preserving_previews(&mut s, &key, &roots, &mut effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        assert_eq!(
            crate::saved_file::sha256(&fs::read(result["path"].as_str().unwrap()).unwrap()),
            reply["result"]["save"]["output"]["sha256"].as_str().unwrap()
        );
        pending(&s, &key);
        assert!(!operation::documents(&s, &key).unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(super::super::begin_planned_project_save(&mut s, p["job"].as_str().unwrap(), &token(170000 + index), &|| Ok(())).is_err());
        s.shutdown();
        drop(s);
        assert!(original_stop.load(Ordering::Acquire));
        drained();
    }
}
#[test]
fn stale_original_root_editor_saved_preferences_preview_and_media_context_refuses_transfer_without_rollback_or_files() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    for mutation in 0..7 {
        let (mut s, p, root) = fixture(140010 + mutation, "save_as", false);
        start(&mut s, &p).unwrap();
        delivered(&mut s, &p);
        match mutation {
            0 => {
                s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
            }
            1 => s.state.source_playhead = filmcraft_time::Tick(17),
            2 => s.saved_revision = s.saved_revision.saturating_add(1),
            3 => s.prefs.general.recent_projects.push("later prefs".into()),
            4 => {
                s.previews.generation.fetch_add(1, Ordering::SeqCst);
            }
            5 => s.media.set_use_proxies(!s.media.use_proxies()),
            _ => s.path = Some(root.join("Later human.fcproj").to_str().unwrap().into()),
        };
        let later = native_state(&s);
        assert!(begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160010 + mutation), &|| Ok(())).is_err());
        assert_eq!(native_state(&s), later);
        assert!(!root.join("Chosen.fcproj").exists());
        free(&s);
        s.shutdown();
        drained();
    }
}
#[test]
fn original_delivery_and_operation_deadlines_are_not_renewed_by_transfer() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    for expired_delivery in [true, false] {
        let (mut s, p, root) = fixture(if expired_delivery { 140030 } else { 140031 }, "save_as", false);
        start(&mut s, &p).unwrap();
        delivered(&mut s, &p);
        if expired_delivery {
            let mut state = job(&s, &p).state.lock().unwrap_or_else(|e| e.into_inner());
            let State::Delivered(_, at) = &mut *state else { panic!("original result not delivered") };
            *at = Instant::now() - Duration::from_millis(251);
        } else {
            s.agent_bridge.save_plans.iter_mut().find(|j| j.id == p["job"].as_str().unwrap()).unwrap().check =
                Arc::new(Check::new(Instant::now() - Duration::from_secs(46)));
        }
        assert!(begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160030), &|| Ok(())).is_err());
        assert!(!root.join("Chosen.fcproj").exists());
        free(&s);
        assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
        s.shutdown();
        drained();
    }
}
#[test]
fn Stop_after_original_preview_pause_cancels_consumed_delivery_and_releases_pre_dispatch_leases_without_reencoding() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140040, "save_as", false);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    let stop = job(&s, &p).check.stop.clone();
    let store = s.previews.clone();
    let observed = std::sync::atomic::AtomicBool::new(false);
    let result = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160040), &|| {
        if store.pause_writers(&|| Ok(())).is_err() {
            observed.store(true, Ordering::Release);
            return Err(error("generated Stop after actual preview pause"));
        }
        Ok(())
    });
    assert!(result.is_err());
    assert!(observed.load(Ordering::Acquire));
    assert!(!root.join("Chosen.fcproj").exists());
    free(&s);
    assert!(stop.load(Ordering::Acquire));
    assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
    s.shutdown();
    drained();
}
#[test]
fn transferred_original_Stop_lifetime_stays_live_and_pre_dispatch_cancellation_remains_available() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140050, "save_as", true);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    let stop = job(&s, &p).check.stop.clone();
    let key = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160050), &|| Ok(())).unwrap();
    assert!(!stop.load(Ordering::Acquire));
    stop.store(true, Ordering::Release);
    assert!(operation::dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(operation::prepared(&s, &key).is_ok());
    assert!(!root.join("Chosen.fcproj").exists());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    free(&s);
    assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
    s.shutdown();
    drained();
}
struct StopAfterProject(Arc<std::sync::atomic::AtomicBool>);
impl FileEffects for StopAfterProject {
    fn event(&mut self, event: Event) {
        if matches!(event, Event::Document { role: DocumentRole::Project, result: effects::CallResult::ReturnedOk { .. }, .. }) {
            self.0.store(true, Ordering::Release);
        }
    }
}
#[test]
fn Stop_after_original_document_publication_retains_same_job_receipts_pauses_and_actual_bytes_without_replay() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140060, "save_as", true);
    start(&mut s, &p).unwrap();
    let reply = delivered(&mut s, &p);
    let stop = job(&s, &p).check.stop.clone();
    let key = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160060), &|| Ok(())).unwrap();
    assert!(operation::dispatch_retained(&mut s, &key, &mut StopAfterProject(stop.clone()), &|| Ok(()), &|| Ok(())).is_err());
    assert!(stop.load(Ordering::Acquire));
    pending(&s, &key);
    assert_eq!(crate::saved_file::sha256(&fs::read(root.join("Chosen.fcproj")).unwrap()), reply["result"]["save"]["output"]["sha256"].as_str().unwrap());
    let docs = operation::documents(&s, &key).unwrap();
    assert!(docs.iter().any(|d| d.role == project_save::retained_effects::Role::Project && d.publication.is_some()));
    assert!(operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
    assert!(operation::dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    s.shutdown();
    drained();
}
#[test]
fn wrong_native_owner_invalid_ids_and_metadata_only_destinations_cannot_mint_a_Save_handoff() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut other, _, _) = fixture(140071, "save_as", false);
    let (mut s, p, root) = fixture(140070, "save_as", false);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    assert!(begin_operation(&mut other, p["job"].as_str().unwrap(), &token(160070), &|| Ok(())).is_err());
    for id in ["short", p["job"].as_str().unwrap()] {
        assert!(begin_operation(&mut s, p["job"].as_str().unwrap(), id, &|| Ok(())).is_err());
    }
    assert!(job(&s, &p).prepared.token() == p["query"]["token"].as_str().unwrap());
    assert!(!root.join("Chosen.fcproj").exists());
    free(&s);
    free(&other);
    let (mut blank, mut p, root) = fixture(140072, "save_copy", false);
    p["query"]["intent"] = json!({"kind":"save_copy"});
    start(&mut blank, &p).unwrap();
    let reply = delivered(&mut blank, &p);
    assert_eq!(reply["result"]["status"], "needs_destination");
    assert!(begin_operation(&mut blank, p["job"].as_str().unwrap(), &token(160072), &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    free(&blank);
    s.shutdown();
    other.shutdown();
    blank.shutdown();
    drained();
}
#[test]
fn pending_native_owner_cannot_consume_another_job_and_cancelled_handoff_cannot_reuse_original_payload() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140080, "save_as", false);
    let mut second = p.clone();
    second["job"] = json!(token(150081));
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    start(&mut s, &second).unwrap();
    delivered(&mut s, &second);
    let first = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160080), &|| Ok(())).unwrap();
    assert!(begin_operation(&mut s, second["job"].as_str().unwrap(), &token(160081), &|| Ok(())).is_err());
    assert_eq!(s.agent_bridge.save_plans.len(), 1);
    operation::cancel_before_dispatch(&mut s, &first, &|| Ok(())).unwrap();
    assert!(begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160082), &|| Ok(())).is_err());
    let key = begin_operation(&mut s, second["job"].as_str().unwrap(), &token(160083), &|| Ok(())).unwrap();
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    free(&s);
    assert!(!root.join("Chosen.fcproj").exists());
    s.shutdown();
    drained();
}
#[test]
fn encoded_binding_refuses_foreign_original_payload_and_different_intent_even_when_metadata_matches() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (s, _, root) = fixture(140090, "save_as", false);
    let (other, _, _) = fixture(140091, "save_as", false);
    let path = root.join("Chosen.fcproj").to_str().unwrap().to_string();
    let guard = project_save::guard(&s).unwrap();
    let foreign = project_save::input(&other, &path, true).compute(&|| Ok(())).unwrap();
    assert!(guard.bind_encoded(&s, foreign, &path, true).is_err());
    let wrong = project_save::input(&s, &path, false).compute(&|| Ok(())).unwrap();
    assert!(guard.bind_encoded(&s, wrong, &path, true).is_err());
    let changed_path = project_save::input(&s, "relative other.fcproj", true).compute(&|| Ok(())).unwrap();
    assert!(guard.bind_encoded(&s, changed_path, &path, true).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
}
#[test]
fn later_native_context_after_transfer_is_preserved_and_cannot_publish_the_earlier_payload() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140100, "save_as", false);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    let key = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160100), &|| Ok(())).unwrap();
    s.state.source_playhead = filmcraft_time::Tick(99);
    let later = native_state(&s);
    assert!(operation::dispatch_retained(&mut s, &key, &mut effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert_eq!(native_state(&s), later);
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(s.save_operation_held());
    s.shutdown();
    drained();
}

#[test]
fn original_delivered_payload_enters_same_native_Key_before_slow_pause_without_renewing_its_operation_deadline() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140080, "save_copy", true);
    start(&mut s, &p).unwrap();
    let reply = delivered(&mut s, &p);
    let identity = {
        let state = job(&s, &p).state.lock().unwrap_or_else(|e| e.into_inner());
        let State::Delivered(result, _) = &*state else { panic!("actual original delivery required") };
        result.payload_identity().unwrap()
    };
    let stop = job(&s, &p).check.stop.clone();
    let store = s.previews.clone();
    let waited = std::sync::atomic::AtomicBool::new(false);
    let key = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160080), &|| {
        if !waited.load(Ordering::Acquire) && store.pause_writers(&|| Ok(())).is_err() {
            waited.store(true, Ordering::Release);
            std::thread::sleep(DELIVERY + Duration::from_millis(50));
        }
        Ok(())
    })
    .unwrap();
    assert!(waited.load(Ordering::Acquire));
    assert_eq!(operation::prepared(&s, &key).unwrap().payload_identity(), identity);
    assert_eq!(operation::prepared(&s, &key).unwrap().description(), reply["result"]["save"]);
    assert!(!stop.load(Ordering::Acquire));
    assert_eq!(poll(&mut s, &json!({"job":p["job"]})).unwrap_err().to_string().contains("unknown/expired"), true);
    assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(super::super::begin_planned_project_save(&mut s, p["job"].as_str().unwrap(), &token(170080), &|| Ok(())).is_err());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    free(&s);
    assert!(stop.load(Ordering::Acquire));
    s.shutdown();
    drained();
}
#[test]
fn expired_delivery_during_native_binding_is_refused_before_worker_acquisition_or_file_output() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140081, "save_copy", true);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    let calls = AtomicUsize::new(0);
    let error = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160081), &|| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            std::thread::sleep(DELIVERY + Duration::from_millis(50));
        }
        Ok(())
    })
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("expired before native ownership transfer"), "{error}");
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
    free(&s);
    s.shutdown();
    drained();
}

#[test]
fn original_operation_deadline_expiring_during_owned_pause_is_not_reset_by_delivery_transfer() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drained();
    let (mut s, p, root) = fixture(140082, "save_copy", true);
    start(&mut s, &p).unwrap();
    delivered(&mut s, &p);
    s.agent_bridge.save_plans.iter_mut().find(|j| j.id == p["job"].as_str().unwrap()).unwrap().check =
        Arc::new(Check::new(Instant::now() - Duration::from_secs(44)));
    let store = s.previews.clone();
    let waited = std::sync::atomic::AtomicBool::new(false);
    let error = begin_operation(&mut s, p["job"].as_str().unwrap(), &token(160082), &|| {
        if !waited.load(Ordering::Acquire) && store.pause_writers(&|| Ok(())).is_err() {
            waited.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(1100));
        }
        Ok(())
    })
    .err()
    .unwrap()
    .to_string();
    assert!(waited.load(Ordering::Acquire));
    assert!(error.contains("exceeded its deadline"), "{error}");
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(start(&mut s, &p).unwrap()["status"], "cancelled");
    free(&s);
    s.shutdown();
    drained();
}
