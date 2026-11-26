use super::tests::{exact_batch, read, setup, token};
use crate::Session;
use filmcraft_project::{BinEntry, ItemId};
use serde_json::{Value, json};

fn fixture() -> (Session, ItemId) {
    let mut s = setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let anchor = s.state.active_sequence.unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    s.execute("agent.bindSource", json!({"item":item.0,"binding":{"projectId":"p_test","assetId":"a_test","mediaId":"a_test:original","role":"original"}}))
        .unwrap();
    (s, anchor)
}
fn scope(anchor: ItemId) -> Value {
    json!({"projectId":"p_test","anchorSequence":anchor.0})
}
fn create(anchor: ItemId, name: &str) -> Value {
    json!({"command":"agent.createSequence","params":{"name":name,"scope":scope(anchor)}})
}
fn select(s: &mut Session, n: usize, anchor: ItemId, item: ItemId) -> Value {
    read(s, n);
    s.execute("agent.selectSequence", json!({"token":token(n),"item":item.0,"scope":scope(anchor)})).unwrap()
}

#[test]
fn create_preserves_existing_project_inherits_settings_and_undo_restores_previous_sequence() {
    let (mut s, anchor) = fixture();
    let before = s.project.clone();
    let settings = s.active_sequence().unwrap().settings.clone();
    read(&mut s, 1);
    let request = exact_batch(1, vec![create(anchor, "Proposal story")]);
    let receipt = s.execute("agent.apply", request.clone()).unwrap();
    let child = s.state.active_sequence.unwrap();
    assert_ne!(child, anchor);
    assert_eq!(receipt["action"], "sequence_create");
    assert_eq!(s.project.sequence(child).unwrap().settings, settings);
    assert_eq!(s.project.sequence(child).unwrap().all_tracks().count(), 2);
    assert!(s.project.sequence(child).unwrap().all_tracks().all(|t| t.items.is_empty()));
    assert_eq!(s.project.item(child).unwrap().metadata[filmcraft_project::clip_identity::TRACKING_KEY], "1");
    let mut stripped = (*s.project).clone();
    stripped.items.remove(&child);
    stripped.root.children.retain(|c| !matches!(c,BinEntry::Item(id) if *id==child));
    stripped.next_id = before.next_id;
    assert_eq!(stripped, *before);
    assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
    read(&mut s, 2);
    let undo = s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
    assert_eq!(s.project, before);
    assert_eq!(s.state.active_sequence, Some(anchor));
    assert_eq!(undo["sequence"], anchor.0);
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.project.sequence(child).is_some());
}
#[test]
fn selection_has_receipt_without_project_dirty_revision_or_history_changes() {
    let (mut s, anchor) = fixture();
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![create(anchor, "Story")])).unwrap();
    let child = s.state.active_sequence.unwrap();
    let before = s.project.clone();
    let revision = s.revision;
    let history = s.history.undo.len();
    let saved = s.saved_revision;
    let receipt = select(&mut s, 2, anchor, anchor);
    assert_eq!(receipt["status"], "selected");
    assert_eq!(s.project, before);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.undo.len(), history);
    assert_eq!(s.saved_revision, saved);
    assert_eq!(s.execute("agent.selectSequence", json!({"token":token(2),"item":anchor.0,"scope":scope(anchor)})).unwrap(), receipt);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(2)})).unwrap()["result"], receipt);
    select(&mut s, 3, anchor, child);
    read(&mut s, 4);
    s.execute("agent.undo", json!({"token":token(4),"commit":token(1)})).unwrap();
    assert_eq!(s.state.active_sequence, Some(anchor));
}
#[test]
fn hostile_creation_and_foreign_navigation_refuse_without_changes() {
    let (mut s, anchor) = fixture();
    let before = s.project.clone();
    let revision = s.revision;
    for (n, name) in ["", " leading", "line\nbreak"].iter().enumerate() {
        read(&mut s, n + 1);
        assert!(s.execute("agent.apply", exact_batch(n + 1, vec![create(anchor, name)])).is_err());
    }
    read(&mut s, 10);
    assert!(s.execute("agent.apply", exact_batch(10, vec![create(anchor, "Story"), create(anchor, "Other")])).is_err());
    let mut nonexact = exact_batch(10, vec![create(anchor, "Story")]);
    nonexact["exact"] = json!(false);
    assert!(s.execute("agent.apply", nonexact).is_err());
    let mut foreign = create(anchor, "Story");
    foreign["params"]["scope"]["projectId"] = json!("foreign");
    assert!(s.execute("agent.apply", exact_batch(10, vec![foreign])).is_err());
    assert!(s.execute("agent.selectSequence", json!({"token":token(10),"item":999999,"scope":scope(anchor)})).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.revision, revision);
    s.execute("agent.apply", exact_batch(10, vec![create(anchor, "Story")])).unwrap();
    let created = s.project.clone();
    read(&mut s, 11);
    assert!(s.execute("agent.apply", exact_batch(11, vec![create(anchor, "story")])).is_err());
    assert_eq!(s.project, created);
}
#[test]
fn stale_selection_and_creation_cannot_follow_changed_editor_context() {
    let (mut s, anchor) = fixture();
    read(&mut s, 1);
    s.state.linked_selection = !s.state.linked_selection;
    let before = s.project.clone();
    assert!(s.execute("agent.apply", exact_batch(1, vec![create(anchor, "Story")])).is_err());
    assert!(s.execute("agent.selectSequence", json!({"token":token(1),"item":anchor.0,"scope":scope(anchor)})).is_err());
    assert_eq!(s.project, before);
}
#[test]
fn recovery_creation_is_one_undoable_step() {
    let (mut s, anchor) = fixture();
    let before = s.project.clone();
    let history = s.history.undo.len();
    s.execute("agent.replayBatch", json!({"label":"New story","operations":[create(anchor,"Recovered story")],"exact":true})).unwrap();
    assert_eq!(s.history.undo.len(), history + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    assert_eq!(s.state.active_sequence, Some(anchor));
}

#[test]
fn existing_unadopted_sequence_is_not_a_navigation_target() {
    let (mut s, anchor) = fixture();
    let result = s.execute("file.newSequence", json!({"name":"Unadopted"})).unwrap();
    let foreign = result["sequence"].as_u64().unwrap();
    s.execute("sequence.open", json!({"item":anchor.0})).unwrap();
    let before = s.project.clone();
    let revision = s.revision;
    read(&mut s, 1);
    assert!(s.execute("agent.selectSequence", json!({"token":token(1),"item":foreign,"scope":scope(anchor)})).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.revision, revision);
    assert_eq!(s.state.active_sequence, Some(anchor));
}

#[test]
fn creation_rejects_exhausted_allocation_counter_and_unknown_fields() {
    let (mut s, anchor) = fixture();
    std::sync::Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let before = s.project.clone();
    read(&mut s, 1);
    assert!(s.execute("agent.apply", exact_batch(1, vec![create(anchor, "Story")])).is_err());
    let mut op = create(anchor, "Story");
    op["params"]["path"] = json!("/not-allowed");
    assert!(s.execute("agent.apply", exact_batch(1, vec![op])).is_err());
    assert_eq!(s.project, before);
}

#[test]
fn new_sequences_inherit_only_existing_effect_tracking_without_read_time_changes() {
    use filmcraft_project::clip_identity::EFFECT_TRACKING_KEY;
    for enabled in [false, true] {
        let (mut s, anchor) = fixture();
        if enabled {
            s.execute("poema.trackEffects", json!({})).unwrap();
        }
        let before = s.project.clone();
        read(&mut s, 1);
        s.execute("agent.apply", exact_batch(1, vec![create(anchor, "New graphics sequence")])).unwrap();
        let child = s.state.active_sequence.unwrap();
        assert_eq!(s.project.item(child).unwrap().metadata.get(EFFECT_TRACKING_KEY).map(String::as_str), enabled.then_some("1"));
        let created = s.project.clone();
        let revision = s.revision;
        let history = s.history.undo.len();
        read(&mut s, 2);
        let page = s.execute("agent.readGraphics", json!({"token":token(2),"scope":scope(anchor),"target":{"kind":"clips"},"offset":0,"limit":1})).unwrap();
        assert_eq!(page["effectTracking"], enabled);
        assert_eq!(page["total"], 0);
        assert_eq!(s.project, created);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), history);
        s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
        assert_eq!(s.project, before);
        assert_eq!(s.state.active_sequence, Some(anchor));
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.project, created);
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *created);
    }
}

