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
fn fixture() -> (Scratch, Value) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!("poema-project-creation-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let request = json!({"kind":"poema-native-project-creation","version":1,"scope":{"projectId":"p_creation","workspaceId":"11111111-1111-4111-8111-111111111111"},"workspace":"11111111-1111-4111-8111-111111111111","update":"22222222-2222-4222-8222-222222222222","name":"New film","projectPath":root.join("New film.fcproj"),"activationToken":format!("{:064x}",1)});
    (Scratch(root), request)
}
fn created(request: &Value) -> Session {
    let mut s = Session::default();
    s.create_project_workspace(request).unwrap();
    s
}
fn control(s: &mut Session) -> crate::session_worker::SessionWorker {
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(s).unwrap();
    s.agent_operation_control().enable_transport();
    worker
}
fn activate(s: &mut Session, r: &Value) -> Result<Value> {
    let f = s.workspace_update_status().unwrap();
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":f["token"],"scope":r["scope"],"workspace":r["workspace"],"update":r["update"],"nativeSession":f["nativeSession"]}),
    )
}
#[test]
fn empty_creation_reuses_the_human_constructor_and_native_format_without_a_seed_sequence_or_history() {
    let (dir, r) = fixture();
    let s = created(&r);
    let f = s.workspace_update_status().unwrap();
    let path = PathBuf::from(r["projectPath"].as_str().unwrap());
    let raw = fs::read(&path).unwrap();
    let mut human = Session::default();
    human.execute("file.newProject", json!({"name":r["name"],"workspace":r["scope"]})).unwrap();
    assert_eq!(*s.project, *human.project);
    assert!(s.project.sequences().next().is_none());
    assert!(s.state.active_sequence.is_none());
    assert!(s.history.undo.is_empty() && s.history.redo.is_empty());
    assert!(s.agent_bridge.tickets.is_empty());
    assert_eq!(s.revision, 2);
    assert!(!s.is_dirty());
    assert!(s.persistence.is_none());
    assert!(s.is_display_only());
    assert_eq!(f["projectCreation"]["sha256"], history_checkpoint::digest(&raw));
    assert_eq!(f["projectCreation"]["bytes"], raw.len());
    assert_eq!(f["projectCreation"]["history"], "fresh");
    assert_eq!(f["projectCreation"]["mediaEvidence"], false);
    assert_eq!(f["projectCreation"]["sequence"], Value::Null);
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":path})).unwrap();
    assert_eq!(*s.project, *reopened.project);
    assert!(reopened.history.undo.is_empty());
    assert_eq!(reopened.matches_project_workspace("p_creation", r["workspace"].as_str().unwrap()).unwrap(), Some(true));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
    }
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2); // complete project and inert publication candidate
}
#[test]
fn creation_rejects_live_sessions_without_changing_unsaved_project_history_or_disk() {
    let (dir, r) = fixture();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Existing human work"})).unwrap();
    s.execute("file.newBin", json!({"name":"Human unsaved bin"})).unwrap();
    let before = s.project.clone();
    let revision = s.revision;
    let history = s.history.undo.len();
    assert!(s.create_project_workspace(&r).is_err());
    assert!(std::sync::Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.undo.len(), history);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
}
#[test]
fn hostile_creation_descriptors_fail_before_any_publication_or_session_mutation() {
    let (dir, r) = fixture();
    let mut bads = vec![Value::Null, json!({}), json!([])];
    for (key, value) in [
        ("version", json!(3)),
        ("kind", json!("foreign")),
        ("name", json!("")),
        ("name", json!("x".repeat(1025))),
        ("name", json!("bad\u{85}name")),
        ("activationToken", json!("short")),
        ("workspace", json!("33333333-3333-4333-8333-333333333333")),
        ("update", json!("foreign")),
        ("projectPath", json!("relative.fcproj")),
        ("extra", json!(true)),
    ] {
        let mut bad = r.clone();
        bad[key] = value;
        bads.push(bad);
    }
    let mut bad = r.clone();
    bad["scope"]["anchorSequence"] = json!(1);
    bads.push(bad);
    for bad in bads {
        let mut s = Session::default();
        let before = s.project.clone();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.create_project_workspace(&bad))).unwrap().is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, 1);
        assert!(s.agent_bridge.workspace_update.is_none());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }
}
#[test]
fn existing_target_and_redirected_parent_refuse_without_overwriting_or_adopting() {
    let (dir, r) = fixture();
    let path = PathBuf::from(r["projectPath"].as_str().unwrap());
    fs::write(&path, b"Existing data stays unchanged").unwrap();
    let before = fs::read(&path).unwrap();
    let mut s = Session::default();
    assert!(s.create_project_workspace(&r).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(s.revision, 1);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    #[cfg(unix)]
    {
        let real = dir.0.join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.0.join("redirected");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut redirected = r.clone();
        redirected["projectPath"] = json!(link.join("new.fcproj"));
        assert!(s.create_project_workspace(&redirected).is_err());
        assert_eq!(fs::read_dir(real).unwrap().count(), 0);
    }
}
#[test]
fn creation_remains_paused_until_exact_native_activation_and_starts_only_its_new_persistence_root() {
    let (dir, r) = fixture();
    let mut s = created(&r);
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).unwrap();
    let _worker = control(&mut s);
    for (command, p) in [("file.newBin", json!({"name":"Must not run"})), ("file.save", json!({})), ("edit.undo", json!({}))] {
        assert!(s.execute(command, p).is_err());
    }
    let read = s.execute("agent.readProject", json!({"token":format!("{:064x}",2),"scope":r["scope"],"projectPath":r["projectPath"]})).unwrap();
    assert_eq!(read["sequence"], Value::Null);
    assert_eq!(read["mediaEvidence"], false);
    let fence = s.workspace_update_status().unwrap();
    let foreign = json!({"token":fence["token"],"scope":r["scope"],"workspace":r["workspace"],"update":r["update"],"nativeSession":"f".repeat(32)});
    assert!(s.execute("agent.activateWorkspaceUpdate", foreign).is_err());
    assert!(s.persistence.is_none());
    assert_eq!(activate(&mut s, &r).unwrap()["status"], "activated");
    assert!(s.persistence.is_some());
    assert!(!s.is_display_only());
    assert!(s.history.undo.is_empty());
    s.execute("file.newBin", json!({"name":"Human new work"})).unwrap();
    assert_eq!(s.history.undo.len(), 1);
}
#[test]
fn changed_created_bytes_refuse_activation_and_preserve_the_paused_candidate() {
    let (dir, r) = fixture();
    let mut s = created(&r);
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).unwrap();
    let _worker = control(&mut s);
    let before = s.workspace_update_status().unwrap();
    fs::write(r["projectPath"].as_str().unwrap(), b"changed generated output").unwrap();
    assert!(activate(&mut s, &r).is_err());
    assert_eq!(s.workspace_update_status().unwrap(), before);
    assert!(s.persistence.is_none());
}
#[test]
fn persistence_collision_retains_the_complete_new_project_without_claiming_an_active_writer() {
    let (dir, r) = fixture();
    fs::create_dir(dir.0.join("native-state")).unwrap();
    let mut s = created(&r);
    let raw = fs::read(r["projectPath"].as_str().unwrap()).unwrap();
    assert!(s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).is_err());
    assert!(s.persistence.is_none());
    assert!(s.is_display_only());
    assert_eq!(fs::read(r["projectPath"].as_str().unwrap()).unwrap(), raw);
}

