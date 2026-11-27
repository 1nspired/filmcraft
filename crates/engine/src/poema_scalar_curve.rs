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
struct Point {
    time: i64,
    value: f64,
    #[serde(default)]
    in_handle: Option<[f64; 2]>,
    #[serde(default)]
    out_handle: Option<[f64; 2]>,
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

pub(crate) fn set(s: &mut Session, p: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let r: Request = serde_json::from_value(p.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if r.param.is_empty() || r.param.len() > 128 || r.points.len() == 1 || r.points.len() > 4096 {
        return Err(bad(CMD, "need a named scalar parameter and zero or 2..4096 points"));
    }
    let mut keys = Vec::with_capacity(r.points.len());
    let mut previous = None;
    for p in r.points {
        if !(Tick::MIN.0..=Tick::MAX.0).contains(&p.time) || previous.is_some_and(|t| t >= p.time) || !p.value.is_finite() || p.value.abs() > 1e6 {
            return Err(bad(CMD, "curve times must be bounded and strictly increasing, values finite and bounded"));
        }
        for h in [p.in_handle, p.out_handle].into_iter().flatten() {
            if h.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
                return Err(bad(CMD, "invalid tangent"));
            }
        }
        let mut k = Keyframe::new(Tick(p.time), ParamValue::Float(p.value));
        k.interp = Interpolation::Cubic;
        k.in_handle = p.in_handle;
        k.out_handle = p.out_handle;
        keys.push(k);
        previous = Some(p.time);
    }
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
}
