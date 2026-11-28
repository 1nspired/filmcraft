//! Bounded scalar curves are native keyframes, never editorial metadata.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, Interpolation, Keyframe, ParamValue};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
const CMD: &str = "poema.setScalarCurve";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    clip: u64,
    effect: usize,
    param: String,
    points: Vec<Point>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Point {
    pub(crate) time: i64,
    pub(crate) value: f64,
    #[serde(default)]
    pub(crate) in_handle: Option<[f64; 2]>,
    #[serde(default)]
    pub(crate) out_handle: Option<[f64; 2]>,
}
pub(crate) const CURVE_BY: &str = "poema.gainCurve.by.v1";
pub(crate) const CURVE_PRESENT: &str = "poema.gainCurve.present.v1";
pub(crate) const CURVE_BASIS: &str = "poema.gainCurve.basis.v1";
/// Only tagged Poema Volume curves gain editorial human intent. Ordinary
/// FilmCraft clips/parameters and staged agent operations retain their behavior.
pub(crate) fn human_curve_target(item: &filmcraft_project::TrackItem, effect: &Value, param: &str, p: &Value) -> bool {
    if param != "level" || !p.get("mask").is_none_or(Value::is_null) {
        return false;
    }
    let Some(identity) = item.identity.as_ref().filter(|i| i.annotations.contains_key(crate::poema_metadata::KEY)) else {
        return false;
    };
    let e = match effect {
        Value::Number(n) => n.as_u64().and_then(|i| usize::try_from(i).ok()).and_then(|i| item.effects.get(i)),
        Value::String(id) => item.effects.iter().find(|e| &e.effect == id),
        _ => None,
    };
    e.is_some_and(|e| {
        e.effect == "volume"
            && (e.params.get("level").is_some_and(|v| v.keyframes.iter().any(|k| k.interp == Interpolation::Cubic))
                || identity.annotations.contains_key(CURVE_BY))
    })
}
/// Cross-language SHA-256 over scalar bytes, never numeric editorial state.
pub(crate) fn curve_basis(p: &filmcraft_project::Param) -> Result<String> {
    param_basis(p, b"poema-clip-gain-curve-v1\0", CMD)
}
/// Shared byte-exact digest; domains keep source-clock and sequence-clock curves distinct.
pub(crate) fn param_basis(p: &filmcraft_project::Param, domain: &[u8], command: &str) -> Result<String> {
    use sha2::Digest;
    let float = |v: &ParamValue| match v {
        ParamValue::Float(n) if n.is_finite() => Ok(*n),
        _ => Err(bad(command, "curve basis requires finite Float values")),
    };
    if p.keyframes.len() > 4096 {
        return Err(bad(command, "curve basis exceeds point bound"));
    }
    let mut h = sha2::Sha256::new();
    h.update(domain);
    h.update(float(&p.value)?.to_le_bytes());
    h.update((p.keyframes.len() as u32).to_le_bytes());
    for k in &p.keyframes {
        h.update(k.time.0.to_le_bytes());
        h.update(float(&k.value)?.to_le_bytes());
        h.update([match k.interp {
            Interpolation::Linear => 0,
            Interpolation::Hold => 1,
            Interpolation::Bezier => 2,
            Interpolation::AutoBezier => 3,
            Interpolation::ContinuousBezier => 4,
            Interpolation::EaseIn => 5,
            Interpolation::EaseOut => 6,
            Interpolation::Cubic => 7,
        }]);
        h.update(k.out_influence.to_le_bytes());
        h.update(k.in_influence.to_le_bytes());
        for raw in [k.in_handle, k.out_handle] {
            if let Some(v) = raw {
                h.update([1]);
                h.update(v[0].to_le_bytes());
                h.update(v[1].to_le_bytes());
            } else {
                h.update([0]);
            }
        }
    }
    Ok(format!("{:x}", h.finalize()))
}

pub(crate) fn mark_human_curve(item: &mut filmcraft_project::TrackItem, human: bool) -> Result<()> {
    if !human {
        return Ok(());
    }
    let present = item.effect("volume").and_then(|e| e.param("level")).is_some_and(|p| !p.keyframes.is_empty());
    let basis = item.effect("volume").and_then(|e| e.param("level")).ok_or_else(|| bad(CMD, "curve parameter disappeared"))?;
    let basis = curve_basis(basis)?;
    let Some(identity) = item.identity.as_mut() else {
        return Ok(());
    };
    if present {
        identity.protection = Some("human".into());
    } // Legacy curve clear does not pin.
    identity.annotations.insert(CURVE_BY.into(), "human".into());
    identity.annotations.insert(CURVE_PRESENT.into(), present.to_string());
    identity.annotations.insert(CURVE_BASIS.into(), basis);
    identity.validate().map_err(|e| bad(CMD, e))?;
    Ok(())
}

