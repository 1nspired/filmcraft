use super::*;
use std::sync::{Arc, Mutex, atomic::Ordering};

pub(super) fn populate(s: &mut Session) {
    s.shortcuts.register_external(vec![crate::shortcuts::CommandInfo::new("test.auxiliary", "Auxiliary command", &["Test"], Some("Cmd+Alt+J"))]);
    s.shortcuts.begin_editing();
    s.shortcuts.set("test.auxiliary", "Cmd+Alt+K", None, false, true).unwrap();
    s.shortcuts.set("test.auxiliary", "Cmd+Alt+L", None, false, true).unwrap();
    assert!(s.shortcuts.undo());
    s.presets.user = crate::presets::builtin_presets().into_iter().take(1).collect();
    s.export_presets.user = filmcraft_export::builtin_presets().into_iter().take(1).collect();
    s.export_presets.favorites = vec![s.export_presets.user[0].name.clone()];
    s.browser.dir = "/generated/media".into();
    s.browser.back = vec!["/generated".into()];
    s.browser.forward = vec!["/generated/next".into()];
    s.browser.selection = vec!["/generated/media/test.mxf".into()];
    s.offline.prompt = true;
    s.media.add_stored_paths(vec!["relative/test.mxf".into()], Some("/generated/project.fcproj"));
    s.media.add_stored_paths(vec!["unsaved.mxf".into()], None);
}
fn encoded(s: &Session) -> Value {
    serde_json::to_value(native_auxiliary::Snapshot::capture(s).unwrap()).unwrap()
}
#[test]
fn auxiliary_recovery_shared_snapshot_retains_shortcut_history_defaults_libraries_browser_and_origins() {
    let mut source = Session::default();
    populate(&mut source);
    let expected = encoded(&source);
    let snapshot = native_editor::Snapshot::capture(&source).unwrap();
    let raw = history_checkpoint::bytes(&snapshot, 80 * 1024 * 1024).unwrap();
    let decoded: native_editor::Snapshot = serde_json::from_slice(&raw).unwrap();
    let mut cold = Session::default();
    decoded.install(&mut cold).unwrap();
    assert_eq!(encoded(&cold), expected);
    assert!(cold.shortcuts.can_undo());
    assert!(cold.shortcuts.can_redo());
    assert!(cold.shortcuts.redo());
    assert_eq!(cold.shortcuts.primary("test.auxiliary").as_deref(), Some("Cmd+Alt+L"));
    cold.shortcuts.cancel_editing();
    assert_eq!(cold.shortcuts.primary("test.auxiliary").as_deref(), Some("Cmd+Alt+J"));
    assert!(cold.presets.user[0].builtin);
    assert!(cold.export_presets.user[0].builtin);
    assert_eq!(cold.media.path_origin("relative/test.mxf"), source.media.path_origin("relative/test.mxf"));
    assert_eq!(cold.media.path_origin("unsaved.mxf"), Some(None));
    assert_eq!(cold.media.path_origin("user-picked.mxf"), None);
    assert!(!cold.worker_available);
    assert!(cold.persistence.is_none());
}
#[test]
fn auxiliary_recovery_completed_job_is_inert_and_retains_nonfinite_report_bits() {
    let mut source = Session::default();
    let progress = Arc::new(filmcraft_export::Progress::default());
    progress.done.store(9, Ordering::Relaxed);
    progress.total.store(10, Ordering::Relaxed);
    progress.finished.store(true, Ordering::Relaxed);
    *progress.status.lock().unwrap() = "Completed".into();
    let nan = f64::from_bits(0x7ff8000000000042);
    let report = filmcraft_export::Report {
        path: "/generated/export.mp4".into(),
        frames: 9,
        seconds: nan,
        bytes: 123,
        render_fps: f64::NEG_INFINITY,
        extra_files: vec!["/generated/audio.mxf".into()],
    };
    source.jobs.push(crate::Job { id: 42, label: "Old export".into(), progress: progress.clone(), result: Arc::new(Mutex::new(Some(Ok(report)))) });
    let snapshot = native_auxiliary::Snapshot::capture(&source).unwrap();
    let raw = history_checkpoint::bytes(&snapshot, 80 * 1024 * 1024).unwrap();
    let decoded: native_auxiliary::Snapshot = serde_json::from_slice(&raw).unwrap();
    let mut cold = Session::default();
    decoded.install(&mut cold).unwrap();
    assert_eq!(encoded(&cold), encoded(&source));
    assert!(!Arc::ptr_eq(&cold.jobs[0].progress, &progress));
    assert!(!Arc::ptr_eq(&cold.jobs[0].result, &source.jobs[0].result));
    let result = cold.jobs[0].result.lock().unwrap();
    let report = result.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(report.seconds.to_bits(), nan.to_bits());
    assert_eq!(report.render_fps.to_bits(), f64::NEG_INFINITY.to_bits());
    assert!(!cold.worker_available);
    assert!(cold.persistence.is_none());
}
#[test]
fn auxiliary_recovery_busy_autosave_retains_editor_while_guarded_handoff_refuses_live_jobs() {
    let mut source = Session::default();
    source.state.source_playhead = filmcraft_time::Tick(987654321);
    source.jobs.push(crate::Job { id: 1, label: "Live".into(), progress: Arc::new(filmcraft_export::Progress::default()), result: Arc::new(Mutex::new(None)) });
    assert!(native_editor::Snapshot::capture(&source).is_err());
    assert!(workspace_update::require_idle_writer_jobs(&source).is_err());
    let journal = native_editor::Snapshot::capture_journal(&source).unwrap();
    assert!(journal.auxiliary.is_none());
    assert_eq!(journal.editor.source_playhead, source.state.source_playhead);
    let raw = history_checkpoint::bytes(&journal, 80 * 1024 * 1024).unwrap();
    assert!(!serde_json::from_slice::<Value>(&raw).unwrap().as_object().unwrap().contains_key("auxiliary"));
    let mut cold = Session::default();
    journal.install(&mut cold).unwrap();
    assert!(cold.jobs.is_empty());
    assert_eq!(cold.state.source_playhead, source.state.source_playhead);
}
#[test]
fn auxiliary_recovery_legacy_payload_keeps_original_canonical_shape_and_comparison_scope() {
    let source = Session::default();
    let mut value = serde_json::to_value(native_editor::Snapshot::capture(&source).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("auxiliary");
    let raw = serde_json::to_vec(&value).unwrap();
    let old: native_editor::Snapshot = serde_json::from_slice(&raw).unwrap();
    assert!(old.auxiliary.is_none());
    assert_eq!(serde_json::to_value(&old).unwrap(), value);
    let mut changed = Session::default();
    populate(&mut changed);
    assert!(old.matches(&changed).unwrap());
}
#[test]
fn auxiliary_recovery_stopped_queue_keeps_immutable_project_and_never_starts_encoding() {
    let mut source = tests::source_clock_fixture();
    source.execute("export.queue.add", json!({"path":"/generated/stopped-export.mp4"})).unwrap();
    assert_eq!(source.export_queue.items.len(), 1);
    let queued = source.export_queue.items[0].project.clone();
    source.execute("file.newBin", json!({"name":"Later human edit"})).unwrap();
    assert_ne!(*queued, *source.project);
    assert!(workspace_update::require_idle_writer_jobs(&source).is_ok());
    let snapshot = native_auxiliary::Snapshot::capture(&source).unwrap();
    let raw = history_checkpoint::bytes(&snapshot, 80 * 1024 * 1024).unwrap();
    let decoded: native_auxiliary::Snapshot = serde_json::from_slice(&raw).unwrap();
    let mut cold = Session::default();
    decoded.install(&mut cold).unwrap();
    assert_eq!(*cold.export_queue.items[0].project, *queued);
    assert!(!cold.export_queue.running);
    assert!(!cold.export_queue.is_active());
    assert!(cold.jobs.is_empty());
    source.export_queue.running = true;
    assert!(native_auxiliary::Snapshot::capture(&source).is_err());
    assert!(native_editor::Snapshot::capture_journal(&source).unwrap().auxiliary.is_none());
}

#[test]
fn auxiliary_recovery_refuses_malformed_payload_and_display_projection_has_no_library_writer() {
    let mut source = Session::default();
    populate(&mut source);
    let projection = source.display_projection();
    assert!(projection.is_display_only());
    assert!(serde_json::to_value(projection.shortcuts.recovery()).unwrap()["directory"].is_null());
    assert!(serde_json::to_value(projection.presets.recovery()).unwrap()["directory"].is_null());
    assert!(serde_json::to_value(projection.export_presets.recovery()).unwrap()["directory"].is_null());
    assert_eq!(projection.browser.dir, source.browser.dir);
    let baseline = encoded(&source);
    for damage in ["version", "flags", "origins"] {
        let mut value = baseline.clone();
        match damage {
            "version" => value["version"] = json!(9),
            "flags" => value["effects"]["builtin"] = json!([]),
            _ => {
                value["unrecognizedAuthority"] = json!(true);
            }
        }
        match serde_json::from_value::<native_auxiliary::Snapshot>(value) {
            Ok(snapshot) => assert!(snapshot.validate().is_err()),
            Err(_) => assert_eq!(damage, "origins"),
        }
        assert_eq!(encoded(&source), baseline);
    }
    // Recovery numbers are hostile even when normal UI editing never reaches
    // them. Cache generation wraps safely; exhausted queue identities refuse.
    let mut exhausted = baseline;
    exhausted["shortcuts"]["revision"] = json!(u64::MAX);
    let snapshot: native_auxiliary::Snapshot = serde_json::from_value(exhausted).unwrap();
    let mut restored = Session::default();
    snapshot.install(&mut restored).unwrap();
    assert!(restored.shortcuts.undo());
    assert_eq!(restored.shortcuts.revision, 1);
    let mut queue_source = tests::source_clock_fixture();
    let mut value = encoded(&queue_source);
    value["queue"]["next_id"] = json!(u64::MAX);
    let snapshot: native_auxiliary::Snapshot = serde_json::from_value(value).unwrap();
    snapshot.install(&mut queue_source).unwrap();
    assert!(queue_source.execute("export.queue.add", json!({"path":"/generated/exhausted.mp4"})).is_err());
    assert!(queue_source.export_queue.items.is_empty());
}

#[test]
fn auxiliary_recovery_uses_shared_document_origins_for_current_and_historical_paths_without_mutating_source() {
    let mut source = tests::source_clock_fixture();
    source.path = Some("/generated/history/project.fcproj".into());
    let item = source.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut source.project).item_mut(item).unwrap().as_media_mut().unwrap().media = filmcraft_project::MediaRef::File { path: "current.wav".into() };
    let mut past = (*source.project).clone();
    past.item_mut(item).unwrap().as_media_mut().unwrap().media = filmcraft_project::MediaRef::File { path: "prior.wav".into() };
    source.history.undo.push(("Prior relative media".into(), Arc::new(past)));
    source.media.clear_stored();
    let graph = source.project.clone();
    let snapshot = native_auxiliary::Snapshot::capture(&source).unwrap();
    assert_eq!(source.media.path_origin("current.wav"), None);
    assert_eq!(source.media.path_origin("prior.wav"), None);
    assert!(Arc::ptr_eq(&source.project, &graph));
    let mut cold = Session::default();
    snapshot.install(&mut cold).unwrap();
    assert_eq!(cold.media.follow("current.wav").unwrap(), "/generated/history/current.wav");
    assert_eq!(cold.media.follow("prior.wav").unwrap(), "/generated/history/prior.wav");
    assert_eq!(cold.media.path_origin("prior.wav"), Some(Some(std::path::PathBuf::from("/generated/history/project.fcproj"))));
    assert!(!cold.worker_available);
}
