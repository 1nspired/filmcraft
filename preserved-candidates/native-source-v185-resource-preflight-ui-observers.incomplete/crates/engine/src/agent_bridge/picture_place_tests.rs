use super::{
    assembly_tests::fixture,
    captions, picture_place, project_audit, source_edit,
    tests::{exact_batch, read, token},
    tracks,
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, ItemId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(170_000);
fn hash(v: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(project_audit::value_bytes(v).unwrap()))
}
fn setup() -> (crate::Session, u64) {
    let (mut s, item) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let u = s.sequence_rate().frame_duration().0;
    let p = Arc::make_mut(&mut s.project);
    let mut c = CaptionTrack::new(TrackId(p.alloc_id()), "Authored words".into(), CaptionFormat::Subtitle);
    c.captions.push(Caption {
        id: ClipId(p.alloc_id()),
        start: Tick(u * 3 + 7),
        duration: Tick(u * 25 + 9),
        text: "Retain these exact words".into(),
        speaker: Some("Speaker".into()),
        cue_id: Some("original-cue".into()),
        settings: "align:start".into(),
    });
    p.sequence_mut(seq).unwrap().caption_tracks.push(c);
    s.execute("markers.add", json!({"name":"Authored moment","time":u*20})).unwrap();
    s.state.ripple_sequence_markers = true;
    (s, item)
}
fn op(s: &crate::Session, item: u64, target: Option<u64>) -> Value {
    let q = s.active_sequence().unwrap();
    let u = s.sequence_rate().frame_duration().0;
    // Preserve original editorial order even when it is not timeline order.
    json!({"command":picture_place::COMMAND,"params":{
        "scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "beforeStateSha256":source_edit::digest(s).unwrap(),"beforeSha256":tracks::digest(q).unwrap(),
        "beforeCaptionSha256":captions::digest(&q.caption_tracks).unwrap(),"placementDefaultsSha256":source_edit::placement_defaults(s).unwrap(),
        "planSha256":"a".repeat(64),"placementSha256":"b".repeat(64),"targetTrack":target,
        "pictureOnly":true,"ripple":false,"captionPolicy":"preserve-authored",
        "windows":[
            {"index":0,"item":item,"assetId":"a_source","mediaId":"m_original","startTicks":u*20,"sourceInTicks":u*8,"durationTicks":u*4},
            {"index":1,"item":item,"assetId":"a_source","mediaId":"m_original","startTicks":u*4,"sourceInTicks":u*2,"durationTicks":u*6}],
        "replacements":[]}})
}
fn apply(s: &mut crate::Session, operation: Value) -> crate::Result<Value> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![operation]))
}
fn reject(s: &mut crate::Session, operation: Value) -> String {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    let before =
        (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone(), s.events.len(), serde_json::to_value(&s.state).unwrap());
    let e = s.execute("agent.apply", exact_batch(n, vec![operation])).unwrap_err().to_string();
    assert!(Arc::ptr_eq(&s.project, &before.0), "{e}");
    assert_eq!((s.revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3));
    assert_eq!(s.journal, before.4);
    assert_eq!(s.events.len(), before.5);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
    e
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
fn normalized(s: &crate::Session) -> String {
    let mut p = s.project.to_json();
    for t in s.active_sequence().unwrap().all_tracks() {
        for c in &t.items {
            if let Some(i) = &c.identity {
                p = p.replace(&i.id, &format!("instance:{}", c.id.0));
            }
        }
    }
    p
}

#[test]
fn picture_pass_matches_human_placement_and_is_one_owned_history_step() {
    let (mut s, item) = setup();
    let mut human = fork(&s);
    let u = s.sequence_rate().frame_duration().0;
    let before = s.project.to_json();
    let revision = s.revision;
    let history = s.history.undo.len();
    let journal = s.journal.len();
    let operation = op(&s, item, None);
    let receipt = apply(&mut s, operation).unwrap();
    let x = &receipt["results"][0];
    human.execute("sequence.addTracks", json!({"video":1,"audio":0,"submix":0})).unwrap();
    let mut ids = Vec::new();
    for (index, (at, source, duration)) in [(20, 8, 4), (4, 2, 6)].into_iter().enumerate() {
        let r = human
            .execute(
                "timeline.place",
                json!({"item":item,"track":"V2","time":u*at,"sourceIn":u*source,"duration":u*duration,"video":true,"audio":false,"insert":false}),
            )
            .unwrap();
        let id = r["clips"][0].as_u64().unwrap();
        ids.push(ClipId(id));
        let seq = human.state.active_sequence.unwrap();
        let clip = Arc::make_mut(&mut human.project).sequence_mut(seq).unwrap().video_tracks[1].items.iter_mut().find(|c| c.id.0 == id).unwrap();
        let identity = clip.identity.as_mut().unwrap();
        identity.annotations.insert("poema.picture.plan".into(), "a".repeat(64));
        identity.annotations.insert("poema.picture.placement".into(), "b".repeat(64));
        identity.annotations.insert("poema.picture.index".into(), index.to_string());
    }
    human.state.selection = ids;
    assert_eq!(normalized(&s), normalized(&human));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
    assert_eq!(receipt["action"], "picture_placement");
    assert_eq!(x["humanCommands"], json!(["sequence.addTracks", "timeline.place"]));
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), history + 1);
    assert_eq!(s.journal.len(), journal + 1);
    assert_eq!(x["beforeAudioSha256"], x["afterAudioSha256"]);
    assert_eq!(x["beforeCaptionSha256"], x["afterCaptionSha256"]);
    assert_eq!(x["afterStateSha256"], source_edit::digest(&s).unwrap());
    assert_eq!(x["outcomes"][0]["startTicks"], u * 20);
    assert_eq!(x["outcomes"][1]["startTicks"], u * 4);
    let after = s.project.to_json();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    s.execute("agent.undo", json!({"token":token(n),"commit":receipt["token"]})).unwrap();
    assert_eq!(s.project.to_json(), before);
    let redo = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, redo);
    s.execute("agent.redo", json!({"token":token(redo),"commit":receipt["token"],"undo":token(n)})).unwrap();
    assert_eq!(s.project.to_json(), after);
    assert_eq!(filmcraft_project::Project::from_json(&after).unwrap().to_json(), after);
}

