//! Mixer rejection must preserve the project and the human's entire history branch.
use crate::Session;
use serde_json::{Value, json};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn preserve_redo(s: &mut Session) {
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    assert!(!s.history.redo.is_empty());
    s.history.merge_key = Some("existing human mixer gesture".into());
}

fn rejected(s: &mut Session, command: &str, args: Value) {
    let project = s.project.clone();
    let bytes = serde_json::to_vec(&*project).unwrap();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let merge = s.history.merge_key.clone();
    let journal = s.journal.clone();
    assert!(s.execute(command, args.clone()).is_err(), "{command} accepted {args}");
    assert!(std::sync::Arc::ptr_eq(&project, &s.project), "published a project after rejecting {command} {args}");
    assert_eq!(serde_json::to_vec(&*s.project).unwrap(), bytes);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.merge_key, merge);
    assert_eq!(s.journal, journal);
    for (before, after) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(before.len(), after.len());
        assert!(before.iter().zip(after).all(|((a, p), (b, q))| a == b && std::sync::Arc::ptr_eq(p, q)));
    }
}

#[test]
fn audio_gain_malformed_values_and_targets_preserve_the_entire_human_branch() {
    let mut s = demo();
    s.execute("clip.audioGain", json!({"clips":[30],"mode":"set","db":12})).unwrap();
    preserve_redo(&mut s);
    for args in [
        json!(null),
        json!([]),
        json!(false),
        json!({}),
        json!({"mode":"set","clips":[30]}),
        json!({"clips":[30],"mode":null,"db":-6}),
        json!({"clips":[30],"mode":"typo","db":-6}),
        json!({"clips":[30],"mode":"set","db":-6,"relative":false}),
        json!({"clips":[30],"mode":"set","db":-6,"clipz":[32]}),
    ] {
        rejected(&mut s, "clip.audioGain", args);
    }
    for value in [json!(null), json!(true), json!("-6"), json!("not-a-number"), json!([]), json!({}), json!(-96.001), json!(96.001), json!(1e300)] {
        rejected(&mut s, "clip.audioGain", json!({"clips":[30],"mode":"set","db":value}));
    }
    for value in [json!(null), json!(1), json!("true"), json!([]), json!({})] {
        rejected(&mut s, "clip.audioGain", json!({"clips":[30],"db":-6,"relative":value}));
    }
    for clips in [
        json!(null),
        json!(30),
        json!("30"),
        json!([]),
        json!([30, 30]),
        json!([30, 0]),
        json!([30, -1]),
        json!([30, 30.5]),
        json!([30, "bad-id"]),
        json!([30, 900000000]),
        json!([30, null]),
        json!([30, u64::MAX]),
        json!(vec![30; 4097]),
    ] {
        rejected(&mut s, "clip.audioGain", json!({"clips":clips,"mode":"set","db":-6}));
        rejected(&mut s, "clip.audioPeak", json!({"clips":clips}));
    }
    rejected(&mut s, "clip.audioPeak", json!({"clips":[30],"unknown":false}));
}

#[test]
fn audio_gain_refuses_locked_linked_sound_and_any_out_of_range_result_before_editing() {
    let mut s = demo();
    let second = s.active_sequence().unwrap().audio_tracks[0].items[1].id.0;
    s.execute("clip.audioGain", json!({"clips":[second],"mode":"set","db":95})).unwrap();
    preserve_redo(&mut s);
    rejected(&mut s, "clip.audioGain", json!({"clips":[30,second],"mode":"adjust","db":2}));
    let sid = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].locked = true;
    for target in [29, 30] {
        rejected(&mut s, "clip.audioGain", json!({"clips":[target],"mode":"set","db":-6}));
        rejected(&mut s, "clip.audioGain", json!({"clips":[target],"mode":"normalizeAll","db":-6}));
    }
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].locked = false;
    s.execute("sequence.linkedSelection", json!({"on":false})).unwrap();
    rejected(&mut s, "clip.audioGain", json!({"clips":[29],"mode":"set","db":-6}));
}

