//! Bind an already imported original only after original-owner ingest and fresh
//! native probing agree. No import, relink, proxy remap or legacy cut writer.
use super::{BINDING_KEY, Binding, Operation, media_guard, organization_scope, parse};
use crate::{Result, Session, commands::bad};
use filmcraft_project::{Interpretation, ItemId, MediaRef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Component, Path},
    sync::Arc,
};

pub(super) const COMMAND: &str = "agent.admitSource";
pub(super) const KEY: &str = "poema.admission.v1";
pub(super) const GROUP_LIMIT: usize = 2000;
const SAFE: u64 = 9_007_199_254_740_991;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Generation {
    pub(super) kind: String,
    pub(super) version: u32,
    pub(super) sha256: String,
    pub(super) bytes: u64,
}
impl Generation {
    pub(super) fn valid(&self) -> bool {
        self.kind == "poema-media-generation" && self.version == 1 && hash(&self.sha256) && (1..=SAFE).contains(&self.bytes)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    item: u64,
    path: String,
    binding: Binding,
    generation: Generation,
    owner_receipt_sha256: String,
    scope: organization_scope::Scope,
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Request = parse(COMMAND, &op.params)?;
    if !(1..=SAFE).contains(&p.item)
        || p.path.is_empty()
        || p.path.len() > 4096
        || !Path::new(&p.path).is_absolute()
        || Path::new(&p.path).components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || p.path.chars().any(char::is_control)
        || !hash(&p.owner_receipt_sha256)
        || !hash(&p.generation.sha256)
        || p.generation.kind != "poema-media-generation"
        || p.generation.version != 1
        || !(1..=SAFE).contains(&p.generation.bytes)
        || p.binding.role != "original"
        || !p.scope.valid()
        || p.binding.project_id != p.scope.project_id()
        || [&p.binding.project_id, &p.binding.asset_id, &p.binding.media_id].iter().any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err(error("need exact imported path, original binding, complete owner generation, receipt and workspace scope"));
    }
    Ok(())
}
// A group is only original bindings for one workspace. Each member retains
// its own original-owner receipt and byte proof; no editing command can mix in.
pub(super) fn validate_group(operations: &[Operation], exact: bool) -> Result<()> {
    if !exact || operations.is_empty() || operations.len() > GROUP_LIMIT || operations.iter().any(|op| op.command != COMMAND) {
        return Err(error("need 1–2000 exact original bindings for one workspace"));
    }
    let mut items = std::collections::BTreeSet::new();
    let mut assets = std::collections::BTreeSet::new();
    let mut receipts = std::collections::BTreeSet::new();
    let mut scope = None;
    let mut path_bytes = 0usize;
    for op in operations {
        validate(op)?;
        let p: Request = parse(COMMAND, &op.params)?;
        let current_scope = p.scope.clone();
        path_bytes = path_bytes.checked_add(p.path.len()).ok_or_else(|| error("source group path budget overflow"))?;
        if !items.insert(p.item)
            || !assets.insert(p.binding.asset_id)
            || !receipts.insert(p.owner_receipt_sha256)
            || scope.as_ref().is_some_and(|previous| previous != &current_scope)
            || path_bytes > 2 * 1024 * 1024
        {
            return Err(error("source group needs distinct native items, owner assets/receipts and one bounded workspace scope"));
        }
        scope = Some(current_scope);
    }
    Ok(())
}
pub(super) fn validate_results(operations: &[Operation], results: &[Value]) -> Result<()> {
    if operations.len() != results.len() {
        return Err(error("source group receipt must contain every exact original binding"));
    }
    for (op, result) in operations.iter().zip(results) {
        let p: Request = parse(COMMAND, &op.params)?;
        let expected = json!({"item":p.item,"binding":p.binding,"ownerReceiptSha256":p.owner_receipt_sha256,"generation":p.generation,"sourceAdmission":true});
        if result != &expected {
            return Err(error("source group receipt differs from its original operation"));
        }
    }
    Ok(())
}
pub(super) struct Proof {
    pub(super) item: ItemId,
    witness: Arc<media_guard::AdmissionWitness>,
}
pub(super) struct Input {
    operation: Operation,
    request: Request,
    expected_info: filmcraft_media::MediaInfo,
}
pub(super) struct Prepared {
    operation: Operation,
    info: filmcraft_media::MediaInfo,
    proof: Arc<Proof>,
}
pub(super) fn input(s: &Session, op: &Operation) -> Result<Input> {
    validate(op)?;
    let p: Request = parse(COMMAND, &op.params)?;
    p.scope.require(s)?;
    let id = ItemId(p.item);
    let it = s.project.item(id).ok_or_else(|| error("native imported source is missing"))?;
    let media = it.as_media().ok_or_else(|| error("admission requires imported file media, not a nest or subclip"))?;
    if it.metadata.contains_key(BINDING_KEY)
        || it.metadata.contains_key(KEY)
        || media.offline
        || media.proxy.is_some()
        || media.interpret != Interpretation::default()
        || !matches!(&media.media, MediaRef::File { path } if path == &p.path)
        || (!media.info.has_video() && !media.info.has_audio())
    {
        return Err(error("need an unbound online original at the exact imported path without proxy or interpretation overrides"));
    }
    let expected_info = media.info.clone();
    for other in s.project.items.values().filter(|other| other.id != id) {
        if let Some(raw) = other.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == p.binding.project_id && b.asset_id == p.binding.asset_id && b.role == "original" {
                return Err(error("this owner asset already has another native original binding"));
            }
        }
    }
    Ok(Input { operation: op.clone(), request: p, expected_info })
}
pub(super) fn prepare(input: Input, media: &crate::MediaPool, services: &dyn crate::Services, check: &media_guard::Check) -> Result<Arc<Prepared>> {
    let p = &input.request;
    let witness = media_guard::admit(Path::new(&p.path), p.generation.bytes, &p.generation.sha256, check)?;
    check.files.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    check.check()?;
    let fresh = media.probe_file(&p.path, services).map_err(|e| error(e.to_string()))?;
    check.check()?;
    if fresh.info() != &input.expected_info {
        return Err(error("fresh native stream metadata differs from the imported source; refresh/reimport before admission"));
    }
    media_guard::validate_admission(&witness, check)?;
    check.files.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(Arc::new(Prepared { operation: input.operation, info: input.expected_info, proof: Arc::new(Proof { item: ItemId(p.item), witness }) }))
}
pub(super) fn run(s: &mut Session, op: &Operation, check: &media_guard::Check) -> Result<(Value, Arc<Proof>)> {
    let prepared = prepare(input(s, op)?, &s.media, s.services.as_ref(), check)?;
    run_prepared(s, op, &prepared)
}
pub(super) fn run_prepared(s: &mut Session, op: &Operation, prepared: &Prepared) -> Result<(Value, Arc<Proof>)> {
    let current = input(s, op)?;
    if op.command != prepared.operation.command || op.params != prepared.operation.params || current.expected_info != prepared.info {
        return Err(error("prepared original differs from its exact operation or imported stream declaration"));
    }
    let p = current.request;
    let id = ItemId(p.item);
    // Reuse the existing native binding operation inside private staging. The
    // entire batch still publishes one native history step, including the proof.
    let binding = serde_json::to_value(&p.binding).map_err(|e| error(e.to_string()))?;
    super::bind_source(s, &json!({"item":p.item,"binding":binding}))?;
    let marker = json!({"kind":"poema-native-source-binding","version":1,"ownerReceiptSha256":p.owner_receipt_sha256,"generation":p.generation});
    let raw = serde_json::to_string(&marker).map_err(|e| error(e.to_string()))?;
    Arc::make_mut(&mut s.project).item_mut(id).ok_or_else(|| error("staged source disappeared"))?.metadata.insert(KEY.into(), raw);
    Ok((
        json!({"item":p.item,"binding":binding,"ownerReceiptSha256":p.owner_receipt_sha256,"generation":p.generation,"sourceAdmission":true}),
        prepared.proof.clone(),
    ))
}
pub(super) fn publication(proof: &Proof, check: &media_guard::Check) -> Result<()> {
    media_guard::validate_admission(&proof.witness, check)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    kind: String,
    version: u32,
    owner_receipt_sha256: String,
    generation: Generation,
}

