//! One original-reference handoff through shared human marker services. No
//! detection, source-clock inversion or editing of picture/audio/captions.
use super::{Operation, markers, parse, project_audit, reaction_reference_inputs, sequences};
use crate::{EngineError, Result, Session, commands::bad};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub(super) const COMMAND: &str = "agent.importReactionReferences";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(s: impl Into<String>) -> EngineError {
    bad(COMMAND, s)
}
fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn reference_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 1024 && s.encode_utf16().count() <= 256 && !s.chars().any(char::is_control)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    original_timeline: String,
    before_project_sha256: String,
    before_marker_sha256: String,
    plan_sha256: String,
    inputs_sha256: String,
    cues: Vec<Cue>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cue {
    query: reaction_reference_inputs::Query,
    clip: u64,
    identity: String,
    record_ticks: i64,
    name: String,
    comment: String,
    color: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    kind: String,
    version: u64,
    project_id: String,
    timeline_id: String,
    marker_id: String,
    marker_sha256: String,
}
fn reference(c: &Cue, p: &Request) -> Result<Reference> {
    let r: Reference = serde_json::from_str(&c.comment).map_err(|e| error(e.to_string()))?;
    if r.kind != "poema-original-reaction-marker"
        || r.version != 1
        || r.project_id != p.scope.project_id
        || r.timeline_id != p.original_timeline
        || !reference_key(&r.marker_id)
        || r.marker_sha256 != c.query.key
    {
        return Err(error("cue does not retain its exact original scope and metadata identity"));
    }
    Ok(r)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    if project_audit::value_bytes(&op.params)?.len() > 4 * 1024 * 1024 - 8192 {
        return Err(error("complete reference publication exceeds 4 MiB"));
    }
    let p: Request = parse(COMMAND, &op.params)?;
    if !id(&p.scope.project_id)
        || p.scope.anchor_sequence == 0
        || p.scope.anchor_sequence > SAFE as u64
        || [&p.before_project_sha256, &p.before_marker_sha256, &p.plan_sha256, &p.inputs_sha256].iter().any(|s| !hash(s))
        || p.cues.is_empty()
        || p.cues.len() > 512
    {
        return Err(error("explicit original reference identities and bounded current native basis required"));
    }
    reaction_reference_inputs::validate_queries(&p.original_timeline, &p.cues.iter().map(|c| c.query.clone()).collect::<Vec<_>>())?;
    let mut ids = BTreeSet::new();
    for c in &p.cues {
        if c.clip == 0
            || c.clip > SAFE as u64
            || !filmcraft_project::clip_identity::ClipIdentity::valid_id(&c.identity)
            || !(0..=SAFE).contains(&c.record_ticks)
            || c.name.len() > 1024
            || c.name.contains('\0')
            || c.comment.len() > 65536
            || c.comment.contains('\0')
            || filmcraft_project::Label::from_name(&c.color).is_none()
            || !ids.insert(reference(c, &p)?.marker_id)
        {
            return Err(error("malformed, repeated or unsupported original reference cue"));
        }
    }
    Ok(())
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation) -> Result<()> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &p.scope)?
        || sha(&*s.project)? != p.before_project_sha256
        || markers::digest(&s.active_sequence().ok_or(EngineError::NoSequence)?.markers)? != p.before_marker_sha256
    {
        return Err(error("current project or authored cues changed before reference handoff"));
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation, checkpoint: &dyn Fn() -> Result<()>) -> Result<Value> {
    checkpoint()?;
    validate_live_basis(s, op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let before = s.project.clone();
    let editor = project_audit::value_bytes(&s.state)?;
    let queries = p.cues.iter().map(|c| c.query.clone()).collect::<Vec<_>>();
    let measured = reaction_reference_inputs::measure(s, &p.scope, &p.original_timeline, &queries)?;
    if measured.rows.len() != p.cues.len() {
        return Err(error("native reference correspondence is incomplete"));
    }
    let mut existing = BTreeSet::new();
    for m in &s.active_sequence().ok_or(EngineError::NoSequence)?.markers {
        let Ok(value) = serde_json::from_str::<Value>(&m.comment) else {
            continue;
        };
        if value.get("kind").and_then(Value::as_str) != Some("poema-original-reaction-marker") {
            continue;
        }
        let r: Reference = serde_json::from_value(value).map_err(|e| error(e.to_string()))?;
        if r.version != 1 || !id(&r.project_id) || !id(&r.timeline_id) || !reference_key(&r.marker_id) || !hash(&r.marker_sha256) {
            return Err(error("an existing original marker reference is malformed"));
        }
        existing.insert((r.project_id, r.timeline_id, r.marker_id));
    }
    for (c, row) in p.cues.iter().zip(&measured.rows) {
        let r = reference(c, &p)?;
        if existing.contains(&(r.project_id, r.timeline_id, r.marker_id)) {
            return Err(error("original cue already has a native reference; preserve its human position"));
        }
        let matches = row.get("matches").and_then(Value::as_array).ok_or_else(|| error("missing complete current cue matches"))?;
        let m = matches.first().ok_or_else(|| error("original event is no longer in a current component"))?;
        if row.get("status").and_then(Value::as_str) != Some("mapped")
            || matches.len() != 1
            || m.get("clip").and_then(Value::as_u64) != Some(c.clip)
            || m.get("identity").and_then(Value::as_str) != Some(c.identity.as_str())
            || m.get("recordTicks").and_then(Value::as_i64) != Some(c.record_ticks)
        {
            return Err(error("reference placement differs from the current native source clock or instance"));
        }
    }
    let mut added = Vec::new();
    for c in &p.cues {
        checkpoint()?;
        let basis = markers::digest(&s.active_sequence().ok_or(EngineError::NoSequence)?.markers)?;
        let gesture = Operation {
            command: "agent.marker".into(),
            params: json!({"scope":p.scope,"target":{"kind":"sequence"},"beforeSha256":basis,"edit":{"type":"add","kind":"comment","position":{"ticks":c.record_ticks},"duration":{"ticks":0},"name":c.name,"comment":c.comment,"color":c.color}}),
        };
        let result = markers::run(s, &gesture)?;
        let marker = result
            .get("marker")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= SAFE as u64)
            .ok_or_else(|| error("shared marker gesture lost its native identity"))?;
        added.push(json!({"originalMarker":reference(c,&p)?.marker_id,"marker":marker,"clip":c.clip,"identity":c.identity,"recordTicks":c.record_ticks,"commentSha256":format!("{:x}",Sha256::digest(c.comment.as_bytes()))}));
    }
    checkpoint()?;
    let mut expected = (*before).clone();
    expected.next_id = s.project.next_id;
    expected.sequence_mut(sequence).ok_or(EngineError::NoSequence)?.markers = s.active_sequence().ok_or(EngineError::NoSequence)?.markers.clone();
    if project_audit::bytes(&expected)? != project_audit::bytes(&s.project)? || project_audit::value_bytes(&s.state)? != editor {
        return Err(error("reference handoff changed picture, audio, captions or editor context"));
    }
    let after = sha(&*s.project)?;
    if after == p.before_project_sha256 {
        return Err(error("reference handoff makes no change"));
    }
    Ok(
        json!({"beforeProjectSha256":p.before_project_sha256,"afterProjectSha256":after,"beforeMarkerSha256":p.before_marker_sha256,"afterMarkerSha256":markers::digest(&s.active_sequence().ok_or(EngineError::NoSequence)?.markers)?,
        "planSha256":p.plan_sha256,"inputsSha256":p.inputs_sha256,"sourceClockPolicy":"shared-native-clip-clock","added":added,"humanCommands":{"markers.add":p.cues.len()},"pictureUnchanged":true,"audioUnchanged":true,"captionsUnchanged":true}),
    )
}
