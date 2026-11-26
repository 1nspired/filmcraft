//! Generated complete human Clear parity, exact private proof and owned history.
use super::{Batch, Operation, project_clear, project_plane, tests::token};
use crate::Session;
use filmcraft_project::ItemId;
use serde_json::{Value, json};
use std::sync::Arc;
const W: &str = "0b96619d-cf57-4b36-86ca-f404b84bd245";
const C: &str = "e9d6a409-75be-449d-8146-ea7072b461f8";
fn scope() -> Value {
    json!({"projectId":"p_typed_clear","workspaceId":W})
}
fn own(s: &mut Session) {
    let mut owner = Session::default();
    owner.execute("file.newProject", json!({"name":"Root identity","workspace":scope()})).unwrap();
    let project = Arc::make_mut(&mut s.project);
    project.metadata.insert(crate::project_workspace::KEY.into(), owner.project.metadata[crate::project_workspace::KEY].clone());
    let ids = project.sequences().map(|it| it.id).collect::<Vec<_>>();
    if let Some(first) = ids.first() {
        let association =
            crate::editorial_workspace::validate(&json!({"scope":{"projectId":"p_typed_clear","anchorSequence":first.0},"workspaceId":W})).unwrap();
        for id in &ids {
            let item = project.item_mut(*id).unwrap();
            if id == first {
                item.metadata.insert(crate::editorial_workspace::KEY.into(), serde_json::to_string(&association).unwrap());
            } else {
                item.metadata
                    .insert(crate::editorial_workspace::SEQUENCE_KEY.into(), json!({"projectId":"p_typed_clear","anchorSequence":first.0}).to_string());
            }
        }
    }
    s.path = Some(crate::temp_dir().join("generated-typed-clear.fcproj").to_string_lossy().into_owned());
}
fn empty() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated typed Clear","workspace":scope()})).unwrap();
    s.path = Some(crate::temp_dir().join("generated-typed-clear.fcproj").to_string_lossy().into_owned());
    s
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn batch(s: &mut Session, n: usize, entries: &[u64]) -> Value {
    let d = read(s, n);
    json!({"token":token(n),"label":"Clear","exact":true,"authority":"document_state","operations":[{"command":project_clear::COMMAND,"params":{"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"entries":entries}}]})
}
fn full(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"clipboardMarkers":s.state.clipboard_markers,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"offline":s.offline})
}
fn human(s: &Session) -> Session {
    Session { project: s.project.clone(), state: s.state.clone(), history: s.history.clone(), revision: s.revision, path: s.path.clone(), ..Session::default() }
}
fn same(a: &Session, b: &Session) {
    assert_eq!(a.project.to_json(), b.project.to_json());
    assert_eq!(project_clear::editor_hash(&a.state).unwrap(), project_clear::editor_hash(&b.state).unwrap());
    assert_eq!(a.offline.missing, b.offline.missing);
}
fn cycle(s: &mut Session, h: &mut Session, commit: usize, n: usize) {
    read(s, n);
    h.execute("edit.undo", json!({})).unwrap();
    s.execute("agent.undo", json!({"token":token(n),"commit":token(commit)})).unwrap();
    same(s, h);
    read(s, n + 1);
    h.execute("edit.redo", json!({})).unwrap();
    s.execute("agent.redo", json!({"token":token(n+1),"commit":token(commit),"undo":token(n)})).unwrap();
    same(s, h);
}
#[test]
fn typed_clear_matches_complete_shared_human_service_for_media_bins_subclips_and_sequences() {
    for seed in 0..16usize {
        let (mut s, targets) = crate::project_clear_tests::fixture(seed as u64);
        own(&mut s);
        let mut entries =
            targets.iter().enumerate().filter(|(i, id)| id.0 > 0 && id.0 != 999_999_999 && (i + seed) % 3 == 0).map(|(_, id)| id.0).collect::<Vec<_>>();
        entries.sort_unstable();
        entries.dedup();
        if entries.is_empty() {
            entries.push(targets[0].0);
        }
        let mut h = human(&s);
        let request = batch(&mut s, 110000 + seed * 10, &entries);
        let before = s.revision;
        let result = s.execute("agent.apply", request).unwrap();
        let counts = h.execute("project.delete", json!({"items":entries})).unwrap();
        assert_eq!(result["action"], "project_clear");
        assert_eq!(result["authority"], "document_state");
        assert_eq!(result["results"][0]["humanResult"], counts);
        assert_eq!(s.revision, before + 1);
        same(&s, &h);
        cycle(&mut s, &mut h, 110000 + seed * 10, 110001 + seed * 10);
    }
}
#[test]
fn typed_clear_owning_and_last_sequence_use_original_human_undo_redo_view_behavior() {
    for last in [false, true] {
        let mut s = empty();
        let ids = (0..3)
            .map(|i| {
                s.execute("file.newSequence", json!({"name":format!("Sequence{i}"),"width":64,"height":48,"fps":25})).unwrap()["sequence"].as_u64().unwrap()
            })
            .collect::<Vec<_>>();
        let entries = if last { ids.clone() } else { vec![ids[0]] };
        let mut h = human(&s);
        let request = batch(&mut s, 111000, &entries);
        let result = s.execute("agent.apply", request).unwrap();
        assert_eq!(result["results"][0]["beforeSequenceScope"]["anchorSequence"], ids[0]);
        assert_eq!(result["results"][0]["sequenceScope"], if last { Value::Null } else { json!({"projectId":"p_typed_clear","anchorSequence":ids[1]}) });
        h.execute("project.delete", json!({"items":entries})).unwrap();
        same(&s, &h);
        cycle(&mut s, &mut h, 111000, 111001);
        assert!(crate::editorial_workspace::association(&s.project).is_ok());
    }
}
#[test]
fn typed_clear_refuses_stale_editor_and_both_clipboards_atomically_even_without_a_project_edit() {
    for mode in 0..3 {
        let mut s = empty();
        let bin = s.execute("file.newBin", json!({"name":"Keep on refusal"})).unwrap()["bin"].as_u64().unwrap();
        let request = batch(&mut s, 112000, &[bin]);
        if mode == 0 {
            s.state.project_selection.push(ItemId(bin));
        } else if mode == 1 {
            let demo = crate::project_clear_tests::fixture(0).0;
            let clip = demo.project.sequences().next().unwrap().as_sequence().unwrap().video_tracks[0].items[0].clone();
            s.state.clipboard.push((filmcraft_project::TrackKind::Video, 0, clip));
        } else {
            let marker: filmcraft_project::Marker =
                serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"Human clipboard marker","comment":"","color":"Iris","kind":"Comment"}))
                    .unwrap();
            s.state.clipboard_markers.push(marker);
        }
        let before = full(&s);
        let project = s.project.clone();
        assert!(s.execute("agent.apply", request).is_err());
        assert_eq!(full(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
    }
}
#[test]
fn typed_clear_historical_predicate_rejects_collateral_declarations_effects_or_private_editor_tampering() {
    let (mut s, targets) = crate::project_clear_tests::fixture(0);
    own(&mut s);
    let request = batch(&mut s, 113000, &[targets[0].0]);
    let before = s.project.clone();
    let result = s.execute("agent.apply", request.clone()).unwrap();
    let op: Operation = super::parse(project_clear::COMMAND, &request["operations"][0]).unwrap();
    let receipt = result["results"][0].clone();
    project_clear::validate_transition(&before, &s.project, &op, &receipt).unwrap();
    for change in 0..6 {
        let mut bad = receipt.clone();
        match change {
            0 => bad["effects"]["offlined"] = json!([]),
            1 => bad["editorProof"]["afterEditor"]["unrecognized"] = json!(true),
            2 => bad["editorProof"]["clipboardSha256"] = json!("a".repeat(64)),
            3 => bad["beforeEditorSha256"] = json!("b".repeat(64)),
            4 => bad["sequence"] = Value::Null,
            _ => bad["humanResult"]["offline"] = json!(999),
        };
        if bad != receipt {
            assert!(project_clear::validate_transition(&before, &s.project, &op, &bad).is_err());
        }
    }
    let mut collateral = (*s.project).clone();
    collateral.name.push_str(" foreign");
    assert!(project_clear::validate_transition(&before, &collateral, &op, &receipt).is_err());
}
#[test]
fn typed_clear_repeated_already_offline_media_keeps_one_human_history_step_with_equal_project_hashes() {
    let (mut s, _) = crate::project_clear_tests::fixture(0);
    own(&mut s);
    let used = s.project.items.values().find(|it| it.as_media().is_some_and(|m| matches!(m.media, filmcraft_project::MediaRef::File { .. }))).unwrap().id;
    s.execute("project.delete", json!({"items":[used.0]})).unwrap();
    let mut h = human(&s);
    let request = batch(&mut s, 114000, &[used.0]);
    let count = s.history.undo.len();
    let result = s.execute("agent.apply", request).unwrap();
    h.execute("project.delete", json!({"items":[used.0]})).unwrap();
    assert_eq!(result["results"][0]["beforeSha256"], result["results"][0]["afterSha256"]);
    assert_eq!(s.history.undo.len(), count + 1);
    same(&s, &h);
    cycle(&mut s, &mut h, 114000, 114001);
}
#[test]
fn typed_clear_closed_schema_root_scope_and_visible_targets_refuse_mixed_or_foreign_publication() {
    for mode in 0..7 {
        let mut s = empty();
        let bin = s.execute("file.newBin", json!({"name":"Keep"})).unwrap()["bin"].as_u64().unwrap();
        let mut request = batch(&mut s, 115000, &[bin]);
        match mode {
            0 => request["operations"][0]["params"]["entries"] = json!([bin, bin]),
            1 => request["operations"][0]["params"]["entries"] = json!([0]),
            2 => request["operations"][0]["params"]["entries"] = json!([999999]),
            3 => request["operations"][0]["params"]["scope"]["workspaceId"] = json!(C),
            4 => request["operations"][0]["params"]["extra"] = json!(true),
            5 => {
                let op = request["operations"][0].clone();
                request["operations"].as_array_mut().unwrap().push(op);
            }
            _ => request["authority"] = json!("media_inputs"),
        };
        let before = full(&s);
        assert!(s.execute("agent.apply", request).is_err());
        assert_eq!(full(&s), before);
    }
}
#[test]
fn typed_clear_staging_cannot_evict_live_media_and_published_clear_preserves_generated_file_bytes() {
    use crate::media_test_util::{make_movie, session_with, tmp_dir};
    let root = tmp_dir("typed-clear-cache");
    let file = root.join("generated.mov");
    make_movie(&file, filmcraft_media::DemoScene::Aurora, 64, 48, 3);
    let bytes = std::fs::read(&file).unwrap();
    let (mut s, items, _) = session_with(&[&file]);
    own(&mut s);
    let id = items[0];
    assert!(s.source(id).is_some());
    assert!(s.media.cached(id).is_some());
    let request = batch(&mut s, 116000, &[id.0]);
    let parsed: Batch = super::parse("agent.apply", &request).unwrap();
    let stage = super::stage_batch(&s, &parsed).unwrap();
    assert!(s.media.cached(id).is_some(), "unpublished stage cannot evict the live cache");
    drop(stage);
    s.execute("agent.apply", request).unwrap();
    assert!(s.media.cached(id).is_none());
    assert!(s.offline.missing.contains(&id));
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn typed_clear_saved_checkpoint_reopens_exact_project_and_owned_cold_undo_redo_without_recipe_replay() {
    let root = crate::temp_dir().join(format!("typed-clear-history-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let mut s = empty();
    s.path = Some(root.join("generated.fcproj").to_string_lossy().into_owned());
    let first = s.execute("file.newSequence", json!({"name":"Removed owner","width":64,"height":48,"fps":25})).unwrap()["sequence"].as_u64().unwrap();
    s.execute("file.newSequence", json!({"name":"Survivor","width":64,"height":48,"fps":25})).unwrap();
    let before = s.project.clone();
    let request = batch(&mut s, 117000, &[first]);
    s.execute("agent.apply", request).unwrap();
    let after = s.project.clone();
    let persist = |s: &mut Session| {
        filmcraft_format::atomic_write(
            std::path::Path::new(s.path.as_ref().unwrap()),
            &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
        )
        .unwrap();
        s.saved_revision = s.revision;
    };
    persist(&mut s);
    read(&mut s, 117001);
    let saved = s.execute("agent.saveHistoryCheckpoint", json!({"token":token(117001),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap();
    let mut cold = Session::default();
    cold.execute("file.open", json!({"path":s.path})).unwrap();
    read(&mut cold, 117002);
    cold.execute(
        "agent.restoreHistoryCheckpoint",
        json!({"token":token(117002),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}),
    )
    .unwrap();
    read(&mut cold, 117003);
    cold.execute("agent.undo", json!({"token":token(117003),"commit":token(117000)})).unwrap();
    assert_eq!(cold.project.to_json(), before.to_json());
    read(&mut cold, 117004);
    cold.execute("agent.redo", json!({"token":token(117004),"commit":token(117000),"undo":token(117003)})).unwrap();
    assert_eq!(cold.project.to_json(), after.to_json());
    std::fs::remove_dir_all(root).unwrap();
}
