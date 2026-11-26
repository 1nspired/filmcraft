//! Shared native strip/send project services for human and exact agent callers.
//! Preparation retains the original human parameter defaults and clamping.
//! This is not authorization: typed callers still validate exact scope, locks,
//! fields, values and observed targets. No history, persistence, preview, device,
//! preference or source ownership is created here.
use super::{FADER_MAX_DB, FADER_MIN_DB, InputMap, MASTER_STRIP, TrackSend, bad, bool_p, channels_from, f64_p, mode_p, pm, str_p, strip_ref, u64_p};
use crate::{EngineError, Result};
use filmcraft_project::{Sequence, TrackId};
use serde_json::Value;

pub(crate) struct StripChange {
    strip: TrackId,
    params: Value,
    output: Option<Option<TrackId>>,
}
pub(crate) fn prepare_strip(seq: &Sequence, id: TrackId, p: &Value) -> Result<StripChange> {
    let output = match p.get("output") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            let o = strip_ref(seq, v).ok_or_else(|| bad("mixer.setStrip", format!("no output {v}")))?;
            if o == MASTER_STRIP {
                Some(None)
            } else if o == id || !seq.submix_tracks.iter().any(|t| t.id == o) {
                return Err(bad("mixer.setStrip", "output must be the Mix or a submix"));
            } else if seq
                .submix_tracks
                .iter()
                .position(|t| t.id == id)
                .is_some_and(|me| seq.submix_tracks.iter().position(|t| t.id == o).is_some_and(|it| it <= me))
            {
                return Err(bad("mixer.setStrip", "a submix can only feed a submix after it (no feedback)"));
            } else {
                Some(Some(o))
            }
        }
    };
    Ok(StripChange { strip: id, params: p.clone(), output })
}
impl StripChange {
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<()> {
        let Self { strip: id, params: p, output } = self;
        q.with_strip_mut(id, |t| {
            if let Some(v) = str_p(&p, "name") {
                t.name = v.to_string();
            }
            if let Some(v) = f64_p(&p, "volumeDb") {
                t.volume_db = v.clamp(FADER_MIN_DB, FADER_MAX_DB);
            }
            if let Some(v) = f64_p(&p, "pan") {
                t.pan = v.clamp(-100.0, 100.0);
            }
            if let Some(v) = bool_p(&p, "muted") {
                t.muted = v;
            }
            if let Some(v) = bool_p(&p, "solo") {
                t.solo = v;
            }
            if let Some(v) = bool_p(&p, "recordArm") {
                t.mixer.record_arm = v;
            }
            if let Some(v) = bool_p(&p, "soloSafe") {
                t.mixer.solo_safe = v;
            }
            if let Some(m) = mode_p(&p) {
                t.mixer.mode = m;
            }
            if let Some(m) = str_p(&p, "inputMap").and_then(InputMap::from_name) {
                t.mixer.input_map = m;
            }
            if let Some(c) = str_p(&p, "channels").and_then(channels_from) {
                t.channels = c;
            }
            if let Some(o) = output {
                t.mixer.output = o;
            }
        })
        .ok_or_else(|| EngineError::Other("no such strip".into()))
    }
}

fn send_target(seq: &Sequence, p: &Value, id: TrackId, cmd: &str) -> Result<TrackId> {
    let v = p.get("target").ok_or_else(|| bad(cmd, "need `target` (a submix)"))?;
    let t = strip_ref(seq, v).filter(|t| seq.submix_tracks.iter().any(|x| x.id == *t)).ok_or_else(|| bad(cmd, "`target` must be a submix"))?;
    let pos = |x: TrackId| seq.submix_tracks.iter().position(|y| y.id == x);
    if t == id || pos(id).is_some_and(|me| pos(t).is_some_and(|it| it <= me)) {
        return Err(bad(cmd, "a submix can only send to a submix after it (no feedback)"));
    }
    Ok(t)
}

pub(crate) struct AddSendChange {
    strip: TrackId,
    send: TrackSend,
}
pub(crate) fn prepare_add_send(seq: &Sequence, id: TrackId, p: &Value) -> Result<AddSendChange> {
    if id == MASTER_STRIP {
        return Err(bad("mixer.addSend", "the Mix track has no sends"));
    }
    let target = send_target(seq, p, id, "mixer.addSend")?;
    let mut snd = TrackSend::new(target);
    snd.level_db = f64_p(p, "levelDb").unwrap_or(0.0).clamp(FADER_MIN_DB, FADER_MAX_DB);
    snd.pre_fader = bool_p(p, "preFader").unwrap_or(false);
    snd.pan = f64_p(p, "pan").unwrap_or(0.0).clamp(-100.0, 100.0);
    if seq.strip(id).is_some_and(|t| t.mixer.sends.len() >= pm::MAX_SENDS) {
        return Err(bad("mixer.addSend", format!("all {} send slots are in use", pm::MAX_SENDS)));
    }
    Ok(AddSendChange { strip: id, send: snd })
}
impl AddSendChange {
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<usize> {
        let Self { strip: id, send: snd } = self;
        q.with_strip_mut(id, |t| {
            t.mixer.sends.push(snd);
            t.mixer.sends.len() - 1
        })
        .ok_or_else(|| EngineError::Other("no such strip".into()))
    }
}