/// Validate the SAME retained original-owner proof shape used by original admission.
/// This reads metadata only; callers must match it to full native byte witnesses.
pub(super) fn current_generation(item: &filmcraft_project::ProjectItem, scope: &crate::project_workspace::ProjectScope) -> Result<Generation> {
    let raw = item.metadata.get(KEY).filter(|s|s.len()<=2048).ok_or_else(||error("source has no bounded original-owner admission"))?;
    let marker: Marker = serde_json::from_str(raw).map_err(|e|error(e.to_string()))?;
    let raw = item.metadata.get(BINDING_KEY).filter(|s|s.len()<=2048).ok_or_else(||error("source has no bounded original binding"))?;
    let binding: Binding = serde_json::from_str(raw).map_err(|e|error(e.to_string()))?;
    let media = item.as_media().ok_or_else(||error("original admission is not media"))?;
    let MediaRef::File{path} = &media.media else {return Err(error("original admission is not file media"));};
    if marker.kind!="poema-native-source-binding" || marker.version!=1 || media.offline || media.proxy.is_some() {
        return Err(error("original admission marker, online state or proxy differs"));
    }
    validate(&Operation{command:COMMAND.into(),params:json!({"item":item.id.0,"path":path,"binding":binding,"generation":marker.generation,"ownerReceiptSha256":marker.owner_receipt_sha256,"scope":scope})})?;
    Ok(marker.generation)
}

