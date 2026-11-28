//! Poema audition state belongs to the native owner, outside edit Undo. Track
//! flags remain the renderer's truth; the typed intent survives history swaps.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ItemId, Project, TrackKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

const CMD: &str = "poema.setMonitor";
pub(crate) const KEY: &str = "poema.monitor.v1";
const EDITORIAL: &str = "poema.editorial.v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Flags {
    muted: bool,
    solo: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: u8,
    tracks: BTreeMap<String, Flags>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    track: u64,
    muted: Option<bool>,
    solo: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    updates: Vec<Update>,
}

fn state(project: &Project, sequence: ItemId) -> Result<Option<State>> {
    let Some(text) = project.item(sequence).and_then(|i| i.metadata.get(KEY)) else { return Ok(None) };
    if text.len() > 32768 {
        return Err(bad(CMD, "monitor state exceeds 32 KiB"));
    }
    let value: State = serde_json::from_str(text).map_err(|e| bad(CMD, e.to_string()))?;
    if value.schema != 1 || value.tracks.len() > 256 || value.tracks.keys().any(|key| key.parse::<u64>().is_err()) {
        return Err(bad(CMD, "invalid or excessive monitor state"));
    }
    Ok(Some(value))
}
/// No alternate native setter may silently disagree with audition intent.
/// Deleting a lane keeps its intent dormant for native Undo/Redo.
pub(crate) fn validate(project: &Project) -> Result<()> {
    for id in project.items.keys() {
        if let Some(monitor) = state(project, *id)? {
            let q = project.sequence(*id).ok_or_else(|| bad(CMD, "monitor metadata requires a sequence"))?;
            for track in &q.audio_tracks {
                if let Some(flags) = monitor.tracks.get(&track.id.0.to_string())
                    && (flags.muted != track.muted || flags.solo != track.solo)
                {
                    return Err(bad(CMD, "versioned track flags disagree with native audition; use the monitor command"));
                }
            }
        }
    }
    Ok(())
}
fn tagged(s: &Session) -> bool {
    s.state.active_sequence.and_then(|id| s.project.item(id)).is_some_and(|i| i.metadata.contains_key(EDITORIAL) || i.metadata.contains_key(KEY))
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    let count = params.get("updates").and_then(Value::as_array).map(Vec::len).ok_or_else(|| bad(CMD, "need updates"))?;
    if count == 0 || count > 128 {
        return Err(bad(CMD, "need 1..128 explicit audio monitor updates"));
    }
    let request: Request = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if !tagged(s) {
        return Err(bad(CMD, "native audition requires a Poema sequence"));
    }
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let mut next = (*s.project).clone();
    let mut monitor = state(&next, sequence)?.unwrap_or(State { schema: 1, tracks: BTreeMap::new() });
    let q = next.sequence_mut(sequence).ok_or(crate::EngineError::NoSequence)?;
    let mut changed_tracks = BTreeMap::new();
    for update in request.updates {
        if update.muted.is_none() && update.solo.is_none() {
            return Err(bad(CMD, "monitor update needs muted and/or solo"));
        }
        let track = q.audio_tracks.iter_mut().find(|t| t.id.0 == update.track).ok_or_else(|| bad(CMD, "monitor target must be an audio track"))?;
        if let Some(value) = update.muted {
            track.muted = value;
        }
        if let Some(value) = update.solo {
            track.solo = value;
        }
        let flags = Flags { muted: track.muted, solo: track.solo };
        monitor.tracks.insert(update.track.to_string(), flags.clone());
        changed_tracks.insert(update.track, flags);
    }
    if monitor.tracks.len() > 256 {
        return Err(bad(CMD, "monitor track limit exceeded"));
    }
    q.check().map_err(crate::EngineError::Other)?;
    let encoded = serde_json::to_string(&monitor).map_err(|e| bad(CMD, e.to_string()))?;
    if encoded.len() > 32768 {
        return Err(bad(CMD, "monitor state exceeds 32 KiB"));
    }
    let item = next.item_mut(sequence).ok_or(crate::EngineError::NoSequence)?;
    if item.metadata.get(KEY) != Some(&encoded) {
        item.metadata.insert(KEY.to_string(), encoded);
    }
    let changed = serde_json::to_value(&next).map_err(|e| bad(CMD, e.to_string()))? != serde_json::to_value(&s.project).map_err(|e| bad(CMD, e.to_string()))?;
    if changed {
        let before = s.project.clone();
        let revision = s.revision;
        s.project = Arc::new(next);
        s.history.merge_key = None;
        s.bump();
        crate::agent_bridge::retain_owned_after_monitor(s, &before, revision);
    }
    Ok(
        json!({"changed":changed,"revision":s.revision,"tracks":changed_tracks.into_iter().map(|(id, f)|json!({"track":id,"muted":f.muted,"solo":f.solo})).collect::<Vec<_>>()}),
    )
}

