//! Real original Session/trim/publication graph checks; generated Root only.
use super::*;
use crate::agent_bridge::application_tab_scope::ApplicationTabReceipt;
use crate::{History, Session};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"native-application-history-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn setup(limit: usize) -> (Root, Session, filmcraft_project::ItemId, filmcraft_project::ItemId) {
    let root = std::env::temp_dir().join(format!("native-application-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("generated-root.txt"), b"native-application-history-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let main = s.state.active_sequence.unwrap();
    s.execute(
        "project.associateEditorialOwner",
        json!({"scope":{"projectId":"p_application_tabs","anchorSequence":main.0},"workspaceId":"6cd868fc-9e1c-485b-87aa-9c60e13b00bf"}),
    )
    .unwrap();
    Arc::make_mut(&mut s.project).metadata.insert(
        "poema.projectWorkspace.v1".into(),
        serde_json::to_string(&json!({"kind":"poema-native-project-workspace","version":1,"scope":scope()})).unwrap(),
    );
    s.execute("file.newSequence", json!({"name":"Generated archive other tab","video":0,"audio":0})).unwrap();
    let other = s.state.active_sequence.unwrap();
    s.execute("sequence.open", json!({"item":main.0})).unwrap();
    for track in ["V2", "V3", "A2", "A3"] {
        s.execute("timeline.setTrack", json!({"track":track,"syncLock":false})).unwrap();
    }
    s.execute("file.save", json!({"path":root.join("generated.fcproj")})).unwrap();
    s.history = History { limit, ..History::default() };
    s.agent_operation_control().enable_transport();
    s.events.clear();
    (Root(root), s, main, other)
}
fn scope() -> Value {
    json!({"projectId":"p_application_tabs","workspaceId":"6cd868fc-9e1c-485b-87aa-9c60e13b00bf"})
}
fn prepare(s: &mut Session, n: u64) -> crate::ApplicationTabScope {
    let path = s.path.clone();
    s.prepare_application_tab_scope(&json!({"token":format!("{n:064x}"),"scope":scope(),"projectPath":path})).unwrap()
}
fn native_state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"limit":s.history.limit,"path":s.path,"revision":s.revision,"saved":s.saved_revision,"journal":s.journal,"trim":format!("{:?}",s.trim_play)})
}
fn publish(s: &mut Session, scope: crate::ApplicationTabScope, item: filmcraft_project::ItemId) -> ApplicationTabReceipt {
    let switched = s.state.active_sequence != Some(item);
    let action = scope.prepare_activation(s, item.0).unwrap();
    let p = action.enter_publication(s).unwrap();
    if switched {
        s.execute("sequence.open", json!({"item":item.0})).unwrap();
    }
    let receipt = p.finish(s);
    assert!(receipt.confirmed(), "{}", receipt.document());
    receipt
}
fn cold_pool(history: &ApplicationTabHistory) -> Vec<Arc<Project>> {
    history
        .projects()
        .iter()
        .map(|p| {
            let raw = project_bytes(p, MAX_BYTES).unwrap();
            Arc::new(super::super::project(std::str::from_utf8(&raw).unwrap()).unwrap())
        })
        .collect()
}
fn roundtrip(receipt: &ApplicationTabReceipt) -> ApplicationTabHistory {
    let h = receipt.capture_history().unwrap();
    let proof = h.proof().unwrap();
    let pool = cold_pool(&h);
    let mapping: Vec<_> = (0..pool.len()).collect();
    let cold = ApplicationTabHistory::from_proof(&proof, &pool, &mapping).unwrap();
    assert_eq!(cold.proof().unwrap(), proof);
    assert_eq!(cold.result(), receipt.document());
    for (a, b) in h.projects().iter().zip(cold.projects()) {
        assert!(!Arc::ptr_eq(a, b));
    }
    cold
}
fn start(s: &mut Session, kind: &str, moved: bool) {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":if kind=="loop"{"roll"}else{kind}})).unwrap();
    if kind == "loop" {
        s.execute("trim.playAround", json!({"clock":0})).unwrap();
    } else {
        s.execute("trim.shuttle", json!({"direction":if kind=="roll"{"forward"}else{"reverse"},"clock":0})).unwrap();
        if moved {
            s.execute("trim.tick", json!({"clock":0.25})).unwrap();
        }
    }
}
#[test]
fn same_active_history_roundtrip_is_readonly_and_cannot_restore_a_live_scope() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 1);
    let receipt = publish(&mut s, scope, main);
    let before = native_state(&s);
    let cold = roundtrip(&receipt);
    assert_eq!(native_state(&s), before);
    assert_eq!(cold.original_read()["capabilities"]["sequenceViewPublication"], false);
    assert_eq!(cold.result()["before"], cold.result()["after"]);
    assert!(cold.actual_snapshot_complete());
}
#[test]
fn all_original_trim_clocks_and_loop_preserve_zero_history_roots_for_focus_and_switch() {
    for limit in [0, 128] {
        for (kind, moved) in [("roll", false), ("roll", true), ("ripple", false), ("ripple", true), ("trim", false), ("trim", true), ("loop", false)] {
            for switched in [false, true] {
                let (_root, mut s, main, other) = setup(limit);
                start(&mut s, kind, moved);
                let scope = prepare(&mut s, 2);
                s.execute("trim.tick", json!({"clock":0.5})).unwrap();
                let receipt = publish(&mut s, scope, if switched { other } else { main });
                let before = native_state(&s);
                let h = roundtrip(&receipt);
                assert_eq!(native_state(&s), before);
                assert_eq!(s.trim_play.active(), !switched);
                assert_eq!(h.result()["switched"], switched);
                assert_eq!(h.saved.before.limit, limit);
                assert!(h.saved.origin.forecast.is_some());
                if limit == 0 {
                    assert!(h.saved.before.undo.is_empty());
                    assert!(h.projects().len() >= 1);
                }
            }
        }
    }
}
#[test]
fn uncertain_actual_effect_retains_complete_distinct_actual_graph_without_rollback() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 3);
    let p = scope.prepare_activation(&mut s, main.0).unwrap().enter_publication(&mut s).unwrap();
    s.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    let receipt = p.finish(&s);
    assert!(!receipt.confirmed());
    let before = native_state(&s);
    let h = roundtrip(&receipt);
    assert_eq!(h.result()["status"], "unknown");
    assert!(h.actual_snapshot_complete());
    let actual = h.saved.actual.as_ref().unwrap();
    assert_ne!(actual.project, h.saved.after.project);
    assert_eq!(native_state(&s), before);
}
#[test]
fn distinct_equal_byte_cow_roots_are_not_collapsed_by_pool_mapping() {
    let (_root, mut s, main, _) = setup(128);
    let equal = Arc::new((*s.project).clone());
    assert!(!Arc::ptr_eq(&equal, &s.project));
    assert_eq!(project_bytes(&equal, MAX_BYTES).unwrap(), project_bytes(&s.project, MAX_BYTES).unwrap());
    s.history.undo.push(("Earlier distinct equal-byte root".into(), equal));
    let scope = prepare(&mut s, 4);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    assert_eq!(h.projects().len(), 2);
    let p = h.proof().unwrap();
    let cold = cold_pool(&h);
    assert!(ApplicationTabHistory::from_proof(&p, &cold, &[0, 0]).is_err());
    let collapsed = vec![cold[0].clone(), cold[0].clone()];
    assert!(ApplicationTabHistory::from_proof(&p, &collapsed, &[0, 1]).is_err());
    assert!(ApplicationTabHistory::from_proof(&p, &cold, &[0, 1]).is_ok());
}
fn modified(proof: &Value, change: impl FnOnce(&mut Value)) -> Value {
    let mut value: Value = serde_json::from_str(proof["data"].as_str().unwrap()).unwrap();
    change(&mut value);
    let payload: Payload = serde_json::from_value(value).unwrap();
    let raw = String::from_utf8(bytes(&payload, MAX_BYTES).unwrap()).unwrap();
    let mut p = proof.clone();
    p["data"] = json!(raw);
    p["bytes"] = json!(raw.len());
    p["sha256"] = json!(digest(raw.as_bytes()));
    p
}
#[test]
fn rehashed_tampering_of_native_editor_history_result_and_original_scope_is_rejected() {
    let (_root, mut s, _, other) = setup(128);
    let scope = prepare(&mut s, 5);
    let h = publish(&mut s, scope, other).capture_history().unwrap();
    let p = h.proof().unwrap();
    let pool = cold_pool(&h);
    let map: Vec<_> = (0..pool.len()).collect();
    for field in ["original", "before", "after", "result", "forecast", "missing_root"] {
        let bad = modified(&p, |v| match field {
            "original" => v["record"]["original"]["scope"]["projectId"] = json!("p_foreign"),
            "before" => v["record"]["before"]["clipboard"] = json!("f".repeat(64)),
            "after" => v["record"]["after"]["revision"] = json!(999),
            "result" => v["record"]["result"]["agentUndoOwned"] = json!(true),
            "forecast" => v["record"]["before"]["forecast"]["snapshots"] = json!([999]),
            _ => v["record"]["origin"]["project"] = json!(999),
        });
        assert!(ApplicationTabHistory::from_proof(&bad, &pool, &map).is_err(), "{field}");
    }
}
#[test]
fn foreign_truncated_extra_and_oversized_pool_proofs_fail_without_native_effects() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 6);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    let p = h.proof().unwrap();
    let pool = cold_pool(&h);
    let map: Vec<_> = (0..pool.len()).collect();
    let before = native_state(&s);
    for field in ["kind", "version", "sha256", "bytes", "data", "extra"] {
        let mut bad = p.clone();
        bad[field] = match field {
            "version" | "bytes" => json!(0),
            "data" => json!("{"),
            "extra" => json!(true),
            _ => json!("foreign"),
        };
        assert!(ApplicationTabHistory::from_proof(&bad, &pool, &map).is_err());
    }
    let mut too_many = pool.clone();
    while too_many.len() <= MAX_STEPS + 1 {
        too_many.push(Arc::new((*pool[0]).clone()));
    }
    assert!(ApplicationTabHistory::from_proof(&p, &too_many, &map).is_err());
    assert!(ApplicationTabHistory::from_proof(&p, &pool, &[]).is_err());
    assert_eq!(native_state(&s), before);
}
#[test]
fn extra_unreferenced_snapshot_or_foreign_project_hash_is_never_accepted() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 7);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    let p = h.proof().unwrap();
    let mut pool = cold_pool(&h);
    let map: Vec<_> = (0..pool.len()).collect();
    let mut foreign = (*pool[0]).clone();
    foreign.name.push_str(" foreign");
    pool[0] = Arc::new(foreign);
    assert!(ApplicationTabHistory::from_proof(&p, &pool, &map).is_err());
    let mut same = cold_pool(&h);
    same.push(Arc::new((*same[0]).clone()));
    let mut bad = h.saved.clone();
    bad.confirmed = true;
    let extra = ApplicationTabHistory { saved: bad, projects: same };
    assert!(extra.proof().is_err());
}
#[test]
fn unavailable_actual_snapshot_remains_explicitly_incomplete_and_never_fabricates_recovery() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 8);
    let p = scope.prepare_activation(&mut s, main.0).unwrap().enter_publication(&mut s).unwrap();
    s.history.limit = 1001;
    let receipt = p.finish(&s);
    assert!(!receipt.confirmed());
    assert_eq!(receipt.document()["actualSnapshotRetained"], false);
    assert!(receipt.document()["actualSnapshotError"].is_string());
    let h = roundtrip(&receipt);
    assert!(!h.actual_snapshot_complete());
    assert_eq!(s.history.limit, 1001);
}

