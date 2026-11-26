//! Hostile explicit targets must not become defaults or partial selections.
use crate::Session;
use serde_json::{Value, json};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let id = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.state.selection = vec![id];
    s.execute("clip.rename", json!({"clip":id.0,"name":"Redo must survive rejection"})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("existing human gesture".into());
    s
}

fn rejected(s: &mut Session, command: &str, args: Value) {
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let merge = s.history.merge_key.clone();
    let journal = s.journal.clone();
    assert!(s.execute(command, args.clone()).is_err(), "{command} accepted {args}");
    assert!(std::sync::Arc::ptr_eq(&project, &s.project), "published project for {command} {args}");
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state, "changed state for {command} {args}");
    assert_eq!(s.revision, revision);
    assert_eq!(s.history.merge_key, merge);
    assert_eq!(s.journal, journal);
    for (before, after) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(before.len(), after.len());
        assert!(before.iter().zip(after).all(|((a, p), (b, q))| a == b && std::sync::Arc::ptr_eq(p, q)));
    }
}

fn retime_demo() -> Session {
    let mut s = demo();
    s.history.merge_key = None;
    s.execute("clip.speedDuration", json!({"speed":50,"reverse":true,"ripple":false,"interpolation":"frameBlending"})).unwrap();
    s.execute("clip.rename", json!({"name":"Keep the human Redo branch"})).unwrap();
    s.undo().unwrap();
    assert!(!s.history.redo.is_empty());
    let clip = s.active_sequence().unwrap().find_item(s.state.selection[0]).unwrap().1;
    assert_eq!(clip.speed, 0.5);
    assert!(clip.reverse);
    s.history.merge_key = Some("existing human retime gesture".into());
    s
}

#[test]
fn explicit_bad_retime_speed_never_becomes_normal_speed() {
    let mut s = retime_demo();
    for value in [json!("not-a-speed"), json!(null), json!(false), json!([]), json!({}), json!(0), json!(-1), json!(f64::from_bits(1))] {
        rejected(&mut s, "clip.speedDuration", json!({"speed":value,"reverse":true,"ripple":false,"interpolation":"frameBlending"}));
    }
}

#[test]
fn explicit_bad_retime_reverse_never_turns_reverse_off() {
    let mut s = retime_demo();
    for value in [json!("not-a-boolean"), json!(null), json!(0), json!(1), json!([]), json!({})] {
        rejected(&mut s, "clip.speedDuration", json!({"speed":50,"reverse":value,"ripple":false,"interpolation":"frameBlending"}));
    }
}

#[test]
fn explicit_bad_retime_ripple_never_publishes_an_edit() {
    let mut s = retime_demo();
    for value in [json!("not-a-boolean"), json!(null), json!(0), json!(1), json!([]), json!({})] {
        rejected(&mut s, "clip.speedDuration", json!({"speed":200,"reverse":false,"ripple":value,"interpolation":"frameSampling"}));
    }
}

#[test]
fn explicit_bad_retime_interpolation_never_preserves_a_default_and_commits() {
    let mut s = retime_demo();
    for value in [json!(123), json!(null), json!(true), json!([]), json!({}), json!("not-a-mode")] {
        rejected(&mut s, "clip.speedDuration", json!({"speed":200,"reverse":false,"ripple":true,"interpolation":value}));
    }
}

#[test]
fn non_object_retime_params_never_execute_selection_defaults() {
    let mut s = retime_demo();
    for value in [json!(null), json!(true), json!(100), json!("not-options"), json!([]), json!([{}])] {
        rejected(&mut s, "clip.speedDuration", value);
    }
}

