//! Explicit finishing gestures through the SAME human gain/transition commands.
//! No alternate mixer, transition compositor, source clock or history writer.
use super::{Operation, finishing, parse, project_audit, sequences};
use crate::{Result, Session, commands::bad};
use filmcraft_project::{ClipId, ClipIdentity, ItemId, TrackId, TrackItem, TrackKind};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) const COMMAND: &str = "agent.finish";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    clip: u64,
    identity: String,
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Edit {
    #[serde(rename = "gain")]
    Gain { clip: u64, identity: String, targets: Vec<Target>, mode: String, db: f64 },
    #[serde(rename = "transition")]
    Transition { clip: u64, identity: String, neighbor: Option<Target>, edge: String, effect: String, frames: i64 },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    edit: Edit,
}
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, m)
}
fn valid(t: &Target) -> bool {
    (1..=SAFE as u64).contains(&t.clip) && ClipIdentity::valid_id(&t.identity)
}
fn frame_unit(rate: filmcraft_time::FrameRate) -> Result<Tick> {
    if rate.num <= 0 || rate.den <= 0 {
        return Err(error("transition needs a positive exact frame rate"));
    }
    let n = filmcraft_time::TICKS_PER_SECOND as i128 * rate.den as i128;
    let unit = n / rate.num as i128;
    if n % rate.num as i128 != 0 || unit < 1 || unit > SAFE as i128 || rate.frame_duration().0 != unit as i64 {
        return Err(error("transition needs exact supported sequence frames"));
    }
    Ok(Tick(unit as i64))
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    let ok = match &r.edit {
        Edit::Gain { clip, identity, targets, mode, db } => {
            valid(&Target { clip: *clip, identity: identity.clone() })
                && !targets.is_empty()
                && targets.len() <= 16
                && targets.iter().all(valid)
                && matches!(mode.as_str(), "set" | "adjust")
                && db.is_finite()
                && (-96.0..=96.0).contains(db)
        }
        Edit::Transition { clip, identity, neighbor, edge, effect, frames } => {
            valid(&Target { clip: *clip, identity: identity.clone() })
                && neighbor.as_ref().is_none_or(valid)
                && op.params.get("edit").and_then(Value::as_object).is_some_and(|e| e.contains_key("neighbor"))
                && matches!(edge.as_str(), "in" | "out")
                && matches!(effect.as_str(), "cross_dissolve" | "constant_power" | "constant_gain" | "exponential_fade")
                && (1..=10000).contains(frames)
        }
    };
    if !ok {
        return Err(error("explicit finishing targets, supported gesture and bounded values required"));
    }
    Ok(())
}
fn clock(c: &TrackItem) -> Result<()> {
    if !(-SAFE..=SAFE).contains(&c.start.0)
        || !(1..=SAFE).contains(&c.duration.0)
        || c.start.0.checked_add(c.duration.0).is_none_or(|x| !(-SAFE..=SAFE).contains(&x))
        || !(0..=SAFE).contains(&c.source_in.0)
        || !c.speed.is_finite()
        || c.speed <= 0.0
    {
        return Err(error("unsafe native finishing clip clock"));
    }
    Ok(())
}
fn target<'a>(s: &'a Session, t: &Target, scope: &sequences::Scope) -> Result<(TrackId, &'a TrackItem)> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (track, c) = q.find_item(ClipId(t.clip)).ok_or_else(|| error("current finishing clip is missing"))?;
    if c.identity.as_ref().is_none_or(|i| i.id != t.identity || i.protection.is_some())
        || q.track(track).is_none_or(|t| t.locked)
        || super::assembly::binding(s, c.item)?.project_id != scope.project_id
        || s.project.item(c.item).and_then(|i| i.as_media()).is_none_or(|m| m.offline)
        || s.media.offline_status(c.item).is_some()
    {
        return Err(error("finishing target is locked, protected, untracked, offline or foreign"));
    }
    clock(c)?;
    Ok((track, c))
}
/// Authored transitions touching one clip. The enclosing read ticket supplies
/// document identity; IDs are read references, not generic mutation permission.
pub(super) fn context(s: &Session, identity: &str, budget: &mut usize) -> Result<Value> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut found = None;
    let mut count = 0usize;
    for t in q.all_tracks() {
        for c in &t.items {
            count += 1;
            if count > 100000 {
                return Err(error("finishing lookup exceeds its clip bound"));
            }
            if c.identity.as_ref().is_some_and(|i| i.id == identity) && found.replace((t, c)).is_some() {
                return Err(error("ambiguous finishing identity"));
            }
        }
    }
    let (track, clip) = found.ok_or_else(|| error("current finishing identity is absent"))?;
    clock(clip)?;
    if !clip.gain_db.is_finite() || track.transitions.len() > 4096 {
        return Err(error("invalid gain or oversized transition inventory"));
    }
    let mut rows = Vec::new();
    for (index, tr) in track.transitions.iter().enumerate().filter(|(_, t)| t.from == Some(clip.id) || t.to == Some(clip.id)) {
        if rows.len() >= 32
            || tr.id.0 == 0
            || tr.id.0 > SAFE as u64
            || !(-SAFE..=SAFE).contains(&tr.start.0)
            || !(1..=SAFE).contains(&tr.duration.0)
            || tr.start.0.checked_add(tr.duration.0).is_none_or(|n| !(-SAFE..=SAFE).contains(&n))
        {
            return Err(error("transition context exceeds its complete bounded read"));
        }
        let endpoint = |id: Option<ClipId>| -> Result<Value> {
            let Some(id) = id else { return Ok(Value::Null) };
            let c = track.item(id).ok_or_else(|| error("dangling transition endpoint"))?;
            clock(c)?;
            Ok(json!({"identity":c.identity.as_ref().map(|i|&i.id)}))
        };
        let from = endpoint(tr.from)?;
        let to = endpoint(tr.to)?;
        let bounds = filmcraft_edit::transitions::bounds(track, tr).ok_or_else(|| error("transition has no current endpoint"))?;
        let rate = q.settings.frame_rate;
        let unit = frame_unit(rate)?;
        rows.push(json!({"id":tr.id.0,"start":tr.start.0,"duration":tr.duration.0,"from":from,"to":to,
            "align":filmcraft_edit::transitions::alignment(tr,bounds.1,unit).id(),"reverse":tr.reverse,
            "effect":finishing::effect(&tr.effect,index+1,budget)?}));
    }
    Ok(json!({"identity":identity,"gainDb":clip.gain_db,"transitions":rows}))
}
fn handles(s: &Session, c: &TrackItem, start: Tick, end: Tick) -> Result<()> {
    // Validate arithmetic before using the renderer's canonical source clock,
    // including reverse, retime and hold. No JavaScript or alternate clock.
    let span = c.duration.0 as f64 * c.speed;
    let lo = start.0.checked_sub(c.start.0).ok_or_else(|| error("transition source range overflow"))?;
    let hi = end.0.checked_sub(1).and_then(|x| x.checked_sub(c.start.0)).ok_or_else(|| error("transition source range overflow"))?;
    if !span.is_finite()
        || span > SAFE as f64
        || [lo, hi].iter().any(|n| (*n as f64 * c.speed).abs() > SAFE as f64)
        || c.frame_hold.is_some_and(|h| !(0..=SAFE).contains(&h.0))
    {
        return Err(error("unsafe native transition source clock"));
    }
    let media = s.project.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("transition needs an imported original"))?;
    for at in [start, Tick(end.0 - 1)] {
        let t = c.source_time_at(at);
        if t < Tick::ZERO || t >= media.info.duration || t.0 > SAFE {
            return Err(error("transition needs available original source handles; no edge-frame substitution"));
        }
    }
    Ok(())
}
pub(super) fn audio_review(s: &Session, start: Tick, end: Tick, edges: bool) -> Result<Vec<Value>> {
    let sr = s.active_sequence().ok_or(crate::EngineError::NoSequence)?.settings.sample_rate;
    if !(1..=384000).contains(&sr) || start < Tick::ZERO || end <= start {
        return Err(error("audio finishing needs a reviewable positive sequence range"));
    }
    let ticks = filmcraft_time::TICKS_PER_SECOND as i128;
    let first = (start.0 as i128)
        .checked_mul(sr as i128)
        .and_then(|n| n.checked_add(ticks - 1))
        .map(|n| n / ticks)
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| error("audio review sample conversion overflow"))?;
    let last = end.to_units_floor(sr as i64);
    if first < 0 || last <= first || last > 10_000_000_000 {
        return Err(error("audio finishing range cannot be reviewed exactly"));
    }
    let length = (last - first).min((sr as i64 / 10).max(1));
    let positions = if edges { vec![first, last - length] } else { vec![first + (last - first - length) / 2] };
    let mut rows = Vec::new();
    for at in positions {
        let row = json!({"kind":"audio","startSample":at,"samples":length,"sampleRate":sr});
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    Ok(rows)
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    let sid = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)?
        || s.project.items.len() > 100000
        || s.active_sequence().is_none_or(|q| q.all_tracks().flat_map(|t| &t.items).take(100001).count() > 100000)
    {
        return Err(error("finishing requires this bounded adopted workspace"));
    }
    let mut expected = (*s.project).clone();
    let (command, args, identities, review) = match r.edit {
        Edit::Gain { clip, identity, targets, mode, db } => {
            let root = Target { clip, identity };
            let (track, _) = target(s, &root, &r.scope)?;
            let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
            if q.track(track).is_none_or(|t| t.kind != TrackKind::Audio) {
                return Err(error("choose an explicit audio clip for gain"));
            }
            let mut actual = crate::commands::with_links(s, &[ClipId(clip)])
                .into_iter()
                .filter(|id| q.audio_tracks.iter().any(|t| t.item(*id).is_some()))
                .map(|id| id.0)
                .collect::<Vec<_>>();
            actual.sort_unstable();
            actual.dedup();
            let mut requested = targets.iter().map(|t| t.clip).collect::<Vec<_>>();
            requested.sort_unstable();
            if actual != requested || actual.is_empty() || actual.len() > 16 {
                return Err(error("gain must name every affected linked audio identity exactly once"));
            }
            let mut changed = false;
            let mut identities = Vec::new();
            let mut review = Vec::new();
            for t in &targets {
                let (_, c) = target(s, t, &r.scope)?;
                let gain = if mode == "set" { db } else { c.gain_db + db };
                if !gain.is_finite() || !(-96.0..=96.0).contains(&gain) {
                    return Err(error("gain would be clamped"));
                }
                changed |= gain != c.gain_db;
                for row in audio_review(s, c.start.max(Tick::ZERO), Tick(c.start.0 + c.duration.0), false)? {
                    if !review.contains(&row) {
                        review.push(row);
                    }
                }
                expected.sequence_mut(sid).and_then(|q| q.find_item_mut(ClipId(t.clip))).ok_or_else(|| error("missing expected gain target"))?.1.gain_db = gain;
                identities.push(t.identity.clone());
            }
            if !changed {
                return Err(error("gain already has the requested value"));
            }
            ("clip.audioGain", json!({"clips":[clip],"mode":mode,"db":db}), identities, review)
        }
        Edit::Transition { clip, identity, neighbor, edge, effect, frames } => {
            let root = Target { clip, identity };
            let (track, c) = target(s, &root, &r.scope)?;
            let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
            let kind = if effect == "cross_dissolve" { TrackKind::Video } else { TrackKind::Audio };
            let t = q.track(track).ok_or_else(|| error("missing transition track"))?;
            if t.kind != kind || t.items.len() > 100000 || t.transitions.len() > 4096 {
                return Err(error("transition effect/track kind mismatch or oversized track"));
            }
            for c in &t.items {
                clock(c)?;
            }
            let mut identities = vec![root.identity.clone()];
            if let Some(n) = &neighbor {
                let (nt, nc) = target(s, n, &r.scope)?;
                if nt != track || nc.id == c.id {
                    return Err(error("transition neighbor must be another current clip on this track"));
                }
                identities.push(n.identity.clone());
            }
            let rate = q.settings.frame_rate;
            let unit = frame_unit(rate)?;
            let duration = unit.0.checked_mul(frames).filter(|n| *n > 0 && *n <= SAFE).ok_or_else(|| error("unsafe transition duration"))?;
            let args = json!({"clip":clip,"effect":effect,"edge":edge,"frames":frames});
            let (planned_track, tr) = crate::commands::preview_transition(s, &args, kind)?;
            let endpoints =
                if edge == "in" { (neighbor.as_ref().map(|n| ClipId(n.clip)), Some(c.id)) } else { (Some(c.id), neighbor.as_ref().map(|n| ClipId(n.clip))) };
            if planned_track != track
                || (tr.from, tr.to) != endpoints
                || tr.duration.0 != duration
                || tr.start < Tick::ZERO
                || tr.start.0.checked_add(duration).is_none_or(|n| n > SAFE)
            {
                return Err(error("transition neighbor/range differs from explicit intent"));
            }
            let end = Tick(tr.start.0 + duration);
            let bounds = filmcraft_edit::transitions::bounds(t, &tr).ok_or_else(|| error("invalid proposed transition endpoints"))?;
            if tr.start < bounds.0 || end > bounds.2 {
                return Err(error("transition exceeds its current clips"));
            }
            for existing in &t.transitions {
                if !(1..=SAFE).contains(&existing.duration.0)
                    || !(-SAFE..=SAFE).contains(&existing.start.0)
                    || existing.start.0.checked_add(existing.duration.0).is_none_or(|n| !(-SAFE..=SAFE).contains(&n))
                {
                    return Err(error("invalid existing transition clock"));
                }
                if existing.range().overlaps(&tr.range()) {
                    return Err(error("existing human transition overlaps; replacement needs explicit authoring support"));
                }
            }
            handles(s, c, tr.start, end)?;
            if let Some(n) = &neighbor {
                handles(s, target(s, n, &r.scope)?.1, tr.start, end)?;
            }
            let mut next = expected.next_id;
            if next == 0
                || next >= SAFE as u64
                || q.all_tracks().any(|t| t.id.0 == next || t.items.iter().any(|c| c.id.0 == next) || t.transitions.iter().any(|tr| tr.id.0 == next))
                || expected.item(ItemId(next)).is_some()
            {
                return Err(error("unsafe transition ID allocation"));
            }
            let absent = |_: ItemId| None;
            let zero = |_: ItemId| Tick::ZERO;
            let mut ctx = filmcraft_edit::EditCtx { next_id: &mut next, media_duration: &absent, media_start: &zero, min_duration: unit };
            filmcraft_edit::add_transition(expected.sequence_mut(sid).ok_or(crate::EngineError::NoSequence)?, track, tr, &mut ctx)?;
            expected.next_id = next;
            let review = if kind == TrackKind::Audio {
                audio_review(s, Tick(end.0 - duration), end, true)?
            } else {
                let first = rate.frame_at(Tick(end.0 - duration));
                if first < 0 || first.checked_add(frames).is_none_or(|n| n > 1_000_000) {
                    return Err(error("video transition exceeds review frame bounds"));
                }
                let count = frames.min(3);
                let points = (0..count).map(|i| first + if count == 1 { 0 } else { i * (frames - 1) / (count - 1) }).collect::<Vec<_>>();
                vec![json!({"kind":"frame","startFrame":first,"durationFrames":frames,"frames":points})]
            };
            (if kind == TrackKind::Video { "sequence.applyVideoTransition" } else { "sequence.applyAudioTransition" }, args, identities, review)
        }
    };
    let expected_bytes = project_audit::bytes(&expected)?;
    let result = s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != expected_bytes {
        return Err(error("human finishing changed unexpected project state; staging discarded"));
    }
    let mut budget = 96 * 1024;
    let contexts = identities.iter().map(|i| context(s, i, &mut budget)).collect::<Result<Vec<_>>>()?;
    Ok(json!({"command":command,"result":result,"contexts":contexts,"review":review}))
}
