//! Mechanical assembly of explicit components into one new native sequence.
//! Editorial choices belong to callers. One candidate, one identity reconciliation
//! and one native history commit; never one history snapshot per source row.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{
    ItemId, Label, Marker, MarkerId, MarkerKind, SequenceSettings, TrackKind, Transition, TransitionId,
    clip_identity::{EFFECT_TRACKING_KEY, TRACKING_KEY},
};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Session, commands::bad};

pub(crate) const COMMAND: &str = "sequence.assemble";
const SAFE: i64 = 9_007_199_254_740_991;
const MAX_COMPONENTS: usize = 8192;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Track {
    kind: TrackKind,
    name: String,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum Timing {
    NearestFrame,
    FloorFrame,
    Exact,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Component {
    key: String,
    item: u64,
    track: usize,
    at: Tick,
    duration: Tick,
    source_in: Tick,
    source_out: Tick,
    speed: f64,
    timing: Timing,
    audio_stream: Option<usize>,
    channels: Vec<u16>,
    link: Option<String>,
    enabled: bool,
    gain_db: f64,
    fade_in: Tick,
    fade_out: Tick,
    annotations: BTreeMap<String, String>,
    protection: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Chapter {
    key: String,
    at: Tick,
    duration: Tick,
    name: String,
    comment: String,
    kind: MarkerKind,
    color: Label,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    name: String,
    settings: SequenceSettings,
    tracks: Vec<Track>,
    components: Vec<Component>,
    markers: Vec<Chapter>,
    metadata: BTreeMap<String, String>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
fn text(s: &str, limit: usize) -> bool {
    !s.trim().is_empty() && s.len() <= limit && !s.chars().any(char::is_control)
}
fn end(at: Tick, duration: Tick) -> Result<Tick> {
    if !(0..=SAFE).contains(&at.0) || !(0..=SAFE).contains(&duration.0) {
        return Err(error("clock is outside exact nonnegative native ticks"));
    }
    at.0.checked_add(duration.0).filter(|n| *n <= SAFE).map(Tick).ok_or_else(|| error("native interval end overflow"))
}
fn unit(settings: &SequenceSettings) -> Result<Tick> {
    settings.validate().map_err(error)?;
    let rate = settings.frame_rate;
    let scale = i128::from(TICKS_PER_SECOND) * i128::from(rate.den);
    let step = scale / i128::from(rate.num);
    if scale % i128::from(rate.num) != 0 || !(1..=i128::from(SAFE)).contains(&step) {
        return Err(error("assembly requires an exactly representable bounded sequence frame"));
    }
    Ok(Tick(step as i64))
}
fn validate(s: &Session, value: &Value, max_bytes: usize) -> Result<Request> {
    if s.revision == u64::MAX {
        return Err(error("assembly revision counter is exhausted"));
    }
    if serde_json::to_vec(value).map_err(|e| error(e.to_string()))?.len() > max_bytes {
        return Err(error("assembly exceeds its complete request byte bound"));
    }
    let p: Request = serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    // Nested native settings normally support old-file defaults. A new assembly
    // instead requires every field and rejects unknown/implicitly defaulted data.
    if serde_json::to_value(&p.settings).map_err(|e| error(e.to_string()))? != value["settings"] {
        return Err(error("assembly needs complete exact native settings"));
    }
    let step = unit(&p.settings)?;
    if !text(&p.name, 160)
        || p.name.trim() != p.name
        || p.tracks.is_empty()
        || p.tracks.len() > 64
        || p.components.is_empty()
        || p.components.len() > MAX_COMPONENTS
        || p.markers.len() > 4096
        || p.metadata.len() > 64
        || s.project.sequences().count() >= 128
        || s.project.sequences().any(|it| it.name.to_lowercase() == p.name.to_lowercase())
        || p.tracks.iter().any(|t| !text(&t.name, 1024))
        || p.metadata.iter().any(|(k, v)| !text(k, 128) || v.len() > max_bytes || k == TRACKING_KEY || k == EFFECT_TRACKING_KEY || k == "poema.sequence.v1")
    {
        return Err(error("invalid or oversized assembly name, tracks, metadata or inventory"));
    }
    let mut keys = BTreeSet::new();
    let mut intervals: BTreeMap<usize, Vec<(Tick, Tick)>> = BTreeMap::new();
    let mut links: BTreeMap<&str, Vec<(usize, TrackKind)>> = BTreeMap::new();
    for c in &p.components {
        let track = p.tracks.get(c.track).ok_or_else(|| error("component names a missing destination track"))?;
        if !text(&c.key, 256)
            || !keys.insert(c.key.as_str())
            || c.item == 0
            || c.item > SAFE as u64
            || c.source_in < Tick::ZERO
            || c.source_out <= c.source_in
            || c.source_out.0 > SAFE
            || !c.speed.is_finite()
            || c.speed <= 0.0
            || c.duration <= Tick::ZERO
            || !c.gain_db.is_finite()
            || !(-96.0..=96.0).contains(&c.gain_db)
            || c.annotations.len() > 64
            || c.annotations.iter().any(|(k, v)| k.is_empty() || k.len() > 64 || v.len() > 1024)
            || c.protection.as_ref().is_some_and(|v| v.trim().is_empty() || v.len() > 1024)
        {
            return Err(error("invalid explicit component identity, source clock, processing or notes"));
        }
        let finish = end(c.at, c.duration)?;
        let media = s.project.item(ItemId(c.item)).and_then(|i| i.as_media()).ok_or_else(|| error("assembly needs direct imported media components"))?;
        if media.offline || s.media.offline_status(ItemId(c.item)).is_some() || c.source_out > media.info.duration {
            return Err(error("assembly source is offline or its requested window exceeds the media"));
        }
        let span = c.source_out.0 - c.source_in.0;
        let raw = span as f64 / c.speed;
        let consumed = c.duration.0 as f64 * c.speed;
        if !raw.is_finite() || raw < 1.0 || raw > SAFE as f64 || !consumed.is_finite() || consumed < 1.0 || consumed.round() > (SAFE - c.source_in.0) as f64 {
            return Err(error("assembly retiming exceeds supported exact native clocks"));
        }
        let actual = filmcraft_edit::duration_at_speed(Tick(span), c.speed)?;
        let expected = match c.timing {
            Timing::NearestFrame => p.settings.frame_rate.snap_nearest(actual),
            Timing::FloorFrame => p.settings.frame_rate.snap(actual),
            Timing::Exact => actual,
        };
        if expected != c.duration
            || c.duration < step && track.kind == TrackKind::Video
            || c.timing != Timing::Exact && (c.at.0 % step.0 != 0 || c.duration.0 % step.0 != 0)
            || track.kind == TrackKind::Video && c.timing == Timing::Exact
        {
            return Err(error("explicit component duration/position disagrees with its declared native rounding policy"));
        }
        // TrackItem::source_out consumes rounded(duration * speed). Frame
        // rounding may extend beyond requested Source Out, so validating only
        // that requested endpoint does not establish available media handles.
        let effective_out = c.source_in.0.checked_add(consumed.round() as i64).ok_or_else(|| error("effective source end overflow"))?;
        if effective_out > media.info.duration.0 {
            return Err(error("rounded component source end exceeds available media; no implicit padding or window/speed change"));
        }
        match track.kind {
            TrackKind::Video => {
                if media.info.video.is_none()
                    || c.audio_stream.is_some()
                    || !c.channels.is_empty()
                    || c.gain_db != 0.0
                    || c.fade_in != Tick::ZERO
                    || c.fade_out != Tick::ZERO
                {
                    return Err(error("picture components cannot hide source audio or mix controls"));
                }
                let video = media.info.video.as_ref().ok_or_else(|| error("missing source picture"))?;
                filmcraft_project::validate_frame_size(video.width, video.height).map_err(error)?;
                let mut source_settings = p.settings.clone();
                source_settings.frame_rate = video.frame_rate;
                source_settings.validate().map_err(error)?;
                let par = media.interpret.par.unwrap_or(video.par);
                if par.0 == 0 || par.1 == 0 {
                    return Err(error("source picture rate or pixel aspect is invalid"));
                }
            }
            TrackKind::Audio => {
                let stream = c.audio_stream.and_then(|i| media.info.audio_streams.get(i)).ok_or_else(|| error("explicit audio stream is unavailable"))?;
                if c.audio_stream.is_some_and(|n| n >= filmcraft_media::MAX_AUDIO_STREAMS)
                    || stream.channels == 0
                    || stream.channels > 256
                    || stream.sample_rate == 0
                    || stream.sample_rate > 384000
                    || c.channels.is_empty()
                    || c.channels.len() > 64
                    || c.channels.iter().any(|n| u32::from(*n) >= stream.channels)
                    || c.channels.iter().collect::<BTreeSet<_>>().len() != c.channels.len()
                    || c.fade_in < Tick::ZERO
                    || c.fade_out < Tick::ZERO
                    || c.fade_in.0.checked_add(c.fade_out.0).is_none_or(|n| n > c.duration.0)
                {
                    return Err(error("audio channels/fades are missing, duplicated or exceed this component"));
                }
            }
        }
        if let Some(link) = &c.link {
            if !text(link, 256) {
                return Err(error("invalid explicit link group"));
            }
            links.entry(link).or_default().push((c.track, track.kind));
        }
        intervals.entry(c.track).or_default().push((c.at, finish));
    }
    for rows in intervals.values_mut() {
        rows.sort();
        if rows.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(error("assembly components overlap on a destination track"));
        }
    }
    if links.values().any(|g| {
        g.len() < 2
            || g.len() > 64
            || g.iter().filter(|(_, k)| *k == TrackKind::Video).count() > 1
            || g.iter().map(|(t, _)| t).collect::<BTreeSet<_>>().len() != g.len()
    }) {
        return Err(error("explicit links need distinct member tracks and at most one picture component"));
    }
    let extent = p.components.iter().map(|c| end(c.at, c.duration)).collect::<Result<Vec<_>>>()?.into_iter().max().ok_or_else(|| error("empty assembly"))?;
    let mut marker_keys = BTreeSet::new();
    for m in &p.markers {
        if !text(&m.key, 256)
            || !marker_keys.insert(&m.key)
            || m.name.len() > 1024
            || m.name.contains('\0')
            || m.comment.len() > 65536
            || m.comment.contains('\0')
            || end(m.at, m.duration)? > extent
        {
            return Err(error("invalid, duplicate or out-of-sequence assembly marker"));
        }
    }
    Ok(p)
}

pub(crate) fn run(s: &mut Session, value: &Value) -> Result<Value> {
    run_bounded(s, value, MAX_BYTES)
}

// Only private, scoped sequence-payload publication uses the larger complete
// archive budget. The raw human/control command retains its existing 4 MiB cap.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_prepared(s: &Session, value: &Value) -> Result<()> {
    validate(s, value, 16 * 1024 * 1024).map(|_| ())
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_prepared(s: &mut Session, value: &Value) -> Result<Value> {
    run_bounded(s, value, 16 * 1024 * 1024)
}

fn run_bounded(s: &mut Session, value: &Value, max_bytes: usize) -> Result<Value> {
    let request = validate(s, value, max_bytes)?;
    let previous = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let budget = request
        .components
        .len()
        .checked_mul(8)
        .and_then(|n| n.checked_add(request.tracks.len() + request.markers.len() + 8))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| error("assembly identity budget overflow"))?;
    let start = crate::project_ids::allocation_start(&s.project, budget)?;
    if start.checked_add(budget).is_none_or(|n| n > SAFE as u64) {
        return Err(error("assembly identity allocation exceeds lossless transport"));
    }
    let mut project = (*s.project).clone();
    project.next_id = start;
    let nv = request.tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
    let na = request.tracks.len() - nv;
    let id = project.new_sequence(&request.name, request.settings.clone(), nv, na, None);
    let mut vi = 0;
    let mut ai = 0;
    let mut tracks = Vec::new();
    {
        let item = project.item_mut(id).ok_or_else(|| error("new sequence is missing"))?;
        item.metadata.extend(request.metadata.clone());
        item.metadata.insert(TRACKING_KEY.into(), "1".into());
        item.metadata.insert(EFFECT_TRACKING_KEY.into(), "1".into());
        let q = item.as_sequence_mut().ok_or_else(|| error("new sequence is missing"))?;
        for t in &request.tracks {
            let native = if t.kind == TrackKind::Video {
                let i = vi;
                vi += 1;
                q.video_tracks.get_mut(i)
            } else {
                let i = ai;
                ai += 1;
                q.audio_tracks.get_mut(i)
            }
            .ok_or_else(|| error("new destination track is missing"))?;
            native.name = t.name.clone();
            tracks.push(native.id);
        }
    }
    let mut links = BTreeMap::new();
    let mut landed = Vec::new();
    for c in &request.components {
        let kind = request.tracks.get(c.track).ok_or_else(|| error("missing prepared track"))?.kind;
        let track = *tracks.get(c.track).ok_or_else(|| error("missing prepared destination"))?;
        let source = TimeRange::from_bounds(c.source_in, c.source_out);
        let mut clip =
            crate::placement::component(&mut project, ItemId(c.item), kind, c.at, source, &request.settings, &s.prefs.media.default_media_scaling, COMMAND)?;
        // Retiming starts from the original source window, not the already
        // frame-rounded 1x placeholder made by Project placement.
        clip.duration = c.duration;
        clip.speed = c.speed;
        clip.enabled = c.enabled;
        clip.gain_db = c.gain_db;
        if let Some(stream) = c.audio_stream {
            clip.audio_stream = stream;
            clip.source_channels = c.channels.clone();
        }
        if let Some(key) = &c.link {
            clip.link = Some(*links.entry(key.clone()).or_insert_with(|| project.alloc_id()));
        }
        let clip_id = clip.id;
        project.sequence_mut(id).and_then(|q| q.track_mut(track)).ok_or_else(|| error("new track is missing"))?.items.push(clip);
        landed.push((clip_id, track));
        if kind == TrackKind::Audio {
            for (duration, incoming) in [(c.fade_in, true), (c.fade_out, false)] {
                if duration == Tick::ZERO {
                    continue;
                }
                let effect = filmcraft_project::vtransition::find_transition("constant_gain", filmcraft_project::EffectKind::AudioTransition)
                    .ok_or_else(|| error("native linear audio fade definition is missing"))?
                    .instance();
                let transition = Transition {
                    id: TransitionId(project.alloc_id()),
                    effect,
                    start: if incoming { c.at } else { Tick(c.at.0 + c.duration.0 - duration.0) },
                    duration,
                    from: if incoming { None } else { Some(clip_id) },
                    to: if incoming { Some(clip_id) } else { None },
                    align: Default::default(),
                    reverse: false,
                };
                project.sequence_mut(id).and_then(|q| q.track_mut(track)).ok_or_else(|| error("new fade track is missing"))?.transitions.push(transition);
            }
        }
    }
    for m in &request.markers {
        let marker = Marker {
            id: MarkerId(project.alloc_id()),
            start: m.at,
            duration: m.duration,
            name: m.name.clone(),
            comment: m.comment.clone(),
            kind: m.kind,
            color: m.color,
        };
        project.sequence_mut(id).ok_or_else(|| error("new sequence is missing"))?.markers.push(marker);
    }
    let q = project.sequence_mut(id).ok_or_else(|| error("new sequence is missing"))?;
    for track in q.all_tracks_mut() {
        track.sort();
    }
    q.markers.sort_by_key(|m| m.start);
    if project.next_id.checked_sub(start).is_none_or(|n| n > budget) {
        return Err(error("assembly exceeded its identity budget"));
    }
    crate::clip_identity::reconcile(&s.project, &mut project)?;
    let mut results = Vec::new();
    for (c, (clip, track)) in request.components.iter().zip(&landed) {
        let item = project.sequence_mut(id).and_then(|q| q.find_item_mut(*clip)).ok_or_else(|| error("assembled component is missing"))?.1;
        let identity = item.identity.as_mut().ok_or_else(|| error("assembled component lacks persistent identity"))?;
        identity.annotations = c.annotations.clone();
        identity.protection = c.protection.clone();
        results.push(json!({"key":c.key,"clip":clip.0,"track":track.0,"identity":identity.id,"sourceIn":item.source_in.0,"sourceOut":item.source_out().0,
            "requestedSourceOut":c.source_out.0,"at":item.start.0,"duration":item.duration.0,"speed":item.speed,"link":item.link,"timing":c.timing}));
    }
    filmcraft_project::clip_identity::validate_project(&project).map_err(error)?;
    let mut state = s.state.clone();
    crate::commands::open_sequence_state(&project, &mut state, id)?;
    s.commit_edit("Assemble Sequence", project, state)?;
    s.events.push(crate::Event::OpenSequence(id));
    Ok(
        json!({"sequence":id.0,"previousSequence":previous.0,"components":results,"markers":request.markers.len(),"nativeRounding":"source window divided by speed, then explicit nearestFrame/floorFrame/exact policy"}),
    )
}
