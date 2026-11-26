use super::*;
use serde_json::json;
use std::sync::Arc;

pub(super) fn setup() -> Session {
    let mut s = super::tests::setup();
    s.path = Some("/fixture/workspace-update.fcproj".into());
    s.agent_operation_control().enable_transport();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item.0;
    s.execute("agent.bindSource", json!({"item":item,"binding":{"projectId":"p_update","assetId":"a_update","mediaId":"original","role":"original"}})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn request(s: &mut Session, n: usize) -> Value {
    let token = super::tests::token(n);
    let scope = json!({"projectId":"p_update","anchorSequence":s.state.active_sequence.unwrap().0});
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
    json!({"token":token,"scope":scope,"workspace":"11111111-1111-4111-8111-111111111111","update":format!("22222222-2222-4222-8222-{n:012x}"),"nativeSession":s.agent_bridge.epoch})
}
#[test]
fn workspace_update_pause_preserves_dirty_native_state_and_blocks_human_agent_and_direct_history_writes() {
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let audio = q.audio_tracks[0].items[0].id.0;
    s.execute("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-7})).unwrap();
    let r = request(&mut s, 90_000);
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let history = (s.history.undo.len(), s.history.redo.len(), s.history.merge_key.clone());
    let dirty = s.is_dirty();
    let paused = s.execute("agent.beginWorkspaceUpdate", r.clone()).unwrap();
    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["dirty"], dirty);
    assert_eq!(paused["sourceValidation"], "not-performed");
    assert_eq!(paused["writeAuthority"], false);
    assert!(s.is_display_only());
    for (command, params) in [
        ("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-12})),
        ("edit.undo", json!({})),
        ("agent.snapshot", json!({"token":super::tests::token(90_001)})),
        ("file.save", json!({})),
        ("file.openDemoProject", json!({})),
        ("playhead.set", json!({"frame":2})),
    ] {
        assert!(s.execute(command, params).is_err(), "{command}");
    }
    assert!(s.edit("Not a second writer", |_, _| Ok(())).is_err());
    assert!(s.undo().is_none());
    assert!(s.redo().is_none());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert_eq!((s.history.undo.len(), s.history.redo.len(), s.history.merge_key.clone()), history);
    assert_eq!(s.execute("agent.beginWorkspaceUpdate", r.clone()).unwrap(), paused);
    let mut address = r.clone();
    address.as_object_mut().unwrap().remove("token");
    address.as_object_mut().unwrap().remove("nativeSession");
    assert_eq!(s.execute("agent.workspaceUpdateStatus", address).unwrap(), paused);
    assert_eq!(s.execute("agent.cancelWorkspaceUpdate", r.clone()).unwrap()["status"], "cancelled");
    assert!(!s.is_display_only());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.execute("agent.beginWorkspaceUpdate", r).unwrap()["status"], "cancelled");
    s.execute("edit.undo", json!({})).unwrap();
    assert_ne!(s.project, project);
}
#[test]
fn workspace_update_foreign_stale_and_cancelled_bases_cannot_pause_or_release_another_owner() {
    let mut s = setup();
    let r = request(&mut s, 91_000);
    for key in ["workspace", "update", "token"] {
        let mut bad = r.clone();
        bad[key] = json!("invalid");
        assert!(s.execute("agent.beginWorkspaceUpdate", bad).is_err());
        assert!(!s.is_display_only());
    }
    s.execute("playhead.set", json!({"frame":2})).unwrap();
    assert!(s.execute("agent.beginWorkspaceUpdate", r).is_err());
    let r = request(&mut s, 91_010);
    s.execute("agent.cancelWorkspaceUpdate", r.clone()).unwrap();
    assert_eq!(s.execute("agent.beginWorkspaceUpdate", r).unwrap()["status"], "cancelled");
    assert!(!s.is_display_only());
    let r = request(&mut s, 91_020);
    s.execute("agent.beginWorkspaceUpdate", r.clone()).unwrap();
    for key in ["workspace", "update", "token"] {
        let mut foreign = r.clone();
        foreign[key] = json!("33333333-3333-4333-8333-333333333333");
        assert!(s.execute("agent.cancelWorkspaceUpdate", foreign).is_err());
        assert!(s.is_display_only());
    }
    assert_eq!(s.cancel_workspace_update().unwrap().unwrap()["status"], "cancelled");
    assert!(!s.is_display_only());
}
#[test]
fn workspace_update_cancel_survives_metadata_lru_turnover_without_restoring_or_renewing_source_authority() {
    let mut s = setup();
    let r = request(&mut s, 92_000);
    s.execute("agent.beginWorkspaceUpdate", r.clone()).unwrap();
    for n in 92_001..92_025 {
        let scope = r["scope"].clone();
        s.execute("agent.readHistoryCheckpointBasis", json!({"token":super::tests::token(n),"scope":scope})).unwrap();
    }
    assert!(!history_checkpoint::has_basis(&s, r["token"].as_str().unwrap()));
    assert!(s.execute("agent.apply", json!({"token":r["token"],"label":"Not an edit ticket","operations":[]})).is_err());
    assert_eq!(s.execute("agent.cancelWorkspaceUpdate", r).unwrap()["status"], "cancelled");
    assert!(!s.is_display_only());
}
