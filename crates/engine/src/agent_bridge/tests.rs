use std::sync::Arc;

use filmcraft_project::ItemId;
use serde_json::{Value, json};

use crate::Session;

pub(super) fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

pub(super) fn token(n: usize) -> String {
    format!("poema-test-ticket-{n:032}")
}
pub(super) fn read(s: &mut Session, n: usize) -> Value {
    s.execute("agent.snapshot", json!({"token": token(n)})).unwrap()
}

pub(super) fn source_clock_fixture() -> Session {
    use filmcraft_project::MediaRef;
    use filmcraft_time::FrameRate;
    let mut s = setup();
    let sequence = s.state.active_sequence.unwrap();
    let rate = FrameRate::new(25, 1);
    let p = Arc::make_mut(&mut s.project);
    let q = p.sequence_mut(sequence).unwrap();
    q.settings.frame_rate = rate;
    q.video_tracks.truncate(1);
    q.video_tracks[0].items.truncate(1);
    // This fixture isolates one source clock. Removed neighbours cannot leave
    // authored demo transitions pointing at clips which no longer exist.
    q.video_tracks[0].transitions.clear();
    let c = &mut q.video_tracks[0].items[0];
    c.start = rate.tick_of(0);
    c.duration = rate.tick_of(20);
    c.source_in = rate.tick_of(100);
    c.speed = 1.0;
    c.reverse = false;
    c.enabled = true;
    c.frame_hold = None;
    c.effects.clear();
    let item = c.item;
    let source = p.item_mut(item).unwrap();
    source
        .metadata
        .insert("poema.binding.v1".into(), json!({"projectId":"p_source_clock","assetId":"a_clock","mediaId":"original","role":"original"}).to_string());
    let media = source.as_media_mut().unwrap();
    media.media = MediaRef::File { path: "/generated/source-clock.mxf".into() };
    media.offline = false;
    media.info.duration = rate.tick_of(1000);
    s.execute("poema.trackSequence", json!({})).unwrap();
    s
}
fn source_clock_request(s: &Session, n: usize, frames: Value) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0},"frames":frames})
}

#[test]
fn native_finishing_read_retains_authored_values_automatic_points_and_duplicate_effects_without_mutation() {
    let mut s = source_clock_fixture();
    let seq = s.state.active_sequence.unwrap();
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0];
    c.effects = filmcraft_project::effect::intrinsic_video();
    let mut blur = filmcraft_project::find_effect("gaussian_blur").unwrap().instance();
    blur.params.get_mut("blurriness").unwrap().value = filmcraft_project::ParamValue::Float(17.0);
    c.effects.push(blur.clone());
    blur.params.get_mut("blurriness").unwrap().value = filmcraft_project::ParamValue::Float(42.0);
    c.effects.push(blur);
    let identity = c.identity.as_ref().unwrap().id.clone();
    read(&mut s, 23000);
    let before = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone(), serde_json::to_value(&s.state).unwrap());
    let r = s
        .execute(
            "agent.readFinishing",
            json!({"token":token(23000),"scope":{"projectId":"p_source_clock","anchorSequence":seq.0},"identity":identity,"offset":0,"limit":8}),
        )
        .unwrap();
    assert_eq!(r["status"], "read");
    let fx = r["effects"].as_array().unwrap();
    let duplicates: Vec<_> = fx.iter().filter(|e| e["effect"] == "gaussian_blur").collect();
    assert_eq!(duplicates.len(), 2);
    assert_eq!(duplicates[0]["params"]["blurriness"]["value"]["Float"], 17.0);
    assert_eq!(duplicates[1]["params"]["blurriness"]["value"]["Float"], 42.0);
    assert_eq!(fx.iter().find(|e| e["effect"] == "motion").unwrap()["params"]["position"]["value"]["Vec2"]["x"], Value::Null);
    assert!(Arc::ptr_eq(&s.project, &before.0));
    assert_eq!((s.revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3));
    assert_eq!(s.journal, before.4);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.5);
}

