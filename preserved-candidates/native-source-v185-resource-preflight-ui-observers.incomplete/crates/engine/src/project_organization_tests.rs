use crate::Session;
use filmcraft_project::BinId;
use serde_json::{Value, json};
use std::sync::Arc;

fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,
        "undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}
fn with_redo() -> Session {
    let mut s = Session::default();
    s.execute("file.newBin", json!({"name":"Human redo sentinel"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.history.redo.is_empty());
    s
}
fn reject(s: &mut Session, command: &str, args: Value) {
    let before = state(s);
    let result = s.execute(command, args.clone());
    assert!(result.is_err(), "accepted {command} {args}: {result:?}");
    assert_eq!(state(s), before, "rejection changed project/view/history");
}
#[test]
fn bin_creation_missing_parent_never_redirects_to_root_or_consumes_redo() {
    let mut s = with_redo();
    for parent in [json!(999999), json!(-1), json!(1.5), json!("1"), json!(true)] {
        reject(&mut s, "file.newBin", json!({"name":"Proposal","parent":parent}));
    }
}
#[test]
fn bin_creation_id_exhaustion_is_an_error_not_a_panic() {
    let mut s = with_redo();
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let before = state(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("file.newBin", json!({"name":"No ID left"}))));
    assert!(result.is_ok(), "exhausted IDs must report an error");
    let error = result.unwrap().unwrap_err();
    assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }), "last-resort panic containment is not input validation");
    assert_eq!(state(&s), before);
}
#[test]
fn bin_creation_reserves_above_existing_ids_without_renumbering() {
    let mut s = Session::default();
    Arc::make_mut(&mut s.project).root.id = BinId(5_000_000);
    let result = s.execute("file.newBin", json!({"name":"New child"})).unwrap();
    assert_eq!(result["bin"], 5_000_001);
    assert_eq!(s.project.root.id, BinId(5_000_000));
    assert_eq!(s.project.next_id, 5_000_002);
}
#[test]
fn bin_names_reject_hostile_explicit_values_without_silent_defaults() {
    let mut s = with_redo();
    for name in [json!(null), json!(false), json!(1), json!(""), json!("  \t"), json!("a\0b"), json!("x".repeat(1025))] {
        reject(&mut s, "file.newBin", json!({"name":name}));
    }
}
#[test]
fn bin_creation_parent_and_rename_preserve_one_step_history_and_saved_reopen() {
    let mut s = Session::default();
    let root = s.project.root.id;
    for args in [json!({}), json!({"name":"一日の素材","parent":null}), json!({"name":"Explicit root","parent":root.0})] {
        let before = s.project.clone();
        let undo = s.history.undo.len();
        let bin = s.execute("file.newBin", args).unwrap()["bin"].as_u64().unwrap();
        assert_eq!(s.history.undo.len(), undo + 1);
        assert!(s.project.root.find_bin(BinId(bin)).is_some());
        let after = s.project.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
        s.execute("edit.redo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &after));
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    }
}

#[test]
fn bin_shared_services_match_human_commands_and_preserve_other_project_payloads() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut expected = (*s.project).clone();
    let before_items = s.project.items.clone();
    let bin = crate::project_organization::create(&mut expected, " 素材 ", None).unwrap();
    let result = s.execute("file.newBin", json!({"name":" 素材 "})).unwrap();
    assert_eq!(result["bin"], bin.0);
    assert_eq!(s.project.to_json(), expected.to_json());
    assert_eq!(s.project.items, before_items);
    crate::project_organization::rename(&mut expected, bin, "  Human camera A  ").unwrap();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    s.execute("project.renameBin", json!({"bin":bin.0,"name":"  Human camera A  "})).unwrap();
    assert_eq!(s.project.to_json(), expected.to_json());
    assert_eq!(s.project.items, before_items);
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
}
#[test]
fn bin_rename_refuses_bad_targets_and_names_but_needs_no_new_identity() {
    let mut s = Session::default();
    let id = s.execute("file.newBin", json!({"name":"Camera"})).unwrap()["bin"].as_u64().unwrap();
    s.execute("file.newBin", json!({"name":"Human redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let root = s.project.root.id.0;
    for target in [json!(999999), json!(root), json!(null), json!(-1), json!(true), json!("1")] {
        reject(&mut s, "project.renameBin", json!({"bin":target,"name":"Wrong target"}));
    }
    for name in [json!(null), json!(false), json!(1), json!(" "), json!("a\0b"), json!("x".repeat(1025))] {
        reject(&mut s, "project.renameBin", json!({"bin":id,"name":name}));
    }
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    s.execute("project.renameBin", json!({"bin":id,"name":"Valid rename"})).unwrap();
    assert_eq!(s.project.next_id, u64::MAX);
    assert_eq!(s.project.root.find_bin(BinId(id)).unwrap().name, "Valid rename");
}
#[test]
fn bin_tree_depth_and_duplicate_identity_refuse_before_native_publication() {
    let mut s = with_redo();
    let root = s.project.root.id;
    Arc::make_mut(&mut s.project).root.children.push(filmcraft_project::BinEntry::Bin(filmcraft_project::Bin {
        id: root,
        name: "Ambiguous".into(),
        children: Vec::new(),
    }));
    reject(&mut s, "file.newBin", json!({"name":"No ambiguity"}));
    reject(&mut s, "project.renameBin", json!({"bin":root.0,"name":"No ambiguity"}));
    let mut s = Session::default();
    let mut parent = s.project.root.id;
    for i in 0..63 {
        parent = Arc::make_mut(&mut s.project).add_bin(&format!("Level {i}"), Some(parent));
    }
    let last = s.execute("file.newBin", json!({"name":"Depth 64 is supported","parent":parent.0})).unwrap()["bin"].as_u64().unwrap();
    reject(&mut s, "file.newBin", json!({"name":"Too deep","parent":last}));
    s.execute("project.renameBin", json!({"bin":last,"name":"Deepest rename"})).unwrap();
}

#[test]
fn selection_group_uses_one_original_gesture_first_parent_global_default_and_exact_owned_payloads() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let parent = s.execute("file.newBin", json!({"name":"Parent"})).unwrap()["bin"].as_u64().unwrap();
    s.execute("file.newBin", json!({"name":"Bin 01","parent":parent})).unwrap();
    let other = s.execute("file.newBin", json!({"name":"Bin 03"})).unwrap()["bin"].as_u64().unwrap();
    let item = *s.project.items.keys().next().unwrap();
    s.execute("project.moveToBin", json!({"items":[item.0],"bin":parent})).unwrap();
    s.state.project_selection = vec![item, filmcraft_project::ItemId(other)];
    let before = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let mut expected = (*before).clone();
    let (bin, moved) = crate::project_organization::group_selection(&mut expected, &s.state.project_selection, None, true).unwrap();
    let result = s.execute("file.newBinFromSelection", json!({})).unwrap();
    assert_eq!(result, json!({"bin":bin.0,"moved":moved}));
    assert_eq!(moved, 2);
    assert_eq!(s.project.to_json(), expected.to_json());
    assert!(
        s.project.root.find_bin(BinId(parent)).unwrap().children.iter().any(|entry| matches!(entry,filmcraft_project::BinEntry::Bin(child) if child.id==bin))
    );
    assert_eq!(s.project.root.find_bin(bin).unwrap().name, "Bin 02");
    assert_eq!(s.project.items, before.items);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), after.to_json());
}

