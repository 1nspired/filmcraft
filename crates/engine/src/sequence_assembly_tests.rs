use std::sync::Arc;

use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde_json::{Value, json};

use crate::Session;

fn setup() -> (Session, ItemId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let item = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.video.is_some() && m.info.has_audio())).unwrap().id;
    (s, item)
}
fn component(s: &Session, item: ItemId, kind: &str) -> Value {
    let u = s.sequence_rate().frame_duration().0;
    json!({"key":kind,"item":item.0,"track":if kind=="picture"{0}else{1},"at":0,"duration":u*12,"sourceIn":0,"sourceOut":u*12,
        "speed":1.0,"timing":"nearestFrame","audioStream":if kind=="sound"{Some(0)}else{None},"channels":if kind=="sound"{vec![0]}else{vec![]},
        "link":"av","enabled":true,"gainDb":0.0,"fadeIn":0,"fadeOut":0,"annotations":{"curation":"The human choice stands"},"protection":null})
}
fn request(s: &Session, item: ItemId) -> Value {
    json!({"name":"Original populated stringout","settings":s.active_sequence().unwrap().settings,
        "tracks":[{"kind":"Video","name":"Original picture"},{"kind":"Audio","name":"Original source sound"}],
        "components":[component(s,item,"picture"),component(s,item,"sound")],"markers":[],"metadata":{"poema.original.curation":"Full original notes remain unshortened"}})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn normalize(mut c: filmcraft_project::TrackItem) -> filmcraft_project::TrackItem {
    c.id = ClipId(0);
    c.identity = None;
    c.link = None;
    for e in &mut c.effects {
        e.instance_id = None;
    }
    c
}

#[test]
fn populated_sequence_keeps_existing_project_sources_and_exact_one_step_history() {
    let (mut s, item) = setup();
    s.execute("project.setMarks", json!({"item":item.0,"in":123,"out":456})).unwrap();
    let before = s.project.clone();
    let old_state = s.state.clone();
    let undo = s.history.undo.len();
    let result = s.execute("sequence.assemble", request(&s, item)).unwrap();
    let seq = ItemId(result["sequence"].as_u64().unwrap());
    let after = s.project.clone();
    let q = after.sequence(seq).unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 1);
    assert_eq!(q.audio_tracks[0].items.len(), 1);
    assert_eq!(q.video_tracks[0].items[0].link, q.audio_tracks[0].items[0].link);
    assert!(q.video_tracks[0].items[0].link.is_some());
    for c in q.all_tracks().flat_map(|t| &t.items) {
        assert_eq!(c.identity.as_ref().unwrap().annotations["curation"], "The human choice stands");
        assert_eq!(c.identity.as_ref().unwrap().parent, None);
        assert!(c.effects.iter().all(|e| e.instance_id.is_some()));
    }
    let mut stripped = (*after).clone();
    stripped.items.remove(&seq);
    stripped.root.children.retain(|x| !matches!(x, filmcraft_project::BinEntry::Item(id) if *id==seq));
    stripped.next_id = before.next_id;
    assert_eq!(stripped, *before);
    assert_eq!(s.state.source_item, old_state.source_item);
    assert_eq!(s.state.linked_selection, old_state.linked_selection);
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, after);
}

#[test]
fn construction_matches_human_source_placement_intrinsics_par_and_scaling() {
    for scaling in ["none", "setToFrameSize", "scaleToFrameSize"] {
        let (mut s, item) = setup();
        let video = Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.video.as_mut().unwrap();
        video.width = 4096;
        video.height = 2160;
        video.par = (4, 3);
        s.prefs.media.default_media_scaling = scaling.into();
        let mut p = request(&s, item);
        p["components"].as_array_mut().unwrap().truncate(1);
        p["components"][0]["link"] = Value::Null;
        p["components"][0]["annotations"] = json!({});
        let at = s.active_sequence().unwrap().duration();
        let track = s.active_sequence().unwrap().video_tracks[0].id;
        let range = TimeRange::new(Tick::ZERO, Tick(s.sequence_rate().frame_duration().0 * 12));
        let ids = crate::commands::place_item(&mut s, item, range, at, Some(track), None, false, "Human Source placement", None).unwrap();
        let human = normalize(s.active_sequence().unwrap().find_item(ids[0]).unwrap().1.clone());
        s.execute("edit.undo", json!({})).unwrap();
        let result = s.execute("sequence.assemble", p).unwrap();
        let assembled = normalize(s.project.sequence(ItemId(result["sequence"].as_u64().unwrap())).unwrap().video_tracks[0].items[0].clone());
        let mut expected = human;
        expected.start = Tick::ZERO;
        assert_eq!(assembled, expected, "native scaling policy {scaling}");
    }
}