fn overlay(s: &mut crate::Session, item: u64) -> u64 {
    s.execute("sequence.addTracks", json!({"video":1,"audio":0,"submix":0})).unwrap();
    let u = s.sequence_rate().frame_duration().0;
    s.execute("timeline.place", json!({"item":item,"track":"V2","time":u*4,"sourceIn":u,"duration":u*6,"video":true,"audio":false,"insert":false})).unwrap();
    s.execute("timeline.place", json!({"item":item,"track":"V2","time":u*30,"sourceIn":u,"duration":u*3,"video":true,"audio":false,"insert":false})).unwrap();
    s.active_sequence().unwrap().video_tracks[1].id.0
}
fn replacement(s: &crate::Session, operation: &mut Value) {
    let c = &s.active_sequence().unwrap().video_tracks[1].items[0];
    operation["params"]["replacements"] = json!([{"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"sha256":hash(c)}]);
}

#[test]
fn picture_pass_replaces_explicit_overlay_and_keeps_retained_material_sound_and_captions() {
    let (mut s, item) = setup();
    let target = overlay(&mut s, item);
    let kept = project_audit::value_bytes(&s.active_sequence().unwrap().video_tracks[1].items[1]).unwrap();
    let audio = hash(&s.active_sequence().unwrap().audio_tracks);
    let captions = hash(&s.active_sequence().unwrap().caption_tracks);
    let mut operation = op(&s, item, Some(target));
    replacement(&s, &mut operation);
    let receipt = apply(&mut s, operation).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks.len(), 2);
    assert_eq!(q.video_tracks[1].items.len(), 3);
    assert_eq!(project_audit::value_bytes(q.video_tracks[1].items.iter().find(|c| c.start == s.sequence_rate().tick_of(30)).unwrap()).unwrap(), kept);
    assert_eq!(hash(&q.audio_tracks), audio);
    assert_eq!(hash(&q.caption_tracks), captions);
    assert_eq!(receipt["results"][0]["removed"].as_array().unwrap().len(), 1);
    assert_eq!(receipt["results"][0]["createdTrack"], false);
    assert_eq!(receipt["results"][0]["humanCommands"], json!(["edit.clear", "timeline.place"]));
}

#[test]
fn picture_pass_saved_journal_uses_the_same_native_services_without_new_editorial_decisions() {
    let (mut s, item) = setup();
    let mut recovered = fork(&s);
    let operation = op(&s, item, None);
    apply(&mut s, operation).unwrap();
    let (command, params) = s.journal.last().cloned().unwrap();
    assert_eq!(command, "agent.replayBatch");
    recovered.execute(&command, params).unwrap();
    assert_eq!(normalized(&s), normalized(&recovered));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&recovered.state).unwrap());
}

