//! Human Clear conformance. The frozen pre-extraction command is an independent
//! reference algorithm; production has exactly one shared service.
use crate::{Result, Session};
use filmcraft_project::{Bin, BinEntry, BinId, ItemId, ItemKind, Label, MediaRef};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};
use std::sync::Arc;

fn legacy_clear(s: &mut Session, asked: Vec<ItemId>) -> Result<Value> {
    // A bin named here goes with everything in it, as in the Project panel of any editor.
    let root = s.project.root.id;
    let named: Vec<filmcraft_project::BinId> =
        asked.iter().map(|i| filmcraft_project::BinId(i.0)).filter(|b| *b != root && s.project.root.find_bin(*b).is_some()).collect();
    // a bin inside another named bin goes with that one
    let bins: Vec<filmcraft_project::BinId> =
        named.iter().copied().filter(|b| !named.iter().any(|o| o != b && s.project.root.find_bin(*o).is_some_and(|o| o.find_bin(*b).is_some()))).collect();
    let mut items: Vec<ItemId> = asked.iter().copied().filter(|i| !named.contains(&filmcraft_project::BinId(i.0))).collect();
    for b in &bins {
        if let Some(bin) = s.project.root.find_bin(*b) {
            bin.all_items(&mut items);
        }
    }
    items.sort_by_key(|i| i.0);
    items.dedup();
    let mut used = std::collections::HashSet::new();
    for it in s.project.sequences().filter(|it| !items.contains(&it.id)) {
        if let Some(q) = it.as_sequence() {
            for t in q.all_tracks() {
                for clip in t
                    .items
                    .iter()
                    .filter(|clip| !items.contains(&clip.item) || s.project.item(clip.item).is_some_and(|item| matches!(item.kind, ItemKind::Media(_))))
                {
                    // Preserve the whole valid file-backed chain, not just its root.
                    if !s.project.resolve_media(clip.item).is_some_and(|(_, media, _)| matches!(media.media, MediaRef::File { .. })) {
                        continue;
                    }
                    let mut id = clip.item;
                    for _ in 0..16 {
                        used.insert(id);
                        match s.project.item(id).map(|item| &item.kind) {
                            Some(ItemKind::Subclip { parent, .. }) => id = *parent,
                            _ => break,
                        }
                    }
                }
            }
        }
    }
    let mut gone = Vec::new();
    let mut offlined = Vec::new();
    let mut retained_subclips = Vec::new();
    for i in items {
        let keep =
            used.contains(&i) && s.project.item(i).and_then(filmcraft_project::ProjectItem::as_media).is_some_and(|m| matches!(m.media, MediaRef::File { .. }));
        if keep {
            offlined.push(i);
        } else if used.contains(&i) && s.project.item(i).is_some_and(|item| matches!(item.kind, ItemKind::Subclip { .. })) {
            retained_subclips.push(i);
        } else {
            gone.push(i);
        }
    }
    s.edit("Clear", |pr, st| {
        for i in &offlined {
            if let Some(m) = pr.item_mut(*i).and_then(filmcraft_project::ProjectItem::as_media_mut) {
                m.offline = true;
            }
        }
        for b in &bins {
            pr.root.remove_bin(*b);
        }
        for i in offlined.iter().chain(&retained_subclips) {
            if pr.root.parent_of(*i).is_none() {
                pr.root.children.push(filmcraft_project::BinEntry::Item(*i));
            }
        }
        for i in &gone {
            pr.items.remove(i);
            pr.root.remove_item(*i);
        }
        let seq_ids: Vec<ItemId> = pr.sequences().map(|q| q.id).collect();
        for sid in seq_ids {
            if let Some(q) = pr.sequence_mut(sid) {
                for t in q.all_tracks_mut() {
                    t.items.retain(|x| !gone.contains(&x.item));
                    filmcraft_edit::remove_orphan_transitions(t);
                }
            }
        }
        st.project_selection.clear();
        Ok(())
    })?;
    for i in gone.iter().chain(&offlined) {
        s.media.remove(*i);
    }
    crate::relink::refresh(s);
    s.fix_state();
    Ok(json!({"items": gone.len(), "bins": bins.len(), "offline": offlined.len()}))
}
fn inspect(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"offline":s.offline,"revision":s.revision,
        "undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}
pub(crate) fn fixture(seed: u64) -> (Session, Vec<ItemId>) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let project = Arc::make_mut(&mut s.project);
    let outer = project.add_bin("Outer", None);
    let inner = project.add_bin("Nested", Some(outer));
    let spare = project.add_bin("Spare", None);
    let media = project.items.values().filter(|item| item.as_media().is_some()).map(|item| item.id).collect::<Vec<_>>();
    let mut targets = vec![ItemId(outer.0), ItemId(inner.0), ItemId(spare.0), ItemId(project.root.id.0)];
    for (index, id) in media.iter().enumerate() {
        if let Some(clip) = project.item_mut(*id).and_then(filmcraft_project::ProjectItem::as_media_mut) {
            if index % 2 == 0 {
                clip.media = MediaRef::File { path: std::env::temp_dir().join(format!("generated-clear-absent-{}", id.0)).to_string_lossy().into_owned() };
                clip.offline = seed % 2 == 0;
            }
        }
        project.move_to_bin(&[*id], Some(if index % 2 == 0 { inner } else { spare })).unwrap();
    }
    let first = *media.first().unwrap();
    let range = TimeRange::new(Tick::ZERO, project.item(first).unwrap().duration());
    let parent = project.add_item("Retained parent", Label::Iris, ItemKind::Subclip { parent: first, range, restrict_trims: false }, Some(inner));
    let subclip = project.add_item("Surviving outer subclip", Label::Iris, ItemKind::Subclip { parent, range, restrict_trims: false }, None);
    let sequences = project.sequences().map(|item| item.id).collect::<Vec<_>>();
    for id in &sequences {
        if let Some(sequence) = project.sequence_mut(*id) {
            for track in sequence.all_tracks_mut() {
                for clip in &mut track.items {
                    if clip.item == first {
                        clip.item = subclip;
                    }
                }
            }
        }
    }
    targets.extend(media);
    targets.extend([parent, subclip]);
    targets.extend(sequences);
    targets.push(ItemId(999_999_999));
    s.state.project_selection = vec![first, ItemId(outer.0)];
    s.state.source_item = Some(parent);
    s.state.source_history = vec![parent, first, subclip];
    (s, targets)
}

