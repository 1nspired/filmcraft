use serde_json::json;

use crate::Session;
use filmcraft_project::ClipId;
use filmcraft_time::{TICKS_PER_SECOND, Tick};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn audit_first_caption_rejects_wrong_tracks_without_leaving_a_track_or_history() {
    for track in [json!("C999"), json!("C0"), json!(999999), json!(-1), json!(0.5), json!(null), json!(true), json!([]), json!({})] {
        let mut s = demo();
        let seq = s.state.active_sequence.unwrap();
        std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.clear();
        s.history.merge_key = Some("existing human gesture".into());
        let before = s.project.clone();
        let state = serde_json::to_value(&s.state).unwrap();
        let revision = s.revision;
        let undo = s.history.undo.len();
        let redo = s.history.redo.len();
        let journal = s.journal.clone();
        assert!(s.execute("captions.add", json!({"track":track,"text":"Must not create a track"})).is_err(), "accepted {track}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.project), "published a project for {track}");
        assert_eq!(s.active_sequence().unwrap().caption_tracks.len(), 0);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
        assert_eq!(s.history.redo.len(), redo);
        assert_eq!(s.history.merge_key.as_deref(), Some("existing human gesture"));
        assert_eq!(s.journal, journal);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    }
}

#[test]
fn audit_first_caption_and_implicit_track_are_one_native_undo_redo_step() {
    for explicit in [false, true] {
        let mut s = demo();
        let seq = s.state.active_sequence.unwrap();
        std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.clear();
        let before = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.len();
        let journal = s.journal.len();
        let mut args = json!({"text":"First caption — שלום 日本語","seconds":1.0,"durationSeconds":2.0});
        if explicit {
            args["track"] = json!("C1");
        }
        let id = ClipId(s.execute("captions.add", args).unwrap()["caption"].as_u64().unwrap());
        assert_eq!(s.revision, revision + 1);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(s.journal.len(), journal + 1, "no separately journaled implicit newTrack");
        assert_eq!(s.active_sequence().unwrap().caption_tracks.len(), 1);
        let added = s.active_sequence().unwrap().find_caption(id).unwrap().1;
        assert_eq!(added.text, "First caption — שלום 日本語");
        assert_eq!(s.state.caption_selection, vec![id]);
        s.active_sequence().unwrap().check().unwrap();
        let after = s.project.clone();
        s.undo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*before).unwrap());
        s.redo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*after).unwrap());
        let restored: filmcraft_project::Project = serde_json::from_slice(&serde_json::to_vec(&*s.project).unwrap()).unwrap();
        assert_eq!(restored.sequence(seq).unwrap().find_caption(id).unwrap().1.text, "First caption — שלום 日本語");
    }
}

