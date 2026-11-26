use super::{
    assembly_tests::fixture,
    tests::{exact_batch, read, token},
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
fn setup() -> crate::Session {
    let (mut s, _) = fixture();
    s.execute("sequence.addTracks", json!({"video":1,"audio":1})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let unit = s.sequence_rate().frame_duration();
    let p = Arc::make_mut(&mut s.project);
    let mut captions = CaptionTrack::new(TrackId(p.alloc_id()), "Exact words".into(), CaptionFormat::Subtitle);
    captions.captions.push(Caption {
        id: ClipId(p.alloc_id()),
        start: Tick(unit.0 * 5 - 7),
        duration: Tick(unit.0 * 10 + 11),
        text: "Keep exact words".into(),
        speaker: Some("Human".into()),
        cue_id: Some("original".into()),
        settings: "align:start".into(),
    });
    captions.vtt_blocks.push("NOTE original metadata".into());
    let q = p.sequence_mut(sid).unwrap();
    q.caption_tracks.push(captions);
    for t in q.all_tracks_mut() {
        if let Some(c) = t.items.first_mut() {
            c.source_in = unit;
            c.speed = 0.75;
            c.gain_db = -7.0;
            c.source_channels = vec![0];
        }
    }
    s
}
fn params(s: &crate::Session, copy: bool, mode: &str, policy: &str, start: i64, picture_only: bool) -> Value {
    let q = s.active_sequence().unwrap();
    let mut placements = vec![json!({"clip":q.video_tracks[0].items[0].id.0,"track":q.video_tracks[1].id.0,"startTicks":start})];
    if !picture_only {
        placements.push(json!({"clip":q.audio_tracks[0].items[0].id.0,"track":q.audio_tracks[1].id.0,"startTicks":start}));
    }
    json!({"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"beforeSha256":super::tracks::digest(q).unwrap(),"beforeCaptionSha256":super::captions::digest(&q.caption_tracks).unwrap(),"copy":copy,"mode":mode,"linkedPolicy":policy,"placements":placements})
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn apply(s: &mut crate::Session, n: usize, p: Value) -> crate::Result<Value> {
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![json!({"command":"agent.arrange","params":p})]))
}
fn reject(s: &mut crate::Session, p: Value, contains: &str) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(74000);
    let before = state(s);
    let e = apply(s, NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), p).unwrap_err().to_string();
    assert!(e.contains(contains), "{e}");
    assert_eq!(state(s), before);
}
fn normalize(s: &crate::Session, before: &filmcraft_project::Project) -> Value {
    let known: BTreeSet<_> = before
        .sequences()
        .filter_map(|i| i.as_sequence())
        .flat_map(|q| q.all_tracks().flat_map(|t| &t.items))
        .filter_map(|c| c.identity.as_ref().map(|x| x.id.clone()))
        .collect();
    let mut p = (*s.project).clone();
    let fresh: BTreeMap<_, _> = p
        .sequences()
        .filter_map(|i| i.as_sequence())
        .flat_map(|q| q.all_tracks().flat_map(|t| &t.items))
        .filter_map(|c| c.identity.as_ref().filter(|x| !known.contains(&x.id)).map(|x| (x.id.clone(), format!("fresh-{}", c.id.0))))
        .collect();
    for i in p.items.values_mut() {
        if let Some(q) = i.as_sequence_mut() {
            for c in q.all_tracks_mut().flat_map(|t| &mut t.items) {
                if let Some(x) = &mut c.identity {
                    if let Some(id) = fresh.get(&x.id) {
                        x.id = id.clone();
                    }
                    if let Some(parent) = x.parent.as_mut().and_then(|id| fresh.get(id).cloned().map(|new| (id, new))) {
                        *parent.0 = parent.1;
                    }
                }
            }
        }
    }
    serde_json::to_value(p).unwrap()
}
fn roundtrip(s: &mut crate::Session, p: Value) -> Value {
    let before = s.project.clone();
    let mut human = crate::Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    let moves = p["placements"].as_array().unwrap().iter().map(|r| json!({"clip":r["clip"],"track":r["track"],"time":r["startTicks"]})).collect::<Vec<_>>();
    human.execute("timeline.move", json!({"moves":moves,"linked":false,"copy":p["copy"],"insert":p["mode"]=="insert"})).unwrap();
    let count = s.history.undo.len();
    let r = apply(s, 71000, p.clone()).unwrap();
    assert_eq!(r["action"], "arrange_edit");
    assert_eq!(r["results"][0]["humanCommand"], "timeline.move");
    assert_eq!(normalize(s, &before), normalize(&human, &before));
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
    assert_eq!(s.history.undo.len(), count + 1);
    let after = s.project.clone();
    read(s, 71001);
    s.execute("agent.undo", json!({"token":token(71001),"commit":token(71000)})).unwrap();
    assert_eq!(s.project, before);
    read(s, 71002);
    s.execute("agent.redo", json!({"token":token(71002),"commit":token(71000),"undo":token(71001)})).unwrap();
    assert_eq!(s.project, after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
    r["results"][0].clone()
}
#[test]
fn cross_track_move_and_copy_in_every_collision_mode_match_current_human_payload_caption_history_and_saved_format() {
    for copy in [false, true] {
        for mode in ["gap", "overwrite", "insert"] {
            let mut s = setup();
            let start = s.sequence_rate().tick_of(5).0;
            let p = params(&s, copy, mode, "preserve", start, false);
            let r = roundtrip(&mut s, p);
            assert_eq!(r["copy"], copy);
            assert_eq!(r["placed"].as_array().unwrap().len(), 2);
            if mode == "insert" {
                assert_eq!(r["collateral"]["createdCues"], 1);
            } else {
                assert_eq!(r["beforeCaptionSha256"], r["afterCaptionSha256"]);
            }
        }
    }
}
#[test]
fn explicit_picture_only_arrangement_keeps_locked_linked_dialogue_in_place_while_preserve_refuses() {
    let mut s = setup();
    s.execute("timeline.setTrack", json!({"track":"A1","locked":true})).unwrap();
    let audio = serde_json::to_value(&s.active_sequence().unwrap().audio_tracks).unwrap();
    let p = params(&s, false, "gap", "preserve", 0, true);
    reject(&mut s, p, "every current linked partner");
    let p = params(&s, false, "gap", "explicit", 0, true);
    roundtrip(&mut s, p);
    assert_eq!(serde_json::to_value(&s.active_sequence().unwrap().audio_tracks).unwrap(), audio);
}
#[test]
fn preserve_requires_matching_offsets_and_does_not_silently_desynchronize_insert_collateral() {
    let mut s = setup();
    let mut p = params(&s, false, "gap", "preserve", 0, false);
    p["placements"][1]["startTicks"] = json!(1);
    reject(&mut s, p, "desynchronize");
    s.execute("timeline.setTrack", json!({"track":"A1","syncLock":false})).unwrap();
    let p = params(&s, true, "insert", "preserve", s.sequence_rate().tick_of(5).0, false);
    reject(&mut s, p, "collateral linked sound");
    let p = params(&s, true, "insert", "explicit", s.sequence_rate().tick_of(5).0, false);
    roundtrip(&mut s, p);
}
#[test]
fn gap_refuses_occupied_and_overwrite_reports_hidden_split_fragments_and_repairs_removed_inspectors() {
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let c = q.video_tracks[0].items[1].id.0;
    let dest = q.video_tracks[1].id.0;
    s.execute("timeline.move", json!({"moves":[{"clip":c,"track":dest,"time":0}],"linked":false})).unwrap();
    let p = params(&s, false, "gap", "explicit", 0, true);
    reject(&mut s, p, "occupied");
    let p = params(&s, false, "overwrite", "explicit", 0, true);
    s.state.selection = vec![ClipId(c)];
    let r = roundtrip(&mut s, p);
    assert_eq!(r["collateral"]["removedClips"], 1);
    assert!(s.state.selection.is_empty());
    let mut s = setup();
    let q = s.active_sequence().unwrap();
    let c = q.video_tracks[0].items[1].id.0;
    let dest = q.video_tracks[1].id.0;
    let unit = s.sequence_rate().frame_duration();
    s.execute("timeline.move", json!({"moves":[{"clip":c,"track":dest,"time":0}],"linked":false})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let q = p.sequence_mut(sid).unwrap();
    q.find_item_mut(ClipId(c)).unwrap().1.duration = Tick(unit.0 * 36);
    let p = params(&s, false, "overwrite", "explicit", unit.0 * 5, true);
    let r = roundtrip(&mut s, p);
    assert_eq!(r["collateral"]["createdFragments"], 1);
    assert_eq!(r["collateral"]["changedClips"], 1);
}
#[test]
fn arrangement_rejects_foreign_stale_malformed_locked_untracked_and_protected_changes_atomically() {
    let mut s = setup();
    let good = params(&s, false, "gap", "preserve", 0, false);
    for (field, value) in [
        ("mode", json!("reorder")),
        ("copy", Value::Null),
        ("linkedPolicy", json!("auto")),
        ("beforeSha256", json!("0".repeat(64))),
        ("beforeCaptionSha256", json!("0".repeat(64))),
    ] {
        let mut p = good.clone();
        p[field] = value;
        reject(&mut s, p, if field.starts_with("before") { "state changed" } else { "arrange" });
    }
    let mut p = good.clone();
    p["scope"]["projectId"] = json!("foreign");
    reject(&mut s, p, "tracked bound current original");
    let mut p = good.clone();
    p["placements"][0]["track"] = good["placements"][1]["track"].clone();
    reject(&mut s, p, "kind");
    let mut p = good.clone();
    p["placements"][0]["startTicks"] = json!(9_007_199_254_740_991i64);
    reject(&mut s, p, "end");
    s.execute("timeline.setTrack", json!({"track":"V2","locked":true})).unwrap();
    let p = params(&s, false, "gap", "preserve", 0, false);
    reject(&mut s, p, "locked");
    s.execute("timeline.setTrack", json!({"track":"V2","locked":false})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    p.sequence_mut(sid).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("Keep".into());
    let p = params(&s, false, "gap", "preserve", 0, false);
    reject(&mut s, p, "protected");
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].identity = None;
    let p = params(&s, false, "gap", "preserve", 0, false);
    let before = state(&s);
    let e = super::arrange::run(&mut s, &serde_json::from_value(json!({"command":"agent.arrange","params":p})).unwrap()).unwrap_err();
    assert!(e.to_string().contains("tracked bound current original"));
    assert_eq!(state(&s), before);
}
#[test]
fn unchanged_linked_audio_need_not_be_replaced_and_holds_reverse_effects_and_source_channels_survive() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let q = p.sequence_mut(sid).unwrap();
    let c = &mut q.video_tracks[0].items[0];
    c.frame_hold = Some(c.source_in);
    c.reverse = true;
    let mut p = params(&s, false, "gap", "preserve", 0, false);
    p["placements"][1]["track"] = json!(s.active_sequence().unwrap().audio_tracks[0].id.0);
    let r = apply(&mut s, 72000, p).unwrap();
    assert_eq!(r["results"][0]["placed"].as_array().unwrap().len(), 2);
    assert!(s.active_sequence().unwrap().video_tracks[1].items[0].frame_hold.is_some());
    assert!(s.active_sequence().unwrap().video_tracks[1].items[0].reverse);
}
#[test]
fn arrangement_is_one_exact_gesture_not_a_raw_or_mixed_batch_and_lost_apply_is_immutable() {
    let mut s = setup();
    let p = params(&s, true, "gap", "preserve", 0, false);
    let op = json!({"command":"agent.arrange","params":p});
    read(&mut s, 73000);
    let before = state(&s);
    let mut bad = exact_batch(73000, vec![op.clone()]);
    bad["exact"] = json!(false);
    assert!(s.execute("agent.apply", bad).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", exact_batch(73000, vec![op.clone(), op.clone()])).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.arrange", p).is_err());
    assert_eq!(state(&s), before);
    let request = exact_batch(73000, vec![op]);
    let r = s.execute("agent.apply", request.clone()).unwrap();
    let after = state(&s);
    assert_eq!(s.execute("agent.apply", request).unwrap(), r);
    assert_eq!(state(&s), after);
}
#[test]
fn imported_large_link_groups_use_a_sorted_overlap_sweep_and_respect_half_open_boundaries() {
    let s = setup();
    let mut q = s.active_sequence().unwrap().clone();
    let base = q.video_tracks[0].items[0].clone();
    for t in q.all_tracks_mut() {
        t.items.clear();
        t.transitions.clear();
        t.sync_lock = true;
    }
    q.video_tracks[0].items = (0..12000)
        .map(|n| {
            let mut c = base.clone();
            c.id = ClipId(100000 + n);
            c.start = Tick(n as i64 * 10);
            c.duration = Tick(10);
            c.link = Some(42);
            c
        })
        .collect();
    assert!(super::arrange::preserve_insert(&q, Tick(5), &BTreeSet::new(), &BTreeSet::new()).is_ok());
    let mut audio = base;
    audio.start = Tick(10);
    audio.duration = Tick(10);
    audio.link = Some(42);
    q.audio_tracks[0].items.push(audio);
    q.audio_tracks[0].sync_lock = false;
    assert!(super::arrange::preserve_insert(&q, Tick(5), &BTreeSet::new(), &BTreeSet::new()).is_err());
    assert!(super::arrange::preserve_insert(&q, Tick(20), &BTreeSet::new(), &BTreeSet::new()).is_ok());
}

#[test]
fn hostile_imported_linked_clocks_refuse_below_dispatch_without_integer_overflow_or_publication() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].items[0].start = Tick(i64::MIN);
    let p = params(&s, false, "gap", "preserve", 1, false);
    let before = state(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::arrange::run(&mut s, &serde_json::from_value(json!({"command":"agent.arrange","params":p})).unwrap())
    }));
    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn copies_can_read_locked_protected_originals_without_editing_them_and_keep_their_annotations() {
    let mut s = setup();
    s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let c = &mut p.sequence_mut(sid).unwrap().video_tracks[0].items[0];
    c.identity.as_mut().unwrap().protection = Some("Keep original".into());
    c.identity.as_mut().unwrap().annotations.insert("editor_note".into(), "Chosen take".into());
    let before = s.project.clone();
    let p = params(&s, true, "gap", "preserve", s.sequence_rate().tick_of(5).0, false);
    roundtrip(&mut s, p);
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].items, before.sequence(sid).unwrap().video_tracks[0].items);
    let copy = &s.active_sequence().unwrap().video_tracks[1].items[0];
    assert_eq!(copy.identity.as_ref().unwrap().protection.as_deref(), Some("Keep original"));
    assert_eq!(copy.identity.as_ref().unwrap().annotations.get("editor_note").unwrap(), "Chosen take");
}
