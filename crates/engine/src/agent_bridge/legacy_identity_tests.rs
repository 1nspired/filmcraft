//! Candidate identity conversion is never a current-state or history grant.
use super::{
    tests::{source_clock_fixture, token},
    *,
};
use serde_json::{Value, json};
use std::sync::Arc;
const W: &str = "c7f2347a-1cc0-4e06-bdcb-c58a8b9ecafb";
const CMD: &str = "agent.inspectIdentityMigration";
fn setup() -> Session {
    let mut s = source_clock_fixture();
    s.path = Some("/generated/identity-migration.fcproj".into());
    s.worker_available = true;
    s.agent_operation_control().enable_transport();
    s
}
fn request(s: &mut Session, n: usize) -> Value {
    let scope = json!({"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0});
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope})).unwrap();
    json!({"token":token(n),"scope":scope,"workspace":W})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"path":s.path,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal,"events":s.events.len()})
}
fn unchanged(s: &mut Session, p: Value) -> Result<Value> {
    let before = state(s);
    let pointer = s.project.clone();
    let r = s.execute(CMD, p);
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&s.project, &pointer));
    r
}
#[test]
fn legacy_identity_inspection_prepares_complete_native_graph_without_installing_or_qualifying_it() {
    let mut s = setup();
    let p = request(&mut s, 70101);
    let r = unchanged(&mut s, p).unwrap();
    assert_eq!(r["status"], "read");
    assert_eq!(r["historyValidation"], "complete_candidate_graph");
    assert!(r["changedSnapshots"].as_u64().unwrap() > 0);
    assert_eq!(r["sourceGraph"]["snapshots"], r["candidateGraph"]["snapshots"]);
    assert_eq!(r["sourceGraph"]["undoSteps"], r["candidateGraph"]["undoSteps"]);
    assert_eq!(r["sourceGraph"]["redoSteps"], r["candidateGraph"]["redoSteps"]);
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["execution"], "not-applied");
    assert!(crate::project_workspace::identity(&s.project).unwrap().is_none());
    assert!(s.execute(CMD, json!({"token":token(70101),"scope":r["scope"],"workspace":W})).is_err());
    assert!(s.execute("agent.readProject", json!({"token":token(70102),"scope":r["projectScope"],"projectPath":s.path})).is_err());
}
#[test]
fn legacy_identity_candidate_changes_only_reserved_owner_metadata_and_preserves_all_editorial_payloads() {
    let s = setup();
    let anchor = s.state.active_sequence.unwrap();
    let scope = crate::project_workspace::ProjectScope { project_id: "p_source_clock".into(), workspace_id: W.into() };
    let before = serde_json::to_value(&s.project).unwrap();
    let mut candidate = crate::project_workspace::legacy_identity_candidate(&s.project, &scope, anchor).unwrap();
    assert_eq!(crate::project_workspace::identity(&candidate).unwrap().unwrap().scope, scope);
    candidate.metadata.remove(crate::project_workspace::KEY);
    candidate.item_mut(anchor).unwrap().metadata.remove(crate::editorial_workspace::KEY);
    assert_eq!(serde_json::to_value(candidate).unwrap(), before);
    assert!(crate::project_workspace::identity(&s.project).unwrap().is_none());
}
#[test]
fn legacy_identity_inspection_validates_original_receipts_and_retains_owned_undo_and_redo_geometry() {
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    super::tests::read(&mut s, 70200);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute(
        "agent.apply",
        super::tests::exact_batch(70200, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
    for (n, redo) in [(70201, false), (70203, true)] {
        if redo {
            super::tests::read(&mut s, 70202);
            s.execute("agent.undo", json!({"token":token(70202),"commit":token(70200)})).unwrap();
        }
        let p = request(&mut s, n);
        let r = unchanged(&mut s, p).unwrap();
        assert!(r["candidateGraph"]["retainedTickets"].as_u64().unwrap() > 0);
        assert_eq!(r["sourceGraph"]["ownedHead"], r["candidateGraph"]["ownedHead"]);
        assert_eq!(s.state.active_sequence, Some(seq));
        if redo {
            assert!(r["candidateGraph"]["redoSteps"].as_u64().unwrap() > 0);
        }
    }
}
#[test]
fn legacy_identity_inspection_refuses_foreign_workspace_source_and_unprovable_historical_anchor() {
    for damage in ["workspace", "source", "historical_anchor"] {
        let mut s = setup();
        let seq = s.state.active_sequence.unwrap();
        if damage == "workspace" {
            Arc::make_mut(&mut s.project).item_mut(seq).unwrap().metadata.insert(crate::editorial_workspace::KEY.into(),json!({"kind":"poema-native-workspace","version":1,"scope":{"projectId":"p_source_clock","anchorSequence":seq.0},"workspaceId":"8c8f086d-4db6-4a43-8e79-016126bc0eaa"}).to_string());
        } else {
            let mut past = (*s.project).clone();
            if damage == "source" {
                let item = past.items.values_mut().find(|v| v.metadata.contains_key(BINDING_KEY)).unwrap();
                let mut value: Value = serde_json::from_str(&item.metadata[BINDING_KEY]).unwrap();
                value["projectId"] = json!("foreign");
                item.metadata.insert(BINDING_KEY.into(), value.to_string());
            } else {
                let mut other = past.items.remove(&seq).unwrap();
                other.id = ItemId(seq.0 + 50000);
                past.items.insert(other.id, other);
            }
            s.history.undo.push(("Unprovable historical node".into(), Arc::new(past)));
        }
        let p = request(&mut s, 70301);
        assert!(unchanged(&mut s, p).is_err(), "{damage}");
    }
}
#[test]
fn legacy_identity_inspection_refuses_stale_or_alias_parameters_and_honors_independent_stop() {
    for mode in ["extra", "root_scope", "stale", "stopped"] {
        let mut s = setup();
        let mut p = request(&mut s, 70401);
        match mode {
            "extra" => {
                p["path"] = json!("/generated/other");
            }
            "root_scope" => {
                p["scope"] = json!({"projectId":"p_source_clock","workspaceId":W});
            }
            "stale" => {
                s.execute("file.newBin", json!({"name":"Human work"})).unwrap();
            }
            _ => {
                assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(70401)})).unwrap()["cancellationAccepted"], true);
            }
        }
        assert!(unchanged(&mut s, p).is_err(), "{mode}");
    }
    assert!(crate::session_worker::supports(CMD));
}
#[test]
fn legacy_identity_candidate_refuses_root_reassignment_and_preserves_exact_existing_owner_bytes() {
    let s = setup();
    let seq = s.state.active_sequence.unwrap();
    let scope = crate::project_workspace::ProjectScope { project_id: "p_source_clock".into(), workspace_id: W.into() };
    let candidate = crate::project_workspace::legacy_identity_candidate(&s.project, &scope, seq).unwrap();
    assert_eq!(
        serde_json::to_value(crate::project_workspace::legacy_identity_candidate(&candidate, &scope, seq).unwrap()).unwrap(),
        serde_json::to_value(&candidate).unwrap()
    );
    let foreign = crate::project_workspace::ProjectScope { project_id: scope.project_id, workspace_id: "8c8f086d-4db6-4a43-8e79-016126bc0eaa".into() };
    assert!(crate::project_workspace::legacy_identity_candidate(&candidate, &foreign, seq).is_err());
}

