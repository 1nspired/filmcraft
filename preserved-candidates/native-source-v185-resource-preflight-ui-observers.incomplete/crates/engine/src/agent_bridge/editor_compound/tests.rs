use super::*;
use crate::{
    History,
    agent_bridge::{editor_settings, tests::token},
    project_workspace,
};
use filmcraft_time::Tick;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
const W: &str = "16ac27d2-2fa8-42d9-af80-c510af875c6a";
const C: &str = "b0050716-4195-4242-942b-862f96d358c1";
fn scope() -> Value {
    json!({"projectId":"p_compound_editor","workspaceId":W})
}
fn setup(demo: bool) -> Session {
    let mut s = Session::default();
    if demo {
        s.execute("file.openDemoProject", json!({})).unwrap();
        let root: ProjectScope = serde_json::from_value(scope()).unwrap();
        s.project = Arc::new(project_workspace::legacy_identity_candidate(&s.project, &root, s.state.active_sequence.unwrap()).unwrap());
        for name in ["V2", "V3", "A2", "A3"] {
            s.execute("timeline.setTrack", json!({"track":name,"syncLock":false})).unwrap();
        }
    } else {
        s.execute("file.newProject", json!({"name":"Generated compound editor","workspace":scope()})).unwrap();
    }
    s.history = History { limit: 128, ..History::default() };
    s.path = Some(crate::temp_dir().join("generated-compound-editor.fcproj").to_string_lossy().into_owned());
    s.agent_operation_control().enable_transport();
    s
}
fn select(s: &mut Session, kind: &str) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":kind})).unwrap();
}
fn dynamic(s: &mut Session, kind: &str, moved: bool) {
    select(s, kind);
    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
    if moved {
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    }
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"markers":s.state.clipboard_markers,
    "revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,"journal":s.journal,
    "trim":format!("{:?}",s.trim_play),"recording":format!("{:?}",s.mixrec),"multicam":format!("{:?}",s.mcrec),"preview":s.previews.live.is_active()})
}
fn twin(s: &Session) -> Session {
    let mut t = Session::default();
    t.project = s.project.clone();
    t.state = s.state.clone();
    t.history = s.history.clone();
    t.revision = s.revision;
    t.saved_revision = s.saved_revision;
    t.path = s.path.clone();
    t.journal = s.journal.clone();
    t.trim_play = s.trim_play.clone();
    t.mixrec = s.mixrec.clone();
    t.mcrec = s.mcrec.clone();
    t
}
fn root(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn prepare_read(s: &mut Session, n: usize) -> Value {
    s.execute(editor_control::PREPARE, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn default() -> Value {
    json!({"kind":"default_transition","definition":"constant_gain"})
}
fn mode(s: &Session, value: &str) -> Value {
    json!({"kind":"clip_mixer_mode","track":s.active_sequence().unwrap().audio_tracks[0].id.0,"mode":value})
}
fn request_for(d: &Value, setting: Value) -> Value {
    json!({"version":2,"token":d["token"],"scope":d["scope"],"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":setting})
}
fn issue(s: &mut Session, n: usize, setting: Value) -> Value {
    request_for(&prepare_read(s, n), setting)
}
fn check_history(s: &Session, p: &Value, r: &Value) -> Result<Retained> {
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
    let retained = super::retained(t).unwrap();
    let pool = retained.projects();
    let typed = super::request(p).unwrap();
    let h = retained.history(&typed.setting).unwrap();
    validate_history(
        pool,
        &(0..pool.len()).collect::<Vec<_>>(),
        h.before_project(),
        h.after_project(),
        s.path.as_ref().unwrap(),
        &typed.scope,
        &typed.token,
        t.revision,
        t.sequence.map(|id| id.0),
        t.after_sequence.map(|id| id.0),
        p,
        r,
        None,
    )
}
fn paired(s: &mut Session, n: usize, setting: Value) -> (Value, Value) {
    let p = issue(s, n, setting);
    let mut human = twin(s);
    let typed = super::request(&p).unwrap();
    let (command, args) = editor_setting::human(&typed.setting);
    human.execute(command, args).unwrap();
    let result = s.execute(COMMAND, p.clone()).unwrap();
    assert_eq!(state(s), state(&human));
    assert_eq!(result["agentTimelineCommits"], 0);
    assert_eq!(result["agentUndoOwned"], false);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["mediaEvidence"], false);
    check_history(s, &p, &result).unwrap();
    (p, result)
}
#[test]
fn explicit_change_read_keeps_empty_root_and_original_readonly_purpose_separate() {
    let mut s = setup(false);
    let before = state(&s);
    let d = prepare_read(&mut s, 180000);
    assert_eq!(state(&s), before);
    assert_eq!(d["version"], 2);
    assert_eq!(d["readPurpose"], "editor_setting_preparation");
    assert_eq!(d["capabilities"]["compoundEditorWrite"], true);
    assert_eq!(d["sequence"], Value::Null);
    assert_eq!(d["writeAuthority"], false);
    let readonly = s.execute(editor_control::READ, json!({"token":token(180001),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(readonly["version"], 1);
    assert_eq!(readonly["capabilities"]["compoundEditorWrite"], false);
    assert!(s.execute(COMMAND, request_for(&readonly, default())).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn compound_defaults_match_actual_human_trim_project_editor_linked_sound_history_and_undo_redo() {
    let mut n = 180010;
    for kind in ["trim", "ripple", "roll"] {
        for moved in [false, true] {
            for limit in [0, 1, 128] {
                let mut s = setup(true);
                dynamic(&mut s, kind, moved);
                s.history.limit = limit;
                s.history.redo.push(("Earlier human Redo".into(), Arc::new((*s.project).clone())));
                s.history.merge_key = Some("previous human drag".into());
                let mut human = twin(&s);
                let p = issue(&mut s, n, default());
                n += 1;
                human.execute("effects.setDefaultTransition", json!({"effect":"constant_gain"})).unwrap();
                let r = s.execute(COMMAND, p.clone()).unwrap();
                assert_eq!(state(&s), state(&human));
                assert_eq!(r["trim"]["dynamicCommitted"], moved);
                assert_eq!(r["historyUnchanged"], !moved);
                assert_eq!(r["trim"]["undoEntryRetained"], moved && limit > 0);
                assert!(!s.trim_play.active());
                check_history(&s, &p, &r).unwrap();
                assert_eq!(r["agentTimelineCommits"], 0);
                assert_eq!(r["agentUndoOwned"], false);
                if moved && limit > 0 {
                    s.execute("edit.undo", json!({})).unwrap();
                    human.execute("edit.undo", json!({})).unwrap();
                    assert_eq!(state(&s), state(&human));
                    s.execute("edit.redo", json!({})).unwrap();
                    human.execute("edit.redo", json!({})).unwrap();
                    assert_eq!(state(&s), state(&human));
                }
            }
        }
    }
}
#[test]
fn compound_default_without_trim_or_with_only_loop_keeps_original_history_and_journal() {
    for looping in [false, true] {
        let mut s = setup(true);
        select(&mut s, "roll");
        if looping {
            s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
        }
        let (_, r) = paired(&mut s, 180100 + usize::from(looping), default());
        assert_eq!(r["trim"]["loopStopped"], looping);
        assert_eq!(r["trim"]["dynamicCommitted"], false);
        assert!(s.history.undo.is_empty());
    }
}
#[test]
fn compound_every_canonical_transition_works_on_a_genuine_empty_root_without_an_anchor() {
    let defs = filmcraft_project::effect_defs()
        .iter()
        .filter(|d| matches!(d.kind, filmcraft_project::EffectKind::AudioTransition | filmcraft_project::EffectKind::VideoTransition))
        .collect::<Vec<_>>();
    assert_eq!(defs.len(), 115);
    for (i, d) in defs.into_iter().enumerate() {
        let mut s = setup(false);
        let (_, r) = paired(&mut s, 180200 + i, json!({"kind":"default_transition","definition":d.id}));
        assert_eq!(r["sequence"], Value::Null);
        assert!(s.project.sequences().next().is_none());
    }
}
#[test]
fn compound_modes_preserve_all_five_human_modes_live_trim_recording_and_preview_owners() {
    for (i, m) in filmcraft_project::AutomationMode::ALL.into_iter().enumerate() {
        for live in 0..5 {
            let mut s = setup(true);
            match live {
                1 => dynamic(&mut s, "roll", true),
                2 => {
                    select(&mut s, "roll");
                    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
                }
                3 => {
                    let track = s.active_sequence().unwrap().audio_tracks[0].id;
                    s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":0})).unwrap();
                }
                4 => {
                    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
                }
                _ => {}
            }
            let before_trim = format!("{:?}", s.trim_play);
            let before_rec = format!("{:?}", s.mixrec);
            let preview = s.previews.live.is_active();
            let setting = mode(&s, m.label());
            let p = issue(&mut s, 180400 + i * 5 + live, setting.clone());
            let original_project = s.project.clone();
            let original_history = s.history.clone();
            let before_journal = s.journal.clone();
            let r = s.execute(COMMAND, p.clone()).unwrap();
            assert!(Arc::ptr_eq(&original_project, &s.project));
            assert_eq!(format!("{:?}", s.trim_play), before_trim);
            assert_eq!(format!("{:?}", s.mixrec), before_rec);
            assert_eq!(s.previews.live.is_active(), preview);
            assert_eq!(s.journal, before_journal);
            assert_eq!(s.history.undo.len(), original_history.undo.len());
            assert_eq!(s.history.redo.len(), original_history.redo.len());
            assert_eq!(r["trim"]["settlementApplied"], false);
            assert_eq!(r["trim"]["dynamicCommitted"], false);
            assert_eq!(r["historyUnchanged"], true);
            assert_eq!(crate::mixer::clip_mode(&s, filmcraft_project::TrackId(setting["track"].as_u64().unwrap())).label(), m.label());
            check_history(&s, &p, &r).unwrap();
        }
    }
}
#[test]
fn zero_offset_restoration_keeps_distinct_equal_byte_snapshots_and_reports_identity_change() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let old = s.project.clone();
    s.project = Arc::new((*old).clone());
    s.state.trim_shift.out_shift = Tick(42);
    let (p, r) = paired(&mut s, 180500, default());
    assert!(Arc::ptr_eq(&s.project, &old));
    assert_eq!(r["projectUnchanged"], false);
    assert_eq!(r["beforeProjectSha256"], r["sha256"]);
    assert_eq!(r["trim"]["restoredBeforeProject"], true);
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
    let retained = super::retained(t).unwrap();
    assert_eq!(retained.projects().len(), 2);
    assert!(!Arc::ptr_eq(&retained.projects()[0], &retained.projects()[1]));
}
#[test]
fn stop_and_unavailable_transport_refuse_before_either_original_human_effect() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let p = issue(&mut s, 180510, default());
    let before = state(&s);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute(COMMAND, p).is_err());
    assert_eq!(state(&s), before);
    let mut s = setup(false);
    s.agent_bridge.operations = super::super::operation_control::AgentOperationControl::default();
    let before = state(&s);
    assert!(s.execute(editor_control::PREPARE, json!({"token":token(180511),"scope":scope(),"projectPath":s.path})).is_err());
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
}
#[test]
fn lost_reply_reconciles_the_original_outcome_after_later_human_trim_and_settings_without_replay() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let p = issue(&mut s, 180520, default());
    let r = s.execute(COMMAND, p.clone()).unwrap();
    s.execute("effects.setDefaultTransition", json!({"effect":"exponential_fade"})).unwrap();
    dynamic(&mut s, "trim", false);
    let later = state(&s);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(state(&s), later);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":p["token"]})).unwrap()["cancellationAccepted"], false);
    let mut different = p;
    different["setting"]["definition"] = json!("cross_dissolve");
    assert!(s.execute(COMMAND, different).is_err());
    assert_eq!(state(&s), later);
}

#[test]
fn stale_native_project_editor_clipboard_history_path_and_private_trim_owners_refuse_atomically() {
    for case in 0..15 {
        let mut s = setup(true);
        select(&mut s, "roll");
        if case < 11 || case == 13 {
            s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
        }
        let p = issue(&mut s, 180600 + case, default());
        match case {
            0 => s.project = Arc::new((*s.project).clone()),
            1 => s.state.snapping = !s.state.snapping,
            2 => s
                .state
                .clipboard_markers
                .push(serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"Private","comment":"","color":"Iris","kind":"Comment"})).unwrap()),
            3 => s.history.limit = 1,
            4 => s.history.undo.push(("Later human Undo".into(), s.project.clone())),
            5 => s.history.redo.push(("Later human Redo".into(), s.project.clone())),
            6 => s.history.merge_key = Some("Later human gesture".into()),
            7 => s.path = Some(crate::temp_dir().join("other-root.fcproj").to_string_lossy().into_owned()),
            8 => s.trim_play.dynamic.as_mut().unwrap().speed = 2.0,
            9 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
            10 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
            }
            11 => {
                s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
            }
            12 => {
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
            13 => {
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
            _ => s.saved_revision = s.saved_revision.saturating_add(1),
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "stale {case}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn hostile_mixed_legacy_readonly_sequence_and_source_purposes_never_publish_compound_effects() {
    for case in 0..14 {
        let mut s = setup(true);
        let mut p = issue(&mut s, 180700 + case, default());
        match case {
            0 => p["version"] = json!(1),
            1 => {
                p.as_object_mut().unwrap().remove("version");
            }
            2 => p["execute"] = json!("edit.undo"),
            3 => p["writeAuthority"] = json!(true),
            4 => p["scope"]["workspaceId"] = json!(C),
            5 => p["beforeSha256"] = json!("0".repeat(64)),
            6 => p["beforeEditorSha256"] = json!("f".repeat(64)),
            7 => p["setting"]["definition"] = json!("Constant Gain"),
            8 => p["setting"]["kind"] = json!("command"),
            9 => p["setting"]["unexpected"] = json!(true),
            10 => {
                p = request_for(&root(&mut s, 180790), default());
            }
            11 => {
                let d = s.execute(editor_control::READ, json!({"token":token(180791),"scope":scope(),"projectPath":s.path})).unwrap();
                p = request_for(&d, default());
            }
            12 => {
                s.execute("agent.snapshot", json!({"token":token(180792)})).unwrap();
                p["token"] = json!(token(180792));
            }
            _ => s.agent_bridge.tickets.back_mut().unwrap().source_library = Some(vec![1]),
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "hostile {case}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn change_tickets_keep_the_original_finite_hand_and_eviction_without_new_project_or_source_authority() {
    let mut s = setup(false);
    let d = prepare_read(&mut s, 180800);
    let before = state(&s);
    for command in [
        "agent.apply",
        "agent.undo",
        "agent.redo",
        editor_setting::COMMAND,
        "agent.readTracks",
        "agent.readBins",
        "agent.saveHistoryCheckpoint",
        "agent.restoreHistoryCheckpoint",
        "agent.inspect",
    ] {
        let e = s.execute(command, json!({"token":d["token"]})).unwrap_err();
        assert!(e.to_string().contains("editor-only ticket"), "{command}: {e}");
        assert_eq!(state(&s), before);
    }
    assert!(super::super::check_edit_ticket_basis(&s, d["token"].as_str().unwrap()).is_err());
    for i in 0..=super::super::TICKET_LIMIT {
        prepare_read(&mut s, 181000 + i);
    }
    assert_eq!(s.agent_bridge.tickets.len(), super::super::TICKET_LIMIT);
    assert!(s.execute(COMMAND, request_for(&d, default())).is_err());
    assert_eq!(state(&s), before);
}
fn rehash(v: &mut Value) {
    let raw = v["data"].as_str().unwrap().as_bytes();
    let bytes = raw.len();
    let hash = history_checkpoint::digest(raw);
    v["bytes"] = json!(bytes);
    v["sha256"] = json!(hash);
}
#[test]
fn compound_native_i64_editor_proof_and_history_proof_reject_rehashed_unrelated_or_hidden_effects() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    s.state.source_playhead = Tick(9007199254740993);
    let p = issue(&mut s, 181400, default());
    let r = s.execute(COMMAND, p.clone()).unwrap();
    assert!(r["editorProof"]["data"].as_str().unwrap().contains("9007199254740993"));
    for case in 0..8 {
        let mut bad = r.clone();
        match case {
            0 => bad["agentUndoOwned"] = json!(true),
            1 => bad["agentTimelineCommits"] = json!(1),
            2 => bad["historyUnchanged"] = json!(false),
            3 => bad["writeAuthority"] = json!(true),
            4 => {
                let mut v: Value = serde_json::from_str(bad["editorProof"]["data"].as_str().unwrap()).unwrap();
                v["afterEditor"]["snapping"] = json!(!s.state.snapping);
                bad["editorProof"]["data"] = json!(serde_json::to_string(&v).unwrap());
                rehash(&mut bad["editorProof"]);
            }
            5 => {
                let mut v: Value = serde_json::from_str(bad["historyProof"]["data"].as_str().unwrap()).unwrap();
                v["snapshots"][0] = json!("0".repeat(64));
                bad["historyProof"]["data"] = json!(serde_json::to_string(&v).unwrap());
                rehash(&mut bad["historyProof"]);
            }
            6 => {
                let mut v: Value = serde_json::from_str(bad["historyProof"]["data"].as_str().unwrap()).unwrap();
                let mut evidence: Value = serde_json::from_str(v["evidence"].as_str().unwrap()).unwrap();
                evidence["before_shift"]["outShift"] = json!(123);
                v["evidence"] = json!(serde_json::to_string(&evidence).unwrap());
                bad["historyProof"]["data"] = json!(serde_json::to_string(&v).unwrap());
                rehash(&mut bad["historyProof"]);
            }
            _ => bad["trim"]["settlementApplied"] = json!(false),
        }
        let before = state(&s);
        assert!(check_history(&s, &p, &bad).is_err(), "forged {case}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn preserved_mode_evidence_has_explicit_version_and_cannot_borrow_settlement_or_unknown_policy() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let p = command_settlement::prepare(&s, "effects.setDefaultTransition").unwrap();
    let c = p.capture_preserved_history("clipMixer.setMode").unwrap();
    let original: Value = serde_json::from_slice(&c.evidence().encode().unwrap()).unwrap();
    assert_eq!(original["version"], 2);
    assert_eq!(original["preserve"], true);
    assert!(Arc::ptr_eq(c.historical().before_project(), c.historical().after_project()));
    assert!(!c.historical().summary().dynamic_committed);
    for case in 0..5 {
        let mut v = original.clone();
        match case {
            0 => v["version"] = json!(1),
            1 => v["preserve"] = json!(false),
            2 => v["preserve"] = Value::Null,
            3 => v["command"] = json!("effects.setDefaultTransition"),
            _ => {
                v.as_object_mut().unwrap().remove("preserve");
            }
        }
        assert!(historical::Evidence::decode(&serde_json::to_vec(&v).unwrap()).and_then(|e| e.validate(c.projects(), "clipMixer.setMode")).is_err());
    }
    let legacy = p.capture_history().unwrap();
    let mut v: Value = serde_json::from_slice(&legacy.evidence().encode().unwrap()).unwrap();
    assert!(v.get("preserve").is_none());
    v["preserve"] = Value::Null;
    assert!(historical::Evidence::decode(&serde_json::to_vec(&v).unwrap()).is_err());
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("filmcraft-compound-editor-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(std::fs::canonicalize(p).unwrap())
    }
    fn file(&self) -> String {
        self.0.join("generated.fcproj").to_string_lossy().into_owned()
    }
    fn companion(&self) -> PathBuf {
        self.0.join(format!(".poema-history-{W}-{C}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(Path::new(s.path.as_ref().unwrap()), &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false))
        .unwrap();
    s.saved_revision = s.revision;
}
fn save(s: &mut Session, n: usize) -> Value {
    persist(s);
    root(s, n);
    s.execute("agent.saveHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap()
}
fn reopened(s: &Session) -> Session {
    let mut c = Session::default();
    c.execute("file.open", json!({"path":s.path})).unwrap();
    c.agent_operation_control().enable_transport();
    c
}
fn restore(s: &mut Session, n: usize, saved: &Value) -> Result<Value> {
    root(s, n);
    s.execute("agent.restoreHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}))
}
#[test]
fn capsule_six_retains_zero_limit_and_equal_byte_zero_offset_evidence_in_a_real_native_cold_restore() {
    for zero in [false, true] {
        let dir = Scratch::new();
        let mut s = setup(true);
        s.path = Some(dir.file());
        dynamic(&mut s, "roll", !zero);
        s.history.limit = 0;
        if zero {
            s.project = Arc::new((*s.project).clone());
            s.state.trim_shift.out_shift = Tick(42);
        }
        let p = issue(&mut s, 181500, default());
        let r = s.execute(COMMAND, p.clone()).unwrap();
        let saved = save(&mut s, 181501);
        let raw: Value = serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap();
        assert_eq!(raw["version"], 6);
        assert_eq!(raw["limit"], 0);
        assert_eq!(raw["projects"].as_array().unwrap().len(), 2);
        assert_eq!(raw["tickets"].as_array().unwrap().len(), 1);
        let mut cold = reopened(&s);
        let original_editor = project_clear::editor_hash(&cold.state).unwrap();
        restore(&mut cold, 181502, &saved).unwrap();
        assert_eq!(project_clear::editor_hash(&cold.state).unwrap(), original_editor);
        assert_eq!(cold.history.limit, 0);
        assert!(cold.history.undo.is_empty());
        let t = cold.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
        assert!(t.editor_control.as_ref().unwrap().settlement.is_none());
        assert_eq!(t.editor_control.as_ref().unwrap().outcome.as_ref().unwrap().projects().len(), 2);
        cold.execute("effects.setDefaultTransition", json!({"effect":"cross_dissolve"})).unwrap();
        let later = state(&cold);
        assert_eq!(cold.execute(COMMAND, p.clone()).unwrap(), r);
        assert_eq!(state(&cold), later);
        assert_eq!(cold.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
        let e = cold.execute("agent.apply", json!({"token":p["token"]})).unwrap_err();
        assert!(e.to_string().contains("editor-only ticket"));
        assert_eq!(state(&cold), later);
    }
}
#[test]
fn capsule_six_hot_restore_preserves_exact_outcome_and_later_human_settings_without_a_live_preparation() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "ripple", true);
    let p = issue(&mut s, 181510, default());
    let r = s.execute(COMMAND, p.clone()).unwrap();
    let saved = save(&mut s, 181511);
    restore(&mut s, 181512, &saved).unwrap();
    let track = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Write"})).unwrap();
    let later = state(&s);
    assert_eq!(s.execute(COMMAND, p).unwrap(), r);
    assert_eq!(state(&s), later);
}
fn owned_bin(s: &mut Session, n: usize) {
    let d = root(s, n);
    s.execute("agent.apply",json!({"token":token(n),"label":"Owned generated bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":{"type":"create","name":"Owned generated bin","parent":null}}}]})).unwrap();
}
#[test]
fn capsule_six_mixes_older_navigation_and_setting_receipts_without_taking_prior_agent_undo() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    let before = s.project.clone();
    owned_bin(&mut s, 181520);
    let after = s.project.clone();
    let d = root(&mut s, 181521);
    let association = crate::editorial_workspace::association(&s.project).unwrap().unwrap();
    s.execute(super::super::project_navigation::COMMAND,json!({"token":d["token"],"scope":scope(),"sequenceScope":association.scope,"item":s.state.active_sequence.unwrap().0,"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"]})).unwrap();
    let d = root(&mut s, 181522);
    let setting = mode(&s, "Touch");
    s.execute(
        editor_setting::COMMAND,
        json!({"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":setting}),
    )
    .unwrap();
    let p = issue(&mut s, 181523, default());
    let r = s.execute(COMMAND, p.clone()).unwrap();
    let saved = save(&mut s, 181524);
    let raw: Value = serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 6);
    assert_eq!(raw["tickets"].as_array().unwrap().len(), 4);
    let mut cold = reopened(&s);
    restore(&mut cold, 181525, &saved).unwrap();
    let d = root(&mut cold, 181526);
    assert!(cold.execute("agent.undo", json!({"token":d["token"],"commit":p["token"]})).is_err());
    let d = root(&mut cold, 181527);
    cold.execute("agent.undo", json!({"token":d["token"],"commit":token(181520)})).unwrap();
    assert_eq!(cold.project, before);
    let d = root(&mut cold, 181528);
    cold.execute("agent.redo", json!({"token":d["token"],"commit":token(181520),"undo":token(181527)})).unwrap();
    assert_eq!(cold.project, after);
    assert_eq!(cold.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
}
#[test]
fn capsule_six_downgrade_forged_map_hidden_effect_and_detached_graph_refuse_before_restore() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.file());
    dynamic(&mut s, "roll", false);
    s.project = Arc::new((*s.project).clone());
    let p = issue(&mut s, 181600, default());
    s.execute(COMMAND, p).unwrap();
    let saved = save(&mut s, 181601);
    let original = std::fs::read(dir.companion()).unwrap();
    for case in 0..16 {
        let mut v: Value = serde_json::from_slice(&original).unwrap();
        match case {
            0..=4 => v["version"] = json!(case + 1),
            5 => v["tickets"][0]["editor_control"]["version"] = json!(2),
            6 => v["tickets"][0]["editor_control"]["snapshots"] = json!([0, 0]),
            7 => v["tickets"][0]["editor_control"]["snapshots"] = json!([999]),
            8 => {
                v["tickets"][0].as_object_mut().unwrap().remove("editor_control");
            }
            9 => v["tickets"][0]["redo_consumed"] = json!(true),
            10 => v["tickets"][0]["after"] = v["tickets"][0]["project"].clone(),
            11 => v["tickets"][0]["project_scope"]["workspaceId"] = json!(C),
            12 => v["tickets"][0]["result"]["agentUndoOwned"] = json!(true),
            13 => {
                let copy = v["projects"][0].clone();
                v["projects"].as_array_mut().unwrap().push(copy);
            }
            14 => v["tickets"][0]["editor_control"]["unknown"] = json!(true),
            _ => v["tickets"][0]["request"]["version"] = json!(1),
        }
        let raw = serde_json::to_vec(&v).unwrap();
        std::fs::write(dir.companion(), &raw).unwrap();
        let mut altered = saved.clone();
        altered["checkpoint"]["sha256"] = json!(history_checkpoint::digest(&raw));
        let mut cold = reopened(&s);
        let before = state(&cold);
        assert!(restore(&mut cold, 181602, &altered).is_err(), "forged capsule {case}");
        assert_eq!(state(&cold), before);
    }
    std::fs::write(dir.companion(), original).unwrap();
}
#[test]
fn new_change_reads_support_exact_existing_settings_catalog_pages_without_source_or_timeline_qualification() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", false);
    let d = prepare_read(&mut s, 181700);
    let before = state(&s);
    let page = json!({"token":d["token"],"scope":scope(),"beforeEditorSha256":d["editorStateSha256"],"offset":0,"limit":32});
    let settings = s.execute(editor_settings::READ, page.clone()).unwrap();
    let catalog = s.execute(editor_setting::CATALOG, page).unwrap();
    assert_eq!(settings["mediaEvidence"], false);
    assert_eq!(catalog["total"], 115);
    assert_eq!(state(&s), before);
}

#[test]
fn targeted_fresh_control_read_attests_exact_current_setting_and_native_history_after_settlement() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    let p = issue(&mut s, 181800, default());
    let r = s.execute(COMMAND, p).unwrap();
    let d = prepare_read(&mut s, 181801);
    assert_eq!(d["historyState"], r["afterHistory"]);
    assert_ne!(r["beforeHistory"], r["afterHistory"]);
    let settings = s
        .execute(editor_settings::READ, json!({"token":d["token"],"scope":scope(),"beforeEditorSha256":d["editorStateSha256"],"offset":0,"limit":32}))
        .unwrap();
    assert_eq!(settings["defaults"]["audio"]["definition"], "constant_gain");
    assert_eq!(d["editorStateSha256"], r["afterEditorSha256"]);
    assert_eq!(d["historyState"]["undoSteps"], 1);
    assert_eq!(r["agentUndoOwned"], false);
}
#[test]
fn zero_limit_commit_is_disclosed_even_when_exact_retained_history_digests_are_equal() {
    let mut s = setup(true);
    dynamic(&mut s, "roll", true);
    s.history.limit = 0;
    let p = issue(&mut s, 181810, default());
    let r = s.execute(COMMAND, p).unwrap();
    assert_eq!(r["beforeHistory"], r["afterHistory"]);
    assert_eq!(r["historyUnchanged"], false);
    assert_eq!(r["trim"]["dynamicCommitted"], true);
    assert_eq!(r["trim"]["undoEntryRetained"], false);
    assert_eq!(prepare_read(&mut s, 181811)["historyState"], r["afterHistory"]);
}
#[test]
fn changed_history_and_private_live_owner_expire_setting_pages_before_control_readback_can_be_paid() {
    for case in 0..3 {
        let mut s = setup(true);
        dynamic(&mut s, "roll", false);
        let d = prepare_read(&mut s, 181820 + case);
        match case {
            0 => s.history.merge_key = Some("Later human gesture".into()),
            1 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
                s.execute("trim.shuttle", json!({"clock":0.0})).unwrap();
            }
            _ => s.history.redo.push(("Later human Redo".into(), s.project.clone())),
        }
        let p = json!({"token":d["token"],"scope":scope(),"beforeEditorSha256":d["editorStateSha256"],"offset":0,"limit":32});
        let before = state(&s);
        assert!(s.execute(editor_settings::READ, p.clone()).is_err());
        assert!(s.execute(editor_setting::CATALOG, p).is_err());
        assert_eq!(state(&s), before);
    }
}

mod following;
