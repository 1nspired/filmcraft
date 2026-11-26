//! Source-event coordinates for original reaction-reference handoff. This is a
//! read of current tracked instances through the existing native clip clock,
//! never a detector result, marker edit, seek or history operation.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ItemId, MediaRef};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BINDING_KEY, Binding, parse, picture_inputs, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
const COMMAND: &str = "agent.reactionReferenceInputs";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn reference_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && value.encode_utf16().count() <= 256 && !value.chars().any(char::is_control)
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(value)?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: sequences::Scope,
    original_timeline: String,
    queries: Vec<Query>,
    #[serde(default)]
    projection: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Query {
    pub(super) key: String,
    pub(super) asset_id: String,
    pub(super) component_ids: Vec<String>,
    pub(super) source_ticks: i64,
}
pub(super) struct Measurement {
    pub(super) rows: Vec<Value>,
    pub(super) source_ids: BTreeSet<ItemId>,
}
pub(super) fn validate_queries(original_timeline: &str, queries: &[Query]) -> Result<()> {
    let mut keys = BTreeSet::new();
    let mut components = 0usize;
    if !id(original_timeline) || queries.is_empty() || queries.len() > 512 {
        return Err(error("need one original timeline and 1–512 complete event queries"));
    }
    for q in queries {
        components = components.checked_add(q.component_ids.len()).ok_or_else(|| error("component query overflow"))?;
        if !hash(&q.key)
            || !keys.insert(&q.key)
            || !id(&q.asset_id)
            || !(0..=SAFE).contains(&q.source_ticks)
            || q.component_ids.is_empty()
            || components > 8192
            || q.component_ids.iter().any(|c| !reference_key(c))
            || q.component_ids.iter().collect::<BTreeSet<_>>().len() != q.component_ids.len()
        {
            return Err(error("malformed, repeated or oversized original source-event query"));
        }
    }
    Ok(())
}
pub(super) fn measure(s: &Session, scope: &sequences::Scope, original_timeline: &str, queries: &[Query]) -> Result<Measurement> {
    validate_queries(original_timeline, queries)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if sequence.0 == 0 || sequence.0 > SAFE as u64 || scope.anchor_sequence == 0 || scope.anchor_sequence > SAFE as u64 {
        return Err(error("source-event sequence exceeds exact transport"));
    }
    if !sequences::belongs(&s.project, sequence, scope)? {
        return Err(error("source-event sequence is outside this workspace"));
    }
    let q = s.project.sequence(sequence).ok_or(EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    if s.project.items.len() > 100_000
        || q.video_tracks.len() > 128
        || q.video_tracks.iter().try_fold(0usize, |n, t| n.checked_add(t.items.len())).is_none_or(|n| n > 4096)
    {
        return Err(error("complete current source-event geometry exceeds its bound"));
    }
    let mut clip_ids = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut tracks = BTreeSet::new();
    for t in &q.video_tracks {
        if t.id.0 == 0 || t.id.0 > SAFE as u64 || !tracks.insert(t.id) {
            return Err(error("invalid or repeated current track identity"));
        }
        for c in &t.items {
            if c.id.0 == 0 || c.id.0 > SAFE as u64 || !clip_ids.insert(c.id) {
                return Err(error("invalid or repeated current clip identity"));
            }
            if let Some(i) = &c.identity
                && (!filmcraft_project::clip_identity::ClipIdentity::valid_id(&i.id) || !identities.insert(&i.id))
            {
                return Err(error("invalid or repeated tracked current instance"));
            }
        }
    }
    let mut assets = BTreeMap::new();
    for (item, source) in &s.project.items {
        if let Some(raw) = source.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == scope.project_id && b.role == "original" && assets.insert(b.asset_id, *item).is_some() {
                return Err(error("ambiguous original source binding"));
            }
        }
    }
    let mut source_ids = BTreeSet::new();
    let mut rows = Vec::new();
    let mut match_budget = 4096usize;
    let mut checked_clocks = BTreeSet::new();
    for query in queries {
        let item = assets.get(&query.asset_id).copied().ok_or_else(|| error("event original is not unambiguously bound"))?;
        if item.0 == 0 || item.0 > SAFE as u64 {
            return Err(error("source-event item exceeds exact transport"));
        }
        let media = s.project.item(item).and_then(|i| i.as_media()).ok_or_else(|| error("event original is not direct media"))?;
        if media.offline
            || !media.info.has_video()
            || !(1..=SAFE).contains(&media.info.duration.0)
            || query.source_ticks >= media.info.duration.0
            || !matches!(&media.media,MediaRef::File{path}if std::path::Path::new(path).is_absolute())
        {
            return Err(error("source event needs current online File picture within its actual duration"));
        }
        source_ids.insert(item);
        let mut matches = Vec::new();
        let mut found = false;
        let mut held = false;
        for track in &q.video_tracks {
            for c in &track.items {
                let Some(identity) = &c.identity else {
                    continue;
                };
                let Some(component) = identity.annotations.get("poema_component") else {
                    continue;
                };
                if !query.component_ids.contains(component) {
                    continue;
                }
                if identity.annotations.get("poema_project").map(String::as_str) != Some(scope.project_id.as_str())
                    || identity.annotations.get("poema_timeline").map(String::as_str) != Some(original_timeline)
                    || c.item != item
                {
                    return Err(error("current component differs from its original project, timeline or source"));
                }
                found = true;
                if checked_clocks.insert(c.id) {
                    picture_inputs::clock(c, media.info.duration.0).map_err(|e| error(e.to_string()))?;
                }
                if c.frame_hold.is_some() {
                    held = true;
                    continue;
                }
                let Some(at) = picture_inputs::event_at(c, query.source_ticks, media.info.duration.0) else {
                    continue;
                };
                match_budget =
                    match_budget.checked_sub(1).ok_or_else(|| error("complete event correspondence exceeds 4096 matches; no instances were truncated"))?;
                matches.push(json!({"clip":c.id.0,"identity":identity.id,"componentId":component,"item":item.0,"track":track.id.0,
                    "recordTicks":at,"observedSourceTicks":c.source_time_at(Tick(at)).0,"enabled":c.enabled,"trackEnabled":track.enabled}));
            }
        }
        let status = if held {
            "unsupported_clock"
        } else if matches.len() > 1 {
            "ambiguous"
        } else if matches.len() == 1 {
            "mapped"
        } else if found {
            "outside_current_source"
        } else {
            "component_absent"
        };
        rows.push(json!({"key":query.key,"assetId":query.asset_id,"sourceTicks":query.source_ticks,"status":status,"matches":matches}));
    }
    Ok(Measurement { rows, source_ids })
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    if project_audit::value_bytes(value)?.len() > 2 * 1024 * 1024 {
        return Err(error("complete source-event query exceeds 2 MiB"));
    }
    let r: Request = parse(COMMAND, value)?;
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let Measurement { rows, source_ids } = measure(s, &r.scope, &r.original_timeline, &r.queries)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let ticket = super::validate_read_basis(s, &r.token)?;
    if ticket.media.is_scoped() {
        ticket.media.require(&super::media_scope::dependencies(&s.project, source_ids.iter().copied())?)?;
    }
    let result = json!({"kind":"native_reaction_reference_inputs","version":1,"status":"read","token":r.token,"revision":s.revision,"sequence":sequence.0,
        "scope":r.scope,"originalTimeline":r.original_timeline,"requestSha256":digest(&(&r.scope,&r.original_timeline,&r.queries))?,
        "projectSha256":format!("{:x}",Sha256::digest(project_audit::bytes(&s.project)?)),"rows":rows,"requiresSourceValidation":r.projection,
        "sourceClockPolicy":"shared-native-clip-clock","writeAuthority":false,"semanticVerification":false});
    if project_audit::value_bytes(&result)?.len() > 2 * 1024 * 1024 {
        return Err(error("complete source-event correspondence exceeds 2 MiB; no rows were truncated"));
    }
    Ok(result)
}
