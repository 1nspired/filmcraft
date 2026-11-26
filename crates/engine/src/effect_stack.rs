//! Shared human clip-effect ordering. Moving an effect retains its complete
//! authored parameters, masks, animation and persistent identity. Fixed effects
//! are barriers; no render, resource lookup or source-clock conversion occurs.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, EffectInstance};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write;

const COMMAND: &str = "effects.reorder";
const MAX_EFFECTS: usize = 4096;
const MAX_BYTES: usize = 16 * 1024 * 1024;
struct Budget {
    used: usize,
    limit: usize,
}
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.used =
            self.used.checked_add(bytes.len()).filter(|n| *n <= self.limit).ok_or_else(|| std::io::Error::other("effect stack exceeds ordering budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    clip: u64,
    from: usize,
    to: usize,
}

pub(crate) fn reordered(effects: &[EffectInstance], from: usize, to: usize) -> Result<Vec<EffectInstance>> {
    if effects.len() > MAX_EFFECTS || from >= effects.len() || to >= effects.len() || from == to {
        return Err(bad(COMMAND, "explicit distinct current effect positions inside the bounded stack are required"));
    }
    serde_json::to_writer(Budget { used: 0, limit: MAX_BYTES }, effects).map_err(|e| bad(COMMAND, e.to_string()))?;
    if effects.get(from.min(to)..=from.max(to)).is_none_or(|span| span.iter().any(|effect| effect.def().is_some_and(|d| d.intrinsic))) {
        return Err(bad(COMMAND, "fixed effects cannot move or be crossed"));
    }
    // Bounds above prove remove/insert valid, including insertion at the end
    // after removal. No part of the instance is reconstructed or reidentified.
    let mut next = effects.to_vec();
    let effect = next.remove(from);
    next.insert(to, effect);
    Ok(next)
}
pub(crate) fn run(s: &mut Session, params: &Value) -> Result<Value> {
    serde_json::to_writer(Budget { used: 0, limit: 512 }, params).map_err(|e| bad(COMMAND, e.to_string()))?;
    let request: Request = serde_json::from_value(params.clone()).map_err(|e| bad(COMMAND, e.to_string()))?;
    if request.clip == 0 || request.clip > 9_007_199_254_740_991 {
        return Err(bad(COMMAND, "an exact current clip is required"));
    }
    let clip = ClipId(request.clip);
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (track, item) = q.find_item(clip).ok_or_else(|| bad(COMMAND, "no such clip"))?;
    if q.track(track).is_none_or(|track| track.locked) {
        return Err(bad(COMMAND, "effect track is locked"));
    }
    let before = &item.effects;
    let effects = reordered(before, request.from, request.to)?;
    let selection = s.state.selected_effect.as_ref().filter(|selected| selected.clip == clip).and_then(|selected| selected.index(before));
    let selection = selection.and_then(|old| {
        let at = if old == request.from {
            request.to
        } else if request.from < request.to && old > request.from && old <= request.to {
            old - 1
        } else if request.to < request.from && old >= request.to && old < request.from {
            old + 1
        } else {
            old
        };
        crate::EffectSel::of(clip, &effects, at)
            .filter(|selected| selected.instance_id.is_some() || effects.iter().filter(|effect| effect.effect == selected.effect).count() == 1)
    });
    let moved = effects.get(request.to).and_then(|effect| effect.instance_id.clone());
    let selected_target = s.state.selected_effect.as_ref().is_some_and(|selected| selected.clip == clip);
    let untracked = item.effects.iter().any(|effect| effect.instance_id.is_none());
    s.edit_sequence("Reorder Effect", |sequence, _, state| {
        sequence.find_item_mut(clip).ok_or_else(|| bad(COMMAND, "clip changed before effect ordering"))?.1.effects = effects;
        if selected_target {
            state.selected_effect = selection;
        }
        // Tracked masks are remapped by the shared project commit boundary.
        // Untracked duplicate positions have no persistent target to follow.
        if state.selected_mask.is_some_and(|selected| selected.clip == clip) && untracked {
            state.selected_mask = None;
        }
        Ok(())
    })?;
    Ok(json!({"clip":request.clip,"from":request.from,"to":request.to,"effectId":moved}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ParamValue, find_effect};
    use filmcraft_time::Tick;
    fn fixture() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let sequence = s.state.active_sequence.unwrap();
        std::sync::Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().video_tracks[0].items[0].effects.clear();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        for name in ["gaussian_blur", "lumetri", "gaussian_blur"] {
            s.execute("effects.apply", json!({"clips":[clip.0],"effect":name})).unwrap();
        }
        s.execute("masks.add", json!({"clip":clip.0,"effect":0,"shape":"rectangle"})).unwrap();
        s.execute("effects.setParam", json!({"clip":clip.0,"effect":0,"param":"blurriness","value":9,"time":0,"keyframe":true})).unwrap();
        s.execute("poema.trackEffects", json!({})).unwrap();
        s
    }
    fn state(s: &Session) -> Value {
        json!({"project":s.project,"state":s.state,"revision":s.revision,"saved_revision":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
    }
    #[test]
    fn human_effect_reorder_preserves_duplicate_instance_masks_animation_identity_and_saved_history() {
        let mut s = fixture();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        let before = s.project.clone();
        let old = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.clone();
        s.execute("effects.select", json!({"clip":clip.0,"effect":0})).unwrap();
        let undo = s.history.undo.len();
        let r = s.execute(COMMAND, json!({"clip":clip.0,"from":0,"to":2})).unwrap();
        assert_eq!(r["effectId"], json!(old[0].instance_id));
        let now = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects;
        assert_eq!(now[2], old[0]);
        assert_eq!(now[0], old[1]);
        assert_eq!(now[1], old[2]);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(now[2].f64_at("blurriness", Tick::ZERO), 9.0);
        assert!(!now[2].masks.is_empty());
        assert!(matches!(now[2].param("blurriness").unwrap().value, ParamValue::Float(_)));
        assert_eq!(s.state.selected_effect.as_ref().unwrap().index(now), Some(2));
        let after = s.project.clone();
        let saved = filmcraft_format::encode(&s.project, false);
        assert_eq!(serde_json::to_value(filmcraft_format::decode(&saved).unwrap().project).unwrap(), serde_json::to_value(&s.project).unwrap());
        s.undo().unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(before).unwrap());
        let restored = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects;
        assert_eq!(s.state.selected_effect.as_ref().unwrap().index(restored), Some(0));
        s.redo().unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(after).unwrap());
        let restored = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects;
        assert_eq!(s.state.selected_effect.as_ref().unwrap().index(restored), Some(2));
    }
    #[test]
    fn human_effect_reorder_rejects_invalid_positions_fixed_crossings_and_unknown_fields_without_consuming_redo() {
        let mut s = fixture();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        s.execute(COMMAND, json!({"clip":clip.0,"from":0,"to":1})).unwrap();
        s.undo().unwrap();
        let before = state(&s);
        let length = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.len();
        for params in [
            json!({"clip":clip.0,"from":0,"to":0}),
            json!({"clip":clip.0,"from":0,"to":length}),
            json!({"clip":clip.0,"from":-1,"to":0}),
            json!({"clip":clip.0,"from":0,"to":1,"extra":true}),
            json!({"clip":0,"from":0,"to":1}),
        ] {
            assert!(s.execute(COMMAND, params).is_err());
            assert_eq!(state(&s), before);
        }
        let blur = find_effect("gaussian_blur").unwrap().instance();
        let fixed = find_effect("motion").unwrap().instance();
        assert!(reordered(&[blur.clone(), fixed, blur], 0, 2).is_err());
    }
    #[test]
    fn human_effect_reorder_refuses_oversized_values_before_cloning() {
        let mut effect = find_effect("lumetri").unwrap().instance();
        effect.params.get_mut("input_lut").unwrap().value = ParamValue::Text("x".repeat(MAX_BYTES + 1));
        assert!(reordered(&[effect, find_effect("gaussian_blur").unwrap().instance()], 0, 1).is_err());
    }
    #[test]
    fn tracked_effect_selection_never_falls_back_to_a_duplicate_after_removal_and_legacy_selection_still_loads() {
        let s = fixture();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        let effects = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects;
        let selected = crate::EffectSel::of(clip, effects, 0).unwrap();
        assert!(selected.instance_id.is_some());
        assert_eq!(selected.index(&effects[1..]), None);
        let legacy: crate::EffectSel = serde_json::from_value(json!({"clip":clip.0,"effect":"gaussian_blur","instance":1})).unwrap();
        assert_eq!(legacy.index(effects), Some(2));
        assert_eq!(serde_json::to_value(&legacy).unwrap().get("instance_id"), None);
    }
    #[test]
    fn human_effect_reorder_keeps_selected_mask_through_native_history_without_borrowing_a_duplicate() {
        let mut s = fixture();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        let original = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects[0].instance_id.clone();
        assert_eq!(s.state.selected_mask.unwrap().effect, 0);
        s.execute(COMMAND, json!({"clip":clip.0,"from":0,"to":2})).unwrap();
        for (step, position) in [2, 0, 2].into_iter().enumerate() {
            let selected = s.state.selected_mask.unwrap();
            assert_eq!(selected.effect, position);
            assert_eq!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects[selected.effect].instance_id, original);
            match step {
                0 => {
                    s.undo().unwrap();
                }
                1 => {
                    s.redo().unwrap();
                }
                _ => {}
            }
        }
        s.execute("effects.remove", json!({"clip":clip.0,"index":2})).unwrap();
        assert!(s.state.selected_mask.is_none());
    }
}