#[test]
fn assembled_picture_and_pcm_match_independent_human_native_placement_samples() {
    let (mut s, item) = setup();
    let mut p = request(&s, item);
    let channels = s.project.item(item).unwrap().as_media().unwrap().info.audio().unwrap().channels;
    p["components"][1]["channels"] = json!((0..channels).collect::<Vec<_>>());
    let before = s.project.clone();
    let settings = s.active_sequence().unwrap().settings.clone();
    let unit = settings.frame_rate.frame_duration();
    let rate = settings.sample_rate;
    let human = s.edit("Human new sequence", |project, _| Ok(project.new_sequence("Independent human reference", settings.clone(), 1, 1, None))).unwrap();
    s.execute("sequence.open", json!({"item":human.0})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks[0].id;
    let audio = q.audio_tracks[0].id;
    crate::commands::place_item(
        &mut s,
        item,
        TimeRange::new(Tick::ZERO, Tick(unit.0 * 12)),
        Tick::ZERO,
        Some(video),
        Some(audio),
        false,
        "Human Source placement",
        None,
    )
    .unwrap();
    let reference = (0..3).map(|n| s.try_render_program_at(0.03125, Tick(unit.0 * n * 4)).unwrap()).collect::<Vec<_>>();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let pcm = filmcraft_render::audio::mix_sequence(&s.project, s.active_sequence().unwrap(), 0, 4096, &provider);
    assert_eq!(pcm.sample_rate, rate);
    assert!(pcm.channels.iter().flatten().any(|n| n.abs() > 0.001), "Reference must contain real native sound, not silence.");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    let result = s.execute("sequence.assemble", p).unwrap();
    assert_eq!(s.state.active_sequence, Some(ItemId(result["sequence"].as_u64().unwrap())));
    for (n, expected) in reference.iter().enumerate() {
        assert_eq!(&s.try_render_program_at(0.03125, Tick(unit.0 * n as i64 * 4)).unwrap(), expected);
    }
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let actual = filmcraft_render::audio::mix_sequence(&s.project, s.active_sequence().unwrap(), 0, 4096, &provider);
    assert_eq!(actual.channels, pcm.channels);
}

#[test]
fn malformed_source_metadata_and_exhausted_identity_space_refuse_without_publication() {
    for mode in ["picture size", "picture aspect", "picture rate", "sample rate", "channels", "numeric ids", "revision"] {
        let (mut s, item) = setup();
        if mode == "revision" {
            s.revision = u64::MAX;
        } else if mode == "numeric ids" {
            Arc::make_mut(&mut s.project).next_id = 9_007_199_254_740_990;
        } else {
            let m = Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap();
            match mode {
                "picture size" => m.info.video.as_mut().unwrap().width = 0,
                "picture aspect" => m.interpret.par = Some((0, 1)),
                "picture rate" => m.info.video.as_mut().unwrap().frame_rate = FrameRate::new(1, i64::MAX),
                "sample rate" => m.info.audio_streams[0].sample_rate = 0,
                "channels" => m.info.audio_streams[0].channels = u32::MAX,
                _ => {}
            }
        }
        let p = request(&s, item);
        let before = state(&s);
        assert!(s.execute("sequence.assemble", p).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
    }
}

#[test]
fn rounded_source_tails_refuse_before_publication_for_picture_and_sound() {
    for kind in ["picture", "sound"] {
        for (source_rate, source_frames, speed) in
            [(FrameRate::new(24000, 1001), 4, 0.6), (FrameRate::new(60000, 1001), 4, 1.0), (FrameRate::new(60000, 1001), 1, 0.5)]
        {
            let (mut s, item) = setup();
            let source_unit = source_rate.frame_duration();
            let media_end = Tick(source_unit.0 * 100);
            let media = Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap();
            media.info.duration = media_end;
            media.info.video.as_mut().unwrap().frame_rate = source_rate;
            let mut p = request(&s, item);
            let mut c = component(&s, item, kind);
            let window = Tick(source_unit.0 * source_frames);
            let rounded = s.sequence_rate().snap_nearest(filmcraft_edit::duration_at_speed(window, speed).unwrap());
            assert!(rounded.0 as f64 * speed > window.0 as f64);
            c["sourceIn"] = json!(media_end.0 - window.0);
            c["sourceOut"] = json!(media_end.0);
            c["duration"] = json!(rounded.0);
            c["speed"] = json!(speed);
            c["link"] = Value::Null;
            p["components"] = json!([c]);
            let before = state(&s);
            let result = s.execute("sequence.assemble", p);
            assert!(result.is_err(), "{kind}: {source_rate:?}, speed {speed}");
            assert_eq!(state(&s), before, "{kind}: refused tail changed native state");
        }
    }
}

#[test]
fn explicitly_declared_floor_preserves_available_source_and_never_hides_a_duration_change() {
    for kind in ["picture", "sound"] {
        let (mut s, item) = setup();
        let source_rate = FrameRate::new(60000, 1001);
        let source_unit = source_rate.frame_duration().0;
        let end = source_unit * 100;
        let media = Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap();
        media.info.duration = Tick(end);
        media.info.video.as_mut().unwrap().frame_rate = source_rate;
        let mut p = request(&s, item);
        let mut c = component(&s, item, kind);
        let duration = s.sequence_rate().frame_duration().0;
        c["sourceIn"] = json!(end - source_unit * 4);
        c["sourceOut"] = json!(end);
        c["duration"] = json!(duration);
        c["timing"] = json!("floorFrame");
        c["link"] = Value::Null;
        p["components"] = json!([c]);
        let mut wrong = p.clone();
        wrong["components"][0]["duration"] = json!(duration * 2);
        let before = state(&s);
        assert!(s.execute("sequence.assemble", wrong).is_err());
        assert_eq!(state(&s), before);
        let result = s.execute("sequence.assemble", p).unwrap();
        let row = &result["components"][0];
        assert_eq!(row["timing"], "floorFrame");
        assert_eq!(row["requestedSourceOut"], end);
        assert_eq!(row["sourceOut"], end - source_unit * 4 + duration);
        assert_eq!(row["duration"], duration);
        assert_eq!(row["speed"], 1.0);
    }
}

#[test]
fn exact_source_end_and_rounded_windows_with_available_media_keep_original_windows() {
    for exact in [false, true] {
        for kind in ["picture", "sound"] {
            if exact && kind == "picture" {
                continue;
            }
            let (mut s, item) = setup();
            let unit = s.sequence_rate().frame_duration().0;
            let mut p = request(&s, item);
            let mut c = component(&s, item, kind);
            c["link"] = Value::Null;
            let source_in = unit * 3;
            let requested_span = if exact { unit / 3 } else { unit * 8 / 5 };
            let duration = if exact { requested_span } else { unit * 2 };
            c["sourceIn"] = json!(source_in);
            c["sourceOut"] = json!(source_in + requested_span);
            c["duration"] = json!(duration);
            c["timing"] = json!(if exact { "exact" } else { "nearestFrame" });
            // Native rounding is permitted when the effective source interval is
            // actually available. An exact end is inclusive as a half-open bound.
            Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.duration = Tick(source_in + duration);
            p["components"] = json!([c]);
            let result = s.execute("sequence.assemble", p).unwrap();
            let row = &result["components"][0];
            assert_eq!(row["sourceIn"], source_in);
            assert_eq!(row["requestedSourceOut"], source_in + requested_span);
            assert_eq!(row["sourceOut"], source_in + duration);
            assert_eq!(row["speed"], 1.0);
        }
    }
}

#[test]
fn retiming_uses_the_original_camera_window_before_sequence_frame_rounding() {
    let (mut s, item) = setup();
    let mut p = request(&s, item);
    let mut settings = s.active_sequence().unwrap().settings.clone();
    settings.frame_rate = FrameRate::new(24000, 1001);
    p["settings"] = serde_json::to_value(settings).unwrap();
    let source = FrameRate::new(60000, 1001).frame_duration().0 * 13;
    let duration = FrameRate::new(24000, 1001).frame_duration().0 * 13;
    p["components"].as_array_mut().unwrap().truncate(1);
    let c = &mut p["components"][0];
    c["link"] = Value::Null;
    c["sourceOut"] = json!(source);
    c["speed"] = json!(0.4);
    c["duration"] = json!(duration);
    let result = s.execute("sequence.assemble", p).unwrap();
    let q = s.project.sequence(ItemId(result["sequence"].as_u64().unwrap())).unwrap();
    let clip = &q.video_tracks[0].items[0];
    assert_eq!(clip.duration.0, duration);
    assert_eq!(clip.source_out().0, source);
    assert_eq!(clip.speed, 0.4);
    assert!(q.audio_tracks[0].items.is_empty());
}

#[test]
fn explicit_audio_keeps_physical_stream_channels_gain_fades_and_no_implicit_container_copies() {
    let (mut s, item) = setup();
    let media = Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap();
    media.info.audio_streams[0].channels = 4;
    media.info.audio_streams.push(media.info.audio_streams[0].clone());
    let mut p = request(&s, item);
    let u = s.sequence_rate().frame_duration().0;
    let a = &mut p["components"][1];
    a["channels"] = json!([1, 3]);
    a["audioStream"] = json!(1);
    a["gainDb"] = json!(-7.0);
    a["fadeIn"] = json!(u * 2);
    a["fadeOut"] = json!(u * 3);
    a["protection"] = json!("The editor pinned this original source choice.");
    let result = s.execute("sequence.assemble", p).unwrap();
    let q = s.project.sequence(ItemId(result["sequence"].as_u64().unwrap())).unwrap();
    assert_eq!(q.audio_tracks.len(), 1);
    let a = &q.audio_tracks[0].items[0];
    assert_eq!(a.audio_stream, 1);
    assert_eq!(a.source_channels, vec![1, 3]);
    assert_eq!(a.gain_db, -7.0);
    assert!(a.identity.as_ref().unwrap().protection.is_some());
    let fades = &q.audio_tracks[0].transitions;
    assert_eq!(fades.len(), 2);
    assert_eq!(fades[0].start, a.start);
    assert_eq!(fades[0].duration.0, u * 2);
    assert_eq!(fades[0].to, Some(a.id));
    assert_eq!(fades[0].from, None);
    assert_eq!(fades[1].start.0, a.end().0 - u * 3);
    assert_eq!(fades[1].duration.0, u * 3);
    assert_eq!(fades[1].from, Some(a.id));
    assert_eq!(fades[1].to, None);
    assert!(fades.iter().all(|f| f.effect.effect == "constant_gain"));
}

#[test]
fn exact_audio_clocks_and_off_frame_chapters_survive_without_an_invented_picture() {
    let (mut s, item) = setup();
    let mut p = request(&s, item);
    p["tracks"] = json!([{"kind":"Audio","name":"Original exact sound"}]);
    let mut a = component(&s, item, "sound");
    a["track"] = json!(0);
    a["at"] = json!(17);
    a["sourceIn"] = json!(9);
    a["sourceOut"] = json!(10000009);
    a["duration"] = json!(10000000);
    a["timing"] = json!("exact");
    a["link"] = Value::Null;
    p["components"] = json!([a]);
    p["markers"] = json!([{"key":"source-chapter","at":23,"duration":19,"name":"Original chapter","comment":"Preserved source correspondence","kind":"Chapter","color":"Green"}]);
    let r = s.execute("sequence.assemble", p).unwrap();
    let q = s.project.sequence(ItemId(r["sequence"].as_u64().unwrap())).unwrap();
    assert!(q.video_tracks.is_empty());
    assert_eq!(q.audio_tracks[0].items[0].start.0, 17);
    assert_eq!(q.audio_tracks[0].items[0].duration.0, 10000000);
    assert_eq!(q.markers[0].start.0, 23);
    assert_eq!(q.markers[0].duration.0, 19);
}

#[test]
fn complete_773_row_assembly_uses_one_snapshot_and_preserves_all_original_notes() {
    let (mut s, item) = setup();
    let mut p = request(&s, item);
    let mut rows = Vec::new();
    let u = s.sequence_rate().frame_duration().0;
    for n in 0..773 {
        for name in ["picture", "sound"] {
            let mut c = component(&s, item, name);
            c["key"] = json!(format!("{name}-{n}"));
            c["link"] = json!(format!("av-{n}"));
            c["at"] = json!(u * 12 * n);
            c["annotations"] = json!({"curation":"Original editorial note. ".repeat(32)});
            rows.push(c);
        }
    }
    p["components"] = json!(rows);
    let undo = s.history.undo.len();
    let before = s.project.clone();
    let r = s.execute("sequence.assemble", p).unwrap();
    let after = s.project.clone();
    let q = s.project.sequence(ItemId(r["sequence"].as_u64().unwrap())).unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 773);
    assert_eq!(q.audio_tracks[0].items.len(), 773);
    assert_eq!(r["components"].as_array().unwrap().len(), 1546);
    assert_eq!(q.video_tracks[0].items[772].identity.as_ref().unwrap().annotations["curation"], "Original editorial note. ".repeat(32));
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, after);
}