#[test]
fn new_sequence_graphics_uses_inherited_tracking_and_preserves_the_parent() {
    let (mut s, anchor) = fixture();
    s.execute("poema.trackEffects", json!({})).unwrap();
    let parent = s.project.item(anchor).unwrap().clone();
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![create(anchor, "Graphic working sequence")])).unwrap();
    let created = s.project.clone();
    let track = s.active_sequence().unwrap().video_tracks[0].id;
    read(&mut s, 2);
    let page = s.execute("agent.readGraphics", json!({"token":token(2),"scope":scope(anchor),"target":{"kind":"clips"},"offset":0,"limit":1})).unwrap();
    let mut request = exact_batch(
        2,
        vec![json!({"command":"agent.editGraphics","params":{"scope":scope(anchor),"beforeGraphicsSha256":page["collectionSha256"],
        "edit":{"type":"create","track":track.0,"frame":0,"durationFrames":24,"content":{"kind":"shape","shape":"rectangle","position":[100,100],"size":[100,100],"points":[]}}}})],
    );
    request["authority"] = json!("document_state");
    let receipt = s.execute("agent.apply", request).unwrap();
    assert_eq!(receipt["action"], "graphics_edit");
    assert_eq!(receipt["results"][0]["beforeCount"], 0);
    assert_eq!(receipt["results"][0]["afterCount"], 1);
    assert_eq!(s.project.item(anchor).unwrap(), &parent);
    let authored = s.project.clone();
    read(&mut s, 3);
    s.execute("agent.undo", json!({"token":token(3),"commit":token(2)})).unwrap();
    assert_eq!(s.project, created);
    read(&mut s, 4);
    s.execute("agent.redo", json!({"token":token(4),"commit":token(2),"undo":token(3)})).unwrap();
    assert_eq!(s.project, authored);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *authored);
}
