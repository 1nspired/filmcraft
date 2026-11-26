//! Actual original human Undo/Redo, source services and explicit fresh source scope.
use super::*;
fn history(s: &Session) -> Value {
    json!({"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key})
}
fn twin(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.history = s.history.clone();
    h.media = s.media.clone();
    h.prefs = s.prefs.clone();
    h.path = s.path.clone();
    h.revision = s.revision;
    h.saved_revision = s.saved_revision;
    h.journal = s.journal.clone();
    h
}
fn equivalent(s: &Session, h: &Session) {
    assert_eq!(project_audit::bytes(&s.project).unwrap(), project_audit::bytes(&h.project).unwrap());
    assert_eq!(project_audit::value_bytes(&s.state).unwrap(), project_audit::value_bytes(&h.state).unwrap());
    assert_eq!(history(s), history(h));
    assert_eq!(s.revision, h.revision);
}
fn history_read(f: &mut Fixture, read: usize, commit: usize, undo: Option<usize>) -> Value {
    f.s.execute("agent.readHistory", json!({"token":token(read),"scope":scope(),"commit":token(commit),"undo":undo.map(token),"projection":true})).unwrap()
}
fn physical(f: &mut Fixture, id: usize, sources: Vec<u64>) {
    inputs(f, id, json!({"kind":"sources","items":sources}));
}
fn source(f: &mut Fixture, id: usize, gesture: &str, items: Vec<u64>) -> (Value, Value) {
    physical(f, id, items);
    let doc = plan(f, id, json!({"gesture":gesture,"options":{"method":"in"}})).unwrap();
    let receipt = worker_command(f, "agent.apply", commit_request(id, &doc)).unwrap();
    (doc, receipt)
}
fn simple_sequence(f: &mut Fixture, name: &str, video: usize, audio: usize) -> u64 {
    f.s.execute("file.newSequence", json!({"name":name,"width":64,"height":48,"fps":{"num":24,"den":1},"video":video,"audio":audio})).unwrap();
    f.s.state.active_sequence.unwrap().0
}
fn identities(f: &Fixture, seq: u64) -> Vec<String> {
    f.s.project.sequence(ItemId(seq)).unwrap().all_tracks().flat_map(|t| &t.items).map(|c| c.identity.as_ref().unwrap().id.clone()).collect()
}
fn fallback() -> (Fixture, u64, u64, Value, Value) {
    let mut f = fixture();
    let first = simple_sequence(&mut f, "Native first fallback uses recorder", 0, 1);
    let recorder = f.items[2];
    f.s.edit("Generated actual fallback recorder placement", |p, _| {
        let item = p
            .make_track_item(ItemId(recorder), TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(40)), FrameRate::FPS_24)
            .unwrap();
        p.sequence_mut(ItemId(first)).unwrap().audio_tracks[0].items.push(item);
        Ok(())
    })
    .unwrap();
    f.s.execute("poema.trackSequence", json!({})).unwrap();
    let (second, _) = timeline(&mut f);
    let clips = f.s.active_sequence().unwrap().video_tracks.iter().flat_map(|t| &t.items).map(|c| c.id.0).collect::<Vec<_>>();
    f.s.execute("clip.synchronize", json!({"clips":clips,"method":"in"})).unwrap();
    let items = f.items[..2].to_vec();
    let (doc, receipt) = source(&mut f, 610, "create_multicam", items);
    let derived = doc["result"]["sequence"].as_u64().unwrap();
    assert_ne!(first, second);
    assert_eq!(f.s.project.sequences().next().unwrap().id.0, first);
    f.s.execute("sequence.open", json!({"item":derived})).unwrap();
    f.s.execute("source.open", json!({"item":derived})).unwrap();
    (f, first, derived, doc, receipt)
}
#[test]
fn source_owned_history_after_later_tab_focus_matches_original_human_undo_redo_live_and_cold() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for gesture in ["merge", "create_multicam"] {
        for cold in [false, true] {
            let mut f = fixture();
            let (main, _) = timeline(&mut f);
            let other = simple_sequence(&mut f, "Previous human focus", 1, 1);
            f.s.execute("file.save", json!({})).unwrap();
            let items = if gesture == "merge" { vec![f.items[0], f.items[2]] } else { f.items.clone() };
            let (doc, _) = source(&mut f, 590, gesture, items);
            let derived = doc["result"][if gesture == "merge" { "item" } else { "sequence" }].as_u64().unwrap();
            assert_ne!(main, other);
            assert_eq!(f.s.state.active_sequence.unwrap().0, other);
            f.s.execute("sequence.open", json!({"item":derived})).unwrap();
            f.s.execute("sequence.open", json!({"item":main})).unwrap();
            f.s.execute("source.open", json!({"item":f.items[2]})).unwrap();
            if cold {
                let (saved, _) = saved_source(&mut f, 591);
                reopen_source(&mut f, 592, &saved);
            }
            let mut h = twin(&f.s);
            let items = f.items.clone();
            physical(&mut f, 593, items.clone());
            let d = history_read(&mut f, 593, 590, None);
            assert_eq!(d["sourceHistoryInputs"]["sequence"], main);
            assert_eq!(d["sourceHistoryInputs"]["writeAuthority"], false);
            h.execute("edit.undo", json!({})).unwrap();
            let receipt = worker_command(&mut f, "agent.undo", json!({"token":token(593),"commit":token(590)})).unwrap();
            equivalent(&f.s, &h);
            assert_eq!(f.s.state.active_sequence.unwrap().0, main);
            physical(&mut f, 594, items);
            let d = history_read(&mut f, 594, 590, Some(593));
            assert_eq!(d["sourceHistoryInputs"]["sequence"], main);
            h.execute("edit.redo", json!({})).unwrap();
            worker_command(&mut f, "agent.redo", json!({"token":token(594),"commit":token(590),"undo":token(593)})).unwrap();
            equivalent(&f.s, &h);
            let before = state(&f.s);
            assert_eq!(worker_command(&mut f, "agent.undo", json!({"token":token(593),"commit":token(590)})).unwrap(), receipt);
            assert_eq!(state(&f.s), before);
            eprintln!("native source view {gesture}/cold={cold}: exact original human UndoRedo preserves later tab and source monitor");
        }
    }
}
#[test]
fn fallback_sequence_and_source_monitor_require_expanded_original_inputs_before_native_history_publication() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for cold in [false, true] {
        let (mut f, first, derived, _, _) = fallback();
        if cold {
            let (saved, _) = saved_source(&mut f, 611);
            reopen_source(&mut f, 612, &saved);
        }
        f.s.execute("sequence.open", json!({"item":derived})).unwrap();
        f.s.execute("source.open", json!({"item":derived})).unwrap();
        let narrow = f.items[..2].to_vec();
        physical(&mut f, 613, narrow);
        let before = state(&f.s);
        let d = history_read(&mut f, 613, 610, None);
        assert_eq!(state(&f.s), before);
        assert_eq!(d["undoAvailable"], true);
        assert_eq!(d["sourceHistoryInputs"]["sources"], json!(f.items));
        assert_eq!(d["sourceHistoryInputs"]["sequence"], first);
        assert_eq!(d["sourceHistoryInputs"]["source"], Value::Null);
        assert_eq!(d["sourceHistoryInputs"]["sourceValidation"], "not-performed");
        assert!(worker_command(&mut f, "agent.undo", json!({"token":token(613),"commit":token(610)})).is_err());
        assert_eq!(state(&f.s), before);
        assert!(f.s.project.item(ItemId(derived)).is_some());
        let sources = d["sourceHistoryInputs"]["sources"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        physical(&mut f, 614, sources);
        let mut h = twin(&f.s);
        h.execute("edit.undo", json!({})).unwrap();
        worker_command(&mut f, "agent.undo", json!({"token":token(614),"commit":token(610)})).unwrap();
        equivalent(&f.s, &h);
        assert_eq!(f.s.state.active_sequence.unwrap().0, first);
        assert_eq!(f.s.state.source_item, None);
        let all = f.items.clone();
        physical(&mut f, 615, all);
        h.execute("edit.redo", json!({})).unwrap();
        worker_command(&mut f, "agent.redo", json!({"token":token(615),"commit":token(610),"undo":token(614)})).unwrap();
        equivalent(&f.s, &h);
        assert_eq!(f.s.state.active_sequence.unwrap().0, first);
        eprintln!(
            "native expanded history cold={cold}: exact original fallback and source-monitor repair; narrow read refused, expanded original bytes required"
        );
    }
}
#[test]
fn history_input_discovery_cannot_replace_fresh_source_witnesses_or_a_matching_root() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in ["metadata", "extra-claim", "changed-byte", "foreign-root"] {
        let (mut f, _, _, _, _) = fallback();
        let narrow = f.items[..2].to_vec();
        physical(&mut f, 620, narrow);
        let d = history_read(&mut f, 620, 610, None);
        assert_eq!(d["sourceHistoryInputs"]["sources"], json!(f.items));
        match mode {
            "metadata" => {
                metadata(&mut f, 621);
                let before = state(&f.s);
                assert!(worker_command(&mut f, "agent.undo", json!({"token":token(621),"commit":token(610)})).is_err());
                assert_eq!(state(&f.s), before);
            }
            "extra-claim" => {
                let before = state(&f.s);
                assert!(
                    worker_command(&mut f, "agent.undo", json!({"token":token(620),"commit":token(610),"sourceHistoryInputs":d["sourceHistoryInputs"]}))
                        .is_err()
                );
                assert_eq!(state(&f.s), before);
            }
            "changed-byte" => {
                let all = f.items.clone();
                physical(&mut f, 621, all);
                use std::io::Write;
                fs::OpenOptions::new().append(true).open(&f.files[2]).unwrap().write_all(b"changed fallback recorder bytes").unwrap();
                let before = state(&f.s);
                assert!(worker_command(&mut f, "agent.undo", json!({"token":token(621),"commit":token(610)})).is_err());
                assert_eq!(state(&f.s), before);
            }
            "foreign-root" => {
                let before = state(&f.s);
                let mut foreign = scope();
                foreign["projectId"] = json!("p_foreign");
                assert!(
                    f.s.execute(READ, json!({"token":token(621),"scope":foreign,"projectPath":f.s.path,"targets":{"kind":"sources","items":f.items}})).is_err()
                );
                assert_eq!(state(&f.s), before);
            }
            _ => unreachable!(),
        }
    }
}
fn derived(gesture: &str, main: bool) -> (Fixture, u64, Vec<String>) {
    let mut f = fixture();
    if main {
        timeline(&mut f);
    }
    let items = if gesture == "merge" { vec![f.items[0], f.items[2]] } else { f.items.clone() };
    let (doc, _) = source(&mut f, 630, gesture, items);
    let seq = doc["result"][if gesture == "merge" { "item" } else { "sequence" }].as_u64().unwrap();
    f.s.execute("sequence.open", json!({"item":seq})).unwrap();
    f.s.execute("poema.trackSequence", json!({})).unwrap();
    if gesture == "merge" {
        let clips = f.s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).map(|c| c.id.0).collect::<Vec<_>>();
        f.s.execute("timeline.select", json!({"clips":clips,"add":false,"toggle":false,"linked":false})).unwrap();
        assert_eq!(f.s.execute("clip.link", json!({"clips":clips})).unwrap()["linked"], false);
    }
    f.s.execute("file.save", json!({})).unwrap();
    let ids = identities(&f, seq);
    (f, seq, ids)
}
#[test]
fn derived_merged_and_multicam_source_timelines_match_human_sync_without_inventing_an_owning_anchor() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for gesture in ["merge", "create_multicam"] {
        for main in [false, true] {
            let (mut f, seq, ids) = derived(gesture, main);
            let association = crate::editorial_workspace::association(&f.s.project).unwrap();
            assert_eq!(association.is_some(), main);
            let owner = association.map(|a| a.scope.anchor_sequence);
            let original = project_audit::bytes(&f.s.project).unwrap();
            inputs(&mut f, 631, json!({"kind":"timeline","sequence":seq,"identities":ids}));
            let doc = plan(&mut f, 631, json!({"gesture":"synchronize","options":{"method":"marker","marker":"Clap"}})).unwrap();
            assert!(doc["result"]["moved"].as_u64().unwrap() > 0);
            let mut h = twin(&f.s);
            let result = h.execute(doc["humanCommand"].as_str().unwrap(), doc["humanParams"].clone()).unwrap();
            // The source publisher accepts the caller's explicit Undo label.
            // Compare identical requests, including the original human label.
            let mut request = commit_request(631, &doc);
            request["label"] = json!(h.history.undo.last().unwrap().0);
            let r = worker_command(&mut f, "agent.apply", request).unwrap();
            assert_eq!(r["results"], json!([result]));
            equivalent(&f.s, &h);
            assert_eq!(crate::editorial_workspace::association(&f.s.project).unwrap().map(|a| a.scope.anchor_sequence), owner);
            assert!(!f.s.project.item(ItemId(seq)).unwrap().metadata.contains_key(crate::editorial_workspace::KEY));
            assert!(!f.s.project.item(ItemId(seq)).unwrap().metadata.contains_key(crate::editorial_workspace::SEQUENCE_KEY));
            let (saved, _) = saved_source(&mut f, 632);
            reopen_source(&mut f, 633, &saved);
            inputs(&mut f, 634, json!({"kind":"timeline","sequence":seq,"identities":ids}));
            worker_command(&mut f, "agent.undo", json!({"token":token(634),"commit":token(631)})).unwrap();
            assert_eq!(project_audit::bytes(&f.s.project).unwrap(), original);
            inputs(&mut f, 635, json!({"kind":"timeline","sequence":seq,"identities":ids}));
            worker_command(&mut f, "agent.redo", json!({"token":token(635),"commit":token(631),"undo":token(634)})).unwrap();
            assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&h.project).unwrap());
            assert_eq!(crate::editorial_workspace::association(&f.s.project).unwrap().map(|a| a.scope.anchor_sequence), owner);
            eprintln!("native derived timeline {gesture}/main={main}: exact human synchronize, saved owned UndoRedo, original Root anchor unchanged");
        }
    }
}
#[test]
fn derived_timeline_inputs_refuse_untracked_foreign_orphan_cyclic_or_locked_source_edits() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in ["untracked", "foreign", "orphan", "cycle", "locked"] {
        let (mut f, seq, ids) = derived("create_multicam", false);
        let mut p = (*f.s.project).clone();
        match mode {
            "untracked" => {
                let item = p.item_mut(ItemId(seq)).unwrap();
                item.metadata.remove(filmcraft_project::clip_identity::TRACKING_KEY);
                for t in item.as_sequence_mut().unwrap().all_tracks_mut() {
                    for c in &mut t.items {
                        c.identity = None;
                    }
                }
            }
            "foreign" => {
                p.item_mut(ItemId(seq)).unwrap().metadata.insert(
                    crate::editorial_workspace::SEQUENCE_KEY.into(),
                    serde_json::to_string(&json!({"projectId":"p_foreign","anchorSequence":seq})).unwrap(),
                );
            }
            "orphan" => {
                assert!(p.root.remove_item(ItemId(seq)));
            }
            "cycle" => p.sequence_mut(ItemId(seq)).unwrap().video_tracks[0].items[0].item = ItemId(seq),
            "locked" => {
                for t in p.sequence_mut(ItemId(seq)).unwrap().all_tracks_mut() {
                    t.locked = true;
                }
            }
            _ => unreachable!(),
        };
        f.s.project = Arc::new(p);
        let before = state(&f.s);
        if mode == "locked" {
            inputs(&mut f, 640, json!({"kind":"timeline","sequence":seq,"identities":ids}));
            assert!(plan(&mut f, 640, json!({"gesture":"synchronize","options":{"method":"marker","marker":"Clap"}})).is_err());
        } else {
            assert!(
                f.s.execute(
                    READ,
                    json!({"token":token(640),"scope":scope(),"projectPath":f.s.path,"targets":{"kind":"timeline","sequence":seq,"identities":ids}})
                )
                .is_err(),
                "{mode}"
            );
        }
        assert_eq!(state(&f.s), before);
    }
}
#[test]
fn capsule_twelve_mixed_source_nochange_and_fallback_history_keeps_native_view_and_prior_receipts() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut f, first, seq, _, real) = fallback();
    let prior = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(610)).unwrap().sequence.unwrap().0;
    f.s.execute("sequence.open", json!({"item":prior})).unwrap();
    let ids = identities(&f, prior);
    inputs(&mut f, 650, json!({"kind":"timeline","sequence":prior,"identities":ids}));
    let doc = plan(&mut f, 650, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    assert_eq!(doc["result"]["moved"], 0);
    let request = commit_request(650, &doc);
    let before = state(&f.s);
    let result = worker_command(&mut f, "agent.apply", request.clone()).unwrap();
    assert_eq!(result["outcome"], "unchanged");
    assert_eq!(state(&f.s), before);
    f.s.execute("sequence.open", json!({"item":seq})).unwrap();
    let (saved, path) = saved_source(&mut f, 651);
    let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(raw["version"], 12);
    reopen_source(&mut f, 652, &saved);
    f.s.execute("sequence.open", json!({"item":seq})).unwrap();
    f.s.execute("source.open", json!({"item":seq})).unwrap();
    let all = f.items.clone();
    physical(&mut f, 653, all.clone());
    let mut h = twin(&f.s);
    h.execute("edit.undo", json!({})).unwrap();
    worker_command(&mut f, "agent.undo", json!({"token":token(653),"commit":token(610)})).unwrap();
    equivalent(&f.s, &h);
    assert_eq!(f.s.state.active_sequence.unwrap().0, first);
    assert_eq!(f.s.execute("agent.receipt", json!({"token":token(610)})).unwrap()["result"], real);
    let before = state(&f.s);
    assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), result);
    assert_eq!(state(&f.s), before);
    assert!(f.s.project.item(ItemId(seq)).is_none());
    physical(&mut f, 654, all);
    h.execute("edit.redo", json!({})).unwrap();
    worker_command(&mut f, "agent.redo", json!({"token":token(654),"commit":token(610),"undo":token(653)})).unwrap();
    equivalent(&f.s, &h);
}