// Undo removes a new binding from the ordinary original-file witness plan.
// Guarded Redo must qualify the actual target snapshot before restoring it;
// current-input validation alone cannot see that newly reintroduced source.
pub(super) fn restored_source_bytes(s: &Session, target: &filmcraft_project::Project) -> u64 {
    // Scheduling only, from native history's already retained generation. The
    // complete guard below still validates every marker, path, probe and byte.
    target.items.values().take(100_001).fold(0u64, |bytes, it| {
        let Some(raw) = it.metadata.get(KEY).filter(|raw| raw.len() <= 2048) else { return bytes };
        if s.project.item(it.id).is_some_and(|old| old.metadata.get(KEY) == Some(raw) && old.metadata.get(BINDING_KEY) == it.metadata.get(BINDING_KEY)) {
            return bytes;
        }
        let Ok(marker) = serde_json::from_str::<Marker>(raw) else { return bytes };
        if marker.kind == "poema-native-source-binding" && marker.version == 1 && marker.generation.valid() && hash(&marker.owner_receipt_sha256) {
            bytes.saturating_add(marker.generation.bytes)
        } else {
            bytes
        }
    })
}

pub(super) fn guard_restored(s: &Session, target: &filmcraft_project::Project) -> Result<Vec<ItemId>> {
    if target.items.len() > 100_000 {
        return Err(error("restored source inventory exceeds its complete verification bound"));
    }
    let check = s.agent_bridge.operations.check().unwrap_or_else(|| Arc::new(media_guard::Check::new(web_time::Instant::now())));
    let mut restored = Vec::new();
    let mut path_bytes = 0usize;
    for it in target.items.values() {
        check.check()?;
        let Some(raw) = it.metadata.get(KEY) else { continue };
        if s.project.item(it.id).is_some_and(|old| {
            old.metadata.get(KEY) == Some(raw) && old.metadata.get(BINDING_KEY) == it.metadata.get(BINDING_KEY) && old.as_media() == it.as_media()
        }) {
            continue;
        }
        let binding_raw = it.metadata.get(BINDING_KEY).ok_or_else(|| error("restored admission has no source binding"))?;
        if raw.len() > 2048 || binding_raw.len() > 2048 {
            return Err(error("restored source proof exceeds its bounded metadata contract"));
        }
        let marker: Marker = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
        let binding: Binding = serde_json::from_str(binding_raw).map_err(|e| error(e.to_string()))?;
        let media = it.as_media().ok_or_else(|| error("restored admission is not imported media"))?;
        let MediaRef::File { path } = &media.media else {
            return Err(error("restored admission is not original file media"));
        };
        path_bytes = path_bytes.checked_add(path.len()).ok_or_else(|| error("restored path budget overflow"))?;
        if marker.kind != "poema-native-source-binding"
            || marker.version != 1
            || !hash(&marker.owner_receipt_sha256)
            || marker.generation.kind != "poema-media-generation"
            || marker.generation.version != 1
            || !hash(&marker.generation.sha256)
            || !(1..=SAFE).contains(&marker.generation.bytes)
            || binding.role != "original"
            || media.offline
            || [&binding.project_id, &binding.asset_id, &binding.media_id].iter().any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            || media.proxy.is_some()
            || media.interpret != Interpretation::default()
            || path.len() > 4096
            || path_bytes > 2 * 1024 * 1024
            || !Path::new(path).is_absolute()
            || path.chars().any(char::is_control)
            || restored.len() >= 2048
        {
            return Err(error("restored admission proof or complete source bounds are invalid"));
        }
        let witness = media_guard::admit(Path::new(path), marker.generation.bytes, &marker.generation.sha256, &check)?;
        let fresh = s.media.probe_file(path, s.services.as_ref()).map_err(|e| error(e.to_string()))?;
        if fresh.info() != &media.info {
            return Err(error("restored admission stream metadata differs from its original imported bytes"));
        }
        restored.push((it.id, witness));
    }
    for (_, witness) in &restored {
        media_guard::validate_admission(witness, &check)?;
    }
    Ok(restored.into_iter().map(|(id, _)| id).collect())
}
