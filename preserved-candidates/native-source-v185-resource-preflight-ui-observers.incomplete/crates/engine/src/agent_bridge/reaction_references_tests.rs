use super::tests::{read, token};
use crate::Session;
use serde_json::{Value, json};

fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn operation(s: &mut Session, n: usize) -> Value {
    read(s, n);
    let scope = json!({"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0});
    let queries = json!([
        {"key":"a".repeat(64),"assetId":"a_source","componentIds":["speaker_component"],"sourceTicks":1037},
        {"key":"b".repeat(64),"assetId":"a_source","componentIds":["speaker_component"],"sourceTicks":1049}
    ]);
    let measured =
        s.execute("agent.reactionReferenceInputs", json!({"token":token(n),"scope":scope,"originalTimeline":"original_story","queries":queries})).unwrap();
    let cues = queries.as_array().unwrap().iter().enumerate().map(|(i,q)|{
        let m=&measured["rows"][i]["matches"][0];
        json!({"query":q,"clip":m["clip"],"identity":m["identity"],"recordTicks":m["recordTicks"],"name":format!("Original cue {i}"),"color":"Teal",
            "comment":json!({"kind":"poema-original-reaction-marker","version":1,"project_id":"p_test","timeline_id":"original_story","marker_id":format!("cue_{i}"),"marker_sha256":q["key"]}).to_string()})
    }).collect::<Vec<_>>();
    json!({"command":"agent.importReactionReferences","params":{"scope":scope,"originalTimeline":"original_story","beforeProjectSha256":measured["projectSha256"],
        "beforeMarkerSha256":super::markers::digest(&s.active_sequence().unwrap().markers).unwrap(),"planSha256":"c".repeat(64),"inputsSha256":"d".repeat(64),"cues":cues}})
}
fn apply(s: &mut Session, n: usize, op: Value) -> crate::Result<Value> {
    s.execute("agent.apply", json!({"token":token(n),"exact":true,"label":"Carry original cues","operations":[op]}))
}
#[test]
fn reference_handoff_uses_shared_human_markers_once_preserves_all_other_state_and_has_one_undo_step() {
    let mut s = super::reaction_reference_inputs_tests::fixture();
    s.execute("markers.add", json!({"time":7,"name":"Existing human cue","comment":"Preserve every byte"})).unwrap();
    let op = operation(&mut s, 89300);
    let before = s.project.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let receipt = apply(&mut s, 89300, op.clone()).unwrap();
    let atomic = s.project.clone();
    assert_eq!(receipt["action"], "reaction_reference_handoff");
    assert_eq!(receipt["results"][0]["added"].as_array().unwrap().len(), 2);
    assert_eq!(receipt["results"][0]["humanCommands"]["markers.add"], 2);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.active_sequence().unwrap().markers.iter().find(|m| m.name == "Existing human cue").unwrap().comment, "Preserve every byte");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&before).unwrap());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&atomic).unwrap());
    s.execute("edit.undo", json!({})).unwrap();
    for cue in op["params"]["cues"].as_array().unwrap() {
        s.execute(
            "markers.add",
            json!({"target":"program","time":cue["recordTicks"],"duration":0,"name":cue["name"],"comment":cue["comment"],"color":cue["color"]}),
        )
        .unwrap();
    }
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&atomic).unwrap());
}
#[test]
fn stale_forged_duplicate_or_ambiguous_references_cannot_publish_partial_cues_or_move_existing_ones() {
    for case in ["position", "identity", "source", "project", "scope", "duplicate", "existing", "count", "late", "ambiguous"] {
        let mut s = super::reaction_reference_inputs_tests::fixture();
        if case == "existing" {
            s.execute("markers.add",json!({"time":80,"name":"Human moved this original cue","comment":json!({"kind":"poema-original-reaction-marker","version":1,"project_id":"p_test","timeline_id":"original_story","marker_id":"cue_0","marker_sha256":"a".repeat(64)}).to_string()})).unwrap();
        }
        if case == "ambiguous" {
            let seq = s.state.active_sequence.unwrap();
            let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            let mut second = q.video_tracks[0].items[0].clone();
            second.id = filmcraft_project::ClipId(9999);
            second.identity.as_mut().unwrap().id = "9".repeat(32);
            second.start = filmcraft_time::Tick(401);
            q.video_tracks[0].items.push(second);
        }
        let mut op = operation(&mut s, 89301);
        match case {
            "position" => op["params"]["cues"][0]["recordTicks"] = json!(138),
            "identity" => op["params"]["cues"][0]["identity"] = json!("9".repeat(32)),
            "source" => op["params"]["cues"][0]["query"]["sourceTicks"] = json!(1040),
            "project" => op["params"]["beforeProjectSha256"] = json!("0".repeat(64)),
            "scope" => op["params"]["scope"]["projectId"] = json!("p_foreign"),
            "duplicate" => op["params"]["cues"][1] = op["params"]["cues"][0].clone(),
            "count" => op["params"]["cues"] = json!(vec![op["params"]["cues"][0].clone(); 513]),
            "late" => op["params"]["cues"][1]["recordTicks"] = json!(150),
            _ => {}
        }
        let before = state(&s);
        assert!(apply(&mut s, 89301, op).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
#[test]
fn stopped_reference_staging_discards_a_candidate_without_changing_its_live_owner() {
    let mut s = super::reaction_reference_inputs_tests::fixture();
    let op = operation(&mut s, 89302);
    let before = state(&s);
    let mut stage = Session::new(s.services.clone());
    stage.project = s.project.clone();
    stage.state = s.state.clone();
    let counter = std::cell::Cell::new(0);
    let check = || {
        let n = counter.get() + 1;
        counter.set(n);
        if n >= 3 { Err(crate::commands::bad("test", "Stopped after one private gesture")) } else { Ok(()) }
    };
    let op = serde_json::from_value(op).unwrap();
    assert!(super::reaction_references::run(&mut stage, &op, &check).is_err());
    assert!(counter.get() >= 3);
    assert_eq!(state(&s), before);
}