#[test]
fn native_source_frames_use_existing_held_clip_clock_without_project_or_history_changes() {
    let mut s = source_clock_fixture();
    let seq = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].frame_hold = Some(rate.tick_of(105));
    read(&mut s, 21000);
    let before = (
        s.project.clone(),
        s.revision,
        s.saved_revision,
        s.history.undo.len(),
        s.history.redo.len(),
        s.journal.clone(),
        serde_json::to_value(&s.state).unwrap(),
    );
    let request = source_clock_request(&s, 21000, json!([0, 9]));
    let r = s.execute("agent.sourceFrames", request.clone()).unwrap();
    assert_eq!(r["frames"][0]["sourceTicks"], rate.tick_of(105).0);
    assert_eq!(r["frames"][1]["sourceTicks"], rate.tick_of(105).0);
    assert_eq!(r["frames"][0]["mapping"], "held");
    assert_eq!(s.execute("agent.sourceFrames", request).unwrap(), r);
    assert!(Arc::ptr_eq(&s.project, &before.0));
    assert_eq!((s.revision, s.saved_revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3, before.4));
    assert_eq!(s.journal, before.5);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
}

#[test]
fn native_source_frames_use_exact_fractional_start_ticks_and_half_open_clip_membership() {
    let mut s = source_clock_fixture();
    let seq = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].start = filmcraft_time::Tick(rate.tick_of(1).0 / 2);
    read(&mut s, 21001);
    let request = source_clock_request(&s, 21001, json!([1, 9, 20]));
    let r = s.execute("agent.sourceFrames", request).unwrap();
    let c = &s.project.sequence(seq).unwrap().video_tracks[0].items[0];
    for (row, frame) in r["frames"].as_array().unwrap().iter().zip([1, 9, 20]) {
        assert_eq!(row["sourceTicks"], c.source_time_at(rate.tick_of(frame)).0);
    }
    let request = source_clock_request(&s, 21001, json!([0]));
    assert!(s.execute("agent.sourceFrames", request).is_err());
}

#[test]
fn native_source_frames_refuse_malformed_scope_unsupported_clocks_and_stale_reads() {
    let mut s = source_clock_fixture();
    read(&mut s, 21002);
    let valid = source_clock_request(&s, 21002, json!([0]));
    assert_eq!(s.execute("agent.sourceFrames", valid).unwrap()["status"], "mapped");
    for frames in [json!([]), json!([-1]), json!([0.5]), json!([1, 1]), json!([9, 1]), json!([1000001]), json!([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])] {
        let request = source_clock_request(&s, 21002, frames);
        assert!(s.execute("agent.sourceFrames", request).is_err());
    }
    let mut request = source_clock_request(&s, 21002, json!([0]));
    request["scope"]["projectId"] = json!("foreign");
    assert!(s.execute("agent.sourceFrames", request).is_err());
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].speed = 0.5;
    let request = source_clock_request(&s, 21002, json!([0]));
    assert!(s.execute("agent.sourceFrames", request).is_err()); // same revision, different actual project
    read(&mut s, 21003);
    let request = source_clock_request(&s, 21003, json!([0]));
    assert!(s.execute("agent.sourceFrames", request).is_err()); // fresh but unsupported moving clock
}

#[test]
fn native_source_frames_keep_neutral_intrinsics_and_reject_unavailable_sources_without_edits() {
    for (index, mode) in ["neutral", "remap", "offline", "proxy", "duplicate", "disabled", "past-source"].iter().enumerate() {
        let mut s = source_clock_fixture();
        let seq = s.state.active_sequence.unwrap();
        let p = Arc::make_mut(&mut s.project);
        let q = p.sequence_mut(seq).unwrap();
        let c = &mut q.video_tracks[0].items[0];
        c.effects = filmcraft_project::effect::intrinsic_video();
        if *mode == "remap" {
            c.effects.iter_mut().find(|e| e.effect == "time_remap").unwrap().params.get_mut("speed").unwrap().value =
                filmcraft_project::ParamValue::Float(101.0);
        }
        if *mode == "past-source" {
            c.source_in = filmcraft_time::Tick(i64::MAX / 2);
        }
        let item = c.item;
        if *mode == "disabled" {
            q.video_tracks[0].enabled = false;
        }
        if *mode == "offline" {
            p.item_mut(item).unwrap().as_media_mut().unwrap().offline = true;
        }
        if *mode == "proxy" {
            let raw = json!({"projectId":"p_source_clock","assetId":"a_clock","mediaId":"proxy","role":"proxy"}).to_string();
            p.item_mut(item).unwrap().metadata.insert("poema.binding.v1".into(), raw);
        }
        if *mode == "duplicate" {
            let old = p.item(item).unwrap().clone();
            let raw = old.metadata["poema.binding.v1"].clone();
            let extra = p.add_item("Ambiguous source", old.label, old.kind, None);
            p.item_mut(extra).unwrap().metadata.insert("poema.binding.v1".into(), raw);
        }
        read(&mut s, 21100 + index);
        let before = (
            s.project.clone(),
            s.revision,
            s.saved_revision,
            s.history.undo.len(),
            s.history.redo.len(),
            s.journal.clone(),
            serde_json::to_value(&s.state).unwrap(),
        );
        let request = source_clock_request(&s, 21100 + index, json!([0]));
        let result = s.execute("agent.sourceFrames", request);
        assert_eq!(result.is_ok(), *mode == "neutral", "{mode}: {result:?}");
        assert!(Arc::ptr_eq(&s.project, &before.0));
        assert_eq!((s.revision, s.saved_revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3, before.4));
        assert_eq!(s.journal, before.5);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
    }
}

