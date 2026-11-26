//! Native original logical input view. Restoration/repartition happens ONLY in
//! a disposable Session; returned originals are planning data, never edit tickets.
use super::*;
use crate::agent_bridge::sequences;

const READ: &str = "agent.condenseInputs";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    identities: Vec<String>,
}

pub(super) fn member_identities(r: &Record) -> BTreeSet<String> {
    r.members
        .iter()
        .flat_map(|m| m.pieces.iter().map(|p| p.identity.clone()))
        .chain(r.members.iter().filter_map(|m| m.original.identity.as_ref().map(|i| i.id.clone())))
        .chain(r.basis.iter().filter_map(|m| m.original.identity.as_ref().map(|i| i.id.clone())))
        .collect()
}

// Do not treat a surviving fragment (including a later human split) as a new
// full source passage. A preexisting sibling's common OUTSIDE parent is not an
// ownership claim. Unknown same-source ancestry refuses instead of guessing.
pub(in crate::agent_bridge) fn assert_unrecorded(s: &Session, targets: &BTreeSet<ClipId>) -> Result<()> {
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let store = load(&s.project, seq)?;
    if store.records.is_empty() {
        return Ok(());
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("ownership scan exceeds its bound"));
    }
    let clips: BTreeMap<_, _> = q.all_tracks().flat_map(|t| &t.items).filter_map(|c| c.identity.as_ref().map(|i| (i.id.as_str(), c))).collect();
    let claimed: BTreeSet<_> = store.records.iter().flat_map(member_identities).collect();
    let outside: BTreeSet<_> = store.records.iter().flat_map(|r| &r.basis).filter_map(|b| b.original.identity.as_ref()?.parent.as_deref()).collect();
    let items: BTreeSet<_> = store.records.iter().flat_map(|r| &r.members).map(|m| m.original.item).collect();
    for id in targets {
        let (_, c) = q.find_item(*id).ok_or_else(|| error("missing current ownership target"))?;
        let identity = c.identity.as_ref().ok_or_else(|| error("untracked ownership target"))?;
        let mut at = Some(identity.id.as_str());
        let mut seen = BTreeSet::new();
        while let Some(current) = at {
            if !seen.insert(current) || seen.len() > 64 {
                return Err(error("cyclic or excessive current lineage"));
            }
            if claimed.contains(current) {
                return Err(error("target belongs to a saved logical passage; use its original identity and version workflow"));
            }
            at = if let Some(parent) = clips.get(current) {
                parent.identity.as_ref().and_then(|i| i.parent.as_deref())
            } else {
                if items.contains(&c.item) && !outside.contains(current) {
                    return Err(error("unknown same-source lineage cannot establish an independent passage"));
                }
                None
            };
        }
    }
    Ok(())
}

