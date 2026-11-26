use super::tests::{exact_batch, read, setup, token};
use filmcraft_project::{ClipId, ItemId};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn fixture() -> (crate::Session, u64) {
    let mut s = setup();
    let item = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video() && m.info.has_audio())).unwrap().id;
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    for t in q.all_tracks_mut() {
        t.items.clear();
        t.transitions.clear();
        t.locked = false;
        t.sync_lock = true;
    }
    s.execute("agent.bindSource", json!({"item":item.0,"binding":{"projectId":"p_test","assetId":"a_source","mediaId":"m_original","role":"original"}}))
        .unwrap();
    let rate = s.sequence_rate();
    for frame in [0, 12, 24] {
        let v = s
            .execute(
                "timeline.place",
                json!({"item":item.0,"track":"V1","frame":frame,"sourceIn":0,"duration":rate.tick_of(12).0,"video":true,"audio":false,"insert":false}),
            )
            .unwrap()["clips"][0]
            .as_u64()
            .unwrap();
        let a = s
            .execute(
                "timeline.place",
                json!({"item":item.0,"track":"A1","frame":frame,"sourceIn":0,"duration":rate.tick_of(12).0,"video":false,"audio":true,"insert":false}),
            )
            .unwrap()["clips"][0]
            .as_u64()
            .unwrap();
        s.execute("clip.audioChannels", json!({"clips":[a],"channels":[0]})).unwrap();
        s.execute("clip.link", json!({"clips":[v,a]})).unwrap();
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    (s, item.0)
}
fn insert(s: &crate::Session, source: u64, frame: i64) -> Value {
    let q = s.active_sequence().unwrap();
    json!({"command":"agent.insert","params":{"item":source,"videoTrack":q.video_tracks[0].id.0,"audioTrack":q.audio_tracks[0].id.0,"frame":frame,"sourceInFrame":1,"durationFrames":3,"audioChannels":[0]}})
}
fn primary(s: &crate::Session) -> u64 {
    s.active_sequence().unwrap().video_tracks[0].items[0].id.0
}
fn moved(s: &crate::Session, frame: i64, mode: &str) -> Value {
    json!({"command":"agent.move","params":{"clip":primary(s),"frame":frame,"mode":mode}})
}
fn deleted(s: &crate::Session) -> Value {
    json!({"command":"agent.rippleDelete","params":{"clip":primary(s)}})
}
fn apply(s: &mut crate::Session, op: Value) -> Value {
    read(s, 1);
    s.execute("agent.apply", exact_batch(1, vec![op])).unwrap()
}
fn assert_rejected(s: &mut crate::Session, op: Value) {
    read(s, 1);
    let before = (s.project.clone(), s.revision, s.history.undo.len(), s.events.len(), s.journal.len());
    assert!(s.execute("agent.apply", exact_batch(1, vec![op])).is_err());
    assert_eq!(s.project, before.0);
    assert_eq!(s.revision, before.1);
    assert_eq!(s.history.undo.len(), before.2);
    assert_eq!(s.events.len(), before.3);
    assert_eq!(s.journal.len(), before.4);
}
fn undo(s: &mut crate::Session) {
    read(s, 2);
    s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
}

fn insertion_captions(s: &mut crate::Session, frame: i64) {
    use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, TrackId};
    use filmcraft_time::Tick;
    let cut = s.sequence_rate().tick_of(frame);
    let sequence = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    for (locked, sync_lock) in [(false, true), (true, true), (false, false)] {
        let mut track = CaptionTrack::new(TrackId(p.alloc_id()), "Insert captions".into(), CaptionFormat::Subtitle);
        track.locked = locked;
        track.sync_lock = sync_lock;
        track.vtt_blocks.push("NOTE exact imported annotation".into());
        track.captions.push(Caption {
            id: ClipId(p.alloc_id()),
            start: cut - Tick(7),
            duration: Tick(30),
            text: "Exact cue".into(),
            speaker: Some("Speaker".into()),
            cue_id: Some("original-cue".into()),
            settings: "align:start".into(),
        });
        p.sequence_mut(sequence).unwrap().caption_tracks.push(track);
    }
}