#[test]
fn audit_caption_creation_at_exhausted_ids_refuses_without_publishing() {
    for next in [u64::MAX, u64::MAX - 1] {
        let mut s = demo();
        let seq = s.state.active_sequence.unwrap();
        std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.clear();
        std::sync::Arc::make_mut(&mut s.project).next_id = next;
        let project = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.len();
        assert!(s.execute("captions.add", json!({"text":"No ID space"})).is_err());
        assert!(std::sync::Arc::ptr_eq(&project, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn audit_caption_existing_track_locks_overlap_and_saved_reopen() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.clear();
    let first =
        ClipId(s.execute("captions.add", json!({"text":"Preserve this first cue","seconds":1,"durationSeconds":2})).unwrap()["caption"].as_u64().unwrap());
    let track = s.active_sequence().unwrap().caption_tracks[0].id;
    for selector in [json!("C1"), json!(track.0)] {
        let project = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.len();
        assert!(s.execute("captions.add", json!({"track":selector,"text":"Overlapping cue","seconds":1.5})).is_err());
        assert!(std::sync::Arc::ptr_eq(&project, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
    }
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks[0].locked = true;
    let locked = s.project.clone();
    assert!(s.execute("captions.add", json!({"track":track.0,"text":"Locked cue","seconds":5})).is_err());
    assert!(std::sync::Arc::ptr_eq(&locked, &s.project));
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks[0].locked = false;
    let second = ClipId(
        s.execute("captions.add", json!({"track":track.0,"text":"Second cue — 日本語","seconds":5,"durationSeconds":1})).unwrap()["caption"].as_u64().unwrap(),
    );
    assert_eq!(s.active_sequence().unwrap().caption_tracks.len(), 1);
    let dir = crate::media_test_util::tmp_dir("audit-caption-atomic-reopen");
    let file = dir.join("captions.fcproj");
    s.execute("file.saveAs", json!({"path":file})).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file})).unwrap();
    let q = reopened.active_sequence().unwrap();
    assert_eq!(q.caption_tracks.len(), 1);
    assert_eq!(q.find_caption(first).unwrap().1.text, "Preserve this first cue");
    assert_eq!(q.find_caption(second).unwrap().1.text, "Second cue — 日本語");
    assert_eq!(q.caption_tracks[0].id, track);
    q.check().unwrap();
}

#[test]
fn captions_import_edit_export_and_burn_in() {
    let dir = std::env::temp_dir().join(format!("fc-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let srt = dir.join("talk.srt");
    std::fs::write(
        &srt,
        "\u{feff}1\r\n00:00:00,500 --> 00:00:02,000\r\nHello there\r\n\r\n2\r\n00:00:02,500 --> 00:00:04,000\r\nSecond line\r\nwith two rows\r\n",
    )
    .unwrap();
    let mut s = demo();
    let r = s.execute("file.import", json!({"paths": [srt.to_string_lossy()]})).unwrap();
    assert_eq!(r["documents"][0]["captions"], 2, "{r}");
    let q = s.active_sequence().unwrap();
    let rate = q.settings.frame_rate;
    let tr = q.caption_tracks[0].clone();
    assert_eq!(tr.name, "talk");
    assert_eq!(tr.captions[0].start, rate.snap_nearest(Tick(TICKS_PER_SECOND / 2)), "snapped to frames");
    let first = tr.captions[0].id;
    let second = tr.captions[1].id;

    // edit text, split, merge, trim, move, add at playhead, delete — each one undoable
    s.execute("captions.setText", json!({"caption": first.0, "text": "Hi!", "speaker": "Ann"})).unwrap();
    let split_at = rate.tick_of(rate.frame_at(tr.captions[1].start) + 10);
    let r = s.execute("captions.split", json!({"caption": second.0, "time": split_at.0})).unwrap();
    let right = ClipId(r["captions"][0].as_u64().unwrap());
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 3);
    s.execute("captions.merge", json!({"captions": [second.0, right.0]})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 2);
    s.execute("captions.trim", json!({"caption": second.0, "edge": "out", "deltaFrames": 12})).unwrap();
    s.execute("captions.move", json!({"captions": [second.0], "deltaFrames": 24})).unwrap();
    s.execute("playhead.set", json!({"seconds": 8.0})).unwrap();
    let r = s.execute("captions.add", json!({"text": "Added"})).unwrap();
    let added = ClipId(r["caption"].as_u64().unwrap());
    assert_eq!(s.state.caption_selection, vec![added]);
    s.execute("edit.clear", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_caption(added).is_none(), "Clear deletes selected captions");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_caption(added).is_some());
    let list = s.execute("captions.list", json!({})).unwrap();
    assert_eq!(list["tracks"][0]["captions"][0]["text"], "Hi!");
    assert_eq!(list["tracks"][0]["captions"][0]["speaker"], "Ann");
    s.active_sequence().unwrap().check().unwrap();

    // export in all three formats and read back: same texts
    for ext in ["srt", "vtt", "scc"] {
        let path = dir.join(format!("out.{ext}")).to_string_lossy().to_string();
        let r = s.execute("captions.export", json!({"path": path})).unwrap();
        assert_eq!(r["captions"], 3, "{ext}: {r}");
        let back = filmcraft_captions::parse(&std::fs::read(&path).unwrap(), filmcraft_captions::Format::from_name(ext).unwrap()).unwrap();
        let texts: Vec<String> = back.cues.iter().map(|c| filmcraft_project::plain_text(&c.text)).collect();
        assert_eq!(texts.len(), 3, "{ext}");
        assert_eq!(texts[0], "Hi!", "{ext}");
    }

    // the program renders captions; hiding the track removes them
    s.execute("playhead.set", json!({"seconds": 1.0})).unwrap();
    let with = s.render_program(0.25).unwrap();
    s.execute("captions.hideAll", json!({})).unwrap();
    let without = s.render_program(0.25).unwrap();
    let diff = with.px.iter().zip(&without.px).filter(|(a, b)| (*a - *b).abs() > 0.05).count();
    assert!(diff > 200, "captions change the picture ({diff} samples)");
    let half = with.w * 4 * (with.h / 2);
    let top_diff = with.px[..half].iter().zip(&without.px[..half]).filter(|(a, b)| (*a - *b).abs() > 0.05).count();
    assert_eq!(top_diff, 0, "bottom-anchored captions stay in the lower half");
    s.execute("captions.showAll", json!({})).unwrap();
    let plan = filmcraft_render::plan::plan_frame(
        &s.project,
        s.state.active_sequence.unwrap(),
        s.playhead(),
        filmcraft_render::RenderOptions { scale: 0.25, captions: true, ..Default::default() },
        &s.media.provider(s.project.clone(), s.services.clone()),
    );
    let cpu = filmcraft_render::plan::execute_cpu(&plan);
    let pd = cpu.px.iter().zip(&with.px).filter(|(a, b)| (*a - *b).abs() > 0.02).count();
    assert!(pd < cpu.px.len() / 200, "the GPU plan matches the CPU render ({pd})");

    // save/open keeps caption tracks
    let proj = dir.join("cap.fcproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": proj})).unwrap();
    let before = s.active_sequence().unwrap().caption_tracks.clone();
    s.execute("file.open", json!({"path": proj})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks, before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caption_track_follows_sync_locked_insert() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({"format": "CEA-608"})).unwrap();
    s.execute("playhead.set", json!({"seconds": 4.0})).unwrap();
    s.execute("captions.add", json!({"text": "later"})).unwrap();
    let start = s.active_sequence().unwrap().caption_tracks[0].captions[0].start;
    let item = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video())).unwrap().id;
    s.execute("source.open", json!({"item": item.0})).unwrap();
    s.execute("playhead.set", json!({"seconds": 0.0})).unwrap();
    s.execute("source.insert", json!({})).unwrap();
    let after = s.active_sequence().unwrap().caption_tracks[0].captions[0].start;
    assert!(after > start, "caption moved right with the insert");
    s.active_sequence().unwrap().check().unwrap();
}

