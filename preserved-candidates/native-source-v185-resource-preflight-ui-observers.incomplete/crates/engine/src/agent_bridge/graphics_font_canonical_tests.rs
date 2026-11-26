use super::*;
use sha2::{Digest, Sha256};
#[test]
fn canonical_font_plans_bind_exact_native_bytes_in_both_instance_and_shared_scope_without_changing_legacy_replies() {
    for shared in [false, true] {
        let mut s = populated();
        if shared {
            let clip = target(&s)["clip"].clone();
            s.execute("graphics.upgradeToSourceGraphic", json!({"clip":clip})).unwrap();
        }
        let t = target(&s);
        let layer = layer_ids(&s)[0].clone();
        let mut req = request(&mut s, 86000, json!({"type":"set","target":t,"layerId":layer,"frame":5,"props":{"font":"Noto Serif"}}));
        if shared {
            let p = page(&mut s, 86000, json!({"kind":"shared_source","identity":t["identity"]}), 0, 16);
            req["operations"][0]["params"]["sharedSource"] =
                json!({"item":p["sharedSourceState"]["item"],"beforeSha256":p["sharedSourceState"]["beforeSha256"]});
        }
        let query = json!({"token":req["token"],"request":req["operations"][0]["params"]});
        let old = s.execute(graphics::PLAN_FONTS, query.clone()).unwrap();
        assert!(old.get("beforeJson").is_none() && old.get("afterJson").is_none());
        let mut query = query;
        query["canonical"] = json!(true);
        let before = state(&s);
        let planned = s.execute(graphics::PLAN_FONTS, query).unwrap();
        assert_eq!(state(&s), before);
        assert_eq!(planned["fontPlan"], old["fontPlan"]);
        for (field, json_field, hash_field) in [("before", "beforeJson", "beforeSha256"), ("after", "afterJson", "afterSha256")] {
            let bytes = planned[json_field].as_str().unwrap();
            assert_eq!(serde_json::from_str::<Value>(bytes).unwrap(), planned[field]);
            assert_eq!(format!("{:x}", Sha256::digest(bytes.as_bytes())), planned["fontPlan"][hash_field]);
            assert_eq!(planned[field].get("sharedSource").is_some(), shared);
        }
        assert_eq!(planned["writeAuthority"], false);
        req["operations"][0]["params"]["fontPlan"] = planned["fontPlan"].clone();
        let receipt = s.execute("agent.apply", req).unwrap();
        assert_eq!(receipt["results"][0]["fontResources"], planned["fontPlan"]);
    }
}
#[test]
fn canonical_font_plan_query_refuses_malformed_modes_and_copied_proofs_without_changing_project_or_history() {
    let mut s = populated();
    let t = target(&s);
    let layer = layer_ids(&s)[0].clone();
    let req = request(&mut s, 86010, json!({"type":"set","target":t,"layerId":layer,"frame":5,"props":{"font":"Noto Serif"}}));
    let before = state(&s);
    for canonical in [json!("true"), Value::Null, json!(1), json!({})] {
        assert!(s.execute(graphics::PLAN_FONTS, json!({"token":req["token"],"request":req["operations"][0]["params"],"canonical":canonical})).is_err());
        assert_eq!(state(&s), before);
    }
    let mut params = req["operations"][0]["params"].clone();
    params["fontPlan"] = Value::Null;
    assert!(s.execute(graphics::PLAN_FONTS, json!({"token":req["token"],"request":params,"canonical":true})).is_err());
    assert_eq!(state(&s), before);
}
