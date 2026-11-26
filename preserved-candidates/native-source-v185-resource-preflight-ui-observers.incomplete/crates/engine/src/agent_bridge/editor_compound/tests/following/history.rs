use super::*;
fn prepared(kind: &str, n: usize) -> (Scratch, Session, Value, Value, Value) {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, kind, true);
    let d = follow(&mut s, n);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    let saved = save(&mut s, n + 1);
    (dir, s, p, r, saved)
}
fn raw(dir: &Scratch) -> Value {
    serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap()
}
fn original_ticket<'a>(v: &'a mut Value, p: &Value) -> &'a mut Value {
    v["tickets"].as_array_mut().unwrap().iter_mut().find(|t| t["token"] == p["token"]).unwrap()
}
fn check_restored(cold: &mut Session, p: &Value, r: &Value) {
    let t = cold.agent_bridge.tickets.iter().find(|t| t.token == p["token"]).unwrap();
    let owner = t.editor_control.as_ref().unwrap();
    assert!(owner.settlement.is_none());
    assert!(owner.following.is_none());
    assert!(owner.outcome.as_ref().unwrap().original.is_some());
    let before = state(cold);
    assert_eq!(cold.execute(COMMAND, p.clone()).unwrap(), *r);
    assert_eq!(cold.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], *r);
    assert_eq!(state(cold), before);
    assert!(cold.execute("agent.apply", json!({"token":p["token"]})).is_err());
    assert_eq!(state(cold), before);
}
fn corrupt_restore(dir: &Scratch, s: &Session, p: &Value, saved: &Value, mutate: impl FnOnce(&mut Value)) {
    let mut v = raw(dir);
    mutate(&mut v);
    let bytes = serde_json::to_vec_pretty(&v).unwrap();
    std::fs::write(dir.companion(), &bytes).unwrap();
    let mut forged = saved.clone();
    forged["checkpoint"]["sha256"] = json!(history_checkpoint::digest(&bytes));
    let mut cold = reopened(s);
    let before = state(&cold);
    assert!(restore(&mut cold, 194999, &forged).is_err());
    assert_eq!(state(&cold), before);
    let _ = p;
}
#[test]
fn capsule_seven_cold_restores_every_trim_kind_and_zero_or_retained_history_without_live_authority() {
    for kind in ["trim", "ripple", "roll"] {
        for limit in [0, 128] {
            let dir = Scratch::new();
            let mut s = setup(true);
            s.path = Some(dir.file());
            s.history.limit = limit;
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 194000);
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
            let (p, r) = run_pair(&mut s, &d, default());
            let saved = save(&mut s, 194001);
            assert_eq!(raw(&dir)["version"], 7);
            let mut cold = reopened(&s);
            let project = cold.project.clone();
            let editor = project_clear::editor_hash(&cold.state).unwrap();
            restore(&mut cold, 194002, &saved).unwrap();
            assert!(Arc::ptr_eq(&project, &cold.project));
            assert_eq!(project_clear::editor_hash(&cold.state).unwrap(), editor);
            assert_eq!(cold.history.limit, limit);
            check_restored(&mut cold, &p, &r);
        }
    }
}
#[test]
fn capsule_seven_preserves_all_five_mode_outcomes_after_further_human_frames_stop_and_changed_settings() {
    for mode in filmcraft_project::AutomationMode::ALL {
        let dir = Scratch::new();
        let mut s = setup(true);
        s.path = Some(dir.file());
        dynamic(&mut s, "roll", false);
        let d = follow(&mut s, 194100);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let setting = super::super::mode(&s, mode.label());
        let (p, r) = run_pair(&mut s, &d, setting);
        s.execute("trim.tick", json!({"clock":0.5})).unwrap();
        s.execute("trim.shuttleStop", json!({})).unwrap();
        let track = s.active_sequence().unwrap().audio_tracks[0].id;
        s.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Off"})).unwrap();
        let saved = save(&mut s, 194101);
        let mut cold = reopened(&s);
        let editor = project_clear::editor_hash(&cold.state).unwrap();
        restore(&mut cold, 194102, &saved).unwrap();
        assert_eq!(project_clear::editor_hash(&cold.state).unwrap(), editor);
        assert!(!cold.trim_play.active());
        check_restored(&mut cold, &p, &r);
    }
}
#[test]
fn capsule_seven_loop_history_remains_descriptive_and_never_recreates_loop_playback() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    select(&mut s, "roll");
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
    let d = follow(&mut s, 194200);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    let saved = save(&mut s, 194201);
    let mut cold = reopened(&s);
    restore(&mut cold, 194202, &saved).unwrap();
    assert!(!cold.trim_play.active());
    assert!(cold.history.undo.is_empty());
    check_restored(&mut cold, &p, &r);
}
#[test]
fn capsule_seven_keeps_equal_byte_distinct_redo_roots_after_the_original_trim_discards_redo() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    let equal = Arc::new((*s.project).clone());
    assert!(!Arc::ptr_eq(&equal, &s.project));
    s.history.redo.push(("Equal-byte original redo".into(), equal));
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 194300);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    assert!(s.history.redo.is_empty());
    let saved = save(&mut s, 194301);
    let mut cold = reopened(&s);
    restore(&mut cold, 194302, &saved).unwrap();
    let t = cold.agent_bridge.tickets.iter().find(|t| t.token == p["token"]).unwrap();
    let original = t.editor_control.as_ref().unwrap().outcome.as_ref().unwrap().original.as_ref().unwrap();
    let h = original.evidence.validate(&original.projects, "effects.setDefaultTransition").unwrap();
    let redo = &h.before_history().redo[0].1;
    assert!(!Arc::ptr_eq(redo, h.before_project()));
    assert_eq!(project_plane::project_basis(redo).unwrap(), project_plane::project_basis(h.before_project()).unwrap());
    check_restored(&mut cold, &p, &r);
}