#[test]
fn legacy_identity_candidate_bounds_malformed_and_deep_inventory_before_cloning() {
    use filmcraft_project::{Bin, BinEntry, BinId};
    let s = setup();
    let seq = s.state.active_sequence.unwrap();
    let scope = crate::project_workspace::ProjectScope { project_id: "p_source_clock".into(), workspace_id: W.into() };
    for deep in [false, true] {
        let mut project = (*s.project).clone();
        let mut child = Bin { id: BinId(if deep { 9000 } else { 0 }), name: "Generated malformed bin".into(), children: vec![] };
        if deep {
            for i in 0..70 {
                child = Bin { id: BinId(9100 + i), name: "Generated deep bin".into(), children: vec![BinEntry::Bin(child)] };
            }
        }
        project.root.children.push(BinEntry::Bin(child));
        assert!(crate::project_workspace::legacy_identity_candidate(&project, &scope, seq).is_err());
        let mut current = setup();
        current.project = Arc::new(project);
        let request = request(&mut current, 70501);
        assert!(current.execute(CMD, request).is_err());
    }
}

const PUB: &str = "agent.publishIdentityMigration";
const RECORD: &str = "agent.readIdentityPublication";
const MIGRATION: &str = "9ac93dfe-2464-47cb-9d6b-4630b2fe5461";
struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = crate::temp_dir().join(format!("identity-publication-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
    fn record(&self) -> std::path::PathBuf {
        self.0.join(format!(".poema-identity-{W}-{MIGRATION}.json"))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn saved_setup() -> (Directory, Session) {
    let dir = Directory::new();
    let mut s = setup();
    s.path = Some(dir.0.join("generated.fcproj").to_str().unwrap().into());
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
    (dir, s)
}
fn publication(s: &mut Session, n: usize) -> Value {
    use sha2::{Digest, Sha256};
    let p = request(s, n);
    let inspection = s.execute(CMD, p).unwrap();
    let fresh = request(s, n + 1);
    json!({"token":fresh["token"],"scope":fresh["scope"],"workspace":W,"migration":MIGRATION,"expected":{"sourceGraphSha256":inspection["sourceGraph"]["sha256"],"candidateGraphSha256":inspection["candidateGraph"]["sha256"],"savedFileSha256":format!("{:x}",Sha256::digest(std::fs::read(s.path.as_ref().unwrap()).unwrap()))}})
}
fn record_request(s: &mut Session, n: usize, expected: Value, project: bool) -> Value {
    let scope = if project {
        json!({"projectId":"p_source_clock","workspaceId":W})
    } else {
        json!({"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0})
    };
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":scope})).unwrap();
    json!({"token":token(n),"scope":scope,"workspace":W,"migration":MIGRATION,"expected":expected})
}
#[test]
fn legacy_identity_publication_atomically_preserves_the_editor_file_and_full_native_graph() {
    let (dir, mut s) = saved_setup();
    let p = publication(&mut s, 70600);
    let before = state(&s);
    let file = std::fs::read(s.path.as_ref().unwrap()).unwrap();
    let r = s.execute(PUB, p.clone()).unwrap();
    assert_eq!(r["status"], "committed");
    assert_eq!(r["revision"], before["revision"].as_u64().unwrap() + 1);
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before["editor"]);
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), file);
    assert!(dir.record().is_file());
    assert!(crate::project_workspace::identity(&s.project).unwrap().is_some());
    assert_eq!(s.history.undo.len(), before["undo"].as_array().unwrap().len());
    assert_eq!(s.history.redo.len(), before["redo"].as_array().unwrap().len());
    let raw = std::fs::read(dir.record()).unwrap();
    let after = state(&s);
    assert!(s.execute(PUB, p.clone()).is_err());
    assert_eq!(state(&s), after);
    assert_eq!(std::fs::read(dir.record()).unwrap(), raw);
    let read = record_request(&mut s, 70602, p["expected"].clone(), true);
    let proof = unchanged_record(&mut s, read).unwrap();
    assert_eq!(proof["liveState"], "applied");
    assert_eq!(proof["publication"], r["publication"]);
}
fn unchanged_record(s: &mut Session, p: Value) -> Result<Value> {
    let before = state(s);
    let old = s.project.clone();
    let r = s.execute(RECORD, p);
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&s.project, &old));
    r
}
#[test]
fn legacy_identity_publication_keeps_original_receipts_and_exact_owned_undo_redo_after_promotion() {
    let (_dir, mut s) = saved_setup();
    super::tests::read(&mut s, 70700);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute(
        "agent.apply",
        super::tests::exact_batch(70700, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
    let original = s.execute("agent.receipt", json!({"token":token(70700)})).unwrap();
    let p = publication(&mut s, 70701);
    s.execute(PUB, p).unwrap();
    assert_eq!(s.execute("agent.receipt", json!({"token":token(70700)})).unwrap(), original);
    super::tests::read(&mut s, 70703);
    s.execute("agent.undo", json!({"token":token(70703),"commit":token(70700)})).unwrap();
    assert!(crate::project_workspace::identity(&s.project).unwrap().is_some());
    super::tests::read(&mut s, 70704);
    s.execute("agent.redo", json!({"token":token(70704),"commit":token(70700),"undo":token(70703)})).unwrap();
    assert!(crate::project_workspace::identity(&s.project).unwrap().is_some());
}
#[test]
fn legacy_identity_publication_refuses_stale_graph_file_changes_stop_and_existing_decisions_without_mutation() {
    for mode in ["hash", "file", "stopped", "existing"] {
        let (dir, mut s) = saved_setup();
        let mut p = publication(&mut s, 70800);
        if mode == "hash" {
            p["expected"]["candidateGraphSha256"] = json!("a".repeat(64));
        } else if mode == "file" {
            std::fs::write(s.path.as_ref().unwrap(), b"changed saved project").unwrap();
        } else if mode == "stopped" {
            s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap();
        } else {
            std::fs::write(dir.record(), b"existing decision preserved").unwrap();
        }
        let before = state(&s);
        assert!(s.execute(PUB, p).is_err(), "{mode}");
        assert_eq!(state(&s), before);
        if mode == "existing" {
            assert_eq!(std::fs::read(dir.record()).unwrap(), b"existing decision preserved");
        } else {
            assert!(!dir.record().exists());
        }
    }
}
#[test]
fn legacy_identity_publication_record_validates_after_cold_reopen_without_installing_the_committed_graph() {
    let (dir, mut s) = saved_setup();
    let p = publication(&mut s, 70900);
    let r = s.execute(PUB, p.clone()).unwrap();
    let raw = std::fs::read(dir.record()).unwrap();
    let mut cold = Session::default();
    cold.execute("file.open", json!({"path":s.path})).unwrap();
    cold.worker_available = true;
    cold.agent_operation_control().enable_transport();
    let request = record_request(&mut cold, 70902, p["expected"].clone(), false);
    let proof = unchanged_record(&mut cold, request).unwrap();
    assert_eq!(proof["liveState"], "recorded");
    assert_eq!(proof["publication"], r["publication"]);
    assert!(crate::project_workspace::identity(&cold.project).unwrap().is_none());
    assert_eq!(std::fs::read(dir.record()).unwrap(), raw);
}
#[test]
fn legacy_identity_publication_record_refuses_truncation_foreign_evidence_and_human_intervention_as_current_ownership() {
    let (dir, mut s) = saved_setup();
    let p = publication(&mut s, 71000);
    s.execute(PUB, p.clone()).unwrap();
    let raw = std::fs::read(dir.record()).unwrap();
    std::fs::write(dir.record(), b"{").unwrap();
    let request = record_request(&mut s, 71002, p["expected"].clone(), true);
    assert!(unchanged_record(&mut s, request).is_err());
    std::fs::write(dir.record(), &raw).unwrap();
    s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let request = record_request(&mut s, 71003, p["expected"].clone(), true);
    let proof = unchanged_record(&mut s, request).unwrap();
    assert_eq!(proof["liveState"], "changed");
    assert_eq!(std::fs::read(dir.record()).unwrap(), raw);
    assert!(crate::session_worker::supports(PUB));
    assert!(crate::session_worker::supports(RECORD));
}

fn recovery_startup(s: &Session, p: &Value, r: &Value, n: usize) -> Value {
    json!({"projectPath":s.path,"scope":p["scope"],"workspace":W,"migration":MIGRATION,"expected":p["expected"],"sha256":r["publication"]["sha256"],"activationToken":token(n)})
}
fn enable_recovered(s: &mut Session, r: &Value) {
    s.worker_available = true;
    s.agent_operation_control().enable_transport();
    let f = &r["fence"];
    s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":f["token"],"scope":f["scope"],"workspace":W,"update":MIGRATION,"nativeSession":f["nativeSession"]}),
    )
    .unwrap();
}
#[test]
fn identity_recovery_cold_startup_restores_the_complete_editor_and_graph_into_a_paused_candidate() {
    let (dir, mut s) = saved_setup();
    s.set_playhead(filmcraft_time::Tick(254016000000));
    s.state.linked_selection = false;
    s.prefs.media.enable_proxies = false;
    s.state.clipboard = vec![(filmcraft_project::TrackKind::Video, 0, s.active_sequence().unwrap().video_tracks[0].items[0].clone())];
    let p = publication(&mut s, 71100);
    let published = s.execute(PUB, p.clone()).unwrap();
    let startup = recovery_startup(&s, &p, &published, 71102);
    let file = std::fs::read(s.path.as_ref().unwrap()).unwrap();
    let record = std::fs::read(dir.record()).unwrap();
    let mut cold = Session::default();
    let r = cold.open_identity_publication(&startup).unwrap();
    assert_eq!(r["execution"], "snapshot-restored-paused");
    assert_eq!(r["publication"], published["publication"]);
    assert_ne!(r["fence"]["nativeSession"], published["nativeSession"]);
    assert!(cold.is_display_only());
    assert_eq!(serde_json::to_value(&cold.state).unwrap(), serde_json::to_value(&s.state).unwrap());
    assert_eq!(serde_json::to_value(&cold.state.clipboard).unwrap(), serde_json::to_value(&s.state.clipboard).unwrap());
    assert_eq!(serde_json::to_value(&cold.prefs).unwrap(), serde_json::to_value(&s.prefs).unwrap());
    assert_eq!(state(&cold)["project"], state(&s)["project"]);
    assert_eq!(cold.revision, s.revision);
    assert_eq!(cold.saved_revision, s.saved_revision);
    assert!(cold.execute("file.newBin", json!({"name":"Forbidden before activation"})).is_err());
    assert!(cold.execute("file.save", json!({})).is_err());
    assert!(cold.execute("agent.openIdentityPublication", startup.clone()).is_err());
    assert!(!crate::session_worker::supports("agent.openIdentityPublication"));
    cold.worker_available = true;
    cold.agent_operation_control().enable_transport();
    let read = record_request(&mut cold, 71103, p["expected"].clone(), true);
    assert_eq!(unchanged_record(&mut cold, read).unwrap()["liveState"], "recovered");
    assert_eq!(std::fs::read(s.path.as_ref().unwrap()).unwrap(), file);
    assert_eq!(std::fs::read(dir.record()).unwrap(), record);
    enable_recovered(&mut cold, &r);
    assert!(!cold.is_display_only());
}
#[test]
fn identity_recovery_retains_original_receipts_and_owned_native_undo_redo_without_recipe_replay() {
    let (_dir, mut s) = saved_setup();
    super::tests::read(&mut s, 71200);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute(
        "agent.apply",
        super::tests::exact_batch(71200, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
    let original = s.execute("agent.receipt", json!({"token":token(71200)})).unwrap();
    let p = publication(&mut s, 71201);
    let pubbed = s.execute(PUB, p.clone()).unwrap();
    let startup = recovery_startup(&s, &p, &pubbed, 71203);
    let mut cold = Session::default();
    let r = cold.open_identity_publication(&startup).unwrap();
    enable_recovered(&mut cold, &r);
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(71200)})).unwrap(), original);
    let current = cold.project.clone();
    super::tests::read(&mut cold, 71204);
    cold.execute("agent.undo", json!({"token":token(71204),"commit":token(71200)})).unwrap();
    assert!(!Arc::ptr_eq(&cold.project, &current));
    assert!(crate::project_workspace::identity(&cold.project).unwrap().is_some());
    super::tests::read(&mut cold, 71205);
    cold.execute("agent.redo", json!({"token":token(71205),"commit":token(71200),"undo":token(71204)})).unwrap();
    assert!(Arc::ptr_eq(&cold.project, &current));
}
#[test]
fn identity_recovery_refuses_foreign_hashes_changed_disk_and_nonfresh_sessions_without_mutation() {
    for mode in ["hash", "expected", "file", "record", "token", "path", "worker", "human"] {
        let (dir, mut s) = saved_setup();
        let p = publication(&mut s, 71300);
        let r = s.execute(PUB, p.clone()).unwrap();
        let mut startup = recovery_startup(&s, &p, &r, 71302);
        let mut cold = Session::default();
        match mode {
            "hash" => startup["sha256"] = json!("a".repeat(64)),
            "expected" => startup["expected"]["sourceGraphSha256"] = json!("a".repeat(64)),
            "file" => std::fs::write(s.path.as_ref().unwrap(), b"later saved human work").unwrap(),
            "record" => std::fs::write(dir.record(), b"{}").unwrap(),
            "token" => startup["activationToken"] = p["token"].clone(),
            "path" => {
                cold.path = s.path.clone();
            }
            "worker" => cold.worker_available = true,
            "human" => {
                cold.execute("file.newBin", json!({"name":"Keep my work"})).unwrap();
            }
            _ => unreachable!(),
        }
        let before = state(&cold);
        let project = cold.project.clone();
        assert!(cold.open_identity_publication(&startup).is_err(), "{mode}");
        assert_eq!(state(&cold), before, "{mode}");
        assert!(Arc::ptr_eq(&cold.project, &project));
        assert!(cold.workspace_update_status().is_none());
    }
}
#[test]
fn identity_recovery_only_native_origin_and_unchanged_cow_graph_can_certify_recovered_state() {
    let (_dir, mut s) = saved_setup();
    let p = publication(&mut s, 71400);
    let r = s.execute(PUB, p.clone()).unwrap();
    let startup = recovery_startup(&s, &p, &r, 71402);
    let mut cold = Session::default();
    let recovered = cold.open_identity_publication(&startup).unwrap();
    enable_recovered(&mut cold, &recovered);
    let read = record_request(&mut cold, 71403, p["expected"].clone(), true);
    assert_eq!(unchanged_record(&mut cold, read).unwrap()["liveState"], "recovered");
    cold.project = Arc::new((*cold.project).clone());
    let read = record_request(&mut cold, 71404, p["expected"].clone(), true);
    assert_eq!(unchanged_record(&mut cold, read).unwrap()["liveState"], "changed");
    let mut impostor = Session::default();
    impostor.project = cold.project.clone();
    impostor.path = cold.path.clone();
    impostor.state = cold.state.clone();
    impostor.revision = cold.revision;
    impostor.saved_revision = cold.saved_revision;
    impostor.worker_available = true;
    impostor.agent_operation_control().enable_transport();
    let read = record_request(&mut impostor, 71405, p["expected"].clone(), true);
    assert_eq!(unchanged_record(&mut impostor, read).unwrap()["liveState"], "recorded");
}
#[test]
fn identity_recovery_later_human_context_clipboard_preferences_and_project_changes_are_never_rolled_back() {
    for mode in ["view", "clipboard", "preferences", "project"] {
        let (dir, mut s) = saved_setup();
        let p = publication(&mut s, 71500);
        let r = s.execute(PUB, p.clone()).unwrap();
        let startup = recovery_startup(&s, &p, &r, 71502);
        let mut cold = Session::default();
        let recovered = cold.open_identity_publication(&startup).unwrap();
        enable_recovered(&mut cold, &recovered);
        match mode {
            "view" => cold.state.linked_selection = !cold.state.linked_selection,
            "clipboard" => {
                cold.state.clipboard = vec![(filmcraft_project::TrackKind::Video, 0, cold.active_sequence().unwrap().video_tracks[0].items[0].clone())]
            }
            "preferences" => cold.prefs.media.enable_proxies = !cold.prefs.media.enable_proxies,
            "project" => {
                cold.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
            }
            _ => unreachable!(),
        }
        let record = std::fs::read(dir.record()).unwrap();
        let read = record_request(&mut cold, 71503, p["expected"].clone(), true);
        assert_eq!(unchanged_record(&mut cold, read).unwrap()["liveState"], "changed", "{mode}");
        assert!(cold.open_identity_publication(&startup).is_err());
        assert_eq!(std::fs::read(dir.record()).unwrap(), record);
    }
}
#[test]
fn identity_recovery_cancel_never_unpauses_or_replays_and_activation_checks_new_epoch_and_disk() {
    for mode in ["cancel", "epoch", "disk"] {
        let (_dir, mut s) = saved_setup();
        let p = publication(&mut s, 71600);
        let r = s.execute(PUB, p.clone()).unwrap();
        let startup = recovery_startup(&s, &p, &r, 71602);
        let mut cold = Session::default();
        let recovered = cold.open_identity_publication(&startup).unwrap();
        cold.worker_available = true;
        cold.agent_operation_control().enable_transport();
        let f = &recovered["fence"];
        let mut activate = json!({"token":f["token"],"scope":f["scope"],"workspace":W,"update":MIGRATION,"nativeSession":f["nativeSession"]});
        if mode == "cancel" {
            cold.cancel_workspace_update().unwrap();
        } else if mode == "epoch" {
            activate["nativeSession"] = r["nativeSession"].clone();
        } else {
            std::fs::write(s.path.as_ref().unwrap(), b"new saved edit").unwrap();
        }
        let before = state(&cold);
        assert!(cold.execute("agent.activateWorkspaceUpdate", activate).is_err());
        assert_eq!(state(&cold), before);
        assert!(cold.is_display_only());
        assert!(cold.execute("file.newBin", json!({"name":"Cannot edit paused candidate"})).is_err());
    }
}
