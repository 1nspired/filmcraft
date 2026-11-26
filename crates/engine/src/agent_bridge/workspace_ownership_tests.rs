use super::tests::{exact_batch, read, token};
use crate::{
    Session,
    editorial_workspace::{self, COMMAND, KEY},
};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

const WORKSPACE: &str = "6ae17621-32f1-4e2b-8940-aef47d1279a5";
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"First film","fps":24})).unwrap();
    s
}
fn params(s: &Session) -> Value {
    json!({"scope":{"projectId":"p_first","anchorSequence":s.state.active_sequence.unwrap().0},"workspaceId":WORKSPACE})
}
fn batch(s: &Session, n: usize) -> Value {
    exact_batch(n, vec![json!({"command":COMMAND,"params":params(s)})])
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal})
}
#[test]
fn workspace_association_uses_human_metadata_service_with_one_owned_history_step_and_no_cut_change() {
    let mut s = setup();
    let p = params(&s);
    let before = s.project.clone();
    let cut = s.active_sequence().unwrap().clone();
    let count = s.history.undo.len();
    read(&mut s, 61001);
    let receipt = s.execute("agent.apply", batch(&s, 61001)).unwrap();
    assert_eq!(s.active_sequence().unwrap(), &cut);
    assert_eq!(s.history.undo.len(), count + 1);
    let mut human = Session { project: before.clone(), state: s.state.clone(), ..Session::default() };
    let human_result = human.execute(COMMAND, p.clone()).unwrap();
    assert_eq!(human_result, receipt["results"][0]);
    assert_eq!(human.project, s.project);
    let mut stripped = (*s.project).clone();
    stripped.item_mut(s.state.active_sequence.unwrap()).unwrap().metadata.remove(KEY);
    assert_eq!(stripped, *before);
    let unchanged = state(&human);
    assert_eq!(human.execute(COMMAND, p.clone()).unwrap()["changed"], false);
    let mut after = state(&human);
    let mut unchanged = unchanged;
    // Native human commands journal even an idempotent invocation. That is
    // separate from project, view, revision and Undo/Redo mutation.
    after.as_object_mut().unwrap().remove("journal");
    unchanged.as_object_mut().unwrap().remove("journal");
    assert_eq!(after, unchanged);
    read(&mut s, 61002);
    let h = s.execute("agent.readHistory", json!({"token":token(61002),"commit":token(61001),"undo":null,"scope":p["scope"]})).unwrap();
    assert_eq!(h["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(61002),"commit":token(61001)})).unwrap();
    assert_eq!(s.project, before);
    read(&mut s, 61003);
    let h = s.execute("agent.readHistory", json!({"token":token(61003),"commit":token(61001),"undo":token(61002),"scope":p["scope"]})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    s.execute("agent.redo", json!({"token":token(61003),"commit":token(61001),"undo":token(61002)})).unwrap();
    assert_eq!(s.project, human.project);
}
#[test]
fn workspace_association_refuses_foreign_invalid_and_mixed_requests_without_destroying_human_redo() {
    let mut s = setup();
    let p = params(&s);
    s.execute(COMMAND, p.clone()).unwrap();
    for changed in [
        json!({"scope":{"projectId":"p_foreign","anchorSequence":p["scope"]["anchorSequence"]},"workspaceId":WORKSPACE}),
        json!({"scope":p["scope"],"workspaceId":"26d3c4b8-5f15-44b7-96b9-c99614547f0b"}),
        json!({"scope":p["scope"],"workspaceId":"invalid"}),
        json!({"scope":p["scope"],"workspaceId":WORKSPACE,"extra":true}),
    ] {
        let before = state(&s);
        assert!(s.execute(COMMAND, changed).is_err());
        assert_eq!(state(&s), before);
    }
    let mut clean = setup();
    clean.execute("metadata.set", json!({"item":clean.state.active_sequence.unwrap().0,"field":"Description","value":"human"})).unwrap();
    clean.undo().unwrap();
    read(&mut clean, 61004);
    let mut mixed = batch(&clean, 61004);
    mixed["exact"] = json!(false);
    let before = state(&clean);
    assert!(clean.execute("agent.apply", mixed).is_err());
    assert_eq!(state(&clean), before);
    clean.redo().unwrap();
    assert_eq!(clean.project.item(clean.state.active_sequence.unwrap()).unwrap().metadata["Description"], "human");
}
#[test]
fn workspace_association_rejects_legacy_bindings_and_corrupt_or_duplicate_metadata() {
    let mut s = setup();
    let id = s.state.active_sequence.unwrap();
    let p = params(&s);
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.insert("poema.binding.v1".into(), "{}".into());
    let before = state(&s);
    assert!(s.execute(COMMAND, p.clone()).is_err());
    assert_eq!(state(&s), before);
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.clear();
    s.execute(COMMAND, p).unwrap();
    let record = s.project.item(id).unwrap().metadata[KEY].clone();
    s.execute("file.newSequence", json!({"name":"Other","fps":24})).unwrap();
    let other = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(other).unwrap().metadata.insert(KEY.into(), record);
    assert!(editorial_workspace::association(&s.project).is_err());
    Arc::make_mut(&mut s.project).item_mut(other).unwrap().metadata.remove(KEY);
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.insert(KEY.into(), "{".into());
    assert!(editorial_workspace::association(&s.project).is_err());
}
#[test]
fn workspace_association_without_any_original_supports_exact_checkpoint_save_and_cold_restore() {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let dir = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("filmcraft-first-workspace-{:x}", u128::from_le_bytes(entropy)));
    fs::create_dir(&dir).unwrap();
    let mut s = setup();
    let p = params(&s);
    s.execute(COMMAND, p.clone()).unwrap();
    let file = dir.join("first.fcproj");
    s.execute("file.save", json!({"path":file})).unwrap();
    let basis = s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(61101),"scope":p["scope"]})).unwrap();
    assert_eq!(basis["writeAuthority"], false);
    let request =
        json!({"token":token(61101),"scope":p["scope"],"workspace":WORKSPACE,"checkpoint":"a4e22630-0a9c-47b2-836c-6dedaa412632","metadataBasis":true});
    let saved = s.execute("agent.saveHistoryCheckpoint", request).unwrap();
    let original = s.project.clone();
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":file})).unwrap();
    assert_eq!(fresh.project, original);
    read(&mut fresh, 61102);
    fresh.execute("agent.restoreHistoryCheckpoint",json!({"token":token(61102),"scope":p["scope"],"workspace":WORKSPACE,"checkpoint":saved["checkpoint"]["checkpoint"],"sha256":saved["checkpoint"]["sha256"]})).unwrap();
    assert_eq!(fresh.project, original);
    read(&mut fresh, 61103);
    let before = state(&fresh);
    let error=fresh.execute("agent.saveHistoryCheckpoint",json!({"token":token(61103),"scope":p["scope"],"workspace":"26d3c4b8-5f15-44b7-96b9-c99614547f0b","checkpoint":"a4e22630-0a9c-47b2-836c-6dedaa412632"})).unwrap_err();
    assert!(error.to_string().contains("different explicitly associated"));
    assert_eq!(state(&fresh), before);
    let foreign = super::sequences::Scope { project_id: "p_foreign".into(), anchor_sequence: p["scope"]["anchorSequence"].as_u64().unwrap() };
    assert!(!super::sequences::belongs(&fresh.project, fresh.state.active_sequence.unwrap(), &foreign).unwrap());
    fs::remove_dir_all(dir).unwrap();
}