#[test]
fn selection_group_exact_failures_leave_no_empty_bin_and_human_skip_behavior_remains_one_step() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let a = s.execute("file.newBin", json!({"name":"Outer"})).unwrap()["bin"].as_u64().unwrap();
    let child = s.execute("file.newBin", json!({"name":"Child","parent":a})).unwrap()["bin"].as_u64().unwrap();
    let item = *s.project.items.keys().next().unwrap();
    s.execute("project.moveToBin", json!({"items":[item.0],"bin":a})).unwrap();
    for entries in [vec![], vec![a, a], vec![999999], vec![item.0, a], vec![s.project.root.id.0]] {
        let mut candidate = (*s.project).clone();
        let before = candidate.to_json();
        assert!(
            crate::project_organization::group_selection(
                &mut candidate,
                &entries.into_iter().map(filmcraft_project::ItemId).collect::<Vec<_>>(),
                Some("Exact"),
                true
            )
            .is_err()
        );
        assert_eq!(candidate.to_json(), before);
    }
    for name in [json!(false), json!(1), json!(""), json!(" "), json!("x\0y"), json!("x".repeat(1025))] {
        reject(&mut s, "file.newBinFromSelection", json!({"items":[child],"name":name}));
    }
    let undo = s.history.undo.len();
    let result = s.execute("file.newBinFromSelection", json!({"items":[item.0,a],"name":"Human skip"})).unwrap();
    assert_eq!(result["moved"], 2, "retain the original human requested-selection count");
    assert_eq!(s.history.undo.len(), undo + 1);
    let before = state(&s);
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let exhausted = state(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("file.newBinFromSelection", json!({"items":[child]}))));
    assert!(result.is_ok());
    let error = result.unwrap().unwrap_err();
    assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }));
    assert_eq!(state(&s), exhausted);
    assert_ne!(before, exhausted);
}
