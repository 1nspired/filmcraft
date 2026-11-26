use super::{
    searches,
    tests::{read, token},
};
use crate::Session;
use filmcraft_project::{BinId, MediaRef};
use serde_json::{Value, json};
use std::sync::Arc;
fn setup() -> Session {
    let mut s = super::tests::setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn scope(s: &Session) -> Value {
    json!({"projectId":"p_searches","anchorSequence":s.state.active_sequence.unwrap().0})
}
fn criteria(text: &str) -> Value {
    json!({"rows":[{"column":"Name","operator":"contains","text":text}],"matchAll":true,"caseSensitive":false})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"events":s.events.len()})
}
fn page(s: &mut Session, n: usize, query: Value, offset: usize, limit: usize, projection: bool) -> Value {
    s.execute("agent.readSearchBins", json!({"token":token(n),"scope":scope(s),"query":query,"offset":offset,"limit":limit,"projection":projection})).unwrap()
}
fn batch(s: &mut Session, n: usize, edit: Value) -> Value {
    let doc = read(s, n);
    json!({"token":token(n),"label":"Exact saved query","exact":true,"authority":"document_state","operations":[{"command":searches::COMMAND,"params":{"scope":scope(s),"beforeSha256":doc["searchState"]["sha256"],"edit":edit}}]})
}
#[test]
fn search_pages_preserve_complete_catalog_order_and_live_matches_without_navigation_or_history() {
    let mut s = setup();
    let mut ids = Vec::new();
    for i in 0..70 {
        ids.push(s.execute("file.newSearchBin", json!({"name":format!("Query {i}"),"column":"Name","text":"ocean"})).unwrap()["bin"].as_u64().unwrap());
    }
    let doc = read(&mut s, 86000);
    assert_eq!(doc["capabilities"]["searchBinTools"], true);
    let before = state(&s);
    let mut all = Vec::new();
    for offset in [0, 32, 64] {
        let p = page(&mut s, 86000, Value::Null, offset, 32, false);
        assert_eq!(p["sha256"], doc["searchState"]["sha256"]);
        all.extend(p["entries"].as_array().unwrap().iter().map(|e| e["bin"].as_u64().unwrap()));
    }
    assert_eq!(all, ids);
    let matches = page(&mut s, 86000, json!(ids[0]), 0, 32, false);
    assert_eq!(matches["mode"], "matches");
    let human = s.execute("project.searchBinItems", json!({"bin":ids[0]})).unwrap();
    assert_eq!(matches["entries"].as_array().unwrap().iter().map(|e| e["item"].clone()).collect::<Vec<_>>(), *human["items"].as_array().unwrap());
    assert_eq!(state(&s), before);
    for patch in [
        json!({"limit":0}),
        json!({"limit":33}),
        json!({"offset":100001}),
        json!({"query":0}),
        json!({"query":-1}),
        json!({"query":"name"}),
        json!({"scope":{"projectId":"p_searches","anchorSequence":999999}}),
        json!({"extra":true}),
    ] {
        let mut args = json!({"token":token(86000),"scope":scope(&s),"query":null,"offset":0,"limit":1});
        for (k, v) in patch.as_object().unwrap() {
            args[k] = v.clone();
        }
        assert!(s.execute("agent.readSearchBins", args).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn search_exact_create_update_delete_match_human_output_and_preserve_owned_undo_redo_and_replay() {
    let mut s = setup();
    let mut id = 0;
    for (i, kind) in ["create", "update", "delete"].iter().enumerate() {
        let n = 86100 + i * 10;
        let edit = match *kind {
            "create" => json!({"type":kind,"name":"Ocean","criteria":criteria("ocean")}),
            "update" => json!({"type":kind,"bin":id,"name":"Beach","criteria":criteria("beach")}),
            _ => json!({"type":kind,"bin":id}),
        };
        let human = match *kind {
            "create" => "file.newSearchBin",
            "update" => "project.editSearchBin",
            _ => "project.deleteSearchBin",
        };
        let mut expected = Session::new(s.services.clone());
        expected.project = s.project.clone();
        expected.state = s.state.clone();
        expected.media = s.media.clone();
        let args = match *kind {
            "create" => json!({"name":"Ocean","column":"Name","text":"ocean"}),
            "update" => json!({"bin":id,"name":"Beach","column":"Name","text":"beach"}),
            _ => json!({"bin":id}),
        };
        expected.execute(human, args).unwrap();
        let request = batch(&mut s, n, edit);
        let before = s.project.clone();
        let items = super::project_audit::value_bytes(&s.project.items).unwrap();
        let count = s.history.undo.len();
        let r = s.execute("agent.apply", request.clone()).unwrap();
        id = r["results"][0]["bin"].as_u64().unwrap();
        assert_eq!(r["action"], "search_edit");
        assert_eq!(r["authority"], "document_state");
        assert_eq!(r["results"][0]["humanCommand"], human);
        assert_eq!(s.project.to_json(), expected.project.to_json());
        assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&expected.state).unwrap());
        assert_eq!(super::project_audit::value_bytes(&s.project.items).unwrap(), items);
        assert_eq!(s.history.undo.len(), count + 1);
        let after = s.project.clone();
        assert_eq!(s.execute("agent.apply", request.clone()).unwrap(), r);
        read(&mut s, n + 1);
        s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
        let unchanged = state(&s);
        assert_eq!(s.execute("agent.apply", request).unwrap(), r);
        assert_eq!(state(&s), unchanged);
        read(&mut s, n + 2);
        s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &after));
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    }
}
#[test]
fn search_unknown_malformed_or_authority_bearing_criteria_refuse_before_publication() {
    let mut s = setup();
    s.execute("file.newBin", json!({"name":"Redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let base = batch(&mut s, 86200, json!({"type":"create","name":"Query","criteria":criteria("camera")}));
    let before = state(&s);
    for mode in ["extra", "missing", "wrong-bool", "rows", "operator", "name", "unsafe-id", "scope", "alias", "mixed"] {
        let mut r = base.clone();
        let p = &mut r["operations"][0]["params"];
        match mode {
            "extra" => p["edit"]["criteria"]["authority"] = json!(true),
            "missing" => {
                p["edit"]["criteria"].as_object_mut().unwrap().remove("matchAll");
            }
            "wrong-bool" => p["edit"]["criteria"]["caseSensitive"] = json!("false"),
            "rows" => p["edit"]["criteria"]["rows"] = json!(vec![json!({"column":"Name","operator":"contains","text":"x"}); 33]),
            "operator" => p["edit"]["criteria"]["rows"][0]["operator"] = json!("near"),
            "name" => p["edit"]["name"] = json!("x".repeat(1025)),
            "unsafe-id" => p["edit"] = json!({"type":"delete","bin":9007199254740992u64}),
            "scope" => p["scope"]["anchorSequence"] = json!(999999),
            "alias" => p["edit"]["criteria"]["rows"][0]["op"] = json!("matches"),
            _ => {
                r["operations"].as_array_mut().unwrap().push(base["operations"][0].clone());
            }
        }
        assert!(s.execute("agent.apply", r).is_err(), "accepted {mode}");
        assert_eq!(state(&s), before);
    }
    let mut r = base;
    r["exact"] = json!(false);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn search_unpaged_change_stale_ticket_and_noop_cannot_publish_or_consume_human_redo() {
    let mut s = setup();
    let id = s.execute("file.newSearchBin", json!({"name":"Old","text":"ocean"})).unwrap()["bin"].as_u64().unwrap();
    let stale = batch(&mut s, 86300, json!({"type":"delete","bin":id}));
    s.execute("file.newSearchBin", json!({"name":"Unpaged","text":"camera"})).unwrap();
    let before = state(&s);
    assert!(s.execute("agent.apply", stale.clone()).is_err());
    assert_eq!(state(&s), before);
    let mut forged = batch(&mut s, 86301, json!({"type":"delete","bin":id}));
    forged["operations"][0]["params"]["beforeSha256"] = stale["operations"][0]["params"]["beforeSha256"].clone();
    assert!(s.execute("agent.apply", forged).is_err());
    assert_eq!(state(&s), before);
    let noop = batch(
        &mut s,
        86302,
        json!({"type":"update","bin":id,"name":"Old","criteria":json!({"rows":[{"column":"All","operator":"contains","text":"ocean"}],"matchAll":true,"caseSensitive":false})}),
    );
    assert!(s.execute("agent.apply", noop).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn search_live_match_metadata_changes_have_distinct_digest_without_mutating_query_definitions() {
    let mut s = setup();
    let id = s.execute("file.newSearchBin", json!({"name":"All visible","column":"Name","operator":"doesNotContain","text":"ZZZ_NEVER"})).unwrap()["bin"]
        .as_u64()
        .unwrap();
    read(&mut s, 86400);
    let before = page(&mut s, 86400, json!(id), 0, 32, false);
    let item = filmcraft_project::ItemId(before["entries"][0]["item"].as_u64().unwrap());
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().name = "Changed metadata only".into();
    let current = state(&s);
    assert!(s.execute("agent.readSearchBins", json!({"token":token(86400),"scope":scope(&s),"query":id,"offset":0,"limit":32})).is_err());
    read(&mut s, 86401);
    let after = page(&mut s, 86401, json!(id), 0, 32, false);
    assert_eq!(after["basis"], before["basis"]);
    assert_ne!(after["sha256"], before["sha256"]);
    assert_eq!(state(&s), current);
}
#[test]
fn search_unsafe_or_unbounded_legacy_definitions_are_unavailable_instead_of_partial() {
    for mode in ["id", "name", "criteria", "duplicate"] {
        let mut s = setup();
        let id = s.execute("file.newSearchBin", json!({"text":"camera"})).unwrap()["bin"].as_u64().unwrap();
        let p = Arc::make_mut(&mut s.project);
        match mode {
            "id" => p.search_bins[0].id = BinId(9007199254740992),
            "name" => p.search_bins[0].name = "x".repeat(1025),
            "criteria" => p.search_bins[0].query.rows[0].text = "x".repeat(4097),
            _ => p.search_bins.push(p.search_bins[0].clone()),
        };
        let doc = read(&mut s, 86500);
        assert_eq!(doc["searchState"]["status"], "unavailable");
        let before = state(&s);
        assert!(s.execute("agent.readSearchBins", json!({"token":token(86500),"scope":scope(&s),"query":null,"offset":0,"limit":1})).is_err());
        assert_eq!(state(&s), before);
        assert!(id > 0);
    }
}
#[test]
fn search_stop_before_publication_retains_project_history_and_prevents_replay() {
    let mut s = setup();
    let r = batch(&mut s, 86600, json!({"type":"create","name":"Cancelled","criteria":criteria("camera")}));
    let before = state(&s);
    let control = s.agent_operation_control();
    let apply = super::commands().into_iter().find(|c| c.id == "agent.apply").unwrap().run;
    let result = super::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        assert_eq!(control.cancel(&json!({"token":token(86600)})).unwrap()["cancellationAccepted"], true);
        apply(s, p)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn search_owned_history_cannot_cross_later_human_work_or_take_its_undo() {
    let mut s = setup();
    let r = batch(&mut s, 86700, json!({"type":"create","name":"Assistant","criteria":criteria("ocean")}));
    let receipt = s.execute("agent.apply", r.clone()).unwrap();
    s.execute("file.newSearchBin", json!({"name":"Human later","text":"camera"})).unwrap();
    read(&mut s, 86701);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(86701),"commit":token(86700)})).is_err());
    assert_eq!(state(&s), before);
    assert_eq!(s.execute("agent.apply", r).unwrap(), receipt);
    assert_eq!(state(&s), before);
}
#[test]
fn search_offline_authored_read_and_edit_never_renew_original_media_authority() {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let dir = crate::temp_dir().join(format!("native-searches-{:x}", u128::from_le_bytes(entropy)));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("source.bin");
    std::fs::write(&file, vec![39u8; 4096]).unwrap();
    let mut s = setup();
    let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata.insert(
            super::BINDING_KEY.into(),
            json!({"projectId":"p_searches","assetId":format!("a_{}",item.id.0),"mediaId":"original","role":"original"}).to_string(),
        );
    }
    Arc::make_mut(&mut s.project).item_mut(source).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    let r = batch(&mut s, 86800, json!({"type":"create","name":"Metadata only","criteria":criteria("ocean")}));
    let witness = s.agent_bridge.media.clone().unwrap();
    std::fs::remove_file(&file).unwrap();
    let mut legacy = r.clone();
    legacy.as_object_mut().unwrap().remove("authority");
    assert!(s.execute("agent.apply", legacy).is_err());
    assert_eq!(s.execute("agent.apply", r).unwrap()["authority"], "document_state");
    assert!(Arc::ptr_eq(&witness, s.agent_bridge.media.as_ref().unwrap()));
    let doc = s
        .execute("agent.projectDocument", json!({"token":token(86801),"basis":token(86800),"mediaScope":{"sequences":[s.state.active_sequence.unwrap().0]}}))
        .unwrap();
    assert_eq!(doc["documentProjection"]["sourceValidation"], "not-performed");
    let p = page(&mut s, 86801, Value::Null, 0, 32, true);
    assert_eq!(p["requiresSourceValidation"], true);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(86801)})).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn search_match_pages_retain_complete_order_and_digest_for_seventy_non_media_hits() {
    let mut s = setup();
    let id = s.execute("file.newSearchBin", json!({"name":"Sequence metadata","column":"Name","operator":"beginsWith","text":"Search hit "})).unwrap()["bin"]
        .as_u64()
        .unwrap();
    let settings = s.active_sequence().unwrap().settings.clone();
    let mut expected = Vec::new();
    for i in 0..70 {
        expected.push(Arc::make_mut(&mut s.project).new_sequence(&format!("Search hit {i}"), settings.clone(), 1, 1, None).0);
    }
    read(&mut s, 86900);
    let before = state(&s);
    let mut items = Vec::new();
    let mut sha = None;
    for offset in [0, 32, 64] {
        let p = page(&mut s, 86900, json!(id), offset, 32, false);
        assert_eq!(p["total"], 70);
        assert_eq!(p["mode"], "matches");
        if let Some(old) = &sha {
            assert_eq!(&p["sha256"], old);
        } else {
            sha = Some(p["sha256"].clone());
        }
        items.extend(p["entries"].as_array().unwrap().iter().map(|e| e["item"].as_u64().unwrap()));
    }
    assert_eq!(items, expected);
    assert_eq!(state(&s), before);
}
