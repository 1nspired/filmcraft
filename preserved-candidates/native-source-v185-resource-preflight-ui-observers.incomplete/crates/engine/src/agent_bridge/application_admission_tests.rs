use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Scratch, Session, Value) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("poema-app-admission-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    fs::create_dir(dir.join("edit")).unwrap();
    let r = json!({"kind":"poema-native-project-creation","version":3,"scope":{"projectId":"p_admission","workspaceId":"11111111-1111-4111-8111-111111111111"},"workspace":"11111111-1111-4111-8111-111111111111","update":"22222222-2222-4222-8222-222222222222","name":"Exact / title","projectPath":dir.join("edit/project.fcproj"),"activationToken":"a".repeat(64),"applicationProject":{"kind":"poema-native-application-admission","version":1,"operationId":"33333333-3333-4333-8333-333333333333","readyFile":dir.join("native-admission-ready.json")}});
    let mut s = Session::default();
    s.create_project_workspace(&r).unwrap();
    (Scratch(dir), s, r)
}
fn status(s: &mut Session, r: &Value) -> Value {
    s.execute(application_admission::STATUS, json!({"scope":r["scope"],"operationId":r["applicationProject"]["operationId"],"projectPath":r["projectPath"]}))
        .unwrap()
}
fn activate(dir: &Scratch, s: &mut Session, r: &Value) -> crate::session_worker::SessionWorker {
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).unwrap();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(s).unwrap();
    s.agent_operation_control().enable_transport();
    let f = s.workspace_update_status().unwrap();
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":f["token"],"scope":r["scope"],"workspace":r["workspace"],"update":r["update"],"nativeSession":f["nativeSession"]}),
    )
    .unwrap();
    worker
}
fn completion(s: &mut Session, r: &Value) -> Value {
    let v = status(s, r);
    json!({"kind":"poema-native-application-ready","version":1,"operationId":v["operationId"],"birthOperationId":r["update"],"scope":r["scope"],"projectPath":r["projectPath"],"nativePid":v["nativePid"],"nativeSession":v["nativeSession"],"checkpointSha256":"b".repeat(64),"fingerprint":"c".repeat(64),"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false})
}
fn publish(r: &Value, v: &Value) {
    let p = r["applicationProject"]["readyFile"].as_str().unwrap();
    fs::write(p, serde_json::to_vec(v).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
#[test]
fn application_hold_survives_native_activation_and_cancel_until_its_exact_private_completion() {
    let (dir, mut s, r) = fixture();
    assert!(s.is_display_only());
    assert_eq!(status(&mut s, &r)["status"], "held");
    let _worker = activate(&dir, &mut s, &r);
    assert!(s.persistence.is_some());
    assert!(s.workspace_update_status().is_none());
    assert!(s.is_display_only());
    let before = s.project.clone();
    for command in ["file.newBin", "file.newProject", "file.save", "edit.undo", "agent.apply"] {
        assert!(s.execute(command, json!({"name":"Must not run"})).is_err());
        assert_eq!(*s.project, *before);
    }
    assert_eq!(s.cancel_workspace_update().unwrap(), None);
    assert!(s.is_display_only());
    let token = "d".repeat(64);
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":r["scope"]})).unwrap();
    s.execute(
        "agent.beginWorkspaceUpdate",
        json!({"token":token,"scope":r["scope"],"workspace":r["workspace"],"update":"44444444-4444-4444-8444-444444444444"}),
    )
    .unwrap();
    s.cancel_workspace_update().unwrap();
    assert!(s.is_display_only());
    let ready = completion(&mut s, &r);
    publish(&r, &ready);
    assert!(!s.is_display_only());
    assert_eq!(status(&mut s, &r)["status"], "ready");
    assert!(s.history.undo.is_empty());
    s.execute("file.newBin", json!({"name":"Later human edit"})).unwrap();
    assert_eq!(s.history.undo.len(), 1);
    let ready_path = r["applicationProject"]["readyFile"].as_str().unwrap();
    let accepted = fs::read(ready_path).unwrap();
    let later = s.project.clone();
    for key in ["checkpointSha256", "fingerprint"] {
        let mut changed = ready.clone();
        changed[key] = json!("e".repeat(64));
        publish(&r, &changed);
        assert!(s.is_display_only());
        assert_eq!(status(&mut s, &r)["admitted"], true);
        assert!(s.execute("file.newBin", json!({"name":"Must not run"})).is_err());
        assert_eq!(*s.project, *later);
        assert_eq!(s.history.undo.len(), 1);
        fs::write(ready_path, &accepted).unwrap();
        assert!(!s.is_display_only());
    }
    let mut respaced = accepted.clone();
    respaced.push(b'\n');
    fs::write(ready_path, respaced).unwrap();
    assert!(s.is_display_only());
    fs::write(ready_path, accepted).unwrap();
    assert!(!s.is_display_only());
    s.execute("file.saveAs", json!({"path":dir.0.join("edit/another.fcproj")})).unwrap();
    assert!(!s.is_display_only());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.root.children.is_empty());
}
#[test]
fn missing_changed_foreign_or_redirected_application_readiness_keeps_native_history_and_files_untouched() {
    let (dir, mut s, r) = fixture();
    let _worker = activate(&dir, &mut s, &r);
    let ready = completion(&mut s, &r);
    let before = s.project.clone();
    let native = fs::read(r["projectPath"].as_str().unwrap()).unwrap();
    for (key, value) in [
        ("nativeSession", json!("f".repeat(32))),
        ("nativePid", json!(0)),
        ("birthOperationId", json!("44444444-4444-4444-8444-444444444444")),
        ("scope", json!({"projectId":"p_foreign","workspaceId":r["workspace"]})),
        ("projectPath", json!(dir.0.join("edit/foreign.fcproj"))),
        ("operationId", json!("44444444-4444-4444-8444-444444444444")),
        ("checkpointSha256", json!("short")),
        ("mediaEvidence", json!(true)),
        ("extra", json!(true)),
    ] {
        let mut bad = ready.clone();
        bad[key] = value;
        publish(&r, &bad);
        assert!(s.is_display_only());
        assert!(s.execute("file.newBin", json!({"name":"Must not run"})).is_err());
        assert_eq!(*s.project, *before);
    }
    let path = PathBuf::from(r["applicationProject"]["readyFile"].as_str().unwrap());
    fs::write(&path, b"broken").unwrap();
    assert!(s.is_display_only());
    fs::write(&path, vec![b'x'; 4097]).unwrap();
    assert!(s.is_display_only());
    publish(&r, &ready);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(s.is_display_only());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(!s.is_display_only());
    fs::remove_file(&path).unwrap();
    assert!(s.is_display_only());
    #[cfg(unix)]
    {
        let other = dir.0.join("other.json");
        fs::write(&other, serde_json::to_vec(&ready).unwrap()).unwrap();
        std::os::unix::fs::symlink(other, &path).unwrap();
        assert!(s.is_display_only());
        fs::remove_file(&path).unwrap();
    }
    assert_eq!(fs::read(r["projectPath"].as_str().unwrap()).unwrap(), native);
    assert!(s.history.undo.is_empty());
}
#[test]
fn application_ready_cannot_be_adopted_from_a_replaced_directory_or_an_existing_startup_record() {
    let (dir, mut s, r) = fixture();
    let _worker = activate(&dir, &mut s, &r);
    let ready = completion(&mut s, &r);
    publish(&r, &ready);
    assert!(!s.is_display_only());
    let moved = dir.0.with_extension("moved");
    fs::rename(&dir.0, &moved).unwrap();
    fs::create_dir(&dir.0).unwrap();
    fs::create_dir(dir.0.join("edit")).unwrap();
    publish(&r, &ready);
    assert!(s.is_display_only());
    fs::remove_dir_all(&dir.0).unwrap();
    fs::rename(&moved, &dir.0).unwrap();
    assert!(!s.is_display_only());
    let mut candidate = r.clone();
    candidate["projectPath"] = json!(dir.0.join("edit/new.fcproj"));
    let mut fresh = Session::default();
    assert!(fresh.create_project_workspace(&candidate).is_err());
    assert!(!dir.0.join("edit/new.fcproj").exists());
}