#[test]
fn omitted_retime_options_keep_human_defaults_and_saved_interpolation() {
    let mut s = retime_demo();
    let mut explicit = retime_demo();
    s.history.merge_key = None;
    explicit.history.merge_key = None;
    let before = serde_json::to_value(&*s.project).unwrap();
    let revision = s.revision;
    let undo = s.history.undo.len();
    let journal = s.journal.len();
    s.execute("clip.speedDuration", json!({})).unwrap();
    explicit.execute("clip.speedDuration", json!({"speed":100,"reverse":false,"ripple":false})).unwrap();
    let clip = s.active_sequence().unwrap().find_item(s.state.selection[0]).unwrap().1;
    assert_eq!(clip.speed, 1.0);
    assert!(!clip.reverse);
    assert_eq!(clip.time_interpolation, filmcraft_project::TimeInterpolation::FrameBlending);
    let edited = serde_json::to_value(&*s.project).unwrap();
    assert_eq!(edited, serde_json::to_value(&*explicit.project).unwrap());
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.journal.len(), journal + 1);
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), edited);
    let file = crate::media_test_util::tmp_dir("retime-options-reopen").join("generated.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    let saved = serde_json::to_value(&*s.project).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), saved);
}

#[test]
fn valid_retime_numeric_types_boolean_values_and_interpolations_remain_supported() {
    for (speed, reverse, ripple, mode, interpolation) in [
        (json!(200), false, false, "frameSampling", filmcraft_project::TimeInterpolation::FrameSampling),
        (json!(125.5), true, true, "frameBlending", filmcraft_project::TimeInterpolation::FrameBlending),
        (json!(50.0), false, true, "opticalFlow", filmcraft_project::TimeInterpolation::OpticalFlow),
        (json!(200), true, false, "FrameBlending", filmcraft_project::TimeInterpolation::FrameBlending),
        (json!(125.5), false, false, "Optical Flow", filmcraft_project::TimeInterpolation::OpticalFlow),
        (json!(200), false, false, "FRAME SAMPLING", filmcraft_project::TimeInterpolation::FrameSampling),
    ] {
        let mut s = demo();
        s.history.merge_key = None;
        let before = serde_json::to_value(&*s.project).unwrap();
        let revision = s.revision;
        let undo = s.history.undo.len();
        s.execute("clip.speedDuration", json!({"speed":speed,"reverse":reverse,"ripple":ripple,"interpolation":mode})).unwrap();
        let clip = s.active_sequence().unwrap().find_item(s.state.selection[0]).unwrap().1;
        assert_eq!(clip.speed, speed.as_f64().unwrap() / 100.0);
        assert_eq!(clip.reverse, reverse);
        assert_eq!(clip.time_interpolation, interpolation);
        let edited = serde_json::to_value(&*s.project).unwrap();
        assert_eq!(s.revision, revision + 1);
        assert_eq!(s.history.undo.len(), undo + 1);
        s.undo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
        s.redo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), edited);
    }
}

#[test]
fn retime_with_no_selection_remains_disabled_without_publication() {
    let mut s = retime_demo();
    s.state.selection.clear();
    rejected(&mut s, "clip.speedDuration", json!({"speed":50,"reverse":false,"ripple":false}));
}

#[test]
fn explicit_bad_clip_or_item_never_becomes_a_selected_target() {
    let mut s = demo();
    let id = s.state.selection[0].0;
    for value in [json!(id as f64 + 0.5), json!(id as f64), json!(-1), json!(null), json!("not-an-id"), json!(true), json!([]), json!({})] {
        rejected(&mut s, "clip.rename", json!({"clip":value,"name":"Wrong clip"}));
        rejected(&mut s, "clip.rename", json!({"item":value,"name":"Wrong item"}));
        rejected(&mut s, "timeline.trim", json!({"clip":value,"edge":"out","mode":"regular","deltaFrames":-1}));
        rejected(&mut s, "project.setMarks", json!({"item":value,"in":0}));
    }
    rejected(&mut s, "clip.rename", json!({"clip":u64::MAX,"name":"Missing clip"}));
    rejected(&mut s, "clip.rename", json!({"item":u64::MAX,"name":"Missing item"}));
}

#[test]
fn explicit_bad_clip_lists_do_not_filter_or_fall_back_to_selection() {
    let mut s = demo();
    let id = s.state.selection[0].0;
    for value in [json!([id, "bad"]), json!([id, id as f64 + 0.5]), json!([id, null]), json!("bad"), json!(null), json!({})] {
        rejected(&mut s, "effects.apply", json!({"clips":value,"effect":"gaussian_blur"}));
        rejected(&mut s, "clip.speedDuration", json!({"clips":value,"speed":50,"reverse":false,"ripple":false}));
    }
}

