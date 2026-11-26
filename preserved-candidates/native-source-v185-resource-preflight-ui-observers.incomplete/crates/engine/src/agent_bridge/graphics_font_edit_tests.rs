use super::*;
use filmcraft_project::{ParamValue, graphic};
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
fn range(s: &mut Session, n: usize, style: Value, clear: bool) -> Value {
    read(s, n);
    let t = target(s);
    let id = layer_ids(s)[0].clone();
    let p = page(s, n, json!({"kind":"character_styles","identity":t["identity"],"layer_id":id,"frame":5}), 0, 1);
    let text = page(s, n, json!({"kind":"character_text","identity":t["identity"],"layer_id":id,"frame":5}), 0, 1);
    let selected: String = text["graphics"][0]["text"].as_str().unwrap().chars().skip(1).take(3).collect();
    json!({"token":token(n),"label":"Exact font range","exact":true,"authority":"document_state","operations":[{"command":graphics::COMMAND,"params":{"scope":scope(s),"beforeGraphicsSha256":p["collectionSha256"],"edit":{"type":"style_range","target":t,"layerId":id,"frame":5,"start":1,"end":4,"textSha256":p["characterState"]["textSha256"],"runsSha256":p["characterState"]["runsSha256"],"rangeTextSha256":format!("{:x}",Sha256::digest(selected.as_bytes())),"clear":clear,"style":style}}}]})
}
fn planned(s: &mut Session, req: &mut Value) -> Value {
    let before = state(s);
    let p = s.execute(graphics::PLAN_FONTS, json!({"token":req["token"],"request":req["operations"][0]["params"]})).unwrap();
    assert_eq!(state(s), before);
    assert_eq!(p["writeAuthority"], false);
    assert_eq!(p["sourceValidation"], "not-performed");
    assert_eq!(p["embeddingRights"], "not-established");
    req["operations"][0]["params"]["fontPlan"] = p["fontPlan"].clone();
    p
}
fn set(s: &mut Session, n: usize, font: &str) -> Value {
    let t = target(s);
    let id = layer_ids(s)[0].clone();
    request(s, n, json!({"type":"set","target":t,"layerId":id,"frame":5,"props":{"font":font}}))
}
#[test]
fn guarded_font_all_five_aliases_match_human_keep_unicode_outside_styles_view_owned_history_and_replay() {
    for (i, (key, value)) in
        [("font", "Noto Serif"), ("family", "Noto Serif"), ("fontStyle", "Bold Italic"), ("font_style", "Bold Italic"), ("style", "Bold Italic")]
            .into_iter()
            .enumerate()
    {
        let mut s = populated();
        let t = target(&s);
        let clip = t["clip"].clone();
        s.execute("graphics.set", json!({"clip":clip,"layer":0,"props":{"text":"A😀e\u{301} more","font":"Inter","font_style":"Regular"},"time":0})).unwrap();
        s.execute("graphics.setCharStyle", json!({"clip":clip,"layer":0,"start":0,"end":8,"style":{"tracking":2.5}})).unwrap();
        let mut human = human(&s);
        let before = s.project.clone();
        let view = serde_json::to_value(&s.state).unwrap();
        let undo = s.history.undo.len();
        let n = 83000 + i * 10;
        let mut req = range(&mut s, n, json!({key:value}), false);
        let plan = planned(&mut s, &mut req);
        assert_ne!(plan["before"]["layoutSha256"], plan["after"]["layoutSha256"]);
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        let at = human.sequence_rate().tick_of(5);
        human
            .execute(
                "graphics.setCharStyle",
                json!({"clip":clip,"layer":0,"time":at.0,"start":1,"end":4,"style":{key:value},"clear":false,"merge":false,"begin":false}),
            )
            .unwrap();
        assert_eq!(*s.project, *human.project, "{key}");
        assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(receipt["results"][0]["fontResources"], plan["fontPlan"]);
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        assert_eq!(s.history.undo.len(), undo + 1);
        let after = s.project.clone();
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
        read(&mut s, n + 1);
        s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
        assert_eq!(*s.project, *before);
        read(&mut s, n + 2);
        s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
        assert_eq!(*s.project, *after);
    }
}
#[test]
fn guarded_font_layer_set_preserves_native_keyframes_and_discloses_missing_family_fallback() {
    let mut s = populated();
    let sid = s.state.active_sequence.unwrap();
    let at = s.sequence_rate().tick_of(5);
    let t = target(&s);
    let id = layer_ids(&s)[0].clone();
    let effect =
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
    let p = effect.params.get_mut("font").unwrap();
    p.put_keyframe(filmcraft_time::Tick::ZERO, ParamValue::Text("Inter".into()));
    p.put_keyframe(at, ParamValue::Text("JetBrains Mono".into()));
    let mut h = human(&s);
    let mut req = request(&mut s, 83100, json!({"type":"set","target":t,"layerId":id,"frame":5,"props":{"font":"Noto Serif","font_style":"Regular"}}));
    planned(&mut s, &mut req);
    s.execute("agent.apply", req).unwrap();
    h.execute("graphics.set", json!({"clip":t["clip"],"layer":0,"time":at.0,"props":{"font":"Noto Serif","font_style":"Regular"},"merge":false,"begin":false}))
        .unwrap();
    assert_eq!(*s.project, *h.project);
    let e = s.active_sequence().unwrap().video_tracks[0].items[0].effects.iter().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
    assert_eq!(e.params["font"].value_at(filmcraft_time::Tick::ZERO), ParamValue::Text("Inter".into()));
    let missing = unique();
    let mut req = set(&mut s, 83101, &missing);
    let p = planned(&mut s, &mut req);
    assert_eq!(p["after"]["missingFont"], true);
    s.execute("agent.apply", req).unwrap();
}
#[test]
fn guarded_font_plans_reject_tampered_body_hashes_tokens_unknown_fields_alias_collisions_and_non_strings_without_writes() {
    for (i, case) in ["before", "after", "proposal", "token", "extra", "body", "copy_to_fresh_read"].into_iter().enumerate() {
        let mut s = populated();
        let n = 83200 + i * 3;
        let mut req = set(&mut s, n, "Noto Serif");
        planned(&mut s, &mut req);
        match case {
            "before" => req["operations"][0]["params"]["fontPlan"]["beforeSha256"] = json!("0".repeat(64)),
            "after" => req["operations"][0]["params"]["fontPlan"]["afterSha256"] = json!("0".repeat(64)),
            "proposal" => req["operations"][0]["params"]["fontPlan"]["proposalSha256"] = json!("0".repeat(64)),
            "token" => req["operations"][0]["params"]["fontPlan"]["readToken"] = json!(token(n + 999)),
            "extra" => req["operations"][0]["params"]["fontPlan"]["grant"] = json!(true),
            "body" => req["operations"][0]["params"]["edit"]["props"]["font"] = json!("JetBrains Mono"),
            _ => {
                read(&mut s, n + 1);
                req["token"] = json!(token(n + 1));
                req["operations"][0]["params"]["fontPlan"]["readToken"] = json!(token(n + 1));
            }
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "{case}");
        assert_eq!(state(&s), before);
    }
    for (i, style) in [
        json!({"font":true}),
        json!({"fontStyle":null}),
        json!({"font":"Inter","family":"Noto Serif"}),
        json!({"style":"Regular","font_style":"Bold"}),
        json!({"font":"a\u{0000}b"}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = populated();
        let n = 83300 + i;
        let req = range(&mut s, n, style, false);
        let before = state(&s);
        assert!(s.execute(graphics::PLAN_FONTS, json!({"token":req["token"],"request":req["operations"][0]["params"]})).is_err());
        assert_eq!(state(&s), before);
    }
}
struct RegisteredFace {
    family: String,
}
impl FontSource for RegisteredFace {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::Static(fonts::NOTO_SERIF_REGULAR),
            origin: "generated-test",
        }]
    }
}
fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("Poema Font Authoring {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}
#[test]
fn guarded_font_live_publication_rejects_newly_resolved_family_after_shared_staging_without_a_live_edit() {
    let mut s = populated();
    let family = unique();
    let mut req = set(&mut s, 83400, &family);
    let p = planned(&mut s, &mut req);
    assert_eq!(p["after"]["missingFont"], true);
    let before = state(&s);
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &req, |s, p| {
        let batch = crate::agent_bridge::parse("agent.apply", p)?;
        let stage = crate::agent_bridge::stage_batch(s, &batch)?;
        assert_eq!(fonts::add_source(&RegisteredFace { family: family.clone() }), 1);
        crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
    });
    let error = result.unwrap_err().to_string();
    assert!(error.contains("native font or glyph resolution changed"), "{error}");
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn guarded_font_publication_allows_unrelated_registry_growth_and_keeps_original_stop_and_no_replay_fence() {
    for (i, stop) in [false, true].into_iter().enumerate() {
        let mut s = populated();
        let n = 83500 + i;
        let mut req = set(&mut s, n, "Noto Serif");
        planned(&mut s, &mut req);
        let before = state(&s);
        let control = s.agent_operation_control();
        let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &req, |s, p| {
            let batch = crate::agent_bridge::parse("agent.apply", p)?;
            let stage = crate::agent_bridge::stage_batch(s, &batch)?;
            assert_eq!(fonts::add_source(&RegisteredFace { family: unique() }), 1);
            if stop {
                assert_eq!(control.cancel(&json!({"token":token(n)}))?["cancellationAccepted"], true);
            }
            crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
        });
        if stop {
            assert!(result.is_err());
            assert_eq!(state(&s), before);
            assert!(s.execute("agent.apply", req).is_err());
            assert_eq!(state(&s), before);
        } else {
            assert_eq!(result.unwrap()["status"], "committed");
            assert_ne!(state(&s), before);
        }
    }
}
#[test]
fn guarded_font_clear_uses_original_complete_override_removal_with_native_resource_readback() {
    let mut s = populated();
    let t = target(&s);
    s.execute(
        "graphics.setCharStyle",
        json!({"clip":t["clip"],"layer":0,"start":0,"end":8,"style":{"font":"Noto Serif","fontStyle":"Regular","underline":true}}),
    )
    .unwrap();
    let mut h = human(&s);
    let mut req = range(&mut s, 83600, json!({}), true);
    let p = planned(&mut s, &mut req);
    // The font remains used outside the cleared range. The complete face set
    // stays intact while requests and rendered layout change inside the range.
    assert_eq!(p["before"]["resourcesSha256"], p["after"]["resourcesSha256"]);
    assert_ne!(p["before"]["requestsSha256"], p["after"]["requestsSha256"]);
    assert_ne!(p["before"]["layoutSha256"], p["after"]["layoutSha256"]);
    s.execute("agent.apply", req).unwrap();
    let at = h.sequence_rate().tick_of(5);
    h.execute("graphics.setCharStyle", json!({"clip":t["clip"],"layer":0,"time":at.0,"start":1,"end":4,"clear":true,"style":{},"merge":false,"begin":false}))
        .unwrap();
    assert_eq!(*s.project, *h.project);
}
