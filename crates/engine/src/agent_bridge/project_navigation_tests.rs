use super::{project_clear, project_navigation as nav, project_plane, tests::token};
use crate::Session;
use filmcraft_project::ItemId;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
const W: &str = "4d9d252e-e851-48f2-814b-cc2f17ecc3d6";
const C: &str = "76f0b53f-b0db-4bfd-bcbc-54f101bd13dd";
fn scope() -> Value {
    json!({"projectId":"p_root_view","workspaceId":W})
}
fn setup() -> (Session, ItemId, ItemId, ItemId) {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated Root navigation","workspace":scope()})).unwrap();
    let mut ids = Vec::new();
    for name in ["Owning", "Owned target", "Foreign active"] {
        let value = s.execute("file.newSequence", json!({"name":name,"fps":25,"video":1,"audio":1})).unwrap();
        ids.push(ItemId(value["sequence"].as_u64().unwrap()));
    }
    let [a, b, c] = [ids[0], ids[1], ids[2]];
    Arc::make_mut(&mut s.project).item_mut(c).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
    s.path = Some(crate::temp_dir().join("generated-root-navigation.fcproj").to_string_lossy().into_owned());
    s.agent_operation_control().enable_transport();
    assert_eq!(crate::editorial_workspace::association(&s.project).unwrap().unwrap().scope.anchor_sequence, a.0);
    assert!(super::sequences::belongs(&s.project, b, &serde_json::from_value(json!({"projectId":"p_root_view","anchorSequence":a.0})).unwrap()).unwrap());
    (s, a, b, c)
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn request(s: &mut Session, n: usize, a: ItemId, b: ItemId) -> Value {
    let d = read(s, n);
    assert_eq!(d["capabilities"]["projectSequenceNavigation"], true);
    json!({"token":token(n),"scope":scope(),"sequenceScope":{"projectId":"p_root_view","anchorSequence":a.0},"item":b.0,"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"clipboardMarkers":s.state.clipboard_markers,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo})
}
fn validate(s: &Session, p: &Value, r: &Value) {
    nav::validate_history(
        &s.project,
        s.path.as_ref().unwrap(),
        &serde_json::from_value(scope()).unwrap(),
        p["token"].as_str().unwrap(),
        r["revision"].as_u64().unwrap(),
        r["previousSequence"].as_u64(),
        r["sequence"].as_u64(),
        p,
        r,
    )
    .unwrap();
}
#[test]
fn root_navigation_matches_human_view_from_foreign_active_without_media_or_history_authority() {
    let (mut s, a, b, c) = setup();
    assert_eq!(s.state.active_sequence, Some(c));
    let p = request(&mut s, 140000, a, b);
    let project = s.project.clone();
    let revision = s.revision;
    let saved = s.saved_revision;
    let undo = s.history.undo.len();
    let redo = s.history.redo.len();
    let mut human = Session { project: s.project.clone(), state: s.state.clone(), revision: s.revision, path: s.path.clone(), ..Session::default() };
    human.execute("sequence.open", json!({"item":b.0})).unwrap();
    let r = s.execute(nav::COMMAND, p.clone()).unwrap();
    assert_eq!(project_clear::editor_hash(&s.state).unwrap(), project_clear::editor_hash(&human.state).unwrap());
    assert_eq!(s.project, project);
    assert_eq!((s.revision, s.saved_revision, s.history.undo.len(), s.history.redo.len()), (revision, saved, undo, redo));
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["requiresFreshSequenceRead"], true);
    validate(&s, &p, &r);
    assert!(s.execute("agent.sourceFrames", json!({"token":token(140000),"scope":{"projectId":"p_root_view","anchorSequence":a.0},"frames":[0]})).is_err());
    read(&mut s, 140001);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(140001),"commit":token(140000)})).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn root_navigation_supports_closed_owned_views_but_never_creates_or_guesses_an_absent_sequence() {
    let (mut s, a, b, c) = setup();
    for id in [c, b, a] {
        s.execute("sequence.close", json!({"item":id.0})).unwrap();
    }
    assert!(s.state.active_sequence.is_none());
    let p = request(&mut s, 140100, a, b);
    let before = s.project.clone();
    s.execute(nav::COMMAND, p).unwrap();
    assert_eq!(s.state.active_sequence, Some(b));
    assert_eq!(s.project, before);
    let mut empty = Session::default();
    empty.execute("file.newProject", json!({"name":"Real absence","workspace":scope()})).unwrap();
    empty.path = s.path.clone();
    empty.agent_operation_control().enable_transport();
    let p = request(&mut empty, 140101, a, b);
    let before = state(&empty);
    assert!(empty.execute(nav::COMMAND, p).is_err());
    assert_eq!(state(&empty), before);
    assert_eq!(empty.project.sequences().count(), 0);
}
#[test]
fn root_navigation_rejects_hostile_targets_scope_aliases_and_non_root_ticket_purposes() {
    let (mut s, a, b, c) = setup();
    for i in 0..10 {
        let mut p = request(&mut s, 140200 + i, a, b);
        match i {
            0 => p["item"] = json!(0),
            1 => p["item"] = json!(-1),
            2 => p["item"] = json!(9007199254740992u64),
            3 => p["item"] = json!(c.0),
            4 => p["scope"]["workspaceId"] = json!(C),
            5 => p["sequenceScope"]["projectId"] = json!("foreign"),
            6 => p["sequenceScope"]["anchorSequence"] = json!(b.0),
            7 => p["beforeSha256"] = json!("0".repeat(64)),
            8 => p["beforeEditorSha256"] = json!("0".repeat(64)),
            _ => p["sourceValidation"] = json!("verified"),
        };
        let before = state(&s);
        assert!(s.execute(nav::COMMAND, p).is_err());
        assert_eq!(state(&s), before);
    }
    s.execute("sequence.open", json!({"item":a.0})).unwrap();
    let mut p = request(&mut s, 140220, a, b);
    s.execute("agent.snapshot", json!({"token":token(140221)})).unwrap();
    p["token"] = json!(token(140221));
    let before = state(&s);
    assert!(s.execute(nav::COMMAND, p).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn root_navigation_preserves_full_project_editor_and_both_clipboard_basis_guards() {
    for mode in ["project", "editor", "clipboard"] {
        let (mut s, a, b, _) = setup();
        let p = request(&mut s, 140300, a, b);
        match mode {
            "project" => {
                s.execute("file.newBin", json!({"name":"Later human bin"})).unwrap();
            }
            "editor" => {
                s.state.source_playhead = filmcraft_time::Tick(123);
            }
            _ => {
                s.state.clipboard_markers.push(
                    serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"Later clipboard","comment":"","color":"Iris","kind":"Comment"}))
                        .unwrap(),
                );
            }
        }
        let before = state(&s);
        assert!(s.execute(nav::COMMAND, p).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn root_navigation_stop_and_lost_reply_reconcile_one_view_without_overwriting_later_human_navigation() {
    let (mut s, a, b, c) = setup();
    let stopped = request(&mut s, 140400, a, b);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(140400)})).unwrap()["cancellationAccepted"], true);
    let before = state(&s);
    assert!(s.execute(nav::COMMAND, stopped.clone()).is_err());
    assert!(s.execute(nav::COMMAND, stopped).is_err());
    assert_eq!(state(&s), before);
    let p = request(&mut s, 140401, a, b);
    let r = s.execute(nav::COMMAND, p.clone()).unwrap();
    s.execute("sequence.open", json!({"item":c.0})).unwrap();
    let later = state(&s);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(140401)})).unwrap()["result"], r);
    assert_eq!(s.execute(nav::COMMAND, p).unwrap(), r);
    assert_eq!(state(&s), later);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(140401)})).unwrap()["cancellationAccepted"], false);
}
#[test]
fn root_navigation_opaque_proof_preserves_native_i64_clocks_and_refuses_rehashed_editor_or_authority_changes() {
    let (mut s, a, b, _) = setup();
    s.state.source_playhead = filmcraft_time::Tick(9007199254740993);
    let p = request(&mut s, 140500, a, b);
    let r = s.execute(nav::COMMAND, p.clone()).unwrap();
    assert!(r["viewProof"]["data"].as_str().unwrap().contains("9007199254740993"));
    let roundtrip: Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    validate(&s, &p, &roundtrip);
    for mode in ["clock", "after", "authority", "unknown"] {
        let mut bad = r.clone();
        if mode == "authority" {
            bad["mediaEvidence"] = json!(true);
        } else if mode == "unknown" {
            bad["viewProof"]["unexpected"] = json!(true);
        } else {
            let mut proof: Value = serde_json::from_str(bad["viewProof"]["data"].as_str().unwrap()).unwrap();
            if mode == "clock" {
                proof["beforeEditor"]["source_playhead"] = json!(9007199254740992u64);
            } else {
                proof["afterEditor"]["snapping"] = json!(!proof["afterEditor"]["snapping"].as_bool().unwrap());
            }
            let data = serde_json::to_string(&proof).unwrap();
            bad["viewProof"]["bytes"] = json!(data.len());
            bad["viewProof"]["sha256"] = json!(format!("{:x}", Sha256::digest(data.as_bytes())));
            bad["viewProof"]["data"] = json!(data);
        }
        assert!(
            nav::validate_history(
                &s.project,
                s.path.as_ref().unwrap(),
                &serde_json::from_value(scope()).unwrap(),
                p["token"].as_str().unwrap(),
                s.revision,
                r["previousSequence"].as_u64(),
                Some(b.0),
                &p,
                &bad
            )
            .is_err()
        );
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("filmcraft-root-view-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(std::fs::canonicalize(p).unwrap())
    }
    fn companion(&self) -> PathBuf {
        self.0.join(format!(".poema-history-{W}-{C}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
}
fn save(s: &mut Session, n: usize) -> Value {
    read(s, n);
    s.execute("agent.saveHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap()
}
fn reopened(s: &Session) -> Session {
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":s.path})).unwrap();
    fresh.agent_operation_control().enable_transport();
    fresh
}
fn restore(s: &mut Session, n: usize, saved: &Value) -> Result<Value, crate::EngineError> {
    read(s, n);
    s.execute("agent.restoreHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}))
}
#[test]
fn root_navigation_checkpoint_four_retains_exact_selected_receipt_across_cold_restore_without_view_replay() {
    let dir = Scratch::new();
    let (mut s, a, b, c) = setup();
    s.path = Some(dir.0.join("generated.fcproj").to_string_lossy().into_owned());
    persist(&mut s);
    let p = request(&mut s, 140600, a, b);
    let r = s.execute(nav::COMMAND, p.clone()).unwrap();
    persist(&mut s);
    let saved = save(&mut s, 140601);
    assert_eq!(saved["checkpoint"]["retainedTickets"], 1);
    let raw: Value = serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 4);
    let mut cold = reopened(&s);
    restore(&mut cold, 140602, &saved).unwrap();
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(140600)})).unwrap()["result"], r);
    cold.execute("sequence.open", json!({"item":c.0})).unwrap();
    let later = state(&cold);
    assert_eq!(cold.execute(nav::COMMAND, p).unwrap(), r);
    assert_eq!(state(&cold), later);
    assert_eq!(cold.state.active_sequence, Some(c));
    assert_ne!(b, c);
}
#[test]
fn root_navigation_checkpoint_four_refuses_forged_rehashed_views_and_legacy_version_aliases_before_restore() {
    let dir = Scratch::new();
    let (mut s, a, b, _) = setup();
    s.path = Some(dir.0.join("generated.fcproj").to_string_lossy().into_owned());
    persist(&mut s);
    let p = request(&mut s, 140700, a, b);
    s.execute(nav::COMMAND, p).unwrap();
    persist(&mut s);
    let saved = save(&mut s, 140701);
    let original = std::fs::read(dir.companion()).unwrap();
    for mode in ["version", "authority", "project", "scope", "proof"] {
        let mut value: Value = serde_json::from_slice(&original).unwrap();
        match mode {
            "version" => value["version"] = json!(2),
            "authority" => value["tickets"][0]["result"]["writeAuthority"] = json!(true),
            "project" => value["tickets"][0]["after"] = json!(999999),
            "scope" => value["tickets"][0]["project_scope"]["workspaceId"] = json!(C),
            _ => value["tickets"][0]["result"]["viewProof"]["sha256"] = json!("0".repeat(64)),
        };
        let raw = serde_json::to_vec(&value).unwrap();
        std::fs::write(dir.companion(), &raw).unwrap();
        let mut forged = saved.clone();
        forged["checkpoint"]["sha256"] = json!(format!("{:x}", Sha256::digest(&raw)));
        let mut cold = reopened(&s);
        let before = state(&cold);
        assert!(restore(&mut cold, 140702, &forged).is_err());
        assert_eq!(state(&cold), before);
    }
    std::fs::write(dir.companion(), original).unwrap();
}