#[test]
fn audio_gain_unchanged_values_do_not_destroy_redo_or_create_an_undo_step() {
    let mut s = demo();
    preserve_redo(&mut s);
    let project = s.project.clone();
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let revision = s.revision;
    let merge = s.history.merge_key.clone();
    for args in [json!({"clips":[30],"mode":"set","db":0}), json!({"clips":[30],"db":0,"relative":true})] {
        let result = s.execute("clip.audioGain", args).unwrap();
        assert_eq!(result["changed"], false);
        assert!(std::sync::Arc::ptr_eq(&project, &s.project));
        assert_eq!(revision, s.revision);
        assert_eq!(merge, s.history.merge_key);
        for (a, b) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
            assert_eq!(a.len(), b.len());
            assert!(a.iter().zip(b).all(|((x, p), (y, q))| x == y && std::sync::Arc::ptr_eq(p, q)));
        }
    }
}

#[derive(Clone, Copy)]
enum PeakFault {
    None,
    SplitChannels,
    Decode,
    LateDecode,
    Short,
    WrongRate,
    Empty,
    Uneven,
    Nonfinite,
    Silent,
}
struct PeakSource {
    info: filmcraft_media::MediaInfo,
    fault: PeakFault,
}
impl filmcraft_media::MediaSource for PeakSource {
    fn info(&self) -> &filmcraft_media::MediaInfo {
        &self.info
    }
    fn video_frame(&self, _: filmcraft_media::FrameRequest) -> filmcraft_media::Result<std::sync::Arc<filmcraft_frame::VideoFrame>> {
        Err(filmcraft_media::MediaError::NoStream("video"))
    }
    fn audio(&self, start: i64, frames: usize, rate: u32) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
        if matches!(self.fault, PeakFault::Decode) || matches!(self.fault, PeakFault::LateDecode) && start > 0 {
            return Err(filmcraft_media::MediaError::NoStream("audio"));
        }
        let length = if matches!(self.fault, PeakFault::Short) { frames.saturating_sub(1) } else { frames };
        let value = if matches!(self.fault, PeakFault::Silent) { 0.0 } else { 0.25 };
        let mut channels = vec![vec![value; length]; 2];
        if matches!(self.fault, PeakFault::SplitChannels) {
            channels = vec![vec![0.9; length], vec![0.1; length]];
        }
        if matches!(self.fault, PeakFault::Nonfinite) {
            channels[1][0] = f32::NAN;
        }
        if matches!(self.fault, PeakFault::Uneven) {
            channels[1].pop();
        }
        if matches!(self.fault, PeakFault::Empty) {
            channels.clear();
        }
        Ok(filmcraft_frame::AudioBuffer { sample_rate: if matches!(self.fault, PeakFault::WrongRate) { rate + 1 } else { rate }, channels })
    }
}
fn peak_source(s: &mut Session, fault: PeakFault) {
    let sid = s.state.active_sequence.unwrap();
    let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let clip = q.find_item_mut(filmcraft_project::ClipId(30)).unwrap().1;
    clip.source_in = filmcraft_time::Tick::ZERO;
    clip.duration = filmcraft_time::Tick(filmcraft_time::TICKS_PER_SECOND * 3);
    let item = clip.item;
    let info = s.project.resolve_media(item).unwrap().1.info.clone();
    s.media.insert(item, std::sync::Arc::new(PeakSource { info, fault }));
}

#[test]
fn audio_normalization_never_publishes_partial_failed_or_nonfinite_peak_measurements() {
    for fault in [
        PeakFault::Decode,
        PeakFault::LateDecode,
        PeakFault::Short,
        PeakFault::WrongRate,
        PeakFault::Empty,
        PeakFault::Uneven,
        PeakFault::Nonfinite,
        PeakFault::Silent,
    ] {
        let mut s = demo();
        s.execute("clip.audioGain", json!({"clips":[30],"mode":"set","db":12})).unwrap();
        peak_source(&mut s, fault);
        preserve_redo(&mut s);
        for mode in ["normalizeAll", "normalizeMax"] {
            rejected(&mut s, "clip.audioGain", json!({"clips":[30],"mode":mode,"db":-6}));
        }
        if !matches!(fault, PeakFault::Silent) {
            rejected(&mut s, "clip.audioPeak", json!({"clips":[30]}));
        }
    }
}

