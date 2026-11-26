use super::super::VolumeSpec;
use super::*;
use crate::{
    Session,
    autosave::AutosaveConfig,
    project_save::{intent::Intent, operation},
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn name(c: char, extension: &str) -> String {
    format!("{}.{}", c.to_string().repeat(32), extension)
}
fn fixture() -> (Session, PathBuf, PathBuf) {
    let base = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir);
    let root = base.join(format!(
        "preview-provider-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let source = root.join("cache");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(name('a', "mov")), vec![0x41; 131075]).unwrap();
    fs::write(source.join(name('b', "wav")), b"audio preview").unwrap();
    fs::write(source.join("unindexed.keep"), b"human data").unwrap();
    let mut s = Session::default();
    s.start_autosave(AutosaveConfig::new(root.join("runtime"))).unwrap();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    s.previews.set_temp_root(Some(root.clone()));
    s.previews.set_dir(Some(source.clone()));
    s.previews.add_audio(&"c".repeat(32));
    (s, root, source)
}
fn specs(root: &Path) -> Vec<VolumeSpec> {
    vec![VolumeSpec { root: root.into(), device: fs::metadata(root).unwrap().dev().to_string(), minimum_free: 0 }]
}
fn begin(s: &mut Session, root: &Path, kind: &str) -> (operation::Key, String) {
    let intent =
        Intent::parse(&if matches!(kind, "save" | "save_all") { json!({"kind":kind}) } else { json!({"kind":kind,"path":root.join("Chosen.fcproj")}) })
            .unwrap();
    let id = format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst));
    (operation::begin(s, &id, &intent, &|| Ok(())).unwrap(), id)
}
fn target(root: &Path, kind: &str) -> PathBuf {
    root.join("FilmCraft Previews").join(if matches!(kind, "save" | "save_all") { "Original" } else { "Chosen" })
}
fn pending(s: &Session, key: &operation::Key) {
    let status = operation::status(s, key, &|| Ok(())).unwrap();
    assert_eq!(status["phase"], "AwaitingReconciliation");
    assert_eq!(status["nativeComplete"], false);
    assert_eq!(status["workersMayResume"], false);
    assert!(s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).is_err());
    assert!(s.previews.pause_writers(&|| Ok(())).is_err());
}
#[test]
fn same_original_four_save_intents_retain_documents_preview_receipts_and_real_worker_pauses_without_new_undo_or_completion() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root, source) = fixture();
        let original = fs::read(source.join(name('a', "mov"))).unwrap();
        let ino = fs::metadata(source.join(name('a', "mov"))).unwrap().ino();
        let history = s.undo_mark();
        let (key, _) = begin(&mut s, &root, kind);
        operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
        pending(&s, &key);
        let report = operation::preview_report(&s, &key).unwrap().unwrap();
        assert!(!operation::documents(&s, &key).unwrap().is_empty());
        assert_eq!(s.undo_mark(), history);
        if kind == "save_copy" {
            assert!(report.files.is_empty());
            assert!(report.directories.is_none());
            assert!(source.join(name('a', "mov")).is_file());
            assert!(!target(&root, kind).exists());
        } else {
            let moved = report.files.iter().find(|r| r.name == name('a', "mov")).unwrap();
            assert_eq!(moved.phase, Phase::Renamed);
            let p = moved.rename.as_ref().unwrap();
            assert!(p.receipt_durable);
            assert_eq!(fs::read(target(&root, kind).join(name('a', "mov"))).unwrap(), original);
            assert_eq!(fs::metadata(target(&root, kind).join(name('a', "mov"))).unwrap().ino(), ino);
            assert!(!source.join(name('a', "mov")).exists());
            assert_eq!(fs::read(p.directory.join("source-archive.bin")).unwrap(), original);
            assert!(report.directories.as_ref().unwrap().complete);
            assert!(!report.directories.as_ref().unwrap().custody);
        }
        assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"human data");
        assert!(
            operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err()
        );
        assert!(operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).is_err());
        s.shutdown();
    }
}
#[test]
fn actual_known_unmoved_rename_stage_collision_uses_original_copy_then_live_source_retirement_and_keeps_foreign_directory() {
    let (mut s, root, source) = fixture();
    let (key, id) = begin(&mut s, &root, "save_as");
    let tag = format!("{:x}", Sha256::digest(format!("{}:preview:{:?}:{}", id, Kind::Rename, name('a', "mov")).as_bytes()));
    let collision = source.join(format!(".poema-move-{tag}.incomplete"));
    fs::create_dir(&collision).unwrap();
    fs::write(collision.join("foreign.keep"), b"untouched foreign operation").unwrap();
    let ino = fs::metadata(source.join(name('a', "mov"))).unwrap().ino();
    operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    pending(&s, &key);
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    let row = report.files.iter().find(|r| r.name == name('a', "mov")).unwrap();
    assert_eq!(row.phase, Phase::Retired);
    assert!(row.rename.is_none());
    assert!(!row.errors.is_empty());
    let copy = row.copy.as_ref().unwrap();
    let retired = row.retirement.as_ref().unwrap();
    assert!(copy.receipt_durable && retired.receipt_durable);
    let dest = target(&root, "save_as").join(name('a', "mov"));
    assert_eq!(fs::read(&dest).unwrap(), vec![0x41; 131075]);
    assert_ne!(fs::metadata(&dest).unwrap().ino(), ino);
    assert_eq!(fs::read(copy.directory.join("source-archive.bin")).unwrap(), vec![0x41; 131075]);
    assert_eq!(fs::metadata(retired.directory.join("retained-source.bin")).unwrap().ino(), ino);
    assert!(!source.join(name('a', "mov")).exists());
    assert_eq!(fs::read(collision.join("foreign.keep")).unwrap(), b"untouched foreign operation");
    s.shutdown();
}
#[test]
fn preexisting_preview_file_or_directory_is_skipped_and_never_overwritten_or_charged_for_payload() {
    let (mut s, root, source) = fixture();
    let to = target(&root, "save_as");
    fs::create_dir_all(&to).unwrap();
    fs::write(to.join(name('a', "mov")), b"human destination").unwrap();
    fs::create_dir(to.join(name('b', "wav"))).unwrap();
    let (key, _) = begin(&mut s, &root, "save_as");
    operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).unwrap();
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    for c in ['a', 'b'] {
        let row = report.files.iter().find(|r| r.name == name(c, if c == 'a' { "mov" } else { "wav" })).unwrap();
        assert_eq!(row.phase, Phase::Skipped);
        assert!(row.rename.is_none() && row.copy.is_none() && row.retirement.is_none());
    }
    assert_eq!(fs::read(to.join(name('a', "mov"))).unwrap(), b"human destination");
    assert!(to.join(name('b', "wav")).is_dir());
    assert!(source.join(name('a', "mov")).exists());
    pending(&s, &key);
    s.shutdown();
}
#[test]
fn late_source_or_destination_changes_refuse_callbacks_and_preserve_later_human_bytes() {
    for changed_source in [true, false] {
        let (mut s, root, source) = fixture();
        let (key, id) = begin(&mut s, &root, "save_as");
        let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
        let mut original = crate::project_save::effects::Original;
        let mut provider = Provider::new(plan, &id, &mut original, &|| Ok(())).unwrap();
        let to = target(&root, "save_as");
        provider.directories(DirectoryRole::Previews, &to).unwrap();
        let path = if changed_source { source.join(name('a', "mov")) } else { to.join(name('a', "mov")) };
        fs::write(&path, b"later human bytes").unwrap();
        assert!(provider.preview_rename(&source.join(name('a', "mov")), &to.join(name('a', "mov"))).is_err());
        assert!(provider.preview_copy(&source.join(name('a', "mov")), &to.join(name('a', "mov"))).is_err());
        let report = provider.into_report();
        let row = report.files.iter().find(|r| r.name == name('a', "mov")).unwrap();
        assert_eq!(row.phase, Phase::Refused);
        assert!(row.rename.is_none() && row.copy.is_none());
        assert_eq!(fs::read(path).unwrap(), b"later human bytes");
        operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
        s.shutdown();
    }
}
#[test]
fn Stop_after_actual_rename_retains_uncertain_receipt_original_data_workers_and_native_owner_without_fallback() {
    let (mut s, root, source) = fixture();
    let from = source.join(name('a', "mov"));
    let to = target(&root, "save_as").join(name('a', "mov"));
    let (key, _) = begin(&mut s, &root, "save_as");
    let stop = || if !from.exists() && to.exists() { Err(error("generated Stop after actual native preview move")) } else { Ok(()) };
    operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &stop, &|| Ok(())).unwrap();
    pending(&s, &key);
    let report = operation::preview_report(&s, &key).unwrap().unwrap();
    let row = report.files.iter().find(|r| r.name == name('a', "mov")).unwrap();
    assert_eq!(row.phase, Phase::Refused);
    let p = row.rename.as_ref().unwrap();
    assert!(row.copy.is_none() && row.retirement.is_none());
    assert_eq!(fs::read(&to).unwrap(), vec![0x41; 131075]);
    assert_eq!(fs::read(p.directory.join("source-archive.bin")).unwrap(), vec![0x41; 131075]);
    assert!(!from.exists());
    assert!(p.receipt_durable);
    assert!(p.receipt.attempted);
    assert_eq!(p.receipt.outcome, relocation::Outcome::Uncertain);
    assert!(p.receipt.error.as_deref().is_some_and(|e| e.contains("generated Stop after actual native preview move")));
    s.shutdown();
}
#[test]
fn wrong_paths_and_repeated_callbacks_cannot_replay_or_remove_a_renamed_source() {
    let (mut s, root, source) = fixture();
    let (key, id) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new(plan, &id, &mut original, &|| Ok(())).unwrap();
    let to = target(&root, "save_as");
    p.directories(DirectoryRole::Previews, &to).unwrap();
    assert!(p.directories(DirectoryRole::Previews, &to).is_err());
    let from = source.join(name('a', "mov"));
    let dest = to.join(name('a', "mov"));
    assert!(p.preview_rename(&source.join("unindexed.keep"), &to.join("unindexed.keep")).is_err());
    assert!(p.preview_rename(&from, &root.join(name('a', "mov"))).is_err());
    p.preview_rename(&from, &dest).unwrap();
    assert!(p.preview_rename(&from, &dest).is_err());
    assert!(p.preview_copy(&from, &dest).is_err());
    assert!(p.preview_remove(&from).is_err());
    assert_eq!(fs::read(dest).unwrap(), vec![0x41; 131075]);
    drop(p);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn aliased_catalog_prerefusal_happens_before_document_or_preview_effects_and_preserves_both_names() {
    let (mut s, root, source) = fixture();
    fs::hard_link(source.join(name('b', "wav")), source.join(name('d', "wav"))).unwrap();
    s.previews.add_audio(&"d".repeat(32));
    let (key, _) = begin(&mut s, &root, "save_as");
    assert!(operation::dispatch_preserving_previews(&mut s, &key, &specs(&root), &mut crate::project_save::effects::Original, &|| Ok(()), &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(!target(&root, "save_as").exists());
    assert!(operation::preview_report(&s, &key).unwrap().is_none());
    assert_eq!(fs::read(source.join(name('d', "wav"))).unwrap(), b"audio preview");
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn preview_credit_roles_sum_to_complete_declared_demand_and_each_role_can_be_assigned_only_once() {
    let (mut s, root, _) = fixture();
    let (key, _) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut ledger = Ledger::new(&plan).unwrap();
    let initial = ledger.remaining.clone();
    for demand in &plan.demands {
        assert_eq!(initial[&demand.device], demand.required_bytes);
    }
    let roles = ledger.charges.keys().cloned().collect::<Vec<_>>();
    for role in roles {
        ledger.enter(role.clone()).unwrap();
        assert!(ledger.enter(role).is_err());
    }
    assert!(ledger.remaining.values().all(|v| *v == 0));
    assert!(add(u64::MAX, 1).is_err());
    assert!(mul(u64::MAX, 2).is_err());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn canceled_native_pause_invalidates_existing_provider_before_any_preview_effect() {
    let (mut s, root, source) = fixture();
    let (key, id) = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let mut original = crate::project_save::effects::Original;
    let mut p = Provider::new(plan, &id, &mut original, &|| Ok(())).unwrap();
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    assert!(p.directories(DirectoryRole::Previews, &target(&root, "save_as")).is_err());
    assert!(p.preview_rename(&source.join(name('a', "mov")), &target(&root, "save_as").join(name('a', "mov"))).is_err());
    assert!(!target(&root, "save_as").exists());
    assert!(source.join(name('a', "mov")).exists());
    drop(p);
    s.shutdown();
}
