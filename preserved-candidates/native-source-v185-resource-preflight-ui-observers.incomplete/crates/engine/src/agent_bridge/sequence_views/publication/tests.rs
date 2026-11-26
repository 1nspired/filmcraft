use super::*;
use crate::{
    History,
    agent_bridge::{editor_control, project_navigation, tests::token},
};
use filmcraft_time::Tick;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
const W: &str = "8be08f84-4574-42b0-81a2-01fa66e66845";
const C: &str = "733a82ec-037b-4b75-8199-e46b3ed72fd6";
fn scope() -> Value {
    json!({"projectId":"p_native_views","workspaceId":W})
}
fn setup() -> (Session, Vec<ItemId>) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let a = s.state.active_sequence.unwrap();
    s.project = Arc::new(project_workspace::legacy_identity_candidate(&s.project, &serde_json::from_value(scope()).unwrap(), a).unwrap());
    for track in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
    }
    let mut ids = vec![a];
    for name in ["Owned second", "Foreign third"] {
        let v = s.execute("file.newSequence", json!({"name":name,"video":0,"audio":0})).unwrap();
        ids.push(ItemId(v["sequence"].as_u64().unwrap()));
    }
    Arc::make_mut(&mut s.project).item_mut(ids[2]).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
    s.execute("sequence.open", json!({"item":a.0})).unwrap();
    s.state.open_sequences = ids.clone();
    s.history = History { limit: 128, ..History::default() };
    s.state.selection = vec![s.active_sequence().unwrap().video_tracks[0].items[0].id];
    s.path = Some(crate::temp_dir().join("generated-native-views.fcproj").to_string_lossy().into_owned());
    s.events.clear();
    s.journal.clear();
    s.agent_operation_control().enable_transport();
    (s, ids)
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"markers":s.state.clipboard_markers,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"limit":s.history.limit,"merge":s.history.merge_key,"prefs":s.prefs,"journal":s.journal,"trim":format!("{:?}",s.trim_play),"events":format!("{:?}",s.events),"recording":format!("{:?}",s.mixrec),"multicam":format!("{:?}",s.mcrec),"previewActive":s.previews.live.is_active()})
}
fn twin(s: &Session) -> Session {
    let mut h = Session::default();
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.history = s.history.clone();
    h.revision = s.revision;
    h.saved_revision = s.saved_revision;
    h.path = s.path.clone();
    h.journal = s.journal.clone();
    h.trim_play = s.trim_play.clone();
    h.mixrec = s.mixrec.clone();
    h.mcrec = s.mcrec.clone();
    h.prefs = s.prefs.clone();
    h.events = s.events.clone();
    h.previews = s.previews.clone();
    h
}
fn dynamic(s: &mut Session, kind: &str, moved: bool) {
    let id = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":id.0,"edge":"out","kind":kind})).unwrap();
    s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0.0})).unwrap();
    if moved {
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    }
}
fn around(s: &mut Session) {
    let id = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":id.0,"edge":"out","kind":"roll"})).unwrap();
    s.execute("trim.playAround", json!({"clock":0.0})).unwrap();
}
fn prepare(s: &mut Session, n: usize) -> Value {
    s.execute(super::super::PREPARE, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn op(kind: &str, item: ItemId, owned: bool) -> Value {
    let mut p = json!({"type":kind,"item":item.0,"owned":owned});
    if kind == "move_tab" {
        p["index"] = json!(u64::MAX);
    }
    p
}
fn args(d: &Value, operation: Value) -> Value {
    json!({"version":1,"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"operation":operation})
}
fn pool(s: &Session, p: &Value) -> (Vec<Arc<Project>>, Vec<usize>, Vec<usize>) {
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
    let r = retained(t).unwrap();
    let mut pool = Vec::new();
    for item in r.projects().iter().chain(r.original.as_ref().unwrap().projects()) {
        if !pool.iter().any(|x| Arc::ptr_eq(x, item)) {
            pool.push(item.clone());
        }
    }
    let a = r.mapping(&pool).unwrap();
    let b = r.original.as_ref().unwrap().mapping(&pool).unwrap();
    (pool, a, b)
}
fn validate(s: &Session, p: &Value, r: &Value) -> Result<Retained> {
    let (pool, a, b) = pool(s, p);
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
    let h = retained(t).unwrap().validate(request(p).unwrap().operation.command().id()).unwrap();
    validate_history(
        &pool,
        &a,
        &b,
        h.before_project(),
        h.after_project(),
        s.path.as_ref().unwrap(),
        &serde_json::from_value(scope()).unwrap(),
        p["token"].as_str().unwrap(),
        t.revision,
        t.sequence.map(|id| id.0),
        t.after_sequence.map(|id| id.0),
        p,
        r,
        None,
    )
}
fn paired(s: &mut Session, d: &Value, operation: Value) -> (Value, Value) {
    let p = args(d, operation);
    let mut human = twin(s);
    let (cmd, params) = match p["operation"]["type"].as_str().unwrap() {
        "open" => ("sequence.open", json!({"item":p["operation"]["item"]})),
        "close" => ("sequence.close", json!({"item":p["operation"]["item"]})),
        "close_others" => ("sequence.closeOthers", json!({"item":p["operation"]["item"]})),
        "move_tab" => ("sequence.moveTab", json!({"item":p["operation"]["item"],"index":p["operation"]["index"]})),
        _ => panic!("unsupported test operation"),
    };
    human.execute(cmd, params).unwrap();
    let r = s.execute(COMMAND, p.clone()).unwrap();
    assert_eq!(state(s), state(&human));
    assert!(Arc::ptr_eq(&s.project, &human.project));
    assert_eq!(r["agentUndoOwned"], false);
    assert_eq!(r["agentTimelineCommits"], 0);
    validate(s, &p, &r).unwrap();
    (p, r)
}
fn reject(s: &mut Session, p: Value) {
    let before = state(s);
    assert!(s.execute(COMMAND, p).is_err());
    assert_eq!(state(s), before);
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("native-view-history-{}-{}.incomplete", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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
fn root(s: &mut Session, n: usize) {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
}
fn save(s: &mut Session, n: usize) -> Value {
    filmcraft_format::atomic_write(Path::new(s.path.as_ref().unwrap()), &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false))
        .unwrap();
    s.saved_revision = s.revision;
    root(s, n);
    s.execute("agent.saveHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap()
}
fn cold(s: &Session) -> Session {
    let mut c = Session::default();
    c.execute("file.open", json!({"path":s.path})).unwrap();
    c.agent_operation_control().enable_transport();
    c
}
fn restore(s: &mut Session, n: usize, saved: &Value) -> Result<Value> {
    root(s, n);
    s.execute("agent.restoreHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}))
}
fn raw(d: &Scratch) -> Value {
    serde_json::from_slice(&std::fs::read(d.companion()).unwrap()).unwrap()
}
fn rehash(v: &mut Value) {
    let data = serde_json::to_vec(&v["data"].as_str().map(|x| serde_json::from_str::<Value>(x).unwrap()).unwrap()).unwrap();
    v["bytes"] = json!(data.len());
    v["sha256"] = json!(history_checkpoint::digest(&data));
    v["data"] = json!(String::from_utf8(data).unwrap());
}
#[test]
fn view_publication_read_and_change_purposes_stay_distinct_and_explicit() {
    let (mut s, ids) = setup();
    let d = s.execute(super::super::READ, json!({"token":token(220000),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(d["version"], 1);
    assert_eq!(d["capabilities"]["sequenceViewPublication"], false);
    reject(&mut s, args(&d, op("open", ids[1], true)));
    let d = prepare(&mut s, 220001);
    assert_eq!(d["version"], 2);
    assert_eq!(d["readPurpose"], "sequence_view_change_preparation");
    assert_eq!(d["capabilities"]["sequenceViewPublication"], true);
    paired(&mut s, &d, op("open", ids[1], true));
}
#[test]
fn view_publication_idle_four_operations_match_actual_human_state_journal_events_and_project_identity() {
    for kind in ["open", "close", "close_others", "move_tab"] {
        let (mut s, ids) = setup();
        let d = prepare(&mut s, 220010);
        paired(&mut s, &d, op(kind, ids[1], true));
    }
}
#[test]
fn view_publication_all_dynamic_kinds_offsets_and_operations_settle_exact_original_human_history() {
    for kind in ["trim", "ripple", "roll"] {
        for moved in [false, true] {
            for action in ["open", "close", "close_others", "move_tab"] {
                let (mut s, ids) = setup();
                dynamic(&mut s, kind, moved);
                let d = prepare(&mut s, 220020);
                let (_, r) = paired(&mut s, &d, op(action, if action == "close" { ids[0] } else { ids[1] }, true));
                assert_eq!(r["publication"]["trim"]["dynamicCommitted"], moved);
                assert!(!s.trim_play.active());
            }
        }
    }
}
#[test]
fn view_publication_advancing_original_clock_keeps_both_original_and_actual_publication_bases() {
    for kind in ["trim", "ripple", "roll"] {
        for action in ["open", "close", "close_others", "move_tab"] {
            let (mut s, ids) = setup();
            dynamic(&mut s, kind, false);
            let d = prepare(&mut s, 220030);
            s.execute("trim.tick", json!({"clock":0.5})).unwrap();
            let (_, r) = paired(&mut s, &d, op(action, ids[1], true));
            assert_eq!(r["original"], d);
            assert_ne!(r["current"]["projectState"]["sha256"], d["projectState"]["sha256"]);
            assert_eq!(r["publication"]["beforeProjectSha256"], r["current"]["projectState"]["sha256"]);
        }
    }
}
#[test]
fn view_publication_loop_only_operations_stop_original_loop_without_agent_or_human_timeline_commit() {
    for action in ["open", "close", "close_others", "move_tab"] {
        let (mut s, ids) = setup();
        around(&mut s);
        let d = prepare(&mut s, 220040);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let before = s.history.undo.len();
        let (_, r) = paired(&mut s, &d, op(action, ids[1], true));
        assert_eq!(s.history.undo.len(), before);
        assert_eq!(r["publication"]["trim"]["loopStopped"], true);
        assert_eq!(r["publication"]["trim"]["dynamicCommitted"], false);
    }
}
#[test]
fn view_publication_zero_one_and_full_history_limits_keep_original_redo_and_no_agent_undo() {
    for limit in [0, 1, 128] {
        for moved in [false, true] {
            let (mut s, ids) = setup();
            s.history.limit = limit;
            s.history.undo.push(("Earlier human step".into(), s.project.clone()));
            s.history.redo.push(("Earlier redo".into(), Arc::new((*s.project).clone())));
            dynamic(&mut s, "roll", moved);
            let d = prepare(&mut s, 220050);
            paired(&mut s, &d, op("open", ids[1], true));
        }
    }
}
#[test]
fn view_publication_close_all_and_open_from_nullable_view_keep_original_residual_editor_state() {
    let (mut s, ids) = setup();
    dynamic(&mut s, "roll", false);
    let d = prepare(&mut s, 220060);
    paired(&mut s, &d, op("close_others", ids[0], true));
    let d = prepare(&mut s, 220061);
    let (_, r) = paired(&mut s, &d, op("close", ids[0], true));
    assert_eq!(r["sequence"], Value::Null);
    assert!(s.state.active_sequence.is_none());
    let d = prepare(&mut s, 220062);
    paired(&mut s, &d, op("open", ids[1], true));
}
#[test]
fn view_publication_missing_tab_close_and_active_close_others_preserve_original_noop_and_selection() {
    let (mut s, ids) = setup();
    s.execute("sequence.close", json!({"item":ids[1].0})).unwrap();
    let d = prepare(&mut s, 220070);
    let (_, r) = paired(&mut s, &d, op("close", ids[1], true));
    assert_eq!(r["publication"]["changed"], false);
    let d = prepare(&mut s, 220071);
    paired(&mut s, &d, op("close_others", ids[0], true));
}
#[test]
fn view_publication_tab_index_end_clamp_and_foreign_root_views_remain_explicit_without_source_authority() {
    for action in ["open", "close", "close_others", "move_tab"] {
        let (mut s, ids) = setup();
        let d = prepare(&mut s, 220080);
        let (_, r) = paired(&mut s, &d, op(action, ids[2], false));
        assert_eq!(r["mediaEvidence"], false);
        assert_eq!(r["writeAuthority"], false);
        if action == "move_tab" {
            assert_eq!(r["publication"]["humanResult"]["index"], 2);
        }
    }
}
#[test]
fn view_publication_hostile_unknown_or_foreign_target_requests_never_partially_settle_trim() {
    for change in 0..11 {
        let (mut s, ids) = setup();
        dynamic(&mut s, "roll", true);
        let d = prepare(&mut s, 220090 + change);
        let mut p = args(&d, op("open", ids[1], true));
        match change {
            0 => p["operation"]["item"] = json!(0),
            1 => p["operation"]["item"] = json!(-1),
            2 => p["operation"]["item"] = json!(u64::MAX),
            3 => p["operation"]["owned"] = json!(false),
            4 => p["operation"]["type"] = json!("edit.delete"),
            5 => p["operation"]["index"] = json!(0),
            6 => p["beforeSha256"] = json!("0".repeat(64)),
            7 => p["beforeEditorSha256"] = json!("0".repeat(64)),
            8 => p["scope"]["projectId"] = json!("foreign"),
            9 => p["version"] = json!(2),
            _ => p["command"] = json!("sequence.open"),
        };
        reject(&mut s, p);
    }
}
#[test]
fn view_publication_foreign_editor_history_clipboard_and_clock_restarts_expire_without_effect() {
    for change in 0..8 {
        let (mut s, ids) = setup();
        dynamic(&mut s, "roll", false);
        let d = prepare(&mut s, 220110 + change);
        match change {
            0 => s.state.snapping = !s.state.snapping,
            1 => s.state.source_playhead = Tick(9_007_199_254_740_993),
            2 => s.history.limit = 1,
            3 => s.history.merge_key = Some("foreign".into()),
            4 => s.state.open_sequences.reverse(),
            5 => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
                s.state.clipboard.push((filmcraft_project::TrackKind::Video, 0, clip));
            }
            6 => {
                s.execute("trim.shuttleStop", json!({})).unwrap();
                s.execute("trim.shuttle", json!({"direction":"forward","clock":0.0})).unwrap();
            }
            _ => s.agent_bridge.epoch = Some("63041ea6-5c0c-48c9-a26c-ab4d194c0ca1".into()),
        };
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        reject(&mut s, args(&d, op("open", ids[1], true)));
    }
}
#[test]
fn view_publication_old_root_snapshot_and_editor_setting_purposes_cannot_acquire_view_writer() {
    for method in [project_plane::READ, editor_control::READ, editor_control::PREPARE] {
        let (mut s, ids) = setup();
        let d = s.execute(method, json!({"token":token(220130),"scope":scope(),"projectPath":s.path})).unwrap();
        reject(&mut s, args(&d, op("open", ids[1], true)));
    }
    let (mut s, ids) = setup();
    let d = s.execute("agent.snapshot", json!({"token":token(220131)})).unwrap();
    reject(
        &mut s,
        json!({"version":1,"token":d["token"],"scope":scope(),"beforeSha256":"0".repeat(64),"beforeEditorSha256":"0".repeat(64),"operation":op("open",ids[1],true)}),
    );
}
#[test]
fn view_publication_independent_stop_and_missing_transport_refuse_before_both_original_effects() {
    let (mut s, ids) = setup();
    dynamic(&mut s, "roll", true);
    let d = prepare(&mut s, 220140);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":d["token"]})).unwrap()["cancellationAccepted"], true);
    reject(&mut s, args(&d, op("open", ids[1], true)));
    let (mut s, ids) = setup();
    let d = prepare(&mut s, 220141);
    s.agent_bridge.operations = Default::default();
    reject(&mut s, args(&d, op("open", ids[1], true)));
}
#[test]
fn view_publication_lost_reply_reconciles_once_after_later_human_view_and_edit_without_replay() {
    let (mut s, ids) = setup();
    dynamic(&mut s, "roll", false);
    let d = prepare(&mut s, 220150);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, r) = paired(&mut s, &d, op("open", ids[1], true));
    s.execute("sequence.open", json!({"item":ids[0].0})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("clip.rename", json!({"clip":clip.0,"name":"Later human work"})).unwrap();
    let before = state(&s);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["token"]})).unwrap()["result"], r);
    assert_eq!(state(&s), before);
    let mut wrong = p;
    wrong["operation"] = op("close", ids[0], true);
    reject(&mut s, wrong);
}
#[test]
fn view_publication_native_tick_and_private_clipboard_proofs_roundtrip_exactly_and_reject_changed_editor() {
    let (mut s, ids) = setup();
    s.state.source_playhead = Tick(9_007_199_254_740_993);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    s.state.clipboard.push((filmcraft_project::TrackKind::Video, 0, clip));
    dynamic(&mut s, "roll", false);
    let d = prepare(&mut s, 220160);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (p, r) = paired(&mut s, &d, op("open", ids[1], true));
    let mut forged = r.clone();
    let mut data: Value = serde_json::from_str(forged["publication"]["editorProof"]["data"].as_str().unwrap()).unwrap();
    assert_eq!(data["beforeEditor"]["source_playhead"], json!(9_007_199_254_740_993_i64));
    data["beforeEditor"]["source_playhead"] = json!(9_007_199_254_740_992_i64);
    forged["publication"]["editorProof"]["data"] = json!(serde_json::to_string(&data).unwrap());
    rehash(&mut forged["publication"]["editorProof"]);
    assert!(validate(&s, &p, &forged).is_err());
}
#[test]
fn view_publication_equal_byte_distinct_history_maps_cannot_be_collapsed() {
    let (mut s, ids) = setup();
    s.history.redo.push(("Equal bytes distinct native snapshot".into(), Arc::new((*s.project).clone())));
    let d = prepare(&mut s, 220170);
    let (p, r) = paired(&mut s, &d, op("open", ids[1], true));
    let (pool, mut a, b) = pool(&s, &p);
    assert!(a.len() > 1);
    a[1] = a[0];
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
    let h = retained(t).unwrap().validate("sequence.open").unwrap();
    assert!(
        validate_history(
            &pool,
            &a,
            &b,
            h.before_project(),
            h.after_project(),
            s.path.as_ref().unwrap(),
            &serde_json::from_value(scope()).unwrap(),
            p["token"].as_str().unwrap(),
            t.revision,
            t.sequence.map(|x| x.0),
            t.after_sequence.map(|x| x.0),
            &p,
            &r,
            None
        )
        .is_err()
    );
}
#[test]
fn view_publication_historical_headers_target_scope_and_publication_changes_refuse_even_when_rehashed() {
    let (mut s, ids) = setup();
    dynamic(&mut s, "roll", false);
    let d = prepare(&mut s, 220180);
    s.execute("trim.tick", json!({"clock":0.25})).unwrap();
    let (p, r) = paired(&mut s, &d, op("open", ids[1], true));
    for change in 0..7 {
        let mut bad = r.clone();
        match change {
            0 => bad["sequence"] = json!(ids[2].0),
            1 => bad["publication"]["operation"]["owned"] = json!(false),
            2 => bad["current"]["sequenceScope"]["projectId"] = json!("foreign"),
            3 => bad["original"]["capabilities"]["sequenceViewPublication"] = json!(false),
            4 => bad["originalHistoryProof"]["kind"] = json!("native_editor_control_history_proof"),
            5 => {
                let mut data: Value = serde_json::from_str(bad["originalBasis"]["data"].as_str().unwrap()).unwrap();
                data["version"] = json!(2);
                bad["originalBasis"]["data"] = json!(serde_json::to_string(&data).unwrap());
                rehash(&mut bad["originalBasis"]);
            }
            _ => bad["publication"]["agentUndoOwned"] = json!(true),
        };
        assert!(validate(&s, &p, &bad).is_err(), "change {change}");
    }
}
#[test]
fn view_publication_capsule_eight_cold_restores_all_operations_and_human_trim_limits_without_live_authority() {
    for kind in ["trim", "ripple", "roll"] {
        for limit in [0, 128] {
            for action in ["open", "close", "close_others", "move_tab"] {
                let dir = Scratch::new();
                let (mut s, ids) = setup();
                s.path = Some(dir.file());
                s.history.limit = limit;
                dynamic(&mut s, kind, false);
                let d = prepare(&mut s, 220190);
                s.execute("trim.tick", json!({"clock":0.25})).unwrap();
                let (p, r) = paired(&mut s, &d, op(action, ids[1], true));
                let saved = save(&mut s, 220191);
                assert_eq!(raw(&dir)["version"], 8);
                let mut c = cold(&s);
                let project = c.project.clone();
                let editor = project_clear::editor_hash(&c.state).unwrap();
                restore(&mut c, 220192, &saved).unwrap();
                assert!(Arc::ptr_eq(&c.project, &project));
                assert_eq!(project_clear::editor_hash(&c.state).unwrap(), editor);
                assert_eq!(c.history.limit, limit);
                let t = c.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
                let o = t.sequence_views.as_ref().unwrap();
                assert!(o.following.is_none() && o.original_settlement.is_none() && o.original_editor.is_none());
                assert!(o.outcome.as_ref().unwrap().original.is_some());
                let before = state(&c);
                assert_eq!(c.execute(COMMAND, p.clone()).unwrap(), r);
                assert_eq!(state(&c), before);
            }
        }
    }
}
#[test]
fn view_publication_capsule_eight_retains_nullable_view_history_without_overwriting_fresh_editor() {
    for action in ["close", "close_others", "move_tab"] {
        let dir = Scratch::new();
        let (mut s, ids) = setup();
        s.path = Some(dir.file());
        s.state.open_sequences = vec![ids[0]];
        around(&mut s);
        let d = prepare(&mut s, 220210);
        let (p, r) = paired(&mut s, &d, op(action, ids[0], true));
        let saved = save(&mut s, 220211);
        let mut c = cold(&s);
        let before = project_clear::editor_hash(&c.state).unwrap();
        restore(&mut c, 220212, &saved).unwrap();
        assert!(!c.trim_play.active());
        assert_eq!(project_clear::editor_hash(&c.state).unwrap(), before);
        assert_eq!(c.execute(COMMAND, p.clone()).unwrap(), r);
        if action == "close" {
            assert_eq!(r["sequence"], Value::Null);
            let ticket = c.agent_bridge.tickets.iter().find(|t| t.token == p["token"].as_str().unwrap()).unwrap();
            assert!(ticket.after_sequence.is_none());
        }
    }
}
#[test]
fn view_publication_capsule_eight_mixes_old_idle_view_and_strict_and_following_control_receipts() {
    let dir = Scratch::new();
    let (mut s, ids) = setup();
    s.path = Some(dir.file());
    root(&mut s, 220220);
    let a = crate::editorial_workspace::association(&s.project).unwrap().unwrap();
    let rootdoc = s.execute(project_plane::READ, json!({"token":token(220221),"scope":scope(),"projectPath":s.path})).unwrap();
    s.execute(project_navigation::COMMAND,json!({"token":rootdoc["token"],"scope":scope(),"sequenceScope":a.scope,"item":ids[0].0,"beforeSha256":rootdoc["projectState"]["sha256"],"beforeEditorSha256":rootdoc["editorStateSha256"]})).unwrap();
    for follow in [false, true] {
        let mut q = json!({"token":token(220222+usize::from(follow)),"scope":scope(),"projectPath":s.path});
        if follow {
            q["clockPolicy"] = json!("original_human_trim");
        }
        let d = s.execute(editor_control::PREPARE, q).unwrap();
        s.execute(editor_control::CONFIGURE,json!({"version":if follow{3}else{2},"token":d["token"],"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":{"kind":"default_transition","definition":"constant_gain"}})).unwrap();
    }
    let d = prepare(&mut s, 220224);
    let (p, r) = paired(&mut s, &d, op("open", ids[1], true));
    let saved = save(&mut s, 220225);
    let v = raw(&dir);
    assert_eq!(v["version"], 8);
    assert!(v["tickets"].as_array().unwrap().iter().any(|t| t["editor_control"].is_object()));
    let mut c = cold(&s);
    restore(&mut c, 220226, &saved).unwrap();
    assert_eq!(c.execute(COMMAND, p).unwrap(), r);
}
#[test]
fn view_publication_capsule_eight_rehashed_downgrade_missing_maps_and_hidden_effects_refuse_atomically() {
    for change in 0..6 {
        let dir = Scratch::new();
        let (mut s, ids) = setup();
        s.path = Some(dir.file());
        dynamic(&mut s, "roll", false);
        let d = prepare(&mut s, 220230);
        s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        let (p, _) = paired(&mut s, &d, op("open", ids[1], true));
        let mut saved = save(&mut s, 220231);
        let mut v = raw(&dir);
        if change == 0 {
            v["version"] = json!(7);
        } else {
            let t = v["tickets"].as_array_mut().unwrap().iter_mut().find(|t| t["token"] == p["token"]).unwrap();
            match change {
                1 => {
                    t.as_object_mut().unwrap().remove("sequence_view");
                }
                2 => t["sequence_view"]["original_snapshots"] = json!([]),
                3 => t["sequence_view"]["version"] = json!(2),
                4 => t["result"]["publication"]["agentUndoOwned"] = json!(true),
                _ => t["result"]["sequence"] = json!(ids[2].0),
            }
        }
        let bytes = serde_json::to_vec_pretty(&v).unwrap();
        std::fs::write(dir.companion(), &bytes).unwrap();
        saved["checkpoint"]["sha256"] = json!(history_checkpoint::digest(&bytes));
        let mut c = cold(&s);
        let before = state(&c);
        assert!(restore(&mut c, 220232, &saved).is_err(), "change {change}");
        assert_eq!(state(&c), before);
    }
}
#[test]
fn view_publication_view_receipt_never_owns_a_human_trim_undo_or_promotes_restored_read_to_write() {
    let (mut s, ids) = setup();
    dynamic(&mut s, "roll", false);
    let d = prepare(&mut s, 220250);
    s.execute("trim.tick", json!({"clock":0.5})).unwrap();
    let (p, _) = paired(&mut s, &d, op("open", ids[1], true));
    root(&mut s, 220251);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(220251),"scope":scope(),"commit":p["token"]})).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", json!({"token":p["token"],"scope":scope()})).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn view_publication_four_operations_preserve_original_synthetic_recording_and_live_mixer_preview_owners() {
    for recording in [false, true] {
        for action in ["open", "close", "close_others", "move_tab"] {
            let (mut s, ids) = setup();
            s.set_playhead(Tick::ZERO);
            if recording {
                s.execute("mixer.recordStart", json!({"time":0})).unwrap();
            } else {
                let track = s.active_sequence().unwrap().audio_tracks[0].id;
                s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":0})).unwrap();
            }
            let live = s.previews.live.clone();
            let recording_before = format!("{:?}", s.mixrec);
            let multicam_before = format!("{:?}", s.mcrec);
            let d = prepare(&mut s, 220270);
            let (_, r) = paired(&mut s, &d, op(action, ids[1], true));
            assert_eq!(format!("{:?}", s.mixrec), recording_before);
            assert_eq!(format!("{:?}", s.mcrec), multicam_before);
            assert!(Arc::ptr_eq(&live, &s.previews.live));
            if recording {
                assert!(s.mixrec.active());
            } else {
                assert!(s.previews.live.is_active());
            }
            assert_eq!(r["mediaEvidence"], false);
            assert_eq!(r["publication"]["trim"]["dynamicCommitted"], false);
        }
    }
}
