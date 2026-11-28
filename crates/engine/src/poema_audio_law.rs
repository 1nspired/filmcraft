//! Mixer-law selection is native project state, never a metadata audio overlay.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{MonoSourceLaw, StereoBalanceLaw, TrackId, TrackKind};
use serde::Deserialize;
use serde_json::{Value, json};
const CMD: &str = "poema.setAudioLaw";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    track: u64,
    stereo_balance: StereoBalanceLaw,
    mono_source: MonoSourceLaw,
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let r: Request = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    let id = TrackId(r.track);
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let track = q.track(id).ok_or_else(|| bad(CMD, "unknown track"))?;
    if track.kind != TrackKind::Audio || track.locked {
        return Err(bad(CMD, "mixer laws require an unlocked audio track"));
    }
    if track.mixer.stereo_balance == r.stereo_balance && track.mixer.mono_source == r.mono_source {
        return Ok(json!({"changed":false}));
    }
    s.edit_sequence("Set audio mixer laws", |q, _, _| {
        let track = q.track_mut(id).ok_or_else(|| bad(CMD, "unknown track"))?;
        track.mixer.stereo_balance = r.stereo_balance;
        track.mixer.mono_source = r.mono_source;
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
    fn explicit_laws_share_native_batch_noop_and_undo() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].id.0;
        let before = serde_json::to_value(&s.project).unwrap();
        let depth = s.history.undo.len();
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let p = json!({"track":id,"stereoBalance":"linear","monoSource":"equalPower"});
        let apply = s.execute("agent.apply", json!({"token":basis["token"],"operations":[{"command":CMD,"params":p}]})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        let rev = s.revision;
        s.execute(CMD, p).unwrap();
        assert_eq!(s.revision, rev);
        let track = &s.active_sequence().unwrap().audio_tracks[0];
        assert_eq!(track.mixer.stereo_balance, StereoBalanceLaw::Linear);
        assert_eq!(track.mixer.mono_source, MonoSourceLaw::EqualPower);
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":basis["token"],"commit":apply["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
    }
    #[test]
    fn saved_laws_round_trip_and_old_strips_keep_native_defaults() {
        let old: filmcraft_project::MixerStrip = serde_json::from_value(json!({})).unwrap();
        assert_eq!(old.stereo_balance, StereoBalanceLaw::EqualPower);
        assert_eq!(old.mono_source, MonoSourceLaw::DualMono);
        let encoded = serde_json::to_value(&old).unwrap();
        assert!(encoded.get("stereo_balance").is_none());
        assert!(encoded.get("mono_source").is_none());
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].id.0;
        s.execute(CMD, json!({"track":id,"stereoBalance":"linear","monoSource":"equalPower"})).unwrap();
        let saved = serde_json::to_value(&s.project).unwrap();
        let restored: filmcraft_project::Project = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), saved);
        let mut locked = s.active_sequence().unwrap().clone();
        locked.audio_tracks[0].locked = true;
        let active = s.state.active_sequence.unwrap();
        *Arc::make_mut(&mut s.project).sequence_mut(active).unwrap() = locked;
        let before = s.project.clone();
        assert!(s.execute(CMD, json!({"track":id,"stereoBalance":"equalPower","monoSource":"dualMono"})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn hostile_laws_and_picture_targets_refuse_without_mutation() {
        let mut s = demo();
        let id = s.active_sequence().unwrap().audio_tracks[0].id.0;
        let video = s.active_sequence().unwrap().video_tracks[0].id.0;
        for p in [
            json!({"track":id}),
            json!({"track":id,"stereoBalance":"other","monoSource":"equalPower"}),
            json!({"track":id,"stereoBalance":"linear","monoSource":1}),
            json!({"track":id,"stereoBalance":"linear","monoSource":"equalPower","gain":-3}),
            json!({"track":video,"stereoBalance":"linear","monoSource":"equalPower"}),
        ] {
            let before = s.project.clone();
            let rev = s.revision;
            assert!(s.execute(CMD, p).is_err());
            assert!(Arc::ptr_eq(&before, &s.project));
            assert_eq!(s.revision, rev);
        }
    }
}