#[test]
fn audio_peak_reports_unavailable_originals_instead_of_measuring_playback_silence() {
    for offline in [true, false] {
        let mut s = demo();
        let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(30)).unwrap().1.item;
        let m = std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap();
        m.media = filmcraft_project::MediaRef::File { path: "/nonexistent-poema-generated-audio-peak-source.wav".into() };
        m.offline = offline;
        preserve_redo(&mut s);
        rejected(&mut s, "clip.audioPeak", json!({"clips":[30]}));
        rejected(&mut s, "clip.audioGain", json!({"clips":[30],"mode":"normalizeAll","db":-6}));
    }
}

#[test]
fn audio_peak_and_normalization_use_the_renderers_routed_source_channels() {
    let mut s = demo();
    peak_source(&mut s, PeakFault::SplitChannels);
    let sid = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(filmcraft_project::ClipId(30)).unwrap().1.source_channels = vec![1];
    let peak = s.execute("clip.audioPeak", json!({"clips":[30]})).unwrap();
    assert!((peak["peakDb"].as_f64().unwrap() + 20.0).abs() < 1e-5, "{peak}");
    assert_one_step(&mut s, "clip.audioGain", json!({"clips":[30],"mode":"normalizeAll","db":-6}));
    let peak = s.execute("clip.audioPeak", json!({"clips":[30]})).unwrap();
    assert!((peak["peakDb"].as_f64().unwrap() + 6.0).abs() < 1e-5, "{peak}");
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(filmcraft_project::ClipId(30)).unwrap().1.source_channels = vec![0, 99];
    preserve_redo(&mut s);
    rejected(&mut s, "clip.audioPeak", json!({"clips":[30]}));
    rejected(&mut s, "clip.audioGain", json!({"clips":[30],"mode":"normalizeAll","db":-6}));
}

#[test]
fn audio_peak_rejects_hostile_clocks_and_sample_rates_without_panicking() {
    for (speed, duration, source_in, rate) in [
        (f64::NAN, 1, 0, 48000),
        (f64::INFINITY, 1, 0, 48000),
        (-1.0, 1, 0, 48000),
        (0.0, 1, 0, 48000),
        (1.0, 0, 0, 48000),
        (1.0, i64::MAX, 0, 48000),
        (1.0, 1, i64::MIN, 48000),
        (1.0, 1, i64::MAX, 48000),
        (1.0, 1, 0, 0),
        (1.0, 1, 0, u32::MAX),
    ] {
        let mut s = demo();
        let sid = s.state.active_sequence.unwrap();
        let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
        q.settings.sample_rate = rate;
        let clip = q.find_item_mut(filmcraft_project::ClipId(30)).unwrap().1;
        clip.speed = speed;
        clip.duration = filmcraft_time::Tick(duration);
        clip.source_in = filmcraft_time::Tick(source_in);
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::mixer::clip_peaks(&s, &[filmcraft_project::ClipId(30)])));
        assert!(read.is_ok(), "hostile peak clock panicked");
        assert!(read.unwrap().is_err(), "hostile peak clock was accepted");
    }
}

#[test]
fn audio_normalization_refuses_the_whole_selection_when_a_later_stream_is_unreadable() {
    let mut s = demo();
    peak_source(&mut s, PeakFault::None);
    let sid = s.state.active_sequence.unwrap();
    let second = s.active_sequence().unwrap().audio_tracks[0].items[1].id;
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(second).unwrap().1.audio_stream = 900;
    preserve_redo(&mut s);
    for mode in ["normalizeAll", "normalizeMax"] {
        rejected(&mut s, "clip.audioGain", json!({"clips":[30,second.0],"mode":mode,"db":-6}));
    }
}

