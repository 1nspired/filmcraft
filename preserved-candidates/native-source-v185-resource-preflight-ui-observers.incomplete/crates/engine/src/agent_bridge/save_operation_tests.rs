use super::super::{
    project_font_replacement_tests::{scope, setup as owner, state},
    project_plane, snapshot_jobs,
    tests::token,
};
use super::*;
use crate::{autosave::AutosaveConfig, project_save::effects};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn fixture(n: usize, kind: &str) -> (Session, Value, Value, PathBuf) {
    let base = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir);
    let root = base.join(format!("native-save-controller-{}-{n}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = owner();
    s.path = None;
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    s.start_autosave(AutosaveConfig::new(root.join("runtime"))).unwrap();
    s.sync_persistence();
    s.persistence.as_ref().unwrap().flush();
    s.apply_persistence_events();
    let read = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    let intent = if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) };
    let plan = json!({"job":token(n+10000),"query":{"token":token(n),"scope":scope(),"beforeProjectSha256":read["projectState"]["sha256"],"intent":intent}});
    let request = json!({"job":plan["job"],"operation":token(n+20000)});
    (s, plan, request, root)
}
fn deliver(s: &mut Session, p: &Value) {
    s.execute("agent.startProjectSavePlan", p.clone()).unwrap();
    let end = web_time::Instant::now() + Duration::from_secs(3);
    loop {
        let r = s.execute("agent.projectSavePlanStatus", json!({"job":p["job"]})).unwrap();
        if r["status"] == "delivered" {
            break;
        }
        assert_eq!(r["status"], "running");
        assert!(web_time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn released(s: &Session) {
    assert!(!s.save_operation_held());
    drop(s.previews.pause_writers(&|| Ok(())).unwrap());
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
}
#[test]
fn finite_native_controller_retains_original_key_and_pauses_for_all_four_intents_without_publication() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(180000 + i, kind);
        deliver(&mut s, &p);
        let before = state(&s);
        let bytes = fs::read(root.join("Original.fcproj")).unwrap();
        let first = s.execute(BEGIN, r.clone()).unwrap();
        assert_eq!(first["status"], "held");
        assert_eq!(first["pending"]["phase"], "Prepared");
        assert_eq!(s.execute(BEGIN, r.clone()).unwrap(), first);
        assert_eq!(s.execute(STATUS, r.clone()).unwrap(), first);
        assert!(s.agent_bridge.save_plans.is_empty());
        assert!(s.save_operation_held());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
        assert!(!root.join("Chosen.fcproj").exists());
        assert_eq!(s.execute(CANCEL, r.clone()).unwrap()["status"], "cancelled");
        released(&s);
        assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "cancelled");
        assert_eq!(s.execute(BEGIN, r.clone()).unwrap()["status"], "cancelled");
        assert_eq!(state(&s), before);
        s.shutdown();
    }
}
#[test]
fn native_controller_allows_only_exact_lifecycle_and_original_root_validation_while_pending() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(180010, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    s.execute(project_plane::VALIDATE, json!({"token":p["query"]["token"],"scope":scope()})).unwrap();
    for (command, args) in [
        ("file.save", json!({})),
        ("file.newBin", json!({"name":"No"})),
        ("edit.undo", json!({})),
        ("agent.readProject", json!({})),
        ("agent.cancelProjectSavePlan", json!({"job":p["job"]})),
    ] {
        assert!(s.execute(command, args).is_err(), "{command}");
    }
    let other = json!({"job":p["job"],"operation":token(190010)});
    for command in [BEGIN, STATUS, CANCEL] {
        assert!(s.execute(command, other.clone()).is_err());
    }
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    released(&s);
    s.shutdown();
}
#[test]
fn native_controller_Stop_before_begin_tombstones_exact_job_and_operation_without_pausing() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(180020, "save_as");
    let before = state(&s);
    assert_eq!(s.execute(CANCEL, r.clone()).unwrap()["status"], "cancelled");
    released(&s);
    assert_eq!(s.execute("agent.startProjectSavePlan", p).unwrap()["status"], "cancelled");
    assert_eq!(s.execute(BEGIN, r.clone()).unwrap()["status"], "cancelled");
    let mut wrong = r.clone();
    wrong["job"] = json!(token(195020));
    assert!(s.execute(BEGIN, wrong).is_err());
    assert_eq!(state(&s), before);
    s.shutdown();
}
#[test]
fn native_controller_foreign_session_and_saved_descriptions_cannot_recreate_actual_key() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(180030, "save_as");
    deliver(&mut s, &p);
    let description = s.execute(BEGIN, r.clone()).unwrap();
    let (mut other, _, _, _) = fixture(180031, "save_as");
    let before = state(&other);
    assert!(other.execute(BEGIN, r.clone()).is_err());
    assert!(other.execute(STATUS, r.clone()).is_err());
    assert!(other.execute(BEGIN, description).is_err());
    assert_eq!(state(&other), before);
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
    other.shutdown();
}
#[test]
fn native_controller_rejects_malformed_requests_and_stale_original_root_without_pauses_or_output() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, root) = fixture(180040, "save_as");
    deliver(&mut s, &p);
    for bad in [
        Value::Null,
        json!({}),
        json!({"job":"bad","operation":r["operation"]}),
        json!({"job":r["job"],"operation":r["job"]}),
        json!({"job":r["job"],"operation":r["operation"],"writeAuthority":true}),
    ] {
        for command in [BEGIN, STATUS, CANCEL] {
            assert!(s.execute(command, bad.clone()).is_err());
        }
    }
    s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let before = state(&s);
    assert!(s.execute(BEGIN, r).is_err());
    released(&s);
    assert_eq!(state(&s), before);
    assert!(!root.join("Chosen.fcproj").exists());
    s.shutdown();
}
#[test]
fn native_controller_cancel_after_dispatch_keeps_same_actual_pending_key_and_worker_leases() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(180050, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let owner = s.agent_bridge.save_operation.take().unwrap();
    let _ = operation::dispatch(&mut s, &owner.key, &mut effects::Original, &|| Ok(())).unwrap();
    s.agent_bridge.save_operation = Some(owner);
    let before = state(&s);
    assert!(s.execute(CANCEL, r.clone()).is_err());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["pending"]["phase"], "AwaitingReconciliation");
    assert_eq!(s.execute(BEGIN, r).unwrap()["pending"]["phase"], "AwaitingReconciliation");
    assert_eq!(state(&s), before);
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    s.shutdown();
}
#[test]
fn native_controller_display_projection_has_no_lifecycle_key_or_write_door() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(180060, "save_as");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let mut display = s.display_projection();
    assert!(display.agent_bridge.save_operation.is_none());
    for command in [BEGIN, STATUS, CANCEL] {
        assert!(display.execute(command, r.clone()).is_err());
    }
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn native_controller_cancelled_operation_memory_keeps_original_finite_tombstone_bound() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _, _, _) = fixture(180070, "save_as");
    for i in 0..super::super::TICKET_LIMIT + 1 {
        let r = json!({"job":token(210000+i),"operation":token(220000+i)});
        assert_eq!(s.execute(CANCEL, r).unwrap()["status"], "cancelled");
    }
    assert_eq!(s.agent_bridge.stopped_save_operations.len(), super::super::TICKET_LIMIT);
    assert!(s.agent_bridge.save_operation.is_none());
    released(&s);
    s.shutdown();
}