#[test]
fn clip_source_mapping_describes_selected_stream_and_native_channel_inputs_without_editing() {
    let mut s = setup();
    let sequence = s.state.active_sequence.unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    let project = Arc::make_mut(&mut s.project);
    let media = project.item_mut(item).unwrap().as_media_mut().unwrap();
    media.info.video.as_mut().unwrap().frame_rate = filmcraft_time::FrameRate::new(60000, 1001);
    media.info.audio_streams = vec![
        filmcraft_media::AudioStreamInfo { sample_rate: 48000, channels: 2, codec: "generated".into(), bits_per_sample: None },
        filmcraft_media::AudioStreamInfo { sample_rate: 96000, channels: 6, codec: "generated".into(), bits_per_sample: None },
    ];
    let q = project.sequence_mut(sequence).unwrap();
    q.audio_tracks[0].channels = filmcraft_project::AudioChannels::Stereo;
    let audio = &mut q.audio_tracks[0].items[0];
    audio.item = item;
    audio.audio_stream = 1;
    audio.source_channels.clear();
    let before = (
        s.project.clone(),
        s.revision,
        s.saved_revision,
        s.history.undo.len(),
        s.history.redo.len(),
        s.journal.clone(),
        serde_json::to_value(&s.state).unwrap(),
    );
    let doc = read(&mut s, 20000);
    assert_eq!(doc["capabilities"]["sourceMapping"], true);
    let source = doc["sources"].as_array().unwrap().iter().find(|r| r["item"] == item.0).unwrap();
    assert_eq!(source["sourceFrameRate"], json!({"num":60000,"den":1001}));
    assert_eq!(
        doc["sequence"]["audio"][0]["items"][0]["audioInput"],
        json!({"stream":1,"streamChannels":6,"sampleRate":96000,"trackWidth":2,"channels":[0,1,2,3,4,5]})
    );
    assert!(Arc::ptr_eq(&s.project, &before.0));
    assert_eq!((s.revision, s.saved_revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3, before.4));
    assert_eq!(s.journal, before.5);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
}

#[test]
fn clip_source_mapping_retains_mono_duplicates_and_surround_order_and_marks_bad_stream_unavailable() {
    let mut s = setup();
    let sequence = s.state.active_sequence.unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    for (index, channels, width, stream, expected) in [
        (0, vec![1], filmcraft_project::AudioChannels::Stereo, 0, json!([1, 1])),
        (1, vec![5, 4, 3, 2, 1, 0], filmcraft_project::AudioChannels::Surround51, 0, json!([5, 4, 3, 2, 1, 0])),
        (2, vec![99], filmcraft_project::AudioChannels::Stereo, 0, json!([0, 0])),
        (3, vec![], filmcraft_project::AudioChannels::Stereo, 10, Value::Null),
    ] {
        let project = Arc::make_mut(&mut s.project);
        project.item_mut(item).unwrap().as_media_mut().unwrap().info.audio_streams =
            vec![filmcraft_media::AudioStreamInfo { sample_rate: 48000, channels: 6, codec: "generated".into(), bits_per_sample: None }];
        let track = &mut project.sequence_mut(sequence).unwrap().audio_tracks[0];
        track.channels = width;
        let audio = &mut track.items[0];
        audio.item = item;
        audio.audio_stream = stream;
        audio.source_channels = channels;
        let doc = read(&mut s, 20001 + index);
        let input = &doc["sequence"]["audio"][0]["items"][0]["audioInput"];
        if expected.is_null() {
            assert!(input.is_null());
        } else {
            assert_eq!(input["channels"], expected);
        }
    }
}

