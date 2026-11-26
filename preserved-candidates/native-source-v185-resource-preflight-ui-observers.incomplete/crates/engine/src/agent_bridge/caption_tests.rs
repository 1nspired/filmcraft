use super::tests::{read, token};
use crate::Session;
use serde_json::{Value, json};
use std::sync::Arc;

fn setup() -> Session {
    let mut s = super::tests::setup();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks.clear();
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s
}
fn human(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.media = s.media.clone();
    h
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn request(s: &Session, n: usize, edit: Value) -> Value {
    json!({"token":token(n),"label":"Caption through shared human control","exact":true,"operations":[{"command":"agent.caption","params":{
      "scope":{"projectId":"p_caption","anchorSequence":s.state.active_sequence.unwrap().0},"beforeSha256":super::captions::summary(s)["sha256"],"edit":edit}}]})
}
fn step(s: &mut Session, n: usize, edit: Value, command: &str, params: Value) -> Value {
    let mut h = human(s);
    h.execute(command, params).unwrap();
    let before = serde_json::to_value(&*s.project).unwrap();
    let undo = s.history.undo.len();
    read(s, n);
    let batch = request(s, n, edit);
    let receipt = s.execute("agent.apply", batch.clone()).unwrap();
    assert_eq!(receipt["action"], "caption_edit");
    assert_eq!(receipt["results"][0]["humanCommand"], command);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), serde_json::to_value(&*h.project).unwrap());
    assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt);
    let after = serde_json::to_value(&*s.project).unwrap();
    read(s, n + 1);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    read(s, n + 2);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), after);
    assert_eq!(serde_json::to_value(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project).unwrap(), after);
    receipt["results"][0].clone()
}

#[test]
fn every_caption_format_uses_the_human_constructor_and_exact_history_reopen() {
    for (i, format) in ["Subtitle", "CEA-608", "CEA-708", "Teletext"].iter().enumerate() {
        let mut s = setup();
        step(
            &mut s,
            30000 + i * 10,
            json!({"type":"create_track","name":"Generated captions","language":"en","format":format}),
            "captions.newTrack",
            json!({"name":"Generated captions","language":"en","format":format}),
        );
        if *format == "CEA-608" {
            assert_eq!(s.active_sequence().unwrap().caption_tracks[0].style.background_color, [0, 0, 0, 255]);
        }
    }
}

#[test]
fn caption_gestures_share_human_values_without_changing_picture_sound_or_metadata() {
    let mut s = setup();
    s.execute("captions.newTrack", json!({"name":"Generated","language":"en","format":"Subtitle"})).unwrap();
    let tid = s.active_sequence().unwrap().caption_tracks[0].id.0;
    let fd = s.sequence_rate().frame_duration().0;
    let result = step(
        &mut s,
        30100,
        json!({"type":"add","track":tid,"start_frame":0,"end_frame":24,"text":"Original"}),
        "captions.add",
        json!({"track":tid,"time":0,"duration":24*fd,"text":"Original"}),
    );
    let cue = result["caption"].as_u64().unwrap();
    step(
        &mut s,
        30110,
        json!({"type":"text","track":tid,"caption":cue,"text":"He said yes.","speaker":"Groom"}),
        "captions.setText",
        json!({"caption":cue,"text":"He said yes.","speaker":"Groom"}),
    );
    step(
        &mut s,
        30120,
        json!({"type":"times","track":tid,"caption":cue,"start_frame":3,"end_frame":18}),
        "captions.setTimes",
        json!({"caption":cue,"startTime":3*fd,"endTime":18*fd}),
    );
    let mut style = s.active_sequence().unwrap().caption_tracks[0].style.clone();
    style.size = 61.0;
    style.background = true;
    step(
        &mut s,
        30130,
        json!({"type":"style","track":tid,"style":style}),
        "captions.setStyle",
        json!({"track":tid,"font":style.font,"size":style.size,"color":style.color,"background":style.background,"backgroundColor":style.background_color,
          "align":"center","anchor":"bottom","margin":style.margin,"lineSpacing":style.line_spacing,"outline":style.outline,"outlineColor":style.outline_color}),
    );
    for (i, (kind, key, value)) in [("enabled", "enabled", false), ("lock", "locked", true), ("lock", "locked", false)].iter().enumerate() {
        let mut edit = json!({"type":kind,"track":tid});
        edit[*key] = json!(value);
        let mut params = json!({"track":tid});
        params[*key] = json!(value);
        step(&mut s, 30140 + i * 10, edit, "captions.setTrack", params);
    }
    step(&mut s, 30180, json!({"type":"delete","track":tid,"caption":cue}), "captions.delete", json!({"captions":[cue],"ripple":false}));
}

