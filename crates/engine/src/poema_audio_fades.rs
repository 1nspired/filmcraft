//! Explicit audio fades live in a native optional intrinsic, never annotations.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, EffectInstance, ParamValue, TrackKind, find_effect};
use serde::Deserialize;
use serde_json::{Value, json};
const CMD: &str = "poema.setAudioFades";
const EFFECT: &str = "clip_audio_fades";
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Curve {
    EqualPower,
    Linear,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    clip: u64,
    fade_in: Option<f64>,
    fade_out: Option<f64>,
    fade_in_curve: Option<Curve>,
    fade_out_curve: Option<Curve>,
}
fn valid(effect: &EffectInstance) -> bool {
    let Some(def) = find_effect(EFFECT) else { return false };
    let mut expected = def.instance();
    for key in ["in_seconds", "out_seconds"] {
        let v = effect.f64_at(key, filmcraft_time::Tick::ZERO);
        if !v.is_finite() || !(0.0..=5.0).contains(&v) || format!("{v:.6}").parse::<f64>().ok() != Some(v) {
            return false;
        }
        let Some(p) = expected.param_mut(key) else { return false };
        p.value = ParamValue::Float(v);
    }
    for key in ["in_linear", "out_linear"] {
        let Some(value) = effect.param(key).and_then(|p| p.value.as_bool()) else { return false };
        let Some(p) = expected.param_mut(key) else { return false };
        p.value = ParamValue::Bool(value);
    }
    effect == &expected
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let r: Request = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if r.fade_in.is_none() && r.fade_out.is_none() && r.fade_in_curve.is_none() && r.fade_out_curve.is_none() {
        return Err(bad(CMD, "supply an explicit fade property"));
    }
    for value in [r.fade_in, r.fade_out].into_iter().flatten() {
        if !value.is_finite() || !(0.0..=5.0).contains(&value) || format!("{value:.6}").parse::<f64>().ok() != Some(value) {
            return Err(bad(CMD, "fade duration must be 0..5 seconds at saved six-decimal precision"));
        }
    }
    let id = ClipId(r.clip);
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (tid, item) = q.find_item(id).ok_or_else(|| bad(CMD, "unknown clip"))?;
    let track = q.track(tid).ok_or_else(|| bad(CMD, "unknown track"))?;
    if track.kind != TrackKind::Audio || track.locked {
        return Err(bad(CMD, "fades require an unlocked explicit audio clip"));
    }
    if item.effects.len() > 128 {
        return Err(bad(CMD, "clip effect limit exceeded"));
    }
    let fades: Vec<_> = item.effects.iter().filter(|e| e.effect == EFFECT).collect();
    if fades.len() > 1 || fades.first().is_some_and(|e| !valid(e)) {
        return Err(bad(CMD, "existing fade envelope is not a supported static intrinsic"));
    }
    let mut effect = fades
        .first()
        .map(|e| (*e).clone())
        .or_else(|| find_effect(EFFECT).map(|d| d.instance()))
        .ok_or_else(|| bad(CMD, "native fade definition unavailable"))?;
    for (key, value) in [("in_seconds", r.fade_in), ("out_seconds", r.fade_out)] {
        if let Some(value) = value {
            effect.param_mut(key).ok_or_else(|| bad(CMD, "fade parameter unavailable"))?.value = ParamValue::Float(value);
        }
    }
    for (key, value) in [("in_linear", r.fade_in_curve), ("out_linear", r.fade_out_curve)] {
        if let Some(value) = value {
            effect.param_mut(key).ok_or_else(|| bad(CMD, "fade curve unavailable"))?.value = ParamValue::Bool(matches!(value, Curve::Linear));
        }
    }
    let clear = effect.f64_at("in_seconds", filmcraft_time::Tick::ZERO) == 0.0
        && effect.f64_at("out_seconds", filmcraft_time::Tick::ZERO) == 0.0
        && effect.param("in_linear").and_then(|p| p.value.as_bool()) == Some(false)
        && effect.param("out_linear").and_then(|p| p.value.as_bool()) == Some(false);
    if (clear && fades.is_empty()) || (!clear && fades.first().is_some_and(|e| **e == effect)) {
        return Ok(json!({"changed":false}));
    }
    if !clear && fades.is_empty() && item.effects.len() >= 128 {
        return Err(bad(CMD, "clip effect limit exceeded"));
    }
    s.edit_sequence("Set clip audio fades", |q, _, _| {
        let item = q.find_item_mut(id).ok_or_else(|| bad(CMD, "unknown clip"))?.1;
        if let Some(index) = item.effects.iter().position(|e| e.effect == EFFECT) {
            if clear {
                item.effects.remove(index);
            } else {
                item.effects[index] = effect;
            }
        } else if !clear {
            item.effects.push(effect);
        }
        q.check().map_err(crate::EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({"changed":true}))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn demo() -> Session {
        let mut s = Session::default();
        let (p, id) = crate::demo::demo_project(&s.media);
        s.project = Arc::new(p);
        s.state.active_sequence = Some(id);
        s
    }
    #[test]
    fn native_fades_batch_noop_and_undo_preserve_complete_original() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let applied = s
            .execute(
                "agent.apply",
                json!({"token":basis["token"],"operations":[{"command":CMD,"params":{"clip":id,"fadeIn":0.123456,"fadeOut":1.5,"fadeOutCurve":"linear"}}]}),
            )
            .unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        let effect = s.active_sequence().unwrap().find_item(ClipId(id)).unwrap().1.effect(EFFECT).unwrap();
        assert!(valid(effect));
        assert_eq!(effect.f64_at("in_seconds", filmcraft_time::Tick::ZERO), 0.123456);
        let rev = s.revision;
        s.execute(CMD, json!({"clip":id,"fadeIn":0.123456})).unwrap();
        assert_eq!(s.revision, rev);
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    }
    #[test]
    fn bad_fade_properties_and_picture_targets_refuse_atomically() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
        let picture = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        for p in [
            json!({"clip":id}),
            json!({"clip":id,"fadeIn":-0.1}),
            json!({"clip":id,"fadeOut":6}),
            json!({"clip":id,"fadeIn":0.1234567}),
            json!({"clip":id,"fadeIn":true}),
            json!({"clip":id,"fadeInCurve":"other"}),
            json!({"clip":id,"fadeIn":1,"extra":true}),
            json!({"clip":picture,"fadeIn":1}),
        ] {
            let before = s.project.clone();
            let rev = s.revision;
            assert!(s.execute(CMD, p).is_err());
            assert!(Arc::ptr_eq(&before, &s.project));
            assert_eq!(s.revision, rev);
        }
    }

    #[test]
    fn zero_duration_keeps_explicit_curve_and_locked_targets_refuse() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
        s.execute(CMD, json!({"clip":id,"fadeIn":0,"fadeInCurve":"linear"})).unwrap();
        let effect = s.active_sequence().unwrap().find_item(ClipId(id)).unwrap().1.effect(EFFECT).unwrap();
        assert_eq!(effect.param("in_linear").unwrap().value.as_bool(), Some(true));
        assert_eq!(effect.f64_at("in_seconds", filmcraft_time::Tick::ZERO), 0.0);
        let seq = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
        let tid = q.find_item(ClipId(id)).unwrap().0;
        q.track_mut(tid).unwrap().locked = true;
        let before = s.project.clone();
        let rev = s.revision;
        assert!(s.execute(CMD, json!({"clip":id,"fadeIn":1})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, rev);
    }

    #[test]
    fn fade_round_trip_in_a_batch_keeps_effect_order_and_is_a_real_noop() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
        s.execute(CMD, json!({"clip":id,"fadeIn":0.25})).unwrap();
        s.execute("poema.setChannelSum", json!({"clip":id,"enabled":true})).unwrap();
        let before = serde_json::to_value(&s.project).unwrap();
        let rev = s.revision;
        let depth = s.history.undo.len();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let applied = s
            .execute(
                "agent.preview",
                json!({"token":basis["token"],"operations":[
                    {"command":CMD,"params":{"clip":id,"fadeIn":0.5}},
                    {"command":CMD,"params":{"clip":id,"fadeIn":0.25}}
                ]}),
            )
            .unwrap();
        assert_eq!(applied["changed"], false);
        let committed = s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":applied["preview"]})).unwrap();
        assert_eq!(committed["changed"], false);
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
        assert_eq!(s.revision, rev);
        assert_eq!(s.history.undo.len(), depth);
    }
}
