//! Native execution building block for original Poema plans. Editorial choices
//! and waveform/output qualification belong to the caller, not this module.
//! One exact private operation uses the SAME Extract algebra as human edits.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ClipId, ClipIdentity, TrackId, TrackItem, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Operation, assembly, caption_ripple, condense_restore, parse};
use crate::{Result, Session, commands::bad};

pub(super) const COMMAND: &str = "agent.condense";
const SAFE: i64 = 9_007_199_254_740_991;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    project_id: String,
    receipt_sha256: String,
    version: u8,
    passages: Vec<Passage>,
    #[serde(default)]
    caption_basis: Option<caption_ripple::Basis>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Passage {
    clip: u64,
    identity: String,
    source_in: i64,
    source_out: i64,
    start: i64,
    kept: Vec<[i64; 2]>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if p.project_id.is_empty()
        || p.project_id.len() > 256
        || p.project_id.chars().any(char::is_control)
        || p.receipt_sha256.len() != 64
        || !p.receipt_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !(1..=5).contains(&p.version)
        || p.passages.is_empty()
        || p.passages.len() > 16
        || op.params.to_string().len() > 64 * 1024
        || p.caption_basis.as_ref().is_some_and(|b| !b.valid())
    {
        return Err(error("need explicit project, original receipt, version and 1–16 complete current passages"));
    }
    let mut ids = BTreeSet::new();
    let mut segments = 0usize;
    for b in &p.passages {
        segments += b.kept.len();
        if b.clip == 0
            || b.clip > SAFE as u64
            || !ids.insert(b.clip)
            || !ClipIdentity::valid_id(&b.identity)
            || !(0..=SAFE).contains(&b.start)
            || !(0..=SAFE).contains(&b.source_in)
            || !(1..=SAFE).contains(&b.source_out)
            || b.source_out <= b.source_in
            || b.start.checked_add(b.source_out - b.source_in).is_none_or(|n| n > SAFE)
            || b.kept.is_empty()
            || segments > 256
            || b.kept.iter().any(|[a, z]| *a < b.source_in || *z > b.source_out || z <= a)
            || b.kept.windows(2).any(|w| w[1][0] < w[0][1])
        {
            return Err(error("invalid/duplicate current identity, exact bounds or complete ordered kept segments (maximum 256)"));
        }
    }
    Ok(())
}

fn warp(time: Tick, ranges: &[TimeRange]) -> Tick {
    time - ranges.iter().filter(|r| r.end() <= time).fold(Tick::ZERO, |sum, r| sum + r.duration)
}
fn cuts(item: &TrackItem, ranges: &[TimeRange]) -> Vec<(Tick, Tick)> {
    let mut at = item.start;
    let mut out = Vec::new();
    for r in ranges.iter().filter(|r| r.start < item.end() && r.end() > item.start) {
        if at < r.start {
            out.push((at, r.start));
        }
        at = at.max(r.end());
    }
    if at < item.end() {
        out.push((at, item.end()));
    }
    out
}

// Native point parameters explicitly use NaN for "auto" (see ParamValue).
// Treat matching auto coordinates as equal without making auto equal to zero,
// a finite replacement, a different parameter type or changed keyframe data.
pub(super) fn same_item(a: &TrackItem, b: &TrackItem) -> bool {
    if a == b {
        return true;
    }
    if a.start != b.start || a.duration != b.duration || a.source_in != b.source_in {
        return false;
    }
    let (mut a, mut b) = (a.clone(), b.clone());
    let normalize = |a: &mut filmcraft_project::ParamValue, b: &mut filmcraft_project::ParamValue| {
        if let (filmcraft_project::ParamValue::Vec2(a), filmcraft_project::ParamValue::Vec2(b)) = (a, b) {
            if a.x.is_nan() && b.x.is_nan() {
                a.x = 0.0;
                b.x = 0.0;
            }
            if a.y.is_nan() && b.y.is_nan() {
                a.y = 0.0;
                b.y = 0.0;
            }
        }
    };
    for (ae, be) in a.effects.iter_mut().zip(&mut b.effects) {
        for (key, ap) in &mut ae.params {
            if let Some(bp) = be.params.get_mut(key) {
                normalize(&mut ap.value, &mut bp.value);
                for (a, b) in ap.keyframes.iter_mut().zip(&mut bp.keyframes) {
                    normalize(&mut a.value, &mut b.value);
                }
            }
        }
    }
    a == b
}

