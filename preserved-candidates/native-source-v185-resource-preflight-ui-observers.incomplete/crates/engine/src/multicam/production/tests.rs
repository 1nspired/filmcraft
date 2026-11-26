use super::*;
use crate::media_test_util::{make_movie, tmp_dir};
use filmcraft_media::DemoScene;
use filmcraft_project::{Marker, MarkerId, MarkerKind};
use std::path::PathBuf;

const METHODS: [&str; 5] = ["in", "out", "timecode", "marker", "audio"];
fn bytes(s: &Session) -> Vec<u8> {
    serde_json::to_vec(&serde_json::to_value(&*s.project).unwrap()).unwrap()
}
fn editor(s: &Session) -> Value {
    serde_json::to_value(&s.state).unwrap()
}
fn fork(s: &Session) -> Session {
    let mut other = Session::new(s.services.clone());
    other.project = s.project.clone();
    other.media = s.media.clone();
    other.state = s.state.clone();
    other.prefs = s.prefs.clone();
    other.revision = s.revision;
    other.saved_revision = s.saved_revision;
    other.path = s.path.clone();
    other
}
fn setup() -> (Session, Vec<ItemId>, ItemId) {
    let root = tmp_dir("multicam-shared-production");
    let paths: Vec<PathBuf> = ["one.mov", "two.mov"].iter().map(|n| root.join(n)).collect();
    for p in &paths {
        make_movie(p, DemoScene::OceanSunset, 64, 48, 48);
    }
    let mut s = Session::default();
    let imported = s.execute("file.import", json!({"paths":paths.iter().map(|p|p.to_string_lossy()).collect::<Vec<_>>()})).unwrap();
    let mut items: Vec<ItemId> = imported["items"].as_array().unwrap().iter().map(|v| ItemId(v.as_u64().unwrap())).collect();
    let source = s.source(items[0]).unwrap();
    let audio = source.audio(0, 96000, 48000).unwrap();
    let mono = filmcraft_audio_dsp::sync::mixdown(&audio.channels.iter().map(|c| c.as_slice()).collect::<Vec<_>>());
    let wav = root.join("recorder.wav");
    std::fs::write(&wav, crate::voiceover::write_wav_f32(&mono, 48000)).unwrap();
    let row = s.execute("file.import", json!({"paths":[wav]})).unwrap();
    items.push(ItemId(row["items"][0].as_u64().unwrap()));
    let rate = FrameRate::FPS_24;
    let mut project = (*s.project).clone();
    for (k, id) in items.iter().enumerate() {
        let media = project.item_mut(*id).unwrap().as_media_mut().unwrap();
        media.mark_in = Some(rate.tick_of([4, 12, 8][k]));
        media.mark_out = Some(rate.tick_of([35, 31, 39][k]));
        media.info.start_timecode = Some([86424, 172834, 86428][k]);
        media.markers.push(Marker {
            id: MarkerId(900 + id.0),
            start: rate.tick_of([16, 6, 10][k]),
            duration: Tick::ZERO,
            name: "Clap".into(),
            comment: String::new(),
            kind: MarkerKind::Comment,
            color: Label::Rose,
        });
        project.item_mut(*id).unwrap().metadata.insert("Camera Angle".into(), ["2", "1", "3"][k].into());
    }
    let sequence = project.new_sequence("Original owning edit", SequenceSettings { width: 64, height: 48, frame_rate: rate, ..Default::default() }, 2, 2, None);
    for (k, id) in items[..2].iter().enumerate() {
        let link = project.alloc_id();
        let range = TimeRange::new(rate.tick_of([4, 12][k]), rate.tick_of([28, 20][k]));
        let at = rate.tick_of([0, 40][k]);
        let mut v = project.make_track_item(*id, TrackKind::Video, at, range, rate).unwrap();
        v.link = Some(link);
        let mut a = project.make_track_item(*id, TrackKind::Audio, at, range, rate).unwrap();
        a.link = Some(link);
        let q = project.sequence_mut(sequence).unwrap();
        q.video_tracks[k].items.push(v);
        q.audio_tracks[k].items.push(a);
    }
    s.project = Arc::new(project);
    s.state.active_sequence = Some(sequence);
    s.state.open_sequences = vec![sequence];
    s.state.project_selection = items.clone();
    s.state.selection = s.project.sequence(sequence).unwrap().video_tracks.iter().flat_map(|t| t.items.iter().map(|c| c.id)).collect();
    (s, items, sequence)
}
fn params(command: &str, method: &str, items: &[ItemId]) -> Value {
    let mut p = json!({"method":method,"marker":"clap","ignoreHours":true,"offset":2});
    if command == "clip.mergeClips" {
        p["items"] = json!([items[0].0, items[2].0]);
        p["removeVideoAudio"] = json!(true);
    }
    if command == "clip.createMulticam" {
        p["items"] = json!(items.iter().map(|i| i.0).collect::<Vec<_>>());
        p["audio"] = json!("switch");
        p["processedBin"] = json!(true);
    }
    p
}
fn exercise(command: &str) {
    let (base, items, original) = setup();
    for method in METHODS {
        let mut planned = fork(&base);
        let mut human = fork(&base);
        let p = params(command, method, &items);
        let before = bytes(&planned);
        let state = editor(&planned);
        let revision = planned.revision;
        let project = planned.project.clone();
        let prepared = prepare(&planned, command, &p).unwrap();
        assert!(Arc::ptr_eq(&planned.project, &project));
        assert_eq!(bytes(&planned), before);
        assert_eq!(editor(&planned), state);
        assert_eq!(planned.revision, revision);
        assert!(planned.history.undo.is_empty());
        assert!(planned.history.redo.is_empty());
        assert!(planned.journal.is_empty());
        let result = prepared.apply(&mut planned).unwrap();
        let expected = human.execute(command, p).unwrap();
        assert_eq!(result, expected, "{command}/{method}");
        assert_eq!(bytes(&planned), bytes(&human), "{command}/{method}");
        assert_eq!(editor(&planned), editor(&human));
        assert_eq!(planned.history.undo.len(), 1, "{command}/{method}");
        assert_eq!(planned.revision, revision + 1);
        let final_project = bytes(&planned);
        // Native human history stores projects; current valid selections survive Undo.
        // Compare the complete actual human Undo/Redo state, not a fabricated editor restore.
        planned.execute("edit.undo", json!({})).unwrap();
        human.execute("edit.undo", json!({})).unwrap();
        assert_eq!(bytes(&planned), before);
        assert_eq!(bytes(&planned), bytes(&human));
        assert_eq!(editor(&planned), editor(&human));
        assert!(planned.history.undo.is_empty());
        assert_eq!(planned.history.redo.len(), 1);
        planned.execute("edit.redo", json!({})).unwrap();
        human.execute("edit.redo", json!({})).unwrap();
        assert_eq!(bytes(&planned), final_project);
        assert_eq!(bytes(&planned), bytes(&human));
        assert_eq!(editor(&planned), editor(&human));
        assert_eq!(planned.history.undo.len(), 1);
        assert!(planned.history.redo.is_empty());
        if command == "clip.synchronize" {
            assert_eq!(planned.state.active_sequence, Some(original));
            let q = planned.project.sequence(original).unwrap();
            for (v, a) in q.video_tracks.iter().zip(&q.audio_tracks) {
                assert_eq!(v.items.len(), 1);
                assert_eq!(a.items.len(), 1);
                assert_eq!(
                    (v.items[0].link, v.items[0].start, v.items[0].source_in, v.items[0].duration),
                    (a.items[0].link, a.items[0].start, a.items[0].source_in, a.items[0].duration)
                );
            }
            if method == "in" {
                assert_eq!(result["clips"][0]["anchor"], FrameRate::FPS_24.tick_of(4).0);
                assert_eq!(result["clips"][1]["anchor"], FrameRate::FPS_24.tick_of(10).0);
            }
            if method == "out" {
                assert_eq!(result["clips"][0]["anchor"], FrameRate::FPS_24.tick_of(32).0);
                assert_eq!(result["clips"][1]["anchor"], FrameRate::FPS_24.tick_of(30).0);
            }
        } else {
            let id = ItemId(result[if command == "clip.mergeClips" { "item" } else { "sequence" }].as_u64().unwrap());
            assert_ne!(id, original);
            assert_eq!(planned.state.active_sequence, Some(original));
            let q = planned.project.sequence(id).unwrap();
            assert_eq!(q.video_tracks.len(), if command == "clip.mergeClips" { 1 } else { 2 });
            assert_eq!(q.audio_tracks.len(), if command == "clip.mergeClips" { 1 } else { 3 });
            if command == "clip.mergeClips" {
                assert_eq!(q.merged.as_ref().unwrap().sync, method);
            } else {
                assert_eq!(q.multicam.as_ref().unwrap().sync, method);
            }
            if method == "out" {
                assert_eq!(result["clips"][0]["anchor"], FrameRate::FPS_24.tick_of(36).0);
            }
        }
        eprintln!("shared native production {command}/{method}: readonly prepare, exact human result, linked sound, one UndoRedo");
    }
}
#[test]
fn all_five_timeline_sync_methods_preserve_current_instance_ranges_linked_sound_and_one_human_history_step() {
    exercise("clip.synchronize");
}
#[test]
fn all_five_merge_methods_share_preparation_and_exact_native_component_history() {
    exercise("clip.mergeClips");
}
#[test]
fn all_five_multicam_creation_methods_share_preparation_and_exact_native_camera_history() {
    exercise("clip.createMulticam");
}