#[test]
fn clear_service_matches_frozen_human_command_for_nested_overlap_unknown_root_and_subclip_cases() {
    for seed in 0..64u64 {
        let (mut actual, targets) = fixture(seed);
        let mut legacy = Session::default();
        legacy.project = actual.project.clone();
        legacy.state = actual.state.clone();
        legacy.revision = actual.revision;
        // Give both independent owners the same pre-existing history and Redo.
        legacy.history = actual.history.clone();
        legacy.journal = actual.journal.clone();
        let mut asked = targets
            .iter()
            .enumerate()
            .filter(|(i, _)| (seed.rotate_left(*i as u32) ^ (*i as u64).wrapping_mul(17)) % 4 == 0)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();
        if let Some(id) = asked.first().copied() {
            asked.extend([id, id]);
        }
        let predicted = crate::project_clear::predict(&actual.project, &asked).unwrap().0;
        let expected = legacy_clear(&mut legacy, asked.clone()).unwrap();
        let result = crate::project_clear::run(&mut actual, &asked).unwrap();
        assert_eq!(result, expected, "receipt seed {seed}: {asked:?}");
        assert_eq!(actual.project.to_json(), predicted.to_json(), "prediction seed {seed}");
        let actual_state = inspect(&actual);
        let legacy_state = inspect(&legacy);
        for key in ["project", "editor", "offline", "revision", "undo", "redo", "merge", "journal"] {
            assert_eq!(actual_state[key], legacy_state[key], "{key} seed {seed}: {asked:?}");
        }
        for redo in [false, true] {
            if redo {
                actual.redo().unwrap();
                legacy.redo().unwrap();
            } else {
                actual.undo().unwrap();
                legacy.undo().unwrap();
            }
            assert_eq!(inspect(&actual), inspect(&legacy), "owned history seed {seed}, redo {redo}");
        }
    }
}