#[test]
fn same_pending_Save_key_prepares_complete_original_recovery_for_all_four_intents_without_publishing_or_resuming() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(280000 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let before = state(&s);
        let project = s.project.clone();
        let original = fs::read(root.join("Original.fcproj")).unwrap();
        let files = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>();
        let prepared = s.execute(RECOVERY, r.clone()).unwrap();
        let again = s.execute(RECOVERY, r.clone()).unwrap();
        assert_eq!(prepared, again);
        assert_eq!(prepared["kind"], "native_Save_recovery_read");
        assert_eq!(prepared["projectReadSha256"], super::super::project_plane::basis(&s).unwrap());
        assert_eq!(prepared["phase"], "Prepared");
        assert_eq!(prepared["job"], r["job"]);
        assert_eq!(prepared["operation"], r["operation"]);
        assert_eq!(prepared["scope"], scope());
        for field in ["staged", "current", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(prepared[field], false);
        }
        let owner = s.agent_bridge.save_operation.as_ref().unwrap();
        let frame = owner.recovery.as_ref().unwrap();
        let raw = frame.test_bytes();
        assert_eq!(prepared["bytes"], raw.len());
        assert_eq!(prepared["sha256"], super::super::history_checkpoint::digest(raw));
        let snapshot: Value = serde_json::from_slice(raw).unwrap();
        assert_eq!(snapshot["payload"]["kind"], "poema-native-project-recovery");
        assert_eq!(snapshot["payload"]["history"]["scope"], scope());
        assert_eq!(snapshot["payload"]["basis"]["scope"], scope());
        assert_eq!(prepared["projectSha256"], snapshot["payload"]["basis"]["project_sha256"]);
        assert_eq!(snapshot["payload"]["history"]["epoch"], prepared["nativeSession"]);
        assert_eq!(snapshot["payload"]["basis"]["native_session"], prepared["nativeSession"]);
        assert_eq!(state(&s), before);
        assert!(std::sync::Arc::ptr_eq(&s.project, &project));
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), original);
        assert!(!root.join("Chosen.fcproj").exists());
        assert_eq!(fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), files);
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
        s.execute(CANCEL, r.clone()).unwrap();
        assert!(s.execute(RECOVERY, r).is_err());
        released(&s);
        s.shutdown();
    }
}
#[test]
fn pending_Save_recovery_uses_complete_original_COW_undo_redo_graph_and_editor_instead_of_saved_file_fallback() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, mut p, r, root) = fixture(280010, "save_copy");
    s.execute("file.newBin", json!({"name":"Unsaved first"})).unwrap();
    s.execute("file.newBin", json!({"name":"Unsaved second"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.sync_persistence();
    s.persistence.as_ref().unwrap().flush();
    s.apply_persistence_events();
    let token = token(390010);
    let read = s.execute(project_plane::READ, json!({"token":token,"scope":scope(),"projectPath":s.path})).unwrap();
    p["query"]["token"] = json!(token);
    p["query"]["beforeProjectSha256"] = read["projectState"]["sha256"].clone();
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let before = state(&s);
    let result = s.execute(RECOVERY, r.clone()).unwrap();
    let raw = s.agent_bridge.save_operation.as_ref().unwrap().recovery.as_ref().unwrap().test_bytes();
    let envelope: Value = serde_json::from_slice(raw).unwrap();
    let history = &envelope["payload"]["history"];
    assert_eq!(history["undo"].as_array().unwrap().len(), s.history.undo.len());
    assert_eq!(history["redo"].as_array().unwrap().len(), s.history.redo.len());
    assert!(!s.history.undo.is_empty());
    assert!(!s.history.redo.is_empty());
    assert!(history["projects"].as_array().unwrap().len() >= 3);
    let current = history["projects"][0].as_str().unwrap();
    assert!(current.contains("Unsaved first"));
    assert!(!String::from_utf8(fs::read(root.join("Original.fcproj")).unwrap()).unwrap().contains("Unsaved first"));
    assert_eq!(result["projectSha256"], super::super::history_checkpoint::digest(current.as_bytes()));
    assert_eq!(state(&s), before);
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn pending_Save_recovery_rejects_foreign_malformed_and_display_requests_without_replacing_the_original_key() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(280020, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let before = state(&s);
    for bad in [Value::Null, json!({}), json!({"job":r["job"],"operation":token(299999)}), json!({"job":r["job"],"operation":r["operation"],"path":"/foreign"})]
    {
        assert!(s.execute(RECOVERY, bad).is_err());
    }
    let mut display = s.display_projection();
    assert!(display.execute(RECOVERY, r.clone()).is_err());
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.save_operation.as_ref().unwrap().recovery.is_none());
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn stopped_pending_Save_recovery_read_retains_same_native_owner_and_can_be_observed_by_a_fresh_read_without_replanning() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(280030, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let before = state(&s);
    let control = task::Control::new(RECOVERY, &r).unwrap();
    control.enter();
    control.stop();
    s.save_lifecycle_control = Some(control);
    assert!(s.execute(RECOVERY, r.clone()).is_err());
    s.save_lifecycle_control = None;
    assert!(s.agent_bridge.save_operation.as_ref().unwrap().recovery.is_none());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    assert!(s.agent_bridge.save_plans.is_empty());
    assert_eq!(state(&s), before);
    assert_eq!(s.execute(RECOVERY, r.clone()).unwrap()["phase"], "Prepared");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn retained_pending_Save_recovery_refuses_later_saved_file_changes_and_preserves_human_bytes_and_owner() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, root) = fixture(280040, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    s.execute(RECOVERY, r.clone()).unwrap();
    let before = state(&s);
    let file = root.join("Original.fcproj");
    fs::write(&file, b"later original human bytes").unwrap();
    assert!(s.execute(RECOVERY, r.clone()).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"later original human bytes");
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.save_operation.as_ref().unwrap().recovery.is_some());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn dispatched_Save_recovery_reads_actual_native_after_basis_without_releasing_or_repeating_the_original_service() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save_copy", "save_as"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(280050 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let prepared_recovery = s.execute(RECOVERY, r.clone()).unwrap();
        let owner = s.agent_bridge.save_operation.take().unwrap();
        operation::dispatch(&mut s, &owner.key, &mut effects::Original, &|| Ok(())).unwrap();
        s.agent_bridge.save_operation = Some(owner);
        let before = state(&s);
        let published = fs::read(root.join("Chosen.fcproj")).unwrap();
        let read = s.execute(RECOVERY, r.clone()).unwrap();
        assert_eq!(read["phase"], "AwaitingReconciliation");
        assert_eq!(read["retainedFrames"].as_array().unwrap().len(), 2);
        assert_eq!(read["retainedFrames"][0]["sha256"], prepared_recovery["sha256"]);
        assert_eq!(read["retainedFrames"][0]["phase"], "Prepared");
        assert_eq!(read["retainedFrames"][1]["sha256"], read["sha256"]);
        assert_eq!(read["nativeComplete"], false);
        assert_eq!(state(&s), before);
        assert_eq!(fs::read(root.join("Chosen.fcproj")).unwrap(), published);
        assert!(s.execute(CANCEL, r.clone()).is_err());
        assert_eq!(s.execute(STATUS, r).unwrap()["pending"]["phase"], "AwaitingReconciliation");
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.shutdown();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn same_actual_Save_key_stores_complete_private_recovery_bytes_for_all_four_intents_without_dispatch_or_resume() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(480000 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        s.execute(RECOVERY, r.clone()).unwrap();
        let before = state(&s);
        let original = fs::read(root.join("Original.fcproj")).unwrap();
        let mut owner = s.agent_bridge.save_operation.take().unwrap();
        let frame = owner.recovery.as_mut().unwrap();
        let check = Check::new(web_time::Instant::now());
        let intent = frame.prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap();
        assert_eq!(intent, frame.prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap());
        assert!(frame.prepare_private_record(&s, &owner.key, 0, &check).is_err());
        let target = PathBuf::from(intent["destination"].as_str().unwrap());
        assert!(!target.exists());
        let mut foreign = intent.clone();
        foreign["sha256"] = json!("f".repeat(64));
        assert!(frame.publish_private_record(&s, &owner.key, &foreign, &check).is_err());
        assert!(!target.exists());
        let receipt = frame.publish_private_record(&s, &owner.key, &intent, &check).unwrap();
        assert_eq!(receipt["phase"], "published");
        assert_eq!(receipt["native_complete"], false);
        assert_eq!(receipt["workers_may_resume"], false);
        assert_eq!(fs::read(&target).unwrap(), frame.test_bytes());
        let raw: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(raw["payload"]["history"]["scope"], scope());
        assert!(frame.publish_private_record(&s, &owner.key, &intent, &check).is_err());
        assert_eq!(frame.inspect_private_record(&check).unwrap()["matches_published"], true);
        s.agent_bridge.save_operation = Some(owner);
        assert_eq!(state(&s), before);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), original);
        assert!(!root.join("Chosen.fcproj").exists());
        assert_eq!(s.execute(STATUS, r.clone()).unwrap()["pending"]["phase"], "Prepared");
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.execute(CANCEL, r).unwrap();
        assert!(target.is_file());
        released(&s);
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn same_actual_Save_key_durably_retains_distinct_before_and_real_after_native_frames_without_replaying_Save_or_releasing_workers() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save_copy", "save_as"].into_iter().enumerate() {
        let (mut s, p, r, _) = fixture(480010 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        s.execute(RECOVERY, r.clone()).unwrap();
        let mut owner = s.agent_bridge.save_operation.take().unwrap();
        let check = Check::new(web_time::Instant::now());
        let frame = owner.recovery.as_mut().unwrap();
        let intent = frame.prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap();
        frame.publish_private_record(&s, &owner.key, &intent, &check).unwrap();
        let before_path = PathBuf::from(intent["destination"].as_str().unwrap());
        let before_bytes = fs::read(&before_path).unwrap();
        operation::dispatch(&mut s, &owner.key, &mut effects::Original, &|| Ok(())).unwrap();
        s.agent_bridge.save_operation = Some(owner);
        s.execute(RECOVERY, r.clone()).unwrap();
        let after_state = state(&s);
        let mut owner = s.agent_bridge.save_operation.take().unwrap();
        let frame = owner.recovery_after.as_mut().unwrap();
        let intent = frame.prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap();
        let after_path = PathBuf::from(intent["destination"].as_str().unwrap());
        assert_ne!(after_path, before_path);
        frame.publish_private_record(&s, &owner.key, &intent, &check).unwrap();
        assert_eq!(fs::read(after_path).unwrap(), frame.test_bytes());
        assert_eq!(fs::read(before_path).unwrap(), before_bytes);
        assert!(owner.recovery.as_mut().unwrap().publish_private_record(&s, &owner.key, &intent, &check).is_err());
        s.agent_bridge.save_operation = Some(owner);
        assert_eq!(state(&s), after_state);
        assert_eq!(s.execute(STATUS, r.clone()).unwrap()["pending"]["phase"], "AwaitingReconciliation");
        assert!(s.execute(CANCEL, r).is_err());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.shutdown();
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn private_native_recovery_publication_rejects_changed_original_files_without_a_candidate_and_retains_the_real_key() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, root) = fixture(480020, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    s.execute(RECOVERY, r.clone()).unwrap();
    let mut owner = s.agent_bridge.save_operation.take().unwrap();
    let frame = owner.recovery.as_mut().unwrap();
    let check = Check::new(web_time::Instant::now());
    let intent = frame.prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap();
    fs::write(root.join("Original.fcproj"), b"later human bytes").unwrap();
    assert!(frame.publish_private_record(&s, &owner.key, &intent, &check).is_err());
    assert!(!PathBuf::from(intent["destination"].as_str().unwrap()).exists());
    assert!(!PathBuf::from(intent["candidate"].as_str().unwrap()).exists());
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), b"later human bytes");
    s.agent_bridge.save_operation = Some(owner);
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn a_fresh_recovery_read_cannot_renew_the_original_forty_five_second_Save_write_lifetime_or_publish_a_prepared_frame() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(480030, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    s.execute(RECOVERY, r.clone()).unwrap();
    let mut owner = s.agent_bridge.save_operation.take().unwrap();
    let check = Check::new(web_time::Instant::now());
    let intent = owner.recovery.as_mut().unwrap().prepare_private_record(&s, &owner.key, 1024 * 1024, &check).unwrap();
    s.agent_bridge.save_operation = Some(owner);
    std::thread::sleep(Duration::from_secs(46));
    assert_eq!(s.execute(RECOVERY, r.clone()).unwrap()["phase"], "Prepared");
    let mut owner = s.agent_bridge.save_operation.take().unwrap();
    let fresh = Check::new(web_time::Instant::now());
    assert!(owner.recovery.as_mut().unwrap().publish_private_record(&s, &owner.key, &intent, &fresh).is_err());
    assert!(!PathBuf::from(intent["destination"].as_str().unwrap()).exists());
    assert!(!PathBuf::from(intent["candidate"].as_str().unwrap()).exists());
    s.agent_bridge.save_operation = Some(owner);
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    released(&s);
    s.shutdown();
}

#[test]
fn finite_physical_recovery_intent_uses_same_Key_and_complete_original_frame_for_all_intents_without_creating_files() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(680000 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let frame = s.execute(RECOVERY, r.clone()).unwrap();
        let state_before = state(&s);
        let entries = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>();
        let request = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576});
        let first = s.execute(RECORD_PREPARE, request.clone()).unwrap();
        assert_eq!(first, s.execute(RECORD_PREPARE, request).unwrap());
        let read = s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap();
        assert_eq!(read, first);
        assert_eq!(read["record"]["receipt"]["phase"], "prepared");
        assert_eq!(read["record"]["receipt"]["intent"]["sha256"], frame["sha256"]);
        assert_eq!(read["record"]["receipt"]["intent"]["bytes"], frame["bytes"]);
        assert_eq!(read["record"]["receipt"]["intent"]["reserve_bytes"], 1048576);
        assert_eq!(read["record"]["receipt"]["call_attempted"], false);
        assert_eq!(read["nativeSession"], frame["nativeSession"]);
        assert_eq!(read["frameSha256"], frame["sha256"]);
        for field in ["current", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(read[field], false);
        }
        assert_eq!(entries, fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());
        assert_eq!(state(&s), state_before);
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
}
#[test]
fn physical_recovery_intent_refuses_foreign_capture_unknown_fields_or_changed_reserve_without_replacing_the_original_owner() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, root) = fixture(680010, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let request = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576});
    let good = s.execute(RECORD_PREPARE, request.clone()).unwrap();
    for field in ["path", "intent", "writeAuthority"] {
        let mut bad = request.clone();
        bad[field] = json!(true);
        assert!(s.execute(RECORD_PREPARE, bad).is_err());
    }
    for field in ["job", "operation", "capture"] {
        let mut bad = request.clone();
        bad[field] = if field == "capture" { json!("00000000-0000-0000-0000-000000000000") } else { json!(token(780010)) };
        assert!(s.execute(RECORD_PREPARE, bad).is_err());
    }
    let mut bad = request.clone();
    bad["reserveBytes"] = json!(0);
    assert!(s.execute(RECORD_PREPARE, bad).is_err());
    assert_eq!(s.execute(RECORD_PREPARE, request).unwrap(), good);
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn original_physical_intent_read_retains_before_context_after_actual_Save_and_prepares_the_distinct_real_after_context() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(680020, "save_as");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let before_req = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576});
    let first = s.execute(RECORD_PREPARE, before_req).unwrap();
    let owner = s.agent_bridge.save_operation.take().unwrap();
    operation::dispatch(&mut s, &owner.key, &mut effects::Original, &|| Ok(())).unwrap();
    s.agent_bridge.save_operation = Some(owner);
    let after = s.execute(RECOVERY, r.clone()).unwrap();
    assert_eq!(s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap(), first);
    let last = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":after["capture"],"reserveBytes":1048576})).unwrap();
    assert_ne!(last["capture"], first["capture"]);
    assert_eq!(last["phase"], "AwaitingReconciliation");
    assert_eq!(last["record"]["receipt"]["call_attempted"], false);
    assert!(s.execute(CANCEL, r).is_err());
    s.shutdown();
}
#[test]
fn stopped_physical_intent_wait_keeps_original_Key_and_can_be_read_without_native_cancellation_or_file_publication() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(680030, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let request = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576});
    let control = task::Control::new(RECORD_PREPARE, &request).unwrap();
    control.enter();
    control.stop();
    s.save_lifecycle_control = Some(control);
    assert!(s.execute(RECORD_PREPARE, request.clone()).is_err());
    s.save_lifecycle_control = None;
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    let ready = s.execute(RECORD_PREPARE, request).unwrap();
    assert_eq!(ready["record"]["receipt"]["candidate_created"], false);
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}

