//! Guarded Source replacement, placement and range gestures over human services.
use super::{Operation, arrange, assembly, captions, parse, project_audit, sequences, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, ItemId, Sequence, TrackId, TrackItem, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub(super) const COMMAND: &str = "agent.sourceEdit";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bytes(v: &impl Serialize) -> Result<Vec<u8>> {
    project_audit::value_bytes(v)
}
fn sha(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(bytes(v)?)))
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Window {
    start: i64,
    end: i64,
}
impl Window {
    fn valid(&self) -> bool {
        self.start >= 0 && self.end > self.start && self.end <= SAFE
    }
    fn range(&self) -> TimeRange {
        TimeRange::from_bounds(Tick(self.start), Tick(self.end))
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Slot {
    clip: u64,
    identity: String,
    source_in_ticks: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Edit {
    Replace {
        item: u64,
        #[serde(rename = "assetId")]
        asset: String,
        #[serde(rename = "mediaId")]
        media: String,
        window: Window,
        clips: Vec<Slot>,
    },
    Place {
        item: u64,
        #[serde(rename = "assetId")]
        asset: String,
        #[serde(rename = "mediaId")]
        media: String,
        window: Window,
        at: i64,
        #[serde(rename = "videoTrack")]
        video: Option<u64>,
        #[serde(rename = "audioTracks")]
        audio: Vec<u64>,
        mode: String,
    },
    Range {
        window: Window,
        tracks: Vec<u64>,
        mode: String,
    },
}
impl Edit {
    fn window(&self) -> &Window {
        match self {
            Self::Replace { window, .. } | Self::Place { window, .. } | Self::Range { window, .. } => window,
        }
    }
    fn mode(&self) -> &str {
        match self {
            Self::Replace { .. } => "replace",
            Self::Place { mode, .. } | Self::Range { mode, .. } => mode,
        }
    }
    fn source(&self) -> Option<(u64, &str, &str)> {
        match self {
            Self::Replace { item, asset, media, .. } | Self::Place { item, asset, media, .. } => Some((*item, asset, media)),
            Self::Range { .. } => None,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_state_sha256: String,
    before_sha256: String,
    before_caption_sha256: String,
    placement_defaults_sha256: Option<String>,
    linked_policy: String,
    marker_policy: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if !id(p.scope.anchor_sequence)
        || p.scope.project_id.is_empty()
        || p.scope.project_id.len() > 256
        || p.scope.project_id.chars().any(char::is_control)
        || ![&p.before_state_sha256, &p.before_sha256, &p.before_caption_sha256].iter().all(|s| hash(s))
        || !p.edit.window().valid()
        || !matches!(p.linked_policy.as_str(), "preserve" | "explicit")
        || !matches!(p.marker_policy.as_str(), "retain" | "ripple")
        || p.marker_policy == "ripple" && !matches!(p.edit.mode(), "insert" | "extract")
        || !op.params.as_object().is_some_and(|o| o.contains_key("placementDefaultsSha256"))
        || match &p.edit {
            Edit::Place { .. } => p.placement_defaults_sha256.as_ref().is_none_or(|s| !hash(s)),
            _ => p.placement_defaults_sha256.is_some(),
        }
        || p.edit.source().is_some_and(|(_, asset, media)| [asset, media].iter().any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control)))
    {
        return Err(error("need current complete state, positive exact ranges and explicit link/marker policy"));
    }
    let valid = match &p.edit {
        Edit::Replace { item, clips, window, .. } => {
            id(*item)
                && !clips.is_empty()
                && clips.len() <= 128
                && clips.iter().all(|c| {
                    id(c.clip)
                        && c.identity.len() == 32
                        && c.identity.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        && c.source_in_ticks >= window.start
                        && c.source_in_ticks < window.end
                })
                && clips.iter().map(|c| c.clip).collect::<BTreeSet<_>>().len() == clips.len()
        }
        Edit::Place { item, at, video, audio, window, mode, .. } => {
            id(*item)
                && *at >= 0
                && at.checked_add(window.end - window.start).is_some_and(|n| n <= SAFE)
                && (video.is_some() || !audio.is_empty())
                && video.is_none_or(id)
                && audio.len() <= 32
                && audio.iter().copied().all(id)
                && audio.iter().collect::<BTreeSet<_>>().len() == audio.len()
                && matches!(mode.as_str(), "insert" | "overwrite")
                && op.params["edit"].as_object().is_some_and(|o| o.contains_key("videoTrack"))
        }
        Edit::Range { tracks, mode, .. } => {
            !tracks.is_empty()
                && tracks.len() <= 128
                && tracks.iter().copied().all(id)
                && tracks.iter().collect::<BTreeSet<_>>().len() == tracks.len()
                && matches!(mode.as_str(), "lift" | "extract")
        }
    };
    if valid { Ok(()) } else { Err(error("need distinct current targets, explicit source destinations and a supported Source gesture")) }
}
fn clips(q: &Sequence) -> Result<BTreeMap<u64, (TrackId, &TrackItem)>> {
    let mut out = BTreeMap::new();
    for t in q.all_tracks() {
        for c in &t.items {
            if out.len() >= 100_000
                || !id(c.id.0)
                || !id(t.id.0)
                || c.start.0 < 0
                || c.duration.0 <= 0
                || c.source_in.0 < 0
                || c.source_in.0 > SAFE
                || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
                || out.insert(c.id.0, (t.id, c)).is_some()
            {
                return Err(error("unsupported, duplicated or oversized Source clip clocks/identities"));
            }
            crate::clip_ops::replacement_source_out(c, c.source_in)?;
        }
    }
    Ok(out)
}
fn basis(q: &Sequence) -> Result<Vec<Value>> {
    clips(q)?.iter().map(|(id, (track, c))| Ok(json!({"clip":id,"track":track.0,"sha256":sha(c)?}))).collect()
}
pub(super) fn digest(s: &Session) -> Result<String> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if s.project.item(seq).and_then(|i| i.metadata.get(filmcraft_project::clip_identity::TRACKING_KEY)).map(String::as_str) != Some("1") {
        return Err(error("Source editing requires an adopted tracked native sequence"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    clips(q)?;
    tracks::digest(q)?;
    captions::digest(&q.caption_tracks)?;
    super::markers::digest(&q.markers)?;
    sha(q)
}
pub(super) fn placement_defaults(s: &Session) -> Result<String> {
    sha(&s.prefs.media.default_media_scaling)
}
pub(super) fn audio_layout(media: &filmcraft_project::MediaClip) -> Vec<Value> {
    if !media.info.has_audio() {
        return Vec::new();
    }
    let maps = media.interpret.audio_channels.as_ref().map(|m| m.clips.as_slice()).unwrap_or(&[]);
    crate::commands::audio_placement_specs(maps, media.info.audio_streams.len())
        .into_iter()
        .map(|spec| {
            let channels = spec.source_channels.unwrap_or_else(|| if spec.audio_stream == 0 { maps.first().cloned().unwrap_or_default() } else { Vec::new() });
            json!({"audioStream":spec.audio_stream,"sourceChannels":channels})
        })
        .collect()
}
pub(super) fn summary(s: &Session) -> Value {
    let result = (|| -> Result<Value> {
        Ok(
            json!({"status":"current","sha256":digest(s)?,"placementDefaultsSha256":placement_defaults(s)?,"clips":basis(s.active_sequence().ok_or(EngineError::NoSequence)?)?}),
        )
    })();
    result.unwrap_or_else(|e| json!({"status":"unavailable","reason":e.to_string()}))
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if digest(s)? != p.before_state_sha256 {
        return Err(error("authored Source state changed since this plan"));
    }
    if matches!(p.edit, Edit::Place { .. }) && p.placement_defaults_sha256.as_deref() != Some(placement_defaults(s)?.as_str()) {
        return Err(error("Source placement defaults changed since this plan"));
    }
    Ok(())
}
fn row(track: TrackId, c: &TrackItem) -> Result<Value> {
    let identity = c.identity.as_ref().ok_or_else(|| error("Source results need persistent native identities"))?;
    Ok(
        json!({"clip":c.id.0,"track":track.0,"identity":identity.id,"parentIdentity":identity.parent,"item":c.item.0,"startTicks":c.start.0,"durationTicks":c.duration.0,"sourceInTicks":c.source_in.0,"sha256":sha(c)?}),
    )
}
fn changes<K: Ord, V: Serialize>(a: &BTreeMap<K, &V>, b: &BTreeMap<K, &V>) -> Result<usize> {
    let mut count = 0;
    for (id, x) in a {
        if let Some(y) = b.get(id)
            && bytes(x)? != bytes(y)?
        {
            count += 1;
        }
    }
    Ok(count)
}
fn preserve_window(q: &Sequence, window: &Window, dest: &BTreeSet<u64>) -> Result<()> {
    let mut groups: BTreeMap<u64, Vec<(i64, i64, bool)>> = BTreeMap::new();
    for t in q.all_tracks() {
        for c in &t.items {
            if let Some(link) = c.link {
                let a = c.start.0.max(window.start);
                let b = c.end().0.min(window.end);
                if a < b {
                    groups.entry(link).or_default().push((a, b, !t.locked && dest.contains(&t.id.0)));
                }
            }
        }
    }
    for values in groups.values_mut() {
        values.sort_unstable_by_key(|v| v.0);
        let mut ends = [-1, -1];
        for (a, b, affected) in values {
            let k = usize::from(*affected);
            if ends[1 - k] > *a {
                return Err(error("Source range would split linked picture/sound; use explicit only for an intended independent edit"));
            }
            ends[k] = ends[k].max(*b);
        }
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)? {
        return Err(error("Source edit is outside this workspace"));
    }
    validate_live_basis(s, op)?;
    let before = s.project.clone();
    project_audit::bytes(&before)?;
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let old_state = s.state.clone();
    if tracks::digest(old)? != p.before_sha256 || captions::digest(&old.caption_tracks)? != p.before_caption_sha256 {
        return Err(error("complete Source track/caption state changed"));
    }
    let all = clips(old)?;
    let unit = s.sequence_rate().frame_duration().0;
    if unit <= 0 {
        return Err(error("invalid Source sequence rate"));
    }
    let mut requested = Vec::new();
    let mut source_asset = None;
    if let Some((item, asset, media_id)) = p.edit.source() {
        let binding = assembly::binding(s, ItemId(item))?;
        if binding.project_id != p.scope.project_id || binding.asset_id != asset || binding.media_id != media_id {
            return Err(error("replacement/placement source binding differs from the intended original"));
        }
        source_asset = Some(binding.asset_id);
        let media = s.project.item(ItemId(item)).and_then(|i| i.as_media()).ok_or_else(|| error("Source gesture requires direct imported media"))?;
        if media.offline || p.edit.window().end > media.info.duration.0 {
            return Err(error("Source window is offline or exceeds the original duration"));
        }
    }
    let command = match &p.edit {
        Edit::Replace { item, clips: targets, window, .. } => {
            let selected: BTreeMap<_, _> = targets.iter().map(|r| (r.clip, r)).collect();
            for r in targets {
                let (track, c) = all.get(&r.clip).ok_or_else(|| error("missing replacement instance"))?;
                if old.track(*track).is_none_or(|t| t.locked) || c.identity.is_none() || assembly::binding(s, c.item)?.project_id != p.scope.project_id {
                    return Err(error("replacement target is locked, untracked or foreign"));
                }
                if c.identity.as_ref().is_none_or(|i| i.id != r.identity) {
                    return Err(error("replacement persistent identity differs from the current instance"));
                }
                if crate::clip_ops::replacement_source_out(c, Tick(r.source_in_ticks))?.0 > window.end {
                    return Err(error("the current slot consumes more source media than this window; no source clamping"));
                }
                if p.linked_policy == "preserve"
                    && let Some(link) = c.link
                {
                    for (_, peer) in all.values().filter(|(_, x)| x.link == Some(link)) {
                        let target = selected.get(&peer.id.0).ok_or_else(|| error("preserve requires every current linked replacement member"))?;
                        if peer.item != c.item
                            || peer.speed != c.speed
                            || peer.reverse != c.reverse
                            || target.source_in_ticks - peer.source_in.0 != r.source_in_ticks - c.source_in.0
                        {
                            return Err(error("replacement source clocks would desynchronize linked sound"));
                        }
                    }
                }
            }
            let raw: Vec<_> = targets.iter().map(|r| (ClipId(r.clip), Tick(r.source_in_ticks))).collect();
            crate::clip_ops::replace_explicit(s, ItemId(*item), &raw)?;
            requested = targets.iter().map(|r| r.clip).collect();
            "clip.replaceWithClip"
        }
        Edit::Place { item, window, at, video, audio, mode, .. } => {
            let duration = window.end - window.start;
            if at % unit != 0 || duration % unit != 0 {
                return Err(error("Source placement must match exact sequence-frame boundaries"));
            }
            let media = s.project.item(ItemId(*item)).and_then(|i| i.as_media()).ok_or_else(|| error("missing placement source"))?;
            if video.is_some() && media.info.video.is_none() {
                return Err(error("source has no picture"));
            }
            let mappings = media.interpret.audio_channels.as_ref().map(|m| m.clips.as_slice()).unwrap_or(&[]);
            let specs = crate::commands::audio_placement_specs(mappings, media.info.audio_streams.len());
            if !audio.is_empty() {
                if !media.info.has_audio() || specs.len() != audio.len() {
                    return Err(error("name every destination for the source's complete current audio interpretation/streams"));
                }
                let first = old.audio_tracks.iter().position(|t| t.id.0 == audio[0]).ok_or_else(|| error("missing audio destination"))?;
                for (i, id) in audio.iter().enumerate() {
                    if old.audio_tracks.get(first + i).is_none_or(|t| t.id.0 != *id) {
                        return Err(error("audio destinations must match the native contiguous source-map order"));
                    }
                }
            }
            let dest: BTreeSet<_> = video.iter().chain(audio.iter()).copied().collect();
            for id in &dest {
                let t = old.track(TrackId(*id)).ok_or_else(|| error("missing Source destination"))?;
                if t.locked || t.kind == TrackKind::Video && Some(*id) != *video || t.kind == TrackKind::Audio && !audio.contains(id) {
                    return Err(error("Source destination is locked or the wrong kind"));
                }
            }
            if p.linked_policy == "preserve" {
                if mode == "insert" {
                    arrange::preserve_insert(old, Tick(*at), &BTreeSet::new(), &dest)?;
                } else {
                    preserve_window(old, &Window { start: *at, end: *at + duration }, &dest)?;
                }
            }
            requested = crate::commands::place_item_with_marker_policy(
                s,
                ItemId(*item),
                window.range(),
                Tick(*at),
                video.map(TrackId),
                audio.first().copied().map(TrackId),
                mode == "insert",
                "Source Edit",
                None,
                Some(p.marker_policy == "ripple"),
            )?
            .iter()
            .map(|c| c.0)
            .collect();
            for id in &requested {
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                let (_, c) = q.find_item(ClipId(*id)).ok_or_else(|| error("native placement lost an instance"))?;
                if c.start.0 != *at || c.duration.0 != duration || c.source_in.0 != window.start || c.item.0 != *item {
                    return Err(error("native placement clamped or changed its explicit Source range"));
                }
            }
            "timeline.place"
        }
        Edit::Range { window, tracks: targets, mode } => {
            if window.start % unit != 0 || window.end % unit != 0 {
                return Err(error("Source range edits need exact sequence-frame boundaries"));
            }
            let dest: BTreeSet<_> = targets.iter().copied().collect();
            for id in &dest {
                if old.track(TrackId(*id)).is_none_or(|t| t.locked) {
                    return Err(error("range target is missing or locked"));
                }
            }
            if p.linked_policy == "preserve" {
                if mode == "extract" {
                    arrange::preserve_insert(old, Tick(window.start), &BTreeSet::new(), &dest)?;
                } else {
                    preserve_window(old, window, &dest)?;
                }
            }
            crate::range_ops::prepare(
                s,
                targets.iter().copied().map(TrackId).collect(),
                window.range(),
                if mode == "lift" { crate::range_ops::Kind::Lift } else { crate::range_ops::Kind::Extract },
                p.marker_policy == "ripple",
            )?
            .execute(s)?;
            if mode == "lift" { "sequence.lift" } else { "sequence.extract" }
        }
    };
    // The full authored sequence (including markers/marks, excluding user preferences)
    // is a separate readback obligation alongside track/caption state.
    let after_state = digest(s)?;
    if after_state == p.before_state_sha256 {
        return Err(error("Source gesture made no change"));
    }
    arrange::protected_bounds(s, old, &p.scope.project_id)?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let now = clips(after)?;
    for (id, (track, c)) in &now {
        if !all.contains_key(id) && c.identity.is_none() {
            return Err(error("Source gesture created an untracked instance"));
        }
        if let Some((_, previous)) = all.get(id)
            && bytes(previous)? != bytes(c)?
            && c.identity.is_none()
        {
            return Err(error("Source gesture lost current tracking"));
        }
        if !old.all_tracks().any(|t| t.id == *track) {
            return Err(error("Source gesture created an undeclared track"));
        }
    }
    let mut projection = (*before).clone();
    projection.next_id = s.project.next_id;
    projection.items.get_mut(&sequence).ok_or(EngineError::NoSequence)?.metadata.insert(filmcraft_project::clip_identity::TRACKING_KEY.into(), "1".into());
    let q = projection.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    for (a, b) in [(&mut q.video_tracks, &after.video_tracks), (&mut q.audio_tracks, &after.audio_tracks)] {
        if a.len() != b.len() {
            return Err(error("Source gesture changed the track inventory"));
        }
        for (old, new) in a.iter_mut().zip(b) {
            if old.id != new.id {
                return Err(error("Source gesture reordered tracks"));
            }
            old.items = new.items.clone();
            old.transitions = new.transitions.clone();
        }
    }
    if q.caption_tracks.len() != after.caption_tracks.len() {
        return Err(error("Source gesture changed caption tracks"));
    }
    for (a, b) in q.caption_tracks.iter_mut().zip(&after.caption_tracks) {
        if a.id != b.id {
            return Err(error("Source gesture reordered caption tracks"));
        }
        a.captions = b.captions.clone();
    }
    if p.marker_policy == "ripple" {
        match &p.edit {
            Edit::Place { at, window, .. } => crate::sequence_tools::ripple_markers(&mut q.markers, Tick(*at), Tick(window.end - window.start)),
            Edit::Range { window, .. } => crate::sequence_tools::ripple_markers(&mut q.markers, Tick(window.end), Tick(window.start - window.end)),
            _ => {}
        }
    }
    if matches!(&p.edit,Edit::Range{mode,..} if mode=="extract") {
        q.mark_in = None;
        q.mark_out = None;
        q.split = Default::default();
    }
    if bytes(&projection)? != bytes(&s.project)? {
        return Err(error("Source gesture changed unrelated project, track or marker state"));
    }
    let mut expected_state = old_state;
    if matches!(p.edit, Edit::Place { .. }) {
        expected_state.selection = requested.iter().copied().map(ClipId).collect();
    }
    crate::move_ops::repair_clip_state(after, &mut expected_state);
    if let Edit::Range { window, mode, .. } = &p.edit
        && mode == "extract"
    {
        expected_state.playheads.insert(sequence, s.sequence_rate().snap(Tick(window.start)));
        if expected_state.selection_follows_playhead {
            let targeting = s.targeting().targeted;
            let ids: Vec<_> = after.all_tracks().filter(|t| targeting.contains(&t.id)).filter_map(|t| t.item_at(Tick(window.start)).map(|c| c.id)).collect();
            expected_state.selection = crate::commands::with_links(s, &ids);
        }
    }
    if bytes(&expected_state)? != bytes(&s.state)? {
        return Err(error("Source gesture changed unrelated editor state"));
    }
    let mut affected = Vec::new();
    let mut removed = 0;
    let mut changed = 0;
    let mut created = 0;
    for id in all.keys().chain(now.keys()).copied().collect::<BTreeSet<_>>() {
        let a = all.get(&id);
        let b = now.get(&id);
        if a.is_some()
            && b.is_some()
            && a.map(|(t, c)| Ok::<_, EngineError>((t, bytes(c)?))).transpose()? == b.map(|(t, c)| Ok::<_, EngineError>((t, bytes(c)?))).transpose()?
        {
            continue;
        }
        match (a, b) {
            (Some(_), None) => removed += 1,
            (None, Some(_)) => created += 1,
            _ => changed += 1,
        };
        affected.push(json!({"clip":id,"before":a.map(|(t,c)|row(*t,c)).transpose()?,"after":b.map(|(t,c)|row(*t,c)).transpose()?}));
    }
    if affected.len() > 4096 {
        return Err(error("Source gesture collateral exceeds 4096 clip changes"));
    }
    let ot: BTreeMap<_, _> = old.all_tracks().flat_map(|t| t.transitions.iter().map(|x| (x.id, x))).collect();
    let nt: BTreeMap<_, _> = after.all_tracks().flat_map(|t| t.transitions.iter().map(|x| (x.id, x))).collect();
    let oc: BTreeMap<_, _> = old.caption_tracks.iter().flat_map(|t| t.captions.iter().map(|x| (x.id, x))).collect();
    let nc: BTreeMap<_, _> = after.caption_tracks.iter().flat_map(|t| t.captions.iter().map(|x| (x.id, x))).collect();
    let om: BTreeMap<_, _> = old.markers.iter().map(|m| (m.id, m)).collect();
    let nm: BTreeMap<_, _> = after.markers.iter().map(|m| (m.id, m)).collect();
    let outcomes = requested
        .iter()
        .map(|id| {
            let (t, c) = now.get(id).ok_or_else(|| error("lost requested Source outcome"))?;
            row(*t, c)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"beforeStateSha256":p.before_state_sha256,"afterStateSha256":after_state,"beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":captions::digest(&after.caption_tracks)?,"intent":p.edit,"linkedPolicy":p.linked_policy,"markerPolicy":p.marker_policy,"sourceAssetId":source_asset,"affected":affected,"outcomes":outcomes,"collateral":{"removedClips":removed,"changedClips":changed,"createdClips":created,"removedTransitions":ot.keys().filter(|id|!nt.contains_key(id)).count(),"createdTransitions":nt.keys().filter(|id|!ot.contains_key(id)).count(),"changedTransitions":changes(&ot,&nt)?,"removedCues":oc.keys().filter(|id|!nc.contains_key(id)).count(),"createdCues":nc.keys().filter(|id|!oc.contains_key(id)).count(),"changedCues":changes(&oc,&nc)?,"removedMarkers":om.keys().filter(|id|!nm.contains_key(id)).count(),"createdMarkers":nm.keys().filter(|id|!om.contains_key(id)).count(),"changedMarkers":changes(&om,&nm)?},"humanCommand":command}),
    )
}
