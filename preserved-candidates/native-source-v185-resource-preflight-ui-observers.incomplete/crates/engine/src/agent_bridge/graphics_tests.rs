#[path = "graphics_character_edit_tests.rs"]
mod character_edit;
#[path = "graphics_character_tests.rs"]
mod characters;
#[path = "graphics_font_edit_tests.rs"]
mod font_edit;
#[path = "graphics_font_service_tests.rs"]
mod font_services;
#[path = "graphics_font_tests.rs"]
mod fonts;
#[path = "graphics_shared_font_tests.rs"]
mod shared_fonts;
use super::{
    graphics, project_audit,
    tests::{read, token},
};
use crate::Session;
use filmcraft_project::Project;
use serde_json::{Value, json};
use std::sync::Arc;

#[path = "graphics_pin_tests.rs"]
mod pin_design;

mod layout {
    use super::*;
    use crate::agent_bridge::{Operation, graphics_layout};

    pub(super) fn layout_page(s: &mut Session, n: usize, target: Value, offset: usize, limit: usize) -> Value {
        s.execute("agent.readGraphicsLayout", json!({"token":token(n),"scope":scope(s),"target":target,"offset":offset,"limit":limit,"projection":true}))
            .unwrap()
    }
    pub(super) fn layout_request(s: &mut Session, n: usize, edit: Value) -> Value {
        read(s, n);
        let digest = layout_page(s, n, json!({"kind":"clips"}), 0, 1)["collectionSha256"].clone();
        json!({"token":token(n),"label":"Layout gesture","exact":true,"authority":"document_state","operations":[{
            "command":graphics_layout::COMMAND,"params":{"scope":scope(s),"beforeLayoutSha256":digest,"edit":edit}}]})
    }
    fn videos() -> Session {
        use filmcraft_geom::Vec2;
        use filmcraft_project::ParamValue;
        let mut s = fixture();
        s.execute("file.newOfflineFile", json!({"name":"Video","seconds":4,"fps":24,"video":true,"audio":false,"width":32,"height":24})).unwrap();
        let item = s.project.items.values().find(|i| i.as_media().is_some()).unwrap().id.0;
        for track in ["V1", "V2"] {
            s.execute("timeline.place", json!({"item":item,"track":track,"time":0,"sourceIn":0,"duration":254016000000i64})).unwrap();
        }
        let sid = s.state.active_sequence.unwrap();
        for (i, t) in Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks.iter_mut().enumerate() {
            let m = t.items[0].effects.iter_mut().find(|e| e.effect == "motion").unwrap();
            m.params.get_mut("position").unwrap().value = ParamValue::Vec2(Vec2::new(20.0 + i as f64 * 60.0, 30.0 + i as f64 * 40.0));
        }
        s.execute("poema.trackSequence", json!({})).unwrap();
        s.execute("poema.trackEffects", json!({})).unwrap();
        s
    }
    fn video_targets(s: &Session) -> Vec<Value> {
        s.active_sequence()
            .unwrap()
            .video_tracks
            .iter()
            .flat_map(|t| &t.items)
            .map(|c| json!({"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id}))
            .collect()
    }

    #[test]
    fn graphics_layout_inventory_is_read_only_complete_and_covers_video_motion_and_source_headers() {
        let mut s = videos();
        let snapshot = read(&mut s, 71200);
        assert_eq!(snapshot["capabilities"]["graphicsLayout"], true);
        assert!(crate::agent_bridge::commands().iter().any(|c| c.id == "agent.readGraphicsLayout"));
        assert!(!crate::agent_bridge::commands().iter().any(|c| c.id == graphics_layout::COMMAND));
        let before = state(&s);
        let a = layout_page(&mut s, 71200, json!({"kind":"clips"}), 0, 1);
        let b = layout_page(&mut s, 71200, json!({"kind":"clips"}), 1, 1);
        assert_eq!(a["total"], 2);
        assert_eq!(a["nextOffset"], 1);
        assert_eq!(b["nextOffset"], Value::Null);
        assert_eq!(a["collectionSha256"], b["collectionSha256"]);
        assert_eq!(a["effectTracking"], true);
        assert_eq!(a["requiresSourceValidation"], true);
        assert_eq!(a["rows"][0]["graphic"], false);
        assert!(a["rows"][0]["motionId"].is_string());
        assert_eq!(a["rows"][0]["sourceSize"], json!([32, 24]));
        assert_eq!(state(&s), before);
        let prior = graphics_layout::digest(&s).unwrap();
        let sid = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[1].items[0].name = "Changed outside page".into();
        assert_ne!(graphics_layout::digest(&s).unwrap(), prior);
        let prior = graphics_layout::digest(&s).unwrap();
        let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
        Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.video.as_mut().unwrap().width = 64;
        assert_ne!(graphics_layout::digest(&s).unwrap(), prior);
        let current = state(&s);
        assert!(
            s.execute(
                "agent.readGraphicsLayout",
                json!({"token":token(71200),"scope":scope(&s),"target":{"kind":"clips"},"offset":0,"limit":1,"projection":true})
            )
            .is_err()
        );
        assert_eq!(state(&s), current);
    }

    #[test]
    fn graphics_layout_all_alignment_and_distribution_modes_match_human_services_and_owned_history() {
        for (n, (align, mode)) in
            ["left", "hcenter", "right", "top", "vcenter", "bottom"].into_iter().flat_map(|a| ["frame", "group", "selection"].map(|m| (a, m))).enumerate()
        {
            let mut s = populated();
            let mut h = human(&s);
            let before = s.project.clone();
            let t = target(&s);
            let ids = layer_ids(&s);
            let view = serde_json::to_value(&s.state).unwrap();
            let undo = s.history.undo.len();
            let req = layout_request(&mut s, 71300 + n, json!({"type":"align_layers","target":t,"layerIds":ids,"frame":5,"align":align,"mode":mode}));
            let receipt = s.execute("agent.apply", req.clone()).unwrap();
            let time = h.sequence_rate().tick_of(5);
            h.execute("graphics.align", json!({"clip":t["clip"],"layers":[0,1,2],"align":align,"to":mode,"time":time.0})).unwrap();
            assert_eq!(*s.project, *h.project, "{align}/{mode}");
            assert_ne!(*s.project, *before);
            assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
            assert_eq!(s.history.undo.len(), undo + 1);
            assert_eq!(receipt["action"], "graphics_layout_edit");
            assert_eq!(receipt["authority"], "document_state");
            assert_eq!(receipt["results"][0]["humanCommand"], "graphics.align");
            assert_ne!(receipt["results"][0]["beforeLayoutSha256"], receipt["results"][0]["afterLayoutSha256"]);
            assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
            assert_eq!(s.history.undo.len(), undo + 1);
            let after = s.project.clone();
            assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
            read(&mut s, 71400 + n);
            s.execute("agent.undo", json!({"token":token(71400+n),"commit":token(71300+n)})).unwrap();
            assert_eq!(*s.project, *before);
            read(&mut s, 71500 + n);
            s.execute("agent.redo", json!({"token":token(71500+n),"commit":token(71300+n),"undo":token(71400+n)})).unwrap();
            assert_eq!(*s.project, *after);
        }
        for (n, (axis, spacing)) in ["horizontal", "vertical"].into_iter().flat_map(|a| ["centers", "gaps"].map(|m| (a, m))).enumerate() {
            let mut s = populated();
            let mut h = human(&s);
            let t = target(&s);
            let ids = layer_ids(&s);
            let req = layout_request(&mut s, 71600 + n, json!({"type":"distribute_layers","target":t,"layerIds":ids,"frame":5,"axis":axis,"spacing":spacing}));
            s.execute("agent.apply", req).unwrap();
            let time = h.sequence_rate().tick_of(5);
            h.execute("graphics.distribute", json!({"clip":t["clip"],"layers":[0,1,2],"axis":axis,"space":spacing=="gaps","time":time.0})).unwrap();
            assert_eq!(*s.project, *h.project, "{axis}/{spacing}");
        }
    }

    #[test]
    fn graphics_layout_video_groups_use_shared_motion_preserve_linked_content_and_refuse_late_lock() {
        let mut s = videos();
        let targets = video_targets(&s);
        let mut h = human(&s);
        let before = s.project.clone();
        let view = serde_json::to_value(&s.state).unwrap();
        let undo = s.history.undo.len();
        let req = layout_request(&mut s, 71700, json!({"type":"align_clips","targets":targets,"frame":0,"align":"left","mode":"group"}));
        let receipt = s.execute("agent.apply", req).unwrap();
        h.execute("graphics.align", json!({"clips":targets.iter().map(|t|t["clip"].clone()).collect::<Vec<_>>(),"time":0,"align":"left","to":"group"}))
            .unwrap();
        assert_eq!(*s.project, *h.project);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
        assert_eq!(receipt["results"][0]["beforeCount"], 2);
        assert_eq!(receipt["results"][0]["afterCount"], 2);
        read(&mut s, 71701);
        s.execute("agent.undo", json!({"token":token(71701),"commit":token(71700)})).unwrap();
        assert_eq!(*s.project, *before);
        s.execute("timeline.setTrack", json!({"track":"V2","locked":true})).unwrap();
        let req = layout_request(&mut s, 71702, json!({"type":"align_clips","targets":targets,"frame":0,"align":"right","mode":"group"}));
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err());
        assert_eq!(state(&s), before);
    }

    #[test]
    fn graphics_layout_hostile_requests_preserve_project_view_journal_and_both_history_branches() {
        for case in [
            "digest",
            "target",
            "identity",
            "layer",
            "duplicate",
            "scope",
            "frame",
            "fraction",
            "unknown",
            "mixed",
            "exact",
            "no_op",
            "locked",
            "protected",
            "tracking",
            "shared",
            "pin",
            "responsive",
            "wrong_type",
        ] {
            let mut s = populated();
            let t = target(&s);
            let ids = layer_ids(&s);
            s.execute("graphics.newText", json!({"text":"Redo","frame":200,"track":1})).unwrap();
            s.execute("edit.undo", json!({})).unwrap();
            assert!(!s.history.redo.is_empty());
            let sid = s.state.active_sequence.unwrap();
            let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
            match case {
                "locked" => Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].locked = true,
                "protected" => {
                    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().protection =
                        Some("Human protection".into())
                }
                "tracking" => {
                    Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
                }
                "shared" => {
                    Arc::make_mut(&mut s.project).source_graphics.insert(item, Default::default());
                }
                "pin" => {
                    s.execute("graphics.pin", json!({"clip":t["clip"],"layer":0,"to":1})).unwrap();
                }
                "responsive" => {
                    s.execute("graphics.setRoll", json!({"clip":t["clip"],"mode":"roll"})).unwrap();
                }
                "no_op" => {
                    // A single-layer Selection alignment deliberately uses the
                    // human Frame fallback. Establish the actual fixed point.
                    s.execute("graphics.align", json!({"clip":t["clip"],"layers":[0],"align":"left","to":"frame","time":0})).unwrap();
                    s.execute("graphics.newText", json!({"text":"Redo retained","frame":200,"track":1})).unwrap();
                    s.execute("edit.undo", json!({})).unwrap();
                }
                _ => {}
            }
            let mut req = layout_request(&mut s, 71800, json!({"type":"align_layers","target":t,"layerIds":ids,"frame":0,"align":"right","mode":"group"}));
            let p = &mut req["operations"][0]["params"];
            match case {
                "digest" => p["beforeLayoutSha256"] = json!("0".repeat(64)),
                "target" => p["edit"]["target"]["clip"] = json!(999999),
                "identity" => p["edit"]["target"]["identity"] = json!("f".repeat(32)),
                "layer" => p["edit"]["layerIds"][1] = json!("f".repeat(32)),
                "duplicate" => p["edit"]["layerIds"] = json!([ids[0], ids[0]]),
                "scope" => p["scope"]["projectId"] = json!("foreign"),
                "frame" => p["edit"]["frame"] = json!(-1),
                "fraction" => p["edit"]["frame"] = json!(0.5),
                "unknown" => p["edit"]["force"] = json!(true),
                "mixed" => {
                    let op = req["operations"][0].clone();
                    req["operations"].as_array_mut().unwrap().push(op);
                }
                "exact" => req["exact"] = json!(false),
                "wrong_type" => p["edit"] = json!({"type":"align_clips","targets":[t],"frame":0,"align":"right","mode":"frame"}),
                "no_op" => p["edit"] = json!({"type":"align_layers","target":t,"layerIds":[ids[0]],"frame":0,"align":"left","mode":"frame"}),
                _ => {}
            }
            let before = state(&s);
            let error = s.execute("agent.apply", req).unwrap_err();
            assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }), "{case}: {error}");
            assert_eq!(state(&s), before, "{case}");
        }
    }

    #[test]
    fn graphics_layout_index_paging_and_late_human_work_never_grant_stale_authority_or_replay() {
        let mut s = populated();
        let t = target(&s);
        read(&mut s, 71900);
        let before = state(&s);
        let a = layout_page(&mut s, 71900, json!({"kind":"layers","identity":t["identity"]}), 0, 2);
        let b = layout_page(&mut s, 71900, json!({"kind":"layers","identity":t["identity"]}), 2, 2);
        assert_eq!(a["total"], 3);
        assert_eq!(a["rows"][1]["position"], 1);
        assert_eq!(b["rows"][0]["position"], 2);
        assert_eq!(a["collectionSha256"], b["collectionSha256"]);
        assert_eq!(state(&s), before);
        for args in [json!({"kind":"layers","identity":"f".repeat(32)}), json!({"kind":"layers","identity":t["identity"],"frame":0})] {
            assert!(
                s.execute("agent.readGraphicsLayout", json!({"token":token(71900),"scope":scope(&s),"target":args,"offset":0,"limit":16,"projection":true}))
                    .is_err()
            );
            assert_eq!(state(&s), before);
        }
        let ids = layer_ids(&s);
        let req = layout_request(&mut s, 71901, json!({"type":"align_layers","target":t,"layerIds":ids,"frame":0,"align":"right","mode":"group"}));
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        let published = state(&s);
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        assert_eq!(state(&s), published);
        s.execute("graphics.set", json!({"clip":t["clip"],"layer":1,"time":0,"props":{"text":"Later human edit"},"merge":false,"begin":false})).unwrap();
        read(&mut s, 71902);
        let human = state(&s);
        assert!(s.execute("agent.undo", json!({"token":token(71902),"commit":token(71901)})).is_err());
        assert_eq!(state(&s), human);
    }

    #[test]
    fn graphics_layout_contract_accepts_full_native_target_bound_without_a_hidden_sixty_four_limit() {
        let s = populated();
        let t = target(&s);
        let ids = (1..=4096).map(|n| format!("{n:032x}")).collect::<Vec<_>>();
        let base = json!({"scope":scope(&s),"beforeLayoutSha256":"a".repeat(64),"edit":{"type":"align_layers","target":t,"layerIds":ids,"frame":0,"align":"left","mode":"frame"}});
        let operation = |params| Operation { command: graphics_layout::COMMAND.into(), params };
        assert!(graphics_layout::validate(&operation(base.clone())).is_ok());
        let mut over = base.clone();
        over["edit"]["layerIds"].as_array_mut().unwrap().push(json!(format!("{:032x}", 4097)));
        assert!(graphics_layout::validate(&operation(over)).is_err());
        let mut clips = base;
        clips["edit"] = json!({"type":"align_clips","targets":(1..=4096).map(|n|json!({"clip":n,"identity":format!("{n:032x}")})).collect::<Vec<_>>(),"frame":0,"align":"left","mode":"group"});
        assert!(graphics_layout::validate(&operation(clips.clone())).is_ok());
        clips["edit"]["targets"][4095] = clips["edit"]["targets"][0].clone();
        assert!(graphics_layout::validate(&operation(clips)).is_err());
    }

    #[test]
    fn graphics_layout_stop_after_shared_preparation_prevents_publication_and_replay() {
        let mut s = populated();
        let t = target(&s);
        let ids = layer_ids(&s);
        let req = layout_request(&mut s, 71910, json!({"type":"align_layers","target":t,"layerIds":ids,"frame":0,"align":"right","mode":"group"}));
        let control = s.agent_operation_control();
        let before = state(&s);
        let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &req, |s, p| {
            let batch = crate::agent_bridge::parse("agent.apply", p)?;
            let stage = crate::agent_bridge::stage_batch(s, &batch)?;
            assert_eq!(control.cancel(&json!({"token":token(71910)}))?["cancellationAccepted"], true);
            crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
        });
        assert!(result.is_err());
        assert_eq!(state(&s), before);
        assert!(s.execute("agent.apply", req).is_err());
        assert_eq!(state(&s), before);
    }

    #[test]
    fn graphics_layout_read_bounds_scope_and_untracked_state_never_authorize_an_edit() {
        let mut s = populated();
        let sid = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
        read(&mut s, 71920);
        let before = state(&s);
        let args = json!({"token":token(71920),"scope":scope(&s),"target":{"kind":"clips"},"offset":0,"limit":1,"projection":true});
        let actual = s.execute("agent.readGraphicsLayout", args.clone()).unwrap();
        assert_eq!(actual["effectTracking"], false);
        assert_eq!(state(&s), before);
        for case in ["offset", "limit", "zero", "scope", "extra", "foreign", "ticket"] {
            let mut bad = args.clone();
            match case {
                "offset" => bad["offset"] = json!(4097),
                "limit" => bad["limit"] = json!(17),
                "zero" => bad["limit"] = json!(0),
                "scope" => bad["scope"]["projectId"] = json!("foreign"),
                "extra" => bad["force"] = json!(true),
                "foreign" => bad["target"] = json!({"kind":"layers","identity":"f".repeat(32)}),
                _ => bad["token"] = json!(token(71921)),
            }
            assert!(s.execute("agent.readGraphicsLayout", bad).is_err(), "{case}");
            assert_eq!(state(&s), before);
        }
    }

    #[test]
    fn graphics_layout_complete_layer_index_observes_all_4096_refs_without_authored_parameters_or_edits() {
        let mut s = populated();
        let t = target(&s);
        let sid = s.state.active_sequence.unwrap();
        let mut template = s.active_sequence().unwrap().video_tracks[0].items[0].effects.iter().find(|e| e.effect == "graphic_shape").unwrap().clone();
        template.params.clear();
        template.masks.clear();
        let effects = (0..4096)
            .map(|n| {
                let mut e = template.clone();
                e.instance_id = Some(format!("{n:032x}"));
                if let Some(layer) = e.layer.as_mut() {
                    layer.uid = n + 1;
                }
                e
            })
            .collect();
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].effects = effects;
        read(&mut s, 71930);
        let before = state(&s);
        let args = json!({"token":token(71930),"scope":scope(&s),"target":{"kind":"layers","identity":t["identity"]},"offset":0,"limit":1,"projection":true,"index":true});
        let index = s.execute("agent.readGraphicsLayout", args.clone()).unwrap();
        assert_eq!(index["index"], true);
        assert_eq!(index["total"], 4096);
        assert_eq!(index["rows"].as_array().unwrap().len(), 4096);
        assert_eq!(index["nextOffset"], Value::Null);
        assert_eq!(index["rows"][4095]["position"], 4095);
        assert_eq!(index["rows"][4095]["layerId"], format!("{:032x}", 4095));
        assert!(index["rows"].as_array().unwrap().iter().all(|r| r.as_object().unwrap().len() == 4));
        let page = layout_page(&mut s, 71930, json!({"kind":"layers","identity":t["identity"]}), 4095, 1);
        assert_eq!(page["collectionSha256"], index["collectionSha256"]);
        assert_eq!(page["rows"][0], index["rows"][4095]);
        let mut bad = args;
        bad["offset"] = json!(1);
        assert!(s.execute("agent.readGraphicsLayout", bad).is_err());
        assert_eq!(state(&s), before);
    }

    #[test]
    fn graphics_layout_complete_clip_index_is_compact_at_full_bound_and_old_paged_contract_is_unchanged() {
        use filmcraft_project::ClipId;
        let mut s = populated();
        let sid = s.state.active_sequence.unwrap();
        let template = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let items = (0..4096)
            .map(|n| {
                let mut c = template.clone();
                c.id = ClipId(20_000 + n);
                c.identity.as_mut().unwrap().id = format!("{n:032x}");
                c.effects.clear();
                c.name = format!("Generated clip {n}");
                c
            })
            .collect();
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items = items;
        let snapshot = read(&mut s, 71931);
        assert_eq!(snapshot["capabilities"]["graphicsLayoutIndex"], true);
        let before = state(&s);
        let index = s
            .execute(
                "agent.readGraphicsLayout",
                json!({"token":token(71931),"scope":scope(&s),"target":{"kind":"clips"},"offset":0,"limit":16,"projection":true,"index":true}),
            )
            .unwrap();
        assert_eq!(index["total"], 4096);
        assert_eq!(index["rows"].as_array().unwrap().len(), 4096);
        assert_eq!(index["nextOffset"], Value::Null);
        assert_eq!(index["rows"][4095]["clip"], 24095);
        assert_eq!(index["rows"][0]["protected"], false);
        assert!(index["rows"].as_array().unwrap().iter().all(|r| r.as_object().unwrap().len() == 8 && !r.as_object().unwrap().contains_key("name")));
        let bytes = project_audit::value_bytes(&index).unwrap().len();
        assert!(bytes > 64 * 1024 && bytes < 2 * 1024 * 1024);
        let page = layout_page(&mut s, 71931, json!({"kind":"clips"}), 0, 16);
        assert!(page.get("index").is_none());
        assert_eq!(page["rows"].as_array().unwrap().len(), 16);
        assert_eq!(page["rows"][0]["name"], "Generated clip 0");
        assert_eq!(page["collectionSha256"], index["collectionSha256"]);
        assert_eq!(state(&s), before);
    }
}