/// Keep the original Arc for ordinary FilmCraft projects. If audition intent
/// exists, validate it BEFORE popping history, then restore its flags/field
/// presence over the old geometry. Missing tracks remain dormant for Redo.
pub(crate) fn restore(current: &Arc<Project>, target: &Arc<Project>) -> Result<Arc<Project>> {
    let mut states = Vec::new();
    for (id, item) in &current.items {
        if item.metadata.contains_key(KEY)
            && let Some(value) = state(current, *id)?
        {
            states.push((*id, value));
        }
    }
    if states.is_empty() {
        return Ok(target.clone());
    }
    let mut next = (**target).clone();
    for (id, monitor) in states {
        let Some(q) = next.sequence_mut(id) else { continue };
        for track in &mut q.audio_tracks {
            if let Some(flags) = monitor.tracks.get(&track.id.0.to_string()) {
                track.muted = flags.muted;
                track.solo = flags.solo;
            }
        }
        q.check().map_err(crate::EngineError::Other)?;
        let encoded = serde_json::to_string(&monitor).map_err(|e| bad(CMD, e.to_string()))?;
        if let Some(item) = next.item_mut(id) {
            item.metadata.insert(KEY.to_string(), encoded);
        }
    }
    Ok(Arc::new(next))
}

/// Native GUI audio M/S buttons use this same non-history door on Poema
/// sequences. General/mixed versioned M/S on a monitored sequence is refused
/// until that separate composition is proved; it cannot corrupt Undo intent.
pub(crate) fn command(s: &mut Session, id: &str, params: &Value) -> Option<Result<Value>> {
    let address = match id {
        "timeline.setTrack" => "track",
        "mixer.setStrip" => "strip",
        _ => return None,
    };
    if !tagged(s) || params.get("muted").is_none() && params.get("solo").is_none() {
        return None;
    }
    let pure = params.as_object().is_some_and(|p| p.keys().all(|key| [address, "muted", "solo"].contains(&key.as_str())));
    let sequence = s.state.active_sequence?;
    let monitored = match state(&s.project, sequence) {
        Ok(value) => value.is_some(),
        Err(e) => return Some(Err(e)),
    };
    if s.agent_bridge.staging || !pure {
        return if monitored { Some(Err(bad(CMD, "mixed/versioned M/S on an auditioned sequence needs its separate history proof"))) } else { None };
    }
    let resolved = crate::commands::track_p(s, params, address, CMD);
    let track = match resolved {
        Ok(Some(track)) => track,
        Ok(None) => return Some(Err(bad(CMD, "need audio track"))),
        Err(e) => return Some(Err(e)),
    };
    if !s.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|t| t.id == track && t.kind == TrackKind::Audio)) {
        return None;
    }
    let mut update = serde_json::Map::new();
    update.insert("track".to_string(), json!(track.0));
    for key in ["muted", "solo"] {
        if let Some(value) = params.get(key) {
            update.insert(key.to_string(), value.clone());
        }
    }
    Some(set(s, &json!({"updates":[update]})))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo(tag: bool) -> Session {
        let mut s = Session::default();
        let (mut p, id) = crate::demo::demo_project(&s.media);
        if tag {
            p.item_mut(id).unwrap().metadata.insert(EDITORIAL.into(), "{}".into());
        }
        s.project = Arc::new(p);
        s.state.active_sequence = Some(id);
        s
    }
    fn track(s: &Session) -> u64 {
        s.active_sequence().unwrap().audio_tracks[0].id.0
    }
    fn flags(s: &Session) -> (bool, bool) {
        let t = &s.active_sequence().unwrap().audio_tracks[0];
        (t.muted, t.solo)
    }
    fn snapshot(s: &mut Session) -> Value {
        s.execute("agent.snapshot", json!({})).unwrap()
    }
    fn apply(s: &mut Session) -> Value {
        let basis = snapshot(s);
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        s.execute("agent.apply", json!({"token":basis["token"],"operations":[{"command":"poema.patchClip","params":{"clip":c,"gainDb":-6.0}}]})).unwrap()
    }
    #[test]
    fn audition_invalidates_reads_but_not_owned_history_and_survives_both_swaps() {
        let mut s = demo(true);
        let t = track(&s);
        let before_depth = s.history.undo.len();
        let applied = apply(&mut s);
        let basis = snapshot(&mut s);
        let (_, events, _subscription) = s.command_journal.subscribe().unwrap();
        s.execute("agent.monitor", json!({"token":basis["token"],"updates":[{"track":t,"muted":true}]})).unwrap();
        assert_eq!(s.history.undo.len(), before_depth + 1);
        assert_eq!(flags(&s), (true, false));
        assert!(s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).is_err());
        let basis = snapshot(&mut s);
        s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(flags(&s), (true, false));
        assert_eq!(s.history.undo.len(), before_depth);
        s.execute("mixer.setStrip", json!({"strip":t,"muted":false,"solo":true})).unwrap();
        let basis = snapshot(&mut s);
        s.execute("agent.redo", json!({"token":basis["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(flags(&s), (false, true));
        assert_eq!(s.history.undo.len(), before_depth + 1);
        assert_eq!(
            events.try_iter().map(|e| e["command"].as_str().unwrap().to_owned()).collect::<Vec<_>>(),
            ["agent.monitor", "agent.undo", "mixer.setStrip", "agent.redo"]
        );
        let rev = s.revision;
        s.execute("timeline.setTrack", json!({"track":t,"solo":true})).unwrap();
        assert_eq!(s.revision, rev); // no event and no history for an exact no-op
        assert_eq!(s.history.undo.len(), before_depth + 1);
    }
    #[test]
    fn hostile_or_mixed_updates_are_atomic_and_never_reanimate_stale_undo_ownership() {
        let mut s = demo(true);
        let t = track(&s);
        let applied = apply(&mut s);
        for updates in [
            json!([]),
            json!([{"track":t,"muted":true},{"track":0,"solo":true}]),
            json!([{"track":t}]),
            json!([{"track":t,"muted":null}]),
            json!([{"track":t,"muted":"yes"}]),
            json!([{"track":t,"wat":true}]),
            Value::Array(vec![json!({"track":t,"solo":true}); 129]),
        ] {
            let before = s.project.clone();
            let rev = s.revision;
            let depth = s.history.undo.len();
            assert!(s.execute(CMD, json!({"updates":updates})).is_err());
            assert!(Arc::ptr_eq(&s.project, &before));
            assert_eq!(s.revision, rev);
            assert_eq!(s.history.undo.len(), depth);
        }
        s.execute("timeline.setTrack", json!({"track":t,"volumeDb":-2.0})).unwrap(); // human edit invalidates owned Undo
        s.execute(CMD, json!({"updates":[{"track":t,"muted":true}]})).unwrap();
        let basis = snapshot(&mut s);
        assert!(s.execute("agent.undo", json!({"token":basis["token"],"commit":applied["commit"]})).is_err());
        let before = s.project.clone();
        let audio = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
        assert!(s.execute("poema.configureClip", json!({"clip":audio,"trackEnabled":true})).is_err());
        assert!(s.execute("timeline.setTrack", json!({"track":t,"solo":true,"volumeDb":-4.0})).is_err());
        let basis = snapshot(&mut s);
        assert!(
            s.execute("agent.apply", json!({"token":basis["token"],"operations":[{"command":"timeline.setTrack","params":{"track":t,"solo":true}}]})).is_err()
        );
        assert!(Arc::ptr_eq(&s.project, &before));
    }
    #[test]
    fn malformed_persisted_intent_refuses_without_consuming_history() {
        let mut s = demo(true);
        apply(&mut s);
        let id = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.insert(KEY.into(), r#"{"schema":9,"tracks":{}}"#.into());
        let before = s.project.clone();
        let depth = s.history.undo.len();
        let rev = s.revision;
        assert!(s.undo().is_none());
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(s.history.undo.len(), depth);
        assert_eq!(s.revision, rev);
    }
    #[test]
    fn ordinary_native_mute_remains_a_versioned_edit_with_original_arc_undo() {
        let mut s = demo(false);
        let t = track(&s);
        let before = s.project.clone();
        let depth = s.history.undo.len();
        assert!(s.execute(CMD, json!({"updates":[{"track":t,"muted":true}]})).is_err());
        s.execute("timeline.setTrack", json!({"track":t,"muted":true})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        assert_eq!(flags(&s), (true, false));
        s.undo().unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(flags(&s), (false, false));
    }
    #[test]
    fn cancelling_a_native_gesture_retains_audition_and_corrupt_intent_keeps_history() {
        let mut s = demo(true);
        let t = track(&s);
        let mark = s.undo_mark();
        apply(&mut s);
        s.execute(CMD, json!({"updates":[{"track":t,"solo":true}]})).unwrap();
        assert_eq!(s.revert_to(mark), 1);
        assert_eq!(flags(&s), (false, true));
        let mark = s.undo_mark();
        apply(&mut s);
        let depth = s.history.undo.len();
        let id = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.insert(KEY.into(), "corrupt".into());
        assert_eq!(s.revert_to(mark), 0);
        assert_eq!(s.history.undo.len(), depth);
    }
    #[test]
    fn monitor_intent_is_self_contained_and_survives_serialization_and_history() {
        let mut s = demo(true);
        let t = track(&s);
        s.execute(CMD, json!({"updates":[{"track":t,"solo":true}]})).unwrap();
        let encoded = serde_json::to_vec(&s.project).unwrap();
        let project: Project = serde_json::from_slice(&encoded).unwrap();
        s.project = Arc::new(project);
        apply(&mut s);
        s.undo().unwrap();
        assert_eq!(flags(&s), (false, true));
        assert_eq!(state(&s.project, s.state.active_sequence.unwrap()).unwrap().unwrap().tracks[&t.to_string()], Flags { muted: false, solo: true });
    }
}
