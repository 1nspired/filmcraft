//! Reapply ORIGINAL qualified source choices to saved logical passages. No
//! editorial planner, merging of differently finished clips, or project rollback.
use super::*;
use filmcraft_project::Sequence;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Choice {
    project_id: String,
    receipt_sha256: String,
    version: u8,
    passages: Vec<Passage>,
    #[serde(default)]
    previous_receipts: Option<Vec<Previous>>,
    #[serde(default)]
    caption_basis: Option<caption_ripple::Basis>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Previous {
    identity: String,
    receipt_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Passage {
    identity: String,
    source_in: i64,
    source_out: i64,
    kept: Vec<[i64; 2]>,
}
pub(in crate::agent_bridge) struct RecordedSpan {
    pub(in crate::agent_bridge) original: TrackItem,
    pub(in crate::agent_bridge) identity: String,
    pub(in crate::agent_bridge) members: BTreeSet<ClipId>,
    pub(in crate::agent_bridge) leader_pieces: BTreeSet<ClipId>,
}
fn origin(m: &Member) -> Result<&str> {
    m.origin.as_deref().or_else(|| m.original.identity.as_ref().map(|i| i.id.as_str())).ok_or_else(|| error("missing physical member origin"))
}
fn basis(m: &Member) -> BasisMember {
    BasisMember { track: m.track, kind: m.kind, original: m.original.clone(), full_start: m.full_start, source: m.source.clone(), input: None }
}
pub(super) fn leader(r: &Record) -> Result<BasisMember> {
    r.basis
        .iter()
        .find(|m| m.original.identity.as_ref().is_some_and(|i| i.id == r.identity))
        .cloned()
        .or_else(|| r.members.iter().find(|m| m.original.identity.as_ref().is_some_and(|i| i.id == r.identity)).map(basis))
        .ok_or_else(|| error("missing original logical passage basis"))
}
pub(super) fn leader_pieces(r: &Record) -> Result<Vec<&Piece>> {
    let mut pieces = Vec::new();
    for m in &r.members {
        if origin(m)? == r.identity {
            pieces.extend(&m.pieces);
        }
    }
    pieces.sort_by_key(|p| p.source_in);
    if pieces.windows(2).any(|w| w[0].source_in + w[0].duration > w[1].source_in) {
        return Err(error("overlapping logical leader pieces"));
    }
    Ok(pieces)
}
pub(super) fn input(s: &Session, q: &Sequence, track: TrackId, c: &TrackItem) -> Result<Option<Value>> {
    let t = q.track(track).ok_or_else(|| error("missing original member track"))?;
    if t.kind != TrackKind::Audio {
        return Ok(None);
    }
    let media = s.project.item(c.item).and_then(|i| i.as_media()).ok_or_else(|| error("missing original audio"))?;
    let stream = media.info.audio_streams.get(c.audio_stream).ok_or_else(|| error("missing selected original stream"))?;
    if stream.channels == 0
        || stream.channels > 64
        || stream.sample_rate == 0
        || c.source_channels.len() > 64
        || c.source_channels.iter().any(|ch| u32::from(*ch) >= stream.channels)
    {
        return Err(error("invalid original audio input bounds"));
    }
    let width = filmcraft_render::mixer::width_of(t.channels);
    let channels = filmcraft_render::audio::source_input_channels(c, stream.channels as usize, width);
    if channels.is_empty() {
        return Err(error("no original renderer input"));
    }
    Ok(Some(json!({"stream":c.audio_stream,"streamChannels":stream.channels,"sampleRate":stream.sample_rate,
        "trackWidth":width,"channels":channels,"enabled":t.enabled,"muted":t.muted,"solo":t.solo})))
}
pub(super) fn validate_basis(r: &Record) -> Result<()> {
    if r.basis.is_empty() {
        if r.members.iter().any(|m| m.origin.is_some()) {
            return Err(error("partition origin without original basis"));
        }
        return Ok(());
    }
    if r.basis.len() > 128 {
        return Err(error("too many original physical members"));
    }
    let mut ids = BTreeSet::new();
    for b in &r.basis {
        let c = &b.original;
        let id = c.identity.as_ref().filter(|i| i.validate().is_ok()).ok_or_else(|| error("invalid original basis identity"))?;
        if !ids.insert(id.id.as_str())
            || !range(c.start, c.duration)
            || !range(c.source_in, c.duration)
            || !range(b.full_start, c.duration)
            || !condense::forward_clock(c)
            || b.track.0 == 0
            || b.track.0 > SAFE as u64
            || c.id.0 == 0
            || c.id.0 > SAFE as u64
            || c.item.0 == 0
            || c.item.0 > SAFE as u64
        {
            return Err(error("invalid or duplicate original physical basis"));
        }
        let mut parts: Vec<_> = r.members.iter().filter(|m| origin(m).is_ok_and(|o| o == id.id)).collect();
        parts.sort_by_key(|m| m.original.source_in);
        let mut at = c.source_in;
        for m in parts {
            let p = &m.original;
            if m.track != b.track
                || m.kind != b.kind
                || p.item != c.item
                || p.link != c.link
                || m.source != b.source
                || p.source_in != at
                || p.source_out() > c.source_out()
                || m.full_start != b.full_start + p.source_in - c.source_in
            {
                return Err(error("finishing partitions do not tile the original physical member"));
            }
            at = p.source_out();
            if r.full {
                let mut source_at = p.source_in;
                for piece in &m.pieces {
                    if piece.source_in != source_at || piece.start != m.full_start + piece.source_in - p.source_in || piece.link != p.link {
                        return Err(error("saved full member has a source/timeline gap"));
                    }
                    source_at = piece.source_in + piece.duration;
                }
                if source_at != p.source_out() {
                    return Err(error("saved full member is incomplete"));
                }
            }
        }
        if at != c.source_out() {
            return Err(error("missing original finishing partition"));
        }
    }
    if r.members.iter().any(|m| origin(m).is_err() || !ids.contains(origin(m).unwrap_or(""))) {
        return Err(error("foreign finishing partition origin"));
    }
    let leader = leader(r)?;
    if r.basis.iter().any(|m| m.original.link != leader.original.link) || leader.original.link.is_none() && r.basis.len() != 1 {
        return Err(error("inconsistent original physical link ownership"));
    }
    Ok(())
}
pub(in crate::agent_bridge) fn validate_choice(op: &Operation) -> Result<()> {
    let p: Choice = parse(CHOOSE, &op.params)?;
    if !project(&p.project_id)
        || !hash(&p.receipt_sha256)
        || !(1..=5).contains(&p.version)
        || p.passages.is_empty()
        || p.passages.len() > 16
        || op.params.to_string().len() > 64 * 1024
        || p.caption_basis.as_ref().is_some_and(|b| !b.valid())
    {
        return Err(error("explicit original receipt, version and 1-16 saved logical passages required"));
    }
    if let Some(previous) = &p.previous_receipts
        && (previous.len() != p.passages.len() || previous.iter().zip(&p.passages).any(|(r, b)| r.identity != b.identity || !hash(&r.receipt_sha256)))
    {
        return Err(error("complete ordered previous receipt ownership required for replanning"));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0;
    for b in p.passages {
        total += b.kept.len();
        if !ClipIdentity::valid_id(&b.identity)
            || !seen.insert(b.identity)
            || !range(Tick(b.source_in), Tick(b.source_out.checked_sub(b.source_in).ok_or_else(|| error("source span overflow"))?))
            || b.kept.is_empty()
            || total > 256
            || b.kept.iter().any(|[a, z]| *a < b.source_in || *z > b.source_out || a >= z)
            || b.kept.windows(2).any(|w| w[1][0] < w[0][1])
        {
            return Err(error("invalid saved logical source span or complete ordered choice"));
        }
    }
    Ok(())
}
fn normalized(ranges: impl IntoIterator<Item = [i64; 2]>) -> Vec<[i64; 2]> {
    let mut out: Vec<[i64; 2]> = Vec::new();
    for [a, z] in ranges {
        if let Some(last) = out.last_mut().filter(|last| last[1] == a) {
            last[1] = z;
        } else {
            out.push([a, z]);
        }
    }
    out
}
fn translated(a: Tick, delta: i64) -> Result<Tick> {
    a.0.checked_add(delta).filter(|n| (0..=SAFE).contains(n)).map(Tick).ok_or_else(|| error("logical passage translation exceeds exact bounds"))
}

// A full saved record is still checked against current native geometry and
// its original renderer input. Current effects/gain/notes are intentionally not
// compared to an old template: the editor's finishing is the new authority.
pub(super) fn repartition(s: &mut Session, identity: &str) -> Result<()> {
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let mut store = load(&s.project, seq)?;
    let r = store.records.iter_mut().find(|r| r.identity == identity).ok_or_else(|| error("unknown saved passage"))?;
    if !r.full || r.basis.is_empty() {
        return Err(error("original renderer-input basis required before version switching; historical inverse remains available"));
    }
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if q.settings.frame_rate != r.rate || q.settings.sample_rate != r.sample_rate || q.all_tracks().flat_map(|t| &t.items).take(100001).count() > 100000 {
        return Err(error("changed native settings or oversized current sequence"));
    }
    let mut delta = None;
    let mut members = Vec::new();
    let mut ids = BTreeSet::new();
    let mut links = BTreeSet::new();
    for m in &r.members {
        let origin = origin(m)?.to_string();
        let b =
            r.basis.iter().find(|b| b.original.identity.as_ref().is_some_and(|i| i.id == origin)).ok_or_else(|| error("missing original physical owner"))?;
        if q.track(m.track).is_none_or(|t| t.locked || t.kind != m.kind) || source(s, m.original.item, &r.project_id)? != m.source {
            return Err(error("changed original source or locked member track"));
        }
        for p in &m.pieces {
            let (track, c) = q.find_item(p.clip).ok_or_else(|| error("missing retained finishing fragment"))?;
            let shift = c.start.0.checked_sub(p.start.0).ok_or_else(|| error("logical translation overflow"))?;
            if track != m.track
                || c.identity.as_ref().map(|i| i.id.as_str()) != Some(p.identity.as_str())
                || c.item != m.original.item
                || c.source_in != p.source_in
                || c.duration != p.duration
                || c.link != p.link
                || !condense::forward_clock(c)
                || delta.is_some_and(|d| d != shift)
                || !ids.insert(c.id)
                || input(s, q, track, c)? != b.input
            {
                return Err(error("saved full geometry, clock, links or original qualified audio input changed"));
            }
            delta = Some(shift);
            if let Some(link) = c.link {
                links.insert(link);
            }
            members.push(Member {
                track,
                kind: m.kind,
                original: c.clone(),
                full_start: c.start,
                source: m.source.clone(),
                pieces: vec![Piece::of(c)?],
                origin: Some(origin.clone()),
            });
            if members.len() > 128 {
                return Err(error("full finishing partition budget exceeded; nothing was flattened"));
            }
        }
    }
    if q.all_tracks().flat_map(|t| &t.items).any(|c| c.link.is_some_and(|l| links.contains(&l)) && !ids.contains(&c.id)) {
        return Err(error("another human clip joined the saved physical group"));
    }
    let delta = delta.ok_or_else(|| error("saved passage has no current anchor"))?;
    for b in &mut r.basis {
        b.full_start = translated(b.full_start, delta)?;
    }
    for gap in &mut r.gaps {
        gap.start = translated(gap.start, delta)?;
    }
    r.members = members;
    save(s, seq, &store)
}

pub(in crate::agent_bridge) fn run_choice(s: &mut Session, op: &Operation) -> Result<Value> {
    validate_choice(op)?;
    let p: Choice = parse(CHOOSE, &op.params)?;
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let scope = p.caption_basis.as_ref().map(|b| b.scope());
    let caption_witness = caption_ripple::prepare(s, p.caption_basis, &p.project_id)?;
    let store = load(&s.project, seq)?;
    let mut changed = false;
    for (i, b) in p.passages.iter().enumerate() {
        let r = store.records.iter().find(|r| r.identity == b.identity).ok_or_else(|| error("no saved logical passage"))?;
        let leader = leader(r)?;
        if r.project_id != p.project_id
            || r.receipt_sha256 != p.previous_receipts.as_ref().and_then(|v| v.get(i)).map_or(p.receipt_sha256.as_str(), |r| r.receipt_sha256.as_str())
            || r.basis.is_empty()
            || leader.original.source_in.0 != b.source_in
            || leader.original.source_out().0 != b.source_out
        {
            return Err(error("original plan, source span or renderer-input basis does not match the saved passage"));
        }
        let old = normalized(leader_pieces(r)?.into_iter().map(|p| [p.source_in.0, (p.source_in + p.duration).0]));
        changed |= r.receipt_sha256 != p.receipt_sha256 || r.version != p.version || old != normalized(b.kept.clone());
    }
    if !changed {
        return Err(error("the complete requested version is already selected; no history step was added"));
    }
    // Restore and repartition only on the outer staging Session. Every later
    // failure discards these internal graphs; the live cut is published once.
    for (i, b) in p.passages.iter().enumerate() {
        let full = load(&s.project, seq)?.records.iter().find(|r| r.identity == b.identity).is_some_and(|r| r.full);
        if !full {
            let basis = scope.as_ref().map(|scope| caption_ripple::current_basis(s, scope)).transpose()?.flatten();
            let mut params = json!({"projectId":p.project_id,"identity":b.identity,"receiptSha256":p.previous_receipts.as_ref().and_then(|v|v.get(i)).map_or(p.receipt_sha256.as_str(),|r|r.receipt_sha256.as_str())});
            if let Some(basis) = basis {
                params["captionBasis"] = serde_json::to_value(basis).map_err(|e| error(e.to_string()))?;
            }
            super::run(s, &Operation { command: COMMAND.into(), params })?;
        }
    }
    for b in &p.passages {
        repartition(s, &b.identity)?;
    }
    let mut store = load(&s.project, seq)?;
    for b in &p.passages {
        let r = store.records.iter_mut().find(|r| r.identity == b.identity).ok_or_else(|| error("lost logical receipt owner"))?;
        r.receipt_sha256 = p.receipt_sha256.clone();
    }
    save(s, seq, &store)?;
    let mut spans = Vec::new();
    let mut passages = Vec::new();
    for b in &p.passages {
        let r = store.records.iter().find(|r| r.identity == b.identity).ok_or_else(|| error("lost logical passage"))?;
        let mut original = leader(r)?.original;
        original.start = leader(r)?.full_start;
        let members = r.members.iter().flat_map(|m| m.pieces.iter().map(|p| p.clip)).collect();
        let leader_pieces = leader_pieces(r)?.into_iter().map(|p| p.clip).collect();
        passages
            .push(json!({"clip":original.id.0,"identity":b.identity,"sourceIn":b.source_in,"sourceOut":b.source_out,"start":original.start.0,"kept":b.kept}));
        spans.push(RecordedSpan { original, identity: b.identity.clone(), members, leader_pieces });
    }
    let basis = scope.as_ref().map(|scope| caption_ripple::current_basis(s, scope)).transpose()?.flatten();
    let mut params = json!({"projectId":p.project_id,"receiptSha256":p.receipt_sha256,"version":p.version,"passages":passages});
    if let Some(basis) = basis {
        params["captionBasis"] = serde_json::to_value(basis).map_err(|e| error(e.to_string()))?;
    }
    let mut result = condense::run_recorded(s, &Operation { command: condense::COMMAND.into(), params }, &spans)?;
    if let Some(proof) = caption_ripple::finish_condense(s, caption_witness, "choice")? {
        result["captionEdit"] = proof;
    }
    Ok(result)
}

pub(in crate::agent_bridge) fn capture_recorded(
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
    let seq = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let mut store = load(before, seq)?;
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let old = before.sequence(seq).ok_or(crate::EngineError::NoSequence)?;
    for record in &mut store.records {
        condense_captions::refresh(&mut record.caption_gaps, old, q);
    }
    let mut summaries = Vec::new();
    let previous_ids: BTreeSet<_> =
        before.sequence(seq).ok_or(crate::EngineError::NoSequence)?.all_tracks().flat_map(|t| t.items.iter().map(|c| c.id)).collect();
    for (n, identity) in leaders.iter().enumerate() {
        let r = store.records.iter_mut().find(|r| &r.identity == identity).ok_or_else(|| error("missing saved version record"))?;
        if !r.full || r.project_id != owner || r.receipt_sha256 != receipt {
            return Err(error("saved version scope changed"));
        }
        let removals = own.get(n).ok_or_else(|| error("missing owned version removals"))?;
        let foreign: Vec<_> = all.iter().copied().filter(|g| !removals.contains(g)).collect();
        for m in &mut r.members {
            let c = &m.original;
            if owners.get(&c.id) != Some(&n) || m.pieces.len() != 1 || m.pieces.first().is_none_or(|p| p.clip != c.id) {
                return Err(error("version did not retain the complete physical finishing basis"));
            }
            let parent = c.identity.as_ref().ok_or_else(|| error("missing finishing identity"))?.id.as_str();
            m.pieces = q
                .all_tracks()
                .flat_map(|t| &t.items)
                .filter(|p| p.id == c.id || !previous_ids.contains(&p.id) && p.identity.as_ref().and_then(|i| i.parent.as_deref()) == Some(parent))
                .map(Piece::of)
                .collect::<Result<Vec<_>>>()?;
            m.pieces.sort_by_key(|p| p.source_in);
            m.full_start = warp(m.full_start, &foreign);
        }
        for b in &mut r.basis {
            b.full_start = warp(b.full_start, &foreign);
        }
        r.version = version;
        r.full = removals.is_empty();
        if !r.full {
            r.gaps = removals.iter().map(|g| TimeRange::new(warp(g.start, all), g.duration)).collect();
        }
        r.caption_gaps = condense_captions::capture(old, q, removals)?;
        summaries.push(json!({"identity":identity,"receiptSha256":receipt,"recordedState":if r.full{"original_native_span"}else{"condensed"},"needsCurrentValidation":true}));
    }
    save(s, seq, &store)?;
    Ok(json!(summaries))
}