#[test]
fn asynchronous_read_validation_checks_real_document_identity_without_seeking_or_history() {
    let mut s = setup();
    read(&mut s, 900);
    s.set_playhead(s.sequence_rate().tick_of(10));
    let before = (s.project.clone(), s.revision, s.saved_revision, s.history.undo.len(), s.journal.len(), serde_json::to_value(&s.state).unwrap());
    let args = json!({"token":token(900)});
    assert_eq!(s.execute("agent.validateSnapshot", args.clone()).unwrap()["status"], "current");
    assert_eq!(s.project, before.0);
    assert_eq!(s.revision, before.1);
    assert_eq!(s.saved_revision, before.2);
    assert_eq!(s.history.undo.len(), before.3);
    assert_eq!(s.journal.len(), before.4);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.5);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(901)})).is_err());
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(900),"command":"file.newProject"})).is_err());
    s.project = Arc::new((*s.project).clone());
    assert!(s.execute("agent.validateSnapshot", args).is_err(), "same path/revision does not authorize a replacement document");
}
fn first(s: &Session) -> u64 {
    s.active_sequence().unwrap().video_tracks[0].items[0].id.0
}
fn trim(clip: u64) -> Value {
    json!({"command": "timeline.trim", "params": {"clip": clip, "edge": "out", "mode": "regular", "deltaFrames": -2}})
}
pub(super) fn batch(n: usize, operations: Vec<Value>) -> Value {
    json!({"token": token(n), "label": "Poema revision", "operations": operations})
}
fn binding(s: &Session) -> Value {
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    json!({"command": "agent.bindSource", "params": {"item": item.0, "binding": {"projectId": "p_test", "assetId": "asset_canon", "mediaId": "canon_original", "role": "original"}}})
}

fn linked_setup() -> (Session, u64, u64) {
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    let v = q.video_tracks[0].items[0].clone();
    q.video_tracks[0].items[0].link = Some(900_000);
    let a = &mut q.audio_tracks[0].items[0];
    a.link = Some(900_000);
    a.start = v.start;
    a.duration = v.duration;
    let audio = a.id.0;
    (s, v.id.0, audio)
}

pub(super) fn exact_batch(n: usize, operations: Vec<Value>) -> Value {
    let mut b = batch(n, operations);
    b["exact"] = json!(true);
    b
}

#[test]
fn exact_linked_split_then_half_trim_is_not_applied_twice_and_guarded_undo_restores_each_step() {
    let (mut s, video, audio) = linked_setup();
    let original = s.project.clone();
    let rate = s.sequence_rate();
    let item = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(video)).unwrap().1;
    let frames = rate.frame_at(item.duration);
    let half = frames / 2;
    let cut = item.start + rate.tick_of(half);
    read(&mut s, 1);
    let razor = |id| json!({"command": "timeline.razor", "params": {"clip": id, "time": cut.0}});
    let receipt = s.execute("agent.apply", exact_batch(1, vec![razor(video), razor(audio)])).unwrap();
    assert_eq!(receipt["results"][1]["deduplicated"], true);
    let split = s.project.clone();
    read(&mut s, 2);
    let trim_half = |id| json!({"command": "timeline.trim", "params": {"clip": id, "edge": "out", "mode": "regular", "deltaFrames": -(half / 2)}});
    let result = s.execute("agent.apply", exact_batch(2, vec![trim_half(video), trim_half(audio)])).unwrap();
    assert_eq!(result["validated"], true);
    for id in [video, audio] {
        assert_eq!(s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(id)).unwrap().1.duration, rate.tick_of(half - half / 2));
    }
    read(&mut s, 3);
    let undo = json!({"token": token(3), "commit": token(2)});
    let result = s.execute("agent.undo", undo.clone()).unwrap();
    assert_eq!(s.project, split);
    let rev = s.revision;
    assert_eq!(s.execute("agent.undo", undo).unwrap(), result);
    assert_eq!(s.revision, rev);
    assert_eq!(s.execute("agent.receipt", json!({"token": token(3)})).unwrap()["result"], result);
    read(&mut s, 4);
    s.execute("agent.undo", json!({"token": token(4), "commit": token(1)})).unwrap();
    assert_eq!(s.project, original);
}

