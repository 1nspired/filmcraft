//! Raw historical native payloads use the original editor/clipboard codec.
//! No live Scope, worker, operation, picker or replay ownership is decoded.
use super::*;
fn mark() -> filmcraft_project::Marker {
    filmcraft_project::Marker {
        id: filmcraft_project::MarkerId(77),
        start: filmcraft_time::Tick(0),
        duration: filmcraft_time::Tick(0),
        name: "original clipboard marker".into(),
        comment: "native raw payload".into(),
        kind: filmcraft_project::MarkerKind::Comment,
        color: filmcraft_project::Label::Rose,
    }
}
fn clipboard(s: &mut Session) {
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    s.state.clipboard = vec![(filmcraft_project::TrackKind::Video, 0, item)];
    s.state.clipboard_markers = vec![mark()];
}
fn decode(h: &ApplicationTabHistory) -> ApplicationTabHistory {
    let pool = cold_pool(h);
    let map: Vec<_> = (0..pool.len()).collect();
    ApplicationTabHistory::from_proof(&h.proof().unwrap(), &pool, &map).unwrap()
}
fn pack(payload: &Payload, version: u8) -> Value {
    let raw = bytes(payload, MAX_BYTES).unwrap();
    json!({"kind":KIND,"version":version,"bytes":raw.len(),"sha256":digest(&raw),"data":String::from_utf8(raw).unwrap()})
}
fn reject(h: &ApplicationTabHistory, proof: &Value) {
    let pool = cold_pool(h);
    let map: Vec<_> = (0..pool.len()).collect();
    assert!(ApplicationTabHistory::from_proof(proof, &pool, &map).is_err());
}
#[test]
fn complete_raw_clipboard_markers_preferences_and_auxiliary_roundtrip_on_original_codec() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    s.prefs.media.enable_proxies = !s.prefs.media.enable_proxies;
    let scope = prepare(&mut s, 101);
    let receipt = publish(&mut s, scope, main);
    let before = native_state(&s);
    let h = receipt.capture_history().unwrap();
    assert_eq!(h.proof().unwrap()["version"], 2);
    assert!(h.native_snapshot_complete());
    let cold = decode(&h);
    for snapshot in [&cold.saved.origin, &cold.saved.before, &cold.saved.after] {
        let native = snapshot.native.as_ref().unwrap();
        assert_eq!(native.clipboard.len(), 1);
        assert_eq!(native.clipboard_markers, vec![mark()]);
        assert_eq!(native.preferences.media.enable_proxies, s.prefs.media.enable_proxies);
        assert!(native.auxiliary.is_some());
        assert!(snapshot.native_error.is_none());
    }
    // The original installer restores the same raw fields; this private test does
    // not introduce an RPC importer or claim normal application cold admission.
    let raw = cold.saved.after.native.as_ref().unwrap();
    let mut target = Session::default();
    target.project = cold.projects[cold.saved.after.project].clone();
    raw.install(&mut target).unwrap();
    assert_eq!(target.state.clipboard, s.state.clipboard);
    assert_eq!(target.state.clipboard_markers, s.state.clipboard_markers);
    assert_eq!(target.prefs.media.enable_proxies, s.prefs.media.enable_proxies);
    assert_eq!(native_state(&s), before);
}
#[test]
fn uncertain_actual_preserves_distinct_raw_clipboard_and_preferences_without_replay() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    let scope = prepare(&mut s, 102);
    let publication = scope.prepare_activation(&mut s, main.0).unwrap().enter_publication(&mut s).unwrap();
    s.state.clipboard_markers[0].name = "later actual marker".into();
    s.prefs.media.enable_proxies = !s.prefs.media.enable_proxies;
    let receipt = publication.finish(&s);
    assert!(!receipt.confirmed());
    let before = native_state(&s);
    let cold = decode(&receipt.capture_history().unwrap());
    assert!(cold.native_snapshot_complete());
    assert_eq!(cold.saved.before.native.as_ref().unwrap().clipboard_markers[0].name, "original clipboard marker");
    let actual = cold.saved.actual.as_ref().unwrap().native.as_ref().unwrap();
    assert_eq!(actual.clipboard_markers, s.state.clipboard_markers);
    assert_eq!(actual.preferences.media.enable_proxies, s.prefs.media.enable_proxies);
    assert_eq!(cold.result()["canReplay"], false);
    assert_eq!(native_state(&s), before);
}
#[test]
fn busy_job_keeps_original_human_tab_behavior_and_raw_fields_without_importing_live_auxiliary() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    s.jobs.push(crate::Job {
        id: 9,
        label: "generated live owner".into(),
        progress: Arc::new(filmcraft_export::Progress::default()),
        result: Arc::new(std::sync::Mutex::new(None)),
    });
    let scope = prepare(&mut s, 103);
    let receipt = publish(&mut s, scope, main);
    let h = receipt.capture_history().unwrap();
    let cold = decode(&h);
    assert!(receipt.confirmed());
    assert!(!cold.native_snapshot_complete());
    for snapshot in [&cold.saved.origin, &cold.saved.before, &cold.saved.after] {
        let raw = snapshot.native.as_ref().unwrap();
        assert_eq!(raw.clipboard.len(), 1);
        assert!(raw.auxiliary.is_none());
        assert!(snapshot.native_error.is_none());
    }
    assert_eq!(s.jobs.len(), 1);
    assert!(s.jobs[0].result.lock().unwrap().is_none());
    s.jobs.clear();
    assert!(!cold.native_snapshot_complete());
}
#[test]
fn raw_payload_editor_clipboard_merge_saved_revision_and_auxiliary_tampering_is_rejected() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    let scope = prepare(&mut s, 104);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    for field in ["editor", "clipboard", "merge", "saved", "auxiliary"] {
        let mut payload: Payload = serde_json::from_str(h.proof().unwrap()["data"].as_str().unwrap()).unwrap();
        let raw = payload.record.after.native.as_mut().unwrap();
        match field {
            "editor" => raw.editor.snapping = !raw.editor.snapping,
            "clipboard" => raw.clipboard_markers[0].comment.push_str(" forged"),
            "merge" => raw.merge_key = Some("foreign".into()),
            "saved" => raw.saved_revision = payload.record.after.revision + 1,
            _ => {
                let mut value = serde_json::to_value(&*raw).unwrap();
                value["auxiliary"]["version"] = json!(99);
                *raw = serde_json::from_value(value).unwrap();
            }
        }
        reject(&h, &pack(&payload, 2));
    }
}
#[test]
fn genuine_legacy_hash_only_proofs_roundtrip_without_fabricating_raw_snapshots() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    let scope = prepare(&mut s, 105);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    let mut record = h.saved.clone();
    record.version = 1;
    for snapshot in [&mut record.origin, &mut record.before, &mut record.after].into_iter().chain(record.actual.iter_mut()) {
        snapshot.native = None;
        snapshot.native_error = None;
    }
    let payload = Payload { version: 1, snapshots: h.hashes().unwrap(), record };
    let proof = pack(&payload, 1);
    let pool = cold_pool(&h);
    let map: Vec<_> = (0..pool.len()).collect();
    let old = ApplicationTabHistory::from_proof(&proof, &pool, &map).unwrap();
    assert_eq!(old.proof().unwrap(), proof);
    assert!(!old.native_snapshot_complete());
    assert!(old.saved.after.native.is_none());
    assert!(old.saved.after.native_error.is_none());
}
#[test]
fn version_mixing_missing_raw_markers_and_fake_complete_error_states_never_promote() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 106);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    let p = h.proof().unwrap();
    for kind in ["outer", "payload", "record", "missing", "both", "empty-error", "oversized-error"] {
        let mut payload: Payload = serde_json::from_str(p["data"].as_str().unwrap()).unwrap();
        let mut version = 2;
        match kind {
            "outer" => version = 1,
            "payload" => payload.version = 1,
            "record" => payload.record.version = 1,
            "missing" => {
                payload.record.after.native = None;
                payload.record.after.native_error = None;
            }
            "both" => payload.record.after.native_error = Some("not a failure when raw exists".into()),
            _ => {
                payload.record.after.native = None;
                payload.record.after.native_error = Some(if kind == "empty-error" { String::new() } else { "x".repeat(513) });
            }
        }
        reject(&h, &pack(&payload, version));
    }
}
#[test]
fn oversized_original_clipboard_refuses_at_original_trim_bound_before_publication() {
    let (_root, mut s, _, _) = setup(128);
    clipboard(&mut s);
    s.state.clipboard_markers[0].comment = "x".repeat(8 * 1024 * 1024);
    let before = native_state(&s);
    let project = s.project.clone();
    let saved = std::fs::read(s.path.as_ref().unwrap()).unwrap();
    let error = match s.prepare_application_tab_scope(&json!({"token":format!("{:064x}",107),"scope":scope(),"projectPath":s.path})) {
        Ok(_) => panic!("original oversized clipboard wire bound must refuse before a live publication"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("trim evidence exceeds its wire bound"));
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(native_state(&s), before);
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    assert_eq!(s.state.clipboard_markers[0].comment.len(), 8 * 1024 * 1024);
}
#[test]
fn failed_original_auxiliary_capture_stays_explicit_without_fabricating_raw_editor_data() {
    let (_root, mut s, main, _) = setup(128);
    clipboard(&mut s);
    let progress = Arc::new(filmcraft_export::Progress::default());
    progress.finished.store(true, std::sync::atomic::Ordering::Relaxed);
    s.jobs.push(crate::Job {
        id: 0,
        label: "generated malformed historical job".into(),
        progress,
        result: Arc::new(std::sync::Mutex::new(Some(Err("generated finished error".into())))),
    });
    let original = prepare(&mut s, 109);
    let receipt = publish(&mut s, original, main);
    assert!(receipt.confirmed());
    let before = native_state(&s);
    let cold = decode(&receipt.capture_history().unwrap());
    assert!(!cold.native_snapshot_complete());
    for snapshot in [&cold.saved.origin, &cold.saved.before, &cold.saved.after] {
        assert!(snapshot.native.is_none());
        let error = snapshot.native_error.as_ref().unwrap();
        assert!(error.contains("invalid original completed job identities"));
        assert!(error.chars().count() <= 512);
    }
    assert_eq!(native_state(&s), before);
    assert_eq!(s.jobs.len(), 1);
    assert_eq!(s.state.clipboard.len(), 1);
}
#[test]
fn confirmed_after_raw_state_is_captured_from_actual_human_result_instead_of_inferred() {
    let (_root, mut s, _, other) = setup(128);
    clipboard(&mut s);
    let initial = s.prefs.media.enable_proxies;
    let scope = prepare(&mut s, 108);
    let publication = scope.prepare_activation(&mut s, other.0).unwrap().enter_publication(&mut s).unwrap();
    s.execute("sequence.open", json!({"item":other.0})).unwrap();
    s.prefs.media.enable_proxies = !initial;
    let receipt = publication.finish(&s);
    assert!(receipt.confirmed());
    let cold = decode(&receipt.capture_history().unwrap());
    assert_eq!(cold.saved.before.native.as_ref().unwrap().preferences.media.enable_proxies, initial);
    assert_eq!(cold.saved.after.native.as_ref().unwrap().preferences.media.enable_proxies, !initial);
    assert!(cold.native_snapshot_complete());
    assert_eq!(s.prefs.media.enable_proxies, !initial);
}
