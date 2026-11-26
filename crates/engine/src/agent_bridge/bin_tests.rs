use super::{
    bins, project_audit,
    tests::{read, token},
};
use crate::Session;
use filmcraft_project::{BinId, ItemId, MediaRef};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup() -> Session {
    let mut s = super::tests::setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn scope(s: &Session) -> Value {
    json!({"projectId":"p_bins","anchorSequence":s.state.active_sequence.unwrap().0})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"events":s.events.len()})
}
fn page(s: &mut Session, n: usize, offset: usize, limit: usize, projection: bool) -> Value {
    s.execute("agent.readBins", json!({"token":token(n),"scope":scope(s),"offset":offset,"limit":limit,"projection":projection})).unwrap()
}
fn batch(s: &mut Session, n: usize, edit: Value) -> Value {
    let p = page(s, n, 0, 1, false);
    json!({"token":token(n),"label":"Exact native organization","exact":true,"authority":"document_state","operations":[{"command":bins::COMMAND,"params":{"scope":scope(s),"beforeSha256":p["sha256"],"edit":edit}}]})
}
fn bin(s: &mut Session, name: &str, parent: Option<u64>) -> u64 {
    s.execute("file.newBin", json!({"name":name,"parent":parent})).unwrap()["bin"].as_u64().unwrap()
}
fn step(s: &mut Session, n: usize, edit: Value, human: &str, args: Value) -> Value {
    let mut other = Session::new(s.services.clone());
    other.project = s.project.clone();
    other.state = s.state.clone();
    other.media = s.media.clone();
    other.execute(human, args).unwrap();
    let before = s.project.clone();
    let items = project_audit::value_bytes(&s.project.items).unwrap();
    let undo = s.history.undo.len();
    read(s, n);
    let request = batch(s, n, edit);
    let receipt = s.execute("agent.apply", request.clone()).unwrap();
    assert_eq!(receipt["action"], "bin_edit");
    assert_eq!(receipt["authority"], "document_state");
    assert_eq!(receipt["results"][0]["humanCommand"], human);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.project.to_json(), other.project.to_json());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&other.state).unwrap());
    assert_eq!(project_audit::value_bytes(&s.project.items).unwrap(), items, "organization cannot alter any item payload, source, cut, caption or sound");
    let after = s.project.clone();
    assert_eq!(s.execute("agent.apply", request.clone()).unwrap(), receipt);
    assert!(Arc::ptr_eq(&s.project, &after));
    read(s, n + 1);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    let unchanged = state(s);
    assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
    assert_eq!(state(s), unchanged, "lost acknowledgement never replays after Undo");
    read(s, n + 2);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    receipt
}
#[test]
fn physical_bin_pages_are_complete_ordered_bounded_and_read_only() {
    let mut s = setup();
    for i in 0..70 {
        bin(&mut s, &format!("Untrusted name {i}"), None);
    }
    let doc = read(&mut s, 84000);
    assert_eq!(doc["capabilities"]["binTools"], true);
    assert_eq!(doc["binState"]["status"], "current");
    let before = state(&s);
    let mut offset = 0;
    let mut rows = Vec::new();
    loop {
        let p = page(&mut s, 84000, offset, 32, false);
        assert_eq!(p["sha256"], doc["binState"]["sha256"]);
        assert_eq!(p["total"], doc["binState"]["entries"]);
        rows.extend(p["entries"].as_array().unwrap().clone());
        match p["nextOffset"].as_u64() {
            Some(next) => offset = next as usize,
            None => break,
        }
    }
    fn bins_in_tree(b: &filmcraft_project::Bin) -> usize {
        1 + b
            .children
            .iter()
            .filter_map(|entry| match entry {
                filmcraft_project::BinEntry::Bin(b) => Some(bins_in_tree(b)),
                _ => None,
            })
            .sum::<usize>()
    }
    assert_eq!(rows.len(), s.project.items.len() + bins_in_tree(&s.project.root));
    assert_eq!(rows[0]["parent"], Value::Null);
    assert_eq!(rows[0]["id"], s.project.root.id.0);
    assert_eq!(rows[0]["children"], s.project.root.children.len());
    assert_eq!(rows.iter().filter(|r| r["kind"] == "item").count(), s.project.items.len());
    assert_eq!(state(&s), before);
    for args in [
        json!({"offset":0,"limit":0}),
        json!({"offset":0,"limit":33}),
        json!({"offset":100001,"limit":1}),
        json!({"offset":-1,"limit":1}),
        json!({"offset":0,"limit":1,"extra":true}),
    ] {
        let mut p = args;
        p["token"] = json!(token(84000));
        p["scope"] = scope(&s);
        assert!(s.execute("agent.readBins", p).is_err());
        assert_eq!(state(&s), before);
    }
    let last = rows.last().unwrap()["id"].as_u64().unwrap();
    s.execute("project.renameBin", json!({"bin":last,"name":"Changed unpaged bin"})).unwrap();
    let current = state(&s);
    assert!(s.execute("agent.readBins", json!({"token":token(84000),"scope":scope(&s),"offset":0,"limit":1})).is_err());
    assert_eq!(state(&s), current);
    assert_ne!(read(&mut s, 84001)["binState"]["sha256"], doc["binState"]["sha256"]);
}
#[test]
fn bin_gestures_match_human_services_with_exact_owned_history_and_unchanged_items() {
    let mut s = setup();
    let a = bin(&mut s, "Camera A", None);
    let b = bin(&mut s, "Camera B", None);
    let created = step(&mut s, 84100, json!({"type":"create","name":"一日の素材","parent":a}), "file.newBin", json!({"name":"一日の素材","parent":a}));
    let child = created["results"][0]["entries"][0].as_u64().unwrap();
    assert!(
        s.project
            .root
            .find_bin(BinId(a))
            .unwrap()
            .children
            .iter()
            .any(|entry| matches!(entry,filmcraft_project::BinEntry::Bin(bin) if bin.id.0==child && bin.name=="一日の素材"))
    );
    step(&mut s, 84110, json!({"type":"rename","bin":a,"name":"  Human camera  "}), "project.renameBin", json!({"bin":a,"name":"  Human camera  "}));
    assert_eq!(s.project.root.find_bin(BinId(a)).unwrap().name, "Human camera");
    let item = s.project.items.keys().next().unwrap().0;
    step(&mut s, 84120, json!({"type":"move","entries":[item,a],"parent":b}), "project.moveToBin", json!({"items":[item,a],"bin":b}));
    let children = &s.project.root.find_bin(BinId(b)).unwrap().children;
    assert!(children.iter().any(|entry| matches!(entry,filmcraft_project::BinEntry::Item(id) if id.0==item)));
    assert!(children.iter().any(|entry| matches!(entry,filmcraft_project::BinEntry::Bin(bin) if bin.id.0==a)));
    let root = s.project.root.id.0;
    step(&mut s, 84130, json!({"type":"move","entries":[a],"parent":root}), "project.moveToBin", json!({"items":[a],"bin":root}));
    step(&mut s, 84140, json!({"type":"create","name":"Root folder","parent":null}), "file.newBin", json!({"name":"Root folder","parent":null}));
    step(&mut s, 84150, json!({"type":"move","entries":[a,child],"parent":b}), "project.moveToBin", json!({"items":[a,child],"bin":b}));
    assert!(s.project.root.find_bin(BinId(a)).unwrap().children.is_empty());
    assert!(s.project.root.find_bin(BinId(b)).unwrap().children.iter().any(|entry| matches!(entry,filmcraft_project::BinEntry::Bin(bin) if bin.id.0==child)));
}
#[test]
fn incomplete_cycle_duplicate_unknown_and_malformed_bin_requests_never_publish_or_consume_redo() {
    let mut s = setup();
    let a = bin(&mut s, "Outer", None);
    let child = bin(&mut s, "Inner", Some(a));
    let item = s.project.items.keys().next().unwrap().0;
    bin(&mut s, "Human redo", None);
    s.execute("edit.undo", json!({})).unwrap();
    for (i, edit) in [
        json!({"type":"create","name":"Unknown parent","parent":999999}),
        json!({"type":"create","name":"Missing parent"}),
        json!({"type":"create","name":null,"parent":null}),
        json!({"type":"create","name":" ","parent":null}),
        json!({"type":"rename","bin":0,"name":"Root"}),
        json!({"type":"rename","bin":999999,"name":"Unknown"}),
        json!({"type":"move","entries":[item,999999],"parent":a}),
        json!({"type":"move","entries":[item,item],"parent":a}),
        json!({"type":"move","entries":[a],"parent":child}),
        json!({"type":"move","entries":[a],"parent":a}),
        json!({"type":"move","entries":[0],"parent":a}),
        json!({"type":"move","entries":[],"parent":a}),
        json!({"type":"move","entries":[item]}),
        json!({"type":"move","entries":[item],"parent":null,"extra":true}),
    ]
    .into_iter()
    .enumerate()
    {
        let n = 84200 + i;
        read(&mut s, n);
        let r = batch(&mut s, n, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", r).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn exact_move_service_rejects_partial_work_while_human_skip_semantics_stay_intact() {
    let mut s = setup();
    let a = bin(&mut s, "Target", None);
    let item = *s.project.items.keys().next().unwrap();
    let original = s.project.to_json();
    let mut candidate = (*s.project).clone();
    assert!(crate::project_organization::move_entries(&mut candidate, &[item, ItemId(999999)], Some(BinId(a)), true).is_err());
    assert_eq!(candidate.to_json(), original);
    assert_eq!(s.execute("project.moveToBin", json!({"items":[item.0,999999],"bin":a})).unwrap()["moved"], 1);
    let before = s.project.clone();
    let mut candidate = (*before).clone();
    assert!(crate::project_organization::move_entries(&mut candidate, &[ItemId(a)], Some(BinId(a)), true).is_err());
    assert_eq!(candidate.to_json(), before.to_json());
}
#[test]
fn wrong_scope_stale_tree_nonexact_mixed_and_noop_bin_gestures_are_atomic() {
    for mode in 0..8 {
        let mut s = setup();
        let a = bin(&mut s, "Exact", None);
        let n = 84300 + mode;
        read(&mut s, n);
        let mut r = batch(&mut s, n, json!({"type":"rename","bin":a,"name":"New"}));
        match mode {
            0 => r["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
            1 => r["operations"][0]["params"]["scope"]["anchorSequence"] = json!(999999),
            2 => r["exact"] = json!(false),
            3 => {
                let op = r["operations"][0].clone();
                r["operations"].as_array_mut().unwrap().push(op);
            }
            4 => r["operations"][0]["params"]["edit"]["name"] = json!("Exact"),
            5 => {
                s.execute("project.renameBin", json!({"bin":a,"name":"Later human"})).unwrap();
            }
            6 => {
                s.project = Arc::new((*s.project).clone());
            }
            _ => r["operations"][0]["params"]["edit"] = json!({"type":"move","entries":[a],"parent":null}),
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", r).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn complete_bin_inventory_refuses_dangling_duplicate_unsafe_and_unfiled_entries() {
    for mode in 0..4 {
        let mut s = setup();
        let a = bin(&mut s, "Original", None);
        let p = Arc::make_mut(&mut s.project);
        match mode {
            0 => p.root.children.push(filmcraft_project::BinEntry::Item(ItemId(999999))),
            1 => {
                p.root.children.push(filmcraft_project::BinEntry::Bin(filmcraft_project::Bin { id: BinId(a), name: "Duplicate".into(), children: Vec::new() }))
            }
            2 => p.root.id = BinId(9_007_199_254_740_992),
            _ => {
                let item = *p.items.keys().next().unwrap();
                p.root.remove_item(item);
            }
        }
        let n = 84400 + mode;
        let doc = read(&mut s, n);
        assert_eq!(doc["binState"]["status"], "unavailable");
        let before = state(&s);
        assert!(s.execute("agent.readBins", json!({"token":token(n),"scope":scope(&s),"offset":0,"limit":1})).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn move_depth_limits_are_atomic_in_the_shared_service() {
    let mut s = setup();
    let a = bin(&mut s, "Moving parent", None);
    bin(&mut s, "Moving child", Some(a));
    let mut parent = s.project.root.id;
    for i in 0..63 {
        parent = crate::project_organization::create(Arc::make_mut(&mut s.project), &format!("Level {i}"), Some(parent)).unwrap();
    }
    let before = s.project.to_json();
    let mut p = (*s.project).clone();
    assert!(crate::project_organization::move_entries(&mut p, &[ItemId(a)], Some(parent), true).is_err());
    assert_eq!(p.to_json(), before);
    assert!(s.execute("project.moveToBin", json!({"items":[a],"bin":parent.0})).is_err());
    assert_eq!(s.project.to_json(), before);
}
#[test]
fn bin_history_refuses_later_human_work_and_does_not_borrow_its_undo() {
    let mut s = setup();
    read(&mut s, 84500);
    let r = batch(&mut s, 84500, json!({"type":"create","name":"Assistant","parent":null}));
    let receipt = s.execute("agent.apply", r.clone()).unwrap();
    bin(&mut s, "Later human", None);
    read(&mut s, 84501);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(84501),"commit":token(84500)})).is_err());
    assert_eq!(state(&s), before);
    assert_eq!(s.execute("agent.apply", r).unwrap(), receipt);
    assert_eq!(state(&s), before);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.project.root.children.iter().any(|e| matches!(e,filmcraft_project::BinEntry::Bin(b) if b.name=="Later human")));
}
#[test]
fn bin_stop_before_publication_leaves_no_project_history_or_replay() {
    let mut s = setup();
    read(&mut s, 84600);
    let r = batch(&mut s, 84600, json!({"type":"create","name":"Cancelled","parent":null}));
    let before = state(&s);
    let control = s.agent_operation_control();
    let apply = super::commands().into_iter().find(|c| c.id == "agent.apply").unwrap().run;
    let result = super::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        assert_eq!(control.cancel(&json!({"token":token(84600)})).unwrap()["cancellationAccepted"], true);
        apply(s, p)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).unwrap();
        let p = crate::temp_dir().join(format!("native-bins-{:x}", u128::from_le_bytes(bytes)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn authored_bins_remain_offline_document_work_without_renewing_source_evidence() {
    let dir = Scratch::new();
    let file = dir.0.join("source.bin");
    std::fs::write(&file, vec![39u8; 4096]).unwrap();
    let mut s = setup();
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata.insert(
            super::BINDING_KEY.into(),
            json!({"projectId":"p_bins","assetId":format!("a_{}",item.id.0),"mediaId":"original","role":"original"}).to_string(),
        );
    }
    let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(source).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    read(&mut s, 84700);
    let r = batch(&mut s, 84700, json!({"type":"create","name":"Offline documentary bin","parent":null}));
    let witness = s.agent_bridge.media.clone().unwrap();
    let before = s.project.clone();
    std::fs::remove_file(&file).unwrap();
    let mut legacy = r.clone();
    legacy.as_object_mut().unwrap().remove("authority");
    assert!(s.execute("agent.apply", legacy).is_err());
    let receipt = s.execute("agent.apply", r.clone()).unwrap();
    assert_eq!(receipt["authority"], "document_state");
    assert!(Arc::ptr_eq(&witness, s.agent_bridge.media.as_ref().unwrap()));
    let after = s.project.clone();
    let doc = s
        .execute("agent.projectDocument", json!({"token":token(84701),"basis":token(84700),"mediaScope":{"sequences":[s.state.active_sequence.unwrap().0]}}))
        .unwrap();
    assert_eq!(doc["documentProjection"]["sourceValidation"], "not-performed");
    page(&mut s, 84701, 0, 32, true);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(84701)})).is_err());
    s.execute("agent.undo", json!({"token":token(84701),"commit":token(84700)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.execute("agent.projectDocument", json!({"token":token(84702),"basis":token(84701),"mediaScope":{"sequences":[s.state.active_sequence.unwrap().0]}}))
        .unwrap();
    s.execute("agent.redo", json!({"token":token(84702),"commit":token(84700),"undo":token(84701)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert!(Arc::ptr_eq(&witness, s.agent_bridge.media.as_ref().unwrap()));
}
#[test]
fn bin_commit_and_owned_history_survive_actual_file_and_companion_cold_reopen() {
    let dir = Scratch::new();
    let file = dir.0.join("bins.fcproj");
    let mut s = setup();
    s.path = Some(file.to_string_lossy().into_owned());
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|item| item.as_media().is_some()) {
        item.metadata.insert(
            super::BINDING_KEY.into(),
            json!({"projectId":"p_bins","assetId":format!("a_generated_{}",item.id.0),"mediaId":"original","role":"original"}).to_string(),
        );
    }
    let original = s.project.to_json();
    read(&mut s, 84800);
    let r = batch(&mut s, 84800, json!({"type":"create","name":"Persisted bin","parent":null}));
    s.execute("agent.apply", r).unwrap();
    filmcraft_format::atomic_write(&file, &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false)).unwrap();
    s.saved_revision = s.revision;
    read(&mut s, 84801);
    let save=s.execute("agent.saveHistoryCheckpoint",json!({"token":token(84801),"scope":scope(&s),"workspace":"53c48674-39fe-489a-8c6e-51241af6dccb","checkpoint":"901c432f-8c98-4f7e-806a-e53fc4745ee1"})).unwrap();
    let after = s.project.to_json();
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":file})).unwrap();
    assert_eq!(fresh.project.to_json(), after);
    read(&mut fresh, 84802);
    fresh.execute("agent.restoreHistoryCheckpoint",json!({"token":token(84802),"scope":scope(&fresh),"workspace":"53c48674-39fe-489a-8c6e-51241af6dccb","checkpoint":"901c432f-8c98-4f7e-806a-e53fc4745ee1","sha256":save["checkpoint"]["sha256"]})).unwrap();
    read(&mut fresh, 84803);
    fresh.execute("agent.undo", json!({"token":token(84803),"commit":token(84800)})).unwrap();
    assert_eq!(fresh.project.to_json(), original);
    read(&mut fresh, 84804);
    fresh.execute("agent.redo", json!({"token":token(84804),"commit":token(84800),"undo":token(84803)})).unwrap();
    assert_eq!(fresh.project.to_json(), after);
}

#[test]
fn physical_inventory_includes_hidden_graphic_dependencies_without_authorizing_them_as_human_targets() {
    let mut s = setup();
    let target = bin(&mut s, "Visible target", None);
    let frame = s.sequence_rate().frame_at(s.active_sequence().unwrap().duration()) + 1;
    s.execute("graphics.newShape", json!({"shape":"rectangle","track":0,"frame":frame,"position":[10,10],"size":[12,10]})).unwrap();
    let hidden = s.project.items.values().find(|item| !crate::project_panel::listed(&s.project, item)).unwrap().id;
    read(&mut s, 84900);
    let p = page(&mut s, 84900, 0, 32, false);
    let row = p["entries"].as_array().unwrap().iter().find(|row| row["id"] == hidden.0).unwrap();
    assert_eq!(row["visible"], false);
    assert_eq!(row["itemType"], "Graphic");
    let r = batch(&mut s, 84900, json!({"type":"move","entries":[hidden.0],"parent":target}));
    let before = state(&s);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn guarded_selection_group_matches_the_original_human_one_step_and_exact_owned_undo_redo() {
    let mut s = setup();
    let parent = bin(&mut s, "Grouping parent", None);
    let first = bin(&mut s, "Bin 01", Some(parent));
    let other = bin(&mut s, "Another source bin", None);
    let item = s.project.items.keys().next().unwrap().0;
    let doc = read(&mut s, 84900);
    assert_eq!(doc["capabilities"]["binSelectionGrouping"], true);
    let receipt = step(
        &mut s,
        84910,
        json!({"type":"group","entries":[first,item,other],"name":null}),
        "file.newBinFromSelection",
        json!({"items":[first,item,other],"name":null}),
    );
    let group = receipt["results"][0]["entries"][0].as_u64().unwrap();
    assert_eq!(receipt["results"][0]["moved"], 3);
    assert!(
        s.project.root.children.iter().any(|entry| matches!(entry,filmcraft_project::BinEntry::Bin(bin) if bin.id.0==group)),
        "original first-bin selection groups at root"
    );
    assert_eq!(s.project.root.find_bin(BinId(group)).unwrap().name, "Bin 02");
    step(
        &mut s,
        84920,
        json!({"type":"group","entries":[group],"name":"一日の選択"}),
        "file.newBinFromSelection",
        json!({"items":[group],"name":"一日の選択"}),
    );
}

#[test]
fn guarded_selection_group_refuses_partial_unknown_root_hidden_duplicate_and_malformed_requests_atomically() {
    let mut s = setup();
    let a = bin(&mut s, "Outer group", None);
    bin(&mut s, "Inner group", Some(a));
    let item = s.project.items.keys().next().unwrap().0;
    s.execute("project.moveToBin", json!({"items":[item],"bin":a})).unwrap();
    let frame = s.sequence_rate().frame_at(s.active_sequence().unwrap().duration()) + 1;
    s.execute("graphics.newShape", json!({"shape":"rectangle","track":0,"frame":frame,"position":[10,10],"size":[12,10]})).unwrap();
    let hidden = s.project.items.values().find(|item| !crate::project_panel::listed(&s.project, item)).unwrap().id.0;
    for (index, edit) in [
        json!({"type":"group","entries":[hidden],"name":null}),
        json!({"type":"group","entries":[],"name":null}),
        json!({"type":"group","entries":[item,item],"name":null}),
        json!({"type":"group","entries":[item,999999],"name":null}),
        json!({"type":"group","entries":[item,a],"name":null}),
        json!({"type":"group","entries":[0],"name":null}),
        json!({"type":"group","entries":[item]}),
        json!({"type":"group","entries":[item],"name":false}),
        json!({"type":"group","entries":[item],"name":" "}),
        json!({"type":"group","entries":[item],"name":null,"parent":null}),
    ]
    .into_iter()
    .enumerate()
    {
        let n = 84930 + index;
        read(&mut s, n);
        let request = batch(&mut s, n, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", request).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn historical_selection_group_requires_the_exact_shared_prediction_and_created_bin_receipt() {
    let mut s = setup();
    let first = bin(&mut s, "Selected", None);
    read(&mut s, 84960);
    let request = batch(&mut s, 84960, json!({"type":"group","entries":[first],"name":null}));
    let operation: super::Operation = serde_json::from_value(request["operations"][0].clone()).unwrap();
    let before = s.project.clone();
    let receipt = s.execute("agent.apply", request).unwrap();
    let result = &receipt["results"][0];
    super::project_plane::validate_history_transition(&before, &s.project, &operation, result).unwrap();
    let mut forged = (*s.project).clone();
    let group = BinId(result["entries"][0].as_u64().unwrap());
    forged.root.find_bin_mut(group).unwrap().name = "Forged later name".into();
    let mut forged_result = result.clone();
    forged_result["afterSha256"] = bins::project_summary(&forged)["sha256"].clone();
    assert!(super::project_plane::validate_history_transition(&before, &forged, &operation, &forged_result).is_err());
    for value in [json!([first]), json!([])] {
        let mut bad = result.clone();
        bad["entries"] = value;
        assert!(super::project_plane::validate_history_transition(&before, &s.project, &operation, &bad).is_err());
    }
    let mut bad = result.clone();
    bad["moved"] = json!(0);
    assert!(super::project_plane::validate_history_transition(&before, &s.project, &operation, &bad).is_err());
}
