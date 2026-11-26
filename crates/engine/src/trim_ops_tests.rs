use crate::{Session, commands, trim_ops};
use filmcraft_project::ClipId;
use filmcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

fn setup() -> (Session, [ClipId; 3], [ClipId; 3]) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Validated trims","video":2,"audio":2})).unwrap();
    let item = s.project.items.values().find(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).unwrap().id;
    let unit = s.sequence_rate().frame_duration();
    for n in 0..3 {
        s.execute("timeline.place", json!({"item":item.0,"frame":n*24,"sourceIn":unit.0*24,"duration":unit.0*24})).unwrap();
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = std::array::from_fn(|i| q.video_tracks[0].items[i].id);
    let audio = std::array::from_fn(|i| q.audio_tracks[0].items[i].id);
    (s, video, audio)
}
fn redo(s: &mut Session) {
    s.execute("timeline.setTrack", json!({"track":"V2","name":"Keep the Redo branch"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.history.merge_key = Some("existing human gesture".into());
    assert!(!s.history.redo.is_empty());
}
fn rejected(s: &mut Session, command: &str, args: Value) {
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let merge = s.history.merge_key.clone();
    let journal = s.journal.clone();
    // Bypass the outer Session panic guard too: hostile values must return
    // a useful error rather than becoming CommandPanicked.
    let result = catch_unwind(AssertUnwindSafe(|| (commands::find(command).unwrap().run)(s, &args)));
    assert!(result.is_ok(), "{command} panicked on {args}");
    assert!(result.unwrap().is_err(), "{command} accepted {args}");
    assert!(s.execute(command, args.clone()).is_err(), "{command} accepted {args}");
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.merge_key, merge);
    assert_eq!(s.journal, journal);
    for (before, after) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(before.len(), after.len());
        assert!(before.iter().zip(after).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
    }
}
fn request(kind: trim_ops::Kind, v: [ClipId; 3]) -> (&'static str, Value) {
    match kind {
        trim_ops::Kind::Roll => ("timeline.roll", json!({"left":v[0].0,"right":v[1].0})),
        trim_ops::Kind::Slip => ("timeline.slip", json!({"clip":v[1].0})),
        trim_ops::Kind::Slide => ("timeline.slide", json!({"clip":v[1].0})),
    }
}
fn kinds() -> [trim_ops::Kind; 3] {
    [trim_ops::Kind::Roll, trim_ops::Kind::Slip, trim_ops::Kind::Slide]
}

#[test]
fn trim_requests_reject_malformed_conflicting_zero_and_saturated_time_without_losing_redo() {
    let (mut s, v, _) = setup();
    redo(&mut s);
    for kind in kinds() {
        let (cmd, p) = request(kind, v);
        rejected(&mut s, cmd, p.clone());
        for key in ["delta", "deltaFrames"] {
            for value in [json!(null), json!(false), json!("1"), json!(1.0), json!(1.5), json!([]), json!({}), json!(0), json!(i64::MIN), json!(i64::MAX)] {
                let mut p = p.clone();
                p[key] = value;
                rejected(&mut s, cmd, p);
            }
        }
        for malformed in [json!(1), json!("not-time"), json!(null)] {
            let mut p = p.clone();
            p["delta"] = malformed;
            p["deltaFrames"] = json!(1);
            rejected(&mut s, cmd, p);
        }
        let mut unknown = p.clone();
        unknown["deltaFrames"] = json!(1);
        unknown["followLinkz"] = json!(false);
        rejected(&mut s, cmd, unknown);
        for value in [json!(null), json!(true), json!(5), json!("params"), json!([])] {
            rejected(&mut s, cmd, value);
        }
    }
}

#[test]
fn roll_malformed_link_flags_never_follow_sound_even_when_linked_selection_is_off() {
    let (mut s, v, _) = setup();
    redo(&mut s);
    for linked in [false, true] {
        s.state.linked_selection = linked;
        for flag in [json!("false"), json!(null), json!(0), json!([]), json!({})] {
            rejected(&mut s, "timeline.roll", json!({"left":v[0].0,"right":v[1].0,"deltaFrames":1,"followLinks":flag}));
        }
    }
}

#[test]
fn roll_requires_distinct_adjacent_items_on_the_same_video_or_audio_track() {
    let (mut s, v, a) = setup();
    let unit = s.sequence_rate().frame_duration();
    s.execute("timeline.move", json!({"moves":[{"clip":v[1].0,"track":"V2","time":unit.0*24}],"linked":false})).unwrap();
    redo(&mut s);
    for (left, right) in [(v[0], v[1]), (v[0], a[1]), (v[0], v[0]), (v[0], v[2]), (v[1], v[0])] {
        rejected(&mut s, "timeline.roll", json!({"left":left.0,"right":right.0,"deltaFrames":1,"followLinks":false}));
    }
}

#[test]
fn prepared_trims_expire_on_native_document_sequence_and_implicit_linked_state_changes() {
    for kind in kinds() {
        for change in ["project", "sequence", "linked"] {
            let (mut s, v, _) = setup();
            redo(&mut s);
            let (_, mut p) = request(kind, v);
            p["deltaFrames"] = json!(1);
            let plan = trim_ops::prepare(&s, &p, kind).unwrap();
            match change {
                "project" => s.project = Arc::new((*s.project).clone()),
                "sequence" => s.state.active_sequence = s.project.sequences().find(|i| Some(i.id) != s.state.active_sequence).map(|i| i.id),
                _ => s.state.linked_selection = !s.state.linked_selection,
            }
            let before = s.project.clone();
            let revision = s.revision;
            let undo = s.history.undo.len();
            let redo = s.history.redo.len();
            assert!(plan.execute(&mut s).unwrap_err().to_string().contains("prepared trim expired"));
            assert!(Arc::ptr_eq(&before, &s.project));
            assert_eq!(s.revision, revision);
            assert_eq!(s.history.undo.len(), undo);
            assert_eq!(s.history.redo.len(), redo);
        }
    }
}

fn roundtrip(s: &mut Session, cmd: &str, p: Value) -> Value {
    let before = s.project.clone();
    let revision = s.revision;
    let undo = s.history.undo.len();
    let result = s.execute(cmd, p).unwrap();
    let after = s.project.clone();
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&before, &s.project));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&after, &s.project));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    result
}

