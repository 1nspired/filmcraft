//! Original music placement through human track, placement and gain services.
//! Geometry is native; original editorial policy and source evidence stay with
//! Poema. This private operation only runs in agent.apply's staging session.
use super::{BINDING_KEY, Binding, Operation, assembly, captions, media_guard, parse, picture_inputs, project_audit, sequences, source_edit, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ItemId, MediaRef};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const COMMAND: &str = "agent.placeMusic";
const READ: &str = "agent.readMusicDestination";
const ROLES: &str = "poema.audio.roles.v1";
const SAFE: i64 = 9_007_199_254_740_991;
const MAX: usize = crate::sequence_tools::MAX_TRACKS;
fn error(m: impl Into<String>) -> EngineError {
    bad(COMMAND, m)
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Anchor {
    Start,
    Midpoint,
    Explicit {
        ticks: i64,
    },
    SourceEvent {
        #[serde(rename = "assetId")]
        asset: String,
        #[serde(rename = "sourceTicks")]
        source: i64,
    },
}
fn valid_anchor(a: &Anchor) -> bool {
    match a {
        Anchor::Explicit { ticks } => (0..=SAFE).contains(ticks),
        Anchor::SourceEvent { asset, source } => text(asset) && (0..=SAFE).contains(source),
        _ => true,
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_state_sha256: String,
    before_sha256: String,
    before_caption_sha256: String,
    before_roles_sha256: String,
    placement_defaults_sha256: String,
    original_plan_sha256: String,
    original_receipt_sha256: String,
    item: u64,
    asset_id: String,
    media_id: String,
    anchor: Anchor,
    requested_at_ticks: i64,
    native_start_ticks: i64,
    native_duration_ticks: i64,
    targets: Vec<Option<u64>>,
    song_role: String,
    gain_db: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadRequest {
    token: String,
    scope: sequences::Scope,
    item: u64,
    asset_id: String,
    anchor: Anchor,
    #[serde(default)]
    projection: bool,
}
fn roles(s: &Session) -> Result<BTreeMap<u64, String>> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let raw = s.project.item(seq).and_then(|i| i.metadata.get(ROLES));
    if raw.is_some_and(|v| v.len() > 128 * 1024) {
        return Err(error("audio role annotations exceed their complete bound"));
    }
    let values: BTreeMap<u64, String> = raw.map(|v| serde_json::from_str(v).map_err(|e| error(e.to_string()))).transpose()?.unwrap_or_default();
    if values.len() > MAX || values.iter().any(|(n, role)| !id(*n) || role.is_empty() || role.len() > 64 || role.chars().any(char::is_control)) {
        return Err(error("malformed audio role annotations"));
    }
    if let Some(raw) = raw
        && serde_json::to_string(&values).map_err(|e| error(e.to_string()))? != *raw
    {
        return Err(error("audio role annotations require their canonical unique-key carrier"));
    }
    Ok(values)
}
pub(super) fn summary(s: &Session) -> Value {
    let result = (|| -> Result<Value> {
        let roles = roles(s)?;
        Ok(json!({"status":"current","roles":roles,"sha256":sha(&roles)?}))
    })();
    result.unwrap_or_else(|e| json!({"status":"unavailable","reason":e.to_string()}))
}
fn unique_original(s: &Session, project: &str, asset: &str) -> Result<Option<ItemId>> {
    let mut found = None;
    for (id, item) in &s.project.items {
        if let Some(raw) = item.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == project && b.asset_id == asset && b.role == "original" && found.replace(*id).is_some() {
                return Err(error("ambiguous original music or pivot binding"));
            }
        }
    }
    Ok(found)
}
fn original<'a>(s: &'a Session, scope: &sequences::Scope, item: u64, asset: &str, picture: bool) -> Result<&'a filmcraft_project::MediaClip> {
    if !id(item) || !text(asset) || unique_original(s, &scope.project_id, asset)? != Some(ItemId(item)) {
        return Err(error("music source is not its unique bound original"));
    }
    let m = s.project.item(ItemId(item)).and_then(|i| i.as_media()).ok_or_else(|| error("music needs direct original media"))?;
    if m.offline
        || !(1..=SAFE).contains(&m.info.duration.0)
        || (if picture { !m.info.has_video() } else { !m.info.has_audio() })
        || !matches!(&m.media, MediaRef::File { path } if std::path::Path::new(path).is_absolute())
    {
        return Err(error("music or pivot needs an online bounded File original"));
    }
    Ok(m)
}
fn midpoint(duration: i64) -> Result<i64> {
    duration
        .checked_add(TICKS_PER_SECOND)
        .and_then(|n| n.checked_div(TICKS_PER_SECOND * 2))
        .and_then(|n| n.checked_mul(TICKS_PER_SECOND))
        .filter(|n| *n <= SAFE)
        .ok_or_else(|| error("music midpoint overflow"))
}
fn geometry(s: &Session, scope: &sequences::Scope, item: u64, asset: &str, anchor: &Anchor) -> Result<(Value, Vec<u64>)> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !id(scope.anchor_sequence)
        || !text(&scope.project_id)
        || !sequences::belongs(&s.project, seq, scope)?
        || !valid_anchor(anchor)
        || s.project.items.len() > 100_000
    {
        return Err(error("music read is outside its bounded adopted workspace"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    let first = q.video_tracks.first().filter(|t| !t.items.is_empty()).ok_or_else(|| error("music placement needs the current picture cut"))?;
    if first.items.len() > 100_000 || q.audio_tracks.len() > MAX || !(0..=SAFE).contains(&q.duration().0) {
        return Err(error("current music destination exceeds its complete bound"));
    }
    let media = original(s, scope, item, asset, false)?;
    let maps = media.interpret.audio_channels.as_ref().map(|m| m.clips.as_slice()).unwrap_or(&[]);
    if media.info.audio_streams.is_empty()
        || media.info.audio_streams.len() > filmcraft_media::MAX_AUDIO_STREAMS
        || media.info.audio_streams.iter().any(|a| a.channels == 0 || a.channels > 64 || a.sample_rate == 0)
        || maps.len().max(1).checked_add(media.info.audio_streams.len() - 1).is_none_or(|n| n > MAX)
        || maps.len() > MAX
        || maps.iter().any(|m| m.is_empty() || m.len() > 64 || m.iter().any(|c| media.info.audio_streams.first().is_none_or(|a| u32::from(*c) >= a.channels)))
    {
        return Err(error("complete music channel interpretation is invalid or exceeds native bounds"));
    }
    let layout = source_edit::audio_layout(media);
    if layout.is_empty() || layout.len() > MAX {
        return Err(error("complete music audio interpretation exceeds native destinations"));
    }
    let mut dependencies = vec![item];
    let mut pivot = Value::Null;
    let mut occurrences = 0usize;
    let (requested, choice) = match anchor {
        Anchor::Start => (0, "start"),
        Anchor::Midpoint => (midpoint(q.duration().0)?, "back-half"),
        Anchor::Explicit { ticks } => (*ticks, "explicit"),
        Anchor::SourceEvent { asset, source } => {
            if let Some(event_item) = unique_original(s, &scope.project_id, asset)? {
                let event_media = original(s, scope, event_item.0, asset, true)?;
                if *source >= event_media.info.duration.0 {
                    return Err(error("music pivot is outside its original source"));
                }
                dependencies.push(event_item.0);
                for c in first.items.iter().filter(|c| c.item == event_item) {
                    picture_inputs::clock(c, event_media.info.duration.0)?;
                    if c.frame_hold.is_some() {
                        return Err(error("held music event clocks require an explicit placement"));
                    }
                    if let Some(at) = picture_inputs::event_at(c, *source, event_media.info.duration.0) {
                        occurrences = occurrences.checked_add(1).ok_or_else(|| error("music event occurrence overflow"))?;
                        if pivot.is_null() {
                            let identity = c.identity.as_ref().ok_or_else(|| error("music event needs a persistent current instance"))?;
                            pivot = json!({"clip":c.id.0,"identity":identity.id,"track":first.id.0,"recordTicks":at,"sourceTicks":source,"observedSourceTicks":c.source_time_at(Tick(at)).0,"enabled":c.enabled,"trackEnabled":first.enabled});
                        }
                    }
                }
            } else {
                // Prove absence from explicit current bindings; a name/path match
                // on an unbound item is not evidence for a structural fallback.
                for c in &first.items {
                    let b = assembly::binding(s, c.item)?;
                    if b.project_id != scope.project_id || b.role != "original" || b.asset_id == *asset {
                        return Err(error("current picture cannot prove the original pivot absent"));
                    }
                }
            }
            if let Some(at) = pivot.get("recordTicks").and_then(Value::as_i64) {
                let rounded = at
                    .checked_add(TICKS_PER_SECOND / 2)
                    .and_then(|n| n.checked_div(TICKS_PER_SECOND))
                    .and_then(|n| n.checked_mul(TICKS_PER_SECOND))
                    .ok_or_else(|| error("music event rounding overflow"))?;
                (rounded, "kiss")
            } else {
                (midpoint(q.duration().0)?, "back-half")
            }
        }
    };
    // Exact human timeline.place policy: snap both requested boundaries, then
    // make_track_item enforces at least one sequence frame. Report the tail
    // difference rather than claiming sample-exact whole-take duration.
    let rate = q.settings.frame_rate;
    if !(1..=SAFE).contains(&rate.frame_duration().0) {
        return Err(error("native music frame unit exceeds exact transport bounds"));
    }
    let raw_end = requested.checked_add(media.info.duration.0).filter(|n| *n <= SAFE).ok_or_else(|| error("music end exceeds exact transport range"))?;
    let start = rate.snap_nearest(Tick(requested));
    let duration = (rate.snap_nearest(Tick(raw_end)) - start).max(rate.frame_duration());
    let end = start.0.checked_add(duration.0).filter(|n| *n <= SAFE).ok_or_else(|| error("snapped music end exceeds exact transport range"))?;
    if start.0 < 0 || duration.0 <= 0 {
        return Err(error("invalid native music placement geometry"));
    }
    dependencies.sort_unstable();
    dependencies.dedup();
    Ok((
        json!({"requestedAtTicks":requested,"nativeStartTicks":start.0,"nativeDurationTicks":duration.0,"nativeEndTicks":end,"sourceDurationTicks":media.info.duration.0,"tailDeltaTicks":duration.0-media.info.duration.0,"audioPlacements":layout,"anchorChoice":choice,"pivot":pivot,"pivotOccurrences":occurrences,"anchorPolicy":"original-first-saved-event-first-spine-occurrence-rounded-second","placementPolicy":"shared-human-boundary-frame-snap","sequenceExtentPolicy":"native-includes-audio-tails"}),
        dependencies,
    ))
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    let r: ReadRequest = parse(READ, value)?;
    super::validate_metadata_read(s, &r.token, r.projection, READ)?;
    let (g, dependencies) = geometry(s, &r.scope, r.item, &r.asset_id, &r.anchor)?;
    super::validate_read_basis(s, &r.token)?.media.require(&dependencies)?;
    let role_map = roles(s)?;
    Ok(
        json!({"kind":"native_music_destination","version":1,"status":"read","token":r.token,"revision":s.revision,"sequence":s.state.active_sequence.map(|v|v.0),"scope":r.scope,"item":r.item,"assetId":r.asset_id,"anchor":r.anchor,"geometry":g,"sourceItems":dependencies,"roles":role_map,"rolesSha256":sha(&role_map)?,"requiresSourceValidation":r.projection,"writeAuthority":false,"semanticVerification":false}),
    )
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if project_audit::value_bytes(&op.params)?.len() > 256 * 1024
        || !id(p.item)
        || !id(p.scope.anchor_sequence)
        || !text(&p.scope.project_id)
        || !text(&p.asset_id)
        || !text(&p.media_id)
        || !valid_anchor(&p.anchor)
        || [
            &p.before_state_sha256,
            &p.before_sha256,
            &p.before_caption_sha256,
            &p.before_roles_sha256,
            &p.placement_defaults_sha256,
            &p.original_plan_sha256,
            &p.original_receipt_sha256,
        ]
        .iter()
        .any(|s| !hash(s))
        || !(0..=SAFE).contains(&p.requested_at_ticks)
        || !(0..=SAFE).contains(&p.native_start_ticks)
        || !(1..=SAFE).contains(&p.native_duration_ticks)
        || p.native_start_ticks.checked_add(p.native_duration_ticks).is_none_or(|n| n > SAFE)
        || p.targets.is_empty()
        || p.targets.len() > MAX
        || p.targets.iter().flatten().any(|n| !id(*n))
        || p.targets.iter().flatten().collect::<BTreeSet<_>>().len() != p.targets.iter().flatten().count()
        || !matches!(p.song_role.as_str(), "love" | "reception" | "first_look")
        || !p.gain_db.is_finite()
        || !(-96.0..=96.0).contains(&p.gain_db)
    {
        return Err(error("need complete bounded original music intent and explicit native destinations"));
    }
    Ok(())
}
pub(super) fn require_sources(s: &Session, op: &Operation, witnesses: &media_guard::Witnesses) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    let (_, dependencies) = geometry(s, &p.scope, p.item, &p.asset_id, &p.anchor)?;
    witnesses.require(&dependencies)
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if source_edit::digest(s)? != p.before_state_sha256
        || tracks::digest(q)? != p.before_sha256
        || captions::digest(&q.caption_tracks)? != p.before_caption_sha256
        || sha(&roles(s)?)? != p.before_roles_sha256
        || source_edit::placement_defaults(s)? != p.placement_defaults_sha256
    {
        return Err(error("current music destination, roles, captions or placement defaults changed"));
    }
    let (g, _) = geometry(s, &p.scope, p.item, &p.asset_id, &p.anchor)?;
    if g["requestedAtTicks"] != p.requested_at_ticks || g["nativeStartTicks"] != p.native_start_ticks || g["nativeDurationTicks"] != p.native_duration_ticks {
        return Err(error("music anchor or native human placement geometry changed"));
    }
    Ok(())
}
fn discard(s: &mut Session) {
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s.events.clear();
}
pub(super) fn run(s: &mut Session, op: &Operation, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
    checkpoint()?;
    validate(op)?;
    validate_live_basis(s, op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let (g, _) = geometry(s, &p.scope, p.item, &p.asset_id, &p.anchor)?;
    let before = s.project.clone();
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let old = before.sequence(sequence).ok_or(EngineError::NoSequence)?;
    let old_state = s.state.clone();
    let binding = assembly::binding(s, ItemId(p.item))?;
    if binding.media_id != p.media_id {
        return Err(error("music original media binding changed"));
    }
    let layout = g["audioPlacements"].as_array().ok_or_else(|| error("music layout missing"))?;
    if layout.len() != p.targets.len() || s.revision.checked_add(8).is_none() {
        return Err(error("name every native music audio component destination"));
    }
    let mut role_map = roles(s)?;
    let first = if let Some(id) = p.targets.first().copied().flatten() {
        old.audio_tracks.iter().position(|t| t.id.0 == id).ok_or_else(|| error("music destination missing"))?
    } else {
        old.audio_tracks.len()
    };
    let end = p.native_start_ticks + p.native_duration_ticks;
    let picture_links: BTreeSet<_> = old.video_tracks.iter().flat_map(|t| t.items.iter().filter_map(|c| c.link)).collect();
    let mut additions = 0usize;
    for (i, target) in p.targets.iter().enumerate() {
        let position = first.checked_add(i).ok_or_else(|| error("music track position overflow"))?;
        if let Some(track) = old.audio_tracks.get(position) {
            if *target != Some(track.id.0)
                || track.locked
                || track.muted
                || !track.enabled
                || role_map.get(&track.id.0).is_some_and(|r| r != "music")
                || track.items.iter().any(|c| c.link.is_some_and(|l| picture_links.contains(&l)))
                || track.items.iter().any(|c| c.start.0 < end && c.end().0 > p.native_start_ticks)
                || track.transitions.iter().any(|t| t.start.0 < end && (t.start + t.duration).0 > p.native_start_ticks)
            {
                return Err(error("music requires every current contiguous lane free, enabled, unlocked and independent of camera scratch"));
            }
        } else {
            if target.is_some() || position != old.audio_tracks.len() + additions {
                return Err(error("new music destinations must append contiguously after existing lanes"));
            }
            additions += 1;
        }
    }
    if old.audio_tracks.len().checked_add(additions).is_none_or(|n| n > MAX) {
        return Err(error("complete music destinations exceed the native track limit"));
    }
    let before_project = sha(&*before)?;
    checkpoint()?;
    discard(s);
    if additions > 0 {
        s.execute("sequence.addTracks", json!({"video":0,"audio":additions,"submix":0}))?;
        discard(s);
    }
    let destinations =
        s.active_sequence().ok_or(EngineError::NoSequence)?.audio_tracks.iter().skip(first).take(p.targets.len()).map(|t| t.id).collect::<Vec<_>>();
    if destinations.len() != p.targets.len() {
        return Err(error("native human track service did not create the complete destinations"));
    }
    checkpoint()?;
    let raw=s.execute("timeline.place",json!({"item":p.item,"track":destinations.first().map(|t|t.0),"time":p.requested_at_ticks,"sourceIn":0,"duration":g["sourceDurationTicks"],"video":false,"audio":true,"insert":false}))?;
    let ids = raw["clips"]
        .as_array()
        .ok_or_else(|| error("native music placement returned no identities"))?
        .iter()
        .map(|v| v.as_u64().filter(|n| id(*n)).ok_or_else(|| error("invalid created music clip")))
        .collect::<Result<Vec<_>>>()?;
    if ids.len() != destinations.len() || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(error("native human placement omitted or duplicated an audio stream"));
    }
    discard(s);
    checkpoint()?;
    s.execute("clip.audioGain", json!({"clips":ids,"mode":"set","db":p.gain_db}))?;
    discard(s);
    checkpoint()?;
    for track in &destinations {
        role_map.insert(track.0, "music".into());
    }
    if role_map.len() > MAX {
        return Err(error("complete music role history exceeds its bound"));
    }
    let raw_roles = serde_json::to_string(&role_map).map_err(|e| error(e.to_string()))?;
    s.edit("Retain Original Music Intent", |project, _| {
        project.item_mut(sequence).ok_or(EngineError::NoSequence)?.metadata.insert(ROLES.into(), raw_roles);
        let q = project.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
        for (i, id) in ids.iter().enumerate() {
            let destination = destinations.get(i).ok_or_else(|| error("music destination missing"))?;
            let (track, c) = q.find_item_mut(filmcraft_project::ClipId(*id)).ok_or_else(|| error("created music disappeared"))?;
            if track != *destination
                || c.item.0 != p.item
                || c.start.0 != p.native_start_ticks
                || c.duration.0 != p.native_duration_ticks
                || c.source_in.0 != 0
                || c.gain_db != p.gain_db
                || c.speed != 1.0
                || c.reverse
                || c.frame_hold.is_some()
                || u64::try_from(c.audio_stream).ok() != layout[i]["audioStream"].as_u64()
                || serde_json::to_value(&c.source_channels).map_err(|e| error(e.to_string()))? != layout[i]["sourceChannels"]
            {
                return Err(error("native music placement changed original intent, channels or exact human geometry"));
            }
            let identity = c.identity.as_mut().ok_or_else(|| error("music needs tracked native instances"))?;
            identity.annotations.insert("poema.music.plan".into(), p.original_plan_sha256.clone());
            identity.annotations.insert("poema.music.receipt".into(), p.original_receipt_sha256.clone());
            identity.annotations.insert("poema.music.song_role".into(), p.song_role.clone());
            identity.annotations.insert("poema.music.component".into(), i.to_string());
            identity.validate().map_err(error)?;
        }
        Ok(())
    })?;
    discard(s);
    checkpoint()?;
    let after = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut expected = (*before).clone();
    expected.next_id = s.project.next_id;
    expected
        .item_mut(sequence)
        .ok_or(EngineError::NoSequence)?
        .metadata
        .insert(ROLES.into(), serde_json::to_string(&role_map).map_err(|e| error(e.to_string()))?);
    let q = expected.sequence_mut(sequence).ok_or(EngineError::NoSequence)?;
    for (i, dest) in destinations.iter().enumerate() {
        let now = after.audio_tracks.get(first + i).filter(|t| t.id == *dest).ok_or_else(|| error("music result track moved"))?;
        if let Some(old) = q.audio_tracks.get_mut(first + i) {
            let created = now.items.iter().filter(|c| ids.contains(&c.id.0)).cloned().collect::<Vec<_>>();
            if created.len() != 1 {
                return Err(error("music destination has undeclared collateral"));
            }
            old.items.extend(created);
            old.items.sort_by_key(|c| c.start);
        } else {
            if now.items.len() != 1 || !now.transitions.is_empty() {
                return Err(error("new music track has undeclared content"));
            }
            q.audio_tracks.push(now.clone());
        }
    }
    if project_audit::bytes(&expected)? != project_audit::bytes(&s.project)? {
        return Err(error("music placement changed unrelated picture, audio, captions, tracks, sources or project state"));
    }
    let mut expected_state = old_state;
    expected_state.selection = ids.iter().copied().map(filmcraft_project::ClipId).collect();
    crate::move_ops::repair_clip_state(after, &mut expected_state);
    if project_audit::value_bytes(&expected_state)? != project_audit::value_bytes(&s.state)? {
        return Err(error("music placement changed unrelated editor context"));
    }
    let outcomes=ids.iter().map(|id|{let (t,c)=after.find_item(filmcraft_project::ClipId(*id)).ok_or_else(||error("music outcome missing"))?;let identity=c.identity.as_ref().ok_or_else(||error("music outcome lost identity"))?;Ok(json!({"clip":id,"identity":identity.id,"item":c.item.0,"track":t.0,"startTicks":c.start.0,"durationTicks":c.duration.0,"audioStream":c.audio_stream,"sourceChannels":c.source_channels,"gainDb":c.gain_db,"sha256":sha(c)?}))}).collect::<Result<Vec<Value>>>()?;
    let mut human_commands = Vec::new();
    if additions > 0 {
        human_commands.push("sequence.addTracks");
    }
    human_commands.extend(["timeline.place", "clip.audioGain"]);
    Ok(
        json!({"beforeProjectSha256":before_project,"afterProjectSha256":sha(&*s.project)?,"beforeStateSha256":p.before_state_sha256,"afterStateSha256":source_edit::digest(s)?,"beforeSha256":p.before_sha256,"afterSha256":tracks::digest(after)?,"beforeCaptionSha256":p.before_caption_sha256,"afterCaptionSha256":captions::digest(&after.caption_tracks)?,"beforeRolesSha256":p.before_roles_sha256,"afterRolesSha256":sha(&role_map)?,"originalPlanSha256":p.original_plan_sha256,"originalReceiptSha256":p.original_receipt_sha256,"geometry":g,"createdTracks":additions,"targets":destinations.iter().map(|t|t.0).collect::<Vec<_>>(),"outcomes":outcomes,"humanCommands":human_commands,"ripple":false,"pictureChanged":false,"captionPolicy":"preserve-authored"}),
    )
}
