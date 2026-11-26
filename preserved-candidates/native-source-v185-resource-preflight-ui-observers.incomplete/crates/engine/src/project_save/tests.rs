use super::*;
use crate::commands::project_name_for;
use std::{collections::BTreeMap, io, sync::Mutex};

#[derive(Default)]
struct Files {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    reads: Mutex<Vec<String>>,
    writes: Mutex<Vec<(String, Vec<u8>)>>,
    fail: Mutex<Option<String>>,
}
impl Services for Files {
    fn read_file(&self, path: &str) -> io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push(path.into());
        self.files.lock().unwrap().get(path).cloned().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn write_file(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.writes.lock().unwrap().push((path.into(), bytes.to_vec()));
        if self.fail.lock().unwrap().as_deref() == Some(path) {
            return Err(io::Error::other("generated write failure"));
        }
        self.files.lock().unwrap().insert(path.into(), bytes.to_vec());
        Ok(())
    }
}
fn generated_path(name: &str) -> String {
    crate::temp_dir().join(format!("fc-save-service-{}", std::process::id())).join(name).to_string_lossy().into_owned()
}
fn fixture(preserve_title: bool) -> Session {
    let mut s = Session::default();
    if preserve_title {
        s.execute("file.newProject",json!({"name":"The editor's film","workspace":{"projectId":"p_generated_save","workspaceId":"12345678-1234-1234-1234-123456789abc"},"preserveDisplayTitle":true})).unwrap();
    } else {
        s.execute("file.openDemoProject", json!({})).unwrap();
    }
    s.execute("file.newBin", json!({"name":"Kept human bin"})).unwrap();
    s.execute("file.newBin", json!({"name":"Undone human bin"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.path = Some(generated_path("Original.fcproj"));
    s.saved_revision = s.revision.saturating_sub(1);
    s.saved_file = Some(saved_file::Baseline::from_bytes(s.path.as_deref().unwrap(), b"original saved bytes"));
    s.previews.set_dir(None);
    s
}
fn equivalent(template: &Session, files: Arc<Files>) -> Session {
    let mut s = Session::default();
    s.project = template.project.clone();
    s.state = template.state.clone();
    s.path = template.path.clone();
    s.revision = template.revision;
    s.saved_revision = template.saved_revision;
    s.saved_file = template.saved_file.clone();
    s.loaded_schema = template.loaded_schema;
    s.history = template.history.clone();
    s.prefs = template.prefs.clone();
    s.prefs_path = template.prefs_path.clone();
    s.previews.set_dir(None);
    s.services = files;
    s
}
fn native_state(s: &Session) -> Value {
    let branch = |rows: &[(String, Arc<Project>)]| rows.iter().map(|(label, p)| json!({"label":label,"project":p.as_ref()})).collect::<Vec<_>>();
    json!({"project":s.project.as_ref(),"path":s.path,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":s.saved_file,"loadedSchema":s.loaded_schema,"state":s.state,"undo":branch(&s.history.undo),"redo":branch(&s.history.redo),"mergeKey":s.history.merge_key,"historyLimit":s.history.limit,"prefs":s.prefs,"prefsPath":s.prefs_path,"previewDir":s.previews.dir(),"previewTemp":s.previews.temp_root()})
}
fn without_ms(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("ms");
    value
}

#[test]
fn preparation_is_read_only_and_freezes_original_complete_project_and_view_bytes() {
    let template = fixture(false);
    let files = Arc::new(Files::default());
    let s = equivalent(&template, files.clone());
    let before = native_state(&s);
    let prepared = prepare(&s, &generated_path("A new storage name.fcproj"), true).unwrap();
    assert_eq!(native_state(&s), before);
    assert!(Arc::ptr_eq(&s.project, &template.project));
    assert!(files.reads.lock().unwrap().is_empty());
    assert!(files.writes.lock().unwrap().is_empty());
    let view = s.project_view();
    let mut expected = s.project.as_ref().clone();
    expected.name = "A new storage name".into();
    expected.root.name = expected.name.clone();
    assert_eq!(prepared.bytes, filmcraft_format::encode_with_view(&expected, Some(&view), false));
    assert!(s.is_dirty());
    assert!(!s.history.undo.is_empty());
    assert!(!s.history.redo.is_empty());
}
#[test]
fn shared_service_matches_independent_original_save_body_for_all_adopt_copy_title_schema_and_destination_variants() {
    let mut cases = 0;
    for preserve in [false, true] {
        for adopt in [false, true] {
            for target in ["Original.fcproj", "New title.fcproj", "Existing.fcproj"] {
                for legacy in [false, true] {
                    for backup_exists in [false, true] {
                        let mut template = fixture(preserve);
                        if legacy {
                            template.loaded_schema = 1;
                        }
                        let a = Arc::new(Files::default());
                        let z = Arc::new(Files::default());
                        let dest = generated_path(target);
                        for files in [&a, &z] {
                            let mut f = files.files.lock().unwrap();
                            f.insert(template.path.clone().unwrap(), b"old loaded bytes".to_vec());
                            f.insert(generated_path("Existing.fcproj"), b"existing human destination".to_vec());
                            if backup_exists {
                                f.insert(schema_backup_path(template.path.as_deref().unwrap(), 1), b"earlier preserved schema backup".to_vec());
                            }
                        }
                        let mut actual = equivalent(&template, a.clone());
                        let mut original = equivalent(&template, z.clone());
                        let got = prepare(&actual, &dest, adopt).unwrap().publish(&mut actual).unwrap();
                        let wanted = original_write_project(&mut original, &dest, adopt).unwrap();
                        assert_eq!(without_ms(got), without_ms(wanted));
                        assert_eq!(native_state(&actual), native_state(&original));
                        assert_eq!(*a.files.lock().unwrap(), *z.files.lock().unwrap());
                        assert_eq!(*a.reads.lock().unwrap(), *z.reads.lock().unwrap());
                        assert_eq!(*a.writes.lock().unwrap(), *z.writes.lock().unwrap());
                        assert!(branch_same(&actual.history.undo, &template.history.undo));
                        assert!(branch_same(&actual.history.redo, &template.history.redo));
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 48);
}
#[test]
fn stale_preparation_refuses_changed_project_editor_path_saved_basis_history_preferences_previews_and_service_before_any_file_access() {
    let mutations: [fn(&mut Session); 18] = [
        |s| s.revision += 1,
        |s| s.saved_revision = s.revision,
        |s| s.path = Some(generated_path("Later path.fcproj")),
        |s| s.loaded_schema = 1,
        |s| s.saved_file = None,
        |s| s.state.source_playhead = filmcraft_time::Tick(1),
        |s| s.prefs.general.recent_projects.push("Later human preference".into()),
        |s| s.prefs_path = Some(crate::temp_dir().join("later-preferences.json")),
        |s| s.previews = Arc::new(previews::PreviewStore::temp()),
        |s| s.previews.set_dir(Some(crate::temp_dir().join("later-human-previews"))),
        |s| s.previews.set_temp_root(Some(crate::temp_dir().join("later-human-scratch"))),
        |s| s.services = Arc::new(Files::default()),
        |s| s.project = Arc::new(s.project.as_ref().clone()),
        |s| s.history.undo[0].0 = "Later human undo".into(),
        |s| s.history.redo[0].0 = "Later human redo".into(),
        |s| s.history.limit += 1,
        |s| s.history.merge_key = Some("later-human-gesture".into()),
        |s| s.history.redo[0].1 = Arc::new(s.history.redo[0].1.as_ref().clone()),
    ];
    for mutate in mutations {
        let t = fixture(false);
        let files = Arc::new(Files::default());
        let mut s = equivalent(&t, files.clone());
        let prepared = prepare(&s, &generated_path("Must not be written.fcproj"), true).unwrap();
        mutate(&mut s);
        let later = native_state(&s);
        let project = s.project.clone();
        let error = prepared.publish(&mut s).unwrap_err().to_string();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(native_state(&s), later);
        assert!(Arc::ptr_eq(&project, &s.project));
        assert!(files.reads.lock().unwrap().is_empty());
        assert!(files.writes.lock().unwrap().is_empty());
    }
}
#[test]
fn failed_publication_preserves_original_human_state_and_exact_schema_backup_partial_effects_without_adoption() {
    for adopt in [false, true] {
        for fail_backup in [false, true] {
            let mut t = fixture(false);
            t.loaded_schema = 1;
            let path = t.path.clone().unwrap();
            let a = Arc::new(Files::default());
            let z = Arc::new(Files::default());
            let failure = if fail_backup { schema_backup_path(&path, 1) } else { path.clone() };
            for files in [&a, &z] {
                files.files.lock().unwrap().insert(path.clone(), b"old schema bytes".to_vec());
                *files.fail.lock().unwrap() = Some(failure.clone());
            }
            let mut actual = equivalent(&t, a.clone());
            let mut original = equivalent(&t, z.clone());
            let before = native_state(&actual);
            let outcome = prepare(&actual, &path, adopt).unwrap().publish(&mut actual);
            let expected = original_write_project(&mut original, &path, adopt);
            assert_eq!(outcome.is_ok(), expected.is_ok());
            if let Err(e) = outcome {
                assert_eq!(e.to_string(), expected.unwrap_err().to_string());
                assert_eq!(native_state(&actual), before);
                assert!(Arc::ptr_eq(&actual.project, &t.project));
            }
            assert_eq!(native_state(&actual), native_state(&original));
            assert_eq!(*a.files.lock().unwrap(), *z.files.lock().unwrap());
            assert_eq!(*a.writes.lock().unwrap(), *z.writes.lock().unwrap());
        }
    }
}
#[test]
fn malformed_owning_display_title_refuses_preparation_without_observing_or_publishing_a_file() {
    let t = fixture(true);
    let files = Arc::new(Files::default());
    let mut s = equivalent(&t, files.clone());
    Arc::make_mut(&mut s.project).metadata.insert("poema.displayTitle.v1".into(), "{}".into());
    let before = native_state(&s);
    assert!(prepare(&s, &generated_path("Wrong title.fcproj"), true).is_err());
    assert_eq!(native_state(&s), before);
    assert!(files.reads.lock().unwrap().is_empty());
    assert!(files.writes.lock().unwrap().is_empty());
}
#[test]
fn original_save_aliases_publish_full_view_without_new_undo_steps_and_same_path_copy_drops_only_saved_baseline() {
    let t = fixture(false);
    let files = Arc::new(Files::default());
    let mut s = equivalent(&t, files.clone());
    let original = s.path.clone().unwrap();
    let steps = s.history.clone();
    for (command, args) in [("file.save", json!({})), ("file.saveAll", json!({})), ("file.saveAs", json!({"path":generated_path("Adopted.fcproj")}))] {
        s.execute(command, args).unwrap();
        assert!(!s.is_dirty());
        assert!(s.saved_file.is_some());
        assert!(branch_same(&s.history.undo, &steps.undo));
        assert!(branch_same(&s.history.redo, &steps.redo));
        let path = s.path.as_ref().unwrap();
        assert_eq!(files.files.lock().unwrap()[path], filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false));
    }
    s.execute("file.newBin", json!({"name":"Later unsaved human bin"})).unwrap();
    let path = s.path.clone().unwrap();
    let before = native_state(&s);
    let project = s.project.clone();
    s.execute("file.saveCopy", json!({"path":path})).unwrap();
    let mut expected = before;
    expected["savedFile"] = Value::Null;
    assert_eq!(native_state(&s), expected);
    assert!(s.is_dirty());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_ne!(s.path.as_deref(), Some(original.as_str()));
}

// Frozen original commands.rs body: the production refactor must be compared
// with this independent prior implementation, not with its own helper result.
fn name_project(p: &mut filmcraft_project::Project, name: String) {
    p.root.name.clone_from(&name);
    p.name = name;
}

fn original_write_project(s: &mut Session, path: &str, adopt: bool) -> Result<Value> {
    let t0 = web_time::Instant::now();
    // Stage Save As naming until the write succeeds. A failed save must retain
    // the current document identity, metadata and history.
    let mut project = s.project.clone();
    let preserve_title = crate::project_workspace::preserves_display_title(&project)?;
    if adopt
        && !preserve_title
        && let Some(name) = project_name_for(path)
        && project.name != name
    {
        name_project(std::sync::Arc::make_mut(&mut project), name);
    }
    // what is open (sequence tabs, how each is shown) goes into the file beside the project
    let view = s.project_view();
    let bytes = filmcraft_format::encode_with_view(&project, Some(&view), false);
    let mut backup = None;
    if adopt && s.path.as_deref() == Some(path) && s.loaded_schema < filmcraft_format::SCHEMA_VERSION {
        let b = schema_backup_path(path, s.loaded_schema);
        if s.services.read_file(&b).is_err()
            && let Ok(old) = s.services.read_file(path)
        {
            s.services.write_file(&b, &old).map_err(|e| EngineError::Other(format!("{b}: {e}")))?;
            backup = Some(b);
        }
    }
    s.services.write_file(path, &bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if adopt {
        s.project = project;
        s.path = Some(path.to_string());
        s.saved_file = Some(crate::saved_file::Baseline::from_bytes(path, &bytes));
        s.previews_follow_path();
        s.note_recent_project();
        s.saved_revision = s.revision;
        s.loaded_schema = filmcraft_format::SCHEMA_VERSION;
    } else if s.path.as_deref() == Some(path) {
        // Save a Copy does not adopt the document. Overwriting the current
        // path through that door must not leave an obsolete recovery baseline.
        s.saved_file = None;
    }
    Ok(json!({"path": path, "bytes": bytes.len(), "ms": t0.elapsed().as_secs_f64() * 1000.0, "backup": backup}))
}

fn schema_backup_path(path: &str, schema: u32) -> String {
    let stem = path.strip_suffix(".fcproj").unwrap_or(path);
    format!("{stem} (schema v{schema} backup).fcproj")
}
