use super::{
    assembly_tests, captions, music, source_edit,
    tests::{exact_batch, read, token},
    tracks,
};
use crate::Session;
use filmcraft_project::{AudioChannelMap, AudioChannels, ItemId, MediaRef};
use filmcraft_time::{TICKS_PER_SECOND as T, Tick};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(710_000);
struct InputFile(PathBuf);
impl Drop for InputFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn fixture() -> (Session, u64, InputFile) {
    let (mut s, item) = assembly_tests::fixture();
    let file = std::env::temp_dir().join(format!("poema_native_bed_{}_{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&file, b"Complete generated original input bytes").unwrap();
    let m = Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap();
    m.media = MediaRef::File { path: file.to_str().unwrap().into() };
    m.info.duration = Tick(T * 7 + 123);
    m.info.audio_streams[0].channels = 2;
    (s, item, InputFile(file))
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn geometry(s: &mut Session, item: u64, anchor: Value) -> Value {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    let before = state(s);
    let r=s.execute("agent.readMusicDestination",json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"item":item,"assetId":"a_source","anchor":anchor})).unwrap();
    assert_eq!(state(s), before);
    r
}
fn op(s: &mut Session, item: u64, anchor: Value, targets: Vec<Option<u64>>) -> Value {
    let r = geometry(s, item, anchor.clone());
    let q = s.active_sequence().unwrap();
    json!({"command":"agent.placeMusic","params":{"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"beforeStateSha256":source_edit::digest(s).unwrap(),"beforeSha256":tracks::digest(q).unwrap(),"beforeCaptionSha256":captions::digest(&q.caption_tracks).unwrap(),"beforeRolesSha256":r["rolesSha256"],"placementDefaultsSha256":source_edit::placement_defaults(s).unwrap(),"originalPlanSha256":"a".repeat(64),"originalReceiptSha256":"b".repeat(64),"item":item,"assetId":"a_source","mediaId":"m_original","anchor":anchor,"requestedAtTicks":r["geometry"]["requestedAtTicks"],"nativeStartTicks":r["geometry"]["nativeStartTicks"],"nativeDurationTicks":r["geometry"]["nativeDurationTicks"],"targets":targets,"songRole":"love","gainDb":-12.0}})
}
fn apply(s: &mut Session, op: Value) -> Value {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![op])).unwrap()
}
fn reject(s: &mut Session, op: Value) {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    let before = state(s);
    assert!(s.execute("agent.apply", exact_batch(n, vec![op])).is_err());
    assert_eq!(state(s), before);
}
fn fork(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.media = s.media.clone();
    h.state = s.state.clone();
    h.prefs = s.prefs.clone();
    h.revision = s.revision;
    h
}
#[test]
fn music_placement_uses_human_boundaries_gain_and_tracks_as_one_native_history_step() {
    let (mut s, item, _file) = fixture();
    let mut human = fork(&s);
    let before = s.project.to_json();
    let rev = s.revision;
    let undo = s.history.undo.len();
    let operation = op(&mut s, item, json!({"kind":"explicit","ticks":T+7}), vec![None]);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    let receipt = s.execute("agent.apply", exact_batch(n, vec![operation])).unwrap();
    let result = &receipt["results"][0];
    assert_eq!(s.revision, rev + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(receipt["action"], "music_placement");
    assert_eq!(result["humanCommands"], json!(["sequence.addTracks", "timeline.place", "clip.audioGain"]));
    assert_eq!(result["outcomes"].as_array().unwrap().len(), 1);

    human.execute("sequence.addTracks", json!({"video":0,"audio":1,"submix":0})).unwrap();
    let dest = human.active_sequence().unwrap().audio_tracks[1].id.0;
    let ids = human
        .execute("timeline.place", json!({"item":item,"track":dest,"time":T+7,"sourceIn":0,"duration":T*7+123,"video":false,"audio":true,"insert":false}))
        .unwrap()["clips"]
        .clone();
    human.execute("clip.audioGain", json!({"clips":ids,"mode":"set","db":-12})).unwrap();
    let actual = &s.active_sequence().unwrap().audio_tracks[1].items[0];
    let expected = &human.active_sequence().unwrap().audio_tracks[1].items[0];
    assert_eq!(
        (actual.start, actual.duration, actual.source_in, actual.gain_db, actual.audio_stream, &actual.source_channels),
        (expected.start, expected.duration, expected.source_in, expected.gain_db, expected.audio_stream, &expected.source_channels)
    );
    assert_eq!(result["geometry"]["tailDeltaTicks"], expected.duration.0 - (T * 7 + 123));
    assert!(actual.link.is_none());
    let after = s.project.to_json();
    let undo = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, undo);
    s.execute("agent.undo", json!({"token":token(undo),"commit":token(n)})).unwrap();
    assert_eq!(s.project.to_json(), before);
    let m = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, m);
    s.execute("agent.redo", json!({"token":token(m),"commit":token(n),"undo":token(undo)})).unwrap();
    assert_eq!(s.project.to_json(), after);
}
#[test]
fn music_placement_preserves_every_current_channel_map_and_container_stream_without_borrowing_camera_links() {
    let (mut s, item, _file) = fixture();
    let media = Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap();
    media.interpret.audio_channels = Some(AudioChannelMap { format: AudioChannels::Mono, clips: vec![vec![0], vec![1]] });
    let second = media.info.audio_streams[0].clone();
    media.info.audio_streams.push(second);
    let before = s.active_sequence().unwrap().clone();
    let operation = op(&mut s, item, json!({"kind":"start"}), vec![None, None, None]);
    let r = apply(&mut s, operation);
    assert_eq!(r["results"][0]["outcomes"].as_array().unwrap().len(), 3);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks, before.video_tracks);
    assert_eq!(q.audio_tracks[0], before.audio_tracks[0]);
    let clips = q.audio_tracks.iter().skip(1).map(|t| &t.items[0]).collect::<Vec<_>>();
    assert_eq!(clips.iter().map(|c| c.audio_stream).collect::<Vec<_>>(), vec![0, 0, 1]);
    assert_eq!(clips[0].source_channels, vec![0]);
    assert_eq!(clips[1].source_channels, vec![1]);
    assert!(clips[2].source_channels.is_empty());
    let group = clips[0].link.unwrap();
    assert!(clips.iter().all(|c| c.link == Some(group) && c.gain_db == -12.0));
    assert!(!q.video_tracks.iter().flat_map(|t| &t.items).any(|c| c.link == Some(group)));
}
#[test]
fn music_reuses_free_existing_lanes_and_preserves_other_authored_state() {
    let (mut s, item, _file) = fixture();
    s.execute("sequence.addTracks", json!({"video":0,"audio":1,"submix":0})).unwrap();
    let dest = s.active_sequence().unwrap().audio_tracks[1].id.0;
    let before = s.active_sequence().unwrap().clone();
    let operation = op(&mut s, item, json!({"kind":"start"}), vec![Some(dest)]);
    let r = apply(&mut s, operation);
    assert_eq!(r["results"][0]["createdTracks"], 0);
    assert_eq!(r["results"][0]["humanCommands"], json!(["timeline.place", "clip.audioGain"]));
    let q = s.active_sequence().unwrap();
    assert_eq!(q.audio_tracks.len(), before.audio_tracks.len());
    assert_eq!(q.video_tracks, before.video_tracks);
    assert_eq!(q.audio_tracks[0], before.audio_tracks[0]);
    assert_eq!(q.caption_tracks, before.caption_tracks);
    assert_eq!(q.markers, before.markers);
    assert_eq!(music::summary(&s)["roles"][dest.to_string()], "music");
}
#[test]
fn music_source_event_anchor_uses_current_native_clock_first_occurrence_and_original_rounding_then_remeasures_on_apply() {
    let (mut s, item, _file) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0];
    c.source_in = Tick(T);
    c.duration = Tick(T / 2);
    c.speed = 0.5;
    let anchor = json!({"kind":"source_event","assetId":"a_source","sourceTicks":T+T/8});
    let g = geometry(&mut s, item, anchor.clone());
    // The discrete 50% source clock reaches this integer threshold one tick
    // before the algebraic inverse. Use playback itself as the lower-bound oracle.
    let at = g["geometry"]["pivot"]["recordTicks"].as_i64().unwrap();
    assert_eq!(at, T / 4 - 1);
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    assert!(c.source_time_at(Tick(at)).0 >= T + T / 8);
    assert!(c.source_time_at(Tick(at - 1)).0 < T + T / 8);
    assert_eq!(g["geometry"]["requestedAtTicks"], 0);
    assert_eq!(g["geometry"]["anchorChoice"], "kiss");
    let operation = op(&mut s, item, anchor, vec![None]);
    let mut wrong = operation.clone();
    wrong["params"]["requestedAtTicks"] = json!(T);
    reject(&mut s, wrong);
    let r = apply(&mut s, operation);
    assert_eq!(r["results"][0]["geometry"], g["geometry"]);
}
#[test]
fn music_missing_or_trimmed_out_events_use_original_structural_fallback_and_held_clocks_refuse() {
    let (mut s, item, _file) = fixture();
    for asset in ["a_source", "a_absent"] {
        let g = geometry(&mut s, item, json!({"kind":"source_event","assetId":asset,"sourceTicks":T*6}));
        assert_eq!(g["geometry"]["anchorChoice"], "back-half");
        assert!(g["geometry"]["pivot"].is_null());
    }
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].frame_hold = Some(Tick(0));
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    assert!(s.execute("agent.readMusicDestination",json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":seq.0},"item":item,"assetId":"a_source","anchor":{"kind":"source_event","assetId":"a_source","sourceTicks":0}})).is_err());
}
#[test]
fn malformed_stale_overlapping_foreign_and_partial_music_requests_refuse_without_live_changes() {
    for case in [
        "scratch",
        "locked",
        "muted",
        "disabled",
        "role",
        "occupied",
        "duplicate",
        "partial",
        "foreign",
        "source",
        "gain",
        "negative",
        "clock",
        "extra",
        "stale",
    ] {
        let (mut s, item, _file) = fixture();
        s.execute("sequence.addTracks", json!({"video":0,"audio":1,"submix":0})).unwrap();
        let seq = s.state.active_sequence.unwrap();
        let dest = s.active_sequence().unwrap().audio_tracks[1].id.0;
        match case {
            "locked" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().audio_tracks[1].locked = true,
            "muted" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().audio_tracks[1].muted = true,
            "disabled" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().audio_tracks[1].enabled = false,
            "role" => {
                Arc::make_mut(&mut s.project)
                    .item_mut(seq)
                    .unwrap()
                    .metadata
                    .insert("poema.audio.roles.v1".into(), json!({dest.to_string():"dialogue"}).to_string());
            }
            "occupied" => {
                s.execute("timeline.place", json!({"item":item,"track":dest,"time":0,"duration":T,"video":false,"audio":true})).unwrap();
            }
            _ => {}
        }
        let mut operation = op(&mut s, item, json!({"kind":"start"}), vec![Some(dest)]);
        match case {
            "scratch" => operation["params"]["targets"] = json!([s.active_sequence().unwrap().audio_tracks[0].id.0]),
            "duplicate" => operation["params"]["targets"] = json!([dest, dest]),
            "partial" => operation["params"]["targets"] = json!([]),
            "foreign" => operation["params"]["scope"]["projectId"] = json!("p_other"),
            "source" => operation["params"]["mediaId"] = json!("wrong"),
            "gain" => operation["params"]["gainDb"] = json!(1000),
            "negative" => operation["params"]["nativeStartTicks"] = json!(-1),
            "clock" => operation["params"]["nativeDurationTicks"] = json!(1),
            "extra" => operation["params"]["paths"] = json!(["/foreign"]),
            "stale" => operation["params"]["beforeRolesSha256"] = json!("0".repeat(64)),
            _ => {}
        }
        reject(&mut s, operation);
    }
}
#[test]
fn music_read_and_publication_require_source_coverage_even_for_an_unused_take() {
    let (mut s, item, _file) = fixture();
    let operation = op(&mut s, item, json!({"kind":"start"}), vec![None]);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    let empty = super::media_guard::capture_scoped(&s.project, &s.media, vec![]).unwrap();
    s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(n)).unwrap().media = empty;
    let before = state(&s);
    assert!(s.execute("agent.apply", exact_batch(n, vec![operation])).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn music_staging_honors_stop_between_human_services_without_mutating_the_live_session() {
    let (mut s, item, _file) = fixture();
    let operation = op(&mut s, item, json!({"kind":"start"}), vec![None]);
    let op: super::Operation = serde_json::from_value(operation).unwrap();
    let before = state(&s);
    let mut stage = fork(&s);
    let calls = std::cell::Cell::new(0);
    assert!(
        music::run(&mut stage, &op, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 4 { Err(crate::commands::bad("test", "stopped")) } else { Ok(()) }
        })
        .is_err()
    );
    assert_eq!(state(&s), before);
    assert_eq!(calls.get(), 4);
}

#[test]
fn music_role_carrier_rejects_duplicate_or_noncanonical_keys_without_poisoning_general_native_reads() {
    for raw in [r#"{"1":"music","1":"dialogue"}"#, r#"{ "1" : "music" }"#] {
        let (mut s, item, _file) = fixture();
        let operation = op(&mut s, item, json!({"kind":"start"}), vec![None]);
        let seq = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).item_mut(seq).unwrap().metadata.insert("poema.audio.roles.v1".into(), raw.into());
        assert_eq!(music::summary(&s)["status"], "unavailable");
        reject(&mut s, operation);
    }
}
