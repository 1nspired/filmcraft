use std::sync::Arc;

use filmcraft_project::{ClipId, TimeInterpolation};
use serde_json::{Value, json};

use super::assembly_tests::fixture;
use super::tests::{exact_batch, read, token};

fn op(s: &crate::Session, percent: f64, ripple: bool, reverse: bool) -> Value {
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    json!({"command":"agent.retime","params":{"clips":[clip],"projectId":"p_test","speedPercent":percent,
        "reverse":reverse,"ripple":ripple,"interpolation":"frameSampling"}})
}
fn reject(s: &mut crate::Session, request: Value) {
    let before =
        (s.project.clone(), s.revision, s.saved_revision, s.history.undo.len(), s.events.len(), s.journal.len(), serde_json::to_value(&s.state).unwrap());
    match s.execute("agent.snapshot", json!({"token":token(1)})) {
        Ok(_) => assert!(s.execute("agent.apply", exact_batch(1, vec![request])).is_err()),
        Err(error) => assert!(error.to_string().contains("persistent identity"), "malformed tracking must fail before granting a plan: {error}"),
    }
    assert_eq!(s.project, before.0);
    assert_eq!(s.revision, before.1);
    assert_eq!(s.saved_revision, before.2);
    assert_eq!(s.history.undo.len(), before.3);
    assert_eq!(s.events.len(), before.4);
    assert_eq!(s.journal.len(), before.5);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
}

