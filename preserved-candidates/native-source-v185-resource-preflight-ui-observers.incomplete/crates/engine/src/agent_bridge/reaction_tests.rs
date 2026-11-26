use super::{
    assembly_tests::fixture,
    captions, markers, project_audit, reactions, source_edit,
    tests::{exact_batch, read, token},
    tracks,
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, ItemId, MediaRef, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(190_000);
fn hash(v: &impl serde::Serialize) -> String {
    format!("{:x}", Sha256::digest(project_audit::value_bytes(v).unwrap()))
}
fn setup() -> (crate::Session, u64, u64) {
    let (mut s, item) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let u = s.sequence_rate().frame_duration().0;
    // Declared media isolates transaction/processing semantics. Actual decoding
    // and rendered output are exercised by the separate generated MOV acceptance.
    Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().media =
        MediaRef::File { path: "/generated/reaction-transaction.mov".into() };
    s.execute("sequence.addTracks", json!({"video":1,"audio":0})).unwrap();
    let c = s
        .execute("timeline.place", json!({"item":item,"track":"V2","frame":0,"sourceIn":0,"duration":u*36,"video":true,"audio":false,"insert":false}))
        .unwrap()["clips"][0]
        .as_u64()
        .unwrap();
    s.execute("clip.enable", json!({"clips":[c]})).unwrap();
    s.execute("poema.annotateClips", json!({"clips":[c],"annotations":{"editor_note":"Keep the authored reaction processing"}})).unwrap();
    s.execute("effects.apply", json!({"clip":c,"effect":"gaussian_blur"})).unwrap();
    s.execute("effects.setParam", json!({"clip":c,"effect":"gaussian_blur","param":"blurriness","value":17})).unwrap();
    s.execute("effects.setParam", json!({"clip":c,"effect":"motion","param":"scale","value":70,"keyframe":true,"time":0})).unwrap();
    s.execute("effects.setParam", json!({"clip":c,"effect":"motion","param":"scale","value":130,"keyframe":true,"time":u*30})).unwrap();
    s.execute("masks.add", json!({"clip":c,"effect":"opacity","shape":"rectangle","center":[32,24],"size":[24,16]})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    let marker = s.execute("markers.add", json!({"time":u*18,"name":"Authored laugh"})).unwrap()["marker"].as_u64().unwrap();
    let p = Arc::make_mut(&mut s.project);
    let mut caption = CaptionTrack::new(TrackId(p.alloc_id()), "Keep words".into(), CaptionFormat::Subtitle);
    caption.captions.push(Caption {
        id: ClipId(p.alloc_id()),
        start: Tick(u * 3 + 7),
        duration: Tick(u * 20 + 9),
        text: "Authored words stay timed".into(),
        speaker: None,
        cue_id: Some("original-cue".into()),
        settings: "align:start".into(),
    });
    p.sequence_mut(seq).unwrap().caption_tracks.push(caption);
    (s, c, marker)
}
fn operation(s: &crate::Session, clip: u64, marker: u64) -> Value {
    let q = s.active_sequence().unwrap();
    let c = q.find_item(ClipId(clip)).unwrap().1;
    let u = s.sequence_rate().frame_duration().0;
    json!({"command":reactions::COMMAND,"params":{"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
 "beforeProjectSha256":hash(&*s.project),"beforeStateSha256":source_edit::digest(s).unwrap(),"beforeSha256":tracks::digest(q).unwrap(),"beforeCaptionSha256":captions::digest(&q.caption_tracks).unwrap(),"beforeMarkerSha256":markers::digest(&q.markers).unwrap(),
 "planSha256":"a".repeat(64),"decisionsSha256":"b".repeat(64),"audioPolicy":"preserve-authored","captionPolicy":"preserve-authored","transitionPolicy":"preserve-authored",
 "targets":[{"slot":0,"clip":clip,"identity":c.identity.as_ref().unwrap().id,"sha256":hash(c),"assetId":"a_source","mediaId":"m_original"}],
 "steps":[{"type":"split","target":0,"second":1,"timeTicks":u*10},{"type":"split","target":1,"second":2,"timeTicks":u*22},{"type":"enable","target":1,"marker":marker,"kind":"laugh","atTicks":u*18,"strength":0.8}]}})
}
fn apply(s: &mut crate::Session, op: Value) -> crate::Result<Value> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![op]))
}
fn rejected(s: &mut crate::Session, op: Value) -> String {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    let before =
        (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone(), s.events.len(), serde_json::to_value(&s.state).unwrap());
    let e = s.execute("agent.apply", exact_batch(n, vec![op])).unwrap_err().to_string();
    assert!(Arc::ptr_eq(&s.project, &before.0), "{e}");
    assert_eq!(
        (s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone(), s.events.len(), serde_json::to_value(&s.state).unwrap()),
        (before.1, before.2, before.3, before.4, before.5, before.6)
    );
    e
}
fn fork(s: &crate::Session) -> crate::Session {
    let mut n = crate::Session::new(s.services.clone());
    n.project = s.project.clone();
    n.state = s.state.clone();
    n.media = s.media.clone();
    n.prefs = s.prefs.clone();
    n.path = s.path.clone();
    n.revision = s.revision;
    n
}
fn normalized(s: &crate::Session) -> String {
    let mut value = s.project.to_json();
    for t in s.active_sequence().unwrap().all_tracks() {
        for c in &t.items {
            if let Some(i) = &c.identity {
                value = value.replace(&i.id, &format!("instance:{}", c.id.0));
            }
        }
    }
    value
}
fn annotate(s: &mut crate::Session, c: u64, marker: u64, at: i64) {
    let mut a = s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.as_ref().unwrap().annotations.clone();
    for (k, v) in [
        ("poema.reaction.plan", "a".repeat(64)),
        ("poema.reaction.decisions", "b".repeat(64)),
        ("poema.reaction.marker", marker.to_string()),
        ("poema.reaction.kind", "laugh".into()),
        ("poema.reaction.at", at.to_string()),
        ("poema.reaction.strength", "0.8".into()),
    ] {
        a.insert(k.into(), v);
    }
    s.execute("poema.annotateClips", json!({"clips":[c],"annotations":a})).unwrap();
}