fn caption_insert(s: &crate::Session, source: u64, frame: i64) -> Value {
    let mut op = insert(s, source, frame);
    op["params"]["captionBasis"] = json!({"version":1,"beforeSha256":super::captions::summary(s)["sha256"],
        "scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0}});
    op
}

#[test]
fn caption_insert_matches_human_timing_for_picture_sound_and_linked_groups() {
    for frame in [6, 12] {
        for (video, audio) in [(true, false), (false, true), (true, true)] {
            let (mut s, source) = fixture();
            insertion_captions(&mut s, frame);
            let original = s.project.clone();
            let before = s.active_sequence().unwrap().caption_tracks.clone();
            let mut human = crate::Session::new(s.services.clone());
            human.project = s.project.clone();
            human.state = s.state.clone();
            human.media = s.media.clone();
            human
                .execute(
                    "timeline.place",
                    json!({"item":source,"track":if video {"V1"} else {"A1"},"audioTrack":if video&&audio {json!("A1")} else {Value::Null},
                "frame":frame,"sourceIn":s.sequence_rate().tick_of(1).0,"duration":s.sequence_rate().tick_of(3).0,"video":video,"audio":audio,"insert":true}),
                )
                .unwrap();
            let mut op = caption_insert(&s, source, frame);
            if !video {
                op["params"]["videoTrack"] = Value::Null;
            }
            if !audio {
                op["params"]["audioTrack"] = Value::Null;
                op["params"]["audioChannels"] = json!([]);
            }
            let receipt = apply(&mut s, op);
            let q = s.active_sequence().unwrap();
            assert_eq!(q.caption_tracks, human.active_sequence().unwrap().caption_tracks);
            assert_eq!(&q.caption_tracks[1..], &before[1..], "locked and non-sync-locked tracks stay exact");
            assert_eq!(q.caption_tracks[0].captions.len(), 2);
            assert_eq!(q.caption_tracks[0].captions[0].duration, filmcraft_time::Tick(7));
            assert_eq!(q.caption_tracks[0].captions[1].cue_id, None);
            let proof = &receipt["results"][0]["captionRipple"];
            assert_eq!(proof["policy"], "insert_span_v1");
            assert_eq!(proof["createdCues"], 1);
            assert_eq!(proof["openedSpans"], json!([{"startTicks":s.sequence_rate().tick_of(frame).0,"durationTicks":s.sequence_rate().tick_of(3).0}]));
            let inserted = s.project.clone();
            undo(&mut s);
            assert_eq!(s.project, original);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(s.project, inserted);
            assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *s.project);
        }
    }
}

#[test]
fn caption_insert_requires_exact_basis_and_preserves_protected_collateral_atomically() {
    for mode in 0..3 {
        let (mut s, source) = fixture();
        insertion_captions(&mut s, 12);
        let mut op = caption_insert(&s, source, 12);
        match mode {
            0 => {
                op["params"].as_object_mut().unwrap().remove("captionBasis");
            }
            1 => op["params"]["captionBasis"]["beforeSha256"] = json!("0".repeat(64)),
            _ => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items.last().unwrap().id;
                s.execute("poema.protectClips", json!({"clips":[clip.0],"reason":"Human protection"})).unwrap();
            }
        }
        assert_rejected(&mut s, op);
    }
}

#[test]
fn caption_insert_accepts_empty_tracks_without_creating_or_retiming_cues() {
    let (mut s, source) = fixture();
    insertion_captions(&mut s, 12);
    let sequence = s.state.active_sequence.unwrap();
    for track in &mut Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().caption_tracks {
        track.captions.clear();
    }
    let before = s.active_sequence().unwrap().caption_tracks.clone();
    let op = caption_insert(&s, source, 12);
    let receipt = apply(&mut s, op);
    assert_eq!(s.active_sequence().unwrap().caption_tracks, before);
    let proof = &receipt["results"][0]["captionRipple"];
    assert_eq!(proof["beforeSha256"], proof["afterSha256"]);
    assert_eq!(proof["createdCues"], 0);
    assert_eq!(proof["retimedCues"], 0);
}

