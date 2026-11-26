use super::READ;
use crate::Session;
use filmcraft_project::{Camera, ClipId, ItemId, MulticamAudio, MulticamSel, MulticamSource, SequenceSettings, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;
const WORKSPACE: &str = "d80e6c1b-456b-4227-a2ce-258badc319d1";
fn scope() -> Value {
    json!({"projectId":"p_multicam","workspaceId":WORKSPACE})
}
fn req(s: &Session) -> Value {
    json!({"version":1,"requestId":"11111111-2222-4333-8444-555555555555","scope":scope(),"projectPath":s.path,"timeTicks":null,"cameraOffset":0,"cutOffset":0,"limit":8})
}
fn setup(count: usize, explicit: bool) -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Multicam read","workspace":scope()})).unwrap();
    s.path = Some("/generated/multicam.fcproj".into());
    let initial = s.execute("file.newSequence", json!({"name":"Edit","video":2,"audio":2})).unwrap();
    let initial_sequence = ItemId(initial["sequence"].as_u64().unwrap());
    let template = super::super::tests::source_clock_fixture().active_sequence().unwrap().video_tracks[0].items[0].clone();
    let p = Arc::make_mut(&mut s.project);
    let source = p.new_sequence("Camera source", SequenceSettings::default(), count, count, None);
    let cameras = {
        let q = p.sequence(source).unwrap();
        (0..count)
            .map(|i| Camera {
                name: format!("Camera {i}"),
                video_track: (i != 3).then_some(q.video_tracks[i].id),
                audio_tracks: vec![q.audio_tracks[i].id],
                enabled: i != 1,
                source: None,
            })
            .collect()
    };
    if explicit {
        p.sequence_mut(source).unwrap().multicam = Some(MulticamSource { cameras, audio: MulticamAudio::SwitchAudio, sync: "timecode".into() });
    }
    let sequence = initial_sequence;
    let q = p.sequence_mut(sequence).unwrap();
    let mut video = template;
    video.id = ClipId(80000);
    video.identity.as_mut().unwrap().id = "a".repeat(32);
    video.identity.as_mut().unwrap().parent = None;
    video.item = source;
    video.start = Tick::ZERO;
    video.source_in = Tick::ZERO;
    video.duration = Tick(254_016_000_000 * 10);
    video.speed = 1.0;
    video.reverse = false;
    video.frame_hold = None;
    video.multicam = Some(MulticamSel { enabled: true, angle: 0 });
    video.link = Some(90000);
    let mut audio = video.clone();
    audio.id = ClipId(80001);
    audio.identity.as_mut().unwrap().id = "b".repeat(32);
    q.video_tracks[0].items.push(video);
    q.audio_tracks[0].items.push(audio);
    s.state.active_sequence = Some(sequence);
    s.state.open_sequences = vec![sequence];
    s.set_playhead(Tick::ZERO);
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal,
      "recorder":format!("{:?}",s.mcrec),"tickets":s.agent_bridge.tickets.len()})
}
fn read(s: &mut Session, p: Value) -> Value {
    let before = state(s);
    let project = s.project.clone();
    let r = s.execute(READ, p).unwrap();
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&project, &s.project));
    r
}
fn now(s: &mut Session) -> Value {
    let p = req(s);
    read(s, p)
}
#[test]
fn native_multicam_read_has_real_absence_no_ticket_and_no_mutation_or_source_authority() {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Empty","workspace":scope()})).unwrap();
    s.path = Some("/generated/empty.fcproj".into());
    let r = now(&mut s);
    assert!(r["sequence"].is_null());
    assert!(r["frameRate"].is_null());
    assert!(r["inspection"]["clip"].is_null());
    assert_eq!(r["inspection"]["pagination"]["cameraTotal"], 0);
    for key in ["newReadTicket", "mediaEvidence", "writeAuthority", "recordingOwnership"] {
        assert_eq!(r[key], false);
    }
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(s.agent_bridge.tickets.len(), 0);
    assert!(s.execute("agent.apply", json!({"token":r["requestId"],"label":"not a ticket","operations":[]})).is_err());
}
#[test]
fn native_multicam_paged_camera_read_matches_original_full_inspector_and_preserves_hidden_audio_only_rows() {
    let mut s = setup(43, true);
    let human = s.execute("multicam.inspect", json!({})).unwrap();
    let mut cameras = Vec::new();
    let mut shown = Vec::new();
    for offset in (0..48).step_by(8) {
        let mut p = req(&s);
        p["cameraOffset"] = json!(offset);
        let r = read(&mut s, p);
        let data = &r["inspection"];
        assert_eq!(data["pagination"]["cameraTotal"], 43);
        assert_eq!(data["pagination"]["shownTotal"], 41);
        for row in data["cameras"].as_array().unwrap() {
            assert!(row["audioTracks"].as_array().unwrap().len() == 1);
            let mut legacy = row.clone();
            for key in ["shownCamera", "sourceItem", "videoTrack", "audioTracks"] {
                legacy.as_object_mut().unwrap().remove(key);
            }
            cameras.push(legacy);
        }
        shown.extend(data["shown"].as_array().unwrap().iter().cloned());
    }
    assert_eq!(json!(cameras), human["cameras"]);
    assert_eq!(json!(shown), human["shown"]);
    let r = now(&mut s);
    assert_eq!(r["inspection"]["cameras"][1]["shownCamera"], Value::Null);
    assert_eq!(r["inspection"]["cameras"][2]["shownCamera"], 2);
    assert_eq!(r["inspection"]["cameras"][3]["video"], false);
}
#[test]
fn native_multicam_borrowed_camera_access_keeps_the_original_plain_sequence_fallback_and_explicit_camera_payload() {
    for explicit in [false, true] {
        let mut s = setup(40, explicit);
        let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
        let q = s.project.sequence(item).unwrap();
        let all = q.cameras();
        assert_eq!(q.camera_count(), 40);
        for (i, expected) in all.cameras.iter().enumerate() {
            assert_eq!(q.camera_at(i).unwrap().as_ref(), expected);
        }
        assert!(q.camera_at(40).is_none());
        assert_eq!(q.camera_audio(), all.audio);
        let human = s.execute("multicam.inspect", json!({})).unwrap();
        let r = now(&mut s);
        assert_eq!(r["inspection"]["audio"], human["audio"]);
        assert_eq!(r["inspection"]["cameras"].as_array().unwrap().len(), 8);
    }
}
#[test]
fn native_multicam_metadata_retains_exact_large_tick_strings_and_each_original_source_clock() {
    for mode in 0..4 {
        let mut s = setup(2, true);
        let id = s.state.active_sequence.unwrap();
        let c = &mut Arc::make_mut(&mut s.project).sequence_mut(id).unwrap().video_tracks[0].items[0];
        c.source_in = Tick(9_007_199_254_740_993);
        c.start = Tick(10);
        c.duration = Tick(1000);
        match mode {
            1 => {
                c.speed = 0.4;
            }
            2 => {
                c.reverse = true;
            }
            3 => {
                c.frame_hold = Some(Tick(9_007_199_254_740_995));
            }
            _ => {}
        }
        let expected = c.source_time_at(Tick(37)).0;
        let mut p = req(&s);
        p["timeTicks"] = json!("37");
        let r = read(&mut s, p);
        assert_eq!(r["timeTicks"], "37");
        assert_eq!(r["inspection"]["sourceTime"], expected.to_string());
        assert!(expected > 9_007_199_254_740_991);
    }
}
#[test]
fn native_multicam_read_can_observe_the_original_live_pass_without_borrowing_it_or_relaxing_ordinary_idle_guards() {
    let mut s = setup(5, true);
    s.execute("multicam.recordStart", json!({"time":0})).unwrap();
    s.execute("multicam.cut", json!({"camera":2,"time":1})).unwrap();
    s.execute("multicam.cut", json!({"camera":3,"time":2})).unwrap();
    let r = now(&mut s);
    assert_eq!(r["inspection"]["recorder"]["recording"], true);
    assert_eq!(r["inspection"]["pagination"]["cutTotal"], 2);
    assert_eq!(r["inspection"]["recorder"]["cuts"][1]["time"], "2");
    assert!(s.execute("agent.snapshot", json!({"token":"22222222-3333-4444-8555-666666666666"})).is_err());
    assert!(s.mcrec.active());
    s.execute("multicam.recordStop", json!({"time":10})).unwrap();
    assert!(!s.mcrec.active());
    assert_eq!(now(&mut s)["inspection"]["recorder"]["recording"], false);
}
#[test]
fn native_multicam_reads_use_fresh_original_view_paging_camera_metadata_and_later_human_angle_edits() {
    let mut s = setup(43, true);
    s.execute("multicam.gridLayout", json!({"layout":"4x4"})).unwrap();
    s.execute("multicam.page", json!({"page":1})).unwrap();
    let first = now(&mut s);
    assert_eq!(first["inspection"]["gridPage"]["first"], 16);
    s.execute("multicam.switchAngle", json!({"camera":3})).unwrap();
    let next = now(&mut s);
    assert_ne!(first["projectStateSha256"], next["projectStateSha256"]);
    assert_ne!(first["revision"], next["revision"]);
    assert_eq!(next["inspection"]["angle"], 4);
    s.execute("multicam.editCameras", json!({"cameras":[{"angle":0,"name":"New camera","enabled":false}]})).unwrap();
    let last = now(&mut s);
    assert_eq!(last["inspection"]["cameras"][0]["name"], "New camera");
    assert_eq!(last["inspection"]["pagination"]["shownTotal"], 40);
    assert_eq!(last["inspection"]["cameras"][2]["shownCamera"], 1);
}
#[test]
fn native_multicam_read_refuses_foreign_roots_paths_unknown_fields_bad_pages_and_inexact_ticks_without_changes() {
    let mut s = setup(4, true);
    for (key, value) in [
        ("version", json!(2)),
        ("scope", json!({"projectId":"foreign","workspaceId":WORKSPACE})),
        ("projectPath", json!("/generated/foreign.fcproj")),
        ("requestId", json!("short")),
        ("limit", json!(0)),
        ("limit", json!(33)),
        ("cameraOffset", json!(100001)),
        ("cutOffset", json!(-1)),
        ("timeTicks", json!(1)),
        ("timeTicks", json!("01")),
        ("timeTicks", json!("-0")),
        ("timeTicks", json!("9223372036854775808")),
        ("unexpected", json!(true)),
    ] {
        let mut p = req(&s);
        p[key] = value;
        let before = state(&s);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(READ, p))).unwrap().is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn native_multicam_read_refuses_nonrepresentable_selected_or_camera_ids_and_oversized_metadata_without_mutation() {
    for mode in 0..5 {
        let mut s = setup(4, true);
        let sequence = s.state.active_sequence.unwrap();
        let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
        let p = Arc::make_mut(&mut s.project);
        if mode == 0 {
            p.sequence_mut(sequence).unwrap().video_tracks[0].items[0].id = ClipId(9_007_199_254_740_992);
        } else {
            let camera = &mut p.sequence_mut(source).unwrap().multicam.as_mut().unwrap().cameras[0];
            match mode {
                1 => camera.name = "x".repeat(4097),
                2 => camera.audio_tracks = vec![TrackId(1); 257],
                3 => camera.video_track = Some(TrackId(9_007_199_254_740_992)),
                _ => camera.source = Some(ItemId(9_007_199_254_740_992)),
            }
        }
        let before = state(&s);
        let req = req(&s);
        assert!(s.execute(READ, req).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn native_multicam_recorder_cut_pages_and_extreme_read_positions_are_bounded_exact_and_effect_free() {
    let mut s = setup(2, true);
    s.execute("multicam.recordStart", json!({})).unwrap();
    s.mcrec.cuts = (0..75).map(|i| filmcraft_edit::multicam::Cut { time: Tick(9_007_199_254_740_993 + i), angle: 0 }).collect();
    for offset in [0, 32, 64, 96] {
        let mut p = req(&s);
        p["limit"] = json!(32);
        p["cutOffset"] = json!(offset);
        let r = read(&mut s, p);
        assert_eq!(r["inspection"]["pagination"]["cutTotal"], 75);
        assert_eq!(r["inspection"]["recorder"]["cuts"].as_array().unwrap().len(), (75usize.saturating_sub(offset)).min(32));
        if offset < 75 {
            assert_eq!(r["inspection"]["recorder"]["cuts"][0]["time"], (9_007_199_254_740_993i64 + offset as i64).to_string());
        }
    }
    for time in ["-9223372036854775808", "9223372036854775807"] {
        let mut p = req(&s);
        p["timeTicks"] = json!(time);
        let r = read(&mut s, p);
        assert!(r["inspection"]["clip"].is_null());
        assert_eq!(r["timeTicks"], time);
    }
}

// Exact predecessor human inspector and camera recipe: independent compatibility oracle.
use crate::multicam::multicam_clip_at;
use filmcraft_project::{Sequence, TrackItem};
fn audio_partners(q: &Sequence, video: &TrackItem) -> Vec<ClipId> {
    let Some(l) = video.link else { return Vec::new() };
    q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.link == Some(l) && i.multicam.is_some()).map(|i| i.id).collect()
}
fn predecessor_inspect_at(s: &Session, t: Tick) -> Value {
    let rec = json!({"recording": s.mcrec.active(), "cuts": s.mcrec.cuts.iter().map(|c| json!({"time": c.time.0, "angle": c.angle})).collect::<Vec<_>>()});
    let Some((track, clip)) = multicam_clip_at(s, t) else {
        return json!({"clip": null, "recorder": rec, "audioFollowsVideo": s.state.multicam_audio_follows_video});
    };
    let q = s.project.sequence(clip.item);
    let cams = q.map(predecessor_cameras).unwrap_or_default();
    let audio_angle = s
        .active_sequence()
        .map(|seq| audio_partners(seq, &clip))
        .and_then(|ps| ps.first().and_then(|c| s.active_sequence().and_then(|seq| seq.find_item(*c)).and_then(|x| x.1.multicam.map(|m| m.angle))));
    json!({
        "clip": clip.id.0,
        "track": track.0,
        "source": clip.item.0,
        "sourceName": s.project.item(clip.item).map(|i| i.name.clone()),
        "sourceTime": clip.source_time_at(t).0,
        "angle": clip.multicam.map(|m| m.angle),
        "audioAngle": audio_angle,
        "audio": cams.audio.name(),
        "shown": cams.shown_angles(),
        "cameras": cams.cameras.iter().enumerate().map(|(i, c)| json!({"angle": i, "name": c.name, "enabled": c.enabled, "video": c.video_track.is_some()})).collect::<Vec<_>>(),
        "recorder": rec,
        "audioFollowsVideo": s.state.multicam_audio_follows_video,
        "view": s.state.multicam_view,
    })
}

fn predecessor_cameras(q: &Sequence) -> MulticamSource {
    if let Some(m) = &q.multicam {
        return m.clone();
    }
    MulticamSource {
        cameras: q
            .video_tracks
            .iter()
            .enumerate()
            .map(|(i, t)| Camera {
                name: t.name.clone(),
                video_track: Some(t.id),
                audio_tracks: q.audio_tracks.get(i).map(|a| vec![a.id]).unwrap_or_default(),
                enabled: true,
                source: None,
            })
            .collect(),
        audio: MulticamAudio::AllCameras,
        sync: String::new(),
    }
}

#[test]
fn native_multicam_original_full_inspector_matches_the_unchanged_predecessor_recipe_in_all_generated_contexts() {
    for explicit in [false, true] {
        for count in [0, 1, 5, 43] {
            let mut s = setup(count, explicit);
            for time in [Tick(-1), Tick::ZERO, Tick(7), Tick(254_016_000_000 * 11)] {
                assert_eq!(crate::multicam::inspect_at(&s, time), predecessor_inspect_at(&s, time));
            }
            let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
            let q = s.project.sequence(source).unwrap();
            assert_eq!(q.cameras(), predecessor_cameras(q));
            if explicit && count >= 5 {
                s.execute("multicam.recordStart", json!({"time":0})).unwrap();
                s.execute("multicam.cut", json!({"camera":2,"time":1})).unwrap();
                assert_eq!(crate::multicam::inspect_at(&s, Tick(1)), predecessor_inspect_at(&s, Tick(1)));
            }
        }
    }
}