fn captioned() -> crate::Session {
    use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, TrackId};
    use filmcraft_time::Tick;
    let (mut s, _) = fixture();
    let sequence = s.state.active_sequence.unwrap();
    let cut = s.sequence_rate().tick_of(12);
    let inside = s.sequence_rate().tick_of(8);
    let duration = s.sequence_rate().frame_duration();
    let project = Arc::make_mut(&mut s.project);
    for (index, locked, sync_lock) in [(0, false, true), (1, true, true), (2, false, false)] {
        let mut track = CaptionTrack::new(TrackId(90000 + index * 10), "Speed captions".into(), CaptionFormat::Subtitle);
        track.locked = locked;
        track.sync_lock = sync_lock;
        track.vtt_blocks.push("NOTE original speed caption metadata".into());
        for (offset, start, length) in [(1, inside, duration), (2, cut - Tick(7), Tick(30)), (3, cut + Tick(100), Tick(23))] {
            track.captions.push(Caption {
                id: ClipId(90000 + index * 10 + offset),
                start,
                duration: length,
                text: "Keep exact words".into(),
                speaker: Some("Speaker".into()),
                cue_id: Some(format!("source-{index}-{offset}")),
                settings: "align:start".into(),
            });
        }
        project.sequence_mut(sequence).unwrap().caption_tracks.push(track);
    }
    project.next_id = 100000;
    s
}
fn caption_op(s: &crate::Session, percent: f64, reverse: bool) -> Value {
    let mut request = op(s, percent, true, reverse);
    request["params"]["captionBasis"] = json!({"version":1,"beforeSha256":super::captions::summary(s)["sha256"],
        "scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0}});
    request
}

#[test]
fn caption_retime_matches_shared_human_speed_for_shorten_extend_fractional_and_zero_shift_with_owned_history() {
    for (percent, reverse, interpolation) in [
        (40.0, false, "frameSampling"),
        (200.0, true, "frameBlending"),
        (35.0, true, "opticalFlow"),
        (125.0, false, "frameSampling"),
        (100.0, true, "frameSampling"),
    ] {
        let mut s = captioned();
        s.state.linked_selection = false;
        let before = s.project.clone();
        let old = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let clips = vec![old.id.0, s.active_sequence().unwrap().audio_tracks[0].items[0].id.0];
        let mut human = crate::Session::new(s.services.clone());
        human.project = s.project.clone();
        human.state = s.state.clone();
        human.media = s.media.clone();
        human.execute("clip.speedDuration", json!({"clips":clips,"speed":percent,"reverse":reverse,"ripple":true,"interpolation":interpolation})).unwrap();
        let mut request = caption_op(&s, percent, reverse);
        request["params"]["interpolation"] = json!(interpolation);
        let count = s.history.undo.len();
        read(&mut s, 1);
        let receipt = s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
        assert_eq!(s.project, human.project);
        assert_eq!(s.history.undo.len(), count + 1);
        let proof = &receipt["results"][0]["captionRipple"];
        assert_eq!(proof["policy"], "ripple_retime_v1");
        assert_eq!(proof["beforeCues"], 9);
        assert_eq!(proof["afterSha256"], super::captions::summary(&s)["sha256"]);
        let delta = s.active_sequence().unwrap().find_item(old.id).unwrap().1.duration - old.duration;
        assert_eq!(proof["steps"], if delta == filmcraft_time::Tick::ZERO { json!([]) } else { json!([{"cutTicks":old.end().0,"deltaTicks":delta.0}]) });
        assert_eq!(&s.active_sequence().unwrap().caption_tracks[1..], &before.sequence(s.state.active_sequence.unwrap()).unwrap().caption_tracks[1..]);
        let after = s.project.clone();
        read(&mut s, 2);
        s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
        assert_eq!(s.project, before);
        read(&mut s, 3);
        s.execute("agent.redo", json!({"token":token(3),"commit":token(1),"undo":token(2)})).unwrap();
        assert_eq!(s.project, after);
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
    }
}

#[test]
fn caption_retime_multigroup_uses_actual_sequential_native_origins_without_duplicate_linked_ripples() {
    let mut s = captioned();
    let before = s.project.clone();
    let rate = s.sequence_rate();
    let q = s.active_sequence().unwrap();
    let clips = vec![q.video_tracks[0].items[0].id.0, q.video_tracks[0].items[2].id.0];
    let mut human = crate::Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute("clip.speedDuration", json!({"clips":clips,"speed":50,"reverse":false,"ripple":true,"interpolation":"frameSampling"})).unwrap();
    let mut request = caption_op(&s, 50.0, false);
    request["params"]["clips"] = json!(clips);
    read(&mut s, 1);
    let receipt = s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
    assert_eq!(s.project, human.project);
    assert_eq!(receipt["results"][0]["retimed"], 4);
    assert_eq!(
        receipt["results"][0]["captionRipple"]["steps"],
        json!([
        {"cutTicks":rate.tick_of(12).0,"deltaTicks":rate.tick_of(12).0},
        {"cutTicks":rate.tick_of(48).0,"deltaTicks":rate.tick_of(12).0}])
    );
    read(&mut s, 2);
    s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
    assert_eq!(s.project, before);
}

#[test]
fn caption_retime_requires_exact_basis_preserves_protection_and_accepts_empty_tracks() {
    for variant in 0..7 {
        let mut s = captioned();
        let mut request = caption_op(&s, 40.0, false);
        match variant {
            0 => {
                request["params"].as_object_mut().unwrap().remove("captionBasis");
            }
            1 => request["params"]["captionBasis"]["beforeSha256"] = json!("0".repeat(64)),
            2 => request["params"]["captionBasis"]["scope"]["projectId"] = json!("p_foreign"),
            3 => request["params"]["captionBasis"]["scope"]["anchorSequence"] = json!(0),
            4 => request["params"]["ripple"] = json!(false),
            5 => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items[1].id.0;
                s.execute("poema.protectClips", json!({"clips":[clip],"reason":"Keep my choice"})).unwrap();
            }
            _ => request["params"]["captionBasis"]["version"] = json!(2),
        }
        reject(&mut s, request);
    }
    let mut s = captioned();
    let sequence = s.state.active_sequence.unwrap();
    for t in &mut Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().caption_tracks {
        t.captions.clear();
    }
    let request = caption_op(&s, 50.0, false);
    read(&mut s, 1);
    let r = s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
    assert_eq!(r["results"][0]["captionRipple"]["beforeSha256"], r["results"][0]["captionRipple"]["afterSha256"]);
    assert_eq!(r["results"][0]["captionRipple"]["createdCues"], 0);
}

#[test]
fn retime_matches_human_speed_command_linked_sound_undo_redo_and_saved_reopen() {
    for (percent, ripple, reverse, interpolation) in
        [(40.0, true, false, "frameSampling"), (200.0, false, true, "frameBlending"), (35.0, true, true, "opticalFlow"), (125.0, true, false, "frameSampling")]
    {
        let (mut s, _) = fixture();
        s.state.linked_selection = false;
        let before = s.project.clone();
        let q = s.active_sequence().unwrap();
        let clips = vec![q.video_tracks[0].items[0].id.0, q.audio_tracks[0].items[0].id.0];
        let mut request = op(&s, percent, ripple, reverse);
        request["params"]["interpolation"] = json!(interpolation);
        let history = s.history.undo.len();
        let revision = s.revision;
        read(&mut s, 1);
        let batch = exact_batch(1, vec![request]);
        let receipt = s.execute("agent.apply", batch.clone()).unwrap();
        assert_eq!(receipt["validated"], true);
        assert_eq!(receipt["results"][0]["retimed"], 2);
        assert_eq!(s.history.undo.len(), history + 1);
        assert_eq!(s.revision, revision + 1);
        assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt, "a retained identical request is not applied twice");
        assert_eq!(s.revision, revision + 1);
        let after = s.project.clone();
        let loaded = filmcraft_format::decode(&filmcraft_format::encode(&after, false)).unwrap();
        assert_eq!(loaded.project, *after);
        let view = read(&mut s, 2);
        let clip = &view["sequence"]["video"][0]["items"][0];
        assert_eq!(clip["speed"], percent / 100.0);
        assert_eq!(clip["reverse"], reverse);
        assert_eq!(clip["timeInterpolation"], interpolation);
        assert_eq!(clip["frameHold"], Value::Null);
        s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
        assert_eq!(s.project, before);
        s.execute("clip.speedDuration", json!({"clips":clips,"speed":percent,"reverse":reverse,"ripple":ripple,"interpolation":interpolation})).unwrap();
        assert_eq!(s.project, after, "human and agent must reach identical native state");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.project, before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.project, after);
    }
}

