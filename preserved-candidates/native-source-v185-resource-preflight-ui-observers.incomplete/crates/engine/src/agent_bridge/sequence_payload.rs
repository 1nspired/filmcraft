//! Bounded private upload of a complete sequence projection. No paths are read,
//! no cut/history is edited, and a sealed payload grants no publication authority.
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use web_time::Instant;

use super::{check_edit_ticket_basis, parse, sequence_publish, sequences, source_edit, token_ok};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};

pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const CHUNK_BYTES: usize = 128 * 1024;
const ACTIVE: usize = 2;
const RECORDS: usize = 16;
const LIFETIME: Duration = Duration::from_secs(10 * 60);
const BEGIN: &str = "agent.beginSequencePayload";
const APPEND: &str = "agent.appendSequencePayload";
const STATUS: &str = "agent.sequencePayloadStatus";
const DISCARD: &str = "agent.discardSequencePayload";

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Begin {
    token: String,
    payload: String,
    scope: sequences::Scope,
    bytes: usize,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    token: String,
    payload: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Append {
    token: String,
    payload: String,
    offset: usize,
    data: String,
}

pub(super) struct Record {
    request: Begin,
    started: Instant,
    revision: u64,
    defaults: String,
    scaling: String,
    status: &'static str,
    received_bytes: usize,
    bytes: Vec<u8>,
    raw: Option<Arc<Vec<u8>>>,
    projection: Option<Arc<sequence_publish::Projection>>,
    hash: Sha256,
}
#[derive(Clone)]
pub(super) struct Ready {
    pub(super) id: String,
    pub(super) sha256: String,
    pub(super) raw: Arc<Vec<u8>>,
    pub(super) projection: Arc<sequence_publish::Projection>,
    pub(super) scaling: String,
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(BEGIN, message)
}
impl Record {
    fn received(&self) -> &[u8] {
        self.raw.as_ref().map_or(self.bytes.as_slice(), |raw| raw.as_slice())
    }
    fn clear(&mut self, status: &'static str) {
        self.bytes = Vec::new();
        self.raw = None;
        self.projection = None;
        self.status = status;
    }
}
fn sweep(s: &mut Session) {
    for r in &mut s.agent_bridge.sequence_payloads {
        if matches!(r.status, "uploading" | "ready") && r.started.elapsed() >= LIFETIME {
            r.clear("expired");
        }
    }
}
fn current(s: &Session, r: &Record) -> Result<()> {
    check_edit_ticket_basis(s, &r.request.token)?;
    let phase = s.agent_bridge.operations.progress(&r.request.token);
    if !matches!(phase["phase"].as_str(), Some("queued" | "verifying_inputs"))
        || r.started.elapsed() >= LIFETIME
        || source_edit::placement_defaults(s)? != r.defaults
        || !sequences::belongs(&s.project, s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?, &r.request.scope)?
    {
        return Err(error("payload basis expired, cancelled, or its workspace/placement defaults changed"));
    }
    Ok(())
}
fn view(s: &Session, r: &Record) -> Value {
    json!({"kind":"poema-sequence-payload","version":1,"token":r.request.token,"payload":r.request.payload,
        "scope":r.request.scope,"sha256":r.request.sha256,"totalBytes":r.request.bytes,"receivedBytes":r.received_bytes,
        "prefixSha256":format!("{:x}",r.hash.clone().finalize()),"status":r.status,"revision":r.revision,
        "placementDefaultsSha256":r.defaults,"basisCurrent":current(s,r).is_ok(),"writeAuthority":false})
}
fn target(s: &Session, p: &Target) -> Result<usize> {
    if !token_ok(&p.token) || !token_ok(&p.payload) || p.token == p.payload {
        return Err(error("distinct read/payload IDs required"));
    }
    s.agent_bridge
        .sequence_payloads
        .iter()
        .position(|r| r.request.token == p.token && r.request.payload == p.payload)
        .ok_or_else(|| error("payload is unknown/expired; no upload or edit was replayed"))
}
fn begin(s: &mut Session, value: &Value) -> Result<Value> {
    let p: Begin = parse(BEGIN, value)?;
    if !token_ok(&p.token) || !token_ok(&p.payload) || p.token == p.payload || p.bytes == 0 || p.bytes > MAX_BYTES || !valid_hash(&p.sha256) {
        return Err(error("bounded complete payload bytes/hash, scope and distinct IDs required"));
    }
    sweep(s);
    if let Some(r) = s.agent_bridge.sequence_payloads.iter().find(|r| r.request.payload == p.payload) {
        if r.request != p {
            return Err(error("payload ID already belongs to different bytes, scope or read"));
        }
        return Ok(view(s, r));
    }
    check_edit_ticket_basis(s, &p.token)?;
    if !sequences::belongs(&s.project, s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?, &p.scope)?
        || s.agent_bridge.sequence_payloads.iter().filter(|r| matches!(r.status, "uploading" | "ready")).count() >= ACTIVE
    {
        return Err(error("workspace mismatch or complete payload staging slots are busy"));
    }
    while s.agent_bridge.sequence_payloads.len() >= RECORDS {
        let index = s
            .agent_bridge
            .sequence_payloads
            .iter()
            .position(|r| !matches!(r.status, "uploading" | "ready"))
            .ok_or_else(|| error("payload reconciliation records are busy"))?;
        s.agent_bridge.sequence_payloads.remove(index);
    }
    let r = Record {
        request: p,
        started: Instant::now(),
        revision: s.revision,
        defaults: source_edit::placement_defaults(s)?,
        scaling: s.prefs.media.default_media_scaling.clone(),
        status: "uploading",
        received_bytes: 0,
        bytes: Vec::new(),
        raw: None,
        projection: None,
        hash: Sha256::new(),
    };
    current(s, &r)?;
    let result = view(s, &r);
    s.agent_bridge.sequence_payloads.push_back(r);
    Ok(result)
}
fn append(s: &mut Session, value: &Value) -> Result<Value> {
    let p: Append = parse(APPEND, value)?;
    if p.data.is_empty() || p.data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
        return Err(error("payload chunk exceeds its 128 KiB decoded bound"));
    }
    let bytes = filmcraft_project::gtemplate::base64_decode(&p.data).ok_or_else(|| error("payload chunk is not base64"))?;
    if bytes.is_empty() || bytes.len() > CHUNK_BYTES || filmcraft_project::gtemplate::base64_encode(&bytes) != p.data {
        return Err(error("payload chunk is not canonical bounded base64"));
    }
    sweep(s);
    let index = target(s, &Target { token: p.token, payload: p.payload })?;
    let record = s.agent_bridge.sequence_payloads.get(index).ok_or_else(|| error("payload disappeared"))?;
    current(s, record)?;
    if !matches!(record.status, "uploading" | "ready") {
        return Err(error("payload is closed; read status/receipt rather than appending again"));
    }
    let end = p.offset.checked_add(bytes.len()).filter(|n| *n <= record.request.bytes).ok_or_else(|| error("payload chunk offset overflow/overrun"))?;
    if p.offset < record.received().len() {
        if record.received().get(p.offset..end) != Some(bytes.as_slice()) {
            return Err(error("repeated payload chunk contradicts retained bytes"));
        }
        return Ok(view(s, record));
    }
    if p.offset != record.received().len() || record.status != "uploading" {
        return Err(error("payload chunks must be contiguous; no holes or ready-payload mutation"));
    }
    let r = s.agent_bridge.sequence_payloads.get_mut(index).ok_or_else(|| error("payload disappeared"))?;
    r.bytes.extend_from_slice(&bytes);
    r.received_bytes = r.bytes.len();
    r.hash.update(&bytes);
    if r.bytes.len() == r.request.bytes {
        if format!("{:x}", r.hash.clone().finalize()) != r.request.sha256 {
            r.clear("refused");
            return Err(error("complete payload hash differs; nothing was published"));
        }
        let parsed = serde_json::from_slice::<Value>(&r.bytes).map_err(|e| error(e.to_string()));
        let projection = parsed.and_then(|v| sequence_publish::validate_projection(s, &v));
        match projection {
            Ok(projection) => {
                let r = s.agent_bridge.sequence_payloads.get_mut(index).ok_or_else(|| error("payload disappeared"))?;
                if projection.scope != r.request.scope {
                    r.clear("refused");
                    return Err(error("sealed projection belongs to another workspace"));
                }
                r.projection = Some(Arc::new(projection));
                r.raw = Some(Arc::new(std::mem::take(&mut r.bytes)));
                r.status = "ready";
            }
            Err(e) => {
                s.agent_bridge.sequence_payloads.get_mut(index).ok_or_else(|| error("payload disappeared"))?.clear("refused");
                return Err(e);
            }
        }
    }
    let r = s.agent_bridge.sequence_payloads.get(index).ok_or_else(|| error("payload disappeared"))?;
    Ok(view(s, r))
}
fn status(s: &mut Session, value: &Value) -> Result<Value> {
    let p: Target = parse(STATUS, value)?;
    sweep(s);
    let index = target(s, &p)?;
    Ok(view(s, s.agent_bridge.sequence_payloads.get(index).ok_or_else(|| error("payload disappeared"))?))
}
fn discard(s: &mut Session, value: &Value) -> Result<Value> {
    let p: Target = parse(DISCARD, value)?;
    sweep(s);
    let index = target(s, &p)?;
    let r = s.agent_bridge.sequence_payloads.get_mut(index).ok_or_else(|| error("payload disappeared"))?;
    if r.status == "committed" {
        return Err(error("committed payload cannot be discarded; reconcile its native receipt"));
    }
    r.clear("discarded");
    Ok(view(s, s.agent_bridge.sequence_payloads.get(index).ok_or_else(|| error("payload disappeared"))?))
}
pub(super) fn ready(s: &Session, token: &str, payload: &str, sha256: &str) -> Result<Ready> {
    let index = target(s, &Target { token: token.into(), payload: payload.into() })?;
    let r = s.agent_bridge.sequence_payloads.get(index).ok_or_else(|| error("payload disappeared"))?;
    current(s, r)?;
    if r.status != "ready" || r.request.sha256 != sha256 {
        return Err(error("complete immutable matching payload required"));
    }
    Ok(Ready {
        id: payload.into(),
        sha256: sha256.into(),
        raw: r.raw.clone().ok_or_else(|| error("sealed payload bytes missing"))?,
        projection: r.projection.clone().ok_or_else(|| error("sealed projection missing"))?,
        scaling: r.scaling.clone(),
    })
}
pub(super) fn committed(s: &mut Session, payload: &str) {
    if let Some(r) = s.agent_bridge.sequence_payloads.iter_mut().find(|r| r.request.payload == payload) {
        r.clear("committed");
    }
}
// Exact original bytes and captured scaling, saved only in the audited native
// journal. Public Apply still accepts only a small sealed-payload reference.
pub(super) fn restore(s: &mut Session, token: &str, value: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Replay {
        projection_source: String,
        payload_sha256: String,
        media_scaling: String,
    }
    let p: Replay = parse(BEGIN, value)?;
    if p.projection_source.len() > MAX_BYTES
        || hash(p.projection_source.as_bytes()) != p.payload_sha256
        || !matches!(p.media_scaling.as_str(), "none" | "setToFrameSize" | "scaleToFrameSize")
    {
        return Err(error("journal projection bytes/hash/scaling are invalid"));
    }
    let projection = sequence_publish::validate_projection(s, &serde_json::from_str(&p.projection_source).map_err(|e| error(e.to_string()))?)?;
    let payload = format!("replayed-{}", &p.payload_sha256);
    let request = Begin {
        token: token.into(),
        payload: payload.clone(),
        scope: projection.scope.clone(),
        bytes: p.projection_source.len(),
        sha256: p.payload_sha256.clone(),
    };
    begin(s, &serde_json::to_value(request).map_err(|e| error(e.to_string()))?)?;
    let r = s.agent_bridge.sequence_payloads.iter_mut().find(|r| r.request.payload == payload).ok_or_else(|| error("journal payload missing"))?;
    r.hash.update(p.projection_source.as_bytes());
    r.received_bytes = p.projection_source.len();
    r.raw = Some(Arc::new(p.projection_source.into_bytes()));
    r.projection = Some(Arc::new(projection));
    r.scaling = p.media_scaling;
    r.status = "ready";
    Ok(json!({"payload":payload,"payloadSha256":p.payload_sha256}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec(BEGIN, "Begin Private Sequence Payload", r#"{"token":str,"payload":str,"scope":object,"bytes":n,"sha256":str}"#, begin),
        spec(APPEND, "Append Private Sequence Payload", r#"{"token":str,"payload":str,"offset":n,"data":base64}"#, append),
        spec(STATUS, "Read Private Sequence Payload Status", r#"{"token":str,"payload":str}"#, status),
        spec(DISCARD, "Discard Private Sequence Payload", r#"{"token":str,"payload":str}"#, discard),
    ]
}
