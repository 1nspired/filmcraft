use super::*;
use crate::agent_bridge::save_operation::managed_dispatch::{self, COMMAND};
use std::{
    cell::Cell,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::{Arc, Condvar, Mutex, mpsc},
};
const ACTION: &str = "147d30e8-af30-4a35-9acc-ac3721058f1a";
const SEND: &str = "fd89fd07-7c58-4793-af65-872086611f52";
const EVIDENCE: &str = "e368098d-ecc3-46cf-a18f-273ab5a14d6f";
const INTENT: &str = "b22f088b-e31e-4e74-9183-f2164178f104";
fn hash_bytes(v: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(v))
}
fn write(file: &std::path::Path, value: &Value) -> Vec<u8> {
    let raw = serde_json::to_vec_pretty(value).unwrap();
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file).unwrap();
    f.write_all(&raw).unwrap();
    f.sync_all().unwrap();
    raw
}
fn stamp(path: &std::path::Path) -> Value {
    let s = fs::metadata(path).unwrap();
    json!({"dev":s.dev().to_string(),"ino":s.ino().to_string(),"mode":s.mode().to_string(),"uid":s.uid().to_string(),"size":s.len().to_string(),"mtimeNs":(i128::from(s.mtime())*1_000_000_000+i128::from(s.mtime_nsec())).to_string(),"ctimeNs":(i128::from(s.ctime())*1_000_000_000+i128::from(s.ctime_nsec())).to_string()})
}
fn pointer(catalog: &Value, raw: &[u8]) -> Value {
    json!({"kind":"original_Save_activity","version":1,"catalog":"native-save-evidence.json","sha256":hash_bytes(raw),"operations":catalog["operations"].as_array().unwrap().iter().map(|r|json!({"id":r["id"],"action":r["action"],"request":r["request"],"nativeSession":r["basis"]["nativeSession"],"phase":r["phase"]})).collect::<Vec<_>>(),"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false})
}
struct Fixture {
    session: Session,
    request: Value,
    params: Value,
    root: PathBuf,
    manifest: PathBuf,
    directory: PathBuf,
    plan: Value,
}
fn fixture(n: usize, kind: &str, worker: Option<&crate::session_worker::SessionWorker>, install: bool) -> Fixture {
    let (mut s, p, r, root) = super::super::fixture(n, kind);
    let scope = s.execute(project_plane::READ, json!({"token":token(n+50000),"scope":scope(),"projectPath":s.path})).unwrap()["scope"].clone();
    let manifest = root.join("workspace.json");
    let directory = root.join(format!("workspace-state-{}", scope["workspaceId"].as_str().unwrap()));
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let m = json!({"kind":"poema-saved-native-workspace","version":2,"id":scope["workspaceId"],"projectId":scope["projectId"],"projectPath":s.path,"readPlane":"workspace","sequenceId":s.state.active_sequence.map(|v|v.0),"contextSnapshot":root.join("context.json"),"captionSidecar":root.join("captions.json"),"nativeData":root.join("runtime"),"binary":std::env::current_exe().unwrap(),"binarySha256":"a".repeat(64),"sessionPath":root.join("session.json"),"nativePort":63991,"httpPort":63992,"nativePid":std::process::id(),"providerEnabled":false,"settingsPath":null});
    write(&manifest, &m);
    write(&root.join("workspace.json.lock"), &json!({"pid":std::process::id(),"identity":"98899f51-344e-426a-bc09-66e95b96689f"}));
    write(&directory.join("activity.json"), &json!({"status":"active","action":"agent","operationId":ACTION,"job":{"id":SEND,"status":"running"}}));
    if install {
        s.install_managed_save_root(&manifest).unwrap();
    }
    if let Some(w) = worker {
        w.install(&mut s).unwrap();
        s.agent_operation_control().enable_transport();
    }
    deliver(&mut s, &p);
    s.execute(BEGIN, r.clone()).unwrap();
    let frame = publish(&mut s, &r);
    let q = request(&r, &frame, &root);
    let admission = s.execute(ADMISSION_PREPARE, q).unwrap();
    let plan = s.agent_bridge.save_operation.as_ref().unwrap().plan_description.clone();
    let recovery = s.execute(RECOVERY, r.clone()).unwrap();
    let record = s.execute(RECORD_READ, json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"]})).unwrap();
    let files = json!({"kind":"native_project_save_files","version":1,"planSha256":hash_bytes(&serde_json::to_vec(&plan).unwrap()),"token":plan["token"],"scope":scope,"revision":frame["revision"],"storageRoots":[{"directory":root}],"parents":[],"absences":[],"entries":[{"path":plan["save"]["output"]["path"]}],"knownProjectOutputBytes":plan["save"]["output"]["bytes"],"writeAuthority":false,"fileCustody":false,"overwriteAuthority":false,"startupAuthority":false,"workerQuiescence":false,"allEffectsVerified":false,"publicationSpaceQualified":false});
    let aggregate = json!({"kind":"native_Save_recovery_records","version":1,"reserveBytes":1048576,"records":[record.clone()],"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
    let mut refs = Vec::new();
    for (role, value) in
        [("plan", plan.clone()), ("files", files), ("admission", admission), ("recovery_preparation", recovery), ("recovery_record", aggregate)]
    {
        let bytes = serde_json::to_vec(&value).unwrap();
        let sha = hash_bytes(&bytes);
        let name = format!("native-save-observation-{EVIDENCE}-{role}-{sha}.json");
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(directory.join(&name)).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        refs.push(json!({"role":role,"name":name,"bytes":bytes.len(),"sha256":sha}));
    }
    let digest = |role: &str| refs.iter().find(|r| r["role"] == role).unwrap()["sha256"].clone();
    let intent = json!({"kind":"original_Save_dispatch_intent","version":1,"id":INTENT,"action":ACTION,"sendJob":SEND,"request":r,"scope":scope,"nativeSession":frame["nativeSession"],"nativePid":std::process::id(),"planSha256":digest("plan"),"filesSha256":digest("files"),"admissionSha256":digest("admission"),"before":{"capture":frame["capture"],"sha256":frame["sha256"],"bytes":frame["bytes"],"recordIntentSha256":hash_bytes(&serde_json::to_vec(&record["record"]["receipt"]["intent"]).unwrap())},"manager":{"pid":std::process::id(),"manifestSha256":hash_bytes(&serde_json::to_vec(&m).unwrap()),"generation":stamp(&manifest)},"overwritePolicy":"original_observed_generation_only","current":false,"fileCustody":false,"overwriteAuthority":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false,"requestReplay":false});
    let catalog = json!({"kind":"poema-native-Save-evidence","version":7,"identity":{"workspaceId":scope["workspaceId"],"projectId":scope["projectId"],"projectPath":s.path,"readPlane":"workspace","sequenceId":s.state.active_sequence.map(|v|v.0)},"operations":[{"id":EVIDENCE,"action":ACTION,"request":r,"intent":plan["intent"],"basis":{"kind":"archived_original_Save_basis","version":1,"token":plan["token"],"scope":scope,"nativeSession":frame["nativeSession"],"revision":frame["revision"],"sequence":frame["sequence"],"projectSha256":frame["projectReadSha256"],"projectPath":frame["projectPath"],"nativeDirectory":null},"phase":"before_dispatch","nativeObservation":null,"observations":refs,"dispatchIntent":intent}],"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false});
    let raw = write(&directory.join("native-save-evidence.json"), &catalog);
    write(
        &directory.join("activity.json"),
        &json!({"status":"active","action":"agent","operationId":ACTION,"job":{"id":SEND,"status":"running"},"nativeSave":pointer(&catalog,&raw)}),
    );
    let params = json!({"job":r["job"],"operation":r["operation"],"capture":frame["capture"],"manifestPath":manifest,"catalogSha256":hash_bytes(&raw),"intentId":INTENT});
    Fixture { session: s, request: r, params, root, manifest, directory, plan }
}
fn serial() -> std::sync::MutexGuard<'static, ()> {
    snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}
fn unchanged(f: &Fixture) {
    assert!(!f.root.join("Chosen.fcproj").exists());
    assert_eq!(operation::status(&f.session, &f.session.agent_bridge.save_operation.as_ref().unwrap().key, &|| Ok(())).unwrap()["phase"], "Prepared");
}
fn rehash(f: &mut Fixture, c: &Value) {
    let raw = write(&f.directory.join("native-save-evidence.json"), c);
    f.params["catalogSha256"] = json!(hash_bytes(&raw));
    write(
        &f.directory.join("activity.json"),
        &json!({"status":"active","action":"agent","operationId":ACTION,"job":{"id":SEND,"status":"running"},"nativeSave":pointer(c,&raw)}),
    );
}
#[test]
fn managed_native_dispatch_consumes_same_admission_once_for_all_four_intents_and_retains_actual_effects_both_frames_and_workers() {
    let _serial = serial();
    for (i, kind) in ["save", "save_as", "save_copy", "save_all"].into_iter().enumerate() {
        let mut f = fixture(990000 + i, kind, None, true);
        let original = operation::test_key_seal(&f.session.agent_bridge.save_operation.as_ref().unwrap().key);
        let result = f.session.execute(COMMAND, f.params.clone()).unwrap();
        assert_eq!(result["pending"]["phase"], "AwaitingReconciliation");
        assert_eq!(result["nativeComplete"], false);
        let owner = f.session.agent_bridge.save_operation.as_ref().unwrap();
        assert!(std::sync::Arc::ptr_eq(&original, &operation::test_key_seal(&owner.key)));
        assert!(owner.admission.as_ref().unwrap().original.is_none());
        assert!(owner.dispatch_attempted);
        let output = f.plan["save"]["output"]["path"].as_str().unwrap();
        assert_eq!(hash_bytes(&fs::read(output).unwrap()), f.plan["save"]["output"]["sha256"].as_str().unwrap());
        assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
        assert!(f.session.execute(CANCEL, f.request.clone()).is_err());
        let effects = f.session.execute(EFFECTS, f.request.clone()).unwrap();
        assert_eq!(effects["observation"]["phase"], "AwaitingReconciliation");
        assert_eq!(effects["observation"]["bodyReturned"], true);
        assert_eq!(effects["observation"]["body"]["status"], "ok");
        assert_eq!(effects["observation"]["nativeAfterAvailable"], true);
        assert_eq!(effects["workersMayResume"], false);
        let frames = f.session.execute(RECOVERY, f.request.clone()).unwrap();
        assert_eq!(frames["phase"], "AwaitingReconciliation");
        assert_eq!(frames["retainedFrames"].as_array().unwrap().len(), 2);
        let ready = f
            .session
            .execute(RECORD_PREPARE, json!({"job":f.request["job"],"operation":f.request["operation"],"capture":frames["capture"],"reserveBytes":1048576}))
            .unwrap();
        let after = f
            .session
            .execute(
                RECORD_PUBLISH,
                json!({"job":f.request["job"],"operation":f.request["operation"],"capture":frames["capture"],"intent":ready["record"]["receipt"]["intent"]}),
            )
            .unwrap();
        assert_eq!(after["record"]["matches_published"], true);
        assert!(f.session.previews.pause_writers(&|| Ok(())).is_err());
        assert!(f.session.save_operation_held());
        f.session.shutdown();
    }
}
#[test]
fn copied_manager_controls_cannot_install_a_missing_native_startup_origin_or_retarget_the_original_owner() {
    let _serial = serial();
    let mut f = fixture(990010, "save_copy", None, false);
    assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
    unchanged(&f);
    assert!(f.session.install_managed_save_root(&f.manifest).is_err());
    assert!(f.session.execute("agent.installManagedSaveRoot", json!({"manifestPath":f.manifest})).is_err());
    assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
    f.session.execute(CANCEL, f.request).unwrap();
    f.session.shutdown();
}
#[test]
fn native_dispatch_refuses_forged_or_incomplete_resources_and_authority_before_any_project_effect() {
    let _serial = serial();
    for (i, fault) in ["missing", "rehash-plan", "rehash-admission", "intent-authority", "basis", "resource-path", "foreign-Send"].into_iter().enumerate() {
        let mut f = fixture(990020 + i, "save_copy", None, true);
        let mut c: Value = serde_json::from_slice(&fs::read(f.directory.join("native-save-evidence.json")).unwrap()).unwrap();
        match fault {
            "missing" => {
                let r = c["operations"][0]["observations"][0]["name"].as_str().unwrap();
                fs::rename(f.directory.join(r), f.root.join("held-resource")).unwrap();
            }
            "rehash-plan" | "rehash-admission" => {
                let role = if fault == "rehash-plan" { "plan" } else { "admission" };
                let r = c["operations"][0]["observations"].as_array_mut().unwrap().iter_mut().find(|r| r["role"] == role).unwrap();
                let mut v: Value = serde_json::from_slice(&fs::read(f.directory.join(r["name"].as_str().unwrap())).unwrap()).unwrap();
                if role == "plan" {
                    v["save"]["output"]["sha256"] = json!("f".repeat(64));
                } else {
                    v["admission"]["operation"] = json!("foreign");
                }
                let raw = serde_json::to_vec(&v).unwrap();
                let h = hash_bytes(&raw);
                let name = format!("native-save-observation-{EVIDENCE}-{role}-{h}.json");
                write_bytes(&f.directory.join(&name), &raw);
                r["name"] = json!(name);
                r["sha256"] = json!(h);
                r["bytes"] = json!(raw.len());
                let field = if role == "plan" { "planSha256" } else { "admissionSha256" };
                c["operations"][0]["dispatchIntent"][field] = json!(h);
                if role == "plan" {
                    let fref = c["operations"][0]["observations"].as_array_mut().unwrap().iter_mut().find(|r| r["role"] == "files").unwrap();
                    let mut file: Value = serde_json::from_slice(&fs::read(f.directory.join(fref["name"].as_str().unwrap())).unwrap()).unwrap();
                    file["planSha256"] = json!(h);
                    let raw = serde_json::to_vec(&file).unwrap();
                    let hash = hash_bytes(&raw);
                    let name = format!("native-save-observation-{EVIDENCE}-files-{hash}.json");
                    write_bytes(&f.directory.join(&name), &raw);
                    fref["name"] = json!(name);
                    fref["sha256"] = json!(hash);
                    fref["bytes"] = json!(raw.len());
                    c["operations"][0]["dispatchIntent"]["filesSha256"] = json!(hash);
                }
                rehash(&mut f, &c);
            }
            "intent-authority" => {
                c["operations"][0]["dispatchIntent"]["writeAuthority"] = json!(true);
                rehash(&mut f, &c);
            }
            "basis" => {
                c["operations"][0]["basis"]["nativeSession"] = json!("0".repeat(32));
                rehash(&mut f, &c);
            }
            "resource-path" => {
                c["operations"][0]["observations"][0]["name"] = json!("../credential.json");
                rehash(&mut f, &c);
            }
            _ => {
                c["operations"][0]["action"] = json!("5abb06f4-9f53-4626-b2b7-aef06c61bc65");
                c["operations"][0]["dispatchIntent"]["action"] = c["operations"][0]["action"].clone();
                rehash(&mut f, &c);
            }
        }
        assert!(f.session.execute(COMMAND, f.params.clone()).is_err(), "{fault}");
        unchanged(&f);
        f.session.execute(CANCEL, f.request).unwrap();
        f.session.shutdown();
    }
}
fn write_bytes(path: &std::path::Path, raw: &[u8]) {
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
    f.write_all(raw).unwrap();
    f.sync_all().unwrap();
}
#[test]
fn original_private_before_record_and_observed_output_are_rechecked_immediately_before_consumption() {
    let _serial = serial();
    for (i, which) in ["before", "output"].into_iter().enumerate() {
        let mut f = fixture(990040 + i, "save_copy", None, true);
        let file = if which == "before" {
            let q = json!({"job":f.request["job"],"operation":f.request["operation"],"capture":f.params["capture"]});
            let r = f.session.execute(RECORD_READ, q).unwrap();
            PathBuf::from(r["record"]["receipt"]["intent"]["destination"].as_str().unwrap())
        } else {
            f.root.join("Chosen.fcproj")
        };
        fs::write(&file, b"later human bytes").unwrap();
        assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
        assert_eq!(fs::read(file).unwrap(), b"later human bytes");
        assert!(f.session.agent_bridge.save_operation.as_ref().unwrap().admission.as_ref().unwrap().original.is_some());
        f.session.execute(CANCEL, f.request).unwrap();
        f.session.shutdown();
    }
}
#[test]
fn same_byte_replaced_original_manager_lease_or_manifest_cannot_be_adopted_at_dispatch() {
    let _serial = serial();
    for (i, name) in ["workspace.json", "workspace.json.lock"].into_iter().enumerate() {
        let mut f = fixture(990050 + i, "save_copy", None, true);
        let file = f.root.join(name);
        let bytes = fs::read(&file).unwrap();
        let replacement = f.root.join("replacement");
        write_bytes(&replacement, &bytes);
        fs::rename(replacement, &file).unwrap();
        assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
        unchanged(&f);
        assert_eq!(fs::read(file).unwrap(), bytes);
        assert!(f.session.execute(CANCEL, f.request).is_err());
        assert!(f.session.previews.pause_writers(&|| Ok(())).is_err());
        f.session.shutdown();
    }
}
#[test]
fn stopped_or_changed_current_root_after_full_capture_refuses_before_native_effects_and_never_recaptures() {
    let _serial = serial();
    for (i, which) in ["activity", "resource", "manifest"].into_iter().enumerate() {
        let mut f = fixture(990060 + i, "save_copy", None, true);
        let ran = Cell::new(false);
        let directory = f.directory.clone();
        let manifest = f.manifest.clone();
        let action = || {
            if !ran.replace(true) {
                let path = match which {
                    "activity" => directory.join("activity.json"),
                    "manifest" => manifest.clone(),
                    _ => {
                        let c: Value = serde_json::from_slice(&fs::read(directory.join("native-save-evidence.json")).unwrap()).unwrap();
                        directory.join(c["operations"][0]["observations"][0]["name"].as_str().unwrap())
                    }
                };
                let bytes = fs::read(&path).unwrap();
                let tmp = directory.join("later-control");
                write_bytes(&tmp, &bytes);
                fs::rename(tmp, path).unwrap();
            }
            Ok(())
        };
        assert!(managed_dispatch::observed_dispatch(&mut f.session, &f.params, &action).is_err());
        assert!(ran.get());
        unchanged(&f);
        assert!(f.session.execute(COMMAND, f.params.clone()).is_err());
        if which == "manifest" {
            assert!(f.session.execute(CANCEL, f.request).is_err());
        } else {
            f.session.execute(CANCEL, f.request).unwrap();
        }
        f.session.shutdown();
    }
}
#[test]
fn partial_native_publication_retains_original_key_and_receipts_and_never_replays_or_cancels() {
    let _serial = serial();
    let mut f = fixture(990070, "save_as", None, true);
    let output = f.root.join("Chosen.fcproj");
    let activity = f.directory.join("activity.json");
    let changed = Cell::new(false);
    let action = || {
        if output.exists() && !changed.replace(true) {
            let bytes = fs::read(&activity).unwrap();
            let tmp = activity.with_extension("replacement");
            write_bytes(&tmp, &bytes);
            fs::rename(tmp, &activity).unwrap();
        }
        Ok(())
    };
    assert!(managed_dispatch::observed_dispatch(&mut f.session, &f.params, &action).is_err());
    assert!(output.exists());
    let bytes = fs::read(&output).unwrap();
    let effects = f.session.execute(EFFECTS, f.request.clone()).unwrap();
    assert_eq!(effects["observation"]["phase"], "AwaitingReconciliation");
    assert!(f.session.execute(COMMAND, f.params).is_err());
    assert!(f.session.execute(CANCEL, f.request).is_err());
    assert_eq!(fs::read(output).unwrap(), bytes);
    assert!(f.session.save_operation_held());
    f.session.shutdown();
}
fn finish(work: &mut crate::session_worker::Pending) -> crate::session_worker::Completion {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = work.try_finish() {
            return v;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn actual_session_worker_dispatch_returns_same_held_key_and_display_only_state_without_worker_release() {
    let _serial = serial();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    let mut f = fixture(990080, "save_copy", Some(&worker), true);
    let mut work = worker.start(&mut f.session, COMMAND, f.params.clone(), None).unwrap();
    assert!(f.session.is_display_only());
    assert!(f.session.execute("file.save", json!({})).is_err());
    let done = finish(&mut work);
    f.session = done.session;
    assert_eq!(done.reply.result.unwrap()["pending"]["phase"], "AwaitingReconciliation");
    assert_eq!(f.session.pending_save_operation_request(), Some(f.request));
    assert!(f.root.join("Chosen.fcproj").exists());
    assert!(f.session.save_operation_held());
    f.session.shutdown();
}
#[test]
fn independent_queued_dispatch_stop_never_cancels_original_Save_or_installs_a_new_admission() {
    let _serial = serial();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let hold = gate.clone();
    let (tx, rx) = mpsc::channel();
    let worker = crate::session_worker::SessionWorker::test_with_executor(move |s, c, p| {
        if c == COMMAND {
            tx.send(()).unwrap();
            let mut open = hold.0.lock().unwrap();
            while !*open {
                open = hold.1.wait(open).unwrap();
            }
        }
        s.execute(c, p)
    })
    .unwrap();
    let mut f = fixture(990090, "save_copy", Some(&worker), true);
    let control = f.session.agent_operation_control();
    let mut work = worker.start(&mut f.session, COMMAND, f.params.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let stopped = control.cancel_save_wait(&f.request).unwrap();
    assert_eq!(stopped["waitStopAccepted"], true);
    assert_eq!(stopped["nativeOperationCancelled"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let done = finish(&mut work);
    f.session = done.session;
    assert!(done.reply.result.is_err());
    unchanged(&f);
    assert!(f.session.agent_bridge.save_operation.as_ref().unwrap().admission.as_ref().unwrap().original.is_some());
    f.session.execute(CANCEL, f.request).unwrap();
    f.session.shutdown();
}
