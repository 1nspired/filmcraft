use super::{
    assembly_tests::fixture,
    tests::{exact_batch, read, token},
};
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(81000);
fn setup() -> crate::Session {
    let (mut s, _) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let unit = s.sequence_rate().frame_duration();
    let p = Arc::make_mut(&mut s.project);
    let mut captions = CaptionTrack::new(TrackId(p.alloc_id()), "Exact imported words".into(), CaptionFormat::Subtitle);
    captions.captions.push(Caption {
        id: ClipId(p.alloc_id()),
        start: Tick(unit.0 * 5 - 7),
        duration: Tick(unit.0 * 19 + 11),
        text: "Keep exact words".into(),
        speaker: Some("Editor".into()),
        cue_id: Some("native-original".into()),
        settings: "align:start".into(),
    });
    captions.vtt_blocks.push("NOTE exact provenance".into());
    let q = p.sequence_mut(seq).unwrap();
    q.caption_tracks.push(captions);
    for t in q.all_tracks_mut() {
        for c in &mut t.items {
            c.source_in = Tick(unit.0 * 6);
            c.gain_db = -7.;
            c.source_channels = vec![0];
        }
    }
    s
}
fn params(s: &crate::Session, kind: &str, explicit: bool) -> Value {
    let q = s.active_sequence().unwrap();
    let tracks = if explicit { vec![&q.video_tracks[0]] } else { vec![&q.video_tracks[0], &q.audio_tracks[0]] };
    let edit = match kind {
        "roll" => json!({"kind":kind,"pairs":tracks.iter().map(|t|json!({"left":t.items[0].id.0,"right":t.items[1].id.0})).collect::<Vec<_>>()}),
        "slip" => json!({"kind":kind,"clips":tracks.iter().map(|t|t.items[1].id.0).collect::<Vec<_>>()}),
        _ => {
            json!({"kind":"slide","clips":tracks.iter().map(|t|json!({"clip":t.items[1].id.0,"previous":t.items[0].id.0,"next":t.items[2].id.0})).collect::<Vec<_>>()})
        }
    };
    json!({"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"beforeSha256":super::tracks::digest(q).unwrap(),"beforeCaptionSha256":super::captions::digest(&q.caption_tracks).unwrap(),"deltaTicks":s.sequence_rate().frame_duration().0,"linkedPolicy":if explicit{"explicit"}else{"preserve"},"captionPolicy":"retain_sequence_timing","edit":edit})
}
fn apply(s: &mut crate::Session, p: Value) -> crate::Result<Value> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![json!({"command":"agent.advancedTrim","params":p})]))
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
fn reject(s: &mut crate::Session, p: Value, text: &str) {
    let before = state(s);
    let e = apply(s, p).unwrap_err().to_string();
    assert!(e.contains(text), "{e}");
    assert_eq!(state(s), before);
}
fn human(s: &mut crate::Session, p: &Value) {
    let kind = p["edit"]["kind"].as_str().unwrap();
    let d = p["deltaTicks"].as_i64().unwrap();
    if kind == "roll" {
        let pair = &p["edit"]["pairs"][0];
        s.execute("timeline.roll", json!({"left":pair["left"],"right":pair["right"],"delta":d,"followLinks":p["linkedPolicy"]=="preserve"})).unwrap();
    } else {
        let id = if kind == "slip" { p["edit"]["clips"][0].clone() } else { p["edit"]["clips"][0]["clip"].clone() };
        if p["linkedPolicy"] == "explicit" {
            s.state.linked_selection = false;
        }
        s.execute(&format!("timeline.{kind}"), json!({"clip":id,"delta":d})).unwrap();
    }
}
#[test]
fn guarded_advanced_trims_match_shared_human_clocks_payload_cues_history_and_saved_format() {
    for kind in ["roll", "slip", "slide"] {
        for direction in [-1, 1] {
            let mut s = setup();
            let mut p = params(&s, kind, false);
            p["deltaTicks"] = json!(p["deltaTicks"].as_i64().unwrap() * direction);
            let before = s.project.clone();
            let mut h = crate::Session::new(s.services.clone());
            h.project = s.project.clone();
            h.state = s.state.clone();
            h.media = s.media.clone();
            human(&mut h, &p);
            let count = s.history.undo.len();
            let r = apply(&mut s, p).unwrap();
            assert_eq!(r["action"], "advanced_trim");
            assert_eq!(r["results"][0]["kind"], kind);
            assert_eq!(s.project.to_json(), h.project.to_json());
            assert_eq!(s.history.undo.len(), count + 1);
            let after = s.project.clone();
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            read(&mut s, n);
            s.execute("agent.undo", json!({"token":token(n),"commit":r["token"]})).unwrap();
            assert_eq!(s.project.to_json(), before.to_json());
            let m = NEXT.fetch_add(1, Ordering::Relaxed);
            read(&mut s, m);
            s.execute("agent.redo", json!({"token":token(m),"commit":r["token"],"undo":token(n)})).unwrap();
            assert_eq!(s.project.to_json(), after.to_json());
            assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), after.to_json());
        }
    }
}
#[test]
fn explicit_trim_keeps_locked_linked_sound_and_does_not_change_linked_selection() {
    for kind in ["roll", "slip", "slide"] {
        let mut s = setup();
        s.execute("timeline.setTrack", json!({"track":"A1","locked":true})).unwrap();
        let audio = s.active_sequence().unwrap().audio_tracks.clone();
        let linked = s.state.linked_selection;
        let mut p = params(&s, kind, true);
        p["linkedPolicy"] = json!("preserve");
        reject(&mut s, p, "preserve");
        let p = params(&s, kind, true);
        apply(&mut s, p).unwrap();
        assert_eq!(s.active_sequence().unwrap().audio_tracks, audio);
        assert_eq!(s.state.linked_selection, linked);
    }
}
#[test]
fn exact_advanced_trims_refuse_clamp_missing_neighbors_bad_units_and_malformed_policies() {
    for kind in ["roll", "slip", "slide"] {
        let mut s = setup();
        for d in [0, i64::MIN, i64::MAX] {
            let mut p = params(&s, kind, false);
            p["deltaTicks"] = json!(d);
            reject(&mut s, p, "bounded");
        }
        let mut p = params(&s, kind, false);
        p["deltaTicks"] = json!(s.sequence_rate().tick_of(-100).0);
        reject(&mut s, p, "clamp");
        let mut p = params(&s, kind, false);
        p["captionPolicy"] = json!("remap_words");
        reject(&mut s, p, "policy");
    }
    let mut s = setup();
    let mut p = params(&s, "slide", false);
    p["edit"]["clips"][0]["previous"] = Value::Null;
    reject(&mut s, p, "neighbors");
    let mut p = params(&s, "slide", false);
    p["edit"]["clips"][0].as_object_mut().unwrap().remove("previous");
    reject(&mut s, p, "explicit previous");
}
#[test]
fn advanced_trim_refuses_locked_protected_foreign_stale_and_unread_changed_neighbor_context() {
    let mut s = setup();
    let mut p = params(&s, "roll", false);
    p["beforeSha256"] = json!("0".repeat(64));
    reject(&mut s, p, "state changed");
    let mut p = params(&s, "roll", false);
    p["scope"]["projectId"] = json!("foreign");
    reject(&mut s, p, "bound original");
    s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
    let p = params(&s, "slip", false);
    reject(&mut s, p, "locked");
    let mut s = setup();
    let id = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute("poema.protectClips", json!({"clips":[id],"reason":"Keep this neighbor"})).unwrap();
    let p = params(&s, "slide", false);
    reject(&mut s, p, "protect");
}
#[test]
fn advanced_trim_is_one_private_exact_gesture_and_duplicate_apply_is_immutable() {
    let mut s = setup();
    let p = params(&s, "roll", false);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    read(&mut s, n);
    let op = json!({"command":"agent.advancedTrim","params":p});
    let request = exact_batch(n, vec![op.clone()]);
    let mut mixed = request.clone();
    mixed["operations"] = json!([op.clone(), op]);
    let before = state(&s);
    assert!(s.execute("agent.apply", mixed).is_err());
    assert_eq!(state(&s), before);
    let mut loose = request.clone();
    loose["exact"] = json!(false);
    assert!(s.execute("agent.apply", loose).is_err());
    assert_eq!(state(&s), before);
    let r = s.execute("agent.apply", request.clone()).unwrap();
    let after = state(&s);
    assert_eq!(s.execute("agent.apply", request).unwrap(), r);
    assert_eq!(state(&s), after);
    assert!(s.execute("agent.advancedTrim", p).is_err());
}
#[test]
fn advanced_trim_rejects_unsafe_imported_clocks_and_held_slip_without_panicking() {
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[1].source_in = Tick(i64::MIN);
    let p = params(&s, "slip", false);
    reject(&mut s, p, "exact current clocks");
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[1].frame_hold = Some(Tick(0));
    let p = params(&s, "slip", false);
    reject(&mut s, p, "held source");
}
