use super::*;
use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
use std::{
    cell::Cell,
    fs,
    os::unix::fs::DirBuilderExt,
    sync::atomic::{AtomicU64, Ordering},
};
pub(super) struct Scratch(pub(super) std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(super) fn fixture() -> (Scratch, Session, Prepared) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = crate::temp_dir().join(format!("retained-checkpoint-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut s = Session::default();
    s.execute(
        "file.newProject",
        json!({"name":"Retained checkpoint","workspace":{"projectId":"p_checkpoint","workspaceId":"11111111-1111-4111-8111-111111111111"}}),
    )
    .unwrap();
    s.execute("file.saveAs", json!({"path":root.join("Root.fcproj")})).unwrap();
    s.execute("file.newBin", json!({"name":"Human unsaved bin"})).unwrap();
    s.state.source_playhead = filmcraft_time::Tick(9_007_199_254_740_991);
    let data = root.join("native");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
    let cfg = AutosaveConfig::new(data);
    let reservation = ColdJournalReservation::prepare(&cfg, &|| Ok(())).unwrap();
    s.persistence = Some(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &|| Ok(())).unwrap());
    // Only this internal COW/worker composition is synthetic. Production
    // Prepared::capture still requires the actual original Input/Cold owner.
    crate::agent_bridge::capture_native_journal(&mut s).unwrap().unwrap();
    let native = crate::agent_bridge::history_checkpoint::journal::capture_retained(&s).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let description = json!({"nativeSession":crate::agent_bridge::retained_native_epoch(&s),"nativeData":worker.data_dir,"sessionDirectory":worker.session_dir,"serialized":false,"filesWritten":false});
    let prepared = Prepared {
        guard: crate::project_save::guard(&s).unwrap(),
        graph: s.project.clone(),
        native: Some(native),
        encoding_attempted: false,
        encoded: None,
        archive_attempted: false,
        archive: None,
        current_release_attempted: false,
        current_release_ready: false,
        current_native: None,
        completion: None,
        description,
    };
    (Scratch(root), s, prepared)
}
pub(super) fn no_writes(s: &Session, before: &[u8]) {
    let worker = s.persistence.as_ref().unwrap();
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    assert_eq!(fs::read_dir(&worker.session_dir).unwrap().count(), 1);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), before);
}
pub(super) fn stop(s: &mut Session) {
    s.persistence.as_mut().unwrap().stop_worker();
}
#[test]
fn retained_checkpoint_encodes_actual_complete_cow_on_original_worker_and_keeps_its_guard_and_historical_preparation() {
    let (_d, mut s, mut prepared) = fixture();
    let before = fs::read(s.path.as_ref().unwrap()).unwrap();
    let historical = prepared.description();
    let graph = s.project.clone();
    let checks = Cell::new(0);
    let result = prepared
        .encode(&s, &|| {
            checks.set(checks.get() + 1);
            Ok(())
        })
        .unwrap();
    assert!(checks.get() >= 6);
    assert_eq!(result["preparation"], historical);
    assert_eq!(prepared.description(), historical);
    assert_eq!(result["bytes"]["serialized"], true);
    assert_eq!(result["bytes"]["filesWritten"], false);
    assert!(prepared.native.is_none());
    assert!(prepared.encoded.is_some());
    assert!(Arc::ptr_eq(&prepared.graph, &graph));
    prepared.current(&s).unwrap();
    let files = prepared.encoded.as_ref().unwrap().files(s.persistence.as_ref().unwrap()).unwrap();
    assert_eq!(files[0].1, filmcraft_format::encode(&s.project, false));
    let native: Value = serde_json::from_slice(files[2].1).unwrap();
    assert!(native["payload"]["editor"]["auxiliary"].is_object());
    assert_eq!(native["payload"]["history"]["undo"].as_array().unwrap().len(), 1);
    assert_eq!(native["payload"]["editor"]["saved_revision"], s.saved_revision);
    assert_eq!(native["payload"]["editor"]["editor"]["source_playhead"], serde_json::to_value(s.state.source_playhead).unwrap());
    no_writes(&s, &before);
    stop(&mut s);
}
#[test]
fn retained_checkpoint_success_cannot_reencode_or_replace_its_actual_opaque_worker_bytes() {
    let (_d, mut s, mut prepared) = fixture();
    let first = prepared.encode(&s, &|| Ok(())).unwrap();
    assert!(prepared.encode(&s, &|| Ok(())).is_err());
    prepared.current(&s).unwrap();
    assert_eq!(prepared.encoded.as_ref().unwrap().description(s.persistence.as_ref().unwrap()).unwrap(), first["bytes"]);
    stop(&mut s);
}
#[test]
fn retained_checkpoint_initial_source_refusal_preserves_original_unconsumed_cow_and_attempt() {
    let (_d, mut s, mut prepared) = fixture();
    assert!(prepared.encode(&s, &|| Err(error("generated source stop before encoding"))).is_err());
    assert!(!prepared.encoding_attempted);
    assert!(prepared.native.is_some());
    assert!(prepared.encoded.is_none());
    prepared.current(&s).unwrap();
    prepared.encode(&s, &|| Ok(())).unwrap();
    stop(&mut s);
}
#[test]
fn retained_checkpoint_source_loss_after_enqueue_keeps_unresolved_cow_consumption_and_prevents_replay_or_release() {
    let (_d, mut s, mut prepared) = fixture();
    let before = fs::read(s.path.as_ref().unwrap()).unwrap();
    let checks = Cell::new(0);
    assert!(
        prepared
            .encode(&s, &|| {
                let next = checks.get() + 1;
                checks.set(next);
                if next >= 4 { Err(error("generated retained source loss")) } else { Ok(()) }
            })
            .is_err()
    );
    assert!(prepared.encoding_attempted);
    assert!(prepared.native.is_none());
    assert!(prepared.encoded.is_none());
    assert!(prepared.current(&s).is_err());
    assert!(prepared.encode(&s, &|| Ok(())).is_err());
    assert!(s.persistence.as_ref().unwrap().release_current_image(|_| Ok(()), || Ok(())).is_err());
    no_writes(&s, &before);
    stop(&mut s);
}
#[test]
fn retained_checkpoint_refuses_changed_cow_graph_revision_and_editor_before_consuming_original_input() {
    for kind in 0..3 {
        let (_d, mut s, mut prepared) = fixture();
        match kind {
            0 => s.project = Arc::new((*s.project).clone()),
            1 => s.revision += 1,
            _ => s.state.source_playhead = filmcraft_time::Tick(1),
        }
        assert!(prepared.encode(&s, &|| Ok(())).is_err());
        assert!(!prepared.encoding_attempted);
        assert!(prepared.native.is_some());
        stop(&mut s);
    }
}
#[test]
fn retained_checkpoint_actual_encoded_value_is_invalid_after_original_worker_stops_or_releases() {
    for release in [false, true] {
        let (_d, mut s, mut prepared) = fixture();
        prepared.encode(&s, &|| Ok(())).unwrap();
        if release {
            s.persistence.as_ref().unwrap().release_current_image(|_| Ok(()), || Ok(())).unwrap();
        } else {
            stop(&mut s);
        }
        assert!(prepared.current(&s).is_err());
        assert!(prepared.encode(&s, &|| Ok(())).is_err());
        stop(&mut s);
    }
}
#[test]
fn retained_checkpoint_cannot_cross_a_different_actual_persistence_owner() {
    let (_d, mut s, mut prepared) = fixture();
    let (_other, mut foreign, _p) = fixture();
    std::mem::swap(&mut s.persistence, &mut foreign.persistence);
    assert!(prepared.encode(&s, &|| Ok(())).is_err());
    assert!(!prepared.encoding_attempted);
    assert!(prepared.native.is_some());
    stop(&mut s);
    stop(&mut foreign);
}

#[test]
fn retained_checkpoint_unresolved_encoding_and_archive_attempts_are_visible_without_session_revalidation() {
    for kind in 0..3 {
        let (_d, mut s, mut prepared) = fixture();
        prepared.resolved().unwrap();
        match kind {
            0 => prepared.encoding_attempted = true,
            1 => {
                prepared.encoding_attempted = true;
                prepared.native = None;
            }
            _ => prepared.archive_attempted = true,
        }
        assert!(prepared.resolved().is_err());
        assert!(prepared.current(&s).is_err());
        stop(&mut s);
    }
}