#[test]
fn exact_clamping_conflicting_linked_requests_and_locked_partners_publish_nothing() {
    let (mut s, video, audio) = linked_setup();
    let original = s.project.clone();
    for (n, operations) in [
        vec![json!({"command": "timeline.trim", "params": {"clip": video, "edge": "out", "mode": "regular", "deltaFrames": -1_000_000}})],
        vec![trim(video), json!({"command": "timeline.trim", "params": {"clip": audio, "edge": "out", "mode": "regular", "deltaFrames": -3}})],
        vec![json!({"command": "timeline.razor", "params": {"clip": video, "time": 0}})],
    ]
    .into_iter()
    .enumerate()
    {
        read(&mut s, n);
        let revision = s.revision;
        assert!(s.execute("agent.apply", exact_batch(n, operations)).is_err());
        assert_eq!(s.project, original);
        assert_eq!(s.revision, revision);
    }
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().audio_tracks[0].locked = true;
    read(&mut s, 10);
    assert!(s.execute("agent.apply", exact_batch(10, vec![trim(video)])).is_err());
}

#[test]
fn guarded_undo_cannot_cross_a_manual_edit_undo_redo_or_stale_context() {
    for mode in 0..5 {
        let mut s = setup();
        let clip = first(&s);
        read(&mut s, 1);
        s.execute("agent.apply", exact_batch(1, vec![trim(clip)])).unwrap();
        if mode == 4 {
            read(&mut s, 2);
            s.state.linked_selection = !s.state.linked_selection;
        } else {
            match mode {
                0 => {
                    s.execute("timeline.trim", trim(clip)["params"].clone()).unwrap();
                }
                1 => {
                    s.execute("edit.undo", json!({})).unwrap();
                }
                2 => {
                    s.execute("edit.undo", json!({})).unwrap();
                    s.execute("edit.redo", json!({})).unwrap();
                }
                _ => {
                    Arc::make_mut(&mut s.project).notes = "Manual note".into();
                }
            }
            read(&mut s, 2);
        }
        let (project, revision) = (s.project.clone(), s.revision);
        assert!(s.execute("agent.undo", json!({"token": token(2), "commit": token(1)})).is_err());
        assert_eq!(s.project, project);
        assert_eq!(s.revision, revision);
    }
}

#[test]
fn recovered_batches_and_undo_keep_the_same_history_granularity() {
    let mut s = setup();
    let clip = first(&s);
    let original = s.project.clone();
    let base_journal = s.journal.len();
    read(&mut s, 1);
    s.execute("agent.apply", batch(1, vec![trim(clip), trim(clip)])).unwrap();
    read(&mut s, 2);
    s.execute("agent.undo", json!({"token": token(2), "commit": token(1)})).unwrap();
    let recovery = s.journal[base_journal..].to_vec();
    assert_eq!(recovery.len(), 2);
    assert_eq!(recovery[0].0, "agent.replayBatch");
    assert_eq!(recovery[1].0, "edit.undo");
    let mut recovered = setup();
    for (command, params) in recovery {
        recovered.execute(&command, params).unwrap();
    }
    assert_eq!(recovered.project, original);
    assert!(recovered.execute("agent.replayBatch", json!({"label":"bad", "operations":[{"command":"file.newProject", "params":{}}]})).is_err());
}

#[test]
fn snapshot_is_one_read_and_fresh_session_rejects_old_ticket() {
    let mut s = setup();
    let doc = read(&mut s, 1);
    assert_eq!(doc["revision"], s.revision);
    assert_eq!(doc["sequence"]["id"], s.state.active_sequence.unwrap().0);
    assert!(!doc["sources"].as_array().unwrap().is_empty());
    assert!(s.execute("agent.snapshot", json!({"token": token(1)})).is_err());
    let mut other = setup();
    let request = batch(1, vec![trim(first(&other))]);
    assert!(other.execute("agent.apply", request).unwrap_err().to_string().contains("unknown/expired"));
}

