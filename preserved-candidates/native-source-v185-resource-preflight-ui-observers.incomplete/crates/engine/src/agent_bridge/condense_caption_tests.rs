use super::{
    condense_tests,
    tests::{exact_batch, read, token},
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

fn setup() -> crate::Session {
    let mut s = condense_tests::setup();
    let sequence = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let project = Arc::make_mut(&mut s.project);
    for (index, locked, sync_lock) in [(0, false, true), (1, true, true), (2, false, false)] {
        let mut track = CaptionTrack::new(TrackId(90000 + index * 10), "Dialogue captions".into(), CaptionFormat::Subtitle);
        track.locked = locked;
        track.sync_lock = sync_lock;
        track.vtt_blocks.push("NOTE original inverse provenance".into());
        for (offset, start, duration, text) in [
            (1, rate.tick_of(11) + Tick(7), Tick(23), "REMOVED WORDS"),
            (2, rate.tick_of(17) + Tick(7), rate.tick_of(5), "CROSSING WORDS"),
            (3, rate.tick_of(50) + Tick(7), Tick(23), "LATER WORDS"),
        ] {
            track.captions.push(Caption {
                id: ClipId(90000 + index * 10 + offset),
                start,
                duration,
                text: text.into(),
                speaker: Some("Speaker".into()),
                cue_id: Some(format!("original-{index}-{offset}")),
                settings: "align:start".into(),
            });
        }
        project.sequence_mut(sequence).unwrap().caption_tracks.push(track);
    }
    project.next_id = 100000;
    s
}
fn basis(s: &crate::Session) -> Value {
    json!({"version":1,"beforeSha256":super::captions::summary(s)["sha256"],"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0}})
}
fn condense(s: &crate::Session) -> Value {
    let mut op = condense_tests::operation(s);
    op["params"]["captionBasis"] = basis(s);
    op
}
fn restore(s: &crate::Session, identity: &str) -> Value {
    json!({"command":"agent.restoreCondense","params":{"projectId":"p_test","identity":identity,"receiptSha256":"c".repeat(64),"captionBasis":basis(s)}})
}
fn apply(s: &mut crate::Session, op: Value, n: usize) -> Value {
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![op])).unwrap()
}
fn refuse(s: &mut crate::Session, op: Value, n: usize) {
    let before =
        (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.events.len(), s.journal.clone(), serde_json::to_value(&s.state).unwrap());
    read(s, n);
    assert!(s.execute("agent.apply", exact_batch(n, vec![op])).is_err());
    assert!(Arc::ptr_eq(&s.project, &before.0));
    assert_eq!(s.revision, before.1);
    assert_eq!(s.history.undo.len(), before.2);
    assert_eq!(s.history.redo.len(), before.3);
    assert_eq!(s.events.len(), before.4);
    assert_eq!(s.journal, before.5);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
}

#[test]
fn caption_condense_restores_removed_and_shortened_cues_with_exact_ticks_and_later_human_words_after_saved_reopen() {
    for reopen in [false, true] {
        let mut s = setup();
        let sequence = s.state.active_sequence.unwrap();
        let original = s.project.clone();
        let leader = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
        let operation = condense(&s);
        let receipt = apply(&mut s, operation, 9600);
        let proof = &receipt["results"][0]["captionEdit"];
        assert_eq!(proof["operation"], "condense");
        assert_eq!(proof["removedCues"], 1);
        assert_eq!(proof["createdCues"], 0);
        let q = s.active_sequence().unwrap();
        assert!(q.find_caption(ClipId(90001)).is_none());
        assert_eq!(q.find_caption(ClipId(90002)).unwrap().1.duration, s.sequence_rate().tick_of(3));
        assert_eq!(&q.caption_tracks[1..], &original.sequence(sequence).unwrap().caption_tracks[1..]);
        if reopen {
            let decoded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
            let mut fresh = crate::Session::new(s.services.clone());
            fresh.project = Arc::new(decoded.project);
            fresh.state = s.state.clone();
            fresh.media = s.media.clone();
            s = fresh;
        }
        s.edit_sequence("Human caption and audio finishing", |q, _, _| {
            q.caption_tracks[0].caption_mut(ClipId(90002)).unwrap().text = "Human correction".into();
            q.caption_tracks[0].style.size = 62.0;
            q.caption_tracks[0].vtt_blocks.push("NOTE later human annotation".into());
            q.audio_tracks[0].items[1].gain_db = -7.0;
            Ok(())
        })
        .unwrap();
        let human = s.project.clone();
        let operation = restore(&s, &leader);
        let count = s.history.undo.len();
        let receipt = apply(&mut s, operation, 9601);
        assert_eq!(s.history.undo.len(), count + 1);
        assert_eq!(receipt["results"][0]["captionEdit"]["operation"], "restore");
        assert_eq!(receipt["results"][0]["captionEdit"]["createdCues"], 1);
        let mut expected = original.sequence(sequence).unwrap().caption_tracks.clone();
        expected[0].caption_mut(ClipId(90002)).unwrap().text = "Human correction".into();
        expected[0].style.size = 62.0;
        expected[0].vtt_blocks.push("NOTE later human annotation".into());
        assert_eq!(s.active_sequence().unwrap().caption_tracks, expected);
        assert_eq!(s.active_sequence().unwrap().audio_tracks[0].items[1].gain_db, -7.0);
        let after = s.project.clone();
        read(&mut s, 9602);
        s.execute("agent.undo", json!({"token":token(9602),"commit":token(9601)})).unwrap();
        assert_eq!(s.project, human);
        read(&mut s, 9603);
        s.execute("agent.redo", json!({"token":token(9603),"commit":token(9601),"undo":token(9602)})).unwrap();
        assert_eq!(s.project, after);
    }
}

#[test]
fn caption_condense_requires_current_basis_and_refuses_deleted_moved_locked_or_forged_inverse_without_publication() {
    for mode in 0..4 {
        let mut s = setup();
        let mut op = condense(&s);
        match mode {
            0 => {
                op["params"].as_object_mut().unwrap().remove("captionBasis");
            }
            1 => op["params"]["captionBasis"]["beforeSha256"] = json!("0".repeat(64)),
            2 => op["params"]["captionBasis"]["scope"]["projectId"] = json!("p_foreign"),
            _ => op["params"]["captionBasis"]["version"] = json!(2),
        }
        refuse(&mut s, op, 9610 + mode);
    }
    for mode in 0..4 {
        let mut s = setup();
        let leader = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
        let op = condense(&s);
        apply(&mut s, op, 9620);
        s.edit_sequence("Later human work", |q, _, _| {
            match mode {
                0 => q.caption_tracks[0].captions.retain(|c| c.id != ClipId(90002)),
                1 => q.caption_tracks[0].caption_mut(ClipId(90002)).unwrap().start = Tick(600000000000),
                2 => q.caption_tracks[0].locked = true,
                _ => q.caption_tracks[0].sync_lock = false,
            }
            q.caption_tracks[0].sort();
            Ok(())
        })
        .unwrap();
        let op = restore(&s, &leader);
        refuse(&mut s, op, 9621);
    }
}

#[test]
fn caption_condense_canonical_full_and_reselection_preserve_native_cues_and_refresh_owned_inverse() {
    let mut s = setup();
    let sequence = s.state.active_sequence.unwrap();
    let original = s.project.clone();
    let old = condense(&s);
    let passage = old["params"]["passages"][0].clone();
    let op = old.clone();
    apply(&mut s, op, 9630);
    let make = |s: &crate::Session, version: u8, kept: Value| json!({"command":"agent.chooseCondense","params":{"projectId":"p_test","receiptSha256":"c".repeat(64),"version":version,"captionBasis":basis(s),"passages":[{"identity":passage["identity"],"sourceIn":passage["sourceIn"],"sourceOut":passage["sourceOut"],"kept":kept}]}});
    let op = make(&s, 4, json!([[passage["sourceIn"], passage["sourceOut"]]]));
    let full = apply(&mut s, op, 9631);
    assert_eq!(full["results"][0]["captionEdit"]["operation"], "choice");
    assert_eq!(s.active_sequence().unwrap().caption_tracks, original.sequence(sequence).unwrap().caption_tracks);
    let op = make(&s, 1, passage["kept"].clone());
    let selected = apply(&mut s, op, 9632);
    assert_eq!(selected["results"][0]["captionEdit"]["removedCues"], 1);
    let identity = passage["identity"].as_str().unwrap();
    let op = restore(&s, identity);
    apply(&mut s, op, 9633);
    assert_eq!(s.active_sequence().unwrap().caption_tracks, original.sequence(sequence).unwrap().caption_tracks);
}

#[test]
fn caption_condense_adjacent_passages_restore_one_fully_removed_cue_in_either_order_after_saved_restart() {
    for reverse in [false, true] {
        let mut s = setup();
        let sequence = s.state.active_sequence.unwrap();
        let rate = s.sequence_rate();
        let q = Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap();
        for track in q.all_tracks_mut() {
            track.items[0].start = rate.tick_of(10);
            track.items[0].duration = rate.tick_of(20);
            track.items[0].source_in = rate.tick_of(100);
            track.items[1].start = rate.tick_of(30);
        }
        for track in &mut q.caption_tracks {
            let cue = track.captions.iter_mut().find(|c| c.id.0 % 10 == 1).unwrap();
            cue.start = rate.tick_of(29) + Tick(7);
            cue.duration = rate.tick_of(2) - Tick(13);
            track.sort();
        }
        let before = q.caption_tracks.clone();
        let leaders = [q.video_tracks[0].items[0].identity.as_ref().unwrap().id.clone(), q.video_tracks[0].items[1].identity.as_ref().unwrap().id.clone()];
        let mut op = condense(&s);
        op["params"]["passages"][0]["kept"] = json!([[rate.tick_of(102).0, rate.tick_of(118).0]]);
        let second = &s.active_sequence().unwrap().video_tracks[0].items[1];
        let additional = json!({"clip":second.id.0,"identity":leaders[1],"sourceIn":second.source_in.0,"sourceOut":second.source_out().0,"start":second.start.0,"kept":[[rate.tick_of(102).0,rate.tick_of(112).0]]});
        op["params"]["passages"].as_array_mut().unwrap().push(additional);
        apply(&mut s, op, 9640);
        assert!(s.active_sequence().unwrap().find_caption(ClipId(90001)).is_none());
        for (n, index) in if reverse { vec![1, 0] } else { vec![0, 1] }.into_iter().enumerate() {
            let decoded = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
            let mut fresh = crate::Session::new(s.services.clone());
            fresh.project = Arc::new(decoded.project);
            fresh.state = s.state.clone();
            fresh.media = s.media.clone();
            s = fresh;
            let op = restore(&s, &leaders[index]);
            apply(&mut s, op, 9641 + n);
        }
        assert_eq!(s.active_sequence().unwrap().caption_tracks, before);
    }
}