#[test]
fn retained_preparation_refuses_later_cow_context_source_pool_preferences_save_path_and_gesture_changes_without_writes() {
    let (base, items, _) = setup();
    for kind in 0..8 {
        let mut s = fork(&base);
        let p = prepare(&s, "clip.createMulticam", &params("clip.createMulticam", "in", &items)).unwrap();
        match kind {
            0 => s.project = Arc::new((*s.project).clone()),
            1 => s.state.project_selection.reverse(),
            2 => s.path = Some("/later-human-save.fcproj".into()),
            3 => s.saved_revision += 1,
            4 => s.revision += 1,
            5 => s.media = Arc::new(MediaPool::default()),
            6 => s.mcrec.pass += 1,
            _ => s.prefs.auto_save.enabled = !s.prefs.auto_save.enabled,
        }
        let before = bytes(&s);
        let state = editor(&s);
        let revision = s.revision;
        assert!(p.apply(&mut s).unwrap_err().to_string().contains("prepare again"), "case {kind}");
        assert_eq!(bytes(&s), before);
        assert_eq!(editor(&s), state);
        assert_eq!(s.revision, revision);
        assert!(s.history.undo.is_empty());
    }
}

#[test]
fn camera_naming_aliases_keep_the_original_metadata_only_sort_and_audio_processed_bin_options() {
    let (base, items, _) = setup();
    for mode in ["clip", "track", "tracks", "trackname", "trackNames", "enumerate", "metadata", "cameraAngle", "label", "unknown-human-fallback"] {
        for audio in ["camera1", "all", "switch"] {
            let mut s = fork(&base);
            let p = json!({"items":items.iter().map(|i|i.0).collect::<Vec<_>>(),"method":"start","cameraNames":mode,"audio":audio,"processedBin":true,"reference":items[1].0,"name":"Shared exact camera names"});
            let result = prepare(&s, "clip.createMulticam", &p).unwrap().apply(&mut s).unwrap();
            let q = s.project.sequence(ItemId(result["sequence"].as_u64().unwrap())).unwrap();
            let mc = q.multicam.as_ref().unwrap();
            assert_eq!(mc.audio, MulticamAudio::from_name(audio).unwrap());
            assert_eq!(
                mc.cameras.iter().map(|c| c.source.unwrap()).collect::<Vec<_>>(),
                if mode == "metadata" { vec![items[1], items[0], items[2]] } else { items.clone() }
            );
            if mode == "metadata" {
                assert_eq!(result["cameras"], json!(["1", "2", "3"]));
            }
            if matches!(mode, "cameraAngle" | "label") {
                assert_eq!(result["cameras"], json!(["2", "1", "3"]));
            }
            if matches!(mode, "track" | "tracks" | "trackname" | "trackNames" | "enumerate") {
                assert_eq!(result["cameras"], json!(["Camera 1", "Camera 2", "Camera 3"]));
            }
            let processed = s
                .project
                .root
                .children
                .iter()
                .find_map(|e| match e {
                    BinEntry::Bin(b) if b.name == "Processed Clips" => Some(b),
                    _ => None,
                })
                .unwrap();
            assert_eq!(processed.children.len(), 3);
        }
    }
}

