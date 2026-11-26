use super::layout::{layout_page, layout_request};
use super::*;
use filmcraft_project::graphic::layer_indices;
use filmcraft_project::graphic_design::{GraphicMeta, LayerExtra, Pin, PinTarget, RollMode};

fn pins(s: &mut Session, n: usize) -> Value {
    let t = target(s);
    s.execute(
        "agent.readGraphicsLayout",
        json!({"token":token(n),"scope":scope(s),"target":{"kind":"pins","identity":t["identity"]},"offset":0,"limit":16,"projection":true,"index":true}),
    )
    .unwrap()
}
fn request_pins(s: &mut Session, n: usize, edit: Value) -> Value {
    let mut r = layout_request(s, n, edit);
    let p = pins(s, n);
    let t = target(s);
    r["operations"][0]["params"]["pinBasis"] = json!({"identity":t["identity"],"sha256":p["pinCollectionSha256"],"total":p["total"]});
    r
}
fn pin_graphic(s: &mut Session) {
    let t = target(s);
    s.execute("graphics.pin", json!({"clip":t["clip"],"layer":0,"to":1})).unwrap();
}

#[test]
fn pin_constraints_read_reuses_native_graph_is_complete_scoped_and_never_prepares_or_seeks() {
    let mut s = populated();
    pin_graphic(&mut s);
    let ids = layer_ids(&s);
    let t = target(&s);
    read(&mut s, 81000);
    let before = state(&s);
    let p = pins(&mut s, 81000);
    assert_eq!(p["total"], 3);
    assert_eq!(p["pinDesign"]["pinCount"], 1);
    assert_eq!(p["pinDesign"]["invalidPinCount"], 0);
    assert_eq!(p["rows"][0]["pin"]["target"], json!({"kind":"layer","layerId":ids[1]}));
    assert_eq!(p["rows"][0]["pin"]["state"], "valid");
    assert_eq!(p["rows"][2]["pin"], Value::Null);
    assert_eq!(state(&s), before);
    let old = layout_page(&mut s, 81000, json!({"kind":"layers","identity":t["identity"]}), 0, 1);
    assert_eq!(old["rows"][0].as_object().unwrap().len(), 4);
    assert!(old.get("pinDesign").is_none());
    assert_eq!(old["collectionSha256"], p["collectionSha256"]);
    let sid = s.state.active_sequence.unwrap();
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0];
    let e = layer_indices(&c.effects)[0];
    c.effects[e].layer.as_mut().unwrap().pin.as_mut().unwrap().to = PinTarget::Layer(u64::MAX);
    read(&mut s, 81001);
    let p = pins(&mut s, 81001);
    assert_eq!(p["rows"][0]["pin"]["target"], json!({"kind":"missing","uid":"ffffffffffffffff"}));
    assert_eq!(p["pinDesign"]["invalidPinCount"], 1);
}

#[test]
fn pin_and_unpin_share_explicit_frame_preparation_one_owned_step_and_exact_saved_history() {
    for (n, kind) in ["layer", "frame", "inactive", "unpin"].into_iter().enumerate() {
        let mut s = populated();
        if kind == "unpin" {
            pin_graphic(&mut s);
        }
        let t = target(&s);
        let ids = layer_ids(&s);
        let mut h = human(&s);
        let before = s.project.clone();
        let view = serde_json::to_value(&s.state).unwrap();
        let undo = s.history.undo.len();
        let edit = if kind == "unpin" {
            json!({"type":"unpin_layer","target":t,"layerId":ids[0],"frame":5})
        } else {
            json!({"type":"pin_layer","target":t,"layerId":ids[0],"to":if kind=="layer"{json!({"kind":"layer","layerId":ids[1]})}else{json!({"kind":"frame"})},"edges":if kind=="inactive"{json!([])}else{json!(["left","bottom"])},"frame":5})
        };
        let req = request_pins(&mut s, 81100 + n, edit);
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        let time = h.sequence_rate().tick_of(5);
        let to = if kind == "unpin" {
            json!("none")
        } else if kind == "layer" {
            json!(1)
        } else {
            json!("frame")
        };
        let args = if kind == "unpin" {
            json!({"clip":t["clip"],"layer":0,"to":to,"time":time.0})
        } else {
            json!({"clip":t["clip"],"layer":0,"to":to,"edges":if kind=="inactive"{json!([])}else{json!(["left","bottom"])},"time":time.0})
        };
        h.execute("graphics.pin", args).unwrap();
        assert_eq!(*s.project, *h.project);
        assert_ne!(*s.project, *before);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(receipt["results"][0]["humanCommand"], "graphics.pin");
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        let after = s.project.clone();
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
        read(&mut s, 81200 + n);
        s.execute("agent.undo", json!({"token":token(81200+n),"commit":token(81100+n)})).unwrap();
        assert_eq!(*s.project, *before);
        read(&mut s, 81300 + n);
        s.execute("agent.redo", json!({"token":token(81300+n),"commit":token(81100+n),"undo":token(81200+n)})).unwrap();
        assert_eq!(*s.project, *after);
    }
}