#[test]
fn saved_native_project_reopens_with_exact_components_notes_links_and_effect_identities() {
    let (mut s, item) = setup();
    let mut p = request(&s, item);
    p["components"][0]["protection"] = json!("The human selected this interval.");
    p["metadata"]["poema.original.note"] = json!("Original UTF-8 curation — keeps every word.\n".repeat(64));
    let result = s.execute("sequence.assemble", p).unwrap();
    let seq = ItemId(result["sequence"].as_u64().unwrap());
    let assembled = s.project.item(seq).unwrap().clone();
    let directory = crate::temp_dir().join(format!(
        "filmcraft-assembly-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let file = directory.join("assembled.fcproj");
    s.execute("file.saveAs", json!({"path":file})).unwrap();
    assert_eq!(s.project.item(seq), Some(&assembled));
    let expected = s.project.clone();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file})).unwrap();
    assert_eq!(reopened.project, expected);
    assert_eq!(reopened.state.active_sequence, Some(seq));
    filmcraft_project::clip_identity::validate_project(&reopened.project).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn hostile_requests_refuse_before_any_project_editor_history_or_journal_change() {
    let changes: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        (
            "missing flag",
            Box::new(|p| {
                p["components"][0].as_object_mut().unwrap().remove("enabled");
            }),
        ),
        ("bad bool", Box::new(|p| p["components"][0]["enabled"] = json!("true"))),
        ("missing track", Box::new(|p| p["components"][0]["track"] = json!(99))),
        ("negative source", Box::new(|p| p["components"][0]["sourceIn"] = json!(-1))),
        ("overflow", Box::new(|p| p["components"][0]["at"] = json!(i64::MAX))),
        ("source overrun", Box::new(|p| p["components"][0]["sourceOut"] = json!(9_007_199_254_740_991i64))),
        ("wrong duration", Box::new(|p| p["components"][0]["duration"] = json!(1))),
        ("tiny speed", Box::new(|p| p["components"][0]["speed"] = json!(1e-200))),
        ("zero speed", Box::new(|p| p["components"][0]["speed"] = json!(0))),
        ("duplicate channel", Box::new(|p| p["components"][1]["channels"] = json!([0, 0]))),
        ("wrong stream", Box::new(|p| p["components"][1]["audioStream"] = json!(65))),
        ("duplicate key", Box::new(|p| p["components"][1]["key"] = json!("picture"))),
        (
            "overlap",
            Box::new(|p| {
                let mut c = p["components"][0].clone();
                c["key"] = json!("overlap");
                c["link"] = Value::Null;
                p["components"].as_array_mut().unwrap().push(c);
            }),
        ),
        ("excess notes", Box::new(|p| p["components"][0]["annotations"] = json!({"note":"x".repeat(1025)}))),
        ("reserved scope", Box::new(|p| p["metadata"] = json!({"poema.sequence.v1":"fake scope"}))),
        (
            "settings default",
            Box::new(|p| {
                p["settings"].as_object_mut().unwrap().remove("color");
            }),
        ),
        ("settings unknown", Box::new(|p| p["settings"]["guess"] = json!(true))),
        ("wrong frame rate", Box::new(|p| p["settings"]["frame_rate"] = json!({"num":0,"den":1}))),
        ("fade overrun", Box::new(|p| p["components"][1]["fadeOut"] = json!(i64::MAX))),
    ];
    for (name, change) in changes {
        let (mut s, item) = setup();
        let mut p = request(&s, item);
        change(&mut p);
        let before = state(&s);
        assert!(s.execute("sequence.assemble", p).is_err(), "{name}");
        assert_eq!(state(&s), before, "{name}");
    }
}