#[test]
fn caption_reads_and_text_changes_preserve_off_frame_cue_timing_and_all_other_fields() {
    let mut s = setup();
    s.execute("captions.newTrack", json!({"name":"Native originals"})).unwrap();
    s.execute("captions.add", json!({"text":"Original"})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    let t = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks[0];
    let tid = t.id.0;
    let c = &mut t.captions[0];
    c.start = filmcraft_time::Tick(17);
    c.duration = filmcraft_time::Tick(19);
    c.speaker = Some("Groom".into());
    c.cue_id = Some("original-cue".into());
    c.settings = "align:start".into();
    let cue = c.id.0;
    read(&mut s, 30200);
    let before = state(&s);
    let page = s
        .execute("agent.readCaptions", json!({"token":token(30200),"scope":{"projectId":"p_caption","anchorSequence":seq.0},"track":tid,"offset":0,"limit":1}))
        .unwrap();
    assert_eq!(page["cues"][0]["start"], 17);
    assert_eq!(page["cues"][0]["duration"], 19);
    assert_eq!(page["cues"][0]["cueId"], "original-cue");
    assert_eq!(page["nextOffset"], Value::Null);
    assert_eq!(state(&s), before);
    step(
        &mut s,
        30210,
        json!({"type":"text","track":tid,"caption":cue,"text":"Corrected","speaker":"Groom"}),
        "captions.setText",
        json!({"caption":cue,"text":"Corrected","speaker":"Groom"}),
    );
    let c = &s.active_sequence().unwrap().caption_tracks[0].captions[0];
    assert_eq!((c.start.0, c.duration.0), (17, 19));
    assert_eq!(c.cue_id.as_deref(), Some("original-cue"));
    assert_eq!(c.settings, "align:start");
}

#[test]
fn hostile_caption_requests_preserve_project_selection_history_and_redo() {
    let mut s = setup();
    s.execute("captions.newTrack", json!({"name":"Locked"})).unwrap();
    let tid = s.active_sequence().unwrap().caption_tracks[0].id.0;
    s.execute("captions.add", json!({"track":tid,"text":"Existing"})).unwrap();
    s.execute("captions.setTrack", json!({"track":tid,"locked":true})).unwrap();
    s.execute("captions.setTrack", json!({"track":tid,"enabled":false})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    for (i, edit) in [
        json!({"type":"add","track":tid,"start_frame":0,"end_frame":1,"text":"Locked"}),
        json!({"type":"lock","track":tid,"locked":true}),
        json!({"type":"lock","track":tid,"locked":"false"}),
        json!({"type":"delete","track":tid,"caption":9_007_199_254_740_992u64}),
        json!({"type":"add","track":tid,"start_frame":-1,"end_frame":1,"text":"Bad"}),
        json!({"type":"create_track","name":"Bad","language":"en","format":"Other"}),
        json!({"type":"create_track","name":"Bad","language":"en","format":"Subtitle","hidden":true}),
    ]
    .into_iter()
    .enumerate()
    {
        let n = 30300 + i;
        read(&mut s, n);
        let before = state(&s);
        let batch = request(&s, n, edit);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    for (i, mutate) in ["digest", "scope", "mixed", "not_exact"].iter().enumerate() {
        let n = 30400 + i;
        read(&mut s, n);
        let before = state(&s);
        let mut batch = request(&s, n, json!({"type":"create_track","name":"Bad","language":"en","format":"Subtitle"}));
        match *mutate {
            "digest" => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
            "scope" => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(0),
            "mixed" => {
                let op = batch["operations"][0].clone();
                batch["operations"].as_array_mut().unwrap().push(op);
            }
            _ => batch["exact"] = json!(false),
        }
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn fractional_rate_caption_ticks_overlap_refusal_and_read_bounds_remain_exact() {
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().settings.frame_rate = filmcraft_time::FrameRate::new(30000, 1001);
    s.execute("captions.newTrack", json!({"name":"Exact fractional rate"})).unwrap();
    let tid = s.active_sequence().unwrap().caption_tracks[0].id.0;
    let fd = s.sequence_rate().frame_duration().0;
    step(
        &mut s,
        30500,
        json!({"type":"add","track":tid,"start_frame":3,"end_frame":20,"text":"Exact frames"}),
        "captions.add",
        json!({"track":tid,"time":3*fd,"duration":17*fd,"text":"Exact frames"}),
    );
    let c = &s.active_sequence().unwrap().caption_tracks[0].captions[0];
    assert_eq!((c.start.0, c.duration.0), (3 * fd, 17 * fd));
    read(&mut s, 30510);
    let before = state(&s);
    let batch = request(&s, 30510, json!({"type":"add","track":tid,"start_frame":4,"end_frame":9,"text":"Overlap"}));
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
    for (i, (offset, limit)) in [(0, 0), (0, 33), (2, 1)].into_iter().enumerate() {
        let n = 30520 + i;
        read(&mut s, n);
        let before = state(&s);
        assert!(
            s.execute(
                "agent.readCaptions",
                json!({"token":token(n),"scope":{"projectId":"p_caption","anchorSequence":seq.0},"track":tid,"offset":offset,"limit":limit})
            )
            .is_err()
        );
        assert_eq!(state(&s), before);
    }
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().caption_tracks[0].captions[0];
    c.start = filmcraft_time::Tick(i64::MAX);
    c.duration = filmcraft_time::Tick(i64::MAX);
    assert_eq!(super::captions::summary(&s)["status"], "unavailable");
}
