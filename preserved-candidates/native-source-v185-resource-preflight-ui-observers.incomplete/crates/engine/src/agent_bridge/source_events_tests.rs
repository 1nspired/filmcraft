use super::tests::{read, token};
use crate::Session;
use filmcraft_project::{ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

fn fixture() -> Session {
    super::picture_inputs_tests::fixture()
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn request(s: &Session, n: usize, at: i64) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},
        "tracks":[s.active_sequence().unwrap().video_tracks[0].id.0],"queries":[{"key":"a".repeat(64),"assetId":"a_source","sourceTicks":at}]})
}
fn measure(s: &mut Session, n: usize, at: i64) -> Value {
    read(s, n);
    let r = request(s, n, at);
    let before = state(s);
    let result = s.execute("agent.sourceEventInputs", r).unwrap();
    assert_eq!(state(s), before);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["semanticVerification"], false);
    result
}
#[test]
fn source_event_on_unannotated_current_cut_uses_native_clock_without_creating_component_or_history() {
    let mut s = fixture();
    let row = measure(&mut s, 91000, 1037);
    assert_eq!(row["rows"][0]["status"], "mapped");
    let m = &row["rows"][0]["instances"][0];
    assert_eq!(m["recordTicks"], 137);
    assert_eq!(m["observedSourceTicks"], 1037);
    assert!(m.get("componentId").is_none());
    assert_eq!(row["sourceClockPolicy"], "shared-native-clip-clock");
    assert_eq!(row["requiresSourceValidation"], false);
}
#[test]
fn source_event_threshold_equals_every_discrete_native_clock_for_fractional_reversed_and_subframe_starts() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let mut n = 91001;
    for speed in [0.4, 0.8, 1.25, 2.0] {
        for reverse in [false, true] {
            let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0];
            c.start = Tick(101);
            c.speed = speed;
            c.reverse = reverse;
            for at in (990..1220).step_by(7) {
                let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
                let samples = (c.start.0..c.end().0).map(|t| (t, c.source_time_at(Tick(t)).0)).collect::<Vec<_>>();
                let bounds = samples.first().unwrap().1.min(samples.last().unwrap().1)..=samples.first().unwrap().1.max(samples.last().unwrap().1);
                let expected = bounds.contains(&at).then(|| samples.iter().find(|(_, v)| if reverse { *v <= at } else { *v >= at }).unwrap().0);
                let actual = measure(&mut s, n, at);
                n += 1;
                assert_eq!(actual["rows"][0]["instances"].as_array().unwrap().first().and_then(|m| m["recordTicks"].as_i64()), expected);
                assert_eq!(actual["rows"][0]["status"], if expected.is_some() { "mapped" } else { "outside_current_source" });
            }
        }
    }
}
#[test]
fn source_events_keep_all_occurrences_track_order_and_disabled_flags_without_claiming_visibility() {
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    let mut second = q.video_tracks[0].clone();
    second.id = TrackId(9900);
    second.enabled = false;
    second.items[0].id = ClipId(9901);
    second.items[0].identity.as_mut().unwrap().id = "f".repeat(32);
    second.items[0].start = Tick(501);
    second.items[0].enabled = false;
    q.video_tracks.push(second);
    read(&mut s, 91400);
    let mut r = request(&s, 91400, 1037);
    r["tracks"] = json!([9900, s.active_sequence().unwrap().video_tracks[0].id.0]);
    let before = state(&s);
    let result = s.execute("agent.sourceEventInputs", r).unwrap();
    assert_eq!(state(&s), before);
    let rows = result["rows"][0]["instances"].as_array().unwrap();
    assert_eq!(result["rows"][0]["status"], "ambiguous");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["recordTicks"], 538);
    assert_eq!(rows[0]["trackEnabled"], false);
    assert_eq!(rows[0]["enabled"], false);
    assert_eq!(rows[1]["recordTicks"], 137);
    assert_eq!(measure(&mut s, 91401, 1037)["rows"][0]["instances"].as_array().unwrap().len(), 1);
}
#[test]
fn source_event_absence_trimmed_edges_and_holds_are_explicit_without_guessed_placement() {
    let mut s = fixture();
    assert_eq!(measure(&mut s, 91410, 1100)["rows"][0]["status"], "outside_current_source");
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0].frame_hold = Some(Tick(1037));
    assert_eq!(measure(&mut s, 91411, 1037)["rows"][0]["status"], "unsupported_clock");
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items.clear();
    assert_eq!(measure(&mut s, 91412, 1037)["rows"][0]["status"], "source_absent");
}
#[test]
fn malformed_foreign_stale_and_oversized_source_event_requests_refuse_atomically() {
    for case in [
        "foreign",
        "time",
        "unsafe",
        "duplicate",
        "empty",
        "count",
        "tracks",
        "track-count",
        "track-negative",
        "missing-track",
        "audio-track",
        "key",
        "asset",
        "extra",
        "stale",
        "duplicate-instance",
        "untracked",
    ] {
        let mut s = fixture();
        read(&mut s, 91420);
        let mut r = request(&s, 91420, 1037);
        match case {
            "foreign" => r["scope"]["projectId"] = json!("p_foreign"),
            "time" => r["queries"][0]["sourceTicks"] = json!(-1),
            "unsafe" => r["queries"][0]["sourceTicks"] = json!(9_007_199_254_740_992i64),
            "duplicate" => r["queries"] = json!([r["queries"][0], r["queries"][0]]),
            "empty" => r["queries"] = json!([]),
            "count" => r["queries"] = json!(vec![r["queries"][0].clone(); 513]),
            "tracks" => r["tracks"] = json!([r["tracks"][0], r["tracks"][0]]),
            "track-count" => r["tracks"] = json!((1..=129).collect::<Vec<_>>()),
            "track-negative" => r["tracks"] = json!([-1]),
            "missing-track" => r["tracks"] = json!([99999]),
            "audio-track" => r["tracks"] = json!([s.active_sequence().unwrap().audio_tracks[0].id.0]),
            "key" => r["queries"][0]["key"] = json!("A".repeat(64)),
            "asset" => r["queries"][0]["assetId"] = json!("bad\0asset"),
            "extra" => r["queries"][0]["componentIds"] = json!(["invented_story_component"]),
            "stale" => Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0].source_in = Tick(1001),
            "duplicate-instance" => {
                let q = Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap();
                let copy = q.video_tracks[0].items[0].clone();
                q.video_tracks[0].items.push(copy);
            }
            "untracked" => Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0].identity = None,
            _ => unreachable!(),
        }
        let before = state(&s);
        assert!(s.execute("agent.sourceEventInputs", r).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
#[test]
fn source_event_projection_retains_validation_debt_and_changed_requested_sources_expire_a_read() {
    let mut s = fixture();
    read(&mut s, 91430);
    let mut r = request(&s, 91430, 1037);
    r["projection"] = json!(true);
    let result = s.execute("agent.sourceEventInputs", r).unwrap();
    assert_eq!(result["requiresSourceValidation"], true);
    read(&mut s, 91431);
    let r = request(&s, 91431, 1037);
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.duration = Tick(900);
    let before = state(&s);
    assert!(s.execute("agent.sourceEventInputs", r).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn source_event_projection_refuses_a_ticket_that_does_not_cover_the_requested_original_file() {
    let mut s = fixture();
    read(&mut s, 91432);
    let mut r = request(&s, 91432, 1037);
    r["projection"] = json!(true);
    let empty = super::media_guard::capture_scoped(&s.project, &s.media, vec![]).unwrap();
    s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(91432)).unwrap().media = empty;
    let before = state(&s);
    assert!(s.execute("agent.sourceEventInputs", r).is_err());
    assert_eq!(state(&s), before);
}
