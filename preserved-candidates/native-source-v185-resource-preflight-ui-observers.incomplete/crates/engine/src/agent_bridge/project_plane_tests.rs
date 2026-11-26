use super::{project_plane, tests::token};
use crate::Session;
use filmcraft_project::{BinEntry, ItemId, Label};
use serde_json::{Value, json};
use std::sync::Arc;
const WORKSPACE: &str = "6ae17621-32f1-4e2b-8940-aef47d1279a5";
fn scope() -> Value {
    json!({"projectId":"p_project","workspaceId":WORKSPACE})
}
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Project plane","workspace":scope()})).unwrap();
    s.path = Some("/generated/project.fcproj".into());
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"path":s.path})
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn batch(n: usize, command: &str, params: Value) -> Value {
    json!({"token":token(n),"label":"Exact project gesture","exact":true,"authority":"document_state","operations":[{"command":command,"params":params}]})
}
fn bin(s: &mut Session, n: usize, edit: Value) -> Value {
    let d = read(s, n);
    batch(n, "agent.organizeProject", json!({"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":edit}))
}
fn search(s: &mut Session, n: usize, edit: Value) -> Value {
    let d = read(s, n);
    batch(n, "agent.editSearchBins", json!({"scope":scope(),"beforeSha256":d["searchState"]["sha256"],"edit":edit}))
}
fn criteria(text: &str) -> Value {
    json!({"rows":[{"column":"Name","operator":"contains","text":text}],"matchAll":true,"caseSensitive":false})
}
fn settings() -> Value {
    json!({"name":"First exact sequence","width":3840,"height":2160,"frameRate":{"num":48001,"den":2000},"sampleRate":96000,"mix":"Mono","trackType":"Mono","video":2,"audio":4})
}
fn first(s: &mut Session, n: usize) -> Value {
    let d = read(s, n);
    batch(n, project_plane::CREATE, json!({"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"sequence":settings()}))
}
fn owned_cycle(s: &mut Session, n: usize, request: &Value, before: Arc<filmcraft_project::Project>) {
    let result = s.execute("agent.apply", request.clone()).unwrap();
    let after = s.project.clone();
    assert_eq!(result["authority"], "document_state");
    read(s, n + 1);
    let h = s.execute("agent.readHistory", json!({"token":token(n+1),"scope":scope(),"commit":token(n),"undo":null})).unwrap();
    assert_eq!(h["undoAvailable"], true);
    assert_eq!(h["requiresSourceValidation"], true);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert_eq!(s.project, before);
    read(s, n + 2);
    let h = s.execute("agent.readHistory", json!({"token":token(n+2),"scope":scope(),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(s.project, after);
}
#[test]
fn project_plane_reads_real_absence_and_complete_metadata_without_source_or_timeline_authority() {
    let mut s = setup();
    let before = state(&s);
    let d = read(&mut s, 90000);
    assert_eq!(state(&s), before);
    assert!(d["sequence"].is_null());
    assert_eq!(d["scope"], scope());
    assert_eq!(d["projectState"]["sequences"], 0);
    assert_eq!(d["mediaEvidence"], false);
    assert_eq!(d["sourceValidation"], "not-performed");
    assert_eq!(d["writeAuthority"], false);
    assert_eq!(d["capabilities"]["projectFirstSequence"], true);
    let page = s.execute("agent.readBins", json!({"token":token(90000),"scope":scope(),"offset":0,"limit":32})).unwrap();
    assert!(page["sequence"].is_null());
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["requiresSourceValidation"], true);
    let page = s.execute("agent.readSearchBins", json!({"token":token(90000),"scope":scope(),"query":null,"offset":0,"limit":32})).unwrap();
    assert_eq!(page["total"], 0);
    assert!(page["sequence"].is_null());
    assert_eq!(s.execute(project_plane::VALIDATE, json!({"token":token(90000),"scope":scope()})).unwrap()["sha256"], d["projectState"]["sha256"]);
    assert!(s.execute(project_plane::READ, json!({"token":token(90000),"scope":scope(),"projectPath":s.path})).is_err());
}
#[test]
fn project_plane_bin_creation_rename_and_move_match_human_service_and_owned_absent_sequence_history() {
    let mut s = setup();
    let a = s.execute("file.newBin", json!({"name":"A"})).unwrap()["bin"].as_u64().unwrap();
    let b = s.execute("file.newBin", json!({"name":"B"})).unwrap()["bin"].as_u64().unwrap();
    for (n, edit, command, args) in [
        (90100, json!({"type":"create","name":"New","parent":a}), "file.newBin", json!({"name":"New","parent":a})),
        (90110, json!({"type":"rename","bin":a,"name":"  Camera  "}), "project.renameBin", json!({"bin":a,"name":"  Camera  "})),
        (90120, json!({"type":"move","entries":[a],"parent":b}), "project.moveToBin", json!({"items":[a],"bin":b})),
    ] {
        let before = s.project.clone();
        let mut human = Session { project: s.project.clone(), state: s.state.clone(), ..Session::default() };
        human.execute(command, args).unwrap();
        let request = bin(&mut s, n, edit);
        owned_cycle(&mut s, n, &request, before);
        assert_eq!(s.project, human.project);
        assert!(s.state.active_sequence.is_none());
    }
}
#[test]
fn project_plane_saved_queries_match_all_three_human_services_with_shared_receipts_and_owned_history() {
    let mut s = setup();
    for (n, edit, command, args) in [
        (
            90200,
            json!({"type":"create","name":"Query","criteria":criteria("camera")}),
            "file.newSearchBin",
            json!({"name":"Query","rows":criteria("camera")["rows"],"matchAll":true,"caseSensitive":false}),
        ),
        (
            90210,
            json!({"type":"update","bin":1,"name":"Edited","criteria":criteria("beach")}),
            "project.editSearchBin",
            json!({"bin":1,"name":"Edited","rows":criteria("beach")["rows"],"matchAll":true,"caseSensitive":false}),
        ),
        (90220, json!({"type":"delete","bin":1}), "project.deleteSearchBin", json!({"bin":1})),
    ] {
        let before = s.project.clone();
        let mut human = Session { project: s.project.clone(), state: s.state.clone(), ..Session::default() };
        human.execute(command, args).unwrap();
        let request = search(&mut s, n, edit);
        owned_cycle(&mut s, n, &request, before);
        assert_eq!(s.project, human.project);
    }
}
#[test]
fn project_plane_initial_sequence_matches_exact_human_preferences_and_owned_undo_redo_crosses_no_sequence() {
    let mut s = setup();
    s.prefs.labels.defaults.sequence = Label::Rose;
    s.execute("file.newBin", json!({"name":"Before sequence"})).unwrap();
    let before = s.project.clone();
    let mut human = Session { project: s.project.clone(), state: s.state.clone(), prefs: s.prefs.clone(), ..Session::default() };
    human.execute("file.newSequence",json!({"name":"First exact sequence","width":3840,"height":2160,"fps":{"num":48001,"den":2000},"sampleRate":96000,"mix":"Mono","trackType":"Mono","video":2,"audio":4})).unwrap();
    let request = first(&mut s, 90300);
    owned_cycle(&mut s, 90300, &request, before);
    assert_eq!(s.project, human.project);
    assert_eq!(s.state.active_sequence, human.state.active_sequence);
    assert_eq!(s.project.item(s.state.active_sequence.unwrap()).unwrap().label, Label::Rose);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(90300)})).unwrap()["result"]["action"], "project_sequence_create");
}
#[test]
fn project_plane_source_timeline_background_and_sequence_checkpoint_doors_refuse_even_after_first_sequence() {
    let mut s = setup();
    let request = first(&mut s, 90400);
    s.execute("agent.apply", request).unwrap();
    read(&mut s, 90401);
    let before = state(&s);
    for command in ["agent.validateSnapshot", "agent.prepareApply", "agent.inspect", "agent.readSourceSelection", "agent.readBasis", "agent.selectSequence"] {
        let e = s.execute(command, json!({"token":token(90401),"scope":scope()})).unwrap_err();
        assert!(e.to_string().contains("project-plane"), "{command}: {e}");
        assert_eq!(state(&s), before);
    }
    let mut request = bin(&mut s, 90402, json!({"type":"create","name":"Wrong authority","parent":null}));
    request["authority"] = json!("media_inputs");
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn project_plane_hostile_or_foreign_scope_and_unsaved_path_never_mint_a_project_ticket() {
    let mut s = setup();
    let before = state(&s);
    for scope in [
        json!({"projectId":"foreign","workspaceId":WORKSPACE}),
        json!({"projectId":"p_project","workspaceId":"26d3c4b8-5f15-44b7-96b9-c99614547f0b"}),
        json!({"projectId":"p_project","workspaceId":WORKSPACE,"anchorSequence":1}),
        json!({"projectId":"p_project","anchorSequence":0}),
        json!(null),
    ] {
        assert!(s.execute(project_plane::READ, json!({"token":token(90500),"scope":scope,"projectPath":s.path})).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(s.execute(project_plane::READ, json!({"token":token(90500),"scope":scope(),"projectPath":"/foreign.fcproj"})).is_err());
    assert!(s.agent_bridge.tickets.is_empty());
    s.path = None;
    assert!(s.execute(project_plane::READ, json!({"token":token(90501),"scope":scope(),"projectPath":"/generated/project.fcproj"})).is_err());
}
#[test]
fn project_plane_stale_cow_context_and_whole_project_basis_preserve_later_human_work() {
    let mut s = setup();
    let request = bin(&mut s, 90600, json!({"type":"create","name":"Stale","parent":null}));
    s.execute("file.newBin", json!({"name":"Human"})).unwrap();
    let before = state(&s);
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
    let request = bin(&mut s, 90601, json!({"type":"create","name":"Stale context","parent":null}));
    s.state.project_selection = vec![ItemId(1)];
    let before = state(&s);
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
    let request = first(&mut s, 90602);
    s.project = Arc::new((*s.project).clone());
    let before = state(&s);
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
    let mut request = first(&mut s, 90603);
    request["operations"][0]["params"]["beforeSha256"] = json!("f".repeat(64));
    let before = state(&s);
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn project_plane_missing_media_stays_metadata_and_never_becomes_current_source_evidence() {
    let mut s = setup();
    let (demo, _) = crate::demo::demo_project(&s.media);
    let mut item = demo.items.values().find(|it| it.as_media().is_some()).unwrap().clone();
    item.id = ItemId(1000);
    if let filmcraft_project::ItemKind::Media(media) = &mut item.kind {
        media.media = filmcraft_project::MediaRef::File { path: "/generated/missing-original.mov".into() };
    }
    let p = Arc::make_mut(&mut s.project);
    p.items.insert(item.id, item.clone());
    p.root.children.push(BinEntry::Item(item.id));
    let before = s.project.clone();
    let request = bin(&mut s, 90700, json!({"type":"create","name":"Offline metadata","parent":null}));
    owned_cycle(&mut s, 90700, &request, before);
    assert_eq!(s.project.item(item.id), Some(&item));
    let d = read(&mut s, 90703);
    assert_eq!(d["mediaEvidence"], false);
    assert!(d.get("sources").is_none());
}
#[test]
fn project_plane_lost_reply_reconciles_original_receipt_after_human_changes_without_a_second_publication() {
    let mut s = setup();
    let request = first(&mut s, 90800);
    let result = s.execute("agent.apply", request.clone()).unwrap();
    s.execute("file.newBin", json!({"name":"Human after receipt"})).unwrap();
    let before = state(&s);
    assert_eq!(s.execute("agent.apply", request.clone()).unwrap(), result);
    assert_eq!(state(&s), before);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(90800)})).unwrap()["result"], result);
    let mut changed = request;
    changed["label"] = json!("Different");
    assert!(s.execute("agent.apply", changed).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 90801);
    assert!(s.execute("agent.undo", json!({"token":token(90801),"commit":token(90800)})).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn project_plane_stop_uses_existing_operation_ledger_and_cannot_publish_or_replay_first_sequence() {
    let mut s = setup();
    let request = first(&mut s, 90900);
    let before = state(&s);
    let control = s.agent_operation_control();
    let run = super::commands().into_iter().find(|c| c.id == "agent.apply").unwrap().run;
    assert!(
        super::operation_control::run(&mut s, "agent.apply", &request, |s, p| {
            assert_eq!(control.cancel(&json!({"token":token(90900)})).unwrap()["cancellationAccepted"], true);
            run(s, p)
        })
        .is_err()
    );
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", request).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn project_plane_paged_bins_and_queries_remain_scoped_metadata_with_explicit_absent_sequence() {
    let mut s = setup();
    for i in 0..70 {
        s.execute("file.newBin", json!({"name":format!("Bin{i}")})).unwrap();
        s.execute("file.newSearchBin", json!({"name":format!("Query{i}"),"text":"Bin"})).unwrap();
    }
    read(&mut s, 91000);
    let mut total = 0;
    let mut offset = 0;
    loop {
        let p = s.execute("agent.readBins", json!({"token":token(91000),"scope":scope(),"offset":offset,"limit":32})).unwrap();
        assert!(p["sequence"].is_null());
        total += p["entries"].as_array().unwrap().len();
        let Some(next) = p["nextOffset"].as_u64() else { break };
        offset = next;
    }
    assert_eq!(total, 71);
    let mut total = 0;
    let mut offset = 0;
    loop {
        let p = s.execute("agent.readSearchBins", json!({"token":token(91000),"scope":scope(),"query":null,"offset":offset,"limit":32})).unwrap();
        assert!(p["sequence"].is_null());
        total += p["entries"].as_array().unwrap().len();
        let Some(next) = p["nextOffset"].as_u64() else { break };
        offset = next;
    }
    assert_eq!(total, 70);
}
#[test]
fn project_plane_legacy_sequence_tickets_cannot_borrow_project_scope_or_owned_history() {
    let mut s = setup();
    let request = first(&mut s, 91100);
    s.execute("agent.apply", request).unwrap();
    let sequence = s.state.active_sequence.unwrap();
    super::tests::read(&mut s, 91101);
    assert!(s.execute("agent.readBins", json!({"token":token(91101),"scope":scope(),"offset":0,"limit":32})).is_err());
    let legacy = json!({"projectId":"p_project","anchorSequence":sequence.0});
    let h = s.execute("agent.readHistory", json!({"token":token(91101),"scope":legacy,"commit":token(91100),"undo":null})).unwrap();
    assert_eq!(h["undoAvailable"], false);
    read(&mut s, 91102);
    assert!(s.execute("agent.readBins", json!({"token":token(91102),"scope":legacy,"offset":0,"limit":32})).is_err());
}

#[test]
fn project_plane_metadata_history_preserves_human_tab_navigation_without_borrowing_legacy_sequence_ownership() {
    let mut s = setup();
    let a = s.execute("file.newSequence", json!({"name":"First"})).unwrap()["sequence"].as_u64().unwrap();
    let b = s.execute("file.newSequence", json!({"name":"Second"})).unwrap()["sequence"].as_u64().unwrap();
    let before = s.project.clone();
    let request = bin(&mut s, 91200, json!({"type":"create","name":"Metadata only","parent":null}));
    s.execute("agent.apply", request).unwrap();
    let after = s.project.clone();
    s.execute("sequence.open", json!({"item":a})).unwrap();
    read(&mut s, 91201);
    assert_eq!(s.execute("agent.readHistory", json!({"token":token(91201),"scope":scope(),"commit":token(91200),"undo":null})).unwrap()["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(91201),"commit":token(91200)})).unwrap();
    assert_eq!(s.project, before);
    assert_eq!(s.state.active_sequence, Some(ItemId(a)));
    s.execute("sequence.open", json!({"item":b})).unwrap();
    read(&mut s, 91202);
    s.execute("agent.redo", json!({"token":token(91202),"commit":token(91200),"undo":token(91201)})).unwrap();
    assert_eq!(s.project, after);
    assert_eq!(s.state.active_sequence, Some(ItemId(b)));
}
#[test]
fn project_plane_owned_initial_sequence_can_be_undone_after_closing_its_tab_and_redone_to_its_same_anchor() {
    let mut s = setup();
    let before = s.project.clone();
    let request = first(&mut s, 91300);
    s.execute("agent.apply", request).unwrap();
    let id = s.state.active_sequence.unwrap();
    let after = s.project.clone();
    s.execute("sequence.close", json!({"item":id.0})).unwrap();
    assert!(s.state.active_sequence.is_none());
    read(&mut s, 91301);
    assert_eq!(s.execute("agent.readHistory", json!({"token":token(91301),"scope":scope(),"commit":token(91300),"undo":null})).unwrap()["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(91301),"commit":token(91300)})).unwrap();
    assert_eq!(s.project, before);
    assert!(s.state.active_sequence.is_none());
    read(&mut s, 91302);
    s.execute("agent.redo", json!({"token":token(91302),"commit":token(91300),"undo":token(91301)})).unwrap();
    assert_eq!(s.project, after);
    assert_eq!(s.state.active_sequence, Some(id));
}

#[test]
fn project_plane_guarded_client_capabilities_require_an_actually_installed_independent_stop_transport() {
    let mut s = setup();
    let d = read(&mut s, 91400);
    assert_eq!(d["capabilities"]["operationCancellation"], false);
    assert_eq!(d["capabilities"]["guardedOperationDeadlineMs"], 45000);
    assert_eq!(d["capabilities"]["guardedHistoryRead"], true);
    assert_eq!(d["capabilities"]["projectHistoryCheckpointOperations"], true);
    assert_eq!(d["capabilities"]["projectHistoryCheckpointRetention"], true);
    assert!(d["capabilities"].get("sourceFileGuard").is_none());
    s.agent_operation_control().enable_transport();
    let d = read(&mut s, 91401);
    assert_eq!(d["capabilities"]["operationCancellation"], true);
    assert_eq!(d["sourceValidation"], "not-performed");
    assert_eq!(d["mediaEvidence"], false);
}

fn sequence_binding(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::SEQUENCE_SCOPE, json!({"token":token(n),"scope":scope()})).unwrap()
}
#[test]
fn project_sequence_scope_reads_genuine_absence_without_source_or_mutation_authority() {
    let mut s = setup();
    let d = read(&mut s, 96000);
    let before = state(&s);
    let r = sequence_binding(&mut s, 96000);
    assert_eq!(state(&s), before);
    assert_eq!(r["state"], "absent");
    assert!(r["sequenceScope"].is_null());
    assert!(r["sequence"].is_null());
    assert_eq!(r["scope"], scope());
    assert_eq!(r["projectPath"], d["project"]["path"]);
    assert_eq!(r["sha256"], d["projectState"]["sha256"]);
    assert_eq!(r["activeOwned"], false);
    assert_eq!(r["requiresFreshSequenceRead"], true);
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(r["writeAuthority"], false);
}
#[test]
fn project_sequence_scope_uses_the_existing_real_anchor_across_human_tabs_and_closed_views() {
    let mut s = setup();
    let request = first(&mut s, 96010);
    let a = s.execute("agent.apply", request).unwrap()["sequence"].as_u64().unwrap();
    let b = s.execute("file.newSequence", json!({"name":"Second human sequence"})).unwrap()["sequence"].as_u64().unwrap();
    assert_ne!(a, b);
    for (n, id) in [(96011, a), (96012, b)] {
        s.execute("sequence.open", json!({"item":id})).unwrap();
        read(&mut s, n);
        let before = state(&s);
        let r = sequence_binding(&mut s, n);
        assert_eq!(state(&s), before);
        assert_eq!(r["state"], "current");
        assert_eq!(r["sequence"], id);
        assert_eq!(r["sequenceScope"], json!({"projectId":"p_project","anchorSequence":a}));
        assert_eq!(r["activeOwned"], true);
    }
    s.execute("sequence.close", json!({"item":b})).unwrap();
    if let Some(id) = s.state.active_sequence {
        s.execute("sequence.close", json!({"item":id.0})).unwrap();
    }
    read(&mut s, 96013);
    let r = sequence_binding(&mut s, 96013);
    assert_eq!(r["state"], "closed");
    assert!(r["sequence"].is_null());
    assert_eq!(r["sequenceScope"]["anchorSequence"], a);
    assert_eq!(r["activeOwned"], false);
}
#[test]
fn project_sequence_scope_refuses_stale_foreign_legacy_and_media_aliases_atomically() {
    let mut s = setup();
    let request = first(&mut s, 96020);
    s.execute("agent.apply", request).unwrap();
    read(&mut s, 96021);
    let before = state(&s);
    for request in [
        json!({"token":token(96021),"scope":{"projectId":"foreign","workspaceId":WORKSPACE}}),
        json!({"token":token(96021),"scope":{"projectId":"p_project","workspaceId":WORKSPACE,"anchorSequence":1}}),
        json!({"token":token(96021),"scope":scope(),"sourceValidation":"verified"}),
    ] {
        assert!(s.execute(project_plane::SEQUENCE_SCOPE, request).is_err());
        assert_eq!(state(&s), before);
    }
    s.execute("agent.snapshot", json!({"token":token(96022)})).unwrap();
    assert!(s.execute(project_plane::SEQUENCE_SCOPE, json!({"token":token(96022),"scope":scope()})).is_err());
    assert_eq!(state(&s), before);
    s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let changed = state(&s);
    assert!(s.execute(project_plane::SEQUENCE_SCOPE, json!({"token":token(96021),"scope":scope()})).is_err());
    assert_eq!(state(&s), changed);
}
#[test]
fn project_sequence_scope_never_substitutes_an_unowned_or_oversized_secondary_sequence() {
    let mut s = setup();
    let request = first(&mut s, 96030);
    let anchor = s.execute("agent.apply", request).unwrap()["sequence"].as_u64().unwrap();
    let b = s.execute("file.newSequence", json!({"name":"Second human sequence"})).unwrap()["sequence"].as_u64().unwrap();
    Arc::make_mut(&mut s.project).item_mut(ItemId(b)).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
    read(&mut s, 96031);
    let before = state(&s);
    let r = sequence_binding(&mut s, 96031);
    assert_eq!(state(&s), before);
    assert_eq!(r["state"], "outside_scope");
    assert_eq!(r["sequence"], b);
    assert_eq!(r["sequenceScope"]["anchorSequence"], anchor);
    assert_eq!(r["activeOwned"], false);
    Arc::make_mut(&mut s.project).item_mut(ItemId(b)).unwrap().metadata.insert(crate::editorial_workspace::SEQUENCE_KEY.into(), "x".repeat(2049));
    read(&mut s, 96032);
    let before = state(&s);
    assert!(s.execute(project_plane::SEQUENCE_SCOPE, json!({"token":token(96032),"scope":scope()})).is_err());
    assert_eq!(state(&s), before);
}
