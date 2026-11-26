//! Current authored picture/dialogue geometry for the original Poema fitter.
//! Uses the human clip clock, never restored logical condense inputs. No edit,
//! seek, decoder, speech claim or new history ownership is produced.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ItemId, MediaRef, Track, TrackItem};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BINDING_KEY, Binding, parse, project_audit, sequences};
use crate::{Result, Session, commands::bad};

const COMMAND: &str = "agent.pictureInputs";
const SAFE: i64 = 9_007_199_254_740_991;
const MAX_PIECES: usize = 4096;
fn error(s: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, s)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: sequences::Scope,
    dialogue_track: u64,
    picture_track: Option<u64>,
    bites: Vec<Bite>,
    #[serde(default)]
    projection: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bite {
    key: String,
    asset_id: String,
    source_in_ticks: i64,
    source_out_ticks: i64,
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(value)?)))
}
fn ranges(mut rows: Vec<[i64; 2]>) -> Vec<[i64; 2]> {
    rows.sort_unstable();
    let mut out: Vec<[i64; 2]> = Vec::new();
    for r in rows {
        if let Some(last) = out.last_mut()
            && r[0] <= last[1]
        {
            last[1] = last[1].max(r[1]);
        } else {
            out.push(r);
        }
    }
    out
}
fn total(rows: &[[i64; 2]]) -> Result<i64> {
    rows.iter().try_fold(0i64, |n, r| n.checked_add(r[1] - r[0]).filter(|n| *n <= SAFE).ok_or_else(|| error("measurement overflow")))
}
fn strip(t: &Track) -> Result<Value> {
    let mut v = serde_json::to_value(t).map_err(|e| error(e.to_string()))?;
    let o = v.as_object_mut().ok_or_else(|| error("invalid native track"))?;
    o.remove("items");
    o.remove("transitions");
    Ok(v)
}
pub(super) fn clock(c: &TrackItem, duration: i64) -> Result<()> {
    let scaled = (c.duration.0 as f64 * c.speed.abs()).round();
    if !(0..=SAFE).contains(&c.start.0)
        || !(1..=SAFE).contains(&c.duration.0)
        || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
        || !(0..=SAFE).contains(&c.source_in.0)
        || !c.speed.is_finite()
        || c.speed == 0.0
        || !scaled.is_finite()
        || !(0.0..=SAFE as f64).contains(&scaled)
        || c.source_in.0.checked_add(scaled as i64).is_none_or(|n| n > SAFE || n > duration)
        || c.frame_hold.is_some_and(|t| !(0..duration).contains(&t.0))
        || c.effects.iter().any(|e| {
            e.enabled
                && (e.effect == "remix"
                    || e.effect == "time_remap" && e.params.values().any(|p| !p.keyframes.is_empty() || p.value != filmcraft_project::ParamValue::Float(100.0)))
        })
    {
        return Err(error("current piece needs bounded static native clocks; remix and animated remapping require their renderer-input contract"));
    }
    Ok(())
}
// Lower-bound search over the SAME discrete native source clock used by the
// monitor. In particular this preserves rounding at fractional speeds, reverse
// half-open edges, frame holds and non-frame-aligned authored starts.
pub(super) fn overlap(c: &TrackItem, window: [i64; 2]) -> Option<[i64; 2]> {
    if let Some(t) = c.frame_hold {
        return (window[0] <= t.0 && t.0 < window[1]).then_some([c.start.0, c.end().0]);
    }
    let bound = |threshold: i64| {
        let (mut a, mut z) = (0i64, c.duration.0);
        while a < z {
            let mid = a + (z - a) / 2;
            let source = c.source_time_at(Tick(c.start.0 + mid)).0;
            let reached = if c.reverse { source < threshold } else { source >= threshold };
            if reached {
                z = mid;
            } else {
                a = mid + 1;
            }
        }
        c.start.0 + a
    };
    let (a, z) = if c.reverse { (bound(window[1]), bound(window[0])) } else { (bound(window[0]), bound(window[1])) };
    (a < z).then_some([a, z])
}
// A source event is a threshold in the same discrete clock as playback. Callers
// validate the clock first and handle held clocks explicitly; no f64 inverse.
pub(super) fn event_at(c: &TrackItem, at: i64, source_duration: i64) -> Option<i64> {
    let last_tick = c.end().0.checked_sub(1)?;
    let first = c.source_time_at(c.start).0;
    let last = c.source_time_at(Tick(last_tick)).0;
    if at < first.min(last) || at > first.max(last) {
        return None;
    }
    let window = if c.reverse { [0, at.checked_add(1)?] } else { [at, source_duration] };
    overlap(c, window).map(|range| range[0])
}
fn binding<'a>(s: &'a Session, item: ItemId, project_id: &str, bindings: &'a BTreeMap<ItemId, Binding>) -> Result<&'a Binding> {
    let b = bindings.get(&item).ok_or_else(|| error("current input has no unambiguous original binding"))?;
    let m = s.project.item(item).and_then(|i| i.as_media()).ok_or_else(|| error("current input is not direct original media"))?;
    if b.project_id != project_id
        || b.role != "original"
        || b.asset_id.is_empty()
        || b.media_id.is_empty()
        || m.offline
        || !(1..=SAFE).contains(&m.info.duration.0)
        || !matches!(&m.media, MediaRef::File { path } if std::path::Path::new(path).is_absolute())
    {
        return Err(error("current input must be one online explicitly bound original"));
    }
    Ok(b)
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    let r: Request = parse(COMMAND, value)?;
    let mut keys = BTreeSet::new();
    if value.get("pictureTrack").is_none()
        || r.bites.is_empty()
        || r.bites.len() > 64
        || !(1..=SAFE as u64).contains(&r.dialogue_track)
        || r.picture_track.is_some_and(|id| !(1..=SAFE as u64).contains(&id) || id == r.dialogue_track)
        || r.bites.iter().any(|b| {
            b.key.len() != 64
                || !b.key.bytes().all(|x| x.is_ascii_digit() || (b'a'..=b'f').contains(&x))
                || !keys.insert(&b.key)
                || b.asset_id.is_empty()
                || b.asset_id.len() > 256
                || b.asset_id.chars().any(char::is_control)
                || !(0..=SAFE).contains(&b.source_in_ticks)
                || !(1..=SAFE).contains(&b.source_out_ticks)
                || b.source_out_ticks <= b.source_in_ticks
        })
    {
        return Err(error("need 1–64 distinct source choices and explicit separate current dialogue/picture tracks"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, seq, &r.scope)? {
        return Err(error("current picture inputs belong to another workspace"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    q.check().map_err(error)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    project_audit::bytes(&s.project)?;
    if s.project.items.len() > 100_000 || q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("current input scan exceeds its complete bound"));
    }
    let dialogue = q.video_tracks.iter().find(|t| t.id.0 == r.dialogue_track).ok_or_else(|| error("explicit dialogue picture track is missing"))?;
    if dialogue.items.len() > MAX_PIECES {
        return Err(error("complete current dialogue track exceeds 4096 pieces"));
    }
    let picture =
        r.picture_track.map(|id| q.video_tracks.iter().find(|t| t.id.0 == id).ok_or_else(|| error("explicit overlay track is missing"))).transpose()?;
    let mut bindings = BTreeMap::new();
    let mut assets = BTreeSet::new();
    for (id, item) in &s.project.items {
        if item.as_media().is_none() {
            continue;
        }
        if let Some(raw) = item.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == r.scope.project_id && b.role == "original" && !assets.insert(b.asset_id.clone()) {
                return Err(error("duplicate original source correspondence"));
            }
            bindings.insert(*id, b);
        }
    }
    let mut linked: BTreeMap<u64, Vec<(&Track, &TrackItem)>> = BTreeMap::new();
    for t in &q.audio_tracks {
        for a in &t.items {
            if let Some(link) = a.link {
                linked.entry(link).or_default().push((t, a));
            }
        }
    }
    let mut source_ids = BTreeSet::new();
    let mut budget = MAX_PIECES;
    let mut bites = Vec::new();
    let mut all_picture = Vec::new();
    let mut all_audio = Vec::new();
    for b in &r.bites {
        let item = bindings
            .iter()
            .find(|(_, x)| x.project_id == r.scope.project_id && x.role == "original" && x.asset_id == b.asset_id)
            .map(|(id, _)| *id)
            .ok_or_else(|| error("requested bite original is not bound"))?;
        binding(s, item, &r.scope.project_id, &bindings)?;
        let media = s.project.item(item).and_then(|x| x.as_media()).ok_or_else(|| error("missing bite media"))?;
        if !media.info.has_video() || b.source_out_ticks > media.info.duration.0 {
            return Err(error("bite exceeds the bound original picture"));
        }
        source_ids.insert(item);
        let mut pieces = Vec::new();
        let mut picture_ranges = Vec::new();
        let mut audio_ranges = Vec::new();
        for c in dialogue.items.iter().filter(|c| c.item == item) {
            clock(c, media.info.duration.0)?;
            let Some(range) = overlap(c, [b.source_in_ticks, b.source_out_ticks]) else {
                continue;
            };
            budget = budget.checked_sub(1).ok_or_else(|| error("complete current bite mapping exceeds 4096 pieces"))?;
            let identity = c.identity.as_ref().ok_or_else(|| error("current dialogue instance is untracked"))?;
            let mut audio = Vec::new();
            if let Some(members) = c.link.and_then(|id| linked.get(&id)) {
                for &(t, a) in members {
                    budget = budget.checked_sub(1).ok_or_else(|| error("complete linked audio exceeds 4096 pieces"))?;
                    let z = a.start.0.checked_add(a.duration.0).filter(|n| *n <= SAFE).ok_or_else(|| error("linked audio clock overflow"))?;
                    let span = [range[0].max(a.start.0), range[1].min(z)];
                    if span[0] >= span[1] {
                        continue;
                    }
                    let original = binding(s, a.item, &r.scope.project_id, &bindings)?;
                    let am = s.project.item(a.item).and_then(|x| x.as_media()).ok_or_else(|| error("linked sound is not original media"))?;
                    clock(a, am.info.duration.0)?;
                    let stream = am.info.audio_streams.get(a.audio_stream).ok_or_else(|| error("linked sound stream is unavailable"))?;
                    let channels = filmcraft_render::audio::source_input_channels(a, stream.channels as usize, filmcraft_render::mixer::width_of(t.channels));
                    if channels.is_empty() || a.identity.is_none() {
                        return Err(error("linked sound has no tracked current channel input"));
                    }
                    source_ids.insert(a.item);
                    audio_ranges.push(span);
                    audio.push(json!({"range":span,"authored":a,"track":strip(t)?,"binding":original,
                        "input":{"stream":a.audio_stream,"channels":channels,"sampleRate":stream.sample_rate,"streamChannels":stream.channels},
                        "transitions":t.transitions.iter().filter(|x|x.from==Some(a.id)||x.to==Some(a.id)).collect::<Vec<_>>()}));
                }
            }
            picture_ranges.push(range);
            pieces.push(json!({"range":range,"identity":identity.id,"authored":c,"linkedAudio":audio}));
        }
        let picture_ranges = ranges(picture_ranges);
        let audio_ranges = ranges(audio_ranges);
        all_picture.extend_from_slice(&picture_ranges);
        all_audio.extend_from_slice(&audio_ranges);
        bites.push(json!({"key":b.key,"assetId":b.asset_id,"item":item.0,"sourceRange":[b.source_in_ticks,b.source_out_ticks],
            "pictureRanges":picture_ranges,"linkedAudioRanges":audio_ranges,"pieces":pieces}));
    }
    let ticket = super::validate_read_basis(s, &r.token)?;
    if ticket.media.is_scoped() {
        ticket.media.require(&super::media_scope::dependencies(&s.project, source_ids.iter().copied())?)?;
    }
    let sources = source_ids.iter().map(|id| s.project.item(*id).ok_or_else(|| error("input original disappeared"))).collect::<Result<Vec<_>>>()?;
    let request_basis = (
        &r.scope.project_id,
        r.scope.anchor_sequence,
        r.dialogue_track,
        r.picture_track,
        r.bites.iter().map(|b| (&b.key, &b.asset_id, b.source_in_ticks, b.source_out_ticks)).collect::<Vec<_>>(),
    );
    let picture_ranges = ranges(all_picture);
    let audio_ranges = ranges(all_audio);
    let result = json!({"kind":"native_picture_inputs","version":1,"status":"read","token":r.token,"revision":s.revision,
        "sequence":seq.0,"scope":r.scope,"requestSha256":digest(&request_basis)?,"basisSha256":digest(&(seq.0,&r.scope,&s.project))?,
        "frameRate":q.settings.frame_rate,"dialogueTrack":strip(dialogue)?,"pictureTrack":picture,"bites":bites,"sources":sources,
        "pictureRanges":picture_ranges,"pictureTicks":total(&picture_ranges)?,"linkedAudioRanges":audio_ranges,"linkedAudioTicks":total(&audio_ranges)?,
        "requiresSourceValidation":r.projection,"writeAuthority":false,"semanticVerification":false,"signalVerification":false,
        "audioCoverage":"linked authored geometry; mixer audibility and speech require original-owner/output qualification"});
    if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 2 * 1024 * 1024 {
        return Err(error("complete current picture inputs exceed 2 MiB; no rows were truncated"));
    }
    Ok(result)
}
