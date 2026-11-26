use super::*;
use filmcraft_project::{ItemId, graphic};
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use std::sync::atomic::{AtomicU64, Ordering};
fn shared() -> (Session, ItemId, ItemId) {
    let mut s = populated();
    let anchor = s.state.active_sequence.unwrap();
    let owner = scope(&s);
    let clip = target(&s)["clip"].clone();
    let source = s.execute("graphics.upgradeToSourceGraphic", json!({"clip":clip})).unwrap()["item"].as_u64().unwrap();
    let at = s.sequence_rate().tick_of(60);
    let duration = s.sequence_rate().tick_of(48);
    s.execute("timeline.place", json!({"item":source,"track":"V1","time":at.0,"sourceIn":0,"duration":duration.0})).unwrap();
    s.execute("file.newSequence", json!({"name":"Other owned sequence","fps":60,"width":320,"height":180,"video":1,"audio":1})).unwrap();
    let other = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(other).unwrap().metadata.insert(crate::agent_bridge::sequences::KEY.into(), serde_json::to_string(&owner).unwrap());
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s.execute("timeline.place", json!({"item":source,"track":"V1","time":0,"sourceIn":0,"duration":duration.0})).unwrap();
    s.execute("sequence.open", json!({"item":anchor.0})).unwrap();
    (s, ItemId(source), other)
}
fn source_page(s: &mut Session, n: usize) -> Value {
    let t = target(s);
    page(s, n, json!({"kind":"shared_source","identity":t["identity"]}), 0, 16)
}
fn proposed(s: &mut Session, n: usize, font: &str) -> Value {
    let t = target(s);
    let id = layer_ids(s)[0].clone();
    let mut r = request(s, n, json!({"type":"set","target":t,"layerId":id,"frame":5,"props":{"font":font}}));
    let header = source_page(s, n)["sharedSourceState"].clone();
    r["operations"][0]["params"]["sharedSource"] = json!({"item":header["item"],"beforeSha256":header["beforeSha256"]});
    r
}
fn plan(s: &mut Session, r: &mut Value) -> Value {
    let before = state(s);
    let p = s.execute(graphics::PLAN_FONTS, json!({"token":r["token"],"request":r["operations"][0]["params"]})).unwrap();
    assert_eq!(state(s), before);
    r["operations"][0]["params"]["fontPlan"] = p["fontPlan"].clone();
    p
}
#[test]
fn shared_font_inventory_and_exact_set_match_human_all_sequences_master_owned_history_and_replay() {
    let (mut s, source, other) = shared();
    let target = target(&s);
    let mut h = human(&s);
    let before = s.project.clone();
    let view = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let mut r = proposed(&mut s, 84000, "Noto Serif");
    let header = source_page(&mut s, 84000);
    assert_eq!(header["total"], 3);
    assert_eq!(header["sharedSourceState"]["allInstancesEditable"], true);
    assert!(header["graphics"].as_array().unwrap().iter().any(|x| x["sequence"] == other.0));
    let p = plan(&mut s, &mut r);
    assert_eq!(p["before"]["sharedSource"]["memberCount"], 3);
    assert_eq!(p["after"]["sharedSource"]["memberCount"], 3);
    assert_eq!(p["after"]["sharedSource"]["rendererSampling"], "requested_instance_frame_only");
    let receipt = s.execute("agent.apply", r.clone()).unwrap();
    let at = h.sequence_rate().tick_of(5);
    h.execute("graphics.set", json!({"clip":target["clip"],"layer":0,"time":at.0,"props":{"font":"Noto Serif"},"merge":false,"begin":false})).unwrap();
    assert_eq!(*s.project, *h.project);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.execute("agent.apply", r).unwrap(), receipt);
    assert_eq!(s.history.undo.len(), undo + 1);
    for q in s.project.sequences() {
        for c in &q.as_sequence().unwrap().video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.item == source).collect::<Vec<_>>() {
            assert_eq!(
                c.effects.iter().find(|e| e.effect == graphic::TEXT_LAYER).unwrap().params["font"].value,
                filmcraft_project::ParamValue::Text("Noto Serif".into())
            );
        }
    }
    assert_eq!(s.project.source_graphics[&source].layers[0].params["font"].value, filmcraft_project::ParamValue::Text("Noto Serif".into()));
    let after = s.project.clone();
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
    read(&mut s, 84001);
    s.execute("agent.undo", json!({"token":token(84001),"commit":token(84000)})).unwrap();
    assert_eq!(*s.project, *before);
    read(&mut s, 84002);
    s.execute("agent.redo", json!({"token":token(84002),"commit":token(84000),"undo":token(84001)})).unwrap();
    assert_eq!(*s.project, *after);
}
#[test]
fn shared_font_scope_refuses_locked_protected_untracked_drifted_foreign_or_tampered_members_without_writes() {
    for (i, case) in ["locked", "protected", "untracked", "drift", "digest", "item", "missing_scope", "nonfont"].into_iter().enumerate() {
        let (mut s, _, other) = shared();
        if case == "locked" {
            Arc::make_mut(&mut s.project).sequence_mut(other).unwrap().video_tracks[0].locked = true;
        }
        if case == "protected" {
            Arc::make_mut(&mut s.project).sequence_mut(other).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("human".into());
        }
        if case == "untracked" {
            Arc::make_mut(&mut s.project).item_mut(other).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
        }
        if case == "drift" {
            Arc::make_mut(&mut s.project).sequence_mut(other).unwrap().video_tracks[0].items[0]
                .effects
                .iter_mut()
                .find(|e| e.effect == graphic::TEXT_LAYER)
                .unwrap()
                .enabled = false;
        }
        let n = 84100 + i;
        let mut r = proposed(&mut s, n, "Noto Serif");
        match case {
            "digest" => r["operations"][0]["params"]["sharedSource"]["beforeSha256"] = json!("0".repeat(64)),
            "item" => r["operations"][0]["params"]["sharedSource"]["item"] = json!(999999),
            "missing_scope" => {
                r["operations"][0]["params"].as_object_mut().unwrap().remove("sharedSource");
            }
            "nonfont" => r["operations"][0]["params"]["edit"]["props"] = json!({"size":36}),
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute(graphics::PLAN_FONTS, json!({"token":r["token"],"request":r["operations"][0]["params"]})).is_err(), "{case}");
        assert_eq!(state(&s), before);
    }
    let (mut s, _, other) = shared();
    Arc::make_mut(&mut s.project).item_mut(other).unwrap().metadata.remove(crate::agent_bridge::sequences::KEY);
    read(&mut s, 84120);
    let t = target(&s);
    let before = state(&s);
    assert!(
        s.execute(
            "agent.readGraphics",
            json!({"token":token(84120),"scope":scope(&s),"target":{"kind":"shared_source","identity":t["identity"]},"offset":0,"limit":16})
        )
        .is_err()
    );
    assert_eq!(state(&s), before);
}
#[test]
fn shared_font_scope_preserves_later_human_member_changes_between_staging_and_publication() {
    let (mut s, source, _) = shared();
    let mut r = proposed(&mut s, 84200, "Noto Serif");
    plan(&mut s, &mut r);
    let before = s.project.clone();
    let later = std::cell::RefCell::new(None);
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        let batch = crate::agent_bridge::parse("agent.apply", p)?;
        let stage = crate::agent_bridge::stage_batch(s, &batch)?;
        let at = s.sequence_rate().tick_of(120);
        let duration = s.sequence_rate().tick_of(48);
        s.execute("timeline.place", json!({"item":source.0,"track":"V1","time":at.0,"sourceIn":0,"duration":duration.0}))?;
        *later.borrow_mut() = Some(state(s));
        crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), later.into_inner().unwrap());
    assert_ne!(*s.project, *before);
    assert!(
        s.project.source_graphics[&source]
            .layers
            .iter()
            .filter(|e| e.effect == graphic::TEXT_LAYER)
            .all(|e| e.params["font"].value != filmcraft_project::ParamValue::Text("Noto Serif".into()))
    );
}
struct NewFace {
    family: String,
}
impl FontSource for NewFace {
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
    format!("Poema Shared Font {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}
#[test]
fn shared_font_relevant_resolution_change_after_staging_cannot_publish_any_instance() {
    let (mut s, _, _) = shared();
    let family = unique();
    let mut r = proposed(&mut s, 84300, &family);
    plan(&mut s, &mut r);
    let before = state(&s);
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        let batch = crate::agent_bridge::parse("agent.apply", p)?;
        let stage = crate::agent_bridge::stage_batch(s, &batch)?;
        assert_eq!(fonts::add_source(&NewFace { family: family.clone() }), 1);
        crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
    });
    let e = result.unwrap_err().to_string();
    assert!(e.contains("native font or glyph resolution changed"), "{e}");
    assert_eq!(state(&s), before);
}
#[test]
fn shared_font_stop_after_human_preparation_discards_the_entire_source_and_instance_candidate() {
    let (mut s, _, _) = shared();
    let mut r = proposed(&mut s, 84400, "Noto Serif");
    plan(&mut s, &mut r);
    let before = state(&s);
    let control = s.agent_operation_control();
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &r, |s, p| {
        let batch = crate::agent_bridge::parse("agent.apply", p)?;
        let stage = crate::agent_bridge::stage_batch(s, &batch)?;
        assert_eq!(control.cancel(&json!({"token":token(84400)}))?["cancellationAccepted"], true);
        crate::agent_bridge::publish_batch(s, &batch, stage, p, None)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", r).is_err());
    assert_eq!(state(&s), before);
}