#[test]
fn reaction_trace_matches_human_razor_enable_and_preserves_processing_in_one_owned_history_step() {
    let (mut s, c, m) = setup();
    let mut human = fork(&s);
    let u = s.sequence_rate().frame_duration().0;
    let before = s.project.to_json();
    let revision = s.revision;
    let undo = s.history.undo.len();
    let journal = s.journal.len();
    let op = operation(&s, c, m);
    let receipt = apply(&mut s, op).unwrap();
    human.execute("timeline.razor", json!({"clip":c,"time":u*10})).unwrap();
    let right = human.active_sequence().unwrap().video_tracks[1].items.iter().find(|c| c.start.0 == u * 10).unwrap().id.0;
    human.execute("timeline.razor", json!({"clip":right,"time":u*22})).unwrap();
    human.execute("clip.enable", json!({"clips":[right]})).unwrap();
    annotate(&mut human, right, m, u * 18);
    assert_eq!(normalized(&s), normalized(&human));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.journal.len(), journal + 1);
    assert_eq!(receipt["action"], "reaction_reveal");
    let x = &receipt["results"][0];
    assert_eq!(x["beforeAudioSha256"], x["afterAudioSha256"]);
    assert_eq!(x["beforeCaptionSha256"], x["afterCaptionSha256"]);
    assert_eq!(x["beforeMarkerSha256"], x["afterMarkerSha256"]);
    assert_eq!(x["humanCommands"], json!({"timeline.razor":2,"clip.enable":1,"poema.annotateClips":1}));
    assert_eq!(x["outcomes"].as_array().unwrap().len(), 3);
    let after = s.project.to_json();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    s.execute("agent.undo", json!({"token":token(n),"commit":receipt["token"]})).unwrap();
    assert_eq!(s.project.to_json(), before);
    let r = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, r);
    s.execute("agent.redo", json!({"token":token(r),"commit":receipt["token"],"undo":token(n)})).unwrap();
    assert_eq!(s.project.to_json(), after);
}
#[test]
fn reaction_trace_late_failure_preserves_native_redo_and_never_publishes_first_cut() {
    let (mut s, c, m) = setup();
    s.execute("clip.enable", json!({"clips":[c]})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.history.redo.is_empty());
    let mut op = operation(&s, c, m);
    op["params"]["steps"][1]["timeTicks"] = json!(s.sequence_rate().tick_of(100).0);
    let e = rejected(&mut s, op);
    assert!(e.contains("record points"), "{e}");
}
#[test]
fn reaction_trace_rejects_hostile_or_incomplete_params_without_a_panic_or_state_change() {
    let (mut s, c, m) = setup();
    let base = operation(&s, c, m);
    for (key, value) in [
        ("planSha256", json!("bad")),
        ("audioPolicy", json!("change")),
        ("targets", json!([])),
        ("steps", json!([])),
        ("beforeProjectSha256", json!("f".repeat(64))),
    ] {
        let mut op = base.clone();
        op["params"][key] = value;
        rejected(&mut s, op);
    }
    for value in [json!(-1), json!(0.5), json!(9007199254740992u64), Value::Null, json!(true)] {
        let mut op = base.clone();
        op["params"]["targets"][0]["clip"] = value;
        rejected(&mut s, op);
    }
    for (index, key, value) in [
        (0, "target", json!(4)),
        (0, "second", json!(0)),
        (0, "timeTicks", json!(-1)),
        (1, "target", json!(2)),
        (2, "strength", json!(2)),
        (2, "strength", json!("NaN")),
        (2, "marker", json!(0)),
        (2, "kind", json!("invented")),
    ] {
        let mut op = base.clone();
        op["params"]["steps"][index][key] = value;
        rejected(&mut s, op);
    }
    let mut op = base.clone();
    op["params"]["unknown"] = json!(1);
    rejected(&mut s, op);
}
#[test]
fn reaction_trace_requires_current_unprotected_silent_forward_bound_originals() {
    for mode in ["locked", "protected", "linked", "reverse", "hold", "foreign", "changed-effect", "cue-moved"] {
        let (mut s, c, m) = setup();
        let op = operation(&s, c, m);
        let seq = s.state.active_sequence.unwrap();
        let p = Arc::make_mut(&mut s.project);
        match mode {
            "locked" => p.sequence_mut(seq).unwrap().video_tracks[1].locked = true,
            "protected" => p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.identity.as_mut().unwrap().protection = Some("Human".into()),
            "linked" => p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.link = Some(900),
            "reverse" => p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.reverse = true,
            "hold" => p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.frame_hold = Some(Tick(1)),
            "foreign" => {
                let item = p.sequence(seq).unwrap().find_item(ClipId(c)).unwrap().1.item;
                p.item_mut(item).unwrap().metadata.insert(
                    "poema.binding.v1".into(),
                    json!({"projectId":"foreign","assetId":"a_source","mediaId":"m_original","role":"original"}).to_string(),
                );
            }
            "changed-effect" => p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.gain_db = -3.0,
            _ => p.sequence_mut(seq).unwrap().markers.iter_mut().find(|x| x.id.0 == m).unwrap().start = Tick(3),
        }
        rejected(&mut s, op);
    }
}
#[test]
fn reaction_trace_honors_stop_between_human_services_without_live_publication() {
    let (s, c, m) = setup();
    let op =
        super::parse(reactions::COMMAND, &operation(&s, c, m)["params"]).map(|params| super::Operation { command: reactions::COMMAND.into(), params }).unwrap();
    let before = s.project.to_json();
    let mut private = fork(&s);
    let count = std::cell::Cell::new(0);
    let check = || {
        count.set(count.get() + 1);
        if count.get() == 5 {
            return Err(crate::commands::bad(reactions::COMMAND, "Stopped reaction staging"));
        }
        Ok(())
    };
    let error = reactions::run(&mut private, &op, &check).unwrap_err();
    assert!(error.to_string().contains("Stopped"));
    assert_eq!(s.project.to_json(), before);
}
#[test]
fn reaction_trace_annotation_budget_failure_never_drops_existing_notes_or_publishes_splits() {
    let (mut s, c, m) = setup();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let notes = &mut p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.identity.as_mut().unwrap().annotations;
    for i in 0..63 {
        notes.insert(format!("human_{i}"), "Keep this complete note".into());
    }
    let op = operation(&s, c, m);
    assert!(rejected(&mut s, op).contains("annotations"));
}
#[test]
fn reaction_trace_fractional_source_clocks_follow_the_same_human_razor_without_reconstructing_media_time() {
    for speed in [0.5, 1.25, 1.001] {
        let (mut s, c, m) = setup();
        let seq = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.speed = speed;
        let mut human = fork(&s);
        let u = s.sequence_rate().frame_duration().0;
        let op = operation(&s, c, m);
        apply(&mut s, op).unwrap();
        human.execute("timeline.razor", json!({"clip":c,"time":u*10})).unwrap();
        let right = human.active_sequence().unwrap().video_tracks[1].items.iter().find(|c| c.start.0 == u * 10).unwrap().id.0;
        human.execute("timeline.razor", json!({"clip":right,"time":u*22})).unwrap();
        human.execute("clip.enable", json!({"clips":[right]})).unwrap();
        annotate(&mut human, right, m, u * 18);
        assert_eq!(normalized(&s), normalized(&human), "{speed}");
    }
}
#[test]
fn reaction_trace_reports_native_fade_collateral_and_can_require_strict_preservation() {
    for policy in ["preserve-authored", "native-razor"] {
        for frames in [3, 30] {
            let (mut s, c, m) = setup();
            s.execute("sequence.applyVideoTransition", json!({"clip":c,"edge":"out","effect":"cross_dissolve","frames":frames})).unwrap();
            let mut op = operation(&s, c, m);
            op["params"]["transitionPolicy"] = json!(policy);
            if frames == 30 && policy == "preserve-authored" {
                assert!(rejected(&mut s, op).contains("authored transition"));
            } else {
                let count = s.active_sequence().unwrap().video_tracks[1].transitions.len();
                let receipt = apply(&mut s, op).unwrap();
                assert_eq!(s.active_sequence().unwrap().video_tracks[1].transitions.len(), count - usize::from(frames == 30));
                if frames == 30 {
                    assert_eq!(receipt["results"][0]["transitionChanges"][0]["removed"], true);
                }
            }
        }
    }
}
#[test]
fn reaction_trace_large_original_recipe_has_bounded_private_history_and_one_live_step() {
    let (mut s, c, m) = setup();
    let seq = s.state.active_sequence.unwrap();
    let u = s.sequence_rate().frame_duration().0;
    let p = Arc::make_mut(&mut s.project);
    let item = p.sequence(seq).unwrap().find_item(ClipId(c)).unwrap().1.item;
    p.item_mut(item).unwrap().as_media_mut().unwrap().info.duration = Tick(u * 4096);
    p.sequence_mut(seq).unwrap().find_item_mut(ClipId(c)).unwrap().1.duration = Tick(u * 2048);
    let mut op = operation(&s, c, m);
    let mut steps = Vec::new();
    for i in 0..512 {
        steps.push(json!({"type":"split","target":i,"second":i+1,"timeTicks":u*(i+1)*3}));
        steps.push(json!({"type":"enable","target":i,"marker":m,"kind":"laugh","atTicks":u*18,"strength":0.8}));
    }
    op["params"]["steps"] = json!(steps);
    let history = s.history.undo.len();
    let revision = s.revision;
    let receipt = apply(&mut s, op).unwrap();
    assert_eq!(receipt["results"][0]["outcomes"].as_array().unwrap().len(), 513);
    assert_eq!(s.history.undo.len(), history + 1);
    assert_eq!(s.revision, revision + 1);
}
