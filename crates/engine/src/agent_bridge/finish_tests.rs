use super::tests::{read, token};
use crate::Session;
use serde_json::{Value, json};
use std::sync::Arc;
fn setup() -> Session {
    let mut s = super::tests::setup();
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata.insert(
            "poema.binding.v1".into(),
            json!({"projectId":"p_finish","assetId":format!("a_{}",item.id.0),"mediaId":"original","role":"original"}).to_string(),
        );
    }
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    for t in q.all_tracks_mut() {
        t.items.truncate(2);
        t.transitions.clear();
        t.locked = false;
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s
}
fn point(c: &filmcraft_project::TrackItem) -> Value {
    json!({"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id})
}
fn gain(s: &Session, db: f64) -> Value {
    let c = &s.active_sequence().unwrap().audio_tracks[0].items[0];
    json!({"type":"gain","clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"targets":[point(c)],"mode":"set","db":db})
}
fn transition(s: &Session, effect: &str, cross: bool) -> Value {
    let q = s.active_sequence().unwrap();
    let track = if effect == "cross_dissolve" { &q.video_tracks[0] } else { &q.audio_tracks[0] };
    let c = &track.items[if cross { 0 } else { 1 }];
    json!({"type":"transition","clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,
        "neighbor":if cross{point(&track.items[1])}else{Value::Null},"edge":"out","effect":effect,"frames":12})
}
fn batch(s: &Session, n: usize, edit: Value) -> Value {
    json!({"token":token(n),"label":"Finish through the human control","exact":true,"operations":[{"command":"agent.finish","params":{
        "scope":{"projectId":"p_finish","anchorSequence":s.state.active_sequence.unwrap().0},"edit":edit}}]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn human(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.media = s.media.clone();
    h
}
fn pcm(s: &Session, start: i64) -> Vec<f32> {
    let p = s.media.full_res_provider(s.project.clone(), s.services.clone());
    filmcraft_render::audio::mix_sequence(&s.project, s.active_sequence().unwrap(), start, 2400, &p).interleaved()
}

#[test]
fn finishing_gain_uses_human_command_and_preserves_exact_undo_redo_reopen_and_pcm() {
    let mut s = setup();
    let edit = gain(&s, -7.0);
    let mut h = human(&s);
    let original = serde_json::to_value(&*s.project).unwrap();
    let before_pcm = pcm(&s, 12000);
    h.execute("clip.audioGain", json!({"clips":[edit["clip"]],"mode":"set","db":-7})).unwrap();
    read(&mut s, 27000);
    let req = batch(&s, 27000, edit);
    let r = s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(r["action"], "finishing_edit");
    assert_eq!(r["results"][0]["command"], "clip.audioGain");
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*h.project).unwrap());
    assert_eq!(pcm(&s, 12000), pcm(&h, 12000));
    assert_ne!(pcm(&s, 12000), before_pcm);
    assert_eq!(s.history.undo.len(), 1);
    assert_eq!(s.execute("agent.apply", req).unwrap(), r);
    let after = serde_json::to_value(&*s.project).unwrap();
    read(&mut s, 27001);
    s.execute("agent.undo", json!({"token":token(27001),"commit":token(27000)})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), original);
    assert_eq!(pcm(&s, 12000), before_pcm);
    read(&mut s, 27002);
    s.execute("agent.redo", json!({"token":token(27002),"commit":token(27000),"undo":token(27001)})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), after);
    assert_eq!(serde_json::to_value(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project).unwrap(), after);
}
#[test]
fn finishing_transitions_match_each_human_effect_and_rendered_picture_or_audio() {
    for (index, effect) in ["cross_dissolve", "constant_power", "constant_gain", "exponential_fade"].iter().enumerate() {
        for cross in [false, true] {
            let mut s = setup();
            let edit = transition(&s, effect, cross);
            let mut h = human(&s);
            let command = if *effect == "cross_dissolve" { "sequence.applyVideoTransition" } else { "sequence.applyAudioTransition" };
            let params = json!({"clip":edit["clip"],"edge":"out","effect":effect,"frames":12});
            h.execute(command, params).unwrap();
            let n = 27100 + index * 2 + usize::from(cross);
            read(&mut s, n);
            let req = batch(&s, n, edit);
            let r = s.execute("agent.apply", req).unwrap();
            assert_eq!(r["results"][0]["command"], command);
            assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*h.project).unwrap(), "{effect} cross={cross}");
            let track = if *effect == "cross_dissolve" { &s.active_sequence().unwrap().video_tracks[0] } else { &s.active_sequence().unwrap().audio_tracks[0] };
            let tr = &track.transitions[0];
            let at = tr.start + tr.duration.mul_ratio(1, 2);
            if *effect == "cross_dissolve" {
                assert_eq!(s.render_program_at(0.05, at).unwrap().px, h.render_program_at(0.05, at).unwrap().px);
            } else {
                assert_eq!(pcm(&s, at.to_units_floor(48000)), pcm(&h, at.to_units_floor(48000)));
            }
        }
    }
}
#[test]
fn finishing_refuses_incomplete_linked_audio_targets_instead_of_silently_editing_a_partner() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let link = q.audio_tracks[0].items[0].link;
    q.audio_tracks[0].items[1].link = link;
    read(&mut s, 27200);
    let edit = gain(&s, -7.0);
    let before = state(&s);
    assert!(s.execute("agent.apply", batch(&s, 27200, edit.clone())).is_err());
    assert_eq!(state(&s), before);
    let mut complete = edit;
    complete["targets"].as_array_mut().unwrap().push(point(&s.active_sequence().unwrap().audio_tracks[0].items[1]));
    s.execute("agent.apply", batch(&s, 27200, complete)).unwrap();
    assert!(s.active_sequence().unwrap().audio_tracks[0].items.iter().all(|c| c.gain_db == -7.0));
}
#[test]
fn finishing_rejects_existing_transition_replacement_and_leaves_human_history_untouched() {
    let mut s = setup();
    let edit = transition(&s, "cross_dissolve", true);
    s.execute("sequence.applyVideoTransition", json!({"clip":edit["clip"],"edge":"out","effect":"cross_dissolve","frames":20})).unwrap();
    s.execute("clip.audioGain", json!({"clips":[s.active_sequence().unwrap().audio_tracks[0].items[0].id.0],"mode":"set","db":-3})).unwrap();
    s.undo().unwrap();
    read(&mut s, 27300);
    let before = state(&s);
    let redo = s.history.redo.clone();
    assert!(s.execute("agent.apply", batch(&s, 27300, edit)).is_err());
    assert_eq!(state(&s), before);
    assert!(redo.iter().zip(&s.history.redo).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
}
#[test]
fn finishing_hostile_shapes_scope_locks_and_clamping_never_publish() {
    let mut s = setup();
    read(&mut s, 27400);
    let before = state(&s);
    let valid = batch(&s, 27400, gain(&s, -7.0));
    for (field, value) in [
        ("clip", json!(0)),
        ("clip", json!(u64::MAX)),
        ("identity", json!("bad")),
        ("db", json!(1000)),
        ("db", json!("-7")),
        ("mode", json!("normalizeMax")),
        ("targets", json!([])),
        ("extra", json!(true)),
    ] {
        let mut req = valid.clone();
        req["operations"][0]["params"]["edit"][field] = value;
        assert!(s.execute("agent.apply", req).is_err(), "{field}");
        assert_eq!(state(&s), before);
    }
    for (field, value) in [("projectId", json!("foreign")), ("anchorSequence", json!(u64::MAX))] {
        let mut req = valid.clone();
        req["operations"][0]["params"]["scope"][field] = value;
        assert!(s.execute("agent.apply", req).is_err());
        assert_eq!(state(&s), before);
    }
    let mut req = valid.clone();
    req["exact"] = json!(false);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(&s), before);
    let mut req = valid.clone();
    let duplicate = req["operations"][0].clone();
    req["operations"].as_array_mut().unwrap().push(duplicate);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(&s), before);
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().audio_tracks[0].locked = true;
    read(&mut s, 27401);
    let before = state(&s);
    assert!(s.execute("agent.apply", batch(&s, 27401, gain(&s, -7.0))).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn finishing_refuses_implicit_neighbors_missing_handles_and_noop_gain() {
    let mut s = setup();
    read(&mut s, 27500);
    let mut edit = transition(&s, "cross_dissolve", true);
    edit["neighbor"] = Value::Null;
    let before = state(&s);
    assert!(s.execute("agent.apply", batch(&s, 27500, edit)).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", batch(&s, 27500, gain(&s, 0.0))).is_err());
    assert_eq!(state(&s), before);
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[1].source_in = filmcraft_time::Tick::ZERO;
    read(&mut s, 27501);
    let before = state(&s);
    assert!(s.execute("agent.apply", batch(&s, 27501, transition(&s, "cross_dissolve", true))).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn authored_transition_and_gain_reads_are_complete_ticket_scoped_and_do_not_migrate() {
    let mut s = setup();
    let edit = transition(&s, "constant_power", true);
    s.execute("sequence.applyAudioTransition", json!({"clip":edit["clip"],"edge":"out","effect":"constant_power","frames":12})).unwrap();
    read(&mut s, 27600);
    let before = state(&s);
    let sid = s.state.active_sequence.unwrap();
    let r = s
        .execute(
            "agent.readFinishing",
            json!({"token":token(27600),"scope":{"projectId":"p_finish","anchorSequence":sid.0},"identity":edit["identity"],"offset":0,"limit":4}),
        )
        .unwrap();
    assert_eq!(r["clipGainDb"], 0.0);
    assert_eq!(r["transitions"].as_array().unwrap().len(), 1);
    assert_eq!(r["transitions"][0]["effect"]["effect"], "constant_power");
    assert_eq!(state(&s), before);
}

#[test]
fn finishing_audio_review_rounds_inward_at_sample_boundaries_and_refuses_empty_or_unsafe_ranges() {
    use filmcraft_time::{TICKS_PER_SECOND, Tick};
    let mut s = setup();
    let rows = super::finish::audio_review(&s, Tick(1), Tick(TICKS_PER_SECOND), true).unwrap();
    assert_eq!(rows[0]["startSample"], 1);
    assert_eq!(rows[0]["samples"], 4800);
    assert_eq!(rows[1]["startSample"], 43200);
    assert!(super::finish::audio_review(&s, Tick(1), Tick(2), false).is_err());
    assert!(super::finish::audio_review(&s, Tick(-1), Tick(TICKS_PER_SECOND), false).is_err());
    assert!(super::finish::audio_review(&s, Tick(1), Tick(i64::MAX), false).is_err());
    let sid = s.state.active_sequence.unwrap();
    for rate in [0, 384001] {
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().settings.sample_rate = rate;
        assert!(super::finish::audio_review(&s, Tick::ZERO, Tick(TICKS_PER_SECOND), false).is_err());
    }
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().settings.sample_rate = 44100;
    let rows = super::finish::audio_review(&s, Tick(1), Tick(TICKS_PER_SECOND - 1), true).unwrap();
    assert_eq!(rows[0]["startSample"], 1);
    assert_eq!(rows[1]["startSample"], 39689);
}

#[test]
fn finishing_read_refuses_dangling_transition_without_repairing_the_human_project_or_redo() {
    let mut s = setup();
    let edit = transition(&s, "cross_dissolve", true);
    s.execute("sequence.applyVideoTransition", json!({"clip":edit["clip"],"edge":"out","effect":"cross_dissolve","frames":12})).unwrap();
    s.execute("clip.audioGain", json!({"clips":[s.active_sequence().unwrap().audio_tracks[0].items[0].id.0],"mode":"set","db":-3})).unwrap();
    s.undo().unwrap();
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items.truncate(1);
    read(&mut s, 27700);
    let before = state(&s);
    let project = s.project.clone();
    let error = s
        .execute(
            "agent.readFinishing",
            json!({"token":token(27700),"scope":{"projectId":"p_finish","anchorSequence":sid.0},"identity":edit["identity"],"offset":0,"limit":4}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("dangling transition"));
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&s.project, &project));
}
