use crate::{Session, search_organization as search};
use filmcraft_project::{Bin, BinEntry, BinId, FindOp, FindQuery, FindRow, ItemId};
use serde_json::{Value, json};
use std::sync::Arc;
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}
fn redo() -> Session {
    let mut s = Session::default();
    s.execute("file.newBin", json!({"name":"Human redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s
}
fn reject(s: &mut Session, command: &str, args: Value) {
    let before = state(s);
    let result = s.execute(command, args.clone());
    assert!(result.is_err(), "accepted {command} {args}: {result:?}");
    assert_eq!(state(s), before, "failed search mutated history/editor/project");
}
#[test]
fn saved_search_creation_exhaustion_never_panics_or_uses_last_resort_containment() {
    let mut s = redo();
    Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let before = state(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("file.newSearchBin", json!({"text":"camera"}))));
    assert!(result.is_ok());
    let error = result.unwrap().unwrap_err();
    assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }));
    assert_eq!(state(&s), before);
}
#[test]
fn saved_search_explicit_malformed_rows_names_and_flags_do_not_fall_back_to_defaults() {
    let mut s = redo();
    for args in [
        json!({"text":"x","name":null}),
        json!({"text":"x","name":false}),
        json!({"text":"x","name":"  "}),
        json!({"text":"x","name":"x".repeat(1025)}),
        json!({"text":"x","name":"a\0b"}),
        json!({"rows":null}),
        json!({"rows":"x"}),
        json!({"rows":[false]}),
        json!({"rows":[{"text":12}]}),
        json!({"text":"x","matchAll":"false"}),
        json!({"text":"x","caseSensitive":0}),
        json!({"text":false}),
        json!({"text":"x","column":null}),
        json!({"text":"x","operator":false}),
        json!({"text":"x","op":"near"}),
        json!({"text":"x","operator":"matches","op":"contains"}),
        json!({"rows":[{"text":"x","extra":true}]}),
        json!({"rows":[{"text":"x"}],"text":"other"}),
        json!({"text":"x","column":"c".repeat(257)}),
        json!({"name":"long query","text":"x".repeat(4097)}),
        json!({"name":"nul query","text":"x\0y"}),
        json!({"rows":vec![json!({"text":"x"});33]}),
    ] {
        reject(&mut s, "file.newSearchBin", args);
    }
}
#[test]
fn saved_search_creation_reserves_above_legacy_ids_without_renumbering() {
    let mut s = Session::default();
    Arc::make_mut(&mut s.project).root.id = BinId(5_000_000);
    let result = s.execute("file.newSearchBin", json!({"text":"camera"})).unwrap();
    assert_eq!(result["bin"], 5_000_001);
    assert_eq!(s.project.next_id, 5_000_002);
    assert_eq!(s.project.root.id, BinId(5_000_000));
}
#[test]
fn saved_search_human_dialog_empty_name_uses_the_query_title_and_unicode_byte_bounds() {
    let mut s = Session::default();
    let r = s.execute("file.newSearchBin", json!({"name":"","column":"All","operator":"contains","text":"ocean"})).unwrap();
    assert_eq!(r["name"], "ocean");
    let name = "界".repeat(341);
    let r = s.execute("file.newSearchBin", json!({"name":name,"text":"x"})).unwrap();
    assert_eq!(r["name"].as_str().unwrap().len(), 1023);
    reject(&mut s, "file.newSearchBin", json!({"name":"界".repeat(342),"text":"x"}));
}
#[test]
fn saved_search_shared_create_edit_delete_match_human_services_and_preserve_other_payloads() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let items = s.project.items.clone();
    let tree = s.project.root.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let mut expected = (*s.project).clone();
    let query = FindQuery::simple("Name", FindOp::Contains, "ocean");
    let bin = search::create(&mut expected, Some(" Ocean "), query).unwrap();
    let result = s.execute("file.newSearchBin", json!({"name":" Ocean ","column":"Name","text":"ocean"})).unwrap();
    assert_eq!(result["bin"], bin.id.0);
    assert_eq!(s.project.to_json(), expected.to_json());
    let query = FindQuery {
        rows: vec![FindRow { column: "Name".into(), op: FindOp::DoesNotContain, text: "NO_MATCH".into() }],
        match_all: false,
        case_sensitive: true,
    };
    search::update(&mut expected, bin.id, Some("Camera metadata"), Some(query)).unwrap();
    s.execute("project.editSearchBin",json!({"bin":bin.id.0,"name":"Camera metadata","rows":[{"column":"Name","operator":"doesNotContain","text":"NO_MATCH"}],"matchAll":false,"caseSensitive":true})).unwrap();
    assert_eq!(s.project.to_json(), expected.to_json());
    search::delete(&mut expected, bin.id).unwrap();
    s.execute("project.deleteSearchBin", json!({"bin":bin.id.0})).unwrap();
    assert_eq!(s.project.to_json(), expected.to_json());
    assert_eq!(s.project.items, items);
    assert_eq!(s.project.root, tree);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
}
#[test]
fn saved_search_each_human_gesture_is_one_undo_redo_step_with_exact_saved_format_reopen() {
    let mut s = Session::default();
    let mut id = 0;
    for command in ["file.newSearchBin", "project.editSearchBin", "project.deleteSearchBin"] {
        let before = s.project.clone();
        let count = s.history.undo.len();
        let args = match command {
            "file.newSearchBin" => json!({"text":"camera"}),
            "project.editSearchBin" => json!({"bin":id,"name":"Only metadata"}),
            _ => json!({"bin":id}),
        };
        let r = s.execute(command, args).unwrap();
        if command == "file.newSearchBin" {
            id = r["bin"].as_u64().unwrap();
        }
        let after = s.project.clone();
        assert_eq!(s.history.undo.len(), count + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
        s.execute("edit.redo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &after));
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    }
}
#[test]
fn saved_search_update_rejects_malformed_partial_options_and_keeps_explicit_empty_query() {
    let mut s = Session::default();
    let id = s.execute("file.newSearchBin", json!({"text":"camera"})).unwrap()["bin"].as_u64().unwrap();
    s.execute("file.newBin", json!({"name":"Redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    for mut args in [
        json!({"name":null}),
        json!({"name":""}),
        json!({"matchAll":false}),
        json!({"column":"Name"}),
        json!({"rows":false}),
        json!({"rows":[{"text":"x","column":false}]}),
        json!({"rows":[{"text":"x"}],"caseSensitive":null}),
    ] {
        args["bin"] = json!(id);
        reject(&mut s, "project.editSearchBin", args);
    }
    s.execute("project.editSearchBin", json!({"bin":id,"rows":[],"matchAll":true,"caseSensitive":false})).unwrap();
    let r = s.execute("project.searchBinItems", json!({"bin":id})).unwrap();
    assert_eq!(r["items"], json!([]));
    assert_eq!(r["query"]["rows"], json!([]));
}
#[test]
fn saved_search_query_results_share_find_operators_metadata_case_and_live_project_order() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let p = Arc::make_mut(&mut s.project);
    let first = *p.items.keys().next().unwrap();
    p.item_mut(first).unwrap().metadata.insert("Scene".into(), "CiTy Road".into());
    for op in FindOp::ALL {
        for all in [true, false] {
            for case in [true, false] {
                let q = FindQuery {
                    rows: vec![
                        FindRow { column: "Name".into(), op, text: "ocean".into() },
                        FindRow { column: "Scene".into(), op: FindOp::Contains, text: "city".into() },
                    ],
                    match_all: all,
                    case_sensitive: case,
                };
                assert_eq!(search::results(p, &q).unwrap(), q.find(p));
            }
        }
    }
    let id = s.execute("file.newSearchBin", json!({"column":"Scene","text":"city"})).unwrap()["bin"].as_u64().unwrap();
    assert!(s.execute("project.searchBinItems", json!({"bin":id})).unwrap()["items"].as_array().unwrap().contains(&json!(first.0)));
    Arc::make_mut(&mut s.project).item_mut(first).unwrap().metadata.insert("Scene".into(), "Elsewhere".into());
    assert_eq!(s.execute("project.searchBinItems", json!({"bin":id})).unwrap()["items"], json!([]));
}
#[test]
fn saved_search_ambiguous_missing_or_deep_project_metadata_refuses_without_mutation() {
    for mode in ["duplicate-search", "collision", "missing-item", "deep-tree"] {
        let mut s = redo();
        if mode == "duplicate-search" {
            let q = FindQuery::simple("Name", FindOp::Contains, "x");
            let a = search::create(Arc::make_mut(&mut s.project), Some("x"), q).unwrap();
            Arc::make_mut(&mut s.project).search_bins.push(a);
        }
        if mode == "collision" {
            let p = Arc::make_mut(&mut s.project);
            p.search_bins.push(filmcraft_project::SearchBin { id: p.root.id, name: "x".into(), query: FindQuery::simple("Name", FindOp::Contains, "x") });
        }
        if mode == "missing-item" {
            Arc::make_mut(&mut s.project).root.children.push(BinEntry::Item(ItemId(99999)));
        }
        if mode == "deep-tree" {
            let mut chain = Bin { id: BinId(1000), name: "deep".into(), children: vec![] };
            for i in 0..66 {
                chain = Bin { id: BinId(1001 + i), name: "deep".into(), children: vec![BinEntry::Bin(chain)] };
            }
            Arc::make_mut(&mut s.project).root.children.push(BinEntry::Bin(chain));
        }
        reject(&mut s, "file.newSearchBin", json!({"text":"x"}));
    }
}
#[test]
fn saved_search_unknown_or_ambiguous_id_reads_edits_deletes_preserve_project_and_redo() {
    let mut s = redo();
    for command in ["project.searchBinItems", "project.editSearchBin", "project.deleteSearchBin"] {
        for bin in [json!(99999), json!(-1), json!(1.5), json!("1"), json!(null)] {
            reject(&mut s, command, json!({"bin":bin,"name":"Query"}));
        }
    }
    let q = FindQuery::simple("Name", FindOp::Contains, "camera");
    let bin = search::create(Arc::make_mut(&mut s.project), Some("x"), q).unwrap();
    Arc::make_mut(&mut s.project).search_bins.push(bin.clone());
    for command in ["project.searchBinItems", "project.editSearchBin", "project.deleteSearchBin"] {
        reject(&mut s, command, json!({"bin":bin.id.0,"name":"No ambiguous target"}));
    }
}
