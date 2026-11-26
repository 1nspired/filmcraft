//! Authored track inventory and exact structural gestures over shared human services.
use super::{Operation, parse, project_audit, sequences};
use crate::{
    EditorState, EngineError, Result, Session,
    commands::bad,
    track_ops::{self, Kind},
};
use filmcraft_project::{AudioChannels, CaptionFormat, Label, Sequence, Track, TrackId, TrackKind};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.track";
const SAFE: u64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn text(s: &str, n: usize) -> bool {
    s.len() <= n && !s.contains('\0')
}
fn audio(s: &str) -> bool {
    matches!(s, "Mono" | "Stereo" | "5.1" | "Adaptive")
}
fn count(q: &Sequence) -> usize {
    q.video_tracks.len().saturating_add(q.audio_tracks.len()).saturating_add(q.submix_tracks.len()).saturating_add(q.caption_tracks.len())
}
pub(super) fn digest(q: &Sequence) -> Result<String> {
    if count(q) > 4096 {
        return Err(error("track inventory exceeds 4096"));
    }
    let mut ids = BTreeSet::new();
    for t in q.all_tracks().chain(&q.submix_tracks) {
        if !id(t.id.0)
            || !ids.insert(t.id)
            || !text(&t.name, 1024)
            || !t.volume_db.is_finite()
            || !t.pan.is_finite()
            || t.mixer.sends.len() > 256
            || t.mixer.output.is_some_and(|n| !id(n.0))
            || t.mixer.sends.iter().any(|s| !id(s.target.0) || !s.level_db.is_finite() || !s.pan.is_finite())
        {
            return Err(error("invalid or duplicated authored track metadata"));
        }
    }
    for t in &q.caption_tracks {
        if !id(t.id.0) || !ids.insert(t.id) || !text(&t.name, 1024) || !text(&t.language, 128) {
            return Err(error("invalid or duplicated caption track metadata"));
        }
    }
    let bytes = project_audit::value_bytes(&(
        &q.video_tracks,
        &q.audio_tracks,
        &q.submix_tracks,
        &q.caption_tracks,
        q.master_volume_db,
        &q.master_effects,
        &q.master_mixer,
    ))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub(super) fn summary(s: &Session) -> Value {
    let Some(q) = s.active_sequence() else { return json!({"status":"unavailable"}) };
    match digest(q) {
        Ok(hash) => json!({"status":"current","sha256":hash,"tracks":count(q)}),
        Err(e) => json!({"status":"unavailable","reason":e.to_string()}),
    }
}
fn header(t: &Track, kind: &str, index: usize) -> Value {
    json!({"id":if kind=="master"{None}else{Some(t.id.0)},"kind":kind,"index":index,"name":t.name,"locked":t.locked,"syncLock":t.sync_lock,"enabled":t.enabled,
        "muted":t.muted,"solo":t.solo,"channels":crate::mixer::channels_name(t.channels),"volumeDb":t.volume_db,"pan":t.pan,"label":t.label.map(|l|l.name()),
        "items":t.items.len(),"transitions":t.transitions.len(),"effects":t.effects.len(),"format":null,"language":null,
        "mixer":{"output":t.mixer.output.map(|id|id.0),"sends":t.mixer.sends,"mode":t.mixer.mode,"automationLanes":t.mixer.lanes.len(),"recordArm":t.mixer.record_arm,"soloSafe":t.mixer.solo_safe,"inputMap":t.mixer.input_map}})
}
fn headers(q: &Sequence) -> Vec<Value> {
    let mut rows = Vec::with_capacity(count(q));
    for (kind, tracks) in [("video", &q.video_tracks), ("audio", &q.audio_tracks), ("submix", &q.submix_tracks)] {
        rows.extend(tracks.iter().enumerate().map(|(i, t)| header(t, kind, i + 1)));
    }
    rows.extend(q.caption_tracks.iter().enumerate().map(|(i,t)|json!({"id":t.id.0,"kind":"caption","index":i+1,"name":t.name,"locked":t.locked,"syncLock":t.sync_lock,"enabled":t.enabled,
        "muted":null,"solo":null,"channels":null,"volumeDb":null,"pan":null,"label":null,"items":t.captions.len(),"transitions":0,"effects":0,"format":t.format.label(),"language":t.language,"mixer":null})));
    rows
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readTracks", p)?;
    if r.offset > 4096 || r.limit == 0 || r.limit > 32 {
        return Err(error("bounded explicit track page required"));
    }
    if r.projection {
        #[cfg(target_arch = "wasm32")]
        return Err(error("track projection requires native source qualification"));
        #[cfg(not(target_arch = "wasm32"))]
        super::validate_read_basis(s, &r.token)?;
    } else {
        super::validate_snapshot(s, &json!({"token":r.token}))?;
    }
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("track sequence is outside this workspace"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let sha = digest(q)?;
    let rows = headers(q);
    if r.offset > rows.len() {
        return Err(error("track offset exceeds inventory"));
    }
    let end = r.offset.saturating_add(r.limit).min(rows.len());
    let page = rows.get(r.offset..end).ok_or_else(|| error("invalid track page"))?;
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sid.0,"sha256":sha,"offset":r.offset,"total":rows.len(),
        "nextOffset":if end<rows.len(){Some(end)}else{None},"tracks":page,"master":header(&q.master_strip(),"master",0),"requiresSourceValidation":r.projection});
    if project_audit::value_bytes(&result)?.len() > 512 * 1024 {
        return Err(error("track page exceeds 512 KiB; reduce page size"));
    }
    Ok(result)
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Add {
        video: usize,
        audio: usize,
        submix: usize,
        video_after: Option<u64>,
        audio_after: Option<u64>,
        submix_after: Option<u64>,
        audio_channels: String,
        submix_channels: String,
    },
    Remove {
        tracks: Vec<u64>,
    },
    Reorder {
        track: u64,
        after: Option<u64>,
    },
    Set {
        track: u64,
        changes: Value,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_sha256: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    if !hash(&r.before_sha256) {
        return Err(error("current complete track digest required"));
    }
    let valid = match r.edit {
        Edit::Add { video, audio: na, submix, video_after, audio_after, submix_after, audio_channels, submix_channels } => {
            [video, na, submix].iter().all(|n| *n <= 99)
                && video + na + submix > 0
                && [video_after, audio_after, submix_after].iter().all(|n| n.is_none_or(id))
                && audio(&audio_channels)
                && audio(&submix_channels)
                && ["video_after", "audio_after", "submix_after"].iter().all(|key| op.params.get("edit").is_some_and(|e| e.get(*key).is_some()))
        }
        Edit::Remove { tracks } => {
            !tracks.is_empty() && tracks.len() <= 4096 && tracks.iter().all(|n| id(*n)) && tracks.iter().collect::<BTreeSet<_>>().len() == tracks.len()
        }
        Edit::Reorder { track, after } => {
            id(track) && after.is_none_or(id) && after != Some(track) && op.params.get("edit").is_some_and(|e| e.get("after").is_some())
        }
        Edit::Set { track, changes } => id(track) && valid_changes(&changes),
    };
    if !valid {
        return Err(error("one explicit bounded track gesture required"));
    }
    Ok(())
}
fn valid_changes(p: &Value) -> bool {
    let Some(o) = p.as_object().filter(|o| !o.is_empty()) else { return false };
    o.iter().all(|(key, v)| match key.as_str() {
        "locked" | "syncLock" | "enabled" | "muted" | "solo" => v.is_boolean(),
        "name" => v.as_str().is_some_and(|s| text(s, 1024)),
        "language" => v.as_str().is_some_and(|s| text(s, 128)),
        "format" => v.as_str().is_some_and(|s| CaptionFormat::from_name(s).is_some()),
        "volumeDb" => v.as_f64().is_some_and(|n| n.is_finite() && (-96.0..=15.0).contains(&n)),
        "pan" => v.as_f64().is_some_and(|n| n.is_finite() && (-100.0..=100.0).contains(&n)),
        "label" => v.is_null() || v.as_str().is_some_and(|s| Label::from_name(s).is_some()),
        _ => false,
    })
}
fn position(q: &Sequence, kind: Kind, after: Option<u64>) -> Result<usize> {
    match after {
        None => Ok(0),
        Some(n) => {
            let (k, i) = track_ops::locate(q, TrackId(n), COMMAND)?;
            if k != kind {
                return Err(error("insertion anchor has a different track kind"));
            }
            Ok(i + 1)
        }
    }
}
fn locked(q: &Sequence, id: TrackId) -> Result<bool> {
    let (kind, i) = track_ops::locate(q, id, COMMAND)?;
    Ok(match kind {
        Kind::Caption => q.caption_tracks.get(i).is_none_or(|t| t.locked),
        Kind::Video => q.video_tracks.get(i).is_none_or(|t| t.locked),
        Kind::Audio => q.audio_tracks.get(i).is_none_or(|t| t.locked),
        Kind::Submix => q.submix_tracks.get(i).is_none_or(|t| t.locked),
    })
}
fn settings(q: &mut Sequence, id: TrackId, p: &Value) -> Result<&'static str> {
    let (kind, i) = track_ops::locate(q, id, COMMAND)?;
    let keys = p.as_object().ok_or_else(|| error("explicit track settings required"))?;
    if locked(q, id)? && !(keys.len() == 1 && keys.contains_key("locked")) {
        return Err(error("track is locked; no automatic unlock"));
    }
    let allowed: &[&str] = match kind {
        Kind::Video | Kind::Audio => &["name", "locked", "syncLock", "enabled", "muted", "solo", "label", "volumeDb", "pan"],
        Kind::Submix => &["name", "muted", "solo", "volumeDb", "pan"],
        Kind::Caption => &["name", "format", "language", "locked", "syncLock", "enabled"],
    };
    if keys.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(error("setting has no shared human gesture on this track kind"));
    }
    if kind == Kind::Caption {
        let t = q.caption_tracks.get_mut(i).ok_or_else(|| error("caption track missing"))?;
        if let Some(v) = p.get("name").and_then(Value::as_str) {
            t.name = v.into();
        }
        if let Some(v) = p.get("language").and_then(Value::as_str) {
            t.language = v.into();
        }
        if let Some(v) = p.get("format").and_then(Value::as_str) {
            t.format = CaptionFormat::from_name(v).ok_or_else(|| error("invalid caption format"))?;
        }
        if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
            t.enabled = v;
        }
        if let Some(v) = p.get("locked").and_then(Value::as_bool) {
            t.locked = v;
        }
        if let Some(v) = p.get("syncLock").and_then(Value::as_bool) {
            t.sync_lock = v;
        }
        return Ok("captions.setTrack");
    }
    let t = match kind {
        Kind::Video => q.video_tracks.get_mut(i),
        Kind::Audio => q.audio_tracks.get_mut(i),
        Kind::Submix => q.submix_tracks.get_mut(i),
        Kind::Caption => None,
    }
    .ok_or_else(|| error("track missing"))?;
    if let Some(v) = p.get("name").and_then(Value::as_str) {
        t.name = v.into();
    }
    if let Some(v) = p.get("locked").and_then(Value::as_bool) {
        t.locked = v;
    }
    if let Some(v) = p.get("syncLock").and_then(Value::as_bool) {
        t.sync_lock = v;
    }
    if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
        t.enabled = v;
    }
    if let Some(v) = p.get("muted").and_then(Value::as_bool) {
        t.muted = v;
    }
    if let Some(v) = p.get("solo").and_then(Value::as_bool) {
        t.solo = v;
    }
    if let Some(v) = p.get("volumeDb").and_then(Value::as_f64) {
        t.volume_db = v;
    }
    if let Some(v) = p.get("pan").and_then(Value::as_f64) {
        t.pan = v;
    }
    if let Some(v) = p.get("label") {
        t.label =
            if v.is_null() { None } else { Some(Label::from_name(v.as_str().ok_or_else(|| error("invalid label"))?).ok_or_else(|| error("invalid label"))?) };
    }
    Ok(if kind == Kind::Submix { "mixer.setStrip" } else { "timeline.setTrack" })
}
fn collateral(before: &Sequence, after: &Sequence) -> Value {
    let clips = |q: &Sequence| q.all_tracks().map(|t| t.items.len()).sum::<usize>();
    let cues = |q: &Sequence| q.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>();
    let transitions = |q: &Sequence| q.all_tracks().map(|t| t.transitions.len()).sum::<usize>();
    let sends = |q: &Sequence| q.audio_tracks.iter().chain(&q.submix_tracks).map(|t| t.mixer.sends.len()).sum::<usize>() + q.master_mixer.sends.len();
    let rerouted = before
        .audio_tracks
        .iter()
        .chain(&before.submix_tracks)
        .filter(|t| t.mixer.output.is_some() && after.mix_track(t.id).is_some_and(|n| n.mixer.output != t.mixer.output))
        .count();
    json!({"removedClips":clips(before).saturating_sub(clips(after)),"removedCues":cues(before).saturating_sub(cues(after)),"removedTransitions":transitions(before).saturating_sub(transitions(after)),"removedSends":sends(before).saturating_sub(sends(after)),"reroutedOutputs":rerouted})
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("track edit is outside this workspace"));
    }
    let before = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if digest(before)? != r.before_sha256 {
        return Err(error("complete track state changed since its read"));
    }
    let mut expected = (*s.project).clone();
    let mut editor: EditorState = s.state.clone();
    let mut identities = Vec::new();
    let (command, args) = match r.edit {
        Edit::Add { video, audio, submix, video_after, audio_after, submix_after, audio_channels, submix_channels } => {
            let total = video + audio + submix;
            let start = crate::project_ids::allocation_start(&expected, total as u64)?;
            expected.next_id = start.checked_add(total as u64).filter(|n| *n <= SAFE).ok_or_else(|| error("track ID allocation exceeds exact transport"))?;
            let q = expected.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
            let positions = [position(q, Kind::Video, video_after)?, position(q, Kind::Audio, audio_after)?, position(q, Kind::Submix, submix_after)?];
            let mut next = start;
            for ((rows, n, prefix, ch), at) in [
                (&mut q.video_tracks, video, "Video", AudioChannels::Stereo),
                (&mut q.audio_tracks, audio, "Audio", crate::mixer::channels_from(&audio_channels).ok_or_else(|| error("invalid audio type"))?),
                (&mut q.submix_tracks, submix, "Submix", crate::mixer::channels_from(&submix_channels).ok_or_else(|| error("invalid submix type"))?),
            ]
            .into_iter()
            .zip(positions)
            {
                if rows.len().saturating_add(n) > crate::sequence_tools::MAX_TRACKS {
                    return Err(error("native track limit exceeded"));
                }
                for i in 0..n {
                    let mut t = Track::new(TrackId(next), if prefix == "Video" { TrackKind::Video } else { TrackKind::Audio }, format!("{prefix} 0"));
                    t.channels = ch;
                    identities.push(next);
                    next += 1;
                    rows.insert(at + i, t);
                }
                if n > 0 {
                    crate::sequence_tools::renumber(rows, prefix);
                }
            }
            (
                "sequence.addTracks",
                json!({"video":video,"audio":audio,"submix":submix,"videoAfter":positions[0],"audioAfter":positions[1],"submixAfter":positions[2],"audioType":audio_channels,"submixType":submix_channels}),
            )
        }
        Edit::Remove { tracks } => {
            let q = expected.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
            for n in &tracks {
                if locked(q, TrackId(*n))? {
                    return Err(error("track is locked; no automatic unlock or deletion"));
                }
            }
            let doomed = tracks.iter().map(|n| TrackId(*n)).collect();
            track_ops::remove_inventory(q, &mut editor, sid, &doomed);
            identities = tracks.clone();
            ("sequence.removeTracks", json!({"tracks":tracks}))
        }
        Edit::Reorder { track, after } => {
            let q = expected.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
            if locked(q, TrackId(track))? {
                return Err(error("track is locked; no automatic unlock or reorder"));
            }
            track_ops::reorder_inventory(q, TrackId(track), after.map(TrackId), COMMAND)?;
            identities.push(track);
            ("sequence.reorderTrack", json!({"track":track,"after":after}))
        }
        Edit::Set { track, changes } => {
            let q = expected.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
            let command = settings(q, TrackId(track), &changes)?;
            let mut args = changes;
            args.as_object_mut()
                .ok_or_else(|| error("explicit changes required"))?
                .insert(if command == "mixer.setStrip" { "strip" } else { "track" }.into(), json!(track));
            identities.push(track);
            (command, args)
        }
    };
    let after_q = expected.sequence(sid).ok_or(EngineError::NoSequence)?;
    let after = digest(after_q)?;
    if after == r.before_sha256 {
        return Err(error("track gesture makes no change"));
    }
    let effects = collateral(before, after_q);
    let expected_bytes = project_audit::bytes(&expected)?;
    let editor_bytes = project_audit::value_bytes(&editor)?;
    s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != expected_bytes || project_audit::value_bytes(&s.state)? != editor_bytes {
        return Err(error("human track command clamped or changed unexpected collateral; staging discarded"));
    }
    Ok(json!({"beforeSha256":r.before_sha256,"afterSha256":after,"tracks":identities,"humanCommand":command,"collateral":effects}))
}