#[test]
fn assembly_insert_is_one_exact_history_step_with_explicit_primary_sound_and_undo_redo() {
    let (mut s, source) = fixture();
    let original = s.project.clone();
    let count = s.history.undo.len();
    let rev = s.revision;
    let op = insert(&s, source, 12);
    let receipt = apply(&mut s, op);
    assert_eq!(receipt["validated"], true);
    assert_eq!(s.revision, rev + 1);
    assert_eq!(s.history.undo.len(), count + 1);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks.len(), 1);
    assert_eq!(q.audio_tracks.len(), 1);
    let (v, a) = (&q.video_tracks[0].items[1], &q.audio_tracks[0].items[1]);
    assert_eq!(v.start, s.sequence_rate().tick_of(12));
    assert_eq!(v.duration, s.sequence_rate().tick_of(3));
    assert_eq!(v.source_in, s.sequence_rate().tick_of(1));
    assert_eq!(v.link, a.link);
    assert!(v.link.is_some());
    assert_ne!(v.identity, a.identity);
    assert_eq!(a.source_channels, vec![0]);
    assert_eq!(a.audio_stream, 0);
    assert_eq!(q.video_tracks[0].items[2].start, s.sequence_rate().tick_of(15));
    let edited = s.project.clone();
    undo(&mut s);
    assert_eq!(s.project, original);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, edited);
}
#[test]
fn assembly_reorder_closes_old_range_preserves_identity_sound_notes_and_total_length() {
    let (mut s, _) = fixture();
    let clip = primary(&s);
    s.execute("poema.annotateClips", json!({"clips":[clip],"annotations":{"editor_note":"Keep my choice"}})).unwrap();
    let original = s.project.clone();
    let op = moved(&s, 36, "reorder");
    apply(&mut s, op);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items.last().unwrap().id, ClipId(clip));
    assert_eq!(q.duration(), s.sequence_rate().tick_of(36));
    for track in [&q.video_tracks[0], &q.audio_tracks[0]] {
        assert_eq!(track.items.iter().map(|c| s.sequence_rate().frame_at(c.start)).collect::<Vec<_>>(), vec![0, 12, 24]);
    }
    let old = original.sequence(s.state.active_sequence.unwrap()).unwrap().find_item(ClipId(clip)).unwrap().1;
    let now = q.find_item(ClipId(clip)).unwrap().1;
    assert_eq!(old.identity, now.identity);
    assert_eq!(old.effects, now.effects);
    assert_eq!(old.source_in, now.source_in);
    undo(&mut s);
    assert_eq!(s.project, original);
}
#[test]
fn assembly_gap_move_never_overwrites_and_undo_restores_it() {
    let (mut s, _) = fixture();
    let original = s.project.clone();
    let clip = primary(&s);
    let op = moved(&s, 50, "gap");
    apply(&mut s, op);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.find_item(ClipId(clip)).unwrap().1.start, s.sequence_rate().tick_of(50));
    assert_eq!(q.video_tracks[0].items.len(), 3);
    assert_eq!(q.audio_tracks[0].items.len(), 3);
    undo(&mut s);
    assert_eq!(s.project, original);
    let (mut occupied, _) = fixture();
    let op = moved(&occupied, 12, "gap");
    assert_rejected(&mut occupied, op);
}
#[test]
fn assembly_ripple_delete_removes_both_members_closes_once_and_undo_restores() {
    let (mut s, _) = fixture();
    s.state.linked_selection = false;
    let original = s.project.clone();
    let op = deleted(&s);
    apply(&mut s, op);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.duration(), s.sequence_rate().tick_of(24));
    for t in [&q.video_tracks[0], &q.audio_tracks[0]] {
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[0].start, filmcraft_time::Tick::ZERO);
        assert_eq!(t.items[1].start, s.sequence_rate().tick_of(12));
    }
    undo(&mut s);
    assert_eq!(s.project, original);
}
#[test]
fn assembly_protected_collateral_ripples_publish_nothing() {
    for mode in 0..3 {
        let (mut s, source) = fixture();
        let last = s.active_sequence().unwrap().video_tracks[0].items.last().unwrap().id.0;
        s.execute("poema.protectClips", json!({"clips":[last],"reason":"Human lock"})).unwrap();
        let op = match mode {
            0 => insert(&s, source, 12),
            1 => moved(&s, 36, "reorder"),
            _ => deleted(&s),
        };
        assert_rejected(&mut s, op);
    }
}
#[test]
fn assembly_locked_partners_and_collateral_sync_mismatches_are_not_silently_skipped() {
    for mode in 0..3 {
        let (mut s, source) = fixture();
        let seq = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
        if mode == 0 {
            q.audio_tracks[0].locked = true;
        } else {
            q.audio_tracks[0].sync_lock = false;
        }
        let mut op = if mode == 0 { deleted(&s) } else { insert(&s, source, 12) };
        if mode > 0 {
            op["params"]["audioTrack"] = Value::Null;
            op["params"]["audioChannels"] = json!([]);
        }
        assert_rejected(&mut s, op);
    }
}
#[test]
fn assembly_rejects_bad_channels_out_of_bounds_sources_foreign_ripples_and_hostile_shapes() {
    for mode in 0..7 {
        let (mut s, source) = fixture();
        let mut op = insert(&s, source, 12);
        match mode {
            0 => op["params"]["audioChannels"] = json!([65535]),
            1 => op["params"]["sourceInFrame"] = json!(1_000_000),
            2 => op["params"]["videoTrack"] = json!(-1),
            3 => op["params"]["frame"] = json!(1.5),
            4 => op["params"]["path"] = json!("/untrusted.MXF"),
            5 => {
                Arc::make_mut(&mut s.project).items.get_mut(&ItemId(source)).unwrap().metadata.remove(super::BINDING_KEY);
            }
            _ => {
                op["params"]["audioChannels"] = json!([0, 0]);
            }
        }
        assert_rejected(&mut s, op);
    }
}
#[test]
fn assembly_batches_require_exact_mode_and_a_single_logical_operation() {
    let (mut s, source) = fixture();
    let op = insert(&s, source, 12);
    read(&mut s, 1);
    let original = s.project.clone();
    assert!(s.execute("agent.apply", super::tests::batch(1, vec![op.clone()])).is_err());
    assert!(s.execute("agent.apply", exact_batch(1, vec![op.clone(), op])).is_err());
    assert_eq!(s.project, original);
}
#[test]
fn assembly_recovery_keeps_insert_and_undo_as_one_history_step() {
    let (mut s, source) = fixture();
    let original = s.project.clone();
    let offset = s.journal.len();
    let op = insert(&s, source, 12);
    apply(&mut s, op);
    undo(&mut s);
    let journal = s.journal[offset..].to_vec();
    assert_eq!(journal.len(), 2);
    assert_eq!(s.project, original);
    let (mut recovered, _) = fixture();
    let baseline = recovered.project.clone();
    for (command, params) in journal {
        recovered.execute(&command, params).unwrap();
    }
    assert_eq!(recovered.project, baseline);
}