#[test]
fn valid_linked_trims_keep_current_payload_captions_one_history_step_and_exact_saved_state() {
    for kind in kinds() {
        for direction in [-1, 1] {
            let (mut s, v, a) = setup();
            s.execute("captions.newTrack", json!({"format":"Subtitle","language":"ja"})).unwrap();
            s.execute("captions.add", json!({"track":"C1","frame":10,"durationFrames":30,"text":"Exact words remain fixed"})).unwrap();
            let before = s.project.clone();
            let unit = s.sequence_rate().frame_duration();
            let (cmd, mut p) = request(kind, v);
            p["deltaFrames"] = json!(direction);
            roundtrip(&mut s, cmd, p);
            let q = s.active_sequence().unwrap();
            let old = before.sequence(s.state.active_sequence.unwrap()).unwrap();
            assert_eq!(q.caption_tracks, old.caption_tracks);
            assert_eq!(q.markers, old.markers);
            for (video, audio) in v.iter().zip(a) {
                let c = q.find_item(*video).unwrap().1;
                let sound = q.find_item(audio).unwrap().1;
                assert_eq!((c.start, c.duration, c.source_in), (sound.start, sound.duration, sound.source_in));
            }
            for (track, original) in q.all_tracks().zip(old.all_tracks()) {
                for (c, was) in track.items.iter().zip(&original.items) {
                    assert_eq!(c.identity, was.identity);
                    assert_eq!(c.effects, was.effects);
                    assert_eq!(c.gain_db, was.gain_db);
                    assert_eq!(c.source_channels, was.source_channels);
                    assert_eq!(c.speed, was.speed);
                    assert_eq!(c.reverse, was.reverse);
                    assert_eq!(c.link, was.link);
                }
            }
            let (_, c) = q.find_item(v[1]).unwrap();
            match kind {
                trim_ops::Kind::Roll => {
                    assert_eq!(c.start, Tick(unit.0 * (24 + direction)));
                    assert_eq!(c.duration, Tick(unit.0 * (24 - direction)));
                }
                trim_ops::Kind::Slip => {
                    assert_eq!(c.start, Tick(unit.0 * 24));
                    assert_eq!(c.source_in, Tick(unit.0 * (24 + direction)));
                }
                trim_ops::Kind::Slide => {
                    assert_eq!(c.start, Tick(unit.0 * (24 + direction)));
                    assert_eq!(c.source_in, Tick(unit.0 * 24));
                }
            }
        }
    }
}