#[test]
fn multiple_edits_publish_once_and_undo_redo_together() {
    let mut s = setup();
    // Rich/unknown effect payloads are retained from the document, never reconstructed from
    // sequence.inspect (which exposes debug values and keyframe/mask counts, not their data).
    let seq = s.state.active_sequence.unwrap();
    let mut fx = filmcraft_project::EffectInstance {
        effect: "poema.test.opaque-effect".into(),
        instance_id: None,
        enabled: false,
        params: Default::default(),
        masks: vec![filmcraft_project::mask::Mask::new(
            "Human mask",
            filmcraft_project::mask::MaskPath::ellipse(filmcraft_geom::Vec2::new(30.0, 40.0), filmcraft_geom::Vec2::new(10.0, 12.0)),
        )],
        post_fader: false,
        essential: false,
        layer: None,
    };
    let mut param = filmcraft_project::Param::new(filmcraft_project::ParamValue::Text("Human decision".into()));
    param.keyframes.push(filmcraft_project::Keyframe::new(s.sequence_rate().tick_of(2), filmcraft_project::ParamValue::Text("Keep this".into())));
    fx.params.insert("opaque".into(), param);
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].effects.push(fx.clone());
    read(&mut s, 1);
    let (old, history, rev) = (s.project.clone(), s.history.undo.len(), s.revision);
    let clip = first(&s);
    let bind = binding(&s);
    let events = s.events.len();
    let request = batch(1, vec![bind, trim(clip), trim(clip)]);
    let receipt = s.execute("agent.apply", request.clone()).unwrap();
    assert_eq!(s.revision, rev + 1);
    assert_eq!(s.history.undo.len(), history + 1);
    assert_eq!(s.events.len(), events + 1);
    assert_eq!(receipt["results"].as_array().unwrap().len(), 3);
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].items[0].effects.last().unwrap(), &fx);
    let edited = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, old);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, edited);
    // Exactly-once replay returns the original receipt, not a new edit, even after undo/redo.
    let rev = s.revision;
    assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
    assert_eq!(s.revision, rev);
    assert!(s.execute("agent.apply", batch(1, vec![trim(clip)])).is_err());
    assert_eq!(s.execute("agent.receipt", json!({"token": token(1)})).unwrap()["result"], receipt);
}

#[test]
fn failure_after_successful_staged_edit_changes_nothing_live() {
    let mut s = setup();
    read(&mut s, 2);
    let old = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let (undo, redo, revision, saved, events, journal) =
        (s.history.undo.len(), s.history.redo.len(), s.revision, s.saved_revision, s.events.clone(), s.journal.clone());
    let request = batch(2, vec![binding(&s), trim(first(&s)), trim(999_999)]);
    assert!(s.execute("agent.apply", request).is_err());
    assert!(Arc::ptr_eq(&s.project, &old));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!((s.history.undo.len(), s.history.redo.len(), s.revision, s.saved_revision), (undo, redo, revision, saved));
    assert_eq!(s.events, events);
    assert_eq!(s.journal, journal);
    assert_eq!(s.execute("agent.receipt", json!({"token": token(2)})).unwrap()["status"], "unapplied");
}

#[test]
fn human_edit_undo_project_replacement_and_context_change_invalidate_read() {
    let mut s = setup();
    for n in 1..=7 {
        read(&mut s, n);
        let clip = first(&s);
        match n {
            1 => {
                s.execute("timeline.trim", trim(clip)["params"].clone()).unwrap();
            }
            2 => {
                s.execute("edit.undo", json!({})).unwrap();
            }
            3 => {
                s.state.linked_selection = !s.state.linked_selection;
            }
            4 => {
                s.state.playheads.insert(s.state.active_sequence.unwrap(), s.sequence_rate().tick_of(12));
            }
            5 => {
                s.execute("file.openDemoProject", json!({})).unwrap();
            }
            6 => {
                s.path = Some("/scratch/renamed.fcproj".into());
            }
            _ => {
                Arc::make_mut(&mut s.project).notes = "edited outside the command's revision counter".into();
            }
        }
        let (project, revision) = (s.project.clone(), s.revision);
        assert!(s.execute("agent.apply", batch(n, vec![trim(clip)])).unwrap_err().to_string().contains("stale snapshot"));
        assert!(Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.revision, revision);
    }
}