#[test]
fn explicit_bad_trim_options_do_not_execute_default_operations() {
    let mut s = demo();
    let id = s.state.selection[0].0;
    for value in [json!("sideways"), json!("IN"), json!(null), json!(1), json!(false), json!([]), json!({})] {
        rejected(&mut s, "timeline.trim", json!({"clip":id,"edge":value,"mode":"regular","deltaFrames":-1}));
    }
    for value in [json!("rippple"), json!("ROLL"), json!(null), json!(1), json!(false), json!([]), json!({})] {
        rejected(&mut s, "timeline.trim", json!({"clip":id,"edge":"out","mode":value,"deltaFrames":-1}));
    }
}

#[test]
fn omitted_targets_and_trim_defaults_keep_valid_human_semantics() {
    let mut s = demo();
    s.history.merge_key = None;
    let id = s.state.selection[0];
    let original = s.project.clone();
    s.execute("clip.rename", json!({"name":"Selected by the human"})).unwrap();
    assert_eq!(s.active_sequence().unwrap().find_item(id).unwrap().1.name, "Selected by the human");
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*original).unwrap());
    let mut explicit = demo();
    explicit.history.merge_key = None;
    explicit.execute("timeline.trim", json!({"clip":id.0,"edge":"out","mode":"regular","deltaFrames":-1})).unwrap();
    s.execute("timeline.trim", json!({"clip":id.0,"deltaFrames":-1})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*explicit.project).unwrap());
    let edited = s.project.clone();
    s.undo().unwrap();
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*edited).unwrap());
    let directory = crate::media_test_util::tmp_dir("command-input-reopen");
    let file = directory.join("generated.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    // Save As deliberately names the project/root after the output file.
    // Assert that is the only change, then compare reopen to the actual save.
    let mut expected_saved = serde_json::to_value(&*edited).unwrap();
    expected_saved["name"] = json!("generated");
    expected_saved["root"]["name"] = json!("generated");
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), expected_saved);
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), expected_saved);
}

#[test]
fn explicit_bad_project_item_lists_do_not_create_partial_sequences() {
    let mut s = demo();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    s.state.project_selection = vec![item];
    for value in [json!([item.0, "bad"]), json!([item.0, null]), json!([item.0, item.0 as f64 + 0.5]), json!("bad"), json!(null)] {
        rejected(&mut s, "file.newSequenceFromClip", json!({"items":value}));
    }
}

#[test]
fn intentional_nullable_states_and_integer_boundaries_remain_explicit() {
    let mut s = demo();
    let clip = s.state.selection[0];
    s.execute("project.selectAll", json!({"bin":null})).unwrap();
    s.execute("file.newBin", json!({"name":"Generated root child","parent":null})).unwrap();
    s.execute("effects.toggleAnimation", json!({"clip":clip.0,"effect":"motion","param":"rotation","mask":null})).unwrap();
    for value in [json!(-1), json!(1.0), json!(1.5), json!(null), json!("1"), json!(false), json!([]), json!({})] {
        assert!(crate::commands::u64_p(&json!({"target":value}), "target").is_err());
    }
    assert_eq!(crate::commands::u64_p(&json!({}), "target").unwrap(), None);
    for value in [0, 1, u64::MAX] {
        assert_eq!(crate::commands::u64_p(&json!({"target":value}), "target").unwrap(), Some(value));
    }
    assert_eq!(crate::commands::nullable_u64_p(&json!({"target":null}), "target").unwrap(), None);
    assert!(crate::commands::nullable_u64_p(&json!({"target":"invalid"}), "target").is_err());
}

fn project_demo() -> (Session, filmcraft_project::ItemId) {
    let mut s = demo();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    s.state.project_selection = vec![item];
    (s, item)
}

fn malformed_lists(id: u64) -> Vec<Value> {
    vec![json!("bad"), json!(null), json!(false), json!({}), json!([id, "bad"]), json!([id, null]), json!([id, id as f64 + 0.5]), json!([id, -1])]
}