#[test]
fn clear_prediction_is_read_only_and_includes_offline_only_changes_with_an_identical_tree() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Survivor","fps":25,"video":1,"audio":0,"width":16,"height":16})).unwrap();
    let id = ItemId(
        s.execute("file.newOfflineFile", json!({"name":"Used","seconds":2,"fps":25,"video":true,"audio":false,"width":16,"height":16})).unwrap()["item"]
            .as_u64()
            .unwrap(),
    );
    s.execute("timeline.place", json!({"item":id.0,"track":"V1","time":0})).unwrap();
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().offline = false;
    let original = s.project.clone();
    let before = inspect(&s);
    let (predicted, effects) = crate::project_clear::predict(&s.project, &[id]).unwrap();
    assert_eq!(inspect(&s), before);
    assert!(Arc::ptr_eq(&s.project, &original));
    assert_eq!(effects.offlined, vec![id]);
    assert!(effects.removed.is_empty());
    assert!(effects.bins.is_empty());
    assert!(effects.retained_subclips.is_empty());
    assert_eq!(predicted.root, original.root);
    assert_eq!(predicted.items.len(), original.items.len());
    let mut expected = (*original).clone();
    expected.item_mut(id).unwrap().as_media_mut().unwrap().offline = true;
    assert_eq!(predicted.to_json(), expected.to_json());
    let undo = s.history.undo.len();
    assert_eq!(s.execute("project.delete", json!({"items":[id.0]})).unwrap(), json!({"items":0,"bins":0,"offline":1}));
    assert_eq!(s.project.to_json(), predicted.to_json());
    assert_eq!(s.history.undo.len(), undo + 1);
    s.undo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &original));
    s.redo().unwrap();
    assert_eq!(s.project.to_json(), predicted.to_json());
}

#[test]
fn clear_preserves_human_duplicate_bin_counts_and_one_step_for_empty_unknown_and_root_requests() {
    for mode in 0..4 {
        let mut s = Session::default();
        let bin = s.execute("file.newBin", json!({"name":"Human bin"})).unwrap()["bin"].as_u64().unwrap();
        let asked = match mode {
            0 => vec![ItemId(bin), ItemId(bin)],
            1 => vec![],
            2 => vec![ItemId(999_999)],
            _ => vec![ItemId(s.project.root.id.0)],
        };
        let before = s.project.clone();
        let undo = s.history.undo.len();
        let result = crate::project_clear::run(&mut s, &asked).unwrap();
        assert_eq!(
            result,
            match mode {
                0 => json!({"items":0,"bins":2,"offline":0}),
                1 => json!({"items":0,"bins":0,"offline":0}),
                _ => json!({"items":1,"bins":0,"offline":0}),
            }
        );
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(s.history.undo.last().unwrap().0, "Clear");
        s.undo().unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
    }
}

#[test]
fn clear_hostile_tree_depth_ambiguous_ids_and_selection_bounds_refuse_before_any_edit() {
    for mode in 0..4 {
        let mut s = Session::default();
        let asked = if mode == 0 { vec![ItemId(1); 100_001] } else { vec![ItemId(1)] };
        let project = Arc::make_mut(&mut s.project);
        match mode {
            1 => {
                let mut child = Bin { id: BinId(500), name: "Leaf".into(), children: vec![] };
                for id in (100..166).rev() {
                    child = Bin { id: BinId(id), name: "Deep".into(), children: vec![BinEntry::Bin(child)] };
                }
                project.root.children.push(BinEntry::Bin(child));
            }
            2 => {
                project.root.children.extend([BinEntry::Item(ItemId(1)), BinEntry::Item(ItemId(1))]);
            }
            3 => {
                project.root.children = vec![BinEntry::Item(ItemId(1)); 100_001];
            }
            _ => {}
        }
        let original = s.project.clone();
        let editor = serde_json::to_value(&s.state).unwrap();
        let revision = s.revision;
        let undo = s.history.undo.len();
        let redo = s.history.redo.len();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::project_clear::run(&mut s, &asked)));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        assert!(Arc::ptr_eq(&s.project, &original));
        assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
        assert_eq!(s.history.redo.len(), redo);
    }
}

