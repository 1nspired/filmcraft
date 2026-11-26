use crate::Session;
use filmcraft_project::clip_identity::EFFECT_TRACKING_KEY;
use filmcraft_project::{ClipId, EffectInstance, Project};
use serde_json::json;
use std::sync::Arc;
fn setup() -> (Session, ClipId) {
    let mut s = super::tests::setup();
    let c = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    for _ in 0..2 {
        s.execute("effects.apply", json!({"clips":[c.0],"effect":"gaussian_blur"})).unwrap();
    }
    s.execute("masks.add", json!({"clip":c.0,"effect":"opacity","shape":"rectangle"})).unwrap();
    (s, c)
}
fn effects(s: &Session, c: ClipId) -> Vec<EffectInstance> {
    s.active_sequence().unwrap().find_item(c).unwrap().1.effects.clone()
}
fn strip(mut p: Project) -> Project {
    for item in p.items.values_mut() {
        item.metadata.remove(EFFECT_TRACKING_KEY);
        if let Some(q) = item.as_sequence_mut() {
            for c in q.all_tracks_mut().flat_map(|t| &mut t.items) {
                for e in &mut c.effects {
                    e.clear_instance_ids();
                }
            }
        }
    }
    p
}
#[test]
fn effect_ids_require_explicit_upgrade_and_repeated_upgrade_adds_no_history() {
    let (mut s, c) = setup();
    let before = s.project.clone();
    let n = s.history.undo.len();
    let bytes = filmcraft_format::encode(&s.project, false);
    let legacy = filmcraft_format::decode(&bytes).unwrap();
    assert_eq!(legacy.project, *before);
    assert!(effects(&s, c).iter().all(|e| e.instance_id.is_none()));
    s.execute("poema.trackEffects", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), n + 1);
    assert_eq!(strip((*s.project).clone()), *before);
    for clip in s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items) {
        let ids: Vec<_> = clip.effects.iter().map(|e| e.instance_id.as_ref().unwrap()).collect();
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::BTreeSet<_>>().len());
        assert!(ids.iter().all(|id| filmcraft_project::ClipIdentity::valid_id(id)));
        for e in &clip.effects {
            assert!(e.masks.iter().all(|m| m.instance_id.is_some()));
        }
    }
    let tracked = s.project.clone();
    s.execute("poema.trackEffects", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), n + 1);
    assert!(Arc::ptr_eq(&s.project, &tracked));
    s.undo().unwrap();
    assert_eq!(s.project, before);
    s.redo().unwrap();
    assert_eq!(s.project, tracked);
    s.undo().unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    assert_ne!(effects(&s, c)[0].instance_id, tracked.sequence(s.state.active_sequence.unwrap()).unwrap().find_item(c).unwrap().1.effects[0].instance_id);
}
#[test]
fn native_effect_edits_reorder_remove_and_add_preserve_survivors_and_mask_identity() {
    let (mut s, c) = setup();
    s.execute("poema.trackEffects", json!({})).unwrap();
    let original = effects(&s, c);
    s.execute("effects.setParam", json!({"clip":c.0,"effect":1,"param":"blurriness","value":42})).unwrap();
    assert_eq!(effects(&s, c)[1].instance_id, original[1].instance_id);
    let mask = original.iter().find(|e| e.effect == "opacity").unwrap().masks[0].instance_id.clone();
    s.execute("masks.set", json!({"clip":c.0,"effect":"opacity","mask":0,"feather":25})).unwrap();
    assert_eq!(effects(&s, c).iter().find(|e| e.effect == "opacity").unwrap().masks[0].instance_id, mask);
    s.edit_sequence("Human effect reorder", |q, _, _| {
        q.find_item_mut(c).unwrap().1.effects.swap(0, 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(effects(&s, c)[0].instance_id, original[1].instance_id);
    assert_eq!(effects(&s, c)[1].instance_id, original[0].instance_id);
    s.execute("effects.remove", json!({"clip":c.0,"index":1})).unwrap();
    let survivors = effects(&s, c);
    assert_eq!(survivors[0].instance_id, original[1].instance_id);
    s.execute("effects.apply", json!({"clips":[c.0],"effect":"gaussian_blur"})).unwrap();
    let new = effects(&s, c);
    assert_eq!(new[0].instance_id, survivors[0].instance_id);
    assert_ne!(new[1].instance_id, original[0].instance_id);
    let saved = s.project.clone();
    s.undo().unwrap();
    s.redo().unwrap();
    assert_eq!(s.project, saved);
}
#[test]
fn native_effect_ids_survive_split_copy_and_schema17_reopen_with_clip_scoped_namespaces() {
    let (mut s, c) = setup();
    s.execute("poema.trackEffects", json!({})).unwrap();
    let fx = effects(&s, c);
    let original = s.active_sequence().unwrap().find_item(c).unwrap().1.clone();
    let at = original.start + s.sequence_rate().tick_of(12);
    s.execute("timeline.razor", json!({"clip":c.0,"time":at.0})).unwrap();
    let right = s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.start == at).unwrap().clone();
    assert_ne!(right.identity.as_ref().unwrap().id, original.identity.as_ref().unwrap().id);
    assert_eq!(right.effects, fx);
    let destination = s.active_sequence().unwrap().duration() + s.sequence_rate().tick_of(24);
    s.execute("timeline.move", json!({"moves":[{"clip":c.0,"track":"V1","time":destination.0}],"copy":true,"insert":false})).unwrap();
    let copy = s.active_sequence().unwrap().video_tracks[0].items.iter().find(|i| i.start == destination).unwrap();
    assert_ne!(copy.identity.as_ref().unwrap().id, original.identity.as_ref().unwrap().id);
    assert_eq!(copy.effects, fx);
    let bytes = filmcraft_format::encode(&s.project, false);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["schema_version"], filmcraft_format::SCHEMA_VERSION);
    let loaded = filmcraft_format::decode(&bytes).unwrap();
    assert_eq!(loaded.project, *s.project);
    let doc = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(
        filmcraft_format::migrate_with(&filmcraft_format::MIGRATIONS[..15], doc, filmcraft_format::SCHEMA_VERSION),
        Err(filmcraft_format::FormatError::TooNew { found, supported: 16 }) if found == filmcraft_format::SCHEMA_VERSION
    ));
    let mut legacy = serde_json::to_value(strip((*s.project).clone())).unwrap();
    // Explicit old-schema envelope: decoding upgrades schema, not tracking state.
    let bytes =
        serde_json::to_vec(&json!({"format":filmcraft_format::FORMAT_ID,"schema_version":16,"generator":"generated legacy","project":legacy.take()})).unwrap();
    let loaded = filmcraft_format::decode(&bytes).unwrap();
    assert!(loaded.migrated());
    assert_eq!(loaded.project, strip((*s.project).clone()));
}
#[test]
fn native_effect_paste_and_presets_copy_new_instances_without_reassigning_intrinsics() {
    let (mut s, c) = setup();
    s.execute("poema.trackEffects", json!({})).unwrap();
    let before = effects(&s, c);
    s.state.selection = vec![c];
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("edit.pasteAttributes", json!({"clips":[c.0]})).unwrap();
    let after = effects(&s, c);
    let ids: std::collections::BTreeSet<_> = before.iter().map(|e| e.instance_id.clone()).collect();
    let copies: Vec<_> = after.iter().filter(|e| e.effect == "gaussian_blur" && !ids.contains(&e.instance_id)).collect();
    assert_eq!(copies.len(), 2);
    for e in before.iter().filter(|e| e.def().unwrap().intrinsic) {
        assert_eq!(after.iter().find(|x| x.effect == e.effect).unwrap().instance_id, e.instance_id);
    }
    let mut pasted = filmcraft_project::find_effect("motion").unwrap().instance();
    pasted.instance_id = before[0].instance_id.clone();
    let intrinsic = after.iter().find(|e| e.effect == "motion").unwrap().instance_id.clone();
    s.edit_sequence("Human preset apply", |q, _, _| {
        crate::presets::put_effects(q.find_item_mut(c).unwrap().1, vec![pasted]);
        Ok(())
    })
    .unwrap();
    assert_eq!(effects(&s, c).iter().find(|e| e.effect == "motion").unwrap().instance_id, intrinsic);
    s.execute("edit.removeAttributes", json!({"clips":[c.0],"effects":false})).unwrap();
    assert_eq!(effects(&s, c).iter().find(|e| e.effect == "motion").unwrap().instance_id, intrinsic);
}
#[test]
fn malformed_effect_or_mask_ids_refuse_load_and_candidate_edits_without_repair_or_redo_loss() {
    for mode in ["effect_missing", "effect_duplicate", "mask_missing", "mask_invalid", "marker"] {
        let (mut s, c) = setup();
        s.execute("poema.trackEffects", json!({})).unwrap();
        s.execute("effects.setParam", json!({"clip":c.0,"effect":0,"param":"blurriness","value":10})).unwrap();
        s.undo().unwrap();
        assert!(!s.history.redo.is_empty());
        let mut damaged = (*s.project).clone();
        let seq = s.state.active_sequence.unwrap();
        if mode == "marker" {
            damaged.item_mut(seq).unwrap().metadata.insert(EFFECT_TRACKING_KEY.into(), "99".into());
        } else {
            let effects = &mut damaged.sequence_mut(seq).unwrap().find_item_mut(c).unwrap().1.effects;
            match mode {
                "effect_missing" => effects[0].instance_id = None,
                "effect_duplicate" => effects[1].instance_id = effects[0].instance_id.clone(),
                "mask_missing" => effects.iter_mut().find(|e| e.effect == "opacity").unwrap().masks[0].instance_id = None,
                "mask_invalid" => effects.iter_mut().find(|e| e.effect == "opacity").unwrap().masks[0].instance_id = Some("bad".into()),
                _ => {}
            }
        }
        assert!(filmcraft_format::decode(&filmcraft_format::encode(&damaged, false)).is_err(), "{mode}");
        // Missing IDs in a persisted opted-in file are damage. Missing IDs on
        // fresh constructor instances during an actual edit are allocation work.
        if mode.ends_with("missing") {
            continue;
        }
        let before = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone());
        assert!(
            s.edit("Damaged candidate", |p, _| {
                *p = damaged;
                Ok(())
            })
            .is_err()
        );
        assert!(Arc::ptr_eq(&s.project, &before.0));
        assert_eq!((s.revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3));
        assert_eq!(s.journal, before.4);
    }
}
