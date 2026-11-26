use super::tests::{exact_batch, read, setup, token};
use crate::Session;
use filmcraft_project::ItemId;
use filmcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};
use std::sync::Arc;

fn fixture() -> (Session, ItemId) {
    let mut s = setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    s.execute(
        "agent.bindSource",
        json!({"item":item.0,"binding":{"projectId":"p_test","assetId":"a_preview","mediaId":"a_preview:original","role":"original"}}),
    )
    .unwrap();
    (s, item)
}
fn op(item: ItemId) -> Value {
    json!({"command":"agent.previewSource","params":{"item":item.0,"projectId":"p_test","sourceInFrame":2,"durationFrames":4}})
}

#[test]
fn preview_changes_only_source_marks_opens_source_and_undo_redo_restore_project() {
    let (mut s, item) = fixture();
    let before = s.project.clone();
    let sequence = s.active_sequence().unwrap().clone();
    read(&mut s, 1);
    let result = s.execute("agent.apply", exact_batch(1, vec![op(item)])).unwrap();
    assert_eq!(result["action"], "source_preview");
    assert_eq!(s.active_sequence().unwrap(), &sequence);
    assert_eq!(s.state.source_item, Some(item));
    let marked = s.project.clone();
    assert!(s.events.iter().any(|e| matches!(e,crate::Event::OpenSource(id) if *id==item)));
    let inspect = s.execute("source.inspect", json!({})).unwrap();
    assert_eq!(inspect["markIn"], result["results"][0]["markIn"]);
    assert_eq!(inspect["markOut"], result["results"][0]["markOut"]);
    assert_eq!(inspect["playhead"], inspect["markIn"]);
    let revision = s.revision;
    assert_eq!(s.execute("agent.apply", exact_batch(1, vec![op(item)])).unwrap(), result);
    assert_eq!(s.revision, revision);
    read(&mut s, 2);
    s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, marked);
}

#[test]
fn preview_hfr_marks_use_contained_camera_frames_and_inclusive_out() {
    let (mut s, item) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    p.sequence_mut(seq).unwrap().settings.frame_rate = FrameRate { num: 24000, den: 1001 };
    p.item_mut(item).unwrap().as_media_mut().unwrap().info.video.as_mut().unwrap().frame_rate = FrameRate { num: 60000, den: 1001 };
    read(&mut s, 1);
    let result = s.execute("agent.apply", exact_batch(1, vec![op(item)])).unwrap();
    let fd = FrameRate { num: 60000, den: 1001 }.frame_duration().0;
    // Requested interval [2,6) at 23.976 = [5,15) at 59.94, Out is camera frame 14.
    assert_eq!(result["results"][0]["markIn"], 5 * fd);
    assert_eq!(result["results"][0]["markOut"], 14 * fd);
    assert_eq!(result["results"][0]["exclusiveEnd"], 15 * fd);
    let mut second = op(item);
    second["params"]["sourceInFrame"] = json!(1);
    second["params"]["durationFrames"] = json!(3);
    read(&mut s, 2);
    let result = s.execute("agent.apply", exact_batch(2, vec![second])).unwrap();
    assert_eq!(result["results"][0]["markIn"], 3 * fd);
    assert_eq!(result["results"][0]["markOut"], 9 * fd);
}

#[test]
fn preview_rejects_hostile_shapes_and_ranges_without_live_changes() {
    let (mut s, item) = fixture();
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    for (n, (key, value)) in [
        ("sourceInFrame", json!(-1)),
        ("sourceInFrame", json!(1.5)),
        ("durationFrames", json!(0)),
        ("durationFrames", json!(1000000)),
        ("projectId", json!("foreign")),
        ("item", json!(0)),
        ("path", json!("/not-a-tool")),
    ]
    .into_iter()
    .enumerate()
    {
        let mut operation = op(item);
        operation["params"][key] = value;
        read(&mut s, n);
        let revision = s.revision;
        assert!(s.execute("agent.apply", exact_batch(n, vec![operation])).is_err());
        assert_eq!(s.revision, revision);
        assert_eq!(s.project, project);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    }
    read(&mut s, 20);
    let mut batch = exact_batch(20, vec![op(item)]);
    batch["exact"] = json!(false);
    assert!(s.execute("agent.apply", batch).is_err());
    read(&mut s, 21);
    assert!(s.execute("agent.apply", exact_batch(21, vec![op(item), op(item)])).is_err());
}

#[test]
fn preview_refuses_stale_offline_interpreted_and_protected_sources() {
    for mode in 0..5 {
        let (mut s, item) = fixture();
        if mode == 1 {
            Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().offline = true;
        }
        if mode == 2 {
            Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().interpret.frame_rate = Some(FrameRate { num: 12, den: 1 });
        }
        if mode == 3 {
            let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
            s.execute("poema.protectClips", json!({"clips":[clip.0],"reason":"Editor choice"})).unwrap();
        }
        if mode == 4 {
            Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.duration = Tick(1);
        }
        read(&mut s, 1);
        if mode == 0 {
            s.state.source_playhead = Tick(123);
        }
        let project = s.project.clone();
        let state = serde_json::to_value(&s.state).unwrap();
        let rev = s.revision;
        assert!(s.execute("agent.apply", exact_batch(1, vec![op(item)])).is_err());
        assert_eq!(s.project, project);
        assert_eq!(s.revision, rev);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    }
}

#[test]
fn preview_recovery_preserves_source_marks_and_one_step_undo() {
    let (mut s, item) = fixture();
    let before = s.project.clone();
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![op(item)])).unwrap();
    let replay = s.journal.last().unwrap().clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute(&replay.0, replay.1).unwrap();
    assert_ne!(s.project, before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
}