#[test]
fn pinned_alignment_and_distribution_require_complete_constraints_and_match_all_human_modes() {
    for (n, (align, mode)) in
        ["left", "hcenter", "right", "top", "vcenter", "bottom"].into_iter().flat_map(|a| ["frame", "group", "selection"].map(|m| (a, m))).enumerate()
    {
        let mut s = populated();
        pin_graphic(&mut s);
        let t = target(&s);
        let ids = layer_ids(&s);
        let mut h = human(&s);
        let before = s.project.clone();
        let r = request_pins(&mut s, 81400 + n, json!({"type":"align_layers","target":t,"layerIds":ids,"frame":5,"align":align,"mode":mode}));
        s.execute("agent.apply", r).unwrap();
        let at = h.sequence_rate().tick_of(5);
        h.execute("graphics.align", json!({"clip":t["clip"],"layers":[0,1,2],"align":align,"to":mode,"time":at.0})).unwrap();
        assert_eq!(*s.project, *h.project, "{align}/{mode}");
        assert_ne!(*s.project, *before);
    }
    for (n, (axis, spacing)) in ["horizontal", "vertical"].into_iter().flat_map(|a| ["centers", "gaps"].map(|m| (a, m))).enumerate() {
        let mut s = populated();
        pin_graphic(&mut s);
        let t = target(&s);
        let ids = layer_ids(&s);
        let mut h = human(&s);
        let r = request_pins(&mut s, 81500 + n, json!({"type":"distribute_layers","target":t,"layerIds":ids,"frame":5,"axis":axis,"spacing":spacing}));
        s.execute("agent.apply", r).unwrap();
        let at = h.sequence_rate().tick_of(5);
        h.execute("graphics.distribute", json!({"clip":t["clip"],"layers":[0,1,2],"axis":axis,"space":spacing=="gaps","time":at.0})).unwrap();
        assert_eq!(*s.project, *h.project);
    }
}

#[test]
fn pin_constraint_basis_hostile_fields_locks_time_design_and_late_human_changes_never_publish() {
    for case in [
        "missing_basis",
        "sha",
        "count",
        "identity",
        "locked",
        "protected",
        "responsive",
        "roll",
        "hold",
        "edges",
        "self",
        "foreign",
        "late",
        "disabled",
        "extra",
    ] {
        let mut s = populated();
        let t = target(&s);
        let ids = layer_ids(&s);
        s.execute("graphics.newText", json!({"text":"Keep Redo","frame":100,"track":1})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        let sid = s.state.active_sequence.unwrap();
        let c = &mut Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0];
        match case {
            "protected" => c.identity.as_mut().unwrap().protection = Some("human".into()),
            "responsive" => {
                c.graphic = Some(Box::new(GraphicMeta { intro: filmcraft_time::Tick(1), design_duration: filmcraft_time::Tick(10), ..Default::default() }))
            }
            "roll" => {
                c.graphic = Some(Box::new(GraphicMeta {
                    roll: filmcraft_project::graphic_design::Roll { mode: RollMode::Roll, ..Default::default() },
                    ..Default::default()
                }))
            }
            "hold" => c.frame_hold = Some(filmcraft_time::Tick(i64::MIN)),
            "disabled" => {
                let i = layer_indices(&c.effects)[0];
                c.effects[i].enabled = false;
            }
            _ => {}
        }
        if case == "locked" {
            Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].locked = true;
        }
        let edit = if case == "disabled" {
            json!({"type":"align_layers","target":t,"layerIds":ids,"frame":5,"align":"right","mode":"group"})
        } else {
            json!({"type":"pin_layer","target":t,"layerId":ids[0],"to":{"kind":"layer","layerId":ids[1]},"edges":["left"],"frame":5})
        };
        let mut r = request_pins(&mut s, 81600, edit);
        let p = &mut r["operations"][0]["params"];
        match case {
            "missing_basis" => {
                p.as_object_mut().unwrap().remove("pinBasis");
            }
            "sha" => p["pinBasis"]["sha256"] = json!("f".repeat(64)),
            "count" => p["pinBasis"]["total"] = json!(2),
            "identity" => p["pinBasis"]["identity"] = json!("f".repeat(32)),
            "edges" => p["edit"]["edges"] = json!(["left", "left"]),
            "self" => p["edit"]["to"]["layerId"] = json!(ids[0]),
            "foreign" => p["edit"]["to"]["layerId"] = json!("f".repeat(32)),
            "extra" => p["force"] = json!(true),
            _ => {}
        }
        if case == "late" {
            s.execute("graphics.set", json!({"clip":t["clip"],"layer":2,"props":{"position":[17,19]},"merge":false,"begin":false})).unwrap();
        }
        let before = state(&s);
        let result = s.execute("agent.apply", r);
        assert!(result.is_err(), "{case}: {result:?}");
        assert_eq!(state(&s), before, "{case}");
    }
}