#[test]
fn outer_proof_is_borrowed_and_bounded_before_payload_decode_without_live_effects() {
    let (_root, mut s, main, _) = setup(128);
    let scope = prepare(&mut s, 9);
    let h = publish(&mut s, scope, main).capture_history().unwrap();
    let p = h.proof().unwrap();
    let pool = cold_pool(&h);
    let map: Vec<_> = (0..pool.len()).collect();
    let before = native_state(&s);
    let borrowed = proof_data(&p).unwrap();
    assert_eq!(borrowed.as_ptr(), p["data"].as_str().unwrap().as_ptr());
    assert!(ApplicationTabHistory::from_proof(&p, &pool, &map).is_ok());
    for field in ["data", "kind", "sha256", "extra", "bytes", "version", "missing"] {
        let mut bad = p.clone();
        match field {
            "data" => bad["data"] = json!("x".repeat(MAX_BYTES + 1)),
            "kind" | "sha256" => bad[field] = json!("x".repeat(1024 * 1024)),
            "extra" => bad["extra"] = json!({"nested":[{"unrecognized":"x".repeat(1024 * 1024)}]}),
            "bytes" => bad["bytes"] = json!(u64::MAX),
            "version" => bad["version"] = json!(1.0),
            _ => {
                bad.as_object_mut().unwrap().remove("sha256");
            }
        }
        assert!(proof_data(&bad).is_err(), "{field}");
        assert!(ApplicationTabHistory::from_proof(&bad, &pool, &map).is_err(), "{field}");
        assert_eq!(native_state(&s), before, "{field}");
    }
}

mod native_editor;