pub(crate) struct SetSendChange {
    strip: TrackId,
    index: usize,
    params: Value,
    target: Option<TrackId>,
}
pub(crate) fn prepare_set_send(seq: &Sequence, id: TrackId, p: &Value) -> Result<SetSendChange> {
    let i = u64_p(p, "send")?.ok_or_else(|| bad("mixer.setSend", "need `send` (0-based)"))?;
    let i = usize::try_from(i).map_err(|_| bad("mixer.setSend", "`send` exceeds the native index range"))?;
    let target = if p.get("target").is_some() { Some(send_target(seq, p, id, "mixer.setSend")?) } else { None };
    Ok(SetSendChange { strip: id, index: i, params: p.clone(), target })
}
impl SetSendChange {
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<()> {
        let Self { strip: id, index: i, params: p, target } = self;
        q.with_strip_mut(id, |t| -> Result<()> {
            let sd = t.mixer.sends.get_mut(i).ok_or_else(|| bad("mixer.setSend", "no such send"))?;
            if let Some(v) = f64_p(&p, "levelDb") {
                sd.level_db = v.clamp(FADER_MIN_DB, FADER_MAX_DB);
            }
            if let Some(v) = f64_p(&p, "pan") {
                sd.pan = v.clamp(-100.0, 100.0);
            }
            if let Some(v) = bool_p(&p, "preFader") {
                sd.pre_fader = v;
            }
            if let Some(v) = bool_p(&p, "muted") {
                sd.muted = v;
            }
            if let Some(t) = target {
                sd.target = t;
            }
            Ok(())
        })
        .ok_or_else(|| EngineError::Other("no such strip".into()))?
    }
}