#[test]
fn retime_recovery_preserves_one_history_step_and_does_not_replay_a_raw_command() {
    let (mut s, _) = fixture();
    let before = s.project.clone();
    let start = s.journal.len();
    let request = op(&s, 50.0, true, false);
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
    let after = s.project.clone();
    let journal = s.journal[start..].to_vec();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].0, "agent.replayBatch");
    let mut recovered = crate::Session { project: before.clone(), state: s.state.clone(), media: s.media.clone(), ..Default::default() };
    let count = recovered.history.undo.len();
    recovered.execute(&journal[0].0, journal[0].1.clone()).unwrap();
    assert_eq!(recovered.project, after);
    assert_eq!(recovered.history.undo.len(), count + 1);
    recovered.execute("edit.undo", json!({})).unwrap();
    assert_eq!(recovered.project, before);
}

#[test]
fn retime_clamps_locked_linked_members_and_protected_collateral_publish_nothing() {
    for mode in 0..5 {
        let (mut s, _) = fixture();
        let seq = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
        if mode == 1 {
            q.audio_tracks[0].locked = true;
        }
        if mode == 2 {
            let mut track = filmcraft_project::Track::new(filmcraft_project::TrackId(9999), filmcraft_project::TrackKind::Audio, "Non-sync collateral".into());
            track.sync_lock = false;
            track.items.push(q.audio_tracks[0].items.remove(1));
            q.audio_tracks.push(track);
        }
        if mode == 3 {
            let id = q.video_tracks[0].items[1].id.0;
            s.execute("poema.protectClips", json!({"clips":[id],"reason":"Keep this human decision"})).unwrap();
        }
        if mode == 4 {
            Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[1].identity = None;
        }
        let request = op(&s, 50.0, mode != 0, false);
        reject(&mut s, request);
    }
}

