use super::{project_plane, project_save_plan, tests::token};
use crate::{Session, project_save};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn scope() -> Value {
    json!({"projectId":"p_save_plan","workspaceId":"44444444-4444-4444-8444-444444444444"})
}
fn root(preserve: bool) -> (Session, PathBuf) {
    let dir = crate::temp_dir().join(format!("fc-save-plan-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&dir).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"The editor's title","workspace":scope(),"preserveDisplayTitle":preserve})).unwrap();
    s.previews.set_dir(None);
    s.execute("file.saveAs", json!({"path":dir.join("Root.fcproj")})).unwrap();
    (s, dir)
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"path":s.path,"revision":s.revision,"savedRevision":s.saved_revision,"savedFile":s.saved_file,"prefs":s.prefs,"previews":s.previews.dir(),"undo":s.history.undo,"redo":s.history.redo})
}
fn read(s: &mut Session) {
    s.execute(project_plane::READ, json!({"token":token(99100),"scope":scope(),"projectPath":s.path})).unwrap();
}
fn args(s: &Session, intent: Value) -> Value {
    json!({"token":token(99100),"scope":scope(),"beforeProjectSha256":project_plane::basis(s).unwrap(),"intent":intent})
}
fn query(s: &mut Session, intent: Value) -> Value {
    let p = args(s, intent);
    s.execute(project_save_plan::PLAN, p).unwrap()
}
#[test]
fn native_save_plan_predicts_original_human_complete_file_title_saved_state_schema_preferences_and_preview_effects_for_all_variants() {
    let mut cases = 0;
    for preserve in [false, true] {
        for dirty in [false, true] {
            for legacy in [false, true] {
                for proxy_mismatch in [false, true] {
                    for variant in 0..7 {
                        let (mut s, dir) = root(preserve);
                        if dirty {
                            s.execute("file.newBin", json!({"name":"Unsaved human bin"})).unwrap();
                        }
                        if legacy {
                            s.loaded_schema = 1;
                        }
                        if proxy_mismatch {
                            s.media.set_use_proxies(!s.prefs.media.enable_proxies);
                        }
                        let original = s.path.clone().unwrap();
                        let next = dir.join("New storage name.fcproj").to_string_lossy().into_owned();
                        let request = match variant {
                            0 => json!({"kind":"save"}),
                            1 => json!({"kind":"save","path":next}),
                            2 => json!({"kind":"save_as","path":original}),
                            3 => json!({"kind":"save_as","path":next}),
                            4 => json!({"kind":"save_copy","path":original}),
                            5 => json!({"kind":"save_copy","path":next}),
                            _ => json!({"kind":"save_all"}),
                        };
                        read(&mut s);
                        let before = state(&s);
                        let root_bytes = std::fs::read(&original).unwrap();
                        let history = s.history.clone();
                        let plan = query(&mut s, request);
                        assert_eq!(plan["status"], "planned");
                        assert_eq!(plan["writeAuthority"], false);
                        assert_eq!(plan["fileCustody"], false);
                        assert_eq!(plan["save"]["overwriteAuthority"], false);
                        assert_eq!(plan["save"]["output"]["targetObserved"], false);
                        assert_eq!(state(&s), before);
                        assert_eq!(std::fs::read(&original).unwrap(), root_bytes);
                        assert!(!std::path::Path::new(&next).exists());
                        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "planning created output or auxiliary paths");
                        s.execute(project_plane::VALIDATE, json!({"token":token(99100),"scope":scope()})).unwrap();
                        let command = plan["resolution"]["humanCommand"].as_str().unwrap();
                        let path = plan["save"]["output"]["path"].as_str().unwrap().to_string();
                        let raw = if command == "file.saveAll" { json!({}) } else { json!({"path":path}) };
                        s.execute(command, raw).unwrap();
                        let output = std::fs::read(&path).unwrap();
                        let description = &plan["save"];
                        assert_eq!(description["output"]["sha256"], crate::saved_file::sha256(&output));
                        assert_eq!(description["output"]["bytes"], output.len());
                        assert_eq!(description["document"]["titleAfter"], s.project.name);
                        assert_eq!(description["document"]["rootTitleAfter"], s.project.root.name);
                        assert_eq!(description["document"]["pathAfter"], json!(s.path));
                        assert_eq!(description["document"]["revisionAfter"], s.revision);
                        assert_eq!(description["document"]["savedRevisionAfter"], s.saved_revision);
                        assert_eq!(description["document"]["dirtyAfter"], s.is_dirty());
                        let baseline = description["document"]["savedBaselineAfter"].as_str().unwrap();
                        if baseline == "clear" {
                            assert!(s.saved_file.is_none());
                        } else if baseline == "replace_with_written_bytes" {
                            assert_eq!(s.saved_file.as_ref().unwrap().sha256, description["output"]["sha256"].as_str().unwrap());
                        }
                        if !description["effects"]["schemaBackupCandidate"].is_null() {
                            let backup = description["effects"]["schemaBackupCandidate"].as_str().unwrap();
                            assert_eq!(std::fs::read(backup).unwrap(), root_bytes);
                        }
                        if !description["effects"]["previews"]["to"].is_null() {
                            assert_eq!(description["effects"]["previews"]["to"], json!(s.previews.dir()));
                        }
                        if description["effects"]["preferences"]["updatesMemory"] == true {
                            assert_eq!(s.prefs.general.recent_projects.first(), Some(&path));
                        }
                        assert_eq!(s.history.undo.len(), history.undo.len());
                        assert_eq!(s.history.redo.len(), history.redo.len());
                        for (a, z) in s.history.undo.iter().chain(&s.history.redo).zip(history.undo.iter().chain(&history.redo)) {
                            assert_eq!(a.0, z.0);
                            assert!(Arc::ptr_eq(&a.1, &z.1));
                        }
                        cases += 1;
                        std::fs::remove_dir_all(dir).unwrap();
                    }
                }
            }
        }
    }
    assert_eq!(cases, 112);
}
#[test]
fn native_save_plan_destination_requests_and_relative_paths_never_invent_a_file_grant_or_consume_the_owning_root() {
    let (mut s, dir) = root(false);
    read(&mut s);
    let before = state(&s);
    for kind in ["save_as", "save_copy"] {
        let p = query(&mut s, json!({"kind":kind}));
        assert_eq!(p["status"], "needs_destination");
        assert_eq!(p["resolution"]["nativeDialog"], true);
        assert!(p["save"].is_null());
        assert_eq!(p["writeAuthority"], false);
    }
    let relative = "Unselected relative save copy.fcproj";
    let p = query(&mut s, json!({"kind":"save_copy","path":relative}));
    assert_eq!(p["resolution"]["relativePathRequiresCurrentDirectoryAdmission"], true);
    assert_eq!(p["save"]["output"]["path"], relative);
    assert_eq!(state(&s), before);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    s.execute(project_plane::VALIDATE, json!({"token":token(99100),"scope":scope()})).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn native_save_plan_refuses_foreign_stale_cancelled_malformed_and_raw_authority_queries_without_publishing_or_narrowing_intent() {
    let (mut s, dir) = root(false);
    read(&mut s);
    let base = args(&s, json!({"kind":"save"}));
    let before = state(&s);
    let mut wrong_scope = base.clone();
    wrong_scope["scope"]["projectId"] = json!("foreign");
    let mut wrong_basis = base.clone();
    wrong_basis["beforeProjectSha256"] = json!("0".repeat(64));
    let mut extra = base.clone();
    extra["grant"] = json!(true);
    let mut wrong_token = base.clone();
    wrong_token["token"] = json!(token(99101));
    for p in [wrong_scope, wrong_basis, extra, wrong_token] {
        assert!(s.execute(project_save_plan::PLAN, p).is_err());
        assert_eq!(state(&s), before);
    }
    for intent in
        [json!({"kind":"save","path":null}), json!({"kind":"save_all","path":dir.join("Do not write.fcproj")}), json!({"kind":"execute","command":"file.save"})]
    {
        let p = args(&s, intent);
        assert!(s.execute(project_save_plan::PLAN, p).is_err());
        assert_eq!(state(&s), before);
    }
    s.execute("agent.cancelSnapshot", json!({"job":token(99100)})).unwrap();
    assert!(s.execute(project_save_plan::PLAN, base.clone()).is_err());
    s.execute(project_plane::READ, json!({"token":token(99102),"scope":scope(),"projectPath":s.path})).unwrap();
    let mut fresh = base;
    fresh["token"] = json!(token(99102));
    s.execute("file.newBin", json!({"name":"Later human data"})).unwrap();
    let later = state(&s);
    assert!(s.execute(project_save_plan::PLAN, fresh).is_err());
    assert_eq!(state(&s), later);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn native_save_description_uses_original_trusted_scratch_and_discloses_preferences_backup_and_unobserved_auxiliary_files() {
    let (mut s, dir) = root(false);
    let scratch = dir.join("Chosen scratch");
    s.prefs.auto_save.scratch_folders.push(scratch.to_string_lossy().into_owned());
    Arc::make_mut(&mut s.project).settings.scratch.video_previews = Some(scratch.to_string_lossy().into_owned());
    s.prefs_path = Some(dir.join("private prefs/preferences.json"));
    s.loaded_schema = 1;
    read(&mut s);
    let dest = dir.join("New title.fcproj");
    let plan = query(&mut s, json!({"kind":"save_as","path":dest}));
    let effect = &plan["save"]["effects"];
    assert_eq!(effect["previews"]["to"], json!(scratch.join("New title")));
    assert_eq!(effect["previews"]["fileCatalogObserved"], false);
    assert_eq!(effect["preferences"]["fileCandidate"], json!(s.prefs_path));
    assert_eq!(effect["destinationChecksRequired"], true);
    assert_eq!(effect["auxiliaryChecksRequired"], true);
    assert!(!scratch.exists());
    assert!(!dir.join("private prefs").exists());
    assert!(!dest.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn original_prepared_save_refuses_later_preview_catalog_and_media_proxy_owner_changes_before_file_access() {
    for change in 0..3 {
        let (mut s, dir) = root(false);
        let target = dir.join("Must remain absent.fcproj");
        let prepared = project_save::prepare(&s, target.to_str().unwrap(), true).unwrap();
        match change {
            0 => {
                s.previews.generation.fetch_add(1, Ordering::Relaxed);
            }
            1 => s.media = Arc::new(crate::MediaPool::default()),
            _ => s.media.set_use_proxies(!s.media.use_proxies()),
        };
        let later = state(&s);
        assert!(prepared.publish(&mut s).is_err());
        assert_eq!(state(&s), later);
        assert!(!target.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn human_save_refuses_revision_exhaustion_before_writing_when_preference_proxy_sync_would_overflow() {
    let (mut s, dir) = root(false);
    s.revision = u64::MAX;
    s.media.set_use_proxies(!s.prefs.media.enable_proxies);
    let target = dir.join("Must not be partially saved.fcproj");
    let before = state(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("file.saveAs", json!({"path":target}))));
    let error = result.unwrap().unwrap_err().to_string();
    assert!(error.contains("revision space exhausted"), "{error}");
    assert_eq!(state(&s), before);
    assert!(!target.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
