//! The Poema stereo-centre switch is the native Channel Mixer, not an
//! annotation pretending to process audio. Unrelated mixer settings refuse.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, EffectInstance, ParamValue, TrackKind, find_effect};
use serde::Deserialize;
use serde_json::{Value, json};
const CMD: &str = "poema.setChannelSum";
const EFFECT: &str = "channel_mixer_a";
fn average() -> Result<EffectInstance> {
    let mut effect = find_effect(EFFECT).ok_or_else(|| bad(CMD, "native Channel Mixer definition unavailable"))?.instance();
    for key in ["l_from_l", "l_from_r", "r_from_l", "r_from_r"] {
        effect.param_mut(key).ok_or_else(|| bad(CMD, "native Channel Mixer coefficients unavailable"))?.value = ParamValue::Float(50.0);
    }
    Ok(effect)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    clip: u64,
    enabled: bool,
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let request: Request = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    let id = ClipId(request.clip);
    let expected = average()?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (tid, item) = q.find_item(id).ok_or_else(|| bad(CMD, "unknown clip"))?;
    let track = q.track(tid).ok_or_else(|| bad(CMD, "unknown track"))?;
    if track.kind != TrackKind::Audio {
        return Err(bad(CMD, "channel sum requires an explicit audio clip"));
    }
    if track.locked {
        return Err(bad(CMD, "audio track is locked"));
    }
    if item.effects.len() > 128 {
        return Err(bad(CMD, "clip effect limit exceeded"));
    }
    if item.audio_stream != 0 || !item.source_channels.is_empty() {
        return Err(bad(CMD, "nondefault stream/channel mapping needs its separate sum proof"));
    }
    let channels =
        s.project.resolve_media(item.item).and_then(|(_, m, _)| m.info.audio()).map(|a| a.channels).ok_or_else(|| bad(CMD, "source has no audio"))?;
    if channels != 2 {
        return Err(bad(CMD, "channel sum currently requires a stereo source"));
    }
    let mixers: Vec<_> = item.effects.iter().filter(|e| e.effect == EFFECT).collect();
    if mixers.len() > 1 || mixers.first().is_some_and(|e| **e != expected) {
        return Err(bad(CMD, "existing Channel Mixer is not the exact static stereo-centre effect"));
    }
    if request.enabled != mixers.is_empty() {
        return Ok(json!({"changed":false}));
    }
    if request.enabled && item.effects.len() >= 128 {
        return Err(bad(CMD, "clip effect limit exceeded"));
    }
    s.edit_sequence("Set stereo channel sum", |q, _, _| {
        let (_, item) = q.find_item_mut(id).ok_or_else(|| bad(CMD, "unknown clip"))?;
        if request.enabled {
            item.effects.push(expected);
        } else {
            item.effects.retain(|e| e.effect != EFFECT);
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
    fn clip(s: &Session) -> u64 {
        s.active_sequence().unwrap().audio_tracks[0].items[0].id.0
    }
    #[test]
    fn exact_native_channel_mixer_shares_one_guarded_batch_and_undo() {
        let mut s = demo();
        let id = clip(&s);
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let applied = s.execute("agent.apply", json!({"token":basis["token"],"operations":[{"command":CMD,"params":{"clip":id,"enabled":true}}]})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        let item = s.active_sequence().unwrap().find_item(ClipId(id)).unwrap().1;
        let effect = item.effects.last().unwrap();
        assert_eq!(effect.effect, EFFECT);
        for key in ["l_from_l", "l_from_r", "r_from_l", "r_from_r"] {
            assert_eq!(effect.f64_at(key, filmcraft_time::Tick::ZERO), 50.0);
        }
        assert!(!effect.is_animated());
        assert!(effect.masks.is_empty());
        let rev = s.revision;
        s.execute(CMD, json!({"clip":id,"enabled":true})).unwrap();
        assert_eq!(s.revision, rev);
        assert_eq!(s.history.undo.len(), depth + 1);
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    }
    #[test]
    fn mapped_streams_and_locked_audio_refuse_without_partial_changes() {
        for mode in 0..3 {
            let mut s = demo();
            let id = clip(&s);
            let seq = s.state.active_sequence.unwrap();
            let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            let tid = q.find_item(ClipId(id)).unwrap().0;
            if mode == 2 {
                q.track_mut(tid).unwrap().locked = true;
            } else {
                let item = q.find_item_mut(ClipId(id)).unwrap().1;
                if mode == 0 {
                    item.audio_stream = 1;
                } else {
                    item.source_channels = vec![0];
                }
            }
            let before = s.project.clone();
            let rev = s.revision;
            let depth = s.history.undo.len();
            assert!(s.execute(CMD, json!({"clip":id,"enabled":true})).is_err());
            assert!(Arc::ptr_eq(&before, &s.project));
            assert_eq!(s.revision, rev);
            assert_eq!(s.history.undo.len(), depth);
        }
    }
    #[test]
    fn hostile_channel_sum_and_unrelated_mixer_settings_refuse_atomically() {
        let mut s = demo();
        let id = clip(&s);
        let video = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        for params in [
            json!({"clip":id}),
            json!({"clip":id,"enabled":null}),
            json!({"clip":id,"enabled":1}),
            json!({"clip":id,"enabled":true,"weights":[0,1]}),
            json!({"clip":0,"enabled":true}),
            json!({"clip":video,"enabled":true}),
        ] {
            let before = s.project.clone();
            let rev = s.revision;
            let depth = s.history.undo.len();
            assert!(s.execute(CMD, params).is_err());
            assert!(Arc::ptr_eq(&before, &s.project));
            assert_eq!(s.revision, rev);
            assert_eq!(s.history.undo.len(), depth);
        }
        let seq = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
        q.find_item_mut(ClipId(id)).unwrap().1.effects.push(find_effect(EFFECT).unwrap().instance());
        let before = s.project.clone();
        assert!(s.execute(CMD, json!({"clip":id,"enabled":true})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