pub(crate) struct RemoveSendChange {
    strip: TrackId,
    index: usize,
}
pub(crate) fn prepare_remove_send(id: TrackId, p: &Value) -> Result<RemoveSendChange> {
    let i = u64_p(p, "send")?.ok_or_else(|| bad("mixer.removeSend", "need `send` (0-based)"))?;
    let i = usize::try_from(i).map_err(|_| bad("mixer.removeSend", "`send` exceeds the native index range"))?;
    Ok(RemoveSendChange { strip: id, index: i })
}
impl RemoveSendChange {
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<()> {
        let Self { strip: id, index: i } = self;
        if !q.with_strip_mut(id, |t| t.remove_send(i)).unwrap_or(false) {
            return Err(bad("mixer.removeSend", "no such send"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use filmcraft_time::Tick;
    use serde_json::json;
    use std::sync::Arc;
    fn setup() -> (Session, TrackId, TrackId, TrackId) {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let a = s.active_sequence().unwrap().audio_tracks[0].id;
        let one = TrackId(s.execute("mixer.addSubmix", json!({"name":"First bus"})).unwrap()["id"].as_u64().unwrap());
        let two = TrackId(s.execute("mixer.addSubmix", json!({"name":"Second bus"})).unwrap()["id"].as_u64().unwrap());
        (s, a, one, two)
    }
    fn state(s: &Session) -> Value {
        json!({"project":s.project,"editor":s.state,"revision":s.revision,"journal":s.journal,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key})
    }
    fn key(s: &mut Session, strip: TrackId, lane: &str, time: i64, value: f64) {
        s.execute("mixer.setKeyframe", json!({"strip":strip.0,"lane":lane,"time":time,"value":value})).unwrap();
    }
    #[test]
    fn shared_mixer_preparation_is_readonly_and_owns_its_original_strip_target() {
        let (s, a, one, _) = setup();
        let before = state(&s);
        let project = s.project.clone();
        let change = prepare_strip(s.active_sequence().unwrap(), a, &json!({"volumeDb":-9.0,"inputMap":"Left","output":one.0})).unwrap();
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
        let mut q = s.active_sequence().unwrap().clone();
        change.apply(&mut q).unwrap();
        let target = q.mix_track(a).unwrap();
        assert_eq!(target.volume_db, -9.0);
        assert_eq!(target.mixer.input_map, InputMap::Left);
        assert_eq!(target.mixer.output, Some(one));
        assert_eq!(state(&s), before);
        assert_eq!(q.mix_track(one), s.active_sequence().unwrap().mix_track(one));
    }
    #[test]
    fn shared_mixer_human_strip_fields_clamps_defaults_and_single_undo_remain_native() {
        let (mut s, a, one, _) = setup();
        let before = s.project.to_json();
        let editor = serde_json::to_value(&s.state).unwrap();
        let original = s.active_sequence().unwrap().mix_track(a).unwrap().clone();
        let count = s.history.undo.len();
        s.execute("mixer.setStrip",json!({"strip":a.0,"name":"Source sound","volumeDb":900.0,"pan":-900.0,"muted":true,"solo":true,"recordArm":true,"soloSafe":true,"mode":"Touch","output":one.0,"inputMap":"Left","channels":"Mono"})).unwrap();
        let t = s.active_sequence().unwrap().mix_track(a).unwrap();
        assert_eq!(t.name, "Source sound");
        assert_eq!(t.volume_db, FADER_MAX_DB);
        assert_eq!(t.pan, -100.0);
        assert!(t.muted && t.solo && t.mixer.record_arm && t.mixer.solo_safe);
        assert_eq!(t.mixer.mode, filmcraft_project::AutomationMode::Touch);
        assert_eq!(t.mixer.output, Some(one));
        assert_eq!(t.mixer.input_map, InputMap::Left);
        assert_eq!(t.channels, filmcraft_project::AudioChannels::Mono);
        assert_eq!(t.items, original.items);
        assert_eq!(t.effects, original.effects);
        assert_eq!(t.transitions, original.transitions);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
        assert_eq!(s.history.undo.len(), count + 1);
        assert_eq!(s.history.undo.last().unwrap().0, "Track Mixer Settings");
        let after = s.project.to_json();
        s.undo().unwrap();
        assert_eq!(s.project.to_json(), before);
        s.redo().unwrap();
        assert_eq!(s.project.to_json(), after);
        s.execute("mixer.setStrip", json!({"strip":a.0,"mode":"unknown","inputMap":"unknown","channels":"unknown","output":null})).unwrap();
        let t = s.active_sequence().unwrap().mix_track(a).unwrap();
        assert_eq!(t.mixer.mode, filmcraft_project::AutomationMode::Touch);
        assert_eq!(t.mixer.input_map, InputMap::Left);
        assert_eq!(t.channels, filmcraft_project::AudioChannels::Mono);
        assert_eq!(t.mixer.output, None);
    }
    #[test]
    fn shared_mixer_master_keeps_original_persistence_boundary_without_fake_track_fields() {
        let (mut s, _, _, _) = setup();
        let original = s.active_sequence().unwrap().clone();
        s.execute(
            "mixer.setStrip",
            json!({"strip":"Mix","name":"Not persisted","pan":22.0,"muted":true,"solo":true,"channels":"Mono","volumeDb":-8.0,"mode":"Off"}),
        )
        .unwrap();
        let q = s.active_sequence().unwrap();
        let master = q.master_strip();
        assert_eq!(q.master_volume_db, -8.0);
        assert_eq!(q.master_mixer.mode, filmcraft_project::AutomationMode::Off);
        assert_eq!(master.name, "Mix");
        assert_eq!(master.pan, 0.0);
        assert!(!master.muted && !master.solo);
        assert_eq!(q.settings.audio_master, original.settings.audio_master);
        assert_eq!(master.channels, original.settings.audio_master);
        assert_eq!(q.audio_tracks, original.audio_tracks);
        assert_eq!(q.submix_tracks, original.submix_tracks);
    }
    #[test]
    fn shared_mixer_send_static_settings_keep_owned_keys_other_sends_and_native_history() {
        let (mut s, a, one, two) = setup();
        s.execute("mixer.addSend", json!({"strip":a.0,"target":one.0,"levelDb":-4.0,"pan":12.0,"preFader":true})).unwrap();
        s.execute("mixer.addSend", json!({"strip":a.0,"target":two.0,"levelDb":-9.0})).unwrap();
        key(&mut s, a, "send.0.level", 0, -3.0);
        key(&mut s, a, "send.0.level", 254016000000, -7.0);
        let before = s.project.to_json();
        let row = s.active_sequence().unwrap().mix_track(a).unwrap().clone();
        let count = s.history.undo.len();
        s.execute("mixer.setSend", json!({"strip":a.0,"send":0,"target":two.0,"levelDb":-5.0,"pan":-25.0,"preFader":false,"muted":true})).unwrap();
        let t = s.active_sequence().unwrap().mix_track(a).unwrap();
        let send = &t.mixer.sends[0];
        assert_eq!(send.target, two);
        assert_eq!(send.level_db, -5.0);
        assert_eq!(send.pan, -25.0);
        assert!(!send.pre_fader && send.muted);
        assert_eq!(t.mixer.sends[1], row.mixer.sends[1]);
        assert_eq!(t.mixer.lanes, row.mixer.lanes);
        assert_eq!(t.lane_value("send.0.level", Tick::ZERO), -3.0);
        assert_eq!(t.items, row.items);
        assert_eq!(s.history.undo.len(), count + 1);
        assert_eq!(s.history.undo.last().unwrap().0, "Send Settings");
        let after = s.project.to_json();
        s.undo().unwrap();
        assert_eq!(s.project.to_json(), before);
        s.redo().unwrap();
        assert_eq!(s.project.to_json(), after);
    }
    #[test]
    fn shared_mixer_send_removal_renumbers_full_automation_payload_and_restores_both_directions() {
        let (mut s, a, one, two) = setup();
        for id in [one, two] {
            s.execute("mixer.addSend", json!({"strip":a.0,"target":id.0})).unwrap();
        }
        key(&mut s, a, "send.0.level", 0, -3.0);
        key(&mut s, a, "send.1.level", 0, -9.0);
        key(&mut s, a, "send.1.level", 254016000000, -11.0);
        key(&mut s, a, "volume", 0, -6.0);
        let before = s.project.to_json();
        let t = s.active_sequence().unwrap().mix_track(a).unwrap().clone();
        let editor = serde_json::to_value(&s.state).unwrap();
        let count = s.history.undo.len();
        s.execute("mixer.removeSend", json!({"strip":a.0,"send":0})).unwrap();
        let after = s.active_sequence().unwrap().mix_track(a).unwrap();
        assert_eq!(after.mixer.sends, vec![t.mixer.sends[1].clone()]);
        assert_eq!(after.mixer.lanes["send.0.level"], t.mixer.lanes["send.1.level"]);
        assert!(!after.mixer.lanes.contains_key("send.1.level"));
        assert_eq!(after.mixer.lanes["volume"], t.mixer.lanes["volume"]);
        assert_eq!(after.items, t.items);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
        assert_eq!(s.history.undo.len(), count + 1);
        let final_project = s.project.to_json();
        s.undo().unwrap();
        assert_eq!(s.project.to_json(), before);
        s.redo().unwrap();
        assert_eq!(s.project.to_json(), final_project);
        assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project.to_json(), final_project);
    }
    #[test]
    fn shared_mixer_invalid_graph_capacity_and_send_indices_preserve_the_human_redo_branch() {
        let (mut s, a, one, two) = setup();
        while s.active_sequence().unwrap().mix_track(a).unwrap().mixer.sends.len() < filmcraft_project::mixer::MAX_SENDS {
            s.execute("mixer.addSend", json!({"strip":a.0,"target":one.0})).unwrap();
        }
        s.execute("mixer.setStrip", json!({"strip":a.0,"pan":-30.0})).unwrap();
        s.undo().unwrap();
        s.history.merge_key = Some("Existing human gesture".into());
        for (cmd, args) in [
            ("mixer.setStrip", json!({"strip":two.0,"output":one.0})),
            ("mixer.setStrip", json!({"strip":one.0,"output":one.0})),
            ("mixer.setStrip", json!({"strip":one.0,"output":a.0})),
            ("mixer.addSend", json!({"strip":"Mix","target":one.0})),
            ("mixer.addSend", json!({"strip":two.0,"target":one.0})),
            ("mixer.addSend", json!({"strip":a.0,"target":one.0})),
            ("mixer.setSend", json!({"strip":a.0,"send":u64::MAX,"target":one.0})),
            ("mixer.removeSend", json!({"strip":a.0,"send":u64::MAX})),
            ("mixer.setSend", json!({"strip":two.0,"send":0,"target":one.0})),
        ] {
            let before = state(&s);
            let project = s.project.clone();
            let undo = s.history.undo.clone();
            let redo = s.history.redo.clone();
            assert!(s.execute(cmd, args).is_err(), "{cmd}");
            assert_eq!(state(&s), before);
            assert!(Arc::ptr_eq(&project, &s.project));
            for (old, new) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
                assert_eq!(old.len(), new.len());
                assert!(old.iter().zip(new).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
            }
        }
    }
}
