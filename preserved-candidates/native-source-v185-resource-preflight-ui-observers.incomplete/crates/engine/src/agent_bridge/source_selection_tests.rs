use super::assembly_tests;
use crate::Session;
use filmcraft_project::{ClipId, ItemId, MediaRef};
use serde_json::{Value, json};
use std::sync::Arc;
fn setup() -> (Session, u64) {
    let (mut s, item) = assembly_tests::fixture();
    s.path = Some("/generated/selection-copy.fcproj".into());
    Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().media =
        MediaRef::File { path: "/missing/selection-original.mxf".into() };
    s.state.selection.clear();
    s.state.project_selection.clear();
    s.state.source_item = None;
    (s, item)
}
fn request(s: &Session) -> Value {
    json!({"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"nonce":"source-selection-nonce-00000000001"})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn read(s: &mut Session) -> Value {
    let args = request(s);
    s.execute("agent.readSourceSelection", args).unwrap()
}
#[test]
fn source_selection_reads_distinct_real_native_planes_without_files_tickets_or_mutation() {
    let (mut s, item) = setup();
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].items[0].id;
    let a = q.audio_tracks[0].items[0].id;
    s.state.selection = vec![a, v];
    s.state.project_selection = vec![ItemId(item)];
    s.execute("source.open", json!({"item":item})).unwrap();
    let before = state(&s);
    let r = read(&mut s);
    assert_eq!(r["kind"], "native_source_selection");
    assert_eq!(r["nonce"], "source-selection-nonce-00000000001");
    assert_eq!(r["timeline"][0]["clip"], a.0);
    assert_eq!(r["timeline"][1]["clip"], v.0);
    assert_eq!(r["timeline"][0]["item"], item);
    assert_eq!(r["projectSelection"][0]["source"]["binding"]["assetId"], "a_source");
    assert_eq!(r["sourceMonitor"]["item"], item);
    assert_eq!(r["requiresSourceValidation"], true);
    assert_eq!(r["writeAuthority"], false);
    assert!(r.get("token").is_none());
    assert!(!r.to_string().contains("/missing/selection-original.mxf"));
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
}
#[test]
fn source_selection_empty_planes_never_guess_focus_or_fall_back_to_the_current_source() {
    let (mut s, item) = setup();
    let r = read(&mut s);
    assert_eq!(r["timeline"], json!([]));
    assert_eq!(r["projectSelection"], json!([]));
    assert_eq!(r["sourceMonitor"], Value::Null);
    s.state.source_item = Some(ItemId(item));
    let r = read(&mut s);
    assert!(r["timeline"].as_array().unwrap().is_empty());
    assert!(r["projectSelection"].as_array().unwrap().is_empty());
    assert_eq!(r["sourceMonitor"]["item"], item);
}
#[test]
fn source_selection_preserves_unavailable_and_non_media_project_targets_instead_of_dropping_them() {
    let (mut s, _) = setup();
    let seq = s.state.active_sequence.unwrap();
    s.state.project_selection = vec![seq, ItemId(8_000_000)];
    s.execute("source.open", json!({"item":seq.0})).unwrap();
    let before = state(&s);
    let r = read(&mut s);
    assert_eq!(r["projectSelection"].as_array().unwrap().len(), 2);
    for row in r["projectSelection"].as_array().unwrap() {
        assert_eq!(row["source"], Value::Null);
    }
    assert_eq!(r["sourceMonitor"]["source"], Value::Null);
    assert_eq!(state(&s), before);
}
#[test]
fn source_selection_observes_selection_changes_without_requiring_a_project_revision_or_changing_history() {
    let (mut s, item) = setup();
    let first = read(&mut s);
    let before = (s.revision, s.history.undo.len(), s.history.redo.len(), s.project.clone());
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.state.selection.push(clip);
    s.state.project_selection.push(ItemId(item));
    let second = read(&mut s);
    assert_eq!(first["revision"], second["revision"]);
    assert_eq!(first["nativeSession"], second["nativeSession"]);
    assert_ne!(first["timeline"], second["timeline"]);
    assert_ne!(first["projectSelection"], second["projectSelection"]);
    assert_eq!((s.revision, s.history.undo.len(), s.history.redo.len(), s.project.clone()), before);
}
#[test]
fn source_selection_hostile_scope_nonce_and_unknown_fields_refuse_without_publishing_authority() {
    let (mut s, _) = setup();
    let good = request(&s);
    let before = state(&s);
    for value in [
        Value::Null,
        json!({"scope":good["scope"],"nonce":"short"}),
        json!({"scope":good["scope"],"nonce":"x".repeat(129)}),
        json!({"scope":good["scope"],"nonce":"bad\\nonce0000000000000000000000000"}),
        json!({"scope":good["scope"],"nonce":good["nonce"],"selection":[1]}),
        json!({"scope":{"projectId":"p_test","anchorSequence":9000000},"nonce":good["nonce"]}),
    ] {
        assert!(s.execute("agent.readSourceSelection", value).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
}
#[test]
fn source_selection_duplicate_unsafe_excessive_and_dangling_selected_identities_refuse_as_a_whole() {
    for kind in [
        "duplicate_timeline",
        "duplicate_project",
        "unsafe_timeline",
        "unsafe_project",
        "unsafe_monitor",
        "excess_timeline",
        "excess_project",
        "dangling_timeline",
    ] {
        let (mut s, item) = setup();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        match kind {
            "duplicate_timeline" => s.state.selection = vec![clip, clip],
            "duplicate_project" => s.state.project_selection = vec![ItemId(item), ItemId(item)],
            "unsafe_timeline" => s.state.selection = vec![ClipId(u64::MAX)],
            "unsafe_project" => s.state.project_selection = vec![ItemId(u64::MAX)],
            "unsafe_monitor" => s.state.source_item = Some(ItemId(u64::MAX)),
            "excess_timeline" => s.state.selection = vec![clip; 4097],
            "excess_project" => s.state.project_selection = vec![ItemId(item); 4097],
            _ => s.state.selection = vec![ClipId(8_000_000)],
        };
        let before = state(&s);
        let args = request(&s);
        assert!(s.execute("agent.readSourceSelection", args).is_err(), "{kind}");
        assert_eq!(state(&s), before);
        assert!(s.agent_bridge.tickets.is_empty());
    }
}
#[test]
fn source_selection_duplicate_selected_clip_instances_or_an_excessive_sequence_refuse_complete_proof() {
    let (mut s, _) = setup();
    let seq = s.state.active_sequence.unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    s.state.selection = vec![clip.id];
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items.push(clip.clone());
    let before = state(&s);
    let args = request(&s);
    assert!(s.execute("agent.readSourceSelection", args).is_err());
    assert_eq!(state(&s), before);
    let (mut s, _) = setup();
    let seq = s.state.active_sequence.unwrap();
    s.state.selection.clear();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items = vec![clip; 100001];
    let before = s.project.clone();
    let args = request(&s);
    assert!(s.execute("agent.readSourceSelection", args).is_err());
    assert_eq!(s.project, before);
}

#[test]
fn source_selection_reports_global_original_binding_ambiguity_even_when_only_one_copy_is_selected() {
    let (mut s, item) = setup();
    let p = Arc::make_mut(&mut s.project);
    let original = p.item(ItemId(item)).unwrap().clone();
    let duplicate = p.add_item("Duplicate original", original.label, original.kind, None);
    p.item_mut(duplicate).unwrap().metadata = original.metadata;
    s.state.project_selection = vec![ItemId(item)];
    let before = state(&s);
    let r = read(&mut s);
    assert_eq!(r["projectSelection"][0]["bindingMatches"], 2);
    assert_eq!(state(&s), before);
}