#[test]
fn human_ripple_delete_keeps_caption_clocks_linked_sound_history_and_saved_document_exact() {
    use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, TrackId};
    use filmcraft_time::Tick;
    let (mut s, _) = fixture();
    let rate = s.sequence_rate();
    let seq = s.state.active_sequence.unwrap();
    let mut expected = Vec::new();
    for (index, locked, sync_lock) in [(0, false, true), (1, true, true), (2, false, false)] {
        let mut track = CaptionTrack::new(TrackId(90000 + index * 10), "Exact source captions".into(), CaptionFormat::Subtitle);
        track.locked = locked;
        track.sync_lock = sync_lock;
        track.captions.push(Caption {
            id: ClipId(90001 + index * 10),
            start: rate.tick_of(12) + Tick(17),
            duration: Tick(23),
            text: "Exact original words".into(),
            speaker: Some("Original speaker".into()),
            cue_id: Some("original cue".into()),
            settings: "align:start".into(),
        });
        track.vtt_blocks.push("NOTE original archive annotation".into());
        let mut after = track.clone();
        if !locked && sync_lock {
            after.captions[0].start = Tick(17);
        }
        expected.push(after);
        Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.push(track);
    }
    Arc::make_mut(&mut s.project).next_id = 100000;
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let first = primary(&s);
    s.execute("edit.rippleDelete", json!({"clips":[first]})).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!(q.caption_tracks, expected);
    assert_eq!(q.video_tracks[0].items[0].start, Tick::ZERO);
    assert_eq!(q.audio_tracks[0].items[0].start, Tick::ZERO);
    assert_eq!(q.video_tracks[0].items[0].link, q.audio_tracks[0].items[0].link);
    assert_eq!(s.history.undo.len(), undo + 1);
    let edited = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, edited);
    let reopened = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project;
    assert_eq!(reopened, *edited);
}

fn caption_ripple_fixture() -> crate::Session {
    use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, TrackId};
    use filmcraft_time::Tick;
    let (mut s, _) = fixture();
    let sequence = s.state.active_sequence.unwrap();
    let at = s.sequence_rate().tick_of(12);
    for (index, locked, sync_lock) in [(0, false, true), (1, true, true), (2, false, false)] {
        let mut track = CaptionTrack::new(TrackId(90000 + index * 10), "Caption ripple collateral".into(), CaptionFormat::Subtitle);
        track.locked = locked;
        track.sync_lock = sync_lock;
        track.captions.push(Caption {
            id: ClipId(90001 + index * 10),
            start: at + Tick(17),
            duration: Tick(23),
            text: "Exact words".into(),
            speaker: Some("Original speaker".into()),
            cue_id: Some("source cue".into()),
            settings: "align:start".into(),
        });
        if index == 0 {
            let mut removed = track.captions[0].clone();
            removed.id = ClipId(90002);
            removed.start = Tick(17);
            track.captions.insert(0, removed);
        }
        track.vtt_blocks.push("NOTE source annotation".into());
        Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().caption_tracks.push(track);
    }
    Arc::make_mut(&mut s.project).next_id = 100000;
    s
}
fn caption_ripple_delete(s: &crate::Session) -> Value {
    let sequence = s.state.active_sequence.unwrap();
    json!({"command":"agent.rippleDelete","params":{"clip":primary(s),"captionBasis":{"version":1,
        "beforeSha256":super::captions::summary(s)["sha256"],"scope":{"projectId":"p_test","anchorSequence":sequence.0}}}})
}

