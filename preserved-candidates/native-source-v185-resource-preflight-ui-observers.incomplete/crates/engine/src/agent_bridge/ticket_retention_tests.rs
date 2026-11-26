use super::{TICKET_LIMIT, project_plane, tests::token};
use crate::Session;
use serde_json::{Value, json};
use std::sync::Arc;
const W: &str = "9380a267-22ca-4b52-9a3c-4a4287f238d1";
fn scope() -> Value {
    json!({"projectId":"p_ticket_retention","workspaceId":W})
}
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated retention","workspace":scope()})).unwrap();
    s.path = Some("/generated/retained.fcproj".into());
    s
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn add(s: &mut Session, n: usize) -> Value {
    let d = read(s, n);
    s.execute("agent.apply",json!({"token":token(n),"label":"Retained original bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":{"type":"create","name":format!("Bin {n}"),"parent":null}}}]})).unwrap()
}
fn receipt(s: &mut Session, n: usize) -> Value {
    s.execute("agent.receipt", json!({"token":token(n)})).unwrap()
}
#[test]
fn published_result_survives_read_pressure_with_original_owned_undo_redo() {
    let mut s = setup();
    let published = add(&mut s, 1);
    let head = s.project.clone();
    for n in 0..=TICKET_LIMIT {
        read(&mut s, 1000 + n);
    }
    assert_eq!(s.agent_bridge.tickets.len(), TICKET_LIMIT);
    assert_eq!(receipt(&mut s, 1), json!({"status":"committed","result":published}));
    read(&mut s, 2000);
    let h = s.execute("agent.readHistory", json!({"token":token(2000),"scope":scope(),"commit":token(1),"undo":null})).unwrap();
    assert_eq!(h["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(2000),"commit":token(1)})).unwrap();
    read(&mut s, 2001);
    let h = s.execute("agent.readHistory", json!({"token":token(2001),"scope":scope(),"commit":token(1),"undo":token(2000)})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    s.execute("agent.redo", json!({"token":token(2001),"commit":token(1),"undo":token(2000)})).unwrap();
    assert_eq!(s.project, head);
}
#[test]
fn unused_read_expires_while_newest_read_and_published_result_remain_bounded() {
    let mut s = setup();
    let published = add(&mut s, 2);
    read(&mut s, 3);
    for n in 0..=TICKET_LIMIT {
        read(&mut s, 3000 + n);
    }
    assert_eq!(receipt(&mut s, 3)["status"], "unknown");
    assert_eq!(receipt(&mut s, 2), json!({"status":"committed","result":published}));
    assert_eq!(s.agent_bridge.tickets.back().unwrap().token, token(3000 + TICKET_LIMIT));
    assert_eq!(s.agent_bridge.tickets.len(), TICKET_LIMIT);
}
#[test]
fn retained_result_never_authorizes_undo_over_a_later_human_project() {
    let mut s = setup();
    let published = add(&mut s, 4);
    for n in 0..=TICKET_LIMIT {
        read(&mut s, 4000 + n);
    }
    s.execute("file.newBin", json!({"name":"Later human bin"})).unwrap();
    let head = s.project.clone();
    let history = s.history.undo.clone();
    read(&mut s, 5000);
    let h = s.execute("agent.readHistory", json!({"token":token(5000),"scope":scope(),"commit":token(4),"undo":null})).unwrap();
    assert_eq!(h["undoAvailable"], false);
    assert!(s.execute("agent.undo", json!({"token":token(5000),"commit":token(4)})).is_err());
    assert!(Arc::ptr_eq(&s.project, &head));
    assert_eq!(s.history.undo.len(), history.len());
    assert_eq!(receipt(&mut s, 4), json!({"status":"committed","result":published}));
}
#[test]
fn all_published_window_still_ages_out_oldest_result_without_evicting_new_purpose() {
    let mut s = setup();
    for n in 0..TICKET_LIMIT + 2 {
        add(&mut s, 6000 + n);
        assert!(s.agent_bridge.tickets.len() <= TICKET_LIMIT);
    }
    assert_eq!(receipt(&mut s, 6000)["status"], "unknown");
    assert_eq!(receipt(&mut s, 6000 + TICKET_LIMIT + 1)["status"], "committed");
    let d = read(&mut s, 7000);
    assert_eq!(s.agent_bridge.tickets.back().unwrap().token, token(7000));
    assert_eq!(s.agent_bridge.tickets.len(), TICKET_LIMIT);
    s.execute("agent.apply",json!({"token":token(7000),"label":"Fresh purpose remains usable","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":{"type":"create","name":"Newest live purpose","parent":null}}}]})).unwrap();
    assert_eq!(receipt(&mut s, 7000)["status"], "committed");
}

#[test]
fn owned_redo_survives_read_pressure_after_undo_without_replaying_the_commit() {
    let mut s = setup();
    let published = add(&mut s, 8000);
    let edited = s.project.clone();
    read(&mut s, 8001);
    let undone = s.execute("agent.undo", json!({"token":token(8001),"commit":token(8000)})).unwrap();
    let original = s.project.clone();
    let history = serde_json::to_value((&s.history.undo, &s.history.redo)).unwrap();
    let revision = s.revision;
    for n in 0..=TICKET_LIMIT {
        read(&mut s, 8100 + n);
    }
    assert!(Arc::ptr_eq(&s.project, &original));
    assert_eq!(serde_json::to_value((&s.history.undo, &s.history.redo)).unwrap(), history);
    assert_eq!(s.revision, revision);
    assert_eq!(receipt(&mut s, 8000), json!({"status":"committed","result":published}));
    assert_eq!(receipt(&mut s, 8001), json!({"status":"committed","result":undone}));
    read(&mut s, 8500);
    let h = s.execute("agent.readHistory", json!({"token":token(8500),"scope":scope(),"commit":token(8000),"undo":token(8001)})).unwrap();
    assert_eq!(h["undoAvailable"], false);
    assert_eq!(h["redoAvailable"], true);
    s.execute("agent.redo", json!({"token":token(8500),"commit":token(8000),"undo":token(8001)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &edited));
}