#[test]
fn malformed_project_and_timeline_selections_preserve_existing_editor_state() {
    let (mut s, item) = project_demo();
    let clip = s.state.selection[0];
    for value in malformed_lists(item.0) {
        rejected(&mut s, "project.select", json!({"items":value}));
    }
    for value in malformed_lists(clip.0) {
        rejected(&mut s, "timeline.select", json!({"clips":value}));
    }
}

#[test]
fn malformed_delete_and_bin_move_targets_never_use_selection_or_apply_a_subset() {
    let (mut s, item) = project_demo();
    let bin = s.execute("file.newBin", json!({"name":"Generated destination"})).unwrap()["bin"].as_u64().unwrap();
    s.history.merge_key = Some("existing human gesture".into());
    for value in malformed_lists(item.0) {
        rejected(&mut s, "project.delete", json!({"items":value}));
        rejected(&mut s, "project.moveToBin", json!({"items":value,"bin":bin}));
    }
    assert!(s.project.item(item).is_some());
}

#[test]
fn malformed_export_and_assembly_targets_never_write_files_or_assemble_a_subset() {
    let (mut s, item) = project_demo();
    let directory = crate::media_test_util::tmp_dir("invalid-project-export-targets");
    for (command, name) in [("file.exportSelectionProject", "subset.fcproj"), ("file.exportAle", "subset.ale"), ("clip.automateToSequence", "unused")] {
        let file = directory.join(name);
        for value in malformed_lists(item.0) {
            rejected(&mut s, command, json!({"items":value,"path":file.to_string_lossy()}));
            assert!(!file.exists(), "rejected {command} wrote a file");
        }
    }
}

#[test]
fn malformed_offline_targets_cannot_remove_even_a_generated_source_file() {
    use filmcraft_project::MediaRef;
    let (mut s, item) = project_demo();
    let file = crate::media_test_util::tmp_dir("invalid-offline-targets").join("generated-original.bin");
    let bytes = b"generated fixture, not user media";
    std::fs::write(&file, bytes).unwrap();
    std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().media =
        MediaRef::File { path: file.to_string_lossy().into_owned() };
    for value in malformed_lists(item.0) {
        rejected(&mut s, "media.makeOffline", json!({"items":value,"deleteFiles":true}));
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
    s.execute("media.makeOffline", json!({"deleteFiles":false})).unwrap();
    assert!(s.project.item(item).unwrap().as_media().unwrap().offline);
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    s.undo().unwrap();
    assert!(!s.project.item(item).unwrap().as_media().unwrap().offline);
}

#[test]
fn malformed_source_marks_preserve_media_and_sequence_marks_and_split_points() {
    use filmcraft_time::Tick;
    let (mut s, item) = project_demo();
    let sequence = s.state.active_sequence.unwrap();
    for id in [item, sequence] {
        s.execute("project.setMarks", json!({"item":id.0,"in":100,"out":200})).unwrap();
        let target = std::sync::Arc::make_mut(&mut s.project).item_mut(id).unwrap();
        target.split.video_in = Some(Tick(110));
        target.split.audio_in = Some(Tick(120));
        target.split.video_out = Some(Tick(180));
        target.split.audio_out = Some(Tick(190));
        if let Some(q) = target.as_sequence_mut() {
            q.split =
                filmcraft_project::SplitMarks { video_in: Some(Tick(111)), audio_in: Some(Tick(121)), video_out: Some(Tick(181)), audio_out: Some(Tick(191)) };
        }
    }
    s.history.merge_key = Some("existing source mark gesture".into());
    for id in [item, sequence] {
        for value in [json!("bad"), json!(100.0), json!(100.5), json!(u64::MAX), json!(true), json!([]), json!({})] {
            rejected(&mut s, "project.setMarks", json!({"item":id.0,"in":value}));
            rejected(&mut s, "project.setMarks", json!({"item":id.0,"in":150,"out":value}));
        }
    }
}

#[test]
fn omitted_targets_and_explicit_empty_selections_retain_human_semantics() {
    for explicit in [false, true] {
        let (mut s, item) = project_demo();
        s.history.merge_key = None;
        let original = serde_json::to_value(&*s.project).unwrap();
        let count = s.history.undo.len();
        s.execute("project.delete", if explicit { json!({"items":[item.0]}) } else { json!({}) }).unwrap();
        assert!(s.project.item(item).is_none());
        assert_eq!(s.history.undo.len(), count + 1);
        let deleted = serde_json::to_value(&*s.project).unwrap();
        s.undo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), original);
        s.redo().unwrap();
        assert_eq!(serde_json::to_value(&*s.project).unwrap(), deleted);
    }
    let (mut s, item) = project_demo();
    let bin = s.execute("file.newBin", json!({"name":"Generated bin"})).unwrap()["bin"].as_u64().unwrap();
    s.state.project_selection = vec![item];
    s.execute("project.moveToBin", json!({"bin":bin})).unwrap();
    assert_eq!(s.project.root.parent_of(item), Some(filmcraft_project::BinId(bin)));
    s.execute("project.moveToBin", json!({"items":[item.0],"bin":null})).unwrap();
    assert_eq!(s.project.root.parent_of(item), Some(s.project.root.id));
    s.execute("project.select", json!({"items":[]})).unwrap();
    assert!(s.state.project_selection.is_empty());
    s.execute("timeline.select", json!({"clips":[]})).unwrap();
    assert!(s.state.selection.is_empty());
}

