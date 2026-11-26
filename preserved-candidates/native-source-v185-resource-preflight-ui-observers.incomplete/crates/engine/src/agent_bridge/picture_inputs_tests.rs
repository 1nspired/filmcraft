use super::tests::{read, token};
use crate::Session;
use filmcraft_project::{ClipId, MediaRef};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn fixture() -> Session {
    let (mut s, item) = super::assembly_tests::fixture();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let source = p.item_mut(filmcraft_project::ItemId(item)).unwrap().as_media_mut().unwrap();
    source.media = MediaRef::File { path: "/generated/picture-inputs.mov".into() };
    source.info.duration = Tick(1_000_000);
    let q = p.sequence_mut(seq).unwrap();
    for t in q.all_tracks_mut() {
        t.items.truncate(1);
        let c = &mut t.items[0];
        c.start = Tick(100);
        c.source_in = Tick(1000);
        c.duration = Tick(100);
        c.effects.clear();
        c.frame_hold = None;
        c.speed = 1.0;
        c.reverse = false;
    }
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn request(s: &Session, n: usize, windows: Value) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "dialogueTrack":s.active_sequence().unwrap().video_tracks[0].id.0,"pictureTrack":null,"bites":windows})
}
fn bite(key: &str, a: i64, z: i64) -> Value {
    json!({"key":key.repeat(64),"assetId":"a_source","sourceInTicks":a,"sourceOutTicks":z})
}
fn measure(s: &mut Session, n: usize, windows: Value) -> Value {
    read(s, n);
    let r = request(s, n, windows);
    let before = state(s);
    let result = s.execute("agent.pictureInputs", r).unwrap();
    assert_eq!(state(s), before);
    result
}
#[test]
fn current_trimmed_instances_intersect_source_choices_and_union_overlapping_bites_without_history_or_speech_claims() {
    let mut s = fixture();
    let r = measure(&mut s, 88000, json!([bite("a", 1020, 1080), bite("b", 1050, 1090)]));
    assert_eq!(r["bites"][0]["pictureRanges"], json!([[120, 180]]));
    assert_eq!(r["bites"][1]["pictureRanges"], json!([[150, 190]]));
    assert_eq!(r["pictureRanges"], json!([[120, 190]]));
    assert_eq!(r["pictureTicks"], 70);
    assert_eq!(r["linkedAudioTicks"], 70);
    assert_eq!(r["semanticVerification"], false);
    assert_eq!(r["signalVerification"], false);
    let audio = &r["bites"][0]["pieces"][0]["linkedAudio"][0];
    assert_eq!(audio["input"]["channels"], json!([0, 0]));
    assert_eq!(audio["binding"]["assetId"], "a_source");
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    q.video_tracks[0].items[0].source_in = Tick(1050);
    q.video_tracks[0].items[0].duration = Tick(25);
    let trimmed = measure(&mut s, 88001, json!([bite("a", 1020, 1080)]));
    assert_eq!(trimmed["pictureRanges"], json!([[100, 125]]));
    assert_eq!(trimmed["pictureTicks"], 25);
    assert_ne!(trimmed["basisSha256"], r["basisSha256"]);
}
#[test]
fn discrete_native_clock_intersection_matches_every_tick_for_fractional_speed_reverse_holds_and_fractional_starts() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    for speed in [0.4, 0.8, 1.25, 2.0] {
        for reverse in [false, true] {
            for hold in [None, Some(Tick(1037))] {
                let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
                let c = &mut q.video_tracks[0].items[0];
                c.start = Tick(101);
                c.speed = speed;
                c.reverse = reverse;
                c.frame_hold = hold;
                for window in [[1000, 1001], [1020, 1060], [1037, 1038], [1050, 1100], [1200, 1201]] {
                    let expected: Vec<_> = (c.start.0..c.end().0)
                        .filter(|t| {
                            let source = c.source_time_at(Tick(*t)).0;
                            window[0] <= source && source < window[1]
                        })
                        .collect();
                    let actual = super::picture_inputs::overlap(c, window);
                    assert_eq!(
                        actual,
                        expected.first().zip(expected.last()).map(|(a, z)| [*a, z + 1]),
                        "speed {speed} reverse {reverse} hold {hold:?} window {window:?}"
                    );
                }
            }
        }
    }
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    let c = &mut q.video_tracks[0].items[0];
    c.frame_hold = None;
    c.speed = 1.25;
    c.reverse = true;
    let expected = super::picture_inputs::overlap(c, [1020, 1060]).unwrap();
    let r = measure(&mut s, 88002, json!([bite("a", 1020, 1060)]));
    assert_eq!(r["pictureRanges"], json!([expected]));
}
#[test]
fn split_pieces_gaps_shorter_linked_sound_and_disabled_mixer_state_are_complete_authored_geometry() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let next = s.project.next_id;
    s.project = Arc::new((*s.project).clone());
    let p = Arc::make_mut(&mut s.project);
    p.next_id += 1;
    let q = p.sequence_mut(seq).unwrap();
    let mut second = q.video_tracks[0].items[0].clone();
    second.id = ClipId(next);
    second.identity.as_mut().unwrap().id = "c".repeat(32);
    second.source_in = Tick(1060);
    second.start = Tick(200);
    second.duration = Tick(20);
    second.link = None;
    q.video_tracks[0].items[0].duration = Tick(30);
    q.video_tracks[0].items.push(second);
    q.audio_tracks[0].items[0].start = Tick(110);
    q.audio_tracks[0].items[0].duration = Tick(10);
    q.audio_tracks[0].muted = true;
    q.audio_tracks[0].items[0].enabled = false;
    let r = measure(&mut s, 88003, json!([bite("a", 1000, 1100)]));
    assert_eq!(r["pictureRanges"], json!([[100, 130], [200, 220]]));
    assert_eq!(r["pictureTicks"], 50);
    assert_eq!(r["linkedAudioRanges"], json!([[110, 120]]));
    assert_eq!(r["linkedAudioTicks"], 10);
    let audio = &r["bites"][0]["pieces"][0]["linkedAudio"][0];
    assert_eq!(audio["track"]["muted"], true);
    assert_eq!(audio["authored"]["enabled"], false);
    assert_eq!(r["signalVerification"], false);
}
#[test]
fn stale_or_foreign_reads_and_hostile_unproven_clock_or_binding_requests_refuse_without_touching_the_cut() {
    for case in ["duplicate", "foreign", "unsafe", "zero", "unknown", "same-track", "remix", "nan", "duplicate-source", "bad-stream", "untracked"] {
        let mut s = fixture();
        let seq = s.state.active_sequence.unwrap();
        let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
        match case {
            "remix" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].effects.push({
                let mut effect = filmcraft_project::find_effect("time_remap").unwrap().instance();
                effect.effect = "remix".into();
                effect
            }),
            "nan" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].speed = f64::NAN,
            "bad-stream" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().audio_tracks[0].items[0].audio_stream = usize::MAX,
            "untracked" => Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].identity = None,
            "duplicate-source" => {
                let p = Arc::make_mut(&mut s.project);
                let mut it = p.item(source).unwrap().clone();
                it.id = filmcraft_project::ItemId(p.next_id);
                p.next_id += 1;
                p.items.insert(it.id, it);
            }
            _ => {}
        }
        // Some invalid project carriers are rejected already by the existing
        // snapshot. Either boundary must preserve the complete current state.
        let before = state(&s);
        if s.execute("agent.snapshot", json!({"token":token(88004)})).is_err() {
            assert_eq!(state(&s), before);
            continue;
        }
        let mut req = request(&s, 88004, json!([bite("a", 1000, 1100)]));
        match case {
            "duplicate" => req["bites"] = json!([bite("a", 1000, 1050), bite("a", 1050, 1100)]),
            "foreign" => req["scope"]["projectId"] = json!("another_film"),
            "unsafe" => req["bites"][0]["sourceOutTicks"] = json!(i64::MAX),
            "zero" => req["bites"][0]["sourceOutTicks"] = json!(1000),
            "unknown" => req["untrusted"] = json!(true),
            "same-track" => req["pictureTrack"] = req["dialogueTrack"].clone(),
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute("agent.pictureInputs", req).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
    let mut s = fixture();
    read(&mut s, 88005);
    let r = request(&s, 88005, json!([bite("a", 1000, 1100)]));
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].name = "Manual change".into();
    let before = state(&s);
    assert!(s.execute("agent.pictureInputs", r).is_err());
    assert_eq!(state(&s), before);
}
