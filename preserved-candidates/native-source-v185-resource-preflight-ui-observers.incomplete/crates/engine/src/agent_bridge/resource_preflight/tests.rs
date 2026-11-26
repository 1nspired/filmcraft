use super::*;
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};
fn world_without_transport() -> Session {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = crate::temp_dir().join(format!("poema-resource-session-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let mut s = Session::default();
    s.start_autosave(crate::autosave::AutosaveConfig::new(root)).unwrap();
    s
}
fn world() -> Session {
    let mut s = world_without_transport();
    s.agent_operation_control().enable_transport();
    s
}
fn request(s: &mut Session, letter: char) -> Value {
    let owner = s.execute(OWNER, json!({})).unwrap();
    json!({"request":letter.to_string().repeat(64),"nativePid":owner["nativePid"],"nativeSession":owner["nativeSession"],"dataDirectory":owner["dataDirectory"]})
}
fn begin_held(s: &mut Session, r: &Value) {
    let v = s.execute(BEGIN, r.clone()).unwrap();
    assert_eq!(v["status"], "held");
    assert_eq!(v["namespaceHeld"], true);
    assert_eq!(v["writeAuthority"], false);
    assert_eq!(v["nativeStartupAuthority"], false);
}
fn released(s: &mut Session, r: &Value) {
    let v = s.execute(RELEASE, r.clone()).unwrap();
    assert_eq!(v["status"], "released");
    assert_eq!(v["namespaceHeld"], false);
}

#[test]
fn original_resource_session_owns_real_worker_preview_and_command_fences_until_its_exact_release() {
    let mut s = world();
    let r = request(&mut s, 'a');
    let before = s.revision;
    begin_held(&mut s, &r);
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
    assert!(s.is_display_only());
    assert!(s.ensure_desktop_startup_transfer().is_err());
    for id in ["file.newSequence", "prefs.set", "history.undo", "render.inToOut"] {
        assert!(s.execute(id, json!({})).is_err());
    }
    s.poll_persistence();
    assert_eq!(s.revision, before);
    released(&mut s, &r);
    assert!(!s.is_display_only());
    s.ensure_desktop_startup_transfer().unwrap();
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    drop(s.previews.pause_writers(&|| Ok(())).unwrap());
}
#[test]
fn lost_resource_begin_reply_reconciles_same_hold_without_another_pause_or_a_new_request() {
    let mut s = world();
    let r = request(&mut s, 'b');
    drop(s.execute(BEGIN, r.clone()).unwrap());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "held");
    begin_held(&mut s, &r);
    let mut other = r.clone();
    other["request"] = json!("c".repeat(64));
    assert!(s.execute(BEGIN, other).is_err());
    begin_held(&mut s, &r);
    released(&mut s, &r);
    assert!(s.execute(BEGIN, r).is_err());
}
#[test]
fn lost_resource_release_reply_and_later_duplicate_release_cannot_resume_a_new_save_owner() {
    let mut s = world();
    let r = request(&mut s, 'd');
    begin_held(&mut s, &r);
    drop(s.execute(RELEASE, r.clone()).unwrap());
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["status"], "released");
    let later = s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap();
    released(&mut s, &r);
    later.current(s.persistence.as_ref().unwrap()).unwrap();
    later.finish(|| Ok(())).unwrap();
}
#[test]
fn original_out_of_band_cancel_revokes_metadata_readiness_but_preserves_exact_owner_cleanup() {
    let mut s = world();
    let r = request(&mut s, 'e');
    begin_held(&mut s, &r);
    let stopped = s.agent_operation_control().cancel(&json!({"token":r["request"]})).unwrap();
    assert_eq!(stopped["cancellationAccepted"], true);
    let v = s.execute(STATUS, r.clone()).unwrap();
    assert_eq!(v["status"], "stale_held");
    assert_eq!(v["namespaceHeld"], false);
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    released(&mut s, &r);
    assert!(!s.is_display_only());
}
#[test]
fn resource_status_and_release_refuse_crossed_native_epochs_pids_roots_and_nonce_without_releasing_owner() {
    let mut s = world();
    let r = request(&mut s, 'f');
    begin_held(&mut s, &r);
    for (key, value) in [
        ("nativeSession", json!("x".repeat(32))),
        ("nativePid", json!(0)),
        ("dataDirectory", json!("/wrong/generated/root")),
        ("request", json!("g".repeat(64))),
    ] {
        let mut bad = r.clone();
        bad[key] = value;
        assert!(s.execute(STATUS, bad.clone()).is_err());
        assert!(s.execute(RELEASE, bad).is_err());
        begin_held(&mut s, &r);
    }
    released(&mut s, &r);
}
#[test]
fn hostile_resource_request_shapes_fail_before_ownership_or_source_writer_effects() {
    let mut s = world();
    let r = request(&mut s, 'h');
    let mut bad = Vec::new();
    for (key, value) in [
        ("nativePid", json!(-1)),
        ("nativePid", json!(1.5)),
        ("nativePid", json!(u64::MAX)),
        ("request", json!("")),
        ("request", json!("q".repeat(129))),
        ("dataDirectory", json!("relative")),
        ("dataDirectory", json!("/wrong\nroot")),
        ("nativeSession", Value::Null),
    ] {
        let mut row = r.clone();
        row[key] = value;
        bad.push(row);
    }
    let mut extra = r.clone();
    extra["authority"] = json!(true);
    bad.push(extra);
    let mut missing = r.clone();
    missing.as_object_mut().unwrap().remove("request");
    bad.push(missing);
    for row in bad {
        assert!(s.execute(BEGIN, row).is_err());
        assert!(!s.is_display_only());
        s.persistence.as_ref().unwrap().pause_resource_preflight(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    }
    begin_held(&mut s, &r);
    released(&mut s, &r);
}
#[test]
fn resource_hold_detects_preview_worker_replacement_and_never_releases_replacement_as_its_owner() {
    let mut s = world();
    let r = request(&mut s, 'i');
    begin_held(&mut s, &r);
    let original = s.previews.clone();
    let other = Session::default();
    s.previews = other.previews.clone();
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["namespaceHeld"], false);
    assert!(s.execute(RELEASE, r.clone()).is_err());
    drop(s.previews.pause_writers(&|| Ok(())).unwrap());
    s.previews = original;
    released(&mut s, &r);
}
#[test]
fn resource_hold_detects_directory_redirection_and_retains_original_physical_ownership() {
    let mut s = world();
    let r = request(&mut s, 'j');
    begin_held(&mut s, &r);
    let root = s.persistence.as_ref().unwrap().data_dir.clone();
    let old = root.with_extension("held-original");
    let later = root.with_extension("held-later");
    fs::rename(&root, &old).unwrap();
    fs::create_dir(&root).unwrap();
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["namespaceHeld"], false);
    assert!(s.execute(RELEASE, r.clone()).is_err());
    fs::rename(&root, later).unwrap();
    fs::rename(old, root).unwrap();
    released(&mut s, &r);
}
#[test]
fn resource_hold_detects_later_human_basis_change_without_rebasing_and_release_preserves_that_work() {
    let mut s = world();
    let r = request(&mut s, 'k');
    begin_held(&mut s, &r);
    s.revision += 1;
    let later = s.revision;
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["namespaceHeld"], false);
    released(&mut s, &r);
    assert_eq!(s.revision, later);
}
#[test]
fn resource_hold_requires_cancellation_transport_and_refuses_unrelated_existing_pause_owners() {
    let mut s = Session::default();
    let info = s.execute(OWNER, json!({})).unwrap();
    assert_eq!(info["available"], false);
    assert!(s
        .execute(BEGIN, json!({"request":"l".repeat(64),"nativePid":std::process::id(),"nativeSession":info["nativeSession"],"dataDirectory":"/missing"}))
        .is_err());
    s = world_without_transport();
    let unready = request(&mut s, 'n');
    assert!(s.execute(BEGIN, unready).is_err());
    assert!(!s.is_display_only());
    s = world();
    let r = request(&mut s, 'm');
    let original = s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap();
    assert!(s.execute(BEGIN, r.clone()).is_err());
    original.current(s.persistence.as_ref().unwrap()).unwrap();
    assert_eq!(s.execute(STATUS, r.clone()).unwrap()["namespaceHeld"], false);
    original.finish(|| Ok(())).unwrap();
    assert!(s.execute(BEGIN, r).is_err());
}