pub(crate) fn handle(value: &Value) -> Result<Option<[f64; 2]>> {
    if value.is_null() {
        return Ok(None);
    }
    let h: [f64; 2] = serde_json::from_value(value.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if h.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
        return Err(bad(CMD, "tangents need finite bounded seconds/value deltas"));
    }
    Ok(Some(h))
}
/// Human graph edits materialize the other linear control on the first drag,
/// inside the same native edit. Whole-curve delivery above/below keeps exact
/// caller-supplied absence and never performs this gesture-specific conversion.
pub(crate) fn edit_handles(prm: &mut filmcraft_project::Param, at: Tick, p: &Value) -> Result<()> {
    if p.get("inHandle").is_none() && p.get("outHandle").is_none() {
        return Ok(());
    }
    let i = prm.keyframes.iter().position(|k| k.time == at).ok_or_else(|| bad(CMD, "unknown keyframe"))?;
    let current = &prm.keyframes[i];
    let value = current
        .value
        .as_f64()
        .filter(|_| matches!(current.value, ParamValue::Float(_)))
        .ok_or_else(|| bad(CMD, "tangents require a scalar Float parameter"))?;
    for (key, outgoing) in [("inHandle", false), ("outHandle", true)] {
        let Some(wire) = p.get(key) else { continue };
        let next = handle(wire)?;
        let cubic = if outgoing {
            prm.keyframes[i].interp == Interpolation::Cubic
        } else {
            prm.keyframes[i].interp == Interpolation::Cubic
                || i.checked_sub(1).and_then(|j| prm.keyframes.get(j)).is_some_and(|k| k.interp == Interpolation::Cubic)
        };
        if !cubic {
            return Err(bad(CMD, "raw tangents require Cubic interpolation"));
        }
        let neighbor = if outgoing { i.checked_add(1).filter(|j| *j < prm.keyframes.len()) } else { i.checked_sub(1) };
        if let Some(j) = neighbor {
            let own = if outgoing { prm.keyframes[i].out_handle } else { prm.keyframes[i].in_handle };
            let partner = if outgoing { prm.keyframes[j].in_handle } else { prm.keyframes[j].out_handle };
            if next.is_some() && own.is_none() && partner.is_none() {
                let n = &prm.keyframes[j];
                let nv = n.value.as_f64().filter(|_| matches!(n.value, ParamValue::Float(_))).ok_or_else(|| bad(CMD, "neighbor must be scalar"))?;
                let ticks = n.time.0.checked_sub(at.0).ok_or_else(|| bad(CMD, "tangent span overflow"))?;
                let h = [-(ticks as f64) / filmcraft_time::TICKS_PER_SECOND as f64 / 3.0, (value - nv) / 3.0];
                if h.iter().any(|x| !x.is_finite() || x.abs() > 1e6) {
                    return Err(bad(CMD, "linear tangent is outside supported bounds"));
                }
                if outgoing {
                    prm.keyframes[j].in_handle = Some(h);
                } else {
                    prm.keyframes[j].out_handle = Some(h);
                }
            }
        }
        if outgoing {
            prm.keyframes[i].out_handle = next;
        } else {
            prm.keyframes[i].in_handle = next;
        }
    }
    Ok(())
}

/// Validated native scalar keys used by clip and additive track command doors.
pub(crate) fn scalar_keys(points: Vec<Point>, command: &str) -> Result<Vec<Keyframe>> {
    if points.len() == 1 || points.len() > 4096 {
        return Err(bad(command, "need zero or 2..4096 points"));
    }
    let mut keys = Vec::with_capacity(points.len());
    let mut previous = None;
    for p in points {
        if !(Tick::MIN.0..=Tick::MAX.0).contains(&p.time) || previous.is_some_and(|t| t >= p.time) || !p.value.is_finite() || p.value.abs() > 1e6 {
            return Err(bad(command, "curve times must be bounded and strictly increasing, values finite and bounded"));
        }
        for h in [p.in_handle, p.out_handle].into_iter().flatten() {
            if h.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
                return Err(bad(command, "invalid tangent"));
            }
        }
        let mut k = Keyframe::new(Tick(p.time), ParamValue::Float(p.value));
        k.interp = Interpolation::Cubic;
        k.in_handle = p.in_handle;
        k.out_handle = p.out_handle;
        keys.push(k);
        previous = Some(p.time);
    }
    Ok(keys)
}

