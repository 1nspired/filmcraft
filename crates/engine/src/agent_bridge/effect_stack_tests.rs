use super::{
    effect_stack, project_audit,
    tests::{read, source_clock_fixture, token},
};
use crate::Session;
use filmcraft_project::ParamValue;
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

fn fixture() -> Session {
    let mut s = source_clock_fixture();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    for effect in ["gaussian_blur", "lumetri", "gaussian_blur"] {
        s.execute("effects.apply", json!({"clips":[clip.0],"effect":effect})).unwrap();
    }
    s.execute("masks.add", json!({"clip":clip.0,"effect":0,"shape":"rectangle"})).unwrap();
    s.execute("effects.setParam", json!({"clip":clip.0,"effect":0,"param":"blurriness","value":17,"time":0,"keyframe":true})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}
fn request(s: &Session, n: usize, edit: Value) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    json!({"token":token(n),"label":"Exact stack gesture","exact":true,"operations":[{"command":effect_stack::COMMAND,"params":{
        "clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"beforeStackSha256":effect_stack::digest(&c.effects).unwrap(),
        "scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0},"edit":edit}}]})
}
#[test]
fn stack_catalog_and_complete_digest_pages_are_read_only_and_never_allocate_identities() {
    let mut s = fixture();
    let doc = read(&mut s, 70000);
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let identity = c.identity.as_ref().unwrap().id.clone();
    let ids = c.effects.iter().map(|e| e.instance_id.clone()).collect::<Vec<_>>();
    let before = state(&s);
    let scope = json!({"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0});
    let page = s.execute("agent.readEffectStack", json!({"token":doc["token"],"scope":scope,"identity":identity,"offset":0,"limit":1})).unwrap();
    assert_eq!(page["effects"][0]["instanceId"], json!(ids[0]));
    assert_eq!(page["total"], 3);
    assert_eq!(page["nextOffset"], 1);
    let last = s.execute("agent.readEffectStack", json!({"token":doc["token"],"scope":scope,"identity":identity,"offset":2,"limit":1})).unwrap();
    assert_eq!(last["sha256"], page["sha256"]);
    assert_eq!(last["nextOffset"], Value::Null);
    let mut offset = 0;
    let mut catalog = Vec::new();
    loop {
        let r = s.execute("agent.readEffectCatalog", json!({"token":doc["token"],"scope":scope,"offset":offset,"limit":32})).unwrap();
        catalog.extend(r["effects"].as_array().unwrap().clone());
        match r["nextOffset"].as_u64() {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert!(catalog.iter().any(|e| e["definition"] == "lumetri" && e["addable"] == true));
    assert!(catalog.iter().any(|e| e["intrinsic"] == true && e["addable"] == false));
    assert_eq!(catalog.len(), filmcraft_project::effect_defs().len());
    assert_eq!(state(&s), before);
}
#[test]
fn stack_remove_enable_and_reorder_match_human_services_preserve_masks_animation_history_and_reopen() {
    for mode in ["remove", "set_enabled", "reorder"] {
        let mut s = fixture();
        let n = 70100;
        let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
        let clip = c.id.0;
        let first = c.effects[0].instance_id.clone().unwrap();
        let second = c.effects[2].instance_id.clone().unwrap();
        let before = s.project.clone();
        let (edit, command, args) = match mode {
            "remove" => (json!({"type":mode,"effectId":second}), "effects.remove", json!({"clip":clip,"index":2})),
            "set_enabled" => (json!({"type":mode,"effectId":second,"enabled":false}), "effects.toggleEnabled", json!({"clip":clip,"index":2})),
            _ => (json!({"type":mode,"effectId":second,"beforeEffectId":first}), "effects.reorder", json!({"clip":clip,"from":2,"to":0})),
        };
        let mut human = Session::new(s.services.clone());
        human.project = s.project.clone();
        human.state = s.state.clone();
        human.media = s.media.clone();
        human.execute(command, args).unwrap();
        read(&mut s, n);
        let req = request(&s, n, edit);
        let undo = s.history.undo.len();
        let journal = s.journal.len();
        let revision = s.revision;
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        assert_eq!(receipt["action"], "effect_stack_edit");
        assert_eq!(project_audit::bytes(&s.project).unwrap(), project_audit::bytes(&human.project).unwrap());
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(s.journal.len(), journal + 1);
        assert_eq!(s.revision, revision + 1);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
        let selected = s.state.selected_mask.unwrap();
        assert_eq!(s.active_sequence().unwrap().find_item(selected.clip).unwrap().1.effects[selected.effect].instance_id.as_deref(), Some(first.as_str()));
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        let after = s.project.clone();
        let saved = filmcraft_format::encode(&s.project, false);
        assert_eq!(project_audit::bytes(&filmcraft_format::decode(&saved).unwrap().project).unwrap(), project_audit::bytes(&after).unwrap());
        read(&mut s, n + 1);
        s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
        assert_eq!(project_audit::bytes(&s.project).unwrap(), project_audit::bytes(&before).unwrap());
        s.redo().unwrap();
        assert_eq!(project_audit::bytes(&s.project).unwrap(), project_audit::bytes(&after).unwrap());
        let kept = s.active_sequence().unwrap().video_tracks[0].items[0].effects.iter().find(|e| e.instance_id.as_ref() == Some(&first)).unwrap();
        assert!(!kept.masks.is_empty());
        assert_eq!(kept.f64_at("blurriness", Tick::ZERO), 17.0);
    }
}
#[test]
fn stack_add_uses_native_defaults_and_fresh_persistent_identity_without_replacing_existing_grade_or_duplicates() {
    let mut s = fixture();
    let old = s.project.clone();
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let clip = c.id;
    let old_effects = c.effects.clone();
    read(&mut s, 70200);
    let receipt = s.execute("agent.apply", request(&s, 70200, json!({"type":"add","definition":"lumetri"}))).unwrap();
    let effects = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects;
    assert_eq!(effects.len(), old_effects.len() + 1);
    assert_eq!(&effects[..old_effects.len()], old_effects.as_slice());
    let added = effects.last().unwrap();
    assert_eq!(json!(added.instance_id), receipt["results"][0]["changedEffectId"]);
    assert!(old_effects.iter().all(|e| e.instance_id != added.instance_id));
    let mut expected = (*old).clone();
    let q = expected.sequence_mut(s.state.active_sequence.unwrap()).unwrap();
    let mut instance = filmcraft_project::find_effect("lumetri").unwrap().instance();
    filmcraft_project::resolve_auto_points(&mut instance, (q.settings.width, q.settings.height), (q.settings.width, q.settings.height));
    instance.instance_id = added.instance_id.clone();
    q.find_item_mut(clip).unwrap().1.effects.push(instance);
    assert_eq!(project_audit::bytes(&s.project).unwrap(), project_audit::bytes(&expected).unwrap());
}
#[test]
fn stack_invalid_targets_stale_digests_noops_and_raw_fields_leave_redo_and_document_untouched() {
    let mut s = fixture();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("effects.toggleEnabled", json!({"clip":clip.0,"index":0})).unwrap();
    s.undo().unwrap();
    read(&mut s, 70300);
    let id = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects[0].instance_id.clone().unwrap();
    let before = state(&s);
    let project = s.project.clone();
    for edit in [
        json!({"type":"remove","effectId":"a".repeat(32)}),
        json!({"type":"set_enabled","effectId":id,"enabled":true}),
        json!({"type":"reorder","effectId":id,"beforeEffectId":id}),
        json!({"type":"reorder","effectId":id}),
        json!({"type":"add","definition":"motion"}),
        json!({"type":"add","definition":"Gaussian Blur"}),
        json!({"type":"add","definition":"gaussian_blur","path":"/not/a/resource"}),
    ] {
        assert!(s.execute("agent.apply", request(&s, 70300, edit)).is_err());
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&s.project, &project));
    }
    let good = request(&s, 70300, json!({"type":"set_enabled","effectId":id,"enabled":false}));
    for (field, value) in [("identity", json!("b".repeat(32))), ("beforeStackSha256", json!("0".repeat(64))), ("clip", json!(0)), ("extra", json!(true))] {
        let mut bad = good.clone();
        bad["operations"][0]["params"][field] = value;
        assert!(s.execute("agent.apply", bad).is_err());
        assert_eq!(state(&s), before);
    }
    let mut mixed = good.clone();
    mixed["operations"].as_array_mut().unwrap().push(good["operations"][0].clone());
    assert!(s.execute("agent.apply", mixed).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn stack_digest_covers_unpaged_parameters_masks_keyframes_and_enablement() {
    let s = fixture();
    let effects = &s.active_sequence().unwrap().video_tracks[0].items[0].effects;
    let before = effect_stack::digest(effects).unwrap();
    let mut altered = effects.clone();
    altered[2].enabled = false;
    assert_ne!(effect_stack::digest(&altered).unwrap(), before);
    let mut altered = effects.clone();
    altered[0].masks[0].inverted = true;
    assert_ne!(effect_stack::digest(&altered).unwrap(), before);
    let mut altered = effects.clone();
    altered[0].params.get_mut("blurriness").unwrap().keyframes[0].value = ParamValue::Float(19.0);
    assert_ne!(effect_stack::digest(&altered).unwrap(), before);
}