#[test]
fn direct_preferences_writer_preserves_memory_and_bytes_until_same_resource_hold_releases() {
    let mut s = world();
    let r = request(&mut s, 'o');
    let file = s.prefs_path.clone().unwrap();
    s.set_prefs(s.prefs.clone()).unwrap();
    let before = fs::read(&file).unwrap();
    let memory = serde_json::to_value(&s.prefs).unwrap();
    let mut later = s.prefs.clone();
    later.auto_save.max_versions = later.auto_save.max_versions.saturating_add(1);
    assert_ne!(serde_json::to_value(&later).unwrap(), memory);
    begin_held(&mut s, &r);
    assert!(s.set_prefs(later.clone()).is_err());
    assert!(s.set_prefs_with(later.clone(), &mut crate::project_save::effects::Original).is_err());
    assert_eq!(fs::read(&file).unwrap(), before);
    assert_eq!(serde_json::to_value(&s.prefs).unwrap(), memory);
    begin_held(&mut s, &r);
    released(&mut s, &r);
    s.set_prefs(later.clone()).unwrap();
    assert_eq!(serde_json::to_value(&s.prefs).unwrap(), serde_json::to_value(later).unwrap());
    assert_ne!(fs::read(file).unwrap(), before);
}
#[test]
fn direct_preview_relocation_keeps_original_files_and_directory_until_same_resource_hold_releases() {
    let mut s = world();
    let root = s.persistence.as_ref().unwrap().data_dir.clone();
    let old = root.join("generated-preview-before");
    fs::create_dir(&old).unwrap();
    let name = format!("{}.wav", "a".repeat(32));
    let original = old.join(&name);
    fs::write(&original, b"GENERATED_PREVIEW_LOCATION_SENTINEL").unwrap();
    s.previews.set_dir(Some(old.clone()));
    s.path = Some(root.join("generated-project.fcproj").to_str().unwrap().to_string());
    let destination = crate::previews::dir_for_project(s.path.as_deref().unwrap());
    assert!(!destination.exists());
    let r = request(&mut s, 'p');
    begin_held(&mut s, &r);
    s.previews_follow_path();
    s.previews_follow_path_with(&mut crate::project_save::effects::Original);
    assert_eq!(s.previews.dir(), Some(old));
    assert_eq!(fs::read(&original).unwrap(), b"GENERATED_PREVIEW_LOCATION_SENTINEL");
    assert!(!destination.exists());
    begin_held(&mut s, &r);
    released(&mut s, &r);
    s.previews_follow_path();
    assert_eq!(s.previews.dir(), Some(destination.clone()));
    assert_eq!(fs::read(destination.join(name)).unwrap(), b"GENERATED_PREVIEW_LOCATION_SENTINEL");
    assert!(!original.exists());
}