#[test]
fn audio_gain_and_normalization_preserve_other_human_values_and_render_the_exact_gain() {
    let mut s = demo();
    let sid = s.state.active_sequence.unwrap();
    let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    q.audio_tracks.truncate(1);
    q.audio_tracks[0].items.truncate(1);
    q.audio_tracks[0].items[0].start = filmcraft_time::Tick::ZERO;
    let mix = |s: &Session| {
        let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
        filmcraft_render::audio::mix_sequence(&s.project, s.active_sequence().unwrap(), 0, 4800, &provider).interleaved()
    };
    let before = serde_json::to_value(&*s.project).unwrap();
    let pcm = mix(&s);
    assert!(pcm.iter().any(|x| x.abs() > 0.01));
    s.execute("clip.audioGain", json!({"clips":[29],"mode":"set","db":6})).unwrap();
    let after = serde_json::to_value(&*s.project).unwrap();
    let scaled = mix(&s);
    let factor = filmcraft_render::audio::db_to_gain(6.0);
    assert!(pcm.iter().zip(&scaled).all(|(a, b)| (a * factor - b).abs() < 1e-6));
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    assert_eq!(mix(&s), pcm);
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), after);
    assert_eq!(mix(&s), scaled);
    for (args, gain) in [(json!({"clips":[30],"db":-3,"relative":true}), 3.0), (json!({"clips":[30],"db":-4,"relative":false}), -4.0)] {
        assert_one_step(&mut s, "clip.audioGain", args);
        assert_eq!(s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(30)).unwrap().1.gain_db, gain);
    }
    assert_one_step(&mut s, "clip.audioGain", json!({"clips":[30],"mode":"normalizeAll","db":-6}));
    let pk = s.execute("clip.audioPeak", json!({"clips":[30]})).unwrap();
    assert!((pk["peakDb"].as_f64().unwrap() + 6.0).abs() < 1e-6);
    let gain = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(30)).unwrap().1.gain_db;
    let normalized = mix(&s);
    let factor = filmcraft_render::audio::db_to_gain(gain);
    // Normalization targets the clip's pre-mixer peak. Preserve the human's
    // existing track/master gain and compare its exact rendered scaling.
    assert!(pcm.iter().zip(&normalized).all(|(a, b)| (a * factor - b).abs() < 1e-6));
    let saved = s.project.to_json();
    let restored = filmcraft_project::Project::from_json(&saved).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), serde_json::to_value(&*s.project).unwrap());
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures").join(format!("gain-input-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generated.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), serde_json::to_value(&*s.project).unwrap());
    assert_eq!(mix(&reopened), normalized);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn missing_mixer_deletions_preserve_redo_and_do_not_create_lanes() {
    for (command, args) in [
        ("mixer.removeInsert", json!({"strip":"A1","slot":999})),
        ("mixer.removeSend", json!({"strip":"A1","send":999})),
        ("mixer.deleteKeyframe", json!({"strip":"A1","lane":"volume","time":0})),
    ] {
        let mut s = demo();
        preserve_redo(&mut s);
        rejected(&mut s, command, args);
        assert!(s.active_sequence().unwrap().audio_tracks[0].mixer.lanes.is_empty());
    }
}

#[test]
fn invalid_insert_choices_never_default_wrap_or_escape_the_definition() {
    let mut s = demo();
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dehummer"})).unwrap();
    preserve_redo(&mut s);
    for value in [
        json!(null),
        json!(-1),
        json!(0.5),
        json!(1.0),
        json!(4294967296_u64),
        json!(u64::MAX),
        json!(999),
        json!("not-a-choice"),
        json!(true),
        json!([]),
        json!({}),
    ] {
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"freq":value}}));
    }
}

#[test]
fn malformed_insert_controls_and_param_objects_do_not_commit_partial_settings() {
    let mut s = demo();
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dehummer"})).unwrap();
    preserve_redo(&mut s);
    for value in [json!(null), json!(1), json!("true"), json!([]), json!({})] {
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"enabled":value}));
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"postFader":value}));
        rejected(&mut s, "mixer.addInsert", json!({"strip":"A1","effect":"dehummer","postFader":value}));
    }
    for value in [json!(null), json!(1), json!(false), json!("bad"), json!([])] {
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"enabled":false,"postFader":true,"params":value}));
    }
    rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"enabled":false,"postFader":true,"params":{"gain":-12,"freq":"bad"}}));
    rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"enabled":false,"params":{"gain":-12,"unknown_param":2}}));
}

#[test]
fn insert_parameter_types_follow_definitions_not_saved_scalar_coercions() {
    let mut s = demo();
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dynamics_rack"})).unwrap();
    assert_eq!(s.active_sequence().unwrap().audio_tracks[0].effects[0].param("comp_on").unwrap().value.as_f64(), Some(1.0));
    preserve_redo(&mut s);
    for value in [json!(1), json!(0.0), json!("true"), json!(null), json!([]), json!({})] {
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"comp_on":value}}));
    }
    for value in [json!(false), json!("-12"), json!(null), json!([]), json!({})] {
        rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"comp_threshold":value}}));
    }
}