#[test]
fn unsafe_operations_missing_targets_and_unbounded_inputs_are_refused() {
    let mut s = setup();
    let clip = first(&s);
    read(&mut s, 1);
    let old = s.project.clone();
    for operation in [
        json!({"command": "file.saveAs", "params": {"path": "/must-not-write"}}),
        json!({"command": "file.newProject", "params": {}}),
        json!({"command": "edit.undo", "params": {}}),
        json!({"command": "agent.apply", "params": {}}),
        json!({"command": "timeline.trim", "params": {"edge": "out", "mode": "regular", "deltaFrames": -2}}),
        json!({"command": "timeline.trim", "params": {"clip": clip, "edge": "out", "mode": "regular", "deltaFrames": i64::MIN}}),
        json!({"command": "timeline.razor", "params": {"clip": clip, "time": i64::MAX}}),
    ] {
        assert!(s.execute("agent.apply", batch(1, vec![trim(clip), operation])).is_err());
        assert!(Arc::ptr_eq(&s.project, &old));
    }
    assert!(s.execute("agent.apply", batch(1, vec![trim(clip); 33])).is_err());
    assert!(s.execute("agent.apply", batch(1, vec![])).is_err());
}

#[test]
fn source_correspondence_survives_split_duplicate_serialization_and_undo() {
    let mut s = setup();
    read(&mut s, 1);
    let bind = binding(&s);
    let source = ItemId(bind["params"]["item"].as_u64().unwrap());
    let clip = first(&s);
    let start = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.start;
    let time = start + s.sequence_rate().tick_of(12);
    s.execute("agent.apply", batch(1, vec![bind.clone(), json!({"command": "timeline.razor", "params": {"clip": clip, "time": time.0}})])).unwrap();
    let q = s.active_sequence().unwrap();
    let parts: Vec<_> = q.video_tracks.iter().flat_map(|t| &t.items).filter(|i| i.item == source).collect();
    assert!(parts.len() >= 2);
    assert_ne!(parts[0].id, parts[1].id);
    let part_count = parts.len();
    // A real Option-drag copy keeps the explicit source id, never inferred from its name.
    let destination = q.duration() + s.sequence_rate().tick_of(24);
    let copied = s.execute("timeline.move", json!({"moves": [{"clip": clip, "track": "V1", "time": destination.0}], "copy": true, "insert": false})).unwrap();
    assert!(!copied["copied"].as_array().unwrap().is_empty());
    let q = s.active_sequence().unwrap();
    let copies = q.video_tracks.iter().flat_map(|t| &t.items).filter(|i| i.item == source).count();
    assert!(copies > part_count);
    let bytes = filmcraft_format::encode_with_view(&s.project, None, false);
    let loaded = filmcraft_format::decode(&bytes).unwrap();
    assert_eq!(loaded.project, *s.project);
    let raw = &loaded.project.item(source).unwrap().metadata[super::BINDING_KEY];
    assert_eq!(serde_json::from_str::<Value>(raw).unwrap(), bind["params"]["binding"]);
    let mut different = bind["params"].clone();
    different["binding"]["assetId"] = json!("wrong_asset");
    assert!(s.execute("agent.bindSource", different).is_err());
    s.execute("edit.undo", json!({})).unwrap(); // copy
    s.execute("edit.undo", json!({})).unwrap(); // binding + split batch
    assert!(!s.project.item(source).unwrap().metadata.contains_key(super::BINDING_KEY));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.project.item(source).unwrap().metadata.contains_key(super::BINDING_KEY));
}

#[test]
fn bounded_receipts_expire_closed_and_malformed_binding_is_visible() {
    let mut s = setup();
    for n in 0..=super::TICKET_LIMIT {
        read(&mut s, n);
    }
    assert_eq!(s.agent_bridge.tickets.len(), super::TICKET_LIMIT);
    assert_eq!(s.execute("agent.receipt", json!({"token": token(0)})).unwrap()["status"], "unknown");
    assert!(s.execute("agent.apply", batch(0, vec![trim(first(&s))])).is_err());
    let item = ItemId(binding(&s)["params"]["item"].as_u64().unwrap());
    Arc::make_mut(&mut s.project).items.get_mut(&item).unwrap().metadata.insert(super::BINDING_KEY.into(), "broken".into());
    assert!(s.execute("agent.snapshot", json!({"token": token(999)})).is_err());
}
