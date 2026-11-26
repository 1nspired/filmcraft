//! Geometry-only source-event lookup on explicitly requested current picture
//! tracks. Original component/story annotations are not required or fabricated.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{MediaRef, clip_identity::ClipIdentity};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BINDING_KEY, Binding, parse, picture_inputs, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
const COMMAND: &str = "agent.sourceEventInputs";
const SAFE: u64 = 9_007_199_254_740_991;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: sequences::Scope,
    tracks: Vec<u64>,
    queries: Vec<Query>,
    #[serde(default)]
    projection: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    key: String,
    asset_id: String,
    source_ticks: i64,
}
fn bounded_id(id: u64) -> bool {
    (1..=SAFE).contains(&id)
}
fn asset_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    if project_audit::value_bytes(value)?.len() > 2 * 1024 * 1024 {
        return Err(error("complete source-event request exceeds 2 MiB"));
    }
    let r: Request = parse(COMMAND, value)?;
    let mut keys = BTreeSet::new();
    if r.tracks.is_empty()
        || r.tracks.len() > 128
        || r.tracks.iter().any(|id| !bounded_id(*id))
        || r.tracks.iter().collect::<BTreeSet<_>>().len() != r.tracks.len()
        || r.queries.is_empty()
        || r.queries.len() > 512
        || r.queries.iter().any(|q| {
            q.key.len() != 64
                || !q.key.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || !keys.insert(&q.key)
                || !asset_id(&q.asset_id)
                || !(0..=SAFE as i64).contains(&q.source_ticks)
        })
    {
        return Err(error("need 1–128 explicit unique picture tracks and 1–512 unique bounded source-event queries"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !bounded_id(sequence.0) || !bounded_id(r.scope.anchor_sequence) || !sequences::belongs(&s.project, sequence, &r.scope)? {
        return Err(error("source-event sequence is outside the exact workspace scope"));
    }
    let q = s.project.sequence(sequence).ok_or(EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    if s.project.items.len() > 100_000
        || q.video_tracks.len() > 128
        || q.video_tracks.iter().map(|t| t.id).collect::<BTreeSet<_>>().len() != q.video_tracks.len()
    {
        return Err(error("complete source-event picture inventory exceeds its bound or repeats track IDs"));
    }
    let tracks = r
        .tracks
        .iter()
        .map(|id| q.video_tracks.iter().find(|t| t.id.0 == *id).ok_or_else(|| error("requested current picture track is absent")))
        .collect::<Result<Vec<_>>>()?;
    let mut clip_ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut count = 0usize;
    for t in &tracks {
        count = count.checked_add(t.items.len()).filter(|n| *n <= 4096).ok_or_else(|| error("complete requested picture geometry exceeds 4096 instances"))?;
        for c in &t.items {
            let identity = c.identity.as_ref().ok_or_else(|| error("requested picture track contains an untracked instance"))?;
            if !bounded_id(c.id.0) || !clip_ids.insert(c.id) || !ClipIdentity::valid_id(&identity.id) || !identities.insert(&identity.id) {
                return Err(error("requested picture tracks contain invalid or repeated current instances"));
            }
        }
    }
    let mut assets = BTreeMap::new();
    for (item, source) in &s.project.items {
        if let Some(raw) = source.metadata.get(BINDING_KEY) {
            let binding: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if binding.project_id == r.scope.project_id && binding.role == "original" && assets.insert(binding.asset_id, *item).is_some() {
                return Err(error("ambiguous original source binding"));
            }
        }
    }
    let mut source_ids = BTreeSet::new();
    let mut checked_clocks = BTreeSet::new();
    let mut rows = Vec::new();
    let mut budget = 4096usize;
    for query in &r.queries {
        let item = assets.get(&query.asset_id).copied().ok_or_else(|| error("event original is not unambiguously bound"))?;
        let media = s.project.item(item).and_then(|i| i.as_media()).ok_or_else(|| error("event original is not direct media"))?;
        if !bounded_id(item.0)
            || media.offline
            || !media.info.has_video()
            || !(1..=SAFE as i64).contains(&media.info.duration.0)
            || query.source_ticks >= media.info.duration.0
            || !matches!(&media.media, MediaRef::File { path } if std::path::Path::new(path).is_absolute())
        {
            return Err(error("source event needs online File picture within its actual duration"));
        }
        source_ids.insert(item);
        let mut held = false;
        let mut mapped = 0usize;
        let mut instances = Vec::new();
        for track in &tracks {
            for c in &track.items {
                if c.item != item {
                    continue;
                }
                budget = budget.checked_sub(1).ok_or_else(|| error("complete source-event correspondence exceeds 4096 instances; nothing truncated"))?;
                if checked_clocks.insert(c.id) {
                    picture_inputs::clock(c, media.info.duration.0).map_err(|e| error(e.to_string()))?;
                }
                let (status, at) = if c.frame_hold.is_some() {
                    held = true;
                    ("unsupported_clock", None)
                } else if let Some(at) = picture_inputs::event_at(c, query.source_ticks, media.info.duration.0) {
                    mapped = mapped.checked_add(1).ok_or_else(|| error("source-event count overflow"))?;
                    ("mapped", Some(at))
                } else {
                    ("outside_current_source", None)
                };
                let identity = c.identity.as_ref().ok_or_else(|| error("source-event identity disappeared"))?;
                instances.push(json!({"clip":c.id.0,"identity":identity.id,"item":item.0,"track":track.id.0,"status":status,
                    "recordTicks":at,"observedSourceTicks":at.map(|t| c.source_time_at(Tick(t)).0),"enabled":c.enabled,"trackEnabled":track.enabled}));
            }
        }
        let status = if held {
            "unsupported_clock"
        } else if mapped > 1 {
            "ambiguous"
        } else if mapped == 1 {
            "mapped"
        } else if !instances.is_empty() {
            "outside_current_source"
        } else {
            "source_absent"
        };
        rows.push(json!({"key":query.key,"assetId":query.asset_id,"sourceTicks":query.source_ticks,"status":status,"instances":instances}));
    }
    let ticket = super::validate_read_basis(s, &r.token)?;
    if ticket.media.is_scoped() {
        ticket.media.require(&super::media_scope::dependencies(&s.project, source_ids.iter().copied())?)?;
    }
    let result = json!({"kind":"native_source_event_inputs","version":1,"status":"read","token":r.token,"revision":s.revision,"sequence":sequence.0,
        "scope":r.scope,"tracks":r.tracks,"requestSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(&(&r.scope.project_id,r.scope.anchor_sequence,&r.tracks,r.queries.iter().map(|q|(&q.key,&q.asset_id,q.source_ticks)).collect::<Vec<_>>()))?)),
        "projectSha256":format!("{:x}",Sha256::digest(project_audit::bytes(&s.project)?)),"rows":rows,"requiresSourceValidation":r.projection,
        "sourceClockPolicy":"shared-native-clip-clock","writeAuthority":false,"semanticVerification":false});
    if project_audit::value_bytes(&result)?.len() > 2 * 1024 * 1024 {
        return Err(error("complete source-event result exceeds 2 MiB; nothing truncated"));
    }
    Ok(result)
}