pub(super) fn forward_clock(c: &TrackItem) -> bool {
    c.speed == 1.0
        && !c.reverse
        && c.frame_hold.is_none()
        && !c.effects.iter().any(|e| {
            e.enabled
                && (e.effect == "remix"
                    || e.effect == "time_remap" && e.params.values().any(|v| !v.keyframes.is_empty() || v.value != filmcraft_project::ParamValue::Float(100.0)))
        })
}

pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    run_inner(s, op, None)
}
pub(super) fn run_recorded(s: &mut Session, op: &Operation, recorded: &[condense_restore::RecordedSpan]) -> Result<Value> {
    run_inner(s, op, Some(recorded))
}
fn run_inner(s: &mut Session, op: &Operation, recorded: Option<&[condense_restore::RecordedSpan]>) -> Result<Value> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    if recorded.is_some_and(|r| r.len() != p.passages.len()) {
        return Err(error("missing complete recorded passage correspondence"));
    }
    let before = s.project.clone();
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let old = before.sequence(seq).ok_or(crate::EngineError::NoSequence)?;
    old.settings.validate().map_err(error)?;
    let caption_witness = caption_ripple::prepare(s, p.caption_basis, &p.project_id)?;
    if before.items.len() > 100_000 {
        return Err(error("project audit is bounded to 100000 items"));
    }
    let count = old.all_tracks().flat_map(|t| &t.items).take(100_001).count();
    if count > 100_000 || old.all_tracks().flat_map(|t| &t.transitions).take(100_001).count() > 100_000 {
        return Err(error("condense exceeds the bounded native instance/transition audit"));
    }
    let originals: BTreeMap<_, _> = old.all_tracks().flat_map(|t| t.items.iter().map(move |c| (c.id, c))).collect();
    let mut targets = BTreeSet::new();
    let mut tracks = BTreeSet::new();
    let mut ranges = Vec::new();
    let mut owners = BTreeMap::new();
    let mut own_ranges = Vec::new();
    for (owner, b) in p.passages.iter().enumerate() {
        let first = ranges.len();
        let saved = recorded.and_then(|r| r.get(owner));
        let leader = if let Some(r) = saved {
            if r.identity != b.identity || r.original.id.0 != b.clip || r.members.is_empty() || r.members.len() > 128 {
                return Err(error("invalid recorded physical group"));
            }
            &r.original
        } else {
            old.find_item(ClipId(b.clip)).map(|(_, c)| c).ok_or_else(|| error("current passage is missing"))?
        };
        if leader.identity.as_ref().map(|i| i.id.as_str()) != Some(b.identity.as_str())
            || leader.start.0 != b.start
            || leader.source_in.0 != b.source_in
            || leader.source_out().0 != b.source_out
            || leader.duration.0 != b.source_out - b.source_in
        {
            return Err(error("passage does not match the current native instance and full laid span"));
        }
        for t in old.all_tracks() {
            for c in t
                .items
                .iter()
                .filter(|c| saved.map_or_else(|| c.id == leader.id || leader.link.is_some() && c.link == leader.link, |r| r.members.contains(&c.id)))
            {
                if !targets.insert(c.id) || targets.len() > 128 {
                    return Err(error("repeated or oversized linked groups; maximum 128 members"));
                }
                tracks.insert(t.id);
                owners.insert(c.id, owner);
            }
        }
        let mut at = b.source_in;
        for [a, z] in &b.kept {
            if *a > at {
                ranges.push(TimeRange::new(Tick(b.start + at - b.source_in), Tick(a - at)));
            }
            at = *z;
        }
        if at < b.source_out {
            ranges.push(TimeRange::new(Tick(b.start + at - b.source_in), Tick(b.source_out - at)));
        }
        own_ranges.push(ranges.get(first..).ok_or_else(|| error("missing owned removal range"))?.to_vec());
    }
    if recorded.is_none() {
        condense_restore::assert_unrecorded(s, &targets)?;
    }
    ranges.sort_by_key(|r| r.start);
    if ranges.is_empty() && recorded.is_none() || ranges.len() > 272 || ranges.windows(2).any(|w| w[0].end() > w[1].start) {
        return Err(error("need nonempty, disjoint exact removals; overlapping passages cannot silently change one another"));
    }
    // Bound every allocation the shared split algebra can make before entering
    // it; its infallible ID supplier must never reach overflow or unsafe JSON IDs.
    let allowance = (count as u64).checked_mul(ranges.len() as u64).and_then(|n| n.checked_mul(4)).ok_or_else(|| error("ID budget overflow"))?;
    if before.next_id.checked_add(allowance).is_none_or(|n| n > SAFE as u64) {
        return Err(error("native allocation counter exceeds the exact supported range"));
    }
    let mut expected: BTreeMap<ClipId, (TrackId, Vec<TrackItem>)> = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut total = 0usize;
    for t in old.all_tracks() {
        let affected = !t.locked && (tracks.contains(&t.id) || t.sync_lock);
        for c in &t.items {
            if !(0..=SAFE).contains(&c.start.0)
                || !(1..=SAFE).contains(&c.duration.0)
                || !(0..=SAFE).contains(&c.source_in.0)
                || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
            {
                return Err(error("existing instance exceeds exact supported timeline/source coordinates"));
            }
            if let Some(i) = &c.identity {
                origins.insert(i.id.as_str(), c.id);
            }
            let spans = if affected { cuts(c, &ranges) } else { vec![(c.start, c.end())] };
            let mut pieces = Vec::new();
            for (a, z) in spans {
                let mut piece = c.clone();
                if affected {
                    piece.start = warp(a, &ranges);
                    piece.duration = z - a;
                    // This is a readback expectation, NOT a second cut writer.
                    // New content cuts require a forward 1x source clock below.
                    if a != c.start {
                        piece.source_in += a - c.start;
                    }
                }
                pieces.push(piece);
            }
            let changed = pieces.len() != 1 || pieces.first().is_none_or(|piece| !same_item(piece, c));
            if changed || targets.contains(&c.id) {
                let media = before.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("changed material needs bound original media"))?;
                if t.locked
                    || c.identity.is_none()
                    || media.offline
                    || s.media.offline_status(c.item).is_some()
                    || assembly::binding(s, c.item)?.project_id != p.project_id
                {
                    return Err(error("cannot change or skip locked, untracked, offline or foreign material"));
                }
                if targets.contains(&c.id) && (!c.enabled || !forward_clock(c)) {
                    return Err(error("condensing requires forward 1x target clocks without hold/remix/time remapping"));
                }
                if targets.contains(&c.id)
                    && (c.source_in.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE || n > media.info.duration.0)
                        || t.kind == TrackKind::Video && pieces.iter().any(|x| x.duration < old.settings.frame_rate.frame_duration()))
                {
                    return Err(error("target exceeds original media or a retained picture fragment is shorter than one frame"));
                }
                if !targets.contains(&c.id) && (pieces.len() != 1 || pieces.first().is_none_or(|x| x.duration != c.duration || x.source_in != c.source_in)) {
                    return Err(error(format!("removal would cut unrelated instance {}, not merely ripple it", c.id.0)));
                }
            }
            if targets.contains(&c.id) {
                // Membership in a batch is not waveform qualification for a
                // different passage. Long J/L leads/tails can cross that
                // passage's removals outside their own leader's heard span.
                let owner = owners.get(&c.id).and_then(|n| own_ranges.get(*n)).ok_or_else(|| error("missing linked passage ownership"))?;
                if ranges.iter().any(|r| r.overlaps(&c.range()) && !owner.contains(r)) {
                    return Err(error("another passage would cut this linked member outside its own qualified removals"));
                }
                if !affected {
                    return Err(error("every linked target track must participate"));
                }
                for r in &ranges {
                    for edge in [r.start, r.end()].into_iter().filter(|edge| c.start < *edge && *edge < c.end()) {
                        let source = c.source_in.0.checked_add(edge.0 - c.start.0).ok_or_else(|| error("source clock overflow"))?;
                        if t.kind == TrackKind::Video {
                            if edge.0 % old.settings.frame_rate.frame_duration().0 != 0 {
                                return Err(error("introduced picture edge is not an exact native frame"));
                            }
                        } else {
                            let media = before.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("missing audio source"))?;
                            let hz = media
                                .info
                                .audio_streams
                                .get(c.audio_stream)
                                .map(|a| a.sample_rate)
                                .filter(|n| *n > 0)
                                .ok_or_else(|| error("missing original audio stream clock"))?;
                            if i128::from(source) * i128::from(hz) % i128::from(TICKS_PER_SECOND) != 0 {
                                return Err(error("introduced audio edge is not an exact original sample"));
                            }
                        }
                    }
                }
            }
            if affected && t.transitions.iter().any(|x| x.from == Some(c.id) && targets.contains(&c.id) || x.to == Some(c.id) && targets.contains(&c.id)) {
                return Err(error("attached target transitions need candidate-output qualification"));
            }
            total += pieces.len();
            if total > 100_000 {
                return Err(error("candidate exceeds 100000 resulting instances"));
            }
            expected.insert(c.id, (t.id, pieces));
        }
        if affected && t.transitions.iter().any(|x| ranges.iter().any(|r| x.start < r.end() && x.end() > r.start)) {
            return Err(error("a removal crosses a transition"));
        }
    }
    // Closing one range through one linked group must not cut another selected
    // passage's kept region; the independent full-span request is authoritative.
    for b in &p.passages {
        for [a, z] in &b.kept {
            let kept = TimeRange::new(Tick(b.start + a - b.source_in), Tick(z - a));
            if ranges.iter().any(|r| r.overlaps(&kept)) {
                return Err(error("another passage would remove a requested kept segment"));
            }
        }
    }
    let track_ids: Vec<_> = tracks.into_iter().collect();
    s.edit_sequence("Condense Original Dialogue", |q, ctx, st| {
        for r in ranges.iter().rev() {
            filmcraft_edit::extract(q, &track_ids, *r, ctx);
            if st.ripple_sequence_markers {
                crate::sequence_tools::ripple_markers(&mut q.markers, r.end(), -r.duration);
            }
        }
        Ok(())
    })?;
    let after = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut expected_captions = old.caption_tracks.clone();
    for range in ranges.iter().rev() {
        for track in expected_captions.iter_mut().filter(|t| !t.locked && t.sync_lock) {
            filmcraft_edit::captions::extract_range(track, *range);
        }
    }
    if after.caption_tracks != expected_captions {
        return Err(error("condense caption state differs from shared exact Extract timing"));
    }
    let mut found: BTreeMap<ClipId, usize> = BTreeMap::new();
    let mut link_regions = BTreeMap::new();
    let mut region_links = BTreeMap::new();
    for t in after.all_tracks() {
        for c in &t.items {
            let origin = if expected.contains_key(&c.id) {
                Some(c.id)
            } else {
                c.identity.as_ref().and_then(|i| i.parent.as_deref()).and_then(|id| origins.get(id).copied())
            }
            .ok_or_else(|| error("candidate introduced an instance without native original lineage"))?;
            let (track, pieces) = expected.get(&origin).ok_or_else(|| error("missing expected original"))?;
            let index = found.entry(origin).or_default();
            let mut wanted = pieces.get(*index).cloned().ok_or_else(|| error("candidate duplicated a retained segment"))?;
            *index += 1;
            let old_clip = originals.get(&origin).copied().ok_or_else(|| error("missing original instance"))?;
            if c.identity.as_ref().map(|i| (&i.annotations, &i.protection)) != old_clip.identity.as_ref().map(|i| (&i.annotations, &i.protection)) {
                return Err(error("candidate changed original annotations or protection"));
            }
            wanted.id = c.id;
            wanted.identity = c.identity.clone();
            wanted.link = c.link;
            if *track != t.id || !same_item(&wanted, c) {
                return Err(error("native Extract differed from exact kept content, routing, effects or collateral expectations"));
            }
            if let Some(link) = old_clip.link {
                let old_start = c.source_in - old_clip.source_in + old_clip.start;
                let region = ranges.iter().filter(|r| r.end() <= old_start).count();
                let group = (link, region);
                let new_link = c.link.ok_or_else(|| error("candidate detached linked material"))?;
                if link_regions.insert(new_link, group).is_some_and(|old| old != group)
                    || region_links.insert(group, new_link).is_some_and(|old| old != new_link)
                {
                    return Err(error("candidate joined different fragments or detached a J/L partner"));
                }
            }
        }
    }
    if expected.iter().any(|(id, (_, pieces))| found.get(id).copied().unwrap_or(0) != pieces.len()) {
        return Err(error("candidate lost a complete retained segment"));
    }
    // Inspection coordinates come from the SAME audited native extraction,
    // not a second JavaScript ripple clock or the original full source length.
    let passage_ranges = p
        .passages
        .iter()
        .enumerate()
        .map(|(n, b)| {
            let ids = recorded.and_then(|r| r.get(n)).map(|r| r.leader_pieces.clone()).unwrap_or_else(|| BTreeSet::from([ClipId(b.clip)]));
            let pieces =
                ids.iter().map(|id| expected.get(id).map(|(_, p)| p)).collect::<Option<Vec<_>>>().ok_or_else(|| error("missing inspected logical passage"))?;
            let start = pieces.iter().flat_map(|p| p.iter()).map(|c| c.start.0).min().ok_or_else(|| error("empty inspected logical passage"))?;
            let end = pieces.iter().flat_map(|p| p.iter()).map(|c| c.end().0).max().ok_or_else(|| error("empty inspected logical passage"))?;
            Ok(json!({"identity":b.identity,"startTicks":start,"endTicks":end}))
        })
        .collect::<Result<Vec<_>>>()?;
    let leaders: Vec<_> = p.passages.iter().map(|p| p.identity.clone()).collect();
    let restorations = if recorded.is_some() {
        condense_restore::capture_recorded(s, &before, &owners, &own_ranges, &ranges, &leaders, &p.project_id, &p.receipt_sha256, p.version)?
    } else {
        condense_restore::capture(s, &before, &owners, &own_ranges, &ranges, &leaders, &p.project_id, &p.receipt_sha256, p.version)?
    };
    let mut result = json!({"condensed":p.passages.len(),"version":p.version,"originalReceiptSha256":p.receipt_sha256,"restorations":restorations,
        "joinTicks":ranges.iter().map(|r|warp(r.start,&ranges).0).collect::<BTreeSet<_>>(),
        "passageRanges":passage_ranges,
        "removedTicks":ranges.iter().fold(Tick::ZERO,|n,r|n+r.duration).0,"removals":ranges.len(),"instances":total});
    if let Some(proof) = caption_ripple::finish_condense(s, caption_witness, "condense")? {
        result["captionEdit"] = proof;
    }
    Ok(result)
}