fn fixture() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Graphics","fps":23.976,"width":128,"height":128,"video":2,"audio":1})).unwrap();
    s.prefs.graphics.smart_quotes = false;
    s.prefs.graphics.ligatures = false;
    s.execute("project.associateEditorialOwner", json!({"scope":scope(&s),"workspaceId":"00000000-0000-0000-0000-000000000001"})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s
}
fn populated() -> Session {
    let mut s = fixture();
    let c =
        s.execute("graphics.newText", json!({"text":"Duplicate","position":[8,48],"size":24,"track":0,"durationFrames":48})).unwrap()["clip"].as_u64().unwrap();
    s.execute("graphics.newText", json!({"clip":c,"text":"Duplicate","position":[8,90],"size":24})).unwrap();
    s.execute("graphics.newShape", json!({"clip":c,"shape":"rectangle","position":[100,64],"size":[16,64]})).unwrap();
    s
}
fn scope(s: &Session) -> Value {
    json!({"projectId":"p_graphics","anchorSequence":s.state.active_sequence.unwrap().0})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}
fn page(s: &mut Session, n: usize, target: Value, offset: usize, limit: usize) -> Value {
    let args = json!({"token":token(n),"scope":scope(s),"target":target,"offset":offset,"limit":limit});
    s.execute("agent.readGraphics", args).unwrap()
}
fn request(s: &mut Session, n: usize, edit: Value) -> Value {
    read(s, n);
    let sha = page(s, n, json!({"kind":"clips"}), 0, 1)["collectionSha256"].clone();
    json!({"token":token(n),"label":"Graphic gesture","exact":true,"authority":"document_state","operations":[{
        "command":graphics::COMMAND,"params":{"scope":scope(s),"beforeGraphicsSha256":sha,"edit":edit}}]})
}
fn text_content() -> Value {
    json!({"kind":"text","text":"A \"quoted\" title","position":[8,80],"size":24,"vertical":false,"boxSize":null})
}
fn shape_content() -> Value {
    json!({"kind":"shape","shape":"ellipse","position":[64,64],"size":[64,48],"points":[]})
}
fn target(s: &Session) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    json!({"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id})
}
fn layer_ids(s: &Session) -> Vec<String> {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    filmcraft_project::graphic::layer_indices(&c.effects).iter().map(|i| c.effects[*i].instance_id.clone().unwrap()).collect()
}

#[test]
fn graphics_geometry_matches_human_layout_at_explicit_frames_without_seeking_or_editing() {
    use filmcraft_project::{ClipId, ParamValue};
    use filmcraft_time::Tick;
    let mut s = populated();
    let target = target(&s);
    let clip = ClipId(target["clip"].as_u64().unwrap());
    s.execute(
        "graphics.set",
        json!({"clip":clip.0,"layer":0,"time":0,"props":{"text":"First\nSecond\nThird","box_width":20,"box_height":5},"merge":false,"begin":false}),
    )
    .unwrap();
    let sid = s.state.active_sequence.unwrap();
    let end = s.sequence_rate().tick_of(24);
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let (_, item) = q.find_item_mut(clip).unwrap();
    let graphic_index = filmcraft_project::graphic::layer_indices(&item.effects)[0];
    assert_eq!(item.effects[graphic_index].effect, "graphic_text");
    let position = item.effects[graphic_index].params.get_mut("position").unwrap();
    position.put_keyframe(Tick(0), ParamValue::Vec2(filmcraft_geom::Vec2::new(8.0, 48.0)));
    position.put_keyframe(end, ParamValue::Vec2(filmcraft_geom::Vec2::new(64.0, 80.0)));
    s.set_playhead(s.sequence_rate().tick_of(7));
    let mut positions = Vec::new();
    for (n, frame) in [0, 12, 24, 100].into_iter().enumerate() {
        let mut human = human(&s);
        human.set_playhead(human.sequence_rate().tick_of(frame));
        let expected = human.execute("graphics.list", json!({"clip":clip.0})).unwrap();
        read(&mut s, n + 700);
        let before = state(&s);
        let mut rows = Vec::new();
        for offset in [0, 2] {
            let actual = page(&mut s, n + 700, json!({"kind":"geometry","identity":target["identity"],"frame":frame}), offset, 2);
            assert_eq!(actual["evaluationTarget"], json!({"identity":target["identity"],"frame":frame}));
            rows.extend(actual["graphics"].as_array().unwrap().clone());
        }
        assert_eq!(rows.len(), 3);
        for (i, row) in rows.iter().enumerate() {
            let expected = &expected["layers"][i];
            assert_eq!(row["layerId"], json!(layer_ids(&s)[i]));
            assert_eq!(row["positionPx"], expected["position"]);
            for key in ["kind", "text", "textType", "box", "overflow", "enabled", "anchor", "scale", "scaleWidth", "localBounds", "quad"] {
                assert_eq!(row[key], expected[key], "{key} at frame {frame}");
            }
        }
        assert_eq!(rows[0]["textType"], "paragraph");
        assert_eq!(rows[0]["overflow"], true);
        positions.push(rows[0]["positionPx"].clone());
        assert_eq!(state(&s), before, "evaluation must preserve project, editor, journal and history");
    }
    assert_ne!(positions[0], positions[1]);
    assert_ne!(positions[1], positions[2]);
    assert_eq!(positions[2], positions[3], "the human endpoint clamp holds the last animated value");
}

#[test]
fn graphics_geometry_refuses_hostile_frames_foreign_identity_and_stale_reads_without_mutation() {
    let mut s = populated();
    let target = target(&s);
    read(&mut s, 710);
    let before = state(&s);
    let good = json!({"token":token(710),"scope":scope(&s),"target":{"kind":"geometry","identity":target["identity"],"frame":0},"offset":0,"limit":1});
    for frame in [json!(-1), json!(1_000_001), json!(1.5), json!(null), json!("12")] {
        let mut bad = good.clone();
        bad["target"]["frame"] = frame;
        assert!(s.execute("agent.readGraphics", bad).is_err());
        assert_eq!(state(&s), before);
    }
    let mut foreign = good.clone();
    foreign["target"]["identity"] = json!("f".repeat(32));
    assert!(s.execute("agent.readGraphics", foreign).is_err());
    assert_eq!(state(&s), before);
    s.execute("graphics.set", json!({"clip":target["clip"],"layer":0,"time":0,"props":{"text":"Human change"},"merge":false,"begin":false})).unwrap();
    let after_human = state(&s);
    assert!(s.execute("agent.readGraphics", good).is_err());
    assert_eq!(state(&s), after_human);
}

#[test]
fn graphics_shared_layout_refuses_invalid_clip_endpoints_without_panicking() {
    use filmcraft_project::ClipId;
    use filmcraft_time::Tick;
    for (start, duration) in [(0, 0), (0, -1), (i64::MAX, 2), (i64::MIN, 0)] {
        let mut s = populated();
        let clip = ClipId(target(&s)["clip"].as_u64().unwrap());
        let sid = s.state.active_sequence.unwrap();
        let (_, c) = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(clip).unwrap();
        c.start = Tick(start);
        c.duration = Tick(duration);
        assert!(crate::graphics::list_layers_at(&s, clip, Tick(0)).is_err());
    }
}
fn human(s: &Session) -> Session {
    let mut h = Session::new(s.services.clone());
    h.project = s.project.clone();
    h.state = s.state.clone();
    h.media = s.media.clone();
    h.prefs = s.prefs.clone();
    h
}
fn normalized(before: &Project, after: &Project) -> Project {
    let mut p = after.clone();
    for id in p.sequences().map(|i| i.id).collect::<Vec<_>>() {
        let q = p.sequence_mut(id).unwrap();
        for c in q.all_tracks_mut().flat_map(|t| &mut t.items) {
            let old = before.sequence(id).and_then(|q| q.find_item(c.id)).map(|(_, c)| c);
            if old.is_none() {
                c.identity.as_mut().unwrap().id = format!("{:032x}", c.id.0);
            }
            for (i, e) in c.effects.iter_mut().enumerate() {
                if old.is_none_or(|c| !c.effects.iter().any(|o| o.instance_id == e.instance_id)) {
                    e.instance_id = Some(format!("{:032x}", i + 1));
                }
                for (j, mask) in e.masks.iter_mut().enumerate() {
                    if old.and_then(|c| c.effects.get(i)).is_none_or(|e| !e.masks.iter().any(|m| m.instance_id == mask.instance_id)) {
                        mask.instance_id = Some(format!("{:032x}", j + 1));
                    }
                }
            }
        }
    }
    p
}

#[test]
fn graphics_identity_preparation_matches_the_human_command_and_preserves_authored_output_history() {
    let mut s = fixture();
    s.execute("edit.undo", json!({})).unwrap();
    let clip = s.execute("graphics.newText", json!({"text":"Existing title","position":[8,80],"size":24,"track":0,"durationFrames":48})).unwrap()["clip"]
        .as_u64()
        .unwrap();
    s.execute("masks.add", json!({"clip":clip,"effect":"opacity","shape":"rectangle"})).unwrap();
    let before = s.project.clone();
    let mut h = human(&s);
    let history = s.history.undo.len();
    let req = request(&mut s, 71100, json!({"type":"enable_tracking"}));
    let receipt = s.execute("agent.apply", req).unwrap();
    assert_eq!(receipt["results"][0]["humanCommand"], "poema.trackEffects");
    assert_eq!(receipt["results"][0]["beforeEffectTracking"], false);
    assert_eq!(receipt["results"][0]["afterEffectTracking"], true);
    assert!(receipt["results"][0]["clip"].is_null());
    assert!(receipt["results"][0]["identity"].is_null());
    assert_ne!(receipt["results"][0]["beforeGraphicsSha256"], receipt["results"][0]["afterGraphicsSha256"]);
    h.execute("poema.trackEffects", json!({})).unwrap();
    equal_human(&before, &mut s, &mut h);
    assert_eq!(s.history.undo.len(), history + 1);
    let after = s.project.clone();
    read(&mut s, 71101);
    let inventory = page(&mut s, 71101, json!({"kind":"clips"}), 0, 1);
    assert_eq!(inventory["effectTracking"], true);
    assert_eq!(s.project, after);
    s.execute("agent.undo", json!({"token":token(71101),"commit":token(71100)})).unwrap();
    assert_eq!(s.project, before);
    read(&mut s, 71102);
    s.execute("agent.redo", json!({"token":token(71102),"commit":token(71100),"undo":token(71101)})).unwrap();
    assert_eq!(s.project, after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
}

#[test]
fn graphics_identity_preparation_refuses_noop_stale_foreign_and_unadopted_inputs_without_losing_redo() {
    for case in ["already", "stale", "foreign", "extra", "unadopted"] {
        let mut s = populated();
        let clip = target(&s)["clip"].clone();
        s.execute("graphics.set", json!({"clip":clip,"layer":0,"time":0,"props":{"text":"Temporary human title"},"merge":false,"begin":false})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        assert!(!s.history.redo.is_empty());
        if case != "already" {
            let sid = s.state.active_sequence.unwrap();
            Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
            if case == "unadopted" {
                Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::TRACKING_KEY);
            }
        }
        let mut req = request(&mut s, 71110, json!({"type":"enable_tracking"}));
        if case == "stale" {
            req["operations"][0]["params"]["beforeGraphicsSha256"] = json!("0".repeat(64));
        } else if case == "foreign" {
            req["operations"][0]["params"]["scope"]["projectId"] = json!("foreign");
        } else if case == "extra" {
            req["operations"][0]["params"]["edit"]["force"] = json!(true);
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
fn equal_human(before: &Project, s: &mut Session, h: &mut Session) {
    assert_eq!(project_audit::bytes(&normalized(before, &s.project)).unwrap(), project_audit::bytes(&normalized(before, &h.project)).unwrap());
    assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&h.state).unwrap());
    let a = s.render_program(1.0).unwrap();
    let b = h.render_program(1.0).unwrap();
    assert_eq!(a.px, b.px, "Program picture must equal the independent human gesture");
    assert!(a.px.chunks(4).any(|p| p[0] > 0.1 || p[1] > 0.1 || p[2] > 0.1), "graphics fixture must contain visible picture");
}
#[test]
fn graphics_scoped_pages_are_read_only_have_stable_layers_and_complete_digest() {
    let mut s = populated();
    read(&mut s, 71000);
    let before = state(&s);
    let a = page(&mut s, 71000, json!({"kind":"clips"}), 0, 1);
    assert_eq!(a["total"], 1);
    assert_eq!(a["graphics"][0]["layers"], 3);
    assert_eq!(a["graphics"][0]["canvas"]["width"], 128);
    assert_eq!(a["graphics"][0]["canvas"]["height"], 128);
    assert_eq!(a["graphics"][0]["name"], "Duplicate");
    let identity = a["graphics"][0]["identity"].clone();
    let b = page(&mut s, 71000, json!({"kind":"layers","identity":identity}), 0, 1);
    let c = page(&mut s, 71000, json!({"kind":"layers","identity":identity}), 1, 1);
    assert_eq!(a["collectionSha256"], b["collectionSha256"]);
    assert_eq!(b["collectionSha256"], c["collectionSha256"]);
    assert_ne!(b["graphics"][0]["layerId"], c["graphics"][0]["layerId"]);
    assert!(b["graphics"][0]["properties"].as_array().unwrap().iter().any(|d| d["id"] == "text"));
    assert_eq!(state(&s), before);
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].effects.last_mut().unwrap().enabled = false;
    assert_ne!(graphics::digest(&s).unwrap(), a["collectionSha256"].as_str().unwrap());
    let before_owner = graphics::digest(&s).unwrap();
    let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).source_graphics.insert(source, Default::default());
    assert_ne!(graphics::digest(&s).unwrap(), before_owner, "shared source owner data belongs to the complete collection digest");
}
#[test]
fn graphics_creation_matches_human_picture_preferences_identity_history_and_reopen() {
    for content in [text_content(), shape_content()] {
        let mut s = fixture();
        let mut h = human(&s);
        let before = s.project.clone();
        let track = s.active_sequence().unwrap().video_tracks[0].id.0;
        let req = request(&mut s, 71001, json!({"type":"create","track":track,"frame":0,"durationFrames":37,"content":content}));
        let undo = s.history.undo.len();
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        assert_eq!(receipt["action"], "graphics_edit");
        assert_eq!(receipt["authority"], "document_state");
        let (command, mut args) = if content["kind"] == "text" {
            ("graphics.newText", json!({"text":content["text"],"position":[8,80],"size":24,"vertical":false}))
        } else {
            ("graphics.newShape", json!({"shape":"ellipse","position":[64,64],"size":[64,48],"points":[]}))
        };
        args["track"] = json!(0);
        args["time"] = json!(0);
        args["durationFrames"] = json!(37);
        h.execute(command, args).unwrap();
        equal_human(&before, &mut s, &mut h);
        assert_eq!(s.history.undo.len(), undo + 1);
        let after = s.project.clone();
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        let encoded = filmcraft_format::encode(&s.project, false);
        assert_eq!(filmcraft_format::decode(&encoded).unwrap().project, *after);
        read(&mut s, 71002);
        s.execute("agent.undo", json!({"token":token(71002),"commit":token(71001)})).unwrap();
        assert_eq!(*s.project, *before);
        read(&mut s, 71003);
        s.execute("agent.redo", json!({"token":token(71003),"commit":token(71001),"undo":token(71002)})).unwrap();
        assert_eq!(*s.project, *after);
    }
}
#[test]
fn graphics_layer_gestures_match_human_services_without_touching_other_authored_content() {
    for mode in ["add", "set", "delete", "reorder"] {
        let mut s = populated();
        let mut h = human(&s);
        let before = s.project.clone();
        let target = target(&s);
        let ids = layer_ids(&s);
        let (edit, command, args) = match mode {
            "add" => (
                json!({"type":"add_layer","target":target,"content":shape_content()}),
                "graphics.newShape",
                json!({"clip":target["clip"],"shape":"ellipse","position":[64,64],"size":[64,48],"points":[]}),
            ),
            "set" => (
                json!({"type":"set","target":target,"layerId":ids[1],"frame":0,"props":{"text":"Second only","fill_color":"#00ff00"}}),
                "graphics.set",
                json!({"clip":target["clip"],"layer":1,"time":0,"props":{"text":"Second only","fill_color":"#00ff00"},"merge":false,"begin":false}),
            ),
            "delete" => (json!({"type":"delete_layer","target":target,"layerId":ids[1]}), "graphics.deleteLayer", json!({"clip":target["clip"],"layer":1})),
            _ => (
                json!({"type":"reorder_layer","target":target,"layerId":ids[0],"beforeLayerId":null}),
                "graphics.arrangeLayer",
                json!({"clip":target["clip"],"layer":0,"to":2}),
            ),
        };
        let req = request(&mut s, 71010, edit);
        s.execute("agent.apply", req).unwrap();
        h.execute(command, args).unwrap();
        equal_human(&before, &mut s, &mut h);
        read(&mut s, 71011);
        s.execute("agent.undo", json!({"token":token(71011),"commit":token(71010)})).unwrap();
        assert_eq!(*s.project, *before);
    }
}
#[test]
fn graphics_hostile_targets_properties_scope_and_clocks_preserve_redo_and_real_state() {
    for case in [
        "digest",
        "target",
        "layer",
        "locked",
        "protected",
        "scope",
        "frame",
        "bool",
        "resource",
        "unknown",
        "clamp",
        "no_op",
        "mixed",
        "exact",
        "reorder_missing",
        "source_clock",
        "shared_source",
    ] {
        let mut s = populated();
        let target = target(&s);
        let ids = layer_ids(&s);
        s.execute("graphics.newText", json!({"text":"Redo","frame":200,"track":1})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        if case == "locked" {
            Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].locked = true;
        }
        if case == "protected" {
            Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0]
                .identity
                .as_mut()
                .unwrap()
                .protection = Some("Human protected".into());
        }
        if case == "source_clock" {
            Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0].speed = 1e100;
        }
        if case == "shared_source" {
            let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
            Arc::make_mut(&mut s.project).source_graphics.insert(source, Default::default());
        }
        let mut req = request(&mut s, 71020, json!({"type":"set","target":target,"layerId":ids[0],"frame":0,"props":{"text":"After"}}));
        let p = &mut req["operations"][0]["params"];
        match case {
            "digest" => p["beforeGraphicsSha256"] = json!("0".repeat(64)),
            "target" => p["edit"]["target"]["clip"] = json!(999999),
            "layer" => p["edit"]["layerId"] = json!("1".repeat(32)),
            "scope" => p["scope"]["projectId"] = json!("another"),
            "frame" => p["edit"]["frame"] = json!(1000),
            "source_clock" => p["edit"]["frame"] = json!(1),
            "bool" => p["edit"]["props"] = json!({"enabled":"false"}),
            "resource" => p["edit"]["props"] = json!({"font":"/private/font.ttf"}),
            "unknown" => p["edit"]["props"] = json!({"fontSize":200}),
            "clamp" => p["edit"]["props"] = json!({"opacity":1000}),
            "no_op" => p["edit"]["props"] = json!({"text":"Duplicate"}),
            "mixed" => {
                let op = req["operations"][0].clone();
                req["operations"].as_array_mut().unwrap().push(op);
            }
            "exact" => req["exact"] = json!(false),
            "reorder_missing" => p["edit"] = json!({"type":"reorder_layer","target":target,"layerId":ids[0]}),
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case} changed project/editor/history/Redo");
    }
}
#[test]
fn graphics_creation_has_no_implicit_destination_or_overlap_fallback() {
    for case in ["missing", "audio", "locked", "overlap", "fraction", "negative", "overflow"] {
        let mut s = populated();
        let track = s.active_sequence().unwrap().video_tracks[0].id.0;
        let mut edit = json!({"type":"create","track":track,"frame":0,"durationFrames":24,"content":text_content()});
        match case {
            "missing" => edit["track"] = json!(9999),
            "audio" => edit["track"] = json!(s.active_sequence().unwrap().audio_tracks[0].id.0),
            "locked" => Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].locked = true,
            "fraction" => edit["durationFrames"] = json!(0.5),
            "negative" => edit["frame"] = json!(-1),
            "overflow" => edit["durationFrames"] = json!(i64::MAX),
            _ => {}
        }
        let req = request(&mut s, 71030, edit);
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
#[test]
fn graphics_metadata_reads_do_not_enable_tracking_and_refuse_foreign_or_stale_context() {
    let mut s = fixture();
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
    read(&mut s, 71040);
    assert_eq!(page(&mut s, 71040, json!({"kind":"clips"}), 0, 1)["effectTracking"], false);
    let track = s.active_sequence().unwrap().video_tracks[0].id.0;
    let req = request(&mut s, 71041, json!({"type":"create","track":track,"frame":0,"durationFrames":24,"content":text_content()}));
    let before = state(&s);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(&s), before);
    for (target, offset, limit) in [(json!({"kind":"clips"}), 1, 1), (json!({"kind":"clips"}), 0, 0), (json!({"kind":"layers","identity":"bad"}), 0, 1)] {
        assert!(s.execute("agent.readGraphics", json!({"token":token(71041),"scope":scope(&s),"target":target,"offset":offset,"limit":limit})).is_err());
    }
    s.state.linked_selection = !s.state.linked_selection;
    let before = state(&s);
    assert!(s.execute("agent.readGraphics", json!({"token":token(71041),"scope":scope(&s),"target":{"kind":"clips"},"offset":0,"limit":1})).is_ok());
    assert_eq!(state(&s), before, "explicit documentary graphics reads ignore unrelated selection mode");
    s.project = Arc::new((*s.project).clone());
    assert!(s.execute("agent.readGraphics", json!({"token":token(71041),"scope":scope(&s),"target":{"kind":"clips"},"offset":0,"limit":1})).is_err());
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn graphics_stop_and_later_human_edits_cannot_become_owned_undo() {
    let mut s = fixture();
    let control = s.agent_operation_control();
    control.enable_transport();
    let track = s.active_sequence().unwrap().video_tracks[0].id.0;
    let edit = json!({"type":"create","track":track,"frame":0,"durationFrames":24,"content":text_content()});
    let req = request(&mut s, 71050, edit.clone());
    let before = state(&s);
    assert_eq!(control.cancel(&json!({"token":token(71050)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(&s), before);
    let req = request(&mut s, 71051, edit);
    s.execute("agent.apply", req).unwrap();
    let c = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("graphics.setText", json!({"clip":c.0,"layer":0,"text":"Later human text"})).unwrap();
    read(&mut s, 71052);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(71052),"commit":token(71051)})).is_err());
    assert_eq!(state(&s), before);
}

#[path = "graphics_font_canonical_tests.rs"]
mod font_canonical;