#[test]
fn retime_multiple_groups_and_split_edit_durations_follow_the_same_native_command() {
    for mode in 0..3 {
        let (mut s, _) = fixture();
        let seq = s.state.active_sequence.unwrap();
        if mode == 1 {
            let rate = s.sequence_rate();
            let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            q.audio_tracks[0].items[0].duration = rate.tick_of(10);
        }
        if mode == 2 {
            let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            // Multiple selected members of one group on the same track are a
            // native supported case; do not replace its grouping algorithm.
            let link = q.video_tracks[0].items[0].link;
            q.video_tracks[0].items[1].link = link;
            q.audio_tracks[0].items[1].link = link;
        }
        let before = s.project.clone();
        let q = s.active_sequence().unwrap();
        let clips = vec![q.video_tracks[0].items[0].id.0, q.video_tracks[0].items[1].id.0];
        let mut request = op(&s, 200.0, true, false);
        request["params"]["clips"] = json!(clips);
        read(&mut s, 1);
        s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
        let after = s.project.clone();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.project, before);
        s.execute("clip.speedDuration", json!({"clips":clips,"speed":200,"reverse":false,"ripple":true,"interpolation":"frameSampling"})).unwrap();
        assert_eq!(s.project, after);
    }
}

#[test]
fn retime_hostile_shapes_ranges_missing_sources_and_stale_tickets_refuse() {
    for mode in 0..12 {
        let (mut s, source) = fixture();
        let mut request = op(&s, 50.0, true, false);
        match mode {
            0 => request["params"]["speedPercent"] = json!(0),
            1 => request["params"]["speedPercent"] = json!(-10),
            2 => request["params"]["speedPercent"] = json!(1e-300),
            3 => request["params"]["speedPercent"] = json!(1e300),
            4 => request["params"]["clips"] = json!([999999]),
            5 => request["params"]["clips"] = json!([]),
            6 => request["params"]["interpolation"] = json!("guess"),
            7 => request["params"]["projectId"] = json!("foreign"),
            8 => request["params"]["path"] = json!("/not/allowed"),
            9 => {
                request["params"].as_object_mut().unwrap().remove("ripple");
            }
            10 => Arc::make_mut(&mut s.project).item_mut(filmcraft_project::ItemId(source)).unwrap().as_media_mut().unwrap().offline = true,
            _ => {
                let seq = s.state.active_sequence.unwrap();
                Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].identity = None;
            }
        }
        reject(&mut s, request);
    }
    let (mut s, _) = fixture();
    let request = op(&s, 50.0, true, false);
    read(&mut s, 1);
    s.state.linked_selection = !s.state.linked_selection;
    let before = s.project.clone();
    assert!(s.execute("agent.apply", exact_batch(1, vec![request])).is_err());
    assert_eq!(s.project, before);
}

#[test]
fn retime_must_be_one_exact_operation_and_preserves_prior_holds_and_effects() {
    let (mut s, _) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    q.video_tracks[0].items[0].frame_hold = Some(filmcraft_time::Tick(100));
    q.video_tracks[0].items[0].hold_filters = true;
    let request = op(&s, 200.0, false, true);
    read(&mut s, 1);
    let mut batch = exact_batch(1, vec![request.clone()]);
    batch["exact"] = json!(false);
    assert!(s.execute("agent.apply", batch).is_err());
    assert!(s.execute("agent.apply", exact_batch(1, vec![request.clone(), request.clone()])).is_err());
    s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
    let c = s.active_sequence().unwrap().find_item(ClipId(s.active_sequence().unwrap().video_tracks[0].items[0].id.0)).unwrap().1;
    assert_eq!(c.frame_hold, Some(filmcraft_time::Tick(100)));
    assert!(c.hold_filters);
    assert_eq!(c.time_interpolation, TimeInterpolation::FrameSampling);
}