#[test]
fn explicit_null_marks_clear_only_the_named_side_with_undo_redo_and_saved_reopen() {
    use filmcraft_time::Tick;
    let (mut s, item) = project_demo();
    s.state.source_item = Some(item);
    s.history.merge_key = None;
    // Omitted item retains Source Monitor targeting; integer ticks stay exact.
    s.execute("project.setMarks", json!({"in":100,"out":200})).unwrap();
    let target = std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap();
    target.split =
        filmcraft_project::SplitMarks { video_in: Some(Tick(110)), audio_in: Some(Tick(120)), video_out: Some(Tick(180)), audio_out: Some(Tick(190)) };
    let before = serde_json::to_value(&*s.project).unwrap();
    let revision = s.revision;
    let count = s.history.undo.len();
    s.execute("project.setMarks", json!({"in":null})).unwrap();
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), count + 1);
    let target = s.project.item(item).unwrap();
    assert_eq!(target.as_media().unwrap().mark_in, None);
    assert_eq!(target.as_media().unwrap().mark_out, Some(Tick(200)));
    assert_eq!(target.split.video_in, None);
    assert_eq!(target.split.audio_in, None);
    assert_eq!(target.split.video_out, Some(Tick(180)));
    assert_eq!(target.split.audio_out, Some(Tick(190)));
    let cleared = serde_json::to_value(&*s.project).unwrap();
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), cleared);
    let sequence = s.state.active_sequence.unwrap();
    s.execute("project.setMarks", json!({"item":sequence.0,"in":100,"out":200})).unwrap();
    let target = std::sync::Arc::make_mut(&mut s.project).item_mut(sequence).unwrap();
    target.split =
        filmcraft_project::SplitMarks { video_in: Some(Tick(110)), audio_in: Some(Tick(120)), video_out: Some(Tick(180)), audio_out: Some(Tick(190)) };
    target.as_sequence_mut().unwrap().split = target.split;
    let before_sequence_clear = serde_json::to_value(&*s.project).unwrap();
    s.execute("project.setMarks", json!({"item":sequence.0,"out":null})).unwrap();
    let target = s.project.item(sequence).unwrap();
    assert_eq!(target.split.video_in, Some(Tick(110)));
    assert_eq!(target.split.audio_in, Some(Tick(120)));
    assert_eq!(target.split.video_out, None);
    assert_eq!(target.split.audio_out, None);
    let q = s.project.sequence(sequence).unwrap();
    assert_eq!(q.mark_in, Some(Tick(100)));
    assert_eq!(q.mark_out, None);
    assert_eq!(q.split, target.split);
    let sequence_cleared = serde_json::to_value(&*s.project).unwrap();
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before_sequence_clear);
    s.redo().unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), sequence_cleared);
    let file = crate::media_test_util::tmp_dir("nullable-source-marks-reopen").join("generated.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    let saved = serde_json::to_value(&*s.project).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), saved);
}