#[test]
fn caption_commands_need_tracks_and_navigate() {
    let mut s = demo();
    assert!(s.execute("captions.split", json!({})).is_err(), "disabled without captions");
    for (sec, text) in [(1.0, "one"), (3.0, "two"), (6.0, "three")] {
        s.execute("playhead.set", json!({"seconds": sec})).unwrap();
        s.execute("captions.add", json!({"text": text, "durationSeconds": 1.0})).unwrap();
    }
    s.execute("playhead.set", json!({"seconds": 0.0})).unwrap();
    s.execute("captions.next", json!({})).unwrap();
    s.execute("captions.next", json!({})).unwrap();
    let id = s.state.caption_selection[0];
    assert_eq!(s.active_sequence().unwrap().find_caption(id).unwrap().1.text, "two");
    s.execute("captions.previous", json!({})).unwrap();
    let id = s.state.caption_selection[0];
    assert_eq!(s.active_sequence().unwrap().find_caption(id).unwrap().1.text, "one");
    s.execute("captions.setStyle", json!({"color": "#ffff00", "anchor": "top", "size": 40})).unwrap();
    let st = &s.active_sequence().unwrap().caption_tracks[0].style;
    assert_eq!(st.color, [255, 255, 0, 255]);
    assert!(s.execute("captions.setStyle", json!({"anchor": "sideways"})).is_err());
    s.execute("captions.setTimes", json!({"caption": id.0, "startSeconds": 0.5, "endSeconds": 2.5})).unwrap();
    assert!(s.execute("captions.setTimes", json!({"caption": id.0, "endSeconds": 3.5})).is_err(), "would overlap `two`");
    s.execute("captions.deleteTrack", json!({"track": "C1"})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks.is_empty());
}

#[test]
fn caption_list_track_filter_never_falls_back_to_every_track() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    s.execute("captions.newTrack", json!({})).unwrap();
    let all = s.execute("captions.list", json!({})).unwrap();
    let ids: Vec<u64> = all["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{all}");
    for (i, id) in ids.iter().enumerate() {
        for sel in [json!(id), json!(format!("C{}", i + 1))] {
            let r = s.execute("captions.list", json!({"track": sel})).unwrap();
            let got: Vec<u64> = r["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
            assert_eq!(got, vec![*id], "{sel}");
        }
    }
    let missing = ids.iter().max().unwrap() + 999;
    for sel in [json!(missing), json!("C3"), json!("C0"), json!("nope")] {
        assert!(s.execute("captions.list", json!({"track": sel})).is_err(), "{sel} names no caption track");
    }
}