pub(in crate::agent_bridge) fn read(s: &mut Session, params: &Value) -> Result<Value> {
    let r: Read = parse(READ, params)?;
    if r.identities.is_empty()
        || r.identities.len() > 16
        || r.identities.iter().any(|i| !ClipIdentity::valid_id(i))
        || r.identities.iter().collect::<BTreeSet<_>>().len() != r.identities.len()
    {
        return Err(error("1-16 distinct explicit logical/current identities required"));
    }
    crate::agent_bridge::validate_snapshot(s, &json!({"token":r.token}))?;
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, seq, &r.scope)? {
        return Err(error("logical inputs belong to another workspace"));
    }
    let store = load(&s.project, seq)?;
    let saved = r.identities.iter().filter(|id| store.records.iter().any(|p| &p.identity == *id)).count();
    if saved != 0 && saved != r.identities.len() {
        return Err(error("plan saved logical passages and new independent groups separately; mixed ownership needs its own publication contract"));
    }
    let mut stage = Session::new(s.services.clone());
    stage.project = s.project.clone();
    stage.media = s.media.clone();
    stage.state = s.state.clone();
    stage.revision = s.revision;
    stage.saved_revision = s.saved_revision;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if s.project.items.len() > 100_000 || q.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("logical input scan exceeds its bound"));
    }
    let mut laid = BTreeMap::new();
    if saved != 0 {
        for id in &r.identities {
            let p = store.records.iter().find(|p| &p.identity == id).ok_or_else(|| error("missing saved logical passage"))?;
            if p.project_id != r.scope.project_id || p.basis.is_empty() {
                return Err(error("original renderer-input basis and owning film required"));
            }
            if p.members.iter().any(|m| {
                m.original.identity.as_ref().is_some_and(|i| i.protection.is_some())
                    || m.pieces.iter().any(|p| q.find_item(p.clip).is_none_or(|(_, c)| c.identity.as_ref().is_none_or(|i| i.protection.is_some())))
            }) {
                return Err(error("logical inputs contain missing, untracked or human-protected current fragments"));
            }
            let ticks = versions::leader_pieces(p)?
                .iter()
                .try_fold(0i64, |n, p| n.checked_add(p.duration.0))
                .filter(|n| positive(*n))
                .ok_or_else(|| error("invalid current logical laid duration"))?;
            laid.insert(id.clone(), ticks);
            if !p.full {
                super::run(
                    &mut stage,
                    &Operation { command: COMMAND.into(), params: json!({"projectId":r.scope.project_id,"identity":id,"receiptSha256":p.receipt_sha256}) },
                )?;
            }
        }
        for id in &r.identities {
            versions::repartition(&mut stage, id)?;
        }
    }
    let updated = load(&stage.project, seq)?;
    let mut original = stage.active_sequence().ok_or(crate::EngineError::NoSequence)?.clone();
    for t in original.all_tracks_mut() {
        t.items.clear();
        t.transitions.clear();
    }
    original.caption_tracks.clear();
    original.markers.clear();
    let mut passages = Vec::new();
    let mut used = BTreeSet::new();
    for id in &r.identities {
        let (members, previous, ticks): (Vec<(TrackId, TrackItem)>, Option<String>, i64) = if saved != 0 {
            let p = updated.records.iter().find(|p| &p.identity == id).ok_or_else(|| error("lost original logical basis"))?;
            (
                p.basis
                    .iter()
                    .map(|b| {
                        let mut c = b.original.clone();
                        c.start = b.full_start;
                        (b.track, c)
                    })
                    .collect(),
                Some(p.receipt_sha256.clone()),
                *laid.get(id).ok_or_else(|| error("missing laid duration"))?,
            )
        } else {
            let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
            let c = q
                .all_tracks()
                .flat_map(|t| &t.items)
                .find(|c| c.identity.as_ref().is_some_and(|i| &i.id == id))
                .ok_or_else(|| error("missing independent current leader"))?;
            let members: Vec<_> = q
                .all_tracks()
                .flat_map(|t| t.items.iter().filter(|p| p.id == c.id || c.link.is_some() && p.link == c.link).map(move |p| (t.id, p.clone())))
                .collect();
            assert_unrecorded(s, &members.iter().map(|(_, c)| c.id).collect())?;
            (members, None, c.duration.0)
        };
        for (track, c) in members {
            if !used.insert(c.id)
                || used.len() > 128
                || c.identity.as_ref().is_none_or(|i| i.protection.is_some())
                || !c.enabled
                || !condense::forward_clock(&c)
            {
                return Err(error("repeated, protected, retimed or oversized logical inputs"));
            }
            source(&stage, c.item, &r.scope.project_id)?;
            let t = original.track_mut(track).ok_or_else(|| error("missing original input track"))?;
            if t.locked {
                return Err(error("logical input track is locked"));
            }
            t.items.push(c);
        }
        passages.push(json!({"identity":id,"previousReceiptSha256":previous,"laidTicks":ticks}));
    }
    for t in original.all_tracks_mut() {
        t.items.sort_by_key(|c| c.start);
    }
    let result = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":seq.0,"scope":r.scope,
        "view":"original_condense_inputs","execution":"not-applied","saved":saved!=0,"passages":passages,
        "originalSequence":crate::commands::inspect_sequence(&stage,seq,&original)});
    if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 512 * 1024 {
        return Err(error("complete original logical input view exceeds its wire budget"));
    }
    Ok(result)
}
