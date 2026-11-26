//! Scoped caption reads/gestures over the existing human caption commands.
use super::{Operation, parse, project_audit, sequences};
use crate::{Result, Session, commands::bad};
use filmcraft_project::{Caption, CaptionFormat, CaptionStyle, CaptionTrack, ClipId, TrackId};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.caption";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, m)
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn style(s: &CaptionStyle) -> bool {
    crate::captions::valid_style(s)
}
fn audit(tracks: &[CaptionTrack]) -> Result<()> {
    if tracks.len() > 32 {
        return Err(error("caption track inventory exceeds 32"));
    }
    let mut ids = BTreeSet::new();
    let mut count = 0usize;
    let mut budget = 4 * 1024 * 1024usize;
    for t in tracks {
        if !id(t.id.0) || !ids.insert(t.id.0) || !text(&t.name, 1024) || !text(&t.language, 128) || !style(&t.style) {
            return Err(error("invalid caption track metadata"));
        }
        if t.captions.len() > 100_000 {
            return Err(error("caption cue inventory exceeds 100000"));
        }
        for c in &t.captions {
            count = count.saturating_add(1);
            let bytes = c
                .text
                .len()
                .saturating_add(c.settings.len())
                .saturating_add(c.speaker.as_ref().map_or(0, String::len))
                .saturating_add(c.cue_id.as_ref().map_or(0, String::len));
            budget = budget.checked_sub(bytes.saturating_mul(6).saturating_add(512)).ok_or_else(|| error("complete caption state exceeds 4 MiB"))?;
            if count > 100_000
                || !id(c.id.0)
                || !ids.insert(c.id.0)
                || !(0..=SAFE).contains(&c.start.0)
                || !(1..=SAFE).contains(&c.duration.0)
                || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
                || !text(&c.text, 8192)
                || !text(&c.settings, 2048)
                || c.speaker.as_ref().is_some_and(|s| !text(s, 1024))
                || c.cue_id.as_ref().is_some_and(|s| !text(s, 1024))
            {
                return Err(error("invalid or oversized caption cue"));
            }
        }
        for block in &t.vtt_blocks {
            budget = budget.checked_sub(block.len().saturating_mul(6).saturating_add(32)).ok_or_else(|| error("caption header budget exceeded"))?;
        }
        t.check().map_err(error)?;
    }
    Ok(())
}
pub(super) fn digest(tracks: &[CaptionTrack]) -> Result<String> {
    audit(tracks)?;
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(tracks).map_err(|e| error(e.to_string()))?)))
}
pub(super) fn summary(s: &Session) -> Value {
    let Some(q) = s.active_sequence() else { return json!({"status":"unavailable"}) };
    match digest(&q.caption_tracks) {
        Ok(hash) => {
            json!({"status":"current","sha256":hash,"tracks":q.caption_tracks.len(),"cues":q.caption_tracks.iter().map(|t|t.captions.len()).sum::<usize>()})
        }
        Err(e) => json!({"status":"unavailable","reason":e.to_string()}),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    track: Option<u64>,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readCaptions", p)?;
    if r.limit == 0 || r.limit > 32 || r.offset > 100_000 || r.track.is_some_and(|n| !id(n)) {
        return Err(error("bounded explicit caption page required"));
    }
    if r.projection {
        #[cfg(target_arch = "wasm32")]
        return Err(error("caption projection requires native source qualification"));
        #[cfg(not(target_arch = "wasm32"))]
        super::validate_read_basis(s, &r.token)?;
    } else {
        super::validate_snapshot(s, &json!({"token":r.token}))?;
    }
    let sid = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("caption sequence is outside this workspace"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let hash = digest(&q.caption_tracks)?;
    let headers: Vec<Value> = q.caption_tracks.iter().map(|t|json!({"id":t.id.0,"name":t.name,"format":t.format.label(),"language":t.language,"enabled":t.enabled,"locked":t.locked,"syncLock":t.sync_lock,"style":t.style,"cues":t.captions.len()})).collect();
    let track = r.track.map(|id| q.caption_track(TrackId(id)).ok_or_else(|| error("caption track not present"))).transpose()?;
    if track.is_none() && r.offset != 0 {
        return Err(error("cue offset needs one exact track"));
    }
    let total = track.map_or(0, |t| t.captions.len());
    if r.offset > total {
        return Err(error("caption offset exceeds track"));
    }
    let end = r.offset.saturating_add(r.limit).min(total);
    let cues: Vec<Value> = track
        .map(|t| t.captions.get(r.offset..end).ok_or_else(|| error("invalid cue page")))
        .transpose()?
        .unwrap_or(&[])
        .iter()
        .map(|c| json!({"id":c.id.0,"start":c.start.0,"duration":c.duration.0,"text":c.text,"speaker":c.speaker,"cueId":c.cue_id,"settings":c.settings}))
        .collect();
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sid.0,"sha256":hash,"tracks":headers,"track":r.track,"offset":r.offset,"total":total,"nextOffset":if end<total{Some(end)}else{None},"cues":cues,"requiresSourceValidation":r.projection});
    if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 128 * 1024 {
        return Err(error("complete caption page exceeds 128 KiB; reduce page size"));
    }
    Ok(result)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    CreateTrack { name: String, language: String, format: String },
    Add { track: u64, start_frame: i64, end_frame: i64, text: String },
    Text { track: u64, caption: u64, text: String, speaker: Option<String> },
    Times { track: u64, caption: u64, start_frame: i64, end_frame: i64 },
    Delete { track: u64, caption: u64 },
    Style { track: u64, style: CaptionStyle },
    Enabled { track: u64, enabled: bool },
    Lock { track: u64, locked: bool },
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
    if r.before_sha256.len() != 64 || !r.before_sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(error("exact current caption digest required"));
    }
    let frames = |a: i64, z: i64| a >= 0 && z > a && z <= 1_000_000;
    let valid = match &r.edit {
        Edit::CreateTrack { name, language, format } => {
            text(name, 1024) && !name.trim().is_empty() && text(language, 128) && CaptionFormat::from_name(format).is_some()
        }
        Edit::Add { track, start_frame, end_frame, text: t } => id(*track) && frames(*start_frame, *end_frame) && text(t, 8192),
        Edit::Text { track, caption, text: t, speaker } => {
            id(*track)
                && id(*caption)
                && text(t, 8192)
                && speaker.as_ref().is_none_or(|s| text(s, 1024))
                && op.params.get("edit").is_some_and(|e| e.get("speaker").is_some())
        }
        Edit::Times { track, caption, start_frame, end_frame } => id(*track) && id(*caption) && frames(*start_frame, *end_frame),
        Edit::Delete { track, caption } => id(*track) && id(*caption),
        Edit::Style { track, style: s } => {
            let keys = ["font", "size", "color", "background", "background_color", "align", "anchor", "margin", "line_spacing", "outline", "outline_color"];
            id(*track)
                && style(s)
                && op
                    .params
                    .get("edit")
                    .and_then(|e| e.get("style"))
                    .and_then(Value::as_object)
                    .is_some_and(|o| o.len() == keys.len() && o.keys().all(|k| keys.contains(&k.as_str())))
        }
        Edit::Enabled { track, .. } | Edit::Lock { track, .. } => id(*track),
    };
    if !valid {
        return Err(error("explicit bounded caption gesture required"));
    }
    Ok(())
}
fn frame(s: &Session, n: i64) -> Result<Tick> {
    let rate = s.sequence_rate();
    if rate.num <= 0 || rate.den <= 0 {
        return Err(error("positive exact sequence rate required"));
    }
    let numerator = filmcraft_time::TICKS_PER_SECOND as i128 * rate.den as i128;
    if numerator % rate.num as i128 != 0 {
        return Err(error("caption edit requires exact integer-tick frames"));
    }
    rate.frame_duration().0.checked_mul(n).filter(|v| (0..=SAFE).contains(v)).map(Tick).ok_or_else(|| error("caption time exceeds exact supported range"))
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let sid = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("caption edit is outside workspace"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if digest(&q.caption_tracks)? != r.before_sha256 {
        return Err(error("caption state changed since the read"));
    }
    let mut expected = (*s.project).clone();
    let mut track_id = None;
    let mut caption_id = None;
    let target = match &r.edit {
        Edit::CreateTrack { .. } => None,
        Edit::Add { track, .. }
        | Edit::Text { track, .. }
        | Edit::Times { track, .. }
        | Edit::Delete { track, .. }
        | Edit::Style { track, .. }
        | Edit::Enabled { track, .. }
        | Edit::Lock { track, .. } => Some(*track),
    };
    if let Some(id) = target {
        let t = q.caption_track(TrackId(id)).ok_or_else(|| error("caption track missing"))?;
        if t.locked && !matches!(r.edit, Edit::Lock { .. }) {
            return Err(error("caption track is locked; no automatic unlock"));
        }
        track_id = Some(id);
    }
    let (command, args) = match r.edit {
        Edit::CreateTrack { name, language, format } => {
            let next = crate::project_ids::allocation_start(&expected, 1)?;
            expected.next_id = next.checked_add(1).filter(|n| *n <= SAFE as u64).ok_or_else(|| error("caption ID allocation exhausted"))?;
            let t = crate::captions::new_track(TrackId(next), &json!({"name":name,"language":language,"format":format}));
            expected.sequence_mut(sid).ok_or(crate::EngineError::NoSequence)?.caption_tracks.insert(0, t);
            track_id = Some(next);
            ("captions.newTrack", json!({"name":name,"language":language,"format":format}))
        }
        Edit::Add { track, start_frame, end_frame, text } => {
            let start = frame(s, start_frame)?;
            let end = frame(s, end_frame)?;
            let next = expected.next_id;
            expected.next_id = next.checked_add(1).filter(|n| *n <= SAFE as u64).ok_or_else(|| error("caption ID allocation exhausted"))?;
            let t = expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?;
            t.captions.push(Caption {
                id: ClipId(next),
                start,
                duration: end - start,
                text: text.clone(),
                speaker: None,
                cue_id: None,
                settings: String::new(),
            });
            t.sort();
            t.check().map_err(error)?;
            caption_id = Some(next);
            ("captions.add", json!({"track":track,"time":start.0,"duration":(end-start).0,"text":text}))
        }
        Edit::Text { track, caption, text, speaker } => {
            let c = expected
                .sequence_mut(sid)
                .and_then(|q| q.caption_track_mut(TrackId(track)))
                .and_then(|t| t.captions.iter_mut().find(|c| c.id.0 == caption))
                .ok_or_else(|| error("cue not on this track"))?;
            c.text = text.clone();
            c.speaker = speaker.clone();
            caption_id = Some(caption);
            ("captions.setText", json!({"caption":caption,"text":text,"speaker":speaker}))
        }
        Edit::Times { track, caption, start_frame, end_frame } => {
            let start = frame(s, start_frame)?;
            let end = frame(s, end_frame)?;
            let t = expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?;
            let c = t.captions.iter_mut().find(|c| c.id.0 == caption).ok_or_else(|| error("cue not on this track"))?;
            c.start = start;
            c.duration = end - start;
            t.sort();
            t.check().map_err(error)?;
            caption_id = Some(caption);
            ("captions.setTimes", json!({"caption":caption,"startTime":start.0,"endTime":end.0}))
        }
        Edit::Delete { track, caption } => {
            let t = expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?;
            let index = t.captions.iter().position(|c| c.id.0 == caption).ok_or_else(|| error("cue not on this track"))?;
            t.captions.remove(index);
            caption_id = Some(caption);
            ("captions.delete", json!({"captions":[caption],"ripple":false}))
        }
        Edit::Style { track, style } => {
            expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?.style = style.clone();
            (
                "captions.setStyle",
                json!({"track":track,"font":style.font,"size":style.size,"color":style.color,"background":style.background,"backgroundColor":style.background_color,
                "align":format!("{:?}",style.align).to_lowercase(),"anchor":format!("{:?}",style.anchor).to_lowercase(),"margin":style.margin,"lineSpacing":style.line_spacing,"outline":style.outline,"outlineColor":style.outline_color}),
            )
        }
        Edit::Enabled { track, enabled } => {
            expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?.enabled = enabled;
            ("captions.setTrack", json!({"track":track,"enabled":enabled}))
        }
        Edit::Lock { track, locked } => {
            expected.sequence_mut(sid).and_then(|q| q.caption_track_mut(TrackId(track))).ok_or_else(|| error("caption track missing"))?.locked = locked;
            ("captions.setTrack", json!({"track":track,"locked":locked}))
        }
    };
    let after = digest(&expected.sequence(sid).ok_or(crate::EngineError::NoSequence)?.caption_tracks)?;
    if after == r.before_sha256 {
        return Err(error("caption gesture makes no change"));
    }
    let expected_bytes = project_audit::bytes(&expected)?;
    s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != expected_bytes {
        return Err(error("caption control clamped or changed unexpected state; staging discarded"));
    }
    Ok(json!({"beforeSha256":r.before_sha256,"afterSha256":after,"track":track_id,"caption":caption_id,"humanCommand":command}))
}
