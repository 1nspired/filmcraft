//! Saved per-passage inverse edits. Never restore a project/history snapshot or
//! reconstruct retained finishing from an old template. Shared native gap/trim
//! operations preserve current pieces; only wholly removed members use templates.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use filmcraft_edit::{Edge, TrimMode};
use filmcraft_project::{ClipId, ClipIdentity, ItemId, Project, TrackId, TrackItem, TrackKind};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Operation, assembly, caption_ripple, condense, condense_captions, parse};
use crate::{Result, Session, commands::bad};

pub(super) const COMMAND: &str = "agent.restoreCondense";
pub(super) const CHOOSE: &str = "agent.chooseCondense";
#[path = "condense_inputs.rs"]
mod inputs;
#[path = "condense_versions.rs"]
mod versions;
pub(super) use inputs::{assert_unrecorded, read as read_inputs};
pub(super) use versions::{RecordedSpan, capture_recorded, run_choice, validate_choice};
const KEY: &str = "poema.condense.restore.v1";
const SAFE: i64 = 9_007_199_254_740_991;
const BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Piece {
    clip: ClipId,
    identity: String,
    start: Tick,
    duration: Tick,
    source_in: Tick,
    link: Option<u64>,
}
impl Piece {
    fn of(c: &TrackItem) -> Result<Self> {
        Ok(Self {
            clip: c.id,
            identity: c.identity.as_ref().ok_or_else(|| error("missing retained identity"))?.id.clone(),
            start: c.start,
            duration: c.duration,
            source_in: c.source_in,
            link: c.link,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Member {
    track: TrackId,
    kind: TrackKind,
    original: TrackItem,
    full_start: Tick,
    source: Value,
    pieces: Vec<Piece>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}
// Immutable physical-member basis, distinct from the finishing partitions
// which can survive, split or become wholly hidden in later choices.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BasisMember {
    track: TrackId,
    kind: TrackKind,
    original: TrackItem,
    full_start: Tick,
    source: Value,
    input: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    project_id: String,
    sequence: ItemId,
    identity: String,
    receipt_sha256: String,
    version: u8,
    full: bool,
    rate: FrameRate,
    sample_rate: u32,
    gaps: Vec<TimeRange>,
    members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    basis: Vec<BasisMember>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    caption_gaps: Vec<condense_captions::Gap>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u8,
    records: Vec<Record>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    project_id: String,
    identity: String,
    receipt_sha256: String,
    #[serde(default)]
    caption_basis: Option<caption_ripple::Basis>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
fn positive(n: i64) -> bool {
    (1..=SAFE).contains(&n)
}
fn range(a: Tick, d: Tick) -> bool {
    (0..=SAFE).contains(&a.0) && positive(d.0) && a.0.checked_add(d.0).is_some_and(|n| n <= SAFE)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn project(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}

// A saved passage may need originals no longer present in its visible pieces.
// Share the bounded native record parser with source dependency selection;
// metadata alone never qualifies those originals or authorizes restoration.
pub(super) fn source_dependencies(p: &Project, sequence: ItemId) -> Result<Vec<ItemId>> {
    let store = load(p, sequence)?;
    let sources: BTreeSet<_> =
        store.records.iter().flat_map(|r| r.members.iter().map(|m| m.original.item).chain(r.basis.iter().map(|m| m.original.item))).collect();
    Ok(sources.into_iter().collect())
}

fn load(p: &Project, sequence: ItemId) -> Result<Store> {
    let Some(raw) = p.item(sequence).and_then(|i| i.metadata.get(KEY)) else {
        return Ok(Store { version: 1, records: vec![] });
    };
    if raw.len() > BYTES {
        return Err(error("saved restoration exceeds its byte budget"));
    }
    let store: Store = serde_json::from_str(raw).map_err(|e| error(format!("invalid saved restoration: {e}")))?;
    if ![1, 2, 3].contains(&store.version) || store.records.len() > 32 {
        return Err(error("unsupported or oversized restoration records"));
    }
    let mut leaders = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut piece_ids = BTreeSet::new();
    let mut original_ids = BTreeSet::new();
    let mut count = 0usize;
    for r in &store.records {
        if !project(&r.project_id)
            || r.sequence != sequence
            || !ClipIdentity::valid_id(&r.identity)
            || !leaders.insert(&r.identity)
            || !hash(&r.receipt_sha256)
            || !(1..=5).contains(&r.version)
            || r.rate.num <= 0
            || r.rate.den <= 0
            || r.sample_rate == 0
            || r.gaps.is_empty()
            || r.gaps.len() > 272
            || r.gaps.iter().any(|g| !range(g.start, g.duration))
            || r.gaps.windows(2).any(|g| g[0].start >= g[1].start)
            || r.gaps.iter().try_fold(0i64, |n, g| n.checked_add(g.duration.0)).is_none_or(|n| n > SAFE)
            || r.members.is_empty()
            || r.members.len() > 128
            || !(r.members.iter().any(|m| m.original.identity.as_ref().is_some_and(|i| i.id == r.identity))
                || r.basis.iter().any(|m| m.original.identity.as_ref().is_some_and(|i| i.id == r.identity)))
        {
            return Err(error("invalid saved passage scope, clocks, gaps or members"));
        }
        for m in &r.members {
            let c = &m.original;
            if m.track.0 == 0
                || m.track.0 > SAFE as u64
                || c.id.0 == 0
                || c.id.0 > SAFE as u64
                || c.item.0 == 0
                || c.item.0 > SAFE as u64
                || c.link.is_some_and(|n| n == 0 || n > SAFE as u64)
                || !original_ids.insert(c.id)
                || !range(c.start, c.duration)
                || !range(c.source_in, c.duration)
                || !range(m.full_start, c.duration)
                || c.identity.as_ref().is_none_or(|i| i.validate().is_err())
                || !condense::forward_clock(c)
                || m.pieces.len() > 512
                || m.pieces.windows(2).any(|p| p[0].source_in.0.checked_add(p[0].duration.0).is_none_or(|end| end > p[1].source_in.0))
            {
                return Err(error("invalid saved original member"));
            }
            for piece in &m.pieces {
                count += 1;
                if count > 4096
                    || piece.clip.0 == 0
                    || piece.clip.0 > SAFE as u64
                    || !piece_ids.insert(piece.clip)
                    || piece.link.is_some_and(|n| n == 0 || n > SAFE as u64)
                    || !ClipIdentity::valid_id(&piece.identity)
                    || !identities.insert(&piece.identity)
                    || !range(piece.start, piece.duration)
                    || !range(piece.source_in, piece.duration)
                    || piece.source_in < c.source_in
                    || piece.source_in + piece.duration > c.source_out()
                {
                    return Err(error("invalid or oversized saved retained pieces"));
                }
            }
        }
        versions::validate_basis(r)?;
        condense_captions::validate(&r.caption_gaps, &r.gaps)?;
        let leader = versions::leader(r)?;
        if r.members.iter().any(|m| m.original.link != leader.original.link)
            || leader.original.link.is_none() && (if r.basis.is_empty() { r.members.len() } else { r.basis.len() }) != 1
        {
            return Err(error("saved original link ownership is inconsistent"));
        }
        if !r.full {
            let pieces = versions::leader_pieces(r)?;
            if pieces.is_empty() {
                return Err(error("saved leader has no surviving source anchor"));
            }
            let mut at = leader.original.source_in;
            let mut removed = Tick::ZERO;
            let mut gaps = Vec::new();
            for p in &pieces {
                if p.source_in > at {
                    let duration = p.source_in - at;
                    gaps.push(TimeRange::new(leader.full_start + at - leader.original.source_in - removed, duration));
                    removed += duration;
                }
                if p.start != leader.full_start + p.source_in - leader.original.source_in - removed {
                    return Err(error("saved source/timeline restoration geometry is inconsistent"));
                }
                at = p.source_in + p.duration;
            }
            if at < leader.original.source_out() {
                gaps.push(TimeRange::new(leader.full_start + at - leader.original.source_in - removed, leader.original.source_out() - at));
            }
            if gaps != r.gaps {
                return Err(error("saved restoration gaps do not match complete retained source content"));
            }
        }
    }
    Ok(store)
}
fn save(s: &mut Session, sequence: ItemId, store: &Store) -> Result<()> {
    let version = if store.records.iter().any(|r| !r.caption_gaps.is_empty()) { 3 } else { 2 };
    let raw = serde_json::to_string(&Store { version, records: store.records.clone() }).map_err(|e| error(e.to_string()))?;
    if raw.len() > BYTES {
        return Err(error("complete restoration records exceed four MiB; nothing was truncated"));
    }
    Arc::make_mut(&mut s.project).item_mut(sequence).ok_or(crate::EngineError::NoSequence)?.metadata.insert(KEY.into(), raw);
    load(&s.project, sequence)?;
    Ok(())
}
fn source(s: &Session, item: ItemId, owner: &str) -> Result<Value> {
    let it = s.project.item(item).ok_or_else(|| error("missing original source"))?;
    let m = it.as_media().ok_or_else(|| error("restoration needs original media"))?;
    if m.info.audio_streams.len() > filmcraft_media::MAX_AUDIO_STREAMS {
        return Err(error("restoration source stream metadata exceeds its bound"));
    }
    if m.offline || s.media.offline_status(item).is_some() || assembly::binding(s, item)?.project_id != owner {
        return Err(error("original is offline, unbound or belongs to another film"));
    }
    Ok(json!({"media":m.media,"duration":m.info.duration,"videoRate":m.info.video.as_ref().map(|v|v.frame_rate),
        "audioStreams":m.info.audio_streams,"binding":it.metadata.get(super::BINDING_KEY)}))
}
fn warp(t: Tick, gaps: &[TimeRange]) -> Tick {
    t - gaps.iter().filter(|g| g.end() <= t).fold(Tick::ZERO, |n, g| n + g.duration)
}

pub(super) fn capture(
    s: &mut Session,
    before: &Project,
    owners: &BTreeMap<ClipId, usize>,
    own: &[Vec<TimeRange>],
    all: &[TimeRange],
    leaders: &[String],
    owner: &str,
    receipt: &str,
    version: u8,
) -> Result<Value> {
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let old = before.sequence(sequence).ok_or(crate::EngineError::NoSequence)?;
    let previous_ids: BTreeSet<_> = old.all_tracks().flat_map(|t| t.items.iter().map(|c| c.id)).collect();
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut store = load(before, sequence)?;
    for record in &mut store.records {
        condense_captions::refresh(&mut record.caption_gaps, old, q);
    }
    let claimed: BTreeSet<_> = store.records.iter().flat_map(|r| &r.members).flat_map(|m| &m.pieces).map(|p| p.clip).collect();
    if owners.keys().any(|id| claimed.contains(id)) || store.records.len() + own.iter().filter(|r| !r.is_empty()).count() > 32 {
        return Err(error("passage already has a restoration record or the record budget is full; use its version workflow"));
    }
    let mut summaries = Vec::new();
    for (n, identity) in leaders.iter().enumerate() {
        let removals = own.get(n).ok_or_else(|| error("missing own removals"))?;
        // A completely unchanged passage needs no inverse operation.
        if removals.is_empty() {
            continue;
        }
        let foreign: Vec<_> = all.iter().copied().filter(|r| !removals.contains(r)).collect();
        let mut members = Vec::new();
        for t in old.all_tracks() {
            for c in t.items.iter().filter(|c| owners.get(&c.id) == Some(&n)) {
                let parent = c.identity.as_ref().ok_or_else(|| error("missing original identity"))?.id.as_str();
                let mut pieces: Vec<_> = q
                    .all_tracks()
                    .flat_map(|t| &t.items)
                    .filter(|p| p.id == c.id || !previous_ids.contains(&p.id) && p.identity.as_ref().and_then(|i| i.parent.as_deref()) == Some(parent))
                    .map(Piece::of)
                    .collect::<Result<_>>()?;
                pieces.sort_by_key(|p| p.source_in);
                members.push(Member {
                    track: t.id,
                    kind: t.kind,
                    original: c.clone(),
                    full_start: warp(c.start, &foreign),
                    source: source(s, c.item, owner)?,
                    pieces,
                    origin: None,
                });
            }
        }
        let basis = members
            .iter()
            .map(|m| {
                Ok(BasisMember {
                    track: m.track,
                    kind: m.kind,
                    original: m.original.clone(),
                    full_start: m.full_start,
                    source: m.source.clone(),
                    input: versions::input(s, old, m.track, &m.original)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        store.records.push(Record {
            project_id: owner.into(),
            sequence,
            identity: identity.clone(),
            receipt_sha256: receipt.into(),
            version,
            full: false,
            rate: q.settings.frame_rate,
            sample_rate: q.settings.sample_rate,
            gaps: removals.iter().map(|r| TimeRange::new(warp(r.start, all), r.duration)).collect(),
            members,
            basis,
            caption_gaps: condense_captions::capture(old, q, removals)?,
        });
        summaries.push(json!({"identity":identity,"receiptSha256":receipt,"recordedState":"condensed","needsCurrentValidation":true}));
    }
    save(s, sequence, &store)?;
    Ok(json!(summaries))
}

pub(super) fn catalog(s: &Session, sequence: ItemId) -> Result<Value> {
    Ok(json!(
        load(&s.project, sequence)?
            .records
            .iter()
            .map(|r| {
                let leader = versions::leader(r)?;
                Ok(json!({"identity":r.identity,"projectId":r.project_id,
        "receiptSha256":r.receipt_sha256,"condenseVersion":r.version,"selection":match r.version {4=>json!("full"),5=>json!("tight"),_=>json!({"alternative":r.version})},"recordedState":if r.full {"original_native_span"}else{"condensed"},
        "members":r.members.len(),"needsCurrentValidation":true,"versionChoicesAvailable":!r.basis.is_empty(),
        "sourceIn":leader.original.source_in.0,"sourceOut":leader.original.source_out().0,
        "memberIdentities":inputs::member_identities(r)}))
            })
            .collect::<Result<Vec<_>>>()?
    ))
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    if !project(&r.project_id) || !ClipIdentity::valid_id(&r.identity) || !hash(&r.receipt_sha256) || r.caption_basis.as_ref().is_some_and(|b| !b.valid()) {
        return Err(error("explicit saved passage and original receipt required"));
    }
    Ok(())
}

pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let request: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let caption_witness = caption_ripple::prepare(s, request.caption_basis, &request.project_id)?;
    let mut store = load(&s.project, sequence)?;
    let index = store
        .records
        .iter()
        .position(|r| r.identity == request.identity)
        .ok_or_else(|| error("no saved native passage; historical plans cannot fabricate restoration"))?;
    let r = store.records.get(index).cloned().ok_or_else(|| error("missing restoration record"))?;
    if r.full || r.project_id != request.project_id || r.receipt_sha256 != request.receipt_sha256 {
        return Err(error("passage is already full or belongs to another plan/film"));
    }
    let before = s.project.clone();
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    if q.settings.frame_rate != r.rate
        || q.settings.sample_rate != r.sample_rate
        || before.items.len() > 100000
        || q.all_tracks().take(1025).count() > 1024
        || q.all_tracks().flat_map(|t| &t.items).take(100001).count() > 100000
        || before.next_id.checked_add(1024 + (q.caption_tracks.len() as u64) * 272).is_none_or(|n| n > SAFE as u64)
    {
        return Err(error("changed clocks or native size/allocation limits prevent restoration"));
    }
    let tracks: BTreeSet<_> = r.members.iter().map(|m| m.track).collect();
    let mut ids = BTreeSet::new();
    let mut delta = None;
    let mut current = BTreeMap::new();
    let mut links = BTreeSet::new();
    for m in &r.members {
        if q.track(m.track).is_none_or(|t| t.locked || t.kind != m.kind) || source(s, m.original.item, &r.project_id)? != m.source {
            return Err(error("original track/source changed or is locked"));
        }
        let media = before.item(m.original.item).and_then(|i| i.as_media()).ok_or_else(|| error("missing original media"))?;
        if m.original.source_out() > media.info.duration
            || m.kind == TrackKind::Video && !media.info.has_video()
            || m.kind == TrackKind::Audio
                && media.info.audio_streams.get(m.original.audio_stream).is_none_or(|a| {
                    a.sample_rate == 0
                        || a.channels == 0
                        || m.original.source_channels.len() > 64
                        || m.original.source_channels.iter().any(|c| u32::from(*c) >= a.channels)
                })
        {
            return Err(error("saved original member exceeds current media/stream/channel bounds"));
        }
        if let Some(l) = m.original.link {
            links.insert(l);
        }
        for p in &m.pieces {
            let (track, c) = q.find_item(p.clip).ok_or_else(|| error("a retained passage fragment was removed or reshaped"))?;
            let shift = c.start.0.checked_sub(p.start.0).ok_or_else(|| error("passage offset overflow"))?;
            if track != m.track
                || c.identity.as_ref().map(|i| i.id.as_str()) != Some(p.identity.as_str())
                || c.item != m.original.item
                || c.source_in != p.source_in
                || c.duration != p.duration
                || c.link != p.link
                || !condense::forward_clock(c)
                || c.audio_stream != m.original.audio_stream
                || delta.is_some_and(|d| d != shift)
                || !ids.insert(c.id)
            {
                return Err(error("passage source geometry, clock or links were reshaped; no human work was replaced"));
            }
            if m.kind == TrackKind::Audio
                && media
                    .info
                    .audio_streams
                    .get(c.audio_stream)
                    .is_none_or(|a| c.source_channels.len() > 64 || c.source_channels.iter().any(|ch| u32::from(*ch) >= a.channels))
            {
                return Err(error("current audio channel routing exceeds its original stream"));
            }
            delta = Some(shift);
            if let Some(l) = c.link {
                links.insert(l);
            }
            current.insert(c.id, c.clone());
        }
    }
    let delta = delta.ok_or_else(|| error("the passage has no surviving anchor"))?;
    let add =
        |a: Tick, d: i64| a.0.checked_add(d).filter(|n| (0..=SAFE).contains(n)).map(Tick).ok_or_else(|| error("restoration exceeds exact timeline bounds"));
    let gaps: Vec<_> = r.gaps.iter().map(|g| Ok(TimeRange::new(add(g.start, delta)?, g.duration))).collect::<Result<_>>()?;
    condense_captions::assert_locations(&before, sequence, &r.caption_gaps)?;
    let mut expected_captions = q.caption_tracks.clone();
    let mut caption_next = before.next_id;
    for (n, gap) in gaps.iter().enumerate().rev() {
        let mut ctx = filmcraft_edit::EditCtx {
            next_id: &mut caption_next,
            media_duration: &|_| None,
            media_start: &|_| Tick::ZERO,
            min_duration: r.rate.frame_duration(),
        };
        condense_captions::restore_gap(&mut expected_captions, *gap, r.caption_gaps.get(n), &mut ctx)?;
    }
    let mut expected = BTreeMap::new();
    let mut link_shifts = BTreeMap::new();
    let mut expected_transitions = BTreeMap::new();
    for t in q.all_tracks() {
        let affected = !t.locked && (tracks.contains(&t.id) || t.sync_lock);
        if t.transitions.len() > 100000 {
            return Err(error("restoration transition audit exceeds its bound"));
        }
        let mut transitions = t.transitions.clone();
        for tr in &mut transitions {
            if !range(tr.start, tr.duration) {
                return Err(error("existing transition exceeds exact bounds"));
            }
            if affected
                && (tr.from.is_some_and(|id| ids.contains(&id))
                    || tr.to.is_some_and(|id| ids.contains(&id))
                    || gaps.iter().any(|g| tr.start < g.start && tr.end() > g.start))
            {
                return Err(error("restoration intersects a transition needing explicit qualification"));
            }
            let shift = if affected { gaps.iter().filter(|g| g.start <= tr.start).fold(0i64, |n, g| n + g.duration.0) } else { 0 };
            tr.start = add(tr.start, shift)?;
            if !range(tr.start, tr.duration) {
                return Err(error("shifted transition exceeds exact bounds"));
            }
        }
        expected_transitions.insert(t.id, transitions);
        for c in &t.items {
            if !range(c.start, c.duration) || !range(c.source_in, c.duration) {
                return Err(error("existing clip exceeds exact native bounds"));
            }
            if c.link.is_some_and(|l| links.contains(&l)) && !ids.contains(&c.id) {
                return Err(error("another human clip joined this passage's link group"));
            }
            if affected && gaps.iter().any(|g| c.start < g.start && c.end() > g.start) {
                return Err(error("restoration would split existing material; no cut was made"));
            }
            if ids.contains(&c.id) {
                continue;
            }
            let shift = if affected {
                gaps.iter().filter(|g| g.start <= c.start).try_fold(0i64, |n, g| n.checked_add(g.duration.0).ok_or_else(|| error("ripple overflow")))?
            } else {
                0
            };
            let mut wanted = c.clone();
            wanted.start = add(c.start, shift)?;
            if !range(wanted.start, wanted.duration) {
                return Err(error("ripple exceeds native bounds"));
            }
            if shift != 0 && (c.identity.is_none() || source(s, c.item, &r.project_id).is_err()) {
                return Err(error("cannot ripple untracked, offline or foreign material"));
            }
            if let Some(link) = c.link
                && link_shifts.insert(link, shift).is_some_and(|old| old != shift)
            {
                return Err(error("restoration would desynchronize unrelated linked material"));
            }
            expected.insert(c.id, (t.id, wanted));
        }
    }
    if q.markers.len() > 100000 {
        return Err(error("restoration marker audit exceeds its bound"));
    }
    let mut expected_markers = q.markers.clone();
    for marker in &expected_markers {
        let shift = if s.state.ripple_sequence_markers { gaps.iter().filter(|g| g.start <= marker.start).fold(0i64, |n, g| n + g.duration.0) } else { 0 };
        let start = add(marker.start, shift)?;
        if marker.duration.0 < 0 || start.0.checked_add(marker.duration.0).is_none_or(|n| n > SAFE) {
            return Err(error("restored marker exceeds exact bounds"));
        }
    }
    if s.state.ripple_sequence_markers {
        for gap in gaps.iter().rev() {
            crate::sequence_tools::ripple_markers(&mut expected_markers, gap.start, gap.duration);
        }
    }
    // Calculate exact retained expectations independently of the native edit.
    let mut desired = BTreeMap::new();
    for m in &r.members {
        let full_start = add(m.full_start, delta)?;
        if !range(full_start, m.original.duration) {
            return Err(error("restored full member exceeds exact bounds"));
        }
        for (n, p) in m.pieces.iter().enumerate() {
            let mut c = current.get(&p.clip).cloned().ok_or_else(|| error("missing current retained member"))?;
            c.source_in = if n == 0 { m.original.source_in } else { p.source_in };
            let end = m.pieces.get(n + 1).map_or(m.original.source_out(), |p| p.source_in);
            c.duration = end - c.source_in;
            c.start = add(full_start, c.source_in.0 - m.original.source_in.0)?;
            c.link = m.original.link;
            if !range(c.start, c.duration) {
                return Err(error("invalid full native member bounds"));
            }
            desired.insert(c.id, c);
        }
    }
    let restored = s.edit_sequence("Restore Full Original Dialogue", |q, ctx, st| {
        for (n, gap) in gaps.iter().enumerate().rev() {
            let mut split_links = HashMap::new();
            for track in q.all_tracks_mut().filter(|t| !t.locked && (tracks.contains(&t.id) || t.sync_lock)) {
                filmcraft_edit::insert_track_gap(track, gap.start, gap.duration, ctx, &mut split_links);
            }
            condense_captions::restore_gap(&mut q.caption_tracks, *gap, r.caption_gaps.get(n), ctx)?;
            if st.ripple_sequence_markers {
                crate::sequence_tools::ripple_markers(&mut q.markers, gap.start, gap.duration);
            }
        }
        let mut restored = Vec::new();
        for m in &r.members {
            let mut member_ids = Vec::new();
            if m.pieces.is_empty() {
                let mut c = m.original.clone();
                c.id = ClipId(ctx.alloc());
                c.start = add(m.full_start, delta)?;
                if !filmcraft_edit::track_range_empty(q.track(m.track).ok_or_else(|| error("missing member track"))?, c.range()) {
                    return Err(error("restored original would overwrite human work"));
                }
                member_ids.push(c.id);
                filmcraft_edit::overwrite(q, vec![(m.track, c)], ctx).map_err(|e| error(e.to_string()))?;
            } else {
                // Tails first, then the head, matching original Poema's full restoration.
                for p in m.pieces.iter().rev() {
                    let wanted = desired.get(&p.clip).ok_or_else(|| error("missing full expectation"))?;
                    let c = q.find_item(p.clip).map(|(_, c)| c).ok_or_else(|| error("missing retained native fragment"))?;
                    let tail = wanted.source_out() - c.source_out();
                    if tail != Tick::ZERO && filmcraft_edit::trim(q, p.clip, Edge::Out, TrimMode::Regular, tail, ctx).map_err(|e| error(e.to_string()))? != tail
                    {
                        return Err(error("full tail restoration would clamp"));
                    }
                    let c = q.find_item(p.clip).map(|(_, c)| c).ok_or_else(|| error("missing retained native fragment"))?;
                    let head = wanted.source_in - c.source_in;
                    if head != Tick::ZERO && filmcraft_edit::trim(q, p.clip, Edge::In, TrimMode::Regular, head, ctx).map_err(|e| error(e.to_string()))? != head
                    {
                        return Err(error("full head restoration would clamp"));
                    }
                    q.find_item_mut(p.clip).ok_or_else(|| error("missing restored member"))?.1.link = m.original.link;
                }
                member_ids.extend(m.pieces.iter().map(|p| p.clip));
            }
            restored.push(member_ids);
        }
        Ok(restored)
    })?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let expected_allocations = caption_next
        .checked_add(r.members.iter().filter(|m| m.pieces.is_empty()).count() as u64)
        .ok_or_else(|| error("caption/media restoration allocation overflow"))?;
    if q.caption_tracks != expected_captions || s.project.next_id != expected_allocations {
        return Err(error("caption/media restoration differed from exact inverse metadata or allocation"));
    }
    if q.markers != expected_markers || q.all_tracks().any(|t| expected_transitions.get(&t.id) != Some(&t.transitions)) {
        return Err(error("native restore differed from marker/transition preservation expectations"));
    }
    let mut allowed: BTreeSet<_> = expected.keys().copied().collect();
    for (m, member_ids) in r.members.iter().zip(&restored) {
        for id in member_ids {
            allowed.insert(*id);
            let (track, c) = q.find_item(*id).ok_or_else(|| error("native restore lost a full member"))?;
            let wanted = if let Some(w) = desired.get(id) {
                w.clone()
            } else {
                let mut w = m.original.clone();
                w.id = *id;
                w.identity = c.identity.clone();
                w.start = add(m.full_start, delta)?;
                w
            };
            if track != m.track || !condense::same_item(c, &wanted) {
                return Err(error("native full restoration differed from retained content/finishing expectations"));
            }
        }
    }
    for (id, (track, wanted)) in &expected {
        if q.find_item(*id).is_none_or(|(t, c)| t != *track || !condense::same_item(c, wanted)) {
            return Err(error("native restore changed unrelated content or finishing"));
        }
    }
    if q.all_tracks().flat_map(|t| &t.items).any(|c| !allowed.contains(&c.id)) {
        return Err(error("native restore introduced unowned material"));
    }
    // Exact inspection coordinates come from the audited restored native graph,
    // including all J/L members and current fragment finishing boundaries.
    // They describe proposed output, not waveform or semantic qualification.
    let mut boundaries = BTreeSet::new();
    for id in restored.iter().flatten() {
        let (_, c) = q.find_item(*id).ok_or_else(|| error("missing restored inspection member"))?;
        boundaries.insert(c.start.0);
        boundaries.insert(c.end().0);
    }
    let start = boundaries.first().copied().ok_or_else(|| error("missing restored inspection range"))?;
    let end = boundaries.last().copied().ok_or_else(|| error("missing restored inspection range"))?;
    let record = store.records.get_mut(index).ok_or_else(|| error("missing saved passage"))?;
    record.full = true;
    record.gaps = gaps.clone();
    for b in &mut record.basis {
        b.full_start = add(b.full_start, delta)?;
    }
    for (m, member_ids) in record.members.iter_mut().zip(&restored) {
        m.full_start = add(m.full_start, delta)?;
        m.pieces = member_ids.iter().map(|id| Piece::of(q.find_item(*id).ok_or_else(|| error("missing saved restored identity"))?.1)).collect::<Result<_>>()?;
    }
    save(s, sequence, &store)?;
    let mut result = json!({"restored":"original_native_span","identity":r.identity,"originalReceiptSha256":r.receipt_sha256,"members":r.members.len(),
        "insertedTicks":gaps.iter().fold(Tick::ZERO,|n,g|n+g.duration).0,"retainedFinishing":true,"retainedPieces":true,"canonicalVersionSelected":false,
        "restorationRange":{"startTicks":start,"endTicks":end},"boundaryTicks":boundaries});
    if let Some(proof) = caption_ripple::finish_condense(s, caption_witness, "restore")? {
        result["captionEdit"] = proof;
    }
    Ok(result)
}
