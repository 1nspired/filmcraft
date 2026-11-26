use super::*;
use crate::{FsServices, Services, autosave::AutosaveConfig, project_save::prepare};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
fn fixture() -> (Session, PathBuf) {
    let root =
        crate::temp_dir().join(format!("poema-save-quiescence-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&root).unwrap();
    let mut s = Session::default();
    s.start_autosave(AutosaveConfig::new(root.join("native"))).unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Original.fcproj")})).unwrap();
    s.edit("Generated human metadata", |p, _| {
        p.name = "Edited".into();
        Ok(())
    })
    .unwrap();
    (s, root)
}
#[test]
fn quiescent_original_save_service_preserves_all_four_human_variants_full_bytes_and_native_history_without_a_new_write_command() {
    for kind in ["save", "save_as", "save_copy", "save_all"] {
        let (mut s, root) = fixture();
        let before_path = s.path.clone();
        let history = s.history.undo.clone();
        let path = if matches!(kind, "save" | "save_all") { s.path.clone().unwrap() } else { root.join("Chosen.fcproj").to_str().unwrap().into() };
        let adopt = kind != "save_copy";
        let (receipt, description) = with_quiescent(&mut s, &|| Ok(()), |s| {
            assert!(s.previews.pause_writers(&|| Ok(())).is_err());
            let prepared = prepare(s, &path, adopt)?;
            let description = prepared.description();
            let receipt = prepared.publish(s)?;
            Ok((receipt, description))
        })
        .unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(receipt["bytes"], bytes.len());
        assert_eq!(description["output"]["sha256"], crate::saved_file::sha256(&bytes));
        assert_eq!(s.path, if adopt { Some(path) } else { before_path });
        assert_eq!(s.is_dirty(), !adopt);
        assert_eq!(history.len(), s.history.undo.len());
        for (a, b) in history.iter().zip(&s.history.undo) {
            assert_eq!(a.0, b.0);
            assert!(Arc::ptr_eq(&a.1, &b.1));
        }
        s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
        drop(s.previews.pause_writers(&|| Ok(())).unwrap());
        s.shutdown();
    }
}
#[test]
fn quiescent_save_callback_error_and_caught_panic_resume_original_workers_and_sync_newly_saved_state_without_rollback_or_replay() {
    for panic_after_write in [false, true] {
        let (mut s, root) = fixture();
        let target = root.join("Result.fcproj").to_str().unwrap().to_string();
        let result = with_quiescent(&mut s, &|| Ok(()), |s| -> Result<()> {
            if !panic_after_write {
                return Err(error("generated prepublication refusal"));
            }
            prepare(s, &target, true)?.publish(s)?;
            panic!("generated failure after original file publication");
        });
        assert!(result.is_err());
        if panic_after_write {
            let saved = fs::read(&target).unwrap();
            assert!(!s.is_dirty());
            assert!(s.persistence.as_ref().unwrap().auto_save_now().unwrap().is_none());
            assert_eq!(fs::read(&target).unwrap(), saved);
        } else {
            assert!(!root.join("Result.fcproj").exists());
            assert!(s.is_dirty());
        }
        s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
        drop(s.previews.pause_writers(&|| Ok(())).unwrap());
        s.shutdown();
    }
}
struct Counted {
    writes: AtomicUsize,
}
impl Services for Counted {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        FsServices.read_file(path)
    }
    fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        FsServices.write_file(path, bytes)
    }
}
#[test]
fn quiescent_save_late_stop_retains_the_original_published_file_and_releases_workers_without_a_second_publication() {
    let (mut s, root) = fixture();
    let services = Arc::new(Counted { writes: AtomicUsize::new(0) });
    s.services = services.clone();
    let stop = AtomicBool::new(false);
    let target = root.join("Stopped.fcproj").to_str().unwrap().to_string();
    let result = with_quiescent(&mut s, &|| if stop.load(Ordering::Acquire) { Err(error("generated late Stop")) } else { Ok(()) }, |s| {
        let receipt = prepare(s, &target, true)?.publish(s)?;
        stop.store(true, Ordering::Release);
        Ok(receipt)
    });
    assert!(result.is_err());
    assert_eq!(services.writes.load(Ordering::SeqCst), 1);
    let saved = fs::read(&target).unwrap();
    s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
    assert_eq!(fs::read(&target).unwrap(), saved);
    assert_eq!(services.writes.load(Ordering::SeqCst), 1);
    s.shutdown();
}
#[test]
fn quiescent_save_reuses_original_idle_and_display_owner_fences_without_cancelling_or_adopting_unfinished_human_work() {
    let (mut s, root) = fixture();
    s.jobs.push(crate::Job { id: 1, label: "existing human work".into(), progress: Arc::default(), result: Arc::default() });
    let ran = AtomicBool::new(false);
    assert!(
        with_quiescent(&mut s, &|| Ok(()), |_| {
            ran.store(true, Ordering::Relaxed);
            Ok(())
        })
        .is_err()
    );
    assert!(!ran.load(Ordering::Relaxed));
    assert!(!s.jobs[0].progress.cancel.load(Ordering::Relaxed));
    s.jobs.clear();
    s.view_only = true;
    assert!(with_quiescent(&mut s, &|| Ok(()), |_| Ok(())).is_err());
    s.view_only = false;
    assert!(!root.join("not-written.fcproj").exists());
    s.shutdown();
}
#[test]
fn quiescent_save_refuses_replaced_persistence_or_preview_owners_and_never_resumes_their_replacements_as_its_own() {
    for persistence in [false, true] {
        let (mut s, root) = fixture();
        let result = with_quiescent(&mut s, &|| Ok(()), |s| {
            if persistence {
                drop(s.persistence.take());
                s.start_autosave(AutosaveConfig::new(root.join("replacement"))).unwrap();
            } else {
                s.previews = Arc::new(crate::previews::PreviewStore::default());
            }
            Ok(())
        });
        assert!(result.is_err());
        s.persistence.as_ref().unwrap().pause_project_save(|| Ok(())).unwrap().finish(|| Ok(())).unwrap();
        drop(s.previews.pause_writers(&|| Ok(())).unwrap());
        s.shutdown();
    }
}