#[test]
fn explicit_roll_independence_survives_unrelated_selection_changes_without_acquiring_sound() {
    let (mut s, v, a) = setup();
    let unit = s.sequence_rate().frame_duration();
    let plan = trim_ops::prepare(&s, &json!({"left":v[0].0,"right":v[1].0,"deltaFrames":1,"followLinks":false}), trim_ops::Kind::Roll).unwrap();
    s.state.linked_selection = false;
    s.state.selection = vec![v[2]];
    s.set_playhead(Tick(unit.0 * 60));
    let sound = s.active_sequence().unwrap().find_item(a[1]).unwrap().1.clone();
    let result = plan.execute(&mut s).unwrap();
    assert_eq!(result["linked"], false);
    assert_eq!(s.active_sequence().unwrap().find_item(a[1]).unwrap().1, &sound);
    assert_eq!(s.active_sequence().unwrap().find_item(v[1]).unwrap().1.start, Tick(unit.0 * 25));
    assert_eq!(s.state.selection, vec![v[2]]);
    assert_eq!(s.playhead(), Tick(unit.0 * 60));
}

#[test]
fn clamped_zero_trims_preserve_the_cut_and_existing_redo_branch() {
    for kind in kinds() {
        let (mut s, v, a) = setup();
        let unit = s.sequence_rate().frame_duration();
        s.edit_sequence("Handle limits", |q, _, _| {
            if matches!(kind, trim_ops::Kind::Roll) {
                for track in q.all_tracks_mut() {
                    for (n, c) in track.items.iter_mut().enumerate() {
                        c.start = Tick(unit.0 * (n as i64));
                        c.duration = unit;
                    }
                }
            } else if matches!(kind, trim_ops::Kind::Slip) {
                for id in [v[1], a[1]] {
                    q.find_item_mut(id).unwrap().1.source_in = Tick::ZERO;
                }
            }
            Ok(())
        })
        .unwrap();
        redo(&mut s);
        let (cmd, mut p) = request(kind, v);
        p["deltaFrames"] = json!(-1);
        if matches!(kind, trim_ops::Kind::Slide) {
            p["clip"] = json!(v[0].0);
        }
        let project = s.project.clone();
        let revision = s.revision;
        let state = serde_json::to_value(&s.state).unwrap();
        let undo = s.history.undo.clone();
        let redo = s.history.redo.clone();
        let merge = s.history.merge_key.clone();
        let result = s.execute(cmd, p).unwrap();
        if matches!(kind, trim_ops::Kind::Roll) {
            assert_eq!(result["delta"], 0);
        } else {
            assert!(result.is_null());
        }
        assert!(Arc::ptr_eq(&project, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
        assert_eq!(s.history.merge_key, merge);
        for (before, after) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
            assert_eq!(before.len(), after.len());
            assert!(before.iter().zip(after).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
        }
    }
}

#[test]
fn exotic_positive_and_negative_frame_deltas_use_native_boundaries_and_reject_invalid_rates() {
    for frames in [-1, 1] {
        let (mut s, v, _) = setup();
        let seq = s.state.active_sequence.unwrap();
        let rate = FrameRate { num: 37516, den: 1000 };
        Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().settings.frame_rate = rate;
        let before = s.active_sequence().unwrap().find_item(v[1]).unwrap().1.source_in;
        roundtrip(&mut s, "timeline.slip", json!({"clip":v[1].0,"deltaFrames":frames}));
        assert_eq!(s.active_sequence().unwrap().find_item(v[1]).unwrap().1.source_in, before + rate.tick_of(frames));
    }
    for rate in [FrameRate { num: 0, den: 1 }, FrameRate { num: 24, den: 0 }, FrameRate { num: i64::MAX, den: 1 }, FrameRate { num: 1, den: i64::MAX }] {
        let (mut s, v, _) = setup();
        let seq = s.state.active_sequence.unwrap();
        redo(&mut s);
        Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().settings.frame_rate = rate;
        rejected(&mut s, "timeline.slip", json!({"clip":v[1].0,"deltaFrames":1}));
    }
}

#[test]
fn shared_slip_helper_handles_minimum_signed_delta_without_an_abs_panic() {
    let (mut s, v, a) = setup();
    let result = catch_unwind(AssertUnwindSafe(|| {
        s.edit_sequence("Extreme native slip", |q, ctx, _| commands::slip_together(q, ctx, &[(v[1], Tick(i64::MIN)), (a[1], Tick(i64::MIN))]))
    }));
    assert!(result.is_ok());
    assert!(result.unwrap().is_ok());
    let q = s.active_sequence().unwrap();
    assert_eq!(q.find_item(v[1]).unwrap().1.source_in, Tick::ZERO);
    assert_eq!(q.find_item(a[1]).unwrap().1.source_in, Tick::ZERO);
}