#[test]
fn picture_pass_rechecks_preferences_at_publication_and_cannot_undo_a_later_human_edit() {
    let (mut s, item) = setup();
    let operation = op(&s, item, None);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    let request = exact_batch(n, vec![operation]);
    let batch: super::Batch = serde_json::from_value(request.clone()).unwrap();
    let stage = super::stage_batch(&s, &batch).unwrap();
    let before = s.project.clone();
    s.prefs.media.default_media_scaling = "setToFrameSize".into();
    assert!(super::publish_batch(&mut s, &batch, stage, &request, None).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    let operation = op(&s, item, None);
    let result = apply(&mut s, operation).unwrap();
    s.execute("markers.add", json!({"name":"Later human decision","time":1})).unwrap();
    let cut = s.project.to_json();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    assert!(s.execute("agent.undo", json!({"token":token(n),"commit":result["token"]})).is_err());
    assert_eq!(s.project.to_json(), cut);
}

#[test]
fn picture_pass_refuses_outcome_ids_that_javascript_cannot_represent_exactly() {
    let (mut s, item) = setup();
    Arc::make_mut(&mut s.project).next_id = 9_007_199_254_741_000;
    let operation = op(&s, item, None);
    let error = reject(&mut s, operation);
    assert!(error.contains("exact transport identity bounds"), "{error}");
}

#[test]
fn picture_pass_stop_between_human_windows_leaves_the_live_cut_unmodified() {
    let (s, item) = setup();
    let operation: super::Operation = serde_json::from_value(op(&s, item, None)).unwrap();
    let before = s.project.to_json();
    let mut private = fork(&s);
    let calls = std::cell::Cell::new(0);
    let checkpoint = || {
        calls.set(calls.get() + 1);
        if calls.get() == 6 {
            return Err(crate::commands::bad("agent.placePicturePass", "Stopped between original windows"));
        }
        Ok(())
    };
    let error = picture_place::run(&mut private, &operation, &checkpoint).unwrap_err();
    assert!(error.to_string().contains("Stopped between original windows"));
    assert_eq!(private.active_sequence().unwrap().video_tracks[1].items.len(), 1);
    assert_eq!(s.project.to_json(), before);
    assert_eq!(s.active_sequence().unwrap().video_tracks.len(), 1);
}

#[test]
fn picture_pass_large_original_plan_retains_all_windows_and_one_history_step() {
    let (mut s, item) = setup();
    let seq = s.state.active_sequence.unwrap();
    let u = s.sequence_rate().frame_duration().0;
    let p = Arc::make_mut(&mut s.project);
    p.item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().info.duration = Tick(u * 800);
    for t in p.sequence_mut(seq).unwrap().all_tracks_mut() {
        t.items.last_mut().unwrap().duration = Tick(u * 600);
    }
    let mut operation = op(&s, item, None);
    operation["params"]["windows"] = json!(
        (0..512)
            .map(|i| json!({"index":i,"item":item,"assetId":"a_source","mediaId":"m_original","startTicks":u*i,"sourceInTicks":0,"durationTicks":u}))
            .collect::<Vec<_>>()
    );
    let history = s.history.undo.len();
    let revision = s.revision;
    let journal = s.journal.len();
    let r = apply(&mut s, operation).unwrap();
    let rows = r["results"][0]["outcomes"].as_array().unwrap();
    assert_eq!(rows.len(), 512);
    assert_eq!(s.active_sequence().unwrap().video_tracks[1].items.len(), 512);
    assert_eq!(s.history.undo.len(), history + 1);
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.journal.len(), journal + 1);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["startTicks"], u * i as i64);
        assert_eq!(row["durationTicks"], u);
    }
}

