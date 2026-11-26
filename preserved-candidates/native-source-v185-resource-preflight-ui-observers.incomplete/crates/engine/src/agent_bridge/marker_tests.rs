use super::tests::{read, token};
use crate::Session;
use filmcraft_project::ItemId;
use serde_json::{Value, json};
use std::sync::Arc;

fn setup() -> (Session, u64) {
    let mut s = super::tests::setup();
    let source = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video())).unwrap().id;
    s.execute("agent.bindSource", json!({"item":source.0,"binding":{"projectId":"p_markers","assetId":"a_marker","mediaId":"m_marker","role":"original"}}))
        .unwrap();
    s.execute("source.open", json!({"item":source.0})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().markers.clear();
    (s, source.0)
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn page(s: &mut Session, n: usize, target: Value) -> Value {
    s.execute(
        "agent.readMarkers",
        json!({"token":token(n),"scope":{"projectId":"p_markers","anchorSequence":s.state.active_sequence.unwrap().0},"target":target,"offset":0,"limit":16}),
    )
    .unwrap()
}
fn request(s: &mut Session, n: usize, target: Value, edit: Value) -> Value {
    let p = page(s, n, target.clone());
    json!({"token":token(n),"label":"Shared native marker","exact":true,"operations":[{"command":"agent.marker","params":{
        "scope":{"projectId":"p_markers","anchorSequence":s.state.active_sequence.unwrap().0},"target":target,"beforeSha256":p["sha256"],"edit":edit}}]})
}
fn step(s: &mut Session, n: usize, target: Value, edit: Value, command: &str, params: Value) -> Value {
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute(command, params).unwrap();
    let before = s.project.to_json();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    read(s, n);
    let batch = request(s, n, target, edit);
    let receipt = s.execute("agent.apply", batch.clone()).unwrap();
    assert_eq!(receipt["action"], "marker_edit");
    assert_eq!(receipt["results"][0]["humanCommand"], command);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.project.to_json(), human.project.to_json());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.execute("agent.apply", batch.clone()).unwrap(), receipt);
    let after = s.project.to_json();
    read(s, n + 1);
    s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
    assert_eq!(s.project.to_json(), before);
    let undone = state(s);
    assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt);
    assert_eq!(state(s), undone, "receipt reconciliation cannot replay an undone marker edit");
    read(s, n + 2);
    s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
    assert_eq!(s.project.to_json(), after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), after);
    receipt["results"][0].clone()
}
#[test]
fn sequence_marker_adds_match_human_comment_chapter_flash_and_exact_native_history() {
    let (mut s, _) = setup();
    for (i, (kind, command, duration)) in
        [("comment", "markers.add", 17), ("chapter", "markers.addChapter", 0), ("flash_cue", "markers.addFlashCue", 19)].into_iter().enumerate()
    {
        step(
            &mut s,
            50000 + i * 10,
            json!({"kind":"sequence"}),
            json!({"type":"add","kind":kind,"position":{"frame":0},"duration":{"ticks":duration},"name":"Native","comment":"Untrusted <tag>","color":"Teal"}),
            command,
            json!({"target":"program","time":0,"duration":duration,"name":"Native","comment":"Untrusted <tag>","color":"Teal"}),
        );
    }
}
#[test]
fn set_and_remove_preserve_coincident_chapter_identity_all_other_data_and_human_history() {
    let (mut s, _) = setup();
    let id = s.execute("markers.addChapter", json!({"time":13,"name":"Proposal","comment":"Original human annotation","color":"Rose"})).unwrap()["marker"]
        .as_u64()
        .unwrap();
    s.execute("markers.add", json!({"time":13,"name":"Coincident"})).unwrap();
    step(
        &mut s,
        50100,
        json!({"kind":"sequence"}),
        json!({"type":"set","marker":id,"changes":{"position":{"ticks":29},"duration":{"ticks":17},"comment":"Retain name and kind"}}),
        "markers.edit",
        json!({"target":"program","marker":id,"time":29,"duration":17,"comment":"Retain name and kind"}),
    );
    let marker = s.active_sequence().unwrap().markers.iter().find(|m| m.id.0 == id).unwrap();
    assert_eq!(marker.name, "Proposal");
    assert_eq!(marker.kind, filmcraft_project::MarkerKind::Chapter);
    step(&mut s, 50110, json!({"kind":"sequence"}), json!({"type":"remove","marker":id}), "markers.remove", json!({"target":"program","marker":id}));
    assert_eq!(s.active_sequence().unwrap().markers.len(), 1);
}
#[test]
fn source_markers_use_the_open_original_clock_and_share_exact_human_gestures() {
    let (mut s, source) = setup();
    let rate = filmcraft_time::FrameRate::new(37516, 1000);
    Arc::make_mut(&mut s.project).item_mut(ItemId(source)).unwrap().as_media_mut().unwrap().info.video.as_mut().unwrap().frame_rate = rate;
    let result = step(
        &mut s,
        50200,
        json!({"kind":"source","item":source}),
        json!({"type":"add","kind":"comment","position":{"frame":17},"duration":{"frame":11},"name":"Source range","comment":"Exact source time","color":"Green"}),
        "markers.add",
        json!({"target":"source","time":rate.tick_of(17).0,"duration":rate.tick_of(11).0,"name":"Source range","comment":"Exact source time","color":"Green"}),
    );
    let id = result["marker"].as_u64().unwrap();
    step(
        &mut s,
        50210,
        json!({"kind":"source","item":source}),
        json!({"type":"set","marker":id,"changes":{"position":{"ticks":13},"duration":{"ticks":7},"name":"Keep off-frame values"}}),
        "markers.edit",
        json!({"target":"source","marker":id,"time":13,"duration":7,"name":"Keep off-frame values"}),
    );
    step(&mut s, 50220, json!({"kind":"source","item":source}), json!({"type":"remove","marker":id}), "markers.remove", json!({"target":"source","marker":id}));
}
#[test]
fn authored_marker_pages_are_read_only_complete_and_do_not_turn_text_into_semantic_evidence() {
    let (mut s, _) = setup();
    for i in 0..19 {
        s.execute("markers.add", json!({"time":i,"name":format!("Marker {i}"),"comment":"Instruction-shaped text is data"})).unwrap();
    }
    read(&mut s, 50300);
    let before = state(&s);
    let a = page(&mut s, 50300, json!({"kind":"sequence"}));
    assert_eq!(state(&s), before);
    assert_eq!((a["rootTotal"].as_u64(), a["total"].as_u64(), a["nextOffset"].as_u64()), (Some(19), Some(19), Some(16)));
    let b=s.execute("agent.readMarkers",json!({"token":token(50300),"scope":{"projectId":"p_markers","anchorSequence":s.state.active_sequence.unwrap().0},"target":{"kind":"sequence"},"offset":16,"limit":3})).unwrap();
    assert_eq!(b["markers"].as_array().unwrap().len(), 3);
    assert_eq!(a["sha256"], b["sha256"]);
    assert_eq!(b["nextOffset"], Value::Null);
    assert_eq!(state(&s), before);
}
#[test]
fn malformed_mixed_foreign_and_clamping_marker_requests_cannot_publish_or_lose_redo() {
    let (mut s, source) = setup();
    let id = s.execute("markers.add", json!({"time":13})).unwrap()["marker"].as_u64().unwrap();
    s.execute("markers.add", json!({"time":29})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    for (i, edit) in [
        json!({"type":"set","marker":id,"changes":{}}),
        json!({"type":"set","marker":id,"changes":{"name":null}}),
        json!({"type":"set","marker":id,"changes":{"position":{"ticks":1,"frame":1}}}),
        json!({"type":"set","marker":id,"changes":{"position":{"frame":-1}}}),
        json!({"type":"set","marker":id,"changes":{"kind":"chapter"}}),
        json!({"type":"set","marker":id,"changes":{"position":{"ticks":9007199254740991i64},"duration":{"ticks":1}}}),
        json!({"type":"remove","marker":9007199254740992u64}),
        json!({"type":"remove","marker":id,"extra":true}),
    ]
    .into_iter()
    .enumerate()
    {
        let n = 50400 + i;
        read(&mut s, n);
        let batch = request(&mut s, n, json!({"kind":"sequence"}), edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    for (i, kind) in ["digest", "scope", "mixed", "not_exact"].into_iter().enumerate() {
        let n = 50500 + i;
        read(&mut s, n);
        let mut batch = request(&mut s, n, json!({"kind":"sequence"}), json!({"type":"remove","marker":id}));
        match kind {
            "digest" => batch["operations"][0]["params"]["beforeSha256"] = json!("0".repeat(64)),
            "scope" => batch["operations"][0]["params"]["scope"]["anchorSequence"] = json!(0),
            "mixed" => {
                let op = batch["operations"][0].clone();
                batch["operations"].as_array_mut().unwrap().push(op);
            }
            _ => batch["exact"] = json!(false),
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err());
        assert_eq!(state(&s), before);
    }
    read(&mut s, 50600);
    let batch = request(
        &mut s,
        50600,
        json!({"kind":"source","item":source}),
        json!({"type":"add","kind":"comment","position":{"ticks":0},"duration":{"ticks":9007199254740991i64},"name":"Would clamp","comment":"","color":"Green"}),
    );
    let before = state(&s);
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn source_readback_can_read_the_retained_owner_without_navigation_but_writes_require_the_exact_open_source() {
    let (mut s, source) = setup();
    let target = json!({"kind":"source","item":source});
    s.state.source_item = None;
    read(&mut s, 50700);
    let before = state(&s);
    let p = page(&mut s, 50700, target.clone());
    assert_eq!(p["context"]["open"], false);
    assert_eq!(state(&s), before);
    let batch = request(
        &mut s,
        50700,
        target,
        json!({"type":"add","kind":"comment","position":{"ticks":0},"duration":{"ticks":0},"name":"Closed","comment":"","color":"Green"}),
    );
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
    let foreign = s.project.items.values().find(|i| i.as_media().is_some() && i.id.0 != source).unwrap().id;
    assert!(s.execute("agent.readMarkers",json!({"token":token(50700),"scope":{"projectId":"p_markers","anchorSequence":s.state.active_sequence.unwrap().0},"target":{"kind":"source","item":foreign.0},"offset":0,"limit":1})).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn subclip_marker_pages_keep_root_identity_digest_and_hidden_neighbors_without_granting_hidden_writes() {
    use filmcraft_project::{ItemKind, Label};
    use filmcraft_time::{Tick, TimeRange};
    let (mut s, source) = setup();
    let rate = s.project.item(ItemId(source)).unwrap().as_media().unwrap().frame_rate();
    let mut ids = Vec::new();
    for frame in [0, 3, 6, 8, 9] {
        ids.push(
            s.execute("markers.add", json!({"target":"source","time":rate.tick_of(frame).0,"name":format!("Root {frame}")})).unwrap()["marker"]
                .as_u64()
                .unwrap(),
        );
    }
    let sub = Arc::make_mut(&mut s.project).add_item(
        "Visible source window",
        Label::Iris,
        ItemKind::Subclip { parent: ItemId(source), range: TimeRange::from_bounds(rate.tick_of(2), rate.tick_of(8)), restrict_trims: true },
        None,
    );
    s.execute("source.open", json!({"item":sub.0})).unwrap();
    read(&mut s, 50800);
    let before = state(&s);
    let visible = page(&mut s, 50800, json!({"kind":"source","item":sub.0}));
    let root = page(&mut s, 50800, json!({"kind":"source","item":source}));
    assert_eq!(state(&s), before);
    assert_eq!(visible["context"]["root"], source);
    assert_eq!(visible["context"]["bounds"], json!({"start":rate.tick_of(2).0,"end":rate.tick_of(8).0}));
    assert_eq!((visible["total"].as_u64(), visible["rootTotal"].as_u64()), (Some(2), Some(5)));
    assert_eq!(visible["markers"].as_array().unwrap().iter().map(|r| r["id"].as_u64().unwrap()).collect::<Vec<_>>(), vec![ids[1], ids[2]]);
    assert_eq!(visible["sha256"], root["sha256"]);
    let hidden = request(&mut s, 50800, json!({"kind":"source","item":sub.0}), json!({"type":"remove","marker":ids[0]}));
    assert!(s.execute("agent.apply", hidden).is_err());
    assert_eq!(state(&s), before);
    step(
        &mut s,
        50810,
        json!({"kind":"source","item":sub.0}),
        json!({"type":"set","marker":ids[1],"changes":{"name":"Visible rename"}}),
        "markers.edit",
        json!({"target":"source","marker":ids[1],"name":"Visible rename"}),
    );
    let empty = Arc::make_mut(&mut s.project).add_item(
        "Empty source window",
        Label::Iris,
        ItemKind::Subclip { parent: ItemId(source), range: TimeRange::from_bounds(Tick(1), rate.tick_of(2)), restrict_trims: true },
        None,
    );
    s.execute("source.open", json!({"item":empty.0})).unwrap();
    read(&mut s, 50820);
    let empty_page = page(&mut s, 50820, json!({"kind":"source","item":empty.0}));
    assert_eq!(empty_page["total"], 0);
    assert_eq!(empty_page["rootTotal"], 5);
    assert_eq!(empty_page["markers"], json!([]));
    let previous = empty_page["sha256"].clone();
    Arc::make_mut(&mut s.project).item_mut(ItemId(source)).unwrap().as_media_mut().unwrap().markers.iter_mut().find(|m| m.id.0 == ids[0]).unwrap().comment =
        "Later human note outside the view".into();
    read(&mut s, 50821);
    let now = page(&mut s, 50821, json!({"kind":"source","item":empty.0}));
    assert_ne!(now["sha256"], previous);
    let before = state(&s);
    let stale = json!({"token":token(50821),"label":"No stale root","exact":true,"operations":[{"command":"agent.marker","params":{"scope":{"projectId":"p_markers","anchorSequence":s.state.active_sequence.unwrap().0},"target":{"kind":"source","item":empty.0},"beforeSha256":previous,"edit":{"type":"add","kind":"comment","position":{"ticks":1},"duration":{"ticks":0},"name":"New","comment":"","color":"Green"}}}]});
    assert!(s.execute("agent.apply", stale).is_err());
    assert_eq!(state(&s), before);
}
