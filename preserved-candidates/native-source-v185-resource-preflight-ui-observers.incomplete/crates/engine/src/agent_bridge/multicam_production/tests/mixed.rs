//! Generated physical media and original native services. No alternate journal
//! writer or restore path; complete source recovery uses the original worker,
//! saved-file/directory/epoch owner and paused native startup.
use super::*;
use crate::{ApplicationSnapshotCapture, ApplicationTabArchiveInput};
const UPDATE: &str = "b2e946f4-8038-4c79-993e-ed2f2f796bab";
fn metadata_read(f: &mut Fixture, id: usize) -> Value {
    f.s.execute("agent.readProject", json!({"token":token(id),"scope":scope(),"projectPath":f.s.path})).unwrap()
}
fn owned_bin(f: &mut Fixture, id: usize) -> Value {
    let d = metadata_read(f, id);
    f.s.execute("agent.apply", json!({"token":token(id),"label":"Mixed original owned bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":{"type":"create","name":format!("Owned mixed bin {id}"),"parent":null}}}]})).unwrap()
}
fn extra_library_admission(f: &mut Fixture, id: usize) -> Value {
    let path = f._dir.join("additional-retained-library.mov");
    fs::copy(&f.files[0], &path).unwrap();
    let item = f.s.execute("file.import", json!({"paths":[path]})).unwrap()["items"][0].as_u64().unwrap();
    let mut dependencies = f.items.clone();
    dependencies.push(item);
    f.s.execute("agent.readSourceLibrary", json!({"token":token(id),"scope":scope(),"projectPath":f.s.path,"sources":dependencies})).unwrap();
    poll(&mut f.s, id);
    let bytes = fs::read(&path).unwrap();
    f.s.execute("agent.apply",json!({"token":token(id),"label":"Actual additional library admission","exact":true,"operations":[{"command":"agent.admitSource","params":{"item":item,"path":path,"scope":scope(),"binding":{"projectId":"p_production","assetId":"a_additional","mediaId":"a_additional:original","role":"original"},"generation":{"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()},"ownerReceiptSha256":format!("{:x}",Sha256::digest(b"generated additional library owner"))}}]})).unwrap()
}
fn history(s: &Session) -> Value {
    json!({"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key})
}
fn source_roundtrip(f: &mut Fixture, commit: usize, read: usize, undo: bool, undo_token: Option<usize>) {
    let items = f.items.clone();
    inputs(f, read, json!({"kind":"sources","items":items}));
    let request = if undo {
        json!({"token":token(read),"commit":token(commit)})
    } else {
        json!({"token":token(read),"commit":token(commit),"undo":token(undo_token.unwrap())})
    };
    worker_command(f, if undo { "agent.undo" } else { "agent.redo" }, request).unwrap();
}
fn cold_check(f: &mut Fixture, save: usize, reopen: usize, source: usize, receipts: &[(usize, Value)]) {
    let expected = project_audit::bytes(&f.s.project).unwrap();
    let old_history = history(&f.s);
    let (saved, path) = saved_source(f, save);
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["version"], 11);
    assert!(raw["tickets"].as_array().unwrap().iter().any(|t| t["token"] == token(source) && t["multicam_history"].is_object()));
    assert!(raw["tickets"].as_array().unwrap().iter().any(|t| t["source_library"].is_array()));
    reopen_source(f, reopen, &saved);
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), expected);
    assert_eq!(history(&f.s), old_history);
    for (id, receipt) in receipts {
        assert_eq!(f.s.execute("agent.receipt", json!({"token":token(*id)})).unwrap()["result"], *receipt);
    }
    let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(source)).unwrap();
    assert!(t.multicam_history.is_some());
    assert!(t.multicam_production.is_none());
    assert_eq!(t.media.scope_json()["libraryVerified"], false);
    assert!(!f.s.trim_play.active());
}
#[test]
fn capsule_eleven_mixes_real_library_metadata_navigation_settings_and_source_commit_undo_redo_edges() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for edge in ["committed", "undone", "redone"] {
        let mut f = fixture();
        let (main, _) = timeline(&mut f);
        let library = extra_library_admission(&mut f, 409);
        let bin = owned_bin(&mut f, 410);
        let doc = ready_source(&mut f, 411);
        let request = commit_request(411, &doc);
        let receipt = worker_command(&mut f, "agent.apply", request.clone()).unwrap();
        let source_after = project_audit::bytes(&f.s.project).unwrap();
        if edge != "committed" {
            source_roundtrip(&mut f, 411, 412, true, None);
        }
        if edge == "redone" {
            source_roundtrip(&mut f, 411, 413, false, Some(412));
            assert_eq!(project_audit::bytes(&f.s.project).unwrap(), source_after);
        }
        let d = metadata_read(&mut f, 414);
        let owner = crate::editorial_workspace::association(&f.s.project).unwrap().unwrap();
        let navigation = f.s.execute("agent.selectProjectSequence",json!({"token":token(414),"scope":scope(),"sequenceScope":owner.scope,"item":main,"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"]})).unwrap();
        let d = metadata_read(&mut f, 415);
        let track = f.s.active_sequence().unwrap().audio_tracks[0].id.0;
        let setting = f.s.execute("agent.configureEditor",json!({"token":token(415),"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":{"kind":"clip_mixer_mode","track":track,"mode":"Touch"}})).unwrap();
        cold_check(&mut f, 416, 417, 411, &[(409, library), (410, bin), (411, receipt.clone()), (414, navigation), (415, setting)]);
        let before = state(&f.s);
        assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), receipt);
        assert_eq!(state(&f.s), before);
        if edge == "undone" {
            source_roundtrip(&mut f, 411, 418, false, Some(412));
        } else {
            source_roundtrip(&mut f, 411, 418, true, None);
        }
        eprintln!("mixed source capsule11 {edge}: real library, metadata, navigation, settings, exact graph and fresh-source history");
    }
}
fn native_tab_and_ui(f: &mut Fixture, main: u64, id: usize) -> Value {
    let native = f.s.prepare_application_tab_scope(&json!({"token":token(id),"scope":scope(),"projectPath":f.s.path})).unwrap();
    let data = serde_json::to_string(&json!({"kind":"generated-engine-only-tab-ui","version":1})).unwrap();
    let ui =
        json!({"kind":"native_application_tab_ui_proof","version":1,"bytes":data.len(),"sha256":format!("{:x}",Sha256::digest(data.as_bytes())),"data":data});
    let d = json!({"kind":"native_application_tab_view","version":1,"status":"read","applicationInstance":"1234567890abcdef1234567890abcdef","readPurpose":"application_tab_preparation","token":token(id),"native":native.document(),"ui":ui,"uiSha256":ui["sha256"]});
    let reference = json!({"token":token(id),"applicationInstance":d["applicationInstance"],"scope":scope(),"projectPath":f.s.path,"sha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(&d).unwrap()))});
    let preparation = json!({"kind":"native_application_tab_preparation","version":1,"status":"prepared","reference":reference,"document":d});
    let p = native.prepare_activation(&mut f.s, main).unwrap().enter_publication(&mut f.s).unwrap();
    f.s.execute("sequence.open", json!({"item":main})).unwrap();
    let receipt = p.finish(&f.s);
    assert!(receipt.confirmed());
    let outcome = json!({"kind":"native_application_tab_change","version":1,"status":"activated","reference":reference,"operation":{"type":"activate_tab","item":main},"native":receipt.document(),"beforeUi":ui,"afterUi":ui,"preparedUiSha256":ui["sha256"],"receiptComplete":true,"canReplay":false});
    let passive_data = serde_json::to_string(&json!({"kind":"generated-engine-only-passive-ui","version":1})).unwrap();
    let passive = json!({"kind":"native_passive_application_ui_proof","version":1,"bytes":passive_data.len(),"sha256":format!("{:x}",Sha256::digest(passive_data.as_bytes())),"data":passive_data});
    ApplicationSnapshotCapture::capture(&f.s, &[ApplicationTabArchiveInput { preparation: &preparation, outcome: &outcome, receipt: &receipt }])
        .unwrap()
        .with_application_ui(&f.s, passive)
        .unwrap()
        .install(&mut f.s)
        .unwrap();
    reference
}
#[test]
fn capsule_eleven_mixes_source_with_original_compound_following_sequence_views_and_passive_application_records() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for kind in ["compound", "following", "sequence-views", "application-tab-ui"] {
        let mut f = fixture();
        let (main, _) = timeline(&mut f);
        let library = extra_library_admission(&mut f, 429);
        let doc = ready_source(&mut f, 430);
        let receipt = worker_command(&mut f, "agent.apply", commit_request(430, &doc)).unwrap();
        let mut receipts = vec![(429, library), (430, receipt)];
        let mut reference = None;
        match kind {
            "compound" | "following" => {
                f.s.execute("sequence.open", json!({"item":main})).unwrap();
                let clip = f.s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
                f.s.execute("trim.selectEditPoint", json!({"clip":clip,"edge":"out","kind":"trim"})).unwrap();
                f.s.execute("trim.shuttle", json!({"direction":"reverse","clock":0.0})).unwrap();
                f.s.execute("trim.tick", json!({"clock":0.125})).unwrap();
                let mut read = json!({"token":token(431),"scope":scope(),"projectPath":f.s.path});
                if kind == "following" {
                    read["clockPolicy"] = json!("original_human_trim");
                }
                let d = f.s.execute("agent.prepareEditorControl", read).unwrap();
                if kind == "following" {
                    f.s.execute("trim.tick", json!({"clock":0.25})).unwrap();
                }
                let r=f.s.execute("agent.configureEditorControl",json!({"version":if kind=="following"{3}else{2},"token":token(431),"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":{"kind":"default_transition","definition":"constant_gain"}})).unwrap();
                receipts.push((431, r));
                if f.s.trim_play.active() {
                    f.s.execute("trim.shuttleStop", json!({})).unwrap();
                }
            }
            "sequence-views" => {
                let d = f.s.execute("agent.prepareSequenceViews", json!({"token":token(431),"scope":scope(),"projectPath":f.s.path})).unwrap();
                let r=f.s.execute("agent.navigateSequenceView",json!({"version":1,"token":token(431),"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"operation":{"type":"open","item":main,"owned":true}})).unwrap();
                receipts.push((431, r));
            }
            "application-tab-ui" => reference = Some(native_tab_and_ui(&mut f, main, 431)),
            _ => unreachable!(),
        }
        let archive_count = f.s.agent_bridge.application_history.len();
        cold_check(&mut f, 432, 433, 430, &receipts);
        assert_eq!(f.s.agent_bridge.application_history.len(), archive_count);
        if let Some(reference) = reference {
            assert!(f.s.archived_application_tab(&reference).unwrap().is_some());
            assert!(f.s.agent_bridge.application_ui.is_some());
        }
        eprintln!("mixed source capsule11 {kind}: original retained snapshots and cold data-only roles");
    }
}
fn basis(f: &mut Fixture, id: usize) {
    f.s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(id),"scope":scope()})).unwrap();
}
fn stage(f: &mut Fixture) -> Value {
    basis(f, 450);
    let checkpoint = checkpoint(450);
    let saved = f
        .s
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(450),"scope":scope(),"workspace":WORKSPACE,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    basis(f, 451);
    f.s.execute("agent.beginWorkspaceUpdate", json!({"token":token(451),"scope":scope(),"workspace":WORKSPACE,"update":UPDATE})).unwrap();
    basis(f, 452);
    f.s.execute("agent.stageProjectRecovery",json!({"token":token(452),"scope":scope(),"workspace":WORKSPACE,"update":UPDATE,"pauseToken":token(451),"checkpoint":checkpoint,"checkpointSha256":saved["checkpoint"]["sha256"]})).unwrap()
}
pub(super) fn journal_source(gesture: &str, human: bool) -> (Fixture, Value, Value, Vec<u8>) {
    let mut f = fixture();
    let targets = if gesture == "synchronize" {
        let (seq, ids) = timeline(&mut f);
        json!({"kind":"timeline","sequence":seq,"identities":ids})
    } else {
        json!({"kind":"sources","items":if gesture=="merge"{vec![f.items[0],f.items[2]]}else{f.items.clone()}})
    };
    let mut cfg = crate::autosave::AutosaveConfig::new(f._dir.join("source-native-data"));
    cfg.journal_debounce = Duration::from_secs(600);
    f.s.start_autosave(cfg).unwrap();
    let staged = stage(&mut f);
    let request = json!({"projectPath":f.s.path,"scope":scope(),"workspace":WORKSPACE,"update":UPDATE,"sha256":staged["sha256"],"activationToken":token(460),"latestJournal":true});
    f.s.cancel_workspace_update().unwrap();
    // Set the captured editor before the original revision schedules its FIFO snapshot.
    f.s.state.source_playhead = Tick(9_007_199_254_740_999);
    inputs(&mut f, 453, targets);
    let doc = plan(&mut f, 453, json!({"gesture":gesture,"options":{"method":"in","offset":2}})).unwrap();
    let receipt = worker_command(&mut f, "agent.apply", commit_request(453, &doc)).unwrap();
    if human {
        f.s.execute("file.newBin", json!({"name":"Latest independent human bin"})).unwrap();
    }
    let baseline = fs::read(f.s.path.as_ref().unwrap()).unwrap();
    f.s.sync_persistence();
    f.s.persistence.as_ref().unwrap().flush();
    f.s.apply_persistence_events();
    assert!(f.s.persistence.as_ref().unwrap().status.last_error.is_none(), "{:?}", f.s.persistence.as_ref().unwrap().status.last_error);
    assert_eq!(f.s.persistence.as_ref().unwrap().status.journaled_revision, f.s.revision);
    (f, request, receipt, baseline)
}
pub(super) fn activate(f: &mut Fixture) {
    f.worker.install(&mut f.s).unwrap();
    f.s.agent_operation_control().enable_transport();
    let h = f.s.workspace_update_status().unwrap();
    f.s.execute(
        "agent.activateWorkspaceUpdate",
        json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]}),
    )
    .unwrap();
}
#[test]
fn original_complete_persistence_worker_and_latest_journal_startup_preserve_all_three_source_gestures_and_later_human_work() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for gesture in ["merge", "create_multicam", "synchronize"] {
        for human in [false, true] {
            let (mut f, request, receipt, baseline) = journal_source(gesture, human);
            let expected = f.s.project.clone();
            let editor = serde_json::to_value(&f.s.state).unwrap();
            let expected_history = history(&f.s);
            let journal = f.s.journal.clone();
            let directory = f.s.persistence.as_ref().unwrap().session_dir.clone();
            let companion = fs::read(directory.join("native-state.json")).unwrap();
            let project = fs::read(directory.join("snapshot.fcproj")).unwrap();
            assert!(
                serde_json::from_slice::<Value>(&companion).unwrap()["payload"]["history"]["tickets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["multicam_history"].is_object())
            );
            let mut fresh = Session::default();
            assert!(fresh.open_project_recovery(&request).is_err());
            assert!(fresh.path.is_none());
            f.s.persistence.take().unwrap().simulate_crash();
            let fence = fresh.open_project_recovery(&request).unwrap();
            assert_eq!(fence["journalRecovery"]["revision"], f.s.revision);
            assert_eq!(fence["journalRecovery"]["writeAuthority"], false);
            assert!(fresh.is_display_only());
            assert_eq!(project_audit::bytes(&fresh.project).unwrap(), project_audit::bytes(&expected).unwrap());
            assert_eq!(serde_json::to_value(&fresh.state).unwrap(), editor);
            assert_eq!(history(&fresh), expected_history);
            assert_eq!(fresh.journal, journal);
            f.s = fresh;
            activate(&mut f);
            assert!(!f.s.is_display_only());
            assert_eq!(f.s.execute("agent.receipt", json!({"token":token(453)})).unwrap()["result"], receipt);
            let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(453)).unwrap();
            assert!(t.multicam_history.is_some());
            assert!(t.multicam_production.is_none());
            assert_eq!(t.media.scope_json()["libraryVerified"], false);
            let items = f.items.clone();
            inputs(&mut f, 461, json!({"kind":"sources","items":items}));
            assert_eq!(
                f.s.execute("agent.readHistory", json!({"token":token(461),"scope":scope(),"commit":token(453),"undo":null})).unwrap()["undoAvailable"],
                !human
            );
            if human {
                let before = state(&f.s);
                assert!(worker_command(&mut f, "agent.undo", json!({"token":token(461),"commit":token(453)})).is_err());
                assert_eq!(state(&f.s), before);
                f.s.execute("edit.undo", json!({})).unwrap();
                f.s.execute("edit.redo", json!({})).unwrap();
                assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&expected).unwrap());
            } else {
                worker_command(&mut f, "agent.undo", json!({"token":token(461),"commit":token(453)})).unwrap();
                source_roundtrip(&mut f, 453, 462, false, Some(461));
                assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&expected).unwrap());
            }
            assert_eq!(fs::read(f.s.path.as_ref().unwrap()).unwrap(), baseline);
            assert_eq!(fs::read(directory.join("native-state.json")).unwrap(), companion);
            assert_eq!(fs::read(directory.join("snapshot.fcproj")).unwrap(), project);
            f.s.shutdown();
            eprintln!(
                "actual original source journal {gesture}/human={human}: physical native worker, paused startup, exact graph/editor/log, fresh history and original baseline preserved"
            );
        }
    }
}
#[test]
fn source_latest_journal_startup_refuses_changed_saved_file_wrong_pid_or_damaged_companion_without_fallback() {
    let _serial = super::super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in ["saved-file", "wrong-pid", "companion", "missing-companion"] {
        let (mut f, request, _, _) = journal_source("create_multicam", false);
        let dir = f.s.persistence.as_ref().unwrap().session_dir.clone();
        f.s.persistence.take().unwrap().simulate_crash();
        match mode {
            "saved-file" => {
                let target = PathBuf::from(f.s.path.as_ref().unwrap());
                let replacement = dir.join("same-bytes-new-native-file.fcproj");
                fs::write(&replacement, fs::read(&target).unwrap()).unwrap();
                fs::rename(replacement, target).unwrap();
            }
            "wrong-pid" => {
                let p = dir.join("session.json");
                let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                v["pid"] = json!(std::process::id().saturating_add(1));
                fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
            }
            "companion" => {
                let p = dir.join("native-state.json");
                let mut raw = fs::read(&p).unwrap();
                raw[0] = b'!';
                fs::write(p, raw).unwrap();
            }
            "missing-companion" => {
                fs::rename(dir.join("native-state.json"), dir.join("held-native-state.json")).unwrap();
            }
            _ => unreachable!(),
        }
        let baseline = fs::read(f.s.path.as_ref().unwrap()).unwrap();
        let mut fresh = Session::default();
        let before = state(&fresh);
        assert!(fresh.open_project_recovery(&request).is_err(), "{mode}");
        assert_eq!(state(&fresh), before);
        assert!(fresh.path.is_none());
        assert_eq!(fs::read(f.s.path.as_ref().unwrap()).unwrap(), baseline);
    }
}