#[test]
fn picture_pass_keeps_exact_existing_off_grid_record_points() {
    let (mut s, item) = setup();
    let mut operation = op(&s, item, None);
    let point = operation["params"]["windows"][1]["startTicks"].as_i64().unwrap() + 7;
    operation["params"]["windows"][1]["startTicks"] = json!(point);
    let r = apply(&mut s, operation).unwrap();
    assert_eq!(r["results"][0]["outcomes"][1]["startTicks"], point);
}

#[test]
fn picture_pass_refuses_malformed_foreign_and_stale_requests_without_publication() {
    for field in [
        "pictureOnly",
        "ripple",
        "captionPolicy",
        "beforeStateSha256",
        "beforeSha256",
        "beforeCaptionSha256",
        "placementDefaultsSha256",
        "planSha256",
        "placementSha256",
        "scope",
        "windows",
        "replacements",
    ] {
        let (mut s, item) = setup();
        let mut operation = op(&s, item, None);
        operation["params"][field] = match field {
            "pictureOnly" => json!(false),
            "ripple" => json!(true),
            "captionPolicy" => json!("ripple"),
            "scope" => json!({"projectId":"other_film","anchorSequence":s.state.active_sequence.unwrap().0}),
            "windows" => json!([]),
            "replacements" => json!([{"clip":1,"identity":"f".repeat(32),"sha256":"e".repeat(64)}]),
            _ => json!("wrong"),
        };
        reject(&mut s, operation);
    }
    for (field, value) in [
        ("startTicks", json!(-1)),
        ("durationTicks", json!(i64::MAX)),
        ("sourceInTicks", json!(9_007_199_254_740_991_i64)),
        ("index", json!(3)),
        ("assetId", json!("foreign")),
        ("mediaId", json!("foreign")),
        ("item", json!(0)),
        ("extra", json!(true)),
    ] {
        let (mut s, item) = setup();
        let mut operation = op(&s, item, None);
        operation["params"]["windows"][1][field] = value;
        reject(&mut s, operation);
    }
}

#[test]
fn picture_pass_refuses_collisions_locked_protected_and_stale_overlay_instances() {
    for mode in ["collision", "locked", "protected", "identity", "digest", "duplicate", "v1", "implicit"] {
        let (mut s, item) = setup();
        let target = overlay(&mut s, item);
        let seq = s.state.active_sequence.unwrap();
        if mode == "locked" {
            Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[1].locked = true;
        }
        if mode == "protected" {
            Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[1].items[0].identity.as_mut().unwrap().protection =
                Some("Keep human picture".into());
        }
        let mut operation = op(&s, item, Some(target));
        if mode != "collision" {
            replacement(&s, &mut operation);
        }
        match mode {
            "identity" => operation["params"]["replacements"][0]["identity"] = json!("f".repeat(32)),
            "digest" => operation["params"]["replacements"][0]["sha256"] = json!("f".repeat(64)),
            "duplicate" => {
                let r = operation["params"]["replacements"][0].clone();
                operation["params"]["replacements"].as_array_mut().unwrap().push(r);
            }
            "v1" => operation["params"]["targetTrack"] = json!(s.active_sequence().unwrap().video_tracks[0].id.0),
            "implicit" => {
                operation["params"]["targetTrack"] = Value::Null;
                operation["params"]["replacements"] = json!([]);
            }
            _ => {}
        }
        reject(&mut s, operation);
    }
}

