//! `.fcproj` save/open through the command interface: schema envelope, legacy upgrade, Save a Copy,
//! Revert.

use super::*;
use serde_json::json;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("fc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn saved_file_is_versioned_and_save_copy_keeps_state() {
    let d = temp_dir("schema");
    let path = d.join("p.fcproj").to_string_lossy().to_string();
    let mut s = demo();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["schema_version"], filmcraft_format::SCHEMA_VERSION);
    assert_eq!(v["format"], "filmcraft.project");
    s.execute("sequence.addEdit", json!({"seconds": 1.0})).unwrap();
    assert!(s.is_dirty());
    let copy = d.join("copy.fcproj").to_string_lossy().to_string();
    s.execute("file.saveCopy", json!({"path": copy})).unwrap();
    assert!(s.is_dirty(), "Save a Copy leaves the project unsaved");
    assert_eq!(s.path.as_deref(), Some(path.as_str()));
    let mut t = Session::default();
    t.execute("file.open", json!({"path": copy})).unwrap();
    assert_eq!(*t.project, *s.project);
    // Revert discards the edit.
    s.execute("file.revert", json!({})).unwrap();
    assert!(!s.is_dirty());
    assert_ne!(*t.project, *s.project);
    assert!(!Session::default().is_enabled("file.revert"), "nothing to revert to before a save");
    // No temp files left next to the project.
    let stray: Vec<_> = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
    assert!(stray.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn legacy_project_opens_and_first_save_keeps_a_backup() {
    let d = temp_dir("legacy");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../format/tests/fixtures/v1-edit.fcproj");
    let path = d.join("old.fcproj");
    std::fs::copy(&fixture, &path).unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = Session::default();
    let r = s.execute("file.open", json!({"path": p})).unwrap();
    assert_eq!(r["schemaVersion"], 1);
    assert_eq!(r["migrated"], true);
    assert_eq!(s.project.name, "Legacy Edit");
    assert!(s.drain_events().iter().any(|e| matches!(e, Event::Toast { message, .. } if message.contains("Upgraded"))));
    let r = s.execute("file.save", json!({})).unwrap();
    let backup = r["backup"].as_str().unwrap().to_string();
    assert!(backup.ends_with("old (schema v1 backup).fcproj"), "{backup}");
    assert_eq!(std::fs::read(&backup).unwrap(), std::fs::read(&fixture).unwrap());
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["schema_version"], filmcraft_format::SCHEMA_VERSION);
    // Second save: no new backup.
    assert!(s.execute("file.save", json!({})).unwrap()["backup"].is_null());
    // A file from the future is refused clearly.
    let mut v = v;
    v["schema_version"] = json!(filmcraft_format::SCHEMA_VERSION + 1);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let e = Session::default().execute("file.open", json!({"path": p})).unwrap_err().to_string();
    assert!(e.contains("newer version of FilmCraft"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn project_is_named_after_its_file() {
    let d = temp_dir("name");
    let mut s = demo();
    let a = d.join("Apollo 11 - Tranquility.fcproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": a})).unwrap();
    assert_eq!(s.project.name, "Apollo 11 - Tranquility");
    assert_eq!(s.project.root.name, "Apollo 11 - Tranquility");
    assert!(!s.is_dirty(), "renaming on Save As is not an edit");
    // Save a Copy keeps the current name.
    let c = d.join("Backup copy.fcproj").to_string_lossy().to_string();
    s.execute("file.saveCopy", json!({"path": c})).unwrap();
    assert_eq!(s.project.name, "Apollo 11 - Tranquility");
    s.execute("file.open", json!({"path": c})).unwrap();
    assert_eq!(s.project.name, "Apollo 11 - Tranquility");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn failed_save_preserves_project_identity_dirty_state_and_both_history_branches() {
    let d = temp_dir("failed-save-state");
    let original = d.join("Original.fcproj");
    let mut s = demo();
    s.execute("file.saveAs", json!({"path":original})).unwrap();
    let original_bytes = std::fs::read(&original).unwrap();
    s.execute("sequence.addEdit", json!({"seconds":0.25})).unwrap();
    s.execute("sequence.addEdit", json!({"seconds":0.5})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.is_dirty());
    assert!(!s.history.redo.is_empty());
    let blocker = d.join("blocked-parent");
    std::fs::write(&blocker, "Generated failed save sentinel.").unwrap();
    let target = blocker.join("Unexpected rename.fcproj").to_string_lossy().into_owned();
    for command in ["file.saveAs", "file.save", "file.saveCopy"] {
        if command == "file.save" {
            s.path = Some(target.clone());
        }
        let before = s.project.clone();
        let before_path = s.path.clone();
        let before_revision = s.revision;
        let before_saved = s.saved_revision;
        let undo = s.history.undo.clone();
        let redo = s.history.redo.clone();
        assert!(s.execute(command, if command == "file.save" { json!({}) } else { json!({"path":target}) }).is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &s.project), "{command} replaced the project after a failed write");
        assert_eq!(s.path, before_path);
        assert_eq!(s.revision, before_revision);
        assert_eq!(s.saved_revision, before_saved);
        assert!(s.is_dirty());
        assert_eq!(s.history.undo.len(), undo.len());
        assert_eq!(s.history.redo.len(), redo.len());
        for (actual, previous) in s.history.undo.iter().chain(&s.history.redo).zip(undo.iter().chain(&redo)) {
            assert_eq!(actual.0, previous.0);
            assert!(std::sync::Arc::ptr_eq(&actual.1, &previous.1));
        }
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "Generated failed save sentinel.");
    }
    std::fs::remove_dir_all(d).unwrap();
}

/// Opening the demo after a saved project must not keep that path: Save would overwrite it.
#[test]
fn open_demo_project_clears_saved_path_so_save_does_not_overwrite() {
    let d = temp_dir("demo-path");
    let original = d.join("original.fcproj").to_string_lossy().to_string();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name": "Original"})).unwrap();
    s.execute("file.newBin", json!({"name": "ORIGINAL"})).unwrap();
    s.execute("file.saveAs", json!({"path": original})).unwrap();
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(s.path.is_none(), "demo is unsaved, like New Project");
    let err = s.execute("file.save", json!({})).unwrap_err().to_string();
    assert!(err.contains("no path") || err.contains("Save As"), "{err}");
    let mut reopen = Session::default();
    reopen.execute("file.open", json!({"path": original})).unwrap();
    assert!(
        reopen.project.root.children.iter().any(|e| matches!(e, filmcraft_project::BinEntry::Bin(b) if b.name == "ORIGINAL")),
        "original.fcproj still has the ORIGINAL bin"
    );
    let _ = std::fs::remove_dir_all(&d);
}