#[test]
fn capsule_seven_rejects_missing_collapsed_detached_out_of_range_and_relabelled_snapshot_maps_atomically() {
    for case in 0..8 {
        let (dir, s, p, _r, saved) = prepared("roll", 194400);
        corrupt_restore(&dir, &s, &p, &saved, |v| match case {
            0 => {
                original_ticket(v, &p)["editor_control"].as_object_mut().unwrap().remove("original_snapshots");
            }
            1 => original_ticket(v, &p)["editor_control"]["snapshots"][0] = json!(999999),
            2 => original_ticket(v, &p)["editor_control"]["original_snapshots"] = json!([0, 0]),
            3 => original_ticket(v, &p)["editor_control"]["version"] = json!(1),
            4 => v["version"] = json!(6),
            5 => original_ticket(v, &p)["editor_control"]["original_snapshots"][0] = json!(999999),
            6 => {
                let duplicate = v["projects"][0].clone();
                v["projects"].as_array_mut().unwrap().push(duplicate);
            }
            _ => v["tickets"].as_array_mut().unwrap().clear(),
        });
    }
}
#[test]
fn capsule_seven_rebuilds_both_read_documents_instead_of_trusting_original_or_current_metadata() {
    for case in 0..8 {
        let (dir, s, p, _r, saved) = prepared("roll", 194500);
        corrupt_restore(&dir, &s, &p, &saved, |v| {
            let r = &mut original_ticket(v, &p)["result"];
            match case {
                0 => r["originalRead"]["project"]["dirty"] = json!(false),
                1 => r["originalRead"]["revision"] = json!(999),
                2 => r["originalRead"]["historyState"]["limit"] = json!(1),
                3 => r["publicationRead"]["trim"]["settlement"]["dynamicCommitted"] = json!(false),
                4 => r["publicationRead"]["editorStateSha256"] = json!("0".repeat(64)),
                5 => r["originalRead"]["nativeSession"] = json!("f".repeat(32)),
                6 => r["publicationRead"]["sequence"] = Value::Null,
                _ => r["unrecognized"] = json!(true),
            }
        });
    }
}
#[test]
fn capsule_seven_rejects_semantically_changed_nested_publication_and_native_origin_evidence() {
    for case in 0..6 {
        let (dir, s, p, _r, saved) = prepared("roll", 194600);
        corrupt_restore(&dir, &s, &p, &saved, |v| {
            let r = &mut original_ticket(v, &p)["result"];
            match case {
                0 => r["publication"]["beforeHistory"]["limit"] = json!(1),
                1 => r["publication"]["trim"]["dynamicCommitted"] = json!(false),
                2 => r["originalHistoryProof"]["sha256"] = json!("0".repeat(64)),
                3 => r["originalEditorProof"]["kind"] = json!("native_editor_control_editor_proof"),
                _ => {
                    let data = r["originalBasis"]["data"].as_str().unwrap();
                    let mut b: Value = serde_json::from_str(data).unwrap();
                    if case == 4 {
                        b["revision"] = json!(999);
                    } else {
                        b["extra"] = json!(true);
                    }
                    let raw = serde_json::to_vec(&b).unwrap();
                    r["originalBasis"]["data"] = json!(String::from_utf8(raw.clone()).unwrap());
                    r["originalBasis"]["bytes"] = json!(raw.len());
                    r["originalBasis"]["sha256"] = json!(history_checkpoint::digest(&raw));
                }
            }
        });
    }
}
#[test]
fn capsule_seven_hot_restore_keeps_latest_human_save_editor_history_and_project_without_replaying_a_setting() {
    const NEXT: &str = "26cc2f6a-d72c-4f91-9419-cb3b3c98fc3e";
    let (dir, mut s, p, r, _) = prepared("ripple", 194700);
    let first = std::fs::read(dir.companion()).unwrap();
    #[cfg(unix)]
    let first_identity = {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(dir.companion()).unwrap();
        (m.dev(), m.ino(), m.mode(), m.size(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec())
    };
    s.execute("effects.setDefaultTransition", json!({"effect":"cross_dissolve"})).unwrap();
    let track = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Write"})).unwrap();
    persist(&mut s);
    root(&mut s, 194702);
    let saved = s.execute("agent.saveHistoryCheckpoint", json!({"token":token(194702),"scope":scope(),"workspace":W,"checkpoint":NEXT})).unwrap();
    let second = dir.0.join(format!(".poema-history-{W}-{NEXT}.json"));
    assert!(second.is_file());
    assert_ne!(second, dir.companion());
    let project = s.project.clone();
    let editor = project_clear::editor_hash(&s.state).unwrap();
    let history = s.history.clone();
    root(&mut s, 194703);
    s.execute(
        "agent.restoreHistoryCheckpoint",
        json!({"token":token(194703),"scope":scope(),"workspace":W,"checkpoint":NEXT,"sha256":saved["checkpoint"]["sha256"]}),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(project_clear::editor_hash(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), history.undo.len());
    assert_eq!(s.history.redo.len(), history.redo.len());
    assert_eq!(std::fs::read(dir.companion()).unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(dir.companion()).unwrap();
        assert_eq!((m.dev(), m.ino(), m.mode(), m.size(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()), first_identity);
    }
    check_restored(&mut s, &p, &r);
}

#[test]
fn capsule_seven_keeps_legacy_compound_receipts_and_their_old_shapes_beside_following_outcomes() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    let old = issue(&mut s, 194800, default());
    let old_result = s.execute(COMMAND, old.clone()).unwrap();
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 194801);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let setting = json!({"kind":"default_transition","definition":"cross_dissolve"});
    let (p, r) = run_pair(&mut s, &d, setting);
    let saved = save(&mut s, 194802);
    let v = raw(&dir);
    let legacy = v["tickets"].as_array().unwrap().iter().find(|t| t["token"] == old["token"]).unwrap();
    assert_eq!(legacy["editor_control"]["version"], 1);
    assert!(legacy["editor_control"].get("original_snapshots").is_none());
    let mut cold = reopened(&s);
    restore(&mut cold, 194803, &saved).unwrap();
    let before = state(&cold);
    assert_eq!(cold.execute(COMMAND, old).unwrap(), old_result);
    assert_eq!(state(&cold), before);
    check_restored(&mut cold, &p, &r);
}
#[test]
fn capsule_seven_restored_metadata_and_changed_requests_cannot_promote_a_historical_outcome_to_a_live_hand() {
    let (_dir, s, p, r, saved) = prepared("trim", 194900);
    let mut cold = reopened(&s);
    restore(&mut cold, 194902, &saved).unwrap();
    check_restored(&mut cold, &p, &r);
    let before = state(&cold);
    let page = json!({"version":2,"token":p["token"],"scope":scope(),"beforeEditorSha256":r["originalRead"]["editorStateSha256"],"offset":0,"limit":32});
    assert!(cold.execute(editor_settings::READ, page).is_err());
    let mut changed = p.clone();
    changed["setting"]["definition"] = json!("cross_dissolve");
    assert!(cold.execute(COMMAND, changed).is_err());
    let mut old = p.clone();
    old["version"] = json!(2);
    assert!(cold.execute(COMMAND, old).is_err());
    assert!(cold.execute("agent.undo", json!({"token":p["token"]})).is_err());
    assert_eq!(state(&cold), before);
}

// Source-format compatibility fixtures use actual retained native COW/history
// roots. They do not stand in for an actual older-binary interchange run.
#[derive(serde::Serialize)]
struct AdvertisementWire {
    version: u8,
    revision: u64,
    saved_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    following_write: Option<Value>,
}
fn advertisement_header(receipt: &Value) -> AdvertisementWire {
    let inner: Value = serde_json::from_str(receipt["originalBasis"]["data"].as_str().unwrap()).unwrap();
    AdvertisementWire {
        version: inner["version"].as_u64().unwrap() as u8,
        revision: inner["revision"].as_u64().unwrap(),
        saved_revision: inner["saved_revision"].as_u64().unwrap(),
        following_write: inner.get("following_write").cloned(),
    }
}
fn advertisement_envelope(header: &AdvertisementWire) -> Value {
    let bytes = history_checkpoint::bytes(header, LIMIT).unwrap();
    json!({"kind":"native_following_origin_basis","version":1,"sha256":history_checkpoint::digest(&bytes),"bytes":bytes.len(),"data":String::from_utf8(bytes).unwrap()})
}
fn legacy_advertisement(s: &mut Session, request: &Value, receipt: &Value) -> Value {
    let mut legacy = receipt.clone();
    legacy["originalRead"]["capabilities"]["compoundEditorWrite"] = json!(false);
    legacy["publicationRead"]["capabilities"]["compoundEditorWrite"] = json!(false);
    let mut header = advertisement_header(receipt);
    header.version = 1;
    header.following_write = None;
    legacy["originalBasis"] = advertisement_envelope(&header);
    let t = s.agent_bridge.tickets.iter_mut().find(|t| t.token == request["token"]).unwrap();
    assert_eq!(t.committed.as_ref().unwrap().0, *request);
    t.committed.as_mut().unwrap().1 = legacy.clone();
    legacy
}
#[test]
fn advertised_following_reads_keep_legacy_read_purposes_distinct_and_publish_only_the_original_finite_setting() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let before = state(&s);
    let readonly = s.execute(editor_control::READ, json!({"token":token(197000),"scope":scope(),"projectPath":s.path})).unwrap();
    let strict = prepare_read(&mut s, 197001);
    let following = follow(&mut s, 197002);
    assert_eq!(state(&s), before);
    assert_eq!(readonly["version"], 1);
    assert_eq!(readonly["capabilities"]["compoundEditorWrite"], false);
    assert_eq!(strict["version"], 2);
    assert_eq!(strict["capabilities"]["compoundEditorWrite"], true);
    assert!(strict.get("clockPolicy").is_none());
    assert_eq!(following["version"], 3);
    assert_eq!(following["capabilities"]["compoundEditorWrite"], true);
    let bad = request_for(&following, default());
    assert!(s.execute(COMMAND, bad).is_err());
    for command in ["agent.apply", "agent.undo", "agent.readSourceView", "agent.saveHistoryCheckpoint"] {
        assert!(s.execute(command, json!({"token":following["token"]})).is_err());
    }
    assert_eq!(state(&s), before);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (_, receipt) = run_pair(&mut s, &following, default());
    let header = advertisement_header(&receipt);
    assert_eq!(header.version, 2);
    assert_eq!(header.following_write, Some(json!(true)));
    assert_eq!(receipt["originalBasis"], advertisement_envelope(&header));
}
#[test]
fn advertised_following_capsule_seven_roundtrips_new_basis_for_all_trim_kinds_without_restoring_live_authority() {
    for kind in ["trim", "ripple", "roll"] {
        for mode in [false, true] {
            let dir = Scratch::new();
            let mut s = setup(true);
            s.path = Some(dir.file());
            dynamic(&mut s, kind, false);
            let d = follow(&mut s, 197100);
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
            let setting = if mode { super::super::mode(&s, "Touch") } else { default() };
            let (p, r) = run_pair(&mut s, &d, setting);
            if mode {
                s.execute("trim.shuttleStop", json!({})).unwrap();
            }
            let saved = save(&mut s, 197101);
            assert_eq!(raw(&dir)["version"], 7);
            let mut cold = reopened(&s);
            restore(&mut cold, 197102, &saved).unwrap();
            check_restored(&mut cold, &p, &r);
            let owner = cold.agent_bridge.tickets.iter().find(|t| t.token == p["token"]).unwrap().editor_control.as_ref().unwrap();
            assert!(owner.post_binding.is_none());
            let unchanged = state(&cold);
            assert!(
                cold.execute(
                    "agent.readEditorSettings",
                    json!({"version":2,"token":p["token"],"scope":scope(),"beforeEditorSha256":r["originalRead"]["editorStateSha256"],"offset":0,"limit":1})
                )
                .is_err()
            );
            assert_eq!(state(&cold), unchanged);
            assert_eq!(r["originalRead"]["capabilities"]["compoundEditorWrite"], true);
            let mut other = p.clone();
            other["setting"] = json!({"kind":"default_transition","definition":"cross_dissolve"});
            assert!(cold.execute(COMMAND, other).is_err());
        }
    }
}
#[test]
fn advertised_reader_preserves_legacy_unadvertised_capsule_seven_recipe_and_receipt_bytes_without_upgrading_it() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "roll", false);
    let d = follow(&mut s, 197200);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, r) = run_pair(&mut s, &d, default());
    let legacy = legacy_advertisement(&mut s, &p, &r);
    let header = advertisement_header(&legacy);
    assert_eq!(header.version, 1);
    assert!(header.following_write.is_none());
    assert_eq!(legacy["originalBasis"]["data"], format!("{{\"version\":1,\"revision\":{},\"saved_revision\":{}}}", header.revision, header.saved_revision));
    let saved = save(&mut s, 197201);
    let mut cold = reopened(&s);
    restore(&mut cold, 197202, &saved).unwrap();
    check_restored(&mut cold, &p, &legacy);
    assert_eq!(legacy["originalRead"]["capabilities"]["compoundEditorWrite"], false);
    assert_eq!(legacy["publicationRead"]["capabilities"]["compoundEditorWrite"], false);
    let before = state(&cold);
    assert_eq!(cold.execute(COMMAND, p).unwrap(), legacy);
    assert_eq!(state(&cold), before);
}
#[test]
fn advertised_and_legacy_following_outcomes_mix_with_strict_control_history_without_record_rewrite() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    let strict = prepare_read(&mut s, 197300);
    let old_request = request_for(&strict, default());
    let old_receipt = s.execute(COMMAND, old_request.clone()).unwrap();
    let d = follow(&mut s, 197301);
    let (legacy_request, r) = run_pair(&mut s, &d, default());
    let legacy = legacy_advertisement(&mut s, &legacy_request, &r);
    let d = follow(&mut s, 197302);
    let setting = super::super::mode(&s, "Write");
    let (new_request, new_receipt) = run_pair(&mut s, &d, setting);
    let saved = save(&mut s, 197303);
    let mut cold = reopened(&s);
    restore(&mut cold, 197304, &saved).unwrap();
    assert_eq!(cold.execute(COMMAND, old_request).unwrap(), old_receipt);
    check_restored(&mut cold, &legacy_request, &legacy);
    check_restored(&mut cold, &new_request, &new_receipt);
    assert_eq!(advertisement_header(&legacy).version, 1);
    assert_eq!(advertisement_header(&new_receipt).version, 2);
}
#[test]
fn advertised_origin_versions_flags_and_read_capabilities_refuse_rehashed_contradictions_before_restore() {
    for case in 0..8 {
        let (dir, s, p, r, saved) = prepared("roll", 197400 + case * 3);
        corrupt_restore(&dir, &s, &p, &saved, |v| {
            let result = &mut original_ticket(v, &p)["result"];
            if case == 5 {
                result["originalRead"]["capabilities"]["compoundEditorWrite"] = json!(false);
            } else if case == 6 {
                result["publicationRead"]["capabilities"]["compoundEditorWrite"] = json!(false);
            } else {
                let mut header = advertisement_header(&r);
                match case {
                    0 => header.version = 1,
                    1 => header.following_write = Some(json!(false)),
                    2 => header.following_write = None,
                    3 => header.version = 9,
                    4 => header.following_write = Some(json!("true")),
                    _ => header.following_write = Some(Value::Null),
                }
                result["originalBasis"] = advertisement_envelope(&header);
            }
        });
    }
}
#[test]
fn legacy_origin_missing_flag_is_distinct_from_explicit_false_and_new_format_cannot_omit_its_true_flag() {
    for case in 0..3 {
        let dir = Scratch::new();
        let mut s = setup(true);
        s.path = Some(dir.file());
        let d = follow(&mut s, 197500 + case * 3);
        let (p, r) = run_pair(&mut s, &d, default());
        let legacy = legacy_advertisement(&mut s, &p, &r);
        let saved = save(&mut s, 197501 + case * 3);
        corrupt_restore(&dir, &s, &p, &saved, |v| {
            let mut header = advertisement_header(&legacy);
            match case {
                0 => header.following_write = Some(json!(false)),
                1 => header.following_write = Some(json!(true)),
                _ => header.version = 2,
            }
            original_ticket(v, &p)["result"]["originalBasis"] = advertisement_envelope(&header);
        });
    }
}