#[test]
fn diagnosed_missing_and_cyclic_pin_constraints_can_be_explicitly_repaired_with_owned_undo() {
    for cyclic in [false, true] {
        let mut s = populated();
        let sid = s.state.active_sequence.unwrap();
        let c = &mut Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0];
        let indices = layer_indices(&c.effects);
        for (n, i) in indices.iter().enumerate() {
            c.effects[*i].layer = Some(Box::new(LayerExtra { uid: n as u64 + 1, ..Default::default() }));
        }
        c.effects[indices[0]].layer.as_mut().unwrap().pin = Some(Pin { to: PinTarget::Layer(if cyclic { 2 } else { 404 }), left: true, ..Default::default() });
        if cyclic {
            c.effects[indices[1]].layer.as_mut().unwrap().pin = Some(Pin { to: PinTarget::Layer(1), left: true, ..Default::default() });
        }
        let before = s.project.clone();
        let t = target(&s);
        let ids = layer_ids(&s);
        let r = request_pins(&mut s, 81700, json!({"type":"unpin_layer","target":t,"layerId":ids[0],"frame":5}));
        assert!(pins(&mut s, 81700)["pinDesign"]["invalidPinCount"].as_u64().unwrap() > 0);
        s.execute("agent.apply", r).unwrap();
        read(&mut s, 81701);
        let after = pins(&mut s, 81701);
        assert_eq!(after["pinDesign"]["invalidPinCount"], 0);
        s.execute("agent.undo", json!({"token":token(81701),"commit":token(81700)})).unwrap();
        assert_eq!(*s.project, *before);
    }
}

#[test]
fn pin_stop_after_shared_preparation_discards_the_whole_candidate_without_replay() {
    let mut s = populated();
    let t = target(&s);
    let ids = layer_ids(&s);
    let r = request_pins(&mut s, 81800, json!({"type":"pin_layer","target":t,"layerId":ids[0],"to":{"kind":"frame"},"edges":["left"],"frame":5}));
    let control = s.agent_operation_control();
    let before = state(&s);
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        let batch = crate::agent_bridge::parse("agent.apply", p)?;
        let staged = crate::agent_bridge::stage_batch(s, &batch)?;
        assert_eq!(control.cancel(&json!({"token":token(81800)}))?["cancellationAccepted"], true);
        crate::agent_bridge::publish_batch(s, &batch, staged, p, None)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn complete_pin_constraints_cover_all4096_layers_without_hidden_property_or_page_limits() {
    let mut s = populated();
    let t = target(&s);
    let sid = s.state.active_sequence.unwrap();
    let c = &mut Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0];
    let mut e = c.effects.iter().find(|e| e.effect == "graphic_shape").unwrap().clone();
    e.params.clear();
    e.masks.clear();
    c.effects = (0..4096)
        .map(|n| {
            let mut e = e.clone();
            e.instance_id = Some(format!("{n:032x}"));
            e.layer =
                Some(Box::new(LayerExtra { uid: n + 1, pin: Some(Pin { to: PinTarget::Frame, left: true, ..Default::default() }), ..Default::default() }));
            e
        })
        .collect();
    read(&mut s, 81900);
    let before = state(&s);
    let p = pins(&mut s, 81900);
    assert_eq!(p["total"], 4096);
    assert_eq!(p["rows"].as_array().unwrap().len(), 4096);
    assert_eq!(p["pinDesign"]["pinCount"], 4096);
    assert_eq!(p["pinDesign"]["invalidPinCount"], 0);
    let size = serde_json::to_vec(&p).unwrap().len();
    assert!(size > 64 * 1024 && size < 2 * 1024 * 1024);
    assert_eq!(p["rows"][4095]["layerId"], format!("{:032x}", 4095));
    assert_eq!(state(&s), before);
    let small = s
        .execute(
            "agent.readGraphicsLayout",
            json!({"token":token(81900),"scope":scope(&s),"target":{"kind":"pins","identity":t["identity"]},"offset":4095,"limit":1,"projection":true}),
        )
        .unwrap();
    assert_eq!(small["pinCollectionSha256"], p["pinCollectionSha256"]);
    assert_eq!(small["rows"].as_array().unwrap().len(), 1);
}