#[test]
fn clear_used_generated_file_evicts_native_cache_refreshes_relink_and_never_deletes_media() {
    use crate::media_test_util::{make_movie, session_with, tmp_dir};
    let root = tmp_dir("clear-source-preserved");
    let file = root.join("source.mov");
    make_movie(&file, filmcraft_media::DemoScene::Aurora, 64, 48, 3);
    let bytes = std::fs::read(&file).unwrap();
    let (mut s, items, _) = session_with(&[&file]);
    let id = items[0];
    assert!(s.source(id).is_some());
    assert!(s.media.cached(id).is_some());
    let before = s.project.clone();
    let (candidate, effects) = crate::project_clear::predict(&s.project, &[id]).unwrap();
    assert_eq!(effects.offlined, vec![id]);
    assert!(s.media.cached(id).is_some(), "prediction cannot evict caches");
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let count = s.history.undo.len();
    s.execute("project.delete", json!({"items":[id.0]})).unwrap();
    assert_eq!(s.history.undo.len(), count + 1);
    assert_eq!(s.project.to_json(), candidate.to_json());
    assert!(s.media.cached(id).is_none());
    assert!(s.offline.missing.contains(&id));
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    s.undo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    assert!(s.source(id).is_some());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn clear_root_anchor_with_surviving_owned_sequences_keeps_metadata_and_real_scope_usable() {
    let mut s = Session::default();
    let scope = json!({"projectId":"p_clear_root","workspaceId":"1d41b87b-4763-4ee4-a076-8cd9fef00a21"});
    s.execute("file.newProject", json!({"name":"Clear original owner anchor","workspace":scope})).unwrap();
    s.path = Some("/generated/clear-root.fcproj".into());
    let first = ItemId(
        s.execute("file.newSequence", json!({"name":"First anchor","fps":25,"width":64,"height":48,"video":1,"audio":1})).unwrap()["sequence"]
            .as_u64()
            .unwrap(),
    );
    let second = ItemId(
        s.execute("file.newSequence", json!({"name":"Surviving real sequence","fps":25,"width":64,"height":48,"video":1,"audio":1})).unwrap()["sequence"]
            .as_u64()
            .unwrap(),
    );
    assert_eq!(crate::editorial_workspace::association(&s.project).unwrap().unwrap().scope.anchor_sequence, first.0);
    let before = s.project.clone();
    let history = s.history.undo.len();
    s.execute("project.delete", json!({"items":[first.0]})).unwrap();
    assert_eq!(s.history.undo.len(), history + 1);
    assert!(s.project.sequence(first).is_none());
    assert!(s.project.sequence(second).is_some());
    let read = |s: &mut Session, token: &str| s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path}));
    let after = read(&mut s, "root-clear-after-00000000000000000001");
    assert!(after.is_ok(), "Clearing the original anchor must not strand the owning Root: {after:?}");
    let after = after.unwrap();
    assert_eq!(after["sequence"], second.0);
    assert_eq!(after["sourceValidation"], "not-performed");
    assert_eq!(after["writeAuthority"], false);
    let association = crate::editorial_workspace::association(&s.project).unwrap().unwrap();
    assert_eq!(association.scope.anchor_sequence, second.0);
    let authored = s.project.clone();
    s.undo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    assert!(read(&mut s, "root-clear-undo-000000000000000000001").is_ok());
    s.redo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &authored));
    assert!(read(&mut s, "root-clear-redo-000000000000000000001").is_ok());
}

