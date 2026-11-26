use super::{
    assembly_tests::fixture,
    captions, source_edit,
    tests::{exact_batch, read, token},
    tracks,
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(91000);
fn setup() -> (crate::Session, u64) {
    let (mut s, item) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let unit = s.sequence_rate().frame_duration();
    let p = Arc::make_mut(&mut s.project);
    let mut c = CaptionTrack::new(TrackId(p.alloc_id()), "Source words".into(), CaptionFormat::Subtitle);
    c.captions.push(Caption {
        id: ClipId(p.alloc_id()),
        start: Tick(unit.0 * 4 - 7),
        duration: Tick(unit.0 * 20 + 9),
        text: "Retain the original words".into(),
        speaker: Some("Source speaker".into()),
        cue_id: Some("source-native".into()),
        settings: "align:start".into(),
    });
    p.sequence_mut(seq).unwrap().caption_tracks.push(c);
    s.state.ripple_sequence_markers = false;
    s.execute("markers.add", json!({"name":"Authored moment","time":unit.0*20})).unwrap();
    s.execute("project.setMarks", json!({"item":item,"in":unit.0*2})).unwrap();
    (s, item)
}
fn fork(s: &crate::Session) -> crate::Session {
    let mut n = crate::Session::new(s.services.clone());
    n.project = s.project.clone();
    n.media = s.media.clone();
    n.state = s.state.clone();
    n.prefs = s.prefs.clone();
    n.path = s.path.clone();
    n.revision = s.revision;
    n
}
fn op(s: &crate::Session, item: u64, mode: &str, explicit: bool) -> Value {
    let q = s.active_sequence().unwrap();
    let u = s.sequence_rate().frame_duration().0;
    let tracks = if explicit { vec![q.video_tracks[0].id.0] } else { vec![q.video_tracks[0].id.0, q.audio_tracks[0].id.0] };
    let edit = match mode {
        "replace" => {
            json!({"kind":"replace","item":item,"assetId":"a_source","mediaId":"m_original","window":{"start":u*2,"end":u*18},"clips":q.all_tracks().filter(|t|tracks.contains(&t.id.0)).map(|t|json!({"clip":t.items[0].id.0,"identity":t.items[0].identity.as_ref().unwrap().id,"sourceInTicks":u*2})).collect::<Vec<_>>()})
        }
        "insert" | "overwrite" => {
            json!({"kind":"place","item":item,"assetId":"a_source","mediaId":"m_original","window":{"start":u,"end":u*4},"at":u*5,"videoTrack":q.video_tracks[0].id.0,"audioTracks":if explicit{vec![]}else{vec![q.audio_tracks[0].id.0]},"mode":mode})
        }
        _ => json!({"kind":"range","window":{"start":u*3,"end":u*8},"tracks":tracks,"mode":mode}),
    };
    json!({"command":"agent.sourceEdit","params":{"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"beforeStateSha256":source_edit::digest(s).unwrap(),"beforeSha256":tracks::digest(q).unwrap(),"beforeCaptionSha256":captions::digest(&q.caption_tracks).unwrap(),"placementDefaultsSha256":if matches!(mode,"insert"|"overwrite"){source_edit::summary(s)["placementDefaultsSha256"].clone()}else{Value::Null},"linkedPolicy":if explicit{"explicit"}else{"preserve"},"markerPolicy":"retain","edit":edit}})
}
fn apply(s: &mut crate::Session, operation: Value) -> crate::Result<Value> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![operation]))
}
fn identities(s: &crate::Session) -> Vec<(u64, String)> {
    s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).filter_map(|c| c.identity.as_ref().map(|i| (c.id.0, i.id.clone()))).collect()
}
fn normalized(s: &crate::Session, old: &[(u64, String)]) -> String {
    let mut result = s.project.to_json();
    for (id, uid) in old {
        result = result.replace(uid, &format!("old:{id}"));
    }
    for (id, uid) in identities(s) {
        if !old.iter().any(|(_, x)| *x == uid) {
            result = result.replace(&uid, &format!("new:{id}"));
        }
    }
    result
}

#[test]
fn source_gestures_share_human_cut_caption_processing_and_one_owned_native_history() {
    for mode in ["replace", "insert", "overwrite", "lift", "extract"] {
        let (mut s, item) = setup();
        let seq = s.state.active_sequence.unwrap();
        let unit = s.sequence_rate().frame_duration();
        if ["lift", "extract"].contains(&mode) {
            let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            q.mark_in = Some(Tick(unit.0 * 3));
            q.mark_out = Some(Tick(unit.0 * 7));
        }
        let mut human = fork(&s);
        let original = identities(&s);
        let before = s.project.to_json();
        let operation = op(&s, item, mode, false);
        let undo = s.history.undo.len();
        let result = apply(&mut s, operation).unwrap();
        match mode {
            "replace" => {
                let ids: Vec<_> = human.active_sequence().unwrap().all_tracks().map(|t| t.items[0].id.0).collect();
                human.execute("clip.replaceFromBin", json!({"item":item,"clips":ids})).unwrap();
            }
            "insert" | "overwrite" => {
                human.execute("timeline.place",json!({"item":item,"track":"V1","audioTrack":"A1","time":unit.0*5,"sourceIn":unit.0,"duration":unit.0*3,"video":true,"audio":true,"insert":mode=="insert"})).unwrap();
            }
            _ => {
                human.execute(if mode == "lift" { "sequence.lift" } else { "sequence.extract" }, json!({})).unwrap();
            }
        }
        assert_eq!(normalized(&s, &original), normalized(&human, &original), "{mode}");
        assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap(), "{mode}");
        assert_eq!(result["action"], "source_edit");
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(result["results"][0]["afterStateSha256"], source_edit::digest(&s).unwrap());
        let after = s.project.to_json();
        // User preferences are not authored cut state and timeline Undo does not own them.
        let authored_hash = source_edit::digest(&s).unwrap();
        s.prefs.media.default_media_scaling = "setToFrameSize".into();
        assert_eq!(source_edit::digest(&s).unwrap(), authored_hash, "{mode}");
        let undo_read = NEXT.fetch_add(1, Ordering::Relaxed);
        read(&mut s, undo_read);
        s.execute("agent.undo", json!({"token":token(undo_read),"commit":result["token"]})).unwrap();
        assert_eq!(s.project.to_json(), before);
        assert_eq!(s.prefs.media.default_media_scaling, "setToFrameSize");
        let redo_read = NEXT.fetch_add(1, Ordering::Relaxed);
        read(&mut s, redo_read);
        s.execute("agent.redo", json!({"token":token(redo_read),"commit":result["token"],"undo":token(undo_read)})).unwrap();
        assert_eq!(s.project.to_json(), after);
        assert_eq!(source_edit::digest(&s).unwrap(), authored_hash);
        assert_eq!(filmcraft_project::Project::from_json(&after).unwrap().to_json(), after);
    }
}