fn caption_move_fixture() -> crate::Session {
    let mut s = caption_ripple_fixture();
    let sequence = s.state.active_sequence.unwrap();
    let unit = s.sequence_rate().frame_duration();
    let project = Arc::make_mut(&mut s.project);
    let mut next = project.next_id;
    for track in &mut project.sequence_mut(sequence).unwrap().caption_tracks {
        for frame in [5, 29] {
            let mut cue = track.captions[0].clone();
            cue.id = ClipId(next);
            next += 1;
            cue.start = filmcraft_time::Tick(unit.0 * frame + 7);
            cue.duration = filmcraft_time::Tick(unit.0 * 2 + 23);
            track.captions.push(cue);
        }
        track.sort();
    }
    project.next_id = next;
    s
}
fn caption_move(s: &crate::Session, clip: u64, frame: i64, mode: &str) -> Value {
    let target = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    json!({"command":"agent.move","params":{"clip":clip,"frame":frame,"mode":mode,"sourceStartTicks":target.start.0,"durationTicks":target.duration.0,"captionBasis":{"version":1,
        "beforeSha256":super::captions::summary(s)["sha256"],"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0}}}})
}
#[test]
fn caption_move_matches_human_gap_and_extract_insert_with_exact_metadata_history_and_reopen() {
    for (mode, index, frame, gap_first, off_frame) in [
        ("gap", 0, 50, false, false),
        ("gap", 0, 0, true, false),
        ("reorder", 0, 30, false, false),
        ("reorder", 2, 6, false, false),
        ("reorder", 0, 30, false, true),
    ] {
        let mut s = caption_move_fixture();
        if off_frame {
            let sequence = s.state.active_sequence.unwrap();
            for track in Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().all_tracks_mut() {
                for (index, c) in track.items.iter_mut().enumerate() {
                    if index == 0 {
                        c.duration += filmcraft_time::Tick(7);
                    } else {
                        c.start += filmcraft_time::Tick(7);
                    }
                }
            }
        }
        let clip = s.active_sequence().unwrap().video_tracks[0].items[index].id;
        if gap_first {
            s.execute("timeline.move", json!({"moves":[{"clip":clip.0,"track":"V1","time":s.sequence_rate().tick_of(50).0}]})).unwrap();
        }
        let before = s.project.clone();
        let old = s.active_sequence().unwrap().find_item(clip).unwrap().1.clone();
        let link = old.link;
        let ids: Vec<_> = s
            .active_sequence()
            .unwrap()
            .all_tracks()
            .flat_map(|t| &t.items)
            .filter(|c| c.id == clip || link.is_some() && c.link == link)
            .map(|c| c.id)
            .collect();
        let destination = if mode == "reorder" && s.sequence_rate().tick_of(frame) > old.start {
            s.sequence_rate().tick_of(frame) - old.duration
        } else {
            s.sequence_rate().tick_of(frame)
        };
        let mut human = crate::Session::new(s.services.clone());
        human.project = s.project.clone();
        human.state = s.state.clone();
        human.media = s.media.clone();
        if mode == "gap" {
            human.execute("timeline.move", json!({"moves":[{"clip":clip.0,"track":"V1","time":destination.0}]})).unwrap();
        } else if !off_frame {
            human.state.selection = ids.clone();
            human.execute("edit.copy", json!({})).unwrap();
            human.execute("edit.rippleDelete", json!({"clips":ids.iter().map(|c|c.0).collect::<Vec<_>>()})).unwrap();
            human.set_playhead(destination);
            human.execute("edit.pasteInsert", json!({})).unwrap();
        }
        let op = caption_move(&s, clip.0, frame, mode);
        let receipt = apply(&mut s, op.clone());
        let normalize = |tracks: &[filmcraft_project::CaptionTrack]| {
            let mut tracks = tracks.to_vec();
            for c in tracks.iter_mut().flat_map(|t| &mut t.captions) {
                if c.id.0 >= before.next_id {
                    c.id = ClipId(0);
                }
            }
            tracks
        };
        if off_frame {
            // Clipboard Paste uses a frame-snapped playhead. The exact native
            // reorder retains a sub-frame post-Extract destination instead;
            // check that policy against explicit tick expectations, not a
            // falsely equivalent snapped human operation.
            let unit = s.sequence_rate().frame_duration().0;
            let cues = &s.active_sequence().unwrap().caption_tracks[0].captions;
            assert_eq!(cues.len(), 3);
            assert_eq!((cues[0].id.0, cues[0].start.0, cues[0].duration.0), (90001, 10, 23));
            assert_eq!((cues[1].id.0, cues[1].start.0, cues[1].duration.0), (100001, 17 * unit, unit - 7));
            assert!(cues[2].id.0 >= before.next_id);
            assert_eq!((cues[2].start.0, cues[2].duration.0), (30 * unit, unit + 30));
            assert_eq!(cues[2].cue_id, None);
            assert_eq!(cues[1].text, cues[2].text);
            assert_eq!(cues[1].speaker, cues[2].speaker);
            assert_eq!(cues[1].settings, cues[2].settings);
            assert_eq!(receipt["results"][0]["frame"], 17);
        } else {
            assert_eq!(
                normalize(&s.active_sequence().unwrap().caption_tracks),
                normalize(&human.active_sequence().unwrap().caption_tracks),
                "{mode} to {frame}"
            );
        }
        assert_eq!(&s.active_sequence().unwrap().caption_tracks[1..], &before.sequence(s.state.active_sequence.unwrap()).unwrap().caption_tracks[1..]);
        let moved = s.active_sequence().unwrap().find_item(clip).unwrap().1;
        assert_eq!(moved.start, destination);
        assert_eq!(moved.identity, old.identity);
        assert_eq!(moved.effects, old.effects);
        assert_eq!(moved.source_in, old.source_in);
        let proof = &receipt["results"][0]["captionRipple"];
        assert_eq!(proof["policy"], "sequence_media_move_v1");
        assert_eq!(proof["mode"], mode);
        assert_eq!(proof["sourceStartTicks"], old.start.0);
        assert_eq!(proof["destinationTicks"], destination.0);
        assert_eq!(proof["requestedDestinationTicks"], s.sequence_rate().tick_of(frame).0);
        assert_eq!(proof["afterSha256"], super::captions::summary(&s)["sha256"]);
        if mode == "gap" {
            assert_eq!(proof["beforeSha256"], proof["afterSha256"]);
            assert_eq!(proof["createdCues"], 0);
        } else {
            assert_eq!(proof["createdCues"], 1);
            assert_eq!(proof["removedCues"], if index == 0 { 2 } else { 1 });
        }
        let edited = s.project.clone();
        undo(&mut s);
        assert_eq!(s.project, before);
        read(&mut s, 3);
        s.execute("agent.redo", json!({"token":token(3),"commit":token(1),"undo":token(2)})).unwrap();
        assert_eq!(s.project, edited);
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *edited);
    }
}
#[test]
fn caption_move_refuses_missing_wrong_or_stale_basis_occupied_ranges_and_protected_collateral() {
    for variant in 0..8 {
        let mut s = caption_move_fixture();
        let mut op = caption_move(&s, primary(&s), 30, "reorder");
        match variant {
            0 => {
                op["params"].as_object_mut().unwrap().remove("captionBasis");
            }
            1 => op["params"]["captionBasis"]["beforeSha256"] = json!("0".repeat(64)),
            2 => op["params"]["captionBasis"]["scope"]["projectId"] = json!("p_foreign"),
            3 => op["params"]["frame"] = json!(6),
            4 => {
                op["params"]["mode"] = json!("gap");
                op["params"]["frame"] = json!(12);
            }
            5 => {
                let collateral = s.active_sequence().unwrap().video_tracks[0].items[2].id;
                s.execute("poema.protectClips", json!({"clips":[collateral.0],"reason":"Keep this choice"})).unwrap();
            }
            6 => op["params"]["sourceStartTicks"] = json!(7),
            _ => op["params"]["durationTicks"] = json!(s.sequence_rate().tick_of(12).0 + 7),
        }
        assert_rejected(&mut s, op);
    }
}
#[test]
fn caption_move_accepts_empty_tracks_without_caption_changes() {
    let mut s = caption_move_fixture();
    let sequence = s.state.active_sequence.unwrap();
    for track in &mut Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().caption_tracks {
        track.captions.clear();
    }
    let op = caption_move(&s, primary(&s), 30, "reorder");
    let r = apply(&mut s, op);
    let proof = &r["results"][0]["captionRipple"];
    assert_eq!(proof["beforeSha256"], proof["afterSha256"]);
    assert_eq!(proof["createdCues"], 0);
    assert_eq!(proof["removedCues"], 0);
}

#[test]
fn caption_ripple_delete_matches_human_cut_exact_collateral_receipts_owned_history_and_reopen() {
    let mut s = caption_ripple_fixture();
    let before = s.project.clone();
    let mut human = crate::Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute("edit.rippleDelete", json!({"clips":[primary(&s)]})).unwrap();
    let op = caption_ripple_delete(&s);
    let undo_steps = s.history.undo.len();
    let r = apply(&mut s, op.clone());
    assert_eq!(s.project, human.project);
    assert_eq!(s.history.undo.len(), undo_steps + 1);
    let proof = &r["results"][0]["captionRipple"];
    assert_eq!(proof["beforeCues"], 4);
    assert_eq!(proof["afterCues"], 3);
    assert_eq!(proof["removedCues"], 1);
    assert_eq!(proof["retimedCues"], 1);
    assert_eq!(proof["beforeSha256"], op["params"]["captionBasis"]["beforeSha256"]);
    assert_eq!(proof["afterSha256"], super::captions::summary(&s)["sha256"]);
    assert_eq!(proof["closedSpans"], json!([{"startTicks":0,"durationTicks":s.sequence_rate().tick_of(12).0}]));
    assert_eq!(s.execute("agent.apply", exact_batch(1, vec![op])).unwrap(), r, "same receipt, no second deletion");
    let edited = s.project.clone();
    undo(&mut s);
    assert_eq!(s.project, before);
    read(&mut s, 3);
    s.execute("agent.redo", json!({"token":token(3),"commit":token(1),"undo":token(2)})).unwrap();
    assert_eq!(s.project, edited);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *edited);
}

#[test]
fn caption_ripple_delete_requires_exact_basis_policy_scope_and_bounded_metadata_without_changes() {
    for mutate in 0..7 {
        let mut s = caption_ripple_fixture();
        let mut op = caption_ripple_delete(&s);
        match mutate {
            0 => {
                op["params"].as_object_mut().unwrap().remove("captionBasis");
            }
            1 => op["params"]["captionBasis"]["version"] = json!(2),
            2 => op["params"]["captionBasis"]["beforeSha256"] = json!("f".repeat(64)),
            3 => op["params"]["captionBasis"]["scope"]["projectId"] = json!("p_other"),
            4 => op["params"]["captionBasis"]["scope"]["anchorSequence"] = json!(999999),
            5 => op["params"]["captionBasis"]["extra"] = json!(true),
            _ => {
                let id = s.state.active_sequence.unwrap();
                Arc::make_mut(&mut s.project).sequence_mut(id).unwrap().caption_tracks[0].captions[0].text = "x".repeat(8193);
            }
        }
        assert_rejected(&mut s, op);
    }
}

#[test]
fn caption_ripple_delete_rejects_changed_caption_context_and_foreign_collateral_without_publishing() {
    let mut s = caption_ripple_fixture();
    let op = caption_ripple_delete(&s);
    read(&mut s, 1);
    s.execute("captions.setTrack", json!({"track":"C1","locked":true})).unwrap();
    let before = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.len());
    assert!(s.execute("agent.apply", exact_batch(1, vec![op])).is_err());
    assert_eq!((s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.len()), before);
    let mut s = caption_ripple_fixture();
    let sequence = s.state.active_sequence.unwrap();
    let original = s.active_sequence().unwrap().video_tracks[0].items[2].item;
    let mut foreign = s.project.item(original).unwrap().clone();
    let project = Arc::make_mut(&mut s.project);
    foreign.id = ItemId(project.alloc_id());
    foreign
        .metadata
        .insert(super::BINDING_KEY.into(), json!({"projectId":"p_foreign","assetId":"a_foreign","mediaId":"original","role":"original"}).to_string());
    let foreign_id = foreign.id;
    project.items.insert(foreign_id, foreign);
    project.root.children.push(filmcraft_project::BinEntry::Item(foreign_id));
    project.sequence_mut(sequence).unwrap().video_tracks[0].items[2].item = foreign_id;
    let op = caption_ripple_delete(&s);
    assert_rejected(&mut s, op);
}

#[test]
fn caption_ripple_trim_requires_explicit_basis_before_publication() {
    for (edge, frames) in [("in", 1), ("in", -1), ("out", 1), ("out", -1)] {
        let mut s = caption_ripple_fixture();
        let clip = primary(&s);
        read(&mut s, 1);
        let before = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.len());
        let error = s
            .execute(
                "agent.apply",
                exact_batch(1, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":edge,"mode":"ripple","deltaFrames":frames}})]),
            )
            .unwrap_err();
        assert!(error.to_string().contains("exact caption basis"));
        assert_eq!((s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.len()), before);
    }
}

fn caption_trim_operation(s: &crate::Session, edge: &str, frames: i64) -> Value {
    let sequence = s.state.active_sequence.unwrap();
    json!({"command":"timeline.trim","params":{"clip":primary(s),"edge":edge,"mode":"ripple","deltaFrames":frames,
        "captionBasis":{"version":1,"beforeSha256":super::captions::summary(s)["sha256"],"scope":{"projectId":"p_test","anchorSequence":sequence.0}}}})
}

#[test]
fn caption_ripple_trim_matches_human_both_edges_directions_created_ids_history_and_saved_reopen() {
    use filmcraft_time::Tick;
    for (edge, frames) in [("in", 1), ("in", -1), ("out", 1), ("out", -1)] {
        let mut s = caption_ripple_fixture();
        let sequence = s.state.active_sequence.unwrap();
        let rate = s.sequence_rate();
        let q = Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap();
        for track in q.all_tracks_mut() {
            track.items[0].source_in = rate.tick_of(4);
        }
        if edge == "out" && frames > 0 {
            let cue = &mut q.caption_tracks[0].captions[1];
            cue.start = rate.tick_of(12) - Tick(7);
            cue.duration = Tick(30);
        }
        let before = s.project.clone();
        let op = caption_trim_operation(&s, edge, frames);
        let mut human = crate::Session::new(s.services.clone());
        human.project = s.project.clone();
        human.state = s.state.clone();
        human.media = s.media.clone();
        human.execute("timeline.trim", json!({"clip":primary(&s),"edge":edge,"mode":"ripple","deltaFrames":frames})).unwrap();
        let r = apply(&mut s, op.clone());
        assert_eq!(s.project, human.project, "edge={edge}, frames={frames}");
        let proof = &r["results"][0]["captionRipple"];
        assert_eq!(proof["policy"], "ripple_trim_v1");
        assert_eq!(proof["edge"], edge);
        assert_eq!(proof["deltaTicks"], rate.tick_of(frames).0);
        assert_eq!(proof["afterSha256"], super::captions::summary(&s)["sha256"]);
        if edge == "out" && frames > 0 {
            assert_eq!(proof["createdCues"], 1);
            assert_eq!(s.project.next_id, before.next_id + 1);
        }
        let edited = s.project.clone();
        assert_eq!(s.execute("agent.apply", exact_batch(1, vec![op])).unwrap(), r);
        undo(&mut s);
        assert_eq!(s.project, before);
        read(&mut s, 3);
        s.execute("agent.redo", json!({"token":token(3),"commit":token(1),"undo":token(2)})).unwrap();
        assert_eq!(s.project, edited);
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *edited);
    }
}

