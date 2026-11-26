use crate::{EffectSel, Session};
use filmcraft_project::{AudioChannels, Label, Marker, MarkerId, MarkerKind, Param, ParamValue, TrackId, TrackSend};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn history(s: &mut Session, command: &str, params: Value) -> Value {
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let result = s.execute(command, params).unwrap();
    let after = s.project.clone();
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    result
}
fn high_id(s: &mut Session, id: u64) {
    let sequence = s.state.active_sequence.unwrap();
    let foreign = s.project.sequences().find(|item| item.id != sequence).unwrap().id;
    let p = Arc::make_mut(&mut s.project);
    p.sequence_mut(foreign).unwrap().video_tracks[0].id = TrackId(id);
    p.sequence_mut(sequence).unwrap().markers.push(Marker {
        id: MarkerId(id - 1),
        start: Tick(13),
        duration: Tick::ZERO,
        name: "Imported identity".into(),
        comment: "Keep unchanged".into(),
        kind: MarkerKind::Chapter,
        color: Label::Rose,
    });
}
#[test]
fn track_creation_reserves_above_legacy_ids_without_migrating_existing_payloads() {
    let mut s = demo();
    high_id(&mut s, 5_000_000);
    let sequence = s.state.active_sequence.unwrap();
    let before = s.project.clone();
    let result = history(&mut s, "sequence.addTracks", json!({"video":1,"audio":1,"submix":1,"audioType":"mono","submixType":"5.1"}));
    assert_eq!(result, json!({"video":[5000001],"audio":[5000002],"submix":[5000003]}));
    assert_eq!(s.project.next_id, 5000004);
    let mut expected = (*before).clone();
    expected.next_id = 5000004;
    let q = expected.sequence_mut(sequence).unwrap();
    q.video_tracks.push(filmcraft_project::Track::new(TrackId(5000001), filmcraft_project::TrackKind::Video, format!("Video {}", q.video_tracks.len() + 1)));
    let mut audio = filmcraft_project::Track::new(TrackId(5000002), filmcraft_project::TrackKind::Audio, format!("Audio {}", q.audio_tracks.len() + 1));
    audio.channels = AudioChannels::Mono;
    q.audio_tracks.push(audio);
    let mut submix = filmcraft_project::Track::new(TrackId(5000003), filmcraft_project::TrackKind::Audio, "Submix 1".into());
    submix.channels = AudioChannels::Surround51;
    q.submix_tracks.push(submix);
    assert_eq!(s.project.to_json(), expected.to_json());
}
#[test]
fn mixer_track_creation_shares_bounded_reservation_and_preserves_its_human_name_and_type() {
    let mut s = demo();
    high_id(&mut s, 6_000_000);
    let result = history(&mut s, "mixer.addSubmix", json!({"name":"Human bus","channels":"Mono"}));
    assert_eq!(result, json!({"id":6000001,"ref":"S1"}));
    let q = s.active_sequence().unwrap();
    assert_eq!(q.submix_tracks[0].channels, AudioChannels::Mono);
    assert_eq!(q.submix_tracks[0].name, "Human bus");
    for command in ["mixer.addSubmix", "sequence.addTracks"] {
        Arc::make_mut(&mut s.project).next_id = u64::MAX;
        let before = state(&s);
        assert!(s.execute(command, json!({})).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn caption_track_creation_reserves_identity_and_keeps_native_format_defaults() {
    let mut s = demo();
    high_id(&mut s, 7_000_000);
    let created = history(&mut s, "captions.newTrack", json!({"format":"CEA-608","name":"Original captions","language":"ja"}));
    assert_eq!(created["track"], 7000001);
    let caption = &s.active_sequence().unwrap().caption_tracks[0];
    assert_eq!(caption.id.0, 7000001);
    assert_eq!(caption.language, "ja");
    assert_eq!(caption.style.background_color, [0, 0, 0, 255]);
    let before = s.project.clone();
    history(&mut s, "captions.deleteTrack", json!({"track":7000001}));
    let sequence = s.state.active_sequence.unwrap();
    let mut expected = (*before).clone();
    expected.sequence_mut(sequence).unwrap().caption_tracks.remove(0);
    assert_eq!(s.project.to_json(), expected.to_json());
}
#[test]
fn explicit_track_deletion_keeps_linked_partners_and_repairs_only_removed_targets_and_inspectors() {
    let mut s = demo();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let removed = q.video_tracks[0].id;
    let fallback = q.video_tracks[1].id;
    let clip = q.video_tracks[0].items[0].id;
    let audio = serde_json::to_value(&q.audio_tracks).unwrap();
    let sequence = s.state.active_sequence.unwrap();
    let mut targeting = s.targeting();
    targeting.targeted = vec![removed, fallback];
    targeting.video_dest = Some(removed);
    s.state.targeting.insert(sequence, targeting);
    s.state.selection = vec![clip];
    s.state.selected_effect = Some(EffectSel { clip, effect: "opacity".into(), instance: 0, instance_id: None });
    s.state.selected_mask = Some(crate::masks::MaskSel { clip, effect: 0, mask: 0 });
    history(&mut s, "sequence.deleteTrack", json!({"track":removed.0}));
    let q = s.active_sequence().unwrap();
    assert!(q.track(removed).is_none());
    assert_eq!(serde_json::to_value(&q.audio_tracks).unwrap(), audio);
    assert!(s.state.selection.is_empty());
    assert!(s.state.selected_effect.is_none());
    assert!(s.state.selected_mask.is_none());
    let targeting = s.state.targeting.get(&sequence).unwrap();
    assert_eq!(targeting.video_dest, Some(fallback));
    assert!(!targeting.targeted.contains(&removed));
}
#[test]
fn empty_native_sequences_keep_their_redo_branch_when_bulk_deletion_has_no_targets() {
    let mut s = demo();
    let ids: Vec<_> = s.active_sequence().unwrap().all_tracks().map(|t| t.id.0).collect();
    s.execute("sequence.removeTracks", json!({"tracks":ids})).unwrap();
    s.execute("sequence.addTracks", json!({"video":1})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let before = state(&s);
    assert!(s.execute("sequence.deleteTracks", json!({"video":"empty","audio":"empty"})).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn exact_and_legacy_submix_removal_share_rerouting_send_lane_compaction_and_native_history() {
    for command in ["mixer.deleteSubmix", "sequence.removeTracks"] {
        let mut s = demo();
        let removed = s.execute("mixer.addSubmix", json!({"name":"Removed"})).unwrap()["id"].as_u64().unwrap();
        let kept = s.execute("mixer.addSubmix", json!({"name":"Kept"})).unwrap()["id"].as_u64().unwrap();
        let sequence = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap();
        let t = &mut q.audio_tracks[0];
        t.mixer.output = Some(TrackId(removed));
        t.mixer.sends = vec![TrackSend::new(TrackId(removed)), TrackSend::new(TrackId(kept)), TrackSend::new(TrackId(removed))];
        for (i, level) in [-6.0, -12.0, -18.0].into_iter().enumerate() {
            t.mixer.lanes.insert(format!("send.{i}.level"), Param::new(ParamValue::Float(level)));
        }
        t.mixer.lanes.insert("volume".into(), Param::new(ParamValue::Float(-3.0)));
        let expected_send = t.mixer.sends[1].clone();
        let expected_lane = t.mixer.lanes["send.1.level"].clone();
        let before = s.project.clone();
        let params = if command == "mixer.deleteSubmix" { json!({"strip":removed}) } else { json!({"tracks":[removed]}) };
        history(&mut s, command, params);
        let mut expected = (*before).clone();
        let q = expected.sequence_mut(sequence).unwrap();
        q.submix_tracks.remove(0);
        let t = &mut q.audio_tracks[0];
        t.mixer.output = None;
        t.mixer.sends = vec![expected_send];
        t.mixer.lanes.retain(|k, _| !k.starts_with("send."));
        t.mixer.lanes.insert("send.0.level".into(), expected_lane);
        assert_eq!(s.project.to_json(), expected.to_json());
    }
}
#[test]
fn exact_multi_kind_removal_is_one_gesture_and_does_not_silently_keep_or_remove_extra_tracks() {
    let mut s = demo();
    let bus = s.execute("mixer.addSubmix", json!({})).unwrap()["id"].as_u64().unwrap();
    s.execute("captions.newTrack", json!({"format":"Subtitle"})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks.last().unwrap().id;
    let audio = q.audio_tracks.last().unwrap().id;
    let caption = q.caption_tracks[0].id;
    let sequence = s.state.active_sequence.unwrap();
    let before = s.project.clone();
    let result = history(&mut s, "sequence.removeTracks", json!({"tracks":[video.0,audio.0,bus,caption.0]}));
    assert_eq!(result["deleted"], 4);
    let mut expected = (*before).clone();
    let q = expected.sequence_mut(sequence).unwrap();
    q.video_tracks.pop();
    q.audio_tracks.pop();
    q.submix_tracks.clear();
    q.caption_tracks.remove(0);
    assert_eq!(s.project.to_json(), expected.to_json());
    let ids: Vec<_> = s.active_sequence().unwrap().video_tracks.iter().map(|t| t.id.0).collect();
    history(&mut s, "sequence.removeTracks", json!({"tracks":ids}));
    assert!(s.active_sequence().unwrap().video_tracks.is_empty());
}
#[test]
fn video_track_reorder_preserves_payloads_and_persistent_targets_while_default_names_follow_order() {
    let mut s = demo();
    let sequence = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().video_tracks[0].name = "Human main picture".into();
    let before = s.project.clone();
    let q = before.sequence(sequence).unwrap();
    assert_eq!(q.video_tracks.len(), 3);
    let first = q.video_tracks[0].id;
    let last = q.video_tracks[2].id;
    let mut targeting = s.targeting();
    targeting.video_dest = Some(first);
    s.state.targeting.insert(sequence, targeting);
    let editor = serde_json::to_value(&s.state).unwrap();
    let mut expected = (*before).clone();
    let q = expected.sequence_mut(sequence).unwrap();
    let mut second = q.video_tracks[1].clone();
    second.name = "Video 1".into();
    let mut third = q.video_tracks[2].clone();
    third.name = "Video 2".into();
    q.video_tracks = vec![second, third, q.video_tracks[0].clone()];
    history(&mut s, "sequence.reorderTrack", json!({"track":first.0,"after":last.0}));
    assert_eq!(s.project.to_json(), expected.to_json());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    history(&mut s, "sequence.reorderTrack", json!({"track":first.0,"after":null}));
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].id, first);
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].name, "Human main picture");
}
#[test]
fn audio_submix_and_caption_order_keeps_channels_routes_cues_and_every_track_identity() {
    let mut s = demo();
    s.execute("sequence.addTracks", json!({"video":0,"submix":2,"submixType":"5.1"})).unwrap();
    s.execute("captions.newTrack", json!({"format":"CEA-608","language":"ja"})).unwrap();
    s.execute("captions.newTrack", json!({"format":"Subtitle","language":"en"})).unwrap();
    s.execute("captions.add", json!({"track":"C1","text":"Keep exact words","seconds":1.0})).unwrap();
    let sequence = s.state.active_sequence.unwrap();
    for (track, after) in [("A1", "A3"), ("S1", "S2"), ("C1", "C2")] {
        let before = s.project.clone();
        let id = super::track_ops::resolve(s.active_sequence().unwrap(), &json!(track), "test").unwrap();
        let anchor = super::track_ops::resolve(s.active_sequence().unwrap(), &json!(after), "test").unwrap();
        history(&mut s, "sequence.reorderTrack", json!({"track":track,"after":after}));
        let q = s.active_sequence().unwrap();
        if track.starts_with('C') {
            assert_eq!(q.caption_tracks.last().unwrap().id, id);
            assert_eq!(
                serde_json::to_value(q.caption_tracks.last().unwrap()).unwrap(),
                serde_json::to_value(&before.sequence(sequence).unwrap().caption_tracks[0]).unwrap()
            );
        } else {
            let rows = if track.starts_with('A') { &q.audio_tracks } else { &q.submix_tracks };
            assert_eq!(rows.last().unwrap().id, id);
            assert_eq!(rows[rows.len() - 2].id, anchor);
            let mut old = serde_json::to_value(if track.starts_with('A') {
                &before.sequence(sequence).unwrap().audio_tracks[0]
            } else {
                &before.sequence(sequence).unwrap().submix_tracks[0]
            })
            .unwrap();
            old["name"] = json!(rows.last().unwrap().name);
            assert_eq!(serde_json::to_value(rows.last().unwrap()).unwrap(), old);
        }
    }
}
#[test]
fn malformed_foreign_duplicate_and_noop_track_requests_preserve_project_editor_and_redo() {
    let mut s = demo();
    s.execute("timeline.setTrack", json!({"track":"V1","name":"Temporary"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let first = s.active_sequence().unwrap().video_tracks[0].id.0;
    let foreign = s.project.sequences().find(|item| Some(item.id) != s.state.active_sequence).unwrap().id;
    let foreign = s.project.sequence(foreign).unwrap().video_tracks[0].id.0;
    let before = state(&s);
    for (command, params) in [
        ("sequence.removeTracks", json!({"tracks":[]})),
        ("sequence.removeTracks", json!({"tracks":[first,"V1"]})),
        ("sequence.removeTracks", json!({"tracks":[first,999999]})),
        ("sequence.removeTracks", json!({"tracks":[foreign]})),
        ("sequence.reorderTrack", json!({"track":"V1","after":foreign})),
        ("sequence.reorderTrack", json!({"track":"V1","after":null})),
        ("sequence.reorderTrack", json!({"track":"V1","after":"A1"})),
        ("sequence.reorderTrack", json!({"track":"V1","after":"V1"})),
        ("sequence.reorderTrack", json!({"track":"V1"})),
        ("sequence.addTracks", json!({"audioType":true})),
        ("sequence.addTracks", json!({"video":1,"typo":true})),
        ("mixer.addSubmix", json!({"channels":false})),
        ("mixer.addSubmix", json!({"channels":"Unknown"})),
        ("timeline.setTrack", json!({"track":"V1","muted":null})),
        ("timeline.setTrack", json!({"track":"V1","enabled":"false"})),
        ("timeline.setTrack", json!({"track":"V1","pan":true})),
        ("timeline.setTrack", json!({"track":"A1","volumeDb":16})),
        ("timeline.setTrack", json!({"track":"A1","volumeDb":-97})),
        ("timeline.setTrack", json!({"track":"V1","name":"x".repeat(1025)})),
        ("timeline.setTrack", json!({"track":"V1","name":"nul\0name"})),
    ] {
        assert!(s.execute(command, params.clone()).is_err(), "{command}: {params}");
        assert_eq!(state(&s), before, "{command}: {params}");
    }
}
#[test]
fn strict_shared_track_settings_retain_human_pan_clamping_and_exact_native_undo_redo() {
    let mut s = demo();
    let sequence = s.state.active_sequence.unwrap();
    let before = s.project.clone();
    history(
        &mut s,
        "timeline.setTrack",
        json!({"track":"A1","locked":true,"syncLock":false,"enabled":false,"muted":true,"solo":true,"name":"Music bed","volumeDb":-12,"pan":200,"label":"Teal"}),
    );
    let mut expected = (*before).clone();
    let t = &mut expected.sequence_mut(sequence).unwrap().audio_tracks[0];
    t.locked = true;
    t.sync_lock = false;
    t.enabled = false;
    t.muted = true;
    t.solo = true;
    t.name = "Music bed".into();
    t.volume_db = -12.0;
    t.pan = 100.0;
    t.label = Some(Label::Teal);
    assert_eq!(s.project.to_json(), expected.to_json());
}