fn assert_one_step(s: &mut Session, command: &str, args: Value) {
    let before = serde_json::to_value(&*s.project).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.len();
    s.history.merge_key = None;
    s.execute(command, args).unwrap();
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = serde_json::to_value(&*s.project).unwrap();
    assert_ne!(before, after);
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), after);
}

#[test]
fn valid_insert_settings_preserve_automation_and_definition_limits() {
    let mut s = demo();
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dehummer"})).unwrap();
    s.execute("mixer.setKeyframe", json!({"strip":"A1","lane":"fx.0.gain","time":0,"value":-30})).unwrap();
    s.execute("mixer.setKeyframe", json!({"strip":"A1","lane":"fx.0.gain","time":254016000000_i64,"value":-15})).unwrap();
    let keys = serde_json::to_value(&s.active_sequence().unwrap().audio_tracks[0].effects[0].param("gain").unwrap().keyframes).unwrap();
    assert_one_step(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"enabled":false,"postFader":true,"params":{"freq":0,"gain":-1000}}));
    let e = &s.active_sequence().unwrap().audio_tracks[0].effects[0];
    assert!(!e.enabled && e.post_fader);
    assert_eq!(e.param("freq").unwrap().value.as_f64(), Some(0.0));
    assert_eq!(e.param("gain").unwrap().value.as_f64(), Some(-80.0));
    assert_eq!(serde_json::to_value(&e.param("gain").unwrap().keyframes).unwrap(), keys);
    assert_one_step(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"freq":"60 Hz","gain":1000}}));
    let e = &s.active_sequence().unwrap().audio_tracks[0].effects[0];
    assert_eq!(e.param("freq").unwrap().value.as_f64(), Some(1.0));
    assert_eq!(e.param("gain").unwrap().value.as_f64(), Some(0.0));
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dynamics_rack","postFader":false})).unwrap();
    assert_one_step(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":1,"params":{"comp_on":false,"comp_threshold":-24}}));
    assert!(s.active_sequence().unwrap().audio_tracks[0].effects[1].param("comp_on").unwrap().value.as_f64() == Some(0.0));
}

#[test]
fn valid_mixer_deletions_are_single_steps_and_save_reopen_exactly() {
    for case in ["insert", "send", "keyframe"] {
        let mut s = demo();
        let (command, args) = match case {
            "insert" => {
                s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dehummer"})).unwrap();
                ("mixer.removeInsert", json!({"strip":"A1","slot":0}))
            }
            "send" => {
                s.execute("mixer.addSubmix", json!({"name":"Generated send target"})).unwrap();
                s.execute("mixer.addSend", json!({"strip":"A1","target":"S1","levelDb":-6})).unwrap();
                ("mixer.removeSend", json!({"strip":"A1","send":0}))
            }
            _ => {
                s.execute("mixer.setKeyframe", json!({"strip":"A1","lane":"volume","time":0,"value":-12})).unwrap();
                ("mixer.deleteKeyframe", json!({"strip":"A1","lane":"volume","time":0}))
            }
        };
        assert_one_step(&mut s, command, args);
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures").join(format!("mixer-input-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join(format!("generated-{case}.fcproj"));
        s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
        let saved = serde_json::to_value(&*s.project).unwrap();
        let mut reopened = Session::default();
        reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
        assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), saved);
    }
}

#[test]
fn old_insert_missing_a_parameter_receives_the_definition_and_keeps_other_values() {
    let mut s = demo();
    s.execute("mixer.addInsert", json!({"strip":"A1","effect":"dehummer"})).unwrap();
    s.edit_sequence("Generated old insert", |q, _, _| {
        q.audio_tracks[0].effects[0].params.remove("freq");
        Ok(())
    })
    .unwrap();
    preserve_redo(&mut s);
    rejected(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"freq":4294967296_u64}}));
    assert_one_step(&mut s, "mixer.setInsert", json!({"strip":"A1","slot":0,"params":{"freq":0}}));
    let e = &s.active_sequence().unwrap().audio_tracks[0].effects[0];
    assert_eq!(e.param("freq").unwrap().value.as_f64(), Some(0.0));
    assert_eq!(e.param("gain").unwrap().value.as_f64(), Some(-40.0));
}
