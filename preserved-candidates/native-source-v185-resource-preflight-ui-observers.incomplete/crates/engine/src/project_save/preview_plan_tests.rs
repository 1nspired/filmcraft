use super::*;
use crate::autosave::AutosaveConfig;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn id() -> String {
    format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst))
}
fn fixture() -> (Session, PathBuf, PathBuf) {
    let base = std::env::var_os("POEMA_MANAGED_RUN").map(PathBuf::from).unwrap_or_else(crate::temp_dir);
    let root = base.join(format!(
        "native-preview-plan-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let source = root.join("cache");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(format!("{}.mov", "a".repeat(32))), vec![0x41; 131075]).unwrap();
    fs::write(source.join(format!("{}.wav", "b".repeat(32))), b"audio preview").unwrap();
    fs::write(source.join("unindexed.keep"), b"unindexed human data").unwrap();
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
fn begin(s: &mut Session, root: &Path, kind: &str) -> Key {
    let intent = super::super::intent::Intent::parse(&if matches!(kind, "save" | "save_all") {
        json!({"kind":kind})
    } else {
        json!({"kind":kind,"path":root.join("Chosen.fcproj")})
    })
    .unwrap();
    operation::begin(s, &id(), &intent, &|| Ok(())).unwrap()
}
#[test]
fn native_preview_preflight_captures_complete_real_paused_catalog_stale_entries_exact_bytes_and_source_plus_two_destination_payloads_without_writes() {
    let (mut s, root, source) = fixture();
    let key = begin(&mut s, &root, "save_as");
    let before = fs::read_dir(&source).unwrap().map(|v| v.unwrap().file_name()).collect::<Vec<_>>();
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let view = plan.description();
    assert_eq!(view["catalogEntries"], 3);
    assert_eq!(view["observedEntries"], 3);
    assert_eq!(view["mayMoveTemporaryFiles"], true);
    assert_eq!(
        plan.entries.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
        vec![format!("{}.mov", "a".repeat(32)), format!("{}.wav", "b".repeat(32)), format!("{}.wav", "c".repeat(32))]
    );
    assert_eq!(plan.demands.len(), 1);
    let demand = &plan.demands[0];
    assert!(demand.source_archive_bytes >= 131075 + 13);
    assert_eq!(demand.destination_payload_bytes, 2 * demand.source_archive_bytes);
    assert!(demand.required_bytes > demand.source_archive_bytes + demand.destination_payload_bytes);
    assert!(!root.join("FilmCraft Previews/Chosen").exists());
    assert!(!root.join("Chosen.fcproj").exists());
    assert_eq!(fs::read(source.join("unindexed.keep")).unwrap(), b"unindexed human data");
    assert_eq!(fs::read_dir(&source).unwrap().map(|v| v.unwrap().file_name()).collect::<Vec<_>>(), before);
    plan.current(&s, &key, &|| Ok(())).unwrap();
    assert_eq!(view["spaceReserved"], false);
    assert_eq!(view["writeAuthority"], false);
    assert_eq!(view["mediaEvidence"], false);
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn native_preview_preflight_detects_same_length_later_source_bytes_and_original_index_generation_without_rolling_back_human_changes() {
    for index in [false, true] {
        let (mut s, root, source) = fixture();
        let key = begin(&mut s, &root, "save_as");
        let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
        if index {
            s.previews.add_audio(&"d".repeat(32));
        } else {
            fs::write(source.join(format!("{}.wav", "b".repeat(32))), b"later preview").unwrap();
        }
        assert!(plan.current(&s, &key, &|| Ok(())).is_err());
        assert!(!root.join("Chosen.fcproj").exists());
        if !index {
            assert_eq!(fs::read(source.join(format!("{}.wav", "b".repeat(32)))).unwrap(), b"later preview");
            operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
        }
        s.shutdown();
    }
}
#[test]
fn native_preview_preflight_existing_target_skips_its_payload_budget_and_later_missing_parent_creation_is_refused_without_adoption() {
    let (mut s, root, source) = fixture();
    let target = root.join("FilmCraft Previews/Chosen");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(format!("{}.mov", "a".repeat(32))), b"existing destination stays").unwrap();
    let key = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    assert_eq!(plan.demands[0].source_archive_bytes, allocation(13, plan.volumes[0].before.allocation_unit).unwrap());
    assert_eq!(fs::read(target.join(format!("{}.mov", "a".repeat(32)))).unwrap(), b"existing destination stays");
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
    let (mut s, root, _) = fixture();
    let key = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let target = root.join("FilmCraft Previews/Chosen");
    fs::create_dir(&target).unwrap();
    assert!(plan.current(&s, &key, &|| Ok(())).is_err());
    assert!(source.exists());
    assert!(!root.join("Chosen.fcproj").exists());
    s.shutdown();
}
#[test]
fn native_preview_preflight_aggregates_same_device_roots_once_and_refuses_wrong_missing_or_insufficient_storage_context_before_any_publication() {
    let (mut s, root, source) = fixture();
    let key = begin(&mut s, &root, "save_as");
    let mut roots = specs(&root);
    let mut nested = roots[0].clone();
    nested.root = source;
    nested.minimum_free = 123456;
    roots.push(nested);
    let plan = Plan::capture(&s, &key, &roots, &|| Ok(())).unwrap();
    assert_eq!(plan.demands.len(), 1);
    assert_eq!(plan.demands[0].minimum_free, 123456);
    let mut bad = specs(&root);
    bad[0].device = "wrong-device".into();
    assert!(Plan::capture(&s, &key, &bad, &|| Ok(())).is_err());
    assert!(Plan::capture(&s, &key, &[], &|| Ok(())).is_err());
    let mut full = specs(&root);
    full[0].minimum_free = u64::MAX;
    assert!(Plan::capture(&s, &key, &full, &|| Ok(())).is_err());
    let duplicate = vec![roots[0].clone(), roots[0].clone()];
    assert!(Plan::capture(&s, &key, &duplicate, &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(!root.join("FilmCraft Previews/Chosen").exists());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn native_preview_preflight_requires_same_original_live_save_and_producer_pause_and_rejects_escaped_or_malformed_catalog_names() {
    let (mut s, root, _) = fixture();
    let other = crate::previews::PreviewStore::default();
    let wrong = other.pause_writers(&|| Ok(())).unwrap();
    assert!(s.previews.save_catalog(&wrong).is_err());
    drop(wrong);
    let key = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let (mut b, br, _) = fixture();
    let kb = begin(&mut b, &br, "save_as");
    assert!(plan.current(&b, &kb, &|| Ok(())).is_err());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    assert!(plan.current(&s, &key, &|| Ok(())).is_err());
    operation::cancel_before_dispatch(&mut b, &kb, &|| Ok(())).unwrap();
    s.shutdown();
    b.shutdown();
    let (mut s, root, _) = fixture();
    s.previews.add_audio("../../escape");
    let sentinel = root.join("escape.wav");
    fs::write(&sentinel, b"human sentinel").unwrap();
    let key = begin(&mut s, &root, "save_as");
    assert!(Plan::capture(&s, &key, &specs(&root), &|| Ok(())).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"human sentinel");
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn native_preview_preflight_preserves_original_copy_non_temporary_and_same_directory_policy_with_zero_migration_payload_or_creation_budget() {
    for mode in ["copy", "not-temporary", "same"] {
        let (mut s, root, _) = fixture();
        let kind = if mode == "copy" { "save_copy" } else { "save_as" };
        if mode == "not-temporary" {
            s.previews.set_temp_root(None);
            assert!(!s.previews.dir().unwrap().starts_with(crate::temp_dir()));
        }
        if mode == "same" {
            let target = root.join("FilmCraft Previews/Chosen");
            fs::create_dir_all(&target).unwrap();
            s.previews.set_dir(Some(target));
        }
        let key = begin(&mut s, &root, kind);
        let selected = if mode == "copy" { Vec::new() } else { specs(&root) };
        let plan = Plan::capture(&s, &key, &selected, &|| Ok(())).unwrap();
        assert!(!plan.may_move);
        assert!(plan.entries.is_empty());
        assert!(plan.demands.iter().all(|v| v.required_bytes == 0));
        assert!(!root.join("Chosen.fcproj").exists());
        operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
        s.shutdown();
    }
}
struct Cwd(PathBuf);
impl Drop for Cwd {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).unwrap();
    }
}
#[test]
fn native_preview_preflight_uses_actual_native_cwd_for_relative_paths_and_refuses_a_changed_cwd_without_host_guess_or_internal_fallback() {
    let (mut s, root, _) = fixture();
    let old = std::env::current_dir().unwrap();
    let _cwd = Cwd(old);
    std::env::set_current_dir(&root).unwrap();
    s.previews.set_temp_root(Some(PathBuf::from("cache")));
    s.previews.set_dir(Some(PathBuf::from("cache")));
    let intent = super::super::intent::Intent::parse(&json!({"kind":"save_as","path":"Chosen.fcproj"})).unwrap();
    let key = operation::begin(&mut s, &id(), &intent, &|| Ok(())).unwrap();
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    assert_eq!(plan.cwd, root);
    assert_eq!(plan.source.as_ref().unwrap().record().path, root.join("cache"));
    fs::create_dir(root.join("other")).unwrap();
    std::env::set_current_dir(root.join("other")).unwrap();
    assert!(plan.current(&s, &key, &|| Ok(())).is_err());
    assert!(!root.join("Chosen.fcproj").exists());
    assert!(!root.join("other/Chosen.fcproj").exists());
    std::env::set_current_dir(&root).unwrap();
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
#[test]
fn native_preview_preflight_final_file_fence_after_budget_and_native_postflight_refuses_late_earlier_file_mutation_without_any_effect() {
    let (mut s, root, source) = fixture();
    let key = begin(&mut s, &root, "save_as");
    let plan = Plan::capture(&s, &key, &specs(&root), &|| Ok(())).unwrap();
    let target = source.join(format!("{}.mov", "a".repeat(32)));
    assert!(plan.current_with(&s, &key, &|| Ok(()), &mut || fs::write(&target, b"later source after complete scans and native postflight")).is_err());
    assert_eq!(fs::read(target).unwrap(), b"later source after complete scans and native postflight");
    assert!(!root.join("Chosen.fcproj").exists());
    operation::cancel_before_dispatch(&mut s, &key, &|| Ok(())).unwrap();
    s.shutdown();
}