#[test]
fn finite_publication_writes_complete_original_frames_for_all_four_intents_without_completing_native_Save() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(880000 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let before = state(&s);
        let project = fs::read(root.join("Original.fcproj")).unwrap();
        let frame = s.execute(RECOVERY, r.clone()).unwrap();
        let prepared = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
        let intent = prepared["record"]["receipt"]["intent"].clone();
        let request = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":intent});
        let published = s.execute(RECORD_PUBLISH, request.clone()).unwrap();
        assert_eq!(published["record"]["receipt"]["phase"], "published");
        assert_eq!(published["record"]["matches_published"], true);
        let owner = s.agent_bridge.save_operation.as_ref().unwrap();
        assert_eq!(fs::read(intent["destination"].as_str().unwrap()).unwrap(), owner.recovery.as_ref().unwrap().test_bytes());
        assert_eq!(fs::read(intent["candidate"].as_str().unwrap()).unwrap(), owner.recovery.as_ref().unwrap().test_bytes());
        for k in ["current", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(published[k], false);
        }
        assert_eq!(state(&s), before);
        assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), project);
        assert!(!root.join("Chosen.fcproj").exists());
        assert!(s.execute(RECORD_PUBLISH, request).is_err());
        assert_eq!(s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap(), published);
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
}
#[test]
fn finite_publication_requires_exact_original_prepared_intent_and_refuses_foreign_requests_before_any_file_io() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(880010, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let mut q = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":{}});
    assert!(s.execute(RECORD_PUBLISH, q.clone()).is_err());
    let ready = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
    q["intent"] = ready["record"]["receipt"]["intent"].clone();
    for field in ["destination", "sha256", "candidate", "operation"] {
        let mut bad = q.clone();
        bad["intent"][field] = json!("foreign");
        assert!(s.execute(RECORD_PUBLISH, bad).is_err());
    }
    for field in ["reserveBytes", "writeAuthority", "extra"] {
        let mut bad = q.clone();
        bad[field] = json!(true);
        assert!(s.execute(RECORD_PUBLISH, bad).is_err());
    }
    let i = &q["intent"];
    assert!(!PathBuf::from(i["destination"].as_str().unwrap()).exists());
    assert!(!PathBuf::from(i["candidate"].as_str().unwrap()).exists());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(RECORD_PUBLISH, q).unwrap();
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn finite_publication_preserves_foreign_output_and_inspects_failed_attempt_without_replay_or_cleanup() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(880020, "save_copy");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let ready = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
    let i = &ready["record"]["receipt"]["intent"];
    let dest = i["destination"].as_str().unwrap();
    fs::write(dest, b"later foreign human bytes").unwrap();
    let q = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":i});
    assert!(s.execute(RECORD_PUBLISH, q.clone()).is_err());
    let read = s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap();
    assert_eq!(read["record"]["matches_published"], false);
    assert_eq!(read["record"]["receipt"]["call_attempted"], true);
    assert!(s.execute(RECORD_PUBLISH, q).is_err());
    assert_eq!(fs::read(dest).unwrap(), b"later foreign human bytes");
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}
#[test]
fn finite_publication_preserves_actual_before_and_after_files_across_original_Save_As_dispatch() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(880030, "save_as");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let before = s.execute(RECOVERY, r.clone()).unwrap();
    let publish = |s: &mut Session, frame: &Value| {
        let ready = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
        s.execute(RECORD_PUBLISH, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":ready["record"]["receipt"]["intent"]}))
            .unwrap()
    };
    let first = publish(&mut s, &before);
    let owner = s.agent_bridge.save_operation.take().unwrap();
    operation::dispatch(&mut s, &owner.key, &mut effects::Original, &|| Ok(())).unwrap();
    s.agent_bridge.save_operation = Some(owner);
    let after = s.execute(RECOVERY, r.clone()).unwrap();
    let last = publish(&mut s, &after);
    assert_ne!(first["capture"], last["capture"]);
    assert_ne!(first["record"]["receipt"]["intent"]["destination"], last["record"]["receipt"]["intent"]["destination"]);
    for frame in [before, after] {
        let v = s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap();
        assert_eq!(v["record"]["matches_published"], true);
        assert_eq!(v["workersMayResume"], false);
    }
    assert!(s.execute(CANCEL, r).is_err());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    s.shutdown();
}
#[test]
fn finite_publication_fresh_reads_do_not_renew_original_write_budget_after_real_expiry() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(880040, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = s.execute(RECOVERY, r.clone()).unwrap();
    let ready = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
    std::thread::sleep(Duration::from_secs(46));
    let read = s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap();
    assert_eq!(read, ready);
    let i = &ready["record"]["receipt"]["intent"];
    assert!(s.execute(RECORD_PUBLISH, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":i})).is_err());
    assert!(!PathBuf::from(i["destination"].as_str().unwrap()).exists());
    assert!(!PathBuf::from(i["candidate"].as_str().unwrap()).exists());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    s.execute(CANCEL, r).unwrap();
    s.shutdown();
}

#[test]
fn finite_complete_effect_read_uses_original_key_for_all_four_before_and_actual_after_results_without_release() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let (mut s, p, r, root) = fixture(410000 + i, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let before = s.execute(EFFECTS, r.clone()).unwrap();
        assert_eq!(before["kind"], "native_Save_effects_read");
        assert_eq!(before["scope"], scope());
        assert_eq!(before["observation"]["phase"], "Prepared");
        assert_eq!(before["observation"]["body"], Value::Null);
        assert_eq!(before["job"], r["job"]);
        assert_eq!(s.execute(EFFECTS, r.clone()).unwrap(), before);
        let owner = s.agent_bridge.save_operation.take().unwrap();
        crate::project_save::operation::dispatch_retained(&mut s, &owner.key, &mut effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        s.agent_bridge.save_operation = Some(owner);
        let after = s.execute(EFFECTS, r.clone()).unwrap();
        assert_eq!(after["nativeSession"], before["nativeSession"]);
        assert_eq!(after["nativePid"], std::process::id());
        assert_eq!(after["observation"]["phase"], "AwaitingReconciliation");
        assert_eq!(after["observation"]["body"]["status"], "ok");
        assert_eq!(after["observation"]["nativeAfterAvailable"], true);
        assert_eq!(after["observation"]["nativeAfterCurrent"], true);
        assert!(!after["observation"]["documents"].as_array().unwrap().is_empty());
        for flag in ["current", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"] {
            assert_eq!(after[flag], false);
        }
        let file = if matches!(kind, "save" | "save_all") { root.join("Original.fcproj") } else { root.join("Chosen.fcproj") };
        let bytes = fs::read(&file).unwrap();
        assert_eq!(s.execute(EFFECTS, r.clone()).unwrap(), after);
        assert_eq!(fs::read(file).unwrap(), bytes);
        assert!(s.execute(CANCEL, r.clone()).is_err());
        assert!(s.save_operation_held());
        assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn finite_effect_read_refuses_unknown_foreign_extra_display_and_cancelled_owners_without_key_import() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, root) = fixture(410010, "save_copy");
    assert!(s.execute(EFFECTS, r.clone()).is_err());
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let bytes = fs::read(root.join("Original.fcproj")).unwrap();
    for bad in [
        json!({"job":r["job"],"operation":token(410011)}),
        json!({"job":token(410012),"operation":r["operation"]}),
        json!({"job":r["job"],"operation":r["operation"],"nativeComplete":true}),
    ] {
        assert!(s.execute(EFFECTS, bad).is_err());
    }
    assert!(s.display_projection().execute(EFFECTS, r.clone()).is_err());
    assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
    assert_eq!(s.execute(EFFECTS, r.clone()).unwrap()["observation"]["phase"], "Prepared");
    s.execute(CANCEL, r.clone()).unwrap();
    assert!(s.execute(EFFECTS, r).is_err());
    released(&s);
    s.shutdown();
}
#[test]
fn finite_effect_read_Stop_retains_exact_native_key_and_fresh_observation_does_not_restart_or_cancel() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, p, r, _) = fixture(410020, "save");
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let before = s.execute(EFFECTS, r.clone()).unwrap();
    let control = task::Control::new(EFFECTS, &r).unwrap();
    control.enter();
    control.stop();
    s.save_lifecycle_control = Some(control);
    assert!(s.execute(EFFECTS, r.clone()).is_err());
    s.save_lifecycle_control = None;
    assert!(s.agent_bridge.save_plans.is_empty());
    assert_eq!(s.execute(EFFECTS, r.clone()).unwrap(), before);
    assert!(s.save_operation_held());
    s.execute(CANCEL, r).unwrap();
    released(&s);
    s.shutdown();
}
#[test]
fn complete_effect_read_capability_requires_actual_original_worker_and_independent_transport() {
    let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _, _, _) = fixture(410030, "save");
    let a = s.execute(project_plane::READ, json!({"token":token(410031),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(a["capabilities"]["projectSaveEffectsRead"], false);
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let a = s.execute(project_plane::READ, json!({"token":token(410032),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(a["capabilities"]["projectSaveEffectsRead"], false);
    s.agent_operation_control().enable_transport();
    let a = s.execute(project_plane::READ, json!({"token":token(410033),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(a["capabilities"]["projectSaveEffectsRead"], true);
    s.shutdown();
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod admission_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    fn request(r: &Value, frame: &Value, root: &std::path::Path) -> Value {
        json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"volumes":[{"root":root,"device":fs::metadata(root).unwrap().dev().to_string(),"minimumFree":1048576}]})
    }
    fn read_request(q: &Value) -> Value {
        json!({"job":q["job"],"operation":q["operation"],"capture":q["capture"]})
    }
    fn publish(s: &mut Session, r: &Value) -> Value {
        let frame = s.execute(RECOVERY, r.clone()).unwrap();
        let ready = s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
        let published = s
            .execute(
                RECORD_PUBLISH,
                json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"intent":ready["record"]["receipt"]["intent"]}),
            )
            .unwrap();
        assert_eq!(published["record"]["matches_published"], true);
        frame
    }
    fn staged(n: usize, kind: &str) -> (Session, Value, Value, PathBuf, Value) {
        let (mut s, p, r, root) = fixture(n, kind);
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let f = publish(&mut s, &r);
        let q = request(&r, &f, &root);
        (s, p, r, root, q)
    }
    fn held(s: &mut Session, r: &Value) {
        assert_eq!(s.execute(STATUS, r.clone()).unwrap()["pending"]["phase"], "Prepared");
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    }
    #[test]
    fn finite_admission_retains_same_original_frames_and_paused_key_for_all_four_intents() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
            let (mut s, _, r, root, q) = staged(980000 + i, kind);
            let bytes = fs::read(root.join("Original.fcproj")).unwrap();
            let before = state(&s);
            let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
            assert_eq!(first["kind"], "native_Save_file_admission_read");
            assert_eq!(first["capture"], q["capture"]);
            assert_eq!(first["admission"]["kind"], "native_original_complete_Save_file_admission");
            assert_eq!(first["volumes"], q["volumes"]);
            assert_eq!(first["phase"], "Prepared");
            for flag in
                ["current", "fileCustody", "overwriteAuthority", "spaceReserved", "writeAuthority", "nativeComplete", "startupAuthority", "workersMayResume"]
            {
                assert_eq!(first[flag], false);
            }
            assert_eq!(s.execute(ADMISSION_PREPARE, q.clone()).unwrap(), first);
            assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
            assert_eq!(state(&s), before);
            assert_eq!(fs::read(root.join("Original.fcproj")).unwrap(), bytes);
            assert!(!root.join("Chosen.fcproj").exists());
            held(&mut s, &r);
            s.execute(CANCEL, r).unwrap();
            released(&s);
            s.shutdown();
        }
    }
    #[test]
    fn admission_requires_actual_complete_published_before_frame_before_installing_a_capture_attempt() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, p, r, root) = fixture(980010, "save_as");
        deliver(&mut s, &p);
        s.execute(BEGIN, r.clone()).unwrap();
        let unknown = json!({"capture":"8d09aa43-252a-4a10-b457-c346422fd9db"});
        let q = request(&r, &unknown, &root);
        assert!(s.execute(ADMISSION_PREPARE, q).is_err());
        let frame = s.execute(RECOVERY, r.clone()).unwrap();
        let q = request(&r, &frame, &root);
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert!(s.agent_bridge.save_operation.as_ref().unwrap().admission.is_none());
        s.execute(RECORD_PREPARE, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"reserveBytes":1048576})).unwrap();
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert!(s.agent_bridge.save_operation.as_ref().unwrap().admission.is_none());
        publish(&mut s, &r);
        s.execute(ADMISSION_PREPARE, q).unwrap();
        assert!(!root.join("Chosen.fcproj").exists());
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn later_human_destination_is_not_recaptured_by_repeated_preparation_read_or_original_dispatch() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, root, q) = staged(980020, "save_copy");
        let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
        let target = root.join("Chosen.fcproj");
        fs::write(&target, b"later human target").unwrap();
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        let mut owner = s.agent_bridge.save_operation.take().unwrap();
        let original = owner.admission.as_mut().unwrap().original.take().unwrap();
        assert!(operation::dispatch_prepared_files(&mut s, &owner.key, original, &mut effects::Original, &|| Ok(()), &|| Ok(())).is_err());
        s.agent_bridge.save_operation = Some(owner);
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        assert_eq!(fs::read(target).unwrap(), b"later human target");
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn failed_initial_native_capture_is_tombstoned_and_cannot_restart_with_a_replacement_volume_policy() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, root, q) = staged(980030, "save_copy");
        let mut wrong = q.clone();
        wrong["volumes"][0]["device"] = json!("0");
        assert!(s.execute(ADMISSION_PREPARE, wrong.clone()).is_err());
        assert!(s.agent_bridge.save_operation.as_ref().unwrap().admission.is_some());
        assert!(s.execute(ADMISSION_PREPARE, wrong).is_err());
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert!(s.execute(ADMISSION_READ, read_request(&q)).is_err());
        assert!(!root.join("Chosen.fcproj").exists());
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn copied_admission_unknown_fields_and_foreign_root_or_capture_cannot_replace_the_live_original_slot() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, root, q) = staged(980040, "save_copy");
        for mutation in [
            json!({"volumes":[]}),
            json!({"volumes":[q["volumes"][0].clone(),q["volumes"][0].clone()]}),
            json!({"volumes":[{"root":"../bad","device":"0","minimumFree":0}]}),
            json!({"volumes":[{"root":root,"device":"00","minimumFree":0}]}),
            json!({"writeAuthority":true}),
        ] {
            let mut bad = q.clone();
            for (k, v) in mutation.as_object().unwrap() {
                bad[k] = v.clone();
            }
            assert!(s.execute(ADMISSION_PREPARE, bad).is_err());
            assert!(s.agent_bridge.save_operation.as_ref().unwrap().admission.is_none());
        }
        let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
        assert!(s.execute(ADMISSION_PREPARE, first.clone()).is_err());
        let mut changed = q.clone();
        changed["volumes"][0]["minimumFree"] = json!(0);
        assert!(s.execute(ADMISSION_PREPARE, changed).is_err());
        let mut foreign = read_request(&q);
        foreign["capture"] = json!("8d09aa43-252a-4a10-b457-c346422fd9db");
        assert!(s.execute(ADMISSION_READ, foreign).is_err());
        let (mut other, _, _, _) = fixture(980041, "save_copy");
        assert!(other.execute(ADMISSION_READ, read_request(&q)).is_err());
        other.shutdown();
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn preparation_rechecks_actual_before_record_but_read_never_repairs_changed_physical_recovery_files() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, _, q) = staged(980050, "save_copy");
        let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
        let record = s.execute(RECORD_READ, read_request(&q)).unwrap();
        let path = PathBuf::from(record["record"]["receipt"]["intent"]["destination"].as_str().unwrap());
        fs::write(&path, b"later human recovery-file change").unwrap();
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        assert_eq!(fs::read(path).unwrap(), b"later human recovery-file change");
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn stopping_a_retained_admission_read_keeps_original_slot_for_a_fresh_descriptive_read() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, _, q) = staged(980060, "save_copy");
        let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
        let stopped = task::Control::new(ADMISSION_READ, &read_request(&q)).unwrap();
        stopped.enter();
        stopped.stop();
        s.save_lifecycle_control = Some(stopped);
        assert!(s.execute(ADMISSION_READ, read_request(&q)).is_err());
        s.save_lifecycle_control = None;
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        s.shutdown();
    }
    #[test]
    fn a_fresh_admission_read_after_original_expiry_cannot_renew_native_write_or_recapture_targets() {
        let _serial = snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, _, r, root, q) = staged(980070, "save_copy");
        let first = s.execute(ADMISSION_PREPARE, q.clone()).unwrap();
        std::thread::sleep(Duration::from_secs(46));
        assert_eq!(s.execute(ADMISSION_READ, read_request(&q)).unwrap(), first);
        assert!(s.execute(ADMISSION_PREPARE, q.clone()).is_err());
        assert!(!root.join("Chosen.fcproj").exists());
        held(&mut s, &r);
        s.execute(CANCEL, r).unwrap();
        released(&s);
        s.shutdown();
    }

    mod dispatch_tests {
        include!("save_operation/managed_dispatch_tests.rs");
    }
}
