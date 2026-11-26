use crate::{Session, commands, move_ops};
use filmcraft_project::{ClipId, TrackId, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

fn setup() -> (Session, TrackItem, TrackItem) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Native movement","video":3,"audio":3})).unwrap();
    let source = s.project.items.values().find(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).unwrap().id;
    s.execute("timeline.place", json!({"item":source.0,"seconds":0})).unwrap();
    let unit = s.sequence_rate().frame_duration();
    s.edit_sequence("Trimmed human fixture", |q, _, _| {
        for t in q.all_tracks_mut() {
            for c in &mut t.items {
                c.start = Tick(unit.0 * 4);
                c.duration = Tick(unit.0 * 12);
                c.source_in = Tick(unit.0 * 9);
                c.speed = 0.75;
                c.gain_db = -7.0;
                c.source_channels = vec![1, 0];
            }
        }
        Ok(())
    })
    .unwrap();
    let q = s.active_sequence().unwrap();
    let v = q.video_tracks[0].items[0].clone();
    let a = q.audio_tracks[0].items[0].clone();
    (s, v, a)
}
fn tick(s: &Session, n: i64) -> i64 {
    s.sequence_rate().tick_of(n).0
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn row(c: ClipId, track: &str, time: i64) -> Value {
    json!({"clip":c.0,"track":track,"time":time})
}
fn roundtrip(s: &mut Session, p: Value) -> Value {
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let result = s.execute("timeline.move", p).unwrap();
    let after = s.project.clone();
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), s.project.to_json());
    result
}
#[test]
fn movement_rejects_every_malformed_row_or_option_before_any_partial_publication() {
    let (mut s, v, _) = setup();
    s.execute("timeline.setTrack", json!({"track":"V3","name":"Redo survives"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let good = row(v.id, "V2", tick(&s, 24));
    let before = state(&s);
    for bad in [
        Value::Null,
        json!(5),
        json!({}),
        json!({"clip":v.id.0,"track":"V2"}),
        json!({"clip":v.id.0,"track":null,"time":0}),
        json!({"clip":v.id.0,"track":"V2","time":"soon"}),
        json!({"clip":v.id.0,"track":"V2","time":0,"extra":true}),
    ] {
        assert!(s.execute("timeline.move", json!({"moves":[good,bad],"linked":false})).is_err());
        assert_eq!(state(&s), before);
    }
    for key in ["insert", "linked", "copy"] {
        for value in [Value::Null, json!(0), json!("true"), json!([])] {
            let mut p = json!({"moves":[good]});
            p[key] = value;
            assert!(s.execute("timeline.move", p).is_err());
            assert_eq!(state(&s), before);
        }
    }
    for moves in [Value::Null, json!("all"), json!([]), json!([good, good]), Value::Array(vec![good.clone(); 4097])] {
        assert!(s.execute("timeline.move", json!({"moves":moves})).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(s.execute("timeline.move", json!({"moves":[good],"unexpected":true})).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn movement_rejects_wrong_kind_and_overlapping_explicit_destinations_without_losing_history() {
    let (mut s, v, a) = setup();
    let before = state(&s);
    let time = tick(&s, 24);
    for moves in [json!([row(v.id, "A2", time)]), json!([row(a.id, "V2", time)])] {
        assert!(s.execute("timeline.move", json!({"moves":moves,"linked":false})).is_err());
        assert_eq!(state(&s), before);
    }
    let copy = s.execute("timeline.move", json!({"moves":[row(v.id,"V1",time)],"linked":false,"copy":true})).unwrap();
    let second = ClipId(copy["copied"][0].as_u64().unwrap());
    let before = state(&s);
    assert!(s.execute("timeline.move", json!({"moves":[row(v.id,"V2",time),row(second,"V2",time+1)],"linked":false})).is_err());
    assert_eq!(state(&s), before);
    roundtrip(&mut s, json!({"moves":[row(v.id,"V2",time),row(second,"V2",time+v.duration.0)],"linked":false}));
}
#[test]
fn movement_plan_is_the_same_native_operation_and_cannot_cross_document_or_sequence_changes() {
    let (mut direct, v, a) = setup();
    let p = json!({"moves":[row(v.id,"V2",tick(&direct,24)),row(a.id,"A2",tick(&direct,24))],"linked":false,"copy":true});
    let mut prepared = Session::new(direct.services.clone());
    prepared.project = direct.project.clone();
    prepared.state = direct.state.clone();
    prepared.media = direct.media.clone();
    let plan = move_ops::prepare(&prepared, &p).unwrap();
    let raw = direct.execute("timeline.move", p.clone()).unwrap();
    let typed = plan.execute(&mut prepared).unwrap();
    assert_eq!(raw, typed);
    assert_eq!(direct.project.to_json(), prepared.project.to_json());
    assert_eq!(serde_json::to_value(&direct.state).unwrap(), serde_json::to_value(&prepared.state).unwrap());
    let plan = move_ops::prepare(&prepared, &p).unwrap();
    prepared.execute("timeline.setTrack", json!({"track":"V3","name":"Later human"})).unwrap();
    let before = state(&prepared);
    assert!(plan.execute(&mut prepared).is_err());
    assert_eq!(state(&prepared), before);
    let plan = move_ops::prepare(&prepared, &p).unwrap();
    prepared.state.active_sequence = prepared.project.sequences().find(|i| Some(i.id) != prepared.state.active_sequence).map(|i| i.id);
    let before = state(&prepared);
    assert!(plan.execute(&mut prepared).is_err());
    assert_eq!(state(&prepared), before);
}
#[test]
fn exhausted_move_copy_and_boundary_split_ids_return_errors_without_panicking_even_below_execute() {
    for next in [u64::MAX, u64::MAX - 1] {
        for copy in [false, true] {
            let (mut s, v, _) = setup();
            Arc::make_mut(&mut s.project).next_id = next;
            let before = state(&s);
            let p = json!({"moves":[row(v.id,"V2",tick(&s,24))],"copy":copy,"linked":false});
            let r = catch_unwind(AssertUnwindSafe(|| (commands::find("timeline.move").unwrap().run)(&mut s, &p)));
            assert!(r.is_ok(), "native handler must return, not panic");
            assert!(r.unwrap().is_err());
            assert_eq!(state(&s), before);
        }
    }
}
#[test]
fn copies_and_new_tracks_reserve_above_imported_foreign_ids_and_keep_existing_payloads() {
    for new in [false, true] {
        let (mut s, v, a) = setup();
        let active = s.state.active_sequence.unwrap();
        let foreign = s.project.sequences().find(|i| i.id != active).unwrap().id;
        Arc::make_mut(&mut s.project).sequence_mut(foreign).unwrap().video_tracks[0].id = TrackId(5_000_000);
        let old = s.project.clone();
        let p = json!({"moves":[row(v.id,if new{"new"}else{"V2"},tick(&s,24)),row(a.id,if new{"new"}else{"A2"},tick(&s,24))],"copy":true,"linked":false});
        let result = roundtrip(&mut s, p);
        let q = s.active_sequence().unwrap();
        for (index, id) in result["copied"].as_array().unwrap().iter().enumerate() {
            let original = if index == 0 { &v } else { &a };
            let id = ClipId(id.as_u64().unwrap());
            assert!(id.0 > 5_000_000);
            let (_, copy) = q.find_item(id).unwrap();
            assert_eq!(copy.source_in, v.source_in);
            assert_eq!(copy.duration, v.duration);
            assert_eq!(copy.speed, v.speed);
            assert_eq!(copy.gain_db, v.gain_db);
            assert_eq!(copy.source_channels, v.source_channels);
            assert_eq!(copy.effects, original.effects);
        }
        assert_eq!(q.find_item(v.id).unwrap().1, &v);
        assert_eq!(q.find_item(a.id).unwrap().1, &a);
        assert_eq!(s.project.item(foreign).unwrap(), old.item(foreign).unwrap());
        if new {
            assert!(q.video_tracks.last().unwrap().id.0 > 5_000_000);
            assert!(q.audio_tracks.last().unwrap().id.0 > 5_000_000);
        }
    }
}
#[test]
fn current_trim_gain_channels_and_persistent_lineage_survive_cross_track_movement_and_copy() {
    let (mut s, mut v, mut a) = setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    v = q.find_item(v.id).unwrap().1.clone();
    a = q.find_item(a.id).unwrap().1.clone();
    let p = json!({"moves":[row(v.id,"V2",tick(&s,24)),row(a.id,"A2",tick(&s,24))],"linked":false});
    roundtrip(&mut s, p);
    let q = s.active_sequence().unwrap();
    let moved = q.find_item(v.id).unwrap().1;
    assert_eq!(moved.source_in, v.source_in);
    assert_eq!(moved.duration, v.duration);
    assert_eq!(moved.identity, v.identity);
    assert_eq!(moved.effects, v.effects);
    assert_eq!(moved.speed, v.speed);
    let p = json!({"moves":[row(v.id,"V3",tick(&s,48)),row(a.id,"A3",tick(&s,48))],"copy":true,"linked":false});
    let result = roundtrip(&mut s, p);
    let q = s.active_sequence().unwrap();
    let vc = q.find_item(ClipId(result["copied"][0].as_u64().unwrap())).unwrap().1;
    let ac = q.find_item(ClipId(result["copied"][1].as_u64().unwrap())).unwrap().1;
    assert_ne!(vc.identity.as_ref().unwrap().id, v.identity.as_ref().unwrap().id);
    assert_eq!(vc.identity.as_ref().unwrap().parent.as_ref(), Some(&v.identity.as_ref().unwrap().id));
    assert!(vc.link.is_some() && vc.link == ac.link && vc.link != v.link);
    assert_eq!(vc.duration, v.duration);
    assert_eq!(ac.source_channels, a.source_channels);
    assert_eq!(ac.gain_db, a.gain_db);
}
#[test]
fn insertion_uses_native_caption_splits_and_global_identity_reservation_with_exact_saved_timing() {
    let (mut s, v, _) = setup();
    s.execute("captions.newTrack", json!({"format":"Subtitle","language":"ja"})).unwrap();
    let unit = s.sequence_rate().frame_duration();
    s.execute("captions.add", json!({"track":"C1","time":unit.0*2,"duration":unit.0*10,"text":"元の言葉"})).unwrap();
    let old = s.active_sequence().unwrap().caption_tracks[0].clone();
    let p = json!({"moves":[row(v.id,"V2",unit.0*6)],"copy":true,"insert":true,"linked":false});
    roundtrip(&mut s, p);
    let q = s.active_sequence().unwrap();
    let track = &q.caption_tracks[0];
    assert_eq!(track.language, old.language);
    assert_eq!(track.format, old.format);
    assert_eq!(track.style, old.style);
    assert_eq!(track.captions.len(), 2);
    let left = &track.captions[0];
    let right = &track.captions[1];
    assert_eq!((left.start, left.duration), (Tick(unit.0 * 2), Tick(unit.0 * 4)));
    assert_eq!((right.start, right.duration), (Tick(unit.0 * 18), Tick(unit.0 * 6)));
    assert_eq!(left.text, old.captions[0].text);
    assert_eq!(right.text, old.captions[0].text);
    assert_eq!(left.id, old.captions[0].id);
    assert_ne!(left.id, right.id);
}
#[test]
fn no_op_and_out_of_bounds_end_refuse_without_clearing_redo() {
    let (mut s, v, _) = setup();
    s.execute("timeline.setTrack", json!({"track":"V3","name":"Redo"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let before = state(&s);
    for p in [json!({"moves":[row(v.id,"V1",v.start.0)],"linked":false}), json!({"moves":[row(v.id,"V2",Tick::MAX.0-v.duration.0+1)],"linked":false})] {
        assert!(s.execute("timeline.move", p).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn prepared_implicit_link_policy_expires_after_human_toggle_but_explicit_intent_does_not() {
    let (mut s, v, _) = setup();
    let p = json!({"moves":[row(v.id,"V2",tick(&s,24))]});
    let plan = move_ops::prepare(&s, &p).unwrap();
    s.state.linked_selection = !s.state.linked_selection;
    let before = state(&s);
    assert!(plan.execute(&mut s).is_err());
    assert_eq!(state(&s), before);
    let p = json!({"moves":[row(v.id,"V2",tick(&s,24))],"linked":false});
    let plan = move_ops::prepare(&s, &p).unwrap();
    s.state.linked_selection = !s.state.linked_selection;
    assert!(plan.execute(&mut s).is_ok());
}

#[test]
fn an_oversized_implicit_link_group_refuses_before_allocating_or_mutating() {
    let (mut s, v, a) = setup();
    Arc::make_mut(&mut s.project).next_id = 5_000_000;
    let unit = s.sequence_rate().frame_duration();
    s.edit_sequence("Large native link fixture", |q, ctx, _| {
        q.audio_tracks[0].items.clear();
        for i in 0..4096 {
            let mut c = a.clone();
            c.id = ClipId(ctx.alloc());
            c.start = Tick(unit.0 * (i + 4));
            c.duration = unit;
            q.audio_tracks[0].items.push(c);
        }
        Ok(())
    })
    .unwrap();
    let before = state(&s);
    let result = s.execute("timeline.move", json!({"moves":[row(v.id,"V2",tick(&s,24))],"linked":true}));
    assert!(result.unwrap_err().to_string().contains("4096"));
    assert_eq!(state(&s), before);
}
