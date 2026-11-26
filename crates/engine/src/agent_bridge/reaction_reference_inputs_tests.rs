use super::tests::{read, token};
use crate::Session;
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn fixture() -> Session {
    let mut s = super::picture_inputs_tests::fixture();
    let seq = s.state.active_sequence.unwrap();
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0];
    let notes = &mut c.identity.as_mut().unwrap().annotations;
    notes.insert("poema_project".into(), "p_test".into());
    notes.insert("poema_timeline".into(), "original_story".into());
    notes.insert("poema_component".into(), "speaker_component".into());
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn request(s: &Session, n: usize, at: i64) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"originalTimeline":"original_story",
        "queries":[{"key":"a".repeat(64),"assetId":"a_source","componentIds":["speaker_component"],"sourceTicks":at}]})
}
fn measure(s: &mut Session, n: usize, at: i64) -> Value {
    read(s, n);
    let r = request(s, n, at);
    let before = state(s);
    let result = s.execute("agent.reactionReferenceInputs", r).unwrap();
    assert_eq!(state(s), before);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["semanticVerification"], false);
    result
}
#[test]
fn original_source_event_uses_current_native_clip_clock_without_moving_history_selection_or_cues() {
    let mut s = fixture();
    let result = measure(&mut s, 89000, 1037);
    assert_eq!(result["rows"][0]["status"], "mapped");
    assert_eq!(result["rows"][0]["matches"][0]["recordTicks"], 137);
    assert_eq!(result["rows"][0]["matches"][0]["observedSourceTicks"], 1037);
    assert_eq!(result["sourceClockPolicy"], "shared-native-clip-clock");
}
#[test]
fn source_threshold_mapping_matches_every_discrete_clock_tick_for_fractional_speed_reverse_and_fractional_starts() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let mut n = 89001;
    for speed in [0.4, 0.8, 1.25, 2.0] {
        for reverse in [false, true] {
            let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0];
            c.start = Tick(101);
            c.speed = speed;
            c.reverse = reverse;
            for at in (990..1220).step_by(11) {
                let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
                let values = (c.start.0..c.end().0).map(|t| (t, c.source_time_at(Tick(t)).0)).collect::<Vec<_>>();
                let bounds = values.first().unwrap().1.min(values.last().unwrap().1)..=values.first().unwrap().1.max(values.last().unwrap().1);
                let expected = bounds.contains(&at).then(|| values.iter().find(|(_, v)| if reverse { *v <= at } else { *v >= at }).unwrap().0);
                let result = measure(&mut s, n, at);
                n += 1;
                match expected {
                    Some(t) => {
                        assert_eq!(result["rows"][0]["status"], "mapped");
                        assert_eq!(result["rows"][0]["matches"][0]["recordTicks"], t);
                    }
                    None => {
                        assert_eq!(result["rows"][0]["status"], "outside_current_source");
                        assert_eq!(result["rows"][0]["matches"], json!([]));
                    }
                }
            }
        }
    }
}
#[test]
fn repeated_current_instances_are_explicitly_ambiguous_and_trimmed_or_held_events_do_not_gain_a_guessed_position() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    let mut second = q.video_tracks[0].items[0].clone();
    second.id = filmcraft_project::ClipId(9999);
    second.identity.as_mut().unwrap().id = "9".repeat(32);
    second.start = Tick(301);
    q.video_tracks[0].items.push(second);
    let result = measure(&mut s, 89200, 1037);
    assert_eq!(result["rows"][0]["status"], "ambiguous");
    assert_eq!(result["rows"][0]["matches"].as_array().unwrap().len(), 2);
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    q.video_tracks[0].items.truncate(1);
    q.video_tracks[0].items[0].source_in = Tick(1050);
    assert_eq!(measure(&mut s, 89201, 1037)["rows"][0]["status"], "outside_current_source");
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].frame_hold = Some(Tick(1037));
    assert_eq!(measure(&mut s, 89202, 1037)["rows"][0]["status"], "unsupported_clock");
}
#[test]
fn malformed_foreign_stale_or_oversized_cue_queries_refuse_without_changing_the_cut_or_history() {
    for case in ["foreign", "time", "duplicate", "empty", "count", "component", "extra", "stale", "duplicate-instance"] {
        let mut s = fixture();
        read(&mut s, 89210);
        let mut r = request(&s, 89210, 1037);
        match case {
            "foreign" => r["scope"]["projectId"] = json!("p_foreign"),
            "time" => r["queries"][0]["sourceTicks"] = json!(-1),
            "duplicate" => r["queries"] = json!([r["queries"][0], r["queries"][0]]),
            "empty" => r["queries"] = json!([]),
            "count" => r["queries"] = json!(vec![r["queries"][0].clone(); 513]),
            "component" => r["queries"][0]["componentIds"] = json!(["bad\0component"]),
            "extra" => r["command"] = json!("markers.add"),
            "stale" => {
                Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0].source_in = Tick(1001);
            }
            "duplicate-instance" => {
                let seq = s.state.active_sequence.unwrap();
                let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
                let copy = q.video_tracks[0].items[0].clone();
                q.video_tracks[0].items.push(copy);
            }
            _ => unreachable!(),
        }
        let before = state(&s);
        if case == "duplicate-instance" {
            let scope = serde_json::from_value(r["scope"].clone()).unwrap();
            let queries = serde_json::from_value::<Vec<super::reaction_reference_inputs::Query>>(r["queries"].clone()).unwrap();
            assert!(super::reaction_reference_inputs::measure(&s, &scope, "original_story", &queries).is_err());
        }
        assert!(s.execute("agent.reactionReferenceInputs", r).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