pub(crate) fn set(s: &mut Session, p: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let r: Request = serde_json::from_value(p.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if r.param.is_empty() || r.param.len() > 128 || r.points.len() == 1 || r.points.len() > 4096 {
        return Err(bad(CMD, "need a named scalar parameter and zero or 2..4096 points"));
    }
    let keys = scalar_keys(r.points, CMD)?;
    let id = ClipId(r.clip);
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let track = q.find_item(id).and_then(|(t, _)| q.track(t)).ok_or_else(|| bad(CMD, "unknown clip"))?;
    if track.locked {
        return Err(bad(CMD, "track is locked"));
    }
    let target = q
        .find_item(id)
        .map(|(_, c)| c)
        .and_then(|c| c.effects.get(r.effect))
        .and_then(|e| e.params.get(&r.param))
        .ok_or_else(|| bad(CMD, "unknown parameter"))?;
    if !matches!(target.value, ParamValue::Float(_)) {
        return Err(bad(CMD, "Cubic tangents require a scalar Float parameter"));
    }
    if target.keyframes == keys {
        return Ok(json!({"changed":false}));
    }
    s.edit_sequence("Set scalar curve", |q, _, _| {
        let (_, clip) = q.find_item_mut(id).ok_or_else(|| bad(CMD, "unknown clip"))?;
        let target = clip.effects.get_mut(r.effect).and_then(|e| e.params.get_mut(&r.param)).ok_or_else(|| bad(CMD, "unknown parameter"))?;
        target.keyframes = keys;
        q.check().map_err(crate::EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({"changed":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn demo() -> (Session, u64, usize) {
        let mut s = Session::default();
        let (p, id) = crate::demo::demo_project(&s.media);
        s.project = Arc::new(p);
        s.state.active_sequence = Some(id);
        let c = &s.active_sequence().unwrap().audio_tracks[0].items[0];
        let clip = c.id.0;
        let effect = c.effects.iter().position(|e| e.effect == "volume").unwrap();
        (s, clip, effect)
    }
    fn request(clip: u64, effect: usize) -> Value {
        json!({"clip":clip,"effect":effect,"param":"level","points":[{"time":0,"value":-18.0},{"time":508032000000i64,"value":-6.0}]})
    }
    #[test]
    fn native_curve_batch_noop_clear_and_undo_preserve_complete_project() {
        let (mut s, c, e) = demo();
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let p = request(c, e);
        let applied = s.execute("agent.apply", json!({"token":basis["token"],"operations":[{"command":CMD,"params":p}]})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        let rev = s.revision;
        s.execute(CMD, p).unwrap();
        assert_eq!(s.revision, rev);
        let doc = serde_json::to_value(&s.project).unwrap();
        let loaded: filmcraft_project::Project = serde_json::from_value(doc.clone()).unwrap();
        assert_eq!(serde_json::to_value(loaded).unwrap(), doc);
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
        s.execute(CMD, request(c, e)).unwrap();
        s.execute(CMD, json!({"clip":c,"effect":e,"param":"level","points":[]})).unwrap();
        assert!(s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.effects[e].params["level"].keyframes.is_empty());
    }
    #[test]
    fn first_human_handle_drag_materializes_linear_partner_in_one_undo() {
        let (mut s, c, e) = demo();
        s.execute(CMD, request(c, e)).unwrap();
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        s.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":0,"outHandle":[2.0/3.0,4.0]})).unwrap();
        let p = &s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.effects[e].params["level"];
        assert_eq!(p.keyframes[1].in_handle, Some([-2.0 / 3.0, -4.0]));
        assert!((p.scalar_at(Tick::from_seconds_f64(1.0)) + 12.0).abs() < 1e-9);
        assert_eq!(s.history.undo.len(), depth + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
        s.execute("effects.setInterpolation", json!({"clip":c,"effect":e,"param":"level","mediaTime":508032000000i64,"interpolation":"linear"})).unwrap();
        s.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":508032000000i64,"inHandle":[-0.5,-4.0]})).unwrap();
    }
    #[test]
    fn malformed_points_and_handle_gestures_refuse_atomically() {
        let (mut s, c, e) = demo();
        s.execute(CMD, request(c, e)).unwrap();
        let mut bad = request(c, e);
        bad["points"][1]["time"] = json!(0);
        let mut huge = request(c, e);
        huge["points"][0]["outHandle"] = json!([0.3, 1e7]);
        for p in [
            bad,
            huge,
            json!({"clip":c,"effect":e,"param":"missing","points":[]}),
            json!({"clip":c,"effect":e,"param":"level","points":[{"time":0,"value":0}]}),
            json!({"clip":c,"effect":e,"param":"level","points":[],"extra":1}),
        ] {
            let before = s.project.clone();
            let rev = s.revision;
            assert!(s.execute(CMD, p).is_err());
            assert!(Arc::ptr_eq(&s.project, &before));
            assert_eq!(s.revision, rev);
        }
        let before = s.project.clone();
        let rev = s.revision;
        assert!(s.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":0,"value":-9,"outHandle":[0.2,1e7]})).is_err());
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(s.revision, rev);
    }
    fn tagged(s: &mut Session, c: u64) {
        let mut ann = serde_json::Map::new();
        ann.insert(crate::poema_metadata::KEY.into(), json!(r#"{"schema":1,"fields":[],"props":{}}"#));
        ann.insert(CURVE_BY.into(), json!("fixture"));
        ann.insert(CURVE_PRESENT.into(), json!("true"));
        s.execute("poema.setCarrier",json!({"clip":c,"identity":{"id":"scalar-provenance","protection":"agent","annotations":ann,"handoffSpans":[{"carrier":"poema:sel_curve","sourceIn":"0","sourceOut":"508032000000"}]}})).unwrap();
    }
    #[test]
    fn human_scalar_edits_stamp_by_and_pin_in_the_same_native_undo() {
        let (mut s, c, e) = demo();
        s.execute(CMD, request(c, e)).unwrap();
        tagged(&mut s, c);
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        s.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":0,"value":-15})).unwrap();
        let id = s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.as_ref().unwrap();
        assert_eq!(id.protection.as_deref(), Some("human"));
        assert_eq!(id.annotations[CURVE_BY], "human");
        assert_eq!(id.annotations[CURVE_PRESENT], "true");
        assert_eq!(id.handoff_spans[0].carrier, "poema:sel_curve");
        assert_eq!(s.history.undo.len(), depth + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
        s.execute("effects.setParam", json!({"clip":c,"effect":e,"param":"level","time":0,"value":-15})).unwrap();
        assert_eq!(s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.as_ref().unwrap().annotations[CURVE_BY], "human");
    }
    #[test]
    fn human_curve_clear_intent_is_native_and_undoable() {
        let (mut s, c, e) = demo();
        s.execute(CMD, request(c, e)).unwrap();
        tagged(&mut s, c);
        let before = serde_json::to_value(&s.project).unwrap();
        s.execute("effects.toggleAnimation", json!({"clip":c,"effect":e,"param":"level"})).unwrap();
        let id = s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.as_ref().unwrap();
        assert_eq!(id.annotations[CURVE_PRESENT], "false");
        assert_eq!(id.protection.as_deref(), Some("agent"));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    }
    #[test]
    fn staged_agent_and_ordinary_native_curves_do_not_gain_human_provenance() {
        let (mut s, c, e) = demo();
        s.execute(CMD, request(c, e)).unwrap();
        tagged(&mut s, c);
        s.agent_bridge.staging = true;
        s.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":0,"value":-15})).unwrap();
        s.agent_bridge.staging = false;
        assert_eq!(s.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.as_ref().unwrap().annotations[CURVE_BY], "fixture");
        let (mut ordinary, c, e) = demo();
        ordinary.execute(CMD, request(c, e)).unwrap();
        ordinary.execute("effects.setKeyframe", json!({"clip":c,"effect":e,"param":"level","mediaTime":0,"value":-15})).unwrap();
        assert!(ordinary.active_sequence().unwrap().find_item(ClipId(c)).unwrap().1.identity.is_none());
    }
    #[test]
    fn curve_basis_has_a_fixed_independent_cross_language_vector() {
        let mut p = filmcraft_project::Param::new(ParamValue::Float(0.0));
        for (time, value) in [(0, -18.0), (508032000000, -6.0)] {
            let mut k = Keyframe::new(Tick(time), ParamValue::Float(value));
            k.interp = Interpolation::Cubic;
            p.keyframes.push(k);
        }
        assert_eq!(curve_basis(&p).unwrap(), "b46deaa0b26e2f7eaf75cc77370b3221bb5228abfee13f4b4a46cfe4733a7e67");
        let before = curve_basis(&p).unwrap();
        p.keyframes[0].out_handle = Some([0.3, 4.0]);
        assert_ne!(curve_basis(&p).unwrap(), before);
    }
}
