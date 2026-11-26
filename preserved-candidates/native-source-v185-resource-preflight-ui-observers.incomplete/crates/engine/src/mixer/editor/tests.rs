use super::*;
use crate::{EditorState, History};
use filmcraft_project::Project;
use serde_json::json;
use std::sync::Arc;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.history.undo.clear();
    s.history.redo.clear();
    s.history.merge_key = None;
    s.journal.clear();
    s
}
fn preserve(s: &mut Session) {
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("existing human fader drag".into());
    assert!(!s.history.redo.is_empty());
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,
        "dirty":s.is_dirty(),"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
fn same_history(a: &History, b: &History) {
    assert_eq!(a.merge_key, b.merge_key);
    assert_eq!(a.limit, b.limit);
    for (x, y) in [(&a.undo, &b.undo), (&a.redo, &b.redo)] {
        assert_eq!(x.len(), y.len());
        assert!(x.iter().zip(y).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
    }
}
fn human_copy(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.history = s.history.clone();
    h.revision = s.revision;
    h.saved_revision = s.saved_revision;
    h.journal = s.journal.clone();
    h
}
fn paired(s: &mut Session, change: Change, command: &str, args: Value, journaled: bool) -> Value {
    let before = s.project.clone();
    let history = s.history.clone();
    let revision = s.revision;
    let saved = s.saved_revision;
    let journal = s.journal.clone();
    let mut h = human_copy(s);
    let actual = change.apply(s).unwrap();
    let expected = h.execute(command, args.clone()).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&h.state).unwrap());
    assert!(Arc::ptr_eq(&before, &s.project));
    assert!(Arc::ptr_eq(&before, &h.project));
    assert_eq!((s.revision, s.saved_revision, s.is_dirty()), (revision, saved, revision != saved));
    assert_eq!((h.revision, h.saved_revision, h.is_dirty()), (revision, saved, revision != saved));
    same_history(&s.history, &history);
    same_history(&h.history, &history);
    assert_eq!(s.journal, journal);
    let mut expected_journal = journal;
    if journaled {
        expected_journal.push((command.to_string(), args));
    }
    assert_eq!(h.journal, expected_journal);
    actual
}
#[test]
fn shared_editor_mode_preparation_is_readonly_and_all_five_modes_match_original_human_controls() {
    for mode in AutomationMode::ALL {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].id;
        let before = state(&s);
        let prepared = prepare_mode(&s, id, mode).unwrap();
        assert_eq!(state(&s), before);
        paired(&mut s, prepared, "clipMixer.setMode", json!({"track":id.0,"mode":mode.label()}), false);
        assert_eq!(s.state.clip_mixer_modes.get(&id.0), Some(&mode));
    }
}
#[test]
fn shared_editor_modes_preserve_stored_defaults_other_tracks_unrelated_state_and_locked_track_semantics() {
    let mut s = demo();
    preserve(&mut s);
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let id = q.audio_tracks[0].id;
    q.audio_tracks[0].locked = true;
    s.state.clip_mixer_modes.insert(900000, AutomationMode::Write);
    s.state.snapping = false;
    s.state.default_audio_transition = "constant_gain".into();
    let mode = crate::mixer::clip_mode(&s, id);
    assert!(!s.state.clip_mixer_modes.contains_key(&id.0));
    let change = prepare_mode(&s, id, mode).unwrap();
    paired(&mut s, change, "clipMixer.setMode", json!({"track":"A1","mode":mode.label()}), false);
    assert_eq!(s.state.clip_mixer_modes.get(&id.0), Some(&mode));
    assert_eq!(s.state.clip_mixer_modes.get(&900000), Some(&AutomationMode::Write));
    assert!(!s.state.snapping);
    assert_eq!(s.state.default_audio_transition, "constant_gain");
}
#[test]
fn shared_editor_transition_defaults_match_every_actual_native_audio_and_video_definition_with_original_journaling() {
    let mut counts = [0, 0];
    for def in filmcraft_project::effect_defs() {
        let index = match def.kind {
            EffectKind::AudioTransition => 0,
            EffectKind::VideoTransition => 1,
            _ => continue,
        };
        counts[index] += 1;
        let mut s = demo();
        preserve(&mut s);
        let before = state(&s);
        let prepared = prepare_default_transition(def.id).unwrap();
        assert_eq!(state(&s), before);
        paired(&mut s, prepared, "effects.setDefaultTransition", json!({"effect":def.id}), true);
        let got = if index == 0 { &s.state.default_audio_transition } else { &s.state.default_video_transition };
        assert_eq!(got, def.id);
    }
    assert!(counts[0] > 0 && counts[1] > 0);
    println!("qualified actual original transition defaults: audio={} video={}", counts[0], counts[1]);
}
#[test]
fn shared_editor_transition_names_aliases_and_empty_session_keep_original_global_state_behavior() {
    for (name, id) in [("Cross Dissolve", "cross_dissolve"), ("Exponential Fade", "exponential_fade")] {
        let mut s = Session::default();
        assert!(s.state.active_sequence.is_none());
        let prepared = prepare_default_transition(name).unwrap();
        paired(&mut s, prepared, "effects.setDefaultTransition", json!({"effect":name}), true);
        assert!(s.state.default_audio_transition == id || s.state.default_video_transition == id);
    }
    let mut s = demo();
    preserve(&mut s);
    let id = s.state.default_video_transition.clone();
    let prepared = prepare_default_transition(&id).unwrap();
    paired(&mut s, prepared, "effects.setDefaultTransition", json!({"effect":id}), true);
}
#[test]
fn shared_editor_bad_modes_tracks_and_transition_inputs_preserve_complete_human_redo_and_merge_ownership() {
    for (command, args) in [
        ("clipMixer.setMode", json!({"track":"V1","mode":"Read"})),
        ("clipMixer.setMode", json!({"track":"Mix","mode":"Read"})),
        ("clipMixer.setMode", json!({"track":999999,"mode":"Read"})),
        ("clipMixer.setMode", json!({"track":"A1","mode":"unknown"})),
        ("clipMixer.setMode", json!({"track":"A1","mode":false})),
        ("effects.setDefaultTransition", json!({"effect":"volume"})),
        ("effects.setDefaultTransition", json!({"effect":"not_an_effect"})),
        ("effects.setDefaultTransition", json!({"effect":null})),
    ] {
        let mut s = demo();
        preserve(&mut s);
        let before = state(&s);
        let project = s.project.clone();
        let history = s.history.clone();
        assert!(s.execute(command, args).is_err());
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
        same_history(&s.history, &history);
    }
    let s = demo();
    for track in [s.active_sequence().unwrap().video_tracks[0].id, TrackId(999999)] {
        assert!(prepare_mode(&s, track, AutomationMode::Read).is_err());
    }
    for name in ["volume", "gaussian_blur", "not_an_effect", ""] {
        assert!(prepare_default_transition(name).is_err());
    }
}
#[test]
fn shared_editor_prepared_modes_revalidate_current_sequence_and_target_without_replacing_other_current_state() {
    let mut s = demo();
    preserve(&mut s);
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    let change = prepare_mode(&s, id, AutomationMode::Touch).unwrap();
    s.state.active_sequence = None;
    let before = state(&s);
    assert!(change.apply(&mut s).is_err());
    assert_eq!(state(&s), before);
    let mut s = demo();
    preserve(&mut s);
    let sid = s.state.active_sequence.unwrap();
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    let change = prepare_mode(&s, id, AutomationMode::Latch).unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks.clear();
    let before = state(&s);
    assert!(change.apply(&mut s).is_err());
    assert_eq!(state(&s), before);
    let mut s = demo();
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    let change = prepare_mode(&s, id, AutomationMode::Write).unwrap();
    s.state.snapping = false;
    s.state.default_video_transition = "dip_to_black".into();
    change.apply(&mut s).unwrap();
    assert!(!s.state.snapping);
    assert_eq!(s.state.default_video_transition, "dip_to_black");
}
#[test]
fn shared_editor_controls_preserve_actual_live_preview_and_recording_owners() {
    let mut s = demo();
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("clipMixer.touch", json!({"track":id.0,"lane":"volume","value":-9,"time":0})).unwrap();
    assert!(s.previews.live.is_active());
    let project = s.project.clone();
    let revision = s.revision;
    let history = s.history.clone();
    s.execute("clipMixer.setMode", json!({"track":id.0,"mode":"Touch"})).unwrap();
    assert!(s.previews.live.is_active());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    same_history(&s.history, &history);
    s.previews.live.clear_all();
    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
    assert!(s.mixrec.active());
    let project = s.project.clone();
    let history = s.history.clone();
    let revision = s.revision;
    let mode = prepare_mode(&s, id, AutomationMode::Latch).unwrap();
    mode.apply(&mut s).unwrap();
    s.execute("effects.setDefaultTransition", json!({"effect":"constant_gain"})).unwrap();
    assert!(s.mixrec.active());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.revision, revision);
    same_history(&s.history, &history);
}
#[test]
fn shared_editor_settings_keep_existing_serialized_editor_and_project_formats_and_later_human_project_history() {
    let mut s = demo();
    preserve(&mut s);
    let project = s.project.clone();
    let before_project = serde_json::to_value(&s.project).unwrap();
    let history = s.history.clone();
    let id = s.active_sequence().unwrap().audio_tracks[0].id;
    prepare_mode(&s, id, AutomationMode::Write).unwrap().apply(&mut s).unwrap();
    prepare_default_transition("exponential_fade").unwrap().apply(&mut s).unwrap();
    let encoded = serde_json::to_vec(&s.state).unwrap();
    let decoded: EditorState = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), serde_json::to_value(&s.state).unwrap());
    let decoded: Project = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project;
    assert_eq!(serde_json::to_value(decoded).unwrap(), before_project);
    assert!(Arc::ptr_eq(&project, &s.project));
    same_history(&s.history, &history);
    s.redo().unwrap();
    assert_eq!(s.state.clip_mixer_modes.get(&id.0), Some(&AutomationMode::Write));
    assert_eq!(s.state.default_audio_transition, "exponential_fade");
    s.undo().unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), before_project);
    assert_eq!(s.state.clip_mixer_modes.get(&id.0), Some(&AutomationMode::Write));
}