#[test]
fn source_preserve_refuses_locked_sound_but_explicit_picture_replacement_keeps_it() {
    let (mut s, item) = setup();
    s.execute("timeline.setTrack", json!({"track":"A1","locked":true})).unwrap();
    let before = s.project.clone();
    let failed = op(&s, item, "replace", false);
    assert!(apply(&mut s, failed).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    let sound = s.active_sequence().unwrap().audio_tracks.clone();
    let operation = op(&s, item, "replace", true);
    apply(&mut s, operation).unwrap();
    assert_eq!(s.active_sequence().unwrap().audio_tracks, sound);
}

#[test]
fn source_slot_consumption_uses_current_retiming_and_refuses_short_source_without_clamping() {
    let (mut s, item) = setup();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].speed = 0.5;
    let mut operation = op(&s, item, "replace", true);
    operation["params"]["edit"]["window"]["end"] = json!(s.sequence_rate().frame_duration().0 * 7);
    let before = s.project.clone();
    assert!(apply(&mut s, operation).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    let mut operation = op(&s, item, "replace", true);
    operation["params"]["edit"]["window"]["end"] = json!(s.sequence_rate().frame_duration().0 * 9);
    apply(&mut s, operation).unwrap();
}

#[test]
fn source_basis_rejects_changed_placement_defaults_and_exact_mixed_or_missing_intent() {
    let (mut s, item) = setup();
    let operation = op(&s, item, "overwrite", false);
    s.prefs.media.default_media_scaling = if s.prefs.media.default_media_scaling == "none" { "setToFrameSize".into() } else { "none".into() };
    let before = s.project.clone();
    assert!(apply(&mut s, operation).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    let mut operation = op(&s, item, "overwrite", false);
    operation["params"]["edit"].as_object_mut().unwrap().remove("videoTrack");
    assert!(apply(&mut s, operation).is_err());
    let operation = op(&s, item, "overwrite", false);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    let mut batch = exact_batch(n, vec![operation.clone(), operation]);
    assert!(s.execute("agent.apply", batch.clone()).is_err());
    batch["operations"].as_array_mut().unwrap().truncate(1);
    batch["exact"] = json!(false);
    assert!(s.execute("agent.apply", batch).is_err());
}

#[test]
fn source_requests_bind_the_current_persistent_instance_and_original_asset() {
    for field in ["identity", "assetId", "mediaId"] {
        let (mut s, item) = setup();
        let mut operation = op(&s, item, "replace", true);
        if field == "identity" {
            operation["params"]["edit"]["clips"][0][field] = json!("f".repeat(32));
        } else {
            operation["params"]["edit"][field] = json!("other_original");
        }
        let before = s.project.clone();
        let revision = s.revision;
        let history = (s.history.undo.len(), s.history.redo.len());
        assert!(apply(&mut s, operation).is_err(), "{field}");
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!((s.history.undo.len(), s.history.redo.len()), history);
    }
    for mode in ["replace", "extract"] {
        let (mut s, item) = setup();
        let operation = op(&s, item, mode, false);
        s.prefs.media.default_media_scaling = "setToFrameSize".into();
        apply(&mut s, operation).unwrap();
    }
}

#[test]
fn source_snapshot_audio_layout_matches_native_interpretation_and_extra_container_streams() {
    let (mut s, item) = setup();
    let media = Arc::make_mut(&mut s.project).item_mut(filmcraft_project::ItemId(item)).unwrap().as_media_mut().unwrap();
    media.interpret.audio_channels =
        Some(filmcraft_project::AudioChannelMap { format: filmcraft_project::AudioChannels::Stereo, clips: vec![vec![1, 0], vec![3, 2]] });
    media.info.audio_streams.push(media.info.audio_streams[0].clone());
    assert_eq!(
        source_edit::audio_layout(media),
        vec![json!({"audioStream":0,"sourceChannels":[1,0]}), json!({"audioStream":0,"sourceChannels":[3,2]}), json!({"audioStream":1,"sourceChannels":[]})]
    );
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let doc = read(&mut s, n);
    let row = doc["sources"].as_array().unwrap().iter().find(|r| r["item"] == item).unwrap();
    assert_eq!(row["audioPlacements"].as_array().unwrap().len(), 3);
}