#[test]
fn picture_pass_retains_the_human_last_frame_policy_without_rounding_the_plan() {
    let (mut s, item) = setup();
    let seq = s.state.active_sequence.unwrap();
    let u = s.sequence_rate().frame_duration().0;
    // The human service retains a full terminal record frame when its source
    // frame starts inside the media. Preserve that exact original policy.
    let media = Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap();
    media.info.duration = Tick(u * 10 - 1);
    let mut operation = op(&s, item, None);
    operation["params"]["windows"][0]["sourceInTicks"] = json!(u);
    operation["params"]["windows"][1]["sourceInTicks"] = json!(u * 4);
    let r = apply(&mut s, operation).unwrap();
    assert_eq!(r["results"][0]["outcomes"][1]["durationTicks"], u * 6);
    assert_eq!(s.project.sequence(seq).unwrap().video_tracks.len(), 2);
}

#[test]
fn picture_pass_discards_earlier_human_placement_when_later_allocation_fails() {
    let (mut s, item) = setup();
    s.execute("sequence.addTracks", json!({"video":1,"audio":0,"submix":0})).unwrap();
    let q = s.active_sequence().unwrap();
    let target = q.video_tracks[1].id.0;
    let inventory =
        q.all_tracks().map(|t| t.items.len() + t.transitions.len()).sum::<usize>() + q.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>();
    let budget = ((inventory + crate::sequence_tools::MAX_TRACKS) * 8 + 64) as u64;
    Arc::make_mut(&mut s.project).next_id = u64::MAX - budget - 4;
    let operation = op(&s, item, Some(target));
    let mut human = fork(&s);
    let u = s.sequence_rate().frame_duration().0;
    human.execute("timeline.place", json!({"item":item,"track":"V2","time":u*20,"sourceIn":u*8,"duration":u*4,"video":true,"audio":false})).unwrap();
    assert!(human.execute("timeline.place", json!({"item":item,"track":"V2","time":u*4,"sourceIn":u*2,"duration":u*6,"video":true,"audio":false})).is_err());
    let message = reject(&mut s, operation);
    assert!(message.contains("ID space exhausted"), "{message}");
    assert!(s.active_sequence().unwrap().video_tracks[1].items.is_empty());
}

#[test]
fn picture_pass_refuses_valid_but_obsolete_state_and_placement_defaults() {
    for field in ["beforeStateSha256", "beforeSha256", "beforeCaptionSha256", "placementDefaultsSha256"] {
        let (mut s, item) = setup();
        let mut operation = op(&s, item, None);
        operation["params"][field] = json!("f".repeat(64));
        assert!(reject(&mut s, operation).contains("changed"));
    }
    let (mut s, item) = setup();
    let operation = op(&s, item, None);
    s.prefs.media.default_media_scaling = "setToFrameSize".into();
    assert!(reject(&mut s, operation).contains("changed"));
}

#[test]
fn picture_pass_rejects_multiple_operations_and_nonexact_batches() {
    let (mut s, item) = setup();
    let operation = op(&s, item, None);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    assert!(s.execute("agent.apply", exact_batch(n, vec![operation.clone(), operation.clone()])).is_err());
    let mut batch = exact_batch(n, vec![operation]);
    batch["exact"] = json!(false);
    assert!(s.execute("agent.apply", batch).is_err());
}