#[test]
fn shared_duration_rule_rejects_hostile_speed_and_keeps_the_native_tick_rounding() {
    use filmcraft_time::Tick;
    for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(filmcraft_edit::duration_at_speed(Tick(101), speed).is_err());
    }
    assert_eq!(filmcraft_edit::duration_at_speed(Tick(101), 2.0).unwrap(), Tick(51));
    assert_eq!(filmcraft_edit::duration_at_speed(Tick(101), 0.4).unwrap(), Tick(253));
}

#[test]
fn repeated_retime_uses_the_current_trimmed_source_span_and_preserves_identity_and_sound() {
    let (mut s, _) = fixture();
    let first = op(&s, 35.0, true, true);
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![first])).unwrap();
    let before = s.project.clone();
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let expected = filmcraft_edit::duration_at_speed(c.source_out() - c.source_in, 2.0).unwrap();
    let carrier = c.identity.clone();
    let second = op(&s, 200.0, false, false);
    read(&mut s, 2);
    s.execute("agent.apply", exact_batch(2, vec![second])).unwrap();
    for t in s.active_sequence().unwrap().all_tracks() {
        assert_eq!(t.items[0].duration, expected);
        assert!(!t.items[0].reverse);
        assert_eq!(t.items[0].speed, 2.0);
    }
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].items[0].identity, carrier);
    read(&mut s, 3);
    s.execute("agent.undo", json!({"token":token(3),"commit":token(2)})).unwrap();
    assert_eq!(s.project, before);
}

#[test]
fn foreign_bound_collateral_is_refused_even_when_only_a_local_clip_was_targeted() {
    let (mut s, _) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let source = s.active_sequence().unwrap().video_tracks[0].items[1].item;
    let project = Arc::make_mut(&mut s.project);
    let mut foreign = project.item(source).unwrap().clone();
    foreign.id = filmcraft_project::ItemId(project.alloc_id());
    foreign
        .metadata
        .insert(super::BINDING_KEY.into(), json!({"projectId":"another-film","assetId":"a_foreign","mediaId":"original","role":"original"}).to_string());
    let foreign_id = foreign.id;
    project.items.insert(foreign_id, foreign);
    project.sequence_mut(seq).unwrap().video_tracks[0].items[1].item = foreign_id;
    let request = op(&s, 50.0, true, false);
    reject(&mut s, request);
}

#[test]
fn retime_keeps_native_fade_following_and_one_undo_step() {
    let (mut s, _) = fixture();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    let gap = s.sequence_rate().tick_of(12);
    let seq = s.state.active_sequence.unwrap();
    for track in Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().all_tracks_mut() {
        for item in track.items.iter_mut().skip(1) {
            item.start += gap;
        }
    }
    s.state.selection = vec![clip];
    s.execute("sequence.applyVideoTransition", json!({"clip":clip.0,"edge":"out","frames":3})).unwrap();
    let before = s.project.clone();
    let request = op(&s, 200.0, false, false);
    read(&mut s, 1);
    s.execute("agent.apply", exact_batch(1, vec![request])).unwrap();
    let q = s.active_sequence().unwrap();
    let changed = q.find_item(clip).unwrap().1;
    let fade = q.video_tracks[0].transitions.iter().find(|t| t.from == Some(clip)).unwrap();
    assert_eq!(fade.end(), changed.end());
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("clip.speedDuration", json!({"clips":[clip.0],"speed":200,"reverse":false,"ripple":false,"interpolation":"frameSampling"})).unwrap();
    assert_eq!(s.project, after);
}

#[test]
fn retime_refuses_caption_ripple_but_non_ripple_keeps_caption_tracks_unchanged() {
    let (mut s, _) = fixture();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.push(filmcraft_project::CaptionTrack::new(
        filmcraft_project::TrackId(9999),
        "Preserved captions".into(),
        filmcraft_project::CaptionFormat::Subtitle,
    ));
    let request = op(&s, 200.0, true, false);
    reject(&mut s, request);
    let captions = s.active_sequence().unwrap().caption_tracks.clone();
    let request = op(&s, 200.0, false, false);
    read(&mut s, 2);
    s.execute("agent.apply", exact_batch(2, vec![request])).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks, captions);
}