#[test]
fn version2_creation_preserves_exact_display_title_through_human_saves_and_cold_reopen() {
    let (dir, mut r) = fixture();
    r["version"] = json!(2);
    r["name"] = json!("generated / 金 — final");
    r["projectPath"] = json!(dir.0.join("project.fcproj"));
    let mut s = created(&r);
    let mut human = Session::default();
    human.execute("file.newProject", json!({"name":r["name"],"workspace":r["scope"],"preserveDisplayTitle":true})).unwrap();
    assert_eq!(*human.project, *s.project);
    assert_eq!(s.workspace_update_status().unwrap()["projectCreation"]["version"], 2);
    s.configure_workspace_update_persistence(crate::autosave::AutosaveConfig::new(dir.0.join("native-state"))).unwrap();
    let _worker = control(&mut s);
    activate(&mut s, &r).unwrap();
    s.execute("file.newBin", json!({"name":"Human work"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert_eq!(s.project.name, r["name"].as_str().unwrap());
    assert_eq!(s.project.root.name, s.project.name);
    let undo = s.history.undo.len();
    let moved = dir.0.join("another-storage-file.fcproj");
    s.execute("file.saveAs", json!({"path":moved})).unwrap();
    assert_eq!(s.project.name, r["name"].as_str().unwrap());
    assert_eq!(s.history.undo.len(), undo);
    let copy = dir.0.join("copy.fcproj");
    s.execute("file.saveCopy", json!({"path":copy})).unwrap();
    for file in [moved, copy] {
        let mut reopened = Session::default();
        reopened.execute("file.open", json!({"path":file})).unwrap();
        assert_eq!(reopened.project.name, r["name"].as_str().unwrap());
        assert!(crate::project_workspace::preserves_display_title(&reopened.project).unwrap());
        reopened.execute("file.save", json!({})).unwrap();
        assert_eq!(reopened.project.name, r["name"].as_str().unwrap());
    }
}
#[test]
fn version2_unsafe_filenames_and_malformed_display_titles_refuse_before_publication() {
    let (dir, mut r) = fixture();
    r["version"] = json!(2);
    r["name"] = json!("A / film");
    for leaf in ["../escaped.fcproj", "CON.fcproj", "com1.fcproj", "LPT9.fcproj", "non portable.fcproj", ".hidden.fcproj", "trailing_.fcproj ", "金.fcproj"] {
        let mut bad = r.clone();
        bad["projectPath"] = json!(dir.0.join(leaf));
        let mut s = Session::default();
        assert!(s.create_project_workspace(&bad).is_err());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }
    r["projectPath"] = json!(dir.0.join("project.fcproj"));
    for name in ["", "bad\u{0085}title", &"é".repeat(513)] {
        let mut bad = r.clone();
        bad["name"] = json!(name);
        let mut s = Session::default();
        assert!(s.create_project_workspace(&bad).is_err());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }
}
#[test]
fn malformed_display_title_marker_refuses_human_save_without_renaming_or_writing() {
    let (dir, r) = fixture();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Exact / title","workspace":r["scope"],"preserveDisplayTitle":true})).unwrap();
    for raw in [
        "broken".into(),
        json!({"kind":"poema-native-display-title","version":1,"scope":{"projectId":"p_foreign","workspaceId":r["workspace"]}}).to_string(),
        "x".repeat(2049),
    ] {
        std::sync::Arc::make_mut(&mut s.project).metadata.insert("poema.displayTitle.v1".into(), raw);
        let before = s.project.clone();
        let file = dir.0.join("not-written.fcproj");
        assert!(s.execute("file.saveAs", json!({"path":file})).is_err());
        assert_eq!(*s.project, *before);
        assert!(!file.exists());
    }
    assert!(Session::default().execute("file.newProject", json!({"name":"No workspace","preserveDisplayTitle":true})).is_err());
}

#[test]
fn ordinary_human_projects_keep_filename_naming_without_explicit_display_title_policy() {
    let (dir, r) = fixture();
    for params in [json!({"name":"Ordinary"}), json!({"name":"Ordinary","workspace":r["scope"],"preserveDisplayTitle":false})] {
        let mut s = Session::default();
        s.execute("file.newProject", params).unwrap();
        let path = dir.0.join("Human chosen filename.fcproj");
        s.execute("file.saveAs", json!({"path":path})).unwrap();
        assert_eq!(s.project.name, "Human chosen filename");
        assert!(!crate::project_workspace::preserves_display_title(&s.project).unwrap());
    }
}
