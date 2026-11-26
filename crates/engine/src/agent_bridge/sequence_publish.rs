//! One guarded populated sequence, using the same component factory as human
//! placement. The original editorial owner remains outside the native engine.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use filmcraft_project::{BinEntry, Interpretation, ItemId, MediaRef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    BINDING_KEY, Binding, Operation, admission, check_edit_ticket_basis, parse, project_audit,
    sequence_payload::{self, Ready},
    sequences,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
};
pub(super) const COMMAND: &str = "agent.assembleSequence";
const ARCHIVE: &str = "poema.stringout.original.v1";
const OWNER: &str = "poema.stringout.owner.v1";
const SAFE: u64 = 9_007_199_254_740_991;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Owner {
    workspace_id: String,
    operation_id: String,
    receipt_sha256: String,
    plan_sha256: String,
    basis_sha256: String,
    request_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Original {
    item: u64,
    asset_id: String,
    media_id: String,
    path: String,
    generation: admission::Generation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Projection {
    kind: String,
    version: u32,
    pub(super) scope: sequences::Scope,
    owner: Owner,
    originals: Vec<Original>,
    data: Value,
    summary: Value,
    native_edit_requested: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    payload: String,
    payload_sha256: String,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
fn id(n: u64) -> bool {
    (1..=SAFE).contains(&n)
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() })
        && matches!(s.as_bytes().get(14), Some(b'1'..=b'5'))
        && matches!(s.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b' | b'A' | b'B'))
}
pub(super) fn validate_projection(s: &Session, value: &Value) -> Result<Projection> {
    let p: Projection = parse(COMMAND, value)?;
    let current = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if p.kind != "poema-native-stringout-projection"
        || p.version != 1
        || p.native_edit_requested
        || !p.summary.is_object()
        || !sequences::belongs(&s.project, current, &p.scope)?
        || p.originals.is_empty()
        || p.originals.len() > 2000
        || s.project.items.len() > 100_000
        || !uuid(&p.owner.workspace_id)
        || !uuid(&p.owner.operation_id)
        || [&p.owner.receipt_sha256, &p.owner.plan_sha256, &p.owner.basis_sha256, &p.owner.request_sha256].iter().any(|v| !sequence_payload::valid_hash(v))
    {
        return Err(error("complete source projection, original owner identity and current workspace required"));
    }
    crate::sequence_assembly::validate_prepared(s, &p.data)?;
    let raw = p.data.get("metadata").and_then(|v| v.get(ARCHIVE)).and_then(Value::as_str).ok_or_else(|| error("complete original plan archive required"))?;
    let archive: Value = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
    let receipt = &archive["receipt"];
    let plan = &archive["plan"];
    let expected_scope = json!({"project_id":p.scope.project_id,"workspace_id":p.owner.workspace_id,"operation_id":p.owner.operation_id});
    if archive.as_object().is_none_or(|v| v.len() != 2)
        || receipt["kind"] != "poema-source-stringout"
        || receipt["status"] != "complete"
        || receipt["scope"] != expected_scope
        || plan["scope"] != expected_scope
        || receipt["plan"]["sha256"] != p.owner.plan_sha256
        || receipt["basis_sha256"] != p.owner.basis_sha256
        || receipt["request_sha256"] != p.owner.request_sha256
        || receipt["request"] != plan["request"]
        || plan["kind"] != "poema-stringout-plan"
        || plan["version"] != 1
        || p.data["name"] != plan["request"]["name"]
    {
        return Err(error("native projection archive contradicts its original-owner receipt/scope"));
    }
    let mut bindings: BTreeMap<String, Vec<ItemId>> = BTreeMap::new();
    for it in s.project.items.values() {
        if let Some(raw) = it.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == p.scope.project_id && b.role == "original" {
                bindings.entry(b.asset_id).or_default().push(it.id);
            }
        }
    }
    let mut items = BTreeSet::new();
    let mut assets = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let archived_media = plan["media"].as_array().ok_or_else(|| error("original source archive missing"))?;
    if archived_media.len() != p.originals.len() {
        return Err(error("original source inventory is incomplete"));
    }
    for original in &p.originals {
        if !id(original.item)
            || !text(&original.asset_id, 256)
            || !text(&original.media_id, 256)
            || !text(&original.path, 4096)
            || !std::path::Path::new(&original.path).is_absolute()
            || !original.generation.valid()
            || !items.insert(original.item)
            || !assets.insert(&original.asset_id)
            || !paths.insert(&original.path)
            || bindings.get(&original.asset_id).is_none_or(|v| v.as_slice() != [ItemId(original.item)])
        {
            return Err(error("original binding/path/generation inventory is invalid or ambiguous"));
        }
        let it = s.project.item(ItemId(original.item)).ok_or_else(|| error("original item missing"))?;
        let binding: Binding = parse(
            COMMAND,
            &serde_json::from_str::<Value>(it.metadata.get(BINDING_KEY).ok_or_else(|| error("original binding missing"))?).map_err(|e| error(e.to_string()))?,
        )?;
        let media = it.as_media().ok_or_else(|| error("original is not direct media"))?;
        if binding.project_id != p.scope.project_id
            || binding.asset_id != original.asset_id
            || binding.media_id != original.media_id
            || binding.role != "original"
            || media.offline
            || media.proxy.is_some()
            || media.interpret != Interpretation::default()
            || !matches!(&media.media,MediaRef::File{path} if path==&original.path)
        {
            return Err(error("original is foreign, offline, redirected or has unsupported proxy/interpretation overrides"));
        }
        let matches: Vec<_> = archived_media.iter().filter(|m| m["asset_id"] == original.asset_id).collect();
        if matches.len() != 1
            || matches[0]["original_path"] != original.path
            || matches[0]["observation"]["generation"] != serde_json::to_value(&original.generation).map_err(|e| error(e.to_string()))?
        {
            return Err(error("original generation does not match its complete source archive"));
        }
    }
    let components = p.data["components"].as_array().ok_or_else(|| error("native component inventory missing"))?;
    let used: BTreeSet<_> = components.iter().map(|c| c["item"].as_u64().ok_or_else(|| error("component source missing"))).collect::<Result<_>>()?;
    if used != items {
        return Err(error("native components borrow an unlisted source or omit a planned original"));
    }
    Ok(p)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if !super::token_ok(&p.payload) || !sequence_payload::valid_hash(&p.payload_sha256) {
        return Err(error("sealed payload ID and complete byte hash required"));
    }
    Ok(())
}
pub(super) fn live(s: &Session, op: &Operation, token: &str) -> Result<Ready> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    let payload = sequence_payload::ready(s, token, &p.payload, &p.payload_sha256)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("native read expired"))?;
    let check = s.agent_bridge.operations.check().unwrap_or_else(|| Arc::new(super::media_guard::Check::new(web_time::Instant::now())));
    // The payload's expected generations do not qualify new bytes. They must
    // match retained native full-byte witnesses, rechecked by normal publication.
    for original in &payload.projection.originals {
        ticket.media.require_generation(original.item, &original.path, original.generation.bytes, &original.generation.sha256, &check)?;
    }
    Ok(payload)
}
pub(super) fn run(s: &mut Session, payload: &Ready) -> Result<(Value, Value)> {
    let p = &payload.projection;
    let before = s.project.clone();
    let old = project_audit::bytes(&before)?;
    let previous = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, previous, &p.scope)? {
        return Err(error("current sequence is outside the payload workspace"));
    }
    s.prefs.media.default_media_scaling = payload.scaling.clone();
    let result = crate::sequence_assembly::run_prepared(s, &p.data)?;
    let sequence = ItemId(result["sequence"].as_u64().filter(|n| id(*n)).ok_or_else(|| error("native assembly did not create a valid sequence"))?);
    let item = Arc::make_mut(&mut s.project).item_mut(sequence).ok_or_else(|| error("assembled item missing"))?;
    item.metadata.insert(sequences::KEY.into(), serde_json::to_string(&p.scope).map_err(|e| error(e.to_string()))?);
    item.metadata.insert(OWNER.into(), json!({"owner":p.owner,"payloadSha256":payload.sha256}).to_string());
    let new_hash = sequence_payload::hash(&project_audit::value_bytes(item)?);
    let mut stripped = (*s.project).clone();
    stripped.items.remove(&sequence);
    stripped.root.children.retain(|c| !matches!(c,BinEntry::Item(item) if *item==sequence));
    stripped.next_id = before.next_id;
    if project_audit::bytes(&stripped)? != old {
        return Err(error("populated sequence assembly changed existing project content"));
    }
    let after = sequence_payload::hash(&project_audit::bytes(&s.project)?);
    let rows = result["components"].as_array().ok_or_else(|| error("assembly omitted component correspondence"))?;
    let reply = json!({"sequence":sequence.0,"previousSequence":previous.0,"scope":p.scope,"owner":p.owner,"payload":payload.id,"payloadSha256":payload.sha256,
        "beforeProjectSha256":sequence_payload::hash(&old),"afterProjectSha256":after,"sequenceSha256":new_hash,
        "componentMapSha256":sequence_payload::hash(&project_audit::value_bytes(rows)?),"components":rows.len(),"markers":result["markers"],
        "humanCommand":"sequence.assemble","oldProjectPreserved":true});
    let raw = std::str::from_utf8(&payload.raw).map_err(|e| error(e.to_string()))?;
    let journal = json!({"command":COMMAND,"params":{"projectionSource":raw,"payloadSha256":payload.sha256,"mediaScaling":payload.scaling}});
    Ok((reply, journal))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    sequence: u64,
}
fn read(s: &mut Session, value: &Value) -> Result<Value> {
    let p: Read = parse("agent.readSequenceAssembly", value)?;
    check_edit_ticket_basis(s, &p.token)?;
    if !id(p.sequence) || !sequences::belongs(&s.project, s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?, &p.scope)? {
        return Err(error("complete sequence readback needs this current workspace"));
    }
    let item = s.project.item(ItemId(p.sequence));
    if item.is_some() && !sequences::belongs(&s.project, ItemId(p.sequence), &p.scope)? {
        return Err(error("requested assembly belongs to another workspace"));
    }
    let state = match item {
        Some(item) => {
            let q = item.as_sequence().ok_or_else(|| error("assembly read target is not a sequence"))?;
            let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == p.token).ok_or_else(|| error("assembly read ticket expired"))?;
            super::media_scope::require_sequence(&s.project, &ticket.media, item.id)?;
            json!({"sequence":item.id.0,"sha256":sequence_payload::hash(&project_audit::value_bytes(item)?),"name":item.name,"settings":q.settings,
                "picture":q.video_tracks.iter().map(|t|t.items.len()).sum::<usize>(),"audio":q.audio_tracks.iter().map(|t|t.items.len()).sum::<usize>(),"markers":q.markers.len(),
                "owner":item.metadata.get(OWNER).map(|raw|serde_json::from_str::<Value>(raw).map_err(|e|error(e.to_string()))).transpose()?})
        }
        None => Value::Null,
    };
    Ok(json!({"kind":"native_sequence_assembly","status":"read","token":p.token,"revision":s.revision,"scope":p.scope,
        "projectSha256":sequence_payload::hash(&project_audit::bytes(&s.project)?),"sequence":state,"sourceValidation":"not-performed","writeAuthority":false}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "agent.readSequenceAssembly",
        label: "Read Complete Native Sequence Assembly",
        menu: &[],
        shortcut: None,
        params: r#"{"token":str,"scope":object,"sequence":id}"#,
        enabled: always,
        run: read,
        journal: false,
    }]
}
