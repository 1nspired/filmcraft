//! Exact instance placement over the shared human move/copy service.
use super::{Operation, assembly, caption_ripple, captions, parse, project_audit, sequences, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, Sequence, TrackId, TrackItem};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub(super) const COMMAND: &str = "agent.arrange";
const SAFE: u64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Placement {
    clip: u64,
    track: u64,
    start_ticks: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_sha256: String,
    before_caption_sha256: String,
    copy: bool,
    mode: String,
    linked_policy: String,
    placements: Vec<Placement>,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if !hash(&p.before_sha256)
        || !hash(&p.before_caption_sha256)
        || !id(p.scope.anchor_sequence)
        || p.scope.project_id.is_empty()
        || p.scope.project_id.len() > 256
        || p.scope.project_id.chars().any(char::is_control)
        || !matches!(p.mode.as_str(), "gap" | "overwrite" | "insert")
        || !matches!(p.linked_policy.as_str(), "preserve" | "explicit")
        || p.placements.is_empty()
        || p.placements.len() > 200
        || p.placements.iter().any(|r| !id(r.clip) || !id(r.track) || r.start_ticks < 0 || r.start_ticks as u64 > SAFE)
        || p.placements.iter().map(|r| r.clip).collect::<BTreeSet<_>>().len() != p.placements.len()
    {
        return Err(error("bounded exact placements, mode, linked policy and complete state hashes required"));
    }
    Ok(())
}
fn bytes(v: &impl serde::Serialize) -> Result<Vec<u8>> {
    project_audit::value_bytes(v)
}
fn changes<K: Ord, V: serde::Serialize>(before: &BTreeMap<K, &V>, after: &BTreeMap<K, &V>) -> Result<usize> {
    let mut n = 0;
    for (id, v) in before {
        if let Some(now) = after.get(id)
            && bytes(v)? != bytes(now)?
        {
            n += 1;
        }
    }
    Ok(n)
}
fn ids(q: &Sequence) -> BTreeMap<ClipId, (TrackId, &TrackItem)> {
    q.all_tracks().flat_map(|t| t.items.iter().map(move |c| (c.id, (t.id, c)))).collect()
}
pub(super) fn protected_bounds(s: &Session, before: &Sequence, project: &str) -> Result<()> {
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let current = ids(after);
    for t in before.all_tracks() {
        for c in &t.items {
            let changed = match current.get(&c.id) {
                None => true,
                Some((track, n)) => *track != t.id || bytes(n)? != bytes(c)?,
            };
            if changed && (t.locked || c.identity.is_none() || assembly::binding(s, c.item)?.project_id != project) {
                return Err(error("arrangement would change locked, untracked, unbound or foreign collateral"));
            }
        }
    }
    for t in after.all_tracks() {
        for c in &t.items {
            if !id(c.id.0) || c.link.is_some_and(|l| !id(l)) || c.start.0 < 0 || c.duration.0 <= 0 || c.end().0 < 0 || c.end().0 as u64 > SAFE {
                return Err(error("arrangement exceeds exact clip/time transport"));
            }
        }
        for x in &t.transitions {
            if !id(x.id.0) {
                return Err(error("arrangement transition identity exceeds exact transport"));
            }
        }
    }
    if after.caption_tracks.iter().flat_map(|t| &t.captions).any(|c| !id(c.id.0) || c.start.0 < 0 || c.duration.0 <= 0 || c.end().0 as u64 > SAFE) {
        return Err(error("arrangement cue identity/time exceeds exact transport"));
    }
    Ok(())
}
pub(super) fn preserve_insert(q: &Sequence, at: Tick, moved: &BTreeSet<u64>, dest: &BTreeSet<u64>) -> Result<()> {
    let mut groups: BTreeMap<u64, Vec<(Tick, Tick, bool)>> = BTreeMap::new();
    for t in q.all_tracks() {
        for c in &t.items {
            if !moved.contains(&c.id.0)
                && let Some(link) = c.link
            {
                groups.entry(link).or_default().push((c.start, c.end(), !t.locked && (t.sync_lock || dest.contains(&t.id.0))));
            }
        }
    }
    // A sweep bounds imported many-member link groups to O(n log n), rather
    // than comparing every pair. Only overlapping sound/picture after `at`
    // with different ripple participation can be newly desynchronized.
    for rows in groups.values_mut() {
        rows.sort_unstable_by_key(|r| r.0);
        let mut ends = [Tick::MIN; 2];
        for &(start, end, participates) in rows.iter() {
            let start = start.max(at);
            if end <= start {
                continue;
            }
            let k = usize::from(participates);
            if ends[1 - k] > start {
                return Err(error("native insertion would desynchronize collateral linked sound; use an explicit split-edit policy only when intended"));
            }
            ends[k] = ends[k].max(end);
        }
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)? {
        return Err(error("arrangement is outside this workspace"));
    }
    let before = s.project.clone();
    project_audit::bytes(&before)?;
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let old_state = s.state.clone();
    if tracks::digest(old)? != p.before_sha256 || captions::digest(&old.caption_tracks)? != p.before_caption_sha256 {
        return Err(error("complete arrangement state changed since its read"));
    }
    let all = ids(old);
    let requested: BTreeMap<_, _> = p.placements.iter().map(|r| (r.clip, r)).collect();
    let mut effective = Vec::new();
    let mut at = Tick::MAX;
    let mut end = Tick::ZERO;
    for row in &p.placements {
        let (source, c) = all.get(&ClipId(row.clip)).ok_or_else(|| error("missing current clip instance"))?;
        if c.identity.is_none() || assembly::binding(s, c.item)?.project_id != p.scope.project_id {
            return Err(error("arrangement requires tracked bound current original instances"));
        }
        if c.start.0 < 0 || c.start.0 as u64 > SAFE {
            return Err(error("source clock exceeds exact movement arithmetic"));
        }
        let track = old.track(TrackId(row.track)).ok_or_else(|| error("missing destination"))?;
        if track.locked || (!p.copy && old.track(*source).is_none_or(|t| t.locked)) {
            return Err(error("source/destination is locked"));
        }
        let start = Tick(row.start_ticks);
        let stop = row
            .start_ticks
            .checked_add(c.duration.0)
            .filter(|n| *n > row.start_ticks && *n as u64 <= SAFE)
            .ok_or_else(|| error("placement end exceeds exact transport"))?;
        if p.linked_policy == "preserve"
            && let Some(link) = c.link
        {
            for (_, peer) in all.values().filter(|(_, x)| x.link == Some(link)) {
                let r = requested.get(&peer.id.0).ok_or_else(|| error("preserve requires every current linked partner, including unchanged sound"))?;
                let peer_offset = r.start_ticks.checked_sub(peer.start.0).ok_or_else(|| error("linked source clock exceeds exact movement arithmetic"))?;
                let offset = row.start_ticks.checked_sub(c.start.0).ok_or_else(|| error("source clock exceeds exact movement arithmetic"))?;
                if peer_offset != offset {
                    return Err(error("requested linked positions would desynchronize sound"));
                }
            }
        }
        if p.copy || row.track != source.0 || start != c.start {
            effective.push(json!({"clip":row.clip,"track":row.track,"time":row.start_ticks}));
            at = at.min(start);
            end = end.max(Tick(stop));
        }
    }
    if effective.is_empty() {
        return Err(error("arrangement makes no change"));
    }
    let moved: BTreeSet<_> = if p.copy { BTreeSet::new() } else { effective.iter().filter_map(|x| x["clip"].as_u64()).collect() };
    let destinations: BTreeSet<_> = effective.iter().filter_map(|x| x["track"].as_u64()).collect();
    if p.mode == "insert" && p.linked_policy == "preserve" {
        preserve_insert(old, at, &moved, &destinations)?;
    }
    if p.mode == "gap" {
        for value in &effective {
            let r = requested.get(&value["clip"].as_u64().ok_or_else(|| error("missing resolved clip"))?).ok_or_else(|| error("missing placement"))?;
            let c = all.get(&ClipId(r.clip)).ok_or_else(|| error("missing source"))?.1;
            if old.track(TrackId(r.track)).is_some_and(|t| {
                t.items.iter().any(|other| !moved.contains(&other.id.0) && other.start.0 < r.start_ticks + c.duration.0 && other.end().0 > r.start_ticks)
            }) {
                return Err(error("gap destination is occupied; overwrite must be explicit"));
            }
        }
    }
    let caption_basis = caption_ripple::current_basis(s, &p.scope)?;
    let witness = caption_ripple::prepare(s, caption_basis, &p.scope.project_id)?;
    let raw = json!({"moves":effective,"linked":false,"copy":p.copy,"insert":p.mode=="insert"});
    let plan = crate::move_ops::prepare(s, &raw)?;
    if plan.moves.len() != effective.len()
        || plan.moves.iter().any(|(clip, track, start)| requested.get(&clip.0).is_none_or(|r| r.track != track.0 || r.start_ticks != start.0))
    {
        return Err(error("native placement would clamp or skip explicit intent"));
    }
    let human = plan.execute(s)?;
    if p.mode == "insert" {
        caption_ripple::finish_insert(s, witness, at, end - at)?;
    } else if captions::digest(&s.active_sequence().ok_or(EngineError::NoSequence)?.caption_tracks)? != p.before_caption_sha256 {
        return Err(error("non-ripple arrangement changed sequence captions"));
    }
    protected_bounds(s, old, &p.scope.project_id)?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let copies: Vec<u64> = if p.copy {
        human["copied"]
            .as_array()
            .ok_or_else(|| error("native copy omitted identities"))?
            .iter()
            .map(|x| x.as_u64().ok_or_else(|| error("invalid copied identity")))
            .collect::<Result<_>>()?
    } else {
        Vec::new()
    };
    if p.copy && copies.len() != p.placements.len() {
        return Err(error("native copy omitted a requested instance"));
    }
    let existing_links: BTreeSet<_> = all.values().filter_map(|(_, c)| c.link).collect();
    let mut copied_links = BTreeMap::new();
    let mut placed = Vec::new();
    for (i, row) in p.placements.iter().enumerate() {
        let original = all.get(&ClipId(row.clip)).ok_or_else(|| error("missing origin"))?.1;
        let actual_id = if p.copy { *copies.get(i).ok_or_else(|| error("missing copy"))? } else { row.clip };
        let (track, actual) = after.find_item(ClipId(actual_id)).ok_or_else(|| error("requested placement disappeared"))?;
        let mut expected = original.clone();
        expected.start = Tick(row.start_ticks);
        if p.copy {
            let identity = actual.identity.as_ref().ok_or_else(|| error("copy lost persistent identity"))?;
            let origin = original.identity.as_ref().ok_or_else(|| error("source lost identity"))?;
            if actual_id == row.clip
                || all.contains_key(&ClipId(actual_id))
                || identity.id == origin.id
                || identity.parent.as_deref() != Some(&origin.id)
                || identity.annotations != origin.annotations
                || identity.protection != origin.protection
            {
                return Err(error("native copy lineage or annotations changed"));
            }
            if let Some(link) = original.link {
                let peers = p.placements.iter().filter(|r| all.get(&ClipId(r.clip)).is_some_and(|(_, x)| x.link == Some(link))).count();
                if peers == 1 {
                    if actual.link.is_some() {
                        return Err(error("singleton copy retained an old link"));
                    }
                } else {
                    let new = actual.link.filter(|n| !existing_links.contains(n)).ok_or_else(|| error("copy is linked to an original"))?;
                    if copied_links.insert(link, new).is_some_and(|old| old != new) {
                        return Err(error("linked copies disagree"));
                    }
                }
            } else if actual.link.is_some() {
                return Err(error("unlinked copy gained a link"));
            }
            expected.id = ClipId(actual_id);
            expected.identity = actual.identity.clone();
            expected.link = actual.link;
        }
        if track.0 != row.track || bytes(&expected)? != bytes(actual)? {
            return Err(error("placement changed current trim, processing, source, linked state or exact requested position"));
        }
        placed.push(json!({"sourceClip":row.clip,"clip":actual_id,"track":row.track,"startTicks":row.start_ticks,"identity":actual.identity.as_ref().map(|x|&x.id),"parentIdentity":actual.identity.as_ref().and_then(|x|x.parent.as_ref())}));
    }
    if copied_links.values().collect::<BTreeSet<_>>().len() != copied_links.len() {
        return Err(error("distinct copied groups share a link"));
    }
    // Only track payloads, proven native captions and allocated IDs may change.
    let mut projection = (*before).clone();
    projection.next_id = s.project.next_id;
    let item = projection.items.get_mut(&sequence).ok_or(EngineError::NoSequence)?;
    item.metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
    let q = projection.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    for (old_tracks, new_tracks) in [(&mut q.video_tracks, &after.video_tracks), (&mut q.audio_tracks, &after.audio_tracks)] {
        if old_tracks.len() != new_tracks.len() {
            return Err(error("arrangement created or removed a track"));
        }
        for (a, b) in old_tracks.iter_mut().zip(new_tracks) {
            if a.id != b.id {
                return Err(error("arrangement reordered tracks"));
            }
            a.items = b.items.clone();
            a.transitions = b.transitions.clone();
        }
    }
    q.caption_tracks = after.caption_tracks.clone();
    if project_audit::bytes(&projection)? != project_audit::bytes(&s.project)? {
        return Err(error("arrangement changed unrelated project or track settings"));
    }
    let mut expected_state = old_state;
    if p.copy {
        expected_state.selection = copies.iter().copied().map(ClipId).collect();
        expected_state.transition_selection.clear();
    }
    crate::move_ops::repair_clip_state(after, &mut expected_state);
    if bytes(&expected_state)? != bytes(&s.state)? {
        return Err(error("arrangement changed unrelated editor state"));
    }
    let now = ids(after);
    let old_trans: BTreeMap<_, _> = old.all_tracks().flat_map(|t| t.transitions.iter().map(|x| (x.id, x))).collect();
    let new_trans: BTreeMap<_, _> = after.all_tracks().flat_map(|t| t.transitions.iter().map(|x| (x.id, x))).collect();
    let old_cues: BTreeMap<_, _> = old.caption_tracks.iter().flat_map(|t| t.captions.iter().map(|x| (x.id, x))).collect();
    let new_cues: BTreeMap<_, _> = after.caption_tracks.iter().flat_map(|t| t.captions.iter().map(|x| (x.id, x))).collect();
    let mut changed_clips = 0;
    for (id, old) in &all {
        if !requested.contains_key(&id.0)
            && let Some(new) = now.get(id)
            && bytes(old)? != bytes(new)?
        {
            changed_clips += 1;
        }
    }
    let collateral = json!({"removedClips":all.keys().filter(|id|!now.contains_key(id)).count(),"changedClips":changed_clips,"createdFragments":now.keys().filter(|id|!all.contains_key(id)&&!copies.contains(&id.0)).count(),"removedTransitions":old_trans.keys().filter(|id|!new_trans.contains_key(id)).count(),"createdTransitions":new_trans.keys().filter(|id|!old_trans.contains_key(id)).count(),"changedTransitions":changes(&old_trans,&new_trans)?,"removedCues":old_cues.keys().filter(|id|!new_cues.contains_key(id)).count(),"createdCues":new_cues.keys().filter(|id|!old_cues.contains_key(id)).count(),"changedCues":changes(&old_cues,&new_cues)?});
    Ok(
        json!({"beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":captions::digest(&after.caption_tracks)?,"copy":p.copy,"mode":p.mode,"linkedPolicy":p.linked_policy,"placed":placed,"collateral":collateral,"humanCommand":"timeline.move"}),
    )
}
