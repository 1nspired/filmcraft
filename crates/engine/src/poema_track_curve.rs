//! Additive sequence-clock automation, with native numeric state and checked editorial provenance.
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::mixer::{LANE_PAN_OFFSET, LANE_VOLUME_OFFSET, OffsetCurveIntent};
use filmcraft_project::{AudioChannels, ItemId, Param, ParamValue, Project, TrackId, TrackKind};
use serde::Deserialize;
use serde_json::{Value, json};
const CMD: &str = "poema.setTrackCurve";
const DOMAIN: &[u8] = b"poema-track-offset-curve-v1\0";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    track: u64,
    param: String,
    points: Vec<crate::poema_scalar_curve::Point>,
    #[serde(default)]
    by: Option<String>,
}
pub(crate) fn basis(p: &Param) -> Result<String> {
    crate::poema_scalar_curve::param_basis(p, DOMAIN, CMD)
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let r: Request = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    let (lane, field, min, max) = match r.param.as_str() {
        "gain" => (LANE_VOLUME_OFFSET, "gain_curve", -60.0, 12.0),
        "pan" => (LANE_PAN_OFFSET, "pan_curve", -1.0, 1.0),
        _ => return Err(bad(CMD, "param must be gain or pan")),
    };
    if r.points.iter().any(|p| !p.value.is_finite() || p.value < min || p.value > max) {
        return Err(bad(CMD, "offset point value outside its range"));
    }
    let by = r.by.unwrap_or_else(|| if s.agent_bridge.staging { "agent" } else { "human" }.into());
    if by.is_empty() || by.len() > 1024 || by.encode_utf16().count() > 256 {
        return Err(bad(CMD, "provenance must contain 1..256 UTF-16 units"));
    }
    let keys = crate::poema_scalar_curve::scalar_keys(r.points, CMD)?;
    let curve = if keys.is_empty() { None } else { Some(Param { value: ParamValue::Float(0.0), keyframes: keys }) };
    let intent = curve.as_ref().map(|p| basis(p).map(|basis| OffsetCurveIntent { by, basis })).transpose()?;
    let id = TrackId(r.track);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let track = q
        .track(id)
        .filter(|t| t.kind == TrackKind::Audio && t.channels == AudioChannels::Stereo)
        .ok_or_else(|| bad(CMD, "need an existing stereo audio track"))?;
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let patch = alias_patch(&s.project, seq, &[(id, field, curve.is_some())])?;
    if track.mixer.lanes.get(lane) == curve.as_ref() && track.mixer.offset_intents.get(lane) == intent.as_ref() && patch.is_none() {
        return Ok(json!({"changed":false}));
    }
    s.edit("Set additive track curve", move |project, _| {
        let q = project.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        let tr = q.track_mut(id).ok_or_else(|| bad(CMD, "unknown track"))?;
        match (curve, intent) {
            (Some(curve), Some(intent)) => {
                tr.mixer.lanes.insert(lane.into(), curve);
                tr.mixer.offset_intents.insert(lane.into(), intent);
            }
            _ => {
                tr.mixer.lanes.remove(lane);
                tr.mixer.offset_intents.remove(lane);
            }
        }
        q.check().map_err(EngineError::Other)?;
        if let Some(text) = patch {
            project.item_mut(seq).ok_or(EngineError::NoSequence)?.metadata.insert(crate::poema_metadata::KEY.into(), text);
        }
        Ok(())
    })?;
    Ok(json!({"changed":true}))
}

/// Native human keyframe tools share the existing checked clone and Undo. Staged agent writes
/// require their explicit intent; an unhooked agent change must retain a detectable stale basis.
pub(crate) fn reconcile_human(project: &mut Project, before: &Project, seq: ItemId, human: bool) -> Result<()> {
    if !human {
        return Ok(());
    }
    let Some(old) = before.sequence(seq) else { return Ok(()) };
    let Some(q) = project.sequence_mut(seq) else { return Ok(()) };
    let mut changed = Vec::new();
    for tr in &mut q.audio_tracks {
        let previous = old.audio_tracks.iter().find(|t| t.id == tr.id);
        for (lane, field) in [(LANE_VOLUME_OFFSET, "gain_curve"), (LANE_PAN_OFFSET, "pan_curve")] {
            let a = previous.and_then(|t| t.mixer.lanes.get(lane));
            let b = tr.mixer.lanes.get(lane);
            if a == b {
                continue;
            }
            let has_keys = b.is_some_and(|p| !p.keyframes.is_empty());
            if let Some(p) = b.filter(|p| !p.keyframes.is_empty()) {
                let current = basis(p)?;
                if tr.mixer.offset_intents.get(lane).is_none_or(|i| i.basis != current) {
                    tr.mixer.offset_intents.insert(lane.into(), OffsetCurveIntent { by: "human".into(), basis: current });
                }
            } else {
                tr.mixer.offset_intents.remove(lane);
            }
            changed.push((tr.id, field, has_keys));
        }
    }
    if let Some(text) = alias_patch(project, seq, &changed)? {
        project.item_mut(seq).ok_or(EngineError::NoSequence)?.metadata.insert(crate::poema_metadata::KEY.into(), text);
    }
    Ok(())
}