#[test]
fn original_method_aliases_and_project_timeline_selection_fallbacks_retain_exact_targets() {
    let (base, items, _) = setup();
    for aliases in [
        &["in", "inpoint", "inpoints", "start", "clipstart"][..],
        &["out", "outpoint", "outpoints", "end", "clipend"][..],
        &["timecode", "tc"][..],
        &["marker", "clipmarker", "markers"][..],
        &["audio", "sound", "waveform"][..],
    ] {
        let mut results = Vec::new();
        for alias in aliases {
            let mut s = fork(&base);
            let r = prepare(&s, "clip.createMulticam", &json!({"method":alias,"marker":"CLAP"})).unwrap().apply(&mut s).unwrap();
            results.push((r, bytes(&s), editor(&s)));
        }
        assert!(results.windows(2).all(|w| w[0] == w[1]));
    }
    let mut s = fork(&base);
    s.state.project_selection.clear();
    let p = prepare(&s, "clip.createMulticam", &json!({"method":"in"})).unwrap();
    let r = p.apply(&mut s).unwrap();
    let q = s.project.sequence(ItemId(r["sequence"].as_u64().unwrap())).unwrap();
    assert_eq!(q.multicam.as_ref().unwrap().cameras.iter().map(|c| c.source.unwrap()).collect::<Vec<_>>(), items[..2]);
}

#[test]
fn invalid_source_gestures_are_readonly_and_do_not_allocate_source_or_history() {
    let (s, items, _) = setup();
    let before = bytes(&s);
    let state = editor(&s);
    let revision = s.revision;
    let undo = s.history.undo.len();
    let redo = s.history.redo.len();
    let journal = s.journal.clone();
    for (command, p) in [
        ("other.command", json!({})),
        ("clip.createMulticam", json!({"items":[items[0].0],"reference":999999})),
        ("clip.createMulticam", json!({"items":[items[2].0]})),
        ("clip.createMulticam", json!({"method":"marker","marker":"missing"})),
        ("clip.mergeClips", json!({"items":[items[0].0,items[1].0]})),
        ("clip.synchronize", json!({"clips":[],"method":"in"})),
    ] {
        assert!(prepare(&s, command, &p).is_err(), "{command}");
        assert_eq!(bytes(&s), before);
        assert_eq!(editor(&s), state);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
        assert_eq!(s.history.redo.len(), redo);
        assert_eq!(s.journal, journal);
    }
}