#[test]
fn caption_ripple_trim_rejects_wrong_basis_mixed_batches_and_foreign_collateral_atomically() {
    for case in 0..6 {
        let mut s = caption_ripple_fixture();
        let mut op = caption_trim_operation(&s, "out", -1);
        match case {
            0 => op["params"]["captionBasis"]["beforeSha256"] = json!("f".repeat(64)),
            1 => op["params"]["captionBasis"]["scope"]["projectId"] = json!("p_other"),
            2 => op["params"]["captionBasis"]["version"] = json!(2),
            3 => op["params"]["captionBasis"]["unknown"] = json!(true),
            4 => op["params"]["mode"] = json!("regular"),
            _ => op["params"]["deltaFrames"] = json!(-1000),
        }
        assert_rejected(&mut s, op);
    }
    let mut s = caption_ripple_fixture();
    let op = caption_trim_operation(&s, "out", -1);
    read(&mut s, 1);
    let before = s.project.clone();
    assert!(s.execute("agent.apply", exact_batch(1, vec![op.clone(), op])).is_err());
    assert_eq!(s.project, before);
    let mut s = caption_ripple_fixture();
    let sequence = s.state.active_sequence.unwrap();
    let mut foreign = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video())).unwrap().clone();
    let project = Arc::make_mut(&mut s.project);
    let id = ItemId(project.next_id);
    project.next_id += 1;
    foreign.id = id;
    foreign.metadata.insert(super::BINDING_KEY.into(), json!({"projectId":"p_foreign","assetId":"foreign","mediaId":"foreign","role":"original"}).to_string());
    project.items.insert(id, foreign);
    project.sequence_mut(sequence).unwrap().video_tracks[0].items[2].item = id;
    let op = caption_trim_operation(&s, "out", -1);
    assert_rejected(&mut s, op);
}