fn alias_patch(project: &Project, seq: ItemId, changed: &[(TrackId, &str, bool)]) -> Result<Option<String>> {
    if changed.is_empty() {
        return Ok(None);
    }
    let item = project.item(seq).ok_or(EngineError::NoSequence)?;
    let Some(text) = item.metadata.get(crate::poema_metadata::KEY) else { return Ok(None) };
    if text.len() > 64 * 1024 {
        return Err(bad(CMD, "editorial metadata exceeds bound"));
    }
    let mut meta: Value = serde_json::from_str(text).map_err(|e| bad(CMD, e.to_string()))?;
    let before = meta.clone();
    let aliases = meta.get_mut("tracks").and_then(Value::as_object_mut).ok_or_else(|| bad(CMD, "invalid track aliases"))?;
    for &(id, field, present) in changed {
        let Some(alias) = aliases.get_mut(&id.0.to_string()) else { continue };
        let fields = alias.get_mut("fields").and_then(Value::as_array_mut).ok_or_else(|| bad(CMD, "invalid track fields"))?;
        if present {
            if !fields.iter().any(|v| v.as_str() == Some(field)) {
                if fields.len() >= 128 {
                    return Err(bad(CMD, "too many track fields"));
                }
                fields.push(json!(field));
            }
        } else {
            fields.retain(|v| v.as_str() != Some(field));
        }
        if field == "gain_curve" {
            fields.retain(|v| !matches!(v.as_str(), Some("ride" | "ride_note" | "ride_db")));
            let props = alias.get_mut("props").and_then(Value::as_object_mut).ok_or_else(|| bad(CMD, "invalid track properties"))?;
            for key in ["ride", "ride_note", "ride_db"] {
                props.remove(key);
            }
        }
    }
    if before == meta {
        return Ok(None);
    }
    let text = serde_json::to_string(&meta).map_err(|e| bad(CMD, e.to_string()))?;
    if text.len() > 64 * 1024 {
        return Err(bad(CMD, "editorial metadata exceeds bound"));
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Session, u64) {
        let mut s = Session::default();
        let mut p = Project::new("synthetic track curves");
        let seq = p.new_sequence("q", Default::default(), 1, 1, None);
        let tr = p.sequence(seq).unwrap().audio_tracks[0].id.0;
        s.project = std::sync::Arc::new(p);
        s.state.active_sequence = Some(seq);
        let q = s.active_sequence().unwrap();
        let meta = json!({"schema":1,"project_id":"p_fixture","sequence_id":"q_fixture","root":{},"settings":{"width":q.settings.width,"height":q.settings.height,"fps":q.settings.frame_rate.as_f64()},"fields":["tracks","settings"],"tracks":{tr.to_string():{"id":"a1","fields":["id","kind","clips","ride","ride_note","ride_db"],"props":{"ride":{"kind":"level"},"ride_note":"computed","ride_db":-3.0}}},"assets":{},"links":{}});
        s.execute("poema.setSequenceMetadata", json!({"metadata":meta})).unwrap();
        (s, tr)
    }
    fn req(track: u64) -> Value {
        json!({"track":track,"param":"gain","by":"fixture_computer","points":[{"time":0,"value":-12.0},{"time":508032000000i64,"value":-3.0}]})
    }
    #[test]
    fn checked_set_repeat_human_keyframe_clear_and_undo_preserve_project_state() {
        let (mut s, track) = fixture();
        let before = s.project.clone();
        let depth = s.history.undo.len();
        let request = req(track);
        s.execute(CMD, request.clone()).unwrap();
        let q = s.active_sequence().unwrap();
        let tr = q.track(TrackId(track)).unwrap();
        assert_eq!(tr.volume_db, 0.0);
        assert_eq!(tr.pan, 0.0);
        let p = &tr.mixer.lanes[LANE_VOLUME_OFFSET];
        assert_eq!(p.keyframes.len(), 2);
        assert_eq!(p.keyframes[0].time.0, 0);
        assert_eq!(p.keyframes[1].time.0, 508032000000);
        assert_eq!(p.keyframes[0].value, ParamValue::Float(-12.0));
        assert_eq!(p.keyframes[1].value, ParamValue::Float(-3.0));
        assert_eq!(tr.mixer.offset_intents[LANE_VOLUME_OFFSET].by, "fixture_computer");
        assert_eq!(tr.mixer.offset_intents[LANE_VOLUME_OFFSET].basis, basis(p).unwrap());
        let seq = s.state.active_sequence.unwrap();
        let meta: Value = serde_json::from_str(&s.project.item(seq).unwrap().metadata[crate::poema_metadata::KEY]).unwrap();
        assert_eq!(meta["tracks"][track.to_string()]["props"], json!({}));
        assert!(meta["tracks"][track.to_string()]["fields"].as_array().unwrap().contains(&json!("gain_curve")));
        let changed = s.project.clone();
        let revision = s.revision;
        assert_eq!(s.history.undo.len(), depth + 1);
        assert_eq!(s.execute(CMD, request).unwrap(), json!({"changed":false}));
        assert_eq!(s.revision, revision);
        assert_eq!(s.project, changed);
        s.execute("mixer.setKeyframe", json!({"strip":track,"lane":LANE_VOLUME_OFFSET,"time":0,"value":-9.0})).unwrap();
        assert_eq!(s.active_sequence().unwrap().track(TrackId(track)).unwrap().mixer.offset_intents[LANE_VOLUME_OFFSET].by, "human");
        s.undo().unwrap();
        assert_eq!(s.project, changed);
        s.execute(CMD, json!({"track":track,"param":"gain","points":[]})).unwrap();
        let tr = s.active_sequence().unwrap().track(TrackId(track)).unwrap();
        assert!(!tr.mixer.lanes.contains_key(LANE_VOLUME_OFFSET));
        assert!(tr.mixer.offset_intents.is_empty());
        let meta: Value = serde_json::from_str(&s.project.item(seq).unwrap().metadata[crate::poema_metadata::KEY]).unwrap();
        assert!(!meta["tracks"][track.to_string()]["fields"].as_array().unwrap().contains(&json!("gain_curve")));
        s.undo().unwrap();
        assert_eq!(s.project, changed);
        s.undo().unwrap();
        assert_eq!(s.project, before);
    }
    #[test]
    fn native_clear_lane_bakes_the_held_offset_and_refuses_unrepresentable_bakes() {
        let (mut s, track) = fixture();
        s.execute(CMD, req(track)).unwrap();
        let before = s.project.clone();
        s.execute("playhead.set", json!({"seconds":0.0})).unwrap();
        s.execute("mixer.clearLane", json!({"strip":track,"lane":LANE_VOLUME_OFFSET})).unwrap();
        let tr = s.active_sequence().unwrap().track(TrackId(track)).unwrap();
        assert_eq!(tr.volume_db, -12.0);
        assert!(!tr.mixer.lanes.contains_key(LANE_VOLUME_OFFSET));
        assert!(!tr.mixer.offset_intents.contains_key(LANE_VOLUME_OFFSET));
        s.undo().unwrap();
        assert_eq!(s.project, before);
        s.execute("timeline.setTrack", json!({"track":track,"pan":20.0})).unwrap();
        s.execute(CMD, json!({"track":track,"param":"pan","points":[{"time":0,"value":-0.5},{"time":508032000000i64,"value":0.5}]})).unwrap();
        let before = s.project.clone();
        s.execute("mixer.clearLane", json!({"strip":track,"lane":LANE_PAN_OFFSET})).unwrap();
        assert_eq!(s.active_sequence().unwrap().track(TrackId(track)).unwrap().pan, -30.0);
        s.undo().unwrap();
        assert_eq!(s.project, before);
        s.execute("timeline.setTrack", json!({"track":track,"volumeDb":15.0})).unwrap();
        s.execute(CMD, json!({"track":track,"param":"gain","points":[{"time":0,"value":6.0},{"time":508032000000i64,"value":12.0}]})).unwrap();
        let before = s.project.clone();
        let revision = s.revision;
        assert!(s.execute("mixer.clearLane", json!({"strip":track,"lane":LANE_VOLUME_OFFSET})).is_err());
        assert_eq!(s.project, before);
        assert_eq!(s.revision, revision);
    }

    #[test]
    fn hostile_inputs_refuse_atomically_and_staged_agent_edits_do_not_claim_human_intent() {
        let (mut s, track) = fixture();
        let before = s.project.clone();
        let revision = s.revision;
        for request in [
            json!({"track":u64::MAX,"param":"gain","points":[]}),
            json!({"track":track,"param":"other","points":[]}),
            json!({"track":track,"param":"pan","points":[{"time":0,"value":-2.0},{"time":1,"value":0.0}]}),
            json!({"track":track,"param":"gain","points":[{"time":0,"value":-6.0}]}),
            json!({"track":track,"param":"gain","by":"","points":[]}),
            json!({"track":track,"param":"gain","points":[{"time":0,"value":-6.0},{"time":0,"value":-3.0}]}),
            json!({"track":track,"param":"gain","points":[{"time":0,"value":-6.0,"outHandle":[0.1,1000001.0]},{"time":1,"value":-3.0}]}),
        ] {
            assert!(s.execute(CMD, request).is_err());
            assert_eq!(s.project, before);
            assert_eq!(s.revision, revision);
        }
        let snapshot = s.execute("agent.snapshot", json!({})).unwrap();
        let applied = s
            .execute("agent.apply", json!({"token":snapshot["token"],"label":"agent track curve","operations":[{"command":CMD,"params":req(track)}]}))
            .unwrap();
        assert_eq!(s.active_sequence().unwrap().track(TrackId(track)).unwrap().mixer.offset_intents[LANE_VOLUME_OFFSET].by, "fixture_computer");
        let snapshot = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":snapshot["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(s.project, before);
    }
}