fn root_clear_fixture() -> (Session, Value, Vec<ItemId>) {
    let mut s = Session::default();
    let scope = json!({"projectId":"p_retirement","workspaceId":"cae9db44-642d-456a-8ca6-ecf7b68d5290"});
    s.execute("file.newProject", json!({"name":"Generated retirement lifecycle","workspace":scope})).unwrap();
    s.path = Some("/generated/retirement.fcproj".into());
    let mut ids = Vec::new();
    for name in ["Original anchor", "Owned successor", "Owned follower"] {
        ids.push(ItemId(
            s.execute("file.newSequence", json!({"name":name,"fps":25,"video":1,"audio":1,"width":64,"height":48})).unwrap()["sequence"].as_u64().unwrap(),
        ));
    }
    (s, scope, ids)
}
fn root_sequence_binding(s: &mut Session, scope: &Value, number: u64) -> Value {
    let token = format!("root-retirement-{number:030}");
    s.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":s.path})).unwrap();
    s.execute("agent.readProjectSequenceScope", json!({"token":token,"scope":scope})).unwrap()
}
#[test]
fn clear_retired_anchor_continues_only_known_owned_followers_through_save_and_human_undo_redo() {
    let (mut s, scope, ids) = root_clear_fixture();
    let root_identity = crate::project_workspace::identity(&s.project).unwrap();
    let before = s.project.clone();
    let predicted = crate::project_clear::predict(&s.project, &[ids[0]]).unwrap().0;
    s.execute("project.delete", json!({"items":[ids[0].0]})).unwrap();
    assert_eq!(s.project.to_json(), predicted.to_json());
    assert_eq!(crate::project_workspace::identity(&s.project).unwrap(), root_identity);
    assert_eq!(crate::project_workspace::retired_anchor_ids(&s.project).unwrap(), vec![ids[0].0]);
    assert_eq!(s.state.active_sequence, Some(ids[2]));
    let binding = root_sequence_binding(&mut s, &scope, 1);
    assert_eq!(binding["state"], "current");
    assert_eq!(binding["sequence"], ids[2].0);
    assert_eq!(binding["sequenceScope"]["anchorSequence"], ids[1].0);
    assert_eq!(binding["activeOwned"], true);
    assert_eq!(binding["mediaEvidence"], false);
    assert_eq!(binding["writeAuthority"], false);
    let decoded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project;
    assert_eq!(decoded.to_json(), s.project.to_json());
    assert_eq!(crate::project_workspace::retired_anchor_ids(&decoded).unwrap(), vec![ids[0].0]);
    let after = s.project.clone();
    s.undo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    assert_eq!(root_sequence_binding(&mut s, &scope, 2)["sequenceScope"]["anchorSequence"], ids[0].0);
    s.redo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(root_sequence_binding(&mut s, &scope, 3)["sequenceScope"]["anchorSequence"], ids[1].0);
    // Repeat with a follower whose original marker still names the first anchor.
    s.execute("project.delete", json!({"items":[ids[1].0]})).unwrap();
    let binding = root_sequence_binding(&mut s, &scope, 4);
    assert_eq!(binding["sequenceScope"]["anchorSequence"], ids[2].0);
    assert_eq!(binding["activeOwned"], true);
    assert_eq!(crate::project_workspace::retired_anchor_ids(&s.project).unwrap(), vec![ids[0].0, ids[1].0]);
}
#[test]
fn clear_anchor_does_not_adopt_unmarked_survivors_or_claim_sequence_absence() {
    let (mut s, scope, ids) = root_clear_fixture();
    let foreign = Arc::make_mut(&mut s.project).new_sequence("Unmarked imported sequence", Default::default(), 1, 1, None);
    s.state.active_sequence = Some(foreign);
    s.execute("project.delete", json!({"items":ids.iter().map(|id|id.0).collect::<Vec<_>>()})).unwrap();
    assert!(s.project.sequence(foreign).is_some());
    assert_eq!(crate::editorial_workspace::association(&s.project).unwrap(), None);
    let binding = root_sequence_binding(&mut s, &scope, 10);
    assert_eq!(binding["state"], "unassociated");
    assert_eq!(binding["sequence"], foreign.0);
    assert_eq!(binding["sequenceScope"], Value::Null);
    assert_eq!(binding["activeOwned"], false);
    assert_eq!(binding["requiresFreshSequenceRead"], true);
    assert!(!s.project.item(foreign).unwrap().metadata.contains_key(crate::editorial_workspace::KEY));
    assert!(!s.project.item(foreign).unwrap().metadata.contains_key(crate::editorial_workspace::SEQUENCE_KEY));
    let actual =
        ItemId(s.execute("file.newSequence", json!({"name":"New actual owned anchor","fps":25,"video":1,"audio":1})).unwrap()["sequence"].as_u64().unwrap());
    assert_ne!(actual, foreign);
    let binding = root_sequence_binding(&mut s, &scope, 11);
    assert_eq!(binding["state"], "current");
    assert_eq!(binding["sequenceScope"]["anchorSequence"], actual.0);
    s.state.active_sequence = Some(foreign);
    assert_eq!(root_sequence_binding(&mut s, &scope, 12)["state"], "outside_scope");
}
#[test]
fn clear_last_owned_sequence_keeps_real_absence_and_reserves_retired_ids_before_next_creation() {
    let (mut s, scope, ids) = root_clear_fixture();
    s.execute("project.delete", json!({"items":ids.iter().map(|id|id.0).collect::<Vec<_>>()})).unwrap();
    assert_eq!(s.project.sequences().count(), 0);
    assert!(s.state.active_sequence.is_none());
    let binding = root_sequence_binding(&mut s, &scope, 20);
    assert_eq!(binding["state"], "absent");
    assert_eq!(binding["sequenceScope"], Value::Null);
    assert_eq!(binding["sequence"], Value::Null);
    Arc::make_mut(&mut s.project).next_id = 1;
    let actual =
        ItemId(s.execute("file.newSequence", json!({"name":"Fresh real sequence","fps":25,"video":1,"audio":1})).unwrap()["sequence"].as_u64().unwrap());
    assert!(actual.0 > ids[0].0);
    assert!(!crate::project_workspace::retired_anchor_ids(&s.project).unwrap().contains(&actual.0));
    assert_eq!(root_sequence_binding(&mut s, &scope, 21)["sequenceScope"]["anchorSequence"], actual.0);
}
#[test]
fn malformed_or_foreign_retired_anchor_records_never_grant_ownership_or_consume_human_history() {
    for mode in 0..6 {
        let (mut s, _, ids) = root_clear_fixture();
        s.execute("project.delete", json!({"items":[ids[0].0]})).unwrap();
        let project = Arc::make_mut(&mut s.project);
        let key = crate::project_workspace::RETIRED_ANCHORS_KEY;
        let mut record: Value = serde_json::from_str(project.metadata.get(key).unwrap()).unwrap();
        match mode {
            0 => record["scope"]["workspaceId"] = json!("f1c52b1e-9896-483b-822f-faeae4b50000"),
            1 => record["anchors"] = json!([ids[0].0, ids[0].0]),
            2 => record["anchors"] = json!([ids[1].0]),
            3 => record["anchors"] = json!([0]),
            4 => record["extra"] = json!(true),
            _ => {
                project.metadata.remove(crate::project_workspace::KEY);
            }
        }
        project.metadata.insert(key.into(), serde_json::to_string(&record).unwrap());
        let before = s.project.clone();
        let history = s.history.undo.len();
        let editor = serde_json::to_value(&s.state).unwrap();
        assert!(crate::project_workspace::retired_anchor_ids(&s.project).is_err());
        assert!(s.execute("file.newBin", json!({"name":"Must not allocate a reclaimed identity"})).is_err());
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(s.history.undo.len(), history);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    }
}
