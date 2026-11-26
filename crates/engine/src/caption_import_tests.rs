use super::*;
use filmcraft_project::{BinEntry, BinId, CaptionTrack, MarkerId};
use std::sync::Arc;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
fn input(sequence: u64) -> Value {
    json!({"sequence":sequence,"sourceDocument":"{\"original\":\"שלום 日本語\",\"conf\":0.2,\"low_conf\":true}","tracks":[{
        "name":"Exact originals","language":"und","format":"CEA-608","enabled":true,"locked":true,"syncLock":false,
        "blocks":["STYLE\n::cue { color: lime; }"],"cues":[{"start":12867180480000i64,"end":12872514816000i64,"text":"He said yes — שלום 日本語","speaker":"Groom","cueId":"original-20","settings":"align:start"}]}]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}

#[test]
fn exact_caption_import_preserves_legacy_ids_inactive_sequence_and_one_history_step() {
    let mut s = demo();
    let target = s.state.active_sequence.unwrap();
    let base = (*s.project).clone();
    let original_sequence = base.sequence(target).unwrap().clone();
    s.execute("file.newSequence", json!({"name":"Active sibling","video":1,"audio":1})).unwrap();
    let before = (*s.project).clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let revision = s.revision;
    let request = input(target.0);
    let receipt = s.execute(COMMAND, request.clone()).unwrap();
    assert_eq!(receipt["captions"], 1);
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    let q = s.project.sequence(target).unwrap();
    let t = &q.caption_tracks[0];
    let c = &t.captions[0];
    assert!(t.id.0 > 900010 && c.id.0 > t.id.0);
    assert!(s.project.next_id > c.id.0);
    assert!(t.locked && !t.sync_lock);
    assert_eq!(t.style.background_color, [0, 0, 0, 255]);
    assert_eq!(t.vtt_blocks, vec!["STYLE\n::cue { color: lime; }"]);
    assert_eq!(c.start.0, 12867180480000);
    assert_eq!(c.end().0, 12872514816000);
    assert_eq!(c.duration.0, 5334336000);
    assert_eq!(c.speaker.as_deref(), Some("Groom"));
    assert_eq!(c.cue_id.as_deref(), Some("original-20"));
    assert_eq!(c.settings, "align:start");
    let raw = &s.project.item(target).unwrap().metadata[ARCHIVE];
    let records: BTreeMap<String, ImportRecord> = serde_json::from_str(raw).unwrap();
    let record = &records[receipt["sourceSha256"].as_str().unwrap()];
    assert_eq!(record.source_document, request["sourceDocument"].as_str().unwrap());
    assert_eq!(record.tracks[0].track, t.id.0);
    assert_eq!(record.tracks[0].captions, vec![c.id.0]);
    let mut stripped = (*s.project).clone();
    stripped.next_id = before.next_id;
    let item = stripped.item_mut(target).unwrap();
    item.metadata.remove(ARCHIVE);
    item.as_sequence_mut().unwrap().caption_tracks.remove(0);
    assert_eq!(stripped, before);
    assert_eq!(s.project.sequence(target).unwrap().video_tracks, original_sequence.video_tracks);
    let after = (*s.project).clone();
    s.undo().unwrap();
    assert_eq!(*s.project, before);
    s.redo().unwrap();
    assert_eq!(*s.project, after);
    let unchanged = state(&s);
    assert!(s.execute(COMMAND, request).unwrap_err().to_string().contains("already imported"));
    assert_eq!(state(&s), unchanged);
    let reopened = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap();
    assert_eq!(reopened.project, after);
}

#[test]
fn caption_file_import_without_a_sequence_is_atomic_and_snaps_as_before() {
    let mut s = Session::default();
    let before = (*s.project).clone();
    let revision = s.revision;
    let undo = s.history.undo.len();
    let bytes = b"1\n00:00:01,001 --> 00:00:02,003\nOriginal cue\n";
    let r = crate::captions::import(&mut s, "original.srt", bytes, filmcraft_captions::Format::Srt, None).unwrap();
    assert_eq!(r["captions"], 1);
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    let q = s.active_sequence().unwrap();
    let c = &q.caption_tracks[0].captions[0];
    let rate = q.settings.frame_rate;
    assert_eq!(c.start, rate.snap_nearest(Tick(TICKS_PER_SECOND + TICKS_PER_SECOND / 1000)));
    let after = (*s.project).clone();
    s.undo().unwrap();
    assert_eq!(*s.project, before);
    s.redo().unwrap();
    assert_eq!(*s.project, after);
}

#[test]
fn malformed_imports_preserve_project_selection_undo_and_redo() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    s.execute("captions.newTrack", json!({"name":"Human redo"})).unwrap();
    s.undo().unwrap();
    for change in 0..12 {
        let mut request = input(seq.0);
        match change {
            0 => request["sequence"] = json!(0),
            1 => request["tracks"][0]["cues"][0]["end"] = request["tracks"][0]["cues"][0]["start"].clone(),
            2 => request["tracks"][0]["cues"][0]["start"] = json!(i64::MIN),
            3 => request["tracks"][0]["cues"][0]["text"] = json!("x".repeat(8193)),
            4 => request["tracks"][0]["cues"].as_array_mut().unwrap().push(input(seq.0)["tracks"][0]["cues"][0].clone()),
            5 => request["tracks"][0]["format"] = json!("unknown"),
            6 => request["tracks"][0]["style"] = json!({"size":48}),
            7 => request["tracks"][0]["unknown"] = json!(true),
            8 => request["sourceDocument"] = json!("x".repeat(512 * 1024 + 1)),
            9 => request["sequence"] = json!(u64::MAX),
            10 => request["tracks"] = json!([]),
            _ => request["tracks"][0]["cues"][0]["text"] = json!("bad\0text"),
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, request).is_err(), "case {change}");
        assert_eq!(state(&s), before, "case {change}");
    }
    for next in [0, u64::MAX - 1, u64::MAX] {
        Arc::make_mut(&mut s.project).next_id = next;
        let before = state(&s);
        assert!(s.execute(COMMAND, input(seq.0)).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn caption_allocation_covers_other_sequences_bins_source_and_instance_markers() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    let mut other = p.sequence(seq).unwrap().clone();
    other.caption_tracks = vec![CaptionTrack::new(TrackId(2_000_000), "Existing".into(), CaptionFormat::Subtitle)];
    p.add_item("Other", filmcraft_project::Label::Iris, filmcraft_project::ItemKind::Sequence(other.into()), None);
    p.root.children.push(BinEntry::Bin(filmcraft_project::Bin { id: BinId(3_000_000), name: "Legacy bin".into(), children: vec![] }));
    let marker = p.sequence(seq).unwrap().markers[0].clone();
    let mut marker2 = marker.clone();
    marker2.id = MarkerId(4_000_000);
    p.items.values_mut().find_map(|i| i.as_media_mut()).unwrap().markers.push(marker2);
    let q = p.sequence_mut(seq).unwrap();
    let c = &mut q.video_tracks[0].items[0];
    let mut marker3 = marker;
    marker3.id = MarkerId(5_000_000);
    c.markers.push(marker3);
    c.group = Some(6_000_000);
    let before = (*s.project).clone();
    let r = s.execute(COMMAND, input(seq.0)).unwrap();
    assert_eq!(r["tracks"][0]["track"], 6_000_001);
    let mut stripped = (*s.project).clone();
    stripped.next_id = before.next_id;
    let item = stripped.item_mut(seq).unwrap();
    item.metadata.remove(ARCHIVE);
    item.as_sequence_mut().unwrap().caption_tracks.remove(0);
    assert_eq!(stripped, before);
}

#[test]
fn noncanonical_or_damaged_archive_refuses_without_overwriting_metadata() {
    for raw in ["{} ", "{\"duplicate\":{},\"duplicate\":{}}", "not JSON"] {
        let mut s = demo();
        let seq = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).item_mut(seq).unwrap().metadata.insert(ARCHIVE.into(), raw.into());
        let before = state(&s);
        assert!(s.execute(COMMAND, input(seq.0)).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn exact_import_preserves_native_format_defaults_explicit_styles_and_off_frame_metadata() {
    for format in ["Subtitle", "CEA-608", "CEA-708", "Teletext"] {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name":"Exact","video":1,"audio":1,"fps":30000.0/1001.0})).unwrap();
        let seq = s.state.active_sequence.unwrap();
        let mut request = input(seq.0);
        request["tracks"][0]["format"] = json!(format);
        request["tracks"][0]["cues"][0]["start"] = json!(-17);
        request["tracks"][0]["cues"][0]["end"] = json!(19);
        s.execute(COMMAND, request.clone()).unwrap();
        let q = s.active_sequence().unwrap();
        let t = &q.caption_tracks[0];
        assert_eq!(t.style, crate::captions::new_track(TrackId(1), &json!({"format":format})).style);
        assert_eq!(t.captions[0].start, Tick(-17));
        assert_eq!(t.captions[0].duration, Tick(36));
        request.as_object_mut().unwrap().remove("sourceDocument");
        let mut style = t.style.clone();
        style.size = 72.0;
        request["tracks"][0]["style"] = json!(style);
        s.execute(COMMAND, request).unwrap();
        assert_eq!(s.active_sequence().unwrap().caption_tracks[0].style, style);
    }
}

#[test]
fn bounded_id_inventory_and_highest_id_overflow_refuse_without_changing_the_project() {
    let mut s = demo();
    let seq = s.state.active_sequence.unwrap();
    let p = Arc::make_mut(&mut s.project);
    p.sequence_mut(seq).unwrap().markers[0].id = MarkerId(u64::MAX);
    let before = state(&s);
    assert!(s.execute(COMMAND, input(seq.0)).is_err());
    assert_eq!(state(&s), before);
    let mut p = Project::default();
    p.root.children = vec![BinEntry::Item(ItemId(1)); 100001];
    assert!(allocation_start(&p, 2).is_err());
}

use filmcraft_time::TICKS_PER_SECOND;
